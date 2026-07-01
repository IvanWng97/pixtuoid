//! Ratatui widget paint functions: footer, labels, wall display, tooltips,
//! ticker queue, and theme picker overlay.

mod connection;
mod dashboard;
mod help;
mod hud;
mod panel;
mod tooltip;
mod welcome;

pub(super) use connection::paint_connection_panel;
pub(super) use dashboard::paint_dashboard;
pub(super) use help::paint_help_overlay;
pub(super) use hud::{
    paint_elevator_indicator, paint_footer, paint_theme_picker, paint_version_popup,
    paint_wall_display, version_popup_url_rect, VERSION_POPUP_URL,
};
pub(crate) use panel::{borderless_panel, PANEL_PAD_X, PANEL_PAD_Y};
pub(super) use welcome::paint_welcome;
// `pub`: the snapshot example reuses the real formatter for its
// --source-warning screenshots so the wording cannot drift from production
// (the pixtuoid lib target is not a semver surface).
pub use hud::source_warning_message;
pub use tooltip::paint_chitchat_bubbles;
pub(super) use tooltip::{
    paint_coffee_tooltip, paint_furniture_tooltip, paint_mascot_tooltip, paint_pet_tooltip,
};
pub(crate) use tooltip::{paint_hover_tooltip, paint_label_widgets};

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Clear};

use pixtuoid_scene::theme::Theme;

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
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
/// slices a multi-byte glyph).
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

/// Time (ms) the marquee dwells on each character while scrolling — matches the
/// wall ticker's 150ms cadence (~6.7 chars/sec).
const MARQUEE_MS_PER_CHAR: u64 = 150;
/// Time (ms) the marquee holds at each end (head / tail) before reversing.
const MARQUEE_END_PAUSE_MS: u64 = 1200;

/// Visible char-window of `s` for a ping-pong auto-scrolling field `width`
/// columns wide, at time `now`. If `s` fits, it is returned unchanged (the
/// caller pads/uses it exactly as it would `truncate`'s output). Otherwise it
/// bounces — hold head → scroll to tail → hold tail → scroll back — purely as a
/// function of `now`, with NO per-frame state (same wallclock trick as
/// `TickerQueue::visible`, so two painters can call it freely). Char-windowed,
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

/// Format a duration in seconds as a compact `"{h}h{m}m"` / `"{m}m"` / `"<1m"`
/// string (no prefix). The HUD uptime badge prepends "↑"; the tooltip uses the
/// bare form. Bucket thresholds: ≥1h shows hours+minutes, ≥1m shows minutes.
fn compact_hms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        "<1m".to_string()
    }
}

/// Persistent scrolling ticker queue. Messages append to the end and scroll
/// off the left naturally — like a news crawl. The queue rebuilds only when
/// the set of active tool details changes, preserving scroll continuity.
pub struct TickerQueue {
    buffer: String,
    last_snapshot: String,
}

