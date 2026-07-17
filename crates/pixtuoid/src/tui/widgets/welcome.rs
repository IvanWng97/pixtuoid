//! The first-run onboarding overlay painter (ratatui). Pure presentation over a
//! `tui::welcome::WelcomeRow` snapshot + an `elapsed_ms` clock that drives the
//! typewriter reveal and the staged "move-in" of roster rows. Borderless (via
//! `panel::borderless_panel`), painted TOPMOST in both draw paths.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{badge_color_for, borderless_panel, centered_in, to_color, PANEL_PAD_X, PANEL_PAD_Y};
use crate::tui::welcome::OnboardingFrame;
use pixtuoid_scene::theme::Theme;

const WELCOME_W: u16 = 54;
/// Typewriter speed for the subtitle reveal.
const TYPE_MS_PER_CHAR: u64 = 38;
/// After the subtitle finishes, roster rows fade in one every `ROW_STAGGER_MS`.
const ROW_LEAD_MS: u64 = 140;
const ROW_STAGGER_MS: u64 = 110;
const SUBTITLE: &str = "Let's move your agents in. Pick who walks in:";

/// `elapsed_ms` since the overlay opened (the event loop's clock). Returns the
/// number of subtitle chars revealed and whether typing is still in progress.
fn subtitle_done_ms() -> u64 {
    SUBTITLE.chars().count() as u64 * TYPE_MS_PER_CHAR
}

pub(crate) fn paint_welcome(
    f: &mut ratatui::Frame<'_>,
    frame: &OnboardingFrame,
    bounds: Rect,
    theme: &Theme,
) {
    let rows = &frame.rows;
    let selected = frame.selected;
    let elapsed_ms = frame.elapsed_ms;
    // title (1) + subtitle (1) + blank (1) + rows + blank (1) + hints
    // (2 on audio builds, 1 without the audio offer line).
    let hint_rows: u16 = if cfg!(feature = "audio") { 2 } else { 1 };
    let area = centered_in(
        bounds,
        WELCOME_W + 2 * PANEL_PAD_X,
        rows.len() as u16 + 4 + hint_rows + 2 * PANEL_PAD_Y,
    );
    if area.width < 4 || area.height < 3 {
        return;
    }
    let inner = borderless_panel(f, area, Some("Welcome to pixtuoid"), theme);
    let bg = Style::default().bg(to_color(theme.ui.tooltip_bg));
    let dim = Style::default().fg(to_color(theme.ui.label_idle));
    let bright = Style::default().fg(to_color(theme.ui.neon_brand));

    let mut lines: Vec<Line> = Vec::with_capacity(rows.len() + 4);

    // Typewriter subtitle: reveal N chars by elapsed, with a blinking caret while
    // still typing.
    let total = SUBTITLE.chars().count();
    let typed = ((elapsed_ms / TYPE_MS_PER_CHAR) as usize).min(total);
    let shown: String = SUBTITLE.chars().take(typed).collect();
    let caret = if typed < total && (elapsed_ms / 450).is_multiple_of(2) {
        "\u{2588}"
    } else {
        ""
    };
    lines.push(Line::from(Span::styled(format!("  {shown}{caret}"), dim)));
    lines.push(Line::from(""));

    // Roster: each detected CLI fades in AFTER the subtitle finishes typing, one
    // every ROW_STAGGER_MS — the staged "move-in" feel. A row not yet due breaks
    // the loop (later rows are strictly later).
    let base = subtitle_done_ms() + ROW_LEAD_MS;
    for (i, row) in rows.iter().enumerate() {
        if elapsed_ms < base + i as u64 * ROW_STAGGER_MS {
            break;
        }
        let is_sel = i == selected;
        let badge = format!("[{:<2}]", row.label_prefix);
        // `[x]`/`[ ]` (not a check glyph) so the checkbox is legible in any font.
        let check = if row.checked { "[x]" } else { "[ ]" };
        let mut name_style = if row.checked {
            Style::default().fg(to_color(theme.ui.label_active))
        } else {
            dim
        };
        if is_sel {
            name_style = name_style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::raw(if is_sel { "  \u{25b8} " } else { "    " }),
            Span::styled(
                badge,
                Style::default().fg(badge_color_for(row.label_prefix, theme)),
            ),
            Span::raw(" "),
            Span::styled(check.to_string(), if row.checked { bright } else { dim }),
            Span::raw(" "),
            Span::styled(row.display_name.clone(), name_style),
        ]));
    }

    // The key hints appear once every row is in — incl. the one-line audio
    // offer (#633: the office starts muted; m is the whole opt-in).
    let all_in = base + rows.len().saturating_sub(1) as u64 * ROW_STAGGER_MS;
    if elapsed_ms >= all_in {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  \u{2191}\u{2193} move \u{00b7} space toggle \u{00b7} enter connect \u{00b7} esc skip",
            dim,
        )));
        if cfg!(feature = "audio") {
            lines.push(Line::from(Span::styled(
                // ≤ WELCOME_W(54) cols incl. the 2-col indent — a longer
                // line clips mid-word (review MEDIUM); no line on no-audio
                // builds (the Linux prebuilts — a dead m key reads broken)
                "  \u{2669} office sound \u{2014} press m anytime",
                dim,
            )));
        }
    }

    f.render_widget(Paragraph::new(lines).style(bg), inner);
}
