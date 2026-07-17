use super::*;

// ===================================================================
// Debug overlay (the `w` toggle)
// ===================================================================

#[test]
fn walkable_debug_toggle_tints_blocked_pixels_and_is_reversible() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 60, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();

    // A known-blocked pixel from the live mask, below the busy top wall band.
    let layout = r.cached_layout().expect("layout").clone();
    let (bx, by) = (0..layout.buf_h)
        .flat_map(|y| (0..layout.buf_w).map(move |x| (x, y)))
        .find(|&(x, y)| y > layout.top_margin + 4 && !layout.is_walkable(x, y))
        .expect("some blocked cell below the wall band");

    // Toggle ON → the overlay reddens the blocked cell + changes the frame.
    r.set_debug_walkable(true);
    r.render(&scene, &pack(), now).unwrap();
    let on = r.buf().clone();
    // The mask layer blends blocked cells toward the BLOCKED tint (220,60,60),
    // so the cell must move CLOSER to that red than it was (a warm cell's red
    // channel barely rises, but green/blue drop — distance is the robust check).
    let to_red = |c: pixtuoid_core::sprite::Rgb| {
        (c.r as i32 - 220).abs() + (c.g as i32 - 60).abs() + (c.b as i32 - 60).abs()
    };
    assert!(
        to_red(on.get(bx, by)) < to_red(before.get(bx, by)),
        "debug overlay must tint a blocked cell toward red (was {:?}, now {:?})",
        before.get(bx, by),
        on.get(bx, by),
    );
    let on_diff = region_diff(&before, &on, 0, 0, before.width(), before.height());
    assert!(
        on_diff > 1_000,
        "the debug layer must visibly change the frame"
    );

    // Toggle OFF → the scene returns to the un-overlaid frame (additive layer).
    r.set_debug_walkable(false);
    r.render(&scene, &pack(), now).unwrap();
    let off_diff = region_diff(&before, r.buf(), 0, 0, before.width(), before.height());
    assert!(
        off_diff < 200,
        "toggling the debug layer off must restore the scene (diff={off_diff})"
    );
}

// ===================================================================
// Version popup
// ===================================================================

#[test]
fn version_popup_entrance_reaches_full_scale() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let s = r.version_popup_scale(t0() + Duration::from_millis(250));
    assert!(s > 0.99, "entrance eases to ~1.0, got {s}");
}

#[test]
fn version_popup_dismissal_reaches_zero() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let mid = t0() + Duration::from_millis(250);
    r.set_version_popup(false, mid);
    let s = r.version_popup_scale(mid + Duration::from_millis(200));
    assert!(s < 0.01, "dismissal eases to ~0.0, got {s}");
}

#[test]
fn version_popup_interrupt_continues_from_edge() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    // Interrupt entrance ~halfway.
    let half = t0() + Duration::from_millis(100);
    let scale_at_interrupt = r.version_popup_scale(half);
    r.set_version_popup(false, half);
    let s = r.version_popup_scale(half + Duration::from_millis(1));
    assert!(
        (s - scale_at_interrupt).abs() < 0.2,
        "interrupted animation continues from current scale ({scale_at_interrupt}), not a snap (got {s})"
    );
}

// ===================================================================
// Help overlay
// ===================================================================

