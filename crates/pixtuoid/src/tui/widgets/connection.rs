//! The Sources panel painter (ratatui). Pure presentation over the pre-built
//! row list + per-frame live facet from `tui::connection`; all model logic lives
//! there. Borderless (via `panel::borderless_panel`), painted over the scene in
//! both the normal and floor-transition draw paths.

use std::time::{Duration, SystemTime};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{borderless_panel, centered_in, marquee_or_truncate, marquee_window, to_color};
use crate::tui::connection::{no_action_hint, ConnState, ConnectionRow, LiveInfo};
use crate::tui::theme::Theme;

/// Popup width (clamped to the terminal by `centered_in`).
const CONNECTION_POPUP_W: u16 = 66;
/// Char budget for the display-name column (after the badge).
const NAME_W: usize = 13;
/// Char budget for the connection-state column.
const CONN_W: usize = 15;

/// The column header, kept as one fn so the "Live" position can't drift from the
/// data row. The two trailing spaces before "Live" mirror the fixed 2-col
/// `health_flag` slot each data row carries between the Connection and Live
/// columns (see `connection_line`) — without them "Live" sits 2 cols left of its
/// data. `live_header_aligns_with_the_live_data_column` pins the two together.
fn column_header() -> String {
    format!(
        "  {:<18}{:<width$}  Live",
        "CLI",
        "Connection",
        width = CONN_W
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::tui) fn paint_connection_panel(
    f: &mut ratatui::Frame<'_>,
    rows: &[ConnectionRow],
    live: &[LiveInfo],
    selected: usize,
    confirm: Option<usize>,
    last_result: Option<&str>,
    socket_line: &str,
    now: SystemTime,
    bounds: Rect,
    theme: &Theme,
) {
    // title (1) + socket (1) + blank (1) + header (1) + rows + blank (1)
    // + detail (1) + footer (1) — the title row is drawn by borderless_panel.
    let area = centered_in(
        bounds,
        CONNECTION_POPUP_W + 2 * super::PANEL_PAD_X,
        rows.len() as u16 + 7 + 2 * super::PANEL_PAD_Y,
    );
    if area.width < 4 || area.height < 3 {
        return;
    }
    let inner = borderless_panel(f, area, Some("Sources \u{2014} s/esc close"), theme);

    let dim = Style::default().fg(to_color(theme.ui.label_idle));
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len() + 6);

    lines.push(Line::from(Span::styled(format!("  {socket_line}"), dim)));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(column_header(), dim)));
    for (i, row) in rows.iter().enumerate() {
        let li = live.get(i).cloned().unwrap_or_default();
        lines.push(connection_line(row, &li, selected == i, now, theme));
    }
    lines.push(Line::from(""));

    // Detail line: armed-confirm prompt > last action result > selected row's
    // install location > a no-action hint. Char-safe truncated to the panel width.
    let detail = if let Some(ci) = confirm {
        let name = rows.get(ci).map_or("", |r| r.display_name);
        format!("\u{26a0} disconnect {name}? (y/n)")
    } else if let Some(res) = last_result {
        res.to_string()
    } else if let Some(row) = rows.get(selected) {
        // Health verdict first: a broken install / decode drift (#309, the
        // health-consolidation arc) is what you'd act on here, so it preempts the
        // benign per-state line. Cached on row build (connected rows only).
        if let Some(h) = &row.health {
            h.clone()
        } else {
            // State-aware: surface the install path ONLY when our integration is
            // actually there (Connected). Disconnected shows the action (the path
            // is just the future destination — meaningless until you connect);
            // no-CLI explains why it can't be bound.
            match row.state {
                ConnState::Connected => match &row.config_path {
                    Some(p) => format!("installed at: {}", p.display()),
                    None => "connected".to_string(),
                },
                ConnState::Disconnected => "disconnected \u{2014} press t to connect".to_string(),
                ConnState::NoCli => no_action_hint(row),
            }
        }
    } else {
        String::new()
    };
    // The detail line always shows the SELECTED row's content (path / hint), so
    // it scrolls (ping-pong) when it overflows — same focused-row treatment as
    // the selected list row's cells. Width budget reserves BOTH the 2-space left
    // indent below AND a symmetric 2-col right margin (so a full-width scroll
    // doesn't run flush to the panel edge — left/right padding stays balanced).
    let detail_w = inner.width.saturating_sub(4) as usize;
    lines.push(Line::from(Span::styled(
        format!("  {}", marquee_window(&detail, detail_w, now)),
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "  j/k move \u{00b7} t toggle \u{00b7} s/esc close",
        dim,
    )));

    f.render_widget(Paragraph::new(lines), inner);
}

