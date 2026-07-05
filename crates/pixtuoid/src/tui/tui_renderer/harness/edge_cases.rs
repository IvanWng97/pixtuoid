use super::*;

// ===================================================================
// Graceful degradation
// ===================================================================

#[test]
fn too_small_terminal_returns_no_layout_no_panic() {
    let scene = scene_with(vec![idle("/sm/0.jsonl", 0, t0())], 16);
    let mut r = build(15, 8, vec![]); // below the 20×12 scene minimum
    r.render(&scene, &pack(), t0())
        .expect("render must not panic");
    assert!(
        r.cached_layout().is_none(),
        "a too-small terminal yields no layout"
    );
}

// Regression: the hover/disambiguation label sliced session_id by BYTE
// (`&session_id[..4]`), guarded only by a byte-length check, so byte 4 landing
// inside a multi-byte UTF-8 codepoint panicked the per-frame render loop. A
// Reasonix session_id IS the raw cwd path, so two same-labeled agents under a
// non-ASCII project dir hit it. Must render without panicking.
#[test]
fn colliding_labels_with_multibyte_session_ids_do_not_panic() {
    let mut scene = SceneState::uniform(16);
    // `/naïveté/app`: `ï` occupies bytes 3..5, so `&session_id[..4]` would split
    // it. Both agents share a label ⇒ the disambiguation suffix fires.
    let mut mk = |id: &str, desk: usize| {
        let a = AgentId::from_transcript_path(id);
        let mut s = slot(a, 0, desk, t0());
        s.label = "rx\u{00b7}proj".into();
        s.session_id = Arc::from("/na\u{00ef}vet\u{00e9}/app");
        scene.agents.insert(a, s);
    };
    mk("/mb/0.jsonl", 0);
    mk("/mb/1.jsonl", 1);
    let mut r = build(120, 40, vec![]);
    r.render(&scene, &pack(), t0())
        .expect("render must not panic on a multi-byte session_id");
}

// Regression: render() set last_popup_scale unconditionally, even on an
// Ok(None) (footer-only) frame where compute_with_seed fails at a
// small-but-not-tiny size — leaving a stale popup-click hit-box the mouse
// handler would honor though nothing was painted.
#[test]
fn no_layout_frame_zeroes_the_popup_hit_box() {
    // 100x16 → scene_rect 100x15 passes render()'s 20x12 gate, but buf_h=30 is
    // below compute_with_seed's office minimum → draw_scene returns Ok(None).
    let scene = scene_with(vec![idle("/nl/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 16, vec![]);
    r.set_version_popup(true, t0());
    let t = t0() + Duration::from_millis(150); // mid-entrance ⇒ scale > 0
    assert!(
        r.version_popup_scale(t) > 0.0,
        "the popup is animating this frame"
    );
    r.render(&scene, &pack(), t).expect("render");
    assert!(
        r.cached_layout().is_none(),
        "no layout produced at this size"
    );
    assert_eq!(
        r.last_popup_scale(),
        0.0,
        "a footer-only frame paints no popup → no stale hit-box"
    );
}

// Regression: per-agent MotionState was evicted only on the CURRENT floor, so an
// agent that exited while a different floor was visible leaked its walk-path Vec
// on its own (non-current) floor until that floor was next navigated to. The
// eviction now lives in `evict_missing` (called by the event loop with the live
// snapshot before every render), which this drives like the production loop.
#[test]
fn departed_agent_motion_is_evicted_on_a_non_current_floor() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/ev/floor0.jsonl");
    let b = AgentId::from_transcript_path("/ev/floor1.jsonl");
    // Long-idle so both wander and acquire a MotionState.
    let scene = scene_with(
        vec![
            slot(a, 0, 0, t0() - Duration::from_secs(120)),
            slot(b, 1, cap, t0() - Duration::from_secs(120)),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let mut now = t0();

    // Warm up floor 0, then visit floor 1 so agent B gets a MotionState there.
    for _ in 0..10 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    for _ in 0..10 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    assert!(
        r.floor_motion(1).and_then(|m| m.get(&b)).is_some(),
        "floor-1 agent B should have a MotionState after visiting floor 1"
    );

    // Back to floor 0 (B's floor is now NON-current), then B exits the scene.
    r.navigate_floor(0, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 0);
    let scene_without_b = scene_with(vec![slot(a, 0, 0, t0() - Duration::from_secs(120))], cap);
    now += Duration::from_millis(33);
    r.evict_missing(&scene_without_b);
    r.render(&scene_without_b, &pack(), now).expect("render");

    assert_eq!(
        r.floor_motion(1).map(|m| m.contains_key(&b)),
        Some(false),
        "a departed agent's MotionState must be evicted even on a non-current floor"
    );
}

// Regression: PoseHistory had NO eviction anywhere — one `(Point, SystemTime)`
// per AgentId ever rendered lived for the process lifetime, on every floor —
// and motion eviction lived only inside the normal render path (skipped on
// transition frames). Both belong in `TuiRenderer::evict_missing`, the seam the
// event loop calls with the live snapshot before every render, next to the
// frame-cache eviction — across EVERY floor.
#[test]
fn evict_missing_drops_history_and_motion_on_every_floor() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/ev2/floor0.jsonl");
    let b = AgentId::from_transcript_path("/ev2/floor1.jsonl");
    // Fresh agents: the entry walk populates BOTH history (per-frame walker
    // position records) and motion (entry profile) on their floors.
    let scene = scene_with(vec![slot(a, 0, 0, t0()), slot(b, 1, cap, t0())], cap);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();

    for _ in 0..5 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).expect("render");
    }
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    for _ in 0..5 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).expect("render");
    }
    assert_eq!(
        r.floor_history(0).map(|h| h.contains(a)),
        Some(true),
        "floor-0 history should hold agent A after its entry frames"
    );
    assert_eq!(
        r.floor_history(1).map(|h| h.contains(b)),
        Some(true),
        "floor-1 history should hold agent B after its entry frames"
    );
    assert!(
        r.floor_motion(1).and_then(|m| m.get(&b)).is_some(),
        "floor-1 motion should hold agent B"
    );

    // Both agents leave the scene; the loop hands the new snapshot to
    // evict_missing before the next render.
    let empty = SceneState::uniform(cap);
    r.evict_missing(&empty);

    for floor in 0..2 {
        assert_eq!(
            r.floor_history(floor)
                .map(|h| h.contains(a) || h.contains(b)),
            Some(false),
            "departed agents' PoseHistory must be evicted on floor {floor}"
        );
        assert_eq!(
            r.floor_motion(floor)
                .map(|m| m.contains_key(&a) || m.contains_key(&b)),
            Some(false),
            "departed agents' MotionState must be evicted on floor {floor}"
        );
    }
}

