//! Reasonix source — HOOK-ONLY (no JSONL watcher).
//!
//! Reasonix (github.com/esengine/DeepSeek-Reasonix) ships a CC-style hook
//! system; connecting it registers the shim in the GLOBAL
//! `~/.reasonix/settings.json` — project-scope hooks are trust-gated and would
//! silently not fire. Payloads arrive on the shared hook socket stamped
//! `_pixtuoid_source: "reasonix"` in Reasonix's own envelope, NOT the CC shape:
//!
//! ```json
//! {"event":"PreToolUse","cwd":"/repo","toolName":"bash","toolArgs":{"command":"ls"}}
//! ```
//!
//! camelCase fields, `event` discriminator (not `hook_event_name`), and — the
//! load-bearing difference — **no session id, no transcript path, no tool-call
//! id anywhere** (verified against `internal/hook/hook.go` Payload @v1.2.0).
//! The only situating field is `cwd` (the workspace root), so the AgentId is
//! keyed on it — two concurrent Reasonix sessions in ONE project deliberately
//! render as one sprite, and either session's `SessionEnd` walks that shared
//! sprite out (it walks back in on the other's next `UserPromptSubmit`).
//!
//! Why no JSONL transport: v2 session files are FULL-REWRITTEN (tmp+rename)
//! once per turn — zero mid-turn writes, so a tail watcher shows nothing while
//! the agent works; headless `reasonix run` writes no session file at all; and
//! with no shared key between the file name and hook payloads, a JSONL agent
//! could never coalesce with the hook agent (guaranteed two sprites).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The Reasonix CLI source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "reasonix";

/// Reasonix tools that dispatch an in-process subagent (`internal/agent/task.go`
/// plus `internal/skill/tools.go` @v1.2.0), so the slot reads "Delegating" while
/// the hook-invisible subagent works. `run_skill` is excluded: it is only
/// sometimes a subagent and the args don't say which.
const SUBAGENT_TOOLS: &[&str] = &["task", "explore", "research", "review", "security_review"];

/// Decode one Reasonix hook payload (already identified by
/// `_pixtuoid_source == "reasonix"`). Envelope verified against
/// `internal/hook/hook.go` @v1.2.0. An unregistered event bails rather than
/// dropping silently, so registered-vs-decoded drift is loud.
///
/// The activity arms prepend an [`AgentEvent::Identity`] (#221) because
/// Reasonix is HOOK-ONLY: a slot the reducer's proof-of-life pre-pass
/// synthesizes mid-turn has no JSONL back-fill path, so without the attached
/// identity it would stay a blank `#N` ghost until the next `UserPromptSubmit`.
pub fn decode_rx_hook_payload(v: &Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("reasonix hook payload must be an object"))?;
    let event = obj
        .get("event")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("reasonix payload missing event"))?;
    // `cwd` is the ONLY identity a Reasonix hook carries — an empty one would
    // mint a phantom agent that nothing else coalesces with.
    let cwd = obj
        .get("cwd")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("reasonix payload missing/empty cwd"))?;
    let agent_id = AgentId::from_parts(SOURCE_NAME, cwd);

    // No upstream session id exists; the cwd IS the session key, and the
    // SessionStart arm below must key it identically or coalescing breaks.
    let identity = || AgentEvent::identity(agent_id, SOURCE_NAME, cwd, Some(cwd.into()));

    match event {
        // The `UserPromptSubmit` duplicate is the RESURRECT path — a
        // stale-swept session walks back in on its next prompt, and Reasonix
        // has no other re-creation signal.
        "SessionStart" | "UserPromptSubmit" => Ok(vec![AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: cwd.to_string(),
            cwd: cwd.into(),
            parent_id: None,
        }]),
        "PreToolUse" => {
            let tool = obj
                .get("toolName")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(SOURCE_NAME, "PreToolUse", "toolName");
                    "?"
                });
            Ok(vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: None,
                    detail: Some(rx_tool_detail(tool, obj.get("toolArgs"))),
                },
            ])
        }
        "PostToolUse" => Ok(vec![
            identity(),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: None,
            },
        ]),
        // Deliberately Identity-LESS: an end doesn't prove a session worth
        // registering, so an unknown id here rides the reducer's fallback.
        "Stop" => Ok(vec![AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }]),
        "Notification" => {
            // Upstream's only Notification producer is the approval gate:
            // "approval needed: <tool> <subject>".
            let msg = obj
                .get("message")
                .and_then(|s| s.as_str())
                .unwrap_or("waiting");
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    reason: ellipsize(msg, MAX_DECODED_FIELD_CHARS),
                },
            ])
        }
        // The STRUCTURED approval gate: fires BEFORE the prompt with the tool +
        // subject, and ALONGSIDE `Notification` for the same approval — both →
        // Waiting is idempotent (the gated tool's PostToolUse/Stop resolves it).
        // NOT live-capturable: the gate never fires in headless `reasonix run`
        // (auto-approve), so this arm is decoded from upstream source.
        "PermissionRequest" => {
            let tool = obj
                .get("toolName")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty());
            if tool.is_none() {
                crate::source::drift::missing_field(SOURCE_NAME, "PermissionRequest", "toolName");
            }
            let subject = obj
                .get("subject")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty());
            // Pre-cap each component before concatenating so an oversized
            // `subject` can't force a large intermediate allocation; the outer
            // `ellipsize` stays the authoritative cap.
            let reason = match (tool, subject) {
                (Some(t), Some(s)) => format!(
                    "approval: {} {}",
                    ellipsize(t, MAX_DECODED_FIELD_CHARS),
                    ellipsize(s, MAX_DECODED_FIELD_CHARS),
                ),
                (Some(t), None) => format!("approval: {}", ellipsize(t, MAX_DECODED_FIELD_CHARS)),
                (None, _) => "approval required".to_string(),
            };
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    reason: ellipsize(&reason, MAX_DECODED_FIELD_CHARS),
                },
            ])
        }
        "SessionEnd" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        other => {
            crate::source::drift::unknown_event(SOURCE_NAME, other);
            bail!(
                "unsupported reasonix hook event: {}",
                crate::source::decoder::display_safe(other)
            )
        }
    }
}

