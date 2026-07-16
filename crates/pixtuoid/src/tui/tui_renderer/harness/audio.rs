//! Floor-scoped audio wiring, pinned through the PRODUCTION render path
//! (the online-review HIGH on #636): the renderer must feed the audio
//! thread ONLY the floor being viewed — an inverted filter or a re-leaked
//! `scene.agents.keys()` would silently restore cross-floor sound.

use super::*;
use crate::audio::{drain_frames, AudioHandle};

fn active_on(path: &str, floor_idx: usize, desk: usize) -> AgentSlot {
    let mut s = slot(AgentId::from_transcript_path(path), floor_idx, desk, t0());
    s.state = ActivityState::Active {
        tool_use_id: Some(Arc::from("t")),
        detail: Some(Arc::from("Edit")),
        kind: ToolKind::from_display("Edit"),
    };
    s
}

#[test]
fn audio_stems_count_only_the_viewed_floor() {
    // 1 active on floor 0, 3 actives on floor 1. Viewed floor 0 must read
    // MODERATE typing (1 active); a global count (4 actives) would read
    // BUSY — the tiers differ exactly when the filter matters.
    let cap = 16;
    let scene = scene_with(
        vec![
            active_on("/a/f0.jsonl", 0, 0),
            active_on("/a/f1a.jsonl", 1, cap),
            active_on("/a/f1b.jsonl", 1, cap + 1),
            active_on("/a/f1c.jsonl", 1, cap + 2),
        ],
        cap,
    );
    let mut r = build(80, 40, vec![]);
    let (handle, rx) = AudioHandle::test_pair();
    r.set_audio(handle);
    let pack = pack();
    r.render(&scene, &pack, t0()).expect("render");
    let frames = drain_frames(&rx);
    assert!(!frames.is_empty(), "an enabled handle receives frames");
    let stems = frames.last().unwrap().stems;
    let moderate = pixtuoid_scene::audio::stem_levels(
        &pixtuoid_scene::board::StateCounts {
            active: 1,
            waiting: 0,
            idle: 0,
            exiting: 0,
            total: 1,
        },
        0.0,
    );
    assert_eq!(
        stems.typing, moderate.typing,
        "typing level must reflect the VIEWED floor's 1 active, not all 4"
    );
}

#[test]
fn door_chime_fires_only_for_viewed_floor_arrivals() {
    let cap = 16;
    let mut agents = vec![active_on("/d/f0.jsonl", 0, 0)];
    let scene = scene_with(agents.clone(), cap);
    let mut r = build(80, 40, vec![]);
    let (handle, rx) = AudioHandle::test_pair();
    r.set_audio(handle);
    let pack = pack();
    let mut now = t0();
    r.render(&scene, &pack, now).expect("prime render");
    drain_frames(&rx); // discard the priming frames

    // an arrival on ANOTHER floor: silent on the viewed floor
    agents.push(active_on("/d/f1-new.jsonl", 1, cap));
    let scene = scene_with(agents.clone(), cap);
    now += std::time::Duration::from_millis(33);
    r.render(&scene, &pack, now).expect("render");
    let off_floor: Vec<_> = drain_frames(&rx)
        .into_iter()
        .flat_map(|f| f.events)
        .collect();
    assert!(
        off_floor.is_empty(),
        "a floor-1 walk-in must not chime while viewing floor 0: {off_floor:?}"
    );

    // an arrival on THIS floor chimes
    agents.push(active_on("/d/f0-new.jsonl", 0, 1));
    let scene = scene_with(agents, cap);
    now += std::time::Duration::from_millis(33);
    r.render(&scene, &pack, now).expect("render");
    let on_floor: Vec<_> = drain_frames(&rx)
        .into_iter()
        .flat_map(|f| f.events)
        .collect();
    assert!(
        on_floor.contains(&pixtuoid_scene::audio::OneShot::DoorChime),
        "a floor-0 walk-in must chime while viewing floor 0: {on_floor:?}"
    );
}
