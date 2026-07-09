use std::time::SystemTime;

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::{compact_hms, display_width, source_badge_span, to_color, StateKind};
use crate::tui::renderer::clip_widget_rect;
use pixtuoid_scene::layout::{Layout, DESK_W};
use pixtuoid_scene::overlay::disambig_suffix;
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::tool_glow_for_kind;
use pixtuoid_scene::pose;

/// Borderless tooltip frame shared by every hover/click tooltip: just the padded
/// text. The `Clear` + solid `tooltip_bg` fill + drop shadow come from the shared
/// `super::paint_card_backing`, which the caller paints UNDER this — so every
/// borderless card's backing (tooltip and modal `panel::borderless_panel` alike)
/// has ONE definition and can't drift. The 1-cell uniform padding stands in for
/// the old border, keeping the `+2` width/height math unchanged.
pub(super) fn framed_tooltip<'a>(lines: Vec<Line<'a>>) -> Paragraph<'a> {
    Paragraph::new(lines).block(Block::default().padding(Padding::uniform(1)))
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
            // Tone→role map is single-sourced in `scene::overlay`; this painter
            // only converts the resolved `Rgb` to ratatui `Color`.
            to_color(pixtuoid_scene::overlay::label_tone_rgb(el.tone, theme))
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

/// A char-safe, ~30-column short form of a cwd path for the tooltip: the TAIL
/// (most informative — project dir) with a leading `…` when truncated. Char-
/// sliced, never a byte slice, so a multibyte path can't panic.
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
/// pinned. One coherent card: `[xx]` source badge + label + `·id4` (L1), a dim
/// separator, the `{glyph} {Word}` state line (+ the current tool in its glow
/// hue), the detail / waiting-reason, the `↳ under {parent}` lineage (subagents
/// only), the cwd, and the `⏱` stats (with the active-% meter folded in). Uses
/// the SHARED vocabulary (`StateKind`) + badge (`source_badge_span`) so it can't
/// drift from the footer/board/dashboard. Dim rows use `tooltip_dim`, NOT the
/// live `label_exiting` (C6). Positioned to avoid the cursor + screen edges.
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

    // State vocabulary: an exiting agent reads Exiting regardless of last state.
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

    // The state line: `{glyph} {Word}`, plus the current tool (raw name, glow
    // hue from the TYPED kind — C7) when Active. The detail/reason goes below.
    let mut state_spans = vec![Span::styled(
        format!("{} {}", kind.glyph(), kind.word()),
        Style::default().fg(kind.color(theme)),
    )];
    // An EXITING agent shows none of the live tool/reason affordances — the card
    // already reads `◌ Exiting`, so the walking-out slot's retained Active/Waiting
    // payload (mark_exiting doesn't reset `state`) must not leak a tool span, a
    // `?reason`, or (below) an active-% meter. Gate the whole detail block on the
    // exiting-first `kind`, not the raw `agent.state`.
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
                        Style::default().fg(to_color(tool_glow_for_kind(*tk, &theme.tool_glow))),
                    ));
                }
                if !rest.is_empty() {
                    detail_line = Some(rest.chars().take(34).collect());
                }
            }
        } else if let ActivityState::Waiting { reason } = &agent.state {
            // WHY leads for a blocked agent: the reason IS the detail, `?`-flagged.
            let r: String = reason.chars().take(34).collect();
            detail_line = Some(format!("?{r}"));
        }
    }

    // Body lines (everything below the separator) — built first so the L1 `·id4`
    // right-flush + the separator can size to the widest of them.
    let mut body: Vec<Line> = Vec::new();
    body.push(Line::from(state_spans));
    if let Some(d) = detail_line {
        body.push(Line::from(Span::styled(format!("  {d}"), text_style)));
    }
    // Lineage: subagents only (a resolved parent still in the scene).
    if let Some(parent) = agent.parent_id.and_then(|p| scene.agents.get(&p)) {
        body.push(Line::from(Span::styled(
            format!("\u{21b3} under {}", parent.label),
            dim,
        )));
    }
    body.push(Line::from(Span::styled(
        format!("\u{1f4c1} {}", short_cwd(&agent.cwd)),
        dim,
    )));

    let session_secs = now
        .duration_since(agent.created_at)
        .unwrap_or_default()
        .as_secs();
    let mut stats = format!(
        "\u{23f1} {} \u{b7} {} calls",
        compact_hms(session_secs),
        agent.tool_call_count
    );
    // Active-% meter folded into the ⏱ line (height budget). Fresh agents show no
    // meter (the % is noise before ~5s of accounting); an exiting agent shows none
    // either (keyed off the exiting-first `kind`, matching the tool suppression).
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

    // L1: `[xx] {label}` … right-flushed `·{id4}`. Width = widest body line.
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

    super::paint_card_backing(f, clipped, theme);
    f.render_widget(framed_tooltip(lines), clipped);
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
        super::paint_card_backing(f, r, theme);
        f.render_widget(framed_tooltip(vec![line]), r);
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
pub(crate) fn paint_mascot_tooltip(
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
    fn hover_tooltip_idle_shows_no_meter_and_casts_a_drop_shadow() {
        // An Idle agent's dossier carries NO active-% meter (the meter is folded
        // into the ⏱ line for Active≥5s only). This is also the ONLY coverage for
        // `paint_hover_tooltip`'s backing: it routes through the shared
        // `paint_card_backing`, so a pre-filled bright office must come back
        // dimmed in the drop-shadow band (the other backing caller,
        // `paint_simple_tooltip`, is pinned by the coffee test).
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
            super::paint_hover_tooltip(f, &scene, id, 20, 10, f.area(), now, &theme::NORMAL);
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(
            !text.contains('%'),
            "an idle agent's dossier carries no active-% meter, got: {text:?}"
        );
        // The state line reads the shared vocabulary word.
        assert!(text.contains("Idle"), "idle state word, got: {text:?}");
        // The hover path routes through the shared `paint_card_backing`: a bright
        // equal-channel office cell can only turn into a dimmer equal-channel gray
        // via the drop-shadow dim (the card's `tooltip_bg` is a distinct hue).
        let buf = term.backend().buffer();
        let shadowed = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                // The 200-fill dimmed by the exact SHADOW_FACTOR (derived, not a loose
                // range — so a darkness tweak can't silently pass a near-invisible shadow).
                matches!(buf[(x, y)].bg, ratatui::style::Color::Rgb(r, g, b) if r == g && g == b && r == (200.0 * crate::tui::widgets::SHADOW_FACTOR) as u8)
            })
        });
        assert!(
            shadowed,
            "the agent hover tooltip must cast a drop shadow via the shared backing"
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

    /// The tooltip render path (`paint_coffee_tooltip` → `paint_simple_tooltip` →
    /// the shared `super::paint_card_backing`) casts the drop shadow. Pins that a
    /// future edit dropping the backing call would be caught — the modal path is
    /// covered separately by `panel::borderless_panel_casts_a_flat_offset_shadow`.
    /// Pre-fills the buffer with a bright equal-channel gray; only a shadowed
    /// office cell ends up an equal-channel gray darker than that (the card's
    /// `tooltip_bg` is a distinct hue), so its presence proves the shadow ran.
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
            super::paint_coffee_tooltip(f, 20, 8, scene, &theme::NORMAL);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let shadowed = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                // The 200-fill dimmed by the exact SHADOW_FACTOR (derived, not a loose
                // range — so a darkness tweak can't silently pass a near-invisible shadow).
                matches!(buf[(x, y)].bg, Color::Rgb(r, g, b) if r == g && g == b && r == (200.0 * crate::tui::widgets::SHADOW_FACTOR) as u8)
            })
        });
        assert!(
            shadowed,
            "the tooltip path must dim office cells into a drop shadow"
        );
    }
}
