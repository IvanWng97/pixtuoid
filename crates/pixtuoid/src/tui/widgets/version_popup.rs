use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::panel::window_range;
use super::{borderless_panel, panel_inner_width, to_color, truncate, PanelGeometry};

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

/// The popup's fixed chrome rows around the notes band: a leading blank, a
/// trailing blank, and the link row. The notes window into whatever the
/// height-clamped inner rect leaves after these.
const CHROME_ROWS: u16 = 3;

/// The windowed notes band's overflow marker. Deliberately NOT the shared
/// `panel::overflow_cue`: its `▾` is the affordance the dashboard and Sources
/// panels page with `j`/`k`, and this modal's only binding is dismiss
/// (`dispatch_key`'s version tier maps Enter and swallows the rest), so the
/// chevron promised rows no key could reach. The hidden notes ARE reachable, just
/// not in the terminal, so the marker points at the `↗ Release notes` CTA that
/// `CHROME_ROWS` keeps two rows below it. Truncated because the band is wrapped
/// to `inner_w` and this row is not — a `Paragraph` would clip it silently.
fn notes_marker(hidden: usize, inner_w: usize) -> String {
    truncate(
        &format!("  \u{22ee} {hidden} more \u{2014} see the link"),
        inner_w,
    )
}

/// THE version-popup geometry authority: wrap the notes to the panel's inner
/// width, window them to the inner height, then hand back the scaled/guarded
/// envelope. BOTH `paint_version_popup` and `version_popup_url_rect` ride this
/// with the same `(bounds, notes, scale)`, so the painted link and its click
/// target are the SAME geometry and can't drift (the phantom-browser-launch
/// class, killed structurally). `None` when the terminal is too small to render.
///
/// The WINDOWING is load-bearing: `PanelGeometry::compute` clamps the envelope to
/// `bounds`, so a long note set on a short terminal asks for more rows than the
/// inner rect has and ratatui silently drops the TRAILING lines — the blank and
/// the `↗ Release notes` CTA. The notes are the band that may overflow (marked by
/// [`notes_marker`], this modal's own non-scrolling one); the link is chrome.
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
    let inner = geom.inner()?; // guarded away → nothing to paint or click
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
/// itself, and a hard cut mid-word (`… — Ente`) left a key-swallowing overlay
/// with no exit instruction. A narrow panel therefore drops "What's new in"
/// first, and elides with `…` only if even the short form overruns.
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
        return; // fully dismissed / terminal too small
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

    /// A long release-note set on a short terminal used to overrun the panel's
    /// height-clamped inner rect, and ratatui dropped the TRAILING lines — which
    /// are the blank + the `↗ Release notes` CTA. The notes are the windowable
    /// band; the link is chrome and must never be what falls off.
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

    /// The title carries the ONLY dismiss instruction, so a narrow panel drops
    /// the prose before it drops `Enter to close` — a mid-word cut left the user
    /// with no way to know how to close a modal that swallows every key.
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
        // A wide panel still gets the full sentence.
        let wide_title = version_title("0.16.0", 60);
        assert_eq!(wide_title, "What's new in v0.16.0 \u{2014} Enter to close");
    }

    /// THE framing gate for the SHIPPED notes. Every other test here authors its
    /// own fixture and asserts something about that same fixture, so none of them
    /// can see the real arm outgrow the panel — and two
    /// (`a_short_terminal_windows_the_notes_and_keeps_the_link`,
    /// `the_notes_marker_offers_no_scroll_and_fits_its_row`) assert the marker IS
    /// present, i.e. they are green precisely in the truncating state. #820's
    /// first draft wrapped to 19 rows against the 17 an 80×24 popup holds, so the
    /// classic terminal hid three behind `⋮ 3 more` and cut the last bullet
    /// mid-sentence; only a reviewer rendering the frame by hand caught it, and
    /// every release edits this arm.
    ///
    /// 80×24 is the size that discriminates — where windowing starts. A wider
    /// terminal cannot fail (the panel never grows past `VERSION_POPUP_W`), and a
    /// NARROWER one legitimately windows: at 32×31 the shipped arm hides 8 rows,
    /// which is the design working. So this pins ONE size on purpose.
    #[test]
    fn the_shipped_release_notes_fit_the_classic_terminal_unwindowed() {
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
        assert!(
            !body.iter().any(|l| l.contains("more")),
            "v{version}'s notes are windowed at 80×24: the tail sits behind the marker and \
             the last visible bullet reads mid-sentence — {:?}",
            body.last(),
        );
        // A separate failure mode from the row count: `word_wrap` never splits
        // mid-word, so an over-wide token overflows its row and `Paragraph` clips it.
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

    /// The windowed band's marker must not offer a scroll this modal has no key
    /// for: `dispatch_key`'s version tier maps Enter to dismiss and swallows
    /// everything else, so the shared `⋮ N more ▾` — whose `▾` is what the
    /// dashboard/connection panels page with j/k — promised content the reader
    /// cannot reach. It also has to FIT: the band is wrapped to the inner width,
    /// the marker is not, and a `Paragraph` clips silently.
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
