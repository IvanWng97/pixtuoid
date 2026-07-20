use std::time::SystemTime;

use pixtuoid_core::state::DaemonState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{display_width, to_color, StateCounts};
use crate::tui::renderer::clip_widget_rect;

/// The wall board's text width + cell-origin pin to the painted neon panel's dark
/// INTERIOR (spine 2), so the lit sign's letters can never overrun the glowing
/// frame — the `PANTRY_COFFEE_COLS_*` anti-drift precedent. `NEON_PANEL_INNER_W` =
/// the outer panel minus its `NEON_PANEL_BORDER` on each side (laying text to the
/// full outer `NEON_PANEL_W` overran the frame — the board-overflow bug). Only the
/// horizontal derives; the 3-row height + the `+1` cell ROW stay literal (the
/// half-block 2:1 vertical is a different coordinate system — C2).
pub(super) const BOARD_W: u16 = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W;

/// The board text's top-left terminal cell = the neon panel's dark interior origin
/// (`NEON_PANEL_INNER_X` px, 1:1 with cells; the `+1` row is the half-block 2:1
/// vertical, kept literal — C2). BOTH `paint_wall_display` and `star_hit_rect`
/// read THIS one helper, so the painted text and the click target share an origin.
fn board_cell_origin(scene_rect: Rect) -> (u16, u16) {
    (
        scene_rect.x + pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X,
        scene_rect.y + 1,
    )
}

/// Map a backend-agnostic `BoardTone` to this theme's ratatui color. Mirrors the
/// footer's role→color map so the board's tones (brand/star/state/dim/gateway)
/// resolve to the SAME hues the footer's `FooterTone` uses.
fn board_tone_color(
    tone: pixtuoid_scene::board::BoardTone,
    theme: &pixtuoid_scene::theme::Theme,
) -> Color {
    // The tone→role map is the ONE authority in `scene::board`; this painter only
    // converts the resolved `Rgb` to ratatui `Color`.
    to_color(pixtuoid_scene::board::tone_rgb(tone, theme))
}

