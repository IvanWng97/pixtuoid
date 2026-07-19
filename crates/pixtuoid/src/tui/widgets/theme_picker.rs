use super::{paint_panel, to_color, Overflow};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// The two colors that characterize a theme in the picker swatch: its
/// accent (`neon_brand`) and its dominant office surface (`carpet_base`).
fn theme_swatch(t: &pixtuoid_scene::theme::Theme) -> (Color, Color) {
    (to_color(t.ui.neon_brand), to_color(t.surface.carpet_base))
}

pub(crate) fn paint_theme_picker(
    f: &mut ratatui::Frame<'_>,
    selected: usize,
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    use pixtuoid_scene::theme;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};

    /// Name column (12) + the leading selection marker + the 2-cell swatch.
    const THEME_W: u16 = 28;
    let items: Vec<Line<'static>> = theme::ALL_THEMES
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let prefix = if i == selected { "\u{25b8} " } else { "  " };
            let name_style = if i == selected {
                Style::default()
                    .fg(to_color(theme.ui.neon_brand))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(to_color(theme.ui.label_idle))
            };
            // Each row previews the theme it would switch to via a 2-cell
            // swatch (accent + office floor), so the picker reads visually
            // rather than by name alone.
            let (brand, surface) = theme_swatch(t);
            Line::from(vec![
                TSpan::styled(format!("{prefix}{:<12}", t.name), name_style),
                TSpan::raw(" "),
                TSpan::styled("\u{2588}", Style::default().fg(brand)),
                TSpan::styled("\u{2588}", Style::default().fg(surface)),
            ])
        })
        .collect();
    paint_panel(
        f,
        theme,
        Some("Theme [\u{2191}\u{2193}/jk] Enter/Esc"),
        bounds,
        THEME_W,
        1.0,
        vec![],
        items,
        vec![],
        Overflow::Follow {
            selected: Some(selected),
            scroll: 0,
            cap: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: paint_theme_picker rendered Clear onto an unclamped
    // 28-wide area; on a narrower buffer (reachable via the gate-less
    // floor-transition paint path) Clear panics indexing past the buffer.
    #[test]
    fn theme_picker_narrow_terminal_does_not_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(24, 30)).unwrap();
        term.draw(|f| {
            paint_theme_picker(
                f,
                0,
                Rect::new(0, 0, 24, 30),
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        // Reaching here without a panic is the assertion.
    }

    #[test]
    fn theme_swatch_distinguishes_themes() {
        use pixtuoid_scene::theme;
        // Each theme's (accent, surface) pair should reflect that theme's
        // own palette, not the currently-active one — so the picker rows
        // preview distinct colors.
        let cyber = theme_swatch(&theme::CYBERPUNK);
        let normal = theme_swatch(&theme::NORMAL);
        assert_ne!(
            cyber, normal,
            "distinct themes must yield distinct swatches"
        );
        assert_eq!(cyber.0, to_color(theme::CYBERPUNK.ui.neon_brand));
        assert_eq!(cyber.1, to_color(theme::CYBERPUNK.surface.carpet_base));
    }
}