/// Reasonix-side tool detail: the dispatch family is name-keyed because Reasonix
/// args carry no `subagent_type` for the shared semantic detection to see.
/// Everything else gets a `"name: target"` display from the keys the v1.2.0
/// builtin tools actually emit — `path`, NOT CC's `file_path` (no builtin uses
/// it); `url` is `web_fetch`.
fn rx_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    if SUBAGENT_TOOLS.contains(&tool) {
        return ToolDetail::Task;
    }
    const KEYS: &[&str] = &["command", "path", "pattern", "url"];
    crate::source::decoder::generic_keyed_detail(tool, args, KEYS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all(v: Value) -> Vec<AgentEvent> {
        decode_rx_hook_payload(&v).expect("decodes")
    }

    fn decode(v: Value) -> AgentEvent {
        decode_all(v).pop().expect("at least one event")
    }

    #[test]
    fn session_start_keys_on_cwd() {
        let ev = decode(json!({
            "event": "SessionStart",
            "cwd": "/Users/dev/zirs"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                parent_id,
                ..
            } => {
                assert_eq!(source, SOURCE_NAME);
                assert_eq!(
                    agent_id,
                    AgentId::from_parts(SOURCE_NAME, "/Users/dev/zirs")
                );
                assert_eq!(cwd, std::path::PathBuf::from("/Users/dev/zirs"));
                assert_eq!(parent_id, None);
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn user_prompt_submit_is_the_resurrect_session_start() {
        let ev = decode(json!({
            "event": "UserPromptSubmit",
            "cwd": "/Users/dev/zirs",
            "prompt": "add a feature"
        }));
        assert!(matches!(ev, AgentEvent::SessionStart { agent_id, .. }
                if agent_id == AgentId::from_parts(SOURCE_NAME, "/Users/dev/zirs")));
    }

    #[test]
    fn pre_tool_use_is_activity_start_with_no_tool_id() {
        let ev = decode(json!({
            "event": "PreToolUse",
            "cwd": "/repo",
            "toolName": "read_file",
            "toolArgs": {"path": "src/main.go"}
        }));
        match ev {
            AgentEvent::ActivityStart {
                tool_use_id,
                detail,
                ..
            } => {
                assert_eq!(tool_use_id, None);
                assert_eq!(detail.unwrap().display(), "read_file: src/main.go");
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn tool_target_uses_reasonix_arg_vocabulary() {
        let bash = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "bash", "toolArgs": {"command": "go test ./..."}
        }));
        assert!(
            matches!(bash, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "bash: go test ./...")
        );
        let grep = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "grep", "toolArgs": {"pattern": "TODO", "path": "src"}
        }));
        assert!(
            matches!(grep, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "grep: src")
        );
    }

    #[test]
    fn long_targets_are_truncated() {
        let long = "x".repeat(60);
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "bash", "toolArgs": {"command": long}
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(display.starts_with("bash: "));
                assert!(display.ends_with('…'));
                assert_eq!(display.chars().count(), "bash: ".chars().count() + 41);
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn long_tool_name_is_truncated_at_the_decode_boundary() {
        let long = "T".repeat(MAX_DECODED_FIELD_CHARS * 3);
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": long, "toolArgs": {}
        }));
        match ev {
            AgentEvent::ActivityStart {
                detail: Some(d), ..
            } => {
                let display = d.display();
                assert!(
                    display.ends_with('…'),
                    "name should be ellipsized: {display}"
                );
                assert_eq!(display.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn subagent_dispatch_family_maps_to_task() {
        for tool in ["task", "explore", "research", "review", "security_review"] {
            let ev = decode(json!({
                "event": "PreToolUse", "cwd": "/r",
                "toolName": tool, "toolArgs": {"prompt": "do a thing"}
            }));
            assert!(
                matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if d.is_task()),
                "{tool} must map to ToolDetail::Task"
            );
        }
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "run_skill", "toolArgs": {"name": "lint"}
        }));
        assert!(matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()));
    }

    #[test]
    fn post_tool_use_and_stop_are_activity_end() {
        for event in ["PostToolUse", "Stop"] {
            let ev = decode(json!({"event": event, "cwd": "/r"}));
            assert!(
                matches!(
                    &ev,
                    AgentEvent::ActivityEnd {
                        tool_use_id: None,
                        ..
                    }
                ),
                "{event} must decode to ActivityEnd with no tool id"
            );
        }
    }

    #[test]
    fn notification_maps_to_waiting_with_message() {
        let ev = decode(json!({
            "event": "Notification",
            "cwd": "/r",
            "message": "approval needed: bash rm -rf ./build"
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval needed: bash rm -rf ./build"));
    }

    #[test]
    fn session_end_maps_to_session_end() {
        let ev = decode(json!({"event": "SessionEnd", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
    }

    #[test]
    fn permission_request_maps_to_waiting_with_tool_and_subject() {
        let ev = decode(json!({
            "event": "PermissionRequest",
            "cwd": "/r",
            "toolName": "bash",
            "subject": "rm -rf ./build",
            "toolArgs": {"command": "rm -rf ./build"}
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval: bash rm -rf ./build"));
    }

    #[test]
    fn permission_request_without_subject_uses_tool_only() {
        let ev = decode(json!({
            "event": "PermissionRequest", "cwd": "/r", "toolName": "write_file"
        }));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval: write_file"));
    }

    #[test]
    fn permission_request_without_tool_falls_back() {
        let ev = decode(json!({"event": "PermissionRequest", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. }
            if reason == "approval required"));
    }

    #[test]
    fn permission_request_reason_is_capped_at_the_decode_boundary() {
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let ev = decode(json!({
            "event": "PermissionRequest", "cwd": "/r",
            "toolName": "bash", "subject": long
        }));
        match &ev {
            AgentEvent::Waiting { reason, .. } => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }

    #[test]
    fn all_events_for_one_cwd_share_one_agent_id() {
        let events = [
            json!({"event": "SessionStart", "cwd": "/Users/dev/p"}),
            json!({"event": "UserPromptSubmit", "cwd": "/Users/dev/p"}),
            json!({"event": "PreToolUse", "cwd": "/Users/dev/p", "toolName": "bash",
                   "toolArgs": {"command": "ls"}}),
            json!({"event": "PostToolUse", "cwd": "/Users/dev/p", "toolName": "bash"}),
            json!({"event": "PermissionRequest", "cwd": "/Users/dev/p", "toolName": "bash", "subject": "ls"}),
            json!({"event": "Notification", "cwd": "/Users/dev/p", "message": "approval needed: bash ls"}),
            json!({"event": "Stop", "cwd": "/Users/dev/p"}),
            json!({"event": "SessionEnd", "cwd": "/Users/dev/p"}),
        ];
        let ids: std::collections::BTreeSet<_> = events
            .iter()
            .flat_map(|v| decode_rx_hook_payload(v).unwrap())
            .map(|e| e.agent_id())
            .collect();
        assert_eq!(ids.len(), 1, "all events must coalesce to one AgentId");
    }

    #[test]
    fn activity_arms_prepend_identity_with_cwd_keyed_session() {
        let payloads = [
            json!({"event": "PreToolUse", "cwd": "/Users/dev/p", "toolName": "bash",
                   "toolArgs": {"command": "ls"}}),
            json!({"event": "PostToolUse", "cwd": "/Users/dev/p", "toolName": "bash"}),
            json!({"event": "Notification", "cwd": "/Users/dev/p", "message": "approval needed"}),
            json!({"event": "PermissionRequest", "cwd": "/Users/dev/p", "toolName": "bash", "subject": "ls"}),
        ];
        for payload in payloads {
            let name = payload["event"].clone();
            let events = decode_all(payload);
            assert_eq!(events.len(), 2, "{name}: Identity + activity");
            match &events[0] {
                AgentEvent::Identity {
                    agent_id,
                    source,
                    session_id,
                    cwd,
                    pid: None,
                } => {
                    assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "/Users/dev/p"));
                    assert_eq!(source, SOURCE_NAME);
                    assert_eq!(session_id, "/Users/dev/p", "cwd IS the session key");
                    assert_eq!(
                        cwd.as_deref(),
                        Some(std::path::Path::new("/Users/dev/p")),
                        "rx hooks always know their cwd"
                    );
                }
                other => panic!("{name}: expected leading Identity, got {other:?}"),
            }
        }
    }

    #[test]
    fn stop_session_events_and_session_end_carry_no_identity() {
        for payload in [
            json!({"event": "Stop", "cwd": "/r"}),
            json!({"event": "SessionStart", "cwd": "/r"}),
            json!({"event": "UserPromptSubmit", "cwd": "/r"}),
            json!({"event": "SessionEnd", "cwd": "/r"}),
        ] {
            let name = payload["event"].clone();
            let events = decode_all(payload);
            assert_eq!(events.len(), 1, "{name}: exactly one event");
            assert!(
                !matches!(events[0], AgentEvent::Identity { .. }),
                "{name} must not emit Identity"
            );
        }
    }

    #[test]
    fn empty_or_missing_cwd_is_malformed() {
        assert!(decode_rx_hook_payload(&json!({"event": "Stop", "cwd": ""})).is_err());
        assert!(decode_rx_hook_payload(&json!({"event": "Stop"})).is_err());
    }

    #[test]
    fn unknown_event_bails_loudly() {
        for ev in ["PostLLMCall", "PreCompact", "SubagentStop", "Bogus"] {
            assert!(
                decode_rx_hook_payload(&json!({"event": ev, "cwd": "/r"})).is_err(),
                "{ev} must bail (not registered, must not decode silently)"
            );
        }
    }

    #[test]
    fn non_object_payload_is_malformed() {
        assert!(decode_rx_hook_payload(&json!("just a string")).is_err());
        assert!(decode_rx_hook_payload(&json!(42)).is_err());
    }

    #[test]
    fn notification_without_message_falls_back_to_waiting() {
        let ev = decode(json!({"event": "Notification", "cwd": "/r"}));
        assert!(matches!(ev, AgentEvent::Waiting { reason, .. } if reason == "waiting"));
    }

    #[test]
    fn pre_tool_use_without_tool_name_displays_question_mark() {
        let ev = decode(json!({"event": "PreToolUse", "cwd": "/r"}));
        assert!(
            matches!(ev, AgentEvent::ActivityStart { detail: Some(d), .. }
            if d.display() == "?")
        );
    }

    #[test]
    fn tool_args_spoofed_subagent_type_does_not_make_task() {
        let ev = decode(json!({
            "event": "PreToolUse", "cwd": "/r",
            "toolName": "read_file",
            "toolArgs": {"path": "x.go", "subagent_type": null}
        }));
        assert!(
            matches!(&ev, AgentEvent::ActivityStart { detail: Some(d), .. } if !d.is_task()),
            "spoofed subagent_type must stay Generic"
        );
    }

    #[test]
    fn notification_reason_is_capped_at_the_decode_boundary() {
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let ev = decode(json!({
            "event": "Notification",
            "cwd": "/r",
            "message": long
        }));
        match &ev {
            AgentEvent::Waiting { reason, .. } => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }
}