// Regression: an in-flight floor transition used to leave `last_pet_pos` stale
// from the previous normal frame, so the mouse handler could "pet" a ghost at
// last frame's location mid-slide. The transition path must clear it.
#[test]
fn floor_transition_clears_stale_pet_position() {
    let cap = 16;
    let mut scene = SceneState::uniform(cap);
    let a = AgentId::from_transcript_path("/pettrans/f0.jsonl");
    let b = AgentId::from_transcript_path("/pettrans/f1.jsonl");
    scene.agents.insert(a, slot(a, 0, 0, t0()));
    scene.agents.insert(b, slot(b, 1, cap, t0())); // floor 1 ⇒ navigate_floor(1) valid

    let mut r = build(100, 40, vec![PetKind::Cat]);
    let mut now = t0();
    for _ in 0..3 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    assert!(
        r.cached_pet_pos().is_some(),
        "a pet should be drawn on the normal floor-0 frame"
    );

    r.navigate_floor(1, now);
    r.render(&scene, &pack(), now).expect("render"); // single in-flight transition frame
    assert!(
        r.cached_pet_pos().is_none(),
        "an in-flight floor transition must clear the stale pet position"
    );
}

// ===================================================================
// renderer.rs: Layout::compute None bail (CG7)
// ===================================================================

// A terminal that PASSES the 20×12 scene-rect gate but is too small for
// Layout::compute (buf_w < MIN_W) takes draw_scene's compute-None bail → no
// cached layout, footer-only, no error.
#[test]
fn layout_compute_none_bails_to_footer_only() {
    let scene = scene_with(vec![idle("/lc/0.jsonl", 0, t0())], 16);
    // scene_rect 28×39: width 28 ≥ 20 (passes gate), buf_w 28 < MIN_W → compute
    // returns None, hitting the second bail arm.
    let mut r = build(28, 40, vec![]);
    r.render(&scene, &pack(), t0())
        .expect("render must not error on the compute-None bail");
    assert!(
        r.cached_layout().is_none(),
        "a layout that fails compute yields no cached layout"
    );
}
