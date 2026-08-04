//! The agent-dashboard popup painter (ratatui). Pure presentation over the
//! pre-built row list from `tui::dashboard`; all model / fold / selection logic
//! lives there.

use std::time::SystemTime;

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::AgentId;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{marquee_or_truncate, paint_panel, source_badge_span, to_color, Overflow};
use crate::tui::dashboard::{DashboardRow, RowState, DASHBOARD_VIEWPORT_ROWS};
use pixtuoid_scene::theme::Theme;

const LABEL_W: usize = 32;
const STATE_W: usize = 28;
const POPUP_W: u16 = 76;

pub(crate) fn paint_dashboard(
    f: &mut ratatui::Frame<'_>,
    rows: &[DashboardRow],
    selected: Option<AgentId>,
    scroll: usize,
    now: SystemTime,
    bounds: Rect,
    theme: &Theme,
) {
    if rows.is_empty() {
        /// Fits "No active agents".
        const EMPTY_W: u16 = 24;
        paint_panel(
            f,
            theme,
            Some("Agents"),
            bounds,
            EMPTY_W,
            1.0,
            vec![],
            vec![Line::from(Span::styled(
                "No active agents",
                Style::default().fg(to_color(theme.ui.label_idle)),
            ))],
            vec![],
            Overflow::None,
        );
        return;
    }

    let title = format!(
        " Agents ({})  [\u{2191}\u{2193} \u{2190}\u{2192} z \u{23ce} esc] ",
        rows.len()
    );
    // Build EVERY row; `paint_panel` windows the list at the real inner height,
    // follows the selection, and appends the `⋮ N more ▾` cue. A `None` selection
    // (unselected OR exited) keeps the persisted scroll.
    let selected_idx = selected.and_then(|s| rows.iter().position(|r| r.agent_id == s));
    let list: Vec<Line<'static>> = rows
        .iter()
        .map(|row| dashboard_line(row, selected == Some(row.agent_id), now, theme))
        .collect();
    paint_panel(
        f,
        theme,
        Some(&title),
        bounds,
        POPUP_W,
        1.0,
        vec![],
        list,
        vec![],
        Overflow::Follow {
            selected: selected_idx,
            scroll,
            cap: Some(DASHBOARD_VIEWPORT_ROWS as u16),
        },
    );
}

