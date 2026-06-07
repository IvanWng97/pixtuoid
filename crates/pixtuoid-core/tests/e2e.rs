#![cfg(feature = "test-renderer")]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::render::test_renderer::TestRenderer;
use pixtuoid_core::render::Renderer;
use pixtuoid_core::sprite::format::load_pack_from_strings;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentEvent, AgentId, Reducer, SceneState, Transport};

#[test]
fn scripted_timeline_drives_scene_through_states() {
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let mut renderer = TestRenderer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    let mut now = SystemTime::now();
    let mut step = |events: Vec<AgentEvent>,
                    dt_ms: u64,
                    r: &mut Reducer,
                    s: &mut SceneState,
                    render: &mut TestRenderer| {
        for ev in events {
            r.apply(s, ev, now, Transport::Hook);
        }
        render.record(s);
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
        &mut renderer,
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
        &mut renderer,
    );

    step(
        vec![AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::Waiting {
            agent_id: id,
            reason: "permission?".into(),
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::SessionEnd { agent_id: id }],
        10,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    let snaps = renderer.snapshots.lock().unwrap();
    assert_eq!(snaps.len(), 5);
    assert_eq!(snaps[0].agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(matches!(
        snaps[1].agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));
    // After ActivityEnd the slot is debounced (ACTIVE_GRACE_WINDOW =
    // 1500ms) — it stays visually Active so that rapid CC tool chains
    // (PreToolUse → PostToolUse → PreToolUse) read as continuous work
    // instead of flickering. The transition to Idle is realized later
    // by `reducer.tick` (or by another event arriving past the
    // window). `pending_idle_at` is the signal that the debounce is
    // armed.
    let slot2 = snaps[2].agents.get(&id).unwrap();
    assert!(matches!(slot2.state, ActivityState::Active { .. }));
    assert!(slot2.pending_idle_at.is_some());
    assert!(matches!(
        snaps[3].agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));
    // After SessionEnd the slot is marked for exit (renderer plays the
    // walkout animation) and the reducer's sweep removes it ~4.5s later
    // on the next tick / event. The slot is still present in the
    // immediate snapshot but has `exiting_at` set.
    let exit_slot = snaps[4]
        .agents
        .get(&id)
        .expect("slot still present for exit animation");
    assert!(
        exit_slot.exiting_at.is_some(),
        "SessionEnd should mark exiting_at, not drop immediately"
    );
}

/// Exercise the `Renderer` trait impl + the `count()` convenience wrapper
/// directly — the scripted timeline above records via the inherent `record()`
/// helper, so the trait-signature path and `count()` are otherwise uncovered.
#[test]
fn test_renderer_trait_render_increments_count() {
    let mut r = TestRenderer::new();
    let scene = SceneState::uniform(4);
    // The trait impl ignores the pack, so any in-memory pack satisfies it.
    let pack = load_pack_from_strings(
        "[pack]\nname=\"x\"\nversion=\"1\"\n\
         [palette]\n\"A\"=\"#010203\"\n\
         [animations.idle]\nframes=[\"i.sprite\"]\nframe_ms=100\n",
        &[("i.sprite", "@frame 0\nA")],
    )
    .unwrap();

    assert_eq!(r.count(), 0);
    <TestRenderer as Renderer>::render(&mut r, &scene, &pack, SystemTime::now()).unwrap();
    assert_eq!(r.count(), 1, "trait render must record one snapshot");
}
