//! Ratatui widget paint functions: footer, labels, wall display, tooltips,
//! ticker queue, and theme picker overlay.

mod help;
mod hud;
mod tooltip;

pub(super) use help::paint_help_overlay;
pub(super) use hud::{
    paint_elevator_indicator, paint_footer, paint_theme_picker, paint_version_popup,
    paint_wall_display, version_popup_url_rect, VERSION_POPUP_URL,
};
// `pub`: the snapshot example reuses the real formatter for its
// --source-warning screenshots so the wording cannot drift from production
// (the pixtuoid lib target is not a semver surface).
pub use hud::source_warning_message;
pub use tooltip::paint_chitchat_bubbles;
pub(super) use tooltip::{paint_coffee_tooltip, paint_furniture_tooltip, paint_pet_tooltip};
pub(crate) use tooltip::{paint_hover_tooltip, paint_label_widgets};

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::Color;

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
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
    use pixtuoid_core::{AgentId, AgentSlot};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tooltip::truncate_label;

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        let out = truncate_label("TikTok-Android\u{00b7}a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("\u{00b7}a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }

    #[test]
    fn truncate_label_plain_take_when_suffix_exceeds_budget() {
        // The disambig suffix ("·abcdefgh") is longer than budget=4, so the
        // suffix-preserving branch can't fit and it falls through to a plain
        // budget-char take from the front.
        let out = truncate_label("x\u{00b7}abcdefgh", 4);
        assert_eq!(out.chars().count(), 4);
        assert_eq!(out, "x\u{00b7}ab");
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

            desk_index: 0,
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
        let theme = &crate::tui::theme::NORMAL;
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
        let theme = &crate::tui::theme::NORMAL;
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
