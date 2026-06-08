//! Regression for the real Codex subagent hook lifecycle.
//!
//! Codex's `spawn_agent` subagents signal their lifecycle ONLY via the
//! `SubagentStart`/`SubagentStop` hooks (the subagent has its own rollout file,
//! so the JSONL watcher renders the sprite — but orphaned, since a flat
//! `~/.codex/sessions/.../rollout-*.jsonl` path has no `/subagents/` to derive a
//! parent). The payloads here were captured live (Codex 0.135, gpt-5.5) and
//! sanitized: synthetic UUIDs, generic cwd, the huge `last_assistant_message`
//! truncated. This guards two things that regressed before:
//!   1. the hooks decode at all (they used to `bail!` → silently dropped), and
//!   2. the subagent joins the scope tree so it cascades with its parent,
//!      in BOTH transport arrival orders (hook-first and JSONL-first).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

const PARENT: &str = "01000000-0000-7000-8000-000000000001";
const CHILD: &str = "01000000-0000-7000-8000-000000000002";

/// Decode the captured hook payloads in file order.
fn captured_hook_events() -> Vec<AgentEvent> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/codex/fixtures/hook-payloads.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid hook json");
            decode_hook_payload(v).expect("captured Codex hook payload must decode")
        })
        .collect()
}

#[test]
fn codex_subagent_hook_lifecycle_links_child_and_exits_on_stop() {
    let parent = AgentId::from_parts("codex", PARENT);
    let child = AgentId::from_parts("codex", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    for ev in captured_hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }

    // UserPromptSubmit created the parent; SubagentStart created the child
    // keyed on `agent_id` and linked to the parent session.
    let child_slot = scene
        .agents
        .get(&child)
        .expect("SubagentStart must create the subagent sprite");
    assert_eq!(
        child_slot.parent_id,
        Some(parent),
        "subagent must be linked to its parent session"
    );
    // SubagentStop ends the CHILD (prompt removal); the parent keeps running
    // (its `Stop` is only turn-end → idle, Codex has no SessionEnd).
    assert!(
        child_slot.exiting_at.is_some(),
        "SubagentStop must mark the subagent exiting"
    );
    let parent_slot = scene.agents.get(&parent).expect("parent still present");
    assert!(
        parent_slot.exiting_at.is_none(),
        "parent must keep running after the subagent stops"
    );
}

#[test]
fn codex_subagent_jsonl_first_orphan_is_enriched_by_subagent_start() {
    // The transports race: the subagent's own rollout (JSONL) can create the
    // sprite ORPHANED before the SubagentStart hook arrives. The hook must then
    // enrich the existing slot with the parent link, not be dropped as a dup.
    let parent = AgentId::from_parts("codex", PARENT);
    let child = AgentId::from_parts("codex", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    // Parent session + the subagent's orphan JSONL sprite arrive first.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "codex".into(),
            session_id: PARENT.into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: None,
        },
        now,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "codex".into(),
            session_id: CHILD.into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: None,
        },
        now,
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&child).unwrap().parent_id.is_none(),
        "JSONL-rendered subagent starts orphaned"
    );

    for ev in captured_hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }
    assert_eq!(
        scene.agents.get(&child).unwrap().parent_id,
        Some(parent),
        "SubagentStart hook must enrich the JSONL-first orphan with its parent link"
    );
}

#[test]
fn codex_subagent_stop_before_start_is_a_safe_noop() {
    // The hooks are best-effort and unordered: a SubagentStop can win the race
    // against the child's slot creation. SessionEnd for a not-yet-existing child
    // must be harmless — no panic, no phantom slot, no spurious parent cascade.
    let parent = AgentId::from_parts("codex", PARENT);
    let child = AgentId::from_parts("codex", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "codex".into(),
            session_id: PARENT.into(),
            cwd: PathBuf::from("/home/user/demo-project"),
            parent_id: None,
        },
        now,
        Transport::Hook,
    );
    // SubagentStop decodes to SessionEnd{child}; apply with no child present.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: child },
        now,
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "a SessionEnd for an absent child must not create a phantom slot"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "an orphan SubagentStop must not cascade the unrelated parent"
    );
}
