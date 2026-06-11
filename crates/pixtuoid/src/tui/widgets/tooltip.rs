use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::{compact_hms, to_color};
use crate::tui::layout::{Layout, DESK_W};
use crate::tui::pet::PetKind;
use crate::tui::pixel_painter::character_anchor;
use crate::tui::pose;
use crate::tui::renderer::clip_widget_rect;
use crate::tui::theme::Theme;

/// Rounded-border tooltip frame shared by every hover/click tooltip. Mirrors
/// the version popup / keyboard help framing so all overlays read as one
/// visual family. Border colour is `label_idle` — gentler than `neon_brand`,
/// which we reserve for actionable popups (help / version notes).
pub(super) fn framed_tooltip<'a>(lines: Vec<Line<'a>>, theme: &Theme) -> Paragraph<'a> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(to_color(theme.ui.label_idle)))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));
    Paragraph::new(lines).block(block)
}

/// Horizontal anchor for a tooltip of width `tip_w`: place it just right of the
/// cursor, but flip to the left side if that would overflow the scene's right
/// edge. Shared by the hover and simple tooltips (their Y logic diverges and
/// stays inline).
fn flip_x_anchor(mx: u16, tip_w: u16, scene_rect: Rect) -> u16 {
    let tx = mx.saturating_add(2);
    if tx.saturating_add(tip_w) > scene_rect.x + scene_rect.width {
        mx.saturating_sub(tip_w + 1)
    } else {
        tx
    }
}

/// Labels above each character — uses `character_anchor` to follow the
/// agent along its current path, color-codes by activity, falls back to
/// disambiguating session-id suffix only when multiple agents share a label.
///
/// `hovered` highlights one agent's label: bright white + bold + leading
/// ▸ marker so the focused character is easy to pick out of a crowd.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_label_widgets(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
    scene_rect: Rect,
    hovered: Option<AgentId>,
    theme: &crate::tui::theme::Theme,
) {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for agent in &agents {
        *label_counts.entry(&*agent.label).or_insert(0) += 1;
    }
    for agent in &agents {
        let Some(anchor) = character_anchor(agent, layout, now, rctx) else {
            continue;
        };
        let lx = scene_rect.x + anchor.x.saturating_sub(2);
        let ly = scene_rect.y + (anchor.y / 2).saturating_sub(1);
        let needs_disambig = label_counts.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.chars().count() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            let id4 = disambig_suffix(&agent.session_id);
            std::borrow::Cow::Owned(format!("{}·{id4}", agent.label))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
        let display = truncate_label(&raw, (DESK_W + 4) as usize);
        let is_hovered = hovered == Some(agent.agent_id);
        let label_color = if is_hovered {
            Color::White
        } else if agent.exiting_at.is_some() {
            to_color(theme.ui.label_exiting)
        } else {
            match &agent.state {
                ActivityState::Active { .. } => to_color(theme.ui.label_active),
                ActivityState::Waiting { .. } => to_color(theme.ui.label_waiting),
                ActivityState::Idle => to_color(theme.ui.label_idle),
            }
        };
        let text = if is_hovered {
            format!("▸{}", display)
        } else {
            format!("●{}", display)
        };
        let mut style = Style::default().fg(label_color);
        if is_hovered {
            style = style.add_modifier(ratatui::style::Modifier::BOLD);
        }
        let para = Paragraph::new(Span::styled(text, style));
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

/// Floating detail panel painted near the cursor when an agent is hovered.
/// Shows the label, source, state, current tool detail, cwd, and session
/// id. Positioned to avoid the cursor itself and the screen edges.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_hover_tooltip(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    agent_id: AgentId,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    now: SystemTime,
    theme: &crate::tui::theme::Theme,
) {
    let Some(agent) = scene.agents.get(&agent_id) else {
        return;
    };

    let (state_label, state_detail, state_color) = match &agent.state {
        ActivityState::Idle => ("Idle", String::new(), to_color(theme.ui.label_idle)),
        ActivityState::Active { detail, .. } => (
            "Active",
            detail.as_deref().unwrap_or("").to_string(),
            to_color(theme.ui.label_active),
        ),
        ActivityState::Waiting { reason } => (
            "Waiting",
            reason.to_string(),
            to_color(theme.ui.label_waiting),
        ),
    };
    let cwd_short = agent
        .cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)");

    let session_secs = now
        .duration_since(agent.created_at)
        .unwrap_or_default()
        .as_secs();
    let duration_str = compact_hms(session_secs);
    let active_str = if session_secs >= 5 {
        let pct = (agent.active_ms / 1000)
            .checked_mul(100)
            .and_then(|n| n.checked_div(session_secs))
            .map(|p| p.min(100))
            .unwrap_or(0);
        format!("{pct}%")
    } else {
        "--%".to_string()
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        agent.label.to_string(),
        Style::default()
            .fg(to_color(theme.ui.tooltip_title))
            .add_modifier(ratatui::style::Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::raw("● "),
        Span::styled(state_label, Style::default().fg(state_color)),
    ]));
    if !state_detail.is_empty() {
        let trimmed: String = state_detail.chars().take(34).collect();
        lines.push(Line::from(Span::styled(
            format!("  {}", trimmed),
            Style::default().fg(to_color(theme.ui.tooltip_text)),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("\u{1f4c1} {}", cwd_short),
        Style::default().fg(to_color(theme.ui.label_idle)),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "\u{23f1} {} \u{00b7} {} calls \u{00b7} {} active",
            duration_str, agent.tool_call_count, active_str
        ),
        Style::default().fg(to_color(theme.ui.label_idle)),
    )));

    let content_h = lines.len() as u16;
    let content_w = lines.iter().map(|l| l.width() as u16).max().unwrap_or(20);
    // +2 cols / +2 rows accounts for the rounded Block border on all sides.
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

    f.render_widget(ratatui::widgets::Clear, clipped);
    f.render_widget(framed_tooltip(lines, theme), clipped);
}

