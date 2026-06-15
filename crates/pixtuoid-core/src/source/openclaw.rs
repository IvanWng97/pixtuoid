//! OpenClaw source — HOOK-ONLY, and unlike every other source it produces NO
//! agent activity. OpenClaw (github.com/openclaw/openclaw, `docs.openclaw.ai`)
//! is one always-on gateway DAEMON multiplexing many chat sessions; its backend
//! coding sessions (the bundled `claude-cli` backend) are already visualized by
//! the `cc·` source at full fidelity (a real `claude` writing `~/.claude/...`).
//!
//! So OpenClaw earns a SINGLE presence-gated mascot (the wandering "Molty"
//! lobster) showing the one thing `cc·` can't: is the gateway alive and handling
//! traffic (its motion encodes state — idle ambles, busy shuttles, down leaves).
//! Its plugin (`install/openclaw_plugin.ts`) forwards a strict ALLOWLIST envelope
//! — never message content (the busy tell needs only the run pairing key) —
//! stamped `_pixtuoid_source: "openclaw"` by the shim:
//!
//! ```json
//! {"type":"gateway_start","_pid":12345}
//! {"type":"session_start","sessionId":"…","sessionKey":"agent:main:…"}
//! {"type":"before_agent_run","runId":"…","sessionId":"…"}
//! {"type":"agent_end","runId":"…","sessionId":"…"}
//! {"type":"session_end","sessionId":"…","reason":"idle","messageCount":4}
//! {"type":"gateway_stop","reason":"shutdown"}
//! ```
//!
//! This decoder is PURE (`Value → Vec<DaemonPresenceUpdate>`). The updates ride a
//! `SourceDeath`-style SIBLING channel (NOT the one `AgentEvent` channel —
//! invariant #2), merged into `SceneState::source_presence` by the reducer task,
//! NEVER through `Reducer::apply` (which is `AgentId`-pure). See the design spec
//! `docs/superpowers/specs/2026-06-15-openclaw-lobster-hq-design.md`.
//!
//! Capture-grounded facts (§2 of the spec): tools are invisible under the
//! `claude-cli` backend (no `before_tool_call`), `before_agent_run`/`agent_end`
//! require `allowConversationAccess`, `session_end` fires on clean close but not
//! on SIGTERM. Busy is therefore a SELF-HEALING last-seen decay, never a latch.

use std::time::SystemTime;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::source::AgentEvent;
use crate::state::{DaemonState, SceneState};

pub const SOURCE_NAME: &str = "openclaw";

/// Grace window for busy → idle decay when no `before_agent_run`/`agent_end`
/// arrives (a dropped `agent_end` must self-heal, never strand perpetual busy).
pub const BUSY_DECAY_MS: u64 = 30_000;

/// Presence-TTL: with no activity for this long the daemon is presumed DOWN.
/// Covers SIGTERM, where neither `session_end` nor `gateway_stop` fires (§2.4).
pub const PRESENCE_TTL_MS: u64 = 5 * 60 * 1_000;

/// How long a `Down` presence lingers before it is REMOVED (back to absent).
/// Generously past the renderer's ~2.2s elevator walk-out so the leave animation
/// always completes first, then the entry is GC'd (a permanent Down entry would
/// otherwise leak in `source_presence` forever).
pub const DOWN_REMOVE_MS: u64 = 5_000;

/// One presence delta for the gateway mascot. The decoder emits the hook-derived
/// variants; `PidExited` is emitted by the `ExitWatch` drain task (the reducer
/// wiring), never by this decoder. All consumed by `apply_presence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonPresenceUpdate {
    /// `gateway_start` — the daemon is up; `pid` (its `process.pid`) is armed
    /// for `ExitWatch`. UP-winning + idempotent; resets the session count.
    GatewayUp { pid: Option<i32> },
    /// `gateway_stop` — clean shutdown.
    GatewayDown,
    /// `session_start` — a multiplexed session began (bumps the bubble count).
    SessionStarted,
    /// `session_end` — a session ended.
    SessionEnded,
    /// `before_agent_run` — a turn entered flight, keyed for self-healing busy.
    RunStarted { run_key: String },
    /// `agent_end` — a turn completed (best-effort; the TTL decay is the backstop).
    RunEnded { run_key: String },
    /// The armed gateway pid died (from the `ExitWatch` drain, not this decoder).
    PidExited { pid: i32 },
}

