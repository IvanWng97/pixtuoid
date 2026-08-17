//! Codex CLI source. Watches the Codex session transcript
//! (`~/.codex/sessions/**/rollout-<ts>-<UUID>.jsonl`) via `JsonlWatcher` for
//! the lifecycle signals the shared hook socket lacks — most importantly the
//! post-approval resume (`function_call_output`).
//!
//! Coalescing: hook.session_id == session_meta.id == filename UUID (verified),
//! so both transports merge onto one sprite.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Map, Value};

use crate::source::decoder::{ellipsize, make_tool_detail, MAX_DECODED_FIELD_CHARS};
use crate::source::AgentEvent;
use crate::AgentId;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{live_codex_rollout_ids, CodexSource};

/// The Codex CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "codex";

/// Trailing canonical UUID (`8-4-4-4-12`) of a `rollout-<ts>-<UUID>.jsonl`
/// filename. Equals the hook payload's `session_id`, so hook and JSONL events
/// coalesce. Falls back to the full stem if no trailing UUID is present.
pub fn codex_id_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // `.get()` (not `&stem[..]`): this runs on every file under the watched
    // tree, and a byte split landing mid-codepoint would panic.
    let tail = stem.get(stem.len().saturating_sub(36)..).unwrap_or("");
    if is_uuid(tail) {
        tail.to_string()
    } else {
        stem.to_string()
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Codex's source-specific hook arms — `SubagentStart`/`SubagentStop`. These
/// change the event's SUBJECT (the child's AgentId, not the session's), which
/// the shared CC-shaped arms cannot express; every other Codex hook event
/// falls through (`Ok(None)`) to them. The parent link carried here is the
/// ONLY one a flat Codex rollout gets.
pub(crate) fn decode_codex_hook_custom(v: &Value) -> Result<Option<Vec<AgentEvent>>> {
    use anyhow::anyhow;
    let Some(obj) = v.as_object() else {
        return Ok(None); // shared path reports the malformed payload
    };
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    // The custom-decoder contract: claim our two events FULLY (Err on a
    // malformed instance), Ok(None) for everything else. An empty
    // `session_id`/`agent_id` would mint a phantom that never coalesces with
    // the real rollout — reject rather than decode.
    let guards = |obj: &Map<String, Value>| -> Result<(String, Option<String>)> {
        let session_id = obj
            .get("session_id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing/empty session_id"))?
            .to_string();
        let child = obj
            .get("agent_id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        Ok((session_id, child))
    };
    match event {
        // The subagent owns a SEPARATE rollout (filename UUID == this payload's
        // `agent_id`) that the watcher renders ORPHANED — a flat rollout path
        // has no `/subagents/` for `detect_parent_id`. Keying the child on
        // `agent_id` coalesces with that rollout and joins the parent's scope
        // tree (cascade / liveness / readiness).
        "SubagentStart" => {
            let (session_id, child) = guards(obj)?;
            let child = child.ok_or_else(|| anyhow!("SubagentStart missing/empty agent_id"))?;
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            Ok(Some(vec![AgentEvent::SessionStart {
                agent_id: AgentId::from_parts(SOURCE_NAME, &child),
                source: SOURCE_NAME.to_string(),
                session_id: child,
                cwd,
                parent_id: Some(AgentId::from_parts(SOURCE_NAME, &session_id)),
            }]))
        }
        // End the CHILD promptly, else its rollout lingers to the 30-min
        // stale-sweep. Losing the race against the child's slot creation
        // leaves a harmless no-op plus that same fallback.
        "SubagentStop" => {
            let (_session_id, child) = guards(obj)?;
            let child = child.ok_or_else(|| anyhow!("SubagentStop missing/empty agent_id"))?;
            Ok(Some(vec![AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(SOURCE_NAME, &child),
                as_child: true,
            }]))
        }
        _ => Ok(None),
    }
}

/// Codex rollouts carry the cwd ONLY on the head `session_meta` line, nested
/// under `payload`.
pub(crate) fn extract_codex_cwd(v: &Value) -> Option<PathBuf> {
    v.get("payload")?.get("cwd")?.as_str().map(PathBuf::from)
}

const EVENT_MSG: &str = "event_msg";
const RESPONSE_ITEM: &str = "response_item";
const TURN_CONTEXT: &str = "turn_context";

