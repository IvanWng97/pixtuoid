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
pub(crate) use panel::{borderless_panel, PANEL_PAD_X, PANEL_PAD_Y};
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

/// A `desired_w × desired_h` rect clamped to `bounds` and centered within it,
/// anchored off `bounds`'s origin (not 0,0) so a non-zero-origin bounds rect
/// positions correctly. Shared by the keyboard-help and theme-picker overlays.
/// The width-clamp also keeps `Clear::render` (which does not intersect the
/// buffer area) from panicking on a too-narrow terminal.
fn centered_in(bounds: Rect, desired_w: u16, desired_h: u16) -> Rect {
    let w = desired_w.min(bounds.width);
    let h = desired_h.min(bounds.height);
    Rect {
        x: bounds.x + bounds.width.saturating_sub(w) / 2,
        y: bounds.y + bounds.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
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
mod tests {
    use super::*;
    use footer::{build_status_spans, build_status_summary, FooterStats};
    use pixtuoid_core::state::ActivityState;
    use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, SceneState};
    use std::path::PathBuf;
    use std::sync::Arc;
    use wall_board::BOARD_W;

    // --- scene_stats -------------------------------------------------------

    fn stat_slot(path: &str, state: ActivityState, exiting: bool) -> AgentSlot {
        let now = SystemTime::UNIX_EPOCH;
        AgentSlot {
            agent_id: AgentId::from_transcript_path(path),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "x".into(),
            state,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: exiting.then_some(now),
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        }
    }

    #[test]
    fn scene_stats_buckets_exiting_first_and_totals() {
        use pixtuoid_core::state::ToolKind;
        let active = || ActivityState::Active {
            tool_use_id: None,
            detail: None,
            kind: ToolKind::Other,
        };
        let mut scene = SceneState::uniform(16);
        for s in [
            // An exiting agent buckets as EXITING even though its last state is Active —
            // the one policy that keeps the footer and board from disagreeing on a walkout.
            stat_slot("/exiting-active.jsonl", active(), true),
            stat_slot("/live-active.jsonl", active(), false),
            stat_slot(
                "/waiting.jsonl",
                ActivityState::Waiting {
                    reason: Arc::from("perm"),
                },
                false,
            ),
            stat_slot("/idle.jsonl", ActivityState::Idle, false),
        ] {
            scene.agents.insert(s.agent_id, s);
        }
        let c = scene_stats(&scene);
        assert_eq!(
            c.exiting, 1,
            "an exiting agent buckets as exiting even mid-Active"
        );
        assert_eq!(c.active, 1, "only the LIVE Active counts as active");
        assert_eq!(c.waiting, 1);
        assert_eq!(c.idle, 1);
        assert_eq!(c.total, 4);
        assert_eq!(
            c.active + c.waiting + c.idle + c.exiting,
            c.total,
            "the four buckets must partition the total"
        );
    }

    #[test]
    fn scene_stats_empty_scene_is_all_zero() {
        let c = scene_stats(&SceneState::uniform(16));
        assert_eq!(c, StateCounts::default());
        assert_eq!(c.total, 0);
    }

    // --- state vocabulary --------------------------------------------------

    #[test]
    fn state_vocab_is_total_and_distinct() {
        use std::collections::HashSet;
        let kinds = StateKind::ALL;
        assert_eq!(kinds.len(), 4, "the vocab covers exactly the four buckets");
        // Every state must be distinguishable on EACH redundant channel, so the
        // design never hinges on a single one (colour, glyph shape, or letter).
        let glyphs: HashSet<char> = kinds.iter().map(|k| k.glyph()).collect();
        let letters: HashSet<char> = kinds.iter().map(|k| k.letter()).collect();
        let words: HashSet<&str> = kinds.iter().map(|k| k.word()).collect();
        assert_eq!(glyphs.len(), 4, "each state has a distinct glyph");
        assert_eq!(letters.len(), 4, "each state has a distinct letter");
        assert_eq!(words.len(), 4, "each state has a distinct word");
        // The reserved amber "needs-you" hue and the exiting hue map to their
        // existing theme roles (label_waiting is amber; label_exiting is live).
        let t = &pixtuoid_scene::theme::NORMAL;
        assert_eq!(StateKind::Waiting.color(t), to_color(t.ui.label_waiting));
        assert_eq!(StateKind::Exiting.color(t), to_color(t.ui.label_exiting));
    }

    #[test]
    fn display_width_counts_terminal_columns_not_chars() {
        // The state/HUD glyphs are all East-Asian *ambiguous* = 1 column under the
        // non-CJK `.width()`, so this measure == chars().count() for them (why the
        // swap is snapshot-neutral), while still being correct for wide glyphs.
        assert_eq!(display_width("\u{b7}\u{d7}\u{2191}\u{2193}"), 4); // · × ↑ ↓
        assert_eq!(
            display_width("\u{25cf}\u{25d0}\u{25cb}\u{25cc}"),
            4,
            "● ◐ ○ ◌ are one column each"
        );
        assert_eq!(display_width("[q]uit"), 6);
        // A wide glyph is TWO columns (chars().count() would say 1) — the case that
        // keeps the footer's right-flush correct once a wide chip can appear.
        assert_eq!(display_width("\u{1f99e}"), 2); // 🦞
                                                   // A zero-width combining mark adds no columns.
        assert_eq!(display_width("a\u{0301}"), 1);
    }

    #[test]
    fn state_count_maps_each_kind() {
        let c = StateCounts {
            active: 3,
            waiting: 2,
            idle: 7,
            exiting: 1,
            total: 13,
        };
        assert_eq!(state_count(c, StateKind::Active), 3);
        assert_eq!(state_count(c, StateKind::Waiting), 2);
        assert_eq!(state_count(c, StateKind::Idle), 7);
        assert_eq!(state_count(c, StateKind::Exiting), 1);
    }

    // --- office-wide plumbing (per-floor + gateway rollup) ------------------

    // Maps a desired render `DaemonState` to the stored axes. Busy needs a
    // (placeholder) in-flight run key because Busy is DERIVED from the run set,
    // never stored (#460) — so `gateway_rollup_is_worst_of` still exercises a
    // genuinely-Busy fixture rather than a silently-Idle one.
    fn daemon(state: pixtuoid_core::state::DaemonState) -> pixtuoid_core::state::DaemonPresence {
        use pixtuoid_core::state::{DaemonLiveness, DaemonState};
        let (liveness, in_flight_run_keys) = match state {
            DaemonState::Idle => (DaemonLiveness::UP, Default::default()),
            DaemonState::Busy => (
                DaemonLiveness::UP,
                ["fixture-run".to_string()]
                    .into_iter()
                    .collect::<std::collections::HashSet<String>>(),
            ),
            DaemonState::Degraded => (DaemonLiveness::Up { degraded: true }, Default::default()),
            DaemonState::Down => (DaemonLiveness::Down, Default::default()),
        };
        pixtuoid_core::state::DaemonPresence {
            liveness,
            active_sessions: 0,
            last_seen: SystemTime::UNIX_EPOCH,
            entered_at: SystemTime::UNIX_EPOCH,
            in_flight_run_keys,
            current_pid: None,
        }
    }

    #[test]
    fn gateway_rollup_is_worst_of() {
        use pixtuoid_core::state::DaemonState;
        use std::collections::BTreeMap;
        // Empty map → None (chip SUPPRESSED — distinct from Some(Down) = seen then died).
        assert_eq!(gateway_rollup(&BTreeMap::new()), None);
        // Single daemon → itself.
        let mut m = BTreeMap::new();
        m.insert("gw".to_string(), daemon(DaemonState::Busy));
        assert_eq!(gateway_rollup(&m), Some(DaemonState::Busy));
        // Worst-of across many: Idle + Degraded + Down → Down.
        m.insert("b".to_string(), daemon(DaemonState::Idle));
        m.insert("c".to_string(), daemon(DaemonState::Degraded));
        m.insert("d".to_string(), daemon(DaemonState::Down));
        assert_eq!(gateway_rollup(&m), Some(DaemonState::Down));
        // Degraded outranks Busy/Idle when nothing is Down.
        let mut m2 = BTreeMap::new();
        m2.insert("x".to_string(), daemon(DaemonState::Idle));
        m2.insert("y".to_string(), daemon(DaemonState::Degraded));
        assert_eq!(gateway_rollup(&m2), Some(DaemonState::Degraded));
    }

    #[test]
    fn per_floor_buckets_by_floor_idx() {
        use pixtuoid_core::state::ToolKind;
        let active = || ActivityState::Active {
            tool_use_id: None,
            detail: None,
            kind: ToolKind::Other,
        };
        let mut scene = SceneState::uniform(16);
        let add = |scene: &mut SceneState, path, state, exiting, floor: usize| {
            let mut s = stat_slot(path, state, exiting);
            s.floor_idx = floor;
            scene.agents.insert(s.agent_id, s);
        };
        add(&mut scene, "/f0a.jsonl", active(), false, 0);
        add(&mut scene, "/f0b.jsonl", active(), false, 0);
        add(
            &mut scene,
            "/f1w.jsonl",
            ActivityState::Waiting {
                reason: Arc::from("p"),
            },
            false,
            1,
        );
        add(&mut scene, "/f1x.jsonl", active(), true, 1); // exiting on floor 1
        add(&mut scene, "/f2i.jsonl", ActivityState::Idle, false, 2);

        let pf = per_floor_counts(&scene);
        assert_eq!((pf[0].active, pf[0].total), (2, 2));
        assert_eq!((pf[1].waiting, pf[1].exiting, pf[1].total), (1, 1, 2));
        assert_eq!((pf[2].idle, pf[2].total), (1, 1));
        assert_eq!(pf[3], StateCounts::default(), "an untouched floor is zero");
    }

    // --- marquee_window ----------------------------------------------------

    // A 10-char string scrolled in a 5-col window: max_off=5, scroll_ms=750,
    // pause=1200, cycle = 2*1200 + 2*750 = 3900. Phases (ms):
    //   [0,1200)        hold head  -> "ABCDE"
    //   [1200,1950)     scroll out -> off=(p-1200)/150
    //   [1950,3150)     hold tail  -> "FGHIJ"
    //   [3150,3900)     scroll back-> off=5-((p-3150)/150)
    const M: &str = "ABCDEFGHIJ";
    fn at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
    }

    #[test]
    fn marquee_fits_returns_unchanged_no_ellipsis() {
        // len <= width on both the exact and under cases — today's behavior.
        assert_eq!(marquee_window("short", 10, at(99_999)), "short");
        assert_eq!(marquee_window("EXACTLYTEN", 10, at(99_999)), "EXACTLYTEN");
    }

    #[test]
    fn marquee_zero_width_is_empty() {
        assert_eq!(marquee_window(M, 0, at(0)), "");
        assert_eq!(marquee_window("", 0, at(0)), "");
    }

    #[test]
    fn marquee_holds_head_then_tail() {
        // phase 0 -> head; phase 2000 (in [1950,3150)) -> tail.
        assert_eq!(marquee_window(M, 5, at(0)), "ABCDE");
        assert_eq!(marquee_window(M, 5, at(2000)), "FGHIJ");
    }

    #[test]
    fn marquee_scrolls_out_and_back() {
        // out: phase 1500 -> off=(300/150)=2 -> "CDEFG".
        assert_eq!(marquee_window(M, 5, at(1500)), "CDEFG");
        // back: phase 3450 -> off=5-(300/150)=3 -> "DEFGH".
        assert_eq!(marquee_window(M, 5, at(3450)), "DEFGH");
    }

    #[test]
    fn marquee_is_deterministic_and_cycles() {
        // Same (s,width,now) -> same window; one full cycle (3900ms) later is
        // identical (wallclock modulo).
        assert_eq!(
            marquee_window(M, 5, at(1500)),
            marquee_window(M, 5, at(1500))
        );
        assert_eq!(
            marquee_window(M, 5, at(1500)),
            marquee_window(M, 5, at(1500 + 3900))
        );
    }

    #[test]
    fn marquee_min_overflow_reaches_both_ends() {
        // len == width + 1 (max_off=1): the single-char travel must expose both
        // the first and last char. scroll_ms=150, cycle = 2*1200 + 2*150 = 2700.
        let s = "ABCDEF"; // len 6, width 5
        assert_eq!(marquee_window(s, 5, at(0)), "ABCDE"); // head
        assert_eq!(marquee_window(s, 5, at(1500)), "BCDEF"); // tail-hold [1350,2550)
    }

    #[test]
    fn marquee_never_panics_on_multibyte() {
        // Multi-byte chars must window by char, never slice a byte boundary.
        let s = "café·ünïcödé·scroll·test";
        for ms in [0u64, 500, 1500, 2500, 5000, 9999] {
            let out = marquee_window(s, 8, at(ms));
            assert_eq!(out.chars().count(), 8, "ms={ms}: {out:?}");
        }
    }

    #[test]
    fn marquee_or_truncate_selected_scrolls_unselected_ellipsizes() {
        // Selected (scrolling) emits no ellipsis; unselected keeps `…`.
        assert_eq!(marquee_or_truncate(M, 5, true, at(0)), "ABCDE");
        assert_eq!(marquee_or_truncate(M, 5, false, at(0)), "ABCD\u{2026}");
    }

    // --- build_status_summary ---------------------------------------------

    fn slot_with(state: ActivityState, label: &str) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(&format!("/p/{label}.jsonl")),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: label.into(),
            state,
            state_started_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            last_event_at: SystemTime::UNIX_EPOCH,
            exiting_at: None,
            pending_idle_at: None,

            desk_index: GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        }
    }
    fn active_with(detail: &str, label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from(detail)),
                kind: pixtuoid_core::state::ToolKind::from_display(detail),
            },
            label,
        )
    }
    fn waiting(label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Waiting {
                reason: Arc::from("perm"),
            },
            label,
        )
    }
    fn idle(label: &str) -> AgentSlot {
        slot_with(ActivityState::Idle, label)
    }
    fn active_kind(detail: &str, kind: pixtuoid_core::state::ToolKind, label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from(detail)),
                kind,
            },
            label,
        )
    }
    fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
        let mut s = SceneState::uniform(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
    }

    /// Assemble a `FooterStats` from the scene the way `draw_scene` does (no
    /// gateway, per-floor bucketed from the scene) and render the plain-string
    /// footer oracle.
    fn footer_line(
        scene: &SceneState,
        width: u16,
        floor_info: Option<crate::tui::renderer::FloorInfo>,
        warning: Option<&str>,
    ) -> String {
        let pf = per_floor_counts(scene);
        let stats = FooterStats {
            counts: scene_stats(scene),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        build_status_summary(scene, &stats, width, floor_info, warning)
    }

    const QUIT_SUFFIX: &str = " [?]help [p]ause [t]heme [q]uit ";

    // --- source-death footer warning (#157) -------------------------------

    #[test]
    fn source_warning_message_formats_by_death_count() {
        use pixtuoid_core::source::manager::SourceDeath;
        let d = |s: &str| SourceDeath::new(s, "boom");
        assert_eq!(super::source_warning_message(&[]), None);
        assert_eq!(
            super::source_warning_message(&[d("claude-code")]).unwrap(),
            "claude-code source died — its agents are frozen; restart pixtuoid (see log)"
        );
        assert_eq!(
            super::source_warning_message(&[d("claude-code"), d("codex")]).unwrap(),
            "2 sources died — restart pixtuoid (see log)"
        );
    }

    #[test]
    fn footer_source_warning_replaces_stats_and_keeps_quit() {
        let s = scene_of(vec![idle("myproject")]);
        let line = footer_line(
            &s,
            100,
            None,
            Some("claude-code source died — its agents are frozen; restart pixtuoid (see log)"),
        );
        assert!(line.contains('⚠'), "warning marker present: {line}");
        assert!(line.contains("claude-code source died"), "got: {line}");
        assert!(line.ends_with(" [q]uit "), "quit hint survives: {line}");
        assert!(
            !line.contains('\u{25cb}') && !line.contains('\u{25cf}'),
            "stale state rungs are replaced by the warning: {line}"
        );
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_source_warning_survives_every_width() {
        let s = scene_of(vec![idle("myproject")]);
        for w in [20u16, 30, 40, 60, 80] {
            let line = footer_line(
                &s,
                w,
                None,
                Some("claude-code source died — its agents are frozen; restart pixtuoid (see log)"),
            );
            assert!(
                line.contains('⚠') || line.contains('…'),
                "warning must never be tiered away (w={w}): {line}"
            );
            assert!(
                line.chars().count() <= w as usize,
                "must fit the row (w={w}): {line:?}"
            );
        }
    }

    #[test]
    fn footer_zero_agents() {
        let s = scene_of(vec![]);
        let line = footer_line(&s, 80, None, None);
        assert_eq!(line.len(), 80, "should pad to full width");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_single_idle_agent() {
        let s = scene_of(vec![idle("myproject")]);
        let line = footer_line(&s, 80, None, None);
        // FULL tier: bare count then the sole idle rung `○1 I`.
        assert!(line.contains(" 1 \u{b7} \u{25cb}1 I"), "got: {line}");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_full_width_mixed_states() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            active_with("Edit src/b.rs", "b"),
            active_with("Bash: ls", "c"),
            waiting("d"),
            waiting("e"),
            idle("f"),
            idle("g"),
            idle("h"),
        ]);
        let line = footer_line(&s, 120, None, None);
        // Every non-zero rung + the aggregate tool tally (glyph+count+letter).
        for frag in [
            "\u{25cf}3 A",
            "\u{25d0}2 W",
            "\u{25cb}3 I",
            "Edit\u{d7}2",
            "Bash\u{d7}1",
        ] {
            assert!(line.contains(frag), "missing {frag:?} in: {line}");
        }
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_medium_width_compact() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
        ]);
        let line = footer_line(&s, 60, None, None);
        // Medium drops the tool tally + separators; compact rungs `●1A ◐1W ○1I`.
        assert!(!line.contains("Edit"), "medium drops tools: {line}");
        assert!(line.contains("\u{25cf}1A"), "compact active rung: {line}");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_minimal_width() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let w = QUIT_SUFFIX.len() + 6;
        let line = footer_line(&s, w as u16, None, None);
        assert_eq!(line.len(), w);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_quit_only_below_threshold() {
        let s = scene_of(vec![idle("a")]);
        let w = QUIT_SUFFIX.len();
        let line = footer_line(&s, w as u16, None, None);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_caps_tools_at_four() {
        let s = scene_of(vec![
            active_with("Edit x", "a"),
            active_with("Bash x", "b"),
            active_with("Read x", "c"),
            active_with("Write x", "d"),
            active_with("Grep x", "e"),
            active_with("Glob x", "f"),
        ]);
        let line = footer_line(&s, 200, None, None);
        let crosses = line.matches('\u{00d7}').count();
        assert_eq!(crosses, 4, "expected <=4 tools in breakdown");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_minimal_leads_with_waiting_alarm() {
        // The narrowest stats tier: the waiting ALARM (`▲N`) leads, then the count.
        let s = scene_of(vec![waiting("a"), waiting("b"), idle("c"), idle("d")]);
        let w = QUIT_SUFFIX.len() + 10;
        let line = footer_line(&s, w as u16, None, None);
        assert!(
            line.contains("\u{25b2}2 \u{b7} 4"),
            "▲2 · 4 (alarm leads): {line}"
        );
    }

    #[test]
    fn footer_death_keeps_the_waiting_alarm() {
        // Even a source-death warning (stats stale) keeps the must-not-miss `▲N`.
        let s = scene_of(vec![waiting("a"), waiting("b"), idle("c")]);
        let line = footer_line(&s, 120, None, Some("codex disconnected"));
        assert!(line.contains('\u{26a0}'), "warning present: {line}");
        assert!(
            line.contains("\u{25b2}2 need you"),
            "alarm survives death: {line}"
        );
    }

    fn fi(
        current: usize,
        total_floors: usize,
        total_agents: usize,
    ) -> crate::tui::renderer::FloorInfo {
        crate::tui::renderer::FloorInfo {
            current,
            total_floors,
            total_agents,
        }
    }

    #[test]
    fn footer_with_floor_info() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
        assert!(line.contains(" F2/3 "), "floor breadcrumb: {line}");
        insta::assert_snapshot!(line);
    }

    // Direct assertions for count_str — snapshot tests alone can mask
    // regressions because they're easy to ratify in `cargo insta review`.

    #[test]
    fn count_str_single_floor_shows_bare_n() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = footer_line(&s, 120, None, None);
        assert!(line.contains(" 2 \u{b7} \u{25cb}2 I"), "got: {line}");
        assert!(!line.contains("2/"), "no slash on a single floor: {line}");
    }

    #[test]
    fn count_str_multi_floor_shows_n_slash_total() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
        assert!(line.contains(" 2/5 \u{b7}"), "got: {line}");
    }

    #[test]
    fn count_str_multi_floor_shows_slash_even_when_total_equals_n() {
        // All agents happen to be on the visible floor — still show "/n"
        // to signal the multi-floor context.
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = footer_line(&s, 120, Some(fi(1, 3, 2)), None);
        assert!(line.contains(" 2/2 \u{b7}"), "got: {line}");
    }

    #[test]
    fn count_str_empty_floor_still_shows_total() {
        // The whole point of `total_agents`: when the current floor is
        // empty but other floors have agents, the footer must signal that.
        let s = scene_of(vec![]);
        let line = footer_line(&s, 120, Some(fi(2, 3, 5)), None);
        assert!(line.contains(" 0/5 "), "got: {line}");
    }

    #[test]
    fn count_str_multi_floor_keeps_slash_at_narrow_tier() {
        // Unlike the old footer, the redesign keeps `n/total` at EVERY tier
        // (the design's MEDIUM/MIN both show the slash) — the office context
        // matters most when space is tight.
        let s = scene_of(vec![idle("a"), idle("b"), idle("c")]);
        let line = footer_line(&s, 50, Some(fi(1, 3, 10)), None);
        assert!(
            line.contains("3/10"),
            "slash kept at the narrow tier: {line}"
        );
    }

    // --- build_status_spans ------------------------------------------------

    fn footer_spans_text(
        scene: &SceneState,
        width: u16,
        floor_info: Option<crate::tui::renderer::FloorInfo>,
        theme: &pixtuoid_scene::theme::Theme,
    ) -> String {
        let pf = per_floor_counts(scene);
        let stats = FooterStats {
            counts: scene_stats(scene),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        build_status_spans(scene, &stats, width, floor_info, theme, None)
            .iter()
            .map(|sp| sp.content.as_ref().to_string())
            .collect()
    }

    // Drift guard: the colored footer must render the SAME text as the
    // plain-string footer across every tier — they share `status_segments`,
    // so concatenating the spans must equal build_status_summary exactly.
    #[test]
    fn status_spans_text_matches_summary_across_tiers() {
        let theme = &pixtuoid_scene::theme::NORMAL;
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
            idle("d"),
        ]);
        for (w, fl) in [
            (120u16, None),
            (60, None),
            (28, None),
            (10, None),
            (120, Some(fi(2, 3, 9))),
        ] {
            let summary = footer_line(&s, w, fl, None);
            let spans_text = footer_spans_text(&s, w, fl, theme);
            assert_eq!(spans_text, summary, "tier width {w} drifted");
        }
    }

    #[test]
    fn status_spans_color_code_state_segments() {
        let theme = &pixtuoid_scene::theme::NORMAL;
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
        ]);
        let pf = per_floor_counts(&s);
        let stats = FooterStats {
            counts: scene_stats(&s),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        let spans = build_status_spans(&s, &stats, 120, None, theme, None);
        // The rungs are found by their vocabulary glyph, tinted via StateKind.
        let active = spans
            .iter()
            .find(|sp| sp.content.contains('\u{25cf}'))
            .unwrap();
        let waiting = spans
            .iter()
            .find(|sp| sp.content.contains('\u{25d0}'))
            .unwrap();
        assert_eq!(active.style.fg, Some(to_color(theme.ui.label_active)));
        assert_eq!(waiting.style.fg, Some(to_color(theme.ui.label_waiting)));
    }

    // --- T7 named tests: the redesign's two anti-drift anchors --------------

    #[test]
    fn footer_counts_agree_with_board_on_walkout() {
        // 2 active + 1 exiting. OLD: footer counted all 3 as agents while the
        // board counted 2 live — they disagreed mid-walkout. NOW both read the
        // shared `scene_stats`, and the footer shows a first-class exiting rung.
        let mut gone = active_with("Edit x", "gone");
        gone.exiting_at = Some(SystemTime::UNIX_EPOCH);
        let s = scene_of(vec![
            active_with("Edit a", "a"),
            active_with("Bash b", "b"),
            gone,
        ]);
        let c = scene_stats(&s);
        assert_eq!((c.active, c.exiting, c.total), (2, 1, 3), "shared spine");
        let line = footer_line(&s, 160, None, None);
        assert!(
            line.contains(" 3 \u{b7} \u{25cf}2 A"),
            "total incl. exiting: {line}"
        );
        assert!(
            line.contains("\u{25cc}1 x"),
            "first-class exiting rung: {line}"
        );
    }

    #[test]
    fn footer_tool_hue_reads_kind_field() {
        // A Task dispatch DISPLAYS "Delegating" but its typed kind is Task — the
        // tool segment must tint via ToolKind::Task's glow (== glow.agent), NEVER
        // a re-parse of the "Delegating" string (C7).
        let theme = &pixtuoid_scene::theme::NORMAL;
        let s = scene_of(vec![active_kind(
            "Delegating",
            pixtuoid_core::state::ToolKind::Task,
            "lead",
        )]);
        let pf = per_floor_counts(&s);
        let stats = FooterStats {
            counts: scene_stats(&s),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        let spans = build_status_spans(&s, &stats, 160, None, theme, None);
        let tool = spans
            .iter()
            .find(|sp| sp.content.contains("Delegating"))
            .expect("tool segment present");
        let expected = to_color(pixtuoid_scene::pixel_painter::tool_glow_for_kind(
            pixtuoid_core::state::ToolKind::Task,
            &theme.tool_glow,
        ));
        assert_eq!(tool.style.fg, Some(expected), "hue from the typed kind");
        assert_eq!(
            expected,
            to_color(theme.tool_glow.agent),
            "== the agent glow"
        );
    }

    #[test]
    fn footer_gateway_chip_reflects_rollup_and_suppresses_when_absent() {
        use pixtuoid_core::state::DaemonState;
        let s = scene_of(vec![idle("a")]);
        let pf = per_floor_counts(&s);
        let with_gw = FooterStats {
            counts: scene_stats(&s),
            per_floor: &pf,
            gateway: Some(DaemonState::Degraded),
            audio_audible: false,
            volume_flash: None,
        };
        let line = build_status_summary(&s, &with_gw, 160, None, None);
        assert!(line.contains("\u{2b22}gw err"), "degraded chip: {line}");
        let no_gw = FooterStats {
            counts: scene_stats(&s),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        let line2 = build_status_summary(&s, &no_gw, 160, None, None);
        assert!(
            !line2.contains("gw"),
            "chip suppressed when no daemon: {line2}"
        );
    }

    #[test]
    fn footer_cross_floor_alarm_points_at_waiting_floor() {
        // On floor 1, floor 2 (index 1) has a waiting agent → a `▲F2` cue in the
        // right-flushed floor suffix, telling you where to switch (C1: present
        // even though `per_floor` is office-wide, not the projected floor).
        let s = scene_of(vec![idle("a")]);
        let mut pf = per_floor_counts(&s);
        pf[1].waiting = 1;
        pf[1].total = 1;
        let stats = FooterStats {
            counts: scene_stats(&s),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        let line = build_status_summary(&s, &stats, 160, Some(fi(1, 3, 2)), None);
        assert!(
            line.contains("\u{25b2}F2"),
            "cross-floor waiting cue: {line}"
        );
    }

    // --- the wall board's mood pulse ---------------------------------------
    // The mood-pulse content tests (echoes counts / beacon leads / abbreviates /
    // empty-office) moved WITH `board_mood_segments` into
    // `pixtuoid_scene::board::tests`. What stays here is the binary-local pin that
    // `BOARD_W` tracks the painted panel's interior width.

    #[test]
    fn board_width_pins_to_neon_panel_interior() {
        // Anti-drift spine 2: the board text width IS the painted panel's dark
        // INTERIOR (outer width minus the frame on each side), so the lit letters
        // sit inside the glowing frame — never overrunning it (the overflow bug was
        // pinning BOARD_W to the full OUTER NEON_PANEL_W).
        assert_eq!(
            BOARD_W,
            pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W,
            "board width must equal the painted panel's dark interior width"
        );
        // (interior < outer frame is enforced at COMPILE time in pixel_painter.)
    }
}
