use pixtuoid_core::source::antigravity;
use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{Activity, AgentEvent};
use pixtuoid_core::AgentId;
use serde_json::json;

fn load(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/fixtures/hooks/{name}.json")).unwrap();
    serde_json::from_str(&s).unwrap()
}

fn load_jsonl(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/fixtures/jsonl/{name}.json")).unwrap();
    serde_json::from_str(&s).unwrap()
}

#[test]
fn decode_session_start() {
    let ev = decode_hook_payload(load("session_start")).unwrap();
    let expected_id = AgentId::from_transcript_path("/Users/me/.claude/projects/x/ses-abc.jsonl");
    match ev {
        AgentEvent::SessionStart {
            agent_id,
            session_id,
            source,
            ..
        } => {
            assert_eq!(agent_id, expected_id);
            assert_eq!(session_id, "ses-abc");
            assert_eq!(source, "claude-code");
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn decode_session_start_with_custom_source() {
    let mut payload = load("session_start");
    payload["_pixtuoid_source"] = serde_json::Value::String("antigravity".into());
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::SessionStart { source, .. } => {
            assert_eq!(source, "antigravity");
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn decode_pre_tool_use_write_maps_to_typing() {
    let ev = decode_hook_payload(load("pre_tool_use_write")).unwrap();
    match ev {
        AgentEvent::ActivityStart {
            activity, detail, ..
        } => {
            assert_eq!(activity, Activity::Typing);
            assert!(detail.unwrap().display().contains("Write"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_post_tool_use_is_activity_end() {
    let ev = decode_hook_payload(load("post_tool_use_write")).unwrap();
    assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
}

#[test]
fn decode_notification_is_waiting() {
    let ev = decode_hook_payload(load("notification")).unwrap();
    match ev {
        AgentEvent::Waiting { reason, .. } => assert!(reason.contains("permission")),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_session_end() {
    let ev = decode_hook_payload(load("session_end")).unwrap();
    assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
}

#[test]
fn decode_unknown_event_returns_err() {
    let mut bad = load("session_start");
    bad["hook_event_name"] = serde_json::Value::String("UnknownThing".into());
    assert!(decode_hook_payload(bad).is_err());
}

// Codex subagents (`spawn_agent`) signal their lifecycle ONLY via the
// SubagentStart/SubagentStop hooks: the subagent's own rollout renders the
// sprite but is keyed flat (no `/subagents/` path), so it can't learn its
// parent. The hooks carry a distinct `agent_id` (the subagent, == its
// rollout-filename UUID) plus the parent `session_id`. SubagentStart keys the
// CHILD on `agent_id` and links it to the parent — wiring it into the scope
// tree. Captured live (Codex 0.135, gpt-5.5): the payload carries
// agent_id/agent_type/turn_id beside the common session_id/cwd/transcript_path.
#[test]
fn codex_subagent_start_links_child_to_parent() {
    let ev = decode_hook_payload(json!({
        "hook_event_name": "SubagentStart",
        "session_id": "parent-sess",
        "agent_id": "child-agent",
        "agent_type": "default",
        "turn_id": "turn-1",
        "cwd": "/home/user/demo-project",
        "_pixtuoid_source": "codex"
    }))
    .expect("SubagentStart decodes");
    match ev {
        AgentEvent::SessionStart {
            agent_id,
            source,
            cwd,
            parent_id,
            ..
        } => {
            assert_eq!(source, "codex");
            assert_eq!(
                agent_id,
                AgentId::from_parts("codex", "child-agent"),
                "child keyed on agent_id (coalesces with the subagent rollout UUID)"
            );
            assert_eq!(
                parent_id,
                Some(AgentId::from_parts("codex", "parent-sess")),
                "linked to the parent session"
            );
            assert_eq!(cwd, std::path::PathBuf::from("/home/user/demo-project"));
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn codex_subagent_stop_ends_child_not_parent() {
    let ev = decode_hook_payload(json!({
        "hook_event_name": "SubagentStop",
        "session_id": "parent-sess",
        "agent_id": "child-agent",
        "agent_type": "default",
        "stop_hook_active": false,
        "_pixtuoid_source": "codex"
    }))
    .expect("SubagentStop decodes");
    match ev {
        AgentEvent::SessionEnd { agent_id } => assert_eq!(
            agent_id,
            AgentId::from_parts("codex", "child-agent"),
            "ends the CHILD (keyed on agent_id), never the parent session"
        ),
        other => panic!("expected SessionEnd, got {other:?}"),
    }
}

// A Subagent hook with an absent OR empty agent_id must be rejected (Err →
// logged + skipped by the listener), never default to "" and key a phantom
// child that would never coalesce with the real rollout.
#[test]
fn codex_subagent_hooks_reject_missing_or_empty_agent_id() {
    for event in ["SubagentStart", "SubagentStop"] {
        // absent
        assert!(
            decode_hook_payload(json!({
                "hook_event_name": event,
                "session_id": "parent-sess",
                "_pixtuoid_source": "codex"
            }))
            .is_err(),
            "{event} without agent_id must Err"
        );
        // present-but-empty
        assert!(
            decode_hook_payload(json!({
                "hook_event_name": event,
                "session_id": "parent-sess",
                "agent_id": "",
                "_pixtuoid_source": "codex"
            }))
            .is_err(),
            "{event} with empty agent_id must Err"
        );
    }
}

#[test]
fn cc_jsonl_assistant_tool_use_is_activity_start() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events =
        decode_cc_line(transcript, "claude-code", load_jsonl("assistant_tool_use")).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityStart {
            activity,
            tool_use_id,
            detail,
            ..
        } => {
            assert_eq!(*activity, Activity::Typing);
            assert_eq!(tool_use_id.as_deref(), Some("tu_123"));
            assert!(detail.as_ref().unwrap().display().contains("Write"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn cc_jsonl_tool_result_is_activity_end() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events = decode_cc_line(transcript, "claude-code", load_jsonl("tool_result")).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("tu_123"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_hook_payload_with_multibyte_tool_input_does_not_panic() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-zh",
        "transcript_path": "/tmp/zh.jsonl",
        "cwd": "/tmp",
        "tool_name": "Bash",
        "tool_input": {
            "command": "echo 这是一个非常长的中文命令需要被截断这是一个非常长的中文命令需要被截断"
        }
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert!(d.display().contains("Bash"), "got: {}", d.display());
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

#[test]
fn decode_pre_tool_use_carries_tool_use_id_from_payload() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/Users/me/.claude/projects/x/ses-abc.jsonl",
        "cwd": "/repo",
        "tool_name": "Task",
        "tool_use_id": "toolu_01ABC",
        "tool_input": { "description": "go" }
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart {
            tool_use_id,
            detail,
            ..
        } => {
            assert_eq!(tool_use_id.as_deref(), Some("toolu_01ABC"));
            assert!(detail.expect("detail set").is_task());
        }
        other => panic!("got {other:?}"),
    }
}

// Real CC (verified across ~/.claude/projects: 26K messages, "Agent" 47× and
// "Task" 0×) dispatches subagents via a tool named "Agent" — NOT "Task". Its
// input carries {description, prompt, subagent_type}. Task-detection must
// recognise it, else `active_tasks` subagent-leak suppression and b1 Task-drain
// completion never fire for real subagents (the parent shows the subagent's
// tools — observed live). Both names map to `ToolDetail::Task`.
#[test]
fn decode_pre_tool_use_agent_tool_is_task() {
    for tool in ["Agent", "Task"] {
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "cwd": "/repo",
            "tool_name": tool,
            "tool_use_id": "toolu_01ABC",
            "tool_input": { "description": "go", "subagent_type": "Explore" }
        });
        match decode_hook_payload(payload).unwrap() {
            AgentEvent::ActivityStart { detail, .. } => assert!(
                detail.expect("detail set").is_task(),
                "{tool} must be Task-detected"
            ),
            other => panic!("got {other:?}"),
        }
    }
}

// Resilience: detect a dispatch by its `subagent_type` input, so the NEXT
// rename (Task→Agent→…?) doesn't silently break suppression/completion. A tool
// under a name we've never seen, but carrying subagent_type, is still a Task.
#[test]
fn subagent_dispatch_detected_by_subagent_type_under_novel_name() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/p/ses-abc.jsonl",
        "cwd": "/repo",
        "tool_name": "Delegate2027",
        "tool_use_id": "toolu_01ZZ",
        "tool_input": { "description": "go", "subagent_type": "Explore" }
    });
    match decode_hook_payload(payload).unwrap() {
        AgentEvent::ActivityStart { detail, .. } => assert!(
            detail.expect("detail").is_task(),
            "a tool carrying subagent_type is a dispatch regardless of its name"
        ),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn non_dispatch_tool_is_not_task() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "s",
        "transcript_path": "/p/s.jsonl",
        "cwd": "/repo",
        "tool_name": "Read",
        "tool_use_id": "t",
        "tool_input": { "file_path": "/x" }
    });
    match decode_hook_payload(payload).unwrap() {
        AgentEvent::ActivityStart { detail, .. } => {
            assert!(!detail.expect("detail").is_task(), "Read is not a dispatch")
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn cc_jsonl_agent_tool_use_is_task() {
    let line = serde_json::json!({
        "type": "assistant",
        "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Agent",
             "input": {"description": "x", "subagent_type": "general-purpose"}}
        ]}
    });
    let events = decode_cc_line("/p/parent.jsonl", "claude-code", line).unwrap();
    let task = events.iter().find_map(|e| match e {
        AgentEvent::ActivityStart { detail, .. } => detail.as_ref(),
        _ => None,
    });
    assert!(
        task.expect("ActivityStart present").is_task(),
        "the JSONL 'Agent' tool_use must be Task-detected too"
    );
}

#[test]
fn decode_post_tool_use_carries_tool_use_id_from_payload() {
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/Users/me/.claude/projects/x/ses-abc.jsonl",
        "cwd": "/repo",
        "tool_name": "Task",
        "tool_use_id": "toolu_01ABC"
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("toolu_01ABC"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn cc_jsonl_subagent_line_with_attribution_emits_rename() {
    let transcript = "/Users/me/.claude/projects/x/sess/subagents/agent-abc.jsonl";
    let v = serde_json::json!({
        "type": "assistant",
        "sessionId": "sess",
        "cwd": "/repo",
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Read",
                  "input": { "file_path": "/repo/src/a.rs" } }
            ]
        }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    let has_rename = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::Rename { label, .. } if label == "code-explorer"
        )
    });
    assert!(has_rename, "expected Rename event, got {events:?}");
}

#[test]
fn cc_jsonl_plain_user_message_yields_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events = decode_cc_line(transcript, "claude-code", load_jsonl("user_message")).unwrap();
    assert!(events.is_empty());
}

// CC writes no `session_end` line on `/exit` — only a `<command-name>` user
// event. Decoding it to SessionEnd gives the durable JSONL transport an exit
// signal so a cleanly-exited session is reaped even when the best-effort
// SessionEnd hook is dropped.
#[test]
fn cc_jsonl_exit_command_emits_session_end() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let v = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": "<command-name>/exit</command-name>\n            <command-message>exit</command-message>\n            <command-args></command-args>"
        }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    assert_eq!(events.len(), 1, "got {events:?}");
    assert!(matches!(events[0], AgentEvent::SessionEnd { .. }));
}

#[test]
fn cc_jsonl_quit_command_emits_session_end() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let v = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": "<command-name>/quit</command-name>" }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    assert_eq!(events.len(), 1, "got {events:?}");
    assert!(matches!(events[0], AgentEvent::SessionEnd { .. }));
}

// `/clear` and `/compact` keep the session (and process) alive — they must
// NOT be treated as session-terminating.
#[test]
fn cc_jsonl_non_terminating_slash_command_yields_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    for cmd in ["/clear", "/compact"] {
        let v = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": format!("<command-name>{cmd}</command-name>") }
        });
        let events = decode_cc_line(transcript, "claude-code", v).unwrap();
        assert!(
            events.is_empty(),
            "{cmd} should not end the session: {events:?}"
        );
    }
}

#[test]
fn cc_jsonl_plain_string_user_message_yields_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let v = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": "please fix the /exit bug" }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    assert!(
        events.is_empty(),
        "prose mentioning /exit is not a command: {events:?}"
    );
}

