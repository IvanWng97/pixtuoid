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

/// The TUI keybind tails handed to `build_footer` — the ONE place the terminal's
/// hint chrome is spelled. The stats tiers carry the full hint; the death tier
/// (a partially-frozen office) keeps only quit. (The floating painter supplies
/// its own tails — the tail is the one painter-specific input to the shared model.)
const KEYS_STATS: &str = " [?]help [p]ause [t]heme [q]uit ";
const KEYS_ALERT: &str = " [q]uit ";

/// The pre-computed office/floor tallies the footer renders — assembled once per
/// frame at each of the three `paint_footer` sites (C3). `counts` is the CURRENT
/// (projected) floor's per-state breakdown (the rungs); `per_floor` + `gateway`
/// are office-wide (the cross-floor `▲F{n}` cue + the `⬢gw` chip), always present
/// so they render on a single-floor office too (C1). Marshalled into
/// `pixtuoid_scene::footer::FooterInputs` — the shared model owns the tier policy.
pub(crate) struct FooterStats<'a> {
    pub counts: StateCounts,
    pub per_floor: &'a [StateCounts; MAX_FLOORS],
    pub gateway: Option<DaemonState>,
    /// "You would hear sound right now": the audio system is live AND not
    /// effectively muted (m-state OR pause). Drives the ♩ suffix glyph.
    pub audio_audible: bool,
    /// Transient +/- readout: `Some(percent)` for ~1s after a volume nudge
    /// (the lowfi volume-timer pattern) — renders as `♩ N%`.
    pub volume_flash: Option<u8>,
}

/// One-line footer warning for dead sources (#157); `None` while healthy.
/// Deliberately terse — it shares the footer row — with the full error in the
/// log file (written by default since #157's logging fix; a failed log-file
/// install is announced on pre-altscreen stderr). `pub`: the snapshot
/// example's --source-warning reuses this exact formatter so screenshots
/// can't drift from production wording. (The death>drift MERGE stays in
/// `doctor::footer_warning`; `build_footer` renders the merged string.)
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

/// Marshal this binary's per-frame footer state into the shared model's
/// [`FooterInputs`]. `tools` is held by the caller (borrowed here) so
/// `build_footer` stays a pure function of pre-computed inputs.
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
    // Base style on the whole row (label_idle) for parity with the old
    // single-Span footer: cells past the rendered spans (quit-only tier on a
    // wide-ish terminal) keep the muted footer tone rather than default.
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

/// Colored footer — renders the shared [`build_footer`] model, each segment
/// tinted by its tone via the ONE shared [`footer_tone_rgb`] authority (so the
/// TUI and floating painters can't drift), then to this backend's ratatui `Color`.
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

/// Plain-string footer — the text-contract oracle (insta snapshots + direct
/// substring asserts) that locks the exact footer wording. Delegates to the
/// shared model's [`FooterModel::text`](pixtuoid_scene::footer::FooterModel::text),
/// so it stays byte-identical to the colored `build_status_spans` content.
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

    // The pure tier/policy logic is pinned in `pixtuoid_scene::footer`'s own
    // tests (ported there in the model migration). These pin the BINARY ADAPTER:
    // the FooterStats→FooterInputs marshalling + the ratatui span coloring route
    // through the shared `footer_tone_rgb`, so a rung tints to `label_active` here
    // exactly as `footer_tone_rgb` resolves it.
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
        // The `●1 A` active rung span carries the label_active fg — proving the
        // adapter routes tone→color through the shared `footer_tone_rgb`.
        let active_rgb = footer_tone_rgb(FooterTone::Rung(RungKind::Active), theme);
        let rung = spans
            .iter()
            .find(|s| s.content.contains("\u{25cf}1 A"))
            .expect("active rung span present");
        assert_eq!(rung.style.fg, Some(to_color(active_rgb)));
    }
}
