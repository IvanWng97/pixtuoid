use super::*;
use pixtuoid_core::id::AgentId;
use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn frame_layout_memo_matches_fresh_compute_across_hits_resizes_and_none() {
    let mut ctx = FloorCtx::new();
    let fresh = crate::layout::Layout::compute_with_seed(192, 156, None, 0).unwrap();
    let a = ctx.frame_layout(192, 156, 0).unwrap();
    let b = ctx.frame_layout(192, 156, 0).unwrap();
    // Pointer identity first: the value-equality below would still pass a reverted
    // `Arc::new((**l).clone())`.
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "a memo hit must share the memoized Arc, not deep-clone it"
    );
    for l in [&a, &b] {
        assert_eq!(l.walkable, fresh.walkable);
        assert_eq!(l.reachable, fresh.reachable);
        assert_eq!(l.home_desks.len(), fresh.home_desks.len());
    }
    let resized = ctx.frame_layout(120, 100, 0).unwrap();
    let fresh_resized = crate::layout::Layout::compute_with_seed(120, 100, None, 0).unwrap();
    assert_eq!(resized.walkable, fresh_resized.walkable);
    // A too-small buffer is None and must not poison the memo.
    assert!(ctx.frame_layout(3, 3, 0).is_none());
    assert_eq!(
        ctx.frame_layout(192, 156, 0).unwrap().walkable,
        fresh.walkable
    );
}

#[test]
fn daemons_projects_onto_the_ground_floor_only() {
    use pixtuoid_core::state::{DaemonInstanceId, DaemonLiveness, DaemonPresence};
    let mut scene = SceneState::uniform(16);
    scene.floor_capacities[1] = 16;
    // TWO gateways of one source, so the projection is also pinned to carry the
    // WHOLE roster (an `Option`-shaped projection would drop the second lobster).
    for port in ["18789", "19789"] {
        scene.insert_daemon(
            pixtuoid_core::source::openclaw::SOURCE_NAME,
            DaemonInstanceId::new(port).expect("non-empty"),
            DaemonPresence {
                liveness: DaemonLiveness::UP,
                active_sessions: 0,
                last_seen: SystemTime::UNIX_EPOCH,
                entered_at: SystemTime::UNIX_EPOCH,
                in_flight_runs: Default::default(),
                current_pid: Some(1),
            },
        );
    }
    assert_eq!(
        project_floor_scene(&scene, 0).daemons().count(),
        2,
        "floor 0 carries EVERY gateway mascot"
    );
    assert_eq!(
        project_floor_scene(&scene, 1).daemons().count(),
        0,
        "floor 1+ must NOT (render-once invariant)"
    );
}

#[test]
fn door_anim_excludes_arrived_entry_profiles() {
    use crate::motion::MotionState;
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let id = AgentId::from_transcript_path("/p/door.jsonl");
    let mut fctx = FloorCtx::new();
    let mut ms = MotionState::new(id);
    // Entry walk: duration 2000ms + pause 300ms → walk_arrived at 2300ms.
    ms.entry = Some(crate::motion::WalkLeg {
        started_at: t0,
        profile: WalkProfile {
            duration_ms: 2000,
            pause_ms: 300,
            path_len_octile: 500,
            v_cruise: 0.36,
            accel: 6.5e-4,
        },
        from: crate::layout::Point { x: 0, y: 0 },
    });
    fctx.motion.insert(id, ms);

    fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(1000));
    assert_eq!(
        fctx.door_anim_max_ms, 2300,
        "in-flight entry walk should drive the door cosmetic window"
    );

    // Past arrival, even though MotionState.entry is never cleared for this agent.
    fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(3000));
    assert_eq!(
        fctx.door_anim_max_ms, 0,
        "an arrived entry profile must not hold the door open for the agent's lifetime"
    );
}

#[test]
fn floor_ctx_default_equals_new() {
    let d = FloorCtx::default();
    assert_eq!(
        d.door_anim_max_ms, 0,
        "FloorCtx::default() must match new() (door_anim_max_ms == 0)"
    );
    assert!(
        d.motion.is_empty(),
        "default FloorCtx has no in-flight motion"
    );
}

#[test]
fn lighting_state_default_equals_new() {
    assert_eq!(
        LightingState::default().level(),
        LightingState::new().level(),
        "LightingState::default() must equal new()"
    );
    assert_eq!(
        LightingState::default().level(),
        1.0,
        "a fresh LightingState is fully lit"
    );
}

