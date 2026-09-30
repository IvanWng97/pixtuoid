use super::*;

#[test]
fn furniture_hit_test_resolves_against_rendered_layout() {
    let scene = scene_with(vec![idle("/hit/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
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
    r.render(&scene, &pack(), t0()).unwrap();
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
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, anim, kind } = r.cached_pet_pos().expect("pet placed");
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
    r.render(&scene, &pack(), t0()).unwrap();
    hover_agent(&mut r, id, 140, 48);
    r.render(&scene, &pack(), t0()).unwrap();
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
    r.render(&scene, &pack(), t0()).unwrap();
    let desk = r.cached_layout().expect("layout").home_desks[0];
    let (dx, dy) = (desk.x + 2, desk.y.saturating_sub(4) / 2 + 1);
    assert_eq!(r.hit_test_agent_at(dx, dy), Some(id));

    s.exiting_at = Some(t0());
    let scene = scene_with(vec![s], 16);
    // Mid-exit-walk, inside EXIT_GRACE_WINDOW — off the desk box, not yet GC'd.
    let walk_now = t0() + Duration::from_millis(1500);
    r.render(&scene, &pack(), walk_now).unwrap();
    let drawn = drawn_anchor(&r, &scene, id, walk_now);
    assert_eq!(r.hit_test_agent_at(drawn.x, drawn.y / 2), Some(id));
    assert_eq!(
        r.hit_test_agent_at(dx, dy),
        None,
        "the sprite walked off its desk"
    );
}

/// The sim's placement of `id` at `now` — where the painter blits its sprite.
fn drawn_anchor(
    r: &TuiRenderer<TestBackend>,
    scene: &SceneState,
    id: AgentId,
    now: SystemTime,
) -> pixtuoid_scene::layout::Point {
    let layout = r.cached_layout().expect("rendered layout");
    let observed = pixtuoid_scene::floor::FloorSession::new()
        .observe(
            pixtuoid_scene::floor::FloorInputs {
                scene,
                pack: &pack(),
                now,
                floor: pixtuoid_scene::floor::FloorMeta::for_floor(0, 1),
                pets: pixtuoid_scene::floor::PetInputs::default(),
            },
            pixtuoid_scene::layout::Size {
                w: layout.buf_w,
                h: layout.buf_h,
            },
        )
        .expect("observable floor");
    let frame = &observed.frame;
    frame
        .characters
        .iter()
        .find(|c| frame.agents[c.agent_idx].agent_id == id)
        .expect("the agent is drawn")
        .anchor
}

/// Whether the half-block cell `(col, row)` shows a pixel of the default-size
/// sprite whose top-left is `tl`.
fn cell_shows(tl: pixtuoid_scene::layout::Point, col: u16, row: u16) -> bool {
    let (w, h) = (
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        pixtuoid_scene::layout::CHARACTER_SPRITE_H,
    );
    (tl.x..tl.x + w).contains(&col) && 2 * row + 1 >= tl.y && 2 * row < tl.y + h
}

#[test]
fn a_breathing_sitter_is_hit_at_its_drawn_cells_not_its_seat_anchor() {
    let (cols, rows) = (140, 48);
    let mut s = active("/breath/0.jsonl", 0, "Edit", t0() - Duration::from_secs(60));
    s.label = "BREATH".into();
    let id = s.agent_id;
    let scene = scene_with(vec![s], 16);
    let mut r = build(cols, rows, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout").clone();
    let seat = pixtuoid_scene::pixel_painter::seated_anchor_facing(
        layout.home_desks[0],
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(0)),
    );
    let (now, drawn) = (0..10u64)
        .map(|k| t0() + Duration::from_millis(500 * k))
        .map(|now| (now, drawn_anchor(&r, &scene, id, now)))
        .find(|&(_, drawn)| drawn != seat)
        .expect("within one breath cycle the sitter bobs off its seat anchor");
    r.render(&scene, &pack(), now).unwrap();

    let mut moved = None;
    for row in (seat.y / 2).saturating_sub(2)..=(seat.y + 14) / 2 {
        for col in seat.x.saturating_sub(2)..seat.x + 10 {
            let shows = cell_shows(drawn, col, row);
            assert_eq!(
                r.hit_test_agent_at(col, row) == Some(id),
                shows,
                "cell ({col},{row}) against the sprite drawn at {drawn:?}"
            );
            if shows != cell_shows(seat, col, row) {
                moved = Some(((col, row), shows));
            }
        }
    }
    let ((col, row), shows) = moved.expect("the bob moves the sprite across a cell edge");
    r.set_mouse_pos(Some((col, row)));
    r.render(&scene, &pack(), now).unwrap();
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
        r.render(&scene, &pack(), now).unwrap();
        (r, scene)
    };
    let (mut both, scene) = render(vec![a.clone(), b.clone()]);
    let (solo_a, _) = render(vec![a.clone()]);
    let (solo_b, _) = render(vec![b.clone()]);
    let px = |r: &TuiRenderer<TestBackend>, x, y| r.floor_buf(0).expect("floor buf").get(x, y);
    let (drawn_a, drawn_b) = (
        drawn_anchor(&both, &scene, a.agent_id, now),
        drawn_anchor(&both, &scene, b.agent_id, now),
    );
    let (w, h) = (
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        pixtuoid_scene::layout::CHARACTER_SPRITE_H,
    );
    // Where the two sprites cover each other the frame shows only the top one.
    let (x0, y0) = (drawn_a.x.max(drawn_b.x), drawn_a.y.max(drawn_b.y));
    let (x1, y1) = (
        (drawn_a.x + w).min(drawn_b.x + w),
        (drawn_a.y + h).min(drawn_b.y + h),
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
    both.render(&scene, &pack(), now).unwrap();
    let hovered = scene.agents[&top].label.clone();
    assert!(
        frame_text(both.frame_buffer()).contains(&format!("\u{25b8}{hovered}")),
        "hover marks the label of the agent on top"
    );
}