#[test]
fn help_overlay_renders_shortcuts() {
    let scene = scene_with(vec![idle("/help/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.set_help_open(true);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.help_open());
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("theme") || text.contains("Keyboard") || text.contains("help"),
        "help overlay should list shortcuts; frame was:\n{text}"
    );
}

#[test]
fn onboarding_overlay_renders_roster_and_hint() {
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let scene = scene_with(vec![idle("/onboard/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    // A two-CLI roster; a large elapsed so the typewriter + every staggered row
    // + the key hint are all fully revealed.
    r.set_onboarding_frame(OnboardingFrame {
        open: true,
        rows: vec![
            WelcomeRow {
                source_id: "codex",
                label_prefix: "cx",
                display_name: "Codex".into(),
                checked: true,
            },
            WelcomeRow {
                source_id: "claude-code",
                label_prefix: "cc",
                display_name: "Claude Code".into(),
                checked: false,
            },
        ],
        selected: 0,
        elapsed_ms: 100_000,
        dim: 0.4,
    });
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Welcome to pixtuoid"),
        "onboarding title; frame:\n{text}"
    );
    assert!(text.contains("Codex"), "checked roster row; frame:\n{text}");
    assert!(
        text.contains("Claude Code"),
        "unchecked roster row; frame:\n{text}"
    );
    assert!(
        text.contains("space toggle") && text.contains("esc skip"),
        "key hint shown once rows are in; frame:\n{text}"
    );
    assert!(
        text.contains("press m anytime"),
        "the one-line audio offer rides the hints (#633); frame:\n{text}"
    );
}

#[test]
fn onboarding_dims_the_office_buffer() {
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let scene = scene_with(vec![idle("/dim/0.jsonl", 0, t0())], 16);

    // Baseline: onboarding closed → full-brightness office.
    let mut base = build(100, 40, vec![]);
    base.render(&scene, &pack(), t0()).unwrap();
    let bright = avg_lum(base.buf(), 0, 0, base.buf().width(), base.buf().height());

    // Same scene, onboarding open + fully ramped (large elapsed) → the office
    // pixel buffer is dimmed as the modal backdrop (the card paints on the cell
    // layer, not the buffer, so this measures the office only).
    let mut dimmed = build(100, 40, vec![]);
    dimmed.set_onboarding_frame(OnboardingFrame {
        open: true,
        rows: vec![WelcomeRow {
            source_id: "codex",
            label_prefix: "cx",
            display_name: "Codex".into(),
            checked: true,
        }],
        selected: 0,
        elapsed_ms: 100_000,
        dim: 0.4,
    });
    dimmed.render(&scene, &pack(), t0()).unwrap();
    let dim = avg_lum(
        dimmed.buf(),
        0,
        0,
        dimmed.buf().width(),
        dimmed.buf().height(),
    );

    assert!(
        dim < bright * 0.6,
        "onboarding should dim the office buffer: dim={dim} vs bright={bright}"
    );
}

#[test]
fn onboarding_dims_both_sliding_buffers_on_the_transition_path() {
    // The floor-slide path (render_transition) threads the OnboardingFrame but
    // used to drop its `dim`, so a floor change while the overlay is open flashed
    // the office to full brightness for the ~400ms slide. Both sliding per-floor
    // buffers must now dim by the same factor draw_scene applies. We observe the
    // from-floor buffer (floor_buf(0)) mid-slide with vs without the dim.
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let p = pack();
    let scene = two_floor_scene();
    let now = t0();
    let mid = now + Duration::from_millis(200);

    // Baseline: a mid-slide frame with onboarding CLOSED (default dim = 1.0) →
    // the from-floor buffer stays full brightness.
    let mut bright_r = build(100, 40, vec![]);
    bright_r.render(&scene, &p, now).unwrap();
    bright_r.navigate_floor(1, now);
    bright_r.render(&scene, &p, mid).unwrap();
    assert!(bright_r.transition().is_some(), "baseline still mid-slide");
    let bb = bright_r.floor_buf(0).expect("from-floor buffer exists");
    let bright = avg_lum(bb, 0, 0, bb.width(), bb.height());

    // Same mid-slide frame, onboarding OPEN + fully ramped (dim = 0.4) → the
    // transition path must dim the sliding buffer.
    let mut dim_r = build(100, 40, vec![]);
    dim_r.render(&scene, &p, now).unwrap();
    dim_r.navigate_floor(1, now);
    dim_r.set_onboarding_frame(OnboardingFrame {
        open: true,
        rows: vec![WelcomeRow {
            source_id: "codex",
            label_prefix: "cx",
            display_name: "Codex".into(),
            checked: true,
        }],
        selected: 0,
        elapsed_ms: 100_000,
        dim: 0.4,
    });
    dim_r.render(&scene, &p, mid).unwrap();
    assert!(dim_r.transition().is_some(), "dimmed still mid-slide");
    let db = dim_r.floor_buf(0).expect("from-floor buffer exists");
    let dim = avg_lum(db, 0, 0, db.width(), db.height());

    assert!(
        dim < bright * 0.6,
        "the transition path must dim the sliding buffers: dim={dim} vs bright={bright}"
    );
}

// ===================================================================
// Tooltip variants on hover (exercise widgets/tooltip.rs branches)
// ===================================================================

#[test]
fn coffee_machine_tooltip_on_hover() {
    let scene = scene_with(vec![idle("/tt/c.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // Find a cell that hits the coffee machine.
    let mut hover = None;
    'scan: for my in 0..48u16 {
        for mx in 0..140u16 {
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                hover = Some((mx, my));
                break 'scan;
            }
        }
    }
    let hover = hover.expect("coffee machine should be hit-testable");
    r.set_mouse_pos(Some(hover));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Ivan"),
        "hovering the coffee machine shows the Buy-Ivan-a-coffee tooltip"
    );
}

#[test]
fn furniture_tooltip_on_hover_over_empty_desk() {
    // Agent on desk 0; hover an EMPTY desk so furniture (not agent) tooltip wins.
    let scene = scene_with(vec![idle("/tt/f.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    if layout.home_desks.len() < 2 {
        return;
    }
    let d1 = layout.home_desks[1];
    r.set_mouse_pos(Some((d1.x + 4, d1.y / 2 + 1)));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Desk"),
        "hovering an empty desk shows the Desk furniture tooltip"
    );
}

#[test]
fn pet_tooltip_on_hover() {
    let scene = scene_with(vec![active("/tt/p.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(140, 48, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Cat") || text.contains("purr"),
        "hovering the cat shows its tooltip"
    );
}

#[test]
fn pet_tooltip_shows_custom_name() {
    let scene = scene_with(vec![active("/tt/cn.jsonl", 0, "Edit", t0())], 16);
    let cat = pixtuoid_scene::pet::Pet {
        kind: PetKind::Cat,
        name: "Luna".to_string(),
    };
    let mut r = build_pets(140, 48, vec![cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Luna"),
        "hovering the cat shows its custom name; got:\n{text}"
    );
    assert!(
        !text.contains("Office Cat"),
        "custom name replaces the default, not appended"
    );
}

#[test]
fn pet_tooltip_falls_back_to_default_name_when_not_configured() {
    let scene = scene_with(vec![active("/tt/fb.jsonl", 0, "Edit", t0())], 16);
    // No custom name → default ("Office Cat"). `build` defaults the name.
    let mut r = build(140, 48, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, .. } = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Office Cat"),
        "an unconfigured cat falls back to the default name; got:\n{text}"
    );
}

// ===================================================================
// Tooltip state arms: Active (with detail), Waiting, exiting label,
// active-% numeric, flip-left, bottom-edge flip-up (CG4/CG5/CG6)
// ===================================================================

#[test]
fn hovered_active_agent_tooltip_shows_state_and_detail() {
    // active() sets last_event_at = started; created >5s ago so active_str is
    // a numeric percent (not "--%"), and active_ms>0 forces a non-zero %.
    let mut a = active(
        "/ttA/0.jsonl",
        0,
        "Edit src/lib.rs",
        t0() - Duration::from_secs(600),
    );
    a.active_ms = 120_000; // 120s active over a 600s session ⇒ 20%
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Active"), "active state word: {text}");
    // The detail splits: the tool name (`Edit`) rides the state line, the
    // remaining args (`src/lib.rs`) the indented detail line below.
    assert!(text.contains("Edit"), "tool name on the state line: {text}");
    assert!(text.contains("src/lib.rs"), "detail line args: {text}");
    // Active ≥5s folds a numeric meter into the stats line (no `--%` anymore). Teeth
    // on BOTH computations: 120s/600s ⇒ 20%, and 20% ⇒ 1 filled + 4 empty cells
    // (`filled = (20*5).div_ceil(100).min(5) = 1`).
    assert!(text.contains("20%"), "exact active percent: {text}");
    assert!(
        text.contains("\u{25ae}\u{25af}\u{25af}\u{25af}\u{25af}"),
        "meter fill (1 filled ▮ + 4 empty ▯): {text}"
    );
}

#[test]
fn hovered_burning_agent_tooltip_shows_model_and_fresh_effort() {
    // The `★ {model} · {effort}` dossier row: RAW model, effort suffixed only
    // while fresh (the same burn TTL the flame reads). Model-less agents skip
    // the row entirely — pinned by the negative half.
    use pixtuoid_core::state::EffortObservation;
    let mut a = active(
        "/burn/0.jsonl",
        0,
        "Read src/main.rs",
        t0() - Duration::from_secs(30),
    );
    a.source = std::sync::Arc::from("claude-code");
    a.model = Some("claude-fable-5".into());
    a.effort = Some(EffortObservation::new("ultra".into(), t0()));
    let id = a.agent_id;
    let plain = active(
        "/burn/1.jsonl",
        1,
        "Read src/lib.rs",
        t0() - Duration::from_secs(30),
    );
    let plain_id = plain.agent_id;
    let scene = scene_with(vec![a, plain], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{2605} claude-fable-5 \u{b7} ultra"),
        "model + fresh effort row: {text}"
    );
    // A model-less agent's dossier has NO ★ row.
    super::hover_agent(&mut r, &scene, plain_id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    // (The wall board's own `★ Star` CTA is unrelated — assert the MODEL row
    // specifically is gone, not the glyph.)
    assert!(
        !text.contains("\u{2605} claude-fable-5"),
        "no model row without an observation: {text}"
    );
}

#[test]
fn stale_effort_drops_off_the_dossier() {
    // Past the burn TTL the flame is out — the ★ row must drop the `· effort`
    // suffix too (fresh_effort is the ONE rule both read). An hour-old stamp
    // is comfortably past any sane freshness window (authority:
    // pixtuoid_scene::burn::EFFORT_TTL_SECS = 600, crate-private).
    use pixtuoid_core::state::EffortObservation;
    let mut a = active(
        "/burn/2.jsonl",
        0,
        "Read src/main.rs",
        t0() - Duration::from_secs(30),
    );
    a.source = std::sync::Arc::from("claude-code");
    a.model = Some("claude-fable-5".into());
    a.effort = Some(EffortObservation::new(
        "ultra".into(),
        t0() - Duration::from_secs(3600),
    ));
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{2605} claude-fable-5"),
        "model row stays: {text}"
    );
    assert!(
        !text.contains("\u{b7} ultra"),
        "stale effort suffix must be suppressed: {text}"
    );
}

#[test]
fn the_exit_sentinel_never_renders_in_the_dossier() {
    // The CC decoder's synthesized `ultra_exit` kills the flame via
    // last-seen-wins, but it is an internal token, not an effort — the ★ row
    // shows the bare model, never the sentinel.
    use pixtuoid_core::source::claude_code::ULTRA_EXIT_LABEL;
    use pixtuoid_core::state::EffortObservation;
    let mut a = active(
        "/burn/3.jsonl",
        0,
        "Read src/main.rs",
        t0() - Duration::from_secs(30),
    );
    a.source = std::sync::Arc::from("claude-code");
    a.model = Some("claude-fable-5".into());
    a.effort = Some(EffortObservation::new(ULTRA_EXIT_LABEL.into(), t0()));
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{2605} claude-fable-5"),
        "model row stays: {text}"
    );
    assert!(
        !text.contains(ULTRA_EXIT_LABEL),
        "the internal sentinel must never render: {text}"
    );
}

#[test]
fn hovered_agent_tooltip_shows_source_badge() {
    // The dossier leads with the shared `[xx]` source badge (same builder as the
    // dashboard/Sources panel) so the tooltip can't drift from them.
    let mut a = active(
        "/badge/0.jsonl",
        0,
        "Read src/main.rs",
        t0() - Duration::from_secs(30),
    );
    // The badge resolves the source id → `label_prefix` via the registry; use the
    // real id (the fixtures' shorthand "cc" is a prefix, not a registered id).
    a.source = std::sync::Arc::from("claude-code");
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    // claude-code → the `[cc]` badge prefix.
    assert!(text.contains("[cc]"), "source badge on the tooltip: {text}");
    // …and L1 right-flushes the `·{id4}` disambiguation suffix (the fixtures'
    // session_id is "s"; disambig_suffix is deterministic, zero-seeded SipHash).
    let id4 = pixtuoid_scene::overlay::disambig_suffix("s");
    assert!(
        text.contains(&format!("\u{b7}{id4}")),
        "id4 disambiguation suffix ·{id4}: {text}"
    );
}

#[test]
fn hovered_subagent_tooltip_shows_lineage() {
    // A subagent's dossier carries a `↳ under {parent}` line; a root agent's
    // does not (the parent must resolve in the scene).
    let parent = active(
        "/lin/root.jsonl",
        0,
        "Read a",
        t0() - Duration::from_secs(60),
    );
    let parent_id = parent.agent_id;
    let mut child = active(
        "/lin/child.jsonl",
        1,
        "Edit b",
        t0() - Duration::from_secs(30),
    );
    child.label = "kid".into();
    child.parent_id = Some(parent_id);
    let child_id = child.agent_id;
    let scene = scene_with(vec![parent, child], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, child_id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{21b3} under"),
        "lineage line on the subagent: {text}"
    );
}

#[test]
fn hovered_waiting_agent_tooltip_shows_reason() {
    let mut a = idle("/ttW/0.jsonl", 0, t0() - Duration::from_secs(60));
    a.state = ActivityState::Waiting {
        reason: Arc::from("permission to edit"),
    };
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Waiting"), "waiting state arm: {text}");
    // The reason is `?`-flagged (WHY leads for a blocked agent) — assert the
    // marker, not just the bare reason text.
    assert!(
        text.contains("?permission"),
        "?-flagged reason line: {text}"
    );
}

#[test]
fn hovered_exiting_agent_tooltip_suppresses_meter() {
    // A walking-out agent keeps its retained Active payload (mark_exiting doesn't
    // reset `state`), but the dossier reads `◌ Exiting` — so the active-% meter is
    // suppressed (keyed off the exiting-first `kind`, matching the tool span).
    let mut a = active(
        "/exM/0.jsonl",
        0,
        "Edit src/lib.rs",
        t0() - Duration::from_secs(600),
    );
    a.active_ms = 120_000; // a 20% meter if it were NOT exiting
    a.exiting_at = Some(t0());
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Exiting"), "exiting state word: {text}");
    assert!(
        !text.contains('%'),
        "no active-% meter on an exiting card: {text}"
    );
}

#[test]
fn hovered_exiting_agent_tooltip_suppresses_waiting_reason() {
    // Symmetric to the meter: a Waiting slot now exiting reads `◌ Exiting`, not a
    // `?reason` (the Waiting arm is gated on the exiting-first `kind` too).
    let mut a = idle("/exW/0.jsonl", 0, t0() - Duration::from_secs(60));
    a.state = ActivityState::Waiting {
        reason: Arc::from("permission to edit"),
    };
    a.exiting_at = Some(t0());
    let id = a.agent_id;
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("Exiting"), "exiting state word: {text}");
    assert!(
        !text.contains("?permission"),
        "no ?reason line on an exiting card: {text}"
    );
    assert!(
        !text.contains("Waiting"),
        "state word is Exiting, not Waiting: {text}"
    );
}

#[test]
fn exiting_agent_label_uses_exiting_color() {
    // The exiting_at branch in paint_label_widgets: render an exiting agent and
    // confirm its label still paints (the branch runs without panic). Color is
    // theme-internal; we assert the label survives the exiting code path.
    let mut a = idle("/ttE/0.jsonl", 0, t0() - Duration::from_secs(10));
    a.label = "LEAVING".into();
    a.exiting_at = Some(t0());
    let scene = scene_with(vec![a], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0() + Duration::from_millis(100))
        .unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(text.contains("LEAVING"), "exiting agent label: {text}");
}

#[test]
fn hovered_then_removed_agent_is_a_safe_noop() {
    // The hover hit re-resolves per frame: with the agent removed from the
    // scene the same parked mouse resolves nothing (or another surface) —
    // the render must not panic. (The pinned-id variant of this test died
    // with click-to-pin; hover is re-evaluated each frame by construction.)
    let id = AgentId::from_transcript_path("/ttGone/0.jsonl");
    let scene = scene_with(vec![slot(id, 0, 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    super::hover_agent(&mut r, &scene, id, 120, 44);
    // Re-render with the agent removed → the hover resolves None, no tooltip.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .expect("render must not panic when the hovered agent vanished");
}