#[test]
fn ag_planner_response_emits_activity_start_with_indexed_tool_use_id() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = serde_json::json!({
        "step_index": 2,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "list_dir", "args": { "DirectoryPath": "\"/repo/src\"" } },
            { "name": "read_file", "args": { "AbsolutePath": "\"/repo/README.md\"" } }
        ]
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    assert_eq!(events.len(), 2);
    match &events[0] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-0"));
        }
        other => panic!("got {other:?}"),
    }
    match &events[1] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-1"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn ag_tool_result_emits_activity_end() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = serde_json::json!({
        "step_index": 3,
        "type": "LIST_DIRECTORY",
        "content": "output"
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-0"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn ag_uses_source_namespaced_agent_id() {
    let transcript = "/shared/path.jsonl";
    let v = serde_json::json!({ "step_index": 1, "type": "PLANNER_RESPONSE", "tool_calls": [] });
    let _events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    let ag_id = AgentId::from_parts("antigravity", transcript);
    let cc_id = AgentId::from_parts("claude-code", transcript);
    assert_ne!(
        ag_id, cc_id,
        "different sources must produce different AgentIds"
    );
}

#[test]
fn ag_ask_permission_and_question_emits_waiting() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";

    // ask_permission tool call
    let v_perm = serde_json::json!({
        "step_index": 4,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "ask_permission", "args": { "Reason": "read a file" } }
        ]
    });
    let events_perm = antigravity::decode_ag_line(transcript, "antigravity", v_perm).unwrap();
    assert_eq!(events_perm.len(), 1);
    match &events_perm[0] {
        AgentEvent::Waiting { reason, .. } => {
            assert_eq!(reason, "asking permission");
        }
        other => panic!("expected Waiting, got {other:?}"),
    }

    // ask_question tool call
    let v_quest = serde_json::json!({
        "step_index": 5,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "ask_question", "args": { "questions": [] } }
        ]
    });
    let events_quest = antigravity::decode_ag_line(transcript, "antigravity", v_quest).unwrap();
    assert_eq!(events_quest.len(), 1);
    match &events_quest[0] {
        AgentEvent::Waiting { reason, .. } => {
            assert_eq!(reason, "asking permission");
        }
        other => panic!("expected Waiting, got {other:?}"),
    }
}

