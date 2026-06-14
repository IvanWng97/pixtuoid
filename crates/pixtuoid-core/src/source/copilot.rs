//! GitHub Copilot CLI source. Watches the agentic `copilot` (`@github/copilot`)
//! session transcript (`<copilot_home>/session-state/<sessionId>/events.jsonl`)
//! via `JsonlWatcher`. Transcript-ONLY (Antigravity/Codex-class): the whole
//! lifecycle is persisted to `events.jsonl` — `session.start`,
//! `tool.execution_start/complete`, `permission.requested/completed`,
//! `subagent.started/completed`, `session.task_complete`, `session.shutdown` —
//! so there is no hook install target (the Connection panel shows `cp·` as a
//! no-target flag-flip row, like Antigravity). Only streaming events
//! (`session.idle`, `*_delta`, `*_progress`, …) carry `ephemeral` and never hit
//! disk; the decoder simply ignores everything it doesn't map.
//!
//! Grounded in the canonical schema (npm `@github/copilot` tarball
//! `schemas/session-events.schema.json`) + two real committed on-disk
//! `events.jsonl` files (see `docs/superpowers/specs/2026-06-14-copilot-cli-source-design.md`).
//!
//! Sharp edges (real-byte-confirmed):
//! - **Session id = the PARENT-DIR UUID** of `events.jsonl` (the filename stem is
//!   the constant `events`, NOT the id) — `copilot_id_from_path`.
//! - **Sub-agents INTERLEAVE in the root file**, distinguished by the top-level
//!   envelope `agentId` (== the spawning `task` tool's `data.toolCallId`); there
//!   is no per-agent file split. A line with `agentId` set belongs to that child.
//! - `subagent.completed` is **minimal** on disk (`toolCallId`/`agentName`/
//!   `agentDisplayName` only) — never require model/token/duration fields.
//! - The `ephemeral` envelope flag is inconsistent across CLI versions — never
//!   rely on it; map by `type` and ignore the rest.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::jsonl::JsonlWatcher;
use crate::source::{AgentEvent, Source, TaggedSender, ToolDetail};
use crate::AgentId;

pub const SOURCE_NAME: &str = "copilot";

/// `$COPILOT_HOME` if set, else `~/.copilot`.
pub fn copilot_home() -> PathBuf {
    match std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(crate::platform::user_home()).join(".copilot"),
    }
}

/// The session id = the **parent directory name** of `events.jsonl`
/// (`…/session-state/<sessionId>/events.jsonl`). The filename stem is the
/// constant `events`, so — unlike CC/Codex — the id is the containing dir.
/// Falls back to the stem if there is no parent (defensive).
pub fn copilot_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .or_else(|| path.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

pub fn derive_copilot_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    cwd_basename_label("cp", cwd).unwrap_or_else(|| "cp".to_string())
}

