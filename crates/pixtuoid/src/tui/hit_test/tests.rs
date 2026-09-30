use super::*;

#[test]
fn coffee_machine_hit_test_returns_false_for_origin() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    assert!(!hit_test_coffee_machine(
        &layout,
        crate::tui::geometry::CellArea::half_block(0, 0)
    ));
}

/// The middle cell of the coffee machine on `layout`'s pantry counter.
fn coffee_mid_cell(layout: &Layout) -> (u16, u16) {
    let b = layout.coffee_machine().expect("a coffee machine");
    (b.x + b.width / 2, (b.y + b.height / 2) / 2)
}

#[test]
fn coffee_machine_hit_test_returns_true_for_machine_area() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (mid_x, mid_cell_y) = coffee_mid_cell(&layout);
    assert!(
        hit_test_coffee_machine(
            &layout,
            crate::tui::geometry::CellArea::half_block(mid_x, mid_cell_y)
        ),
        "expected hit at coffee machine area ({mid_x}, {mid_cell_y})"
    );
}

#[test]
fn furniture_hit_test_returns_none_for_empty_space() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    // Scan for an empty cell rather than hardcoding one: which mid-floor cells
    // are open shifts whenever the pod aisle spacing is retuned.
    let empty = (0..(layout.buf_h / 2))
        .flat_map(|cy| (0..layout.buf_w).map(move |cx| (cx, cy)))
        .find(|&(cx, cy)| {
            hit_test_furniture(&layout, crate::tui::geometry::CellArea::half_block(cx, cy))
                .is_none()
        })
        .expect("some open-floor cell must report no furniture");
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(empty.0, empty.1)
        ),
        None
    );
}

#[test]
fn furniture_hit_test_finds_desk() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let desk = layout.home_desks.first().expect("desk");
    let cell_y = (desk.y + 2) / 2;
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(desk.x + 2, cell_y)
        ),
        Some("Desk")
    );
    let vis_w = pixtuoid_scene::layout::furniture_def(pixtuoid_scene::layout::Furniture::Desk)
        .visual
        .w;
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(desk.x + vis_w - 1, cell_y)
        ),
        Some("Desk"),
        "the desk's east overhang column must hover it"
    );
}

#[test]
fn furniture_hit_test_finds_elevator() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let door = layout.door;
    let cell_y = (door.y + pixtuoid_scene::layout::ELEVATOR_H / 2) / 2;
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(
                door.x + pixtuoid_scene::layout::ELEVATOR_W / 2,
                cell_y
            )
        ),
        Some("Elevator")
    );
}

#[test]
fn dense_room_1_has_coat_rack_and_doormat() {
    // Regression (#555): the meeting-decor painters and hover labels must iterate
    // ALL meeting_rooms, not just room 0.
    let mut saw_dual = false;
    for seed in 0..10u64 {
        let layout = Layout::compute_with_seed(192, 160, Some(8), seed).expect("layout");
        if layout.meeting_rooms.len() < 2 {
            continue;
        }
        saw_dual = true;
        let mr = layout.meeting_rooms[1].bounds;
        assert!(mr.width > 20, "seed {seed}: dense room 1 hosts the rack");
        let cx = mr.x + mr.width - 5;
        let cy = mr.y + mr.height / 2 - 4;
        assert_eq!(
            hit_test_furniture(
                &layout,
                crate::tui::geometry::CellArea::half_block(cx, (cy + 3) / 2)
            ),
            Some("Coat Rack"),
            "seed {seed}: room 1 must hover its own coat rack"
        );
        let mat_x = mr.x + mr.width + 1;
        let mat_y = mr.y + mr.height / 2 - 2;
        assert_eq!(
            hit_test_furniture(
                &layout,
                crate::tui::geometry::CellArea::half_block(mat_x + 1, (mat_y + 2) / 2)
            ),
            Some("Doormat"),
            "seed {seed}: room 1 must hover its own doormat"
        );
    }
    assert!(saw_dual, "192x160 seeds 0..10 must reach a dual floor");
}

#[test]
fn furniture_hit_test_finds_meeting_table() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let table = layout.meeting_rooms[0].trio.expect("trio").table;
    let cell_y = table.y / 2;
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(table.x, cell_y)
        ),
        Some("Meeting Table")
    );
}

