use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "antigravity";

/// Source that watches Antigravity CLI conversation log directories.
/// Uses JsonlWatcher with a custom decoder for the Antigravity JSONL
/// format (step_index/PLANNER_RESPONSE/tool_calls schema).
pub struct AntigravitySource {
    pub brain_root: PathBuf,
}

impl AntigravitySource {
    pub fn default_paths() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            brain_root: PathBuf::from(format!("{home}/.gemini/antigravity-cli/brain")),
        }
    }
}

#[async_trait]
impl Source for AntigravitySource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let watcher = JsonlWatcher::new(
            self.brain_root.clone(),
            SOURCE_NAME.to_string(),
            decode_ag_line,
            derive_ag_label,
            ag_session_ended,
        );
        watcher.run(tx).await
    }
}

pub fn decode_ag_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, transcript_path);
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    // A present-but-non-integer `step_index` (format drift / a renamed field)
    // must fail SAFE-AND-VISIBLE: skip the line rather than coerce to 0, which
    // would silently corrupt the `ag-{step}-{i}` tool_use_id pairing — the
    // `> 0` guard below would then drop the prev-step ActivityEnd, leaving the
    // slot stuck Active until the reducer's debounce/stale-sweep. (The
    // `contains_key` guard only checks presence, not type.)
    let Some(step_index) = obj.get("step_index").and_then(|v| v.as_i64()) else {
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
                let name = tc_obj.get("name").and_then(|s| s.as_str()).unwrap_or("?");
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

fn ag_session_ended(_tail: &[u8]) -> bool {
    false
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
        activity: Activity::Typing,
        tool_use_id: Some(format!("ag-{step_index}-{i}")),
        detail: Some(make_tool_detail(name, Some(&normalized))),
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

fn derive_ag_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label("ag", cwd).unwrap_or_else(|| "ag".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_ag_basename_or_bare_prefix() {
        assert_eq!(
            derive_ag_label(
                Path::new("/x"),
                SOURCE_NAME,
                Path::new("/Users/me/dotfiles")
            ),
            "ag·dotfiles"
        );
        // Empty / root cwd fall back to the bare prefix.
        assert_eq!(
            derive_ag_label(Path::new("/x"), SOURCE_NAME, Path::new("")),
            "ag"
        );
        assert_eq!(
            derive_ag_label(Path::new("/x"), SOURCE_NAME, Path::new("/")),
            "ag"
        );
    }

    #[test]
    fn ag_session_ended_is_always_false() {
        // Antigravity writes no end marker — defer to mtime + stale-sweep.
        assert!(!ag_session_ended(b"x"));
        assert!(!ag_session_ended(b""));
    }
}
