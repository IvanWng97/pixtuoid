//! Ratatui widget paint functions.

mod connection;
mod dashboard;
mod elevator;
mod footer;
mod help;
mod panel;
mod theme_picker;
mod tooltip;
mod version_popup;
mod wall_board;
mod welcome;

pub(super) use connection::paint_connection_panel;
pub(super) use dashboard::paint_dashboard;
pub(super) use elevator::paint_elevator_indicator;
pub(super) use footer::{paint_footer, FooterStats};
pub(super) use help::paint_help_overlay;
pub(crate) use panel::{borderless_panel, paint_panel, panel_inner_width, Overflow, PanelGeometry};
pub(super) use theme_picker::paint_theme_picker;
pub use tooltip::paint_chitchat_bubbles;
pub(super) use tooltip::{
    paint_coffee_tooltip, paint_furniture_tooltip, paint_mascot_tooltip, paint_pet_tooltip,
};
pub(crate) use tooltip::{paint_hover_tooltip, paint_label_widgets};
pub(super) use version_popup::{paint_version_popup, version_popup_url_rect, VERSION_POPUP_URL};
pub(super) use wall_board::{paint_wall_display, star_hit_rect};
pub(super) use welcome::paint_welcome;
// `pub`: the snapshot example reuses the real formatter so its --source-warning
// screenshots cannot drift from production.
pub use footer::source_warning_message;
// `pub`: the bin crate's crash reporter derives its issue-report URL from this one
// authority.
pub use version_popup::REPO_URL;

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Clear};

use pixtuoid_scene::theme::Theme;

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// Display columns a string occupies in the terminal — the ONE width authority (the same
/// `unicode-width` ratatui uses), replacing scattered `chars().count()` so a wide glyph
/// in a HUD widget can't miscount its layout.
pub(crate) fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

// `StateCounts` stays `pub`: it is reachable via the pub `DrawCtx::per_floor` field.
pub use pixtuoid_scene::board::StateCounts;
pub(crate) use pixtuoid_scene::board::{
    compact_hms, gateway_rollup, per_floor_counts, scene_stats,
};

// Each state carries FOUR redundant channels (glyph/letter/word/hue); hue is never the
// sole carrier, so the design survives colour removal, a colour-blind viewer, and a
// terminal that tofus a glyph.
pub(crate) use pixtuoid_scene::footer::RungKind as StateKind;

/// A [`StateKind`]'s themed ratatui hue — the binary shim over the shared
/// [`footer_tone_rgb`](pixtuoid_scene::footer::footer_tone_rgb) authority, so the
/// footer/tooltip/dashboard state colours can't drift from the footer model's. It can't
/// be an inherent method: it returns a ratatui `Color` on the foreign `RungKind`.
pub(crate) fn state_color(kind: StateKind, theme: &Theme) -> Color {
    to_color(pixtuoid_scene::footer::footer_tone_rgb(
        pixtuoid_scene::footer::FooterTone::Rung(kind),
        theme,
    ))
}

/// The drop shadow's single uniform darkening factor (0 = black, 1 = unchanged).
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
            (r as f32 * f) as u8,
            (g as f32 * f) as u8,
            (b as f32 * f) as u8,
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