#[test]
fn cc_session_ended_detects_session_end_subtype() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
{"type":"system","subtype":"session_end","sessionId":"s1"}
"#;
    assert!(cc_session_ended(tail));
}

#[test]
fn cc_session_ended_returns_false_for_active_session() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
"#;
    assert!(!cc_session_ended(tail));
}

#[test]
fn cc_session_ended_ignores_string_content_containing_session_end() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"user","message":{"content":[{"type":"tool_result","output":"cat session_end.sh"}]}}
"#;
    assert!(
        !cc_session_ended(tail),
        "should not false-positive on session_end inside tool output"
    );
}

#[test]
fn cc_session_ended_detects_exit_command() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
{"type":"user","message":{"role":"user","content":"<command-name>/exit</command-name>\n            <command-message>exit</command-message>"}}
"#;
    assert!(cc_session_ended(tail));
}

#[test]
fn cc_session_ended_ignores_non_terminating_slash_command() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}
"#;
    assert!(
        !cc_session_ended(tail),
        "/clear keeps the session alive — not an end marker"
    );
}

// A resume after exit (new session_start tail-appended) resets the end state.
#[test]
fn cc_session_ended_exit_then_session_start_is_not_ended() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"user","message":{"role":"user","content":"<command-name>/exit</command-name>"}}
{"type":"system","subtype":"session_start","sessionId":"s1"}
"#;
    assert!(
        !cc_session_ended(tail),
        "session resumed after exit — last marker wins"
    );
}

