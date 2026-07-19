use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{borderless_panel, panel_inner_width, to_color, PanelGeometry};

/// The project repository — opened when the board's ★ Star CTA is clicked.
/// `pub` (not `pub(crate)`): the BIN crate's crash reporter derives its
/// issue-report URL from this same authority — crash.rs is a `main.rs` module,
/// a separate crate the lib's `pub(crate)` can't reach (and the pixtuoid lib
/// target is not a semver surface, so the widening is free).
pub const REPO_URL: &str = "https://github.com/IvanWng97/pixtuoid";

/// The releases page — opened when the version popup's link is clicked. Kept a
/// full literal (const &str can't `concat!`); pinned to `REPO_URL/releases` by a
/// test. The DISPLAY is the short `LINK_LABEL`; this is only the click target.
pub(crate) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";

/// The clickable link's VISIBLE text — short so it fits any usable terminal. The
/// raw URL is ~46 cols and was hard-clipped below ~66 cols (and every frame of the
/// entrance animation); a compact label decouples display width from the link.
/// Its screen rect is `version_popup_url_rect`; clicking opens `VERSION_POPUP_URL`.
const LINK_LABEL: &str = "\u{2197} Release notes";

/// Bullet + hanging-indent for a wrapped release note. `NOTE_CONT` aligns a
/// continuation line under the note text (past the 4-col `NOTE_PREFIX`).
const NOTE_PREFIX: &str = "  \u{00b7} ";
const NOTE_CONT: &str = "    ";

/// Target content width — a comfortable reading measure the notes word-wrap to
/// (clamped to the terminal by the geometry). Chosen over the old URL-driven 68
/// so long notes wrap instead of clipping and the popup fits narrow terminals.
const VERSION_POPUP_W: u16 = 52;

/// The link is not clickable until the entrance animation is ≥70% scaled in —
/// below that the painted cell is smaller than the settled label.
const LINK_CLICKABLE_SCALE: f32 = 0.7;

/// Greedy word-wrap `text` to `width` columns (char-count based — release-note
/// prose is BMP text). A single word longer than `width` gets its own
/// (overflowing) line rather than being split mid-word. `width == 0` or text that
/// already fits → one line (so a short note is byte-identical to before).
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.chars().count() <= width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + wlen <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Wrap every note to the inner width, formatted with the bullet + hanging
/// indent. The returned strings are the content lines the painter styles AND the
/// count the click-rect uses to place the link row.
fn wrap_notes(notes: &[&str], inner_w: u16) -> Vec<String> {
    let budget = (inner_w as usize).saturating_sub(NOTE_PREFIX.chars().count());
    let mut out = Vec::new();
    for note in notes {
        for (i, chunk) in word_wrap(note, budget).into_iter().enumerate() {
            out.push(if i == 0 {
                format!("{NOTE_PREFIX}{chunk}")
            } else {
                format!("{NOTE_CONT}{chunk}")
            });
        }
    }
    out
}

/// THE version-popup geometry authority: wrap the notes to the panel's inner
/// width, then compute the scaled/guarded envelope. BOTH `paint_version_popup`
/// and `version_popup_url_rect` ride this with the same `(bounds, notes, scale)`,
/// so the painted link and its click target are the SAME geometry and can't drift
/// (the phantom-browser-launch class, killed structurally). `None` when the
/// terminal is too small to render.
fn version_geometry(
    bounds: Rect,
    notes: &[&str],
    scale: f32,
) -> Option<(PanelGeometry, Vec<String>)> {
    // Two-phase: the height-independent inner width first, so notes wrap BEFORE
    // the row count (hence the height) is known.
    let inner_w = panel_inner_width(bounds, VERSION_POPUP_W, scale)?;
    let wrapped = wrap_notes(notes, inner_w);
    // Content below the title: blank + wrapped notes + blank + link.
    let content_rows = (wrapped.len() as u16).saturating_add(3);
    // The title TEXT is irrelevant to geometry — only the reserved title row
    // (is_some) matters here; the painter draws the real title into it.
    let geom = PanelGeometry::compute(bounds, VERSION_POPUP_W, content_rows, Some(""), scale);
    geom.inner()?; // guarded away → nothing to paint or click
    Some((geom, wrapped))
}

/// 0-indexed content row (below the title) the link sits on: after the leading
/// blank, the wrapped notes, and a trailing blank.
fn link_row(wrapped_len: usize) -> u16 {
    (wrapped_len as u16).saturating_add(2)
}