/// The busy pairing key: prefer the turn's `runId`, fall back to its `sessionId`,
/// else a constant. The last-seen TTL decay is the real backstop, so a coarse
/// key only affects active-session intensity, never correctness.
fn run_key(obj: &serde_json::Map<String, Value>) -> String {
    obj.get("runId")
        .and_then(|s| s.as_str())
        .or_else(|| obj.get("sessionId").and_then(|s| s.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string()
}

/// Decode one OpenClaw plugin envelope into presence deltas. Reads ONLY
/// allowlisted scalar fields (`type`, `_pid`, `runId`, `sessionId`) — never
/// `messages`/`prompt`/`sessionFile`, even if the plugin regressed and forwarded
/// them (defense in depth for the §4.3 privacy invariant).
pub fn decode_openclaw_hook_payload(v: &Value) -> Result<Vec<DaemonPresenceUpdate>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("openclaw hook payload must be an object"))?;
    let event = obj
        .get("type")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("openclaw payload missing type"))?;
    Ok(match event {
        "gateway_start" => vec![DaemonPresenceUpdate::GatewayUp {
            pid: obj.get("_pid").and_then(|p| p.as_i64()).map(|p| p as i32),
        }],
        "gateway_stop" => vec![DaemonPresenceUpdate::GatewayDown],
        "session_start" => vec![DaemonPresenceUpdate::SessionStarted],
        "session_end" => vec![DaemonPresenceUpdate::SessionEnded],
        "before_agent_run" => vec![DaemonPresenceUpdate::RunStarted {
            run_key: run_key(obj),
        }],
        "agent_end" => vec![DaemonPresenceUpdate::RunEnded {
            run_key: run_key(obj),
        }],
        // Any other forwarded hook is a benign skip (the plugin forwards a
        // filtered set). Log a drift breadcrumb instead of bailing — a NEW
        // upstream gateway event the plugin starts forwarding surfaces here in
        // the user's own stream (defense #2), the always-on backstop the
        // `OPENCLAW_EVENTS` ⇔ decoder-arm consistency test (#3) complements.
        other => {
            tracing::debug!(
                target: "pixtuoid::drift",
                event = other,
                "unhandled openclaw gateway hook event (upstream may have added one)"
            );
            vec![]
        }
    })
}

/// The registry's `hook.custom` entry point. OpenClaw's envelope is ALIEN (no CC
/// `hook_event_name`/`session_id`), so it CLAIMS every event — but yields ZERO
/// `AgentEvent`s: presence rides the sibling channel via
/// `decode_openclaw_hook_payload`. Returning `Ok(Some(vec![]))` (never `Ok(None)`,
/// which would fall through to the shared CC-shaped arms under a divergent
/// `AgentId`) is the claim. The registry's `presence_only: true` flag tells the
/// conformance harness this emptiness is by design.
pub(crate) fn decode_openclaw_hook_custom(_v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    Ok(Some(vec![]))
}

