//! grok's DELEGATING run, captured live from a `spawn_subagent` the model was
//! asked to run BLOCKING. It lives here rather than under
//! `sources/fixtures/grok/` because a child is its own grok session with its own
//! transcript — two sprites, which the conformance harness's one-AgentId rule
//! cannot hold (the same reason `cursor/mod.rs` exists).
//!
//! Both halves of the one run are here: the parent's transcript, whose
//! `subagent_spawned` line registers the child, and the hook wire, where the
//! parent's `subagent_start` names the child and the CHILD's own `subagent_stop`
//! ends it.

use std::path::Path;

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::grok::decode_grok_line;
use pixtuoid_core::source::{AgentEvent, ToolDetail, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;

const PARENT: &str = "01a006a5-e63d-7543-b1af-da3127a85c3b";
const CHILD: &str = "01a006a6-0659-7690-a9cc-20da7c827b72";

fn lines(name: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/grok/fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn hook_events() -> Vec<AgentEvent> {
    lines("hook-payloads.jsonl")
        .iter()
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid hook json");
            decode_hook_payload(v).expect("captured grok hook payload must decode")
        })
        .collect()
}

fn seed(agent_id: AgentId) -> AgentEvent {
    AgentEvent::SessionStart {
        agent_id,
        source: "grok".into(),
        session_id: PARENT.into(),
        cwd: std::path::PathBuf::from("/private/tmp/pixtuoid-capture/proj"),
        parent_id: None,
    }
}

fn transcript_events() -> Vec<AgentEvent> {
    // The path is what grok keys the session on (its PARENT dir), so it has to
    // reproduce the real layout rather than name the fixture file.
    let logical = format!("{PARENT}/updates.jsonl");
    lines("parent-updates.jsonl")
        .iter()
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid transcript json");
            decode_grok_line(&logical, "grok", v).expect("captured grok line must decode")
        })
        .collect()
}

#[test]
fn a_blocking_spawn_is_a_task_and_the_parent_goes_delegating() {
    let parent = AgentId::from_parts("grok", PARENT);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = std::time::SystemTime::now();

    // The parent is registered by the watcher's first-sight seed, not by its own
    // transcript: a JSONL event for an unknown id is a documented no-op.
    r.apply(&mut scene, seed(parent), now, Transport::Jsonl);
    for ev in transcript_events() {
        r.apply(&mut scene, ev, now, Transport::Jsonl);
    }

    let slot = scene.agents.get(&parent).expect("parent registers");
    // Delegating is not its own variant — it is the Task detail on Active.
    match &slot.state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Delegating"))
        }
        other => panic!("a blocking spawn must park the parent in Delegating, got {other:?}"),
    }
}

#[test]
fn the_transcripts_spawn_line_registers_the_child_under_its_parent() {
    let parent = AgentId::from_parts("grok", PARENT);
    let child = AgentId::from_parts("grok", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = std::time::SystemTime::now();

    for ev in transcript_events() {
        r.apply(&mut scene, ev, now, Transport::Jsonl);
    }

    let slot = scene
        .agents
        .get(&child)
        .expect("subagent_spawned registers");
    assert_eq!(slot.parent_id, Some(parent), "child links to its parent");
}

#[test]
fn only_the_blocking_spawn_carries_the_task_detail() {
    let tasks = transcript_events()
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::ActivityStart {
                    detail: Some(ToolDetail::Task),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        tasks, 1,
        "exactly the one `background: false` spawn mints Task"
    );
}

#[test]
fn the_child_ends_on_its_own_subagent_stop_not_the_parents() {
    let child = AgentId::from_parts("grok", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = std::time::SystemTime::now();

    for ev in hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }

    let slot = scene.agents.get(&child).expect("subagent_start registers");
    assert!(
        slot.exiting_at.is_some(),
        "subagent_stop carries the CHILD's own sessionId and must end it"
    );
}
