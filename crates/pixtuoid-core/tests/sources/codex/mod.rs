//! Regression for the real Codex subagent hook lifecycle.
//!
//! Codex's `spawn_agent` subagents signal their lifecycle ONLY via the
//! `SubagentStart`/`SubagentStop` hooks: the subagent has its own rollout file,
//! but a flat `~/.codex/sessions/.../rollout-*.jsonl` path has no `/subagents/`
//! segment to derive a parent from. The payloads here were captured live (Codex
//! 0.135, gpt-5.5) and sanitized: synthetic UUIDs, generic cwd, the huge
//! `last_assistant_message` truncated.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

const PARENT: &str = "01000000-0000-7000-8000-000000000001";
const CHILD: &str = "01000000-0000-7000-8000-000000000002";

fn captured_hook_events() -> Vec<AgentEvent> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/codex/fixtures/hook-payloads.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| {
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

    let child_slot = scene
        .agents
        .get(&child)
        .expect("SubagentStart must create the subagent sprite");
    assert_eq!(
        child_slot.parent_id,
        Some(parent),
        "subagent must be linked to its parent session"
    );
    // SubagentStop ends the CHILD; the parent's `Stop` is only turn-end → idle
    // (its teardown SessionEnd hook is not in this capture).
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

// A SubagentStop decoded before its SubagentStart tombstones the unknown child
// id: Codex has no further end signal of any kind, so a phantom slot registered
// by the late Start would ride the stale sweeps forever (#242).
#[test]
fn codex_reordered_subagent_stop_before_start_does_not_mint_a_phantom() {
    let parent = AgentId::from_parts("codex", PARENT);
    let child = AgentId::from_parts("codex", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    let (stops, rest): (Vec<_>, Vec<_>) = captured_hook_events()
        .into_iter()
        .partition(|ev| matches!(ev, AgentEvent::SessionEnd { .. }));
    for ev in stops {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }
    let later = now + std::time::Duration::from_millis(50);
    for ev in rest {
        r.apply(&mut scene, ev, later, Transport::Hook);
    }

    assert!(
        !scene.agents.contains_key(&child),
        "a SubagentStart reordered after its own Stop must not register"
    );
    let parent_slot = scene.agents.get(&parent).expect("parent registered");
    assert!(
        parent_slot.exiting_at.is_none(),
        "the child's tombstone must not affect the parent"
    );
}

#[test]
fn codex_subagent_stop_before_start_is_a_safe_noop() {
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
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: true,
        },
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
