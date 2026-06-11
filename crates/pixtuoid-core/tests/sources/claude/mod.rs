//! Symmetric regression for the Claude Code subagent lifecycle (parallels the
//! sibling `codex` module). The two CLIs map the SAME scope tree from
//! DIFFERENT signals — this pins the CC side so a future change can't quietly
//! break one while fixing the other:
//!
//!   - **Codex**: the subagent's parent link arrives via the `SubagentStart`
//!     HOOK (`agent_id` + `session_id`); its rollout is flat.
//!   - **CC**: the subagent gets its own transcript under
//!     `<parent-uuid>/subagents/agent-*.jsonl`, so the JSONL watcher derives
//!     the parent link from the `<parent-uuid>` dir component
//!     (`detect_parent_id`), which equals the parent's own session-UUID id —
//!     cwd-independent, so the link survives a git-worktree cwd-split. The
//!     subagent's per-tool hook events are misattributed to the parent and
//!     suppressed via `active_tasks` — but since #241 CC's OWN
//!     `SubagentStart`/`SubagentStop` hooks decode too: instant child
//!     registration, and the ONLY end signal a Workflow-fleet subagent gets
//!     (no per-agent `Agent` tool_use in the parent transcript means b1
//!     Task-drain structurally can't fire, and the transcript has no end
//!     marker, so finished fleet agents used to hold desks until the
//!     10/30-min stale sweeps batch-reaped them).
//!
//! Event shapes mirror the live captures: a `Task` dispatch + a
//! `general-purpose` subagent + a SessionEnd hook on clean exit → cascade
//! (below), and the REAL SubagentStart/Stop wire payloads (CC v2.1.170) in
//! `fixtures/hook-payloads.jsonl` — sanitized (synthetic ids, generic cwd,
//! `last_assistant_message`/`background_tasks` truncated but KEPT so the
//! decoder's tolerance of fields we don't consume stays pinned).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;
use serde_json::json;

// The parent transcript's filename stem ("parent") IS the session UUID the hook
// carries — CC keys on the UUID, not the path (IdKey::SessionId), so hook and
// JSONL coalesce on it.
const PARENT_PATH: &str = "/proj/parent.jsonl";
// The subagent transcript lives under `<parent-uuid>/subagents/`; the watcher's
// detect_parent_id keys on the `<parent-uuid>` component ("parent") — i.e.
// exactly the parent's own AgentId, which is what wires the two together. The
// subagent's own id is its filename stem ("agent-1").
const SUB_PATH: &str = "/proj/parent/subagents/agent-1.jsonl";

fn parent_id() -> AgentId {
    AgentId::from_parts("claude-code", "parent")
}
fn sub_id() -> AgentId {
    AgentId::from_parts("claude-code", "agent-1")
}

#[test]
fn cc_subagent_links_renames_and_cascades_on_parent_exit() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    // Parent SessionStart (hook, keyed on transcript_path).
    for ev in decode_hook_payload(json!({
        "hook_event_name": "SessionStart",
        "session_id": "parent",
        "transcript_path": PARENT_PATH,
        "cwd": "/home/user/demo-project"
    }))
    .unwrap()
    {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }
    assert!(scene.agents.contains_key(&parent_id()), "parent created");

    // Parent dispatches a subagent. Real CC names this tool "Agent" (not
    // "Task") — Task-detection must still fire so the reducer records an
    // active_task and suppresses the subagent's misattributed hook events.
    // (Decodes to [Identity, ActivityStart] since #221 — apply both, like the
    // hook listener would.)
    for ev in decode_hook_payload(json!({
        "hook_event_name": "PreToolUse",
        "session_id": "parent",
        "transcript_path": PARENT_PATH,
        "tool_name": "Agent",
        "tool_input": {"description": "explore", "subagent_type": "general-purpose"},
        "tool_use_id": "task-1"
    }))
    .unwrap()
    {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }

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

    // Clean exit (SessionEnd hook) → parent SessionEnd → cascade → subagent
    // leaves WITH it.
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

// ---- CC SubagentStart/Stop hooks (#241) — the captured wire payloads ------

// The fixture's parent session UUID and the two children's id-space keys.
// The wire's `agent_id` is BARE hex; the transcript filename stem — the JSONL
// watcher's id space (`cc_id_from_path`) — carries the `agent-` prefix.
const HOOK_PARENT: &str = "01000000-0000-7000-8000-0000000000cc";
const HOOK_CHILD_GP: &str = "agent-a0000000000000001";
const HOOK_CHILD_WF: &str = "agent-a0000000000000002";

fn hook_parent_id() -> AgentId {
    AgentId::from_parts("claude-code", HOOK_PARENT)
}

/// Decode the captured hook payloads in file order (mirrors the codex module's
/// loader). The fixture records carry NO `_pixtuoid_source` — exactly like
/// production, where CC's hook entry is the bare shim with no env, so the
/// decoder's claude-code default applies.
fn captured_hook_events() -> Vec<AgentEvent> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/claude/fixtures/hook-payloads.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid hook json");
            decode_hook_payload(v).expect("captured CC subagent hook payload must decode")
        })
        .collect()
}

fn start_parent(r: &mut Reducer, scene: &mut SceneState, now: SystemTime) {
    for ev in decode_hook_payload(json!({
        "hook_event_name": "SessionStart",
        "session_id": HOOK_PARENT,
        "transcript_path": format!(
            "/home/user/.claude/projects/-home-user-demo-project/{HOOK_PARENT}.jsonl"
        ),
        "cwd": "/home/user/demo-project"
    }))
    .unwrap()
    {
        r.apply(scene, ev, now, Transport::Hook);
    }
}