fn make_scene(n: usize, max_desks: usize) -> SceneState {
    let mut s = SceneState::uniform(max_desks);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for i in 0..n {
        let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
        let floor_idx = s.floor_of(GlobalDeskIndex(i));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("cc"),
                session_id: Arc::from(format!("s{i}").as_str()),
                cwd: Arc::from(Path::new("/repo")),
                label: format!("a{i}").into(),
                state: ActivityState::Idle,
                state_started_at: now,
                created_at: now,
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,

                desk_index: GlobalDeskIndex(i),
                floor_idx,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    s
}

#[test]
fn floor_of_maps_desk_to_floor() {
    let s = SceneState::uniform(16);
    assert_eq!(s.floor_of(GlobalDeskIndex(0)), 0);
    assert_eq!(s.floor_of(GlobalDeskIndex(15)), 0);
    assert_eq!(s.floor_of(GlobalDeskIndex(16)), 1);
    assert_eq!(s.floor_of(GlobalDeskIndex(31)), 1);
    assert_eq!(s.floor_of(GlobalDeskIndex(32)), 2);
}

#[test]
fn floor_local_desk_remaps_to_floor_range() {
    let s = SceneState::uniform(16);
    assert_eq!(
        s.floor_local_desk(GlobalDeskIndex(0)),
        FloorLocalDeskIndex(0)
    );
    assert_eq!(
        s.floor_local_desk(GlobalDeskIndex(16)),
        FloorLocalDeskIndex(0)
    );
    assert_eq!(
        s.floor_local_desk(GlobalDeskIndex(17)),
        FloorLocalDeskIndex(1)
    );
    assert_eq!(
        s.floor_local_desk(GlobalDeskIndex(31)),
        FloorLocalDeskIndex(15)
    );
}

#[test]
fn num_floors_with_overflow() {
    let scene = make_scene(20, 16);
    assert_eq!(num_floors(&scene), 2);
}

#[test]
fn num_floors_exact_fit() {
    let scene = make_scene(16, 16);
    assert_eq!(num_floors(&scene), 1);
}

#[test]
fn num_floors_empty() {
    let scene = make_scene(0, 16);
    assert_eq!(num_floors(&scene), 1);
}

#[test]
fn build_floor_scene_filters_and_remaps() {
    let scene = make_scene(20, 16);

    let floor0 = build_floor_scene(&scene, 0);
    assert_eq!(floor0.len(), 16);
    for p in &floor0 {
        assert!(p.desk.0 < 16, "local desk {} out of range", p.desk.0);
    }

    let floor1 = build_floor_scene(&scene, 1);
    assert_eq!(floor1.len(), 4);
    let mut indices: Vec<usize> = floor1.iter().map(|p| p.desk.0).collect();
    indices.sort();
    assert_eq!(indices, vec![0, 1, 2, 3]);
    // The slot's GLOBAL desk_index is untouched: floor 1's agents keep their real
    // allocation until project_floor_scene's documented re-host.
    let mut globals: Vec<usize> = floor1.iter().map(|p| p.slot.desk_index.0).collect();
    globals.sort();
    assert_eq!(globals, vec![16, 17, 18, 19]);
}

#[test]
fn build_floor_scene_remap_is_local_global_coincident() {
    // Within a projected `uniform(cap)` scene the global desk space coincides with
    // its only floor's local space — what makes `single_floor_local` an identity.
    let scene = make_scene(20, 16);
    for floor_idx in 0..num_floors(&scene) {
        let projected = project_floor_scene(&scene, floor_idx);
        for slot in projected.agents.values() {
            assert_eq!(projected.floor_of(slot.desk_index), 0);
            assert_eq!(
                projected.floor_local_desk(slot.desk_index).0,
                slot.desk_index.0,
                "projected scene: bridge must be the identity"
            );
            assert_eq!(
                projected.floor_local_desk(slot.desk_index),
                slot.desk_index.single_floor_local(),
                "typed bridge and identity cast must agree in a projection"
            );
        }
    }
}

#[test]
fn build_floor_scene_skips_agent_below_grown_offset() {
    // Desk 5 is assigned on floor 1 while floor 0 has capacity 4; floor 0 then
    // grows to 8, so `floor_range(1).start` passes the agent's own desk.
    let mut s = SceneState::new([4, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let id = AgentId::from_transcript_path("/p/stale.jsonl");
    s.agents.insert(
        id,
        AgentSlot {
            agent_id: id,
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(Path::new("/repo")),
            label: "stale".into(),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(5),
            floor_idx: 1,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        },
    );
    s.floor_capacities = [8, 4, 0, 0, 0, 0, 0, 0, 0, 0];
    let floor1 = build_floor_scene(&s, 1);
    assert!(
        floor1.is_empty(),
        "agent below grown offset must be skipped, not mapped to desk 0"
    );
}

#[test]
fn num_floors_variable_capacities() {
    // F0: 0..4, F1: 4..12 — 6 agents span 2 floors
    let mut s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for i in 0..6 {
        let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
        let floor_idx = s.floor_of(GlobalDeskIndex(i));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("cc"),
                session_id: Arc::from(format!("s{i}").as_str()),
                cwd: Arc::from(Path::new("/repo")),
                label: format!("a{i}").into(),
                state: ActivityState::Idle,
                state_started_at: now,
                created_at: now,
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    assert_eq!(num_floors(&s), 2);
}

#[test]
fn transition_t_progresses() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let tr = FloorTransition::new(0, 1, start);

    assert!((tr.t(start) - 0.0).abs() < f32::EPSILON);

    let mid = start + Duration::from_millis(450);
    let t_mid = tr.t(mid);
    assert!(
        t_mid > 0.0 && t_mid < 1.0,
        "mid should be between 0 and 1, got {t_mid}"
    );

    let end = start + Duration::from_millis(900);
    assert!((tr.t(end) - 1.0).abs() < f32::EPSILON);
    assert!(!tr.is_done(start + Duration::from_millis(450)));
    assert!(tr.is_done(end));
}

#[test]
fn transition_t_clamps_past_duration() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let tr = FloorTransition::new(0, 1, start);

    let past = start + Duration::from_millis(1000);
    assert!((tr.t(past) - 1.0).abs() < f32::EPSILON);
    assert!(tr.is_done(past));
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
}

#[test]
fn light_steady_state_populated() {
    let mut light = LightingState::new();
    let start = t0();
    for ms in (0..3_000).step_by(33) {
        let level = light.tick(false, start + Duration::from_millis(ms));
        assert!(
            (level - 1.0).abs() < 1e-6,
            "populated steady state drifted: ms={ms} level={level}"
        );
    }
}

#[test]
fn light_holds_during_debounce_window() {
    let mut light = LightingState::new();
    let start = t0();
    light.tick(true, start);
    // 4 s after going empty, inside the 5 s debounce.
    let level = light.tick(true, start + Duration::from_millis(4_000));
    assert!(
        (level - 1.0).abs() < 1e-6,
        "level dropped before debounce expired: {level}"
    );
}

#[test]
fn light_eases_toward_min_after_debounce() {
    let mut light = LightingState::new();
    let start = t0();
    light.tick(true, start);
    // 6 s: the debounce expired 1 s ago, ~1.25 tau of fade.
    let level = light.tick(true, start + Duration::from_millis(6_000));
    assert!(level < 0.95, "no fade started after debounce: {level}");
    assert!(level > LightingState::MIN_LEVEL, "overshot floor: {level}");
}

#[test]
fn light_converges_to_min_when_empty_long_enough() {
    let mut light = LightingState::new();
    let start = t0();
    // A realistic frame cadence for 30 s, so the exponential ease has fully landed.
    for ms in (0..30_000).step_by(33) {
        light.tick(true, start + Duration::from_millis(ms));
    }
    let level = light.level();
    assert!(
        (level - LightingState::MIN_LEVEL).abs() < 1e-3,
        "did not converge to MIN_LEVEL: {level}"
    );
}

#[test]
fn light_rises_back_when_repopulated() {
    let mut light = LightingState::new();
    let start = t0();
    for ms in (0..20_000).step_by(33) {
        light.tick(true, start + Duration::from_millis(ms));
    }
    assert!(light.level() < 0.2);
    let later = start + Duration::from_millis(20_000);
    for ms in (0..3_000).step_by(33) {
        light.tick(false, later + Duration::from_millis(ms));
    }
    let level = light.level();
    assert!(level > 0.95, "did not rise back when repopulated: {level}");
}

#[test]
fn light_resets_empty_since_when_repopulated() {
    let mut light = LightingState::new();
    let start = t0();
    light.tick(true, start);
    light.tick(true, start + Duration::from_millis(3_000));
    light.tick(false, start + Duration::from_millis(3_500));
    // Empty again: the debounce must restart here, so the 7.5 s sample is only
    // 3.9 s into the new window and must still hold at 1.0.
    light.tick(true, start + Duration::from_millis(3_600));
    let level = light.tick(true, start + Duration::from_millis(7_500));
    assert!(
        (level - 1.0).abs() < 1e-6,
        "empty_since did not reset on repopulate: {level}"
    );
}

#[test]
fn light_large_dt_does_not_overshoot_or_nan() {
    let mut light = LightingState::new();
    let start = t0();
    light.tick(true, start);
    let later = start + Duration::from_millis(LightingState::EMPTY_DEBOUNCE_MS + 1_000);
    let level = light.tick(true, later);
    assert!(level.is_finite(), "level went non-finite: {level}");
    assert!(
        level >= LightingState::MIN_LEVEL - 1e-6,
        "level undershot floor: {level}"
    );
}

#[test]
fn light_backward_clock_jump_does_not_move_level() {
    let mut light = LightingState::new();
    let start = t0();
    light.tick(false, start);
    let before = light.level();
    // A backward "now" makes duration_since() error; the impl's `.ok()` collapses
    // dt to 0.
    let backward = start - Duration::from_millis(500);
    let level = light.tick(true, backward);
    assert!(
        (level - before).abs() < 1e-9,
        "backward clock jump moved level: before={before} after={level}"
    );
}

#[test]
fn light_snap_to_empty_forces_min_level() {
    let mut light = LightingState::new();
    light.snap_to_empty();
    assert!((light.level() - LightingState::MIN_LEVEL).abs() < f32::EPSILON);
}

#[test]
fn coffee_record_stamps_only_new_carriers_and_evict_follows_the_scene() {
    let id = AgentId::from_parts("claude-code", "coffee-test");
    let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let t1 = t0 + std::time::Duration::from_secs(60);
    let mut coffee = CoffeeState::new();
    coffee.record([id], t0);
    assert_eq!(coffee.map().get(&id), Some(&t0), "a new carrier is stamped");
    coffee.record([id], t1);
    assert_eq!(
        coffee.map().get(&id),
        Some(&t0),
        "an already-recorded carrier keeps its original fetch stamp"
    );
    let empty = SceneState::new([8; MAX_FLOORS]);
    coffee.evict_missing(&empty);
    assert!(coffee.map().is_empty());
}

#[test]
fn coffee_second_trip_after_steam_window_restamps() {
    let id = AgentId::from_parts("claude-code", "coffee-refetch");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut coffee = CoffeeState::new();
    coffee.record([id], t0);
    let within = t0 + Duration::from_secs(CoffeeState::STEAM_WINDOW_SECS - 1);
    coffee.record([id], within);
    assert_eq!(
        coffee.map().get(&id),
        Some(&t0),
        "a re-report within the steam window keeps the original stamp"
    );
    let refetch = t0 + Duration::from_secs(CoffeeState::STEAM_WINDOW_SECS * 3);
    coffee.record([id], refetch);
    assert_eq!(
        coffee.map().get(&id),
        Some(&refetch),
        "a fetch after the steam window expired must restamp"
    );
}

#[test]
fn coffee_record_keeps_stamp_on_a_backward_clock_step() {
    // An NTP/suspend step makes `duration_since` err, which must read as
    // not-expired: the two forward tests can't catch a treat-error-as-expired
    // regression because their `duration_since` never errs.
    let id = AgentId::from_parts("claude-code", "coffee-backclock");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut coffee = CoffeeState::new();
    coffee.record([id], t0);
    coffee.record([id], t0 - Duration::from_secs(10));
    assert_eq!(
        coffee.map().get(&id),
        Some(&t0),
        "a backward clock step must keep the original stamp, not rewind it"
    );
}

#[test]
fn floor_capacity_clamps_to_zero_on_a_too_small_buffer() {
    assert_eq!(floor_capacity(3, 3, 0), 0);
    assert!(floor_capacity(192, 160, 0) > 0);
}

#[test]
fn transition_escapes_a_backward_clock_step() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let tr = FloorTransition::new(0, 1, start);
    let wobble = start - Duration::from_millis(100);
    assert!(
        !tr.is_done(wobble),
        "a small wobble must not abort the slide"
    );
    assert!((tr.t(wobble) - 0.0).abs() < f32::EPSILON);
    // Without the escape the renderer stays wedged in the transition composite —
    // no labels, tooltips or hit-testing — until the clock re-passes started_at.
    let stepped = start - Duration::from_millis(tr.duration_ms * 2);
    assert!(
        tr.is_done(stepped),
        "a large backward clock step must complete the transition"
    );
}

#[test]
fn render_floor_paints_the_flame_crown_for_a_top_tier_agent() {
    // Driven through the FULL pass: a projection or sim/paint hop dropping
    // slot.model/effort fails here while the unit-level paint test stays green.
    let pack = crate::embedded_pack::test_default_pack();
    let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let mut scene = make_scene(1, 8);
    let slot = scene.agents.values_mut().next().expect("one agent");
    slot.model = Some("claude-fable-5".into());
    slot.effort = Some(pixtuoid_core::state::EffortObservation::new(
        "ultra".into(),
        now,
    ));
    let mut fctx = FloorCtx::new();
    let mut buf = RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
    let mut coffee = CoffeeState::new();
    let mut chitchat = HashMap::new();
    render_floor(
        &mut fctx,
        &mut buf,
        &mut coffee,
        &mut chitchat,
        FrameInputs {
            scene: &scene,
            pack: &pack,
            theme,
            now,
            size: Size { w: 192, h: 160 },
            floor_meta: FloorMeta::ground(),
            active_pet: None,
            floor_pet: None,
            debug_walkable: false,
        },
    )
    .expect("layout");
    // Against the SAME scene with the crown's inputs cleared: the foreground's
    // day/night wash blends every drawable, so the crown's authored RGB never
    // reaches the buffer and an equality probe pins nothing. What must survive
    // the full pass is that the crown CHANGES the frame.
    let mut plain = scene.clone();
    let slot = plain.agents.values_mut().next().expect("one agent");
    slot.model = None;
    slot.effort = None;
    let mut fctx2 = FloorCtx::new();
    let mut buf2 = RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
    let mut coffee2 = CoffeeState::new();
    let mut chitchat2 = HashMap::new();
    render_floor(
        &mut fctx2,
        &mut buf2,
        &mut coffee2,
        &mut chitchat2,
        FrameInputs {
            scene: &plain,
            pack: &pack,
            theme,
            now,
            size: Size { w: 192, h: 160 },
            floor_meta: FloorMeta::ground(),
            active_pet: None,
            floor_pet: None,
            debug_walkable: false,
        },
    )
    .expect("layout");
    // What this pins is that model/effort REACH the painter through projection
    // and the sim/paint hop — not that the crown itself drew. The burn tier also
    // tints the sprite over the crown's own pixels, so no scoping of this diff
    // can isolate the crown; `paint_flame_crown_draws_its_pattern` owns that half.
    let differing = (0..buf.height())
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf.get(x, y) != buf2.get(x, y))
        .count();
    assert!(
        differing > 0,
        "a top-tier agent rendered byte-identical to a model-less one — \
         slot.model/effort were dropped before the painter"
    );
}

