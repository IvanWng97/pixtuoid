use pixtuoid_core::state::{DaemonState, MAX_FLOORS};
use pixtuoid_core::SceneState;
use pixtuoid_scene::footer::{
    build_footer, footer_tone_rgb, footer_tool_tally, FooterFloor, FooterInputs, ToolTally,
};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{to_color, StateCounts};

const KEYS_STATS: &str = " [?]help [p]ause [t]heme [q]uit ";
const KEYS_ALERT: &str = " [q]uit ";

/// `counts` is the CURRENT (projected) floor's per-state breakdown; `per_floor`
/// and `gateway` are office-wide, and are present even on a single-floor office.
pub(crate) struct FooterStats<'a> {
    pub counts: StateCounts,
    pub per_floor: &'a [StateCounts; MAX_FLOORS],
    pub gateway: Option<DaemonState>,
    /// The audio system is live AND not effectively muted (m-state OR pause).
    pub audio_audible: bool,
    /// `Some(percent)` for ~1s after a volume nudge; renders as `♩ N%`.
    pub volume_flash: Option<u8>,
}

/// One-line footer warning for dead sources; `None` while healthy. `pub` because the
/// snapshot example reuses this exact formatter, so screenshots can't drift from
/// production wording.
pub fn source_warning_message(
    deaths: &[pixtuoid_core::source::manager::SourceDeath],
) -> Option<String> {
    match deaths {
        [] => None,
        [d] => Some(format!(
            "{} source died — its agents are frozen; restart pixtuoid (see log)",
            d.source
        )),
        many => Some(format!(
            "{} sources died — restart pixtuoid (see log)",
            many.len()
        )),
    }
}

fn footer_inputs<'a>(
    stats: &FooterStats<'a>,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&'a str>,
    tools: &'a [ToolTally],
) -> FooterInputs<'a> {
    FooterInputs {
        counts: stats.counts,
        per_floor: stats.per_floor,
        gateway: stats.gateway,
        floor: floor_info.map(|fi| FooterFloor {
            current: fi.current,
            total_floors: fi.total_floors,
            total_agents: fi.total_agents,
        }),
        tools,
        audio_audible: stats.audio_audible,
        volume_flash: stats.volume_flash,
        source_warning,
        keys_stats: KEYS_STATS,
        keys_alert: KEYS_ALERT,
    }
}

pub(crate) fn paint_footer(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    stats: &FooterStats<'_>,
    full_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) {
    use ratatui::text::Line;
    let spans = build_status_spans(
        scene,
        stats,
        full_rect.width,
        floor_info,
        theme,
        source_warning,
    );
    // Base style on the whole row so cells past the rendered spans keep the muted
    // footer tone rather than the terminal default.
    let footer =
        Paragraph::new(Line::from(spans)).style(Style::default().fg(to_color(theme.ui.label_idle)));
    f.render_widget(
        footer,
        Rect {
            x: full_rect.x,
            y: full_rect.y + full_rect.height.saturating_sub(1),
            width: full_rect.width,
            height: 1,
        },
    );
}

pub(crate) fn build_status_spans<'a>(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    theme: &pixtuoid_scene::theme::Theme,
    source_warning: Option<&str>,
) -> Vec<Span<'a>> {
    let tools = footer_tool_tally(scene);
    let inputs = footer_inputs(stats, floor_info, source_warning, &tools);
    build_footer(&inputs, term_width)
        .segments
        .into_iter()
        .map(|seg| {
            Span::styled(
                seg.text,
                Style::default().fg(to_color(footer_tone_rgb(seg.tone, theme))),
            )
        })
        .collect()
}

/// Byte-identical to `build_status_spans`'s content — the oracle that locks the exact
/// footer wording.
#[cfg(test)]
pub(crate) fn build_status_summary(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) -> String {
    let tools = footer_tool_tally(scene);
    let inputs = footer_inputs(stats, floor_info, source_warning, &tools);
    build_footer(&inputs, term_width).text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::state::ActivityState;
    use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
    use pixtuoid_scene::footer::{FooterTone, RungKind};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;

    #[test]
    fn build_status_spans_tints_the_active_rung_via_the_shared_tone_authority() {
        let theme = &pixtuoid_scene::theme::NORMAL;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/a.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "a".into(),
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("Bash ls")),
                kind: pixtuoid_core::state::ToolKind::Bash,
            },
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
            audio_audible: false,
            volume_flash: None,
        };
        let spans = build_status_spans(&scene, &stats, 200, None, theme, None);
        let active_rgb = footer_tone_rgb(FooterTone::Rung(RungKind::Active), theme);
        let rung = spans
            .iter()
            .find(|s| s.content.contains("\u{25cf}1 A"))
            .expect("active rung span present");
        assert_eq!(rung.style.fg, Some(to_color(active_rgb)));
    }
}
