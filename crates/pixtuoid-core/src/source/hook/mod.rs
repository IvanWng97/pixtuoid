use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tracing::{debug, warn};

use crate::source::decoder::decode_hook_payload;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

mod pid_watch;
pub(crate) use pid_watch::HookPidWatch;

pub(crate) const MAX_CONCURRENT_CONNS: usize = 128;
pub(crate) const CONN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Typed marker for "another live instance owns the hook endpoint" — bind's
/// ONE recoverable failure (Unix: the sibling `<sock>.lock` advisory lock is
/// held by a live owner; Windows: CreateNamedPipeW fails ACCESS_DENIED
/// against the owner's `first_pipe_instance`). `ClaudeCodeSource::run`
/// downcasts for it and degrades to transcript-only (hooks disabled) instead
/// of taking the whole CC source (and the hook-only Reasonix source riding
/// the same socket) down with the bail. Every other bind error stays fatal.
#[derive(Debug)]
pub struct SocketBusy {
    pub path: PathBuf,
}

impl std::fmt::Display for SocketBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "another pixtuoid instance is listening on {} — close it first, or run this one \
             with PIXTUOID_SOCKET pointing at a different path",
            self.path.display()
        )
    }
}

impl std::error::Error for SocketBusy {}

pub struct HookSocketListener {
    inner: imp::Listener,
    path: PathBuf,
    /// Optional hook-supplied-pid liveness (CodeWhale). Set via
    /// `with_pid_watch`; a builder field rather than a `run` parameter so
    /// `run`'s public signature stays put (no semver break on pixtuoid-core).
    pid_watch: Option<HookPidWatch>,
}