/// The rollout OUTER discriminators this decoder dispatches on — this module's
/// row in the drift surface. Test-gated: the surface emitter is the only reader.
#[cfg(test)]
pub(crate) const DECODED_OUTERS: &[&str] = &[EVENT_MSG, RESPONSE_ITEM, TURN_CONTEXT];

/// The rollout inners, grouped BY BEHAVIOUR — each group is both the matcher
/// (the arms below guard on `contains`) and the export source, so there is one
/// declaration per name and no second copy to drift. Adding a name to a group
/// changes what decodes AND what the drift surface claims, in one edit.
const EM_TURN_START: &[&str] = &["task_started", "turn_started"];
const EM_RESUME: &[&str] = &["exec_command_end", "patch_apply_end"];
const EM_SEARCH: &[&str] = &["web_search_begin", "web_search_end"];
const EM_TURN_END: &[&str] = &["task_complete", "turn_complete", "turn_aborted"];
const EM_TOKENS: &[&str] = &["token_count"];

const RI_TOOL_START: &[&str] = &["function_call", "custom_tool_call"];
const RI_RESUME: &[&str] = &["function_call_output", "custom_tool_call_output"];
const RI_SEARCH: &[&str] = &["web_search_call", "tool_search_call", "tool_search_output"];

/// Derived from the SAME group consts the arms guard on — never a hand-kept
/// mirror. Test-gated: the surface emitter is the only reader.
#[cfg(test)]
pub(crate) fn decoded_event_msg() -> Vec<&'static str> {
    [EM_TURN_START, EM_RESUME, EM_SEARCH, EM_TURN_END, EM_TOKENS].concat()
}

#[cfg(test)]
pub(crate) fn decoded_response_item() -> Vec<&'static str> {
    [RI_TOOL_START, RI_RESUME, RI_SEARCH].concat()
}

/// Decode one transcript line. `tool_use_id` is always `None` so these events
/// are never suppressed by the hook-wins dedup (which keys on `tool_use_id`).
pub fn decode_codex_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, &codex_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let outer = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
    let payload = obj.get("payload").and_then(|p| p.as_object());
    let inner = payload
        .and_then(|p| p.get("type"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let start = || AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: None,
        detail: None,
    };
    let end = || AgentEvent::ActivityEnd {
        agent_id,
        tool_use_id: None,
    };

    let out = match (outer, inner) {
        // `task_started` is what codex serializes today; `turn_started` is
        // upstream's own serde alias, accepted so a future serializer flip to
        // the alias form still drives Active/Idle.
        (EVENT_MSG, i) if EM_TURN_START.contains(&i) => vec![start()],
        // `custom_tool_call` is the SAME item under codex's custom-tool API and is
        // what a modern session serializes — `a_custom_tool_call_is_a_tool_call`.
        (RESPONSE_ITEM, i) if RI_TOOL_START.contains(&i) => {
            if function_call_needs_approval(payload) {
                vec![AgentEvent::Waiting {
                    agent_id,
                    reason: "permission".to_string(),
                }]
            } else {
                vec![codex_tool_start(agent_id, payload)]
            }
        }
        // Resume signals: a command/patch finished running after (auto-)approval.
        // Each is an ActivityStart so the reducer clears any Waiting set by the
        // permission gate.
        (RESPONSE_ITEM, i) if RI_RESUME.contains(&i) => vec![start()],
        (EVENT_MSG, i) if EM_RESUME.contains(&i) => vec![start()],
        // Web/tool search are turn-INTERNAL work pulses that keep the agent
        // Active; only task_complete / turn_aborted end the turn. Both the
        // EventMsg and the raw-Responses-item forms appear in real rollouts.
        // Searching is never permission-prompted — no Waiting branch.
        (RESPONSE_ITEM, i) if RI_SEARCH.contains(&i) => vec![start()],
        (EVENT_MSG, i) if EM_SEARCH.contains(&i) => vec![start()],
        (EVENT_MSG, i) if EM_TURN_END.contains(&i) => vec![end()],
        // `last_token_usage` is that turn's reading; the cumulative twin
        // `total_token_usage` must NOT be read — the reducer accumulates
        // deltas, so summing a running total would double-count. codex's
        // `input_tokens` INCLUDES the cached share (verified), so fresh input
        // = input − cached, saturating because upstream reporting quirks must
        // not wrap; `reasoning_output_tokens` is additive alongside `output`.
        (EVENT_MSG, i) if EM_TOKENS.contains(&i) => {
            let last = payload
                .and_then(|p| p.get("info"))
                .and_then(|i| i.get("last_token_usage"))
                .and_then(|u| u.as_object());
            let fresh = last.map_or(0, |u| {
                let field = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                field("input_tokens")
                    .saturating_sub(field("cached_input_tokens"))
                    .saturating_add(field("output_tokens"))
                    .saturating_add(field("reasoning_output_tokens"))
            });
            if fresh > 0 {
                vec![AgentEvent::Usage {
                    agent_id,
                    fresh_tokens: fresh,
                }]
            } else {
                vec![]
            }
        }
        // `turn_context` opens every turn carrying the model and, on reasoning
        // turns only, the effort — both RAW verbatim, last-seen-wins, so a
        // mid-session switch tracks. Absent effort ≠ downgrade: the reducer
        // only refreshes on Some.
        (TURN_CONTEXT, _) => {
            let model = payload
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty());
            let effort = payload
                .and_then(|p| p.get("effort"))
                .and_then(|e| e.as_str())
                .filter(|e| !e.is_empty());
            if model.is_some() || effort.is_some() {
                vec![AgentEvent::ModelInfo {
                    agent_id,
                    model: model.map(|m| ellipsize(m, MAX_DECODED_FIELD_CHARS)),
                    effort: effort.map(|e| ellipsize(e, MAX_DECODED_FIELD_CHARS)),
                }]
            } else {
                vec![]
            }
        }
        _ => vec![],
    };
    Ok(out)
}

