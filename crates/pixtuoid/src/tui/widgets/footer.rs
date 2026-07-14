use std::collections::HashMap;

use pixtuoid_core::state::{ActivityState, DaemonState, ToolKind, MAX_FLOORS};
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{display_width, state_count, to_color, StateCounts, StateKind};

/// The pre-computed office/floor tallies the footer renders — assembled once per
/// frame at each of the three `paint_footer` sites (C3). `counts` is the CURRENT
/// (projected) floor's per-state breakdown (the rungs); `per_floor` + `gateway`
/// are office-wide (the cross-floor `▲F{n}` cue + the `⬢gw` chip), always present
/// so they render on a single-floor office too (C1).
pub(crate) struct FooterStats<'a> {
    pub counts: StateCounts,
    pub per_floor: &'a [StateCounts; MAX_FLOORS],
    pub gateway: Option<DaemonState>,
}

/// One-line footer warning for dead sources (#157); `None` while healthy.
/// Deliberately terse — it shares the footer row — with the full error in the
/// log file (written by default since #157's logging fix; a failed log-file
/// install is announced on pre-altscreen stderr). `pub`: the snapshot
/// example's --source-warning reuses this exact formatter so screenshots
/// can't drift from production wording.
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

/// Per-segment color role for the footer. The tier-selection logic emits a list
/// of `(text, role)` pieces once; the plain-string and colored-span renderers
/// both consume that list, so their text is always byte-identical and only the
/// color differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SegRole {
    /// Labels, separators, counts, padding, quit hint — muted.
    Neutral,
    /// An activity-state rung — delegates its hue to the shared `StateKind`
    /// vocabulary (so footer/board/tooltip state colours can't drift).
    State(StateKind),
    /// A tool tally segment — hue from the TYPED `ToolKind` (C7), the same
    /// monitor-glow colour the sprite shows, NEVER a re-parse of the name.
    Tool(ToolKind),
    /// The gateway `⬢gw` chip — hue by daemon liveness.
    Gateway(DaemonState),
    /// Source-death warning (#157) — reuses the Waiting attention color
    /// rather than adding a theme key (the nearest themed "needs your eyes").
    Warning,
}

impl SegRole {
    fn color(self, theme: &pixtuoid_scene::theme::Theme) -> Color {
        match self {
            SegRole::Neutral => to_color(theme.ui.label_idle),
            SegRole::State(kind) => kind.color(theme),
            SegRole::Tool(kind) => to_color(pixtuoid_scene::pixel_painter::tool_glow_for_kind(
                kind,
                &theme.tool_glow,
            )),
            SegRole::Gateway(state) => match state {
                DaemonState::Idle => to_color(theme.ui.label_idle),
                DaemonState::Busy => to_color(theme.ui.label_active),
                DaemonState::Degraded | DaemonState::Down => to_color(theme.ui.label_waiting),
            },
            SegRole::Warning => to_color(theme.ui.label_waiting),
        }
    }
}

