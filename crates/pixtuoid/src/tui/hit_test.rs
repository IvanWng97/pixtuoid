//! Hit-test functions for mouse interaction: agent hover, coffee machine
//! click-to-open, and furniture tooltip detection.

use std::time::SystemTime;

use pixtuoid_core::{AgentId, SceneState};

use pixtuoid_scene::layout::{Layout, Size};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::character_anchor;
use pixtuoid_scene::pose;

/// Hit-test the mouse cursor against each agent's current sprite footprint.
/// Returns the agent under `(mx, my)` (in terminal cell coordinates), or
/// `None` if no agent occupies that cell.
///
/// The character sprite is 8×12 pixels, which in cell space is 8 cells
/// wide × 6 cells tall (one cell = 2 vertical pixels). We test against
/// that exact bounding box anchored on the agent's `character_anchor`.
pub(crate) fn hit_test_agent(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
    mx: u16,
    my: u16,
) -> Option<AgentId> {
    // Width-in-cells: the sprite width in px IS the cell width — we don't divide
    // x by 2, since each pixel column is one cell column in the half-block grid.
    const SPRITE_W_CELLS: u16 = pixtuoid_scene::layout::CHARACTER_SPRITE_W;
    // Height-in-cells: the 12 px sprite is 6 half-block cells.
    const SPRITE_H_CELLS: u16 = pixtuoid_scene::layout::CHARACTER_SPRITE_H_CELLS;
    for agent in scene.agents.values() {
        let Some(anchor) = character_anchor(agent, layout, now, rctx) else {
            continue;
        };
        let cell_x = anchor.x;
        let cell_y = anchor.y / 2;
        if mx >= cell_x
            && mx < cell_x.saturating_add(SPRITE_W_CELLS)
            && my >= cell_y
            && my < cell_y.saturating_add(SPRITE_H_CELLS)
        {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Lightweight hit-test for click-to-pin without needing router/overlay state.
/// Uses home desk positions only (no walking agents).
///
/// `scene` must be a SINGLE-FLOOR scene matching `layout` — the caller
/// projects the live scene via `project_floor_scene(scene, current_floor)`
/// first, so only the visible floor's agents are tested, with their
/// re-projected desk indices. (Indexing `layout.home_desks` with a raw
/// multi-floor `desk_index` was exactly the global/local confusion the
/// `GlobalDeskIndex` newtype exists to prevent: while viewing floor ≥ 1 it
/// could pin an invisible agent from another floor.)
pub fn hit_test_from_tui(scene: &SceneState, layout: &Layout, mx: u16, my: u16) -> Option<AgentId> {
    const SPRITE_W: u16 = pixtuoid_scene::layout::CHARACTER_SPRITE_W;
    const SPRITE_H_CELLS: u16 = pixtuoid_scene::layout::CHARACTER_SPRITE_H_CELLS;
    for agent in scene.agents.values() {
        // `single_floor_local()` (the projected-scene identity), NOT the
        // arithmetic bridge: on an out-of-range desk the bridge would wrap onto
        // a synthetic later floor of the uniform projection and could land back
        // in `[0..len)` — hit-testable while invisible to the renderer. The
        // identity keeps the OOB index OOB, so `home_desk` skips it like the
        // render path does.
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // The painter's seated anchor (pixtuoid_scene pixel_painter::anchors::
        // seated_anchor): the 8px sprite centered on DESK_W, 8px above the desk.
        // Derived from the SAME DESK_W the painter centers on — the pairing is
        // pinned against `character_anchor` by
        // `from_tui_pin_box_matches_the_painted_seated_anchor`, so the pin box
        // can't drift from the hover/blit geometry again.
        let ax = desk.x + pixtuoid_scene::layout::DESK_W.saturating_sub(SPRITE_W) / 2;
        let ay = desk.y.saturating_sub(8);
        let cell_x = ax;
        let cell_y = ay / 2;
        if mx >= cell_x
            && mx < cell_x.saturating_add(SPRITE_W)
            && my >= cell_y
            && my < cell_y.saturating_add(SPRITE_H_CELLS)
        {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Hit-test whether the mouse is over the pantry coffee machine.
/// Returns true if `(mx, my)` (terminal cell coords) falls on the coffee
/// machine section of the pantry counter sprite.
pub fn hit_test_coffee_machine(layout: &Layout, mx: u16, my: u16) -> bool {
    let pantry_wp = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, pixtuoid_scene::layout::WaypointKind::Pantry));
    let Some(wp) = pantry_wp else {
        return false;
    };
    let Size { w: cw, h: ch } = layout.pantry_counter_size();
    let sprite_x = wp.pos.x.saturating_sub(cw / 2);
    let sprite_y = wp.pos.y.saturating_sub(ch / 2);
    // Derive the machine box from the painter's shared column source so the click
    // target can't drift from the painted machine (the version-popup / seated-
    // anchor pinning discipline). The small-case previously used a wider [8,13)
    // that false-positived counter cells 8 and 12.
    let (dx0, dx1) = if cw >= pixtuoid_scene::layout::PANTRY_COUNTER_LARGE_W {
        pixtuoid_scene::pixel_painter::PANTRY_COFFEE_COLS_LARGE
    } else {
        pixtuoid_scene::pixel_painter::PANTRY_COFFEE_COLS_SMALL
    };
    let (coffee_x0, coffee_x1) = (sprite_x + dx0, sprite_x + dx1);
    let coffee_y0 = sprite_y;
    let coffee_y1 = sprite_y + ch;
    let cell_y = my * 2;
    mx >= coffee_x0 && mx < coffee_x1 && cell_y >= coffee_y0 && cell_y < coffee_y1
}

/// Hit-test all furniture items in the office. Returns a short label
/// if `(mx, my)` (terminal cell coords) falls on any known item.
/// The coffee machine is handled separately for its click-to-open
/// behavior — this function covers the remaining decorations.
pub fn hit_test_furniture(layout: &Layout, mx: u16, my: u16) -> Option<&'static str> {
    use pixtuoid_scene::layout::{
        furniture_def, Furniture, PlantItem, PlantKind, PodDecor, PodDecorItem, WallDecor,
        WallDecorItem, WaypointKind, DESK_H, DESK_W, ELEVATOR_H, ELEVATOR_W,
    };
    // Hover boxes derive from the one furniture table — `.visual` (the visible
    // sprite) for what the user points at, `.footprint` where the obstacle is
    // the thing — so a geometry edit can't leave a stale hit box behind.
    let visual = |f| furniture_def(f).visual;
    let px = mx;
    let py = my * 2;

    let hit = |x: u16, y: u16, w: u16, h: u16| -> bool {
        px >= x && px < x.saturating_add(w) && py >= y && py < y.saturating_add(h)
    };

    // Home desks
    for desk in &layout.home_desks {
        if hit(desk.x, desk.y, DESK_W + 2, DESK_H) {
            return Some("Desk");
        }
    }

    // Lounge couch: one 20px hover region centred on the sofa. It's 3 seat
    // waypoints now, so per-seat boxes would over-cover and multi-fire — hit
    // it once at couch_sprite_center, mirroring the single furniture paint.
    if let Some(c) = layout.couch_sprite_center {
        if hit(c.x.saturating_sub(10), c.y.saturating_sub(3), 20, 7) {
            return Some("Lounge Sofa");
        }
    }

    // Waypoints
    for wp in &layout.waypoints {
        let Size { w, h } = match wp.kind {
            // Couch hovers via the one-time region above (3 seat waypoints).
            WaypointKind::Couch => continue,
            WaypointKind::Pantry => layout.pantry_counter_size(),
            // Meeting slots hover via the dedicated meeting_sofas loop below;
            // island stands are footprint-less slots on the island body,
            // which has its own hover region — skip.
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair | WaypointKind::Island => {
                continue
            }
            // Footprint owned by furniture_def — same shape the mask + stand
            // point use, so the hover box can't drift from them.
            other => match furniture_def(other.furniture()).footprint {
                Some(fp) => fp,
                None => continue,
            },
        };
        let wx = wp.pos.x.saturating_sub(w / 2);
        let wy = wp.pos.y.saturating_sub(h / 2);
        if hit(wx, wy, w, h) {
            return Some(match wp.kind {
                WaypointKind::Pantry => "Pantry Counter",
                WaypointKind::PhoneBooth => "Phone Booth",
                WaypointKind::StandingDesk => "Standing Desk",
                WaypointKind::VendingMachine => "Vending Machine",
                WaypointKind::Printer => "Printer",
                WaypointKind::SnackShelf => "Snack Shelf",
                // Proven unreachable today (couch + meeting/island slots
                // `continue` above), but this is a per-frame mouse path: skip an
                // unexpected kind rather than panic the whole TUI if a future
                // refactor adds a WaypointKind or drops one of those earlier
                // `continue`s.
                WaypointKind::Couch
                | WaypointKind::MeetingSofa
                | WaypointKind::MeetingChair
                | WaypointKind::Island => continue,
            });
        }
    }

    // Meeting sofas (20px sprite, centred on the sofa point) + tables, per room.
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for sofa in trio.sofas {
            let Size { w, h } = visual(Furniture::MeetingSofaBody); // full 20px sprite, not the 16px footprint
            if hit(
                sofa.x.saturating_sub(w / 2),
                sofa.y.saturating_sub(h / 2),
                w,
                h,
            ) {
                return Some("Meeting Sofa");
            }
        }
        let Size { w, h } = visual(Furniture::MeetingTable);
        if hit(
            trio.table.x.saturating_sub(w / 2),
            trio.table.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some("Meeting Table");
        }
    }

    // Kitchen island (the pantry's centre piece; hover the full sprite).
    if let Some(p) = layout.pantry.and_then(|p| p.kitchen_island) {
        let Size { w, h } = visual(Furniture::KitchenIsland);
        if hit(p.x.saturating_sub(w / 2), p.y.saturating_sub(h / 2), w, h) {
            return Some("Kitchen Island");
        }
    }

    // Plants
    for &PlantItem { kind, pos } in &layout.plants {
        let Size { w, h } = visual(kind.furniture()); // hover the whole visible plant, not just its ground base
        if hit(
            pos.x.saturating_sub(w / 2),
            pos.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some(match kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            });
        }
    }

    // Fish tank — center-anchored like its mask stamp.
    if let Some(tank) = layout.fish_tank {
        let Size { w, h } = visual(Furniture::FishTank);
        if hit(
            tank.x.saturating_sub(w / 2),
            tank.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some("Fish Tank");
        }
    }

    // Head-of-table meeting chairs (the occupant's own hover wins when
    // someone sits — the agent pass runs before furniture).
    for wp in &layout.waypoints {
        if wp.kind == pixtuoid_scene::layout::WaypointKind::MeetingChair {
            let Size { w, h } = visual(Furniture::MeetingChair);
            if hit(
                wp.pos.x.saturating_sub(w / 2),
                wp.pos.y.saturating_sub(h / 2),
                w,
                h,
            ) {
                return Some("Meeting Chair");
            }
        }
    }

    // Floor lamp
    if let Some(lamp) = layout.floor_lamp {
        let Size { w, h } = visual(Furniture::FloorLamp); // full 4×10 lamp sprite
        if hit(
            lamp.x.saturating_sub(w / 2),
            lamp.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some("Floor Lamp");
        }
    }

    // Wall decor
    for &WallDecorItem { kind, pos } in &layout.wall_decor {
        let Size { w, h } = furniture_def(kind.furniture()).visual;
        if hit(pos.x, pos.y, w, h) {
            return Some(match kind {
                WallDecor::Whiteboard => "Whiteboard",
                WallDecor::Bookshelf => "Bookshelf",
                WallDecor::BulletinBoard => "Bulletin Board",
                WallDecor::ExitSign => "Exit Sign",
                WallDecor::MeetingScreen => "Meeting Screen",
            });
        }
    }

    // Pod decor (aisle items)
    for &PodDecorItem { kind, pos } in &layout.pod_decor {
        let Size { w, h } = furniture_def(kind.furniture()).visual;
        if hit(
            pos.x.saturating_sub(w / 2),
            pos.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some(match kind {
                PodDecor::PlantTall => "Tall Plant",
                PodDecor::Whiteboard => "Whiteboard",
                PodDecor::Tv => "TV Stand",
                PodDecor::PhoneBooth => "Phone Booth",
                PodDecor::StandingDesk => "Standing Desk",
            });
        }
    }

    // Lounge side table
    if let Some(t) = layout.lounge_side_table {
        if hit(t.x.saturating_sub(3), t.y.saturating_sub(2), 7, 4) {
            return Some("Side Table");
        }
    }

    // Meeting room procedural items (coat rack, doormat) — EVERY room
    // (#555: room 1 used to render bare of decor, keyed room 0 only). The
    // rack spot (incl. the narrow-fitted-room yield) comes from
    // coat_rack_pos, the same authority the painter enqueues from.
    for room in &layout.meeting_rooms {
        let mr = room.bounds;
        if let Some(rack) = room.coat_rack_pos() {
            if hit(rack.x.saturating_sub(2), rack.y, 5, 8) {
                return Some("Coat Rack");
            }
        }
        if mr.width > 10 {
            let mat_x = mr.x + mr.width + 1;
            let mat_y = mr.y + mr.height / 2 - 2;
            if hit(mat_x, mat_y, 4, 5) {
                return Some("Doormat");
            }
        }
    }

    // Pantry room procedural items (water cooler, trash bin)
    if let Some(pr) = layout.pantry.map(|p| p.bounds) {
        if pr.height > 25 && pr.width > 12 {
            let wx = pr.x + pr.width - 6;
            let wy = pr.y + 8;
            if hit(wx, wy, 3, 6) {
                return Some("Water Cooler");
            }
        }
        if pr.height > 20 {
            let tx = pr.x + 3;
            let ty = pr.y + pr.height - 14;
            if hit(tx, ty, 4, 5) {
                return Some("Trash Bin");
            }
        }
    }

    // Door / elevator
    if let Some(d) = layout.door {
        if hit(d.x, d.y, ELEVATOR_W, ELEVATOR_H) {
            return Some("Elevator");
        }
    }

    None
}

/// Hit-test whether the mouse is over the office pet.
/// `pet_pos` is the pet's center anchor in pixel coordinates.
/// `kind` selects the species; `anim_name` selects the bounding box size
/// via `PetKind::hitbox`.
///
/// Returns true if `(mx, my)` (terminal cell coords) falls inside
/// the sprite's footprint.
pub fn hit_test_pet(
    kind: PetKind,
    pet_pos: pixtuoid_scene::layout::Point,
    anim_name: &str,
    mx: u16,
    my: u16,
) -> bool {
    let Size { w, h } = kind.hitbox(anim_name);
    let tl_x = pet_pos.x.saturating_sub(w / 2);
    let tl_y = pet_pos.y.saturating_sub(h / 2);
    let cell_y = my * 2;
    mx >= tl_x && mx < tl_x.saturating_add(w) && cell_y >= tl_y && cell_y < tl_y.saturating_add(h)
}

/// True if `(mx, my)` (terminal cell coords) falls on the gateway mascot's
/// 14×12 sprite, centered at `pos` (pixel coords). The lobster is symmetric and
/// a single sprite size, so no per-anim hitbox is needed.
pub fn hit_test_mascot(pos: pixtuoid_scene::layout::Point, mx: u16, my: u16) -> bool {
    const W: u16 = 14;
    const H: u16 = 12;
    let tl_x = pos.x.saturating_sub(W / 2);
    let tl_y = pos.y.saturating_sub(H / 2);
    let cell_y = my * 2;
    mx >= tl_x && mx < tl_x.saturating_add(W) && cell_y >= tl_y && cell_y < tl_y.saturating_add(H)
}

#[cfg(test)]
mod tests {
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
            + pixtuoid_scene::layout::DESK_W
                .saturating_sub(pixtuoid_scene::layout::CHARACTER_SPRITE_W)
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

    // --- hit_test_furniture: kinds the compute path never emits -------------
    // compute_with_seed never produces PlantKind::Ficus or WallDecor::Bulletin
    // Board, so the harness real-layout loop can't reach those two return arms.
    // Push them into the pub Vecs of a computed layout and hit their centers.

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
}