#[test]
fn furniture_hit_test_respects_floor_seed() {
    // seed=1 → Lounge variant (no meeting room)
    let layout1 = Layout::compute_with_seed(160, 200, Some(4), 1).expect("layout");
    assert!(layout1.meeting_rooms.is_empty());
    let layout0 = Layout::compute(160, 200, Some(4)).expect("layout");
    if let Some(trio) = layout0.meeting_rooms.first().and_then(|r| r.trio) {
        let table = trio.table;
        let cell_y = table.y / 2;
        assert_ne!(
            hit_test_furniture(
                &layout1,
                crate::tui::geometry::CellArea::half_block(table.x, cell_y)
            ),
            Some("Meeting Table"),
        );
    }
}

#[test]
fn cat_hit_test_inside_sit_sprite() {
    use pixtuoid_scene::layout::Point;
    // cat_sit's `PetKind::hitbox` centred at (50,80) spans x[47..53), y[77..83):
    // cell 39 shows rows 78–79, and cell 38 shows row 77 in its lower half.
    let pos = Point { x: 50, y: 80 };
    for row in [38, 39] {
        assert!(
            hit_test_pet(
                PetKind::Cat,
                pos,
                "cat_sit",
                crate::tui::geometry::CellArea::half_block(50, row)
            ),
            "cell row {row}"
        );
    }
}

#[test]
fn cat_hit_test_outside_returns_false() {
    use pixtuoid_scene::layout::Point;
    let pos = Point { x: 50, y: 80 };
    assert!(!hit_test_pet(
        PetKind::Cat,
        pos,
        "cat_sit",
        crate::tui::geometry::CellArea::half_block(10, 10)
    ));
}

#[test]
fn mascot_hit_test_inside_and_outside() {
    use pixtuoid_scene::layout::Point;
    // The 14x12 sprite centred at (50,80) spans x[43..57), y[74..86); cell 39
    // shows rows 78–79.
    let pos = Point { x: 50, y: 80 };
    assert!(hit_test_mascot(
        pos,
        14,
        12,
        crate::tui::geometry::CellArea::half_block(50, 39)
    ));
    assert!(!hit_test_mascot(
        pos,
        14,
        12,
        crate::tui::geometry::CellArea::half_block(10, 10)
    ));
}

fn scene_with_agent_at_desk(desk_index: usize) -> (SceneState, AgentId) {
    use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex};
    use std::path::Path;
    use std::sync::Arc;
    let id = AgentId::from_transcript_path("/pin/0.jsonl");
    let slot = AgentSlot {
        agent_id: id,
        source: Arc::from("cc"),
        session_id: Arc::from("s"),
        cwd: Arc::from(Path::new("/repo")),
        label: "a".into(),
        state: ActivityState::Idle,
        state_started_at: SystemTime::UNIX_EPOCH,
        created_at: SystemTime::UNIX_EPOCH,
        last_event_at: SystemTime::UNIX_EPOCH,
        exiting_at: None,
        pending_idle_at: None,
        desk_index: GlobalDeskIndex(desk_index),
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
    let mut scene = SceneState::uniform(16);
    scene.agents.insert(id, slot);
    (scene, id)
}

// Hover (`hit_test_agent`) and the harness's seated locator (`hit_test_from_tui`)
// must hit EXACTLY the cells that show the sprite `character_anchor` places.
// 160x200 seats its sitters on even rows; 120x90 on odd ones, where the
// sprite's last cell shows it only in its upper half.
#[test]
fn an_agent_is_hit_from_exactly_the_cells_that_show_it() {
    let even = seated_anchor_hits_its_covering_cells(160, 200, Some(4));
    assert_eq!(even.y % 2, 0, "160x200 must keep an even seated anchor");
    let odd = seated_anchor_hits_its_covering_cells(120, 90, None);
    assert_eq!(odd.y % 2, 1, "120x90 must keep an odd seated anchor");
}

