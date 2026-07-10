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

mod router;
pub use router::HookRouter;

pub(crate) const MAX_CONCURRENT_CONNS: usize = 128;
pub(crate) const CONN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Typed marker for "another live instance owns the hook endpoint" — bind's
/// ONE recoverable failure (Unix: the sibling `<sock>.lock` advisory lock is
/// held by a live owner; Windows: CreateNamedPipeW fails ACCESS_DENIED
/// against the owner's `first_pipe_instance`). `HookRouter::run` downcasts for
/// it and degrades the HOOK PLANE to a quiet `Ok(())` (no `SourceDeath`) instead
/// of dying — so a second instance takes ONLY the hook plane down while the
/// transcript watchers (CC/Codex/…) keep running as independent `SourceManager`
/// tasks. Every other bind error stays fatal → `SourceDeath`.
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
    /// Optional hook-supplied-pid liveness (CodeWhale + opencode — both stamp
    /// `_pid`). Set via `with_pid_watch`; a builder field rather than a `run`
    /// parameter so `run`'s public signature stays put (no semver break on
    /// pixtuoid-core).
    pid_watch: Option<HookPidWatch>,
    /// Optional presence side-channel for the daemon fixture (OpenClaw): its
    /// payloads decode to presence deltas sent here (they yield no `AgentEvent`s).
    /// A builder field like `pid_watch`, so `run`'s signature stays put.
    presence_tx: Option<PresenceSender>,
}

/// The daemon-presence side channel (invariant #2: NOT the one `AgentEvent`
/// channel). The shared, source-tagged tuple form (`(source, delta)`) so N
/// daemons route to distinct `SceneState::daemons` entries — see
/// [`crate::source::daemon::PresenceSender`].
pub(crate) type PresenceSender = crate::source::daemon::PresenceSender;

