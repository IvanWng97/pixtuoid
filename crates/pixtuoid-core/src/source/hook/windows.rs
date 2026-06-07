use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::Semaphore;
use tracing::warn;

use crate::source::TaggedSender;

use super::{handle_conn, CONN_TIMEOUT, MAX_CONCURRENT_CONNS};

/// In-buffer must cover the shim's 1MiB stdin cap (pixtuoid-hook main.rs
/// `take(1 << 20)`) so one payload always fits the pipe quota and the shim's
/// sync write can't stall behind a momentarily busy daemon task.
const IN_BUFFER_SIZE: u32 = 1 << 20;

pub(super) struct Listener {
    server: NamedPipeServer,
    name: String,
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let name = path.to_string_lossy().into_owned();
        // first_pipe_instance: if another process already owns this name
        // (squatting), creation fails ACCESS_DENIED — bail loudly rather
        // than silently queueing behind an impostor. reject_remote_clients
        // is the tokio default; pinned here explicitly. The server stays
        // DUPLEX (tokio default) — the shim's client opens read+write, so an
        // inbound-only pipe would reject it with ACCESS_DENIED (silent event
        // drop). Owner-only SDDL hardening lands in PR 3 (needs the Windows
        // runner) — the default DACL already denies cross-user WRITES.
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .pipe_mode(PipeMode::Byte)
            .in_buffer_size(IN_BUFFER_SIZE)
            .create(&name)
            .with_context(|| format!("creating hook pipe at {name}"))?;
        Ok(Self { server, name })
    }

    pub(super) async fn run(mut self, tx: TaggedSender) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook pipe semaphore closed unexpectedly");
                }
            };
            if let Err(e) = self.server.connect().await {
                // A failed instance isn't guaranteed reusable (tokio's own
                // accept-loop pattern propagates connect errors for this
                // reason) — recreate it; if THAT fails the error converges
                // with the recreate-bail below. Unix accept errors leave the
                // listener fd valid, hence its plain warn+continue.
                warn!("hook pipe connect error: {e}; recreating instance");
                self.server = ServerOptions::new()
                    .reject_remote_clients(true)
                    .pipe_mode(PipeMode::Byte)
                    .in_buffer_size(IN_BUFFER_SIZE)
                    .create(&self.name)
                    .with_context(|| {
                        format!("re-creating hook pipe after connect error at {}", self.name)
                    })?;
                continue;
            }
            // Create the NEXT instance BEFORE handing this one off —
            // tokio's documented pattern; in the gap between handoff and
            // re-create, clients would get ERROR_PIPE_BUSY or NotFound
            // depending on timing.
            let next = ServerOptions::new()
                .reject_remote_clients(true)
                .pipe_mode(PipeMode::Byte)
                .in_buffer_size(IN_BUFFER_SIZE)
                .create(&self.name)
                .with_context(|| format!("re-creating hook pipe at {}", self.name))?;
            let conn = std::mem::replace(&mut self.server, next);
            let tx = tx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tokio::time::timeout(CONN_TIMEOUT, handle_conn(conn, tx)).await;
            });
        }
    }
}
