//! Keyboard-shortcut help overlay. Toggled by '?'; dismissed by Enter / Esc / '?'.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::{centered_in, to_color};
use crate::tui::theme::Theme;

const SHORTCUTS: &[(&str, &str)] = &[
    ("q", "quit"),
    ("Ctrl+C", "quit"),
    ("p", "pause / resume"),
    ("t", "themes"),
    ("w", "walkable / approach / route debug"),
    ("?", "toggle this overlay"),
    ("\u{2191} \u{2193} j k", "switch floor"),
    ("PgUp / PgDn", "switch floor"),
    ("click agent", "pin tooltip"),
    ("Enter / Esc", "dismiss popup"),
];

pub(in crate::tui) fn paint_help_overlay(f: &mut ratatui::Frame<'_>, bounds: Rect, theme: &Theme) {
    let area = centered_in(bounds, 36, SHORTCUTS.len() as u16 + 4);
    if area.width < 4 || area.height < 3 {
        return;
    }
    f.render_widget(Clear, area);

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
    let block = Block::default()
        .title(Span::styled(
            " ? Keyboard ",
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(to_color(theme.ui.neon_brand)))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));
    f.render_widget(Paragraph::new(lines).block(block), area);
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
        for (w, h) in [(200, 60), (40, 20), (24, 30), (10, 4), (4, 3)] {
            render_at(w, h);
        }
    }
}
