//! The shared borderless modal frame for every popup. Renders `Clear` + a
//! solid-bg `Block` with NO border — readability over the busy pixel office
//! without the outline — plus an optional bold inner title line, and returns the
//! inner content `Rect` the caller paints into. Replaces every popup's bordered
//! Block (help / version / theme picker / dashboard / tooltips / connection).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::to_color;
use crate::tui::theme::Theme;

/// Uniform inner padding for every borderless popup — the breathing room that
/// stands in for the removed border. Shared (re-exported via `widgets`) so the
/// version-popup click-rect math derives its offsets from the SAME constants the
/// painter insets by, and can't drift.
pub(in crate::tui) const PANEL_PAD_X: u16 = 2;
pub(in crate::tui) const PANEL_PAD_Y: u16 = 1;

/// Paint a borderless panel over `area`: `Clear`, a solid background fill, a
/// uniform `PANEL_PAD_*` inset, and — when `title` is set and there's room — a
/// bold brand-colored title line at the top of the padded region. Returns the
/// content `Rect` (the padded region, below the title row when one is drawn). No
/// borders are ever drawn; the bg fill + `Clear` + padding keep text legible and
/// off the panel edges.
pub(in crate::tui) fn borderless_panel(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    title: Option<&str>,
    theme: &Theme,
) -> Rect {
    f.render_widget(Clear, area);
    let bg = Style::default().bg(to_color(theme.ui.tooltip_bg));
    // Solid background over the FULL area (the padding region is bg, not blank).
    f.render_widget(Block::default().style(bg), area);
    // Too small to pad — hand back the raw area rather than underflow.
    if area.width <= PANEL_PAD_X * 2 || area.height <= PANEL_PAD_Y * 2 {
        return area;
    }
    let mut inner = Rect {
        x: area.x + PANEL_PAD_X,
        y: area.y + PANEL_PAD_Y,
        width: area.width - PANEL_PAD_X * 2,
        height: area.height - PANEL_PAD_Y * 2,
    };
    if let Some(t) = title {
        if inner.height >= 1 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    t.to_string(),
                    Style::default()
                        .fg(to_color(theme.ui.neon_brand))
                        .add_modifier(Modifier::BOLD),
                )))
                .style(bg),
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: 1,
                },
            );
            inner.y += 1;
            inner.height -= 1;
        }
    }
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_to_string(w: u16, h: u16, title: Option<&str>) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            borderless_panel(f, Rect::new(0, 0, w, h), title, &crate::tui::theme::NORMAL);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..h {
            for x in 0..w {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn borderless_panel_has_no_border_glyphs_and_renders_title() {
        let s = render_to_string(40, 8, Some("Connection"));
        for g in ['╭', '╮', '╰', '╯', '│', '─', '┌', '┐', '└', '┘'] {
            assert!(!s.contains(g), "panel must be borderless, found {g:?}");
        }
        assert!(s.contains("Connection"), "title must render in the body");
    }

    #[test]
    fn borderless_panel_returns_padded_inner_below_the_title_row() {
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        let mut inner = Rect::default();
        term.draw(|f| {
            inner = borderless_panel(
                f,
                Rect::new(0, 0, 20, 6),
                Some("X"),
                &crate::tui::theme::NORMAL,
            );
        })
        .unwrap();
        // PAD_X each side; PAD_Y top + the title row above the content.
        assert_eq!(inner.x, PANEL_PAD_X);
        assert_eq!(
            inner.y,
            PANEL_PAD_Y + 1,
            "content starts below the title row"
        );
        assert_eq!(inner.width, 20 - PANEL_PAD_X * 2);
        assert_eq!(inner.height, 6 - PANEL_PAD_Y * 2 - 1);
        // Untitled: the padded region with no title row.
        term.draw(|f| {
            inner = borderless_panel(f, Rect::new(0, 0, 20, 6), None, &crate::tui::theme::NORMAL);
        })
        .unwrap();
        assert_eq!(inner.y, PANEL_PAD_Y);
        assert_eq!(inner.height, 6 - PANEL_PAD_Y * 2);
    }

    #[test]
    fn borderless_panel_never_panics_across_sizes() {
        for (w, h) in [(80, 20), (40, 8), (10, 3), (4, 2), (2, 1)] {
            let _ = render_to_string(w, h, Some("T"));
            let _ = render_to_string(w, h, None);
        }
    }
}
