use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::to_color;

pub(crate) fn paint_elevator_indicator(
    f: &mut ratatui::Frame<'_>,
    door: pixtuoid_scene::layout::Point,
    current_floor: usize,
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let label = format!(" \u{25b2} F{current_floor} \u{25bc} ");
    // Display COLUMNS via `display_width` (the width authority, shared with the
    // footer): the ▲/▼ arrows are 3-byte single-column glyphs (byte len over-counts).
    let label_w = super::display_width(&label) as u16;
    let door_cell_x = door.x + 8u16.saturating_sub(label_w / 2);
    let door_cell_y = door.y / 2;
    let indicator_y = door_cell_y.saturating_sub(1);

    if let Some(r) = crate::tui::renderer::clip_widget_rect(
        Rect {
            x: scene_rect.x + door_cell_x,
            y: scene_rect.y + indicator_y,
            width: label_w,
            height: 1,
        },
        scene_rect,
    ) {
        let style = Style::default()
            .fg(to_color(theme.ui.neon_brand))
            .bg(to_color(theme.ui.tooltip_bg))
            .add_modifier(Modifier::BOLD);
        f.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The elevator indicator must center by DISPLAY COLUMNS, not byte length:
    // " ▲ F1 ▼ " is 8 columns but 12 bytes (the arrows are 3-byte single-column
    // glyphs), so a byte-length anchor `door.x + 8 - w/2` lands 2 cells left of
    // the door's center.
    #[test]
    fn elevator_indicator_centers_by_display_columns_not_bytes() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let theme = &pixtuoid_scene::theme::NORMAL;
        let door = pixtuoid_scene::layout::Point { x: 20, y: 10 };
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| {
            paint_elevator_indicator(f, door, 1, Rect::new(0, 0, 80, 30), theme);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let row = (door.y / 2 - 1) as usize; // indicator paints one cell above the door
        let bg = to_color(theme.ui.tooltip_bg);
        let cols: Vec<u16> = (0..80u16)
            .filter(|&x| buf.content()[row * 80 + x as usize].style().bg == Some(bg))
            .collect();
        assert_eq!(
            cols.len(),
            " \u{25b2} F1 \u{25bc} ".chars().count(),
            "label must paint exactly its display-column width"
        );
        assert_eq!(
            cols.first(),
            Some(&(door.x + 8 - cols.len() as u16 / 2)),
            "label must center on the 16-px door (door.x + 8)"
        );
    }
}
