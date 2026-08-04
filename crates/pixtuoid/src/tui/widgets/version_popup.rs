use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::panel::window_range;
use super::{borderless_panel, panel_inner_width, to_color, truncate, PanelGeometry};

/// The project repository. `pub`, not `pub(crate)`: the BIN crate's crash
/// reporter derives its issue-report URL from this same authority, and
/// `pub(crate)` in the lib can't reach a `main.rs` module.
pub const REPO_URL: &str = "https://github.com/IvanWng97/pixtuoid";

/// The releases page. Kept a full literal because a `const &str` can't `concat!`;
/// pinned to `REPO_URL/releases` by a test.
pub(crate) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";

/// The clickable link's VISIBLE text — a compact label decouples display width
/// from the link, so it fits any usable terminal and every entrance-animation
/// frame where the raw URL hard-clipped.
const LINK_LABEL: &str = "\u{2197} Release notes";

/// Bullet + hanging-indent for a wrapped release note; `NOTE_CONT` aligns a
/// continuation line under the note text.
const NOTE_PREFIX: &str = "  \u{00b7} ";
const NOTE_CONT: &str = "    ";

/// Target content width — a comfortable reading measure the notes word-wrap to,
/// clamped to the terminal by the geometry.
const VERSION_POPUP_W: u16 = 52;

/// The link is not clickable until the entrance animation is ≥70% scaled in —
/// below that the painted cell is smaller than the settled label.
const LINK_CLICKABLE_SCALE: f32 = 0.7;

/// Greedy word-wrap `text` to `width` columns (char-count based — release-note
/// prose is BMP text). A single word longer than `width` gets its own
/// (overflowing) line rather than being split mid-word.
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

/// The popup's fixed chrome rows around the notes band: a leading blank, a
/// trailing blank, and the link row.
const CHROME_ROWS: u16 = 3;

/// The windowed notes band's overflow marker. Deliberately NOT the shared
/// `panel::overflow_cue`: its `▾` is the affordance the dashboard and Sources
/// panels page with `j`/`k`, and this modal's only binding is dismiss, so the
/// chevron would promise rows no key can reach — the marker points at the
/// `↗ Release notes` CTA instead. Truncated because the band is wrapped to
/// `inner_w` and this row is not, and a `Paragraph` would clip it silently.
fn notes_marker(hidden: usize, inner_w: usize) -> String {
    truncate(
        &format!("  \u{22ee} {hidden} more \u{2014} see the link"),
        inner_w,
    )
}

/// THE version-popup geometry authority. BOTH `paint_version_popup` and
/// `version_popup_url_rect` ride this with the same `(bounds, notes, scale)`, so
/// the painted link and its click target can't drift apart.
///
/// The WINDOWING is load-bearing: `PanelGeometry::compute` clamps the envelope to
/// `bounds`, so a long note set on a short terminal asks for more rows than the
/// inner rect has and ratatui silently drops the TRAILING lines — the blank and
/// the `↗ Release notes` CTA. The notes are the band that may overflow; the link
/// is chrome.
fn version_geometry(
    bounds: Rect,
    notes: &[&str],
    scale: f32,
) -> Option<(PanelGeometry, Vec<String>)> {
    // Two-phase: the height-independent inner width first, so notes wrap BEFORE
    // the row count (hence the height) is known.
    let inner_w = panel_inner_width(bounds, VERSION_POPUP_W, scale)?;
    let wrapped = wrap_notes(notes, inner_w);
    let content_rows = (wrapped.len() as u16).saturating_add(CHROME_ROWS);
    // The title TEXT is irrelevant to geometry — only the reserved title row
    // (is_some) matters here; the painter draws the real title into it.
    let geom = PanelGeometry::compute(bounds, VERSION_POPUP_W, content_rows, Some(""), scale);
    let inner = geom.inner()?;
    let viewport = (inner.height as usize).saturating_sub(CHROME_ROWS as usize);
    let win = window_range(wrapped.len(), None, 0, viewport);
    let mut body: Vec<String> = wrapped
        .into_iter()
        .skip(win.start)
        .take(win.count)
        .collect();
    if let Some(hidden) = win.cue {
        body.push(notes_marker(hidden, inner.width as usize));
    }
    Some((geom, body))
}

