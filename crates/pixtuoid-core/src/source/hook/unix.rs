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
            let _ = tokio::fs::remove_file(path).await;
        }
        // Set umask to 0077 before bind so the socket is created 0700
        // from the start, closing the TOCTOU window between bind and chmod.
        let _restore_umask = {
            let old = unsafe { libc::umask(0o077) };
            struct RestoreUmask(libc::mode_t);
            impl Drop for RestoreUmask {
                fn drop(&mut self) {
                    unsafe {
                        libc::umask(self.0);
                    }
                }
            }
            RestoreUmask(old)
        };
        let listener = UnixListener::bind(path)
            .with_context(|| format!("binding hook socket at {}", path.display()))?;
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
