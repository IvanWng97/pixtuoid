use pixtuoid_core::source::antigravity;
use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::AgentId;
use serde_json::json;

fn load(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/sources/decode/fixtures/hooks/{name}.json"))
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

/// Decode a hook payload expected to yield exactly ONE event (lifecycle arms —
/// SessionStart / UserPromptSubmit / Stop / SessionEnd / Subagent*).
fn decode_single(v: serde_json::Value) -> AgentEvent {
    let mut evs = decode_hook_payload(v).expect("decodes");
    assert_eq!(evs.len(), 1, "expected exactly one event, got {evs:?}");
    evs.pop().expect("one event")
}

/// Decode a tool/permission hook payload and return its ACTIVITY event. Those
/// arms prepend an `Identity` (#221) — assert the pair shape and that the two
/// events coalesce on one AgentId, then hand back the activity event so each
/// test keeps asserting what it always did.
fn decode_activity(v: serde_json::Value) -> AgentEvent {
    let mut evs = decode_hook_payload(v).expect("decodes");
    assert_eq!(evs.len(), 2, "expected Identity + activity, got {evs:?}");
    assert!(
        matches!(evs[0], AgentEvent::Identity { .. }),
        "tool/permission arms must lead with Identity, got {evs:?}"
    );
    let activity = evs.pop().expect("activity event");
    assert_eq!(
        evs[0].agent_id(),
        activity.agent_id(),
        "Identity must coalesce with its activity event"
    );
    activity
}

fn load_jsonl(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/sources/decode/fixtures/jsonl/{name}.json"))
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

#[test]
fn decode_session_start() {
    let ev = decode_single(load("session_start"));
    // CC keys on the session UUID (IdKey::SessionId), which == the transcript
    // filename stem the watcher/per-line decode derive (`cc_id_from_path`).
    let expected_id = AgentId::from_parts("claude-code", "ses-abc");
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
    let ev = decode_single(payload);
    match ev {
        AgentEvent::SessionStart { source, .. } => {
            assert_eq!(source, "antigravity");
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn decode_pre_tool_use_write_maps_to_typing() {
    let ev = decode_activity(load("pre_tool_use_write"));
    match ev {
        AgentEvent::ActivityStart { detail, .. } => {
            assert!(detail.unwrap().display().contains("Write"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_post_tool_use_is_activity_end() {
    let ev = decode_activity(load("post_tool_use_write"));
    assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
}

#[test]
fn decode_notification_is_waiting() {
    let ev = decode_activity(load("notification"));
    match ev {
        AgentEvent::Waiting { reason, .. } => assert!(reason.contains("permission")),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_session_end() {
    let ev = decode_single(load("session_end"));
    // The shared session-keyed SessionEnd arm ends the SESSION ITSELF, never
    // a child — as_child stays false so the reducer's child ledger
    // (#244/#246) is written only by the SubagentStop decoders.
    assert!(matches!(
        ev,
        AgentEvent::SessionEnd {
            as_child: false,
            ..
        }
    ));
}

#[test]
fn decode_unknown_event_returns_err() {
    let mut bad = load("session_start");
    bad["hook_event_name"] = serde_json::Value::String("UnknownThing".into());
    assert!(decode_hook_payload(bad).is_err());
}

// An empty session_id passes `as_str` but (for Codex, keyed on session_id) would
// mint a phantom agent that never coalesces — reject it as malformed.
#[test]
fn empty_session_id_is_rejected() {
    assert!(
        decode_hook_payload(json!({
            "hook_event_name": "SessionStart",
            "session_id": "",
            "transcript_path": "/p/a.jsonl",
            "cwd": "/repo"
        }))
        .is_err(),
        "empty session_id must Err, not mint AgentId(source, \"\")"
    );
}

// An empty attributionAgent must NOT emit a Rename — that would blank a good
// hook-derived label with no recovery until the next Rename.
#[test]
fn cc_empty_attribution_agent_emits_no_rename() {
    let events = decode_cc_line(
        "/p/parent.jsonl",
        "claude-code",
        json!({"type": "assistant", "attributionAgent": "", "message": {"content": []}}),
    )
    .unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::Rename { .. })),
        "empty attributionAgent must not emit a (label-blanking) Rename, got {events:?}"
    );
}

// A TRAILING-colon attributionAgent ("ns:") splits to an EMPTY segment after the
// last colon — it must NOT emit a label-blanking Rename either. The pre-split
// emptiness guard doesn't catch it; the post-split check in decode_cc_line does.
#[test]
fn cc_trailing_colon_attribution_agent_emits_no_rename() {
    let events = decode_cc_line(
        "/p/parent.jsonl",
        "claude-code",
        json!({"type": "assistant", "attributionAgent": "ns:", "message": {"content": []}}),
    )
    .unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::Rename { .. })),
        "trailing-colon attributionAgent must not emit a (label-blanking) Rename, got {events:?}"
    );
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
    let ev = decode_single(json!({
        "hook_event_name": "SubagentStart",
        "session_id": "parent-sess",
        "agent_id": "child-agent",
        "agent_type": "default",
        "turn_id": "turn-1",
        "cwd": "/home/user/demo-project",
        "_pixtuoid_source": "codex"
    }));
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
    let ev = decode_single(json!({
        "hook_event_name": "SubagentStop",
        "session_id": "parent-sess",
        "agent_id": "child-agent",
        "agent_type": "default",
        "stop_hook_active": false,
        "_pixtuoid_source": "codex"
    }));
    match ev {
        AgentEvent::SessionEnd { agent_id, as_child } => {
            assert_eq!(
                agent_id,
                AgentId::from_parts("codex", "child-agent"),
                "ends the CHILD (keyed on agent_id), never the parent session"
            );
            assert!(
                as_child,
                "a SubagentStop end must carry the as_child stamp (the reducer's \
                 child ledger keys on it, #244/#246)"
            );
        }
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

// ---- CC SubagentStart/SubagentStop (#241) ---------------------------------
//
// CC Workflow-tool fleets spawn subagents with NO per-agent `Agent` tool_use in
// the parent transcript (b1 Task-drain structurally can't fire) and their
// transcripts carry no end marker — the SubagentStart/Stop HOOKS are the only
// instant lifecycle signal. Wire facts (captured live, CC v2.1.170):
// SubagentStart carries the parent's session_id/transcript_path/cwd plus
// `agent_id` (BARE hex — NO "agent-" prefix) and `agent_type`
// ("general-purpose" | "workflow-subagent"); SubagentStop adds
// `agent_transcript_path` (the subagent's own transcript, incl. the deeper
// `subagents/workflows/wf_*/` nesting) and noise fields we don't consume.

// The bare wire agent_id must key the child as `agent-<id>` — the transcript
// filename stem (`cc_id_from_path`), i.e. the JSONL watcher's id space — and
// link it to the parent session. Both captured agent_types decode identically.
#[test]
fn cc_subagent_start_keys_prefixed_child_and_links_parent() {
    for agent_type in ["general-purpose", "workflow-subagent"] {
        let ev = decode_single(json!({
            "hook_event_name": "SubagentStart",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "transcript_path": "/home/user/.claude/projects/-home-user-demo-project/01000000-0000-7000-8000-0000000000cc.jsonl",
            "cwd": "/home/user/demo-project",
            "agent_id": "a0000000000000001",
            "agent_type": agent_type
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                parent_id,
                ..
            } => {
                assert_eq!(source, "claude-code");
                assert_eq!(
                    agent_id,
                    AgentId::from_parts("claude-code", "agent-a0000000000000001"),
                    "{agent_type}: child keyed `agent-<id>` — the bare wire id \
                     lacks the prefix the transcript stem carries"
                );
                assert_eq!(
                    parent_id,
                    Some(AgentId::from_parts(
                        "claude-code",
                        "01000000-0000-7000-8000-0000000000cc"
                    )),
                    "{agent_type}: linked to the parent session"
                );
                assert_eq!(cwd, std::path::PathBuf::from("/home/user/demo-project"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }
}

// SubagentStop keys its SessionEnd on `cc_id_from_path(agent_transcript_path)`
// — EXACT parity with the JSONL watcher's id space (the authoritative key) —
// including the deeper `subagents/workflows/wf_*/` nesting of Workflow fleets.
#[test]
fn cc_subagent_stop_keys_on_agent_transcript_path_stem() {
    for nested_path in [
        "/home/user/.claude/projects/-home-user-demo-project/01000000-0000-7000-8000-0000000000cc/subagents/agent-a0000000000000001.jsonl",
        "/home/user/.claude/projects/-home-user-demo-project/01000000-0000-7000-8000-0000000000cc/subagents/workflows/wf_00000000-000/agent-a0000000000000001.jsonl",
    ] {
        let ev = decode_single(json!({
            "hook_event_name": "SubagentStop",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "transcript_path": "/home/user/.claude/projects/-home-user-demo-project/01000000-0000-7000-8000-0000000000cc.jsonl",
            "cwd": "/home/user/demo-project",
            "agent_id": "a0000000000000001",
            "agent_type": "general-purpose",
            "agent_transcript_path": nested_path,
            "stop_hook_active": false,
            "last_assistant_message": "done"
        }));
        match ev {
            AgentEvent::SessionEnd { agent_id, as_child } => {
                assert_eq!(
                    agent_id,
                    AgentId::from_parts("claude-code", "agent-a0000000000000001"),
                    "ends the CHILD keyed on the agent transcript's filename stem \
                     (path: {nested_path})"
                );
                assert!(
                    as_child,
                    "a SubagentStop end must carry the as_child stamp (the reducer's \
                     child ledger keys on it, #244/#246)"
                );
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }
}

// A Stop without `agent_transcript_path` (absent, null, or empty) falls back to
// the prefixed wire agent_id — the same key SubagentStart minted.
#[test]
fn cc_subagent_stop_without_transcript_path_falls_back_to_prefixed_agent_id() {
    for payload in [
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "parent-sess",
            "agent_id": "a0000000000000001"
        }),
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "parent-sess",
            "agent_id": "a0000000000000001",
            "agent_transcript_path": null
        }),
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "parent-sess",
            "agent_id": "a0000000000000001",
            "agent_transcript_path": ""
        }),
    ] {
        let ev = decode_single(payload);
        match ev {
            AgentEvent::SessionEnd { agent_id, as_child } => {
                assert!(
                    as_child,
                    "fallback-path SubagentStop must stamp as_child: true \
                     (the child ledger keys on it, #244/#246)"
                );
                assert_eq!(
                    agent_id,
                    AgentId::from_parts("claude-code", "agent-a0000000000000001")
                );
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }
}

// The keying-parity pin: a Start (prefix-keyed from the bare wire id) and its
// Stop (path-keyed via cc_id_from_path) MUST resolve to one AgentId, or the
// start registers a sprite the stop can never end.
#[test]
fn cc_subagent_start_and_stop_coalesce_on_one_child_id() {
    let start = decode_single(json!({
        "hook_event_name": "SubagentStart",
        "session_id": "01000000-0000-7000-8000-0000000000cc",
        "cwd": "/home/user/demo-project",
        "agent_id": "a0000000000000001",
        "agent_type": "workflow-subagent"
    }))
    .agent_id();
    let stop = decode_single(json!({
        "hook_event_name": "SubagentStop",
        "session_id": "01000000-0000-7000-8000-0000000000cc",
        "agent_id": "a0000000000000001",
        "agent_type": "workflow-subagent",
        "agent_transcript_path": "/home/user/.claude/projects/-home-user-demo-project/01000000-0000-7000-8000-0000000000cc/subagents/workflows/wf_00000000-000/agent-a0000000000000001.jsonl"
    }))
    .agent_id();
    assert_eq!(
        start, stop,
        "Start (prefix fallback) and Stop (transcript stem) must coalesce"
    );
}

// Defensive: the CC docs' SubagentStart example shows an ALREADY-prefixed
// `"agent_id": "agent-abc123"` while the live wire sends bare hex — both
// forms must key identically (no `agent-agent-` double prefix).
#[test]
fn cc_subagent_start_does_not_double_prefix_an_already_prefixed_agent_id() {
    for wire_id in ["abc123", "agent-abc123"] {
        let ev = decode_single(json!({
            "hook_event_name": "SubagentStart",
            "session_id": "parent-sess",
            "cwd": "/home/user/demo-project",
            "agent_id": wire_id,
            "agent_type": "general-purpose"
        }));
        assert_eq!(
            ev.agent_id(),
            AgentId::from_parts("claude-code", "agent-abc123"),
            "wire form {wire_id:?} must key as agent-abc123"
        );
    }
}

// Claim-fully contract (mirrors the codex twin): a malformed CC Subagent
// payload must Err (logged + skipped by the listener), never fall through to
// the shared session-keyed arms or mint a phantom child.
#[test]
fn cc_subagent_hooks_reject_missing_or_empty_agent_id() {
    for event in ["SubagentStart", "SubagentStop"] {
        for payload in [
            json!({"hook_event_name": event, "session_id": "parent-sess"}),
            json!({"hook_event_name": event, "session_id": "parent-sess", "agent_id": ""}),
            json!({"hook_event_name": event, "agent_id": "abc"}),
        ] {
            assert!(
                decode_hook_payload(payload.clone()).is_err(),
                "CC {event} with missing/empty ids must Err, got Ok for {payload}"
            );
        }
    }
}

// Codex coalesces hook↔rollout on the session/agent UUID (NOT a path string), so
// it's separator-agnostic by construction — but the watcher still has to extract
// that UUID from a real backslash Windows rollout path. Pin that the child's hook
// AgentId (keyed on agent_id) equals the watcher's id derived from the on-disk
// Windows rollout filename. Windows-only: codex_id_from_path's file_stem split
// needs `\` to act as a separator (on Unix `\` is an ordinary filename byte).
// The Codex analogue of CC's mixed_separator_and_case_forms_coalesce_on_windows;
// runs on the windows-test job only.
#[cfg(windows)]
#[test]
fn codex_subagent_hook_coalesces_with_its_windows_rollout_path() {
    use pixtuoid_core::source::codex::codex_id_from_path;
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let rollout =
        format!(r"C:\Users\Me\.codex\sessions\2026\06\08\rollout-2026-06-08T22-36-52-{uuid}.jsonl");

    // Hook side: SubagentStart keys the CHILD on agent_id (== the rollout UUID).
    let child = decode_single(json!({
        "hook_event_name": "SubagentStart",
        "session_id": "parent-sess",
        "agent_id": uuid,
        "agent_type": "default",
        "cwd": r"C:\Users\Me\demo",
        "_pixtuoid_source": "codex"
    }))
    .agent_id();

    // Watcher side: the id derived from the on-disk Windows rollout path.
    let watcher = AgentId::from_parts("codex", &codex_id_from_path(std::path::Path::new(&rollout)));

    assert_eq!(
        child, watcher,
        "a Codex subagent hook (agent_id) and its Windows rollout file must coalesce \
         to one AgentId — a mismatch leaves the subagent orphaned from the scope tree"
    );
}

#[test]
fn cc_jsonl_assistant_tool_use_is_activity_start() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events =
        decode_cc_line(transcript, "claude-code", load_jsonl("assistant_tool_use")).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityStart {
            tool_use_id,
            detail,
            ..
        } => {
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
    let ev = decode_activity(payload);
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
        "tool_name": "Agent",
        "tool_use_id": "toolu_01ABC",
        "tool_input": { "description": "go" }
    });
    let ev = decode_activity(payload);
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
// tools — observed live). Since 0.12.0 only "Agent" is a KNOWN name; the
// legacy pre-v2.1.63 "Task" dispatch here rides purely the SEMANTIC
// `subagent_type` detection (this test pins that an old CC's real dispatch —
// which always carries the field — is still caught after the name arm's
// removal).
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
        match decode_activity(payload) {
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
    match decode_activity(payload) {
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
    match decode_activity(payload) {
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
    let ev = decode_activity(payload);
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

// Session lifecycle never reads chat content. The old content-based /exit
// matcher had ZERO true positives across a 135-transcript corpus (modern CC
// persists no /exit user line — slash commands are `type:"system",
// subtype:"local_command"` lines) and false-positived on any user message
// QUOTING the wrapper. Every slash-command-shaped user line — terminating or
// not — must decode to nothing; live /exit reaping is the SessionEnd HOOK's
// job, with the idle sweep as the dropped-hook fallback.
#[test]
fn cc_jsonl_slash_command_user_lines_yield_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    for cmd in ["/exit", "/quit", "/clear", "/compact"] {
        let v = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": format!("<command-name>{cmd}</command-name>") }
        });
        let events = decode_cc_line(transcript, "claude-code", v).unwrap();
        assert!(
            events.is_empty(),
            "{cmd} content must not drive lifecycle: {events:?}"
        );
    }
}

// Regression for the false-positive class: a user message quoting the wrapper
// text mid-prose (common when a session discusses CC internals) must not end
// the session.
#[test]
fn cc_jsonl_quoted_exit_wrapper_mid_prose_yields_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let v = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": "I saw <command-name>/exit</command-name> in a transcript — what writes that?"
        }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    assert!(
        events.is_empty(),
        "quoting the wrapper must not emit SessionEnd: {events:?}"
    );
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

// The tail scan is STRUCTURAL-only: user-message content (including a
// slash-command wrapper, exact or quoted mid-prose) is user-controllable and
// must never read as a session end.
#[test]
fn cc_session_ended_ignores_slash_command_content() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
{"type":"user","message":{"role":"user","content":"<command-name>/exit</command-name>\n            <command-message>exit</command-message>"}}
"#;
    assert!(
        !cc_session_ended(tail),
        "an /exit-wrapper user line is content, not a structural end marker"
    );
    let quoted = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"user","message":{"role":"user","content":"why does <command-name>/quit</command-name> show up wrapped?"}}
"#;
    assert!(
        !cc_session_ended(quoted),
        "quoting the wrapper mid-prose must not end the session"
    );
}

// A resume after a structural end (new session_start tail-appended) resets the
// end state — last marker wins.
#[test]
fn cc_session_ended_end_then_session_start_is_not_ended() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_end","sessionId":"s1"}
{"type":"system","subtype":"session_start","sessionId":"s1"}
"#;
    assert!(
        !cc_session_ended(tail),
        "session resumed after a structural end — last marker wins"
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
fn decode_cc_hook_keys_on_session_id_ignoring_transcript_path() {
    // CC keys on the session UUID (IdKey::SessionId) regardless of any
    // transcript_path the hook carries — keying on the cwd-derived path would
    // rebuild the wrong parent after a git-worktree split. Pin that a present
    // transcript_path whose stem DIFFERS from session_id is ignored: the
    // AgentId must still be the session-id key (which == the watcher's
    // cc_id_from_path of `<session_id>.jsonl`, so the two transports coalesce).
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        // A transcript_path whose stem ("OTHER-stem") is NOT the session_id.
        "transcript_path": "/Users/me/.claude/projects/-Worktree-B/OTHER-stem.jsonl",
        "cwd": "/repo",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" }
    });
    let ev = decode_activity(payload);
    let agent_id = match ev {
        pixtuoid_core::source::AgentEvent::ActivityStart { agent_id, .. } => agent_id,
        other => panic!("expected ActivityStart, got {other:?}"),
    };
    assert_eq!(
        agent_id,
        pixtuoid_core::AgentId::from_parts(
            pixtuoid_core::source::claude_code::SOURCE_NAME,
            "ses-abc"
        ),
        "CC must key on session_id, ignoring transcript_path"
    );
}

// `describe_tool_target` truncates a tool target longer than 40 chars and
// appends an ellipsis. The existing multibyte test uses a 39-char command, so
// the `> 40` branch was never exercised.
#[test]
fn decode_pre_tool_use_long_command_is_ellipsis_truncated() {
    let long_cmd = "echo ".to_string() + &"a".repeat(60); // > 40 chars
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-trunc",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/repo",
        "tool_name": "Bash",
        "tool_input": { "command": long_cmd }
    });
    match decode_activity(payload) {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert!(
                d.display().ends_with('…'),
                "a >40-char Bash command must be ellipsis-truncated, got {}",
                d.display()
            );
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

// `describe_tool_target` early-returns an empty string when the keyed input
// field is absent. A Bash tool with an empty `tool_input` (no `command`) yields
// a display of just the tool name — no `": <target>"` suffix.
#[test]
fn decode_pre_tool_use_missing_target_field_has_no_suffix() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-nocmd",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/repo",
        "tool_name": "Bash",
        "tool_input": {}
    });
    match decode_activity(payload) {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert_eq!(
                d.display(),
                "Bash",
                "absent target field must produce no `: <target>` suffix"
            );
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

// `decode_ag_line` early edge branches: a non-object line and an object with no
// `step_index` both decode to zero events.
#[test]
fn ag_non_object_and_missing_step_index_emit_nothing() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    // Non-object value (bare string).
    assert!(
        antigravity::decode_ag_line(transcript, "antigravity", json!("x"))
            .unwrap()
            .is_empty()
    );
    // Object without `step_index`.
    assert!(
        antigravity::decode_ag_line(transcript, "antigravity", json!({ "foo": 1 }))
            .unwrap()
            .is_empty()
    );
}

// A non-integer `step_index` must fail safe-and-visible: skip the line rather
// than coerce to 0 (which would corrupt the ag-{step}-{i} tool_use_id pairing).
#[test]
fn ag_non_integer_step_index_is_skipped() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = json!({
        "step_index": "not-a-number",
        "type": "PLANNER_RESPONSE",
        "tool_calls": [ { "name": "run_command", "args": { "CommandLine": "ls" } } ]
    });
    assert!(
        antigravity::decode_ag_line(transcript, "antigravity", v)
            .unwrap()
            .is_empty(),
        "a present-but-non-integer step_index must be skipped, not coerced to 0"
    );
}

// A `tool_calls` entry that isn't an object is skipped (`continue`). The
// display text (tool name + `: target` via ag_tool_target →
// generic_tool_display) is pinned by antigravity.rs's own unit tests; here
// assert the event shape + the load-bearing tool_use_id.
#[test]
fn ag_skips_non_object_tool_call_and_keys_run_command() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = json!({
        "step_index": 3,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            42,
            { "name": "run_command", "args": { "CommandLine": "\"git status\"" } }
        ]
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    // The integer entry (index 0) is skipped; only the run_command start emits,
    // and it carries the index-1 id (not index-0 — the skip does not renumber).
    assert_eq!(
        events.len(),
        1,
        "non-object tool_call must be skipped: {events:?}"
    );
    match &events[0] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-3-1"));
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

// A PLANNER_RESPONSE with no `tool_calls` key (the `if let Some(Value::Array)`
// fails to match) decodes to zero events — distinct from an empty array.
#[test]
fn ag_planner_response_without_tool_calls_emits_nothing() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = json!({ "step_index": 2, "type": "PLANNER_RESPONSE" });
    assert!(antigravity::decode_ag_line(transcript, "antigravity", v)
        .unwrap()
        .is_empty());
}

#[test]
fn ag_grep_search_decodes_to_activity_start() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = json!({
        "step_index": 4,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "grep_search", "args": { "SearchPath": "/repo", "query": "TODO" } }
        ]
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-4-0"));
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

#[test]
fn decode_hook_payload_missing_tool_name_still_succeeds() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/tmp/t.jsonl"
    });
    let ev = decode_activity(payload);
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

// The cross-platform LOCKSTEP guard: the hook side (IdKey::SessionId →
// `session_id`) and the watcher side (`cc_id_from_path` of a transcript named
// `<session_id>.jsonl`) must hash to ONE AgentId or every CC session renders as
// two sprites (hook-wins dedup and permission-Waiting silently die). Pinned via
// the REAL seams on both sides (no inline re-simulation): the watcher uses the
// SAME `.with_id_deriver(cc_id_from_path)` that ClaudeCodeSource::run wires.
#[tokio::test]
async fn hook_and_watcher_keys_coalesce_for_one_file() {
    use pixtuoid_core::source::claude_code::{
        cc_derive_label, cc_id_from_path, cc_session_ended, decode_cc_line,
    };
    use pixtuoid_core::source::jsonl::{force_polling_backend_for_tests, JsonlWatcher};
    use pixtuoid_core::source::Transport;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::mpsc;

    force_polling_backend_for_tests(Duration::from_millis(25));

    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-coalesce");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    // Filename stem == session_id, so the UUID-keyed hook and the stem-keyed
    // watcher coalesce.
    let transcript = project_dir.join("ses-coalesce.jsonl");

    // Hook side: decode a SessionStart payload for the same session. The
    // transcript_path is present but IGNORED (CC keys on session_id).
    let transcript_str = transcript.to_string_lossy().to_string();
    let hook_payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "ses-coalesce",
        "transcript_path": transcript_str,
        "cwd": "/repo"
    });
    let hook_id = decode_single(hook_payload).agent_id();

    // Watcher side: run a real JsonlWatcher over projects_root, write a
    // session_start line, and capture the SessionStart AgentId.
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        projects_root.clone(),
        "claude-code".to_string(),
        decode_cc_line,
        cc_session_ended,
    )
    .with_id_deriver(cc_id_from_path)
    .with_label_deriver(cc_derive_label);
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-coalesce",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut watcher_id: Option<AgentId> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, ev @ AgentEvent::SessionStart { .. }))) => {
                watcher_id = Some(ev.agent_id());
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    handle.abort();

    let watcher_id = watcher_id.expect("watcher must emit SessionStart");
    assert_eq!(
        hook_id, watcher_id,
        "hook AgentId ({hook_id}) must equal watcher AgentId ({watcher_id}) for the \
         same file — mismatching IDs split one session into two sprites"
    );
}