#[test]
fn render_floor_paints_records_coffee_state_and_survives_a_tiny_buffer() {
    let pack = crate::embedded_pack::test_default_pack();
    let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let scene = SceneState::new([8; MAX_FLOORS]);
    let mut fctx = FloorCtx::new();
    let mut buf = RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
    let mut coffee = CoffeeState::new();
    let mut chitchat = HashMap::new();

    let none = render_floor(
        &mut fctx,
        &mut buf,
        &mut coffee,
        &mut chitchat,
        FrameInputs {
            scene: &scene,
            pack: &pack,
            theme,
            now,
            size: Size { w: 8, h: 8 },
            floor_meta: FloorMeta::ground(),
            active_pet: None,
            floor_pet: None,
            debug_walkable: false,
        },
    );
    assert!(none.is_none(), "an unlayoutable size returns None");
    assert_eq!(
        (buf.width(), buf.height()),
        (8, 8),
        "the buffer was still sized"
    );

    let layout = render_floor(
        &mut fctx,
        &mut buf,
        &mut coffee,
        &mut chitchat,
        FrameInputs {
            scene: &scene,
            pack: &pack,
            theme,
            now,
            size: Size { w: 160, h: 96 },
            floor_meta: FloorMeta::ground(),
            active_pet: None,
            floor_pet: None,
            debug_walkable: false,
        },
    );
    assert!(layout.is_some(), "a layoutable size returns the layout");
    let bg = theme.surface.bg_fallback;
    assert!(
        buf.as_slice()
            .iter()
            .any(|p| *p != pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 } && *p != bg),
        "the pixel pass painted office content"
    );
}

