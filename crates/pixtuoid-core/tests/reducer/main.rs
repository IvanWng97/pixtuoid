mod activity;
mod child_ledger;
mod display;
mod lifecycle;
mod liveness;
mod tasks;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

fn start(reducer: &mut Reducer, scene: &mut SceneState, id: AgentId) {
    reducer.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
}

/// Delegation scaffold shared by the pre-pass ordering pins: parent created
/// via Hook at `t0`, child created via Jsonl at `t0 + 100ms` with the parent
/// link (the same two-transport shape the sibling lifecycle tests hand-roll).
fn delegating_pair(
    r: &mut Reducer,
    scene: &mut SceneState,
    slug: &str,
    t0: SystemTime,
) -> (AgentId, AgentId) {
    let parent = AgentId::from_transcript_path(&format!("/p/{slug}.jsonl"));
    let child = AgentId::from_parts("claude-code", &format!("/p/{slug}/subagents/agent-1.jsonl"));
    r.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    (parent, child)
}