/// 0-indexed content row (below the title) the link sits on: after the leading
/// blank, the (windowed) notes band, and a trailing blank.
fn link_row(body_len: usize) -> u16 {
    (body_len as u16).saturating_add(2)
}

/// The popup's title, sized to the panel's real inner width. The dismiss hint
/// outranks the prose: the title is the ONLY place the modal says how to close
/// itself, so a narrow panel drops "What's new in" first rather than cutting
/// mid-word and leaving a key-swallowing overlay with no exit instruction.
fn version_title(version: &str, inner_w: usize) -> String {
    let full = format!("What's new in v{version} \u{2014} Enter to close");
    if full.chars().count() <= inner_w {
        return full;
    }
    truncate(&format!("v{version} \u{2014} Enter to close"), inner_w)
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
    let Some((geom, body)) = version_geometry(bounds, notes, scale) else {
        return;
    };
    let outer = geom
        .outer()
        .expect("version_geometry guarantees a rendered geom");
    let inner_w = geom
        .inner()
        .expect("version_geometry guarantees a rendered geom")
        .width;

    let mut items: Vec<Line> = Vec::with_capacity(body.len() + CHROME_ROWS as usize);
    items.push(Line::from(""));
    for w in &body {
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

    let title = version_title(version, inner_w as usize);
    // `borderless_panel(outer)` returns the SAME rect `geom.inner()` and
    // `cell_rect` derive from, so paint and click agree.
    let inner = borderless_panel(f, outer, Some(&title), theme);
    f.render_widget(Paragraph::new(items), inner);
}

/// The screen rect of the clickable link, or `None` when it isn't
/// rendered/clickable. Derived from the SAME `version_geometry` the painter uses,
/// so a click can never land where the link isn't painted.
pub(crate) fn version_popup_url_rect(notes: &[&str], bounds: Rect, scale: f32) -> Option<Rect> {
    let scale = scale.clamp(0.0, 1.0);
    if scale < LINK_CLICKABLE_SCALE {
        return None;
    }
    let (geom, body) = version_geometry(bounds, notes, scale)?;
    // col 2 = past the "  " indent Span the painter renders before the label.
    geom.cell_rect(link_row(body.len()), 2, LINK_LABEL.chars().count() as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> Rect {
        Rect::new(0, 0, 200, 60)
    }

    #[test]
    fn version_popup_url_is_repo_releases() {
        assert_eq!(VERSION_POPUP_URL, format!("{REPO_URL}/releases"));
    }

    #[test]
    fn link_click_rect_is_the_painted_link_cell() {
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
        // 50 cols is where the old ~46-char raw URL hard-clipped.
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
        assert!(version_popup_url_rect(&["a"], Rect::new(0, 0, 3, 60), 1.0).is_none());
    }

    #[test]
    fn a_short_terminal_windows_the_notes_and_keeps_the_link() {
        let notes: Vec<&str> = vec![
            "The office you can hear — press m to turn on sound, a lofi band that layers up \
             as your agents get busier, gentle rain when the office weather rains",
            "Two moods, picked by the office itself — after dark, or whenever it rains, the \
             band switches to a slower night take with deeper bass and lazier drums",
            "Click a sprite to bring its terminal to the front, and press f on a dashboard row",
            "Each OpenClaw gateway now renders as its own mascot, keyed on the resolved port, \
             so two gateways of one profile no longer collapse into a single lobster",
            "The Sources panel folds the install-soundness and decode-drift verdicts into one \
             health line, so a broken hook install is visible without running doctor",
        ];
        let bounds = Rect::new(0, 0, 32, 31);
        let (geom, body) = version_geometry(bounds, &notes, 1.0).expect("renders");
        let inner = geom.inner().expect("rendered ⇒ inner Some");
        assert!(
            (body.len() as u16).saturating_add(3) <= inner.height,
            "the body + its blank/blank/link chrome must FIT the inner rect: \
             {} body rows in {} inner rows",
            body.len(),
            inner.height
        );
        assert!(
            body.last().is_some_and(|l| l.contains("more")),
            "a windowed band must carry the shared overflow cue, got: {body:?}"
        );
        assert!(
            version_popup_url_rect(&notes, bounds, 1.0).is_some(),
            "the link must still be inside the panel — it is the CTA, not filler"
        );
    }

    #[test]
    fn a_narrow_title_keeps_the_dismiss_hint() {
        let inner_w = panel_inner_width(Rect::new(0, 0, 32, 31), VERSION_POPUP_W, 1.0)
            .expect("renders") as usize;
        let title = version_title("0.16.0", inner_w);
        assert!(
            title.chars().count() <= inner_w,
            "the title must fit its row: {title:?} in {inner_w}"
        );
        assert!(
            title.contains("Enter to close"),
            "the dismiss hint outranks the prose: {title:?}"
        );
        let wide_title = version_title("0.16.0", 60);
        assert_eq!(wide_title, "What's new in v0.16.0 \u{2014} Enter to close");
    }

    /// THE framing gate for the SHIPPED notes, which every release edits. Every
    /// other test here authors its own fixture, and two of them are green
    /// precisely in the truncating state, so none can see the real arm outgrow the
    /// panel.
    ///
    /// 80×24 is the TIGHTEST size that must fit unwindowed: the panel never grows
    /// past `VERSION_POPUP_W`, so any terminal at least this wide AND this tall
    /// wraps identically or has more rows, while anything smaller windows by
    /// design.
    #[test]
    fn the_shipped_release_notes_fit_the_classic_terminal_unwindowed() {
        if crate::version::release_notes_are_uncurated() {
            return; // `just bump`'s draft — see `release_notes_are_uncurated`
        }
        let version = env!("CARGO_PKG_VERSION");
        let notes = crate::version::release_notes(version)
            .expect("the shipped version has notes — current_version_has_release_notes");
        let bounds = Rect::new(0, 0, 80, 24);
        let (geom, body) = version_geometry(bounds, notes, 1.0).expect("renders at 80×24");
        let inner = geom.inner().expect("rendered ⇒ inner Some");
        let inner_w = panel_inner_width(bounds, VERSION_POPUP_W, 1.0).expect("renders");
        let wrapped = wrap_notes(notes, inner_w);
        let band = inner.height as usize - CHROME_ROWS as usize;

        assert_eq!(
            body.len(),
            wrapped.len(),
            "v{version} wraps to {} rows but only {} reach an 80×24 popup (the band holds \
             {band}) — trim the prose, don't raise the cap",
            wrapped.len(),
            body.len(),
        );
        // Key on the marker's own GLYPH, and only on the row it can occupy: the
        // body rows are release PROSE, and a `contains("more")` scan matches it.
        assert!(
            !body.last().is_some_and(|l| l.contains('\u{22ee}')),
            "v{version}'s notes are windowed at 80×24: the tail sits behind the marker and \
             the last visible bullet reads mid-sentence — {:?}",
            body.last(),
        );
        // Separate from the row count: `word_wrap` never splits mid-word, so an
        // over-wide token overflows its row and `Paragraph` clips it.
        assert!(
            wrapped
                .iter()
                .all(|l| l.chars().count() <= inner_w as usize),
            "a token wider than the {inner_w}-col measure clips silently: {:?}",
            wrapped
                .iter()
                .find(|l| l.chars().count() > inner_w as usize),
        );
    }

    #[test]
    fn the_notes_marker_offers_no_scroll_and_fits_its_row() {
        let notes: Vec<String> = (0..40)
            .map(|i| format!("release note number {i}"))
            .collect();
        let refs: Vec<&str> = notes.iter().map(String::as_str).collect();
        for bounds in [Rect::new(0, 0, 80, 24), Rect::new(0, 0, 32, 31)] {
            let (geom, body) = version_geometry(bounds, &refs, 1.0).expect("renders");
            let inner = geom.inner().expect("rendered ⇒ inner Some");
            let marker = body.last().expect("the band overflows at both sizes");
            assert!(
                marker.contains("more"),
                "the band IS windowed here, so the last row is the marker: {marker:?}"
            );
            assert!(
                !marker.contains('\u{25be}'),
                "the ▾ reads as `page down` on a modal whose only key is dismiss: {marker:?}"
            );
            assert!(
                marker.contains("the link"),
                "the hidden notes need a reachable destination, and the CTA below is it: {marker:?}"
            );
            assert!(
                marker.chars().count() <= inner.width as usize,
                "the marker must fit its row unclipped: {marker:?} in {}",
                inner.width
            );
        }
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
