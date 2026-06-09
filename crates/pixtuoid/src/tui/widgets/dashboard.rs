//! The agent-dashboard popup painter (ratatui). Pure presentation over the
//! pre-built row list from `tui::dashboard`; all model / fold / selection
//! logic lives there. Mirrors the theme-picker overlay: a centered, cleared,
//! bordered block painted over the scene in both the normal and floor-
//! transition draw paths.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use pixtuoid_core::AgentId;

use super::{centered_in, to_color};
use crate::tui::dashboard::{DashboardRow, RowState, DASHBOARD_VIEWPORT_ROWS};
use crate::tui::theme::Theme;

/// Char budget for the tree-prefix + label column.
const LABEL_W: usize = 22;
/// Char budget for the activity/detail column.
const STATE_W: usize = 22;
/// Popup width (clamped to the terminal by `centered_in`).
const POPUP_W: u16 = 56;

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
    let scroll = crate::tui::dashboard::clamp_scroll(rows, selected, scroll, visible);

    let lines: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(visible)
        .map(|row| dashboard_line(row, selected == Some(row.agent_id), theme))
        .collect();

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
    Line::from(vec![
        Span::styled(label_cell, base.fg(to_color(theme.ui.label_idle))),
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