/// Assert both agent hit tests hit every cell that shows desk 0's seated
/// sprite at `w`×`h`, and none beside it; returns the anchor.
fn seated_anchor_hits_its_covering_cells(
    w: u16,
    h: u16,
    max_desks: Option<usize>,
) -> pixtuoid_scene::layout::Point {
    let layout = Layout::compute(w, h, max_desks).expect("layout");
    let (mut scene, id) = scene_with_agent_at_desk(0);
    // A recent last_event_at keeps the wander machine in its Seated phase;
    // the pose derives as seated either way for an Idle agent at bootstrap.
    let now = SystemTime::now();
    scene.agents.get_mut(&id).expect("slot").last_event_at = now;

    let mut router = pixtuoid_scene::pathfind::AStarRouter::new();
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = pose::PoseHistory::default();
    let mut motion = std::collections::HashMap::new();
    let mut rctx = pose::RouteCtx {
        router: &mut router,
        overlay: &overlay,
        history: &mut history,
        motion: &mut motion,
    };
    let agent = scene.agents.get(&id).expect("slot");
    let anchor = character_anchor(agent, &layout, now, &mut rctx)
        .expect("a seated agent has a painted anchor");

    let (cols, rows) = covering_cells(anchor);
    let mut hits = |col, row| {
        let cell = crate::tui::geometry::CellArea::half_block(col, row);
        let hover = hit_test_agent(&scene, &layout, now, &mut rctx, cell);
        let pin = hit_test_from_tui(&scene, &layout, cell);
        assert_eq!(
            hover, pin,
            "hover and the locator disagree at ({col},{row})"
        );
        pin
    };
    for row in rows.clone() {
        for col in cols.clone() {
            assert_eq!(
                hits(col, row),
                Some(id),
                "{w}x{h}: cell ({col},{row}) shows the sprite"
            );
        }
    }
    let (row, col) = (*rows.start(), cols.start);
    assert_eq!(
        hits(cols.start.wrapping_sub(1), row),
        None,
        "west of the sprite"
    );
    assert_eq!(hits(cols.end, row), None, "east of the sprite");
    assert_eq!(
        hits(col, rows.start().wrapping_sub(1)),
        None,
        "north of the sprite"
    );
    assert_eq!(hits(col, rows.end() + 1), None, "south of the sprite");
    anchor
}

/// The terminal cells that show some pixel of a seated sprite whose top-left
/// is `tl`: its columns, and every half-block row from the one holding its top
/// pixel to the one holding its bottom pixel.
fn covering_cells(
    tl: pixtuoid_scene::layout::Point,
) -> (std::ops::Range<u16>, std::ops::RangeInclusive<u16>) {
    let (w, h) = (
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        pixtuoid_scene::layout::CHARACTER_SPRITE_H,
    );
    (tl.x..tl.x + w, tl.y / 2..=(tl.y + h - 1) / 2)
}

#[test]
fn from_tui_misses_empty_space() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (scene, _id) = scene_with_agent_at_desk(0);
    assert_eq!(
        hit_test_from_tui(
            &scene,
            &layout,
            crate::tui::geometry::CellArea::half_block(0, 0)
        ),
        None
    );
}

#[test]
fn from_tui_skips_agent_with_out_of_range_desk() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (scene, _id) = scene_with_agent_at_desk(layout.home_desks.len() + 100);
    for &(mx, my) in &[(0u16, 0u16), (40, 20), (80, 40)] {
        assert_eq!(
            hit_test_from_tui(
                &scene,
                &layout,
                crate::tui::geometry::CellArea::half_block(mx, my)
            ),
            None
        );
    }
}

// With the ARITHMETIC bridge (`scene.floor_local_desk`) an OOB desk equal to the
// uniform scene's cap wrapped onto a synthetic floor 1 and landed back at local
// 0 — hit-testable at desk 0 while the renderer skipped it. Hence `cap` below.
#[test]
fn from_tui_oob_desk_at_capacity_boundary_does_not_wrap_to_desk_zero() {
    use pixtuoid_core::state::GlobalDeskIndex;
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (mut scene, id) = scene_with_agent_at_desk(0);
    let cap = scene.floor_capacities[0];
    scene.agents.get_mut(&id).expect("slot").desk_index = GlobalDeskIndex(cap);
    let a = pixtuoid_scene::pixel_painter::seated_anchor_facing(
        layout.home_desks[0],
        pixtuoid_scene::layout::CHARACTER_SPRITE_W,
        layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(0)),
    );
    let (cols, rows) = covering_cells(a);
    for row in rows {
        for col in cols.clone() {
            assert_eq!(
                hit_test_from_tui(
                    &scene,
                    &layout,
                    crate::tui::geometry::CellArea::half_block(col, row)
                ),
                None,
                "an OOB desk at the capacity boundary must never hit-test"
            );
        }
    }
}

// BulletinBoard is never emitted by compute_with_seed and Ficus only appears on
// ROOMY-band floors, so both are placed synthetically below.

#[test]
fn furniture_hit_test_ficus_via_synthetic_plant() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let pos = Point { x: 40, y: 40 };
    layout.plants.push(pixtuoid_scene::layout::PlantItem {
        kind: pixtuoid_scene::layout::PlantKind::Ficus,
        pos,
    });
    // Plants are center-anchored on `pos`; hover the center cell.
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(pos.x, pos.y / 2)
        ),
        Some("Ficus")
    );
}

