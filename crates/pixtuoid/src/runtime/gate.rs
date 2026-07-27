//! The connection-gate functional core — the pure, testable heart of
//! `reducer_task`'s event/presence/sweep loop, lifted OUT of the coverage- and
//! mutants-excluded `driver.rs` async shell (issue #103) so the gate decision
//! itself is measured and mutation-checked: a drift in the predicate now reds a
//! test instead of slipping through unguarded (the reason #741/#751 flagged the
//! old hand-copied test mirror of this logic). `driver.rs::reducer_task` is the
//! thin imperative shell that wires the tokio channels to these functions;
//! nothing here touches IO, tokio, or the clock — `now` is a parameter (sans-IO).

use std::collections::HashSet;
use std::time::SystemTime;

use pixtuoid_core::source::{daemon, registry};
use pixtuoid_core::{AgentEvent, Reducer, SceneState, Transport};

use super::ConnectedSources;

/// Resolve which source an incoming event is attributed to, for the connection
/// gate: a source-carrying `SessionStart`/`Identity` names itself; anything else
/// inherits its slot's source (or `None` when it has no slot yet, e.g. a bare
/// activity event that arrives before registration). Pure.
fn event_source<'a>(scene: &'a SceneState, ev: &'a AgentEvent) -> Option<&'a str> {
    match ev {
        AgentEvent::SessionStart { source, .. } | AgentEvent::Identity { source, .. }
            if !source.is_empty() =>
        {
            Some(source)
        }
        _ => scene.agents.get(&ev.agent_id()).map(|s| s.source.as_ref()),
    }
}

/// Apply one incoming event through the connection gate. Drops it (returns
/// `false`, no scene change) when its source is resolved AND not in the
/// connected set; otherwise applies it to the reducer and returns `true` so the
/// caller publishes the new scene.
///
/// The drop is announced once per registered source (`gate_logged` dedup),
/// because this is the only breadcrumb below the gate: without it a gated source
/// emits zero lines at every log level, making "connected but no sprite"
/// indistinguishable from "not connected" — the two hypotheses a reader most
/// needs to separate. Per-source, never per-event: a disconnected watcher
/// streams indefinitely. Bounded by the registry, NOT the wire — an unregistered
/// `_pixtuoid_source` is a DRIFT story (`source/drift.rs`), not a gate one, so it
/// is not keyed here (that would let a long-lived `run` accumulate one entry per
/// distinct raw name seen).
pub(crate) fn apply_gated_event(
    reducer: &mut Reducer,
    scene: &mut SceneState,
    ev: AgentEvent,
    transport: Transport,
    connected: &ConnectedSources,
    now: SystemTime,
    gate_logged: &mut HashSet<&'static str>,
) -> bool {
    if let Some(src) = event_source(scene, &ev).filter(|s| !connected.is_connected(s)) {
        if let Some(known) = registry::descriptor_for(src) {
            if gate_logged.insert(known.name) {
                tracing::debug!(source = known.name, "dropping events: source not connected");
            }
        }
        return false;
    }
    tracing::debug!(?transport, ?ev, "event");
    reducer.apply(scene, ev, now, transport);
    true
}

/// One reconcile-sweep tick — the 1-Hz cadence `reducer_task` runs so exit-grace
/// sweeps fire even when no events arrive. Walks out (idempotently) every slot
/// whose source is the COMPLEMENT of the connected snapshot — so a disconnect
/// evicts live AND a blank-source slot that slipped the per-event gate is swept
/// too — then advances exit-grace and runs each daemon's presence reconcile +
/// TTL decay. Stateless on purpose: no prev-set bookkeeping (registry-DRIVEN, so
/// an Nth daemon needs no edit here).
pub(crate) fn reconcile_sweep_tick(
    reducer: &mut Reducer,
    scene: &mut SceneState,
    connected: &ConnectedSources,
    now: SystemTime,
) {
    let cur = connected.snapshot();
    reducer.reconcile_connected(scene, &cur, now);
    reducer.tick(scene, now);
    for (source, ttl) in registry::daemon_sources() {
        if !connected.is_connected(source) {
            daemon::mark_presence_down(scene, source, now);
        }
        daemon::sweep_presence_ttl(scene, source, ttl, now);
    }
}

