use anyhow::Result;
use serde_json::Value;
use std::path::Path;

use crate::source::decoder::{first_present_str, generic_tool_display};
use crate::source::AgentEvent;
use crate::AgentId;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::AntigravitySource;

/// The Antigravity CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "antigravity";

/// Antigravity-cli writes BOTH `transcript.jsonl` (truncated) and
/// `transcript_full.jsonl` (untruncated) per conversation, carrying the SAME
/// `step_index` stream — so walking both mints two path-keyed `AgentId`s and
/// double-renders the conversation. Watch only the canonical `transcript.jsonl`;
/// the decoder ignores content length, so the untruncated copy loses nothing.
pub(crate) fn admits_transcript(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) != Some("transcript_full.jsonl")
}

/// Decode one Antigravity CLI transcript line into `AgentEvent`s (the step_index / tool_calls JSONL schema).
pub fn decode_ag_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, transcript_path);
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    // A present-but-non-integer OR negative `step_index` must skip the line: a
    // negative would mint a start like `ag--5-0` that no end can ever pair,
    // leaving the slot stuck Active until the reducer's stale-sweep.
    let Some(step_index) = obj
        .get("step_index")
        .and_then(|v| v.as_i64())
        .filter(|&s| s >= 0)
    else {
        return Ok(vec![]);
    };
    let step_type = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    let mut out = Vec::new();

    if step_type == "PLANNER_RESPONSE" {
        if let Some(Value::Array(tool_calls)) = obj.get("tool_calls") {
            for (i, tc) in tool_calls.iter().enumerate() {
                let Some(tc_obj) = tc.as_object() else {
                    continue;
                };
                let name = tc_obj
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or_else(|| {
                        crate::source::drift::missing_field(
                            SOURCE_NAME,
                            "PLANNER_RESPONSE",
                            "name",
                        );
                        "?"
                    });
                let args = tc_obj.get("args");
                out.push(decode_ag_tool_call(agent_id, name, args, step_index, i));
            }
        }
    } else {
        // Antigravity is a closed Google IDE with no fetchable schema, so
        // in-code breadcrumbs are the ONLY currency backstop. A step carrying
        // `tool_calls` under a type that ISN'T `PLANNER_RESPONSE` is the signal
        // that upstream RENAMED that step. Result/input steps carry no
        // `tool_calls`, so this can't false-positive on them.
        if matches!(obj.get("tool_calls"), Some(Value::Array(a)) if !a.is_empty()) {
            crate::source::drift::unknown_event(SOURCE_NAME, step_type);
        }
        if step_type != "USER_INPUT" && step_type != "CONVERSATION_HISTORY" && step_index > 0 {
            // Only the primary (i=0) start gets a matching end; the reducer's
            // pending_idle debounce ages out a multi-tool step's remaining starts.
            out.push(AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: Some(format!("ag-{}-0", step_index - 1)),
            });
        }
    }

    Ok(out)
}

/// Decode one tool call within a `PLANNER_RESPONSE` step. A permission/question
/// prompt becomes `Waiting`; anything else an `ActivityStart` keyed
/// `ag-{step_index}-{i}`. That id is load-bearing: the NEXT step ends the
/// primary with `ag-{step_index-1}-0`, so the `i == 0` start must carry it.
fn decode_ag_tool_call(
    agent_id: AgentId,
    name: &str,
    args: Option<&Value>,
    step_index: i64,
    i: usize,
) -> AgentEvent {
    // `ask_permission`/`ask_question` are UNVERIFIED reverse-engineered tool
    // names — no capture confirms them (the real wire only ever showed
    // `search_web`). Kept as the best-effort Waiting trigger.
    if name == "ask_permission" || name == "ask_question" {
        return AgentEvent::Waiting {
            agent_id,
            reason: "asking permission".to_string(),
            tool_use_id: None,
        };
    }
    let target = ag_tool_target(args);
    AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: Some(format!("ag-{step_index}-{i}")),
        detail: Some(generic_tool_display(name, target.as_deref())),
    }
}