/// The in-scene neon wall board — the office's "lit sign": brand + ★ CTA (L1), the
/// mood pulse echoing the shared counts (L2), and the office context row (L3:
/// uptime + floor + gateway chip). It owns nothing critical exclusively (it may
/// clip off-screen); the must-not-miss signals live in the footer. `counts` is the
/// SAME `scene_stats` the footer reads (spine 1); `floor_info`/`gateway` are the
/// always-present office-wide `DrawCtx` fields (C1). The scrolling ticker is gone.
#[allow(clippy::too_many_arguments)] // a painter's distinct inputs (like paint_footer)
pub(crate) fn paint_wall_display(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    scene_rect: Rect,
    now: SystemTime,
    counts: StateCounts,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    gateway: Option<DaemonState>,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let (cell_x, cell_y) = board_cell_origin(scene_rect);

    // The board's TEXT is the backend-agnostic `pixtuoid_scene::board` model (so
    // floating/wasm build the SAME content); this painter only maps each tone to a
    // ratatui color + owns the cell-space L1 right-flush.
    let model = pixtuoid_scene::board::build_board(
        counts,
        pixtuoid_scene::board::scene_uptime_secs(scene, now),
        floor_info.map(|fi| (fi.current, fi.total_floors)),
        gateway,
    );

    // L1 — brand + ★ Star CTA, the star right-flushed to the panel edge so its
    // left edge lands at `cell_x + BOARD_W - star_w` — the SAME position
    // `star_hit_rect` derives the click target from. The `.max(1)` floor keeps a
    // ≥1-col gap; the assert is STRICT (`<`) so the NATURAL gap is already ≥1,
    // making `.max(1)` a no-op and paint == hit-rect. At the exact-fit boundary
    // (`brand+star == BOARD_W`) `.max(1)` would shove the star one col past the
    // hit-rect (and clip it), so `<` forbids that boundary rather than `<=`.
    let star_w = display_width(&model.star.text);
    let gap = (BOARD_W as usize)
        .saturating_sub(display_width(&model.brand.text) + star_w)
        .max(1);
    debug_assert!(
        display_width(&model.brand.text) + star_w < BOARD_W as usize,
        "brand+star must STRICTLY fit the panel (natural gap ≥1) for the right-flush = star_hit_rect pairing"
    );
    let top_line = Line::from(vec![
        Span::styled(
            model.brand.text.clone(),
            Style::default()
                .fg(board_tone_color(model.brand.tone, theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(gap)),
        Span::styled(
            model.star.text.clone(),
            Style::default()
                .fg(board_tone_color(model.star.tone, theme))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    // L2 — the mood pulse (shared counts, ▲ beacon leads); L3 — office context
    // (uptime · floor · gateway chip). Both are just tone-mapped model segments.
    let styled = |segs: &[pixtuoid_scene::board::BoardSegment]| -> Vec<Span<'static>> {
        segs.iter()
            .map(|s| {
                Span::styled(
                    s.text.clone(),
                    Style::default().fg(board_tone_color(s.tone, theme)),
                )
            })
            .collect()
    };
    let mood_line = Line::from(styled(&model.mood));
    let ctx_line = Line::from(styled(&model.context));

    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: BOARD_W,
            height: 3,
        },
        scene_rect,
    ) {
        f.render_widget(Paragraph::new(vec![top_line, mood_line, ctx_line]), r);
    }
}

/// The precise screen rect of the board's `★ Star` CTA span, clipped to the
/// scene (`None` when it clips away on a very narrow terminal). Derived from the
/// SAME board geometry the L1 painter uses — `cell_x = scene.x + 2`, `cell_y =
/// scene.y + 1`, and the right-flush to `BOARD_W` — so the click target can't
/// drift from the painted star (the phantom-launch class the version-popup
/// url-rect also guards). Replaces the loose `hit_test_branding` (cols `1..31`),
/// which fired anywhere on the top-left row (C9).
pub(crate) fn star_hit_rect(scene_rect: Rect) -> Option<Rect> {
    let (cell_x, cell_y) = board_cell_origin(scene_rect);
    let star_w = display_width(pixtuoid_scene::board::BOARD_STAR) as u16;
    let star_x = cell_x + BOARD_W.saturating_sub(star_w);
    clip_widget_rect(
        Rect {
            x: star_x,
            y: cell_y,
            width: star_w,
            height: 1,
        },
        scene_rect,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_bounds(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    // Read the `width` cells starting at `(x, y)` back as a string — the rendered
    // text of one board row.
    fn row_text(buf: &ratatui::buffer::Buffer, x: u16, y: u16, width: u16) -> String {
        (0..width).map(|dx| buf[(x + dx, y)].symbol()).collect()
    }

    #[test]
    fn wall_board_renders_the_three_model_lines_over_the_panel() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // The mood reads `counts` (passed separately, like production's scene_stats);
        // uptime reads the scene, empty here → "<1m". A gateway + no floor exercises
        // the L3 chip and the single-floor (no breadcrumb) context.
        let counts = StateCounts {
            active: 2,
            waiting: 1,
            idle: 1,
            exiting: 0,
            total: 4,
        };
        let scene = SceneState::uniform(16);
        let scene_rect = full_bounds(120, 44);
        let mut term = Terminal::new(TestBackend::new(120, 44)).unwrap();
        term.draw(|f| {
            paint_wall_display(
                f,
                &scene,
                scene_rect,
                SystemTime::UNIX_EPOCH,
                counts,
                None,
                Some(DaemonState::Idle),
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let (cx, cy) = board_cell_origin(scene_rect);
        let l1 = row_text(buf, cx, cy, BOARD_W);
        let l2 = row_text(buf, cx, cy + 1, BOARD_W);
        let l3 = row_text(buf, cx, cy + 2, BOARD_W);
        // L1: brand left, ★ Star right-flushed.
        assert!(l1.starts_with("pixtuoid v"), "brand leads L1: {l1:?}");
        assert!(
            l1.trim_end().ends_with("\u{2605} Star"),
            "star right-flushed: {l1:?}"
        );
        // L2: the mood pulse, beacon leading.
        assert!(
            l2.contains("\u{25b2}1 wait")
                && l2.contains("\u{25cf}2 work")
                && l2.contains("\u{25cb}1 idle"),
            "mood pulse: {l2:?}"
        );
        // L3: uptime + the ⬢gw chip (no floor breadcrumb on a single floor).
        assert!(l3.contains("\u{2191}<1m"), "uptime: {l3:?}");
        assert!(l3.contains("\u{2b22}gw ok"), "gateway chip: {l3:?}");
        assert!(
            !l3.contains('F'),
            "no floor breadcrumb when floor_info is None: {l3:?}"
        );
    }

    #[test]
    fn star_hit_rect_fits_and_truncates() {
        let star_w = display_width(pixtuoid_scene::board::BOARD_STAR) as u16; // "★ Star" == 6 cols
                                                                              // cell_x = the panel INTERIOR origin; the star right-flushes to the
                                                                              // interior's right edge, which must land INSIDE the outer frame.
        let inner_x = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X;
        let star_x = inner_x + BOARD_W - star_w;
        let wide = star_hit_rect(full_bounds(120, 44)).expect("star fits");
        assert_eq!(
            (wide.x, wide.y, wide.width, wide.height),
            (star_x, 1, star_w, 1)
        );
        assert!(wide.x + wide.width <= 120, "clipped within the scene");
        // The star's right edge sits at or before the panel's inner-right edge, so
        // it never spills onto/past the glowing frame (the overflow bug).
        assert!(
            wide.x + wide.width <= inner_x + BOARD_W,
            "star must land inside the panel interior"
        );
        // A cramped scene truncates the span to its visible columns.
        let narrow = star_hit_rect(full_bounds(star_x + 2, 44)).expect("partial star");
        assert_eq!(narrow.width, 2, "clipped to the 2 visible cols");
        // Too narrow to show any of the star ⇒ no click target (no phantom launch).
        assert!(star_hit_rect(full_bounds(star_x, 44)).is_none());
    }
}
