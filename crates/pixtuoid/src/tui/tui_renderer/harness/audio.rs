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

#[test]
fn floor_switch_reprimes_without_a_chime_volley() {
    // Riding the elevator to a floor full of EXISTING agents must not fire
    // their door chimes: the switch installs a fresh tracker whose first
    // observe only primes. (Completes lens-1 F3: the enabled-path wiring
    // through an actual floor change.)
    let cap = 16;
    let scene = scene_with(
        vec![
            active_on("/s/f0.jsonl", 0, 0),
            active_on("/s/f1a.jsonl", 1, cap),
            active_on("/s/f1b.jsonl", 1, cap + 1),
        ],
        cap,
    );
    let mut r = build(80, 40, vec![]);
    let (handle, rx) = AudioHandle::test_pair();
    r.set_audio(handle);
    let pack = pack();
    let mut now = t0();
    r.render(&scene, &pack, now).expect("prime on floor 0");
    drain_frames(&rx);

    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack, &mut now, 1);
    let after_switch: Vec<_> = drain_frames(&rx)
        .into_iter()
        .flat_map(|f| f.events)
        .collect();
    assert!(
        after_switch.is_empty(),
        "arriving on floor 1 must not chime its existing agents: {after_switch:?}"
    );

    // …but a GENUINE arrival on the new floor still chimes
    let scene = scene_with(
        vec![
            active_on("/s/f0.jsonl", 0, 0),
            active_on("/s/f1a.jsonl", 1, cap),
            active_on("/s/f1b.jsonl", 1, cap + 1),
            active_on("/s/f1-new.jsonl", 1, cap + 2),
        ],
        cap,
    );
    now += std::time::Duration::from_millis(33);
    r.render(&scene, &pack, now).expect("render");
    let arrivals: Vec<_> = drain_frames(&rx)
        .into_iter()
        .flat_map(|f| f.events)
        .collect();
    assert!(
        arrivals.contains(&pixtuoid_scene::audio::OneShot::DoorChime),
        "a real floor-1 arrival chimes after the re-prime: {arrivals:?}"
    );
}

#[test]
fn footer_note_glyph_tracks_effective_audibility() {
    // ♩ = "you would hear sound right now": enabled + unmuted shows it,
    // muting (the m/p combined state on the handle) hides it, and the
    // default DISABLED handle never shows it (a failed lazy spawn must not
    // advertise sound that isn't playing).
    let scene = scene_with(vec![active_on("/f/a.jsonl", 0, 0)], 16);
    let pack = pack();

    let mut silent = build(80, 40, vec![]);
    silent.render(&scene, &pack, t0()).expect("render");
    assert!(
        !frame_text(silent.frame_buffer()).contains('\u{2669}'),
        "a disabled handle shows no note glyph"
    );

    let mut r = build(80, 40, vec![]);
    let (handle, _rx) = AudioHandle::test_pair();
    r.set_audio(handle.clone());
    r.render(&scene, &pack, t0()).expect("render");
    assert!(
        frame_text(r.frame_buffer()).contains('\u{2669}'),
        "enabled + unmuted shows ♩ in the footer"
    );

    handle.set_muted(true);
    r.render(&scene, &pack, t0()).expect("render");
    assert!(
        !frame_text(r.frame_buffer()).contains('\u{2669}'),
        "muting hides the glyph"
    );

    // volume 0 is silence too: enabled + unmuted at 0% must not advertise
    // sound (the audio_audible volume gate, through the real render path)
    handle.set_muted(false);
    handle.set_volume(0.0);
    r.render(&scene, &pack, t0()).expect("render");
    assert!(
        !frame_text(r.frame_buffer()).contains('\u{2669}'),
        "0% volume shows no note glyph"
    );
    handle.set_volume(0.4);
    r.render(&scene, &pack, t0()).expect("render");
    assert!(
        frame_text(r.frame_buffer()).contains('\u{2669}'),
        "restoring volume restores the glyph"
    );
}
