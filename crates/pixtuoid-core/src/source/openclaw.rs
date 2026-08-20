//! OpenClaw — the first DAEMON source (`SourceKind::Daemon`), producing NO agent
//! activity: its backend coding sessions are already visualized by `cc·`. Each
//! observed gateway earns a presence mascot instead, and OpenClaw officially supports
//! several isolated gateways per host, so two live gateways render as two mascots.
//!
//! This module owns ONLY the WIRE decode, including which gateway an envelope came
//! from (its resolved `gatewayPort` → `DaemonInstanceId`); the daemon-agnostic
//! presence STATE MACHINE and decay TTLs live in [`crate::source::daemon`], keyed by
//! (source, instance) so N daemons AND N gateways coexist. Its updates ride an
//! INSTANCE-tagged SIBLING channel merged by `daemon::apply_presence` — never
//! `Reducer::apply`, which is `AgentId`-pure (invariant #2).
//!
//! Capture-grounded: tools are invisible under the `claude-cli` backend (no
//! `before_tool_call`), `before_agent_run`/`agent_end` require
//! `allowConversationAccess`, and `session_end` fires on clean close but not on
//! SIGTERM. Busy is therefore a self-healing decay keyed PER RUN, never a latch —
//! the daemon-wide `last_seen` is refreshed by any event, so keying the decay on it
//! latched Busy on a gateway that kept serving other traffic.

use std::num::NonZeroU16;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::source::daemon::{DaemonPresenceUpdate, DecodedPresence};
use crate::state::DaemonInstanceId;

/// The OpenClaw daemon source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "openclaw";

/// The wire field carrying the gateway's RESOLVED listening port — the "which
/// gateway" key: unique among *live* gateways (two can't bind one port) and STABLE
/// across a restart of the same one, so the mascot persists rather than churning.
/// Deliberately NOT the profile name (never reaches the wire, and one profile may
/// restart on a different port), the pid (changes every restart), or the session id
/// (many per gateway).
const GATEWAY_PORT_FIELD: &str = "gatewayPort";

/// Instance id for an envelope carrying NO [`GATEWAY_PORT_FIELD`] — a plugin file
/// written by a pre-multi-gateway pixtuoid still on disk (installing is a one-shot
/// `connect`; upgrading the binary does NOT re-render it). Folding such gateways into
/// ONE instance keeps their mascot behaving as before instead of vanishing on upgrade;
/// the paired `missing_field` breadcrumb is what tells the user to reconnect.
const LEGACY_INSTANCE_ID: &str = "legacy";