/// A Codex `function_call` requesting escalated sandbox permissions
/// (`arguments` is a JSON string carrying `sandbox_permissions:
/// "require_escalated"`) is an approval gate → Waiting. A bare `justification`
/// is intentionally NOT a signal: Codex emits it on auto-approved commands too,
/// so keying on it would false-Wait.
/// Codex has TWO tool surfaces and only this one marks escalation in the
/// rollout. A `custom_tool_call` carries `input` — a JS snippet, not JSON args —
/// and a recorded escalated run on that surface shows no marker anywhere, so
/// there the gate exists ONLY on the hook wire (`PermissionRequest`). Which
/// surface a turn takes is model-chosen, so neither is dead: a local rollout
/// census found real `require_escalated` calls, and codex 0.147.0's system
/// prompt still instructs the model to send exactly this parameter.
fn function_call_needs_approval(payload: Option<&Map<String, Value>>) -> bool {
    let Some(args_str) = payload
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.as_str())
    else {
        return false;
    };
    let args = match serde_json::from_str::<Value>(args_str) {
        Ok(v) => v,
        Err(e) => {
            crate::source::drift::shape_drift(
                SOURCE_NAME,
                &format!("function_call arguments not parseable: {e}"),
            );
            return false;
        }
    };
    args.get("sandbox_permissions").and_then(|s| s.as_str()) == Some("require_escalated")
}

fn codex_tool_start(agent_id: AgentId, payload: Option<&Map<String, Value>>) -> AgentEvent {
    let name = payload
        .and_then(|p| p.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| {
            crate::source::drift::missing_field(SOURCE_NAME, "tool call", "name");
            "tool"
        });
    AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: None,
        // Codex tool calls are never subagent dispatches (those arrive as the
        // SubagentStart hook), so there is no `subagent_type` to pass.
        detail: Some(make_tool_detail(SOURCE_NAME, name, None)),
    }
}

