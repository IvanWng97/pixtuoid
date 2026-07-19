//! Ratatui widget paint functions: footer, labels, wall display, tooltips,
//! and theme picker overlay.

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
// `pub`: the snapshot example reuses the real formatter for its
// --source-warning screenshots so the wording cannot drift from production
// (the pixtuoid lib target is not a semver surface).
pub use footer::source_warning_message;
// `pub`: the BIN crate's crash reporter (crash.rs, a main.rs module — a separate
// crate) derives its issue-report URL from this one authority (same rationale as
// source_warning_message above).
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

/// Display columns a string occupies in the terminal — the ONE width authority
/// (the same `unicode-width` ratatui uses), replacing scattered `chars().count()`
/// so a wide glyph in the footer/board can't miscount the right-flush. For the
/// HUD's ambiguous-width glyphs (`·×↑↓●◐○◌`) this equals `chars().count()`; it
/// diverges only for genuinely wide (2-col) or zero-width (combining) chars.
pub(crate) fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

// --- Shared scene stats (spine 1: footer + board agree) -----------------------
// The per-scene activity tally, the gateway rollup, and `compact_hms` moved to
// the backend-agnostic `pixtuoid_scene::board` module so `pixtuoid-web` can build
// the wall board too (not just this binary). Re-exported here under their original
// names, so the footer/board call sites are unchanged. `StateCounts` stays `pub`
// (reachable via the pub `DrawCtx::per_floor` field, like its peer `FloorInfo`);
// the binary lib target is not a semver surface. `StateCounts::get` couldn't move
// as an inherent method (orphan rule — `StateKind` is binary-local), so it's the
// free fn `state_count` below.
pub use pixtuoid_scene::board::StateCounts;
pub(crate) use pixtuoid_scene::board::{
    compact_hms, gateway_rollup, per_floor_counts, scene_stats,
};

/// The count for one [`StateKind`] — lets a consumer iterate [`StateKind::ALL`]
/// and pull the matching tally without re-matching. A free fn (not the old
/// `StateCounts::get`) because `StateCounts` is now a foreign type
/// (`pixtuoid_scene::board`) and `StateKind` is binary-local — an inherent impl
/// would violate the orphan rule.
pub(crate) fn state_count(counts: StateCounts, kind: StateKind) -> usize {
    match kind {
        StateKind::Active => counts.active,
        StateKind::Waiting => counts.waiting,
        StateKind::Idle => counts.idle,
        StateKind::Exiting => counts.exiting,
    }
}

// --- Shared state vocabulary (glyph + letter + word + hue) --------------------
// ONE source for how an activity state reads on EVERY surface (footer, board,
// tooltip, and — later — the dashboard). Each state carries FOUR redundant
// channels; hue is never the sole carrier, so the design survives colour
// removal, a colour-blind viewer, and a terminal that tofus a glyph.

/// The four agent activity buckets as a shared vocabulary. `Waiting` owns the
/// reserved amber "needs-you" hue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateKind {
    Active,
    Waiting,
    Idle,
    Exiting,
}

impl StateKind {
    /// Canonical render order (the footer's left-to-right rung order).
    pub(crate) const ALL: [StateKind; 4] = [
        StateKind::Active,
        StateKind::Waiting,
        StateKind::Idle,
        StateKind::Exiting,
    ];

    /// A distinct geometric glyph per state — all East-Asian *ambiguous* width
    /// (1 cell in a non-CJK terminal): `●` active, `◐` waiting, `○` idle, `◌`
    /// exiting. The fill gradient IS the language: full=working, half=paused
    /// on you, empty=idle, dotted=leaving. (Every glyph is Monaspace-Neon-native
    /// — the single-face vocabulary gate in `aa_text`.)
    pub(crate) fn glyph(self) -> char {
        match self {
            StateKind::Active => '\u{25cf}',
            StateKind::Waiting => '\u{25d0}',
            StateKind::Idle => '\u{25cb}',
            StateKind::Exiting => '\u{25cc}',
        }
    }

    /// A distinct single letter — the primary colour-blind channel at the
    /// footer's narrow tier where the full word doesn't fit.
    pub(crate) fn letter(self) -> char {
        match self {
            StateKind::Active => 'A',
            StateKind::Waiting => 'W',
            StateKind::Idle => 'I',
            StateKind::Exiting => 'x',
        }
    }

    /// The full capitalized state word — the tooltip dossier's state line reads
    /// `{glyph} {word}` (the board uses its own casual `work`/`wait`/`idle`).
    pub(crate) fn word(self) -> &'static str {
        match self {
            StateKind::Active => "Active",
            StateKind::Waiting => "Waiting",
            StateKind::Idle => "Idle",
            StateKind::Exiting => "Exiting",
        }
    }

    /// The themed hue — reuses the existing `label_*` roles so state colour is
    /// identical to the name-badges and every other surface (`label_waiting` is
    /// the amber attention hue; `label_exiting` is already live).
    pub(crate) fn color(self, theme: &Theme) -> Color {
        to_color(match self {
            StateKind::Active => theme.ui.label_active,
            StateKind::Waiting => theme.ui.label_waiting,
            StateKind::Idle => theme.ui.label_idle,
            StateKind::Exiting => theme.ui.label_exiting,
        })
    }
}

// --- Shared borderless-card backing (shadow + clear + bg fill) ----------------
// The ONE place the "block board" look every borderless card sits on is defined.
// `borderless_panel` (modals) and the framed tooltips both delegate to
// `paint_card_backing`, so the drop shadow can't be applied inconsistently or
// silently forgotten by a future card.