/// The busy pairing key: a non-empty `runId`, else `sessionId`, else `"_"`. The
/// `!is_empty` filter sits AFTER the pick, so a present-but-EMPTY `runId`
/// short-circuits to `"_"` rather than falling through to `sessionId`. Coarse BY
/// DESIGN — the last-seen TTL decay is the real backstop, so the key only affects
/// busy-bubble intensity, never correctness.
fn run_key(obj: &serde_json::Map<String, Value>) -> String {
    obj.get("runId")
        .and_then(|s| s.as_str())
        .or_else(|| obj.get("sessionId").and_then(|s| s.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or("_")
        .to_string()
}

/// Narrow the wire `gatewayPort` to this gateway's stable instance id. An ABSENT
/// field is a stale installed plugin → [`LEGACY_INSTANCE_ID`]; a PRESENT-but-unusable
/// value is a bug or a hostile sender, so the whole envelope is REJECTED rather than
/// silently bucketed — the case the compatibility fallback must never swallow.
fn gateway_instance(obj: &serde_json::Map<String, Value>, event: &str) -> Result<DaemonInstanceId> {
    let Some(raw) = obj.get(GATEWAY_PORT_FIELD) else {
        crate::source::drift::missing_field(SOURCE_NAME, event, GATEWAY_PORT_FIELD);
        return instance_id(LEGACY_INSTANCE_ID.to_string());
    };
    let port = raw
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .and_then(NonZeroU16::new)
        .ok_or_else(|| {
            // This Err is logged at the `warn` floor = RAW stderr, and serde_json's
            // Display escapes Cc but not DEL or the Cf bidi overrides.
            let raw = crate::source::decoder::display_safe(&raw.to_string());
            anyhow!("openclaw {GATEWAY_PORT_FIELD} must be a port in 1..=65535, got {raw}")
        })?;
    instance_id(port.to_string())
}

/// [`DaemonInstanceId::new`] for an id that is statically non-empty. The `None` arm
/// is unreachable, but production code must not `unwrap`, so it degrades to an error.
fn instance_id(raw: String) -> Result<DaemonInstanceId> {
    DaemonInstanceId::new(raw).ok_or_else(|| anyhow!("openclaw: blank gateway instance id"))
}

/// Decode one OpenClaw plugin envelope into the sending gateway's identity plus its
/// presence deltas. Reads ONLY allowlisted scalar fields — never
/// `messages`/`prompt`/`sessionFile`, even if the plugin regressed and forwarded them
/// (defense in depth for the privacy invariant).
pub fn decode_openclaw_hook_payload(v: &Value) -> Result<DecodedPresence> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("openclaw hook payload must be an object"))?;
    let event = obj
        .get("type")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("openclaw payload missing type"))?;
    let instance = gateway_instance(obj, event)?;
    let pid = obj
        .get("_pid")
        .and_then(|p| p.as_i64())
        .and_then(crate::source::decoder::checked_pid);
    // Presence is ANNOUNCE-only: upstream fires `gateway_start` once per gateway
    // PROCESS start, so a gateway already running when pixtuoid boots stays
    // invisible until it restarts. That is upstream's contract, not a hole to
    // "fix" with a poll. Pinned by `gateway_start_decodes_to_gateway_up_with_pid`.
    let mut out = match event {
        "gateway_start" => vec![DaemonPresenceUpdate::GatewayUp { pid }],
        "gateway_stop" => vec![DaemonPresenceUpdate::GatewayDown],
        "session_start" => vec![DaemonPresenceUpdate::SessionStarted],
        "session_end" => vec![DaemonPresenceUpdate::SessionEnded],
        "before_agent_run" => vec![DaemonPresenceUpdate::RunStarted {
            run_key: run_key(obj),
        }],
        "agent_end" => {
            // `success` alone is NOT enough for Degraded: upstream builds it as
            // `!aborted && !promptError`, so a user CANCELLING a turn produces the
            // same `false` as a provider outage — and Degraded is sticky (no TTL
            // heals it), which would latch the mascot into "model error" until the
            // next run. The plugin's `errored` (the mere PRESENCE of upstream's
            // `error`, as a bare boolean because the string can embed prompt content)
            // separates the two. Both defaults favour an older plugin forwarding
            // neither field: never false-degrade a healthy gateway, never make one
            // un-degradable.
            let ok = obj.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
            let errored = obj.get("errored").and_then(|v| v.as_bool()).unwrap_or(true);
            let run_key = run_key(obj);
            vec![if ok || !errored {
                DaemonPresenceUpdate::RunEnded { run_key }
            } else {
                DaemonPresenceUpdate::RunFailed { run_key }
            }]
        }
        // An unmapped forwarded hook is a benign skip, but it breadcrumbs at `warn`:
        // the consumers (the warn-floor log `pixtuoid doctor` scans, the counting
        // Layer) listen at warn, so a debug-level breadcrumb is invisible to them.
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            vec![]
        }
    };
    // A `_pid` on a NON-`gateway_start` event bootstraps the abrupt-down exit watch
    // for a mid-attached daemon; prepend it so the pid is adopted before the state
    // update applies. An unmapped event stays empty — a lone `PidSeen` would
    // resurrect nothing.
    if event != "gateway_start" && !out.is_empty() {
        if let Some(pid) = pid {
            out.insert(0, DaemonPresenceUpdate::PidSeen { pid });
        }
    }
    Ok(DecodedPresence {
        instance,
        updates: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_PORT: u64 = 18789;

    fn decode_full(mut v: Value) -> DecodedPresence {
        if let Some(o) = v.as_object_mut() {
            o.entry(GATEWAY_PORT_FIELD).or_insert(json!(TEST_PORT));
        }
        decode_openclaw_hook_payload(&v).expect("decodes")
    }

    fn decode(v: Value) -> Vec<DaemonPresenceUpdate> {
        decode_full(v).updates
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
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4_294_967_297i64})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
    }

    #[test]
    fn nonpositive_pid_is_dropped_like_every_sibling_pid_ingest() {
        // kill(0)/kill(-n) target process GROUPS, so a bogus pid's ESRCH receipt
        // synthesizes an instant exit that flaps the LIVE gateway Down.
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": -1})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 0})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
        assert_eq!(
            decode(json!({"type": "session_start", "sessionId": "s1", "_pid": -1})),
            vec![DaemonPresenceUpdate::SessionStarted]
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
        assert_eq!(
            decode(
                json!({"type": "agent_end", "runId": "run_1", "sessionId": "s1",
                          "success": false, "errored": true})
            ),
            vec![DaemonPresenceUpdate::RunFailed {
                run_key: "run_1".into()
            }]
        );
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
    fn a_cancelled_turn_ends_the_run_without_degrading_the_gateway() {
        assert_eq!(
            decode(
                json!({"type": "agent_end", "runId": "run_1", "sessionId": "s1",
                          "success": false, "errored": false})
            ),
            vec![DaemonPresenceUpdate::RunEnded {
                run_key: "run_1".into()
            }],
            "an abort is an ordinary end, not a degradation"
        );
    }

    #[test]
    fn agent_end_success_true_or_absent_decodes_to_run_ended() {
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
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4242})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: Some(4242) }]
        );
    }

    #[test]
    fn unmapped_event_with_pid_emits_nothing_not_a_lone_pid_seen() {
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
    fn two_gateway_ports_decode_to_two_distinct_instances() {
        let a = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 11}));
        let b = decode_full(json!({"type": "gateway_start", "gatewayPort": 19789, "_pid": 22}));
        assert_eq!(a.instance.as_str(), "18789");
        assert_eq!(b.instance.as_str(), "19789");
        assert_ne!(a.instance, b.instance);
    }

    #[test]
    fn the_same_port_after_a_restart_is_the_same_instance() {
        let first = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 11}));
        let after = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 99}));
        assert_eq!(first.instance, after.instance);
    }

    #[test]
    fn every_event_type_carries_the_gateway_identity() {
        for ty in [
            "gateway_start",
            "gateway_stop",
            "session_start",
            "session_end",
            "before_agent_run",
            "agent_end",
        ] {
            let d = decode_full(json!({"type": ty, "gatewayPort": 19789, "sessionId": "s1"}));
            assert_eq!(d.instance.as_str(), "19789", "{ty} lost its identity");
        }
    }

    #[test]
    fn an_unusable_gateway_port_rejects_the_whole_envelope() {
        for bad in [
            json!(0),
            json!(-1),
            json!(65_536),
            json!(4_294_967_297i64),
            json!("18789"),
            json!(18789.5),
            json!(null),
        ] {
            let v = json!({"type": "gateway_start", "gatewayPort": bad, "_pid": 5});
            assert!(
                decode_openclaw_hook_payload(&v).is_err(),
                "gatewayPort {bad} must be rejected"
            );
        }
    }

    #[test]
    fn a_rejected_gateway_port_is_display_safe_in_the_error() {
        let bad = json!("18\u{202e}78\u{7f}9");
        let v = json!({"type": "gateway_start", "gatewayPort": bad, "_pid": 5});
        let msg = decode_openclaw_hook_payload(&v)
            .expect_err("an unusable port is rejected")
            .to_string();
        assert!(
            !msg.contains(['\u{202e}', '\u{7f}']),
            "a hostile gatewayPort reached the terminal sink: {msg:?}"
        );
        assert!(msg.contains("must be a port in 1..=65535"), "got: {msg}");
    }

    #[test]
    fn a_port_less_envelope_falls_back_to_the_one_legacy_instance() {
        let mut decoded = Vec::new();
        let logs = crate::test_capture::capture_logs(|| {
            decoded.push(
                decode_openclaw_hook_payload(&json!({"type": "gateway_start", "_pid": 7}))
                    .expect("decodes"),
            );
            decoded.push(
                decode_openclaw_hook_payload(&json!({"type": "session_start", "sessionId": "s"}))
                    .expect("decodes"),
            );
        });
        let (a, b) = (&decoded[0], &decoded[1]);
        assert_eq!(a.instance.as_str(), LEGACY_INSTANCE_ID);
        assert_eq!(a.instance, b.instance);
        assert_eq!(
            a.updates,
            vec![DaemonPresenceUpdate::GatewayUp { pid: Some(7) }],
            "the fallback changes identity only — never the deltas"
        );
        assert!(
            logs.contains("missing_field") && logs.contains(GATEWAY_PORT_FIELD),
            "a port-less envelope must breadcrumb the MISSING FIELD class naming \
             `{GATEWAY_PORT_FIELD}`, got:\n{logs}"
        );
    }

    #[test]
    fn a_valid_port_breadcrumbs_nothing() {
        let logs = crate::test_capture::capture_logs(|| {
            decode_openclaw_hook_payload(&json!({"type": "gateway_start", "gatewayPort": 18789}))
                .expect("decodes");
        });
        assert!(
            !logs.contains("missing_field"),
            "a port-bearing envelope must be silent, got:\n{logs}"
        );
    }

    #[test]
    fn run_key_fallbacks_are_coarse_by_design() {
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
