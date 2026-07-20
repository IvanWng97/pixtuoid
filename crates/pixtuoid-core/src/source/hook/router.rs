//! `HookRouter` — the single honest owner of the ONE shared hook socket that
//! EVERY source's hooks ride (Codex, Reasonix, CodeWhale, opencode, Cursor, and
//! the OpenClaw daemon). It used to be bound as a side-effect of
//! `ClaudeCodeSource` (a transcript watcher that "happened to" host the socket);
//! lifting it here makes CC a pure `JsonlWatcher` and gives the hook plane a
//! dedicated owner.
//!
//! It implements [`Source`] DELIBERATELY: `Source::run` already hands the one
//! `(Transport, AgentEvent)` sender `handle_conn` sends on, and
//! `SourceManager::spawn_with_health` *generates* the `SourceDeath` that surfaces
//! a fatal exit in the TUI footer (#157) — so the router inherits that path for
//! free instead of rebuilding it. It is INFRASTRUCTURE, not a CLI: it has NO
//! registry/descriptor/badge row (the add-a-CLI checklist does not
//! apply); `source_set_includes_the_hook_router` closes the spawned-but-untested
//! gap.

use std::path::PathBuf;

use anyhow::Result;

use crate::source::jsonl::ChildEndUnclaims;
use crate::source::{AgentEvent, Source, TaggedReceiver, TaggedSender, Transport};

use super::{HookPidWatch, HookSocketListener, PresenceSender, SocketBusy};

/// Infrastructure name — NOT a registered source (no descriptor/badge). Used by
/// `spawn_with_health` to attribute a fatal bind error in the footer.
pub(crate) const SOURCE_NAME: &str = "hook-router";

/// Producer half of the #246 child-end un-claim side-channel (see
/// `ChildEndUnclaims` for the WHY). Interposed between the hook listener and the
/// real channel inside [`HookRouter::run`] — the listener's API stays
/// source-agnostic; this is the ONE seam every decoded `SubagentStop` (CC and
/// Codex alike — all sources' hooks ride the one shared socket) passes through.
/// Every event is forwarded UNCHANGED, transport tag included (invariant #2: the
/// producer's tag flows through). The push happens BEFORE the forward — the
/// order is irrelevant for correctness (the watcher drains on its own scan
/// cadence), but push-first means the un-claim is already pending by the time
/// the reducer applies the end, which keeps tests deterministic. Exits when
/// either side closes (listener gone → `recv` None; reducer gone → send Err).
pub(crate) async fn tee_child_end_unclaims(
    mut rx: TaggedReceiver,
    tx: TaggedSender,
    unclaims: ChildEndUnclaims,
) {
    while let Some((transport, ev)) = rx.recv().await {
        if transport == Transport::Hook {
            if let AgentEvent::SessionEnd {
                agent_id,
                as_child: true,
            } = &ev
            {
                unclaims.push(*agent_id);
            }
        }
        if tx.send((transport, ev)).await.is_err() {
            return;
        }
    }
}

/// The shared hook-socket owner. Binds the ONE socket, runs the per-connection
/// decode loop, interposes the #246 tee, and feeds the daemon-presence side
/// channel. Builder fields mirror the old `ClaudeCodeSource` plumbing exactly —
/// they just live on their honest owner now.
pub struct HookRouter {
    socket_path: PathBuf,
    /// The #246 child-end un-claim PRODUCER handle (the tee). The runtime shares
    /// ONE handle with the CC + Codex watchers (the CONSUMERS). `None` disables
    /// the tee (bare test construction).
    child_end_unclaims: Option<ChildEndUnclaims>,
    /// The daemon-presence side channel (the gateway mascots). Daemon payloads
    /// decode to presence deltas (no `AgentEvent`s) routed here for the reducer
    /// task. `None` disables it (bare test construction).
    presence_tx: Option<PresenceSender>,
}

impl HookRouter {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            child_end_unclaims: None,
            presence_tx: None,
        }
    }

    /// Wire the #246 child-end un-claim producer (the driver passes the shared
    /// handle whose consumers are the CC + Codex watchers).
    pub fn with_child_end_unclaims(mut self, unclaims: Option<ChildEndUnclaims>) -> Self {
        self.child_end_unclaims = unclaims;
        self
    }

    /// Wire the daemon-presence side channel (the driver passes the sender).
    pub fn with_presence_tx(mut self, presence_tx: Option<PresenceSender>) -> Self {
        self.presence_tx = presence_tx;
        self
    }
}

