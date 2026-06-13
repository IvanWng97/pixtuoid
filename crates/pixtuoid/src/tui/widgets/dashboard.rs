//! The agent-dashboard popup painter (ratatui). Pure presentation over the
//! pre-built row list from `tui::dashboard`; all model / fold / selection
//! logic lives there. Mirrors the theme-picker overlay: a centered, cleared,
//! bordered block painted over the scene in both the normal and floor-
//! transition draw paths.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::AgentId;

use super::{centered_in, to_color};
use crate::tui::dashboard::{DashboardRow, RowState, DASHBOARD_VIEWPORT_ROWS};
use crate::tui::theme::Theme;

/// Char budget for the tree-prefix + label column (name only — source is in the badge now).
const LABEL_W: usize = 32;
/// Char budget for the activity/detail column.
const STATE_W: usize = 28;
/// Popup width (clamped to the terminal by `centered_in`).
const POPUP_W: u16 = 76;

pub(in crate::tui) fn paint_dashboard(
    f: &mut ratatui::Frame<'_>,
    rows: &[DashboardRow],
    selected: Option<AgentId>,
    scroll: usize,
    bounds: Rect,
    theme: &Theme,
) {
    let brand = to_color(theme.ui.neon_brand);
    let bg = to_color(theme.ui.tooltip_bg);

    if rows.is_empty() {
        let area = centered_in(bounds, 24, 3);
        f.render_widget(Clear, area);
        let block = Block::default()
            .title(" Agents ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(brand))
            .style(Style::default().bg(bg));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No active agents",
                Style::default().fg(to_color(theme.ui.label_idle)),
            )))
            .block(block),
            area,
        );
        return;
    }

    let desired = rows.len().min(DASHBOARD_VIEWPORT_ROWS);
    let area = centered_in(bounds, POPUP_W, desired as u16 + 2);
    f.render_widget(Clear, area);

    // `centered_in` clamps the popup to the terminal, so the real visible-row
    // count can drop below DASHBOARD_VIEWPORT_ROWS on a short terminal. Re-clamp
    // the scroll against the ACTUAL window (reusing the model's clamp_scroll, so
    // the math can't drift) — otherwise the selected row could sit in the
    // event-loop's wider window but below the painted one.
    let visible = area.height.saturating_sub(2) as usize;
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
        .map(|row| dashboard_line(row, selected == Some(row.agent_id), theme))
        .collect();
    if show_cue {
        let hidden_below = rows.len().saturating_sub(scroll + content_window);
        lines.push(Line::from(Span::styled(
            format!("  \u{22ee} {hidden_below} more \u{25be}"),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }

    // Hint in the title (version-agnostic — no title_bottom API dependency).
    let title = format!(" Agents ({})  [↑↓ ←→ z ⏎ esc] ", rows.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(brand))
        .style(Style::default().bg(bg));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn dashboard_line(row: &DashboardRow, is_selected: bool, theme: &Theme) -> Line<'static> {
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
    let label_cell = format!("{:<LABEL_W$}", truncate(&name, LABEL_W));

    let (glyph, text, color) = match &row.state {
        RowState::Active(Some(detail)) => ('●', detail.to_string(), theme.ui.label_active),
        RowState::Active(None) => ('●', "active".to_string(), theme.ui.label_active),
        RowState::Waiting(reason) => ('◐', format!("waiting: {reason}"), theme.ui.label_waiting),
        RowState::Idle => ('○', "idle".to_string(), theme.ui.label_idle),
    };
    let state_cell = format!("{glyph} {}", truncate(&text, STATE_W));

    let base = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Badge uses the source color but is NEVER reversed — a low-luminance hue
    // inverted becomes invisible against the highlight background.
    let badge_tag = descriptor_for(row.source.as_ref()).map_or("??", |d| d.label_prefix);
    let badge_text = format!("[{badge_tag:<2}]");
    let badge_color = to_color(match badge_tag {
        "cc" => theme.source.claude_code,
        "cx" => theme.source.codex,
        "rx" => theme.source.reasonix,
        "ag" => theme.source.antigravity,
        "cw" => theme.source.codewhale,
        _ => theme.ui.label_idle,
    });

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

/// Truncate to `max` characters (char-safe), appending `…` when clipped.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::NORMAL;
    use pixtuoid_core::AgentId;
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
        let line = dashboard_line(&row, true, &NORMAL);
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
        let line = dashboard_line(&row, false, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_active)),
            "active: name must be tinted label_active"
        );

        // Waiting → label_waiting
        let row = make_row("cc", RowState::Waiting(Arc::from("permission")), "agent");
        let line = dashboard_line(&row, false, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_waiting)),
            "waiting: name must be tinted label_waiting"
        );

        // Idle → label_idle
        let row = make_row("cc", RowState::Idle, "agent");
        let line = dashboard_line(&row, false, &NORMAL);
        assert_eq!(
            line.spans[2].style.fg,
            Some(to_color(NORMAL.ui.label_idle)),
            "idle: name must be tinted label_idle"
        );
    }

    #[test]
    fn dashboard_line_selected_reverses_name_and_state_not_badge() {
        let row = make_row("cc", RowState::Active(None), "agent");
        let line = dashboard_line(&row, true, &NORMAL);
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
        let line = dashboard_line(&row, false, &NORMAL);
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

    // Registry-bridge pin: every registered source must get a real badge color,
    // not the idle fallback. A new source added to REGISTRY without a matching
    // arm in dashboard_line would silently render in the idle color — this turns
    // that drift into a loud failure (same spirit as the registry's
    // every_descriptor_has_two_char_label_prefix pin).
    #[test]
    fn every_registry_source_has_a_non_fallback_badge_color() {
        use pixtuoid_core::source::registry::REGISTRY;
        let theme = &crate::tui::theme::NORMAL;
        let fallback = to_color(theme.ui.label_idle);
        for d in REGISTRY {
            let row = make_row(d.name, RowState::Idle, "x");
            let line = dashboard_line(&row, false, theme);
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
