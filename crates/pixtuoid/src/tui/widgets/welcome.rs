//! The first-run onboarding overlay painter (ratatui). Pure presentation over a
//! `tui::welcome::WelcomeRow` snapshot + an `elapsed_ms` clock that drives the
//! typewriter reveal and the staged "move-in" of roster rows. Borderless (via
//! `panel::borderless_panel`), painted TOPMOST in both draw paths.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{badge_color_for, paint_panel, to_color, Overflow};
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
    let dim = Style::default().fg(to_color(theme.ui.label_idle));
    let bright = Style::default().fg(to_color(theme.ui.neon_brand));

    // Above (fixed chrome): the typewriter subtitle + a blank. Reveal N chars by
    // elapsed, with a blinking caret while still typing.
    let total = SUBTITLE.chars().count();
    let typed = ((elapsed_ms / TYPE_MS_PER_CHAR) as usize).min(total);
    let shown: String = SUBTITLE.chars().take(typed).collect();
    let caret = if typed < total && (elapsed_ms / 450).is_multiple_of(2) {
        "\u{2588}"
    } else {
        ""
    };
    let above = vec![
        Line::from(Span::styled(format!("  {shown}{caret}"), dim)),
        Line::from(""),
    ];

    // List: the roster. Each CLI fades in after the subtitle finishes, one every
    // ROW_STAGGER_MS. A not-yet-due row reserves a BLANK line (fixed height, so the
    // staged reveal never resizes the panel); paint_panel then windows-with-cue if
    // the roster overflows a short terminal.
    let base = subtitle_done_ms() + ROW_LEAD_MS;
    let list: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            if elapsed_ms < base + i as u64 * ROW_STAGGER_MS {
                return Line::from("");
            }
            let is_sel = i == selected;
            let badge = format!("[{:<2}]", row.label_prefix);
            // `[x]`/`[ ]` (not a check glyph) so the checkbox reads in any font.
            let check = if row.checked { "[x]" } else { "[ ]" };
            let mut name_style = if row.checked {
                Style::default().fg(to_color(theme.ui.label_active))
            } else {
                dim
            };
            if is_sel {
                name_style = name_style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
            }
            Line::from(vec![
                Span::raw(if is_sel { "  \u{25b8} " } else { "    " }),
                Span::styled(
                    badge,
                    Style::default().fg(badge_color_for(row.label_prefix, theme)),
                ),
                Span::raw(" "),
                Span::styled(check.to_string(), if row.checked { bright } else { dim }),
                Span::raw(" "),
                Span::styled(row.display_name.clone(), name_style),
            ])
        })
        .collect();

    // Below (fixed chrome): a blank + the key hints, revealed once every row is in.
    // The hint rows stay RESERVED (blank) before `all_in` so the panel height is
    // constant across the whole reveal (incl. the one-line audio offer, #633).
    let all_in = base + rows.len().saturating_sub(1) as u64 * ROW_STAGGER_MS;
    let shown_hints = elapsed_ms >= all_in;
    let mut below = vec![Line::from("")];
    below.push(if shown_hints {
        Line::from(Span::styled(
            "  \u{2191}\u{2193} move \u{00b7} space toggle \u{00b7} enter connect \u{00b7} esc skip",
            dim,
        ))
    } else {
        Line::from("")
    });
    if cfg!(feature = "audio") {
        below.push(if shown_hints {
            Line::from(Span::styled(
                // ≤ WELCOME_W(54) cols incl. the 2-col indent — a longer line
                // clips mid-word; no line on no-audio builds (a dead m key reads
                // broken on the Linux prebuilts).
                "  \u{2669} office sound \u{2014} press m anytime",
                dim,
            ))
        } else {
            Line::from("")
        });
    }

    paint_panel(
        f,
        theme,
        Some("Welcome to pixtuoid"),
        bounds,
        WELCOME_W,
        1.0,
        above,
        list,
        below,
        Overflow::Follow {
            selected: Some(selected),
            scroll: 0,
            cap: None,
        },
    );
}