impl Default for TickerQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TickerQueue {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_snapshot: String::new(),
        }
    }

    pub fn update(&mut self, scene: &SceneState) {
        let mut items: Vec<String> = scene
            .agents
            .values()
            .filter(|a| a.exiting_at.is_none())
            .filter_map(|a| match &a.state {
                ActivityState::Active { detail, .. } => {
                    let tool = detail.as_deref().unwrap_or("working");
                    Some(format!("{}: {}", a.label, tool))
                }
                ActivityState::Waiting { reason } => Some(format!("{}: ?{}", a.label, reason)),
                _ => None,
            })
            .collect();
        items.sort();
        let snapshot = items.join("|");
        if snapshot != self.last_snapshot {
            self.last_snapshot = snapshot;
            for item in &items {
                self.buffer.push_str(item);
                self.buffer.push_str("  |  ");
            }
            const MAX_CHARS: usize = 512;
            let char_count = self.buffer.chars().count();
            if char_count > MAX_CHARS {
                let trim_chars = char_count - MAX_CHARS;
                if let Some((byte_idx, _)) = self.buffer.char_indices().nth(trim_chars) {
                    self.buffer.drain(..byte_idx);
                }
            }
        }
    }

    pub fn visible(&self, width: usize, now: SystemTime) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }
        let elapsed_ms = now
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let chars: Vec<char> = self.buffer.chars().collect();
        let len = chars.len();
        let offset = (elapsed_ms / 150) as usize % len;
        (0..width).map(|i| chars[(offset + i) % len]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hud::{build_status_spans, build_status_summary};
    use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
    use std::path::PathBuf;
    use std::sync::Arc;

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

    // --- TickerQueue -------------------------------------------------------

    #[test]
    fn ticker_default_is_empty() {
        let q = TickerQueue::default();
        assert_eq!(q.visible(40, SystemTime::UNIX_EPOCH), "");
    }

    #[test]
    fn ticker_includes_waiting_reason() {
        let mut q = TickerQueue::new();
        let s = scene_of(vec![waiting("perm-agent")]);
        q.update(&s);
        // The Waiting arm formats "{label}: ?{reason}".
        let text = q.visible(200, SystemTime::UNIX_EPOCH);
        assert!(text.contains("perm-agent"), "got: {text}");
        assert!(text.contains('?'), "waiting marker missing: {text}");
    }

    #[test]
    fn ticker_trims_buffer_past_max() {
        let mut q = TickerQueue::new();
        // Push many distinct snapshots so the buffer grows past MAX_CHARS=512
        // and the drain path runs. Each update with a NEW snapshot appends.
        for i in 0..200 {
            let label = format!("agent-with-a-fairly-long-name-{i:04}");
            let s = scene_of(vec![active_with("Edit some/long/path.rs", &label)]);
            q.update(&s);
        }
        // Buffer must have been trimmed: visible() still works and the kept
        // text stays bounded near MAX_CHARS rather than growing unbounded.
        let text = q.visible(40, SystemTime::UNIX_EPOCH);
        assert_eq!(text.chars().count(), 40, "visible window must fill");
        assert!(
            q.buffer.chars().count() <= 512,
            "buffer must be trimmed to MAX_CHARS, got {}",
            q.buffer.chars().count()
        );
    }

    // --- build_status_summary ---------------------------------------------

    fn slot_with(state: ActivityState, label: &str) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(&format!("/p/{label}.jsonl")),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from(label),
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
        }
    }
    fn active_with(detail: &str, label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from(detail)),
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
    fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
        let mut s = SceneState::uniform(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
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
        let line = build_status_summary(
            &s,
            100,
            None,
            Some("claude-code source died — its agents are frozen; restart pixtuoid (see log)"),
        );
        assert!(line.contains('⚠'), "warning marker present: {line}");
        assert!(line.contains("claude-code source died"), "got: {line}");
        assert!(line.ends_with(" [q]uit "), "quit hint survives: {line}");
        assert!(
            !line.contains(" 1 agents") && !line.contains("idle"),
            "stale stats are replaced by the warning: {line}"
        );
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_source_warning_survives_every_width() {
        let s = scene_of(vec![idle("myproject")]);
        for w in [20u16, 30, 40, 60, 80] {
            let line = build_status_summary(
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
        let line = build_status_summary(&s, 80, None, None);
        assert_eq!(line.len(), 80, "should pad to full width");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_single_idle_agent() {
        let s = scene_of(vec![idle("myproject")]);
        let line = build_status_summary(&s, 80, None, None);
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
        let line = build_status_summary(&s, 120, None, None);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_medium_width_compact() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
        ]);
        let line = build_status_summary(&s, 60, None, None);
        assert!(
            !line.contains("3 agents"),
            "full tier should not fit at width 60"
        );
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_minimal_width() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let w = QUIT_SUFFIX.len() + 6;
        let line = build_status_summary(&s, w as u16, None, None);
        assert_eq!(line.len(), w);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_quit_only_below_threshold() {
        let s = scene_of(vec![idle("a")]);
        let w = QUIT_SUFFIX.len();
        let line = build_status_summary(&s, w as u16, None, None);
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
        let line = build_status_summary(&s, 200, None, None);
        let crosses = line.matches('\u{00d7}').count();
        assert_eq!(crosses, 4, "expected <=4 tools in breakdown");
        insta::assert_snapshot!(line);
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
        let line = build_status_summary(&s, 120, Some(fi(2, 3, 5)), None);
        insta::assert_snapshot!(line);
    }

    // Direct assertions for count_str — snapshot tests alone can mask
    // regressions because they're easy to ratify in `cargo insta review`.

    #[test]
    fn count_str_single_floor_shows_bare_n() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = build_status_summary(&s, 120, None, None);
        assert!(line.contains(" 2 agents "), "got: {line}");
        assert!(
            !line.contains("2/"),
            "should not show slash on single floor"
        );
    }

    #[test]
    fn count_str_multi_floor_shows_n_slash_total() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = build_status_summary(&s, 120, Some(fi(2, 3, 5)), None);
        assert!(line.contains(" 2/5 agents "), "got: {line}");
    }

    #[test]
    fn count_str_multi_floor_shows_slash_even_when_total_equals_n() {
        // All agents happen to be on the visible floor — still show "/n"
        // to signal the multi-floor context.
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = build_status_summary(&s, 120, Some(fi(1, 3, 2)), None);
        assert!(line.contains(" 2/2 agents "), "got: {line}");
    }

    #[test]
    fn count_str_empty_floor_still_shows_total() {
        // The whole point of `total_agents`: when the current floor is
        // empty but other floors have agents, the footer must signal that.
        let s = scene_of(vec![]);
        let line = build_status_summary(&s, 120, Some(fi(2, 3, 5)), None);
        assert!(line.contains(" 0/5 agents "), "got: {line}");
    }

    #[test]
    fn count_str_narrow_tier_uses_bare_n() {
        // "5/12a" is ambiguous at narrow widths; medium/min tiers must
        // drop the slash form regardless of multi-floor status.
        let s = scene_of(vec![idle("a"), idle("b"), idle("c")]);
        let line = build_status_summary(&s, 60, Some(fi(1, 3, 10)), None);
        assert!(
            !line.contains("3/10"),
            "medium tier should not show slash: {line}"
        );
        assert!(line.contains("3a"), "got: {line}");
    }

    // --- build_status_spans ------------------------------------------------

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
            let summary = build_status_summary(&s, w, fl, None);
            let spans_text: String = build_status_spans(&s, w, fl, theme, None)
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect();
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
        let spans = build_status_spans(&s, 120, None, theme, None);
        let active = spans
            .iter()
            .find(|sp| sp.content.contains("active"))
            .unwrap();
        let waiting = spans
            .iter()
            .find(|sp| sp.content.contains("waiting"))
            .unwrap();
        assert_eq!(active.style.fg, Some(to_color(theme.ui.label_active)));
        assert_eq!(waiting.style.fg, Some(to_color(theme.ui.label_waiting)));
    }
}