/// Merge one presence delta into `scene.source_presence` (keyed on the OpenClaw
/// source). Called by the reducer task off the SIBLING channel — NEVER through
/// `Reducer::apply` (which is `AgentId`-pure). Every update refreshes `last_seen`
/// (any event is proof of life) and "any event implies UP" resurrects a wrongly
/// DOWN daemon. See the design spec §4.2.
pub fn apply_presence(scene: &mut SceneState, update: DaemonPresenceUpdate, now: SystemTime) {
    use DaemonPresenceUpdate::*;
    let p = scene
        .source_presence
        .entry(SOURCE_NAME.to_string())
        .or_insert_with(|| crate::state::DaemonPresence {
            state: DaemonState::Idle,
            active_sessions: 0,
            last_seen: now,
            entered_at: now,
            in_flight_run_keys: Default::default(),
            current_pid: None,
        });
    // A transition out of Down (or a fresh GatewayUp) re-anchors the enter
    // animation — the mascot scuttles back in from the elevator. Idle↔Busy
    // does NOT reset it, so the steady wander clock stays continuous.
    let was_down = p.state == DaemonState::Down;
    p.last_seen = now;
    match update {
        // UP-winning + idempotent. A (re)start resets the multiplexed-session
        // count + in-flight runs and rebinds the armed pid — so a later stale
        // `PidExited` for the OLD pid is ignored (restart rebind, §4.2).
        GatewayUp { pid } => {
            p.current_pid = pid;
            p.active_sessions = 0;
            p.in_flight_run_keys.clear();
            p.state = DaemonState::Idle;
        }
        GatewayDown => {
            p.state = DaemonState::Down;
            p.active_sessions = 0;
            p.in_flight_run_keys.clear();
        }
        SessionStarted => {
            p.active_sessions = p.active_sessions.saturating_add(1);
            if p.state == DaemonState::Down {
                p.state = DaemonState::Idle; // any event ⇒ up
            }
        }
        SessionEnded => {
            // saturating: a pre-attach session_start we never saw must not underflow.
            p.active_sessions = p.active_sessions.saturating_sub(1);
            if p.state == DaemonState::Down {
                p.state = DaemonState::Idle;
            }
        }
        RunStarted { run_key } => {
            p.in_flight_run_keys.insert(run_key);
            p.state = DaemonState::Busy;
        }
        RunEnded { run_key } => {
            p.in_flight_run_keys.remove(&run_key);
            if p.in_flight_run_keys.is_empty() {
                p.state = DaemonState::Idle;
            }
        }
        // Only the CURRENTLY-armed pid dying takes the daemon down. A stale
        // `PidExited` for an old pid after a restart (`current_pid` already
        // rebound to the new pid) is a no-op — the live daemon stays up.
        PidExited { pid } => {
            if p.current_pid == Some(pid) {
                p.state = DaemonState::Down;
                p.active_sessions = 0;
                p.in_flight_run_keys.clear();
            }
        }
    }
    // Re-anchor the enter animation on a Down → up resurrection (the entry was
    // not yet TTL-swept). A fresh insert already stamped `entered_at = now`.
    if was_down && p.state != DaemonState::Down {
        p.entered_at = now;
    }
}

/// Decay stale presence on the reducer's sweep tick (a daemon has no per-session
/// pid, so silence is the only abrupt-exit signal): BUSY → IDLE after
/// `BUSY_DECAY_MS` of silence (a dropped `agent_end` self-heals — never a latch),
/// and any live state → DOWN after `PRESENCE_TTL_MS` (covers SIGTERM, where
/// neither `session_end` nor `gateway_stop` fires). v1: openclaw is the only
/// daemon source, so the single TTL pair applies to every presence entry.
pub fn sweep_presence_ttl(scene: &mut SceneState, now: SystemTime) {
    scene.source_presence.retain(|_, p| {
        let idle_ms = now
            .duration_since(p.last_seen)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if p.state == DaemonState::Down {
            // Keep the Down entry only until the walk-out has had time to finish,
            // then drop it (back to absent) so it doesn't leak forever.
            return idle_ms < DOWN_REMOVE_MS;
        }
        if idle_ms >= PRESENCE_TTL_MS {
            p.state = DaemonState::Down;
            // Re-anchor `last_seen` to NOW so the renderer's `now - last_seen`
            // walk-out timer starts at 0 and the mascot plays the elevator
            // leave (without this the entry is ≥TTL stale → it vanishes with no
            // walk-out). Mirrors the explicit GatewayDown/PidExited paths.
            p.last_seen = now;
            p.active_sessions = 0;
            p.in_flight_run_keys.clear();
        } else if p.state == DaemonState::Busy && idle_ms >= BUSY_DECAY_MS {
            p.state = DaemonState::Idle;
            p.in_flight_run_keys.clear();
        }
        true
    });
}

