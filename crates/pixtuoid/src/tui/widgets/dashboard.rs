//! The agent-dashboard popup painter (ratatui). Pure presentation over the
//! pre-built row list from `tui::dashboard`; all model / fold / selection
//! logic lives there. Mirrors the other popups: a centered, cleared, BORDERLESS
//! panel (via `panel::borderless_panel`) painted over the scene in both the
//! normal and floor-transition draw paths.

use std::time::SystemTime;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::AgentId;

use super::{badge_color_for, centered_in, marquee_or_truncate, to_color};
use crate::tui::dashboard::{DashboardRow, RowState, DASHBOARD_VIEWPORT_ROWS};
use pixtuoid_scene::theme::Theme;

/// Char budget for the tree-prefix + label column (name only — source is in the badge now).
const LABEL_W: usize = 32;
/// Char budget for the activity/detail column.
const STATE_W: usize = 28;
/// Popup width (clamped to the terminal by `centered_in`).
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
        let area = centered_in(
            bounds,
            24 + 2 * super::PANEL_PAD_X,
            2 + 2 * super::PANEL_PAD_Y,
        );
        let inner = super::borderless_panel(f, area, Some("Agents"), theme);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No active agents",
                Style::default().fg(to_color(theme.ui.label_idle)),
            ))),
            inner,
        );
        return;
    }

    let desired = rows.len().min(DASHBOARD_VIEWPORT_ROWS);
    // Borderless: 1 title row + the visible content rows (no top/bottom border).
    let area = centered_in(
        bounds,
        POPUP_W + 2 * super::PANEL_PAD_X,
        desired as u16 + 1 + 2 * super::PANEL_PAD_Y,
    );
    // Hint in the title (borderless — it's the panel's first inner row).
    let title = format!(
        " Agents ({})  [\u{2191}\u{2193} \u{2190}\u{2192} z \u{23ce} esc] ",
        rows.len()
    );
    let inner = super::borderless_panel(f, area, Some(&title), theme);

    // `centered_in` clamps the popup to the terminal, so the real visible-row
    // count can drop below DASHBOARD_VIEWPORT_ROWS on a short terminal. Re-clamp
    // the scroll against the ACTUAL window (reusing the model's clamp_scroll, so
    // the math can't drift) — otherwise the selected row could sit in the
    // event-loop's wider window but below the painted one. `inner.height` already
    // excludes the title row.
    let visible = inner.height as usize;
    // When more rows exist than fit, reserve the bottom line for the overflow
    // cue and clamp the selection into the SMALLER content window — so the row
    // we hand to the cue is never the selected one (clamp_scroll parks a
    // down-navigated selection at the window's bottom edge). But probe first:
    // if that reduced clamp already reaches the end (the selection IS the last
    // row, nothing below), no cue is needed, so render the FULL height instead
    // — otherwise the reserved line would render blank.
    let overflow = rows.len() > visible;
    let reserved = if overflow {
        visible.saturating_sub(1)
    } else {
        visible
    };
    let probe_scroll = crate::tui::dashboard::clamp_scroll(rows, selected, scroll, reserved);
    let show_cue = overflow && rows.len() > probe_scroll + reserved;
    let content_window = if show_cue { reserved } else { visible };
    let scroll = crate::tui::dashboard::clamp_scroll(rows, selected, scroll, content_window);
    let mut lines: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(content_window)
        .map(|row| dashboard_line(row, selected == Some(row.agent_id), now, theme))
        .collect();
    if show_cue {
        let hidden_below = rows.len().saturating_sub(scroll + content_window);
        lines.push(Line::from(Span::styled(
            format!("  \u{22ee} {hidden_below} more \u{25be}"),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn dashboard_line(
    row: &DashboardRow,
    is_selected: bool,
    now: SystemTime,
    theme: &Theme,
) -> Line<'static> {
    // Tree prefix: a root with children gets a fold chevron; a childless root
    // gets blank space; a subagent is indented under its parent.
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
    let badge_text = format!("[{badge_tag:<2}]");
    let badge_color = badge_color_for(badge_tag, theme);

    Line::from(vec![
        Span::styled(badge_text, Style::default().fg(badge_color)),
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
            label: Arc::from(label),
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
        // spans[0] = badge
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
        // Active → label_active
        let row = make_row("cc", RowState::Active(None), "agent");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_active)),
            "active: name must be tinted label_active"
        );

        // Waiting → label_waiting
        let row = make_row("cc", RowState::Waiting(Arc::from("permission")), "agent");
        let line = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_waiting)),
            "waiting: name must be tinted label_waiting"
        );

        // Idle → label_idle
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
        // spans[0]=badge, [1]=space, [2]=name, [3]=floor, [4]=state
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
        // Unselected: static `…`-truncated name (spans[2]).
        let unsel = dashboard_line(&row, false, SystemTime::UNIX_EPOCH, &NORMAL);
        let name_unsel = unsel.spans[2].content.to_string();
        assert!(
            name_unsel.contains('\u{2026}'),
            "unselected long name must ellipsize: {name_unsel:?}"
        );
        // Selected: scrolling window — no ellipsis, and it animates across time.
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
        // The state cell (spans[4]) likewise scrolls when selected.
        let (s0, s1) = (
            sel0.spans[4].content.to_string(),
            sel1.spans[4].content.to_string(),
        );
        assert_ne!(s0, s1, "selected state must animate across time");
    }

    // Registry-bridge pin: every registered source must get a real badge color,
    // not the idle fallback. A new source added to REGISTRY without a matching
    // arm in dashboard_line would silently render in the idle color — this turns
    // that drift into a loud failure (same spirit as the registry's
    // every_descriptor_has_two_char_label_prefix pin).
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
