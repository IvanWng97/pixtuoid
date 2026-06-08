//! Symmetric regression for the Claude Code subagent lifecycle (parallels the
//! sibling `codex` module). The two CLIs map the SAME scope tree from
//! DIFFERENT signals — this pins the CC side so a future change can't quietly
//! break one while fixing the other:
//!
//!   - **Codex**: the subagent's parent link arrives via the `SubagentStart`
//!     HOOK (`agent_id` + `session_id`); its rollout is flat.
//!   - **CC**: the subagent gets its own transcript under
//!     `<parent>/subagents/agent-*.jsonl`, so the JSONL watcher derives the
//!     parent link from the PATH (`detect_parent_id`: `<dir>/subagents/…` →
//!     parent key `<dir>.jsonl`). No hook carries it — CC subagent hook events
//!     are misattributed to the parent and suppressed via `active_tasks`.
//!
//! Event shapes mirror the live capture (a `Task` dispatch + a
//! `general-purpose` subagent + a clean `/exit` → cascade).

use std::path::PathBuf;
use std::time::SystemTime;

use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;
use serde_json::json;

const PARENT_PATH: &str = "/proj/parent.jsonl";
// The subagent transcript lives under `<parent>/subagents/`; the watcher's
// detect_parent_id turns this path into parent key `/proj/parent.jsonl` — i.e.
// exactly the parent's own AgentId, which is what wires the two together.
const SUB_PATH: &str = "/proj/parent/subagents/agent-1.jsonl";

fn parent_id() -> AgentId {
    AgentId::from_parts("claude-code", PARENT_PATH)
}
fn sub_id() -> AgentId {
    AgentId::from_parts("claude-code", SUB_PATH)
}

#[test]
fn cc_subagent_links_renames_and_cascades_on_parent_exit() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    // Parent SessionStart (hook, keyed on transcript_path).
    r.apply(
        &mut scene,
        decode_hook_payload(json!({
            "hook_event_name": "SessionStart",
            "session_id": "parent",
            "transcript_path": PARENT_PATH,
            "cwd": "/home/user/demo-project"
        }))
        .unwrap(),
        now,
        Transport::Hook,
    );
    assert!(scene.agents.contains_key(&parent_id()), "parent created");

    // Parent dispatches a subagent. Real CC names this tool "Agent" (not
    // "Task") — Task-detection must still fire so the reducer records an
    // active_task and suppresses the subagent's misattributed hook events.
    r.apply(
        &mut scene,
        decode_hook_payload(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "parent",
            "transcript_path": PARENT_PATH,
            "tool_name": "Agent",
            "tool_input": {"description": "explore", "subagent_type": "general-purpose"},
            "tool_use_id": "task-1"
        }))
        .unwrap(),
        now,
        Transport::Hook,
    );

    // The subagent's own transcript appears: the watcher emits SessionStart with
    // parent_id derived from the `/subagents/` path. Mirror that emission (the
    // key formula is detect_parent_id's, verbatim).
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: sub_id(),
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: Some(parent_id()),
        },
        now,
        Transport::Jsonl,
    );

    // Subagent content decodes via decode_cc_line: attributionAgent → Rename.
    for ev in decode_cc_line(
        SUB_PATH,
        "claude-code",
        json!({
            "type": "assistant",
            "attributionAgent": "general-purpose",
            "message": {"content": [
                {"type": "tool_use", "id": "s1", "name": "Read", "input": {"file_path": "/x"}}
            ]}
        }),
    )
    .unwrap()
    {
        r.apply(&mut scene, ev, now, Transport::Jsonl);
    }

    let sub = scene.agents.get(&sub_id()).expect("subagent present");
    assert_eq!(
        sub.parent_id,
        Some(parent_id()),
        "subagent linked to its parent via the /subagents/ path"
    );
    assert_eq!(
        &*sub.label, "general-purpose",
        "attributionAgent renames the subagent sprite"
    );

    // Clean `/exit` → parent SessionEnd → cascade → subagent leaves WITH it.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: parent_id(),
        },
        now,
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent_id()).unwrap().exiting_at.is_some(),
        "parent exiting"
    );
    assert!(
        scene.agents.get(&sub_id()).unwrap().exiting_at.is_some(),
        "CC subagent cascades out with its parent"
    );
}