#[test]
fn furniture_hit_test_bulletin_board_via_synthetic_wall_decor() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    // Wall decor is TOP-LEFT anchored at `pos` (not centered). Place it in
    // open space so no earlier furniture arm shadows it.
    let pos = Point { x: 60, y: 30 };
    layout
        .wall_decor
        .push(pixtuoid_scene::layout::WallDecorItem {
            kind: pixtuoid_scene::layout::WallDecor::BulletinBoard,
            pos,
        });
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(pos.x, pos.y / 2)
        ),
        Some("Bulletin Board")
    );
}

#[test]
fn cat_hit_test_sleep_smaller_box() {
    use pixtuoid_scene::layout::Point;
    // cat_sleep's hitbox centred at (50,80) spans y[78..82): cell 41 shows rows
    // 82–83 (out), cell 40 rows 80–81 (in).
    let pos = Point { x: 50, y: 80 };
    assert!(!hit_test_pet(
        PetKind::Cat,
        pos,
        "cat_sleep",
        crate::tui::geometry::CellArea::half_block(50, 41)
    ));
    assert!(hit_test_pet(
        PetKind::Cat,
        pos,
        "cat_sleep",
        crate::tui::geometry::CellArea::half_block(50, 40)
    ));
}

// Probing coords that DO hit while the waypoint is present is what proves the
// false comes from the missing-pantry guard rather than an off-counter miss.
#[test]
fn coffee_machine_returns_false_when_no_pantry_waypoint() {
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (mid_x, mid_cell_y) = coffee_mid_cell(&layout);
    assert!(
        hit_test_coffee_machine(
            &layout,
            crate::tui::geometry::CellArea::half_block(mid_x, mid_cell_y)
        ),
        "precondition: coffee machine area should hit with the Pantry waypoint present"
    );
    layout
        .waypoints
        .retain(|w| !matches!(w.kind, pixtuoid_scene::layout::WaypointKind::Pantry));
    assert!(
        !hit_test_coffee_machine(
            &layout,
            crate::tui::geometry::CellArea::half_block(mid_x, mid_cell_y)
        ),
        "no Pantry waypoint ⇒ the early return must yield false at the machine coords"
    );
    assert!(!hit_test_coffee_machine(
        &layout,
        crate::tui::geometry::CellArea::half_block(0, 0)
    ));
}

// The lounge and pod-decor arms below aren't all reachable from
// `compute_with_seed` at the tested sizes, so each is placed synthetically.

/// `label` fires on exactly the cells that show some pixel — either half — of
/// a `size` sprite centred on `pos`, swept half a sprite beyond it on every
/// side.
fn assert_centered_hover_box(
    layout: &Layout,
    label: &str,
    pos: pixtuoid_scene::layout::Point,
    size: Size,
) {
    let (x0, y0) = (pos.x - size.w / 2, pos.y - size.h / 2);
    for mx in pos.x - size.w..pos.x + size.w {
        for my in (pos.y - size.h) / 2..(pos.y + size.h) / 2 {
            let py = my * 2;
            let inside = mx >= x0 && mx < x0 + size.w && py + 1 >= y0 && py < y0 + size.h;
            assert_eq!(
                hit_test_furniture(layout, crate::tui::geometry::CellArea::half_block(mx, my))
                    == Some(label),
                inside,
                "{label} at cell ({mx}, {my})"
            );
        }
    }
}

/// A layout whose lounge pieces sit where the caller puts them.
fn layout_with_lounge(lounge: pixtuoid_scene::layout::Lounge) -> Layout {
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    layout.lounge = Some(lounge);
    // The probes stand the pieces where the real office has its plants and its
    // meeting room's band decor: either would be the topmost fixture there.
    layout.plants.clear();
    layout.meeting_rooms.clear();
    layout
}

// The lounge is one aggregate, so the co-present pieces are parked far from
// every probe.
const PARK: pixtuoid_scene::layout::Point = pixtuoid_scene::layout::Point { x: 130, y: 6 };

#[test]
fn the_lounge_sofa_hovers_on_its_painted_sprite() {
    use pixtuoid_scene::layout::{Furniture, furniture_def};
    let c = Point { x: 40, y: 50 };
    let layout = layout_with_lounge(pixtuoid_scene::layout::Lounge {
        couch_center: c,
        floor_lamp: PARK,
        side_table: PARK,
        fish_tank: None,
    });
    assert_centered_hover_box(
        &layout,
        "Lounge Sofa",
        c,
        furniture_def(Furniture::MeetingSofaBody).visual,
    );
}

