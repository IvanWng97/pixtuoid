//! Keyboard-shortcut help overlay. Toggled by '?'; dismissed by Enter / Esc / '?'.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{borderless_panel, centered_in, to_color};
use crate::tui::theme::Theme;

const SHORTCUTS: &[(&str, &str)] = &[
    ("q", "quit"),
    ("Ctrl+C", "quit"),
    ("p", "pause / resume"),
    ("t", "themes"),
    ("Tab", "agent dashboard"),
    ("s", "sources (connect / health)"),
    // Dev-only overlay — hidden from release-build help (see dispatch_key).
    #[cfg(debug_assertions)]
    ("w", "walkable / approach / route debug"),
    ("?", "toggle this overlay"),
    ("\u{2191} \u{2193} j k", "switch floor"),
    ("PgUp / PgDn", "switch floor"),
    ("click agent", "pin tooltip"),
    ("Enter / Esc", "dismiss popup"),
];

pub(in crate::tui) fn paint_help_overlay(f: &mut ratatui::Frame<'_>, bounds: Rect, theme: &Theme) {
    // Borderless: a title row + 1 lead-blank + the shortcut rows (no top/bottom
    // border). Title is drawn by `borderless_panel`; content fills below it.
    let area = centered_in(
        bounds,
        36 + 2 * super::PANEL_PAD_X,
        SHORTCUTS.len() as u16 + 2 + 2 * super::PANEL_PAD_Y,
    );
    if area.width < 4 || area.height < 3 {
        return;
    }
    let inner = borderless_panel(f, area, Some("? Keyboard"), theme);

    let mut lines: Vec<Line> = Vec::with_capacity(SHORTCUTS.len() + 1);
    lines.push(Line::from(""));
    for (key, desc) in SHORTCUTS {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{key:<13}"),
                Style::default()
                    .fg(to_color(theme.ui.neon_brand))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                desc.to_string(),
                Style::default().fg(to_color(theme.ui.label_idle)),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // The overlay renders Clear + a Block; assert it never panics across the
    // full size range, including narrow/short buffers reachable on small
    // terminals (width clamp + bounds-origin centering must hold).
    fn render_at(w: u16, h: u16) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            paint_help_overlay(f, Rect::new(0, 0, w, h), &crate::tui::theme::NORMAL);
        })
        .unwrap();
    }

    #[test]
    fn help_overlay_renders_without_panic_across_sizes() {
        // (2,2): centered_in clamps the area to 2×2, tripping the width<4
        // early return — must still render Clear-less without panic.
        for (w, h) in [(200, 60), (40, 20), (24, 30), (10, 4), (4, 3), (2, 2)] {
            render_at(w, h);
        }
    }
}
