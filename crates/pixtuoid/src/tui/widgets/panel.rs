//! The shared borderless modal frame for every popup. Delegates the card backing
//! (drop shadow + `Clear` + solid bg fill) to `super::paint_card_backing` — the
//! ONE definition shared with the framed tooltips — then adds a uniform pad and
//! an optional bold inner title line, and returns the inner content `Rect` the
//! caller paints into. NO border (readability over the busy pixel office without
//! the outline). Used by help / version / theme picker / dashboard / connection.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{paint_card_backing, to_color};
use pixtuoid_scene::theme::Theme;

/// Uniform inner padding for every borderless popup — the breathing room that
/// stands in for the removed border. PRIVATE: the geometry authority
/// (`compute` / `inner_rect` / `panel_inner_width`) is the ONE place that insets
/// by it, so no caller reverses the fold or mirrors a click-rect offset any more.
const PANEL_PAD_X: u16 = 2;
const PANEL_PAD_Y: u16 = 1;

/// Minimum renderable envelope. Below this the panel paints nothing — the
/// historical per-caller guard ("nothing legible under 4×3, and `Clear::render`
/// panics indexing past a narrower buffer"), unified into the geometry so every
/// popup shares one threshold. NOT derived from `PANEL_PAD_*` (a separate policy).
const PANEL_MIN_W: u16 = 4;
const PANEL_MIN_H: u16 = 3;

/// Inner content `Rect` of a borderless panel: `outer` inset by `PANEL_PAD_*`
/// with the title row (when present) dropped. Raw-area fallback when `outer` is
/// too small to inset — the historical `borderless_panel` behavior. Extracted so
/// `borderless_panel` RETURNS this and [`PanelGeometry::inner`] READS it: the
/// painted content rect and any geometry query are the SAME value by construction.
fn inner_rect(outer: Rect, has_title: bool) -> Rect {
    if outer.width <= PANEL_PAD_X * 2 || outer.height <= PANEL_PAD_Y * 2 {
        return outer;
    }
    let mut inner = Rect {
        x: outer.x + PANEL_PAD_X,
        y: outer.y + PANEL_PAD_Y,
        width: outer.width - PANEL_PAD_X * 2,
        height: outer.height - PANEL_PAD_Y * 2,
    };
    if has_title && inner.height >= 1 {
        inner.y += 1;
        inner.height -= 1;
    }
    inner
}

/// THE pure geometry authority for a centered borderless popup: the scaled,
/// bounds-clamped, guarded envelope + inner content rect + a content-cell →
/// screen-rect mapping. No `Frame` — unit-testable without a `TestBackend`, so
/// the paint/click lockstep is a pure arithmetic test rather than a render diff.
/// BOTH the painter (fills [`Self::inner`]) and any click-target
/// ([`Self::cell_rect`]) read the SAME value, so they cannot drift (the
/// phantom-browser-launch regression class, killed structurally).
pub(crate) struct PanelGeometry {
    outer: Option<Rect>,
    inner: Option<Rect>,
}

impl PanelGeometry {
    /// `content_rows` is the content BELOW the title; the title row is added here.
    /// Envelope = `(content + 2·PANEL_PAD)` clamped to `bounds`, THEN ·`scale`
    /// (rounded), centered off the SCALED dims, THEN the `<PANEL_MIN → None` guard
    /// (subsumes the 5 per-caller `<4||<3` guards AND version_popup's `.max(2)`
    /// floor + `scale<=0.01` return). `scale` is clamped to `0.0..=1.0`.
    pub(crate) fn compute(
        bounds: Rect,
        content_w: u16,
        content_rows: u16,
        title: Option<&str>,
        scale: f32,
    ) -> Self {
        let scale = scale.clamp(0.0, 1.0);
        let full_w = content_w.saturating_add(2 * PANEL_PAD_X).min(bounds.width);
        let full_h = content_rows
            .saturating_add(title.is_some() as u16)
            .saturating_add(2 * PANEL_PAD_Y)
            .min(bounds.height);
        let w = (full_w as f32 * scale).round() as u16;
        let h = (full_h as f32 * scale).round() as u16;
        if w < PANEL_MIN_W || h < PANEL_MIN_H {
            return Self {
                outer: None,
                inner: None,
            };
        }
        let outer = Rect {
            x: bounds.x + bounds.width.saturating_sub(w) / 2,
            y: bounds.y + bounds.height.saturating_sub(h) / 2,
            width: w,
            height: h,
        };
        Self {
            outer: Some(outer),
            inner: Some(inner_rect(outer, title.is_some())),
        }
    }