impl Source for HookRouter {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        // SocketBusy (another live instance owns the endpoint) takes ONLY the
        // hook plane down, never the transcript sources: the listener returns
        // Ok(()) (no `SourceDeath` — the hooks belong to the owning instance,
        // and the CC/Codex/... watchers run as independent tasks). Every other
        // bind error is fatal → `SourceDeath` (free, because this is a Source).
        let socket = match HookSocketListener::bind(self.socket_path.clone()).await {
            Ok(s) => s,
            Err(e) if e.downcast_ref::<SocketBusy>().is_some() => {
                tracing::warn!(
                    "{e:#}; hook plane disabled — hook-borne signals (permission \
                     Waiting, instant lifecycle, daemon presence) belong to the \
                     owning instance; transcript sources keep running"
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        // #246: route hook events through the un-claim tee when the side-channel
        // is wired (the runtime always wires it; `None` is bare test
        // construction). The tee task is a passive pipe and dies with the
        // listener (its sender drops).
        let tx_hook = match &self.child_end_unclaims {
            Some(unclaims) => {
                // Same capacity as the runtime's event channel: the tee adds a
                // stage, not a different backpressure policy.
                let (tee_tx, tee_rx) =
                    tokio::sync::mpsc::channel(crate::source::EVENT_CHANNEL_CAPACITY);
                tokio::spawn(tee_child_end_unclaims(tee_rx, tx.clone(), unclaims.clone()));
                tee_tx
            }
            None => tx.clone(),
        };
        // Hook-supplied-pid liveness (CodeWhale / opencode): their hooks ride
        // THIS shared socket. The synthesized SessionEnd goes on the main `tx`
        // (it is `as_child: false`, so the #246 tee — which acts only on
        // `as_child: true` — ignores it anyway). `None` on platforms without an
        // exit-watch backend → no-op.
        let socket = socket
            .with_pid_watch(HookPidWatch::spawn(tx.clone()))
            .with_presence(self.presence_tx.clone());
        socket.run(tx_hook).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentId;
    use std::time::Duration;

    /// The #246 tee contract: a Hook-transport `SessionEnd { as_child: true }`
    /// flowing through the forwarding task lands its id in the shared handle,
    /// AND every event — that one included — reaches the downstream channel
    /// UNCHANGED, in order, transport tag intact (invariant #2). Jsonl-tagged
    /// child ends and root (`as_child: false`) hook ends must NOT be pushed:
    /// copilot's transcript decode emits Jsonl child ends
    /// (`subagent.completed`/`failed`), so the transport guard is load-bearing,
    /// not merely defensive.
    #[tokio::test]
    async fn tee_pushes_hook_child_ends_and_forwards_every_event_unchanged() {
        let unclaims = ChildEndUnclaims::new();
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let tee = tokio::spawn(tee_child_end_unclaims(in_rx, out_tx, unclaims.clone()));

        let child = AgentId::from_parts("codex", "child-uuid");
        let root = AgentId::from_parts(crate::source::claude_code::SOURCE_NAME, "root-uuid");
        let events: Vec<(Transport, AgentEvent)> = vec![
            (
                Transport::Hook,
                AgentEvent::ActivityStart {
                    agent_id: root,
                    tool_use_id: Some("tu_1".into()),
                    detail: None,
                },
            ),
            // A JSONL-tagged child end must not feed the handle — copilot's
            // transcript decode emits exactly this shape (`subagent.completed`
            // / `failed` lines), so the transport guard IS the boundary.
            (
                Transport::Jsonl,
                AgentEvent::SessionEnd {
                    agent_id: root,
                    as_child: true,
                },
            ),
            // A root hook end is not a SubagentStop — not pushed.
            (
                Transport::Hook,
                AgentEvent::SessionEnd {
                    agent_id: root,
                    as_child: false,
                },
            ),
            // THE shape: the decoded SubagentStop.
            (
                Transport::Hook,
                AgentEvent::SessionEnd {
                    agent_id: child,
                    as_child: true,
                },
            ),
        ];
        for ev in &events {
            in_tx.send(ev.clone()).await.unwrap();
        }
        for expected in &events {
            let got = tokio::time::timeout(Duration::from_secs(5), out_rx.recv())
                .await
                .expect("tee must forward promptly")
                .expect("tee must not drop the channel");
            assert_eq!(
                &got, expected,
                "event parity: forwarded unchanged, in order"
            );
        }
        assert_eq!(
            unclaims.take_matching(|_| true),
            vec![child],
            "exactly the Hook-transport as_child end lands in the handle"
        );
        drop(in_tx);
        tokio::time::timeout(Duration::from_secs(5), tee)
            .await
            .expect("tee exits when the listener side closes")
            .unwrap();
    }

    /// `name()` is the footer attribution for a fatal bind (`spawn_with_health`
    /// publishes `SourceDeath { source: name }`) — a blank or drifted name
    /// makes that death message unattributable.
    #[test]
    fn router_name_is_the_infrastructure_source_name() {
        assert_eq!(
            HookRouter::new(PathBuf::from("/tmp/x.sock")).name(),
            SOURCE_NAME
        );
    }

    /// Only `SocketBusy` degrades to the quiet hook-plane-off Ok(()); every
    /// OTHER bind failure must surface as Err → `SourceDeath` (a guard→true
    /// mutation would swallow a permanently broken socket path silently).
    #[cfg(unix)]
    #[tokio::test]
    async fn fatal_bind_error_is_err_not_quiet_degradation() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("no-such-dir").join("hook.sock");
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let res = Box::new(HookRouter::new(bad)).run(tx).await;
        assert!(
            res.is_err(),
            "a non-Busy bind failure must be Err (SourceDeath fuel), got {res:?}"
        );
    }
}
