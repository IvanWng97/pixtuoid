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

/// Home-desk-only agent hit-test (no router/overlay state). The production CLICK
/// path now uses `TuiRenderer::hit_test_agent_at` (live-sprite — follows a walking
/// agent, FIND-22); this is RETAINED as the deterministic seated-agent locator for
/// the test harness (`harness::hover_agent`) + its unit tests: a seated agent's
/// `character_anchor` == its desk box, so the harness finds the hover cell without
/// a populated `route_ctx`. Uses home desk positions only (no walking agents).
///
/// `scene` must be a SINGLE-FLOOR scene matching `layout` — the caller
/// projects the live scene via `project_floor_scene(scene, current_floor)`
/// first, so only the visible floor's agents are tested, with their
/// re-projected desk indices. (Indexing `layout.home_desks` with a raw
/// multi-floor `desk_index` was exactly the global/local confusion the
/// `GlobalDeskIndex` newtype exists to prevent: while viewing floor ≥ 1 it
/// could pin an invisible agent from another floor.)
#[cfg(test)]
pub(crate) fn hit_test_from_tui(
    scene: &SceneState,
    layout: &Layout,
    mx: u16,
    my: u16,
) -> Option<AgentId> {
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
        WallDecorItem, WaypointKind, ELEVATOR_H, ELEVATOR_W,
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

    // Home desks: derive the box from the table's `visual` like every sibling arm
    // (top-left-anchored at desk.{x,y}); the old hardcoded DESK_W+2 clipped 2px.
    let desk_vis = visual(Furniture::Desk);
    for desk in &layout.home_desks {
        if hit(desk.x, desk.y, desk_vis.w, desk_vis.h) {
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
            // The shelf's sprite is CENTRED on the waypoint (7x10 visual) while
            // its walkable footprint is the End-anchored 7x2 south strip — a
            // footprint hover box would leave only a 2px band mid-sprite, so
            // hover the visual, like the fish tank and island bodies.
            WaypointKind::SnackShelf => furniture_def(Furniture::SnackShelf).visual,
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
    // (#555: room 1 used to render bare of decor, keyed room 0 only). Both the
    // rack (coat_rack_pos, incl. the narrow-fitted-room yield) and the doormat
    // (doormat_rect) come from the SAME room-aggregate authority the painter
    // draws from, so a geometry edit can't leave a stale hover box behind.
    for room in &layout.meeting_rooms {
        if let Some(rack) = room.coat_rack_pos() {
            if hit(rack.x.saturating_sub(2), rack.y, 5, 8) {
                return Some("Coat Rack");
            }
        }
        if let Some(mat) = room.doormat_rect() {
            if hit(mat.x, mat.y, mat.width, mat.height) {
                return Some("Doormat");
            }
        }
    }

    // Pantry room procedural items (water cooler, trash bin) — placement +
    // fit-gate from the PantryRoom aggregate, shared with the scene painter.
    if let Some(pantry) = layout.pantry {
        if let Some(cooler) = pantry.water_cooler_rect() {
            if hit(cooler.x, cooler.y, cooler.width, cooler.height) {
                return Some("Water Cooler");
            }
        }
        if let Some(bin) = pantry.trash_bin_rect() {
            if hit(bin.x, bin.y, bin.width, bin.height) {
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
mod tests;