#[test]
fn floor_session_render_owns_the_dual_eviction() {
    let pack = crate::embedded_pack::test_default_pack();
    let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let gone = AgentId::from_parts("claude-code", "session-evict");
    let mut session = FloorSession::new();
    session
        .floor
        .ctx
        .motion
        .insert(gone, MotionState::new(gone));
    session.office.coffee.insert(gone, now);

    let scene = SceneState::new([8; MAX_FLOORS]);
    let layout = session.render(FrameInputs {
        scene: &scene,
        pack: &pack,
        theme,
        now,
        size: Size { w: 160, h: 96 },
        floor_meta: FloorMeta::ground(),
        active_pet: None,
        floor_pet: None,
        debug_walkable: false,
    });
    assert!(layout.is_some(), "a layoutable size renders");
    assert!(
        !session.floor.ctx.motion.contains_key(&gone),
        "render() evicts the floor half (motion) — the floating-leak class"
    );
    assert!(
        !session.office.coffee.map().contains_key(&gone),
        "render() evicts the office half (coffee) — the cup leaves with the agent"
    );
}

#[test]
fn floor_session_render_surfaces_the_sims_occupied_waypoints() {
    // `last_occupied` is the set the shared `AudioObserver` reads, so recording it
    // here is what lets a windowed painter avoid re-running the sim.
    let pack = crate::embedded_pack::test_default_pack();
    let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut scene = make_scene(1, 8);
    for slot in scene.agents.values_mut() {
        slot.created_at = now0;
        slot.state_started_at = now0;
        slot.last_event_at = now0;
    }
    let mut session = FloorSession::new();
    assert!(session.last_occupied.is_empty(), "empty before any render");
    // Requiring the FALL back to empty is the anti-stick tooth: an accumulating
    // `last_occupied` is monotone non-decreasing and can never produce it.
    let mut occupied_ever = false;
    let mut fell_back_empty = false;
    for step in 0..600u64 {
        let now = now0 + Duration::from_secs(3 * step);
        let layout = session
            .render(FrameInputs {
                scene: &scene,
                pack: &pack,
                theme,
                now,
                size: Size { w: 160, h: 96 },
                floor_meta: FloorMeta::ground(),
                active_pet: None,
                floor_pet: None,
                debug_walkable: false,
            })
            .expect("160x96 lays out");
        if session.last_occupied.is_empty() {
            if occupied_ever {
                fell_back_empty = true;
                break;
            }
        } else {
            for &wp in &session.last_occupied {
                assert!(
                    wp < layout.waypoints.len(),
                    "occupied index {wp} must be a real waypoint"
                );
            }
            occupied_ever = true;
        }
    }
    assert!(
        occupied_ever,
        "the idle agent never occupied a waypoint in 30 min of sim"
    );
    assert!(
        fell_back_empty,
        "occupancy never fell back to empty — last_occupied accumulates instead of tracking the frame"
    );
    let none = session.render(FrameInputs {
        scene: &scene,
        pack: &pack,
        theme,
        now: now0,
        size: Size { w: 8, h: 8 },
        floor_meta: FloorMeta::ground(),
        active_pet: None,
        floor_pet: None,
        debug_walkable: false,
    });
    assert!(none.is_none());
    assert!(
        session.last_occupied.is_empty(),
        "an unlayoutable render clears the stale occupancy"
    );
}

