use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{first_present_str, generic_tool_display};
use crate::source::AgentEvent;
use crate::AgentId;

// The runtime half (`AntigravitySource` + its watcher wiring) — ONE gate for
// the whole `native` layer of this source; the re-export keeps the pre-split
// `source::antigravity::AntigravitySource` path.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::AntigravitySource;

pub const SOURCE_NAME: &str = "antigravity";

pub fn decode_ag_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, transcript_path);
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    // A present-but-non-integer OR negative `step_index` (format drift / a
    // renamed field) must fail SAFE-AND-VISIBLE: skip the line rather than emit
    // an unmatchable id. A negative would mint a start like `ag--5-0` that no
    // end (the `> 0` branch) can ever pair, leaving the slot stuck Active until
    // the reducer's debounce/stale-sweep; coercing to 0 would silently corrupt
    // the `ag-{step}-{i}` tool_use_id pairing the same way.
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
        // Antigravity is a closed Google IDE with no fetchable transcript
        // schema, so the CI upstream-drift watch (defense #4) can't cover it —
        // defense #2 (in-code breadcrumbs) is the ONLY currency backstop. A step
        // carrying `tool_calls` under a type that ISN'T `PLANNER_RESPONSE` is the
        // signal that upstream RENAMED that step (the `missing_field` breadcrumb
        // above sits inside the now-unreached arm, so a rename would otherwise
        // go silent — sprites blank with no drift trace). Result/input steps
        // carry no `tool_calls`, so this can't false-positive on them.
        if matches!(obj.get("tool_calls"), Some(Value::Array(a)) if !a.is_empty()) {
            crate::source::drift::unknown_event(SOURCE_NAME, step_type);
        }
        if step_type != "USER_INPUT" && step_type != "CONVERSATION_HISTORY" && step_index > 0 {
            // End the first tool from the previous step. Multi-tool steps have
            // their remaining starts aged out by the reducer's pending_idle
            // debounce, but the primary (i=0) start always gets a matching end.
            out.push(AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: Some(format!("ag-{}-0", step_index - 1)),
            });
        }
    }

    Ok(out)
}

/// Decode one tool call within a `PLANNER_RESPONSE` step. A permission/question
/// prompt becomes `Waiting`; anything else becomes an `ActivityStart` keyed
/// `ag-{step_index}-{i}`. That id is load-bearing: the reducer ages out the
/// non-primary (`i > 0`) starts via its pending_idle debounce, and the NEXT
/// step ends the primary with `ag-{step_index-1}-0`, so the `i == 0` start must
/// carry exactly this id to be matched.
fn decode_ag_tool_call(
    agent_id: AgentId,
    name: &str,
    args: Option<&Value>,
    step_index: i64,
    i: usize,
) -> AgentEvent {
    if name == "ask_permission" || name == "ask_question" {
        return AgentEvent::Waiting {
            agent_id,
            reason: "asking permission".to_string(),
        };
    }
    let target = ag_tool_target(args);
    AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: Some(format!("ag-{step_index}-{i}")),
        detail: Some(generic_tool_display(name, target.as_deref())),
    }
}

/// The first present path/command field of an Antigravity tool call's
/// `args`, quote-stripped — the `: target` half of the Generic display.
/// AG tool names have no `describe_tool_target` arm (that dispatch is CC's),
/// so like `cursor_tool_detail` the source extracts its own target and hands
/// it to the shared `generic_tool_display` chokepoint for the caps. (The
/// former `normalize_ag_tool_input` re-KEYED the value into a
/// `{command|pattern|file_path: …}` object for `make_tool_detail` to read —
/// but nothing read AG-named tools' input there, so the keys were dead code
/// and AG displays silently lost their targets.)
fn ag_tool_target(args: Option<&Value>) -> Option<String> {
    // AG's per-tool arg vocabulary (priority order); the shared scan skips a
    // present-but-non-string key rather than giving up on it (the old
    // `.or_else` chain `as_str`d only the first present key).
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
        // A negative step_index would mint an unmatchable `ag--1-0` start id,
        // sticking the slot Active. It must be skipped like a non-integer.
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

        // Control: a non-negative step_index still emits the tool start.
        let v = serde_json::json!({
            "type": "PLANNER_RESPONSE",
            "step_index": 0,
            "tool_calls": [ { "name": "read_file", "args": {} } ],
        });
        let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, v).unwrap();
        assert_eq!(out.len(), 1, "step_index 0 still emits: {out:?}");
    }

    /// The primary-tool End is scoped: USER_INPUT / CONVERSATION_HISTORY
    /// steps are not tool completions (an `&&`→`||` flip would end a tool on
    /// every user prompt), and step 0 has NO previous step to end — an End
    /// there would mint the unmatchable `ag--1-0` (strict `step_index > 0`).
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
        // Control: a real follow-up step ends the previous primary.
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

    /// If upstream RENAMES `PLANNER_RESPONSE`, a step still carrying `tool_calls`
    /// arrives under an unrecognized type. It must SAFE-DEGRADE: the tool starts
    /// are NOT decoded (no spurious unmatched-start sticking the slot Active),
    /// only the previous step's primary end fires — AND it must fire the
    /// `drift::unknown_event` breadcrumb (defense #2, the sole currency backstop
    /// for AG's schema-less format). Both halves are pinned: removing the
    /// breadcrumb call OR changing the event output reddens this.
    #[test]
    fn renamed_planner_step_with_tool_calls_breadcrumbs_and_ends_only() {
        let renamed = serde_json::json!({
            "type": "PLANNER_REPLY", // hypothetical upstream rename of PLANNER_RESPONSE
            "step_index": 2,
            "tool_calls": [ { "name": "read_file", "args": {} } ],
        });
        let logs = crate::test_capture::capture_logs(|| {
            let out = decode_ag_line("/x/t.jsonl", SOURCE_NAME, renamed).unwrap();
            // No spurious start for the un-decoded tool; just the prev-step end.
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

        // Negative: a real result step (no tool_calls) must stay SILENT — the
        // breadcrumb can't false-positive on the legit else-branch traffic.
        let result_step = serde_json::json!({ "type": "TOOL_RESULT", "step_index": 2 });
        let quiet = crate::test_capture::capture_logs(|| {
            decode_ag_line("/x/t.jsonl", SOURCE_NAME, result_step).unwrap();
        });
        assert!(
            !quiet.contains("unknown_event"),
            "a tool_calls-less result step must NOT breadcrumb, got:\n{quiet}"
        );
    }

    /// The Generic display carries the tool's TARGET (quote-stripped), routed
    /// through the shared `generic_tool_display` caps — the observable
    /// contract of `ag_tool_target` (whose dead re-keying predecessor lost
    /// the target entirely).
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
            [
                "run_command: git status", // CommandLine, surrounding quotes stripped
                "grep_search: /repo",      // SearchPath outranks query
                "view_file",               // no recognized field → bare name
            ]
        );
    }

    #[test]
    fn ag_tool_target_falls_through_a_present_non_string_key() {
        // Regression pin for the `first_present_str` switch: a present but
        // non-string HIGHER-priority key must not abort the scan (the old
        // `.or_else` chain `as_str`'d only the first present key and returned
        // None here) — it now falls through. `DirectoryPath` is a number →
        // skipped → the string `AbsolutePath` wins.
        let args = serde_json::json!({ "DirectoryPath": 42, "AbsolutePath": "/repo/x" });
        assert_eq!(ag_tool_target(Some(&args)).as_deref(), Some("/repo/x"));
    }

    // The label / session-ended / default-paths tests live with the runtime
    // half in `native.rs`.
}
