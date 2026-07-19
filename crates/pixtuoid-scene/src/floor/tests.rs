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
    // First call (miss) and second call (memo hit) must both equal a fresh
    // compute — the memo is a pure cache, never a source of divergence.
    let a = ctx.frame_layout(192, 156, 0).unwrap();
    let b = ctx.frame_layout(192, 156, 0).unwrap();
    // The memo HIT must hand out the SAME Arc (a refcount bump), not a fresh
    // deep clone — pointer identity is the whole point of the change, and the
    // value-equality below would still pass a reverted `Arc::new((**l).clone())`.
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "a memo hit must share the memoized Arc, not deep-clone it"
    );
    for l in [&a, &b] {
        assert_eq!(l.walkable, fresh.walkable);
        assert_eq!(l.reachable, fresh.reachable);
        assert_eq!(l.home_desks.len(), fresh.home_desks.len());
    }
    // A resize / different seed is a different key: recompute, not a stale hit.
    let resized = ctx.frame_layout(120, 100, 0).unwrap();
    let fresh_resized = crate::layout::Layout::compute_with_seed(120, 100, None, 0).unwrap();
    assert_eq!(resized.walkable, fresh_resized.walkable);
    // A too-small buffer is None and must not poison the memo.
    assert!(ctx.frame_layout(3, 3, 0).is_none());
    assert_eq!(
        ctx.frame_layout(192, 156, 0).unwrap().walkable,
        fresh.walkable
    );
    // (The corridor re-point half of the prologue runs on every call — hit or
    // miss — inside frame_layout; the router keeps no public getter to assert
    // on, and set_preferred_zone's behavior is pinned by the pathfind tests.)
}

#[test]
fn daemons_projects_onto_the_ground_floor_only() {
    // The gateway mascot is global, not per-floor — the projection carries
    // daemons onto floor 0 ONLY, so a multi-floor office renders the lobster
    // exactly once (a regression dropping the gate / flipping the index would
    // duplicate him on every floor).
    use pixtuoid_core::state::{DaemonLiveness, DaemonPresence};
    let mut scene = SceneState::uniform(16);
    scene.floor_capacities[1] = 16; // a second floor exists
    scene.daemons_mut().insert(
        pixtuoid_core::source::openclaw::SOURCE_NAME.to_string(),
        DaemonPresence {
            liveness: DaemonLiveness::UP,
            active_sessions: 0,
            last_seen: SystemTime::UNIX_EPOCH,
            entered_at: SystemTime::UNIX_EPOCH,
            in_flight_run_keys: Default::default(),
            current_pid: Some(1),
        },
    );
    assert!(
        !project_floor_scene(&scene, 0).daemons().is_empty(),
        "floor 0 carries the mascot"
    );
    assert!(
        project_floor_scene(&scene, 1).daemons().is_empty(),
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
    ms.entry = Some((
        t0,
        WalkProfile {
            duration_ms: 2000,
            pause_ms: 300,
            path_len_octile: 500,
            v_cruise: 0.36,
            accel: 6.5e-4,
        },
    ));
    fctx.motion.insert(id, ms);

    // Mid-walk → profile is in-flight → it sets the door window.
    fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(1000));
    assert_eq!(
        fctx.door_anim_max_ms, 2300,
        "in-flight entry walk should drive the door cosmetic window"
    );

    // Past arrival (>= duration + pause) → excluded so the door closes,
    // even though MotionState.entry is never cleared for this agent.
    fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(3000));
    assert_eq!(
        fctx.door_anim_max_ms, 0,
        "an arrived entry profile must not hold the door open for the agent's lifetime"
    );
}

#[test]
fn floor_ctx_default_equals_new() {
    // Both Default impls delegate to new(); pin the equivalence so a future
    // field addition can't make `default()` diverge silently.
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
    // LightingState::default() delegates to new() — both start fully lit.
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
    // The pair keeps the currency honest (#13): the LOCAL desk lives in
    // the typed FloorLocalDeskIndex, while the slot's GLOBAL desk_index
    // is untouched — floor 1's agents keep their real allocation (16..20)
    // until project_floor_scene's documented re-host.
    let mut globals: Vec<usize> = floor1.iter().map(|p| p.slot.desk_index.0).collect();
    globals.sort();
    assert_eq!(globals, vec![16, 17, 18, 19]);
}