#[test]
fn floor_session_observe_advances_the_world_without_a_pixel_buffer() {
    let pack = crate::embedded_pack::test_default_pack();
    let scene = make_scene(1, 8);
    let id = AgentId::from_transcript_path("/p/0.jsonl");
    let t = t0() + Duration::from_millis(100); // 100ms in: entry walk in flight
    let mut session = FloorSession::new();

    let frame = session
        .observe(&scene, &pack, 160, 96, FloorMeta::ground(), t)
        .expect("a layoutable size observes");
    assert!(
        frame.poses.contains_key(&id),
        "the frame carries the agent's routed pose"
    );
    assert!(
        session.floor.ctx.motion.contains_key(&id),
        "the sim advanced: the entry leg was snapshotted into motion"
    );
    assert!(
        session.floor.ctx.door_anim_max_ms > 0,
        "the epilogue ran headlessly: the in-flight entry drives the door clamp"
    );
    assert_eq!(
        (session.buf().width(), session.buf().height()),
        (0, 0),
        "no pixel buffer was bought"
    );

    assert!(
        session
            .observe(&scene, &pack, 8, 8, FloorMeta::ground(), t)
            .is_none(),
        "an unlayoutable size observes nothing"
    );
}

#[test]
fn session_types_default_equals_new() {
    assert_eq!(PerFloor::default().ctx.door_anim_max_ms, 0);
    assert_eq!(
        (
            PerFloor::default().buf.width(),
            PerFloor::default().buf.height()
        ),
        (0, 0)
    );
    assert!(PerOffice::default().coffee.map().is_empty());
    assert!(PerOffice::default().chitchat.is_empty());
    let s = FloorSession::default();
    assert!(s.floor.ctx.motion.is_empty());
    assert!(s.office.coffee.map().is_empty());
}

#[test]
fn reset_frame_cache_clears_cached_sprites() {
    use crate::frame_cache::FrameKey;
    use pixtuoid_core::{sprite::Frame, AgentId};

    let mut s = FloorSession::new();
    // Prime the cache, so the assertion below distinguishes a real reset from a
    // no-op on an already-empty cache.
    s.floor.ctx.cache.get_or_make(
        FrameKey {
            agent_id: AgentId::from_parts("test", "agent"),
            anim_name: "idle",
            frame_idx: 0,
            flip_x: false,
            glow_tint: None,
            burn: crate::burn::BurnTier::Normal,
        },
        Frame::default,
    );
    assert_eq!(
        s.floor.ctx.cache.len(),
        1,
        "priming must populate the cache"
    );

    s.reset_frame_cache();
    assert_eq!(
        s.floor.ctx.cache.len(),
        0,
        "reset must clear a populated cache"
    );
}