impl HookSocketListener {
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let inner = imp::Listener::bind(&path).await?;
        Ok(Self {
            inner,
            path,
            pid_watch: None,
            presence_tx: None,
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

    /// Attach the presence side-channel so daemon payloads decode to presence
    /// deltas (they produce no `AgentEvent`s). Wired by the `HookRouter`, which
    /// owns the shared socket every CLI's hooks ride.
    pub(crate) fn with_presence(mut self, presence_tx: Option<PresenceSender>) -> Self {
        self.presence_tx = presence_tx;
        self
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        self.inner.run(tx, self.pid_watch, self.presence_tx).await
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

/// Stamp the connection's peeked `_pid` onto every `Identity` event of a
/// decoded batch — the focus-jump wiring. Identity is the carrier (it recurs
/// ahead of every activity, so the reducer's `slot.pid` cache stays fresh);
/// the per-source decoders never see the envelope key, so this single patch
/// point IS the whole wiring. `None` leaves events untouched (the reducer
/// never downgrades a cached pid either — belt and braces).
///
/// The TRANSCRIPT family (any source with a `line_decoder`) is skipped: the
/// shim's `getppid` is their hook-command parent (possibly a transient shell,
/// never recycle-guarded), while their real focus channel is the liveness
/// probes (`cc_pid_for_session`/`codex_pid_for_session`). A stamped stale pid
/// would shadow the probe's recycle guard in `resolve_pid`.
fn patch_identity_pids(evs: &mut [AgentEvent], pid: Option<i32>) {
    let Some(pid) = pid else { return };
    for ev in evs {
        if let AgentEvent::Identity { source, pid: p, .. } = ev {
            let transcript_bearing = crate::source::registry::descriptor_for(source)
                .is_some_and(|d| d.line_decoder().is_some());
            if !transcript_bearing {
                *p = Some(pid);
            }
        }
    }
}

pub(crate) async fn handle_conn(
    stream: impl AsyncRead + Unpin,
    tx: TaggedSender,
    pid_watch: Option<HookPidWatch>,
    presence_tx: Option<PresenceSender>,
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
                // DAEMON demux (registry-DRIVEN — NO source named here): a daemon
                // (the OpenClaw gateway is instance #1) emits ZERO `AgentEvent`s;
                // its payloads decode to presence deltas on the sibling channel
                // (invariant #2), source-tagged so N daemons route to distinct
                // `SceneState::daemons` entries. `presence_decoder_for` returns
                // `None` for every agent source, so this is inert for them — and a
                // 2nd daemon needs NO edit here.
                if let (Some(ptx), Some(src)) = (
                    presence_tx.as_ref(),
                    v.get("_pixtuoid_source")
                        .and_then(serde_json::Value::as_str),
                ) {
                    if let Some(decode) = crate::source::registry::presence_decoder_for(src) {
                        match decode(&v) {
                            Ok(updates) => {
                                for u in updates {
                                    let _ = ptx.send(crate::source::daemon::PresenceMsg {
                                        source: src.to_string(),
                                        delta: u,
                                    });
                                }
                            }
                            Err(e) => warn!("daemon presence decode error: {e}"),
                        }
                        // A daemon produces no AgentEvents — never the agent arms.
                        continue;
                    }
                }
                // Peek the shim-supplied CLI pid BEFORE `v` is consumed by
                // decode. Every unix shim invocation carries `_pid` now (the
                // shim fills it from getppid when the plugin didn't stamp one);
                // the transcript family is filtered downstream in
                // `patch_identity_pids` — the probes are their channel.
                // Deliberately NOT gated on `pid_watch`: the exit-watch backend
                // failing to init (pre-5.3 Linux kernel, Windows) must not
                // take the focus-jump pid cache down with it — only the BIND
                // below needs the watch. `as_u64` already rejects negatives;
                // the `> 0` filter drops a crafted `_pid: 0` too (kill(0)
                // targets the process GROUP — same guard as
                // cc_probe/fd_probe/openclaw).
                let pid = v
                    .get("_pid")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|p| i32::try_from(p).ok())
                    .filter(|p| *p > 0);
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
                        // Second consumer of the SAME peeked `_pid`: the
                        // focus-jump cache (see `patch_identity_pids`).
                        let mut evs = evs;
                        patch_identity_pids(&mut evs, pid);
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
    use tokio::io::AsyncWriteExt;

    #[test]
    fn patch_identity_pids_stamps_only_identity_events() {
        let id = AgentId::from_parts("opencode", "ses_x");
        let mut evs = vec![
            AgentEvent::Identity {
                agent_id: id,
                source: "opencode".into(),
                session_id: "ses_x".into(),
                cwd: None,
                pid: None,
            },
            AgentEvent::ActivityStart {
                agent_id: id,
                tool_use_id: None,
                detail: None,
            },
        ];
        patch_identity_pids(&mut evs, Some(77));
        assert!(
            matches!(evs[0], AgentEvent::Identity { pid: Some(77), .. }),
            "Identity stamped"
        );
        // None leaves the batch untouched.
        patch_identity_pids(&mut evs, None);
        assert!(matches!(evs[0], AgentEvent::Identity { pid: Some(77), .. }));
    }

    #[test]
    fn patch_identity_pids_never_stamps_the_transcript_family() {
        // CC/Codex resolve pid via the recycle-guarded probes; the shim's
        // getppid is their hook-command parent (possibly a transient shell).
        // A stamped stale pid would shadow the probe in `resolve_pid`.
        for source in ["claude-code", "codex", "antigravity", "copilot"] {
            let mut evs = vec![AgentEvent::Identity {
                agent_id: AgentId::from_parts(source, "ses_t"),
                source: source.into(),
                session_id: "ses_t".into(),
                cwd: None,
                pid: None,
            }];
            patch_identity_pids(&mut evs, Some(77));
            assert!(
                matches!(evs[0], AgentEvent::Identity { pid: None, .. }),
                "{source} Identity must stay pid: None"
            );
        }
        // Hook-only sources (no line_decoder) still get the stamp.
        for source in ["opencode", "cursor", "codewhale", "hermes", "reasonix"] {
            let mut evs = vec![AgentEvent::Identity {
                agent_id: AgentId::from_parts(source, "ses_t"),
                source: source.into(),
                session_id: "ses_t".into(),
                cwd: None,
                pid: None,
            }];
            patch_identity_pids(&mut evs, Some(77));
            assert!(
                matches!(evs[0], AgentEvent::Identity { pid: Some(77), .. }),
                "{source} Identity must be stamped"
            );
        }
    }

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
                pid: None,
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

    use crate::test_capture::capture_warns;

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

        let task = tokio::spawn(handle_conn(server, tx, None, None));
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

    /// The per-connection byte ceiling is 2 MiB — 2× the shim's ~1 MiB stdin
    /// cap, so one maximal stamped line ALWAYS fits with headroom. A payload
    /// just over half the ceiling (1.25 MiB — bigger than any single legal
    /// shim line's cap, well under the ceiling) must still decode; a
    /// `2*1024*1024` mutation to `(2+1024)*1024` (≈1.03 MiB) or
    /// `2*(1024+1024)` (4 KiB) truncates it mid-line and loses the event.
    #[tokio::test]
    async fn handle_conn_ceiling_leaves_headroom_over_the_shim_line_cap() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let task = tokio::spawn(handle_conn(server, tx, None, None));
        let pad = "x".repeat((1 << 20) + (1 << 18)); // 1.25 MiB of payload padding
        let line = format!(
            "{{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s1\",\
             \"transcript_path\":\"/p/s1.jsonl\",\"pad\":\"{pad}\"}}\n"
        );
        client.write_all(line.as_bytes()).await.unwrap();
        drop(client);
        let (transport, ev) = rx
            .recv()
            .await
            .expect("a 1.25 MiB line sits inside the 2 MiB conn ceiling");
        assert_eq!(transport, Transport::Hook);
        assert!(matches!(ev, AgentEvent::SessionStart { .. }));
        task.await.unwrap();
    }

    // The daemon demux is registry-DRIVEN (no source named in handle_conn): an
    // OpenClaw line routes to the SIDE channel as a SOURCE-TAGGED tuple
    // `("openclaw", GatewayUp)` and emits ZERO AgentEvents, while an ordinary CC
    // line decodes only on the AgentEvent channel.
    #[tokio::test]
    async fn handle_conn_routes_openclaw_presence_to_the_side_channel_only() {
        use crate::source::daemon::DaemonPresenceUpdate;
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (ptx, mut prx) =
            tokio::sync::mpsc::unbounded_channel::<crate::source::daemon::PresenceMsg>();

        let task = tokio::spawn(handle_conn(server, tx, None, Some(ptx)));
        // A shim-stamped OpenClaw presence line, then an ordinary CC agent line.
        client
            .write_all(
                b"{\"_pixtuoid_source\":\"openclaw\",\"type\":\"gateway_start\",\"_pid\":4242}\n",
            )
            .await
            .unwrap();
        client
            .write_all(
                b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s1\",\
                  \"transcript_path\":\"/Users/me/.claude/projects/x/s1.jsonl\"}\n",
            )
            .await
            .unwrap();
        drop(client);

        // The OpenClaw line → exactly one ("openclaw", GatewayUp) on the SIDE
        // channel, and the AgentEvent channel never sees it (zero AgentEvents).
        let msg = prx.recv().await.expect("one presence update");
        assert_eq!(
            msg.source, "openclaw",
            "presence is source-tagged for N-daemon routing"
        );
        assert!(matches!(
            msg.delta,
            DaemonPresenceUpdate::GatewayUp { pid: Some(4242) }
        ));
        assert!(
            prx.try_recv().is_err(),
            "the CC line must not reach presence"
        );

        // The CC line → exactly one SessionStart on the AgentEvent channel.
        let (transport, ev) = rx.recv().await.expect("one agent event");
        assert_eq!(transport, Transport::Hook);
        assert!(matches!(ev, AgentEvent::SessionStart { .. }));
        assert!(
            rx.try_recv().is_err(),
            "the openclaw line emits no AgentEvent"
        );
        task.await.unwrap();
    }

    // The INVERSE of the routing test above (census #266 arch-drift seam #1): an
    // AGENT source that is shim-stamped `_pixtuoid_source` AND carries `_pid`
    // (opencode/CodeWhale do — three producers write `_pid` at this one socket)
    // must take the AgentEvent path and NEVER the presence side channel, EVEN
    // with `presence_tx` active. The whole daemon demux rests on
    // `presence_decoder_for` returning None for every agent source (so the
    // daemon arm's `continue` is unreachable for them); this pins that invariant
    // from the other side — it FAILS the moment an agent source gains a presence
    // decoder (a dual-natured source), the silent cross-route no other test
    // catches.
    #[tokio::test]
    async fn handle_conn_agent_source_with_pid_never_routes_to_presence() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (ptx, mut prx) =
            tokio::sync::mpsc::unbounded_channel::<crate::source::daemon::PresenceMsg>();

        // Spawn with the presence channel ACTIVE so the demux block is entered.
        let task = tokio::spawn(handle_conn(server, tx, None, Some(ptx)));
        // A shim-stamped opencode `session.created` carrying `_pid` — an AGENT
        // source (presence_decoder_for("opencode") == None), so it must fall
        // through to the agent arm.
        client
            .write_all(
                b"{\"_pixtuoid_source\":\"opencode\",\"type\":\"session.created\",\
                  \"properties\":{\"info\":{\"id\":\"ses_neg\",\"directory\":\"/repo\"}},\
                  \"_pid\":5555}\n",
            )
            .await
            .unwrap();
        drop(client);

        // It decodes to a SessionStart on the AgentEvent channel...
        let (transport, ev) = rx.recv().await.expect("agent event arrived");
        assert_eq!(transport, Transport::Hook);
        assert!(
            matches!(ev, AgentEvent::SessionStart { .. }),
            "opencode session.created decodes to SessionStart, got {ev:?}"
        );
        // ...and NOTHING ever reaches the presence channel.
        assert!(
            prx.try_recv().is_err(),
            "an agent-source payload with _pid must NOT route to presence"
        );
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
            handle_conn(server, tx, None, None),
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
        handle_conn(server, tx, None, None).await;

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        let out = logs.contents();
        assert!(
            !out.contains("cancelled mid-payload"),
            "fully delivered payload must not warn: {out:?}"
        );
    }

    // The pid-bind loop in handle_conn (line 263-268): a payload carrying a
    // shim-stamped `_pid` that decodes to a SessionStart binds that agent to the
    // pid through the LIVE HookPidWatch, so killing the pid ends the slot. The
    // pid_watch.rs suite calls `note()` directly — this drives the bind THROUGH
    // the socket loop end-to-end (peek `_pid` → decode → pid_bind_target →
    // note). Platform-gated like the pid_watch test (no exit-watch backend on
    // Windows / pre-5.3 Linux → spawn returns None → no-op).
    #[tokio::test]
    async fn handle_conn_binds_pid_from_payload_so_killing_it_ends_the_agent() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let Some(watch) = HookPidWatch::spawn(tx.clone()) else {
            return; // no exit-watch backend on this platform — nothing to assert
        };
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn a child to watch");
        let pid = child.id();

        let (mut client, server) = tokio::io::duplex(4096);
        let task = tokio::spawn(handle_conn(server, tx, Some(watch), None));
        // A CodeWhale env-mode envelope: `session_start` decodes to a SessionStart
        // keyed on the cwd, stamped with the child's `_pid`. pid_bind_target binds
        // that agent id to the pid via the live watch.
        let line = format!(
            "{{\"_pixtuoid_source\":\"codewhale\",\"event\":\"session_start\",\
             \"cwd\":\"/repo\",\"_pid\":{pid}}}\n"
        );
        client.write_all(line.as_bytes()).await.unwrap();

        // Drain the SessionStart so it doesn't race the SessionEnd on the channel.
        let (_, ev) = rx.recv().await.expect("the SessionStart from the payload");
        assert!(matches!(ev, AgentEvent::SessionStart { .. }), "got {ev:?}");

        drop(client); // EOF ends the conn loop
        task.await.unwrap();

        // Killing the bound pid must end EXACTLY the cwd-keyed agent. Falsifiable:
        // if line 266's note() were dropped, no SessionEnd ever arrives.
        child.kill().expect("kill the watched child");
        let _ = child.wait();
        let expected = AgentId::from_parts("codewhale", "/repo");
        let (transport, ev) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a SessionEnd within 5s of the watched pid dying")
            .expect("channel still open");
        assert_eq!(transport, Transport::Hook);
        assert!(
            matches!(ev, AgentEvent::SessionEnd { agent_id, as_child: false } if agent_id == expected),
            "the payload-bound agent must end when its pid dies, got {ev:?}"
        );
    }