#[test]
fn build_floor_scene_remap_is_local_global_coincident() {
    // The doc-comment-backed property on `build_floor_scene`: within a
    // projected `uniform(cap)` scene the global desk space coincides with
    // its (only) floor's local space, so the remapped `GlobalDeskIndex`
    // is simultaneously a valid global index for the smaller scene AND —
    // through the typed bridge — the floor-local index the render path
    // needs. This is what makes `single_floor_local` an identity there.
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
    // Agent assigned desk 5 on floor 1 when floor 0 had capacity 4.
    // Floor 0 later grows to capacity 8. floor_range(1).start = 8,
    // so desk 5 < 8 and the agent should be invisible on floor 1.
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
    // Simulate floor 0 capacity growth
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

// ---- LightingState ----------------------------------------------------

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
}

#[test]
fn light_steady_state_populated() {
    let mut light = LightingState::new();
    let start = t0();
    // Many frames over multiple seconds with `empty=false` should not
    // move the level away from 1.0.
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
    // 4 s after going empty (< 5 s debounce) — target should still be
    // 1.0 so level holds.
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
    // Sample at 6 s (debounce expired 1 s ago, ~1.25 tau of fade).
    let level = light.tick(true, start + Duration::from_millis(6_000));
    assert!(level < 0.95, "no fade started after debounce: {level}");
    assert!(level > LightingState::MIN_LEVEL, "overshot floor: {level}");
}

#[test]
fn light_converges_to_min_when_empty_long_enough() {
    let mut light = LightingState::new();
    let start = t0();
    // Step the tick at a realistic frame cadence for 30 s so the
    // exponential ease has fully landed.
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
    // Drive level all the way down.
    for ms in (0..20_000).step_by(33) {
        light.tick(true, start + Duration::from_millis(ms));
    }
    assert!(light.level() < 0.2);
    // Populated → target snaps to 1.0; verify the ease climbs back.
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
    // Empty for 3 s (within debounce).
    light.tick(true, start);
    light.tick(true, start + Duration::from_millis(3_000));
    // Briefly populated — should clear the debounce timer.
    light.tick(false, start + Duration::from_millis(3_500));
    // Empty again — debounce timer must restart from this moment, so
    // 4 s later we should STILL be holding at 1.0, not faded.
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
    // Huge dt (1 day) past the debounce. exp(-dt/tau) underflows to 0
    // so alpha = 1.0; level should land exactly at target (MIN_LEVEL),
    // not overshoot or produce NaN.
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
    // Bring level to a known mid value via a real tick.
    light.tick(false, start);
    let before = light.level();
    // A backward "now" makes duration_since() error; the impl uses
    // `.ok()` so dt collapses to 0 and the level should not change.
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
    // Re-recording an existing carrier must NOT restart its steam window.
    coffee.record([id], t1);
    assert_eq!(
        coffee.map().get(&id),
        Some(&t0),
        "an already-recorded carrier keeps its original fetch stamp"
    );
    // The agent leaving the scene evicts the cup + stamp (one entry).
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
    // Within the window: per-frame walk-back re-reports keep the stamp.
    let within = t0 + Duration::from_secs(CoffeeState::STEAM_WINDOW_SECS - 1);
    coffee.record([id], within);
    assert_eq!(
        coffee.map().get(&id),
        Some(&t0),
        "a re-report within the steam window keeps the original stamp"
    );
    // A report past the window is a genuinely NEW pantry fetch (the old
    // cup's steam long expired) — the stamp must refresh so the fresh cup
    // steams again instead of landing permanently steam-less.
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
    // Backward clock (now < stored → duration_since errs): `is_ok_and` yields
    // false → not-expired → the old stamp is KEPT, not rewound. Guards against a
    // treat-clock-error-as-expired regression that would restart the steam
    // window (and rewind the stamp) on an NTP/suspend step. The two forward
    // tests can't catch it — their `duration_since` never errs.
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
    // A buffer too small for even one cubicle → compute_with_seed None → the
    // `unwrap_or(0)` clamp. Mutating it to `unwrap()` panics boot-seeding;
    // `unwrap_or(1)` seeds a phantom desk. A normal buffer fits ≥ 1 desk (the
    // `> 0` guards an always-None / constant-0 mutant); the exact count is left
    // to the layout tests — re-deriving it here would just re-run the impl.
    assert_eq!(floor_capacity(3, 3, 0), 0);
    assert!(floor_capacity(192, 160, 0) > 0);
}

