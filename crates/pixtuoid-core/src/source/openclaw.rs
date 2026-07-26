//! OpenClaw — the first DAEMON source (`SourceKind::Daemon`), and unlike every
//! agent source it produces NO agent activity. OpenClaw (github.com/openclaw/
//! openclaw, `docs.openclaw.ai`) is one always-on gateway daemon multiplexing
//! many chat sessions; its backend coding sessions (the bundled `claude-cli`
//! backend) are already visualized by the `cc·` source at full fidelity (a real
//! `claude` writing `~/.claude/...`).
//!
//! This module owns ONLY OpenClaw's WIRE decode (`decode_openclaw_hook_payload`),
//! INCLUDING which gateway an envelope came from — its resolved
//! `gatewayPort`, normalized to a `DaemonInstanceId`. The
//! daemon-agnostic presence STATE MACHINE + lifecycle (apply/sweep/mark, the exit
//! watch, the decay TTLs) lives in the shared [`crate::source::daemon`] layer,
//! keyed by (source, instance) so N daemons AND N gateways coexist — exactly as an
//! agent source owns its own decoder but shares the reducer.
//!
//! So each observed gateway earns a presence-gated mascot (a wandering lobster)
//! showing the one thing `cc·` can't: is that gateway alive and handling
//! traffic (its motion encodes state — idle ambles, busy shuttles, down leaves).
//! OpenClaw officially supports several isolated gateways on one host (each with
//! its own profile/state dir and base port), so two running gateways render as two
//! independent lobsters. Its plugin (`install/openclaw_plugin.js`) forwards a
//! strict ALLOWLIST envelope — never message content (the busy tell needs only the
//! run pairing key) — stamped `_pixtuoid_source: "openclaw"` by the shim:
//!
//! ```json
//! {"type":"gateway_start","gatewayPort":18789,"_pid":12345}
//! {"type":"session_start","gatewayPort":18789,"sessionId":"…","sessionKey":"agent:main:…"}
//! {"type":"before_agent_run","gatewayPort":18789,"runId":"…","sessionId":"…"}
//! {"type":"agent_end","gatewayPort":18789,"runId":"…","sessionId":"…"}
//! {"type":"session_end","gatewayPort":18789,"sessionId":"…","reason":"idle","messageCount":4}
//! {"type":"gateway_stop","gatewayPort":18789,"reason":"shutdown"}
//! ```
//!
//! This decoder is PURE — `Value → DecodedPresence` (the sending gateway's
//! identity plus its deltas). The updates ride an INSTANCE-tagged SIBLING channel
//! (NOT the one `AgentEvent` channel — invariant #2), merged into
//! `SceneState::daemons` by the reducer task via `daemon::apply_presence`, NEVER
//! through `Reducer::apply` (which is `AgentId`-pure). See the design specs
//! `docs/superpowers/specs/2026-06-15-openclaw-lobster-hq-design.md` +
//! `2026-06-15-source-kind-daemon-agent-decouple-design.md`.
//!
//! Capture-grounded facts (§2 of the spec): tools are invisible under the
//! `claude-cli` backend (no `before_tool_call`), `before_agent_run`/`agent_end`
//! require `allowConversationAccess`, `session_end` fires on clean close but not
//! on SIGTERM. Busy is therefore a self-healing decay, never a latch — per RUN,
//! on that run's own last observation (the daemon-wide `last_seen` is refreshed by
//! any event, so keying the decay on it latched Busy on a gateway that kept
//! serving other traffic).

use std::num::NonZeroU16;

use anyhow::{anyhow, Result};
use serde_json::Value;

// The presence STATE MACHINE + lifecycle (apply/sweep/mark/exit-watch) and the
// decay knobs (`PresenceTtl::DEFAULT`) live in the shared, daemon-agnostic
// `crate::source::daemon` layer. This module keeps ONLY OpenClaw's wire decode.
use crate::source::daemon::{DaemonPresenceUpdate, DecodedPresence};
use crate::state::DaemonInstanceId;