/// One CLI row: a colored badge (never reversed), the name (tinted/reversed by
/// selection), the connection-state column, and the live-activity column.
fn connection_line(
    row: &ConnectionRow,
    live: &LiveInfo,
    is_selected: bool,
    now: SystemTime,
    theme: &Theme,
) -> Line<'static> {
    let prefix = if is_selected { "\u{25b8} " } else { "  " };

    // Badge: source color, NEVER reversed (a low-luminance hue inverted vanishes
    // against the highlight bg). Same mapping as the dashboard's badge.
    let badge_tag = row.label_prefix;
    let badge_color = to_color(match badge_tag {
        "cc" => theme.source.claude_code,
        "cx" => theme.source.codex,
        "rx" => theme.source.reasonix,
        "ag" => theme.source.antigravity,
        "cw" => theme.source.codewhale,
        "oc" => theme.source.opencode,
        "cp" => theme.source.copilot,
        "cu" => theme.source.cursor,
        _ => theme.ui.label_idle,
    });

    let base = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    let (c_glyph, c_text, c_color) = match row.state {
        ConnState::Connected => ('\u{25cf}', "connected", theme.ui.label_active),
        ConnState::Disconnected => ('\u{25cb}', "disconnected", theme.ui.label_idle),
        ConnState::NoCli => ('\u{2014}', "no CLI", theme.ui.label_idle),
    };
    // glyph + space (2) + text padded to CONN_W - 2.
    let conn_cell = format!("{c_glyph} {:<width$}", c_text, width = CONN_W - 2);

    let (l_glyph, l_text, l_color) = if live.dead {
        (
            '\u{26a0}',
            "transport died".to_string(),
            theme.ui.label_waiting,
        )
    } else if live.agents > 0 {
        let age = live.last_event_age.map(fmt_age).unwrap_or_default();
        let plural = if live.agents == 1 { "" } else { "s" };
        (
            '\u{25cf}',
            format!("{} agent{plural} \u{00b7} {age} ago", live.agents),
            theme.ui.label_active,
        )
    } else {
        ('\u{25cc}', "idle".to_string(), theme.ui.label_idle)
    };

    let name_cell = format!(
        "{:<NAME_W$}",
        marquee_or_truncate(row.display_name, NAME_W, is_selected, now)
    );

    // Health flag (#309 / consolidation): a fixed 2-col slot — `⚠` when this row
    // has a health summary (install broken / decode drift), else blank to keep
    // the Live column aligned. SEPARATE from the Connection column on purpose:
    // ConnState is the lifecycle, health is the sub-state it annotates. The full
    // reason is on the selected row's detail line below.
    let health_flag = if row.health.is_some() {
        "\u{26a0} "
    } else {
        "  "
    };

    Line::from(vec![
        Span::raw(prefix),
        Span::styled(
            format!("[{badge_tag:<2}]"),
            Style::default().fg(badge_color),
        ),
        Span::raw(" "),
        Span::styled(name_cell, base.fg(to_color(theme.ui.tooltip_text))),
        Span::styled(conn_cell, base.fg(to_color(c_color))),
        Span::styled(
            health_flag.to_string(),
            base.fg(to_color(theme.ui.label_waiting)),
        ),
        Span::styled(format!("{l_glyph} {l_text}"), base.fg(to_color(l_color))),
    ])
}

