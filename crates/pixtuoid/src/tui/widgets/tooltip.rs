use std::time::SystemTime;

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::{StateKind, compact_hms, display_width, source_badge_span, state_color, to_color};
use crate::tui::renderer::clip_widget_rect;
use pixtuoid_scene::layout::DESK_W;
use pixtuoid_scene::overlay::{LabelElement, disambig_suffix};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::{AgentFrame, MascotFrame};

/// Borderless tooltip frame shared by every hover/click tooltip: just the padded
/// text. The caller must paint `super::paint_card_backing` UNDER it (the `Clear` +
/// `tooltip_bg` fill + drop shadow); the 1-cell uniform padding is what the
/// callers' `+2` size math accounts for.
pub(super) fn framed_tooltip<'a>(lines: Vec<Line<'a>>) -> Paragraph<'a> {
    Paragraph::new(lines).block(Block::default().padding(Padding::uniform(1)))
}

/// Where a cursor tooltip anchors: the hovered cell, and the scene rect it must
/// stay inside.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TooltipAt {
    pub(crate) mx: u16,
    pub(crate) my: u16,
    pub(crate) scene_rect: Rect,
}

/// Horizontal anchor for a tooltip of width `tip_w`: just right of the cursor,
/// flipped to the left if that would overflow the scene's right edge.
fn flip_x_anchor(mx: u16, tip_w: u16, scene_rect: Rect) -> u16 {
    let tx = mx.saturating_add(2);
    if tx.saturating_add(tip_w) > scene_rect.x + scene_rect.width {
        mx.saturating_sub(tip_w + 1)
    } else {
        tx
    }
}

pub(crate) fn paint_label_widgets(
    f: &mut ratatui::Frame<'_>,
    labels: &[LabelElement],
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    for el in labels {
        let ly = scene_rect.y + (el.anchor_px.y / 2).saturating_sub(1);
        let spans = if el.hovered {
            let style = Style::default()
                .fg(Color::White)
                .add_modifier(ratatui::style::Modifier::BOLD);
            vec![Span::styled(format!("▸{}", el.text), style)]
        } else {
            let ink = pixtuoid_scene::overlay::badge_ink(&el.text, el.tone, theme);
            vec![
                Span::styled(
                    pixtuoid_scene::overlay::BADGE_MARKER.to_string(),
                    Style::default().fg(to_color(ink.marker)),
                ),
                Span::styled(el.text.clone(), Style::default().fg(to_color(ink.name))),
            ]
        };
        let line = ratatui::text::Line::from(spans);
        let half_w = u16::try_from(line.width() / 2).unwrap_or(u16::MAX);
        let lx = scene_rect.x + el.anchor_px.x.saturating_sub(half_w);
        let para = Paragraph::new(line);
        if let Some(r) = clip_widget_rect(
            Rect {
                x: lx,
                y: ly,
                width: DESK_W + 4,
                height: 1,
            },
            scene_rect,
        ) {
            f.render_widget(para, r);
        }
    }
}

/// The dossier's detail-column budget — the ONE quantity BOTH detail sources are
/// clipped to (Active's tool args and Waiting's reason feed the same `detail_line`
/// slot), so widening the card can't leave one row ragged against the other.
const DETAIL_CHARS: usize = 34;

/// A short form of a cwd path: the TAIL (most informative — project dir) with a
/// leading `…`. Char-sliced, never a byte slice, so a multibyte path can't panic.
fn short_cwd(cwd: &std::path::Path) -> String {
    const MAX: usize = 30;
    let s = cwd.to_string_lossy();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX {
        s.into_owned()
    } else {
        format!(
            "\u{2026}{}",
            chars[chars.len() - (MAX - 1)..].iter().collect::<String>()
        )
    }
}