#[test]
fn detect_parent_id_for_subagent_path() {
    use pixtuoid_core::AgentId;
    use std::path::Path;

    // Simulate what jsonl.rs::detect_parent_id does
    let path = Path::new("/Users/me/.claude/projects/x/abc123/subagents/agent-1.jsonl");
    let path_str = path.to_string_lossy();
    let idx = path_str.find("/subagents/");
    assert!(idx.is_some(), "should find /subagents/ in path");
    let parent_dir = &path_str[..idx.unwrap()];
    let parent_jsonl = format!("{parent_dir}.jsonl");
    let parent_id = AgentId::from_parts("claude-code", &parent_jsonl);
    let expected = AgentId::from_parts("claude-code", "/Users/me/.claude/projects/x/abc123.jsonl");
    assert_eq!(parent_id, expected);
}

#[test]
fn detect_parent_id_returns_none_for_regular_path() {
    let path = std::path::Path::new("/Users/me/.claude/projects/x/ses-abc.jsonl");
    let path_str = path.to_string_lossy();
    assert!(
        path_str.find("/subagents/").is_none(),
        "regular path should not match subagent pattern"
    );
}

#[test]
fn decode_hook_payload_missing_session_id_returns_err() {
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/repo"
    });
    assert!(
        decode_hook_payload(payload).is_err(),
        "missing session_id must return Err"
    );
}

#[test]
fn decode_hook_payload_missing_transcript_path_falls_back_to_session_id() {
    // Codex sends transcript_path as string|null, so a missing/null value must
    // NOT error — it falls back to session_id for the AgentId (namespaced by
    // source, so no cross-CLI collision).
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "cwd": "/repo",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" }
    });
    let ev = decode_hook_payload(payload).expect("decodes via session_id fallback");
    let agent_id = match ev {
        pixtuoid_core::source::AgentEvent::ActivityStart { agent_id, .. } => agent_id,
        other => panic!("expected ActivityStart, got {other:?}"),
    };
    assert_eq!(
        agent_id,
        pixtuoid_core::AgentId::from_parts(
            pixtuoid_core::source::claude_code::SOURCE_NAME,
            "ses-abc"
        )
    );
}

#[test]
fn decode_hook_payload_missing_tool_name_still_succeeds() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/tmp/t.jsonl"
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert!(
                d.display().contains("?"),
                "missing tool_name should fall back to '?'"
            );
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}
