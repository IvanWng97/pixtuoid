//! Full-scene serialization regression net (#279). Drives a deterministic,
//! fixed-timestamp script through the reducer and snapshots the ENTIRE
//! serialized `SceneState`. Where the sibling reducer tests assert individual
//! fields, this locks the whole tree's shape in one golden: a refactor that
//! adds / renames / reshapes any `SceneState` / `AgentSlot` / `ActivityState`
//! field — or changes which timestamp the reducer stamps — surfaces as a
//! reviewable snapshot diff. Serialization itself is what #279 added; this
//! regression net is its standing, daemon-independent payoff.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pixtuoid_core::source::{AgentEvent, ToolDetail, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

/// Fixed wall-clock so the snapshot's `SystemTime` fields are deterministic.
fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn full_scene_serialization_is_stable() {
    let mut r = Reducer::new();
    let mut scene = SceneState::uniform(8);

    let parent = AgentId::from_transcript_path("/proj/parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/proj/parent/subagents/agent-1.jsonl");
    let solo = AgentId::from_transcript_path("/other/solo.jsonl");

    // A delegating CC parent (Hook) with its subagent (Jsonl), plus an
    // independent Codex session parked on a permission prompt — three sources
    // and the three non-Idle activity states in one tree.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/proj"),
            parent_id: None,
        },
        at(0),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
            detail: Some(ToolDetail::Task),
        },
        at(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/proj"),
            parent_id: Some(parent),
        },
        at(1),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: child,
            tool_use_id: Some("tool-9".into()),
            detail: Some(ToolDetail::Generic {
                display: "Read · src/main.rs".into(),
            }),
        },
        at(2),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: solo,
            source: "codex".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/other"),
            parent_id: None,
        },
        at(3),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: solo,
            reason: "permission: Bash".into(),
        },
        at(4),
        Transport::Hook,
    );

    insta::assert_yaml_snapshot!(scene);
}