/// The drop shadow's single uniform darkening factor (0 = black, 1 = unchanged) —
/// ONE flat color for the whole shadow, no gradient.
const SHADOW_FACTOR: f32 = 0.42;
/// How far the shadow silhouette is offset down-and-right of the card, in cells —
/// what makes it read as a cast box-shadow (the card floats above it) rather than
/// an outline. This is the width of the visible right band and the height of the
/// visible bottom band.
const SHADOW_OFFSET: u16 = 1;

/// Multiply an `Rgb` color toward black by `f`. Half-block office cells carry a
/// real RGB on BOTH `fg` (top sub-pixel) and `bg` (bottom sub-pixel), so a clean
/// shadow darkens both — ratatui's own `Block::shadow` tints bg-only / stamps a
/// shade glyph, which smears over the pixel art.
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

/// Darken the cell at `(x, y)` by the uniform `SHADOW_FACTOR`, if it is a real
/// `Rgb` and inside `bounds`. With `top_half_only`, darkens only the upper
/// half-block sub-pixel (`fg`) and leaves the lower one (`bg`) lit — a 1px-tall
/// line; otherwise darkens the whole cell. Bounds-checked so it never indexes past
/// the frame.
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
/// uniform `SHADOW_FACTOR` and offset `SHADOW_OFFSET` cells down-and-right. The card
/// is painted over its own cells afterward, so what stays visible is an even L-band
/// — a `SHADOW_OFFSET`-wide strip down the right and a `SHADOW_OFFSET`-tall strip
/// along the bottom, meeting at the corner, all ONE color. The bottom-most row of
/// the silhouette (the visible bottom band + corner) is rendered TOP-HALF only, so
/// the bottom shadow reads as a 1px contact line instead of a full 2px cell, while
/// the vertical right strip stays full cells. Bounds-checked per cell.
fn cast_drop_shadow(f: &mut ratatui::Frame<'_>, area: Rect) {
    let bounds = f.area();
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

/// Paint the shared backing for a borderless card over `area`: cast the drop
/// shadow into the office cells below-right, `Clear` the card's own cells, then
/// fill them with the solid `tooltip_bg`. Both `panel::borderless_panel` (modals)
/// and the framed tooltips delegate here, so the "block board" look — bg fill +
/// shadow — has one definition and can't drift between popup kinds.
fn paint_card_backing(f: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    cast_drop_shadow(f, area);
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(to_color(theme.ui.tooltip_bg))),
        area,
    );
}

/// The badge color for a source's 2-char label prefix — shared by the dashboard
/// and Sources-panel row painters. Resolves via `SourceColors::by_prefix`,
/// falling back to `label_idle` for an unknown prefix (the same fallback the
/// inlined `match` arms used). Never reversed at the call sites: a low-luminance
/// hue inverted vanishes against the highlight bg.
fn badge_color_for(tag: &str, theme: &pixtuoid_scene::theme::Theme) -> Color {
    to_color(theme.source.by_prefix(tag).unwrap_or(theme.ui.label_idle))
}

/// The `[xx]` two-letter source badge span, coloured by the source's theme hue.
/// The ONE badge builder shared by the dashboard, the Sources panel, AND the
/// tooltip dossier so the three can't drift (`tag` is a 2-char `label_prefix`).
/// Never REVERSED — a low-luminance hue inverted vanishes against a highlight bg,
/// so callers reverse the OTHER spans (name/state) on selection, never this one.
pub(crate) fn source_badge_span(tag: &str, theme: &Theme) -> ratatui::text::Span<'static> {
    ratatui::text::Span::styled(
        format!("[{tag:<2}]"),
        Style::default().fg(badge_color_for(tag, theme)),
    )
}

/// Truncate to `max` characters (char-safe), appending `…` when clipped. Shared
/// by the dashboard + connection popup row painters (display-column safe — never
/// slices a multi-byte glyph). Budget: the `…` is INCLUDED, so the clipped
/// output is EXACTLY `max` chars — unlike `decoder::ellipsize`, which excludes
/// it (N+1).
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

/// Time (ms) the marquee dwells on each character while scrolling
/// (~6.7 chars/sec) — the auto-scroll cadence for the dashboard/connection
/// selected-row fields.
const MARQUEE_MS_PER_CHAR: u64 = 150;
/// Time (ms) the marquee holds at each end (head / tail) before reversing.
const MARQUEE_END_PAUSE_MS: u64 = 1200;

/// Visible char-window of `s` for a ping-pong auto-scrolling field `width`
/// columns wide, at time `now`. If `s` fits, it is returned unchanged (the
/// caller pads/uses it exactly as it would `truncate`'s output). Otherwise it
/// bounces — hold head → scroll to tail → hold tail → scroll back — purely as a
/// function of `now`, with NO per-frame state (a stateless wallclock window, so
/// two painters can call it freely). Char-windowed,
/// matching `truncate` (single-column glyphs only; a wide CJK glyph would
/// misalign by a column mid-scroll — the same assumption `truncate` makes).
/// Unlike `truncate`, the scrolling window emits NO `…` — the motion signals
/// "more". `[p]ause` freezes `now`, which freezes the scroll.
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

/// The focused (selected) row auto-scrolls overflowing text via ping-pong; every
/// other row stays statically `…`-truncated. Both honor the same `width` contract
/// so the caller's fixed-width padding is unchanged. Shared by both popups.
fn marquee_or_truncate(s: &str, width: usize, selected: bool, now: SystemTime) -> String {
    if selected {
        marquee_window(s, width, now)
    } else {
        truncate(s, width)
    }
}

#[cfg(test)]
mod tests;