/// Floating "dossier" panel painted near the cursor when an agent is hovered or
/// pinned. Uses the SHARED vocabulary (`StateKind`) + badge (`source_badge_span`)
/// so it can't drift from the footer/board/dashboard; dim rows use `tooltip_dim`,
/// NOT the live `label_exiting`.
pub(crate) fn paint_hover_tooltip(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    agent_id: AgentId,
    at: TooltipAt,
    now: SystemTime,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let TooltipAt { mx, my, scene_rect } = at;
    let Some(agent) = scene.agents.get(&agent_id) else {
        return;
    };

    let kind = if agent.exiting_at.is_some() {
        StateKind::Exiting
    } else {
        match agent.state {
            ActivityState::Active { .. } => StateKind::Active,
            ActivityState::Waiting { .. } => StateKind::Waiting,
            ActivityState::Idle => StateKind::Idle,
        }
    };

    let dim = Style::default().fg(to_color(theme.ui.tooltip_dim));
    let text_style = Style::default().fg(to_color(theme.ui.tooltip_text));

    // The tool's glow hue comes from the TYPED kind, never a re-parse of the
    // displayed name.
    let mut state_spans = vec![Span::styled(
        format!("{} {}", kind.glyph(), kind.word()),
        Style::default().fg(state_color(kind, theme)),
    )];
    // An EXITING agent must show none of the live tool/reason affordances — the
    // walking-out slot retains its Active/Waiting payload (`mark_exiting` doesn't
    // reset `state`), so gate on the exiting-first `kind`, not the raw `agent.state`.
    let mut detail_line: Option<String> = None;
    if !matches!(kind, StateKind::Exiting) {
        if let ActivityState::Active {
            detail, kind: tk, ..
        } = &agent.state
        {
            if let Some(d) = detail.as_deref().filter(|d| !d.is_empty()) {
                let (tool, rest) = d
                    .split_once(char::is_whitespace)
                    .map(|(t, r)| (t.trim_end_matches(':'), r.trim()))
                    .unwrap_or((d.trim_end_matches(':'), ""));
                if !tool.is_empty() {
                    state_spans.push(Span::raw(" \u{b7} "));
                    state_spans.push(Span::styled(
                        tool.to_string(),
                        Style::default().fg(to_color(theme.tool_glow.for_kind(*tk))),
                    ));
                }
                if !rest.is_empty() {
                    detail_line = Some(rest.chars().take(DETAIL_CHARS).collect());
                }
            }
        } else if let ActivityState::Waiting { reason } = &agent.state {
            let r: String = reason.chars().take(DETAIL_CHARS).collect();
            detail_line = Some(format!("?{r}"));
        }
    }

    // Built before L1 so the `·id4` right-flush + the separator can size to the
    // widest body line.
    let mut body: Vec<Line> = Vec::new();
    body.push(Line::from(state_spans));
    if let Some(d) = detail_line {
        body.push(Line::from(Span::styled(format!("  {d}"), text_style)));
    }
    if let Some(parent) = agent.parent_id.and_then(|p| scene.agents.get(&p)) {
        body.push(Line::from(Span::styled(
            format!("\u{21b3} under {}", parent.label),
            dim,
        )));
    }
    body.push(Line::from(Span::styled(
        format!("\u{25a4} {}", short_cwd(&agent.cwd)),
        dim,
    )));
    // The effort is suffixed only while FRESH — the same burn-TTL the flame reads,
    // so the text can't outlive the fire.
    if let Some(model) = agent.model.as_deref() {
        let mut row = format!("\u{2605} {model}");
        if let Some(effort) = pixtuoid_scene::burn::fresh_effort(agent, now) {
            row.push_str(&format!(" \u{b7} {effort}"));
        }
        body.push(Line::from(Span::styled(row, dim)));
    }

    let session_secs = now
        .duration_since(agent.created_at)
        .unwrap_or_default()
        .as_secs();
    let mut stats = format!(
        "\u{25f7} {} \u{b7} {} calls",
        compact_hms(session_secs),
        agent.tool_call_count
    );
    // Skipped at zero so sources with no usage wire keep their dossier unchanged.
    if agent.tokens_used > 0 {
        stats.push_str(&format!(
            " \u{b7} \u{3a3} {} tok",
            pixtuoid_scene::token_meter::compact_tokens(agent.tokens_used)
        ));
    }
    // Fresh agents show no active-% meter — the % is noise before ~5s of accounting.
    if matches!(kind, StateKind::Active) && session_secs >= 5 {
        let pct = (agent.active_ms / 1000)
            .checked_mul(100)
            .and_then(|n| n.checked_div(session_secs))
            .map(|p| p.min(100))
            .unwrap_or(0);
        let filled = (pct as usize * 5).div_ceil(100).min(5);
        let meter: String = "\u{25ae}".repeat(filled) + &"\u{25af}".repeat(5 - filled);
        stats.push_str(&format!(" \u{b7} {meter} {pct}%"));
    }
    body.push(Line::from(Span::styled(stats, dim)));

    let badge_tag = descriptor_for(agent.source.as_ref()).map_or("??", |d| d.label_prefix);
    let l1_head_w = 4 + 1 + display_width(&agent.label); // "[xx]" + space + label
    let id4 = format!("\u{b7}{}", disambig_suffix(&agent.session_id));
    let body_w = body.iter().map(|l| l.width()).max().unwrap_or(0);
    let content_w = body_w.max(l1_head_w + 2 + display_width(&id4));
    let pad = content_w.saturating_sub(l1_head_w + display_width(&id4));
    let l1 = Line::from(vec![
        source_badge_span(badge_tag, theme),
        Span::styled(
            format!(" {}", agent.label),
            Style::default()
                .fg(to_color(theme.ui.tooltip_title))
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(id4, dim),
    ]);
    let separator = Line::from(Span::styled("\u{2500}".repeat(content_w), dim));

    let mut lines: Vec<Line> = Vec::with_capacity(body.len() + 2);
    lines.push(l1);
    lines.push(separator);
    lines.extend(body);

    let content_h = lines.len() as u16;
    let content_w = lines.iter().map(|l| l.width() as u16).max().unwrap_or(20);
    // +2 cols / +2 rows for the frame's 1-cell padding on all sides.
    let tip_w = (content_w + 2).min(scene_rect.width).max(20);
    let tip_h = (content_h + 2).min(scene_rect.height);

    let tx = flip_x_anchor(mx, tip_w, scene_rect);
    let mut ty = my.saturating_add(1);
    if ty.saturating_add(tip_h) > scene_rect.y + scene_rect.height {
        ty = my.saturating_sub(tip_h).max(scene_rect.y);
    }
    let rect = Rect {
        x: tx,
        y: ty,
        width: tip_w,
        height: tip_h,
    };
    let Some(clipped) = clip_widget_rect(rect, scene_rect) else {
        return;
    };

    super::paint_card_backing(f, clipped, theme);
    f.render_widget(framed_tooltip(lines), clipped);
}

fn paint_simple_tooltip(
    f: &mut ratatui::Frame<'_>,
    text: &str,
    at: TooltipAt,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let TooltipAt { mx, my, scene_rect } = at;
    let line = Line::from(Span::styled(
        text,
        Style::default()
            .fg(to_color(theme.ui.tooltip_title))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    // Size by DISPLAY width, not char count: wide glyphs (e.g. the coffee ☕, 2
    // cells) would otherwise undersize the box by a column and clip the content.
    let tip_w = (line.width() as u16 + 2).min(scene_rect.width);
    let tip_h = 3u16.min(scene_rect.height);
    let tx = flip_x_anchor(mx, tip_w, scene_rect);
    // Float above the cursor, flipping below when there is no room. Guard on
    // geometry (cursor within tip_h of the top) rather than the post-saturation
    // `ty`, which can't detect overflow when scene_rect.y == 0 (saturating_sub
    // floors at 0, never < 0).
    let mut ty = my.saturating_sub(tip_h);
    if my < scene_rect.y + tip_h {
        ty = my.saturating_add(1);
    }
    if let Some(r) = clip_widget_rect(
        Rect {
            x: tx,
            y: ty,
            width: tip_w,
            height: tip_h,
        },
        scene_rect,
    ) {
        super::paint_card_backing(f, r, theme);
        f.render_widget(framed_tooltip(vec![line]), r);
    }
}

pub(crate) fn paint_coffee_tooltip(
    f: &mut ratatui::Frame<'_>,
    at: TooltipAt,
    theme: &pixtuoid_scene::theme::Theme,
) {
    paint_simple_tooltip(f, " \u{2615} Buy Ivan a coffee ", at, theme);
}

pub(crate) fn paint_furniture_tooltip(
    f: &mut ratatui::Frame<'_>,
    label: &str,
    at: TooltipAt,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let text = format!(" {} ", label);
    paint_simple_tooltip(f, &text, at, theme);
}

pub(crate) fn paint_pet_tooltip(
    f: &mut ratatui::Frame<'_>,
    kind: PetKind,
    anim_name: &str,
    is_on_cooldown: bool,
    display_name: &str,
    at: TooltipAt,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let idle = format!(" {display_name} ");
    let text: &str = if is_on_cooldown {
        match kind {
            PetKind::Cat => " purr... ",
            PetKind::Dog => " woof! ",
        }
    } else if anim_name == kind.sleep_anim() {
        " Shhh... sleeping "
    } else if anim_name == kind.sit_anim() {
        " Pet me! "
    } else {
        &idle
    };
    paint_simple_tooltip(f, text, at, theme);
}

pub(crate) fn paint_mascot_tooltip(
    f: &mut ratatui::Frame<'_>,
    mascot: &MascotFrame,
    at: TooltipAt,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let text = mascot_tooltip_text(mascot);
    paint_simple_tooltip(f, &text, at, theme);
}

/// The mascot tooltip's text. The verb keys on `busy` — see
/// [`pixtuoid_scene::pixel_painter::MascotFrame::busy`] for why the run state,
/// not the session count — and `degraded` outranks busy/idle.
fn mascot_tooltip_text(mascot: &MascotFrame) -> String {
    let &MascotFrame {
        name,
        ref instance,
        busy,
        degraded,
        active_sessions,
        ..
    } = mascot;
    // `OpenClaw:19789` — the painter sets `instance` only when there IS a sibling to
    // tell apart, so the single-gateway tooltip stays byte-unchanged.
    let name = match instance {
        Some(i) => format!("{name}:{i}"),
        None => name.to_string(),
    };
    let verb = if degraded {
        "model error"
    } else if busy {
        "working"
    } else {
        "idle"
    };
    if active_sessions > 1 {
        format!(" {name} gateway · {verb} · {active_sessions} sessions ")
    } else {
        format!(" {name} gateway · {verb} ")
    }
}

pub fn paint_chitchat_bubbles(
    f: &mut ratatui::Frame<'_>,
    bubbles: &[pixtuoid_scene::chitchat::ChitchatBubble],
    agents: &[AgentFrame],
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    for bubble in bubbles {
        // The speaker's badge anchor, so bubble and badge share one centre.
        let Some(at) = agents
            .iter()
            .find(|a| a.agent_id == bubble.speaker)
            .map(|a| a.label_anchor)
        else {
            continue;
        };
        let text = format!(" {} ", bubble.text);
        // Size by DISPLAY width, not byte length: a wide-glyph quip would otherwise
        // over-size and mis-center the bubble.
        let line = Line::from(text.clone());
        let tip_w = line.width() as u16;
        let tip_h = 1u16;

        let cell_x = scene_rect.x + at.x;
        let cell_y = scene_rect.y + at.y / 2;

        let bx = cell_x.saturating_sub(tip_w / 2);
        let by = cell_y.saturating_sub(3);

        if let Some(r) = clip_widget_rect(
            Rect {
                x: bx,
                y: by,
                width: tip_w,
                height: tip_h,
            },
            scene_rect,
        ) {
            let style = Style::default()
                .bg(to_color(theme.ui.tooltip_bg))
                .fg(Color::White);
            f.render_widget(Paragraph::new(Span::styled(text, style)), r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MascotFrame, TooltipAt, mascot_tooltip_text};
    use pixtuoid_scene::theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    /// Join the whole buffer into one newline-free string, so a `.contains` probe
    /// finds text regardless of which cell the box landed in.
    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        out
    }

    fn row_of(term: &Terminal<TestBackend>, needle: &str) -> Option<u16> {
        let buf = term.backend().buffer();
        let area = buf.area;
        for y in 0..area.height {
            let row: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
            if row.contains(needle) {
                return Some(y);
            }
        }
        None
    }

    /// A lone `OpenClaw` mascot; only the tooltip-bearing fields vary.
    fn mascot(
        instance: Option<&str>,
        busy: bool,
        degraded: bool,
        active_sessions: u32,
    ) -> MascotFrame {
        MascotFrame {
            pos: pixtuoid_scene::layout::Point { x: 0, y: 0 },
            w: 0,
            h: 0,
            name: "OpenClaw",
            instance: instance.map(str::to_string),
            busy,
            degraded,
            active_sessions,
        }
    }

    /// A badge's text centres on its anchor, the sprite's top-centre.
    #[test]
    fn a_badge_centres_its_text_on_the_anchor() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let scene_rect = Rect {
            x: 3,
            y: 1,
            width: 36,
            height: 8,
        };
        let anchor = Point { x: 20, y: 8 };
        let text = "abcdefgh";
        term.draw(|f| {
            super::paint_label_widgets(
                f,
                &[LabelElement {
                    anchor_px: anchor,
                    text: text.into(),
                    tone: LabelTone::Idle,
                    hovered: false,
                }],
                scene_rect,
                &theme::NORMAL,
            )
        })
        .unwrap();
        let row = row_of(&term, text).expect("the badge painted");
        let buf = term.backend().buffer();
        let left = (0..buf.area.width)
            .find(|&x| buf[(x, row)].symbol() != " ")
            .expect("a painted cell");
        // The ● marker plus the name.
        let width = 1 + text.chars().count() as u16;
        assert_eq!(left, scene_rect.x + anchor.x - width / 2);
    }

    /// The classic badge paints the shared model's ink, which
    /// `every_badge_ink_reads_on_its_plate_in_every_theme` holds at WCAG AA:
    /// the ● in the source's hue, the name in the tone, in every theme.
    #[test]
    fn a_badge_paints_the_models_ink() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone, badge_ink};
        let text = "cc\u{b7}repo";
        for theme in pixtuoid_scene::theme::ALL_THEMES {
            for tone in [
                LabelTone::Active,
                LabelTone::Waiting,
                LabelTone::Idle,
                LabelTone::Exiting,
            ] {
                let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
                term.draw(|f| {
                    super::paint_label_widgets(
                        f,
                        &[LabelElement {
                            anchor_px: Point { x: 20, y: 8 },
                            text: text.into(),
                            tone,
                            hovered: false,
                        }],
                        f.area(),
                        theme,
                    )
                })
                .unwrap();
                let row = row_of(&term, "repo").expect("the badge painted");
                let buf = term.backend().buffer();
                let fg = |s: &str| {
                    (0..buf.area.width)
                        .find(|&x| buf[(x, row)].symbol() == s)
                        .and_then(|x| buf[(x, row)].style().fg)
                };
                let ink = badge_ink(text, tone, theme);
                let at = format!("{} {tone:?}", theme.name);
                let marker = pixtuoid_scene::overlay::BADGE_MARKER.to_string();
                assert_eq!(fg(&marker), Some(super::to_color(ink.marker)), "{at}");
                assert_eq!(fg("r"), Some(super::to_color(ink.name)), "{at}");
            }
        }
    }

    #[test]
    fn a_chitchat_bubble_centres_over_its_speakers_badge() {
        use pixtuoid_scene::chitchat::ChitchatBubble;
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        use pixtuoid_scene::pixel_painter::AgentFrame;
        let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let scene_rect = Rect {
            x: 3,
            y: 1,
            width: 36,
            height: 10,
        };
        let speaker = AgentFrame {
            agent_id: pixtuoid_core::AgentId::from_transcript_path("/chat/0.jsonl"),
            top_left: Point { x: 16, y: 14 },
            w: 8,
            h: 12,
            label_anchor: Point { x: 20, y: 14 },
        };
        let (name, quip) = ("abcdefgh", "LGTM!");
        term.draw(|f| {
            super::paint_label_widgets(
                f,
                &[LabelElement {
                    anchor_px: speaker.label_anchor,
                    text: name.into(),
                    tone: LabelTone::Idle,
                    hovered: false,
                }],
                scene_rect,
                &theme::NORMAL,
            );
            super::paint_chitchat_bubbles(
                f,
                &[ChitchatBubble {
                    text: quip,
                    speaker: speaker.agent_id,
                }],
                &[speaker],
                scene_rect,
                &theme::NORMAL,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let centre = |needle: &str| {
            let row = row_of(&term, needle).expect("painted");
            let cells: Vec<u16> = (0..buf.area.width)
                .filter(|&x| {
                    buf[(x, row)].symbol() != " "
                        || buf[(x, row)].bg != ratatui::style::Color::Reset
                })
                .collect();
            let (l, r) = (cells[0], cells[cells.len() - 1]);
            (l + r) / 2
        };
        assert_eq!(centre(quip), centre(name));
    }

    #[test]
    fn mascot_tooltip_paints_gateway_verb_into_buffer() {
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| {
            super::paint_mascot_tooltip(
                f,
                &mascot(None, true, false, 1),
                TooltipAt {
                    mx: 10,
                    my: 3,
                    scene_rect: f.area(),
                },
                &theme::NORMAL,
            )
        })
        .unwrap();
        let busy = buffer_text(&term);
        assert!(
            busy.contains("OpenClaw gateway"),
            "busy paint should render the gateway name+verb, got: {busy:?}"
        );
        assert!(
            busy.contains("working"),
            "busy=true should render the 'working' verb, got: {busy:?}"
        );

        let mut term2 = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term2
            .draw(|f| {
                super::paint_mascot_tooltip(
                    f,
                    &mascot(None, true, true, 1),
                    TooltipAt {
                        mx: 10,
                        my: 3,
                        scene_rect: f.area(),
                    },
                    &theme::NORMAL,
                )
            })
            .unwrap();
        let degraded = buffer_text(&term2);
        assert!(
            degraded.contains("model error"),
            "degraded should render 'model error', got: {degraded:?}"
        );
        assert!(
            !degraded.contains("working"),
            "degraded must override busy → no 'working' verb, got: {degraded:?}"
        );
    }

    #[test]
    fn pet_tooltip_shows_pet_me_on_sit_anim() {
        use pixtuoid_scene::pet::PetKind;
        let kind = PetKind::Dog;
        let sit = kind.sit_anim();
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| {
            super::paint_pet_tooltip(
                f,
                kind,
                sit,
                false,
                "Rex",
                TooltipAt {
                    mx: 10,
                    my: 3,
                    scene_rect: f.area(),
                },
                &theme::NORMAL,
            )
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("Pet me!"),
            "sit anim + not-on-cooldown should render 'Pet me!', got: {text:?}"
        );
        assert!(
            !text.contains("woof"),
            "sit arm must not fall through to the cooldown woof, got: {text:?}"
        );
        assert!(
            !text.contains("sleeping"),
            "sit arm must not be the sleep arm, got: {text:?}"
        );
    }

    #[test]
    fn hover_tooltip_idle_shows_no_meter_and_casts_a_drop_shadow() {
        use std::path::Path;
        use std::sync::Arc;
        use std::time::{Duration, SystemTime};

        use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex};
        use pixtuoid_core::{AgentId, SceneState};

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
        let id = AgentId::from_transcript_path("/fresh/0.jsonl");
        let slot = AgentSlot {
            agent_id: id,
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(Path::new("/repo")),
            label: "fresh".into(),
            state: ActivityState::Idle,
            state_started_at: now,
            // 2s < the 5s freshness floor → `--%`.
            created_at: now - Duration::from_secs(2),
            last_event_at: now,
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
        let mut scene = SceneState::uniform(12);
        scene.agents.insert(id, slot);

        let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
        let bright = ratatui::style::Color::Rgb(200, 200, 200);
        term.draw(|f| {
            // Stand in for the already-flushed office so the drop shadow has real
            // cells to dim.
            let full = f.area();
            for y in 0..full.height {
                for x in 0..full.width {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    cell.set_symbol("\u{2580}");
                    cell.fg = bright;
                    cell.bg = bright;
                }
            }
            super::paint_hover_tooltip(
                f,
                &scene,
                id,
                TooltipAt {
                    mx: 20,
                    my: 10,
                    scene_rect: f.area(),
                },
                now,
                &theme::NORMAL,
            );
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(
            !text.contains('%'),
            "an idle agent's dossier carries no active-% meter, got: {text:?}"
        );
        assert!(text.contains("Idle"), "idle state word, got: {text:?}");
        // A bright equal-channel office cell can only turn into a DIMMER
        // equal-channel gray via the drop-shadow dim — the card's `tooltip_bg` is a
        // distinct hue — so its presence proves the shadow ran.
        let buf = term.backend().buffer();
        let shadowed = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                matches!(buf[(x, y)].bg, ratatui::style::Color::Rgb(r, g, b) if r == g && g == b && r == (200.0 * crate::tui::widgets::SHADOW_FACTOR) as u8)
            })
        });
        assert!(
            shadowed,
            "the agent hover tooltip must cast a drop shadow via the shared backing"
        );
    }

    #[test]
    fn both_detail_sources_clip_at_the_one_card_budget() {
        use std::path::Path;
        use std::sync::Arc;
        use std::time::{Duration, SystemTime};

        use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, ToolKind};
        use pixtuoid_core::{AgentId, SceneState};

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
        let id = AgentId::from_transcript_path("/budget/0.jsonl");
        // Long enough that DETAIL_CHARS + 1 chars still exist to over-render.
        let payload: String = ('a'..='z').chain('A'..='Z').collect();
        let render = |state: ActivityState| {
            let slot = AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from("s"),
                cwd: Arc::from(Path::new("/repo")),
                label: "budget".into(),
                state,
                state_started_at: now,
                created_at: now - Duration::from_secs(2),
                last_event_at: now,
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
            let mut scene = SceneState::uniform(12);
            scene.agents.insert(id, slot);
            let mut term = Terminal::new(TestBackend::new(90, 30)).unwrap();
            term.draw(|f| {
                super::paint_hover_tooltip(
                    f,
                    &scene,
                    id,
                    TooltipAt {
                        mx: 20,
                        my: 12,
                        scene_rect: f.area(),
                    },
                    now,
                    &theme::NORMAL,
                );
            })
            .unwrap();
            buffer_text(&term)
        };

        let budget = super::DETAIL_CHARS;
        let fits: String = payload.chars().take(budget).collect();
        let overruns: String = payload.chars().take(budget + 1).collect();

        let active = render(ActivityState::Active {
            detail: Some(Arc::from(format!("Read {payload}").as_str())),
            kind: ToolKind::Read,
            tool_use_id: None,
        });
        assert!(
            active.contains(&fits) && !active.contains(&overruns),
            "the Active tool-detail row must clip at exactly {budget} chars, got: {active:?}"
        );

        let waiting = render(ActivityState::Waiting {
            reason: Arc::from(payload.as_str()),
        });
        assert!(
            waiting.contains(&fits) && !waiting.contains(&overruns),
            "the Waiting reason row must clip at the SAME {budget} chars, got: {waiting:?}"
        );
    }

    #[test]
    fn simple_tooltip_flips_below_when_cursor_near_top() {
        let scene = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };

        // Box height is 3 (1 content line wrapped in `Padding::uniform(1)`), so the
        // content row = box-top + 1.
        let mut top = Terminal::new(TestBackend::new(40, 24)).unwrap();
        top.draw(|f| {
            super::paint_simple_tooltip(
                f,
                " PROBE ",
                TooltipAt {
                    mx: 5,
                    my: 0,
                    scene_rect: scene,
                },
                &theme::NORMAL,
            )
        })
        .unwrap();
        let top_y = row_of(&top, "PROBE").expect("PROBE rendered when cursor at top");
        assert_eq!(
            top_y, 2,
            "cursor at the top edge → box flips below (top=my+1=1, content row 2)"
        );

        let mut low = Terminal::new(TestBackend::new(40, 24)).unwrap();
        low.draw(|f| {
            super::paint_simple_tooltip(
                f,
                " PROBE ",
                TooltipAt {
                    mx: 5,
                    my: 20,
                    scene_rect: scene,
                },
                &theme::NORMAL,
            )
        })
        .unwrap();
        let low_y = row_of(&low, "PROBE").expect("PROBE rendered when cursor low");
        assert_eq!(
            low_y, 18,
            "cursor well below the top → box floats above (top=my-3=17, content row 18)"
        );
    }

    #[test]
    fn mascot_tooltip_verb_keys_on_run_state_not_session_count() {
        assert_eq!(
            mascot_tooltip_text(&mascot(None, false, false, 0)),
            " OpenClaw gateway · idle "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, false, false, 1)),
            " OpenClaw gateway · idle "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, true, false, 1)),
            " OpenClaw gateway · working "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, true, false, 3)),
            " OpenClaw gateway · working · 3 sessions "
        );
    }

    #[test]
    fn mascot_tooltip_names_the_instance_only_when_there_is_a_sibling() {
        assert_eq!(
            mascot_tooltip_text(&mascot(Some("19789"), true, false, 0)),
            " OpenClaw:19789 gateway · working "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(Some("18789"), false, true, 2)),
            " OpenClaw:18789 gateway · model error · 2 sessions "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, false, false, 0)),
            " OpenClaw gateway · idle ",
            "a single gateway's tooltip stays byte-identical"
        );
    }

    #[test]
    fn mascot_tooltip_degraded_overrides_busy_and_idle() {
        assert_eq!(
            mascot_tooltip_text(&mascot(None, false, true, 0)),
            " OpenClaw gateway · model error "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, true, true, 1)),
            " OpenClaw gateway · model error "
        );
        assert_eq!(
            mascot_tooltip_text(&mascot(None, true, true, 3)),
            " OpenClaw gateway · model error · 3 sessions "
        );
    }

    /// Pre-fills the buffer with a bright equal-channel gray; only a SHADOWED office
    /// cell ends up an equal-channel gray darker than that (the card's `tooltip_bg`
    /// is a distinct hue), so its presence proves the shadow ran.
    #[test]
    fn coffee_tooltip_casts_a_drop_shadow_via_the_shared_backing() {
        use ratatui::style::Color;
        let scene = Rect::new(0, 0, 48, 16);
        let bright = Color::Rgb(200, 200, 200);
        let mut term = Terminal::new(TestBackend::new(48, 16)).unwrap();
        term.draw(|f| {
            let full = f.area();
            for y in 0..full.height {
                for x in 0..full.width {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    cell.set_symbol("\u{2580}");
                    cell.fg = bright;
                    cell.bg = bright;
                }
            }
            super::paint_coffee_tooltip(
                f,
                TooltipAt {
                    mx: 20,
                    my: 8,
                    scene_rect: scene,
                },
                &theme::NORMAL,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let shadowed = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                matches!(buf[(x, y)].bg, Color::Rgb(r, g, b) if r == g && g == b && r == (200.0 * crate::tui::widgets::SHADOW_FACTOR) as u8)
            })
        });
        assert!(
            shadowed,
            "the tooltip path must dim office cells into a drop shadow"
        );
    }
}