    /// The scaled/centered/guarded envelope. `None` ⇔ the guard tripped.
    pub(crate) fn outer(&self) -> Option<Rect> {
        self.outer
    }

    /// The inner content rect below the title row. `None` ⇔ guarded away.
    pub(crate) fn inner(&self) -> Option<Rect> {
        self.inner
    }

    /// Map a content cell — `row` below the title, `col`/`len` chars within
    /// `inner` — to a clipped screen `Rect`. `None` when guarded away or the cell
    /// falls outside `inner`. Reproduces the historical `version_popup_url_rect`
    /// clip, derived ONCE from the same geometry the painter fills.
    pub(crate) fn cell_rect(&self, row: u16, col: u16, len: u16) -> Option<Rect> {
        let inner = self.inner?;
        let x = inner.x + col;
        let y = inner.y + row;
        if x >= inner.right() || y >= inner.bottom() {
            return None;
        }
        let width = len.min(inner.right() - x);
        if width == 0 {
            return None;
        }
        Some(Rect {
            x,
            y,
            width,
            height: 1,
        })
    }
}

/// The inner content WIDTH alone — height-independent, so a width-dependent row
/// builder (word-wrap, marquee) can size before the row count / height is known
/// (version_popup wraps its notes to this before it knows how many rows result).
/// Same clamp+scale+inset math as [`PanelGeometry::compute`]; pinned to agree with
/// `compute(..).inner().width` by a test.
pub(crate) fn panel_inner_width(bounds: Rect, content_w: u16, scale: f32) -> Option<u16> {
    let scale = scale.clamp(0.0, 1.0);
    let full_w = content_w.saturating_add(2 * PANEL_PAD_X).min(bounds.width);
    let w = (full_w as f32 * scale).round() as u16;
    if w < PANEL_MIN_W {
        return None;
    }
    Some(if w <= PANEL_PAD_X * 2 {
        w
    } else {
        w - PANEL_PAD_X * 2
    })
}

/// The visible slice of a scrollable list + the hidden-below count for the cue.
pub(crate) struct ListWindow {
    pub(crate) start: usize,
    pub(crate) count: usize,
    pub(crate) cue: Option<usize>,
}

/// Window a `list_len`-row list into a `viewport`-row region, following
/// `selected` from `scroll`. On overflow, reserve the last line for the
/// `⋮ N more ▾` cue — UNLESS the selection has reached the end (nothing below),
/// then use the full viewport and drop the cue. Pure; reproduces the dashboard's
/// reserve-a-line logic verbatim so every list panel overflows identically.
pub(crate) fn window_range(
    list_len: usize,
    selected: Option<usize>,
    scroll: usize,
    viewport: usize,
) -> ListWindow {
    use crate::tui::dashboard::clamp_scroll_idx;
    let overflow = list_len > viewport;
    let reserved = if overflow {
        viewport.saturating_sub(1)
    } else {
        viewport
    };
    let probe = clamp_scroll_idx(selected, scroll, reserved);
    let show_cue = overflow && list_len > probe + reserved;
    let window = if show_cue { reserved } else { viewport };
    let start = clamp_scroll_idx(selected, scroll, window);
    ListWindow {
        start,
        count: list_len.saturating_sub(start).min(window),
        // guarded arithmetic → `.then(||)` (lazy), not `then_some` (eager).
        cue: show_cue.then(|| list_len.saturating_sub(start + window)),
    }
}

/// The shared "N rows hidden below" cue text. The module owns the WORDS; the
/// caller styles the color (`label_idle`).
pub(crate) fn overflow_cue(hidden: usize) -> String {
    format!("  \u{22ee} {hidden} more \u{25be}")
}

