use super::*;
use pixtuoid_scene::layout::Point;

#[test]
fn offscreen_floor_freezes_and_resyncs_on_return() {
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let theme = pixtuoid_scene::theme::ALL_THEMES[0];
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Two-floor scene: a long-idle (wandering) agent on floor 0, plus a
    // filler on floor 1 so `num_floors` == 2.
    let cap = 16;
    let mut scene = SceneState::uniform(cap);
    let a = AgentId::from_transcript_path("/h/floor0.jsonl");
    let b = AgentId::from_transcript_path("/h/floor1.jsonl");
    scene
        .agents
        .insert(a, slot(a, 0, 0, t0 - Duration::from_secs(120)));
    scene.agents.insert(b, slot(b, 1, cap, t0));

    let term = Terminal::new(TestBackend::new(100, 40)).expect("test backend");
    let mut r = TuiRenderer::new(term, theme, vec![]);

    // Warm up floor 0 so agent A's MotionState initialises and wanders.
    let mut now = t0;
    for _ in 0..10 {
        r.render(&scene, &pack, now).expect("render");
        now += Duration::from_millis(33);
    }
    assert_eq!(r.current_floor(), 0);
    assert!(
        r.floor_motion(0).and_then(|m| m.get(&a)).is_some(),
        "floor-0 agent should have a MotionState after warm-up"
    );

    // Switch to floor 1 and let the transition settle.
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack, &mut now, 1);

    // Baseline: floor 0 is now off-screen.
    let frozen_at = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .map(|ms| ms.wander.last_advanced_at)
        .expect("floor-0 motion present");

    // ~30 s on floor 1 — floor 0 must NOT be advanced.
    for _ in 0..900 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack, now).expect("render");
    }
    let still_frozen = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .map(|ms| ms.wander.last_advanced_at)
        .expect("floor-0 motion present");
    assert_eq!(
        frozen_at, still_frozen,
        "off-screen floor 0 motion must stay frozen while floor 1 is visible"
    );

    // Switch back to floor 0.
    let back_at = now;
    r.navigate_floor(0, now);
    render_until_settled(&mut r, &scene, &pack, &mut now, 0);

    // RESYNC: the stale-resume must re-anchor the phase clock to ~now
    // (clean Seated start) instead of replaying ~30 s of backlogged cycles
    // one transition per frame. wander.phase_started_at would be far in the
    // past if it replayed.
    let ms = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .expect("floor-0 motion present");
    assert!(
        ms.wander.phase_started_at >= back_at,
        "floor-0 agent must resync its wander clock on return (got an anchor before the switch-back ⇒ replay)"
    );
}

// ===================================================================
// Floor navigation
// ===================================================================
#[test]
fn floor_transition_completes_and_lands() {
    let p = pack();
    let scene = two_floor_scene();
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &p, now).unwrap();
    assert_eq!(r.current_floor(), 0);

    r.navigate_floor(1, now);
    assert!(
        r.transition().is_some(),
        "navigation should begin a transition"
    );

    now += Duration::from_millis(450);
    r.render(&scene, &p, now).unwrap();
    assert!(r.transition().is_some(), "still transitioning mid-slide");
    assert!(
        r.cached_layout().is_none(),
        "layout is cleared during a transition"
    );

    now += Duration::from_millis(600); // total 1050ms > 900ms duration
    r.render(&scene, &p, now).unwrap();
    assert!(r.transition().is_none(), "transition complete");
    assert_eq!(r.current_floor(), 1, "landed on the target floor");
    assert!(
        r.cached_layout().is_some(),
        "layout recomputed after landing"
    );
}