#[test]
fn the_side_table_hovers_on_its_painted_sprite() {
    use pixtuoid_scene::layout::{Furniture, furniture_def};
    let t = Point { x: 40, y: 50 };
    let layout = layout_with_lounge(pixtuoid_scene::layout::Lounge {
        couch_center: PARK,
        floor_lamp: PARK,
        side_table: t,
        fish_tank: None,
    });
    assert_centered_hover_box(
        &layout,
        "Side Table",
        t,
        furniture_def(Furniture::LoungeSideTable).visual,
    );
}

#[test]
fn furniture_hit_test_finds_floor_lamp_via_synthetic() {
    let p = Point { x: 40, y: 40 };
    let layout = layout_with_lounge(pixtuoid_scene::layout::Lounge {
        couch_center: PARK,
        floor_lamp: p,
        side_table: PARK,
        fish_tank: None,
    });
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(p.x, p.y / 2)
        ),
        Some("Floor Lamp")
    );
}

#[test]
fn furniture_hit_test_finds_fish_tank_via_synthetic() {
    let p = Point { x: 40, y: 40 };
    let layout = layout_with_lounge(pixtuoid_scene::layout::Lounge {
        couch_center: PARK,
        floor_lamp: PARK,
        side_table: PARK,
        fish_tank: Some(p),
    });
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(p.x, p.y / 2)
        ),
        Some("Fish Tank")
    );
}

#[test]
fn snack_shelf_hovers_across_its_whole_sprite_not_just_the_footprint() {
    // The shelf sprite is CENTRED on the waypoint while the walkable footprint
    // is its End-anchored south strip; hover must cover the sprite the user
    // sees, not that strip.
    let layout = Layout::compute(192, 160, Some(12)).expect("layout");
    let shelf = layout
        .waypoints
        .iter()
        .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::SnackShelf)
        .map(|w| w.pos)
        .expect("192x160 places the snack shelf");
    let vis =
        pixtuoid_scene::layout::furniture_def(pixtuoid_scene::layout::Furniture::SnackShelf).visual;
    let top_y = shelf.y.saturating_sub(vis.h / 2);
    assert_eq!(top_y % 2, 1, "192x160 must keep the shelf's top edge odd");
    // `top_y / 2` holds the shelf's top row; on an odd edge only its lower half
    // shows it.
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(shelf.x, top_y / 2)
        ),
        Some("Snack Shelf"),
        "top shelf row hovers"
    );
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(shelf.x, shelf.y / 2)
        ),
        Some("Snack Shelf"),
        "sprite centre hovers"
    );
}

#[test]
fn furniture_hit_test_finds_meeting_chairs_on_a_real_layout() {
    let layout = Layout::compute(192, 160, Some(12)).expect("layout");
    let chairs: Vec<_> = layout
        .waypoints
        .iter()
        .filter(|w| w.kind == pixtuoid_scene::layout::WaypointKind::MeetingChair)
        .map(|w| w.pos)
        .collect();
    assert_eq!(chairs.len(), 2);
    for c in chairs {
        assert_eq!(
            hit_test_furniture(
                &layout,
                crate::tui::geometry::CellArea::half_block(c.x, c.y / 2)
            ),
            Some("Meeting Chair")
        );
    }
}

#[test]
fn furniture_hit_test_finds_tv_stand_via_synthetic_pod_decor() {
    use pixtuoid_scene::layout::{PodDecor, PodDecorItem, Point};
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let p = Point { x: 50, y: 40 };
    layout.pod_decor.push(PodDecorItem {
        kind: PodDecor::Tv,
        pos: p,
    });
    assert_eq!(
        hit_test_furniture(
            &layout,
            crate::tui::geometry::CellArea::half_block(p.x, p.y / 2)
        ),
        Some("TV Stand")
    );
}

/// The wall board's text sits on the neon sign, so hovering the sign must not
/// raise a furniture tooltip over it.
#[test]
fn the_neon_sign_raises_no_tooltip() {
    use pixtuoid_scene::layout::{NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y};
    let layout = Layout::compute(160, 200, Some(16)).expect("layout");
    let cell =
        crate::tui::geometry::CellArea::half_block(NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y / 2);
    assert_eq!(
        layout.fixture_at(cell.bounds()),
        Some(pixtuoid_scene::layout::FixtureKind::NeonSign)
    );
    assert_eq!(hit_test_furniture(&layout, cell), None);
}
