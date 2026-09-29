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
fn furniture_hit_test_names_every_fixture_not_painted_over() {
    use crate::tui::hit_test::hit_test_furniture;
    use pixtuoid_scene::layout::{Bounds, Layout, TEST_DEFAULT_DESKS};
    use std::collections::HashSet;

    let within = |inner: Bounds, outer: Bounds| {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.width <= outer.x + outer.width
            && inner.y + inner.height <= outer.y + outer.height
    };
    // Seeds 0 and 3 between them place every kind the roster has at this size.
    for seed in [0u64, 3] {
        let layout = Layout::compute_with_seed(160, 200, Some(TEST_DEFAULT_DESKS), seed)
            .unwrap_or_else(|| panic!("layout for seed {seed}"));
        let mut labels = HashSet::new();
        for cy in 0..(layout.buf_h / 2) {
            for cx in 0..layout.buf_w {
                labels.extend(hit_test_furniture(
                    &layout,
                    crate::tui::geometry::CellArea::half_block(cx, cy),
                ));
            }
        }
        let fixtures = layout.fixtures();
        for (i, f) in fixtures.iter().enumerate() {
            let painted_over = fixtures
                .iter()
                .enumerate()
                .any(|(j, g)| (g.depth, j) > (f.depth, i) && within(f.visual, g.visual));
            assert!(
                painted_over || labels.contains(f.kind.name()),
                "seed {seed}: {:?} never resolves",
                f.kind
            );
        }
    }
}

#[test]
fn hovering_an_agent_marks_its_label() {
    let mut s = idle("/hov/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = "HOVERME".into();
    let id = s.agent_id;
    let scene = scene_with(vec![s], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    hover_agent(&mut r, &scene, id, 140, 48);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{25b8}HOVERME") || text.contains("\u{25b8}"),
        "hovering an agent should add the ▸ marker to its label; frame:\n{text}"
    );
}

#[test]
fn click_hit_test_follows_a_walking_sprite_where_from_tui_misses_it() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/w/0.jsonl");
    let mut s = idle("/w/0.jsonl", 0, t0() - Duration::from_secs(300));
    let scene = scene_with(vec![s.clone()], 16);
    let mut r = build(192, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let desk = r.cached_layout().expect("layout").home_desks[0];
    let (dx, dy) = (desk.x + 2, desk.y.saturating_sub(4) / 2 + 1);
    assert_eq!(r.hit_test_agent_at(&scene, t0(), dx, dy), Some(id));
    let layout = r.cached_layout().unwrap();
    assert_eq!(
        crate::tui::hit_test::hit_test_from_tui(
            &scene,
            layout,
            crate::tui::geometry::CellArea::half_block(dx, dy)
        ),
        Some(id)
    );

    s.exiting_at = Some(t0());
    let scene = scene_with(vec![s], 16);
    // Mid-exit-walk, inside EXIT_GRACE_WINDOW — off the desk box, not yet GC'd.
    let walk_now = t0() + Duration::from_millis(1500);
    r.render(&scene, &pack(), walk_now).unwrap();

    let mut live = None;
    'scan: for my in 0..80u16 {
        for mx in 0..192u16 {
            if r.hit_test_agent_at(&scene, walk_now, mx, my) == Some(id) {
                live = Some((mx, my));
                break 'scan;
            }
        }
    }
    let (lx, ly) = live.expect("hit_test_agent_at must find the walking sprite");
    assert_ne!(
        (lx, ly),
        (dx, dy),
        "the sprite moved off its desk during the exit walk"
    );
    let layout = r.cached_layout().unwrap();
    assert_eq!(
        crate::tui::hit_test::hit_test_from_tui(
            &scene,
            layout,
            crate::tui::geometry::CellArea::half_block(lx, ly)
        ),
        None,
        "hit_test_from_tui (home-desk-only) misses the walked-off sprite — the FIND-22 gap"
    );
}