#[test]
fn navigation_blocked_during_active_transition() {
    let cap = 16;
    let scene = scene_with(
        vec![
            idle("/b/0.jsonl", 0, t0()),
            slot(AgentId::from_transcript_path("/b/1.jsonl"), 1, cap, t0()),
            slot(
                AgentId::from_transcript_path("/b/2.jsonl"),
                2,
                2 * cap,
                t0(),
            ),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    r.navigate_floor(2, now); // must be ignored — a transition is in flight
    assert_eq!(
        r.transition().map(|t| t.to_floor),
        Some(1),
        "a second navigate during a transition is a no-op"
    );
}

#[test]
fn navigate_floor_clears_pinned_agent() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/pin/0.jsonl");
    let scene = scene_with(
        vec![
            slot(a, 0, 0, t0()),
            slot(AgentId::from_transcript_path("/pin/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_pinned_agent(Some(a));
    r.navigate_floor(1, now);
    assert!(r.pinned_agent().is_none(), "navigation unpins the agent");
}

#[test]
fn transition_cancelled_when_target_floor_disappears() {
    let cap = 16;
    let f1 = slot(AgentId::from_transcript_path("/c/1.jsonl"), 1, cap, t0());
    let mut scene = scene_with(vec![idle("/c/0.jsonl", 0, t0()), f1.clone()], cap);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    assert!(r.transition().is_some());

    // Floor-1 agent leaves ⇒ num_floors drops to 1 ⇒ transition target gone.
    scene.agents.remove(&f1.agent_id);
    now += Duration::from_millis(100);
    r.render(&scene, &pack(), now).unwrap();
    assert!(
        r.transition().is_none(),
        "transition to a vanished floor must cancel (no infinite slide)"
    );
    assert_eq!(r.current_floor(), 0);
}

#[test]
fn floor_buffers_grow_on_overflow() {
    let cap = 16;
    let mut r = build(100, 40, vec![]);
    let now = t0();
    let one = scene_with(vec![idle("/g/0.jsonl", 0, t0())], cap);
    r.render(&one, &pack(), now).unwrap();
    assert!(r.floor_buf(1).is_none(), "only one floor allocated");

    let two = scene_with(
        vec![
            idle("/g/0.jsonl", 0, t0()),
            slot(AgentId::from_transcript_path("/g/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    );
    r.render(&two, &pack(), now).unwrap();
    assert!(
        r.floor_buf(1).is_some(),
        "floor-1 buffer allocated after overflow"
    );
}

#[test]
fn per_floor_layout_seeds_differ() {
    let scene = two_floor_scene();
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let seed0 = r.current_floor_seed();
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    assert_ne!(
        seed0,
        r.current_floor_seed(),
        "each floor must use a distinct layout seed"
    );
}

// `invalidate_routes` drops every floor's A* path cache. Its only production
// caller is the codecov-ignored resize handler in tui/mod.rs, so the loop body
// is never exercised under coverage. Warm up a wandering agent so the router
// populates its (from,to) path cache, then assert invalidate empties it.
#[test]
fn invalidate_routes_clears_every_floor_router_cache() {
    // Long-idle agents wander; a WalkingOut/WalkingBack leg drives
    // route_walking_pose → AStarRouter::route, populating the (from,to) cache.
    // Several agents + a multi-second timeline guarantee at least one walk leg
    // (a fresh agent bootstraps Seated@now, then sits seated_dwell_ms 15-30s
    // before its first walk-out).
    let agents = (0..8)
        .map(|i| {
            idle(
                &format!("/inv/{i}.jsonl"),
                i,
                t0() - Duration::from_secs(120),
            )
        })
        .collect();
    let scene = scene_with(agents, 16);
    let mut r = build(120, 60, vec![]);
    let mut now = t0();
    // Advance up to ~60s of render time; bail out as soon as the router caches.
    for _ in 0..120 {
        r.render(&scene, &pack(), now).expect("render");
        if !r.floors[0].ctx.router.is_empty() {
            break;
        }
        now += Duration::from_millis(500);
    }
    assert!(
        !r.floors[0].ctx.router.is_empty(),
        "a warmed-up wandering agent should have populated the A* path cache"
    );

    r.invalidate_routes();
    assert!(
        r.floors[0].ctx.router.is_empty(),
        "invalidate_routes must drop every floor's cached A* paths"
    );
    assert_eq!(
        r.floors[0].ctx.router.len(),
        0,
        "cache is empty after invalidate"
    );
}

// The transition path's no-layout guard (`floor::render_floor`'s
// `compute_with_seed(...)?`) — the transition twin of the normal-path
// Ok(None) branch. A 30-col
// terminal passes `render_transition`'s 20×12 scene gate (scene_rect 30×39) but
// buf_w=30 < the office MIN_W=34, so compute_with_seed returns None and the
// floor paints nothing (no render_to_rgb_buffer, no coffee carriers). Mutating
// away the None-guard would paint over the bg-fallback fill (or panic on the
// missing layout), flipping this assertion.
#[test]
fn transition_at_narrow_terminal_paints_no_agents_no_panic() {
    let cap = 16;
    // A floor-0 agent (coffee carrier would-be) + a floor-1 occupant so
    // num_floors==2 and navigate_floor(1) has a destination.
    let scene = scene_with(
        vec![
            idle("/narrow/0.jsonl", 0, t0() - Duration::from_secs(120)),
            slot(
                AgentId::from_transcript_path("/narrow/1.jsonl"),
                1,
                cap,
                t0(),
            ),
        ],
        cap,
    );
    // 30 cols: scene_rect 30×39 passes the 20×12 transition gate; buf_w=30<34
    // fails compute_with_seed's office minimum.
    let mut r = TuiRenderer::new(
        Terminal::new(TestBackend::new(30, 40)).expect("test backend"),
        normal_theme(),
        vec![],
    );
    let mut now = t0();
    r.render(&scene, &pack(), now).expect("render at 30 cols");
    r.navigate_floor(1, now);
    assert!(r.transition().is_some(), "navigation begins a transition");
    // Render ONE in-flight transition frame (slide still active).
    now += Duration::from_millis(33);
    r.render(&scene, &pack(), now)
        .expect("transition render at a narrow terminal must not panic");
    assert!(
        r.transition().is_some(),
        "the slide is still active this frame"
    );

    // The from-floor buffer was ensure_size'd to the theme's bg-fallback, then
    // render_floor returned early (no layout) ⇒ it stays uniform.
    let bg = normal_theme().surface.bg_fallback;
    let from = r.floor_buf(0).expect("floor-0 buffer allocated");
    let non_bg = (0..from.height())
        .flat_map(|y| (0..from.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| from.get(x, y) != bg)
        .count();
    assert_eq!(
        non_bg, 0,
        "compute failed at 30 cols ⇒ no scene/agents painted, buffer stays bg-fallback ({non_bg} stray pixels)"
    );
    // No pantry trip could have completed against a None layout.
    assert!(
        !r.coffee_contains(AgentId::from_transcript_path("/narrow/0.jsonl")),
        "a skipped transition floor records no coffee carriers"
    );
}

// ===================================================================
// Overlays during a floor transition (transition render path)
// ===================================================================

#[test]
fn footer_shows_source_death_warning() {
    let scene = scene_with(vec![idle("/f/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 44, vec![]);
    r.set_source_warning(Some(
        "claude-code source died — its agents are frozen; restart pixtuoid (see log)".into(),
    ));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("source died") && text.contains("restart pixtuoid"),
        "the footer must surface a dead source (#157); footer row:\n{}",
        text.lines().last().unwrap_or("")
    );
    // And it clears once healthy again (e.g. after a future restart-in-place).
    r.set_source_warning(None);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        !text.contains("source died"),
        "footer returns to stats when no source is dead"
    );
}

#[test]
fn source_death_warning_survives_floor_transition() {
    let scene = two_floor_scene();
    let mut r = build(120, 44, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_source_warning(Some(
        "claude-code source died — its agents are frozen; restart pixtuoid (see log)".into(),
    ));
    r.navigate_floor(1, now);
    now += Duration::from_millis(200); // mid-transition
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some(), "still mid-transition");
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("source died"),
        "the warning must not vanish during the ~400ms floor slide"
    );
}

#[test]
fn version_popup_active_during_floor_transition() {
    let scene = two_floor_scene();
    let mut r = build(120, 44, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_version_popup(true, now);
    r.navigate_floor(1, now);
    now += Duration::from_millis(200); // mid-transition
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some(), "still mid-transition");
    assert!(
        r.last_popup_scale() > 0.0,
        "version popup must keep animating through a floor transition"
    );
}

#[test]
fn help_overlay_renders_during_floor_transition() {
    let scene = two_floor_scene();
    let mut r = build(120, 44, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_help_open(true);
    r.navigate_floor(1, now);
    now += Duration::from_millis(200);
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some());
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("theme") || text.contains("Keyboard") || text.contains("help"),
        "help overlay must paint over a floor transition"
    );
}

// ===================================================================
// tui_renderer: render_transition too-small bail (CG9) + getters (CG10)
// ===================================================================

#[test]
fn transition_on_too_small_terminal_clears_state_and_lands() {
    // Two-floor scene on a sub-20×12 terminal: starting a transition hits the
    // render_transition too-small bail → cached layout / pet / popup cleared.
    let scene = two_floor_scene();
    let mut r = build(18, 10, vec![PetKind::Cat]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    r.render(&scene, &pack(), now + Duration::from_millis(100))
        .expect("transition render on a tiny terminal must not panic");
    assert!(r.cached_layout().is_none());
    assert!(r.cached_pet_pos().is_none());
    assert_eq!(r.last_popup_scale(), 0.0);
    // The gate now LANDS the transition instead of leaving it live: render_transition
    // returns before render_floor/ensure_size, so the floor buffer's size signature
    // never changes and the event loop's resize detector can't fire cancel_transition
    // — the slide would otherwise stay live hitting the no-draw path for its whole
    // ~400 ms timer, freezing a stale frame. Landing it drops back to draw_scene's
    // footer-only path, which shares the same threshold behavior.
    assert!(
        r.transition().is_none(),
        "the too-small gate should land (cancel) the stuck transition"
    );
    assert_eq!(r.current_floor(), 1, "landed on the destination floor");
}

#[test]
fn debug_walkable_getter_reflects_setter() {
    let mut r = build(100, 40, vec![]);
    assert!(!r.debug_walkable());
    r.set_debug_walkable(true);
    assert!(r.debug_walkable());
    r.set_debug_walkable(false);
    assert!(!r.debug_walkable());
}

#[test]
fn already_expired_active_pet_clears_on_render() {
    // set_active_pet with a PetState whose petted_at is far in the past → the
    // render-time auto-expire drops it.
    let scene = scene_with(vec![active("/exp/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.set_active_pet(Some(PetState {
        petted_at: t0() - Duration::from_secs(3600), // long expired
        pet_pos: Point { x: 10, y: 10 },
        kind: PetKind::Cat,
        floor_idx: 0,
    }));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        r.active_pet_ref().is_none(),
        "an already-expired pet state must be cleared on render"
    );
}

#[test]
fn current_floor_clamps_when_floor_count_drops() {
    // Land on floor 1, then re-render a scene with only floor 0 ⇒ current_floor
    // must clamp back into range (the nf-shrink clamp).
    let cap = 16;
    let two = two_floor_scene();
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&two, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &two, &pack(), &mut now, 1);
    assert_eq!(r.current_floor(), 1);
    // Drop to a single-floor scene.
    let one = scene_with(vec![idle("/clamp/0.jsonl", 0, t0())], cap);
    r.render(&one, &pack(), now).unwrap();
    assert_eq!(
        r.current_floor(),
        0,
        "current_floor clamps when floors shrink"
    );
}

#[test]
fn theme_picker_renders_during_floor_transition() {
    // Opening the theme picker mid-transition exercises the transition-path
    // theme_picker paint arm.
    let scene = two_floor_scene();
    let mut r = build(140, 48, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_theme_picker(Some(0));
    r.navigate_floor(1, now);
    now += Duration::from_millis(200);
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some(), "still mid-transition");
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("cyberpunk") || text.contains("normal"),
        "theme picker must paint over a floor transition; frame:\n{text}"
    );
}
