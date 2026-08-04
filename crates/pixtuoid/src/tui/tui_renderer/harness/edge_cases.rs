use super::*;

#[test]
fn too_small_terminal_returns_no_layout_no_panic() {
    let scene = scene_with(vec![idle("/sm/0.jsonl", 0, t0())], 16);
    let mut r = build(15, 8, vec![]); // below the 20×12 scene minimum
    r.render(&scene, &pack(), t0())
        .expect("render must not panic");
    assert!(
        r.cached_layout().is_none(),
        "a too-small terminal yields no layout"
    );
}

#[test]
fn colliding_labels_with_multibyte_session_ids_do_not_panic() {
    let mut scene = SceneState::uniform(16);
    // In `/naïveté/app` the `ï` occupies bytes 3..5, so a `&session_id[..4]` slice
    // splits it; the shared label makes the disambiguation suffix fire.
    let mut mk = |id: &str, desk: usize| {
        let a = AgentId::from_transcript_path(id);
        let mut s = slot(a, 0, desk, t0());
        s.label = "rx\u{00b7}proj".into();
        s.session_id = Arc::from("/na\u{00ef}vet\u{00e9}/app");
        scene.agents.insert(a, s);
    };
    mk("/mb/0.jsonl", 0);
    mk("/mb/1.jsonl", 1);
    let mut r = build(120, 40, vec![]);
    r.render(&scene, &pack(), t0())
        .expect("render must not panic on a multi-byte session_id");
}

#[test]
fn no_layout_frame_paints_the_popup_at_its_clickable_scale() {
    // 100x16 → scene_rect 100x15 passes render()'s 20x12 gate, but buf_h=30 is
    // below compute_with_seed's office minimum → draw_scene returns Ok(None).
    let scene = scene_with(vec![idle("/nl/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 16, vec![]);
    r.set_version_popup(true, t0());
    let t = t0() + Duration::from_millis(150); // mid-entrance ⇒ scale > 0
    let painted = r.version_popup_scale(t);
    assert!(painted > 0.0, "the popup is animating this frame");
    r.render(&scene, &pack(), t).expect("render");
    assert!(
        r.cached_layout().is_none(),
        "no layout produced at this size"
    );
    assert_eq!(
        r.last_popup_scale(),
        painted,
        "the hit-box scale must equal the scale the painter used"
    );
    assert!(
        frame_text(r.frame_buffer()).contains("What's new"),
        "the popup must actually paint on the footer-only frame"
    );
}

#[test]
fn modal_overlays_still_paint_when_the_office_cannot_lay_out() {
    use crate::tui::welcome::{OnboardingFrame, WelcomeRow};
    let scene = scene_with(vec![idle("/tiny/0.jsonl", 0, t0())], 16);

    let mut r = build(80, 24, vec![]);
    r.set_help_open(true);
    r.render(&scene, &pack(), t0()).expect("render");
    assert!(
        r.cached_layout().is_none(),
        "80x24 is below the office layout minimum — this IS the footer-only path"
    );
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Keyboard"),
        "the help overlay must paint on the footer-only frame; frame was:\n{text}"
    );

    let mut r = build(80, 24, vec![]);
    r.set_onboarding_frame(OnboardingFrame {
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
    r.render(&scene, &pack(), t0()).expect("render");
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Welcome to pixtuoid"),
        "the onboarding overlay must paint on the footer-only frame; frame was:\n{text}"
    );

    let mut r = build(80, 24, vec![]);
    r.set_connection_frame_parts(
        true,
        Vec::new(),
        Vec::new(),
        0,
        None,
        None,
        "socket  /tmp/p.sock".into(),
    );
    r.render(&scene, &pack(), t0()).expect("render");
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Sources"),
        "the Sources panel must paint on the footer-only frame; frame was:\n{text}"
    );
}

#[test]
fn a_full_height_modal_never_covers_the_footer_row() {
    let scene = scene_with(vec![idle("/fh/0.jsonl", 0, t0())], 16);
    // 32x31 is the office layout's exact minimum, and the real release notes wrap
    // past 31 rows there.
    let mut r = build(32, 31, vec![]);
    r.set_version_popup(true, t0());
    let t = t0() + Duration::from_millis(400); // fully scaled in
    r.render(&scene, &pack(), t).expect("render");
    let text = frame_text(r.frame_buffer());
    let last_row = text.lines().last().unwrap_or_default().to_string();
    assert!(
        text.contains("Enter to close"),
        "the popup must be on screen for this to test anything; frame was:\n{text}"
    );
    assert!(
        last_row.contains("[q]uit"),
        "the footer must survive a full-height modal, got last row {last_row:?};\
         \nframe was:\n{text}"
    );

    // Compare CELLS, not a substring: surviving glyphs are not a READABLE footer,
    // and the shadow's bottom band dims `fg` only.
    let mut plain = build(32, 31, vec![]);
    plain.render(&scene, &pack(), t).expect("render");
    let (lit, dimmed) = (plain.frame_buffer(), r.frame_buffer());
    let footer_y = lit.area.height - 1;
    for x in 0..lit.area.width {
        let (a, b) = (
            lit.cell((x, footer_y)).expect("footer cell"),
            dimmed.cell((x, footer_y)).expect("footer cell"),
        );
        assert_eq!(
            (a.symbol(), a.fg, a.bg),
            (b.symbol(), b.fg, b.bg),
            "the modal (or its drop shadow) repainted footer cell x={x}; \
             frame was:\n{text}"
        );
    }
}

#[test]
fn modal_overlays_still_paint_during_a_slide_on_a_too_small_terminal() {
    let scene = two_floor_scene();
    // 19 cols ⇒ scene_rect 19x11, under the 20x12 gate on BOTH axes.
    let mut r = build(19, 12, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).expect("render");
    r.set_help_open(true);
    r.set_version_popup(true, now);
    r.navigate_floor(1, now);

    let t = now + Duration::from_millis(100); // mid-slide, mid-entrance
    let painted = r.version_popup_scale(t);
    assert!(painted > 0.0, "the popup is animating this frame");
    r.render(&scene, &pack(), t)
        .expect("transition render on a tiny terminal must not panic");

    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Keyboard"),
        "the help overlay must paint on the slide's footer-only frame; \
         frame was:\n{text}"
    );
    assert_eq!(
        r.last_popup_scale(),
        painted,
        "the click hit-box must carry the scale this arm painted at"
    );
}

#[test]
fn departed_agent_motion_is_evicted_on_a_non_current_floor() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/ev/floor0.jsonl");
    let b = AgentId::from_transcript_path("/ev/floor1.jsonl");
    // Long-idle so both wander and acquire a MotionState.
    let scene = scene_with(
        vec![
            slot(a, 0, 0, t0() - Duration::from_secs(120)),
            slot(b, 1, cap, t0() - Duration::from_secs(120)),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let mut now = t0();

    for _ in 0..10 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    for _ in 0..10 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    assert!(
        r.floor_motion(1).and_then(|m| m.get(&b)).is_some(),
        "floor-1 agent B should have a MotionState after visiting floor 1"
    );

    r.navigate_floor(0, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 0);
    let scene_without_b = scene_with(vec![slot(a, 0, 0, t0() - Duration::from_secs(120))], cap);
    now += Duration::from_millis(33);
    r.evict_missing(&scene_without_b);
    r.render(&scene_without_b, &pack(), now).expect("render");

    assert_eq!(
        r.floor_motion(1).map(|m| m.contains_key(&b)),
        Some(false),
        "a departed agent's MotionState must be evicted even on a non-current floor"
    );
}

#[test]
fn evict_missing_drops_history_and_motion_on_every_floor() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/ev2/floor0.jsonl");
    let b = AgentId::from_transcript_path("/ev2/floor1.jsonl");
    // Fresh agents: the entry walk populates BOTH history (per-frame walker
    // position records) and motion (entry profile) on their floors.
    let scene = scene_with(vec![slot(a, 0, 0, t0()), slot(b, 1, cap, t0())], cap);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();

    for _ in 0..5 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).expect("render");
    }
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    for _ in 0..5 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).expect("render");
    }
    assert_eq!(
        r.floor_history(0).map(|h| h.contains(a)),
        Some(true),
        "floor-0 history should hold agent A after its entry frames"
    );
    assert_eq!(
        r.floor_history(1).map(|h| h.contains(b)),
        Some(true),
        "floor-1 history should hold agent B after its entry frames"
    );
    assert!(
        r.floor_motion(1).and_then(|m| m.get(&b)).is_some(),
        "floor-1 motion should hold agent B"
    );

    let empty = SceneState::uniform(cap);
    r.evict_missing(&empty);

    for floor in 0..2 {
        assert_eq!(
            r.floor_history(floor)
                .map(|h| h.contains(a) || h.contains(b)),
            Some(false),
            "departed agents' PoseHistory must be evicted on floor {floor}"
        );
        assert_eq!(
            r.floor_motion(floor)
                .map(|m| m.contains_key(&a) || m.contains_key(&b)),
            Some(false),
            "departed agents' MotionState must be evicted on floor {floor}"
        );
    }
}