/// Paint a borderless panel over `area`: `Clear`, a solid background fill, a
/// uniform `PANEL_PAD_*` inset, and — when `title` is set and there's room — a
/// bold brand-colored title line at the top of the padded region. Returns the
/// content `Rect` (the padded region, below the title row when one is drawn). No
/// borders are ever drawn; the bg fill + `Clear` + padding keep text legible and
/// off the panel edges.
pub(crate) fn borderless_panel(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    title: Option<&str>,
    theme: &Theme,
) -> Rect {
    // Shared backing: drop shadow + Clear + solid bg fill (the padding region is
    // bg, not blank). The title row below re-uses the same fill.
    paint_card_backing(f, area, theme);
    let bg = Style::default().bg(to_color(theme.ui.tooltip_bg));
    // Too small to pad — hand back the raw area rather than underflow.
    if area.width <= PANEL_PAD_X * 2 || area.height <= PANEL_PAD_Y * 2 {
        return area;
    }
    // Title into the first padded row; then the returned content rect IS
    // `inner_rect` — the SAME fn `PanelGeometry::inner` reads, so the painted inner
    // and any geometry query (e.g. a click-target) can't drift. Past the early
    // return the padded region is ≥1 row tall, so the 1-row title always fits.
    if let Some(t) = title {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t.to_string(),
                Style::default()
                    .fg(to_color(theme.ui.neon_brand))
                    .add_modifier(Modifier::BOLD),
            )))
            .style(bg),
            Rect {
                x: area.x + PANEL_PAD_X,
                y: area.y + PANEL_PAD_Y,
                width: area.width - PANEL_PAD_X * 2,
                height: 1,
            },
        );
    }
    inner_rect(area, title.is_some())
}

/// How [`paint_panel`] treats the windowed `list` band.
pub(crate) enum Overflow {
    /// Selection-follow window + `⋮ N more ▾` cue. `cap` limits the visible list
    /// rows regardless of terminal height (dashboard's 16-row cap); `None` fills.
    Follow {
        selected: Option<usize>,
        scroll: usize,
        cap: Option<u16>,
    },
    /// Window from the top with a cue when it overflows; no selection (help).
    CueOnly,
    /// Render the whole list as-is, sized to fit (dashboard's empty state).
    None,
}

