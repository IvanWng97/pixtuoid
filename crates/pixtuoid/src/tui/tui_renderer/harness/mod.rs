//! End-to-end headless floor-switch harness. Drives the real
//! `TuiRenderer` (via ratatui `TestBackend`) through the actual
//! `navigate_floor` → transition → `render` path — the production wiring
//! that the unit-level `advance_wander` tests can't reach — and asserts an
//! off-screen floor freezes while hidden and resyncs (no replay) on return.
use super::*;
use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, SceneState, ToolKind};
use pixtuoid_core::AgentId;
use pixtuoid_scene::pet::PetKind;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn slot(
    id: AgentId,
    floor_idx: usize,
    desk_index: usize,
    started: SystemTime,
) -> AgentSlot {
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
        pid: None,
        model: None,
        effort: None,
        tokens_used: 0,
        last_usage: None,
    }
}

pub(super) fn render_until_settled<B: Backend<Error: Send + Sync + 'static>>(
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

pub(super) fn pack() -> Pack {
    pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack")
}
pub(super) fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}
pub(super) fn normal_theme() -> &'static pixtuoid_scene::theme::Theme {
    pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme")
}
pub(super) fn dark_theme() -> &'static pixtuoid_scene::theme::Theme {
    pixtuoid_scene::theme::theme_by_name("cyberpunk").expect("cyberpunk theme")
}
/// Build a renderer with the given pet KINDS, each using its default name.
pub(super) fn build(cols: u16, rows: u16, kinds: Vec<PetKind>) -> TuiRenderer<TestBackend> {
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
pub(super) fn build_pets(
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
pub(super) fn idle(id: &str, desk: usize, started: SystemTime) -> AgentSlot {
    slot(AgentId::from_transcript_path(id), 0, desk, started)
}
/// Active (typing) agent with a tool `detail`.
pub(super) fn active(id: &str, desk: usize, detail: &str, started: SystemTime) -> AgentSlot {
    let mut s = idle(id, desk, started);
    s.state = ActivityState::Active {
        tool_use_id: Some(Arc::from("t")),
        detail: Some(Arc::from(detail)),
        kind: ToolKind::from_display(detail),
    };
    s.last_event_at = started;
    s
}
pub(super) fn scene_with(agents: Vec<AgentSlot>, cap: usize) -> SceneState {
    let mut s = SceneState::uniform(cap);
    for a in agents {
        s.agents.insert(a.agent_id, a);
    }
    s
}
/// Flatten the ratatui frame into one newline-joined string for substring
/// assertions on rendered text (footer, tooltips, overlays).
pub(super) fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
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
pub(super) fn lum(c: pixtuoid_core::sprite::Rgb) -> f32 {
    0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32
}
/// Average luminance over a rectangle of the RGB buffer (clamped to bounds).
pub(super) fn avg_lum(buf: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> f32 {
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
pub(super) fn region_diff(a: &RgbBuffer, b: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> u64 {
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

// Two-floor scene builder, shared across the floors/render_text/overlays
// sub-modules (kept here rather than localized to any one).
pub(super) fn two_floor_scene() -> SceneState {
    let cap = 16;
    scene_with(
        vec![
            idle("/n/0.jsonl", 0, t0() - Duration::from_secs(120)),
            slot(AgentId::from_transcript_path("/n/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    )
}

mod audio;
mod coffee_pets;
mod dashboard;
mod edge_cases;
mod floors;
mod hit_test;
mod mascot;
mod overlays;
mod render_text;
mod theme_lighting;

/// Drive the PRODUCTION hover path programmatically: scan the rendered layout
/// for a cell whose agent hit-test resolves to `id` and park the mouse there
/// (the dossier-content tests' injection seam — the click-to-pin setter they
/// used before the focus-jump handover is gone; hover is the only dossier
/// trigger now, and this exercises the same `set_mouse_pos` production wires).
/// Panics when the agent isn't hit-testable — a test wiring error, not a case.
pub(super) fn hover_agent(
    r: &mut TuiRenderer<TestBackend>,
    scene: &pixtuoid_core::SceneState,
    id: pixtuoid_core::AgentId,
    cols: u16,
    rows: u16,
) {
    let layout = r.cached_layout().expect("rendered layout").clone();
    for my in 0..rows {
        for mx in 0..cols {
            if crate::tui::hit_test::hit_test_from_tui(scene, &layout, mx, my) == Some(id) {
                r.set_mouse_pos(Some((mx, my)));
                return;
            }
        }
    }
    panic!("agent {id:?} is not hit-testable in this layout");
}
