//! Keyboard-shortcut help overlay. Toggled by '?'; dismissed by Enter / Esc / '?'.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{paint_panel, to_color, Overflow};
use pixtuoid_scene::theme::Theme;

const SHORTCUTS: &[(&str, &str)] = &[
    ("q", "quit"),
    ("Ctrl+C", "quit"),
    ("p", "pause / resume"),
    // Audio rows only exist on audio-capable builds (Linux prebuilts ship
    // without the feature — advertising a dead key reads as broken). Both
    // descriptions fit the 21-col budget (36 inner - 2 indent - 13 key col).
    #[cfg(feature = "audio")]
    ("m", "sound on/off"),
    #[cfg(feature = "audio")]
    ("+/-", "volume; + unmutes"),
    ("t", "themes"),
    ("Tab", "agent dashboard"),
    ("s", "sources (connect / health)"),
    // Dev-only overlay — hidden from release-build help (see dispatch_key).
    #[cfg(debug_assertions)]
    ("w", "walkable / approach / route debug"),
    ("?", "toggle this overlay"),
    ("\u{2191} \u{2193} j k", "switch floor"),
    ("PgUp / PgDn", "switch floor"),
    ("click agent", "focus its terminal"),
    ("f (dashboard)", "focus selected agent's terminal"),
    ("Enter / Esc", "dismiss popup"),
];

pub(crate) fn paint_help_overlay(f: &mut ratatui::Frame<'_>, bounds: Rect, theme: &Theme) {
    /// Content width: the 13-col key column + the widest description.
    const HELP_W: u16 = 36;
    // A lead-blank then one row per shortcut. `paint_panel` adds the title + pad,
    // auto-heights to the actual rows, and windows-with-cue on a short terminal.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(SHORTCUTS.len() + 1);
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
    paint_panel(
        f,
        theme,
        Some("? Keyboard"),
        bounds,
        HELP_W,
        1.0,
        vec![],
        lines,
        vec![],
        Overflow::CueOnly,
    );
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
            paint_help_overlay(f, Rect::new(0, 0, w, h), &pixtuoid_scene::theme::NORMAL);
        })
        .unwrap();
    }

    #[test]
    fn help_overlay_renders_without_panic_across_sizes() {
        // (2,2): PanelGeometry::compute (via paint_panel) guards away below 4×3
        // → nothing paints — must not panic on the degenerate sizes.
        for (w, h) in [(200, 60), (40, 20), (24, 30), (10, 4), (4, 3), (2, 2)] {
            render_at(w, h);
        }
    }
}