/// Copilot persists a real `session.shutdown` event, so a transcript that has
/// already ended carries that marker — the first-sight gate uses it to avoid
/// resurrecting a finished session.
fn copilot_session_ended(tail: &[u8]) -> bool {
    // Substring scan over the tail window. ANCHOR on the structural `"type"`
    // field — a bare `session.shutdown` would false-positive on tool OUTPUT
    // (e.g. a shell result containing "run session.shutdown the cluster"),
    // seeding the cursor at EOF and silently dropping a live session. Content
    // must never drive lifecycle (the CC sharp edge — its own end-checker only
    // matches structural markers for exactly this reason). events.jsonl is
    // compact JSON with `type` first, so `"type":"session.shutdown"` is the
    // real on-disk shape; `"session_end"` is a quoted defensive alias.
    let hay = String::from_utf8_lossy(tail);
    hay.contains("\"type\":\"session.shutdown\"") || hay.contains("\"session_end\"")
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// Decode one `events.jsonl` line into zero or more `AgentEvent`s. Unknown,
/// ephemeral, or malformed shapes return `vec![]` (the watcher logs + continues;
/// this never panics — real files carry embedded-newline / U+2028 corruption,
/// upstream copilot-cli #2649/#2012).
pub fn decode_copilot_line(
    transcript_path: &str,
    source: &str,
    v: Value,
) -> Result<Vec<AgentEvent>> {
    let root = AgentId::from_parts(source, &copilot_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let kind = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
    let data = obj.get("data");

    // A line tagged with a top-level `agentId` belongs to that sub-agent;
    // otherwise it is the root agent. (Sub-agents interleave in the root file.)
    let acting = match str_at(&v, "agentId") {
        Some(aid) if !aid.is_empty() => AgentId::from_parts(source, aid),
        _ => root,
    };

    let out = match kind {
        "session.start" => {
            let session_id = data.and_then(|d| str_at(d, "sessionId")).unwrap_or("");
            let cwd = data
                .and_then(|d| d.get("context"))
                .and_then(|c| str_at(c, "cwd"))
                .unwrap_or("");
            vec![AgentEvent::SessionStart {
                agent_id: root,
                source: source.to_string(),
                session_id: session_id.to_string(),
                cwd: PathBuf::from(cwd),
                parent_id: None,
            }]
        }
        "tool.execution_start" => {
            let Some(d) = data else { return Ok(vec![]) };
            let Some(tool_call_id) = str_at(d, "toolCallId") else {
                return Ok(vec![]);
            };
            let tool_name = str_at(d, "toolName").unwrap_or("");
            // The sub-agent dispatch is the `task` tool (`arguments.agent_type`);
            // make_tool_detail keys on the CC `subagent_type` field, which Copilot
            // doesn't use — so detect `task` by name here (the child sprite still
            // comes from the explicit subagent.started below).
            let detail = if tool_name == "task" {
                ToolDetail::Task
            } else {
                make_tool_detail(tool_name, d.get("arguments"))
            };
            vec![AgentEvent::ActivityStart {
                agent_id: acting,
                tool_use_id: Some(tool_call_id.to_string()),
                detail: Some(detail),
            }]
        }
        "tool.execution_complete" => {
            let Some(d) = data else { return Ok(vec![]) };
            let Some(tool_call_id) = str_at(d, "toolCallId") else {
                return Ok(vec![]);
            };
            vec![AgentEvent::ActivityEnd {
                agent_id: acting,
                tool_use_id: Some(tool_call_id.to_string()),
            }]
        }
        "permission.requested" => {
            // permissionRequest.kind (write/shell/read/…) names the gate; fall
            // back to a generic reason. (Field shape schema-pinned; the on-disk
            // permission bytes are the one needs-human-verify item.)
            let reason = data
                .and_then(|d| d.get("permissionRequest"))
                .and_then(|p| str_at(p, "kind"))
                .map(|k| format!("permission: {k}"))
                .unwrap_or_else(|| "permission".to_string());
            vec![AgentEvent::Waiting {
                agent_id: acting,
                reason,
            }]
        }
        // Approval resolved. On APPROVED the gated tool's own `tool.execution_start`
        // follows immediately and clears the Waiting gate — so emit nothing (a
        // detail-less ActivityStart here would only inflate tool_call_count). On
        // a DENIAL/cancel no tool runs, so emit the clearing ActivityStart
        // ourselves to un-wait the slot.
        "permission.completed" => {
            let approved = data
                .and_then(|d| d.get("result"))
                .and_then(|r| str_at(r, "kind"))
                .is_some_and(|k| k.starts_with("approved"));
            if approved {
                vec![]
            } else {
                vec![AgentEvent::ActivityStart {
                    agent_id: acting,
                    tool_use_id: None,
                    detail: None,
                }]
            }
        }
        "subagent.started" => {
            // The child id is the envelope `agentId` (== data.toolCallId). Register
            // it as a child of the root session, then name it from the display name.
            let Some(child_key) = str_at(&v, "agentId")
                .filter(|s| !s.is_empty())
                .or_else(|| data.and_then(|d| str_at(d, "toolCallId")))
            else {
                return Ok(vec![]);
            };
            let child = AgentId::from_parts(source, child_key);
            let mut evs = vec![AgentEvent::SessionStart {
                agent_id: child,
                source: source.to_string(),
                session_id: child_key.to_string(),
                cwd: PathBuf::new(), // sub-agents carry no cwd; label comes from Rename
                parent_id: Some(root),
            }];
            if let Some(name) = data.and_then(|d| str_at(d, "agentDisplayName")) {
                evs.push(AgentEvent::Rename {
                    agent_id: child,
                    label: name.to_string(),
                });
            }
            evs
        }
        "subagent.completed" | "subagent.failed" => {
            let Some(child_key) = str_at(&v, "agentId")
                .filter(|s| !s.is_empty())
                .or_else(|| data.and_then(|d| str_at(d, "toolCallId")))
            else {
                return Ok(vec![]);
            };
            vec![AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(source, child_key),
                as_child: true,
            }]
        }
        // A finished task/turn → settle the root agent toward idle.
        "session.task_complete" => vec![AgentEvent::ActivityEnd {
            agent_id: root,
            tool_use_id: None,
        }],
        "session.shutdown" => vec![AgentEvent::SessionEnd {
            agent_id: root,
            as_child: false,
        }],
        // Everything else (ephemeral streaming, assistant.*, hook.*, user.message,
        // session.* metadata) is not a sprite-visible lifecycle change → ignore.
        _ => vec![],
    };
    Ok(out)
}

/// Source that watches the Copilot session-state directory.
pub struct CopilotSource {
    pub sessions_root: PathBuf,
}

impl CopilotSource {
    pub fn default_paths() -> Self {
        Self {
            sessions_root: copilot_home().join("session-state"),
        }
    }
}

impl Source for CopilotSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_copilot_line,
            derive_copilot_label,
            copilot_session_ended,
        )
        .with_id_deriver(copilot_id_from_path)
        .run(tx)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Real on-disk session-state path → id is the PARENT DIR uuid, not "events".
    const PATH: &str = "/p/session-state/65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3/events.jsonl";

    fn root() -> AgentId {
        AgentId::from_parts(SOURCE_NAME, "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3")
    }
    fn decode(line: &str) -> Vec<AgentEvent> {
        decode_copilot_line(PATH, SOURCE_NAME, serde_json::from_str(line).unwrap()).unwrap()
    }

    #[test]
    fn id_from_path_uses_the_parent_dir_not_the_stem() {
        assert_eq!(
            copilot_id_from_path(Path::new(PATH)),
            "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3"
        );
    }

    // ── byte-real lines (verbatim from the committed shreya661 / tamirdresher files) ──

    #[test]
    fn real_session_start_registers_root_with_cwd_and_session_id() {
        let line = r#"{"type":"session.start","data":{"sessionId":"65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3","version":1,"producer":"copilot-agent","copilotVersion":"unknown","startTime":"2026-05-22T05:59:45.408Z","selectedModel":"claude-haiku-4.5","context":{"cwd":"d:\\contentforge-fullstack (1)"},"alreadyInUse":false},"id":"0bc5f1ba-1abe-49c9-a303-d843bd0c3fa8","timestamp":"2026-05-22T05:59:45.488Z","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(source, "copilot");
                assert_eq!(session_id, "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3");
                assert_eq!(cwd, Path::new(r"d:\contentforge-fullstack (1)"));
                assert_eq!(*parent_id, None);
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn real_tool_round_is_active_then_idle_keyed_on_tool_call_id() {
        let start = r#"{"type":"tool.execution_start","data":{"toolCallId":"tooluse_9CoqZL2lZlJUsz7TjJsSUk","toolName":"report_intent","arguments":{"intent":"Exploring project setup"}},"id":"595a6493-1763-4c80-b75a-936d4f263a11","timestamp":"2026-05-22T06:00:14.298Z","parentId":"2902a578-0304-4abc-8402-afefefff9e70"}"#;
        match &decode(start)[..] {
            [AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail: Some(_),
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(
                    tool_use_id.as_deref(),
                    Some("tooluse_9CoqZL2lZlJUsz7TjJsSUk")
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
        let complete = r#"{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_9CoqZL2lZlJUsz7TjJsSUk","model":"claude-haiku-4.5","interactionId":"65f25156-0095-4746-ac3e-fa52340df72b","success":true,"result":{"content":"Intent logged","detailedContent":"Exploring project setup"},"toolTelemetry":{}},"id":"cd7e82e8","timestamp":"2026-05-22T06:00:14.323Z","parentId":"d97de833"}"#;
        match &decode(complete)[..] {
            [AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(
                    tool_use_id.as_deref(),
                    Some("tooluse_9CoqZL2lZlJUsz7TjJsSUk")
                );
            }
            other => panic!("expected ActivityEnd, got {other:?}"),
        }
    }

    #[test]
    fn real_task_tool_is_delegating() {
        // The `task` dispatch (real tamirdresher line, trimmed args) → Delegating.
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk","toolName":"task","arguments":{"description":"Incident command response","agent_type":"sisko","name":"sisko-incident-command","mode":"sync"},"turnId":"0"},"id":"a","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => assert!(d.is_task(), "task tool must be Delegating, got {d:?}"),
            other => panic!("expected Delegating ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn real_subagent_started_registers_child_parented_to_root_then_renamed() {
        let line = r#"{"type":"subagent.started","data":{"toolCallId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk","agentName":"sisko","agentDisplayName":"Sisko - Incident Commander / SRE Lead","agentDescription":"Sisko"},"id":"d171d290","timestamp":"2026-05-26T14:14:22.773Z","parentId":"83d641f1","agentId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk"}"#;
        let child = AgentId::from_parts(SOURCE_NAME, "call_SGMJ1yjMtpgFUbZct2fEo2Hk");
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            }, AgentEvent::Rename { agent_id: r, label }] => {
                assert_eq!(*agent_id, child);
                assert_eq!(*parent_id, Some(root()));
                assert_eq!(*r, child);
                assert_eq!(label, "Sisko - Incident Commander / SRE Lead");
            }
            other => panic!("expected SessionStart+Rename, got {other:?}"),
        }
    }

    #[test]
    fn real_subagent_completed_ends_child_as_child() {
        let line = r#"{"type":"subagent.completed","data":{"toolCallId":"call_kuB1BVYZyE3ih6ClBvbyKtZk","agentName":"rom","agentDisplayName":"Rom - Database Reliability Engineer"},"id":"e7ab205e","timestamp":"2026-05-26T14:14:43.099Z","parentId":"f85ba2bd","agentId":"call_kuB1BVYZyE3ih6ClBvbyKtZk"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "call_kuB1BVYZyE3ih6ClBvbyKtZk")
                );
                assert!(*as_child);
            }
            other => panic!("expected child SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn child_tool_line_attributes_to_the_child_via_envelope_agent_id() {
        // Schema-derived (no public capture logs child tool events with agentId;
        // pins the defensive interleave-demux per the design §8.2).
        let line = json!({
            "type": "tool.execution_start",
            "data": {"toolCallId": "tooluse_child1", "toolName": "view", "arguments": {}},
            "id": "x", "timestamp": "t", "parentId": null,
            "agentId": "call_SGMJ1yjMtpgFUbZct2fEo2Hk"
        })
        .to_string();
        match &decode(&line)[..] {
            [AgentEvent::ActivityStart { agent_id, .. }] => assert_eq!(
                *agent_id,
                AgentId::from_parts(SOURCE_NAME, "call_SGMJ1yjMtpgFUbZct2fEo2Hk"),
                "a line with envelope agentId must attribute to the CHILD, not root"
            ),
            other => panic!("expected child ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn permission_requested_waits_and_completed_clears() {
        // Schema-faithful (the one needs-human-verify item — on-disk permission
        // bytes weren't public). Pins BOTH sides of the gate.
        let req = json!({
            "type": "permission.requested",
            "data": {"requestId": "r1", "permissionRequest": {"kind": "shell", "toolCallId": "tc1"}},
            "id": "p1", "timestamp": "t", "parentId": null
        })
        .to_string();
        match &decode(&req)[..] {
            [AgentEvent::Waiting { agent_id, reason }] => {
                assert_eq!(*agent_id, root());
                assert!(reason.contains("shell"), "reason names the gate: {reason}");
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
        // APPROVED → emit nothing (the approved tool's own start clears Waiting;
        // a phantom ActivityStart would inflate tool_call_count).
        let approved = json!({
            "type": "permission.completed",
            "data": {"requestId": "r1", "result": {"kind": "approved"}},
            "id": "p2", "timestamp": "t", "parentId": null
        })
        .to_string();
        assert!(
            decode(&approved).is_empty(),
            "approved → no event (tool start clears the gate)"
        );

        // DENIED → emit the clearing ActivityStart ourselves (no tool follows).
        let denied = json!({
            "type": "permission.completed",
            "data": {"requestId": "r1", "result": {"kind": "denied-interactively-by-user"}},
            "id": "p3", "timestamp": "t", "parentId": null
        })
        .to_string();
        assert!(matches!(
            &decode(&denied)[..],
            [AgentEvent::ActivityStart { .. }]
        ));
    }

    #[test]
    fn real_session_shutdown_ends_the_root() {
        let line = r#"{"type":"session.shutdown","data":{"shutdownType":"routine","totalPremiumRequests":1},"id":"220c4131","timestamp":"2026-05-22T06:17:01.077Z","parentId":"cd21bd01"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(*agent_id, root());
                assert!(!*as_child, "a root shutdown is NOT a child end");
            }
            other => panic!("expected root SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn ephemeral_unknown_and_malformed_lines_are_ignored_not_panicked() {
        // session.idle is ephemeral (never on disk, but be defensive); an unknown
        // type, a missing-data tool line, and a non-object are all no-ops.
        assert!(decode(
            r#"{"type":"session.idle","data":{},"id":"i","timestamp":"t","parentId":null}"#
        )
        .is_empty());
        assert!(decode(r#"{"type":"assistant.message_delta","data":{},"id":"d","timestamp":"t","parentId":null}"#).is_empty());
        assert!(decode(
            r#"{"type":"tool.execution_start","id":"n","timestamp":"t","parentId":null}"#
        )
        .is_empty());
        assert!(
            decode_copilot_line(PATH, SOURCE_NAME, json!("not an object"))
                .unwrap()
                .is_empty()
        );
        assert!(decode_copilot_line(PATH, SOURCE_NAME, json!(["array"]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn session_ended_marker_is_anchored_on_the_type_field() {
        // Real compact on-disk shape → ended.
        assert!(copilot_session_ended(
            br#"...{"type":"session.shutdown","data":{}}"#
        ));
        // A tool result that merely MENTIONS the string must NOT end the session
        // (content must never drive lifecycle — the CC sharp edge).
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_complete","data":{"result":{"content":"run session.shutdown the cluster"}}}"#
        ));
        assert!(!copilot_session_ended(
            br#"{"type":"tool.execution_start"}"#
        ));
    }
}
