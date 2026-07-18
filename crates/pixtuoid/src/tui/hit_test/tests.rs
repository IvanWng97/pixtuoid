use super::*;

#[test]
fn coffee_machine_hit_test_returns_false_for_origin() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    assert!(!hit_test_coffee_machine(&layout, 0, 0));
}

#[test]
fn coffee_machine_hit_test_returns_true_for_machine_area() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let pantry_wp = layout
        .waypoints
        .iter()
        .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::Pantry)
        .expect("pantry");
    let Size { w: cw, h: ch } = layout.pantry_counter_size();
    let sprite_x = pantry_wp.pos.x.saturating_sub(cw / 2);
    let sprite_y = pantry_wp.pos.y.saturating_sub(ch / 2);
    let mid_x = if cw >= 32 {
        sprite_x + 14
    } else {
        sprite_x + 10
    };
    let mid_cell_y = (sprite_y + ch / 2) / 2;
    assert!(
        hit_test_coffee_machine(&layout, mid_x, mid_cell_y),
        "expected hit at coffee machine area ({mid_x}, {mid_cell_y})"
    );
}

#[test]
fn furniture_hit_test_returns_none_for_empty_space() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    // Open floor must report no furniture. Scan for an empty cell rather
    // than hardcoding one — which mid-floor cells are open shifts when the
    // pod aisle spacing is retuned (a hardcoded point goes stale and lands
    // on a reflowed desk). If hit_test_furniture wrongly matched
    // everywhere, no empty cell would be found and `.expect` would panic.
    let empty = (0..(layout.buf_h / 2))
        .flat_map(|cy| (0..layout.buf_w).map(move |cx| (cx, cy)))
        .find(|&(cx, cy)| hit_test_furniture(&layout, cx, cy).is_none())
        .expect("some open-floor cell must report no furniture");
    assert_eq!(hit_test_furniture(&layout, empty.0, empty.1), None);
}

#[test]
fn furniture_hit_test_finds_desk() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let desk = layout.home_desks.first().expect("desk");
    let cell_y = (desk.y + 2) / 2;
    assert_eq!(
        hit_test_furniture(&layout, desk.x + 2, cell_y),
        Some("Desk")
    );
    // The east overhang column the OLD DESK_W+2 box CLIPPED — derive from the
    // table so the test can't re-hardcode the width (desk.x + visual.w - 1).
    let vis_w = pixtuoid_scene::layout::furniture_def(pixtuoid_scene::layout::Furniture::Desk)
        .visual
        .w;
    assert_eq!(
        hit_test_furniture(&layout, desk.x + vis_w - 1, cell_y),
        Some("Desk"),
        "the desk's east overhang column must hover it (old DESK_W+2 box clipped it)"
    );
}

#[test]
fn furniture_hit_test_finds_elevator() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let door = layout.door.expect("door");
    let cell_y = (door.y + 7) / 2;
    assert_eq!(
        hit_test_furniture(&layout, door.x + 8, cell_y),
        Some("Elevator")
    );
}

#[test]
fn dense_room_1_has_coat_rack_and_doormat() {
    // #555's second half: the meeting-decor painters + hover labels
    // iterate ALL meeting_rooms — a dense floor's second room used to
    // render sofas + table but NO coat rack / doormat / notice board
    // (everything keyed room 0).
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
            hit_test_furniture(&layout, cx, (cy + 3) / 2),
            Some("Coat Rack"),
            "seed {seed}: room 1 must hover its own coat rack"
        );
        let mat_x = mr.x + mr.width + 1;
        let mat_y = mr.y + mr.height / 2 - 2;
        assert_eq!(
            hit_test_furniture(&layout, mat_x + 1, (mat_y + 2) / 2),
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
        hit_test_furniture(&layout, table.x, cell_y),
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
            hit_test_furniture(&layout1, table.x, cell_y),
            Some("Meeting Table"),
        );
    }
}

#[test]
fn cat_hit_test_inside_sit_sprite() {
    use pixtuoid_scene::layout::Point;
    // cat_sit is 6x6. Center at (50, 80).
    // Top-left pixel: (50-3, 80-3) = (47, 77).
    // cell_y for my=39 → 78, which is inside [77..83).
    // mx=50 inside [47..53).
    let pos = Point { x: 50, y: 80 };
    assert!(hit_test_pet(PetKind::Cat, pos, "cat_sit", 50, 39));
}

#[test]
fn cat_hit_test_outside_returns_false() {
    use pixtuoid_scene::layout::Point;
    let pos = Point { x: 50, y: 80 };
    // Way outside the 6x6 sprite.
    assert!(!hit_test_pet(PetKind::Cat, pos, "cat_sit", 10, 10));
}

