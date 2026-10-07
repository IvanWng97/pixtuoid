//! Ratatui widget paint functions.

mod connection;
mod dashboard;
mod footer;
mod help;
mod panel;
mod theme_picker;
mod tooltip;
mod version_popup;
mod welcome;

pub(super) use connection::paint_connection_panel;
pub(super) use dashboard::paint_dashboard;
pub use footer::footer_context;
pub(super) use footer::paint_footer;
pub(super) use help::paint_help_overlay;
pub(crate) use panel::{Overflow, Panel, PanelGeometry, borderless_panel};
pub(super) use theme_picker::paint_theme_picker;
pub(crate) use tooltip::{TooltipAt, paint_badges, paint_text_runs, paint_tooltip};
pub(crate) use version_popup::REPO_URL;
pub(super) use version_popup::{paint_version_popup, release_url, version_popup_url_rect};
pub(super) use welcome::paint_welcome;

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Clear};

use pixtuoid_scene::theme::Theme;

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// Display columns a string occupies in the terminal:
/// [`cells`](pixtuoid_scene::display::text::cells), the one width rule.
pub(crate) fn display_width(s: &str) -> usize {
    usize::from(pixtuoid_scene::display::text::cells(s))
}

#[cfg(test)]
pub(crate) use pixtuoid_scene::footer::RungKind as StateKind;

/// The drop shadow's single uniform darkening factor (0 = black, 1 = unchanged).
/// Uniform is an OWNER PREFERENCE, not an unfinished gradient. Pinned by
/// `borderless_panel_casts_a_flat_offset_shadow`.
const SHADOW_FACTOR: f32 = 0.42;
/// How far the shadow silhouette is offset down-and-right of the card, in cells — what
/// makes it read as a cast box-shadow (the card floats above it) rather than an outline.
const SHADOW_OFFSET: u16 = 1;

/// Multiply an `Rgb` color toward black by `f`. Half-block office cells carry a real RGB
/// on BOTH `fg` (top sub-pixel) and `bg` (bottom sub-pixel), so a clean shadow darkens
/// both — ratatui's own `Block::shadow` tints bg-only and smears over the pixel art.
fn dim_rgb(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (f32::from(r) * f) as u8,
            (f32::from(g) * f) as u8,
            (f32::from(b) * f) as u8,
        ),
        other => other,
    }
}

/// Darken the cell at `(x, y)` by the uniform `SHADOW_FACTOR`, if it is a real `Rgb` and
/// inside `bounds`. With `top_half_only`, darkens only the upper half-block sub-pixel
/// (`fg`) and leaves the lower one lit — a 1px-tall line.
fn dim_cell(f: &mut ratatui::Frame<'_>, x: u16, y: u16, bounds: Rect, top_half_only: bool) {
    if x < bounds.x || y < bounds.y || x >= bounds.right() || y >= bounds.bottom() {
        return;
    }
    let cell = &mut f.buffer_mut()[(x, y)];
    cell.fg = dim_rgb(cell.fg, SHADOW_FACTOR);
    if !top_half_only {
        cell.bg = dim_rgb(cell.bg, SHADOW_FACTOR);
    }
}

/// Cast a flat, single-color drop shadow: the card's own silhouette darkened by one
/// uniform `SHADOW_FACTOR` and offset `SHADOW_OFFSET` cells down-and-right. The
/// bottom-most row of the silhouette is rendered TOP-HALF only, so the bottom shadow
/// reads as a 1px contact line instead of a full 2px cell.
///
/// Clipped to `scene_rect`, NOT the frame: the footer is painted before every card and
/// the bottom band dims `fg` only, so a band reaching that row repaints the live `[q]uit`
/// over its still-lit bg. Keeping the card BODY off the footer
/// (`panel::RESERVED_FOOTER_ROWS`) is only half the rule — the silhouette is offset a row
/// further DOWN.
fn cast_drop_shadow(f: &mut ratatui::Frame<'_>, area: Rect) {
    let bounds = crate::tui::renderer::scene_rect(f.area());
    let sx = area.x.saturating_add(SHADOW_OFFSET);
    let sy = area.y.saturating_add(SHADOW_OFFSET);
    let last_row = sy.saturating_add(area.height.saturating_sub(1));
    for y in sy..sy.saturating_add(area.height) {
        let top_half_only = y == last_row;
        for x in sx..sx.saturating_add(area.width) {
            dim_cell(f, x, y, bounds, top_half_only);
        }
    }
}

