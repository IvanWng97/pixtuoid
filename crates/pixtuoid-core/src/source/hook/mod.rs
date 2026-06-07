use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tracing::{debug, warn};

use crate::source::decoder::decode_hook_payload;
use crate::source::{TaggedSender, Transport};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) const MAX_CONCURRENT_CONNS: usize = 128;
pub(crate) const CONN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

pub struct HookSocketListener {
    inner: imp::Listener,
    path: PathBuf,
}

impl HookSocketListener {
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let inner = imp::Listener::bind(&path).await?;
        Ok(Self { inner, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        self.inner.run(tx).await
    }
}

/// Per-connection byte ceiling: 2× the shim's 1MiB stdin cap. lines() buffers
/// until a newline, so without this an adversarial client could grow the
/// buffer unboundedly for the whole CONN_TIMEOUT window × 128 slots (the Unix
/// socket is 0700, but the Windows pipe DACL only hardens in PR 3).
const MAX_CONN_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) async fn handle_conn(stream: impl AsyncRead + Unpin, tx: TaggedSender) {
    let reader = BufReader::new(stream.take(MAX_CONN_BYTES));
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("malformed hook line skipped: {e}");
                        continue;
                    }
                };
                match decode_hook_payload(v) {
                    Ok(ev) => {
                        debug!("hook event: {ev:?}");
                        if tx.send((Transport::Hook, ev)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => warn!("hook decode error: {e}"),
                }
            }
            Ok(None) => return,
            Err(e) => {
                warn!("hook conn read error: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::AgentEvent;
    use tokio::io::AsyncWriteExt;

    // Platform-neutral framing test: handle_conn is generic over AsyncRead,
    // so the SHARED decode path is pinned without any socket/pipe — this is
    // the one test that runs identically on macOS, Linux, and Windows.
    #[tokio::test]
    async fn handle_conn_decodes_one_line_via_duplex() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);

        let task = tokio::spawn(handle_conn(server, tx));
        client
            .write_all(
                b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s1\",\
                  \"transcript_path\":\"/Users/me/.claude/projects/x/s1.jsonl\"}\n",
            )
            .await
            .unwrap();
        drop(client); // EOF ends the conn loop

        let (transport, ev) = rx.recv().await.expect("one decoded event");
        assert_eq!(transport, Transport::Hook);
        assert!(matches!(ev, AgentEvent::SessionStart { .. }));
        task.await.unwrap();
    }
}
