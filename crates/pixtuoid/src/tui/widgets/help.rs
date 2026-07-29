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
    // without the feature — advertising a dead key reads as broken).
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

/// Row indent, shared by the painted row and the width measure so the panel can
/// never be sized narrower than the rows it frames.
const ROW_INDENT: &str = "  ";

/// The key column: the widest key plus a one-space gutter, so even a full-width
/// key (`f (dashboard)`) keeps a gap before its description instead of running
/// into it.
fn key_col_width() -> usize {
    SHORTCUTS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        + 1
}

/// Content width the panel reserves: indent + key column + the widest
/// description. DERIVED from `SHORTCUTS`, never a literal — a hardcoded width
/// goes stale the moment a shortcut is added and hard-clips every long row
/// mid-word at ANY terminal size (the panel is centered, not edge-limited).
fn content_width() -> u16 {
    let widest_desc = SHORTCUTS
        .iter()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(0);
    (ROW_INDENT.chars().count() + key_col_width() + widest_desc) as u16
}

pub(crate) fn paint_help_overlay(f: &mut ratatui::Frame<'_>, bounds: Rect, theme: &Theme) {
    let key_col = key_col_width();
    // A lead-blank then one row per shortcut. `paint_panel` adds the title + pad,
    // auto-heights to the actual rows, and windows-with-cue on a short terminal.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(SHORTCUTS.len() + 1);
    lines.push(Line::from(""));
    for (key, desc) in SHORTCUTS {
        lines.push(Line::from(vec![
            Span::raw(ROW_INDENT),
            Span::styled(
                format!("{key:<key_col$}"),
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
        content_width(),
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

    fn frame_text(w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            paint_help_overlay(f, Rect::new(0, 0, w, h), &pixtuoid_scene::theme::NORMAL);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(c) = buf.cell((x, y)) {
                    out.push_str(c.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    // The panel is CENTERED, so a content width narrower than the widest row
    // clips every long shortcut mid-word at ANY terminal size — not just narrow
    // ones. Render generously and demand every row survive intact, gutter and all.
    #[test]
    fn every_shortcut_row_renders_in_full() {
        let text = frame_text(140, 40);
        let key_col = key_col_width();
        for (key, desc) in SHORTCUTS {
            let row = format!("{ROW_INDENT}{key:<key_col$}{desc}");
            assert!(
                text.contains(&row),
                "shortcut row clipped or gutter lost: {row:?}\nframe:\n{text}"
            );
        }
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
