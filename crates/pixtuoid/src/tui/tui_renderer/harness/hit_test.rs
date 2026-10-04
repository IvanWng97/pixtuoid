use super::*;
use pixtuoid_scene::pixel_painter::AgentFrame;

#[test]
fn furniture_hit_test_resolves_against_rendered_layout() {
    let scene = scene_with(vec![idle("/hit/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let desk = layout.home_desks[0];
    let hit = crate::tui::hit_test::hit_test_furniture(
        layout,
        crate::tui::geometry::CellArea::half_block(desk.x + 4, desk.y / 2 + 1),
    );
    assert_eq!(
        hit,
        Some("Desk"),
        "a desk pixel should hit the Desk furniture in the cached layout"
    );
}

#[test]
fn coffee_machine_hit_test_resolves_on_pantry() {
    use pixtuoid_scene::layout::WaypointKind;
    let scene = scene_with(vec![idle("/cm/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let pantry = layout
        .waypoints
        .iter()
        .find(|w| w.kind == WaypointKind::Pantry)
        .expect("a 140×48 office must lay out a pantry");
    let cx = pantry.pos.x;
    let cy = pantry.pos.y / 2;
    let mut found = false;
    for dx in -14i32..=14 {
        for dy in -4i32..=4 {
            let mx = (cx as i32 + dx).max(0) as u16;
            let my = (cy as i32 + dy).max(0) as u16;
            if crate::tui::hit_test::hit_test_coffee_machine(
                layout,
                crate::tui::geometry::CellArea::half_block(mx, my),
            ) {
                found = true;
            }
        }
    }
    assert!(
        found,
        "the coffee machine should be hit-testable somewhere on the pantry counter"
    );
}

#[test]
fn pet_hit_test_resolves_at_pet_position() {
    let scene = scene_with(vec![active("/ph/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(120, 44, vec![PetKind::Cat]);
    r.render(&scene, pack(), t0()).unwrap();
    let PetHover {
        centre: pos,
        anim,
        kind,
    } = r.cached_pet_pos().expect("pet placed");
    assert!(
        crate::tui::hit_test::hit_test_pet(
            kind,
            pos,
            anim,
            crate::tui::geometry::CellArea::half_block(pos.x, pos.y / 2)
        ),
        "clicking the pet's own position should hit it"
    );
}

#[test]
fn hovering_an_agent_marks_its_label() {
    let mut s = idle("/hov/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = "HOVERME".into();
    let id = s.agent_id;
    let scene = scene_with(vec![s], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    hover_agent(&mut r, id);
    r.render(&scene, pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{25b8}HOVERME") || text.contains("\u{25b8}"),
        "hovering an agent should add the ▸ marker to its label; frame:\n{text}"
    );
}

#[test]
fn click_hit_test_follows_a_walking_sprite() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/w/0.jsonl");
    let mut s = idle("/w/0.jsonl", 0, t0() - Duration::from_secs(300));
    let scene = scene_with(vec![s.clone()], 16);
    let mut r = build(192, 80, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let seat = pixtuoid_scene::sim::seated_top_left(
        layout.home_desks[0],
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(0)),
    );
    let (dx, dy) = (
        seat.x + pixtuoid_scene::layout::CHARACTER_SPRITE_W / 2,
        (seat.y + pixtuoid_scene::layout::CHARACTER_SPRITE_H / 2) / 2,
    );
    assert_eq!(r.hit_test_agent_at(dx, dy), Some(id));

    s.exiting_at = Some(t0());
    let scene = scene_with(vec![s], 16);
    // Mid-exit-walk, inside EXIT_GRACE_WINDOW — off the desk box, not yet GC'd.
    let walk_now = t0() + Duration::from_millis(1500);
    r.render(&scene, pack(), walk_now).unwrap();
    let drawn = drawn(&r, &scene, id, walk_now).top_left;
    assert_eq!(r.hit_test_agent_at(drawn.x, drawn.y / 2), Some(id));
    assert_eq!(
        r.hit_test_agent_at(dx, dy),
        None,
        "the sprite walked off its desk"
    );
}

/// Where the painter blits `id`'s sprite at `now`, sized by the pack's frame.
fn drawn(
    r: &TuiRenderer<TestBackend>,
    scene: &SceneState,
    id: AgentId,
    now: SystemTime,
) -> AgentFrame {
    let layout = r.cached_layout().expect("rendered layout");
    let stepped = pixtuoid_scene::floor::FloorSession::new()
        .step(
            pixtuoid_scene::floor::FloorInputs {
                scene,
                pack: pack(),
                now,
                floor: pixtuoid_scene::floor::FloorMeta::for_floor(0, 1),
                pets: pixtuoid_scene::floor::PetInputs::default(),
            },
            pixtuoid_scene::layout::Size {
                w: layout.buf_w,
                h: layout.buf_h,
            },
        )
        .expect("steppable floor");
    let frame = &stepped.frame;
    let c = frame
        .characters
        .iter()
        .find(|c| frame.agents[c.agent_idx].agent_id == id)
        .expect("the agent is drawn");
    let art = pack()
        .animation(c.anim_name)
        .and_then(|a| a.frames().get(c.frame_idx))
        .map(|f| (f.width(), f.height()))
        .expect("the pack draws the placement");
    AgentFrame {
        agent_id: id,
        top_left: c.top_left,
        w: art.0,
        h: art.1,
        label_anchor: c.label_anchor,
    }
}

/// Whether the half-block cell `(col, row)` shows a pixel of `sprite`.
fn cell_shows(sprite: AgentFrame, col: u16, row: u16) -> bool {
    crate::tui::geometry::CellArea::half_block(col, row).overlaps(
        sprite.top_left,
        sprite.w,
        sprite.h,
    )
}

/// Cells swept past each edge of the sprite, so the sweep sees its misses too.
const SWEEP_MARGIN: u16 = 2;

/// Probe offsets across `breath_offset_y`'s `CYCLE_MS` cycle, spaced under its
/// half, so one lands in the bobbed half whatever the agent's phase.
const BREATH_PROBES_MS: [u64; 10] = [
    0, 500, 1_000, 1_500, 2_000, 2_500, 3_000, 3_500, 4_000, 4_500,
];

#[test]
fn a_breathing_sitter_is_hit_at_its_drawn_cells_not_its_seat_top_left() {
    let (cols, rows) = (140, 48);
    let mut s = active("/breath/0.jsonl", 0, "Edit", t0() - Duration::from_secs(60));
    s.label = "BREATH".into();
    let id = s.agent_id;
    let scene = scene_with(vec![s], 16);
    let mut r = build(cols, rows, vec![]);
    r.render(&scene, pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout").clone();
    let seat = pixtuoid_scene::sim::seated_top_left(
        layout.home_desks[0],
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(0)),
    );
    let (now, drawn) = BREATH_PROBES_MS
        .into_iter()
        .map(|ms| t0() + Duration::from_millis(ms))
        .map(|now| (now, drawn(&r, &scene, id, now)))
        .find(|&(_, drawn)| drawn.top_left != seat)
        .expect("within one breath cycle the sitter bobs off its seat top-left");
    r.render(&scene, pack(), now).unwrap();
    let seated = AgentFrame {
        top_left: seat,
        ..drawn
    };

    let mut moved = None;
    for row in (seat.y / 2).saturating_sub(SWEEP_MARGIN)..=(seat.y + drawn.h) / 2 + SWEEP_MARGIN {
        for col in seat.x.saturating_sub(SWEEP_MARGIN)..seat.x + drawn.w + SWEEP_MARGIN {
            let shows = cell_shows(drawn, col, row);
            assert_eq!(
                r.hit_test_agent_at(col, row) == Some(id),
                shows,
                "cell ({col},{row}) against the sprite drawn at {drawn:?}"
            );
            if shows != cell_shows(seated, col, row) {
                moved = Some(((col, row), shows));
            }
        }
    }
    let ((col, row), shows) = moved.expect("the bob moves the sprite across a cell edge");
    r.set_mouse_pos(Some((col, row)));
    r.render(&scene, pack(), now).unwrap();
    assert_eq!(
        frame_text(r.frame_buffer()).contains('\u{25b8}'),
        shows,
        "hovering ({col},{row}) marks the label iff the cell shows the sprite"
    );
}

#[test]
fn overlapping_agents_hit_the_one_painted_on_top() {
    let (cols, rows) = (140, 48);
    // Two arrivals of one instant walk out of the elevator on one spot; a
    // second cwd dresses the second in another outfit, so the pixels tell
    // them apart.
    let mut a = active("/overlap/a.jsonl", 0, "Edit", t0());
    a.label = "ALPHA".into();
    let mut b = active("/overlap/b.jsonl", 1, "Edit", t0());
    b.label = "BRAVO".into();
    b.cwd = Arc::from(Path::new("/elsewhere"));
    let now = t0() + Duration::from_millis(400);
    let render = |agents: Vec<AgentSlot>| {
        let scene = scene_with(agents, 16);
        let mut r = build(cols, rows, vec![]);
        r.render(&scene, pack(), now).unwrap();
        (r, scene)
    };
    let (mut both, scene) = render(vec![a.clone(), b.clone()]);
    let (solo_a, _) = render(vec![a.clone()]);
    let (solo_b, _) = render(vec![b.clone()]);
    let px = |r: &TuiRenderer<TestBackend>, x, y| r.floor_buf(0).expect("floor buf").get(x, y);
    let (drawn_a, drawn_b) = (
        drawn(&both, &scene, a.agent_id, now),
        drawn(&both, &scene, b.agent_id, now),
    );
    // Where the two sprites cover each other the frame shows only the top one.
    let (x0, y0) = (
        drawn_a.top_left.x.max(drawn_b.top_left.x),
        drawn_a.top_left.y.max(drawn_b.top_left.y),
    );
    let (x1, y1) = (
        (drawn_a.top_left.x + drawn_a.w).min(drawn_b.top_left.x + drawn_b.w),
        (drawn_a.top_left.y + drawn_a.h).min(drawn_b.top_left.y + drawn_b.h),
    );
    let (x, y, top) = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .find_map(|(x, y)| {
            let (pa, pb) = (px(&solo_a, x, y), px(&solo_b, x, y));
            (pa != pb).then(|| {
                let shown = px(&both, x, y);
                let top = if shown == pa {
                    a.agent_id
                } else {
                    assert_eq!(shown, pb, "({x},{y}) shows neither agent");
                    b.agent_id
                };
                (x, y, top)
            })
        })
        .expect("the two arrivals overlap in differently dressed pixels");
    assert_ne!(
        top,
        a.agent_id.min(b.agent_id),
        "premise: the agent on top is not the first by AgentId"
    );
    assert_eq!(both.hit_test_agent_at(x, y / 2), Some(top));
    both.set_mouse_pos(Some((x, y / 2)));
    both.render(&scene, pack(), now).unwrap();
    let hovered = scene.agents[&top].label.clone();
    assert!(
        frame_text(both.frame_buffer()).contains(&format!("\u{25b8}{hovered}")),
        "hover marks the label of the agent on top"
    );
}

#[test]
fn the_drawn_geometry_answers_every_cell_as_the_half_block_does() {
    use crate::tui::geometry::CellArea;
    use crate::tui::hit_test::{
        hit_test_agent, hit_test_coffee_machine, hit_test_furniture, hit_test_pet,
        topmost_mascot_at,
    };
    let now = t0() + Duration::from_secs(20);
    let mut scene = scene_with(
        (0..6)
            .map(|i| active(&format!("/geo/{i}.jsonl"), i, "Edit", t0()))
            .collect(),
        16,
    );
    scene.insert_daemon(
        pixtuoid_core::source::openclaw::SOURCE_NAME,
        pixtuoid_core::state::DaemonInstanceId::new("18789").expect("non-empty"),
        pixtuoid_core::state::DaemonPresence {
            liveness: pixtuoid_core::state::DaemonLiveness::UP,
            active_sessions: 1,
            last_seen: now,
            entered_at: t0(),
            in_flight_runs: Default::default(),
            current_pid: Some(1),
        },
    );
    let cat = pixtuoid_scene::pet::Pet::defaulted(PetKind::Cat);
    for (cols, rows) in [(80, 30), (120, 52), (157, 41)] {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
        let mut floor = PerFloor::new();
        let mut chitchat = std::collections::HashMap::new();
        let mut ctx = DrawCtx::offscreen(
            &mut floor,
            &mut chitchat,
            normal_theme(),
            &scene,
            pack(),
            now,
            FloorMeta::ground(),
        );
        ctx.world.pets.pet = Some(&cat);
        let out = draw_scene(&mut term, &mut ctx).expect("draw");
        let (layout, geometry) = (
            out.layout.as_deref().expect("drawn"),
            out.geometry.expect("drawn"),
        );
        let hits = |at: CellArea| {
            (
                hit_test_agent(&out.agents, at),
                hit_test_coffee_machine(layout, at),
                out.pet_pos
                    .is_some_and(|p| hit_test_pet(p.kind, p.centre, p.anim, at)),
                topmost_mascot_at(&out.mascots, at).map(|m| m.pos),
                hit_test_furniture(layout, at),
            )
        };
        let mut seen = [false; 5];
        for (col, row) in (0..rows).flat_map(|row| (0..cols).map(move |col| (col, row))) {
            let old = hits(CellArea::half_block(col, row));
            assert_eq!(
                geometry.area_at(col, row).map(hits),
                Some(old),
                "{cols}x{rows} cell ({col},{row})"
            );
            let (agent, coffee, pet, mascot, furniture) = old;
            for (seen, hit) in seen.iter_mut().zip([
                agent.is_some(),
                coffee,
                pet,
                mascot.is_some(),
                furniture.is_some(),
            ]) {
                *seen |= hit;
            }
        }
        assert_eq!(
            seen, [true; 5],
            "{cols}x{rows}: every kind is hit somewhere"
        );
    }
}

/// TEMPORARY (3b S2c): the hover ladder answers every cell as the per-type
/// ladder it replaces, except where a later pet or mascot covers what that
/// ladder named first (D1–D3). Deleted with the per-type ladder.
#[test]
fn the_hover_ladder_answers_every_cell_as_the_per_type_ladder_did() {
    use crate::tui::geometry::CellArea;
    use crate::tui::hit_test::{
        SceneHit, hit_test_agent, hit_test_coffee_machine, hit_test_furniture, hit_test_pet,
        scene_hit, topmost_mascot_at,
    };
    use pixtuoid_scene::display::HoverTarget;
    #[derive(Debug, Clone, PartialEq)]
    enum Named {
        Agent(AgentId),
        Coffee,
        Pet,
        Mascot(Option<String>),
        Furniture(&'static str),
        Nothing,
    }
    let mut scene = scene_with(
        (0..6)
            .map(|i| active(&format!("/ab/{i}.jsonl"), i, "Edit", t0()))
            .collect(),
        16,
    );
    for port in ["18789", "18790"] {
        scene.insert_daemon(
            pixtuoid_core::source::openclaw::SOURCE_NAME,
            pixtuoid_core::state::DaemonInstanceId::new(port).expect("non-empty"),
            pixtuoid_core::state::DaemonPresence {
                liveness: pixtuoid_core::state::DaemonLiveness::UP,
                active_sessions: 1,
                last_seen: t0(),
                entered_at: t0(),
                in_flight_runs: Default::default(),
                current_pid: Some(1),
            },
        );
    }
    let cat = pixtuoid_scene::pet::Pet::defaulted(PetKind::Cat);
    let mut seen_kinds = [false; 5];
    let mut deltas = 0;
    for ms in [400, 2_000, 5_000, 20_000, 31_000, 47_000] {
        let now = t0() + Duration::from_millis(ms);
        for (cols, rows) in [(80, 30), (120, 52), (157, 41)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
            let mut floor = PerFloor::new();
            let mut chitchat = std::collections::HashMap::new();
            let mut ctx = DrawCtx::offscreen(
                &mut floor,
                &mut chitchat,
                normal_theme(),
                &scene,
                pack(),
                now,
                FloorMeta::ground(),
            );
            ctx.world.pets.pet = Some(&cat);
            let out = draw_scene(&mut term, &mut ctx).expect("draw");
            let (layout, geometry) = (
                out.layout.as_deref().expect("drawn"),
                out.geometry.expect("drawn"),
            );
            let old_pet = |at| {
                out.pet_pos
                    .is_some_and(|p| hit_test_pet(p.kind, p.centre, p.anim, at))
            };
            let old_mascot =
                |at| topmost_mascot_at(&out.mascots, at).map(|m| m.card.instance.clone());
            let old = |at: CellArea| {
                if let Some(id) = hit_test_agent(&out.agents, at) {
                    Named::Agent(id)
                } else if hit_test_coffee_machine(layout, at) {
                    Named::Coffee
                } else if old_pet(at) {
                    Named::Pet
                } else if let Some(m) = old_mascot(at) {
                    Named::Mascot(m)
                } else if let Some(label) = hit_test_furniture(layout, at) {
                    Named::Furniture(label)
                } else {
                    Named::Nothing
                }
            };
            let new = |at| match scene_hit(&out.hovers, layout, at) {
                Some(SceneHit::Figure(HoverTarget::Agent(id))) => Named::Agent(*id),
                Some(SceneHit::Figure(HoverTarget::Pet(_))) => Named::Pet,
                Some(SceneHit::Figure(HoverTarget::Mascot(k))) => {
                    Named::Mascot(Some(k.instance().as_str().to_string()))
                }
                Some(SceneHit::Coffee) => Named::Coffee,
                Some(SceneHit::Furniture(label)) => Named::Furniture(label),
                None => Named::Nothing,
            };
            for (col, row) in (0..rows).flat_map(|row| (0..cols).map(move |col| (col, row))) {
                let at = geometry.area_at(col, row).expect("inside the scene");
                let (was, is) = (old(at), new(at));
                let declared = match &is {
                    Named::Pet => old_pet(at),
                    Named::Mascot(m) => out.mascots.iter().any(|f| {
                        &f.card.instance == m
                            && crate::tui::hit_test::hit_test_mascot(f.pos, f.w, f.h, at)
                    }),
                    _ => false,
                };
                assert!(
                    was == is || declared,
                    "{ms}ms {cols}x{rows} cell ({col},{row}): was {was:?}, is {is:?}"
                );
                deltas += usize::from(was != is);
                for (seen, hit) in seen_kinds.iter_mut().zip([
                    matches!(was, Named::Agent(_)),
                    was == Named::Coffee,
                    was == Named::Pet,
                    matches!(was, Named::Mascot(_)),
                    matches!(was, Named::Furniture(_)),
                ]) {
                    *seen |= hit;
                }
            }
        }
    }
    assert_eq!(seen_kinds, [true; 5], "every kind is named somewhere");
    assert!(deltas > 0, "premise: a declared delta is exercised");
}
