use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentEvent, AgentId, Reducer, SceneState, Transport};

#[test]
fn scripted_timeline_drives_scene_through_states() {
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let mut snapshots: Vec<SceneState> = Vec::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    let mut now = SystemTime::now();
    let mut step = |events: Vec<AgentEvent>,
                    dt_ms: u64,
                    r: &mut Reducer,
                    s: &mut SceneState,
                    snaps: &mut Vec<SceneState>| {
        for ev in events {
            r.apply(s, ev, now, Transport::Hook);
        }
        snaps.push(s.clone());
        now += Duration::from_millis(dt_ms);
    };

    step(
        vec![AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        }],
        10,
        &mut reducer,
        &mut scene,
        &mut snapshots,
    );

    step(
        vec![AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: Some("Bash: ls".into()),
        }],
        200,
        &mut reducer,
        &mut scene,
        &mut snapshots,
    );

    step(
        vec![AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut snapshots,
    );

    step(
        vec![AgentEvent::Waiting {
            agent_id: id,
            reason: "permission?".into(),
            tool_use_id: None,
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut snapshots,
    );

    step(
        vec![AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        }],
        10,
        &mut reducer,
        &mut scene,
        &mut snapshots,
    );

    let snaps = &snapshots;
    assert_eq!(snaps.len(), 5);
    assert_eq!(snaps[0].agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(matches!(
        snaps[1].agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));
    // ActivityEnd only arms `pending_idle_at` — the slot stays Active for
    // `ACTIVE_GRACE_WINDOW`; `tick` realizes Idle.
    let slot2 = snaps[2].agents.get(&id).unwrap();
    assert!(matches!(slot2.state, ActivityState::Active { .. }));
    assert!(slot2.pending_idle_at.is_some());
    assert!(matches!(
        snaps[3].agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));
    let exit_slot = snaps[4]
        .agents
        .get(&id)
        .expect("slot still present for exit animation");
    assert!(
        exit_slot.exiting_at.is_some(),
        "SessionEnd should mark exiting_at, not drop immediately"
    );
}
