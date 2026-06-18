use std::time::SystemTime;

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::{compact_hms, to_color};
use crate::tui::renderer::clip_widget_rect;
use pixtuoid_scene::layout::{Layout, DESK_W};
use pixtuoid_scene::overlay::LabelTone;
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pose;
use pixtuoid_scene::theme::Theme;

/// Borderless tooltip frame shared by every hover/click tooltip. No outline
/// (the whole UI dropped popup borders) — a solid `tooltip_bg` fill plus a
/// 1-cell uniform padding stands in for the old rounded border, so the content
/// keeps its readable inset and the existing `+2` width/height math is unchanged
/// (padding consumes exactly the two cells the border used to). The caller still
/// renders `Clear` under it. Reads as one visual family with the other
/// borderless popups (`panel::borderless_panel`).
pub(super) fn framed_tooltip<'a>(lines: Vec<Line<'a>>, theme: &Theme) -> Paragraph<'a> {
    let block = Block::default()
        .padding(Padding::uniform(1))
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
    theme: &pixtuoid_scene::theme::Theme,
) {
    for el in pixtuoid_scene::overlay::build_overlay(scene, layout, now, rctx, hovered) {
        let lx = scene_rect.x + el.anchor_px.x.saturating_sub(2);
        let ly = scene_rect.y + (el.anchor_px.y / 2).saturating_sub(1);
        let label_color = if el.hovered {
            Color::White
        } else {
            match el.tone {
                LabelTone::Exiting => to_color(theme.ui.label_exiting),
                LabelTone::Active => to_color(theme.ui.label_active),
                LabelTone::Waiting => to_color(theme.ui.label_waiting),
                LabelTone::Idle => to_color(theme.ui.label_idle),
            }
        };
        let text = if el.hovered {
            format!("▸{}", el.text)
        } else {
            format!("●{}", el.text)
        };
        let mut style = Style::default().fg(label_color);
        if el.hovered {
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
    theme: &pixtuoid_scene::theme::Theme,
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
    theme: &pixtuoid_scene::theme::Theme,
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
    theme: &pixtuoid_scene::theme::Theme,
) {
    paint_simple_tooltip(f, " \u{2615} Buy Ivan a coffee ", mx, my, scene_rect, theme);
}

pub(crate) fn paint_furniture_tooltip(
    f: &mut ratatui::Frame<'_>,
    label: &str,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
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
    theme: &pixtuoid_scene::theme::Theme,
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

/// Hover tooltip for the gateway lobster mascot — which gateway it represents
/// and whether an agent run is in flight (`busy`). The verb keys on the run
/// state, not the session count (a single-user gateway holds one persistent
/// session even at rest); the session count rides along only as a >1 garnish
/// (the multi-tenant power-user case). Plain text (no emoji) to keep
/// `paint_simple_tooltip`'s width math exact.
#[allow(clippy::too_many_arguments)]
pub fn paint_mascot_tooltip(
    f: &mut ratatui::Frame<'_>,
    name: &str,
    busy: bool,
    degraded: bool,
    active_sessions: u32,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let text = mascot_tooltip_text(name, busy, degraded, active_sessions);
    paint_simple_tooltip(f, &text, mx, my, scene_rect, theme);
}

/// The mascot tooltip's text (pure, unit-tested separately from the ratatui
/// paint). Verb keys on the run state; `degraded` (#317: gateway up but its
/// model backend failing every run) takes precedence over busy/idle so a
/// sickly-red lobster reads "model error". The `>1` session count is a power-user
/// garnish only. Plain text (no emoji) to keep the width math exact.
fn mascot_tooltip_text(name: &str, busy: bool, degraded: bool, active_sessions: u32) -> String {
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

/// Paint chitchat speech bubbles above agents who are chatting at a
/// social waypoint. Each bubble is a small Paragraph with the speaker's
/// line of text, positioned above the agent's sprite head.
pub fn paint_chitchat_bubbles(
    f: &mut ratatui::Frame<'_>,
    bubbles: &[pixtuoid_scene::chitchat::ChitchatBubble],
    scene_rect: Rect,
    theme: &pixtuoid_scene::theme::Theme,
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

#[cfg(test)]
mod tests {
    use super::mascot_tooltip_text;
    use pixtuoid_scene::theme;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    /// Join the whole TestBackend buffer into one string (newline-free) so a
    /// `.contains` probe finds text regardless of which cell the box landed in.
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

    /// Row index (y) of the first row whose joined text contains `needle`, if any.
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

    #[test]
    fn mascot_tooltip_paints_gateway_verb_into_buffer() {
        // The pure text fn is unit-tested above; this pins the ratatui paint glue
        // — the right name + verb actually reach the buffer, and `degraded` wins.
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| {
            super::paint_mascot_tooltip(
                f,
                "OpenClaw",
                true,
                false,
                1,
                10,
                3,
                f.area(),
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
                    "OpenClaw",
                    true,
                    true,
                    1,
                    10,
                    3,
                    f.area(),
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
        // The sit arm (not on cooldown, sitting) is the only branch the render
        // harness never exercises (it covers cooldown purr/woof + sleep).
        use pixtuoid_scene::pet::PetKind;
        let kind = PetKind::Dog;
        let sit = kind.sit_anim();
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| {
            super::paint_pet_tooltip(f, kind, sit, false, "Rex", 10, 3, f.area(), &theme::NORMAL)
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
    fn hover_tooltip_fresh_agent_shows_dashes_for_active_pct() {
        // A <5s-old agent shows the literal `--%` active percentage instead of a
        // computed N% (the fresh-agent branch, line 149).
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
            label: Arc::from("fresh"),
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
        };
        let mut scene = SceneState::uniform(12);
        scene.agents.insert(id, slot);

        let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
        term.draw(|f| {
            super::paint_hover_tooltip(f, &scene, id, 20, 10, f.area(), now, &theme::NORMAL)
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("--%"),
            "fresh (<5s) agent should show the literal --% active, got: {text:?}"
        );
        assert!(
            !text.contains("0%"),
            "fresh agent must not compute a numeric percentage, got: {text:?}"
        );
    }

    #[test]
    fn simple_tooltip_flips_below_when_cursor_near_top() {
        // Near the top edge (my < tip_h) the box must float BELOW the cursor; well
        // below the top it floats ABOVE. The probe substring tags its row.
        let scene = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };

        // Box height is 3 (1 content line wrapped in `Padding::uniform(1)`), so
        // the content row = box-top + 1.
        // my=0, flip-below: box-top = my+1 = 1 → content at row 2 (NOT row 1,
        // which is where it lands if the flip-below branch is removed).
        let mut top = Terminal::new(TestBackend::new(40, 24)).unwrap();
        top.draw(|f| super::paint_simple_tooltip(f, " PROBE ", 5, 0, scene, &theme::NORMAL))
            .unwrap();
        let top_y = row_of(&top, "PROBE").expect("PROBE rendered when cursor at top");
        assert_eq!(
            top_y, 2,
            "cursor at the top edge → box flips below (top=my+1=1, content row 2)"
        );

        // my=20, no flip: box-top = my - tip_h = 17 → content at row 18 (above
        // the cursor). Falsifiable against the flip-below path firing here too.
        let mut low = Terminal::new(TestBackend::new(40, 24)).unwrap();
        low.draw(|f| super::paint_simple_tooltip(f, " PROBE ", 5, 20, scene, &theme::NORMAL))
            .unwrap();
        let low_y = row_of(&low, "PROBE").expect("PROBE rendered when cursor low");
        assert_eq!(
            low_y, 18,
            "cursor well below the top → box floats above (top=my-3=17, content row 18)"
        );
    }

    #[test]
    fn mascot_tooltip_verb_keys_on_run_state_not_session_count() {
        // idle vs working keys on `busy`; the session count only shows as a >1
        // garnish (one persistent session is the single-user norm, not "1 session").
        assert_eq!(
            mascot_tooltip_text("OpenClaw", false, false, 0),
            " OpenClaw gateway · idle "
        );
        assert_eq!(
            mascot_tooltip_text("OpenClaw", false, false, 1),
            " OpenClaw gateway · idle "
        );
        assert_eq!(
            mascot_tooltip_text("OpenClaw", true, false, 1),
            " OpenClaw gateway · working "
        );
        assert_eq!(
            mascot_tooltip_text("OpenClaw", true, false, 3),
            " OpenClaw gateway · working · 3 sessions "
        );
    }

    #[test]
    fn mascot_tooltip_degraded_overrides_busy_and_idle() {
        // #317: gateway up but every model call failing → "model error", and it
        // wins over both busy and idle (a degraded gateway is degraded whether or
        // not a run is in flight). The session garnish still rides along.
        assert_eq!(
            mascot_tooltip_text("OpenClaw", false, true, 0),
            " OpenClaw gateway · model error "
        );
        assert_eq!(
            mascot_tooltip_text("OpenClaw", true, true, 1),
            " OpenClaw gateway · model error "
        );
        assert_eq!(
            mascot_tooltip_text("OpenClaw", true, true, 3),
            " OpenClaw gateway · model error · 3 sessions "
        );
    }
}
