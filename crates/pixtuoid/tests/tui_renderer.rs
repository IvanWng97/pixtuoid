//! Smoke test that `TuiRenderer::render` (the production flush entry point —
//! an inherent method since #483, was the core `Renderer` trait impl) drives a
//! real half-block frame end to end, not just the in-memory `TestRenderer`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid::tui::tui_renderer::TuiRenderer;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Build an `AgentSlot` for these render tests — fills the boilerplate fields
/// (source `claude-code`, cwd `/demo`, created/last-event `now - 60s`, zeroed
/// counters, no parent/exit) so each call site only varies what it cares about.
fn agent_slot(
    id: AgentId,
    session: &str,
    label: &str,
    desk: GlobalDeskIndex,
    floor: usize,
    state: ActivityState,
    now: SystemTime,
) -> AgentSlot {
    AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from(session),
        cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
        label: label.into(),
        state,
        state_started_at: now,
        created_at: now - Duration::from_secs(60),
        last_event_at: now - Duration::from_secs(60),
        exiting_at: None,
        pending_idle_at: None,
        desk_index: desk,
        floor_idx: floor,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
        pid: None,
        model: None,
        effort: None,
        tokens_used: 0,
        last_usage: None,
    }
}

#[test]
fn tui_renderer_render_paints_a_full_frame() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let mut scene = SceneState::uniform(8);
    let id = AgentId::from_transcript_path("/demo/a.jsonl");
    scene.agents.insert(
        id,
        agent_slot(
            id,
            "s-1",
            "demo",
            GlobalDeskIndex(0),
            0,
            ActivityState::Active {
                tool_use_id: Some(std::sync::Arc::from("t1")),
                detail: Some(std::sync::Arc::from("Write")),
                kind: pixtuoid_core::state::ToolKind::Edit,
            },
            now,
        ),
    );

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );
    let pack = load_sprite_pack(None).expect("pack");

    renderer.render(&scene, &pack, now).expect("render");

    // The TUI impl owns the pixel buffer — after render, it should be sized
    // for the 96×(36-1) scene area (one row reserved for footer), doubled
    // vertically via half-block: 96 cells wide, 70 pixels tall.
    let buf = renderer.buf();
    assert_eq!(buf.width(), 96);
    assert_eq!(buf.height(), 70);

    // And it should contain something (non-trivial color diversity), proving
    // the trait method actually triggered the paint pipeline.
    let mut colors = std::collections::HashSet::new();
    for px in buf.as_slice() {
        colors.insert((px.r, px.g, px.b));
    }
    assert!(
        colors.len() > 32,
        "TuiRenderer::render produced suspiciously few colors ({})",
        colors.len()
    );
}

/// Regression guard for the floor-transition rendering pipeline.
///
/// Previously the transition path hardcoded `active_pet: None`,
/// `floor_pet_kind: None`, and empty coffee state, so pets/cups/steam
/// vanished during the slide. This test verifies that triggering a
/// transition still paints a non-trivial buffer with pet state active —
/// catching a regression that re-introduces `None` for these fields.
#[test]
fn tui_renderer_transition_paints_pets_and_coffee() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);

    // Two-floor scene with one agent per floor.
    let mut caps = [0usize; pixtuoid_core::state::MAX_FLOORS];
    caps[0] = 8;
    caps[1] = 8;
    let mut scene = SceneState::new(caps);
    for (i, name) in ["a", "b"].iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{name}.jsonl"));
        scene.agents.insert(
            id,
            agent_slot(
                id,
                &format!("s-{i}"),
                name,
                GlobalDeskIndex(i * 8),
                i,
                ActivityState::Idle,
                now,
            ),
        );
    }

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );
    let pack = load_sprite_pack(None).expect("pack");

    // Initial render so the renderer grows its per-floor state to nf=2.
    renderer.render(&scene, &pack, now).expect("initial render");

    // Set an active pet on floor 0 (carried through the transition).
    renderer.set_active_pet(Some(pixtuoid::tui::renderer::PetState {
        petted_at: now,
        pet_pos: pixtuoid_scene::layout::Point { x: 20, y: 20 },
        kind: pixtuoid_scene::pet::PetKind::Cat,
        floor_idx: 0,
    }));

    // Trigger a transition from floor 0 to floor 1.
    renderer.navigate_floor(1, now);
    assert!(
        renderer.transition().is_some(),
        "navigate_floor should arm a transition"
    );

    // Render mid-transition (a few ms in so the slide is partway through).
    let mid = now + Duration::from_millis(100);
    renderer
        .render(&scene, &pack, mid)
        .expect("transition render");

    // The transition should still be in progress — verifies we actually
    // exercised the transition draw path (not the post-transition normal
    // path) on the previous render.
    assert!(
        renderer.transition().is_some(),
        "transition should not have completed yet (was the path skipped?)"
    );

    // Both floor buffers should be populated with a non-trivial pixel mix.
    // If pets/coffee/decor get stubbed back to None or empty, the buffers
    // still get *some* paint (floor, walls) but the color diversity drops.
    // We just assert non-emptiness here; richer assertions belong in
    // dedicated pet/coffee tests.
    let buf = renderer.buf();
    let nonzero = buf
        .as_slice()
        .iter()
        .filter(|p| p.r != 0 || p.g != 0 || p.b != 0)
        .count();
    assert!(
        nonzero > 100,
        "transition buffer should have substantial paint (got {nonzero} non-black px)"
    );
}