#[test]
fn audio_observer_frame_composes_stems_and_track_from_the_scene() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = make_scene(4, 16);
    let occupied = std::collections::HashSet::new();
    let mut obs = AudioObserver::new();
    let frame = obs.frame(&scene, &occupied, |_| None, 0, now);
    let precip = crate::pixel_painter::precipitation_level(now);
    assert_eq!(
        frame.stems,
        crate::audio::stem_levels(&crate::board::per_floor_counts(&scene)[0], precip),
        "stems must equal stem_levels(per_floor_counts[floor], precip)"
    );
    assert_eq!(
        frame.track,
        crate::audio::select_track(
            crate::pixel_painter::is_day_at(now),
            precip,
            crate::audio::track_epoch(now),
        ),
        "track must equal select_track(is_day_at(now), precip, track_epoch(now))"
    );
}

#[test]
fn audio_observer_reprimes_on_floor_switch_so_the_new_floor_is_silent() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = make_scene(4, 16); // agents live on floor 0
    let printer = |i: usize| (i == 0 || i == 1).then_some(crate::layout::WaypointKind::Printer);
    let mut obs = AudioObserver::new();

    let _ = obs.frame(&scene, &std::collections::HashSet::new(), printer, 0, now);
    assert_eq!(obs.primed_floor(), Some(0));

    // Switch to floor 1 with an appliance ALREADY occupied — this would fire
    // PrinterWhir without the reprime.
    let occ0: std::collections::HashSet<usize> = [0usize].into_iter().collect();
    let switch = obs.frame(&scene, &occ0, printer, 1, now);
    assert_eq!(obs.primed_floor(), Some(1));
    assert!(
        switch.events.is_empty(),
        "a floor switch reprimes silently — no cue volley for the new floor"
    );

    let occ01: std::collections::HashSet<usize> = [0usize, 1usize].into_iter().collect();
    let next = obs.frame(&scene, &occ01, printer, 1, now);
    assert!(
        next.events.contains(&crate::audio::OneShot::PrinterWhir),
        "after the reprime, a newly occupied printer still fires"
    );
}

#[test]
fn audio_observer_keeps_cue_edges_warm_so_delivery_resume_fires_no_volley() {
    // Mute-gating: the painter calls frame() EVERY world-frame and gates only
    // DELIVERY, so an arrival during a muted stretch is still consumed here.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let empty = make_scene(0, 16);
    let one = make_scene(1, 16);
    let occ = std::collections::HashSet::new();
    let mut obs = AudioObserver::new();

    let _ = obs.frame(&empty, &occ, |_| None, 0, now);
    let arrival = obs.frame(&one, &occ, |_| None, 0, now);
    assert!(
        arrival.events.contains(&crate::audio::OneShot::DoorChime),
        "an arrival chimes on the frame it happens"
    );
    let resumed = obs.frame(&one, &occ, |_| None, 0, now);
    assert!(
        !resumed.events.contains(&crate::audio::OneShot::DoorChime),
        "no volley on resume — the observer saw the agent while muted"
    );
}

// The frame-seam scale-invariance property lives in `render_scale.rs` — the
// version here asserted only desk count + buffer width, which hold on a garbled frame.

#[test]
fn the_foreground_layer_is_lit_by_the_clock() {
    // The day/night overlays sweep the floor band BEFORE the drawables paint, so
    // every enqueued piece — desks, appliances, plants, wall decor, characters,
    // chairs, partitions — carried no time-of-day term at all. Measured at the
    // whole LAYER rather than one piece: the paint loop is the shared seam, so a
    // new DrawableKind inherits this without touching the test.
    use crate::localclock::at_hour;
    let render = |now: SystemTime| {
        let pack = crate::embedded_pack::test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("normal theme");
        let scene = make_scene(6, 8);
        let mut fctx = FloorCtx::new();
        let mut buf = RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        let mut coffee = CoffeeState::new();
        let mut chitchat = HashMap::new();
        render_floor(
            &mut fctx,
            &mut buf,
            &mut coffee,
            &mut chitchat,
            FrameInputs {
                scene: &scene,
                pack: &pack,
                theme,
                now,
                size: Size { w: 192, h: 160 },
                floor_meta: FloorMeta::ground(),
                active_pet: None,
                floor_pet: None,
                debug_walkable: false,
            },
        )
        .expect("layout");
        buf
    };
    let (noon, night) = (render(at_hour(12)), render(at_hour(2)));
    let (w, h) = (noon.width(), noon.height());
    let frozen = |x: u16, y: u16| noon.get(x, y) == night.get(x, y);
    // CLUSTERED, not counted. Two different blends can round to one u8, so a few
    // scattered matches are arithmetic — a painter left outside the clock instead
    // freezes a whole PIECE, and every pixel of it has a frozen neighbour. A
    // count needs a threshold, and a threshold is what let the corridor runner
    // and the pantry mats pass at 8.3% under a 12% ceiling.
    let mut clustered = Vec::new();
    for y in (h * 30 / 100)..(h * 92 / 100) {
        for x in 0..w {
            if frozen(x, y)
                && [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(dx, dy)| {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        (0..w as i32).contains(&nx)
                            && (0..h as i32).contains(&ny)
                            && frozen(nx as u16, ny as u16)
                    })
            {
                clustered.push((x, y));
            }
        }
    }
    assert!(
        clustered.is_empty(),
        "{} interior pixels are byte-identical at noon and midnight AND adjacent \
         to another such pixel — a painter is running outside the clock, first at \
         {:?}",
        clustered.len(),
        clustered.first()
    );
}