/// The OpenClaw daemon source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "openclaw";

/// The wire field the plugin stamps with the gateway's RESOLVED listening port —
/// OpenClaw's own runtime identity for one gateway, and therefore ours. OpenClaw
/// officially supports several isolated gateways per host and requires each to
/// take a distinct base port, so the port is exactly the "which gateway" key: it
/// is unique among *live* gateways (two can't bind one port) and STABLE across a
/// restart of the same one (so the mascot persists rather than churning).
/// Deliberately NOT the profile name (an install scope that never reaches the
/// wire, and one profile may restart on a different port), the pid (changes every
/// restart), or the session id (many per gateway).
const GATEWAY_PORT_FIELD: &str = "gatewayPort";

/// Instance id for an envelope carrying NO [`GATEWAY_PORT_FIELD`] — i.e. a plugin
/// file written by a pre-multi-gateway pixtuoid that is still on disk (installing
/// the plugin is a one-shot `connect`; upgrading the binary does NOT re-render it).
/// Such a gateway is folded into ONE documented instance so its lobster keeps
/// behaving exactly as it did before this change instead of silently vanishing on
/// upgrade; the paired `missing_field` breadcrumb is what `pixtuoid doctor` and the
/// footer drift nudge surface, telling the user to reconnect OpenClaw. Two
/// STALE-plugin gateways still merge — precisely the old behaviour, never worse.
const LEGACY_INSTANCE_ID: &str = "legacy";

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

/// Narrow the wire `gatewayPort` to this gateway's stable instance id.
///
/// Three outcomes, deliberately distinct: a VALID port is the identity; an ABSENT
/// field is a stale installed plugin → [`LEGACY_INSTANCE_ID`] + a drift breadcrumb
/// (graceful, see that const's WHY); a PRESENT-but-unusable value (0, negative,
/// out of `u16`, non-integer) is a bug or a hostile sender, so the whole envelope
/// is REJECTED rather than silently bucketed — the untrusted-input rule, and the
/// case a compatibility fallback must never swallow.
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
            anyhow!("openclaw {GATEWAY_PORT_FIELD} must be a port in 1..=65535, got {raw}")
        })?;
    instance_id(port.to_string())
}

/// [`DaemonInstanceId::new`] for an id that is statically non-empty (a rendered
/// port, the legacy const). The `None` arm is unreachable, but production code
/// must not `unwrap`, so it degrades to an honest decode error.
fn instance_id(raw: String) -> Result<DaemonInstanceId> {
    DaemonInstanceId::new(raw).ok_or_else(|| anyhow!("openclaw: blank gateway instance id"))
}

