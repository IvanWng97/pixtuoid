//! End-to-end headless floor-switch harness. Drives the real
//! `TuiRenderer` (via ratatui `TestBackend`) through the actual
//! `navigate_floor` → transition → `render` path — the production wiring
//! that the unit-level `advance_wander` tests can't reach — and asserts an
//! off-screen floor freezes while hidden and resyncs (no replay) on return.
use super::*;
use crate::tui::layout::Point;
use crate::tui::pet::PetKind;
use pixtuoid_core::state::{ActivityState, AgentSlot, SceneState};
use pixtuoid_core::AgentId;
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
        label: Arc::from("a"),
        state: ActivityState::Idle,
        state_started_at: started,
        created_at: started,
        last_event_at: started,
        exiting_at: None,
        pending_idle_at: None,
        desk_index,
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
    crate::tui::embedded_pack::load_sprite_pack(None).expect("embedded pack")
}
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}
fn normal_theme() -> &'static crate::tui::theme::Theme {
    crate::tui::theme::theme_by_name("normal").expect("normal theme")
}
fn dark_theme() -> &'static crate::tui::theme::Theme {
    crate::tui::theme::theme_by_name("cyberpunk").expect("cyberpunk theme")
}
/// Build a renderer with the given pet KINDS, each using its default name.
fn build(cols: u16, rows: u16, kinds: Vec<PetKind>) -> TuiRenderer<TestBackend> {
    build_pets(
        cols,
        rows,
        kinds
            .into_iter()
            .map(crate::tui::pet::Pet::defaulted)
            .collect(),
    )
}
/// Build a renderer with fully-specified pets (kind + custom name).
fn build_pets(cols: u16, rows: u16, pets: Vec<crate::tui::pet::Pet>) -> TuiRenderer<TestBackend> {
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
    for y in y0..(y0 + h).min(buf.height) {
        for x in x0..(x0 + w).min(buf.width) {
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
    for y in y0..(y0 + h).min(a.height).min(b.height) {
        for x in x0..(x0 + w).min(a.width).min(b.width) {
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
    let pack = crate::tui::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let theme = crate::tui::theme::ALL_THEMES[0];
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
        .map(|ms| ms.last_advanced_at)
        .expect("floor-0 motion present");

    // ~30 s on floor 1 — floor 0 must NOT be advanced.
    for _ in 0..900 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack, now).expect("render");
    }
    let still_frozen = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .map(|ms| ms.last_advanced_at)
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
    // one transition per frame. wander_phase_started_at would be far in the
    // past if it replayed.
    let ms = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .expect("floor-0 motion present");
    assert!(
            ms.wander_phase_started_at >= back_at,
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
    let d = region_diff(&before, r.buf(), 0, 0, before.width, before.height);
    assert!(
        d > 5_000,
        "switching to a different theme must recolor the floor (diff={d})"
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
    let on_diff = region_diff(&before, &on, 0, 0, before.width, before.height);
    assert!(
        on_diff > 1_000,
        "the debug layer must visibly change the frame"
    );

    // Toggle OFF → the scene returns to the un-overlaid frame (additive layer).
    r.set_debug_walkable(false);
    r.render(&scene, &pack(), now).unwrap();
    let off_diff = region_diff(&before, r.buf(), 0, 0, before.width, before.height);
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
    let early = avg_lum(r.buf(), 0, 0, r.buf().width, r.buf().height);
    for _ in 0..700 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).unwrap();
    }
    let late = avg_lum(r.buf(), 0, 0, r.buf().width, r.buf().height);
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
    assert!(r.coffee_holders_contains(id));
    // Agent gone from the scene ⇒ next render evicts its coffee state.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .unwrap();
    assert!(
        !r.coffee_holders_contains(id),
        "coffee state must be evicted when the agent leaves (no leak)"
    );
}

#[test]
fn coffee_persists_through_floor_transition() {
    // Regression: render_transition_floor discarded its render_to_rgb_buffer
    // result (`let _ =`), so a coffee carrier first DETECTED during a floor
    // slide was never persisted into coffee_holders → the cup never landed.
    // The normal path persists via DrawCtx.new_coffee_carriers; the transition
    // path now threads the same Vec back.
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
            if scratch.coffee_holders_contains(id) {
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
        !r.coffee_holders_contains(agent),
        "agent must not yet hold coffee before the transition"
    );
    r.navigate_floor(1, t);
    assert!(r.transition().is_some(), "navigation begins a transition");
    r.render(&scene, &p, detect_at).unwrap();
    assert!(
        r.coffee_holders_contains(agent),
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
                        crate::tui::pathfind::point_in_walkable_cell(&layout.walkable, pos),
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
    use crate::tui::layout::WaypointKind;
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
    s.label = Arc::from("ZQXLBL");
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
    let cat = crate::tui::pet::Pet {
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
    let d = region_diff(&baseline, r.buf(), 0, 0, baseline.width, baseline.height);
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
        for y in 0..(buf.height / 4).max(1) {
            for x in (0..buf.width).step_by(7) {
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
                crate::tui::layout::WaypointKind::MeetingSofa
                    | crate::tui::layout::WaypointKind::MeetingStand
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
            if crate::tui::chitchat::CHITCHAT_LINES
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
    use crate::tui::layout::{
        Layout, PlantKind, PodDecor, WallDecor, WaypointKind, MAX_VISIBLE_DESKS,
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
        let layout = Layout::compute_with_seed(160, 200, MAX_VISIBLE_DESKS, seed)
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
        if !layout.meeting_sofas.is_empty() {
            assert!(labels.contains("Meeting Sofa"), "seed {seed}: Meeting Sofa");
        }
        if !layout.meeting_tables.is_empty() {
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
    s.label = Arc::from("HOVERME");
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
    a.label = Arc::from("LEAVING");
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