/// Paint the shared backing for a borderless card over `area`: drop shadow, `Clear`, then
/// a solid `tooltip_bg` fill. Both `panel::borderless_panel` (modals) and the framed
/// tooltips delegate here, so the "block board" look can't drift between popup kinds.
fn paint_card_backing(f: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    cast_drop_shadow(f, area);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(to_color(theme.ui.tooltip_bg))),
        area,
    );
}

/// The badge color for a source's 2-char label prefix, falling back to `label_idle` for
/// an unknown prefix.
fn badge_color_for(tag: &str, theme: &pixtuoid_scene::theme::Theme) -> Color {
    to_color(theme.source.by_prefix(tag).unwrap_or(theme.ui.label_idle))
}

/// The `[xx]` two-letter source badge span, coloured by the source's theme hue. Never
/// REVERSED — a low-luminance hue inverted vanishes against a highlight bg, so callers
/// reverse the OTHER spans (name/state) on selection, never this one.
pub(crate) fn source_badge_span(tag: &str, theme: &Theme) -> ratatui::text::Span<'static> {
    ratatui::text::Span::styled(
        format!("[{tag:<2}]"),
        Style::default().fg(badge_color_for(tag, theme)),
    )
}

/// Truncate to `max` characters (char-safe), appending `…` when clipped. The `…` is
/// INCLUDED in the budget, so the clipped output is EXACTLY `max` chars — unlike
/// `decoder::ellipsize`, which excludes it (N+1).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('\u{2026}');
    out
}

/// Time (ms) the marquee dwells on each character while scrolling.
const MARQUEE_MS_PER_CHAR: u64 = 150;
/// Time (ms) the marquee holds at each end (head / tail) before reversing.
const MARQUEE_END_PAUSE_MS: u64 = 1200;

/// Visible char-window of `s` for a ping-pong auto-scrolling field `width` columns wide,
/// at time `now`. If `s` fits, it is returned unchanged. Otherwise it bounces — hold head
/// → scroll to tail → hold tail → scroll back — purely as a function of `now`, with NO
/// per-frame state, so two painters can call it freely. Char-windowed like `truncate` (a
/// wide CJK glyph would misalign by a column mid-scroll); unlike `truncate` it emits NO
/// `…` — the motion signals "more".
fn marquee_window(s: &str, width: usize, now: SystemTime) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    if len <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let max_off = len - width; // >= 1
    let scroll_ms = max_off as u64 * MARQUEE_MS_PER_CHAR; // >= MARQUEE_MS_PER_CHAR
    let pause = MARQUEE_END_PAUSE_MS;
    let cycle = 2 * pause + 2 * scroll_ms; // > 0
    let elapsed = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let phase = elapsed % cycle;
    let off = if phase < pause {
        0 // hold head
    } else if phase < pause + scroll_ms {
        (((phase - pause) / MARQUEE_MS_PER_CHAR) as usize).min(max_off) // scroll out
    } else if phase < 2 * pause + scroll_ms {
        max_off // hold tail
    } else {
        let back = (phase - (2 * pause + scroll_ms)) / MARQUEE_MS_PER_CHAR;
        max_off.saturating_sub(back as usize) // scroll back
    };
    chars[off..off + width].iter().collect()
}

/// The focused (selected) row auto-scrolls overflowing text via ping-pong; every other
/// row stays statically `…`-truncated. Both honor the same `width` contract, so the
/// caller's fixed-width padding is unchanged.
fn marquee_or_truncate(s: &str, width: usize, selected: bool, now: SystemTime) -> String {
    if selected {
        marquee_window(s, width, now)
    } else {
        truncate(s, width)
    }
}

#[cfg(test)]
mod tests;
