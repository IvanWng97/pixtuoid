use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::source::decoder::make_tool_detail;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

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
        "antigravity"
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let watcher = JsonlWatcher::new(
            self.brain_root.clone(),
            "antigravity".to_string(),
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

    if !obj.contains_key("step_index") {
        return Ok(vec![]);
    }

    let step_index = obj.get("step_index").and_then(|v| v.as_i64()).unwrap_or(0);
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
                if name == "ask_permission" || name == "ask_question" {
                    out.push(AgentEvent::Waiting {
                        agent_id,
                        reason: "asking permission".to_string(),
                    });
                } else {
                    let mut normalized_input = serde_json::Map::new();
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
                            normalized_input
                                .insert(key.to_string(), Value::String(clean.to_string()));
                        }
                    }
                    let normalized = Value::Object(normalized_input);
                    out.push(AgentEvent::ActivityStart {
                        agent_id,
                        activity: Activity::Typing,
                        tool_use_id: Some(format!("ag-{step_index}-{i}")),
                        detail: Some(make_tool_detail(name, Some(&normalized))),
                    });
                }
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

fn derive_ag_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    if cwd != Path::new("") && cwd != Path::new("/") {
        if let Some(name) = cwd.file_name().and_then(|n| n.to_str()) {
            return format!("ag·{name}");
        }
    }

    "ag".to_string()
}
