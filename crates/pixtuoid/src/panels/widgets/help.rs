use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{Overflow, Panel, to_color};
use pixtuoid_scene::theme::Theme;

/// A key and what it does, one help row.
pub(crate) type Shortcut = (&'static str, &'static str);

const SHORTCUTS: &[Shortcut] = &[
    ("q", "quit"),
    ("Ctrl+C", "quit"),
    ("p", "pause / resume"),
    #[cfg(feature = "audio")]
    ("m", "sound on/off"),
    #[cfg(feature = "audio")]
    ("+/-", "volume; + unmutes"),
    ("t", "themes"),
    ("Tab", "agent dashboard"),
    ("s", "sources (connect / health)"),
    #[cfg(debug_assertions)]
    ("w", "walkable / approach / route debug"),
    ("?", "toggle this overlay"),
    ("\u{2191} \u{2193} j k", "switch floor"),
    ("PgUp / PgDn", "switch floor"),
    ("click agent", "focus its terminal"),
    ("f (dashboard)", "focus selected agent's terminal"),
    ("Enter / Esc", "dismiss popup"),
];

const ROW_INDENT: &str = "  ";

/// The widest key plus a one-space gutter, so even a full-width key keeps a gap
/// before its description instead of running into it.
fn key_col_width(host: &[Shortcut]) -> usize {
    SHORTCUTS
        .iter()
        .chain(host)
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        + 1
}

/// DERIVED from `SHORTCUTS`, never a literal — a hardcoded width hard-clips
/// every long row mid-word at ANY terminal size (the panel is centered, not
/// edge-limited).
fn content_width(host: &[Shortcut]) -> u16 {
    let widest_desc = SHORTCUTS
        .iter()
        .chain(host)
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(0);
    (ROW_INDENT.chars().count() + key_col_width(host) + widest_desc) as u16
}

/// The shared shortcuts, then `host`'s own.
pub(crate) fn paint_help_overlay(
    f: &mut ratatui::Frame<'_>,
    bounds: Rect,
    theme: &Theme,
    host: &[Shortcut],
) {
    let key_col = key_col_width(host);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(SHORTCUTS.len() + host.len() + 1);
    lines.push(Line::from(""));
    for (key, desc) in SHORTCUTS.iter().chain(host) {
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
    Panel {
        title: Some("? Keyboard"),
        content_w: content_width(host),
        above: vec![],
        list: lines,
        below: vec![],
        overflow: Overflow::CueOnly,
    }
    .paint(f, bounds, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_at(w: u16, h: u16) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            paint_help_overlay(
                f,
                Rect::new(0, 0, w, h),
                &pixtuoid_scene::theme::NORMAL,
                &[],
            );
        })
        .unwrap();
    }

    fn frame_text(w: u16, h: u16, host: &[Shortcut]) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            paint_help_overlay(
                f,
                Rect::new(0, 0, w, h),
                &pixtuoid_scene::theme::NORMAL,
                host,
            );
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

    #[test]
    fn every_shortcut_row_renders_in_full() {
        let text = frame_text(140, 40, &[]);
        let key_col = key_col_width(&[]);
        for (key, desc) in SHORTCUTS {
            let row = format!("{ROW_INDENT}{key:<key_col$}{desc}");
            assert!(
                text.contains(&row),
                "shortcut row clipped or gutter lost: {row:?}\nframe:\n{text}"
            );
        }
    }

    /// A host's own shortcuts list after the shared ones, in the same key
    /// column, which widens to the widest; a host with none lists none.
    #[test]
    fn a_hosts_shortcuts_list_after_the_shared_ones() {
        let host: &[Shortcut] = &[("a-much-wider-key-than-any", "host only")];
        let text = frame_text(160, 50, host);
        let key_col = key_col_width(host);
        assert!(
            key_col > key_col_width(&[]),
            "the column widens to the host's key"
        );
        for (key, desc) in SHORTCUTS.iter().chain(host) {
            let row = format!("{ROW_INDENT}{key:<key_col$}{desc}");
            assert!(text.contains(&row), "row {row:?} missing:\n{text}");
        }
        assert!(!frame_text(160, 50, &[]).contains("host only"));
    }

    #[test]
    fn help_overlay_renders_without_panic_across_sizes() {
        // Once the footer row is reserved, the last two sizes fall under
        // `PanelGeometry`'s minimum envelope and paint nothing.
        for (w, h) in [(200, 60), (40, 20), (24, 30), (10, 4), (4, 3), (2, 2)] {
            render_at(w, h);
        }
    }
}