/// The first present path/command field of an Antigravity tool call's `args`,
/// quote-stripped — the `: target` half of the Generic display.
fn ag_tool_target(args: Option<&Value>) -> Option<String> {
    // Priority order. Only `query` (the `search_web` arg) is capture-confirmed
    // against real wire; the PascalCase keys are reverse-engineered from
    // Windsurf/Cascade and UNVERIFIED.
    const KEYS: &[&str] = &[
        "DirectoryPath",
        "AbsolutePath",
        "TargetFile",
        "CommandLine",
        "SearchPath",
        "query",
    ];
    let raw = first_present_str(args?, KEYS)?;
    let clean = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    Some(clean.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_step_index_is_skipped_not_minted() {
        let v = serde_json::json!({
            "type": "PLANNER_RESPONSE",
            "step_index": -1,
            "tool_calls": [ { "name": "read_file", "args": {} } ],
        });
        let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
        assert!(
            out.is_empty(),
            "negative step_index must emit nothing: {out:?}"
        );

        let v = serde_json::json!({
            "type": "PLANNER_RESPONSE",
            "step_index": 0,
            "tool_calls": [ { "name": "read_file", "args": {} } ],
        });
        let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
        assert_eq!(out.len(), 1, "step_index 0 still emits: {out:?}");
    }

    #[test]
    fn only_a_real_follow_up_step_ends_the_previous_primary_tool() {
        for (ty, idx) in [
            ("USER_INPUT", 3),
            ("CONVERSATION_HISTORY", 2),
            ("EXECUTION_RESULT", 0),
        ] {
            let v = serde_json::json!({ "type": ty, "step_index": idx });
            let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
            assert!(
                out.is_empty(),
                "{ty} at step {idx} must not end anything: {out:?}"
            );
        }
        let v = serde_json::json!({ "type": "EXECUTION_RESULT", "step_index": 2 });
        let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::ActivityEnd { tool_use_id, .. } => {
                assert_eq!(tool_use_id.as_deref(), Some("ag-1-0"));
            }
            other => panic!("expected ActivityEnd, got {other:?}"),
        }
    }

    #[test]
    fn renamed_planner_step_with_tool_calls_breadcrumbs_and_ends_only() {
        let renamed = serde_json::json!({
            "type": "PLANNER_REPLY",
            "step_index": 2,
            "tool_calls": [ { "name": "read_file", "args": {} } ],
        });
        let logs = crate::test_capture::capture_logs(|| {
            let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, renamed).unwrap();
            assert_eq!(out.len(), 1, "renamed step must mint no start: {out:?}");
            match &out[0] {
                AgentEvent::ActivityEnd { tool_use_id, .. } => {
                    assert_eq!(tool_use_id.as_deref(), Some("ag-1-0"));
                }
                other => panic!("expected ActivityEnd only, got {other:?}"),
            }
        });
        assert!(
            logs.contains("unknown_event") && logs.contains("PLANNER_REPLY"),
            "a tool-call step under a renamed type must fire the drift breadcrumb, got:\n{logs}"
        );

        let result_step = serde_json::json!({ "type": "TOOL_RESULT", "step_index": 2 });
        let quiet = crate::test_capture::capture_logs(|| {
            decode_ag_line("/x/t.jsonl", SOURCE_NAME, result_step).unwrap();
        });
        assert!(
            !quiet.contains("unknown_event"),
            "a tool_calls-less result step must NOT breadcrumb, got:\n{quiet}"
        );
    }

    #[test]
    fn tool_call_display_carries_the_quote_stripped_target() {
        use crate::source::ToolDetail;
        let v = serde_json::json!({
            "type": "PLANNER_RESPONSE",
            "step_index": 1,
            "tool_calls": [
                { "name": "run_command", "args": { "CommandLine": "\"git status\"" } },
                { "name": "grep_search", "args": { "SearchPath": "/repo", "query": "TODO" } },
                { "name": "view_file", "args": {} },
            ],
        });
        let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
        let displays: Vec<&str> = out
            .iter()
            .map(|e| match e {
                AgentEvent::ActivityStart {
                    detail: Some(ToolDetail::Generic { display }),
                    ..
                } => display.as_str(),
                other => panic!("expected Generic ActivityStart, got {other:?}"),
            })
            .collect();
        assert_eq!(
            displays,
            ["run_command: git status", "grep_search: /repo", "view_file",]
        );
    }

    #[test]
    fn ag_tool_target_falls_through_a_present_non_string_key() {
        let args = serde_json::json!({ "DirectoryPath": 42, "AbsolutePath": "/repo/x" });
        assert_eq!(ag_tool_target(Some(&args)).as_deref(), Some("/repo/x"));
    }

    #[test]
    fn admits_every_transcript_except_the_full_duplicate() {
        let dir = Path::new("/h/.gemini/antigravity-cli/brain/c1/.system_generated/logs");
        assert!(admits_transcript(&dir.join("transcript.jsonl")));
        assert!(!admits_transcript(&dir.join("transcript_full.jsonl")));
        assert!(admits_transcript(&dir.join("other.jsonl")));
    }
}