/// THE one painter for a centered borderless popup. It frames (backing, title),
/// windows the `list` band into the space between the fixed `above`/`below`
/// chrome, and appends the overflow cue. Auto-heights to the ACTUAL band lengths
/// (no caller-side structural row count that can drift from the lines pushed).
/// `content_w` is the desired content width (clamped to the terminal); `scale`
/// is 1.0 for the static panels. Callers hand PRE-STYLED lines: per-row marquee,
/// highlight and badge stay theirs; the module owns framing, windowing and cue.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_panel(
    f: &mut ratatui::Frame<'_>,
    theme: &Theme,
    title: Option<&str>,
    bounds: Rect,
    content_w: u16,
    scale: f32,
    above: Vec<Line<'static>>,
    list: Vec<Line<'static>>,
    below: Vec<Line<'static>>,
    overflow: Overflow,
) {
    let cap = match &overflow {
        Overflow::Follow { cap, .. } => *cap,
        _ => None,
    };
    // Size for the (capped) list plus the fixed chrome — the ACTUAL Vec lengths.
    let list_size_rows = cap.map_or(list.len(), |c| list.len().min(c as usize));
    let content_rows = (above.len() + list_size_rows + below.len()) as u16;
    let geom = PanelGeometry::compute(bounds, content_w, content_rows, title, scale);
    let Some(outer) = geom.outer() else {
        return; // guarded away → paint nothing
    };
    let inner = borderless_panel(f, outer, title, theme);

    // The list windows into whatever inner height remains after the fixed chrome.
    let viewport = (inner.height as usize).saturating_sub(above.len() + below.len());
    let win = match &overflow {
        Overflow::None => ListWindow {
            start: 0,
            count: list.len().min(viewport),
            cue: None,
        },
        Overflow::CueOnly => window_range(list.len(), Option::None, 0, viewport),
        Overflow::Follow {
            selected, scroll, ..
        } => window_range(list.len(), *selected, *scroll, viewport),
    };

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner.height as usize);
    lines.extend(above);
    lines.extend(list.into_iter().skip(win.start).take(win.count));
    if let Some(hidden) = win.cue {
        lines.push(Line::from(Span::styled(
            overflow_cue(hidden),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    lines.extend(below);
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_to_string(w: u16, h: u16, title: Option<&str>) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            borderless_panel(
                f,
                Rect::new(0, 0, w, h),
                title,
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..h {
            for x in 0..w {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn borderless_panel_has_no_border_glyphs_and_renders_title() {
        let s = render_to_string(40, 8, Some("Connection"));
        for g in ['╭', '╮', '╰', '╯', '│', '─', '┌', '┐', '└', '┘'] {
            assert!(!s.contains(g), "panel must be borderless, found {g:?}");
        }
        assert!(s.contains("Connection"), "title must render in the body");
    }

    #[test]
    fn borderless_panel_returns_padded_inner_below_the_title_row() {
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        let mut inner = Rect::default();
        term.draw(|f| {
            inner = borderless_panel(
                f,
                Rect::new(0, 0, 20, 6),
                Some("X"),
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        // PAD_X each side; PAD_Y top + the title row above the content.
        assert_eq!(inner.x, PANEL_PAD_X);
        assert_eq!(
            inner.y,
            PANEL_PAD_Y + 1,
            "content starts below the title row"
        );
        assert_eq!(inner.width, 20 - PANEL_PAD_X * 2);
        assert_eq!(inner.height, 6 - PANEL_PAD_Y * 2 - 1);
        // Untitled: the padded region with no title row.
        term.draw(|f| {
            inner = borderless_panel(
                f,
                Rect::new(0, 0, 20, 6),
                None,
                &pixtuoid_scene::theme::NORMAL,
            );
        })
        .unwrap();
        assert_eq!(inner.y, PANEL_PAD_Y);
        assert_eq!(inner.height, 6 - PANEL_PAD_Y * 2);
    }

    #[test]
    fn borderless_panel_never_panics_across_sizes() {
        for (w, h) in [(80, 20), (40, 8), (10, 3), (4, 2), (2, 1)] {
            let _ = render_to_string(w, h, Some("T"));
            let _ = render_to_string(w, h, None);
        }
    }

    /// `borderless_panel` (via the shared `paint_card_backing`) casts a flat,
    /// single-color drop shadow: the card's silhouette darkened by ONE uniform
    /// `SHADOW_FACTOR` and offset one cell down-and-right. What stays visible is an
    /// L-band whose right column is FULL cells and whose bottom row is TOP-HALF only
    /// (a 1px contact line — `fg` dimmed, `bg` left lit), both the SAME shade, with
    /// the top-right and bottom-left corners left lit and nothing above/left of the
    /// card. Pre-fills the buffer with a known bright color to stand in for the
    /// already-flushed office, then renders a small inset panel. Values are derived
    /// from `SHADOW_FACTOR` so a darkness tweak doesn't silently gut the assertions.
    #[test]
    fn borderless_panel_casts_a_flat_offset_shadow() {
        use ratatui::style::Color;
        let bright = Color::Rgb(200, 200, 200);
        let dim = (200.0 * crate::tui::widgets::SHADOW_FACTOR) as u8; // the one shadow shade
        assert!(dim < 200, "SHADOW_FACTOR must actually darken");
        let area = Rect::new(5, 4, 8, 4); // small, well inside the 20x12 buffer
        let mut term = Terminal::new(TestBackend::new(20, 12)).unwrap();
        term.draw(|f| {
            let full = f.area();
            for y in 0..full.height {
                for x in 0..full.width {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    cell.set_symbol("\u{2580}");
                    cell.fg = bright;
                    cell.bg = bright;
                }
            }
            borderless_panel(f, area, None, &pixtuoid_scene::theme::NORMAL);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let chan = |c: Color| match c {
            Color::Rgb(r, _, _) => r,
            other => panic!("expected Rgb, got {other:?}"),
        };
        let r = |x: u16, y: u16| chan(buf.cell((x, y)).unwrap().bg);
        let rf = |x: u16, y: u16| chan(buf.cell((x, y)).unwrap().fg);
        // Right column: FULL cell — both half-block sub-pixels darkened to the shade.
        assert_eq!(rf(area.right(), area.y + 1), dim, "right band top px dim");
        assert_eq!(r(area.right(), area.y + 1), dim, "right band bottom px dim");
        // Bottom row: TOP-HALF only — top sub-pixel dim, bottom sub-pixel LIT (1px).
        assert_eq!(rf(area.x + 1, area.bottom()), dim, "bottom band top px dim");
        assert_eq!(
            r(area.x + 1, area.bottom()),
            200,
            "bottom band bottom px stays lit (a 1px line, not a full cell)"
        );
        // SAME COLOR: right band and bottom band are the identical shade.
        assert_eq!(
            rf(area.right(), area.y + 1),
            rf(area.x + 1, area.bottom()),
            "right and bottom bands are ONE uniform color"
        );
        // Corner joins them and is also top-half (top px dim, bottom px lit).
        assert_eq!(rf(area.right(), area.bottom()), dim, "corner top px dim");
        assert_eq!(r(area.right(), area.bottom()), 200, "corner bottom px lit");
        // Offset silhouette: the right band is 1 cell wide and offset DOWN (top-right
        // corner lit); the bottom band is offset RIGHT (bottom-left corner lit).
        assert_eq!(
            r(area.right() + 1, area.y + 1),
            200,
            "right band is 1 cell wide (the next column is lit)"
        );
        assert_eq!(
            r(area.right(), area.y),
            200,
            "top-right corner stays lit (offset down)"
        );
        assert_eq!(
            r(area.x, area.bottom()),
            200,
            "bottom-left corner stays lit (offset right)"
        );
        // Nothing above or left of the card, and cells far from the band stay bright.
        assert_eq!(
            r(area.x, area.y.saturating_sub(1)),
            200,
            "no shadow above the card"
        );
        assert_eq!(
            r(area.x.saturating_sub(1), area.y),
            200,
            "no shadow left of the card"
        );
        assert_eq!(r(0, 0), 200, "cells outside the band stay bright");
    }

    // ---- PanelGeometry: the pure geometry authority (no TestBackend) ----------

    #[test]
    fn geometry_guard_trips_below_min_and_at_zero_scale() {
        let b = Rect::new(0, 0, 100, 50);
        let g = PanelGeometry::compute(b, 20, 5, Some("t"), 1.0);
        assert!(g.outer().is_some() && g.inner().is_some());
        assert!(g.cell_rect(0, 0, 3).is_some());
        // scale 0 → nothing renders (subsumes version's old scale<=0.01 return)
        let z = PanelGeometry::compute(b, 20, 5, Some("t"), 0.0);
        assert!(z.outer().is_none() && z.inner().is_none() && z.cell_rect(0, 0, 3).is_none());
        // width < 4 → None (unifies the 5 per-caller `<4` guards)
        assert!(
            PanelGeometry::compute(Rect::new(0, 0, 3, 50), 20, 5, Some("t"), 1.0)
                .outer()
                .is_none()
        );
        // height < 3 → None
        assert!(
            PanelGeometry::compute(Rect::new(0, 0, 100, 2), 20, 5, Some("t"), 1.0)
                .outer()
                .is_none()
        );
    }

    #[test]
    fn geometry_inner_is_padded_and_drops_the_title_row() {
        let b = Rect::new(0, 0, 100, 50);
        // titled: full 24x8, centered at (38,21); inner inset by PAD, below the title.
        let g = PanelGeometry::compute(b, 20, 5, Some("t"), 1.0);
        assert_eq!(g.outer(), Some(Rect::new(38, 21, 24, 8)));
        assert_eq!(g.inner(), Some(Rect::new(40, 23, 20, 5)));
        // untitled: no title row → inner one row higher and one taller.
        let u = PanelGeometry::compute(b, 20, 5, None, 1.0);
        assert_eq!(u.outer(), Some(Rect::new(38, 21, 24, 7)));
        assert_eq!(u.inner(), Some(Rect::new(40, 22, 20, 5)));
    }

    #[test]
    fn geometry_scale_centers_off_the_scaled_dims() {
        let b = Rect::new(0, 0, 100, 50);
        // full 44x13; at 0.5 → 22x7 centered off the SCALED size (not full-then-scaled).
        let g = PanelGeometry::compute(b, 40, 10, Some("t"), 0.5);
        assert_eq!(g.outer(), Some(Rect::new(39, 21, 22, 7)));
    }

    #[test]
    fn geometry_cell_rect_maps_and_clips() {
        let b = Rect::new(0, 0, 100, 50);
        let g = PanelGeometry::compute(b, 20, 5, Some("t"), 1.0); // inner {40,23,20,5}
        assert_eq!(g.cell_rect(0, 0, 5), Some(Rect::new(40, 23, 5, 1)));
        assert_eq!(g.cell_rect(2, 3, 4), Some(Rect::new(43, 25, 4, 1)));
        // len clamps at the inner right edge (60)
        assert_eq!(g.cell_rect(0, 18, 10), Some(Rect::new(58, 23, 2, 1)));
        // col at/past the right edge → None (the phantom-launch clip)
        assert_eq!(g.cell_rect(0, 20, 5), None);
        // row at/past the bottom edge (28) → None
        assert_eq!(g.cell_rect(5, 0, 3), None);
        // guarded geom → None
        assert_eq!(
            PanelGeometry::compute(b, 20, 5, Some("t"), 0.0).cell_rect(0, 0, 3),
            None
        );
    }

    #[test]
    fn panel_inner_width_agrees_with_compute_inner_width() {
        let b = Rect::new(0, 0, 100, 50);
        for &cw in &[8u16, 20, 60] {
            for &s in &[1.0f32, 0.9, 0.5] {
                // content_rows tall enough that the height guard passes at every scale,
                // so the only difference between the two is that panel_inner_width is
                // height-independent — the widths must still match.
                let inner_w = PanelGeometry::compute(b, cw, 20, Some("t"), s)
                    .inner()
                    .map(|r| r.width);
                assert_eq!(panel_inner_width(b, cw, s), inner_w, "cw={cw} scale={s}");
            }
        }
    }

    #[test]
    fn inner_rect_raw_fallback_at_the_min_width() {
        // outer width == 2*PAD_X: no room to inset → raw area, title kept.
        let raw = inner_rect(Rect::new(0, 0, PANEL_PAD_X * 2, 10), true);
        assert_eq!(raw, Rect::new(0, 0, PANEL_PAD_X * 2, 10));
        // one wider → inset by pad on each side.
        assert_eq!(
            inner_rect(Rect::new(0, 0, PANEL_PAD_X * 2 + 1, 10), false).width,
            1
        );
    }

    // ---- window_range / overflow_cue (the shared list overflow) --------------

    #[test]
    fn window_range_fits_without_a_cue() {
        let w = window_range(4, Some(0), 0, 10);
        assert_eq!((w.start, w.count, w.cue), (0, 4, None));
    }

    #[test]
    fn window_range_reserves_a_line_for_the_cue_when_overflowing() {
        // 20 rows, 6-row viewport, selection at top → 5 rows shown + a cue of 15.
        let w = window_range(20, Some(0), 0, 6);
        assert_eq!((w.start, w.count), (0, 5));
        assert_eq!(w.cue, Some(15));
    }

    #[test]
    fn window_range_drops_the_cue_when_selection_reaches_the_end() {
        // Selection is the last row → nothing below → full viewport, no cue.
        let w = window_range(20, Some(19), 0, 6);
        assert_eq!(w.cue, None);
        assert_eq!((w.start, w.count), (14, 6)); // 19 + 1 - 6
    }

    #[test]
    fn window_range_follows_selection_below_the_window() {
        // Selection mid-list drags the window down to keep it visible.
        let w = window_range(30, Some(12), 0, 8);
        // reserved = 7; probe = 12+1-7 = 6; 30 > 6+7 → cue; window 7 from start 6.
        assert_eq!((w.start, w.count), (6, 7));
        assert_eq!(w.cue, Some(30 - (6 + 7)));
    }

    #[test]
    fn overflow_cue_text_is_the_shared_vocabulary() {
        assert_eq!(overflow_cue(3), "  \u{22ee} 3 more \u{25be}");
    }

    #[test]
    fn paint_panel_windows_a_long_list_and_pins_the_chrome() {
        // The overflow payoff: a long list with fixed above/below chrome on a
        // short terminal windows the list WITH a cue while the chrome stays put —
        // the connection/welcome/dashboard "no longer clips the footer" fix.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
        term.draw(|f| {
            let above = vec![Line::from("HEADERLINE")];
            let below = vec![Line::from("FOOTERLINE")];
            let list: Vec<Line<'static>> =
                (0..20).map(|i| Line::from(format!("row{i:02}"))).collect();
            paint_panel(
                f,
                &pixtuoid_scene::theme::NORMAL,
                Some("T"),
                Rect::new(0, 0, 40, 12),
                30,
                1.0,
                above,
                list,
                below,
                Overflow::Follow {
                    selected: Some(0),
                    scroll: 0,
                    cap: None,
                },
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let mut text = String::new();
        for y in 0..12 {
            for x in 0..40 {
                text.push_str(buf.cell((x, y)).unwrap().symbol());
            }
        }
        assert!(text.contains("HEADERLINE"), "above chrome must stay pinned");
        assert!(text.contains("FOOTERLINE"), "below chrome must stay pinned");
        assert!(text.contains("more"), "overflow cue must show on overflow");
        assert!(text.contains("row00"), "the windowed list top shows");
        assert!(!text.contains("row19"), "far list rows are windowed out");
    }
}