#[test]
fn mascot_hit_test_inside_and_outside() {
    use pixtuoid_scene::layout::Point;
    // 14x12 sprite centered at (50, 80) → top-left pixel (43, 74).
    let pos = Point { x: 50, y: 80 };
    // cell my=39 → pixel 78 ∈ [74..86); mx=50 ∈ [43..57).
    assert!(hit_test_mascot(pos, 50, 39));
    // Far away.
    assert!(!hit_test_mascot(pos, 10, 10));
}

// --- hit_test_from_tui (click-to-pin, home-desk-only) -----------------

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

#[test]
fn from_tui_hits_agent_at_its_desk_anchor() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (scene, id) = scene_with_agent_at_desk(0);
    let d = layout.home_desks[0];
    // Computed FROM the painter's seated-anchor geometry (DESK_W-centered
    // 8px sprite, 8px above the desk) — NOT a mirror of the impl's own
    // literals, so a drift from the painted sprite reddens here.
    let cx = d.x
        + pixtuoid_scene::layout::DESK_W.saturating_sub(pixtuoid_scene::layout::CHARACTER_SPRITE_W)
            / 2;
    let cy = d.y.saturating_sub(8) / 2;
    assert_eq!(hit_test_from_tui(&scene, &layout, cx, cy), Some(id));
}

// The drift-pair guard: the click-to-pin box must cover EXACTLY the cells the
// painter blits the seated sprite into. The oracle is `character_anchor` —
// the SAME anchor the hover tooltip (hit_test_agent) and the sprite blit use
// — so hover and click can't disagree on the same cells (the PANEL_PAD
// pairing class: derive both sides from one geometry, pin with a test).
#[test]
fn from_tui_pin_box_matches_the_painted_seated_anchor() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
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

    let (ax, ay) = (anchor.x, anchor.y / 2);
    // Every cell of the painted 8x6 sprite box pins…
    for dx in 0..pixtuoid_scene::layout::CHARACTER_SPRITE_W {
        for dy in 0..pixtuoid_scene::layout::CHARACTER_SPRITE_H_CELLS {
            assert_eq!(
                hit_test_from_tui(&scene, &layout, ax + dx, ay + dy),
                Some(id),
                "painted sprite cell ({dx},{dy}) must be pinnable"
            );
        }
    }
    // …and the cells just outside it do not (no phantom pin).
    assert_eq!(
        hit_test_from_tui(&scene, &layout, ax.wrapping_sub(1), ay),
        None
    );
    assert_eq!(
        hit_test_from_tui(
            &scene,
            &layout,
            ax + pixtuoid_scene::layout::CHARACTER_SPRITE_W,
            ay
        ),
        None
    );
    assert_eq!(
        hit_test_from_tui(&scene, &layout, ax, ay.wrapping_sub(1)),
        None
    );
    assert_eq!(
        hit_test_from_tui(
            &scene,
            &layout,
            ax,
            ay + pixtuoid_scene::layout::CHARACTER_SPRITE_H_CELLS
        ),
        None
    );
}

#[test]
fn from_tui_misses_empty_space() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (scene, _id) = scene_with_agent_at_desk(0);
    assert_eq!(hit_test_from_tui(&scene, &layout, 0, 0), None);
}

#[test]
fn from_tui_skips_agent_with_out_of_range_desk() {
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    // desk_index past the layout's home-desk count ⇒ `continue` arm.
    let (scene, _id) = scene_with_agent_at_desk(layout.home_desks.len() + 100);
    // No agent occupies any cell — scan a few and confirm None everywhere.
    for &(mx, my) in &[(0u16, 0u16), (40, 20), (80, 40)] {
        assert_eq!(hit_test_from_tui(&scene, &layout, mx, my), None);
    }
}

// Regression for the bridge-choice bug: with the ARITHMETIC bridge
// (`scene.floor_local_desk`), an OOB desk equal to the uniform scene's cap
// wraps onto a synthetic floor 1 and lands back at local 0 — hit-testable
// at desk 0 while the renderer skips it. The identity cast must keep it
// OOB everywhere.
#[test]
fn from_tui_oob_desk_at_capacity_boundary_does_not_wrap_to_desk_zero() {
    use pixtuoid_core::state::GlobalDeskIndex;
    let layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let (mut scene, id) = scene_with_agent_at_desk(0);
    let cap = scene.floor_capacities[0];
    // Re-seat the agent at exactly `cap` — the wrap-prone value.
    scene.agents.get_mut(&id).expect("slot").desk_index = GlobalDeskIndex(cap);
    // Scan desk 0's whole sprite box — the wrapped bridge would hit here.
    let desk0 = layout.home_desks[0];
    let (ax, ay) = (
        desk0.x
            + pixtuoid_scene::layout::DESK_W
                .saturating_sub(pixtuoid_scene::layout::CHARACTER_SPRITE_W)
                / 2,
        desk0.y.saturating_sub(8) / 2,
    );
    for dx in 0..pixtuoid_scene::layout::CHARACTER_SPRITE_W {
        for dy in 0..pixtuoid_scene::layout::CHARACTER_SPRITE_H_CELLS {
            assert_eq!(
                hit_test_from_tui(&scene, &layout, ax + dx, ay + dy),
                None,
                "an OOB desk at the capacity boundary must never hit-test"
            );
        }
    }
}