fn paint_simple_tooltip(
    f: &mut ratatui::Frame<'_>,
    text: &str,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let line = Line::from(Span::styled(
        text,
        Style::default()
            .fg(to_color(theme.ui.tooltip_title))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    // +2 cols / +2 rows wrap the single content line in the rounded border.
    // Size by DISPLAY width, not char count: wide glyphs (e.g. the coffee
    // ☕, 2 cells) would otherwise undersize the box by a column and clip
    // the trailing content. Matches paint_hover_tooltip's `l.width()`.
    let tip_w = (line.width() as u16 + 2).min(scene_rect.width);
    let tip_h = 3u16.min(scene_rect.height);
    let tx = flip_x_anchor(mx, tip_w, scene_rect);
    // Float above the cursor; flip below if there isn't room for the framed
    // tooltip above. Guard on geometry (cursor within tip_h of the top) rather
    // than the post-saturation `ty`, which can't detect overflow when
    // scene_rect.y == 0 (saturating_sub floors at 0, never < 0).
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
        f.render_widget(ratatui::widgets::Clear, r);
        f.render_widget(framed_tooltip(vec![line], theme), r);
    }
}

pub(crate) fn paint_coffee_tooltip(
    f: &mut ratatui::Frame<'_>,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    paint_simple_tooltip(f, " \u{2615} Buy Ivan a coffee ", mx, my, scene_rect, theme);
}

pub(crate) fn paint_furniture_tooltip(
    f: &mut ratatui::Frame<'_>,
    label: &str,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let text = format!(" {} ", label);
    paint_simple_tooltip(f, &text, mx, my, scene_rect, theme);
}

/// Pet tooltip — state-dependent text rendered near the cursor.
/// Same visual style as furniture tooltips (dark bg, light text).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_pet_tooltip(
    f: &mut ratatui::Frame<'_>,
    kind: PetKind,
    anim_name: &str,
    is_on_cooldown: bool,
    display_name: &str,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    // The state strings (cooldown reaction / sleeping / pet-me) are NOT user-
    // configurable; only the idle/walk label is the pet's NAME, which the caller
    // resolves (custom from the `[[pets]]` stanza, else `PetKind::default_name`).
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
    paint_simple_tooltip(f, text, mx, my, scene_rect, theme);
}

