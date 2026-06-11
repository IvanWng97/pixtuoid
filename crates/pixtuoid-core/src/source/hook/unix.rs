use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::UnixListener;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::source::TaggedSender;

use super::{handle_conn, CONN_TIMEOUT, MAX_CONCURRENT_CONNS};

pub(super) struct Listener {
    listener: UnixListener,
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        if path.exists() {
            // An existing socket file is NOT proof of a live daemon (nothing
            // unlinks it on exit/crash) — but it might be one. Probe before
            // reclaiming: unconditionally unlinking a LIVE daemon's socket
            // would leave it accepting on an anonymous inode forever while
            // every hook-borne signal silently routes here. Mirrors the
            // Windows arm's loud bail (`first_pipe_instance` → ACCESS_DENIED).
            match tokio::net::UnixStream::connect(path).await {
                Ok(stream) => {
                    // Close immediately — the probe counts against the live
                    // daemon's MAX_CONCURRENT_CONNS (its CONN_TIMEOUT bounds
                    // it regardless). Typed so the CC source can degrade to
                    // transcript-only instead of dying wholesale.
                    drop(stream);
                    return Err(anyhow::Error::new(super::SocketBusy {
                        path: path.to_path_buf(),
                    }));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    // Genuinely stale (a crashed daemon's residue): reclaim.
                    let _ = tokio::fs::remove_file(path).await;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("probing existing hook socket at {}", path.display())
                    });
                }
            }
        }
        // Bind at a temp name, chmod to owner-only, then atomically rename
        // onto the final path (a rename doesn't disturb the listening inode).
        // The shim only ever connects to the FINAL path, so the socket is
        // never reachable there with looser-than-0600 modes — without
        // touching the process-global umask, which raced every other tokio
        // worker's concurrent file creation (e.g. a JsonlWatcher's
        // create_dir_all) for the duration of the bind.
        //
        // Accepted TOCTOU: two SIMULTANEOUS first starts can both pass the
        // probe and race the rename; the last rename wins the name and the
        // loser keeps an anonymous listener until its next restart. Same
        // one-winner-loudness class as Windows' first_pipe_instance bail,
        // not worth a lockfile.
        let tmp = path.with_file_name(format!(
            "{}.{}.tmp",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            std::process::id()
        ));
        // sun_path caps at 104 bytes (macOS; 108 Linux). A custom
        // PIXTUOID_SOCKET whose FINAL path fits but whose `.<pid>.tmp` twin
        // doesn't must not fail the bind — fall back to a direct bind +
        // chmod at the final name, re-accepting the micro-TOCTOU (pre-chmod
        // window) the temp-rename dance exists to avoid.
        if tmp.as_os_str().len() > 100 {
            let listener = UnixListener::bind(path)
                .with_context(|| format!("binding hook socket at {}", path.display()))?;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("restricting hook socket mode at {}", path.display()))?;
            return Ok(Self { listener });
        }
        // A leftover temp can only be ours-by-name from a crashed prior run
        // that had this very pid — never a live socket.
        let _ = tokio::fs::remove_file(&tmp).await;
        let listener = UnixListener::bind(&tmp)
            .with_context(|| format!("binding hook socket at {}", tmp.display()))?;
        if let Err(e) =
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await
        {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e)
                .with_context(|| format!("restricting hook socket mode at {}", tmp.display()));
        }
        if let Err(e) = tokio::fs::rename(&tmp, path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e).with_context(|| {
                format!(
                    "moving hook socket into place at {} (from {})",
                    path.display(),
                    tmp.display()
                )
            });
        }
        Ok(Self { listener })
    }

    pub(super) async fn run(self, tx: TaggedSender) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook socket semaphore closed unexpectedly");
                }
            };
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = tokio::time::timeout(CONN_TIMEOUT, handle_conn(stream, tx)).await;
                    });
                }
                Err(e) => {
                    warn!("hook socket accept error: {e}");
                }
            }
        }
    }
}