    // The composed focus-jump path (peek `_pid` → decode → patch_identity_pids):
    // a transcript-family (CC) payload's Identity must arrive with pid: None
    // even when the shim stamped `_pid` — the unit test on patch_identity_pids
    // can't catch a wiring regression in handle_conn itself. The peek needs no
    // watch (deliberately un-gated), so this runs on every platform.
    #[tokio::test]
    async fn handle_conn_never_stamps_a_transcript_family_identity_pid() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (mut client, server) = tokio::io::duplex(4096);
        let task = tokio::spawn(handle_conn(server, tx, None, None));
        let me = std::process::id();
        let line = format!(
            "{{\"hook_event_name\":\"PreToolUse\",\"session_id\":\"ses-cc\",\
             \"transcript_path\":\"/p/a.jsonl\",\"cwd\":\"/repo\",\
             \"tool_name\":\"Bash\",\"tool_use_id\":\"t1\",\"_pid\":{me}}}\n"
        );
        client.write_all(line.as_bytes()).await.unwrap();
        drop(client);
        task.await.unwrap();

        let (_, ev) = rx
            .recv()
            .await
            .expect("the Identity ahead of the tool event");
        assert!(
            matches!(&ev, AgentEvent::Identity { source, pid: None, .. } if source == "claude-code"),
            "CC Identity must stay pid: None through the socket loop, got {ev:?}"
        );
        let (_, ev) = rx.recv().await.expect("the paired ActivityStart");
        assert!(matches!(ev, AgentEvent::ActivityStart { .. }), "got {ev:?}");
    }

    // The focus-jump stamp must survive a missing exit-watch backend: the
    // `_pid` peek is deliberately NOT gated on `pid_watch` (pre-5.3 Linux /
    // Windows spawn `None`), so a hook-family Identity still carries the pid
    // with NO watch attached — only the exit-watch BIND needs the watch.
    // This is the regression pin: re-gating the peek fails this test.
    #[tokio::test]
    async fn handle_conn_stamps_identity_pid_even_without_a_pid_watch() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (mut client, server) = tokio::io::duplex(4096);
        let task = tokio::spawn(handle_conn(server, tx, None, None));
        // CodeWhale tool_call_before decodes to [Identity, ActivityStart].
        let line = "{\"_pixtuoid_source\":\"codewhale\",\"event\":\"tool_call_before\",\
                    \"cwd\":\"/repo\",\"tool\":\"exec_shell\",\"_pid\":4242}\n";
        client.write_all(line.as_bytes()).await.unwrap();
        drop(client);
        task.await.unwrap();

        let (_, ev) = rx
            .recv()
            .await
            .expect("the Identity ahead of the tool event");
        assert!(
            matches!(&ev, AgentEvent::Identity { source, pid: Some(4242), .. } if source == "codewhale"),
            "hook-family Identity must carry the peeked pid without a watch, got {ev:?}"
        );
    }

    // The decode-error arm (line 280): a syntactically-valid JSON object whose
    // hook_event_name is unsupported reaches decode's `other =>` bail, warn!s
    // "hook decode error", and the loop CONTINUES emitting nothing. Distinct
    // from the malformed-JSON path (the socket test covers that) — this is
    // valid-JSON-but-undecodable. A following valid SessionStart still decodes,
    // proving the bogus line is dropped, not fallen-through.
    #[tokio::test]
    async fn handle_conn_skips_valid_json_with_unsupported_event_and_emits_nothing() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (logs, _guard) = capture_warns();

        let task = tokio::spawn(handle_conn(server, tx, None, None));
        client
            .write_all(b"{\"hook_event_name\":\"BogusEvent\",\"session_id\":\"s1\"}\n")
            .await
            .unwrap();
        client
            .write_all(
                b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s2\",\
                  \"transcript_path\":\"/Users/me/.claude/projects/x/s2.jsonl\"}\n",
            )
            .await
            .unwrap();
        drop(client);
        task.await.unwrap();

        // EXACTLY one event — the SessionStart; the bogus line emitted nothing.
        let (_, ev) = rx.try_recv().expect("the valid SessionStart");
        assert!(matches!(ev, AgentEvent::SessionStart { .. }), "got {ev:?}");
        assert!(
            rx.try_recv().is_err(),
            "the unsupported-event line must emit no AgentEvent"
        );
        assert!(
            logs.contents().contains("hook decode error"),
            "the bail must log a decode error: {:?}",
            logs.contents()
        );
    }

    // The empty/whitespace-line skip (line 200): a blank line short-circuits
    // BEFORE serde_json::from_str, so it does NOT log the "malformed hook line
    // skipped" warning that from_str("") would otherwise trigger. Falsifiable:
    // deleting the trim().is_empty() guard makes the blank line hit from_str →
    // Err → that warn, flipping the absence assertion.
    #[tokio::test]
    async fn handle_conn_silently_skips_blank_lines_without_a_malformed_warning() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (logs, _guard) = capture_warns();

        let task = tokio::spawn(handle_conn(server, tx, None, None));
        client.write_all(b"   \n").await.unwrap();
        client
            .write_all(
                b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s1\",\
                  \"transcript_path\":\"/Users/me/.claude/projects/x/s1.jsonl\"}\n",
            )
            .await
            .unwrap();
        drop(client);
        task.await.unwrap();

        let (_, ev) = rx.try_recv().expect("the valid SessionStart");
        assert!(matches!(ev, AgentEvent::SessionStart { .. }), "got {ev:?}");
        assert!(
            rx.try_recv().is_err(),
            "only the SessionStart, the blank line produces nothing"
        );
        assert!(
            !logs.contents().contains("malformed hook line skipped"),
            "a blank line must skip before from_str — no malformed warning: {:?}",
            logs.contents()
        );
    }

    // The daemon-presence decode-error arm (line 232): when presence_decoder_for
    // returns Some but decode(&v) errs (an openclaw payload missing `type`), it
    // warn!s AND still `continue`s — the malformed daemon line must NOT fall
    // through to the agent arms. Falsifiable: if the Err arm omitted `continue`,
    // the openclaw line would reach decode_hook_payload below. A following valid
    // CC line still decodes on the agent channel, proving only the daemon line
    // was short-circuited.
    #[tokio::test]
    async fn handle_conn_malformed_openclaw_presence_continues_without_falling_through() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        let (ptx, mut prx) =
            tokio::sync::mpsc::unbounded_channel::<crate::source::daemon::PresenceMsg>();

        let task = tokio::spawn(handle_conn(server, tx, None, Some(ptx)));
        // An openclaw payload that is an object but lacks `type` — its decoder
        // errs, so the demux warns + continues (zero presence msgs).
        client
            .write_all(b"{\"_pixtuoid_source\":\"openclaw\"}\n")
            .await
            .unwrap();
        client
            .write_all(
                b"{\"hook_event_name\":\"SessionStart\",\"session_id\":\"s1\",\
                  \"transcript_path\":\"/Users/me/.claude/projects/x/s1.jsonl\"}\n",
            )
            .await
            .unwrap();
        drop(client);
        task.await.unwrap();

        // The malformed openclaw line produced NO presence delta...
        assert!(
            prx.try_recv().is_err(),
            "a daemon decode failure must emit no presence message"
        );
        // ...and the only agent event is the SessionStart (it did NOT fall
        // through to the agent arms, which would have errored differently).
        let (transport, ev) = rx.try_recv().expect("the valid CC SessionStart");
        assert_eq!(transport, Transport::Hook);
        assert!(matches!(ev, AgentEvent::SessionStart { .. }), "got {ev:?}");
        assert!(
            rx.try_recv().is_err(),
            "the malformed openclaw line emits no AgentEvent"
        );
    }

    #[tokio::test]
    async fn channel_closed_shutdown_leaves_no_breadcrumb() {
        let (mut client, server) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(8);
        drop(rx); // receiver gone = daemon shutdown
        let (logs, _guard) = capture_warns();

        client.write_all(PRE_TOOL_USE_LINE).await.unwrap();
        handle_conn(server, tx, None, None).await;

        let out = logs.contents();
        assert!(
            !out.contains("cancelled mid-payload"),
            "shutdown drop is deliberate, not a loss: {out:?}"
        );
    }
}