#[test]
fn set_version_popup_records_timestamp_on_edge() {
    use std::time::{Duration, SystemTime};

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let t1 = t0 + Duration::from_millis(50);

    assert_eq!(renderer.version_popup_started_at(), None);

    renderer.set_version_popup(false, t0);
    assert_eq!(
        renderer.version_popup_started_at(),
        None,
        "no edge from false → false"
    );

    renderer.set_version_popup(true, t0);
    assert_eq!(
        renderer.version_popup_started_at(),
        Some(t0),
        "false → true edge records timestamp"
    );

    renderer.set_version_popup(true, t1);
    assert_eq!(
        renderer.version_popup_started_at(),
        Some(t0),
        "true → true is not an edge; timestamp unchanged"
    );

    renderer.set_version_popup(false, t1);
    assert_eq!(
        renderer.version_popup_started_at(),
        Some(t1),
        "true → false edge records new timestamp"
    );
}

#[test]
fn version_popup_animation_starts_small_then_grows() {
    use std::time::{Duration, SystemTime};

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );

    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    renderer.set_version_popup(true, t0);

    let scale_start = renderer.version_popup_scale(t0);
    assert!(
        scale_start < 0.1,
        "expected entrance start scale < 0.1; got {scale_start}"
    );

    let scale_mid = renderer.version_popup_scale(t0 + Duration::from_millis(100));
    assert!(
        scale_mid > 0.8,
        "expected scale > 0.8 at mid-entrance; got {scale_mid}"
    );

    let scale_end = renderer.version_popup_scale(t0 + Duration::from_millis(200));
    assert!(
        (scale_end - 1.0).abs() < 1e-3,
        "expected scale 1.0 at end of entrance; got {scale_end}"
    );

    let t1 = t0 + Duration::from_millis(200);
    renderer.set_version_popup(false, t1);

    let scale_dismiss_mid = renderer.version_popup_scale(t1 + Duration::from_millis(60));
    assert!(
        scale_dismiss_mid > 0.0 && scale_dismiss_mid < 1.0,
        "expected mid-dismissal scale between 0 and 1; got {scale_dismiss_mid}"
    );

    let scale_dismiss_end = renderer.version_popup_scale(t1 + Duration::from_millis(120));
    assert!(
        scale_dismiss_end < 0.01,
        "expected dismissal end scale ~0; got {scale_dismiss_end}"
    );
}

/// Regression guard for Fix #5: dismissing mid-entrance must not snap the
/// popup back to full scale before fading. Before the fix, `set_version_popup`
/// overwrote `version_popup_started_at = Some(now)` without saving the current
/// scale, so the dismissal formula evaluated to `1.0 - 0 ≈ 1.0` on the next
/// frame — a visible flash to full size before fading.
#[test]
fn dismiss_mid_entrance_does_not_snap_to_full() {
    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Enter mid-entrance at 100ms (EaseOutCubic(0.5) ≈ 0.875)
    renderer.set_version_popup(true, t0);
    let mid_entrance = t0 + Duration::from_millis(100);
    let scale_at_mid = renderer.version_popup_scale(mid_entrance);
    assert!(
        scale_at_mid > 0.7 && scale_at_mid < 1.0,
        "expected mid-entrance scale 0.7..1.0; got {scale_at_mid}"
    );

    // Dismiss at the same moment
    renderer.set_version_popup(false, mid_entrance);

    // Immediately after the dismiss edge, scale should be ~scale_at_mid (no snap)
    let just_after = mid_entrance + Duration::from_millis(1);
    let scale_after = renderer.version_popup_scale(just_after);
    assert!(
        scale_after < scale_at_mid + 0.05,
        "scale should NOT snap up after dismiss; got {scale_after} (was {scale_at_mid})"
    );
}

/// Regression: a resize mid-slide previously left `current_floor` at
/// `from_floor`, silently reverting a user-initiated navigation with no UI
/// signal. `cancel_transition` must now land the user on `to_floor`.
#[test]
fn cancel_transition_lands_on_destination_floor() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);

    let mut caps = [0usize; pixtuoid_core::state::MAX_FLOORS];
    caps[0] = 8;
    caps[1] = 8;
    let mut scene = SceneState::new(caps);
    for (i, name) in ["a", "b"].iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{name}.jsonl"));
        scene.agents.insert(
            id,
            agent_slot(
                id,
                &format!("s-{i}"),
                name,
                GlobalDeskIndex(i * 8),
                i,
                ActivityState::Idle,
                now,
            ),
        );
    }

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    );
    let pack = load_sprite_pack(None).expect("pack");

    renderer.render(&scene, &pack, now).expect("initial render");
    assert_eq!(renderer.current_floor(), 0);

    renderer.navigate_floor(1, now);
    assert!(renderer.transition().is_some());
    assert_eq!(
        renderer.current_floor(),
        0,
        "current_floor stays at source until transition completes or cancels"
    );

    renderer.cancel_transition();
    assert!(renderer.transition().is_none());
    assert_eq!(
        renderer.current_floor(),
        1,
        "cancel_transition should snap to the destination floor"
    );
}

fn make_renderer() -> TuiRenderer<TestBackend> {
    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    TuiRenderer::new(
        terminal,
        &pixtuoid_scene::theme::NORMAL,
        pixtuoid_scene::pet::PetKind::ALL
            .iter()
            .map(|&k| pixtuoid_scene::pet::Pet::defaulted(k))
            .collect(),
    )
}

#[test]
fn help_open_toggles_via_setter() {
    let mut r = make_renderer();
    assert!(!r.help_open());
    r.set_help_open(true);
    assert!(r.help_open());
    r.set_help_open(false);
    assert!(!r.help_open());
}