#[test]
fn transition_escapes_a_backward_clock_step() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let tr = FloorTransition::new(0, 1, start);
    // A small backward wobble (within one transition duration) is clock
    // jitter: hold at t = 0 and let the clock catch up.
    let wobble = start - Duration::from_millis(100);
    assert!(
        !tr.is_done(wobble),
        "a small wobble must not abort the slide"
    );
    assert!((tr.t(wobble) - 0.0).abs() < f32::EPSILON);
    // A step to before started_at by MORE than the transition's own
    // duration can't be render-loop jitter — without an escape the
    // renderer stays wedged in the transition composite (no labels,
    // tooltips, or hit-testing) until the wall clock re-passes started_at.
    let stepped = start - Duration::from_millis(tr.duration_ms * 2);
    assert!(
        tr.is_done(stepped),
        "a large backward clock step must complete the transition"
    );
}

#[test]
fn render_floor_paints_the_flame_crown_for_a_top_tier_agent() {
    // The PIPELINE-level burn pin: a fable+ultra slot must come out of the
    // full render_floor pass with ember hair + flame pixels — a projection
    // or sim/paint hop silently dropping slot.model/effort fails HERE even
    // while the unit-level paint_character_at test stays green.
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
    // The painter's own constants — not re-hardcoded copies.
    let ember = crate::pixel_painter::FLAME_DEEP;
    let tip = crate::pixel_painter::FLAME_TIP;
    let count = |c| {
        (0..buf.height())
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| buf.get(x, y) == c)
            .count()
    };
    assert!(count(ember) > 0, "ember hair must survive the full pass");
    assert!(count(tip) > 0, "flame tips must survive the full pass");
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

    // A too-small buffer: no layout, `None`, no panic — the buffer is
    // resized+cleared but unpainted.
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

    // A real size: the layout comes back and the pass painted content
    // beyond the cleared background fill.
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

// ---- FloorSession -----------------------------------------------------

#[test]
fn floor_session_render_owns_the_dual_eviction() {
    // FloorSession::render runs BOTH halves of the dual eviction itself —
    // the render caches (motion/pose/frame) and the coffee cup — so a
    // painter can't skip one and leak per-agent state or teleport a
    // recurring agent.
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

    // `gone` is not in the scene → one render() must drop both entries.
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
    // The appliance-cue feed (#633): render() must record the sim's occupancy
    // observation in `last_occupied` — the set the shared `AudioObserver` reads
    // (via `FloorSession::audio_frame`) — so a windowed painter never re-runs the
    // sim. (`last_occupied` is a private field; this test is a child module of
    // `floor`, so it reads it directly.)
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
    // Frame-ACCURACY, not just "ever nonempty": walk the whole sim and
    // record the occupancy-set size each frame. The set must (a) go
    // nonempty (the agent reaches a waypoint, indices valid) AND (b) fall
    // back to empty on a LATER layoutable frame (the agent wanders off).
    // A sticky/accumulating `last_occupied` (the `.extend` mutant) is
    // monotone non-decreasing, so it can never produce the fall — this is
    // the anti-stick tooth the "ever nonempty" check lacked.
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
    // An unlayoutable size clears the stale set — a painter reading it
    // after a shrink must not replay the last big frame's occupancy.
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
    // The headless observation seam the sim/paint split (#450) prepared:
    // eviction + layout prologue + sim_step + the coffee/door epilogue,
    // with NO pixel buffer touched. A fresh agent's entry walk must
    // populate motion and the door-anim clamp, and the frame must carry
    // its pose.
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

    // Too small for any layout: None, never a panic.
    assert!(
        session
            .observe(&scene, &pack, 8, 8, FloorMeta::ground(), t)
            .is_none(),
        "an unlayoutable size observes nothing"
    );
}

