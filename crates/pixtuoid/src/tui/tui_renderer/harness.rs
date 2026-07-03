//! End-to-end headless floor-switch harness. Drives the real
//! `TuiRenderer` (via ratatui `TestBackend`) through the actual
//! `navigate_floor` → transition → `render` path — the production wiring
//! that the unit-level `advance_wander` tests can't reach — and asserts an
//! off-screen floor freezes while hidden and resyncs (no replay) on return.
use super::*;
use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, SceneState, ToolKind};
use pixtuoid_core::AgentId;
use pixtuoid_scene::layout::Point;
use pixtuoid_scene::pet::PetKind;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn slot(id: AgentId, floor_idx: usize, desk_index: usize, started: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: id,
        source: Arc::from("cc"),
        session_id: Arc::from("s"),
        cwd: Arc::from(Path::new("/repo")),
        label: "a".into(),
        state: ActivityState::Idle,
        state_started_at: started,
        created_at: started,
        last_event_at: started,
        exiting_at: None,
        pending_idle_at: None,
        desk_index: GlobalDeskIndex(desk_index),
        floor_idx,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
    }
}

fn render_until_settled<B: Backend<Error: Send + Sync + 'static>>(
    r: &mut TuiRenderer<B>,
    scene: &SceneState,
    pack: &Pack,
    now: &mut SystemTime,
    target_floor: usize,
) {
    // Drive frames until the transition completes and we're on the target.
    for _ in 0..60 {
        *now += Duration::from_millis(33);
        r.render(scene, pack, *now).expect("render");
        if r.current_floor() == target_floor && r.transition().is_none() {
            return;
        }
    }
    panic!("floor transition to {target_floor} did not settle");
}

// ---- shared helpers -------------------------------------------------