// --- hit_test_furniture: arms the real-layout loop may not reach --------
// WallDecor::BulletinBoard is never emitted by compute_with_seed, and the
// Ficus only appears on ROOMY-band floors — push them into the pub Vecs of
// a computed layout and hit their centers, size-independent.

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
    assert_eq!(hit_test_furniture(&layout, pos.x, pos.y / 2), Some("Ficus"));
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
        hit_test_furniture(&layout, pos.x, pos.y / 2),
        Some("Bulletin Board")
    );
}

#[test]
fn cat_hit_test_sleep_smaller_box() {
    use pixtuoid_scene::layout::Point;
    // cat_sleep is 6x4. Center at (50, 80).
    // Top-left: (47, 78). Bottom-right: (53, 82).
    let pos = Point { x: 50, y: 80 };
    // cell_y for my=41 → 82, which is at the boundary (82 >= 82 is false for < check).
    // Actually wait: tl_y = 80 - 2 = 78, h=4 so range is [78..82). cell_y=82 is OUT.
    assert!(!hit_test_pet(PetKind::Cat, pos, "cat_sleep", 50, 41));
    // cell_y for my=40 → 80, inside [78..82).
    assert!(hit_test_pet(PetKind::Cat, pos, "cat_sleep", 50, 40));
}

// --- hit_test_coffee_machine: the missing-pantry guard + small-counter box --

// The Pantry-waypoint early-return: with the Pantry waypoint removed,
// `hit_test_coffee_machine` must be false EVERYWHERE — and specifically at
// the coords that DO hit while the waypoint is present. That second probe
// proves the false comes from the missing-pantry guard, not an off-counter
// miss (it would pass even if the early return were deleted, were it any
// other coordinate).
#[test]
fn coffee_machine_returns_false_when_no_pantry_waypoint() {
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let wp = *layout
        .waypoints
        .iter()
        .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::Pantry)
        .expect("pantry");
    // Mirror the existing true-test geometry to land squarely on the machine.
    let Size { w: cw, h: ch } = layout.pantry_counter_size();
    let sprite_x = wp.pos.x.saturating_sub(cw / 2);
    let sprite_y = wp.pos.y.saturating_sub(ch / 2);
    let mid_x = if cw >= 32 {
        sprite_x + 14
    } else {
        sprite_x + 10
    };
    let mid_cell_y = (sprite_y + ch / 2) / 2;
    // Sanity: the chosen coords ARE a hit while the waypoint is present.
    assert!(
        hit_test_coffee_machine(&layout, mid_x, mid_cell_y),
        "precondition: coffee machine area should hit with the Pantry waypoint present"
    );
    // Drop the Pantry waypoint → the early return must make EVERY probe false.
    layout
        .waypoints
        .retain(|w| !matches!(w.kind, pixtuoid_scene::layout::WaypointKind::Pantry));
    assert!(
        !hit_test_coffee_machine(&layout, mid_x, mid_cell_y),
        "no Pantry waypoint ⇒ the early return must yield false at the machine coords"
    );
    assert!(!hit_test_coffee_machine(&layout, 0, 0));
}

// The small-counter box is derived from the shared `PANTRY_COFFEE_COLS_SMALL`
// = [9,12). Pin the box endpoints to the const (col below/above the machine
// must miss; the machine edges must hit) so the click target can't drift from
// the painter — and keep the x+15 falsifier for the cw>=32 split (x+15 is
// outside the small box but inside the large [11,18), so a hit there means the
// split was dropped). The old [8,13) box false-positived counter cols 8 and 12.
#[test]
fn coffee_machine_small_counter_uses_the_shared_coffee_cols() {
    let (lo, hi) = pixtuoid_scene::pixel_painter::PANTRY_COFFEE_COLS_SMALL;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let wp = *layout
        .waypoints
        .iter()
        .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::Pantry)
        .expect("pantry");
    let h = layout.pantry_counter_size().h;
    layout.pantry.as_mut().expect("pantry").counter_size = Size { w: 20, h };
    let sprite_x = wp.pos.x.saturating_sub(20 / 2);
    let sprite_y = wp.pos.y.saturating_sub(h / 2);
    let cell_y = (sprite_y + h / 2) / 2;
    // The machine edges (cols lo..hi-1) hit; the counter cols just outside
    // (lo-1, hi) miss — pinning the box to the const, with teeth against the
    // old wider [8,13) box (which hit at lo-1 and hi).
    assert!(
        !hit_test_coffee_machine(&layout, sprite_x + lo - 1, cell_y),
        "the counter col just left of the machine must miss"
    );
    assert!(
        hit_test_coffee_machine(&layout, sprite_x + lo, cell_y),
        "the machine's left edge must hit"
    );
    assert!(
        hit_test_coffee_machine(&layout, sprite_x + hi - 1, cell_y),
        "the machine's right edge must hit"
    );
    assert!(
        !hit_test_coffee_machine(&layout, sprite_x + hi, cell_y),
        "the counter col just right of the machine must miss"
    );
    assert!(
        !hit_test_coffee_machine(&layout, sprite_x + 15, cell_y),
        "x+15 is outside the small box; a hit means the cw>=32 split was dropped"
    );
}