/// Drive a source's presence to `Down` (arming the renderer's walk-out) iff it
/// exists and is not already Down — idempotent, so the `DOWN_REMOVE_MS` removal
/// timer in `sweep_presence_ttl` isn't reset on every tick. The runtime calls
/// this to walk the mascot out when its source is DISCONNECTED in the Connection
/// panel: the presence side-channel is separate from the `AgentEvent` connection
/// gate, so a disconnect must reconcile presence too (mirrors the reducer's
/// `reconcile_connected` for agents).
pub fn mark_presence_down(scene: &mut SceneState, source: &str, now: SystemTime) {
    if let Some(p) = scene.source_presence.get_mut(source) {
        if p.state != DaemonState::Down {
            p.state = DaemonState::Down;
            p.last_seen = now;
            p.active_sessions = 0;
            p.in_flight_run_keys.clear();
        }
    }
}

/// A handle to arm gateway-pid exit watches. A dying gateway pid converts to a
/// `PidExited` presence delta — the instant abrupt-down rung (§4.2), reusing the
/// AGNOSTIC `ExitWatch` (pid → channel, no `AgentId` coupling), NOT `HookPidWatch`
/// (which emits an AgentSlot-shaped `SessionEnd` the non-slot mascot can't consume).
pub struct PresenceExitWatch(crate::source::exit_watch::ExitWatch);

impl PresenceExitWatch {
    /// Watch a gateway pid; its death emits `PidExited`. Idempotent per pid.
    pub fn watch(&self, pid: i32) {
        self.0.watch(pid);
    }
}