/// The Codex home dir — honors `CODEX_HOME` when it points at an existing dir,
/// else `~/.codex` (codex's own precedence). The installer routes its
/// `config.toml` path through here too, so the watched sessions root and the
/// installed-hook config can never disagree.
pub fn codex_home() -> PathBuf {
    crate::platform::codex_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every exported rollout name reaches a real arm, and a name outside the
    /// sets reaches none. The OUTERS dispatch on consts so their value cannot
    /// drift; the inners are a second copy, which is exactly why each is driven
    /// here rather than merely listed.
    #[test]
    fn the_decoded_rollout_sets_are_exactly_what_the_arms_match() {
        let drive = |outer: &str, inner: &str| {
            let v = json!({
                "type": outer,
                "payload": {"type": inner, "turn_id": "t1", "cwd": "/r",
                            "name": "bash", "call_id": "c1", "arguments": "{}",
                            "model": "gpt-5.6-sol", "effort": "xhigh",
                            "info": {"last_token_usage": {"input_tokens": 1}}},
            });
            decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v)
                .is_ok_and(|e| !e.is_empty())
        };
        for inner in decoded_event_msg() {
            assert!(
                drive(EVENT_MSG, inner),
                "event_msg/{inner} must reach an arm"
            );
        }
        for inner in decoded_response_item() {
            assert!(
                drive(RESPONSE_ITEM, inner),
                "response_item/{inner} must reach an arm",
            );
        }
        assert!(
            drive(TURN_CONTEXT, "anything"),
            "turn_context matches on `_`"
        );
        assert_eq!(DECODED_OUTERS, [EVENT_MSG, RESPONSE_ITEM, TURN_CONTEXT]);
        assert!(
            !drive(EVENT_MSG, "turn_paused"),
            "an unread inner reaches none"
        );
        assert!(
            !drive("session_meta", "task_started"),
            "an unread outer reaches none"
        );
    }

    #[test]
    fn turn_context_surfaces_model_and_effort_verbatim() {
        let v = serde_json::json!({
            "type": "turn_context",
            "payload": {"turn_id": "t1", "cwd": "/r", "model": "gpt-5.6-sol", "effort": "xhigh"}
        });
        let evs = decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v).unwrap();
        assert!(
            evs.iter().any(
                |e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: Some(f), .. }
                if m == "gpt-5.6-sol" && f == "xhigh")
            ),
            "turn_context must surface model+effort, got {evs:?}"
        );
        let v = serde_json::json!({
            "type": "turn_context",
            "payload": {"turn_id": "t2", "cwd": "/r", "model": "gpt-5.5"}
        });
        let evs = decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "gpt-5.5")),
            "got {evs:?}"
        );
        let v = serde_json::json!({"type": "turn_context", "payload": {"turn_id": "t3"}});
        assert!(
            decode_codex_line("/p/rollout-2026-07-10-abc.jsonl", "codex", v)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn is_uuid_requires_length_dashes_and_hex() {
        assert!(is_uuid("0196fdb2-99d1-7db2-9ded-93a4a0d4a90e"));
        for bad in [
            "",
            "abc",
            "0196fdb299d17db29ded93a4a0d4a90e",
            "0196fdb2-99d1-7db2-9ded-93a4a0d4a90ez",
            "0196fdb2x99d1-7db2-9ded-93a4a0d4a90e",
            "0196fdb2-99d1-7db2-9ded-93a4a0d4a90g",
        ] {
            assert!(!is_uuid(bad), "{bad:?} must not read as a UUID");
        }
    }

    #[test]
    fn subagent_hooks_with_empty_ids_are_err_not_fallthrough() {
        for event in ["SubagentStart", "SubagentStop"] {
            let no_session = json!({"hook_event_name": event, "agent_id": "child"});
            assert!(
                decode_codex_hook_custom(&no_session).is_err(),
                "{event} without session_id must Err (claim-fully), not fall through"
            );
            let empty_child = json!({"hook_event_name": event, "session_id": "s", "agent_id": ""});
            assert!(
                decode_codex_hook_custom(&empty_child).is_err(),
                "{event} with empty agent_id must Err — a phantom child never coalesces"
            );
        }
    }

    #[test]
    fn non_subagent_events_fall_through_to_shared_arms() {
        let stop = json!({"hook_event_name": "Stop", "session_id": "s"});
        assert!(matches!(decode_codex_hook_custom(&stop), Ok(None)));
        assert!(matches!(decode_codex_hook_custom(&json!("nope")), Ok(None)));
    }

    #[test]
    fn session_end_hook_decodes_to_a_clean_session_end() {
        let payload = json!({
            "hook_event_name": "SessionEnd",
            "session_id": "019e7762-9ded-7e33-be41-946ecf105bf4",
            "cwd": "/repo",
            "reason": "other",
            "_pixtuoid_source": SOURCE_NAME,
        });
        assert!(matches!(decode_codex_hook_custom(&payload), Ok(None)));
        let events = crate::source::decoder::decode_hook_payload(payload).unwrap();
        let expected = AgentId::from_parts(SOURCE_NAME, "019e7762-9ded-7e33-be41-946ecf105bf4");
        assert!(
            matches!(
                events.as_slice(),
                [AgentEvent::SessionEnd { agent_id, as_child: false }] if *agent_id == expected
            ),
            "SessionEnd must decode to exactly one clean SessionEnd for the \
             rollout-coalesced id, got {events:?}"
        );
    }

    fn ev(line: Value) -> Vec<AgentEvent> {
        decode_codex_line(
            "/x/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
            SOURCE_NAME,
            line,
        )
        .unwrap()
    }

    #[test]
    fn task_started_is_activity_start() {
        for t in ["task_started", "turn_started"] {
            let out = ev(json!({"type":"event_msg","payload":{"type":t,"turn_id":"t"}}));
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
                "{t}"
            );
        }
    }

    /// Captured off codex 2026-08: a real rollout serializes its tool calls as
    /// `custom_tool_call`, never the `function_call` the composed fixture used, so
    /// reading only the latter let four tool calls decode to nothing while the
    /// turn still showed Active from `task_started`.
    #[test]
    fn a_custom_tool_call_is_a_tool_call() {
        let out = ev(json!({"type":"response_item","payload":{
            "type":"custom_tool_call","call_id":"call_x","name":"exec","input":"ls"}}));
        match out.as_slice() {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => {
                assert!(
                    format!("{d:?}").contains("exec"),
                    "tool name must reach the detail: {d:?}"
                )
            }
            other => panic!("expected an ActivityStart carrying the tool, got {other:?}"),
        }
        let resumed = ev(json!({"type":"response_item","payload":{
            "type":"custom_tool_call_output","call_id":"call_x","output":"ok"}}));
        assert!(
            matches!(resumed.as_slice(), [AgentEvent::ActivityStart { .. }]),
            "its output must resume work like function_call_output does"
        );
    }

    #[test]
    fn function_call_output_resumes_work() {
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"ok"}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn patch_apply_end_resumes_work() {
        let out =
            ev(json!({"type":"event_msg","payload":{"type":"patch_apply_end","success":true}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn web_and_tool_search_keep_the_agent_active() {
        for line in [
            json!({"type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{}}}),
            json!({"type":"event_msg","payload":{"type":"web_search_begin","call_id":"c"}}),
            json!({"type":"event_msg","payload":{"type":"web_search_end","call_id":"c","query":"q","action":{}}}),
            json!({"type":"response_item","payload":{"type":"tool_search_call","call_id":"c","status":"in_progress","arguments":"{}"}}),
            json!({"type":"response_item","payload":{"type":"tool_search_output","call_id":"c","status":"completed","tools":[]}}),
        ] {
            let out = ev(line.clone());
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
                "search event {line} must keep the agent Active"
            );
        }
    }

    #[test]
    fn escalated_function_call_is_waiting() {
        let args =
            r#"{"cmd":"date","sandbox_permissions":"require_escalated","justification":"allow?"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::Waiting { .. }]));
    }

    #[test]
    fn plain_function_call_is_activity_start() {
        let args = r#"{"cmd":"ls"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn justification_without_escalation_is_not_waiting() {
        let args = r#"{"cmd":"ls","justification":"because"}"#;
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}),
        );
        assert!(
            matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
            "{out:?}"
        );
    }

    #[test]
    fn malformed_arguments_does_not_panic_and_starts_work() {
        let out = ev(
            json!({"type":"response_item","payload":{"type":"function_call","name":"x","arguments":"{not json"}}),
        );
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn task_complete_and_abort_end_activity() {
        for t in ["task_complete", "turn_complete", "turn_aborted"] {
            let out = ev(json!({"type":"event_msg","payload":{"type":t,"turn_id":"t"}}));
            assert!(
                matches!(out.as_slice(), [AgentEvent::ActivityEnd { .. }]),
                "{t}"
            );
        }
    }

    #[test]
    fn session_meta_and_unknown_emit_nothing() {
        assert!(ev(json!({"type":"session_meta","payload":{"id":"u","cwd":"/r"}})).is_empty());
        // A token_count without `info` is a rate-limit-only ping.
        assert!(ev(json!({"type":"event_msg","payload":{"type":"token_count"}})).is_empty());
    }

    #[test]
    fn token_count_emits_fresh_usage_from_last_reading() {
        // fresh = (11480−9088) + 87 + 15.
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"input_tokens":999999,"cached_input_tokens":0,"output_tokens":999999},
            "last_token_usage":{"input_tokens":11480,"cached_input_tokens":9088,
                                 "output_tokens":87,"reasoning_output_tokens":15}}}}),
        );
        assert!(
            matches!(
                out.as_slice(),
                [AgentEvent::Usage {
                    fresh_tokens: 2494,
                    ..
                }]
            ),
            "expected fresh=2494 from last_token_usage (never the totals), got {out:?}"
        );
    }

    #[test]
    fn token_count_saturates_and_skips_zero() {
        // cached > input is an upstream reporting quirk.
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "last_token_usage":{"input_tokens":10,"cached_input_tokens":50,"output_tokens":7}}}}),
        );
        assert!(
            matches!(
                out.as_slice(),
                [AgentEvent::Usage {
                    fresh_tokens: 7,
                    ..
                }]
            ),
            "got {out:?}"
        );
        let out = ev(
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0}}}}),
        );
        assert!(out.is_empty(), "zero reading must be silent, got {out:?}");
    }

    #[test]
    fn id_from_rollout_path_is_trailing_uuid() {
        let p = Path::new(
            "/Users/me/.codex/sessions/2026/05/29/rollout-2026-05-29T22-36-52-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
        );
        // Must equal the hook session_id for coalescing.
        assert_eq!(
            codex_id_from_path(p),
            "019e7762-9ded-7e33-be41-946ecf105bf4"
        );
    }

    // `codex_id_from_path` is invoked in three places that must agree — the
    // per-line decode, the registry row's `id_from_path`, and the fixture test
    // above. A disagreement splits one Codex session into two sprites.
    #[test]
    fn decode_line_keys_agent_id_on_codex_id_from_path() {
        let path = "/x/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl";
        let events = decode_codex_line(
            path,
            SOURCE_NAME,
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}),
        )
        .unwrap();
        let expected = AgentId::from_parts(SOURCE_NAME, &codex_id_from_path(Path::new(path)));
        assert_eq!(
            events[0].agent_id(),
            expected,
            "decode_codex_line must key its AgentId on codex_id_from_path (the deriver)"
        );
    }

    #[test]
    fn id_falls_back_to_stem_without_uuid() {
        let p = Path::new("/tmp/notarollout.jsonl");
        assert_eq!(codex_id_from_path(p), "notarollout");
    }

    #[test]
    fn id_handles_non_ascii_filename_without_panic() {
        // A stem whose len-36 byte split lands mid-codepoint.
        let p = Path::new("/tmp/rollout-日本語のとてもながいファイルめい.jsonl");
        let _ = codex_id_from_path(p);
    }

    #[test]
    fn non_object_line_emits_nothing() {
        assert!(ev(json!("just a string")).is_empty());
        assert!(ev(json!(42)).is_empty());
        assert!(ev(json!([1, 2, 3])).is_empty());
    }

    #[test]
    fn function_call_without_arguments_starts_work_not_waiting() {
        let out = ev(json!({
            "type": "response_item",
            "payload": { "type": "function_call", "name": "x" }
        }));
        assert!(
            matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]),
            "{out:?}"
        );
    }

    #[test]
    fn function_call_without_name_falls_back_to_tool_label() {
        // The `arguments` must stay non-escalated so routing reaches
        // codex_tool_start rather than the Waiting arm.
        use crate::source::ToolDetail;
        let out = ev(json!({
            "type": "response_item",
            "payload": { "type": "function_call", "arguments": r#"{"cmd":"ls"}"# }
        }));
        match out.as_slice() {
            [AgentEvent::ActivityStart {
                detail: Some(ToolDetail::Generic { display }),
                ..
            }] => assert_eq!(display, "tool"),
            other => panic!("expected one Generic-detail ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn no_rollout_outer_breadcrumbs_however_new() {
        for outer in [
            "brand_new_outer_2027",
            "session_meta",
            "event_msg",
            "compacted",
            "world_state",
            "inter_agent_communication",
            "inter_agent_communication_metadata",
            "response_item",
        ] {
            let line =
                serde_json::json!({ "type": outer, "payload": { "type": "x", "message": "y" } });
            let quiet = crate::test_capture::capture_logs(|| {
                decode_codex_line("/x/rollout.jsonl", SOURCE_NAME, line).unwrap();
            });
            assert!(
                !quiet.contains(crate::source::drift::TARGET),
                "outer {outer:?} carries nothing we read, so it must stay out of the drift log:\n{quiet}"
            );
        }
    }
}
