//! The shared borderless modal frame for every popup. Delegates the card backing
//! (drop shadow + `Clear` + solid bg fill) to `super::paint_card_backing` — the
//! ONE definition shared with the framed tooltips — then adds a uniform pad and
//! an optional bold inner title line, and returns the inner content `Rect` the
//! caller paints into. NO border (readability over the busy pixel office without
//! the outline). Used by help / version / theme picker / dashboard / connection.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{paint_card_backing, to_color};
use pixtuoid_scene::theme::Theme;

/// Uniform inner padding for every borderless popup — the breathing room that
/// stands in for the removed border. Shared (re-exported via `widgets`) so the
/// version-popup click-rect math derives its offsets from the SAME constants the
/// painter insets by, and can't drift.
pub(crate) const PANEL_PAD_X: u16 = 2;
pub(crate) const PANEL_PAD_Y: u16 = 1;

/// Paint a borderless panel over `area`: `Clear`, a solid background fill, a
/// uniform `PANEL_PAD_*` inset, and — when `title` is set and there's room — a
/// bold brand-colored title line at the top of the padded region. Returns the
/// content `Rect` (the padded region, below the title row when one is drawn). No
/// borders are ever drawn; the bg fill + `Clear` + padding keep text legible and
/// off the panel edges.
pub(crate) fn borderless_panel(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    title: Option<&str>,
    theme: &Theme,
) -> Rect {
    // Shared backing: drop shadow + Clear + solid bg fill (the padding region is
    // bg, not blank). The title row below re-uses the same fill.
    paint_card_backing(f, area, theme);
    let bg = Style::default().bg(to_color(theme.ui.tooltip_bg));
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
            borderless_panel(
                f,
                Rect::new(0, 0, w, h),
                title,
                &pixtuoid_scene::theme::NORMAL,
            );
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
                &pixtuoid_scene::theme::NORMAL,
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
            inner = borderless_panel(
                f,
                Rect::new(0, 0, 20, 6),
                None,
                &pixtuoid_scene::theme::NORMAL,
            );
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

    /// `borderless_panel` (via the shared `paint_card_backing`) darkens the office
    /// cells in the L-band below-right of the popup (right strip fading inner→
    /// outer), and leaves cells outside the band untouched. Pre-fills the buffer
    /// with a known bright color to stand in for the already-flushed office, then
    /// renders a small inset panel.
    #[test]
    fn borderless_panel_casts_a_drop_shadow_with_falloff() {
        use ratatui::style::Color;
        let bright = Color::Rgb(200, 200, 200);
        let area = Rect::new(5, 4, 8, 4); // small, well inside the 20x12 buffer
        let mut term = Terminal::new(TestBackend::new(20, 12)).unwrap();
        term.draw(|f| {
            let full = f.area();
            for y in 0..full.height {
                for x in 0..full.width {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    cell.set_symbol("\u{2580}");
                    cell.fg = bright;
                    cell.bg = bright;
                }
            }
            borderless_panel(f, area, None, &pixtuoid_scene::theme::NORMAL);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let chan = |c: Color| match c {
            Color::Rgb(r, _, _) => r,
            other => panic!("expected Rgb, got {other:?}"),
        };
        // `r` reads a cell's lower sub-pixel (bg); `rf` its upper one (fg). The
        // right band darkens the whole cell; the bottom band only the upper half.
        let r = |x: u16, y: u16| chan(buf.cell((x, y)).unwrap().bg);
        let rf = |x: u16, y: u16| chan(buf.cell((x, y)).unwrap().fg);
        // Right strip darkens, and the inner column is darker than the outer (the
        // penumbra falloff).
        let inner = r(area.right(), area.y + 1);
        let outer = r(area.right() + 1, area.y + 1);
        assert!(
            inner < 200,
            "right band inner column must darken (got {inner})"
        );
        assert!(
            inner < outer,
            "shadow must fade inner→outer (inner {inner} !< outer {outer})"
        );
        // Bottom strip is a 1px contact shadow: its upper sub-pixel (fg) darkens
        // and hugs the card's base, while the lower sub-pixel (bg) stays lit so it
        // never reads as a thick band set off below the popup.
        assert!(
            rf(area.x + 1, area.bottom()) < 200,
            "bottom band upper sub-pixel must darken (the contact shadow)"
        );
        assert_eq!(
            r(area.x + 1, area.bottom()),
            200,
            "bottom band lower sub-pixel stays lit (1px contact shadow, not a 2px band)"
        );
        // The shadow is a cast offset, not an outline: the right strip starts one
        // row BELOW the top edge, so the top-right corner stays lit (the strip
        // never runs the card's full height), and the bottom strip starts one
        // column right of the left edge, so the bottom-left corner stays lit.
        assert_eq!(
            r(area.right(), area.y),
            200,
            "top-right corner stays lit — right strip is offset down, not full-height"
        );
        assert_eq!(
            rf(area.x, area.bottom()),
            200,
            "bottom-left corner stays lit — bottom strip is offset right"
        );
        // …but the right strip reaches one row below the card, so it still meets
        // the bottom strip at the bottom-right corner (the cast connects there).
        assert!(
            r(area.right(), area.bottom()) < 200,
            "bottom-right corner is shadowed (right strip meets the bottom strip)"
        );
        // A cell far from the band is untouched.
        assert_eq!(r(0, 0), 200, "cells outside the band stay bright");
        // The card's top-LEFT stays lit — the shadow is cast down-right only, not
        // an all-sides outline.
        assert_eq!(
            r(area.x.saturating_sub(1), area.y),
            200,
            "no shadow on the lit (top-left) side"
        );
    }
}
