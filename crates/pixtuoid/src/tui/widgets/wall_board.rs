use std::time::SystemTime;

use pixtuoid_core::state::DaemonState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{display_width, to_color, StateCounts};
use crate::tui::renderer::clip_widget_rect;

/// The wall board's text width, DERIVED from the painted neon panel's dark
/// interior so the lit sign's letters can never overrun the glowing frame
/// (laying text to the full outer `NEON_PANEL_W` overran it). Only the
/// horizontal derives — the vertical is a half-block 2:1 coordinate system, so
/// the 3-row height and the `+1` cell row stay literal.
pub(super) const BOARD_W: u16 = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W;

/// The board text's top-left terminal cell = the neon panel's dark interior
/// origin. BOTH `paint_wall_display` and `star_hit_rect` read THIS one helper,
/// so the painted text and the click target share an origin.
fn board_cell_origin(scene_rect: Rect) -> (u16, u16) {
    (
        scene_rect.x + pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X,
        scene_rect.y + 1,
    )
}

/// The tone→role map is the ONE authority in `scene::board`; this only converts
/// the resolved `Rgb` to a ratatui `Color`.
fn board_tone_color(
    tone: pixtuoid_scene::board::BoardTone,
    theme: &pixtuoid_scene::theme::Theme,
) -> Color {
    to_color(pixtuoid_scene::board::tone_rgb(tone, theme))
}

/// The in-scene neon wall board — the office's "lit sign": brand + ★ CTA (L1),
/// the mood pulse (L2), the office context row (L3). It owns nothing critical
/// exclusively, since it may clip off-screen; the must-not-miss signals live in
/// the footer.
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

    let model = pixtuoid_scene::board::build_board(
        counts,
        pixtuoid_scene::board::scene_uptime_secs(scene, now),
        floor_info.map(|fi| (fi.current, fi.total_floors)),
        gateway,
    );

    // The star right-flushes to the panel edge — the SAME position
    // `star_hit_rect` derives the click target from. The assert is STRICT (`<`)
    // so the NATURAL gap is already ≥1, making `.max(1)` a no-op and keeping
    // paint == hit-rect: at the exact-fit boundary `.max(1)` would shove the
    // star one col past the hit-rect and clip it.
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
/// SAME board geometry the L1 painter uses, so the click target can't drift from
/// the painted star into a phantom launch.
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

    fn row_text(buf: &ratatui::buffer::Buffer, x: u16, y: u16, width: u16) -> String {
        (0..width).map(|dx| buf[(x + dx, y)].symbol()).collect()
    }

    #[test]
    fn wall_board_renders_the_three_model_lines_over_the_panel() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // Uptime reads the scene, empty here → "<1m". A gateway + no floor
        // exercises the L3 chip and the single-floor (no breadcrumb) context.
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
        assert!(l1.starts_with("pixtuoid v"), "brand leads L1: {l1:?}");
        assert!(
            l1.trim_end().ends_with("\u{2605} Star"),
            "star right-flushed: {l1:?}"
        );
        assert!(
            l2.contains("\u{25b2}1 wait")
                && l2.contains("\u{25cf}2 work")
                && l2.contains("\u{25cb}1 idle"),
            "mood pulse: {l2:?}"
        );
        assert!(l3.contains("\u{2191}<1m"), "uptime: {l3:?}");
        assert!(l3.contains("\u{2b22}gw ok"), "gateway chip: {l3:?}");
        assert!(
            !l3.contains('F'),
            "no floor breadcrumb when floor_info is None: {l3:?}"
        );
    }

    #[test]
    fn star_hit_rect_fits_and_truncates() {
        let star_w = display_width(pixtuoid_scene::board::BOARD_STAR) as u16;
        let inner_x = pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_X;
        let star_x = inner_x + BOARD_W - star_w;
        let wide = star_hit_rect(full_bounds(120, 44)).expect("star fits");
        assert_eq!(
            (wide.x, wide.y, wide.width, wide.height),
            (star_x, 1, star_w, 1)
        );
        assert!(wide.x + wide.width <= 120, "clipped within the scene");
        assert!(
            wide.x + wide.width <= inner_x + BOARD_W,
            "star must land inside the panel interior"
        );
        let narrow = star_hit_rect(full_bounds(star_x + 2, 44)).expect("partial star");
        assert_eq!(narrow.width, 2, "clipped to the 2 visible cols");
        // Too narrow to show any of the star ⇒ no click target, no phantom launch.
        assert!(star_hit_rect(full_bounds(star_x, 44)).is_none());
    }
}