pub(crate) fn paint_version_popup(
    f: &mut ratatui::Frame<'_>,
    version: &str,
    notes: &[&str],
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
    scale: f32,
) {
    let scale = scale.clamp(0.0, 1.0);
    let Some((geom, wrapped)) = version_geometry(bounds, notes, scale) else {
        return; // fully dismissed / terminal too small
    };
    let outer = geom
        .outer()
        .expect("version_geometry guarantees a rendered geom");

    let mut items: Vec<Line> = Vec::with_capacity(wrapped.len() + 3);
    items.push(Line::from(""));
    for w in &wrapped {
        items.push(Line::from(Span::styled(
            w.clone(),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    items.push(Line::from(""));
    items.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            LINK_LABEL,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));

    let title = format!("What's new in v{version} \u{2014} Enter to close");
    // `borderless_panel(outer)` returns `inner_rect(outer, has_title)` — the SAME
    // rect `geom.inner()` and `cell_rect` derive from, so paint and click agree.
    let inner = borderless_panel(f, outer, Some(&title), theme);
    f.render_widget(Paragraph::new(items), inner);
}

/// The screen rect of the clickable link inside the version popup, or `None` when
/// it isn't rendered/clickable. Derived from the SAME `version_geometry` the
/// painter uses (`cell_rect` off the shared `compute`), so a click can never land
/// where the link isn't painted (the phantom-browser-launch regression class).
pub(crate) fn version_popup_url_rect(notes: &[&str], bounds: Rect, scale: f32) -> Option<Rect> {
    let scale = scale.clamp(0.0, 1.0);
    if scale < LINK_CLICKABLE_SCALE {
        return None;
    }
    let (geom, wrapped) = version_geometry(bounds, notes, scale)?;
    // col 2 = past the "  " indent Span the painter renders before the label.
    geom.cell_rect(
        link_row(wrapped.len()),
        2,
        LINK_LABEL.chars().count() as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> Rect {
        Rect::new(0, 0, 200, 60)
    }

    #[test]
    fn version_popup_url_is_repo_releases() {
        // The click opens the releases page; the ★ CTA opens the repo root.
        assert_eq!(VERSION_POPUP_URL, format!("{REPO_URL}/releases"));
    }

    #[test]
    fn link_click_rect_is_the_painted_link_cell() {
        // The click rect is `cell_rect` off the SAME geometry the painter fills —
        // pinned to the absolute screen cell so paint and click can't drift.
        let notes = &["one", "two", "three"];
        let (geom, wrapped) = version_geometry(wide(), notes, 1.0).expect("renders");
        let inner = geom.inner().expect("rendered ⇒ inner Some");
        let expected = Rect::new(
            inner.x + 2,
            inner.y + link_row(wrapped.len()),
            LINK_LABEL.chars().count() as u16,
            1,
        );
        assert_eq!(version_popup_url_rect(notes, wide(), 1.0), Some(expected));
    }

    #[test]
    fn link_fits_where_the_old_url_clipped() {
        // A 50-col terminal hard-clipped the old ~46-char raw URL. The compact
        // label fits in full — its rect spans the whole label, unclamped.
        let rect = version_popup_url_rect(&["a note"], Rect::new(0, 0, 50, 30), 1.0).expect("fits");
        assert_eq!(rect.width, LINK_LABEL.chars().count() as u16);
    }

    #[test]
    fn long_note_wraps_instead_of_clipping() {
        let long = "This is a deliberately long release note that must wrap across \
                    several lines instead of being cut off at the panel edge somewhere.";
        let (_g, wrapped) = version_geometry(wide(), &[long], 1.0).expect("renders");
        assert!(wrapped.len() > 1, "a long note must wrap to multiple lines");
        assert!(
            wrapped[0].starts_with(NOTE_PREFIX),
            "first line carries the bullet"
        );
        assert!(
            wrapped[1].starts_with(NOTE_CONT),
            "continuation carries the hanging indent"
        );
        // Every wrapped line fits the inner width — never clips.
        let inner_w = panel_inner_width(wide(), VERSION_POPUP_W, 1.0).unwrap() as usize;
        assert!(wrapped.iter().all(|l| l.chars().count() <= inner_w));
    }

    #[test]
    fn short_note_stays_one_line() {
        let (_g, wrapped) = version_geometry(wide(), &["short"], 1.0).expect("renders");
        assert_eq!(wrapped, vec![format!("{NOTE_PREFIX}short")]);
    }

    #[test]
    fn url_rect_none_below_clickable_scale_and_tiny_bounds() {
        assert!(version_popup_url_rect(&["a"], wide(), 0.5).is_none());
        assert!(version_popup_url_rect(&["a"], wide(), 0.0).is_none());
        // 3-col terminal → geometry guards away → None (no phantom click).
        assert!(version_popup_url_rect(&["a"], Rect::new(0, 0, 3, 60), 1.0).is_none());
    }

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
                0.0,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        assert!(
            !buf.content().iter().any(|c| !c.symbol().trim().is_empty()),
            "dismissed popup must paint nothing"
        );
    }
}