#[test]
fn floor_transition_clears_stale_pet_position() {
    let cap = 16;
    let mut scene = SceneState::uniform(cap);
    let a = AgentId::from_transcript_path("/pettrans/f0.jsonl");
    let b = AgentId::from_transcript_path("/pettrans/f1.jsonl");
    scene.agents.insert(a, slot(a, 0, 0, t0()));
    scene.agents.insert(b, slot(b, 1, cap, t0())); // floor 1 ⇒ navigate_floor(1) valid

    let mut r = build(100, 40, vec![PetKind::Cat]);
    let mut now = t0();
    for _ in 0..3 {
        r.render(&scene, &pack(), now).expect("render");
        now += Duration::from_millis(33);
    }
    assert!(
        r.cached_pet_pos().is_some(),
        "a pet should be drawn on the normal floor-0 frame"
    );

    r.navigate_floor(1, now);
    r.render(&scene, &pack(), now).expect("render"); // single in-flight transition frame
    assert!(
        r.cached_pet_pos().is_none(),
        "an in-flight floor transition must clear the stale pet position"
    );
}

#[test]
fn layout_compute_none_bails_to_footer_only() {
    let scene = scene_with(vec![idle("/lc/0.jsonl", 0, t0())], 16);
    // scene_rect 28×39: width 28 ≥ 20 (passes gate), buf_w 28 < MIN_W → compute
    // returns None, hitting the second bail arm.
    let mut r = build(28, 40, vec![]);
    r.render(&scene, &pack(), t0())
        .expect("render must not error on the compute-None bail");
    assert!(
        r.cached_layout().is_none(),
        "a layout that fails compute yields no cached layout"
    );
}