/// Build the footer as an ordered list of `(text, role)` segments, picking the
/// widest tier (full / medium / minimal) that fits inside `term_width` alongside
/// the fixed-right quit suffix. Single source of truth for both the plain-string
/// oracle (`build_status_summary`) and the colored footer (`build_status_spans`).
///
/// Tier breakdown (state rungs carry glyph+count+letter — see `StateKind`):
///   * **full** — `n/total`, a rung per non-zero state incl. a first-class
///     Exiting rung, the aggregate tool tally, and the gateway chip, e.g.
///     `13/20 · ●3 A · ◐2 W · ○7 I · ◌1 x · Edit×2 · ⬢gw ok`.
///   * **medium** — compact rungs (no separators/exiting/tools/chip), e.g.
///     `13/20 · ●3A ◐2W ○7I`.
///   * **minimal** — the waiting alarm LEADS (survives to the narrowest tier)
///     then the count, e.g. `▲2 · 13/20`.
///   * **fallback** — only the quit hint.
fn status_segments(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) -> Vec<(String, SegRole)> {
    let counts = stats.counts;
    // A dead source outranks the stats (#157): the counts below silently go
    // stale once a transport is gone, so the warning IS the status until
    // restart. It survives every width — truncated to fit rather than tiered
    // away — but the waiting ALARM (`▲N need you`) rides along, the one
    // must-not-miss datum even in a partially-frozen office (design DEATH tier).
    if let Some(warn) = source_warning {
        let w = term_width as usize;
        let quit = " [q]uit ";
        let avail = w.saturating_sub(quit.len());
        let alarm = if counts.waiting > 0 {
            format!(" · \u{25b2}{} need you", counts.waiting)
        } else {
            String::new()
        };
        // The `⚠ {warn}` body truncates to fit, but the `▲N need you` alarm is
        // PINNED — the one must-not-miss datum rides through to the narrowest
        // width (design DEATH tier). Only when the fixed chrome + alarm ALONE
        // overflow does the alarm itself get cut. (The old code appended the
        // alarm to `text` before a blanket truncate, so at narrow widths it was
        // the TAIL cut first — the exact datum the comment promised to keep.)
        let prefix = " \u{26a0} ";
        let suffix = " ";
        let full = format!("{prefix}{warn}{alarm}{suffix}");
        let text = if display_width(&full) <= avail {
            full
        } else {
            let chrome = display_width(prefix) + display_width(suffix) + display_width(&alarm);
            let body_budget = avail.saturating_sub(chrome);
            if body_budget >= 1 {
                let mut body: String = warn.chars().take(body_budget.saturating_sub(1)).collect();
                body.push('\u{2026}');
                format!("{prefix}{body}{alarm}{suffix}")
            } else {
                // Too narrow even for the pinned alarm — truncate the whole line.
                let mut t: String = full.chars().take(avail.saturating_sub(1)).collect();
                t.push('\u{2026}');
                t
            }
        };
        let pad = w.saturating_sub(display_width(&text) + quit.len());
        let mut out = vec![(text, SegRole::Warning)];
        if pad > 0 {
            out.push((" ".repeat(pad), SegRole::Neutral));
        }
        out.push((quit.to_string(), SegRole::Neutral));
        return out;
    }

    // `n/total` — the floor's own agent count over the office total (the office
    // total is the sum of the per-floor tallies, == FloorInfo.total_agents).
    // Single-floor offices show just the floor count (no slash).
    let count_str = match floor_info {
        Some(fi) => format!("{}/{}", counts.total, fi.total_agents),
        None => format!("{}", counts.total),
    };

    // The aggregate tool tally: group Active slots by their raw display token
    // (kept verbatim) but carry the TYPED ToolKind for the hue (C7) — a Task
    // slot displays "Delegating" yet tints via `kind = Task`, never the name.
    let mut tool_counts: HashMap<&str, (ToolKind, usize)> = HashMap::new();
    for slot in scene.agents.values() {
        if let ActivityState::Active { detail, kind, .. } = &slot.state {
            if let Some(token) = detail
                .as_deref()
                .and_then(|d| d.split(|c: char| !c.is_alphanumeric()).next())
                .filter(|t| !t.is_empty())
            {
                tool_counts.entry(token).or_insert((*kind, 0)).1 += 1;
            }
        }
    }
    let mut tools: Vec<(&str, ToolKind, usize)> = tool_counts
        .iter()
        .map(|(name, (kind, count))| (*name, *kind, *count))
        .collect();
    tools.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));
    tools.truncate(4);

    // Floor breadcrumb + the cross-floor `▲F{n}` cue: any OTHER floor holding a
    // waiting agent (the one you'd want to switch to). Rides the right-flushed
    // quit suffix so it's present at every tier that keeps the suffix.
    let cross_floor = floor_info.and_then(|fi| {
        let cur = fi.current.saturating_sub(1);
        (0..MAX_FLOORS)
            .find(|&fl| fl != cur && stats.per_floor[fl].waiting > 0)
            .map(|fl| fl + 1)
    });
    let floor_suffix = match floor_info {
        Some(fi) => {
            let cross = match cross_floor {
                Some(n) => format!(" \u{25b2}F{n}"),
                None => String::new(),
            };
            format!(
                " F{}/{}{cross} [\u{2191}\u{2193}]",
                fi.current, fi.total_floors
            )
        }
        None => String::new(),
    };
    let quit = format!("{floor_suffix} [?]help [p]ause [t]heme [q]uit ");

    // --- tier builders --------------------------------------------------------
    // An empty office reads as a bare count on every tier (the board owns the
    // friendly "— office empty —").
    if counts.total == 0 {
        return finish_tier(
            vec![(format!(" {count_str} "), SegRole::Neutral)],
            &quit,
            term_width,
        );
    }

    let seg_full = {
        let mut segs = vec![(format!(" {count_str}"), SegRole::Neutral)];
        for kind in StateKind::ALL {
            let c = state_count(counts, kind);
            if c == 0 {
                continue;
            }
            segs.push((" · ".to_string(), SegRole::Neutral));
            segs.push((
                format!("{}{} {}", kind.glyph(), c, kind.letter()),
                SegRole::State(kind),
            ));
        }
        if !tools.is_empty() {
            segs.push((" · ".to_string(), SegRole::Neutral));
            for (i, (name, kind, count)) in tools.iter().enumerate() {
                if i > 0 {
                    segs.push((" ".to_string(), SegRole::Neutral));
                }
                segs.push((format!("{name}\u{d7}{count}"), SegRole::Tool(*kind)));
            }
        }
        if let Some(g) = stats.gateway {
            segs.push((" · ".to_string(), SegRole::Neutral));
            segs.push((
                format!(
                    "{}gw {}",
                    pixtuoid_scene::board::GATEWAY_GLYPH,
                    pixtuoid_scene::board::gateway_label(g)
                ),
                SegRole::Gateway(g),
            ));
        }
        segs.push((" ".to_string(), SegRole::Neutral));
        segs
    };

    // Medium: compact rungs for the three resident states (exiting/tools/chip
    // drop out for width); space-separated `{glyph}{count}{letter}`.
    let seg_medium = {
        let mut rungs: Vec<(String, SegRole)> = Vec::new();
        for kind in [StateKind::Active, StateKind::Waiting, StateKind::Idle] {
            let c = state_count(counts, kind);
            if c == 0 {
                continue;
            }
            if !rungs.is_empty() {
                rungs.push((" ".to_string(), SegRole::Neutral));
            }
            rungs.push((
                format!("{}{}{}", kind.glyph(), c, kind.letter()),
                SegRole::State(kind),
            ));
        }
        let mut segs = vec![(format!(" {count_str} \u{b7} "), SegRole::Neutral)];
        segs.extend(rungs);
        segs.push((" ".to_string(), SegRole::Neutral));
        segs
    };

    // Minimal: the waiting alarm LEADS (the last stat to survive), then count.
    let seg_min = if counts.waiting > 0 {
        vec![
            (
                format!(" \u{25b2}{}", counts.waiting),
                SegRole::State(StateKind::Waiting),
            ),
            (format!(" \u{b7} {count_str} "), SegRole::Neutral),
        ]
    } else {
        vec![(format!(" {count_str} "), SegRole::Neutral)]
    };

    for tier in [seg_full, seg_medium, seg_min] {
        let stats_len: usize = tier.iter().map(|(s, _)| display_width(s)).sum();
        if stats_len + display_width(&quit) <= term_width as usize {
            return finish_tier(tier, &quit, term_width);
        }
    }
    vec![(quit, SegRole::Neutral)]
}