/// Fit a label into `budget` chars without losing the `·xxxx` session-id
/// disambiguation suffix that the reducer appends to colliding cwds.
/// Truncates from the base (left side of the `·`), not from the suffix —
/// otherwise the disambig becomes useless ("TikTok-Android·a" tells us
/// nothing the base alone wouldn't).
pub(super) fn truncate_label(label: &str, budget: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if label.chars().count() <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind('\u{00b7}') {
        let suffix = &label[sep_byte..];
        let suffix_len = suffix.chars().count();
        if suffix_len < budget {
            let base = &label[..sep_byte];
            let base_take = budget - suffix_len;
            let truncated: String = base.chars().take(base_take).collect();
            return Cow::Owned(format!("{truncated}{suffix}"));
        }
    }
    Cow::Owned(label.chars().take(budget).collect())
}

/// Paint chitchat speech bubbles above agents who are chatting at a
/// social waypoint. Each bubble is a small Paragraph with the speaker's
/// line of text, positioned above the agent's sprite head.
pub fn paint_chitchat_bubbles(
    f: &mut ratatui::Frame<'_>,
    bubbles: &[crate::tui::chitchat::ChitchatBubble],
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    for bubble in bubbles {
        let text = format!(" {} ", bubble.text);
        // Size by DISPLAY width, not byte length: a wide-glyph quip (the line
        // pool can grow) would otherwise over-size + mis-center the bubble.
        // Matches the rest of this file (paint_simple_tooltip uses `.width()`).
        let line = Line::from(text.clone());
        let tip_w = line.width() as u16;
        let tip_h = 1u16;

        let cell_x = scene_rect.x + bubble.anchor.x;
        let cell_y = scene_rect.y + bubble.anchor.y / 2;

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

/// 4-hex-char disambiguation suffix, hashed from the whole `session_id` —
/// shape-agnostic where any SLICE of the id is not: a session_id can be a
/// UUID (CC/Codex — head and tail both unique), a normalized full transcript
/// path (Antigravity — constant head, varying stem tail), or a raw cwd
/// (Reasonix — labels collide exactly when BASENAMES collide, so head AND
/// tail are both constant: `/x/app` vs `/y/app`). Only a digest of the full
/// string distinguishes every shape. Hashing also sidesteps byte-slice
/// panics on multi-byte ids (e.g. `/naïveté/app`) by construction.
/// (`DefaultHasher` is deterministic within a process — the suffix is a
/// per-frame display aid, not a persisted identifier.)
fn disambig_suffix(session_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut h);
    format!("{:04x}", h.finish() & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::disambig_suffix;

    #[test]
    fn uuid_ids_get_distinct_suffixes() {
        let a = disambig_suffix("c0f7fb3f-dc9c-47c3-840d-f775dd2855a3");
        let b = disambig_suffix("019ea57d-7fa7-7812-b864-bdcb9b6c7e17");
        assert_ne!(a, b);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn ag_full_path_ids_get_distinct_suffixes() {
        // Antigravity session_ids are normalized full transcript paths: two
        // same-cwd sessions share the whole prefix; only the stem differs.
        let a = disambig_suffix("/users/me/.gravity/sessions/proj/alpha-01.jsonl");
        let b = disambig_suffix("/users/me/.gravity/sessions/proj/beta-02.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn rx_cwd_ids_with_colliding_basenames_get_distinct_suffixes() {
        // The Reasonix shape that defeats ANY slice of the id: labels collide
        // exactly when basenames collide, so both the head and the tail are
        // constant across the collision (`/work/client-x/app` vs `-y/app`).
        let a = disambig_suffix("/work/client-x/app");
        let b = disambig_suffix("/work/client-y/app");
        assert_ne!(a, b);
    }

    #[test]
    fn multibyte_ids_are_safe_and_deterministic() {
        let a = disambig_suffix("/naïveté/app");
        assert_eq!(a, disambig_suffix("/naïveté/app"));
        assert_eq!(a.len(), 4);
    }
}