fn neon_mood(active: usize, waiting: usize, idle: usize) -> crate::board::OfficeMood {
    crate::board::OfficeMood::of(crate::board::StateCounts {
        active,
        waiting,
        idle,
        exiting: 0,
        total: active + waiting + idle,
    })
}

const ROOM_LIT: bool = false;
const ROOM_DIMMED: bool = true;
/// A live painter's frame tick — well under the shortest stutter flash.
const FRAME: Duration = Duration::from_millis(33);

#[test]
fn neon_first_tick_snaps_to_the_mood() {
    for (mood, room, want) in [
        (neon_mood(2, 1, 0), ROOM_LIT, NeonLevels::ALERT),
        (neon_mood(2, 0, 3), ROOM_LIT, NeonLevels::BUSY),
        (neon_mood(0, 0, 3), ROOM_LIT, NeonLevels::CALM),
        (neon_mood(0, 0, 0), ROOM_DIMMED, NeonLevels::EMPTY),
        (neon_mood(0, 0, 0), ROOM_LIT, NeonLevels::CALM),
    ] {
        assert_eq!(NeonState::new().tick(mood, room, t0()), want, "{mood:?}");
    }
}

/// The join is REAL: a room that once dimmed and was repopulated never eases back
/// to a bit-exact 1.0, so the sign reads the room's VERDICT, not its level.
#[test]
fn neon_holds_through_a_walkout_in_a_room_that_once_dimmed() {
    let mut light = LightingState::new();
    let mut neon = NeonState::new();
    let mut now = t0();
    let mut run = |empty: bool, mood: crate::board::OfficeMood, ms: u64| {
        let mut last = NeonLevels::CALM;
        for _ in 0..ms / FRAME.as_millis() as u64 {
            now += FRAME;
            light.tick(empty, now);
            last = neon.tick(mood, light.dimmed(), now);
        }
        (last, light.level())
    };
    let (starved, _) = run(
        true,
        neon_mood(0, 0, 0),
        LightingState::EMPTY_DEBOUNCE_MS * 3,
    );
    assert_eq!(starved, NeonLevels::EMPTY);
    let (_, level) = run(false, neon_mood(2, 0, 0), 60_000);
    assert!(level < 1.0, "the premise: the f32 ease stalls short of 1.0");
    // The last agent walks out: the tally is Empty, the room is lit and populated.
    let (walkout, _) = run(false, neon_mood(0, 0, 0), NeonState::FADE_MS as u64 * 2);
    assert_eq!(walkout, NeonLevels::CALM, "a lit room keeps its sign");
}

/// The `--empty` still: one tick after the snap must still judge the floor empty.
#[test]
fn light_snap_to_empty_survives_its_first_tick_and_reads_dimmed() {
    let mut light = LightingState::new();
    light.snap_to_empty();
    assert!(light.dimmed());
    let level = light.tick(true, t0());
    assert_eq!(level, LightingState::MIN_LEVEL);
    assert!(light.dimmed(), "the debounce was back-dated, not re-armed");
    light.tick(false, t0() + FRAME);
    assert!(!light.dimmed(), "and a populated floor clears it");
}

#[test]
fn light_is_dimmed_exactly_once_the_debounce_runs_out() {
    let mut light = LightingState::new();
    let debounce = Duration::from_millis(LightingState::EMPTY_DEBOUNCE_MS);
    light.tick(true, t0());
    light.tick(true, t0() + debounce - Duration::from_millis(1));
    assert!(!light.dimmed());
    light.tick(true, t0() + debounce);
    assert!(light.dimmed());
}

#[test]
fn neon_eases_into_a_new_mood_and_lands_on_it() {
    let mut neon = NeonState::new();
    let fade = Duration::from_millis(NeonState::FADE_MS as u64);
    let alert = neon_mood(2, 1, 0);
    neon.tick(neon_mood(2, 0, 0), ROOM_LIT, t0());
    let changed = t0() + FRAME;
    let first = neon.tick(alert, ROOM_LIT, changed);
    assert_eq!(
        first,
        NeonLevels::BUSY,
        "the change frame still shows the old mood"
    );
    let mid = neon.tick(alert, ROOM_LIT, changed + fade / 2);
    assert!(
        mid.alert > NeonLevels::BUSY.alert && mid.alert < NeonLevels::ALERT.alert,
        "mid-fade alert is between the moods: {mid:?}"
    );
    // Not `fade - 1ms`: an ease-out's last millisecond rounds to the target in f32.
    let late = neon.tick(alert, ROOM_LIT, changed + fade * 3 / 4);
    assert_ne!(
        late,
        NeonLevels::ALERT,
        "still crossing over late in the fade"
    );
    assert_eq!(
        neon.tick(alert, ROOM_LIT, changed + fade),
        NeonLevels::ALERT
    );
}

