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
//! Its plugin (`install/openclaw_plugin.js`) forwards a strict ALLOWLIST envelope
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

/// The busy pairing key: prefer a non-empty `runId`; if `runId` is ABSENT (not
/// merely empty) fall back to `sessionId`; an empty pick collapses to a constant
/// `"_"` (the `!is_empty` filter sits AFTER the `runId`-or-`sessionId` pick, so a
/// present-but-empty `runId` short-circuits to `"_"`, NOT the sessionId —
/// pinned by `run_key_fallbacks_are_coarse_by_design`). Coarse BY DESIGN: the
/// last-seen TTL decay is the real backstop, so the key only affects busy-bubble
/// intensity, never correctness.
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
    // Checked narrowing: a crafted out-of-range `_pid` (e.g. 2^32+1) must NOT
    // silently truncate to a valid pid (arming ExitWatch on PID 1) — an
    // unrepresentable pid is dropped (None); the TTL backstop still covers it.
    let pid = obj
        .get("_pid")
        .and_then(|p| p.as_i64())
        .and_then(|p| i32::try_from(p).ok());
    let mut out = match event {
        "gateway_start" => vec![DaemonPresenceUpdate::GatewayUp { pid }],
        "gateway_stop" => vec![DaemonPresenceUpdate::GatewayDown],
        "session_start" => vec![DaemonPresenceUpdate::SessionStarted],
        "session_end" => vec![DaemonPresenceUpdate::SessionEnded],
        "before_agent_run" => vec![DaemonPresenceUpdate::RunStarted {
            run_key: run_key(obj),
        }],
        "agent_end" => {
            // #317: `agent_end` carries `success: boolean` (PluginHookAgentEndEvent).
            // `false` = the run failed (the model backend is broken — auth revoked,
            // provider down) → Degraded; `true`/absent = OK → RunEnded. Absent
            // defaults to OK (an older plugin not forwarding `success` must never
            // false-degrade a healthy gateway).
            let ok = obj.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
            let run_key = run_key(obj);
            vec![if ok {
                DaemonPresenceUpdate::RunEnded { run_key }
            } else {
                DaemonPresenceUpdate::RunFailed { run_key }
            }]
        }
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
    };
    // #318: any NON-`gateway_start` event carrying `_pid` bootstraps the abrupt-
    // down exit watch for a mid-attached / reconnected daemon (`gateway_start`
    // already carries the pid via `GatewayUp`). Prepend `PidSeen` so the pid is
    // adopted before the state update applies. Skip unmapped events (empty `out`).
    if event != "gateway_start" && !out.is_empty() {
        if let Some(pid) = pid {
            out.insert(0, DaemonPresenceUpdate::PidSeen { pid });
        }
    }
    Ok(out)
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
    fn agent_end_success_false_decodes_to_run_failed() {
        // #317: a failed run (the model backend broke) → RunFailed (drives Degraded).
        assert_eq!(
            decode(
                json!({"type": "agent_end", "runId": "run_1", "sessionId": "s1", "success": false})
            ),
            vec![DaemonPresenceUpdate::RunFailed {
                run_key: "run_1".into()
            }]
        );
    }

    #[test]
    fn agent_end_success_true_or_absent_decodes_to_run_ended() {
        // Explicit success:true and a legacy plugin omitting `success` both → OK.
        for v in [
            json!({"type": "agent_end", "runId": "r", "sessionId": "s", "success": true}),
            json!({"type": "agent_end", "runId": "r", "sessionId": "s"}),
        ] {
            assert_eq!(
                decode(v),
                vec![DaemonPresenceUpdate::RunEnded {
                    run_key: "r".into()
                }],
                "success:true/absent must never false-degrade a healthy gateway"
            );
        }
    }

    #[test]
    fn non_gateway_start_event_with_pid_prepends_pid_seen() {
        // #318: a `_pid`-bearing NON-gateway_start event adopts the live pid so a
        // mid-attached daemon can arm the instant abrupt-down rung. PidSeen leads.
        assert_eq!(
            decode(json!({"type": "session_start", "sessionId": "s1", "_pid": 7777})),
            vec![
                DaemonPresenceUpdate::PidSeen { pid: 7777 },
                DaemonPresenceUpdate::SessionStarted,
            ]
        );
        assert_eq!(
            decode(json!({"type": "agent_end", "runId": "r", "_pid": 8888, "success": false})),
            vec![
                DaemonPresenceUpdate::PidSeen { pid: 8888 },
                DaemonPresenceUpdate::RunFailed {
                    run_key: "r".into()
                },
            ]
        );
    }

    #[test]
    fn gateway_start_pid_is_not_double_emitted_as_pid_seen() {
        // gateway_start already carries the pid via GatewayUp — the PidSeen prepend
        // is suppressed for it (event == "gateway_start" guard), so the pid arrives
        // exactly once.
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4242})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: Some(4242) }]
        );
    }

    #[test]
    fn unmapped_event_with_pid_emits_nothing_not_a_lone_pid_seen() {
        // An empty `out` (unmapped event) must stay empty even with `_pid` — a lone
        // PidSeen with no sibling update would never resurrect a Down daemon.
        assert!(
            decode(json!({"type": "model_call_started", "_pid": 5})).is_empty(),
            "no lone PidSeen for an unmapped event"
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
