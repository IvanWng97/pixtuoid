use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
    // Held (never unlocked) for the daemon's lifetime: the kernel releases
    // the advisory lock when the process dies, however abruptly, so the lock
    // — not the socket file, which nothing unlinks on exit/crash — is what
    // the next bind's liveness arbitration reads.
    _lock: std::fs::File,
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        // Liveness arbitration is an EXCLUSIVE advisory lock on a sibling
        // `<sock>.lock`, NOT connect() errnos: a backlog-saturated LIVE
        // daemon yields ECONNREFUSED on macOS (the kernel behavior the shim's
        // stalled-listener test documents) and EAGAIN on Linux, so an
        // errno-guessing probe can unlink a live daemon's socket — leaving it
        // accepting on an anonymous inode forever while every hook-borne
        // signal silently routes here. The lock file is NEVER unlinked
        // (unlock-then-unlink lets a waiter on the old inode and a newcomer
        // on a fresh one both "hold" it — same rule as install/io.rs's
        // ConfigLock) and is derived from the FINAL socket path so both bind
        // branches (temp-rename and the sun_path>100 fallback below — a
        // regular file has no sun_path cap) arbitrate on the same file.
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        ));
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            // O_NOFOLLOW: a symlink planted at `<sock>.lock` (the parent dir
            // may be a shared /tmp) must fail the open, not make the daemon
            // flock — and hold for its lifetime — an arbitrary file.
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .with_context(|| format!("opening hook socket lock at {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A live owner holds the lock. Typed so the CC source can
                // degrade to transcript-only instead of dying wholesale. This
                // also closes the old simultaneous-start rename TOCTOU: of
                // two racing first starts exactly one acquires the lock; the
                // loser degrades instead of leaving an anonymous listener.
                return Err(anyhow::Error::new(super::SocketBusy {
                    path: path.to_path_buf(),
                }));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e)
                    .with_context(|| format!("locking hook socket at {}", lock_path.display()));
            }
        }
        if path.exists() {
            // Lock acquired ⇒ any previous lock-holding owner is dead ⇒ the
            // socket file is residue. Belt-and-braces probe before
            // reclaiming: a connect that SUCCEEDS — or backlogs (WouldBlock:
            // a full accept queue only happens on a live listener) — proves a
            // LIVE owner that predates the lock protocol (an older pixtuoid
            // mid-upgrade, or an arbitrary squatter); defer to it rather than
            // steal. Any OTHER connect error is NOT evidence of life — the
            // lock already arbitrated — so reclaim. Honest residual: a
            // lock-LESS live owner under a saturated backlog yields
            // ECONNREFUSED on macOS (not WouldBlock), so this probe still
            // steals from it — accepted, because the window only exists while
            // pre-lock daemons run (mixed-version upgrade) and ages out once
            // every daemon holds the lock.
            let alive = match tokio::net::UnixStream::connect(path).await {
                // Close immediately — the probe counts against the live
                // daemon's MAX_CONCURRENT_CONNS (its CONN_TIMEOUT bounds it
                // regardless).
                Ok(_stream) => true,
                Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
            };
            if alive {
                return Err(anyhow::Error::new(super::SocketBusy {
                    path: path.to_path_buf(),
                }));
            }
            let _ = tokio::fs::remove_file(path).await;
        }
        // Bind at a temp name, chmod to owner-only, then atomically rename
        // onto the final path (a rename doesn't disturb the listening inode).
        // The shim only ever connects to the FINAL path, so the socket is
        // never reachable there with looser-than-0600 modes — without
        // touching the process-global umask, which raced every other tokio
        // worker's concurrent file creation (e.g. a JsonlWatcher's
        // create_dir_all) for the duration of the bind.
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
            return Ok(Self {
                listener,
                _lock: lock,
            });
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
        Ok(Self {
            listener,
            _lock: lock,
        })
    }

    pub(super) async fn run(
        self,
        tx: TaggedSender,
        pid_watch: Option<super::HookPidWatch>,
    ) -> Result<()> {
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
                    let pid_watch = pid_watch.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ =
                            tokio::time::timeout(CONN_TIMEOUT, handle_conn(stream, tx, pid_watch))
                                .await;
                    });
                }
                Err(e) => {
                    warn!("hook socket accept error: {e}");
                }
            }
        }
    }
}