/// A sign nobody has drawn for longer than a fade has no light to ease from — the
/// wasm still's warm-up step, a floor switched back to.
#[test]
fn neon_snaps_when_its_last_light_is_older_than_a_fade() {
    let fade = Duration::from_millis(NeonState::FADE_MS as u64);
    let (calm, busy) = (neon_mood(0, 0, 1), neon_mood(3, 0, 0));
    let mut fresh = NeonState::new();
    fresh.tick(calm, ROOM_LIT, t0());
    assert_eq!(
        fresh.tick(busy, ROOM_LIT, t0() + fade),
        NeonLevels::CALM,
        "fades"
    );
    let mut stale = NeonState::new();
    stale.tick(calm, ROOM_LIT, t0());
    let later = t0() + fade + Duration::from_millis(1);
    assert_eq!(stale.tick(busy, ROOM_LIT, later), NeonLevels::BUSY, "snaps");
}

#[test]
fn neon_ignores_a_count_change_within_a_mood() {
    let mut neon = NeonState::new();
    neon.tick(neon_mood(0, 2, 0), ROOM_LIT, t0());
    assert_eq!(
        neon.tick(neon_mood(0, 3, 0), ROOM_LIT, t0() + FRAME),
        NeonLevels::ALERT
    );
}

#[test]
fn neon_reversing_mid_fade_starts_from_the_current_light() {
    let mut neon = NeonState::new();
    neon.tick(neon_mood(0, 0, 3), ROOM_LIT, t0());
    let half = Duration::from_millis(NeonState::FADE_MS as u64 / 2);
    neon.tick(neon_mood(0, 1, 3), ROOM_LIT, t0());
    let mid = neon.tick(neon_mood(0, 1, 3), ROOM_LIT, t0() + half);
    let reversed = neon.tick(neon_mood(0, 0, 3), ROOM_LIT, t0() + half);
    assert_eq!(
        reversed, mid,
        "the reversal frame holds the light it interrupted"
    );
}

#[test]
fn neon_holds_on_a_backward_clock() {
    let mut neon = NeonState::new();
    neon.tick(neon_mood(2, 0, 0), ROOM_LIT, t0() + Duration::from_secs(5));
    neon.tick(neon_mood(0, 0, 3), ROOM_LIT, t0() + Duration::from_secs(5));
    let earlier = t0() + Duration::from_secs(1);
    assert_eq!(
        neon.tick(neon_mood(0, 0, 3), ROOM_LIT, earlier),
        NeonLevels::BUSY
    );
}

/// The instant `offset` ms into the stutter cycle that contains `t0()`.
fn in_stutter_cycle(offset: u64) -> SystemTime {
    let base = crate::anim::epoch_ms(t0());
    SystemTime::UNIX_EPOCH + Duration::from_millis(base - base % NeonState::STUTTER_MS + offset)
}

/// A starved sign ticked every `step` across one whole stutter cycle.
fn starved_cycle(step: Duration) -> Vec<(u64, NeonLevels)> {
    let mut neon = NeonState::new();
    let empty = neon_mood(0, 0, 0);
    (0..NeonState::STUTTER_MS)
        .step_by(step.as_millis() as usize)
        .map(|ms| (ms, neon.tick(empty, ROOM_DIMMED, in_stutter_cycle(ms))))
        .collect()
}

#[test]
fn neon_a_starved_tube_flashes_inside_its_windows_and_only_there() {
    for (ms, levels) in starved_cycle(Duration::from_millis(10)).into_iter().skip(1) {
        let in_window = NeonState::STUTTER_FLASHES_MS
            .iter()
            .any(|&(start, end)| (start..end).contains(&ms));
        let want = if in_window {
            NeonLevels::FLASH
        } else {
            NeonLevels::EMPTY
        };
        assert_eq!(levels, want, "{ms}ms");
    }
}

/// A flash shorter than the frame gap can't be drawn: a still (one tick) and the
/// floating window's ambient cadence get the steady tube, never a held flash.
#[test]
fn neon_a_painter_slower_than_a_flash_never_shows_one() {
    let shortest = Duration::from_millis(NeonState::shortest_flash_ms());
    for (ms, levels) in starved_cycle(shortest) {
        assert_eq!(levels, NeonLevels::EMPTY, "{ms}ms at a {shortest:?} tick");
    }
    let just_faster = shortest - Duration::from_millis(1);
    assert!(starved_cycle(just_faster)
        .iter()
        .any(|(_, levels)| *levels == NeonLevels::FLASH));
    let in_a_flash = in_stutter_cycle(NeonState::STUTTER_FLASHES_MS[0].0);
    assert_eq!(
        NeonState::new().tick(neon_mood(0, 0, 0), ROOM_DIMMED, in_a_flash),
        NeonLevels::EMPTY,
        "a still's single tick"
    );
}

#[test]
fn neon_never_flashes_while_lit_or_while_still_coasting_down() {
    let flash_at = in_stutter_cycle(NeonState::STUTTER_FLASHES_MS[0].0);
    let mut lit = NeonState::new();
    lit.tick(neon_mood(0, 0, 3), ROOM_LIT, flash_at - FRAME);
    assert_eq!(
        lit.tick(neon_mood(0, 0, 3), ROOM_LIT, flash_at),
        NeonLevels::CALM
    );
    let mut coasting = NeonState::new();
    let mut now = flash_at - Duration::from_millis(NeonState::FADE_MS as u64 / 2);
    coasting.tick(neon_mood(2, 0, 0), ROOM_LIT, now - FRAME);
    let mut last = coasting.tick(neon_mood(0, 0, 0), ROOM_DIMMED, now);
    while now < flash_at {
        now += Duration::from_millis(10);
        last = coasting.tick(neon_mood(0, 0, 0), ROOM_DIMMED, now);
    }
    assert_ne!(last, NeonLevels::FLASH);
    assert!(last.power > NeonLevels::EMPTY.power, "{last:?}");
}