/// Decode one OpenClaw plugin envelope into the sending gateway's identity plus
/// its presence deltas. Reads ONLY allowlisted scalar fields (`type`,
/// `gatewayPort`, `_pid`, `runId`, `sessionId`, `success`) — never
/// `messages`/`prompt`/`sessionFile`, even if the plugin regressed and forwarded
/// them (defense in depth for the §4.3 privacy invariant).
pub fn decode_openclaw_hook_payload(v: &Value) -> Result<DecodedPresence> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("openclaw hook payload must be an object"))?;
    let event = obj
        .get("type")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("openclaw payload missing type"))?;
    // Identity FIRST: an envelope we can't attribute to a gateway is worthless
    // (and a malformed port must fail the whole decode, not just one delta).
    let instance = gateway_instance(obj, event)?;
    // Checked narrowing: a crafted out-of-range `_pid` (e.g. 2^32+1) must NOT
    // silently truncate to a valid pid (arming ExitWatch on PID 1) — an
    // unrepresentable pid is dropped (None); the TTL backstop still covers it.
    // The pid narrowing (i32 range + `> 0` reject) is the shared `checked_pid`
    // guard — see its doc for why a bogus/zero pid must be dropped here.
    let pid = obj
        .get("_pid")
        .and_then(|p| p.as_i64())
        .and_then(crate::source::decoder::checked_pid);
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
        // filtered set). Emit the structured drift breadcrumb instead of
        // bailing — a NEW upstream gateway event the plugin starts forwarding
        // surfaces here in the user's own stream (defense #2), the always-on
        // backstop the `OPENCLAW_EVENTS` ⇔ decoder-arm consistency test (#3)
        // complements. `warn!` like every sibling drift site: the consumers
        // (the warn-floor log `pixtuoid doctor` scans, the counting Layer)
        // listen at warn, so a debug-level breadcrumb was invisible to them.
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
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
    Ok(DecodedPresence {
        instance,
        updates: out,
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

    /// A gateway port for the arm-mapping cases (any valid port; the identity
    /// cases below use their own literals).
    const TEST_PORT: u64 = 18789;

    /// Decode a payload, stamping the CURRENT plugin's `gatewayPort` unless the
    /// case supplies its own — so the arm-mapping tests exercise the real wire
    /// rather than the stale-plugin fallback (which has its own tests).
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
        // A crafted out-of-i32 `_pid` (2^32+1) must NOT truncate to a valid pid
        // (e.g. 1 = init) and arm ExitWatch against it — checked narrowing drops it.
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 4_294_967_297i64})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
    }

    #[test]
    fn nonpositive_pid_is_dropped_like_every_sibling_pid_ingest() {
        // kill(0)/kill(-n) target process GROUPS, and a bogus pid's ESRCH
        // receipt synthesizes an instant exit that flaps the LIVE gateway
        // Down — cc_probe/fd_probe/the hook `_pid` peek all filter `> 0`;
        // this decoder must too (the N-1-of-N guard gap).
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": -1})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
        assert_eq!(
            decode(json!({"type": "gateway_start", "_pid": 0})),
            vec![DaemonPresenceUpdate::GatewayUp { pid: None }]
        );
        // The #318 PidSeen prepend must drop it too — no adoption of -1.
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
    fn two_gateway_ports_decode_to_two_distinct_instances() {
        // THE multi-gateway premise: the port is the identity, so two live
        // gateways are two mascots. Ports are independent literals here, not
        // values recomputed from the implementation.
        let a = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 11}));
        let b = decode_full(json!({"type": "gateway_start", "gatewayPort": 19789, "_pid": 22}));
        assert_eq!(a.instance.as_str(), "18789");
        assert_eq!(b.instance.as_str(), "19789");
        assert_ne!(a.instance, b.instance);
    }

    #[test]
    fn the_same_port_after_a_restart_is_the_same_instance() {
        // Stable identity across restarts (a NEW pid, the SAME port) — the mascot
        // must persist, so the ids must compare equal.
        let first = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 11}));
        let after = decode_full(json!({"type": "gateway_start", "gatewayPort": 18789, "_pid": 99}));
        assert_eq!(first.instance, after.instance);
    }

    #[test]
    fn every_event_type_carries_the_gateway_identity() {
        // Identity rides EVERY envelope, not just `gateway_start`: pixtuoid can
        // attach mid-life (#318), and a session/run delta must reach the right
        // gateway even when the start was never observed.
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
        // Present-but-invalid is a bug or a hostile sender — never bucketed into
        // the legacy instance (that fallback is ONLY for an absent field).
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
    fn a_port_less_envelope_falls_back_to_the_one_legacy_instance() {
        // A stale installed plugin (upgrading pixtuoid does not re-render it) must
        // keep its lobster rather than silently vanish; the drift breadcrumb is
        // what tells the user to reconnect. Both events land on ONE instance —
        // exactly the pre-multi-gateway behaviour.
        let mut decoded = Vec::new();
        // The breadcrumb is HALF the contract: the fallback keeps the mascot alive,
        // the breadcrumb is what `doctor` buckets and the footer nudge reads, so
        // assert its CLASS + the field it names (every peer source pins its
        // breadcrumbs this way — codex/grok/copilot/cc/antigravity).
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
        // The negative control for the test above: the breadcrumb must fire on the
        // fallback ONLY, or every healthy gateway would look drifted in `doctor`.
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
