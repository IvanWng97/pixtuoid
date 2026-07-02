use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::make_tool_detail;
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
    } else if step_type != "USER_INPUT" && step_type != "CONVERSATION_HISTORY" && step_index > 0 {
        // End the first tool from the previous step. Multi-tool steps have
        // their remaining starts aged out by the reducer's pending_idle
        // debounce, but the primary (i=0) start always gets a matching end.
        out.push(AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: Some(format!("ag-{}-0", step_index - 1)),
        });
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
    let normalized = normalize_ag_tool_input(name, args);
    AgentEvent::ActivityStart {
        agent_id,
        tool_use_id: Some(format!("ag-{step_index}-{i}")),
        detail: Some(make_tool_detail(SOURCE_NAME, name, Some(&normalized))),
    }
}

/// Normalize an Antigravity tool call's `args` to the `{key: value}` shape
/// `make_tool_detail` reads: pick the first present path/command field, strip
/// surrounding quotes, and key it by the tool's category. Returns an empty
/// object when no recognized field is present.
fn normalize_ag_tool_input(name: &str, args: Option<&Value>) -> Value {
    let mut normalized = serde_json::Map::new();
    if let Some(args_obj) = args.and_then(|v| v.as_object()) {
        let raw_val = args_obj
            .get("DirectoryPath")
            .or_else(|| args_obj.get("AbsolutePath"))
            .or_else(|| args_obj.get("TargetFile"))
            .or_else(|| args_obj.get("CommandLine"))
            .or_else(|| args_obj.get("SearchPath"))
            .or_else(|| args_obj.get("query"))
            .and_then(|v| v.as_str());
        if let Some(s) = raw_val {
            let clean = s
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(s);
            let key = match name {
                "run_command" => "command",
                "grep_search" => "pattern",
                _ => "file_path",
            };
            normalized.insert(key.to_string(), Value::String(clean.to_string()));
        }
    }
    Value::Object(normalized)
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

    // The label / session-ended / default-paths tests live with the runtime
    // half in `native.rs`.
}