fn dashboard_line(
    row: &DashboardRow,
    is_selected: bool,
    now: SystemTime,
    theme: &Theme,
) -> Line<'static> {
    let prefix = match (row.depth, row.collapsed, row.child_count) {
        (0, _, 0) => "  ".to_string(),
        (0, true, _) => "▸ ".to_string(),
        (0, false, _) => "▾ ".to_string(),
        _ => "  └ ".to_string(),
    };
    let mut name = format!("{prefix}{}", row.label);
    if row.collapsed && row.child_count > 0 {
        name.push_str(&format!(" ({})", row.child_count));
    }
    let label_cell = format!(
        "{:<LABEL_W$}",
        marquee_or_truncate(&name, LABEL_W, is_selected, now)
    );

    let (glyph, text, color) = match &row.state {
        RowState::Active(Some(detail)) => ('●', detail.to_string(), theme.ui.label_active),
        RowState::Active(None) => ('●', "active".to_string(), theme.ui.label_active),
        RowState::Waiting(reason) => ('◐', format!("waiting: {reason}"), theme.ui.label_waiting),
        RowState::Idle => ('○', "idle".to_string(), theme.ui.label_idle),
    };
    let state_cell = format!(
        "{glyph} {}",
        marquee_or_truncate(&text, STATE_W, is_selected, now)
    );

    let base = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Badge uses the source color but is NEVER reversed — a low-luminance hue
    // inverted becomes invisible against the highlight background.
    let badge_tag = descriptor_for(row.source.as_ref()).map_or("??", |d| d.label_prefix);

    Line::from(vec![
        source_badge_span(badge_tag, theme),
        Span::raw(" "),
        Span::styled(label_cell, base.fg(to_color(color))),
        Span::styled(
            format!(" F{:<2} ", row.floor_idx + 1),
            base.fg(to_color(theme.ui.neon_brand)),
        ),
        Span::styled(state_cell, base.fg(to_color(color))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::AgentId;
    use pixtuoid_scene::theme::NORMAL;
    use std::sync::Arc;

    fn make_row(source: &str, state: RowState, label: &str) -> DashboardRow {
        DashboardRow {
            agent_id: AgentId::from_transcript_path("/x.jsonl"),
            parent_id: None,
            depth: 0,
            label: label.into(),
            source: Arc::from(source),
            floor_idx: 0,
            state,
            child_count: 0,
            collapsed: false,
        }
    }

    #[test]
    fn dashboard_line_badge_uses_source_color_and_is_never_reversed() {
        let row = make_row("codex", RowState::Active(None), "cxagent");
        let line = dashboard_line(&row, true, SystemTime::UNIX_EPOCH, &NORMAL);
        let badge = &line.spans[0];
        assert_eq!(
            badge.style.fg,
            Some(to_color(NORMAL.source.codex)),
            "badge fg must be the codex source color"
        );
        assert!(
            !badge.style.add_modifier.contains(Modifier::REVERSED),
            "badge must NOT be reversed even when row is selected"
        );
    }

    #[test]
    fn dashboard_line_name_tinted_by_state() {
        let row = make_row("cc", RowState::Active(None), "agent");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_active)),
            "active: name must be tinted label_active"
        );

        let row = make_row("cc", RowState::Waiting(Arc::from("permission")), "agent");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_waiting)),
            "waiting: name must be tinted label_waiting"
        );

        let row = make_row("cc", RowState::Idle, "agent");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_idle)),
            "idle: name must be tinted label_idle"
        );
    }

    #[test]
    fn dashboard_line_selected_reverses_name_and_state_not_badge() {
        let row = make_row("cc", RowState::Active(None), "agent");
        let line = dashboard_line(&row, true, SystemTime::UNIX_EPOCH, &NORMAL);
        assert!(
            !line.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
            "badge must not be reversed"
        );
        assert!(
            line.spans[2]
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
            "name must be reversed when selected"
        );
        assert!(
            line.spans[4]
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
            "state must be reversed when selected"
        );
    }

    #[test]
    fn dashboard_line_unknown_source_falls_back_without_panic() {
        let row = make_row("not-a-source", RowState::Idle, "mystery");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        let badge = &line.spans[0];
        assert!(
            badge.content.contains("??"),
            "unknown source badge must contain '??', got: {}",
            badge.content
        );
        assert_eq!(
            badge.style.fg,
            Some(to_color(NORMAL.ui.label_idle)),
            "unknown source badge fg must fall back to label_idle"
        );
    }

    #[test]
    fn dashboard_line_selected_long_field_scrolls_unselected_truncates() {
        let long = "a-very-long-agent-name-that-far-exceeds-the-label-budget-here";
        let detail = "Edit: some/very/long/path/to/a/file/that/overflows.rs";
        let row = make_row("cc", RowState::Active(Some(Arc::from(detail))), long);
        let unsel = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        let name_unsel = unsel.spans[2].content.to_string();
        assert!(
            name_unsel.contains('\u{2026}'),
            "unselected long name must ellipsize: {name_unsel:?}"
        );
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(3000);
        let sel0 = dashboard_line(&row, true, t0, &NORMAL);
        let sel1 = dashboard_line(&row, true, t1, &NORMAL);
        let (n0, n1) = (
            sel0.spans[2].content.to_string(),
            sel1.spans[2].content.to_string(),
        );
        assert!(
            !n0.contains('\u{2026}'),
            "selected scrolling name must not ellipsize: {n0:?}"
        );
        assert_ne!(n0, n1, "selected name must animate across time");
        let (s0, s1) = (
            sel0.spans[4].content.to_string(),
            sel1.spans[4].content.to_string(),
        );
        assert_ne!(s0, s1, "selected state must animate across time");
    }

    #[test]
    fn every_registry_source_has_a_non_fallback_badge_color() {
        use pixtuoid_core::source::registry::REGISTRY;
        let theme = &pixtuoid_scene::theme::NORMAL;
        let fallback = to_color(theme.ui.label_idle);
        for d in REGISTRY {
            let row = make_row(d.name, RowState::Idle, "x");
            let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, theme);
            assert_ne!(
                line.spans[0].style.fg,
                Some(fallback),
                "source {:?} (prefix {:?}) renders the idle FALLBACK badge color — add its arm to the match in dashboard_line",
                d.name,
                d.label_prefix,
            );
        }
    }
}
