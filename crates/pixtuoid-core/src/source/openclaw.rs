//! OpenClaw — the first DAEMON source (`SourceKind::Daemon`), and unlike every
//! agent source it produces NO agent activity. OpenClaw (github.com/openclaw/
//! openclaw, `docs.openclaw.ai`) is one always-on gateway daemon multiplexing
//! many chat sessions; its backend coding sessions (the bundled `claude-cli`
//! backend) are already visualized by the `cc·` source at full fidelity (a real
//! `claude` writing `~/.claude/...`).
//!
//! This module owns ONLY OpenClaw's WIRE decode (`decode_openclaw_hook_payload`).
//! The daemon-agnostic presence STATE MACHINE + lifecycle (apply/sweep/mark, the
//! exit watch, the decay TTLs) lives in the shared [`crate::source::daemon`]
//! layer, keyed by source name so N daemons coexist — exactly as an agent source
//! owns its own decoder but shares the reducer.
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
//! source-tagged SIBLING channel (NOT the one `AgentEvent` channel — invariant
//! #2), merged into `SceneState::daemons` by the reducer task via
//! `daemon::apply_presence`, NEVER through `Reducer::apply` (which is
//! `AgentId`-pure). See the design specs
//! `docs/superpowers/specs/2026-06-15-openclaw-lobster-hq-design.md` +
//! `2026-06-15-source-kind-daemon-agent-decouple-design.md`.
//!
//! Capture-grounded facts (§2 of the spec): tools are invisible under the
//! `claude-cli` backend (no `before_tool_call`), `before_agent_run`/`agent_end`
//! require `allowConversationAccess`, `session_end` fires on clean close but not
//! on SIGTERM. Busy is therefore a SELF-HEALING last-seen decay, never a latch.

use anyhow::{anyhow, Result};
use serde_json::Value;

// The presence STATE MACHINE + lifecycle (apply/sweep/mark/exit-watch) and the
// decay knobs (`PresenceTtl::DEFAULT`) live in the shared, daemon-agnostic
// `crate::source::daemon` layer. This module keeps ONLY OpenClaw's wire decode.
use crate::source::daemon::DaemonPresenceUpdate;

pub const SOURCE_NAME: &str = "openclaw";

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
            // Checked narrowing: a crafted out-of-range `_pid` (e.g. 2^32+1) must
            // NOT silently truncate to a valid pid (arming ExitWatch on PID 1) — an
            // unrepresentable pid is dropped (None); the TTL backstop still covers it.
            pid: obj
                .get("_pid")
                .and_then(|p| p.as_i64())
                .and_then(|p| i32::try_from(p).ok()),
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

// `decode_openclaw_hook_custom` was DELETED: with `SourceKind::Daemon`,
// `decode_hook_payload` short-circuits `is_daemon()` → `Ok(vec![])`, so the
// "claim every event, emit zero AgentEvents" shim is no longer needed — the kind
// makes it implicit. OpenClaw's presence rides the sibling channel via
// `decode_openclaw_hook_payload` (the registry `Daemon { presence_decoder }`).

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
    fn gateway_start_out_of_range_pid_is_dropped_not_truncated() {
        // A crafted out-of-i32 `_pid` (2^32+1) must NOT truncate to a valid pid
        // (e.g. 1 = init) and arm ExitWatch against it — checked narrowing drops it.
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4_294_967_297i64})),
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
}