// The walker's first-sight head scan dispatches to the SCANNED source's own
// cwd extractor (a registry-row fn — invariant #3). Pin each transcript-
// bearing source's extractor against its REAL fixture head bytes, so a wire-
// shape drift (or a row pointing at the wrong extractor) fails here with the
// source's name. Antigravity's lines carry no cwd at all — its row uses the
// shared top-level shape, which must find nothing.
#[test]
fn registry_cwd_extractor_matches_each_sources_real_head_shape() {
    use std::path::{Path, PathBuf};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources/fixtures");
    let cases: [(&str, &str, Option<&str>); 4] = [
        (
            "claude-code",
            "claude-code/tool-call/01000000-0000-7000-8000-0000000000cc.jsonl",
            Some("/home/user/demo-project"),
        ),
        (
            "codex",
            "codex/tool-run/rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000002.jsonl",
            Some("/home/user/demo-project"),
        ),
        (
            "copilot",
            "copilot/tool-run/events.jsonl",
            Some(r"d:\contentforge-fullstack (1)"),
        ),
        ("antigravity", "antigravity/tool-run/transcript.jsonl", None),
    ];
    for (source, rel, expected) in cases {
        let extract = pixtuoid_core::source::registry::cwd_extractor_for(source);
        let content = std::fs::read_to_string(fixtures.join(rel))
            .unwrap_or_else(|e| panic!("read fixture {rel}: {e}"));
        let got = content.lines().find_map(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| extract(&v))
        });
        assert_eq!(
            got,
            expected.map(PathBuf::from),
            "cwd extracted from {source}'s real fixture head"
        );
    }
}

// The mixed-separator/case path-fold (Windows) — retargeted to Antigravity,
// the remaining path-keyed source (IdKey::TranscriptPathThenSessionId). CC no
// longer path-folds (it keys on session_id), so this guards the surviving
// transcript-path key class via `normalize_path_key`.
#[cfg(windows)]
#[test]
fn mixed_separator_and_case_forms_coalesce_on_windows() {
    let a = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "s1",
        "transcript_path": r"C:\Users\Me\.gemini\antigravity-cli\brain\X\s1.jsonl",
        "_pixtuoid_source": "antigravity"
    });
    let b = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "s1",
        "transcript_path": "C:/users/me/.gemini/antigravity-cli/brain/x/s1.jsonl",
        "_pixtuoid_source": "antigravity"
    });
    assert_eq!(
        decode_single(a).agent_id(),
        decode_single(b).agent_id(),
        "backslash and forward-slash forms of the same Windows path must produce \
         the same AgentId after normalize_path_key folds both to lowercase forward-slashes"
    );
}