/// Spawn the gateway-pid exit watcher: pid deaths drain into `PidExited` on
/// `presence_tx`. `None` where the platform has no exit-watch backend (then the
/// `PRESENCE_TTL_MS` sweep is the only abrupt-down signal). Call in a tokio runtime.
pub fn spawn_presence_exit_watch(
    presence_tx: crate::source::hook::PresenceSender,
) -> Option<PresenceExitWatch> {
    let (pid_tx, mut pid_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
    let watch = crate::source::exit_watch::ExitWatch::spawn(pid_tx)?;
    tokio::spawn(async move {
        while let Some(pid) = pid_rx.recv().await {
            if presence_tx
                .send(DaemonPresenceUpdate::PidExited { pid })
                .is_err()
            {
                break;
            }
        }
    });
    Some(PresenceExitWatch(watch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(v: Value) -> Vec<DaemonPresenceUpdate> {
        decode_openclaw_hook_payload(&v).expect("decodes")
    }

    #[test]
    fn gateway_start_decodes_to_gateway_up_with_pid() {
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4242})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: Some(4242) }]
        );
    }

    #[test]
    fn gateway_start_without_pid_is_gateway_up_none() {
        assert_eq!(
            decode(json!({"type": "gateway_start"})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
    }

    #[test]
    fn gateway_stop_decodes_to_gateway_down() {
        assert_eq!(
            decode(json!({"type": "gateway_stop", "reason": "shutdown"})),
            vec![DaemonPresenceUpdate::GatewayDown]
        );
    }

    #[test]
    fn session_start_and_end_count_sessions() {
        assert_eq!(
            decode(json!({"type": "session_start", "sessionId": "s1", "sessionKey": "k1"})),
            vec![DaemonPresenceUpdate::SessionStarted]
        );
        assert_eq!(
            decode(
                json!({"type": "session_end", "sessionId": "s1", "reason": "idle", "messageCount": 4})
            ),
            vec![DaemonPresenceUpdate::SessionEnded]
        );
    }

    #[test]
    fn before_agent_run_and_agent_end_pair_on_runid() {
        assert_eq!(
            decode(json!({"type": "before_agent_run", "runId": "run_1", "sessionId": "s1"})),
            vec![DaemonPresenceUpdate::RunStarted {
                run_key: "run_1".into()
            }]
        );
        assert_eq!(
            decode(json!({"type": "agent_end", "runId": "run_1", "sessionId": "s1"})),
            vec![DaemonPresenceUpdate::RunEnded {
                run_key: "run_1".into()
            }]
        );
    }

    #[test]
    fn run_without_runid_falls_back_to_session_key() {
        assert_eq!(
            decode(json!({"type": "before_agent_run", "sessionId": "s9"})),
            vec![DaemonPresenceUpdate::RunStarted {
                run_key: "s9".into()
            }]
        );
    }

    #[test]
    fn message_content_and_session_file_never_reach_the_updates() {
        // Defense in depth: even if the plugin regressed and forwarded content,
        // the decoder reads only allowlisted scalars, so no secret/path leaks.
        let updates = decode(json!({
            "type": "agent_end",
            "runId": "run_1",
            "sessionId": "s1",
            "messages": [{"role": "assistant", "content": "SECRET_TEXT"}],
            "sessionFile": "/Users/x/.openclaw/agents/main/sessions/SECRET_PATH.jsonl",
            "prompt": "SECRET_PROMPT"
        }));
        let dbg = format!("{updates:?}");
        assert!(
            !dbg.contains("SECRET"),
            "no message/path content may leak: {dbg}"
        );
        assert_eq!(
            updates,
            vec![DaemonPresenceUpdate::RunEnded {
                run_key: "run_1".into()
            }]
        );
    }

    #[test]
    fn unmapped_event_types_are_skipped_not_errored() {
        for ty in [
            "heartbeat_prompt_contribution",
            "model_call_started",
            "after_tool_call",
            "before_compaction",
            "message_received",
        ] {
            assert!(
                decode(json!({"type": ty})).is_empty(),
                "{ty} must skip, not error"
            );
        }
    }

    #[test]
    fn malformed_payloads_are_errors_not_panics() {
        assert!(decode_openclaw_hook_payload(&json!("a string")).is_err());
        assert!(decode_openclaw_hook_payload(&json!(42)).is_err());
        assert!(
            decode_openclaw_hook_payload(&json!({"_pid": 1})).is_err(),
            "missing type"
        );
    }

    // ── presence state machine (apply_presence + sweep_presence_ttl) ──

    fn ms(m: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(m)
    }
    fn st(s: &SceneState) -> DaemonState {
        s.source_presence[SOURCE_NAME].state
    }
    fn sessions(s: &SceneState) -> u32 {
        s.source_presence[SOURCE_NAME].active_sessions
    }
    fn entered_at(s: &SceneState) -> SystemTime {
        s.source_presence[SOURCE_NAME].entered_at
    }
    fn last_seen(s: &SceneState) -> SystemTime {
        s.source_presence[SOURCE_NAME].last_seen
    }
    fn up(s: &mut SceneState, pid: i32, at: u64) {
        apply_presence(
            s,
            DaemonPresenceUpdate::GatewayUp { pid: Some(pid) },
            ms(at),
        );
    }

    #[test]
    fn gateway_up_sets_idle_and_records_pid() {
        let mut s = SceneState::default();
        up(&mut s, 4242, 0);
        assert_eq!(st(&s), DaemonState::Idle);
        assert_eq!(s.source_presence[SOURCE_NAME].current_pid, Some(4242));
    }

    #[test]
    fn gateway_up_resets_sessions_and_in_flight_runs() {
        let mut s = SceneState::default();
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(0));
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(1));
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            ms(2),
        );
        assert_eq!(st(&s), DaemonState::Busy);
        up(&mut s, 1, 3);
        assert_eq!(st(&s), DaemonState::Idle);
        assert_eq!(sessions(&s), 0);
        assert!(s.source_presence[SOURCE_NAME].in_flight_run_keys.is_empty());
    }

    #[test]
    fn gateway_down_sets_down() {
        let mut s = SceneState::default();
        up(&mut s, 1, 0);
        apply_presence(&mut s, DaemonPresenceUpdate::GatewayDown, ms(1));
        assert_eq!(st(&s), DaemonState::Down);
    }

    #[test]
    fn session_count_increments_and_saturates_at_zero() {
        let mut s = SceneState::default();
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(0));
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(1));
        assert_eq!(sessions(&s), 2);
        for i in 0..3 {
            apply_presence(&mut s, DaemonPresenceUpdate::SessionEnded, ms(2 + i));
        }
        assert_eq!(
            sessions(&s),
            0,
            "saturating — a pre-attach miss never underflows"
        );
    }

    #[test]
    fn busy_holds_until_the_last_run_ends() {
        let mut s = SceneState::default();
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "a".into(),
            },
            ms(0),
        );
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "b".into(),
            },
            ms(1),
        );
        assert_eq!(st(&s), DaemonState::Busy);
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunEnded {
                run_key: "a".into(),
            },
            ms(2),
        );
        assert_eq!(st(&s), DaemonState::Busy, "b still in flight");
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunEnded {
                run_key: "b".into(),
            },
            ms(3),
        );
        assert_eq!(st(&s), DaemonState::Idle);
    }

    #[test]
    fn pid_exit_matching_current_takes_the_daemon_down() {
        let mut s = SceneState::default();
        up(&mut s, 7, 0);
        apply_presence(&mut s, DaemonPresenceUpdate::PidExited { pid: 7 }, ms(1));
        assert_eq!(st(&s), DaemonState::Down);
    }

    #[test]
    fn stale_pid_exit_after_restart_leaves_the_daemon_up() {
        // Gateway restart P1→P2; a late NOTE_EXIT for P1 must NOT stomp P2 down.
        let mut s = SceneState::default();
        up(&mut s, 1, 0);
        up(&mut s, 2, 1);
        apply_presence(&mut s, DaemonPresenceUpdate::PidExited { pid: 1 }, ms(2));
        assert_eq!(
            st(&s),
            DaemonState::Idle,
            "P2 stays up; stale P1 exit ignored"
        );
        assert_eq!(s.source_presence[SOURCE_NAME].current_pid, Some(2));
    }

    #[test]
    fn any_event_resurrects_from_down() {
        let mut s = SceneState::default();
        apply_presence(&mut s, DaemonPresenceUpdate::GatewayDown, ms(0));
        assert_eq!(st(&s), DaemonState::Down);
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(1));
        assert_eq!(st(&s), DaemonState::Idle, "any presence event implies up");
    }

    #[test]
    fn entered_at_reanchors_on_resurrection_but_not_on_idle_busy() {
        // The renderer keys the walk-in on `now - entered_at`, so a Down→up
        // resurrection MUST re-anchor entered_at (Molty scuttles back in), while
        // an Idle↔Busy transition must NOT (the steady wander clock stays
        // continuous). This pins both sides of that load-bearing seam.
        let mut s = SceneState::default();
        up(&mut s, 1, 0);
        assert_eq!(entered_at(&s), ms(0));
        // Idle → Busy → Idle keeps the original entry time.
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            ms(2000),
        );
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunEnded {
                run_key: "r".into(),
            },
            ms(3000),
        );
        assert_eq!(entered_at(&s), ms(0), "idle↔busy must not move entered_at");
        // Down, then a resurrecting event re-anchors to the resurrection time.
        apply_presence(&mut s, DaemonPresenceUpdate::GatewayDown, ms(4000));
        apply_presence(&mut s, DaemonPresenceUpdate::SessionStarted, ms(9000));
        assert_eq!(st(&s), DaemonState::Idle);
        assert_eq!(
            entered_at(&s),
            ms(9000),
            "resurrection re-anchors the walk-in"
        );
    }

    #[test]
    fn run_key_fallbacks_are_coarse_by_design() {
        // Coarse key by design (only affects busy intensity, never correctness).
        // Pin the actual behavior so a regression is caught: no runId AND no
        // sessionId ⇒ "_". And an EMPTY runId short-circuits to "_" — it does NOT
        // fall through to sessionId, because the `!is_empty()` filter sits AFTER
        // the runId-or-sessionId pick (any empty pick ⇒ "_"). Coarse but
        // correctness-irrelevant; a colliding key self-heals via the sweep.
        assert_eq!(
            decode(json!({"type": "before_agent_run"})),
            vec![DaemonPresenceUpdate::RunStarted {
                run_key: "_".into()
            }]
        );
        assert_eq!(
            decode(json!({"type": "before_agent_run", "runId": "", "sessionId": "s5"})),
            vec![DaemonPresenceUpdate::RunStarted {
                run_key: "_".into()
            }],
            "an empty runId short-circuits to \"_\" (filter is after the or)"
        );
    }

    #[test]
    fn mark_presence_down_arms_the_walkout_idempotently() {
        // The Connection-panel disconnect path: drive a live presence to Down
        // (re-stamping last_seen so the renderer plays the walk-out), idempotent
        // so the DOWN_REMOVE_MS timer isn't reset every sweep tick.
        let mut s = SceneState::default();
        up(&mut s, 1, 0);
        mark_presence_down(&mut s, SOURCE_NAME, ms(1000));
        assert_eq!(st(&s), DaemonState::Down);
        assert_eq!(
            last_seen(&s),
            ms(1000),
            "Down re-anchors last_seen for the walk-out"
        );
        // A second call while already Down is a no-op (last_seen NOT re-stamped).
        mark_presence_down(&mut s, SOURCE_NAME, ms(5000));
        assert_eq!(
            last_seen(&s),
            ms(1000),
            "idempotent: already-Down is untouched"
        );
        // Unknown source is a no-op (no panic / no phantom entry).
        mark_presence_down(&mut s, "not-a-source", ms(6000));
        assert_eq!(s.source_presence.len(), 1);
    }

    #[test]
    fn sweep_takes_the_daemon_down_after_presence_ttl() {
        let mut s = SceneState::default();
        up(&mut s, 1, 0);
        sweep_presence_ttl(&mut s, ms(PRESENCE_TTL_MS + 1));
        assert_eq!(
            st(&s),
            DaemonState::Down,
            "silence past the TTL ⇒ down (covers SIGTERM)"
        );
        assert_eq!(sessions(&s), 0);
        // L1: the TTL→Down transition re-anchors last_seen to NOW so the renderer
        // (`now - last_seen`) plays the elevator walk-out instead of the mascot
        // vanishing instantly (last_seen would otherwise be ≥TTL stale).
        assert_eq!(
            last_seen(&s),
            ms(PRESENCE_TTL_MS + 1),
            "walk-out anchor re-stamped"
        );
    }

    #[test]
    fn sweep_removes_a_down_entry_after_the_walkout_window() {
        // A Down presence lingers (drawn walking out) until DOWN_REMOVE_MS, then
        // is GC'd back to absent — no permanent leak in source_presence.
        let mut s = SceneState::default();
        apply_presence(&mut s, DaemonPresenceUpdate::GatewayDown, ms(0));
        sweep_presence_ttl(&mut s, ms(DOWN_REMOVE_MS - 1));
        assert!(
            s.source_presence.contains_key(SOURCE_NAME),
            "still present mid walk-out"
        );
        sweep_presence_ttl(&mut s, ms(DOWN_REMOVE_MS + 1));
        assert!(
            !s.source_presence.contains_key(SOURCE_NAME),
            "removed once the walk-out window has elapsed"
        );
    }

    #[test]
    fn sweep_self_heals_a_stranded_busy_after_the_grace_window() {
        // A dropped agent_end strands a run key in flight; the decay clears it.
        let mut s = SceneState::default();
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "stranded".into(),
            },
            ms(0),
        );
        assert_eq!(st(&s), DaemonState::Busy);
        sweep_presence_ttl(&mut s, ms(BUSY_DECAY_MS + 1));
        assert_eq!(
            st(&s),
            DaemonState::Idle,
            "stranded busy self-heals to idle"
        );
        assert!(s.source_presence[SOURCE_NAME].in_flight_run_keys.is_empty());
    }

    #[test]
    fn sweep_within_the_grace_window_keeps_busy() {
        let mut s = SceneState::default();
        apply_presence(
            &mut s,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            ms(0),
        );
        sweep_presence_ttl(&mut s, ms(BUSY_DECAY_MS - 1));
        assert_eq!(st(&s), DaemonState::Busy, "still within the decay grace");
    }
}