#[test]
fn session_types_default_equals_new() {
    // Same convention pin as FloorCtx/LightingState above: Default and
    // new() must not diverge on a future field addition.
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
    // Prime the cache with one entry, so the assertion below distinguishes a
    // real reset from a no-op (a fresh session's cache is already empty).
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
    // Wiring-oracle: the AudioFrame the observer returns must equal the pure model
    // (stem_levels / select_track) applied to the SAME inputs the painters used to
    // assemble by hand — so the consolidation can't drift the composition.
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
            crate::audio::epoch_hours(now),
        ),
        "track must equal select_track(is_day_at(now), precip, epoch_hours(now))"
    );
}

#[test]
fn audio_observer_reprimes_on_floor_switch_so_the_new_floor_is_silent() {
    // Switching the VIEWED floor must reprime the cue tracker: the switch frame
    // primes silently (no volley for agents/appliances already there), then normal
    // edges resume. `primed_floor` tracks the latch.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let scene = make_scene(4, 16); // agents live on floor 0
    let printer = |i: usize| (i == 0 || i == 1).then_some(crate::layout::WaypointKind::Printer);
    let mut obs = AudioObserver::new();

    // Prime floor 0.
    let _ = obs.frame(&scene, &std::collections::HashSet::new(), printer, 0, now);
    assert_eq!(obs.primed_floor(), Some(0));

    // Switch to floor 1 with an appliance ALREADY occupied: the reprime makes the
    // switch frame silent (this would fire PrinterWhir without the reprime).
    let occ0: std::collections::HashSet<usize> = [0usize].into_iter().collect();
    let switch = obs.frame(&scene, &occ0, printer, 1, now);
    assert_eq!(obs.primed_floor(), Some(1));
    assert!(
        switch.events.is_empty(),
        "a floor switch reprimes silently — no cue volley for the new floor"
    );

    // Post-reprime the tracker still works: a NEWLY occupied printer fires.
    let occ01: std::collections::HashSet<usize> = [0usize, 1usize].into_iter().collect();
    let next = obs.frame(&scene, &occ01, printer, 1, now);
    assert!(
        next.events.contains(&crate::audio::OneShot::PrinterWhir),
        "after the reprime, a newly occupied printer still fires"
    );
}

#[test]
fn audio_observer_keeps_cue_edges_warm_so_delivery_resume_fires_no_volley() {
    // B (mute-gating): the painter calls frame() EVERY world-frame and gates only
    // DELIVERY. An agent arriving during a muted stretch is consumed by the still-
    // running observer, so when delivery resumes it must NOT re-fire a chime.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let empty = make_scene(0, 16);
    let one = make_scene(1, 16);
    let occ = std::collections::HashSet::new();
    let mut obs = AudioObserver::new();

    let _ = obs.frame(&empty, &occ, |_| None, 0, now); // prime (empty office)
                                                       // "muted" frame: the agent arrives; the painter still calls frame(), then
                                                       // DROPS the result. The chime fires here (and is discarded by the caller).
    let arrival = obs.frame(&one, &occ, |_| None, 0, now);
    assert!(
        arrival.events.contains(&crate::audio::OneShot::DoorChime),
        "an arrival chimes on the frame it happens"
    );
    // Delivery resumes: the SAME agent is still present → no new chime.
    let resumed = obs.frame(&one, &occ, |_| None, 0, now);
    assert!(
        !resumed.events.contains(&crate::audio::OneShot::DoorChime),
        "no volley on resume — the observer saw the agent while muted"
    );
}
