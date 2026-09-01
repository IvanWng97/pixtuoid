//! DeepSeek Harness (`dsh`) source. Hook-only: the transcript is
//! zstd-compressed (`session.jsonl.zstd`, concatenated frames — not
//! line-readable), so the ONLY plane is a pixtuoid-owned cordis plugin
//! (`pixtuoid/src/install/dsh_plugin.mjs`) mounted through the home-level
//! `$DSH_HOME/cordis.patch.yml`, forwarding emit-only events (never a
//! waterfall/serial listener, so it structurally cannot block dsh) through the
//! shim. Wire shape: upstream `packages/core/session/src/types.ts` (the
//! `session/event` feed), `packages/interaction/user-approval/src/types.ts`
//! (the log-only approval audit pair), `packages/core/agent/src/runtime-types.ts`
//! (#928). One dsh process hosts MANY sessions (the `web` profile is a
//! server), so `sessionId` is the identity and `_pid` only drives liveness.

use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// Stable source id; MUST equal the registry row's `name`.
pub const SOURCE_NAME: &str = "dsh";

/// The payload types the plugin sends — one per subscribed upstream event.
/// The plugin owns the upstream-name → wire-name mapping; the decoder owns
/// nothing upstream-shaped, so an upstream rename lands in ONE file.
const SESSION_START: &str = "session_start";
const SESSION_END: &str = "session_end";
const TOOL_CALL: &str = "tool_call";
const TOOL_RESULT: &str = "tool_result";
const APPROVAL_ASKED: &str = "approval_asked";
const APPROVAL_DECIDED: &str = "approval_decided";
const MODEL: &str = "model";
const USAGE: &str = "usage";

/// Upstream's fail-closed grant is exactly `allowed-once`
/// (`ApprovalOutcome`); every other outcome leaves the call unrun.
const OUTCOME_ALLOWED: &str = "allowed-once";