/// Compact age: seconds under a minute, then minutes, then hours.
fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::connection::{RowFacts, RowInput};
    use crate::tui::theme::NORMAL;

    fn row(source_id: &'static str, label_prefix: &'static str, state: ConnState) -> ConnectionRow {
        ConnectionRow {
            source_id,
            label_prefix,
            display_name: "Name",
            state,
            config_path: None,
            target: None,
            health: None,
        }
    }

    #[test]
    fn connection_line_badge_uses_source_color_and_is_never_reversed() {
        let r = row("codex", "cx", ConnState::Disconnected);
        let line = connection_line(
            &r,
            &LiveInfo::default(),
            true,
            SystemTime::UNIX_EPOCH,
            &NORMAL,
        );
        let badge = &line.spans[1];
        assert_eq!(badge.style.fg, Some(to_color(NORMAL.source.codex)));
        assert!(!badge.style.add_modifier.contains(Modifier::REVERSED));
        // name (spans[3]) IS reversed when selected.
        assert!(line.spans[3]
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
    }

    // The "Live" header column must line up with where each data row's live span
    // actually starts — the 2-col health_flag slot (#309) shifted the data right
    // and the header has to match. Char-count == column here (all glyphs single-
    // width BMP). Guards the regression both online lenses flagged on #315.
    #[test]
    fn live_header_aligns_with_the_live_data_column() {
        let header = column_header();
        let header_live_col = header.find("Live").expect("header has a Live column");

        // health=None → blank 2-col flag; health=Some → `⚠ ` 2-col flag. Both keep
        // the same width, so the live column is fixed regardless of health.
        for health in [None, Some("install broken".to_string())] {
            let r = ConnectionRow {
                source_id: "claude",
                label_prefix: "cc",
                display_name: "Name",
                state: ConnState::Connected,
                config_path: None,
                target: None,
                health,
            };
            let line = connection_line(
                &r,
                &LiveInfo::default(),
                false,
                SystemTime::UNIX_EPOCH,
                &NORMAL,
            );
            let n = line.spans.len();
            let live_col: usize = line.spans[..n - 1]
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            assert_eq!(
                header_live_col, live_col,
                "header Live col must match data live col; header={header:?}"
            );
        }
    }

    #[test]
    fn connection_line_renders_state_and_live_text() {
        let r = row("claude", "cc", ConnState::Connected);
        let live = LiveInfo {
            agents: 2,
            last_event_age: Some(Duration::from_secs(3)),
            dead: false,
        };
        let line = connection_line(&r, &live, false, SystemTime::UNIX_EPOCH, &NORMAL);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[cc]"));
        assert!(text.contains("connected"));
        assert!(text.contains("2 agents"));
        assert!(text.contains("3s ago"));
    }

    #[test]
    fn connection_line_dead_transport_overrides_live_column() {
        let r = row("codex", "cx", ConnState::Connected);
        let live = LiveInfo {
            agents: 1,
            last_event_age: Some(Duration::from_secs(1)),
            dead: true,
        };
        let line = connection_line(&r, &live, false, SystemTime::UNIX_EPOCH, &NORMAL);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("transport died"));
    }

    #[test]
    fn connection_line_singular_vs_plural_agents() {
        let r = row("claude", "cc", ConnState::Connected);
        let one = connection_line(
            &r,
            &LiveInfo {
                agents: 1,
                last_event_age: Some(Duration::from_secs(0)),
                dead: false,
            },
            false,
            SystemTime::UNIX_EPOCH,
            &NORMAL,
        );
        let t1: String = one.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(t1.contains("1 agent "), "singular: {t1}");
        assert!(!t1.contains("1 agents"));
    }

    #[test]
    fn connection_line_selected_long_name_scrolls_unselected_truncates() {
        let r = ConnectionRow {
            source_id: "x",
            label_prefix: "cc",
            display_name: "A-Very-Long-CLI-Display-Name-That-Overflows",
            state: ConnState::Connected,
            config_path: None,
            target: None,
            health: None,
        };
        // Unselected: static `…`-truncated name (spans[3]).
        let unsel = connection_line(
            &r,
            &LiveInfo::default(),
            false,
            SystemTime::UNIX_EPOCH,
            &NORMAL,
        );
        let name_unsel = unsel.spans[3].content.to_string();
        assert!(
            name_unsel.contains('\u{2026}'),
            "unselected long name must ellipsize: {name_unsel:?}"
        );
        // Selected: scrolling window — no ellipsis, animates across time.
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_millis(3000);
        let n0 = connection_line(
            &r,
            &LiveInfo::default(),
            true,
            SystemTime::UNIX_EPOCH,
            &NORMAL,
        )
        .spans[3]
            .content
            .to_string();
        let n1 = connection_line(&r, &LiveInfo::default(), true, t1, &NORMAL).spans[3]
            .content
            .to_string();
        assert!(
            !n0.contains('\u{2026}'),
            "selected scrolling name must not ellipsize: {n0:?}"
        );
        assert_ne!(n0, n1, "selected name must animate across time");
    }

    // Registry-bridge pin: every registered source gets a real badge color, not
    // the idle fallback — a new source added to REGISTRY without a matching arm
    // in `connection_line` would render in the idle color (mirrors the dashboard's
    // `every_registry_source_has_a_non_fallback_badge_color`).
    #[test]
    fn every_registry_source_has_a_non_fallback_badge_color() {
        use crate::tui::connection::build_rows_from;
        use pixtuoid_core::source::registry::REGISTRY;
        let fallback = to_color(NORMAL.ui.label_idle);
        // Build through the real builder so the prefixes come from the registry.
        let inputs: Vec<RowInput> = REGISTRY
            .iter()
            .map(|d| RowInput {
                source_id: d.name,
                label_prefix: d.label_prefix,
                target: None,
                health: None,
                facts: Some(RowFacts {
                    present: true,
                    config_path: None,
                }),
                connected: true,
            })
            .collect();
        for sr in build_rows_from(inputs) {
            let line = connection_line(
                &sr,
                &LiveInfo::default(),
                false,
                SystemTime::UNIX_EPOCH,
                &NORMAL,
            );
            assert_ne!(
                line.spans[1].style.fg,
                Some(fallback),
                "source {:?} (prefix {:?}) renders the idle FALLBACK badge — add its arm to connection_line",
                sr.source_id,
                sr.label_prefix,
            );
        }
    }
}