// --- hit_test_furniture: Option/Vec arms not produced at harness sizes -----
// couch_sprite_center, floor_lamp, the PodDecor::Tv label arm, and
// lounge_side_table aren't all reachable from `compute_with_seed` at the
// tested sizes, so place each synthetically in open floor (probed None first)
// and hit its center; the +offset misses pin each box's literal width.

#[test]
fn furniture_hit_test_finds_lounge_sofa_via_synthetic_center() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let c = Point { x: 40, y: 50 };
    layout.couch_sprite_center = Some(c);
    assert_eq!(
        hit_test_furniture(&layout, c.x, c.y / 2),
        Some("Lounge Sofa")
    );
    // 30px right of center is outside the 20-wide hover box.
    assert_ne!(
        hit_test_furniture(&layout, c.x + 30, c.y / 2),
        Some("Lounge Sofa")
    );
}

#[test]
fn furniture_hit_test_finds_floor_lamp_via_synthetic() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let p = Point { x: 40, y: 40 };
    layout.floor_lamp = Some(p);
    assert_eq!(
        hit_test_furniture(&layout, p.x, p.y / 2),
        Some("Floor Lamp")
    );
}

#[test]
fn furniture_hit_test_finds_fish_tank_via_synthetic() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let p = Point { x: 40, y: 40 };
    layout.fish_tank = Some(p);
    assert_eq!(hit_test_furniture(&layout, p.x, p.y / 2), Some("Fish Tank"));
}

#[test]
fn snack_shelf_hovers_across_its_whole_sprite_not_just_the_footprint() {
    // The shelf sprite (7x10 visual) is CENTRED on the waypoint while the
    // walkable footprint is the End-anchored 7x2 south strip. Hover must
    // cover the sprite a user actually sees — the north (top shelf) row
    // and the centre both label; a footprint-centred box leaves only a
    // 2px mid-sprite band.
    let layout = Layout::compute(192, 160, Some(12)).expect("layout");
    let shelf = layout
        .waypoints
        .iter()
        .find(|w| w.kind == pixtuoid_scene::layout::WaypointKind::SnackShelf)
        .map(|w| w.pos)
        .expect("192x160 places the snack shelf");
    let vis =
        pixtuoid_scene::layout::furniture_def(pixtuoid_scene::layout::Furniture::SnackShelf).visual;
    // div_ceil: hit_test doubles the cell row back to buffer px, so an odd
    // top edge must probe the cell whose pixel pair falls INSIDE the box.
    let top_y = shelf.y.saturating_sub(vis.h / 2);
    assert_eq!(
        hit_test_furniture(&layout, shelf.x, top_y.div_ceil(2)),
        Some("Snack Shelf"),
        "top shelf row hovers"
    );
    assert_eq!(
        hit_test_furniture(&layout, shelf.x, shelf.y / 2),
        Some("Snack Shelf"),
        "sprite centre hovers"
    );
}

#[test]
fn furniture_hit_test_finds_meeting_chairs_on_a_real_layout() {
    // Both head-of-table chairs label on hover (the occupant's own hover
    // wins when someone sits — the agent pass runs first).
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
            hit_test_furniture(&layout, c.x, c.y / 2),
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
    assert_eq!(hit_test_furniture(&layout, p.x, p.y / 2), Some("TV Stand"));
}

#[test]
fn furniture_hit_test_finds_side_table_via_synthetic() {
    use pixtuoid_scene::layout::Point;
    let mut layout = Layout::compute(160, 200, Some(4)).expect("layout");
    let t = Point { x: 30, y: 90 };
    layout.lounge_side_table = Some(t);
    assert_eq!(
        hit_test_furniture(&layout, t.x, t.y / 2),
        Some("Side Table")
    );
    // 6px right of center is outside the 7-wide box (tl = t.x-3, [x-3..x+4)).
    assert_ne!(
        hit_test_furniture(&layout, t.x + 6, t.y / 2),
        Some("Side Table")
    );
}