fn pack() -> Pack {
    pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack")
}
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}
fn normal_theme() -> &'static pixtuoid_scene::theme::Theme {
    pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme")
}
fn dark_theme() -> &'static pixtuoid_scene::theme::Theme {
    pixtuoid_scene::theme::theme_by_name("cyberpunk").expect("cyberpunk theme")
}
/// Build a renderer with the given pet KINDS, each using its default name.
fn build(cols: u16, rows: u16, kinds: Vec<PetKind>) -> TuiRenderer<TestBackend> {
    build_pets(
        cols,
        rows,
        kinds
            .into_iter()
            .map(pixtuoid_scene::pet::Pet::defaulted)
            .collect(),
    )
}
/// Build a renderer with fully-specified pets (kind + custom name).
fn build_pets(
    cols: u16,
    rows: u16,
    pets: Vec<pixtuoid_scene::pet::Pet>,
) -> TuiRenderer<TestBackend> {
    TuiRenderer::new(
        Terminal::new(TestBackend::new(cols, rows)).expect("test backend"),
        normal_theme(),
        pets,
    )
}
/// Idle agent on floor 0 at desk `desk`.
fn idle(id: &str, desk: usize, started: SystemTime) -> AgentSlot {
    slot(AgentId::from_transcript_path(id), 0, desk, started)
}
/// Active (typing) agent with a tool `detail`.
fn active(id: &str, desk: usize, detail: &str, started: SystemTime) -> AgentSlot {
    let mut s = idle(id, desk, started);
    s.state = ActivityState::Active {
        tool_use_id: Some(Arc::from("t")),
        detail: Some(Arc::from(detail)),
        kind: ToolKind::from_display(detail),
    };
    s.last_event_at = started;
    s
}
fn scene_with(agents: Vec<AgentSlot>, cap: usize) -> SceneState {
    let mut s = SceneState::uniform(cap);
    for a in agents {
        s.agents.insert(a.agent_id, a);
    }
    s
}
/// Flatten the ratatui frame into one newline-joined string for substring
/// assertions on rendered text (footer, tooltips, overlays).
fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}
fn lum(c: pixtuoid_core::sprite::Rgb) -> f32 {
    0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32
}
/// Average luminance over a rectangle of the RGB buffer (clamped to bounds).
fn avg_lum(buf: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> f32 {
    let mut sum = 0.0;
    let mut n = 0u32;
    for y in y0..(y0 + h).min(buf.height()) {
        for x in x0..(x0 + w).min(buf.width()) {
            sum += lum(buf.get(x, y));
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}
/// Sum of absolute per-channel differences over a rectangle between two
/// buffers of equal size — a robust "did this region change" metric.
fn region_diff(a: &RgbBuffer, b: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> u64 {
    let mut d = 0u64;
    for y in y0..(y0 + h).min(a.height()).min(b.height()) {
        for x in x0..(x0 + w).min(a.width()).min(b.width()) {
            let (p, q) = (a.get(x, y), b.get(x, y));
            d += (p.r as i32 - q.r as i32).unsigned_abs() as u64
                + (p.g as i32 - q.g as i32).unsigned_abs() as u64
                + (p.b as i32 - q.b as i32).unsigned_abs() as u64;
        }
    }
    d
}

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

fn two_floor_scene() -> SceneState {
    let cap = 16;
    scene_with(
        vec![
            idle("/n/0.jsonl", 0, t0() - Duration::from_secs(120)),
            slot(AgentId::from_transcript_path("/n/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    )
}

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
// Theme / palette
// ===================================================================

#[test]
fn theme_switch_recolors_floor() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();
    r.set_theme(dark_theme());
    r.render(&scene, &pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert!(
        d > 5_000,
        "switching to a different theme must recolor the floor (diff={d})"
    );
}

// The `ptr::eq` guard in `set_theme`: calling it with the SAME &'static theme
// the renderer already holds must NOT flush the frame cache, so the next frame
// is byte-identical. This is the false branch twin of `theme_switch_recolors_floor`
// (the true branch); together they pin both arms of the guard — a mutant that
// always-flushes fails here, one that never-flushes fails there.
#[test]
fn set_theme_with_same_theme_is_a_noop() {
    let scene = scene_with(vec![idle("/t/same.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]); // built with normal_theme()
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();
    // The SAME &'static pointer the renderer holds ⇒ ptr::eq is true ⇒ skip.
    r.set_theme(normal_theme());
    r.render(&scene, &pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert_eq!(
        d, 0,
        "re-setting the identical theme must not recolor (cache not flushed), diff={d}"
    );
}

// ===================================================================
// Debug overlay (the `w` toggle)
// ===================================================================

#[test]
fn walkable_debug_toggle_tints_blocked_pixels_and_is_reversible() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 60, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();

    // A known-blocked pixel from the live mask, below the busy top wall band.
    let layout = r.cached_layout().expect("layout").clone();
    let (bx, by) = (0..layout.buf_h)
        .flat_map(|y| (0..layout.buf_w).map(move |x| (x, y)))
        .find(|&(x, y)| y > layout.top_margin + 4 && !layout.is_walkable(x, y))
        .expect("some blocked cell below the wall band");

    // Toggle ON → the overlay reddens the blocked cell + changes the frame.
    r.set_debug_walkable(true);
    r.render(&scene, &pack(), now).unwrap();
    let on = r.buf().clone();
    // The mask layer blends blocked cells toward the BLOCKED tint (220,60,60),
    // so the cell must move CLOSER to that red than it was (a warm cell's red
    // channel barely rises, but green/blue drop — distance is the robust check).
    let to_red = |c: pixtuoid_core::sprite::Rgb| {
        (c.r as i32 - 220).abs() + (c.g as i32 - 60).abs() + (c.b as i32 - 60).abs()
    };
    assert!(
        to_red(on.get(bx, by)) < to_red(before.get(bx, by)),
        "debug overlay must tint a blocked cell toward red (was {:?}, now {:?})",
        before.get(bx, by),
        on.get(bx, by),
    );
    let on_diff = region_diff(&before, &on, 0, 0, before.width(), before.height());
    assert!(
        on_diff > 1_000,
        "the debug layer must visibly change the frame"
    );

    // Toggle OFF → the scene returns to the un-overlaid frame (additive layer).
    r.set_debug_walkable(false);
    r.render(&scene, &pack(), now).unwrap();
    let off_diff = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert!(
        off_diff < 200,
        "toggling the debug layer off must restore the scene (diff={off_diff})"
    );
}

// ===================================================================
// Lighting
// ===================================================================

// NOTE: the *visible* empty-floor darkening is gated on `look.darkness`
// (time-of-day via `chrono::Local`), so it only manifests at night and is
// timezone-dependent — not robustly assertable through render headlessly.
// The fade math itself is covered by the `LightingState` unit tests in
// floor.rs. Here we only guard the time-independent invariant: an OCCUPIED
// floor must not fade.
#[test]
fn occupied_floor_stays_lit() {
    // A present agent keeps the floor lit (no fade).
    let scene = scene_with(vec![active("/lit/0.jsonl", 0, "Edit x", t0())], 16);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    now += Duration::from_millis(2000);
    r.render(&scene, &pack(), now).unwrap();
    let early = avg_lum(r.buf(), 0, 0, r.buf().width(), r.buf().height());
    for _ in 0..700 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).unwrap();
    }
    let late = avg_lum(r.buf(), 0, 0, r.buf().width(), r.buf().height());
    assert!(
        late > early * 0.9,
        "occupied floor must stay lit (early={early:.1}, late={late:.1})"
    );
}

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
// Coffee state
// ===================================================================

#[test]
fn coffee_state_evicted_when_agent_leaves_scene() {
    let id = AgentId::from_transcript_path("/cof/leave.jsonl");
    let scene = scene_with(vec![slot(id, 0, 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.inject_coffee(id, t0());
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.coffee_contains(id));
    // Agent gone from the scene ⇒ next render evicts its coffee state.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .unwrap();
    assert!(
        !r.coffee_contains(id),
        "coffee state must be evicted when the agent leaves (no leak)"
    );
}

#[test]
fn coffee_persists_through_floor_transition() {
    // Regression: the old render_transition_floor discarded its
    // render_to_rgb_buffer result (`let _ =`), so a coffee carrier first
    // DETECTED during a floor slide was never persisted → the cup never
    // landed. The transition path now records carriers inside the shared
    // `floor::render_floor` seam (the normal path persists via
    // DrawCtx.new_coffee_carriers).
    let p = pack();
    let step = Duration::from_millis(500);
    let cap = 16;
    // Several floor-0 wanderers (the pantry is 1 of ~10 waypoints, so one
    // agent reaches it far sooner than any single one would) + a floor-1
    // occupant so navigate_floor(1) has a destination.
    let n_f0 = 10usize;
    let mut agents: Vec<_> = (0..n_f0)
        .map(|i| {
            idle(
                &format!("/cof/f0_{i}.jsonl"),
                i,
                t0() - Duration::from_secs(120),
            )
        })
        .collect();
    agents.push(slot(
        AgentId::from_transcript_path("/cof/f1.jsonl"),
        1,
        cap,
        t0(),
    ));
    let scene = scene_with(agents, cap);
    let f0_ids: Vec<AgentId> = (0..n_f0)
        .map(|i| AgentId::from_transcript_path(&format!("/cof/f0_{i}.jsonl")))
        .collect();

    // Pass 1 (scratch): find the first frame where the NORMAL render path
    // detects ANY floor-0 wanderer walking back from the pantry, and which one.
    let mut scratch = build(100, 40, vec![]);
    let mut now = t0();
    scratch.render(&scene, &p, now).unwrap();
    let mut hit = None;
    'outer: for _ in 0..400 {
        now += step;
        scratch.render(&scene, &p, now).unwrap();
        for &id in &f0_ids {
            if scratch.coffee_contains(id) {
                hit = Some((id, now));
                break 'outer;
            }
        }
    }
    let (agent, detect_at) = hit.expect("a floor-0 wanderer should fetch coffee while wandering");

    // Pass 2 (real): advance to one step BEFORE detection (no coffee yet),
    // begin a transition, then render AT detect_at — so the carrier is first
    // detected DURING the slide (gap ≤ step < the 900ms transition window, and
    // < the wander stale-resume trigger, so the timeline matches the scratch).
    let mut r = build(100, 40, vec![]);
    let mut t = t0();
    r.render(&scene, &p, t).unwrap();
    while t + step < detect_at {
        t += step;
        r.render(&scene, &p, t).unwrap();
    }
    assert!(
        !r.coffee_contains(agent),
        "agent must not yet hold coffee before the transition"
    );
    r.navigate_floor(1, t);
    assert!(r.transition().is_some(), "navigation begins a transition");
    r.render(&scene, &p, detect_at).unwrap();
    assert!(
        r.coffee_contains(agent),
        "a coffee run completing mid-transition must persist (regression: \
         render_transition_floor dropped new_coffee_carriers)"
    );
}

#[test]
fn injected_coffee_changes_desk_render() {
    // Compare two renders that differ ONLY by coffee state (same scene,
    // same final timestamp) so the diff is attributable to the coffee cup +
    // steam, not elapsed-time animation.
    let id = AgentId::from_transcript_path("/cof/steam.jsonl");
    let scene = scene_with(
        vec![idle("/cof/steam.jsonl", 0, t0() - Duration::from_secs(30))],
        16,
    );
    let t1 = t0() + Duration::from_millis(33);

    let mut base = build(100, 40, vec![]);
    base.render(&scene, &pack(), t0()).unwrap();
    base.render(&scene, &pack(), t1).unwrap();
    let baseline = base.buf().clone();
    let desk = base.cached_layout().expect("layout").home_desks[0];

    let mut r = build(100, 40, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    r.inject_coffee(id, t0()); // fresh fetch ⇒ within steam window
    r.render(&scene, &pack(), t1).unwrap();

    let d = region_diff(
        &baseline,
        r.buf(),
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(6),
        18,
        14,
    );
    assert!(
        d > 0,
        "coffee state should alter the desk render (cup + steam)"
    );
}

// ===================================================================
// Pets
// ===================================================================

#[test]
fn no_pet_when_pets_disabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![]); // no pets
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_none(), "no pet when none enabled");
}

#[test]
fn pet_present_when_enabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_some(), "a cat should be placed");
}

#[test]
fn pet_position_varies_over_its_cycle() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    let mut seen = std::collections::HashSet::new();
    for i in 0..5 {
        let now = t0() + Duration::from_secs(i * 10);
        r.render(&scene, &pack(), now).unwrap();
        if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
            seen.insert((pos.x, pos.y, anim));
        }
    }
    assert!(
        seen.len() >= 2,
        "pet should move/animate across its 40s cycle, saw {} distinct states",
        seen.len()
    );
}

#[test]
fn petting_freezes_pet_position() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, kind, .. } = r.cached_pet_pos().expect("pet placed");
    r.set_active_pet(Some(PetState {
        petted_at: t0(),
        pet_pos: pos,
        kind,
        floor_idx: 0,
    }));
    r.render(&scene, &pack(), t0() + Duration::from_millis(500))
        .unwrap();
    let PetFrame { pos: pos2, .. } = r.cached_pet_pos().expect("pet still placed");
    assert_eq!(pos, pos2, "a petted pet holds its position");
}

#[test]
fn pet_walk_is_frame_stable() {
    // Same `now` rendered by two independent renderers must yield the same pet
    // position — proves A* on (static mask + empty overlay) is deterministic
    // (no per-frame flash).
    let scene = scene_with(vec![active("/pstab/0.jsonl", 0, "Edit", t0())], 16);
    let now = t0() + Duration::from_millis(5_000); // mid walk-phase of cycle 0
    let mut r1 = build(160, 80, vec![PetKind::Cat]);
    let mut r2 = build(160, 80, vec![PetKind::Cat]);
    r1.render(&scene, &pack(), now).unwrap();
    r2.render(&scene, &pack(), now).unwrap();
    assert_eq!(
        r1.cached_pet_pos().map(|f| (f.pos.x, f.pos.y)),
        r2.cached_pet_pos().map(|f| (f.pos.x, f.pos.y)),
        "identical `now` must give identical pet position (no flash)"
    );
}

#[test]
fn pet_walk_never_clips_through_furniture() {
    // Across 4 cycles (many prev/dest pairs) × the whole 35% walk phase, every
    // walking frame must land on a walkable cell — i.e. routed around furniture.
    let scene = scene_with(vec![active("/pwalk/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout after prime").clone();
    for cycle in 0u64..4 {
        for step in 0..35u64 {
            let now = t0() + Duration::from_millis(cycle * 40_000 + step * 400);
            r.render(&scene, &pack(), now).unwrap();
            if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
                if anim == PetKind::Cat.walk_anim() {
                    // Coarse-cell walkable = the predicate A* itself guarantees
                    // (same grid every agent sprite rides). Per-pixel is_walkable
                    // is stricter than the router delivers (pad band / diagonal
                    // corner-graze) and would hold the pet to a higher bar than
                    // the agents.
                    assert!(
                        pixtuoid_scene::pathfind::point_in_walkable_cell(&layout.walkable, pos),
                        "walking pet at ({},{}) is in a blocked routing cell (cycle={cycle} step={step})",
                        pos.x,
                        pos.y
                    );
                }
            }
        }
    }
}

#[test]
fn pet_rest_pos_is_walkable() {
    let scene = scene_with(vec![active("/prest/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout after prime").clone();
    for cycle in 0u64..4 {
        for step in 0..10u64 {
            let now = t0() + Duration::from_millis(cycle * 40_000 + 14_200 + step * 2_600);
            r.render(&scene, &pack(), now).unwrap();
            if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
                if anim != PetKind::Cat.walk_anim() {
                    // Rest pose is a snapped cell center, so it should satisfy the
                    // stronger per-pixel check — assert that directly.
                    assert!(
                        layout.walkable.is_walkable(pos.x, pos.y),
                        "resting pet at ({},{}) is on a blocked cell (cycle={cycle} step={step})",
                        pos.x,
                        pos.y
                    );
                }
            }
        }
    }
}

#[test]
fn pet_leg_boundary_no_pop() {
    // The snapped rest anchor == the next leg's snapped walk-start anchor, so
    // the pet must not teleport across the 40s leg boundary.
    let scene = scene_with(vec![active("/pbnd/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0() + Duration::from_millis(39_600))
        .unwrap();
    let before = r.cached_pet_pos().map(|f| (f.pos.x, f.pos.y));
    r.render(&scene, &pack(), t0() + Duration::from_millis(40_040))
        .unwrap();
    let after = r.cached_pet_pos().map(|f| (f.pos.x, f.pos.y));
    if let (Some((x0, y0)), Some((x1, y1))) = (before, after) {
        let gap = (x0 as i32 - x1 as i32).unsigned_abs() + (y0 as i32 - y1 as i32).unsigned_abs();
        assert!(
            gap <= 16,
            "pet leg boundary teleports (gap={gap}px, ({x0},{y0})→({x1},{y1}))"
        );
    }
}

// ===================================================================
// Version popup
// ===================================================================

#[test]
fn version_popup_entrance_reaches_full_scale() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let s = r.version_popup_scale(t0() + Duration::from_millis(250));
    assert!(s > 0.99, "entrance eases to ~1.0, got {s}");
}

#[test]
fn version_popup_dismissal_reaches_zero() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let mid = t0() + Duration::from_millis(250);
    r.set_version_popup(false, mid);
    let s = r.version_popup_scale(mid + Duration::from_millis(200));
    assert!(s < 0.01, "dismissal eases to ~0.0, got {s}");
}

#[test]
fn version_popup_interrupt_continues_from_edge() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    // Interrupt entrance ~halfway.
    let half = t0() + Duration::from_millis(100);
    let scale_at_interrupt = r.version_popup_scale(half);
    r.set_version_popup(false, half);
    let s = r.version_popup_scale(half + Duration::from_millis(1));
    assert!(
        (s - scale_at_interrupt).abs() < 0.2,
        "interrupted animation continues from current scale ({scale_at_interrupt}), not a snap (got {s})"
    );
}

// ===================================================================
// Gateway lobster mascot — presence-gated wandering creature
// ===================================================================

/// A scene carrying an OpenClaw gateway presence (and nothing else, so the only
/// lobster-red pixels on the floor are the lobster's).
fn gateway_scene(
    state: pixtuoid_core::state::DaemonState,
    entered_at: SystemTime,
    last_seen: SystemTime,
    sessions: u32,
) -> SceneState {
    gateway_scene_runs(state, entered_at, last_seen, sessions, &[])
}

/// As `gateway_scene`, with in-flight RUN keys (the busy bubble tell keys on
/// runs, not sessions).
fn gateway_scene_runs(
    state: pixtuoid_core::state::DaemonState,
    entered_at: SystemTime,
    last_seen: SystemTime,
    sessions: u32,
    runs: &[&str],
) -> SceneState {
    let mut s = SceneState::uniform(16);
    s.daemons_mut().insert(
        pixtuoid_core::source::openclaw::SOURCE_NAME.to_string(),
        pixtuoid_core::state::DaemonPresence {
            state,
            active_sessions: sessions,
            last_seen,
            entered_at,
            in_flight_run_keys: runs.iter().map(|s| s.to_string()).collect(),
            current_pid: Some(1),
        },
    );
    s
}

/// Count the busy activity-bubble pixels (the `0xd6,0xf2,0xf8` rising stream).
fn bubble_px(buf: &RgbBuffer) -> usize {
    let bubble = pixtuoid_core::sprite::Rgb {
        r: 0xd6,
        g: 0xf2,
        b: 0xf8,
    };
    let mut n = 0;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if buf.get(x, y) == bubble {
                n += 1;
            }
        }
    }
    n
}

/// Count the lobster's exclusive carapace reds in the buffer (the lobster sprite is
/// not recolored, so its authored RGBs render exactly). An empty agents scene
/// means no recolored shirts can collide.
fn lobster_px(buf: &RgbBuffer) -> usize {
    let reds = [
        pixtuoid_core::sprite::Rgb {
            r: 0xd2,
            g: 0x40,
            b: 0x2f,
        }, // body
        pixtuoid_core::sprite::Rgb {
            r: 0xe8,
            g: 0x55,
            b: 0x40,
        }, // claw
        pixtuoid_core::sprite::Rgb {
            r: 0xc8,
            g: 0x38,
            b: 0x28,
        }, // antenna
        pixtuoid_core::sprite::Rgb {
            r: 0x9e,
            g: 0x2a,
            b: 0x20,
        }, // shade
    ];
    let mut n = 0;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if reds.contains(&buf.get(x, y)) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn no_gateway_mascot_without_presence() {
    // The ~99% who don't run a gateway see a normal office — no lobster.
    let scene = SceneState::uniform(16);
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert_eq!(lobster_px(r.buf()), 0, "no presence ⇒ no lobster pixels");
}

#[test]
fn gateway_mascot_present_when_up() {
    // entered_at well in the past ⇒ steady wander (past the walk-in).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonState::Idle,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        lobster_px(r.buf()) > 10,
        "a live gateway ⇒ the lobster scuttles the floor"
    );
}

#[test]
fn gateway_mascot_busy_bubbles_track_runs_not_sessions() {
    // Busy (an in-flight RUN) renders the activity-bubble stream; a persistent
    // session with NO run must NOT bubble (the run-vs-session intensity fix).
    let entered = t0() - Duration::from_secs(20);

    // Idle with a live session (sessions=1, NO runs) ⇒ no bubbles.
    let idle = gateway_scene(pixtuoid_core::state::DaemonState::Idle, entered, t0(), 1);
    // Busy with two in-flight runs ⇒ bubbles.
    let busy = gateway_scene_runs(
        pixtuoid_core::state::DaemonState::Busy,
        entered,
        t0(),
        1,
        &["r1", "r2"],
    );

    let mut r = build(160, 80, vec![]);
    // Bubbles animate by `now`; scan a few frames so we don't land on an
    // all-off-screen phase, and assert the idle scene NEVER bubbles.
    let mut busy_max = 0;
    let mut idle_max = 0;
    for k in 0..8u64 {
        let now = t0() + Duration::from_millis(k * 130);
        r.render(&busy, &pack(), now).unwrap();
        busy_max = busy_max.max(bubble_px(r.buf()));
        r.render(&idle, &pack(), now).unwrap();
        idle_max = idle_max.max(bubble_px(r.buf()));
    }
    assert!(busy_max > 0, "an in-flight run ⇒ activity bubbles render");
    assert_eq!(idle_max, 0, "a persistent idle session must NOT bubble");
}

#[test]
fn gateway_mascot_walks_out_then_is_gone() {
    let mut r = build(160, 80, vec![]);
    // Just after going Down ⇒ still walking out, visible.
    let leaving = gateway_scene(
        pixtuoid_core::state::DaemonState::Down,
        t0() - Duration::from_secs(20),
        t0() - Duration::from_millis(400),
        0,
    );
    r.render(&leaving, &pack(), t0()).unwrap();
    assert!(
        lobster_px(r.buf()) > 0,
        "mid walk-out, the lobster is still visible"
    );

    // Well past the walk-out window ⇒ gone (back to a normal office).
    let gone = gateway_scene(
        pixtuoid_core::state::DaemonState::Down,
        t0() - Duration::from_secs(30),
        t0() - Duration::from_secs(10),
        0,
    );
    r.render(&gone, &pack(), t0()).unwrap();
    assert_eq!(
        lobster_px(r.buf()),
        0,
        "after the walk-out, the lobster has left"
    );
}

#[test]
fn gateway_mascot_wanders_over_time() {
    // Same scene rendered across a wander cycle ⇒ the lobster changes position
    // (proves the motion is live, not a fixed sticker).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonState::Idle,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    let mut r = build(160, 80, vec![]);
    let mut tops = std::collections::HashSet::new();
    for k in 0..8u64 {
        let now = t0() + Duration::from_secs(k * 3);
        r.render(&scene, &pack(), now).unwrap();
        // Signature: the topmost-leftmost lobster pixel.
        let buf = r.buf();
        'scan: for y in 0..buf.height() {
            for x in 0..buf.width() {
                let reds = [
                    pixtuoid_core::sprite::Rgb {
                        r: 0xd2,
                        g: 0x40,
                        b: 0x2f,
                    },
                    pixtuoid_core::sprite::Rgb {
                        r: 0xe8,
                        g: 0x55,
                        b: 0x40,
                    },
                ];
                if reds.contains(&buf.get(x, y)) {
                    tops.insert((x, y));
                    break 'scan;
                }
            }
        }
    }
    assert!(
        tops.len() >= 2,
        "the lobster should wander to ≥2 distinct positions, saw {}",
        tops.len()
    );
}

/// Bounding box (in PIXEL coords) of the lobster's carapace reds, or `None` if
/// the lobster isn't on screen. Reuses the `lobster_px` red set.
fn lobster_red_bbox(buf: &RgbBuffer) -> Option<(u16, u16, u16, u16)> {
    let reds = [
        pixtuoid_core::sprite::Rgb {
            r: 0xd2,
            g: 0x40,
            b: 0x2f,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0xe8,
            g: 0x55,
            b: 0x40,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0xc8,
            g: 0x38,
            b: 0x28,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0x9e,
            g: 0x2a,
            b: 0x20,
        },
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let mut any = false;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if reds.contains(&buf.get(x, y)) {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

#[test]
fn gateway_mascot_tooltip_on_hover() {
    // entered_at well in the past ⇒ steady wander (a stable lobster to aim at).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonState::Idle,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    // vec![] = no pet, so the pet hover arm is skipped and the mascot arm runs.
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();

    // Before any hover, no gateway tooltip text is painted.
    assert!(
        !frame_text(r.frame_buffer()).contains("gateway"),
        "no hover ⇒ no mascot tooltip"
    );

    // Center of the lobster's red bbox (pixel coords) → terminal cell.
    let (x0, y0, x1, y1) = lobster_red_bbox(r.buf()).expect("lobster on screen");
    let cx = (x0 + x1) / 2;
    let cy_px = (y0 + y1) / 2;
    // The 14px-wide hitbox tolerates the approximate center; half-block ⇒ /2.
    r.set_mouse_pos(Some((cx, cy_px / 2)));
    r.render(&scene, &pack(), t0()).unwrap();

    // The mascot tooltip reads " <name> gateway · idle " — the literal "gateway"
    // is exclusive to the mascot arm (pet/coffee/furniture tooltips never say it),
    // so this distinguishes the mascot branch from the other fallthroughs.
    assert!(
        frame_text(r.frame_buffer()).contains("gateway"),
        "hovering the lobster shows the gateway mascot tooltip"
    );
}

// ===================================================================
// Help overlay
// ===================================================================

#[test]
fn help_overlay_renders_shortcuts() {
    let scene = scene_with(vec![idle("/help/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.set_help_open(true);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.help_open());
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("theme") || text.contains("Keyboard") || text.contains("help"),
        "help overlay should list shortcuts; frame was:\n{text}"
    );
}

#[test]
fn onboarding_overlay_renders_roster_and_hint() {
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let scene = scene_with(vec![idle("/onboard/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    // A two-CLI roster; a large elapsed so the typewriter + every staggered row
    // + the key hint are all fully revealed.
    r.set_onboarding_frame(OnboardingFrame {
        open: true,
        rows: vec![
            WelcomeRow {
                source_id: "codex",
                label_prefix: "cx",
                display_name: "Codex".into(),
                checked: true,
            },
            WelcomeRow {
                source_id: "claude-code",
                label_prefix: "cc",
                display_name: "Claude Code".into(),
                checked: false,
            },
        ],
        selected: 0,
        elapsed_ms: 100_000,
        dim: 0.4,
    });
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Welcome to pixtuoid"),
        "onboarding title; frame:\n{text}"
    );
    assert!(text.contains("Codex"), "checked roster row; frame:\n{text}");
    assert!(
        text.contains("Claude Code"),
        "unchecked roster row; frame:\n{text}"
    );
    assert!(
        text.contains("space toggle") && text.contains("esc skip"),
        "key hint shown once rows are in; frame:\n{text}"
    );
}

#[test]
fn onboarding_dims_the_office_buffer() {
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let scene = scene_with(vec![idle("/dim/0.jsonl", 0, t0())], 16);

    // Baseline: onboarding closed → full-brightness office.
    let mut base = build(100, 40, vec![]);
    base.render(&scene, &pack(), t0()).unwrap();
    let bright = avg_lum(base.buf(), 0, 0, base.buf().width(), base.buf().height());

    // Same scene, onboarding open + fully ramped (large elapsed) → the office
    // pixel buffer is dimmed as the modal backdrop (the card paints on the cell
    // layer, not the buffer, so this measures the office only).
    let mut dimmed = build(100, 40, vec![]);
    dimmed.set_onboarding_frame(OnboardingFrame {
        open: true,
        rows: vec![WelcomeRow {
            source_id: "codex",
            label_prefix: "cx",
            display_name: "Codex".into(),
            checked: true,
        }],
        selected: 0,
        elapsed_ms: 100_000,
        dim: 0.4,
    });
    dimmed.render(&scene, &pack(), t0()).unwrap();
    let dim = avg_lum(
        dimmed.buf(),
        0,
        0,
        dimmed.buf().width(),
        dimmed.buf().height(),
    );

    assert!(
        dim < bright * 0.6,
        "onboarding should dim the office buffer: dim={dim} vs bright={bright}"
    );
}

// ===================================================================
// Footer / HUD (rendered text)
// ===================================================================

#[test]
fn footer_shows_floor_indicator_on_multi_floor() {
    let scene = two_floor_scene();
    let mut r = build(120, 40, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("1/2") || text.contains("F1"),
        "multi-floor footer should show a floor indicator; frame:\n{text}"
    );
}

// ===================================================================
// Hit-testing against a real rendered layout
// ===================================================================

#[test]
fn furniture_hit_test_resolves_against_rendered_layout() {
    let scene = scene_with(vec![idle("/hit/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // hit_test_furniture takes (pixel_x, cell_y) and doubles y internally.
    let desk = layout.home_desks[0];
    let hit = crate::tui::hit_test::hit_test_furniture(layout, desk.x + 4, desk.y / 2 + 1);
    assert_eq!(
        hit,
        Some("Desk"),
        "a desk pixel should hit the Desk furniture in the cached layout"
    );
}

#[test]
fn coffee_machine_hit_test_resolves_on_pantry() {
    use pixtuoid_scene::layout::WaypointKind;
    let scene = scene_with(vec![idle("/cm/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let pantry = layout
        .waypoints
        .iter()
        .find(|w| w.kind == WaypointKind::Pantry)
        .expect("a 140×48 office must lay out a pantry"); // no silent skip
                                                          // Scan the counter neighbourhood; the machine occupies part of it.
    let cx = pantry.pos.x;
    let cy = pantry.pos.y / 2;
    let mut found = false;
    for dx in -14i32..=14 {
        for dy in -4i32..=4 {
            let mx = (cx as i32 + dx).max(0) as u16;
            let my = (cy as i32 + dy).max(0) as u16;
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                found = true;
            }
        }
    }
    assert!(
        found,
        "the coffee machine should be hit-testable somewhere on the pantry counter"
    );
}

#[test]
fn pet_hit_test_resolves_at_pet_position() {
    let scene = scene_with(vec![active("/ph/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(120, 44, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, anim, kind } = r.cached_pet_pos().expect("pet placed");
    assert!(
        crate::tui::hit_test::hit_test_pet(kind, pos, anim, pos.x, pos.y / 2),
        "clicking the pet's own position should hit it"
    );
}

// ===================================================================
// Rendered text: labels, tooltips, footer (via frame_buffer)
// ===================================================================

#[test]
fn agent_label_painted_above_character() {
    let mut s = idle("/lbl/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = "ZQXLBL".into();
    let scene = scene_with(vec![s], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("ZQXLBL"),
        "the agent's label should be painted above it"
    );
}

#[test]
fn pinned_agent_renders_stats_tooltip() {
    let a = AgentId::from_transcript_path("/pintip/0.jsonl");
    let scene = scene_with(vec![slot(a, 0, 0, t0() - Duration::from_secs(600))], 16);
    let mut r = build(120, 44, vec![]);
    // Baseline without pin.
    r.render(&scene, &pack(), t0()).unwrap();
    let before = frame_text(r.frame_buffer());
    assert!(!before.contains("calls"));
    // Pin → centered stats tooltip appears.
    r.set_pinned_agent(Some(a));
    r.render(&scene, &pack(), t0()).unwrap();
    let after = frame_text(r.frame_buffer());
    assert!(
        after.contains("calls") && after.contains("active"),
        "pinned tooltip should show the agent stat line"
    );
}

#[test]
fn footer_shows_agent_count() {
    let scene = scene_with(
        vec![
            active("/f/0.jsonl", 0, "Edit", t0()),
            idle("/f/1.jsonl", 1, t0()),
            idle("/f/2.jsonl", 2, t0()),
        ],
        16,
    );
    let mut r = build(140, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("agents") && text.contains('3'),
        "full-width footer shows the agent count; frame footer area:\n{}",
        text.lines().last().unwrap_or("")
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
// Per-tool monitor glow (pixel-level)
// ===================================================================

#[test]
fn tool_glow_tint_differs_by_tool() {
    let render_tool = |detail: &str| -> (RgbBuffer, Point) {
        // Long-seated (entry walk done) so it's SeatedTyping at the desk and
        // the monitor screen-glow paints.
        let scene = scene_with(
            vec![active(
                "/tg/0.jsonl",
                0,
                detail,
                t0() - Duration::from_secs(300),
            )],
            16,
        );
        let mut r = build(120, 44, vec![]);
        r.render(&scene, &pack(), t0()).unwrap();
        let desk = r.cached_layout().expect("layout").home_desks[0];
        (r.buf().clone(), desk)
    };
    let (edit, desk) = render_tool("Edit src/main.rs");
    let (bash, _) = render_tool("Bash npm test");
    // Tool tint colours the monitor glow AND the seated worker's skin, both
    // within the cubicle box around the desk.
    let d = region_diff(
        &edit,
        &bash,
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(6),
        20,
        16,
    );
    assert!(
        d > 200,
        "Edit vs Bash should tint the cubicle measurably differently (diff={d})"
    );
}

// ===================================================================
// Tooltip variants on hover (exercise widgets/tooltip.rs branches)
// ===================================================================

#[test]
fn coffee_machine_tooltip_on_hover() {
    let scene = scene_with(vec![idle("/tt/c.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // Find a cell that hits the coffee machine.
    let mut hover = None;
    'scan: for my in 0..48u16 {
        for mx in 0..140u16 {
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                hover = Some((mx, my));
                break 'scan;
            }
        }
    }
    let hover = hover.expect("coffee machine should be hit-testable");
    r.set_mouse_pos(Some(hover));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Ivan"),
        "hovering the coffee machine shows the Buy-Ivan-a-coffee tooltip"
    );
}

#[test]
fn furniture_tooltip_on_hover_over_empty_desk() {
    // Agent on desk 0; hover an EMPTY desk so furniture (not agent) tooltip wins.
    let scene = scene_with(vec![idle("/tt/f.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    if layout.home_desks.len() < 2 {
        return;
    }
    let d1 = layout.home_desks[1];
    r.set_mouse_pos(Some((d1.x + 4, d1.y / 2 + 1)));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Desk"),
        "hovering an empty desk shows the Desk furniture tooltip"
    );
}

#[test]
fn pet_tooltip_on_hover() {
    let scene = scene_with(vec![active("/tt/p.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(140, 48, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Cat") || text.contains("purr"),
        "hovering the cat shows its tooltip"
    );
}

#[test]
fn pet_tooltip_shows_custom_name() {
    let scene = scene_with(vec![active("/tt/cn.jsonl", 0, "Edit", t0())], 16);
    let cat = pixtuoid_scene::pet::Pet {
        kind: PetKind::Cat,
        name: "Luna".to_string(),
    };
    let mut r = build_pets(140, 48, vec![cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Luna"),
        "hovering the cat shows its custom name; got:\n{text}"
    );
    assert!(
        !text.contains("Office Cat"),
        "custom name replaces the default, not appended"
    );
}

#[test]
fn pet_tooltip_falls_back_to_default_name_when_not_configured() {
    let scene = scene_with(vec![active("/tt/fb.jsonl", 0, "Edit", t0())], 16);
    // No custom name → default ("Office Cat"). `build` defaults the name.
    let mut r = build(140, 48, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Office Cat"),
        "an unconfigured cat falls back to the default name; got:\n{text}"
    );
}

// ===================================================================
// Theme picker + version-popup PAINT (renderer.rs / hud.rs branches)
// ===================================================================

#[test]
fn theme_picker_renders_theme_names() {
    let scene = scene_with(vec![idle("/tp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.set_theme_picker(Some(0));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("cyberpunk") || text.contains("normal"),
        "the theme picker lists theme names"
    );
}

#[test]
fn version_popup_paints_when_open() {
    let scene = scene_with(vec![idle("/vp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    // Baseline (no popup).
    r.render(&scene, &pack(), t0()).unwrap();
    let baseline = r.buf().clone();
    // Open popup; render past the 200ms entrance so it's at full scale.
    r.set_version_popup(true, t0());
    let t1 = t0() + Duration::from_millis(250);
    r.render(&scene, &pack(), t1).unwrap();
    assert!(
        r.last_popup_scale() > 0.9,
        "popup should be near full scale"
    );
    let d = region_diff(
        &baseline,
        r.buf(),
        0,
        0,
        baseline.width(),
        baseline.height(),
    );
    assert!(
        d > 1000,
        "an open version popup must paint over the scene (diff={d})"
    );
}

// ===================================================================
// Weather smoke-render (background/* + ambient.rs paint paths)
// ===================================================================

#[test]
fn weather_variants_render_without_panic_and_vary() {
    // Weather is a deterministic hash of wall-clock (changes every ~10min).
    // Render across a week of 10-min steps: every variant's paint path runs
    // (no panic), and the window strip takes several distinct appearances.
    let scene = scene_with(vec![idle("/w/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    let mut sigs = std::collections::HashSet::new();
    for step in 0..120u64 {
        // 10-min steps so each sample can land on a different weather window.
        let now = t0() + Duration::from_secs(step * 600 + 12 * 3600);
        r.render(&scene, &pack(), now).unwrap();
        // Signature the top window strip (where weather effects paint).
        let buf = r.buf();
        let mut s: u64 = 0;
        for y in 0..(buf.height() / 4).max(1) {
            for x in (0..buf.width()).step_by(7) {
                let c = buf.get(x, y);
                s = s
                    .wrapping_mul(1099511628211)
                    .wrapping_add((c.r as u64) << 16 | (c.g as u64) << 8 | c.b as u64);
            }
        }
        sigs.insert(s);
    }
    assert!(
        sigs.len() >= 4,
        "weather/time variation should produce several distinct window renders, saw {}",
        sigs.len()
    );
}

/// Concatenated symbols of a ratatui cell rectangle — for asserting that a
/// specific bit of text (e.g. a chitchat bubble) rendered inside a region.
fn region_text(buf: &ratatui::buffer::Buffer, cx: u16, cy: u16, cw: u16, ch: u16) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in cy..(cy + ch).min(area.y + area.height) {
        for x in cx..(cx + cw).min(area.x + area.width) {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
    }
    out
}

// Meeting-room rendering, end-to-end. Drives idle agents on a floor that has a
// meeting room and asserts that, over simulated time, the room (1) visibly
// fills with characters and (2) hosts a GROUP chitchat — a bubble appearing in
// the meeting-room region requires ≥2 agents seated/standing at meeting SLOTS
// (the new waypoint kinds) AND the whole render pipeline (slots → sit/stand
// sprites → venue-keyed chat → bubble widget) working. Emergent, not forced:
// the long per-spot dwell makes overlaps reliable.
#[test]
fn meeting_room_fills_and_hosts_group_chitchat() {
    let pack = pack();
    let mut now = t0();

    let cap = 64;
    let n_agents = 40usize;
    let mut scene = SceneState::uniform(cap);
    for i in 0..n_agents {
        let id = AgentId::from_transcript_path(&format!("/h/mtg{i}.jsonl"));
        // Stagger start times so wander cycles desync and the room sees a mix
        // of arrivals/departures.
        let started = now - Duration::from_secs(5 + (i as u64 * 11) % 80);
        scene.agents.insert(id, slot(id, 0, i, started));
    }

    let mut r = build(160, 56, vec![]);
    r.render(&scene, &pack, now).expect("render");
    let layout = r.cached_layout().expect("layout").clone();
    let mr = layout
        .meeting_room
        .expect("floor 0 must have a meeting room at this size");

    // Empty-room pixel baseline (same furniture, no agents) so the region diff
    // isolates the characters.
    let mut r0 = build(160, 56, vec![]);
    r0.render(&SceneState::uniform(cap), &pack, now)
        .expect("render");
    let baseline = r0.buf().clone();

    // The layout must actually carry meeting slots (otherwise the test is
    // vacuous — agents could "occupy" the room while just passing through).
    let slot_count = layout
        .waypoints
        .iter()
        .filter(|w| {
            matches!(
                w.kind,
                pixtuoid_scene::layout::WaypointKind::MeetingSofa
                    | pixtuoid_scene::layout::WaypointKind::MeetingStand
            )
        })
        .count();
    assert!(slot_count >= 4, "expected meeting slots, got {slot_count}");

    // Meeting-room cell band (px→cell: x unchanged, y halved), padded upward so
    // a bubble drawn above a sitter's head is included.
    let cell_y0 = (mr.y / 2).saturating_sub(4);
    let cell_h = mr.height / 2 + 8;

    // Step the clock in coarse 250 ms beats rather than 33 ms frames: this test
    // cares about simulated *time* (agents wandering into the room and chatting),
    // not per-frame smoothness, and 250 ms stays well under the stale-resume
    // trigger (≥7 s) so the wander machine advances normally — ~6× fewer renders.
    const BUDGET: usize = 1200; // 250ms beats → 300s simulated
    let mut saw_characters = false;
    let mut chat_iter: Option<usize> = None;
    for iter in 1..=BUDGET {
        now += Duration::from_millis(250);
        r.render(&scene, &pack, now).expect("render");

        if !saw_characters {
            let d = region_diff(&baseline, r.buf(), mr.x, mr.y, mr.width, mr.height);
            saw_characters = d > 4_000;
        }
        if chat_iter.is_none() {
            // A chitchat line inside the meeting-room cell band can only come
            // from ≥2 agents seated/standing at meeting SLOTS forming a group
            // conversation — exercising slots → sit/stand sprites → venue-keyed
            // chat → bubble widget end to end.
            let text = region_text(r.frame_buffer(), mr.x, cell_y0, mr.width + 6, cell_h);
            if pixtuoid_scene::chitchat::CHITCHAT_LINES
                .iter()
                .any(|l| text.contains(l))
            {
                chat_iter = Some(iter);
            }
        }
        if saw_characters && chat_iter.is_some() {
            break;
        }
    }

    assert!(
        saw_characters,
        "agents never visibly occupied the meeting room"
    );
    let chat_iter = chat_iter.expect("no group chitchat bubble ever appeared in the meeting room");
    // Headroom guard: with this density the group should form comfortably
    // within budget. The bound is 3/4 (not 1/2): the 3-seat sofa added meeting
    // slots, which grows the waypoint pool `waypoint_index_for_cycle` selects
    // from and deterministically reshuffles WHEN agents land at meeting slots
    // together (now ~700/1200 vs the old ~half). Still a real "fill erosion"
    // canary — if a future constant change pushes it past 3/4 of the budget it
    // surfaces here as a clear "took too long" rather than an edge-of-budget
    // timeout.
    assert!(
        chat_iter < (BUDGET * 3) / 4,
        "group chitchat took {chat_iter}/{BUDGET} iterations — fill margin eroded; \
         expected within 3/4 of the budget"
    );
}

#[test]
fn meeting_glass_partition_connects_at_window_and_corner() {
    // Regression: the vertical meeting-room divider used to start 4 px below
    // the north wall band (a floating strip) and stop short of the horizontal
    // wall, leaving an L-notch at the inside corner. The glass partition now
    // stitches both joints. Asserted relative to same-row references so the
    // check is immune to time-of-day dim / weather tint applied globally.
    let mut r = build(192, 80, vec![]);
    let scene = scene_with(vec![idle("/h/glass.jsonl", 0, t0())], 16);
    r.render(&scene, &pack(), t0()).expect("render");

    let layout = r.cached_layout().expect("layout").clone();
    let v_x = layout
        .room_walls
        .iter()
        .find(|w| w.start.x == w.end.x)
        .map(|w| w.start.x)
        .expect("standard floor has a vertical divider");
    let h_y = layout
        .room_walls
        .iter()
        .find(|w| w.start.y == w.end.y)
        .map(|w| w.start.y)
        .expect("standard floor has a horizontal divider");
    let top_wall_h = layout.top_margin - 4;

    let buf = r.buf();
    let dist = |a: pixtuoid_core::sprite::Rgb, b: pixtuoid_core::sprite::Rgb| {
        (a.r as i32 - b.r as i32).abs()
            + (a.g as i32 - b.g as i32).abs()
            + (a.b as i32 - b.b as i32).abs()
    };
    // The frosted glass is a translucent cool gradient with no single colour,
    // so reference both its lit (left/dx0) and soft (right/dx2) edges — sampled
    // high on the wall where it's unambiguously glass — plus a floor sample.
    // Any glass pixel (lit / body / soft / seam) is nearer one of the two
    // glass edges than the warm carpet. References share the global lighting.
    let glass_lit = buf.get(v_x, layout.top_margin + 2);
    let glass_soft = buf.get(v_x + 2, layout.top_margin + 2);
    let floor_ref = buf.get(v_x.saturating_sub(8), top_wall_h + 6);
    let is_glass = |p: pixtuoid_core::sprite::Rgb| {
        dist(p, glass_lit).min(dist(p, glass_soft)) < dist(p, floor_ref)
    };

    // Top joint: the row flush with the window band must be glass, not floor.
    assert!(
        is_glass(buf.get(v_x, top_wall_h + 1)),
        "vertical divider should connect up to the window band (no floor gap)"
    );

    // Corner joint: the vertical's own soft edge (which the horizontal run,
    // ending at v_x, never covers) must extend down through the horizontal
    // wall band — that 2-px-wide strip was the L-notch left by the old code.
    assert!(
        is_glass(buf.get(v_x + 2, h_y + 2)),
        "vertical divider should fill the inside corner at the horizontal wall"
    );
}

// ===================================================================
// hit_test_furniture — every per-kind label arm, against a REAL layout
// ===================================================================

// Drive a real production layout (the same `compute_with_seed` the renderer
// calls) and hover the CENTER of each populated furniture field, asserting
// hit_test_furniture returns that kind's label. This closes the waypoint loop
// (Pantry/Phone Booth/Standing Desk/Vending/Printer), meeting sofas, pantry
// table/chairs, plants, floor lamp, wall+pod decor, lounge couch + side table,
// and the procedural meeting/pantry items. Ficus + BulletinBoard are covered
// by synthetic-layout unit tests (compute never emits those two kinds).
#[test]
fn furniture_hit_test_covers_every_kind_on_real_layouts() {
    use crate::tui::hit_test::hit_test_furniture;
    use pixtuoid_scene::layout::{
        Layout, PlantKind, PodDecor, WallDecor, WaypointKind, TEST_DEFAULT_DESKS,
    };
    use std::collections::HashSet;

    // Scan the WHOLE cell grid and collect every label hit_test_furniture
    // returns anywhere. Per-item shadowing (e.g. a floor lamp under the couch
    // region, a chair under the pantry table) means a single center-probe is
    // brittle, but an item's NON-shadowed cells still yield its label — so the
    // returned-label SET reaches every arm that is geometrically reachable.
    let labels_on = |layout: &Layout| -> HashSet<&'static str> {
        let mut set = HashSet::new();
        for cy in 0..(layout.buf_h / 2) {
            for cx in 0..layout.buf_w {
                if let Some(l) = hit_test_furniture(layout, cx, cy) {
                    set.insert(l);
                }
            }
        }
        set
    };

    // Seeds 0 and 3 between them populate every field (seed 3 brings the
    // PhoneBooth/StandingDesk pod-decor + a coat-rack-only meeting room).
    let mut covered: HashSet<&'static str> = HashSet::new();
    for seed in [0u64, 3] {
        let layout = Layout::compute_with_seed(160, 200, Some(TEST_DEFAULT_DESKS), seed)
            .unwrap_or_else(|| panic!("layout for seed {seed}"));
        let labels = labels_on(&layout);

        // For every kind PRESENT in this layout, its label must be reachable.
        for wp in &layout.waypoints {
            let want = match wp.kind {
                WaypointKind::Pantry => Some("Pantry Counter"),
                WaypointKind::PhoneBooth => Some("Phone Booth"),
                WaypointKind::StandingDesk => Some("Standing Desk"),
                WaypointKind::VendingMachine => Some("Vending Machine"),
                WaypointKind::Printer => Some("Printer"),
                WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {
                    None
                }
            };
            if let Some(label) = want {
                assert!(
                    labels.contains(label),
                    "seed {seed}: waypoint {:?} → label {label:?} never resolved",
                    wp.kind
                );
            }
        }
        if !layout.meeting_furniture.is_empty() {
            assert!(labels.contains("Meeting Sofa"), "seed {seed}: Meeting Sofa");
            assert!(
                labels.contains("Meeting Table"),
                "seed {seed}: Meeting Table"
            );
        }
        if layout.pantry_table.is_some() {
            assert!(labels.contains("Pantry Table"), "seed {seed}: Pantry Table");
        }
        if !layout.pantry_chairs.is_empty() {
            assert!(labels.contains("Chair"), "seed {seed}: Chair");
        }
        if layout.floor_lamp.is_some() {
            assert!(labels.contains("Floor Lamp"), "seed {seed}: Floor Lamp");
        }
        if layout.couch_sprite_center.is_some() {
            assert!(labels.contains("Lounge Sofa"), "seed {seed}: Lounge Sofa");
        }
        if layout.lounge_side_table.is_some() {
            assert!(labels.contains("Side Table"), "seed {seed}: Side Table");
        }
        for item in &layout.plants {
            let label = match item.kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            };
            assert!(labels.contains(label), "seed {seed}: plant {:?}", item.kind);
        }
        for item in &layout.wall_decor {
            let label = match item.kind {
                WallDecor::Whiteboard => "Whiteboard",
                WallDecor::Bookshelf => "Bookshelf",
                WallDecor::BulletinBoard => "Bulletin Board",
                WallDecor::ExitSign => "Exit Sign",
                WallDecor::MeetingScreen => "Meeting Screen",
            };
            assert!(
                labels.contains(label),
                "seed {seed}: wall decor {:?}",
                item.kind
            );
        }
        for item in &layout.pod_decor {
            let label = match item.kind {
                PodDecor::PlantTall => "Tall Plant",
                PodDecor::Whiteboard => "Whiteboard",
                PodDecor::Tv => "TV Stand",
                PodDecor::PhoneBooth => "Phone Booth",
                PodDecor::StandingDesk => "Standing Desk",
            };
            assert!(
                labels.contains(label),
                "seed {seed}: pod decor {:?}",
                item.kind
            );
        }
        // Procedural room items (coat rack / doormat / water cooler / trash bin)
        // are emitted by hit_test_furniture from the room bounds, not a layout
        // field, so just gather whatever resolved.
        covered.extend(labels);
    }

    // The procedural meeting/pantry-room items must surface across the two
    // seeds (seed 0 has both a meeting room and a pantry room at 160×200).
    for label in [
        "Coat Rack",
        "Doormat",
        "Water Cooler",
        "Trash Bin",
        "Elevator",
    ] {
        assert!(
            covered.contains(label),
            "procedural/room item {label:?} never resolved across seeds"
        );
    }
}

// ===================================================================
// hit_test_agent + hover marker
// ===================================================================

// Hover an idle agent's own sprite cell → the label gains the '▸' hovered
// marker (exercises hit_test_agent's Some-return + tooltip is_hovered branch).
#[test]
fn hovering_an_agent_marks_its_label() {
    let mut s = idle("/hov/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = "HOVERME".into();
    let scene = scene_with(vec![s], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    // A long-idle agent at its home desk; mirror hit_test_from_tui's anchor.
    let desk = r.cached_layout().expect("layout").home_desks[0];
    let cell_x = desk.x + 2;
    let cell_y = desk.y.saturating_sub(4) / 2 + 1;
    r.set_mouse_pos(Some((cell_x, cell_y)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{25b8}HOVERME") || text.contains("\u{25b8}"),
        "hovering an agent should add the ▸ marker to its label; frame:\n{text}"
    );
}

// ===================================================================
// Tooltip state arms: Active (with detail), Waiting, exiting label,
// active-% numeric, flip-left, bottom-edge flip-up (CG4/CG5/CG6)
// ===================================================================

#[test]
fn pinned_active_agent_tooltip_shows_state_and_detail() {
    // active() sets last_event_at = started; created >5s ago so active_str is
    // a numeric percent (not "--%"), and active_ms>0 forces a non-zero %.
    let mut a = active(
        "/ttA/0.jsonl",
        0,
        "Edit src/lib.rs",
        t0() - Duration::from_secs(600),
    );
    a.active_ms = 120_000; // 120s active over a 600s session ⇒ 20%
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.set_pinned_agent(Some(id));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Active"), "active state arm: {text}");
    assert!(text.contains("Edit src/lib.rs"), "detail line: {text}");
    // active_str numeric branch (session ≥5s): a '%' that is not "--%".
    assert!(
        text.contains('%') && !text.contains("--%"),
        "numeric active %: {text}"
    );
}

#[test]
fn pinned_waiting_agent_tooltip_shows_reason() {
    let mut a = idle("/ttW/0.jsonl", 0, t0() - Duration::from_secs(60));
    a.state = ActivityState::Waiting {
        reason: Arc::from("permission to edit"),
    };
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.set_pinned_agent(Some(id));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Waiting"), "waiting state arm: {text}");
    assert!(text.contains("permission"), "reason line: {text}");
}

#[test]
fn exiting_agent_label_uses_exiting_color() {
    // The exiting_at branch in paint_label_widgets: render an exiting agent and
    // confirm its label still paints (the branch runs without panic). Color is
    // theme-internal; we assert the label survives the exiting code path.
    let mut a = idle("/ttE/0.jsonl", 0, t0() - Duration::from_secs(10));
    a.label = "LEAVING".into();
    a.exiting_at = Some(t0());
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0() + Duration::from_millis(100))
        .unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("LEAVING"), "exiting agent label: {text}");
}

#[test]
fn pinned_then_removed_agent_is_a_safe_noop() {
    // paint_hover_tooltip's early return when the pinned id is gone from scene.
    let id = AgentId::from_transcript_path("/ttGone/0.jsonl");
    let scene = scene_with(vec![slot(id, 0, 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    r.set_pinned_agent(Some(id));
    // Re-render with the agent removed → tooltip paint hits the get()=None bail.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .expect("render must not panic when the pinned agent vanished");
}

// ===================================================================
// Pet tooltip: cooldown (purr/woof per kind) + sleeping arms (CG5)
// ===================================================================

#[test]
fn pet_tooltip_shows_cooldown_reaction_for_cat_and_dog() {
    for (kind, word) in [(PetKind::Cat, "purr"), (PetKind::Dog, "woof")] {
        let scene = scene_with(vec![active("/ck/0.jsonl", 0, "Edit", t0())], 16);
        let mut r = build(140, 48, vec![kind]);
        r.render(&scene, &pack(), t0()).unwrap();
        let PetFrame { pos, .. } = r.cached_pet_pos().expect("pet placed");
        // Activate the petting cooldown so the tooltip shows purr/woof.
        r.set_active_pet(Some(PetState {
            petted_at: t0(),
            pet_pos: pos,
            kind,
            floor_idx: 0,
        }));
        r.set_mouse_pos(Some((pos.x, pos.y / 2)));
        r.render(&scene, &pack(), t0() + Duration::from_millis(200))
            .unwrap();
        let text = frame_text(r.frame_buffer());
        assert!(
            text.contains(word),
            "{kind:?} on cooldown should show '{word}'; got:\n{text}"
        );
    }
}

#[test]
fn pet_tooltip_shows_sleeping_when_all_idle() {
    // With every agent idle the cat sleeps (sleeps_near_idle); hovering it shows
    // the sleeping line. Use a long-idle scene so the pet settles to sleep.
    let scene = scene_with(
        vec![idle("/slp/0.jsonl", 0, t0() - Duration::from_secs(300))],
        16,
    );
    let mut r = build(160, 64, vec![PetKind::Cat]);
    // Scan the pet cycle for a sleeping frame, then hover it.
    let mut hit = None;
    for i in 0..40u64 {
        let now = t0() + Duration::from_secs(i);
        r.render(&scene, &pack(), now).unwrap();
        if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
            if anim == PetKind::Cat.sleep_anim() {
                hit = Some((pos, now));
                break;
            }
        }
    }
    let (pos, now) = hit.expect("a long-idle cat must enter its sleep anim within the window");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), now).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("sleeping"),
        "hovering a sleeping cat shows the sleeping line; got:\n{text}"
    );
}

// Hover a furniture item near the TOP edge so paint_simple_tooltip flips the
// box BELOW the cursor (the my < scene_rect.y + tip_h branch).
#[test]
fn furniture_tooltip_flips_below_near_top_edge() {
    let scene = scene_with(vec![idle("/flip/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // Find a furniture hit at the smallest cell-y (closest to the top edge).
    let mut top_hit = None;
    'scan: for my in 0..6u16 {
        for mx in 0..140u16 {
            if crate::tui::hit_test::hit_test_furniture(layout, mx, my).is_some() {
                top_hit = Some((mx, my));
                break 'scan;
            }
        }
    }
    let (mx, my) = top_hit.expect("some furniture must hover-test near the top edge");
    r.set_mouse_pos(Some((mx, my)));
    r.render(&scene, &pack(), t0())
        .expect("top-edge furniture hover must flip the tooltip below without panic");
}

// Hover an AGENT near the BOTTOM edge so paint_hover_tooltip flips the panel UP
// (the ty overflow branch). Pin it to force the centered/hover panel path.
#[test]
fn agent_tooltip_flips_up_near_bottom_edge() {
    let scene = scene_with(
        vec![idle("/flup/0.jsonl", 0, t0() - Duration::from_secs(120))],
        16,
    );
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout").clone();
    // Pick the home desk nearest the bottom of the scene.
    let bottom_desk = layout
        .home_desks
        .iter()
        .max_by_key(|d| d.y)
        .copied()
        .expect("a home desk");
    let id = AgentId::from_transcript_path("/flup/0.jsonl");
    r.set_pinned_agent(Some(id));
    // Hover near the very bottom rows so the hover-tooltip ty must flip up.
    let my = (44u16).saturating_sub(2);
    r.set_mouse_pos(Some((bottom_desk.x, my)));
    r.render(&scene, &pack(), t0())
        .expect("bottom-edge hover must not panic");
    // Reaching here (no panic, tooltip flipped within bounds) is the assertion.
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

// ===================================================================
// tui_renderer: render_transition too-small bail (CG9) + getters (CG10)
// ===================================================================

#[test]
fn transition_on_too_small_terminal_clears_interaction_state() {
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

// --- agent dashboard overlay -------------------------------------------------

use crate::tui::dashboard::{build_dashboard_rows, DashboardFolds};

#[test]
fn dashboard_popup_renders_labels_states_and_live_tool() {
    let mut r = build(120, 44, vec![]);
    let mut a = active("/h/alpha.jsonl", 0, "Edit reducer.rs", t0());
    a.label = "cc\u{b7}alpha".into();
    let mut b = idle("/h/beta.jsonl", 1, t0());
    b.label = "cc\u{b7}beta".into();
    let scene = scene_with(vec![a, b], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let first = rows[0].agent_id;
    r.set_dashboard_frame_parts(true, rows, Some(first), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Agents ("), "header missing:\n{text}");
    assert!(text.contains("cc\u{b7}alpha"), "alpha row missing:\n{text}");
    assert!(text.contains("cc\u{b7}beta"), "beta row missing:\n{text}");
    assert!(
        text.contains("Edit reducer.rs"),
        "live tool detail missing:\n{text}"
    );
    assert!(text.contains("idle"), "idle state missing:\n{text}");
}

#[test]
fn connection_panel_renders_both_facets_borderless() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![
        ConnectionRow {
            source_id: "claude",
            label_prefix: "cc",
            display_name: "Claude Code",
            state: ConnState::Connected,
            config_path: Some(std::path::PathBuf::from("~/.claude/settings.json")),
            target: None,
            health: None,
        },
        ConnectionRow {
            source_id: "antigravity",
            label_prefix: "ag",
            display_name: "Antigravity",
            state: ConnState::Disconnected,
            config_path: None,
            target: None,
            health: None,
        },
    ];
    let live = vec![
        LiveInfo {
            agents: 2,
            last_event_age: Some(std::time::Duration::from_secs(3)),
            dead: false,
        },
        LiveInfo {
            agents: 1,
            last_event_age: Some(std::time::Duration::from_secs(12)),
            dead: false,
        },
    ];
    r.set_connection_frame_parts(
        true,
        rows,
        live,
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Sources"), "title missing:\n{text}");
    assert!(text.contains("[cc]"), "cc badge missing:\n{text}");
    assert!(text.contains("[ag]"), "ag badge missing:\n{text}");
    assert!(text.contains("2 agents"), "live count missing:\n{text}");
    assert!(text.contains("socket"), "socket line missing:\n{text}");
    // Selected row (cc) is Connected → the detail line shows where it's installed.
    assert!(
        text.contains("installed at"),
        "connected detail (install path) missing:\n{text}"
    );
    // Borderless: the popup (the tooltip_bg-filled region) carries no box glyphs.
    let popup = dash_popup(r.frame_buffer());
    for g in [
        '\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}', '\u{2502}', '\u{2500}',
    ] {
        assert!(
            !popup.contains(g),
            "Sources panel must be borderless, found {g}:\n{popup}"
        );
    }
}

// #309 / health-consolidation: a connected row whose cached health summary is
// set (a) shows a per-row ⚠ FLAG (scannable in the list), and (b) shows the full
// reason in the detail line, PREEMPTING the benign "installed at" hint. The
// health string here is glyph-FREE so the only ⚠ in the buffer is the per-row
// flag the painter adds — proving the flag specifically.
#[test]
fn connection_panel_health_flag_and_detail_preempt_the_install_path() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "reasonix",
        label_prefix: "rx",
        display_name: "Reasonix",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.reasonix/settings.json")),
        target: None,
        health: Some("install broken: shim binary missing".into()), // NO ⚠ prefix
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains('\u{26a0}'),
        "the per-row health flag (⚠) must render (health string carries none):\n{text}"
    );
    assert!(
        text.contains("install broken"),
        "the full reason must show in the detail line:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "the health verdict must PREEMPT the install-path hint:\n{text}"
    );
}

#[test]
fn connection_panel_armed_shows_confirm_prompt() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "codex",
        label_prefix: "cx",
        display_name: "Codex",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.codex/config.toml")),
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        Some(0),
        None,
        String::new(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("(y/n)"),
        "armed confirm prompt missing:\n{text}"
    );
    assert!(text.contains("Codex"), "armed target name missing:\n{text}");
}

// Selected row is Disconnected (no health/confirm/result) → the detail line
// surfaces the connect ACTION, not a (meaningless-until-bound) install path.
#[test]
fn connection_panel_disconnected_selected_shows_connect_hint() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "antigravity",
        label_prefix: "ag",
        display_name: "Antigravity",
        state: ConnState::Disconnected,
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("press t to connect"),
        "disconnected detail hint missing:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "a disconnected row must NOT show an install path:\n{text}"
    );
}

// Selected row is NoCli → the detail line is `no_action_hint` = "<name> not
// detected on this machine" (explains why it can't be bound).
#[test]
fn connection_panel_no_cli_selected_shows_not_detected_hint() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "codex",
        label_prefix: "cx",
        display_name: "Codex",
        state: ConnState::NoCli,
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("not detected on this machine"),
        "no-CLI detail hint missing:\n{text}"
    );
    assert!(
        text.contains("Codex"),
        "no-CLI hint must name the CLI:\n{text}"
    );
}

// Selected Connected row WITHOUT a known config_path → detail is the bare
// "connected" fallback, NOT the "installed at: <path>" arm. Distinguishes the
// `None` config_path branch from the `Some(p)` branch (covered elsewhere).
#[test]
fn connection_panel_connected_without_config_path_shows_connected() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "claude",
        label_prefix: "cc",
        display_name: "Claude Code",
        state: ConnState::Connected,
        config_path: None,
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("connected"),
        "connected fallback detail missing:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "no config_path → must NOT show an install path:\n{text}"
    );
}

// `last_result` (a post-action result string) preempts the per-state install
// hint: even with a Connected row whose config_path WOULD render "installed
// at:", the result string wins (it's higher precedence than the per-state arm).
#[test]
fn connection_panel_last_result_overrides_per_state_detail() {
    use crate::tui::connection::{ConnState, ConnectionRow, LiveInfo};
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    let rows = vec![ConnectionRow {
        source_id: "claude",
        label_prefix: "cc",
        display_name: "Claude Code",
        state: ConnState::Connected,
        config_path: Some(std::path::PathBuf::from("~/.claude/settings.json")),
        target: None,
        health: None,
    }];
    r.set_connection_frame_parts(
        true,
        rows,
        vec![LiveInfo::default()],
        0,
        None,
        Some("X-RESULT-SENTINEL".to_string()),
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("X-RESULT-SENTINEL"),
        "last_result string must render in the detail line:\n{text}"
    );
    assert!(
        !text.contains("installed at"),
        "last_result must PREEMPT the per-state install hint:\n{text}"
    );
}

// Empty rows (selected index out of range), no confirm/result → the detail
// `else` branch yields an empty string. The panel still paints its title /
// socket / footer; no stale per-state hint leaks in.
#[test]
fn connection_panel_empty_rows_renders_panel_with_blank_detail() {
    use crate::tui::connection::LiveInfo;
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    r.set_connection_frame_parts(
        true,
        vec![],
        Vec::<LiveInfo>::new(),
        0,
        None,
        None,
        "socket  /tmp/p.sock  (listening)".into(),
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Sources"), "title missing:\n{text}");
    assert!(text.contains("j/k move"), "footer missing:\n{text}");
    assert!(
        !text.contains("installed at"),
        "empty rows → blank detail, no install hint:\n{text}"
    );
    assert!(
        !text.contains("press t to connect"),
        "empty rows → blank detail, no connect hint:\n{text}"
    );
    assert!(
        !text.contains("not detected"),
        "empty rows → blank detail, no no-CLI hint:\n{text}"
    );
}

#[test]
fn dashboard_collapsed_big_tree_shows_badge_and_hides_children() {
    let mut r = build(120, 44, vec![]);
    let root_id = AgentId::from_transcript_path("/h/root.jsonl");
    let mut root = slot(root_id, 0, 0, t0());
    root.label = "cc\u{b7}root".into();
    let mut agents = vec![root];
    // 6 > AUTO_COLLAPSE_THRESHOLD (5) → the root auto-collapses on open.
    for i in 0..6 {
        let cid = AgentId::from_transcript_path(&format!("/h/root/subagents/agent-{i}.jsonl"));
        let mut c = slot(cid, 0, 1 + i, t0());
        c.label = format!("explorer{i}").into();
        c.parent_id = Some(root_id);
        agents.push(c);
    }
    let scene = scene_with(agents, 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, Some(root_id), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(text.contains("cc\u{b7}root"), "root row missing:\n{text}");
    assert!(
        text.contains("(6)"),
        "collapsed hidden-count badge missing:\n{text}"
    );
    // The popup's own count proves only the root is listed (children hidden);
    // a global `!contains("explorer0")` would false-fail on the office sprite
    // label behind the popup. Child-hiding in the model is covered by
    // `dashboard::tests::root_over_threshold_auto_collapses_and_hides_its_subtree`.
    assert!(
        text.contains("Agents (1)"),
        "collapsed tree must list exactly one row:\n{text}"
    );
}

#[test]
fn dashboard_closed_paints_no_popup() {
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![idle("/h/a.jsonl", 0, t0())], 16);
    r.set_dashboard_frame_parts(false, Vec::new(), None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let text = frame_text(r.frame_buffer());
    assert!(
        !text.contains("Agents ("),
        "no dashboard popup when closed:\n{text}"
    );
}

/// The popup's box-bordered content lines (those containing the vertical rule),
/// so substring assertions don't false-match the office sprite labels behind it.
fn dash_popup(buf: &ratatui::buffer::Buffer) -> String {
    // The popup is borderless (no `│` to key on) but is the only region painted
    // with the UI `tooltip_bg` fill, so isolate it by background color. (The
    // pixel office never produces this exact chrome RGB.)
    let tb = pixtuoid_scene::theme::NORMAL.ui.tooltip_bg;
    let bg = ratatui::style::Color::Rgb(tb.r, tb.g, tb.b);
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        let mut row = String::new();
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell((x, y)) {
                if cell.bg == bg {
                    row.push_str(cell.symbol());
                }
            }
        }
        if !row.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&row);
        }
    }
    out
}

#[test]
fn dashboard_renders_waiting_reason_and_active_without_detail() {
    let mut r = build(120, 44, vec![]);
    let mut w = idle("/h/w.jsonl", 0, t0());
    w.label = "cc\u{b7}wait".into();
    w.state = ActivityState::Waiting {
        reason: Arc::from("permission"),
    };
    let mut a = idle("/h/a.jsonl", 1, t0());
    a.label = "cc\u{b7}act".into();
    a.state = ActivityState::Active {
        tool_use_id: Some(Arc::from("t")),
        detail: None,
        kind: ToolKind::Other,
    };
    let scene = scene_with(vec![w, a], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("waiting: permission"),
        "waiting reason missing:\n{popup}"
    );
    assert!(
        popup.contains("\u{25cf} active"),
        "active-without-detail must render the bare state word:\n{popup}"
    );
}

#[test]
fn dashboard_scrolls_to_keep_a_deep_selection_visible() {
    // 20 roots (> DASHBOARD_VIEWPORT_ROWS = 16) spread across floors so the
    // office only labels a couple — the popup lists all 20. Selecting row 18
    // must scroll the window down (painter re-clamps via clamp_scroll), so the
    // popup shows row 18 and NOT row 00.
    let mut agents = Vec::new();
    for i in 0..20 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let row18 = rows[18].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(row18), 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let buf = r.frame_buffer();
    let text = frame_text(buf);
    let popup = dash_popup(buf);
    // The title sits on the top border (┌…┐), not a │ row — assert it on the full frame.
    assert!(text.contains("Agents (20)"), "all 20 rows counted:\n{text}");
    assert!(
        popup.contains("row18"),
        "deep selection scrolled into view (painter re-clamps scroll to the real window):\n{popup}"
    );
    assert!(
        !popup.contains("row00"),
        "top row must scroll off when a deep row is selected:\n{popup}"
    );
}

#[test]
fn dashboard_empty_scene_shows_placeholder() {
    let mut r = build(120, 44, vec![]);
    let scene = scene_with(vec![], 16);
    r.set_dashboard_frame_parts(true, Vec::new(), None, 0);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("No active agents"),
        "empty dashboard must show the placeholder"
    );
}

// NOTE: a dedicated short-terminal clamp test isn't possible via the harness —
// draw_scene footer-onlys (skips the popup) when the office layout can't fit
// (terminal shorter than ~the office min height), and a 16-row popup (max 18
// tall) always fits whenever the office DOES render. So the painter's
// real-window re-clamp (clamp_scroll on visible = popup-inner-height) can only
// fire with visible == DASHBOARD_VIEWPORT_ROWS in practice — exercised by
// dashboard_scrolls_to_keep_a_deep_selection_visible above (scroll 0 → 3). The
// visible < viewport arithmetic is covered directly by
// dashboard::tests::clamp_scroll_* with small viewports.

#[test]
fn dashboard_badge_text_present_for_cc_and_cx() {
    let mut r = build(120, 44, vec![]);
    let mut cc_slot = idle("/h/cc.jsonl", 0, t0());
    cc_slot.source = Arc::from("claude-code");
    cc_slot.label = "cc\u{b7}alpha".into();
    let mut cx_slot = idle("/h/cx.jsonl", 1, t0());
    cx_slot.source = Arc::from("codex");
    cx_slot.label = "cx\u{b7}beta".into();
    let scene = scene_with(vec![cc_slot, cx_slot], 16);

    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(popup.contains("[cc]"), "cc badge missing:\n{popup}");
    assert!(popup.contains("[cx]"), "cx badge missing:\n{popup}");
}

#[test]
fn dashboard_overflow_cue_appears_below_when_more_than_viewport() {
    // 20 root slots, scroll=0 → visible ≤ DASHBOARD_VIEWPORT_ROWS=16 → hidden_below > 0.
    let mut agents = Vec::new();
    for i in 0..20 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i % 16, t0());
        s.label = format!("overflow{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 32);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains('\u{22ee}'),
        "overflow cue ⋮ must appear:\n{popup}"
    );
}

#[test]
fn dashboard_overflow_cue_absent_when_all_visible() {
    // 8 root slots ≤ DASHBOARD_VIEWPORT_ROWS=16 → no hidden rows → no cue.
    let mut agents = Vec::new();
    for i in 0..8 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("fit{i:02}").into();
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, None, 0);
    r.render(&scene, &pack(), t0()).unwrap();

    let popup = dash_popup(r.frame_buffer());
    assert!(
        !popup.contains('\u{22ee}'),
        "no overflow cue for 8 rows:\n{popup}"
    );
}

#[test]
fn dashboard_overflow_cue_keeps_a_bottom_navigated_selection_visible() {
    // 25 rows; select row20 — clamp_scroll parks it at the window's bottom with
    // several rows still below. The cue must NOT displace the selected row.
    let mut agents = Vec::new();
    for i in 0..25 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let row20 = rows[20].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(row20), 0);
    r.render(&scene, &pack(), t0()).unwrap();
    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("row20"),
        "selected bottom row must stay visible when a cue shows:\n{popup}"
    );
    assert!(
        popup.contains('\u{22ee}'),
        "overflow cue present (rows below):\n{popup}"
    );
}

#[test]
fn dashboard_overflow_no_blank_line_when_selection_is_last_row() {
    // 17 rows (> DASHBOARD_VIEWPORT_ROWS=16) → overflow. Selecting the LAST row
    // scrolls to the very end where nothing is below: no cue is needed, so the
    // popup must fill all 16 visible lines (NOT reserve a now-empty cue line).
    let mut agents = Vec::new();
    for i in 0..17 {
        let mut s = idle(&format!("/h/r{i}.jsonl"), i, t0());
        s.label = format!("row{i:02}").into();
        s.floor_idx = i % 10;
        agents.push(s);
    }
    let scene = scene_with(agents, 16);
    let rows = build_dashboard_rows(&scene, &DashboardFolds::default());
    let last = rows[16].agent_id;
    let mut r = build(120, 44, vec![]);
    r.set_dashboard_frame_parts(true, rows, Some(last), 0);
    r.render(&scene, &pack(), t0()).unwrap();
    let popup = dash_popup(r.frame_buffer());
    assert!(
        popup.contains("row16"),
        "selected last row visible:\n{popup}"
    );
    // The fix renders 16 rows (rows 1..16); the blank-line bug renders only 15
    // (rows 2..16, plus a blank reserved line), so `row01` present distinguishes.
    assert!(
        popup.contains("row01"),
        "all 16 visible lines must be filled — no blank reserved cue line:\n{popup}"
    );
    assert!(
        !popup.contains('\u{22ee}'),
        "no cue when scrolled to the end (nothing below):\n{popup}"
    );
}
