use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use super::{to_color, PANEL_PAD_X, PANEL_PAD_Y};

/// The project repository — opened when the board's ★ Star CTA is clicked.
/// `pub` (not `pub(crate)`): the BIN crate's crash reporter derives its
/// issue-report URL from this same authority — crash.rs is a `main.rs` module,
/// a separate crate the lib's `pub(crate)` can't reach (and the pixtuoid lib
/// target is not a semver surface, so the widening is free).
pub const REPO_URL: &str = "https://github.com/IvanWng97/pixtuoid";
/// URL shown on the version popup's "More details" line and opened on click:
/// `REPO_URL` + `/releases`. Kept a full literal (const &str can't `concat!`);
/// the two are pinned together by `version_popup_url_is_repo_releases`.
pub(crate) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";

/// Prefix rendered before the URL. Its byte-length determines the URL's
/// click-rect x-offset; keep `paint_version_popup` and
/// `version_popup_url_rect` consistent by using this constant.
const URL_PREFIX: &str = "  More details: ";

/// The scaled, bounds-clamped, centered envelope Rect of the version popup.
/// Single source of truth for `paint_version_popup` (which paints into it) and
/// `version_popup_url_rect` (which derives the URL click-rect off it): clamp
/// w_full/h_full to `bounds` BEFORE scaling, then floor the scaled dims at 2.
/// `scale` must already be clamped to `0.0..=1.0` by the caller.
fn version_popup_envelope(bounds: Rect, notes_len: usize, scale: f32) -> Rect {
    // Borderless: no side-border columns, but the shared `borderless_panel`
    // insets content by PANEL_PAD_* — so the envelope must include 2× pad on each
    // axis. Content rows = title + blank + notes + blank + url + 1 slack.
    let needed_w = URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
    let w_full = needed_w.min(bounds.width);
    let h_full = (notes_len as u16 + 5 + 2 * PANEL_PAD_Y).min(bounds.height);
    let w = ((w_full as f32 * scale).round() as u16).max(2);
    let h = ((h_full as f32 * scale).round() as u16).max(2);
    let x = bounds.x + bounds.width.saturating_sub(w) / 2;
    let y = bounds.y + bounds.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

pub(crate) fn paint_version_popup(
    f: &mut ratatui::Frame<'_>,
    version: &str,
    notes: &[&str],
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
    scale: f32,
) {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};

    let scale = scale.clamp(0.0, 1.0);
    if scale <= 0.01 {
        return; // fully dismissed, skip render
    }
    let area = version_popup_envelope(bounds, notes.len(), scale);

    let mut items: Vec<Line> = Vec::with_capacity(notes.len() + 3);
    items.push(Line::from(""));
    for note in notes {
        items.push(Line::from(TSpan::styled(
            format!("  \u{00b7} {note}"),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    items.push(Line::from(""));
    items.push(Line::from(vec![
        TSpan::styled(
            URL_PREFIX,
            Style::default().fg(to_color(theme.ui.label_idle)),
        ),
        TSpan::styled(
            VERSION_POPUP_URL,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));

    let title = format!("What's new in v{version} \u{2014} Enter to close");
    let inner = super::borderless_panel(f, area, Some(&title), theme);
    f.render_widget(Paragraph::new(items), inner);
}

/// Computes the screen rect of the clickable URL inside the version popup.
/// Returns None if the popup would be too small to render. Mirrors the
/// geometry inside `paint_version_popup` (kept in sync by sharing the same
/// width calculation).
pub(crate) fn version_popup_url_rect(notes_len: usize, bounds: Rect, scale: f32) -> Option<Rect> {
    let scale = scale.clamp(0.0, 1.0);
    if scale < 0.7 {
        return None; // URL not clickable until popup reaches 70% scale
    }
    // Mirror paint_version_popup's geometry exactly by deriving from the same
    // shared envelope (clamp-to-bounds-then-scale, centered off the SCALED
    // w/h). Centering off the unscaled w/h leaves the click rect offset from
    // the painted popup at any scale < 1.0.
    let Rect {
        x: popup_x,
        y: popup_y,
        width: w,
        height: h,
    } = version_popup_envelope(bounds, notes_len, scale);
    if w < 4 || h < 3 {
        return None;
    }
    // URL line layout inside the borderless, PANEL_PAD_*-padded popup:
    //   y = popup_y + PAD_Y + 1 (title) + 1 (blank) + notes_len + 1 (blank)
    //   x = popup_x + PAD_X + URL_PREFIX.len()
    // The title row replaces the old top border; the pad shifts both offsets.
    let url_y = popup_y + PANEL_PAD_Y + notes_len as u16 + 3;
    let url_x = popup_x + PANEL_PAD_X + URL_PREFIX.len() as u16;

    // Clip against the popup's PADDED content area: when the painter clipped the
    // envelope (narrow / short terminal), the URL rect must shrink too — otherwise
    // clicks past the visible popup register as URL clicks.
    let inner_right = popup_x + w - PANEL_PAD_X; // content right edge (exclusive)
    let inner_bottom = popup_y + h - PANEL_PAD_Y; // content bottom edge (exclusive)
    if url_x >= inner_right || url_y >= inner_bottom {
        return None;
    }
    let width = (VERSION_POPUP_URL.len() as u16).min(inner_right - url_x);
    if width == 0 {
        return None;
    }
    Some(Rect {
        x: url_x,
        y: url_y,
        width,
        height: 1,
    })
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

    #[test]
    fn version_popup_url_is_repo_releases() {
        // The two URL consts can't `concat!` (const &str), so pin them here — the
        // version popup opens the repo's releases, the ★ CTA opens the repo root.
        assert_eq!(VERSION_POPUP_URL, format!("{REPO_URL}/releases"));
    }

    #[test]
    fn url_rect_fits_inside_normal_popup() {
        let rect = version_popup_url_rect(4, full_bounds(200, 60), 1.0).expect("should fit");
        assert_eq!(rect.width, VERSION_POPUP_URL.len() as u16);
        assert_eq!(rect.height, 1);
    }

    // Regression for the phantom-browser-launch bug: on a narrow terminal
    // the painter clips the popup envelope, but the URL click rect used to
    // extend past the visible popup's right edge, registering clicks on the
    // scene behind as URL clicks. The rect must stay inside the envelope.
    #[test]
    fn url_rect_does_not_extend_past_clipped_popup_right_edge() {
        let bounds = full_bounds(50, 30);
        if let Some(rect) = version_popup_url_rect(4, bounds, 1.0) {
            let needed_w =
                URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
            let w = needed_w.min(bounds.width);
            let popup_x = bounds.width.saturating_sub(w) / 2;
            let popup_right = popup_x + w; // borderless: exclusive panel edge
            assert!(
                rect.x + rect.width <= popup_right,
                "url rect cols {}..{} extend past popup right edge {}",
                rect.x,
                rect.x + rect.width,
                popup_right
            );
        }
    }

    // Regression: at scale < 1.0 the URL click rect must center off the
    // SCALED width, mirroring paint_version_popup. Centering off unscaled
    // w shifts the click area ~((1-scale)*needed_w)/2 columns left of the
    // painted URL.
    #[test]
    fn url_rect_centering_matches_painter_at_partial_scale() {
        let bounds = full_bounds(200, 60);
        // ≥ 0.7 gate; high enough that the padded URL row still clears the
        // scaled height (the extra PANEL_PAD_Y needs a bit more vertical room).
        let scale = 0.9;
        let needed_w =
            URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2 + 2 * PANEL_PAD_X;
        let w_full = needed_w.min(bounds.width);
        let w_scaled = ((w_full as f32 * scale).round() as u16).max(2);
        let expected_popup_x = bounds.width.saturating_sub(w_scaled) / 2;
        // Borderless + padded: url_x = popup_x + PAD_X + prefix.
        let expected_url_x = expected_popup_x + PANEL_PAD_X + URL_PREFIX.len() as u16;
        let rect = version_popup_url_rect(4, bounds, scale)
            .expect("url rect should exist at scale=0.9 with notes_len=4");
        assert_eq!(
            rect.x, expected_url_x,
            "url click rect x={} must match painter's scaled-centering popup_x+pad+prefix={}",
            rect.x, expected_url_x
        );
    }

    // Regression for the off-screen URL row bug: on a too-short terminal,
    // the painter clips the popup envelope vertically, and the URL row used
    // to land on or below the clipped bottom border (where ratatui never
    // paints it). The rect must return None instead.
    #[test]
    fn url_rect_returns_none_when_url_row_falls_outside_clipped_popup() {
        // notes_len=4 → needed h=11 (borderless + 2·PAD_Y). With bounds.height=9
        // the popup clips to h=9; the padded URL row (PAD_Y + notes_len + 3 = 8)
        // lands at the exclusive content bottom (h − PAD_Y = 8) → None.
        let rect = version_popup_url_rect(4, full_bounds(200, 9), 1.0);
        assert!(
            rect.is_none(),
            "expected None when URL row falls on the clipped popup's bottom border: got {rect:?}"
        );
    }

    // The URL is not clickable until the popup reaches 70% entrance scale.
    #[test]
    fn url_rect_none_below_seventy_percent_scale() {
        assert!(version_popup_url_rect(4, full_bounds(200, 60), 0.5).is_none());
        assert!(version_popup_url_rect(4, full_bounds(200, 60), 0.0).is_none());
    }

    // A popup envelope clamped to a tiny bounds (w<4 || h<3) yields no rect.
    #[test]
    fn url_rect_none_when_envelope_too_small() {
        // 3-col bounds → envelope width clamps to 3 (<4) → None.
        assert!(version_popup_url_rect(4, full_bounds(3, 60), 1.0).is_none());
    }

    // paint_version_popup's fully-dismissed early return (scale ≤ 0.01): a
    // near-zero scale paints nothing, so the buffer stays blank.
    #[test]
    fn version_popup_skips_render_when_fully_dismissed() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| {
            paint_version_popup(
                f,
                "1.2.3",
                &["note a", "note b"],
                Rect::new(0, 0, 80, 30),
                &pixtuoid_scene::theme::NORMAL,
                0.0, // fully dismissed
            );
        })
        .unwrap();
        // Nothing painted ⇒ every cell is still the default blank space.
        let buf = term.backend().buffer();
        let any_glyph = buf.content().iter().any(|c| !c.symbol().trim().is_empty());
        assert!(!any_glyph, "dismissed popup must paint nothing");
    }
}