/// Right-flush a chosen stats tier: pad the gap between it and the fixed quit
/// suffix so `[q]uit` sits at the exact edge (display-column measured).
fn finish_tier(
    mut tier: Vec<(String, SegRole)>,
    quit: &str,
    term_width: u16,
) -> Vec<(String, SegRole)> {
    let stats_len: usize = tier.iter().map(|(s, _)| display_width(s)).sum();
    let pad = (term_width as usize).saturating_sub(stats_len + display_width(quit));
    if pad > 0 {
        tier.push((" ".repeat(pad), SegRole::Neutral));
    }
    tier.push((quit.to_string(), SegRole::Neutral));
    tier
}

/// Plain-string footer — renders `status_segments` to text. Test-only: it
/// is the text-contract oracle (insta snapshots + direct substring asserts)
/// that locks the exact footer wording, byte-identical to the colored
/// `build_status_spans` content. Production paints via `build_status_spans`.
#[cfg(test)]
pub(crate) fn build_status_summary(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    source_warning: Option<&str>,
) -> String {
    status_segments(scene, stats, term_width, floor_info, source_warning)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// Colored footer — same segments as `build_status_summary`, each tinted by
/// its role so state / tool / gateway pieces scan by hue.
pub(crate) fn build_status_spans<'a>(
    scene: &SceneState,
    stats: &FooterStats<'_>,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
    theme: &pixtuoid_scene::theme::Theme,
    source_warning: Option<&str>,
) -> Vec<Span<'a>> {
    status_segments(scene, stats, term_width, floor_info, source_warning)
        .into_iter()
        .map(|(s, role)| Span::styled(s, Style::default().fg(role.color(theme))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // status_segments' tool-token guard: a detail whose first split token is
    // empty (leading non-alphanumeric) must be SKIPPED, not counted as a tool.
    #[test]
    fn status_segments_skips_empty_leading_token() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/lead.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "lead".into(),
            // Leading '/' ⇒ first token after split-on-non-alphanumeric is "".
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("/usr/bin/thing")),
                kind: pixtuoid_core::state::ToolKind::Other,
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        // No '×' tool breakdown token survives — the empty leading token was
        // skipped, so the active agent contributes no tool count.
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        let line = build_status_summary(&scene, &stats, 200, None, None);
        assert!(
            !line.contains('\u{00d7}'),
            "empty leading token must not produce a tool count: {line}"
        );
        assert!(
            line.contains("\u{25cf}1 A"),
            "active rung still shows: {line}"
        );
    }

    // Footer tier-selection + padding must measure DISPLAY COLUMNS, not bytes:
    // the full tier carries single-column multi-byte glyphs (·, ×), so a byte
    // measure over-counts width and short-pads the row below the terminal width.
    #[test]
    fn status_segments_pads_to_full_column_width_with_multibyte_glyphs() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/mb.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "mb".into(),
            // A real tool token ⇒ the tail carries `· Bash×1` (·/× are 2-byte,
            // 1-column glyphs).
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        let width: u16 = 200; // wide enough that the full tier is selected either way
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        let segs = status_segments(&scene, &stats, width, None, None);
        let cols: usize = segs.iter().map(|(s, _)| display_width(s)).sum();
        assert_eq!(
            cols, width as usize,
            "footer must fill the full width in display columns: {segs:?}"
        );
        assert!(
            segs.iter().any(|(s, _)| s.contains('\u{00d7}')),
            "full tier (with the tool breakdown) expected at width 200: {segs:?}"
        );
    }

    // DEATH tier: a source-death warning replaces the stats, but the `▲N need you`
    // alarm is the one must-not-miss datum — it must survive to a width so narrow
    // the warning body itself is truncated. (Regression: the alarm used to be
    // appended to the text BEFORE a blanket truncate, so it was the tail cut first.)
    #[test]
    fn death_tier_pins_the_waiting_alarm_through_the_narrowest_width() {
        use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
        use std::path::PathBuf;
        use std::sync::Arc;
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/p/wait.jsonl"),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "w".into(),
            state: ActivityState::Waiting {
                reason: Arc::from("permission"),
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
        };
        let mut scene = SceneState::uniform(16);
        scene.agents.insert(slot.agent_id, slot);
        let pf = crate::tui::widgets::per_floor_counts(&scene);
        let stats = FooterStats {
            counts: crate::tui::widgets::scene_stats(&scene),
            per_floor: &pf,
            gateway: None,
        };
        // A warning long enough that the body must truncate at this width.
        let warn = "transport pixtuoid-hook died: connection refused after 3 retries";
        let line = build_status_summary(&scene, &stats, 40, None, Some(warn));
        assert!(
            line.contains("\u{25b2}1 need you"),
            "the ▲N alarm must survive even when the warning body is truncated: {line}"
        );
        assert!(
            line.contains('\u{2026}'),
            "the warning body itself IS truncated at this width (proving the alarm was pinned, not merely fit): {line}"
        );
    }
}