fn field<'v>(obj: &'v serde_json::Map<String, Value>, key: &str) -> Option<&'v str> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Decode one plugin payload into `AgentEvent`s (`HookCustom::ClaimsAll`).
///
/// Every arm leads or trails an [`AgentEvent::identity`]: activity arms lead
/// (the hook-family pattern — the event may target a slot registered before
/// this daemon attached), session arms trail (the focus-pid carrier, armed
/// from birth — the omp-reviewed shape). The approval pair maps onto the
/// reducer's gated-wait mechanics keyed by `callId`: `asked` opens the wait
/// naming the call it gates, `allowed-once` resumes it, anything else ends it.
pub fn decode_dsh_payload(v: &Value) -> anyhow::Result<Vec<AgentEvent>> {
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let Some(ty) = obj.get("type").and_then(|t| t.as_str()) else {
        crate::source::drift::missing_field(SOURCE_NAME, "plugin", "type");
        return Ok(vec![]);
    };
    let Some(session_id) = field(obj, "sessionId") else {
        crate::source::drift::missing_field(SOURCE_NAME, ty, "sessionId");
        return Ok(vec![]);
    };
    let agent_id = AgentId::from_parts(SOURCE_NAME, session_id);
    let parent_id = field(obj, "parentSession").map(|p| AgentId::from_parts(SOURCE_NAME, p));
    let cwd = || {
        obj.get("cwd")
            .and_then(|c| c.as_str())
            .map(std::path::PathBuf::from)
    };
    let identity = || AgentEvent::identity(agent_id, SOURCE_NAME, session_id.to_string(), cwd());
    let call_id = || field(obj, "callId").map(str::to_string);
    let tool_display = || {
        field(obj, "toolName")
            .map(|t| ellipsize(t, MAX_DECODED_FIELD_CHARS))
            .unwrap_or_default()
    };

    match ty {
        SESSION_START => Ok(vec![
            AgentEvent::SessionStart {
                agent_id,
                source: SOURCE_NAME.to_string(),
                session_id: session_id.to_string(),
                cwd: cwd().unwrap_or_default(),
                parent_id,
            },
            identity(),
        ]),
        SESSION_END => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: parent_id.is_some(),
        }]),
        TOOL_CALL => Ok(vec![
            identity(),
            AgentEvent::ActivityStart {
                agent_id,
                tool_use_id: call_id(),
                // Delegation classification is deliberately absent: dsh's
                // dispatch tool name is unverified upstream, and a wrong
                // `Task` here suppresses the parent's real activity. Settled
                // at first capture (`TOOL_ID_KEY_UNPROVEN` sibling note).
                detail: Some(ToolDetail::Generic {
                    display: tool_display(),
                }),
            },
        ]),
        TOOL_RESULT => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: call_id(),
            },
        ]),
        APPROVAL_ASKED => {
            let reason = match (field(obj, "toolName"), field(obj, "reason")) {
                (Some(t), Some(r)) => format!("{t}: {r}"),
                (Some(t), None) => t.to_string(),
                (None, Some(r)) => r.to_string(),
                (None, None) => "approval".to_string(),
            };
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    reason: ellipsize(&reason, MAX_DECODED_FIELD_CHARS),
                    tool_use_id: call_id(),
                },
            ])
        }
        APPROVAL_DECIDED => {
            let event = if field(obj, "outcome") == Some(OUTCOME_ALLOWED) {
                // The resume: the gated call now runs; its `tool_call` was
                // already streamed before the gate, so only this Start lifts
                // the wait (the omp shape).
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: call_id(),
                    detail: Some(ToolDetail::Generic {
                        display: tool_display(),
                    }),
                }
            } else {
                // rejected | cancelled | unavailable: the call never runs.
                AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id: call_id(),
                }
            };
            Ok(vec![identity(), event])
        }
        MODEL => Ok(vec![AgentEvent::ModelInfo {
            agent_id,
            model: field(obj, "model").map(|m| ellipsize(m, MAX_DECODED_FIELD_CHARS)),
            effort: field(obj, "reasoningEffort").map(|e| ellipsize(e, MAX_DECODED_FIELD_CHARS)),
        }]),
        USAGE => {
            let n = |k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            // Fresh spend only: input + output + cache WRITES; cache reads are
            // re-served context (the CC `fresh_spend` semantics).
            Ok(vec![AgentEvent::Usage {
                agent_id,
                fresh_tokens: n("inputTokens")
                    .saturating_add(n("outputTokens"))
                    .saturating_add(n("cacheWriteTokens")),
            }])
        }
        _ => {
            crate::source::drift::unknown_event(SOURCE_NAME, ty);
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SID: &str = "01b00000-0000-7000-8000-000000000001";

    fn decode(v: Value) -> Vec<AgentEvent> {
        decode_dsh_payload(&v).unwrap()
    }

    fn id() -> AgentId {
        AgentId::from_parts("dsh", SID)
    }

    #[test]
    fn session_start_registers_and_trails_the_pid_carrier() {
        let evs = decode(json!({
            "type": "session_start", "sessionId": SID, "cwd": "/repo",
            "reason": "startup",
        }));
        match &evs[..] {
            [AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            }, AgentEvent::Identity { pid: None, .. }] => {
                assert_eq!(*agent_id, id());
                assert_eq!(source, "dsh");
                assert_eq!(session_id, SID);
                assert_eq!(cwd, &std::path::PathBuf::from("/repo"));
                assert_eq!(*parent_id, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn a_subagent_session_start_links_its_parent() {
        let parent = "01b00000-0000-7000-8000-0000000000aa";
        let evs = decode(json!({
            "type": "session_start", "sessionId": SID, "cwd": "/repo",
            "parentSession": parent, "reason": "startup",
        }));
        assert!(
            matches!(&evs[..], [AgentEvent::SessionStart { parent_id: Some(p), .. }, AgentEvent::Identity { .. }]
                if *p == AgentId::from_parts("dsh", parent)),
            "parentSession is the authoritative link: {evs:?}"
        );
    }

    #[test]
    fn session_end_flags_childness_from_the_carried_parent() {
        let evs = decode(json!({
            "type": "session_end", "sessionId": SID,
            "parentSession": "01b00000-0000-7000-8000-0000000000aa",
        }));
        assert!(matches!(
            &evs[..],
            [AgentEvent::SessionEnd { as_child: true, .. }]
        ));
        let evs = decode(json!({"type": "session_end", "sessionId": SID}));
        assert!(matches!(
            &evs[..],
            [AgentEvent::SessionEnd {
                as_child: false,
                ..
            }]
        ));
    }

    #[test]
    fn a_tool_round_maps_call_and_result_onto_the_real_call_id() {
        let evs = decode(json!({
            "type": "tool_call", "sessionId": SID,
            "callId": "call_1", "toolName": "bash",
        }));
        match &evs[..] {
            [AgentEvent::Identity { .. }, AgentEvent::ActivityStart {
                tool_use_id: Some(t),
                detail: Some(ToolDetail::Generic { display }),
                ..
            }] => {
                assert_eq!(t, "call_1");
                assert_eq!(display, "bash");
            }
            other => panic!("unexpected: {other:?}"),
        }
        let evs = decode(json!({
            "type": "tool_result", "sessionId": SID, "callId": "call_1",
            "isError": false,
        }));
        assert!(matches!(
            &evs[..],
            [AgentEvent::Identity { .. }, AgentEvent::ActivityEnd { tool_use_id: Some(t), .. }]
                if t == "call_1"
        ));
    }

    #[test]
    fn approval_asked_waits_on_the_named_call_and_allowed_once_resumes_it() {
        let evs = decode(json!({
            "type": "approval_asked", "sessionId": SID,
            "approvalId": "ap_1", "callId": "call_9",
            "toolName": "bash", "reason": "rm -rf",
        }));
        match &evs[..] {
            [AgentEvent::Identity { .. }, AgentEvent::Waiting {
                reason,
                tool_use_id: Some(t),
                ..
            }] => {
                assert_eq!(reason, "bash: rm -rf");
                assert_eq!(t, "call_9");
            }
            other => panic!("unexpected: {other:?}"),
        }
        let evs = decode(json!({
            "type": "approval_decided", "sessionId": SID,
            "approvalId": "ap_1", "callId": "call_9",
            "toolName": "bash", "outcome": "allowed-once",
        }));
        assert!(matches!(
            &evs[..],
            [AgentEvent::Identity { .. }, AgentEvent::ActivityStart { tool_use_id: Some(t), .. }]
                if t == "call_9"
        ));
    }

    #[test]
    fn every_non_grant_outcome_ends_the_gated_call() {
        for outcome in ["rejected", "cancelled", "unavailable"] {
            let evs = decode(json!({
                "type": "approval_decided", "sessionId": SID,
                "approvalId": "ap_1", "callId": "call_9", "outcome": outcome,
            }));
            assert!(
                matches!(
                    &evs[..],
                    [AgentEvent::Identity { .. }, AgentEvent::ActivityEnd { tool_use_id: Some(t), .. }]
                        if t == "call_9"
                ),
                "{outcome}: {evs:?}"
            );
        }
    }

    #[test]
    fn model_and_usage_map_to_their_events_with_fresh_spend_semantics() {
        let evs = decode(json!({
            "type": "model", "sessionId": SID,
            "provider": "deepseek", "model": "deepseek-v3", "reasoningEffort": "high",
        }));
        assert!(matches!(
            &evs[..],
            [AgentEvent::ModelInfo { model: Some(m), effort: Some(e), .. }]
                if m == "deepseek-v3" && e == "high"
        ));
        let evs = decode(json!({
            "type": "usage", "sessionId": SID,
            "inputTokens": 100, "outputTokens": 20,
            "cacheWriteTokens": 7, "cacheReadTokens": 100000,
        }));
        assert!(
            matches!(
                &evs[..],
                [AgentEvent::Usage {
                    fresh_tokens: 127,
                    ..
                }]
            ),
            "cache reads are re-served context, never fresh spend: {evs:?}"
        );
    }

    #[test]
    fn unknown_and_field_less_payloads_breadcrumb_and_decode_nothing() {
        let quiet = crate::test_capture::capture_logs(|| {
            assert!(decode(json!({"type": "mystery_event", "sessionId": SID})).is_empty());
            assert!(decode(json!({"type": "tool_call"})).is_empty());
            assert!(decode(json!("bare string")).is_empty());
        });
        assert!(
            quiet.contains("unknown_event") && quiet.contains("mystery_event"),
            "an unknown type must leave a drift breadcrumb: {quiet}"
        );
        assert!(
            quiet.contains("missing_field"),
            "a sessionId-less payload must leave a breadcrumb: {quiet}"
        );
    }
}