// The Workflow-fleet scenario (THE #241 bug): both captured pairs — the
// Agent-tool subagent AND the workflow-subagent — register instantly with a
// parent link on SubagentStart and exit cleanly on SubagentStop, while the
// parent keeps running. No `Agent` tool_use, no JSONL — hooks alone carry the
// whole lifecycle.
#[test]
fn cc_subagent_hook_pairs_register_link_and_exit_both_agent_types() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();
    start_parent(&mut r, &mut scene, now);

    for ev in captured_hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }

    for child_key in [HOOK_CHILD_GP, HOOK_CHILD_WF] {
        let child = AgentId::from_parts("claude-code", child_key);
        let slot = scene
            .agents
            .get(&child)
            .unwrap_or_else(|| panic!("SubagentStart must register {child_key}"));
        assert_eq!(
            slot.parent_id,
            Some(hook_parent_id()),
            "{child_key} must link to the parent session"
        );
        assert!(
            slot.exiting_at.is_some(),
            "SubagentStop must mark {child_key} exiting (the Workflow-fleet \
             desk-starvation fix — no b1, no stale-sweep wait)"
        );
    }
    assert!(
        scene
            .agents
            .get(&hook_parent_id())
            .expect("parent still present")
            .exiting_at
            .is_none(),
        "parent must keep running after its subagents stop"
    );
}

// THE bug's fix, observable cross-transport: a subagent already registered by
// the JSONL watcher (its own transcript under `<parent>/subagents/`) receives
// the hook SubagentStop → the SAME slot exits cleanly. This is the keying
// parity — the hook's SessionEnd derives its key via
// `cc_id_from_path(agent_transcript_path)`, the watcher's exact id space.
#[test]
fn cc_jsonl_registered_subagent_exits_cleanly_on_hook_subagent_stop() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();
    start_parent(&mut r, &mut scene, now);

    // The watcher's first-sight emission for the subagent transcript (id =
    // filename stem, parent from the `<parent-uuid>` path component).
    let child = AgentId::from_parts("claude-code", HOOK_CHILD_GP);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: HOOK_CHILD_GP.into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: Some(hook_parent_id()),
        },
        now,
        Transport::Jsonl,
    );
    assert!(scene.agents.contains_key(&child), "JSONL registered it");

    for ev in captured_hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }
    assert!(
        scene
            .agents
            .get(&child)
            .expect("the hook events must coalesce onto the JSONL slot, not mint a twin")
            .exiting_at
            .is_some(),
        "hook SubagentStop must exit the JSONL-registered subagent slot"
    );
    // The prefix mismatch is the trap: a hook keying on the BARE wire
    // `agent_id` would have minted a second, never-exiting sprite.
    assert!(
        !scene
            .agents
            .contains_key(&AgentId::from_parts("claude-code", "a0000000000000001")),
        "no bare-keyed phantom twin"
    );
}

// #242: hook deliveries ride per-connection tasks and can reorder — for a
// short-lived subagent the SubagentStop can be DECODED before its
// SubagentStart. The Stop's SessionEnd lands on an unknown child id and
// tombstones it; the late Start must then degrade to a no-op instead of
// registering a slot whose end already passed (no future SessionEnd is
// coming — it would hold a desk until the 10/30-min stale sweeps).
#[test]
fn cc_reordered_subagent_stop_before_start_does_not_mint_a_phantom() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();
    start_parent(&mut r, &mut scene, now);

    let (stops, starts): (Vec<_>, Vec<_>) = captured_hook_events()
        .into_iter()
        .partition(|ev| matches!(ev, AgentEvent::SessionEnd { .. }));
    for ev in stops {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }
    // The late Starts land within the tombstone TTL — TWICE, because the gate
    // must not consume the tombstone (a duplicate late Start must no-op too).
    for ev in starts.iter().chain(starts.iter()).cloned() {
        r.apply(
            &mut scene,
            ev,
            now + Duration::from_millis(50),
            Transport::Hook,
        );
    }

    for child_key in [HOOK_CHILD_GP, HOOK_CHILD_WF] {
        assert!(
            !scene
                .agents
                .contains_key(&AgentId::from_parts("claude-code", child_key)),
            "{child_key}: a SubagentStart reordered after its own Stop must not register"
        );
    }
    let parent = scene
        .agents
        .get(&hook_parent_id())
        .expect("parent untouched");
    assert!(
        parent.exiting_at.is_none(),
        "the children's tombstones must not affect the parent"
    );
}

// Hook-first then JSONL: the SubagentStart registration must coalesce with the
// watcher's later SessionStart for the same transcript (duplicate SessionStart
// = enrichment no-op), not split into two sprites.
#[test]
fn cc_hook_first_subagent_coalesces_with_later_jsonl_session_start() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();
    start_parent(&mut r, &mut scene, now);

    // Apply only the two SubagentStart records (drop the Stops).
    for ev in captured_hook_events() {
        if matches!(ev, AgentEvent::SessionStart { .. }) {
            r.apply(&mut scene, ev, now, Transport::Hook);
        }
    }
    let child = AgentId::from_parts("claude-code", HOOK_CHILD_GP);
    assert!(scene.agents.contains_key(&child), "hook registered it");
    let count_before = scene.agents.len();

    // The watcher's later first-sight emission for the same transcript.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: HOOK_CHILD_GP.into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: Some(hook_parent_id()),
        },
        now,
        Transport::Jsonl,
    );
    assert_eq!(
        scene.agents.len(),
        count_before,
        "the JSONL SessionStart must coalesce (no twin sprite)"
    );
    assert_eq!(
        scene.agents.get(&child).unwrap().parent_id,
        Some(hook_parent_id()),
        "parent link survives the duplicate"
    );
}