impl HookSocketListener {
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let inner = imp::Listener::bind(&path).await?;
        Ok(Self {
            inner,
            path,
            pid_watch: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Attach a [`HookPidWatch`] so hook payloads carrying a `_pid` register a
    /// process-exit watch (instant `SessionEnd` on an abrupt CLI exit). `pub(crate)`
    /// — only the in-crate sources wire it.
    pub(crate) fn with_pid_watch(mut self, pid_watch: Option<HookPidWatch>) -> Self {
        self.pid_watch = pid_watch;
        self
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        self.inner.run(tx, self.pid_watch).await
    }
}

/// Per-connection byte ceiling: 2× the shim's ~1MiB stdin cap (the cap is
/// `1MiB − STAMP_HEADROOM` since the pipe-quota fix, so 2MiB sits comfortably
/// above any stamped line). lines() buffers
/// until a newline, so without this an adversarial client could grow the
/// buffer unboundedly for the whole CONN_TIMEOUT window × 128 slots —
/// defense-in-depth behind the owner-only endpoint (Unix socket 0700, Windows
/// pipe owner-only SDDL).
const MAX_CONN_BYTES: u64 = 2 * 1024 * 1024;

/// Breadcrumb for the silent-loss path (R0612-02): the per-connection budget
/// (`CONN_TIMEOUT` in both platform listeners) bounds `handle_conn` by
/// DROPPING its future, so when cancellation lands mid-payload — e.g. the
/// send loop parked on a full reducer channel under heavy load — no code
/// after the cancelled await ever runs, and only a `Drop` impl can record
/// that the payload's already-decoded remainder never reached the reducer.
/// Runtime shutdown dropping the spawned conn task reaches this same `Drop`,
/// so the message reports the loss (count + agent attribution + the budget
/// for context) without asserting WHICH cancellation fired. Disarmed on
/// channel-closed (receiver gone = daemon shutdown): those events are
/// dropped on purpose, not lost.
struct UndeliveredEvents {
    ids: Vec<crate::AgentId>,
    delivered: usize,
}

impl UndeliveredEvents {
    fn new(evs: &[crate::source::AgentEvent]) -> Self {
        Self {
            ids: evs.iter().map(|ev| ev.agent_id()).collect(),
            delivered: 0,
        }
    }

    fn delivered_one(&mut self) {
        self.delivered += 1;
    }

    fn disarm(&mut self) {
        self.delivered = self.ids.len();
    }
}

impl Drop for UndeliveredEvents {
    fn drop(&mut self) {
        let undelivered = &self.ids[self.delivered..];
        if undelivered.is_empty() {
            return;
        }
        let mut agents: Vec<String> = undelivered.iter().map(ToString::to_string).collect();
        // One payload's events share at most a couple of ids, adjacent by
        // construction (Identity precedes its activity event).
        agents.dedup();
        warn!(
            "hook connection cancelled mid-payload: {} decoded event(s) for agent(s) [{}] \
             never reached the reducer (per-connection budget: {CONN_TIMEOUT:?})",
            undelivered.len(),
            agents.join(", ")
        );
    }
}

/// The agent a decoded event should bind to the connection's `_pid` (for the
/// abrupt-exit watch). BOTH `SessionStart` and `Identity` register-or-back-fill
/// a slot, and `Identity` is the ONLY registration carrier for a mid-attached
/// session whose `SessionStart` predates the daemon (opencode, which never
/// re-emits `session.created`). Activity/Waiting/End events never register a new
/// slot, so they don't bind. Pure so the binding rule is unit-testable without
/// the socket loop.
fn pid_bind_target(ev: &AgentEvent) -> Option<AgentId> {
    match ev {
        AgentEvent::SessionStart { agent_id, .. } | AgentEvent::Identity { agent_id, .. } => {
            Some(*agent_id)
        }
        _ => None,
    }
}

pub(crate) async fn handle_conn(
    stream: impl AsyncRead + Unpin,
    tx: TaggedSender,
    pid_watch: Option<HookPidWatch>,
) {
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
                // Peek the shim-supplied CLI pid BEFORE `v` is consumed by
                // decode. Only sources whose shim/plugin stamps `_pid`
                // (CodeWhale, opencode) have it, so this is inert for the rest.
                let pid = pid_watch
                    .as_ref()
                    .and_then(|_| v.get("_pid"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|p| i32::try_from(p).ok());
                match decode_hook_payload(v) {
                    // One payload can decode to multiple events (an Identity
                    // attached ahead of a tool/permission event, #221) — sent
                    // in order on the same channel, so the reducer registers
                    // with real identity before the activity event applies.
                    Ok(evs) => {
                        // Bind every freshly-registered agent to the CLI's pid so
                        // an abrupt exit ends it (see `HookPidWatch`). Binds on
                        // BOTH SessionStart AND Identity (`pid_bind_target`): a
                        // session whose SessionStart predates our attach
                        // (mid-attach) registers through the Identity prepended
                        // ahead of its next tool/permission event — and opencode
                        // emits its SessionStart-carrier (`session.created`) only
                        // ONCE per session (no resurrect-on-prompt), so binding on
                        // SessionStart alone would leave a mid-attached opencode
                        // sprite with NO abrupt-exit signal. `note` is idempotent
                        // per (pid, agent), so the redundant bind is harmless.
                        if let (Some(pid), Some(watch)) = (pid, pid_watch.as_ref()) {
                            for ev in &evs {
                                if let Some(agent_id) = pid_bind_target(ev) {
                                    watch.note(pid, agent_id);
                                }
                            }
                        }
                        let mut undelivered = UndeliveredEvents::new(&evs);
                        for ev in evs {
                            debug!("hook event: {ev:?}");
                            if tx.send((Transport::Hook, ev)).await.is_err() {
                                undelivered.disarm();
                                return;
                            }
                            undelivered.delivered_one();
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
    use crate::AgentId;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn pid_bind_target_covers_both_registration_carriers() {
        // SessionStart and Identity both register-or-back-fill a slot, so both
        // bind the pid. Identity is the mid-attach carrier (a session whose
        // SessionStart predates attach — opencode never re-emits session.created).
        let id = AgentId::from_parts("opencode", "ses_x");
        for ev in [
            AgentEvent::SessionStart {
                agent_id: id,
                source: "opencode".into(),
                session_id: "ses_x".into(),
                cwd: "/r".into(),
                parent_id: None,
            },
            AgentEvent::Identity {
                agent_id: id,
                source: "opencode".into(),
                session_id: "ses_x".into(),
                cwd: None,
            },
        ] {
            assert_eq!(pid_bind_target(&ev), Some(id), "{ev:?} must bind the pid");
        }
        // Activity / Waiting / End never register a NEW slot, so they don't bind.
        for ev in [
            AgentEvent::ActivityStart {
                agent_id: id,
                tool_use_id: None,
                detail: None,
            },
            AgentEvent::ActivityEnd {
                agent_id: id,
                tool_use_id: None,
            },
            AgentEvent::Waiting {
                agent_id: id,
                reason: "x".into(),
            },
            AgentEvent::SessionEnd {
                agent_id: id,
                as_child: false,
            },
        ] {
            assert_eq!(pid_bind_target(&ev), None, "{ev:?} must not bind the pid");
        }
    }

    /// MakeWriter that captures formatted log lines so the tests can assert
    /// on the breadcrumb's presence/absence.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_warns() -> (CaptureWriter, tracing::subscriber::DefaultGuard) {
        let logs = CaptureWriter::default();
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(logs.clone())
                .with_ansi(false)
                .with_max_level(tracing::Level::WARN)
                .finish(),
        );
        (logs, guard)
    }

    // Decodes to TWO events (Identity + ActivityStart, #221) — the shape the
    // mid-payload loss path needs: more than one event per payload.
    const PRE_TOOL_USE_LINE: &[u8] = b"{\"hook_event_name\":\"PreToolUse\",\
        \"session_id\":\"ses-abc\",\
        \"transcript_path\":\"/p/ses-abc.jsonl\",\
        \"cwd\":\"/repo\",\
        \"tool_name\":\"Bash\",\
        \"tool_input\":{\"command\":\"ls\"},\
        \"tool_use_id\":\"t1\"}\n";

    // Platform-neutral framing test: handle_conn is generic over AsyncRead,
    // so the SHARED decode path is pinned without any socket/pipe — this is
    // the one test that runs identically on macOS, Linux, and Windows.
    #[tokio::test]
    async fn handle_conn_decodes_one_line_via_duplex() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);

        let task = tokio::spawn(handle_conn(server, tx, None));
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

    // The silent-loss path (R0612-02): the budget expires while the send loop
    // is parked on a full reducer channel, the timeout wrapper drops the
    // handle_conn future, and the payload's already-decoded remainder is
    // discarded — the breadcrumb is the only trace. Deterministic by
    // construction: nothing drains the capacity-1 channel, so the second send
    // can NEVER complete and the timeout always cancels mid-payload
    // regardless of scheduling.
    #[tokio::test]
    async fn cancelled_conn_leaves_breadcrumb_for_undelivered_events() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(1);
        let (logs, _guard) = capture_warns();

        client.write_all(PRE_TOOL_USE_LINE).await.unwrap();

        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            handle_conn(server, tx, None),
        )
        .await
        .is_err();
        assert!(timed_out, "second send parks on the full channel forever");

        // The Identity made it through; the ActivityStart is the loss.
        let (_, ev) = rx.try_recv().expect("first decoded event delivered");
        assert!(matches!(ev, AgentEvent::Identity { .. }));
        assert!(
            rx.try_recv().is_err(),
            "second event must have been dropped"
        );

        let out = logs.contents();
        assert!(
            out.contains("1 decoded event(s)"),
            "loss breadcrumb missing from logs: {out:?}"
        );
        // Cause-neutral phrasing: runtime-shutdown future-drop reaches the
        // same Drop, so the message must not assert CONN_TIMEOUT fired.
        assert!(
            out.contains("cancelled mid-payload"),
            "breadcrumb must report a cancellation, not assert a cause: {out:?}"
        );
        // Session attribution: the undelivered event's agent id is in the line.
        let expected = AgentId::from_parts("claude-code", "ses-abc");
        assert!(
            out.contains(&expected.to_string()),
            "breadcrumb must attribute the loss to its agent id: {out:?}"
        );
    }

    #[tokio::test]
    async fn clean_delivery_leaves_no_breadcrumb() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (logs, _guard) = capture_warns();

        client.write_all(PRE_TOOL_USE_LINE).await.unwrap();
        drop(client);
        handle_conn(server, tx, None).await;

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        let out = logs.contents();
        assert!(
            !out.contains("cancelled mid-payload"),
            "fully delivered payload must not warn: {out:?}"
        );
    }

    #[tokio::test]
    async fn channel_closed_shutdown_leaves_no_breadcrumb() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        drop(rx); // receiver gone = daemon shutdown
        let (logs, _guard) = capture_warns();

        client.write_all(PRE_TOOL_USE_LINE).await.unwrap();
        handle_conn(server, tx, None).await;

        let out = logs.contents();
        assert!(
            !out.contains("cancelled mid-payload"),
            "shutdown drop is deliberate, not a loss: {out:?}"
        );
    }
}