/// The outcome of gating a daemon-presence delta: `Dropped` (the daemon's source
/// is not connected — nothing applied, nothing to arm) or `Applied`, carrying the
/// gateway pid the shell should arm the abrupt-down exit watch on (`None` when the
/// delta arms nothing, e.g. `SessionStarted`). The enum makes "dropped but has a
/// pid to arm" unrepresentable.
pub(crate) enum PresenceGate {
    Dropped,
    Applied { arm_pid: Option<i32> },
}

/// Apply one daemon-presence delta through the connection gate — the presence
/// twin of [`apply_gated_event`]. Returns [`PresenceGate::Dropped`] when the
/// daemon's source is not connected (a panel-disconnected daemon is instead
/// walked out by [`reconcile_sweep_tick`]); otherwise selects the armable pid
/// (`GatewayUp`/`PidSeen` #318) and applies the delta to `scene.daemons` via the
/// pure `daemon::apply_presence`. The `ExitWatch` registration and the scene
/// publish stay in the shell.
pub(crate) fn apply_gated_presence(
    scene: &mut SceneState,
    key: &daemon::DaemonInstanceKey,
    delta: daemon::DaemonPresenceUpdate,
    connected: &ConnectedSources,
    now: SystemTime,
) -> PresenceGate {
    // The gate is SOURCE-level (one Sources-panel row per CLI), so every instance
    // of a disconnected daemon is dropped together — the instance dimension is
    // rendering identity, never a second connection axis.
    if !connected.is_connected(key.source()) {
        return PresenceGate::Dropped;
    }
    // Selected BEFORE the move into apply_presence; the shell arms after.
    let arm_pid = delta.armable_pid();
    daemon::apply_presence(scene, key, delta, now);
    PresenceGate::Applied { arm_pid }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The connection gate on HOOK transport, end to end (#735). The gate + the
    // 1-Hz reconciler are two layers of ONE contract — a disconnected source
    // renders no PERSISTENT sprite. This drives a REAL decoded hook payload
    // through the SAME `apply_gated_event`/`reconcile_sweep_tick` functions
    // `reducer_task` runs (NOT a re-implementation — the #741/#751 fix), both
    // ways.
    //
    // The two layers matter separately because they cover different events in one
    // payload. A hook `preToolUse` decodes to `[Identity{source}, ActivityStart]`
    // (#221): the per-event gate drops the source-carrying `Identity`, but the
    // bare `ActivityStart` that follows resolves to `None` in `event_source` (its
    // id has no slot yet), slips the gate once, and synthesizes a BLANK-source
    // slot. The reconciler is the documented safety net that sweeps that blank
    // slot (its `""` source is not in the connected set). The residual is bounded
    // but NOT instant: the reconcile MARKS the slot exiting (`cascade_exit`), then
    // it walks out over `EXIT_GRACE_WINDOW` (4.5s) before `sweep_exited` removes
    // it — a few-second blank `#N`, not a one-tick disappearance. Bounded to at
    // most ONE such slot per session id AT A TIME: a later event for the same id
    // coalesces onto that one slot (AgentId identity) rather than adding another.
    // Still not worth a third gating layer: an AgentId-keyed "gated ids" map with
    // its own TTL/GC, to shave a self-correcting cosmetic that rides out alongside
    // the source's real walk-out, is the defensive-arm smell the review taxonomy
    // warns against.
    //
    // `drive_hook_gate` returns (slots BEFORE the reconcile, live slots AFTER) so
    // each layer gets independent teeth: a source-carrying event (SessionStart/
    // Identity) is dropped by the GATE — 0 before reconcile — while a bare
    // activity event only clears at the RECONCILE. A single "after" count would
    // pass even with the gate removed (reconcile alone still sweeps), which is
    // exactly the gap #735 flags.
    fn drive_hook_gate(payload: &serde_json::Value, connected: &[&str]) -> (usize, usize) {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        let cs = ConnectedSources::new(connected.iter().map(|s| s.to_string()).collect());
        let mut scene = SceneState::uniform(8);
        let mut reducer = Reducer::new();
        let mut gate_logged = HashSet::new();
        let now = SystemTime::now();
        // The REAL production functions reducer_task runs — no re-implemented
        // gate/reconcile (the #741/#751 hand-copied mirror is gone).
        for ev in decode_hook_payload(payload.clone()).expect("decode hook") {
            apply_gated_event(
                &mut reducer,
                &mut scene,
                ev,
                Transport::Hook,
                &cs,
                now,
                &mut gate_logged,
            );
        }
        let before = scene.agents.len();
        reconcile_sweep_tick(&mut reducer, &mut scene, &cs, now);
        let live = scene
            .agents
            .values()
            .filter(|s| s.exiting_at.is_none())
            .count();
        (before, live)
    }

    fn cursor_hook(event: &str) -> serde_json::Value {
        // `_pixtuoid_source` is the only attribution the gate trusts — never the
        // public `source` field (which CC overloads for the SessionStart reason).
        serde_json::json!({
            "hook_event_name": event, "session_id": "c7-sess", "cwd": "",
            "conversation_id": "c7-sess", "workspace_roots": ["/x/proj"],
            "tool_name": "Shell", "tool_input": {"command": "ls"},
            "_pixtuoid_source": "cursor"
        })
    }

    #[test]
    fn the_gate_drops_a_disconnected_sources_carrying_hook_event_outright() {
        // A `sessionStart` decodes to a single source-carrying `SessionStart`, so
        // the per-event gate drops it BEFORE it ever registers — no transient at
        // all. This is the layer #735 is about: remove the gate and this reds
        // (the SessionStart registers, and only the reconcile would clean it).
        let (before, live) = drive_hook_gate(&cursor_hook("sessionStart"), &["claude-code"]);
        assert_eq!(
            before, 0,
            "the gate must drop a disconnected SessionStart pre-reconcile"
        );
        assert_eq!(live, 0, "and no sprite survives");
    }

    #[test]
    fn a_disconnected_sources_hook_activity_leaves_no_persistent_sprite() {
        // A bare activity event slips the gate once (its id has no slot, so
        // `event_source` is None) and synthesizes a transient blank slot; the
        // reconcile is the layer that sweeps it. Pins the CONTRACT (no persistent
        // sprite), not the transient count, so a future fix that closes the
        // one-slot window stays green.
        let (_before, live) = drive_hook_gate(&cursor_hook("preToolUse"), &["claude-code"]);
        assert_eq!(
            live, 0,
            "a disconnected source's hook activity must leave no live sprite after the reconcile"
        );
    }

    #[test]
    fn hook_events_for_a_connected_source_render() {
        let (_before, live) = drive_hook_gate(&cursor_hook("preToolUse"), &["cursor"]);
        assert_eq!(
            live, 1,
            "a connected source's hook activity must render exactly one live sprite"
        );
    }

    // The Connection-gate seam: `event_source` decides which source an incoming
    // event belongs to so reducer_task can drop a disconnected source's events.
    // Carrying variants (SessionStart/Identity) self-identify; everything else
    // resolves via the existing slot; an unknown id with no carried source slips
    // the gate once (None) and is swept by the per-tick reconciler.
    #[test]
    fn event_source_extracts_source_for_the_connection_gate() {
        use pixtuoid_core::state::MAX_FLOORS;
        use pixtuoid_core::AgentId;
        use std::path::PathBuf;

        use crate::runtime::FALLBACK_DESKS;

        let now = SystemTime::now();
        let mut scene = SceneState::new([FALLBACK_DESKS; MAX_FLOORS]);
        let mut reducer = Reducer::new();
        let id = AgentId::from_transcript_path("/p/a.jsonl");

        // SessionStart carries the source directly — even before the slot exists.
        let ss = AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        };
        assert_eq!(event_source(&scene, &ss), Some("claude-code"));

        // Identity likewise self-identifies.
        let idy = AgentEvent::Identity {
            agent_id: id,
            source: "codex".into(),
            session_id: "s".into(),
            cwd: None,
            pid: None,
        };
        assert_eq!(event_source(&scene, &idy), Some("codex"));

        // A non-carrying event for an UNKNOWN id slips the gate (None) — the
        // reconciler is the safety net.
        let act = AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: None,
        };
        assert_eq!(event_source(&scene, &act), None);

        // Once registered, the same event resolves via the slot's source.
        reducer.apply(&mut scene, ss, now, Transport::Jsonl);
        assert_eq!(event_source(&scene, &act), Some("claude-code"));

        // An EMPTY source on a carrying variant falls through to the slot.
        let empty = AgentEvent::Identity {
            agent_id: id,
            source: String::new(),
            session_id: "s".into(),
            cwd: None,
            pid: None,
        };
        assert_eq!(event_source(&scene, &empty), Some("claude-code"));
    }

    fn daemon_key(source: &str, instance: &str) -> daemon::DaemonInstanceKey {
        daemon::DaemonInstanceKey::new(
            source,
            pixtuoid_core::state::DaemonInstanceId::new(instance).expect("non-empty"),
        )
    }

    #[test]
    fn presence_gate_drops_a_disconnected_daemon_and_applies_a_connected_one() {
        use pixtuoid_core::source::daemon::DaemonPresenceUpdate;
        use pixtuoid_core::state::DaemonState;

        let cs = ConnectedSources::new(["openclaw".to_string()].into_iter().collect());
        let mut scene = SceneState::uniform(8);
        let now = SystemTime::now();

        // A disconnected daemon's GatewayUp is Dropped: nothing applied, nothing
        // lands in scene.daemons (mutate the gate to `if false` and this reds — the
        // presence twin of the AgentEvent gate's teeth).
        let other = daemon_key("not-connected", "18789");
        let dropped = apply_gated_presence(
            &mut scene,
            &other,
            DaemonPresenceUpdate::GatewayUp { pid: Some(4321) },
            &cs,
            now,
        );
        assert!(
            matches!(dropped, PresenceGate::Dropped),
            "a disconnected daemon's presence must be dropped"
        );
        assert!(scene.daemon(other.source(), other.instance()).is_none());

        // A connected daemon Applies (returns the armable pid) AND the delta lands
        // in scene.daemons as Up->Idle — the daemons assertion gives the
        // `daemon::apply_presence` call itself teeth (deleting it reds this), the
        // whole point of moving the seam into a covered module.
        let oc = daemon_key("openclaw", "18789");
        let applied = apply_gated_presence(
            &mut scene,
            &oc,
            DaemonPresenceUpdate::GatewayUp { pid: Some(4321) },
            &cs,
            now,
        );
        assert!(
            matches!(
                applied,
                PresenceGate::Applied {
                    arm_pid: Some(4321)
                }
            ),
            "a connected daemon's presence must apply and arm its GatewayUp pid"
        );
        assert_eq!(
            scene
                .daemon(oc.source(), oc.instance())
                .map(|p| p.display_state()),
            Some(DaemonState::Idle),
            "apply_presence must land the GatewayUp in scene.daemons"
        );
    }

    #[test]
    fn the_connection_gate_is_source_wide_across_every_instance() {
        use pixtuoid_core::source::daemon::DaemonPresenceUpdate;

        // The Sources panel has ONE openclaw row, so connecting/disconnecting is a
        // source-level decision: a second gateway of a CONNECTED source applies,
        // and every instance of a DISCONNECTED one drops.
        let cs = ConnectedSources::new(["openclaw".to_string()].into_iter().collect());
        let mut scene = SceneState::uniform(8);
        let now = SystemTime::now();
        for port in ["18789", "19789"] {
            let k = daemon_key("openclaw", port);
            assert!(matches!(
                apply_gated_presence(
                    &mut scene,
                    &k,
                    DaemonPresenceUpdate::GatewayUp { pid: None },
                    &cs,
                    now
                ),
                PresenceGate::Applied { .. }
            ));
        }
        assert_eq!(scene.daemons().count(), 2, "both gateways render");
        for port in ["18789", "19789"] {
            let k = daemon_key("other-daemon", port);
            assert!(matches!(
                apply_gated_presence(
                    &mut scene,
                    &k,
                    DaemonPresenceUpdate::GatewayUp { pid: None },
                    &cs,
                    now
                ),
                PresenceGate::Dropped
            ));
        }
        assert_eq!(
            scene.daemons().count(),
            2,
            "a disconnected source contributes no instance"
        );
    }
}
