//! Hit-test functions for mouse interaction: agent hover, coffee machine
//! click-to-open, and furniture tooltip detection.

use std::time::SystemTime;

use pixtuoid_core::{AgentId, SceneState};

use pixtuoid_scene::layout::{Layout, Size};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::character_anchor;
use pixtuoid_scene::pose;

/// Hit-test the mouse cursor against each agent's current sprite footprint,
/// anchored on `character_anchor`. `(mx, my)` is in terminal cell coordinates.
pub(crate) fn hit_test_agent(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
    mx: u16,
    my: u16,
) -> Option<AgentId> {
    // x is NOT halved: in the half-block grid each pixel column is one cell
    // column, while each cell is 2 pixel ROWS.
    const SPRITE_W_CELLS: u16 = pixtuoid_scene::layout::CHARACTER_SPRITE_W;
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

/// Home-desk-only agent hit-test (no router/overlay state) — the deterministic
/// seated-agent locator for the test harness, which has no populated `route_ctx`.
/// A seated agent's `character_anchor` == its desk box, so the two agree.
///
/// `scene` must be a SINGLE-FLOOR scene matching `layout` (the caller projects via
/// `project_floor_scene` first): indexing `layout.home_desks` with a raw
/// multi-floor `desk_index` can pin an invisible agent from another floor.
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
        // `single_floor_local()`, NOT the arithmetic bridge: on an out-of-range
        // desk the bridge would wrap onto a synthetic later floor and could land
        // back in `[0..len)` — hit-testable while invisible to the renderer.
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // Through the painter's OWN seat anchor, not a second copy of its
        // arithmetic: this used to re-derive `desk.y - 8`, which silently stopped
        // matching the moment a desk could seat its occupant on the south side —
        // the agent rendered where the painter put them and was hit-testable
        // where this thought they were.
        let a = pixtuoid_scene::pixel_painter::seated_anchor_for(
            desk,
            SPRITE_W,
            layout.desk_facing_at(desk),
        );
        let (ax, ay) = (a.x, a.y);
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

/// Whether `(mx, my)` (terminal cell coords) falls on the coffee-machine section
/// of the pantry counter sprite.
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
    // target can't drift from the painted machine.
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

/// A short label if `(mx, my)` (terminal cell coords) falls on any known
/// furniture item. The coffee machine is handled separately for its
/// click-to-open behavior.
pub fn hit_test_furniture(layout: &Layout, mx: u16, my: u16) -> Option<&'static str> {
    use pixtuoid_scene::layout::{
        furniture_def, Furniture, PlantItem, PlantKind, PodDecor, PodDecorItem, WallDecor,
        WallDecorItem, WaypointKind, ELEVATOR_H, ELEVATOR_W,
    };
    // Hover boxes derive from the one furniture table — `.visual` (the visible
    // sprite) for what the user points at, `.footprint` where the obstacle is the
    // thing — so a geometry edit can't leave a stale hit box behind.
    let visual = |f| furniture_def(f).visual;
    let px = mx;
    let py = my * 2;

    let hit = |x: u16, y: u16, w: u16, h: u16| -> bool {
        px >= x && px < x.saturating_add(w) && py >= y && py < y.saturating_add(h)
    };

    // Home desks are top-left-anchored, unlike the center-anchored arms below.
    let desk_vis = visual(Furniture::Desk);
    for desk in &layout.home_desks {
        if hit(desk.x, desk.y, desk_vis.w, desk_vis.h) {
            return Some("Desk");
        }
    }

    // ONE hover region centred on the sofa: it's 3 seat waypoints, so per-seat
    // boxes would over-cover and multi-fire.
    if let Some(c) = layout.couch_sprite_center() {
        if hit(c.x.saturating_sub(10), c.y.saturating_sub(3), 20, 7) {
            return Some("Lounge Sofa");
        }
    }

    for wp in &layout.waypoints {
        let Size { w, h } = match wp.kind {
            // Hovers via the one-time region above.
            WaypointKind::Couch => continue,
            WaypointKind::Pantry => layout.pantry_counter_size(),
            // Meeting slots hover via the meeting_sofas loop below; island stands
            // are footprint-less slots on the island body, which has its own
            // hover region.
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair | WaypointKind::Island => {
                continue
            }
            // The shelf's sprite is CENTRED on the waypoint while its walkable
            // footprint is the End-anchored south strip, so a footprint hover box
            // would leave only a 2px band mid-sprite.
            WaypointKind::SnackShelf => furniture_def(Furniture::SnackShelf).visual,
            // Footprint owned by furniture_def — the same shape the mask + stand
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
                // Unreachable today (those kinds `continue` above), but this is a
                // per-frame mouse path: skip an unexpected kind rather than panic
                // the whole TUI.
                WaypointKind::Couch
                | WaypointKind::MeetingSofa
                | WaypointKind::MeetingChair
                | WaypointKind::Island => continue,
            });
        }
    }

    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for sofa in trio.sofas {
            let Size { w, h } = visual(Furniture::MeetingSofaBody); // full sprite, not the footprint
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

    if let Some(p) = layout.pantry.and_then(|p| p.kitchen_island) {
        let Size { w, h } = visual(Furniture::KitchenIsland);
        if hit(p.x.saturating_sub(w / 2), p.y.saturating_sub(h / 2), w, h) {
            return Some("Kitchen Island");
        }
    }

    for &PlantItem { kind, pos } in &layout.plants {
        let Size { w, h } = visual(kind.furniture());
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

    if let Some(tank) = layout.fish_tank() {
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

    // Head-of-table meeting chairs; an occupant's own hover wins, because the
    // agent pass runs before furniture.
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

    if let Some(lamp) = layout.floor_lamp() {
        let Size { w, h } = visual(Furniture::FloorLamp);
        if hit(
            lamp.x.saturating_sub(w / 2),
            lamp.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some("Floor Lamp");
        }
    }

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

    if let Some(t) = layout.lounge_side_table() {
        if hit(t.x.saturating_sub(3), t.y.saturating_sub(2), 7, 4) {
            return Some("Side Table");
        }
    }

    // EVERY room, not just room 0 (#555 left room 1 bare of decor). Rack and
    // doormat come from the SAME room-aggregate authority the painter draws from.
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

    // Placement + fit-gate from the PantryRoom aggregate, shared with the painter.
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

    if let Some(d) = layout.door {
        if hit(d.x, d.y, ELEVATOR_W, ELEVATOR_H) {
            return Some("Elevator");
        }
    }

    None
}

/// Whether `(mx, my)` (terminal cell coords) falls inside the office pet's
/// sprite. `pet_pos` is its center anchor in pixel coordinates; `anim_name`
/// selects the bounding-box size via `PetKind::hitbox`.
pub fn hit_test_pet(
    kind: PetKind,
    pet_pos: pixtuoid_scene::layout::Point,
    anim_name: &str,
    mx: u16,
    my: u16,
) -> bool {
    center_hit(pet_pos, kind.hitbox(anim_name), mx, my)
}

/// Whether cell `(mx, my)` falls on a `size`-px sprite CENTER-anchored at `pos`
/// (pixel coords). Owns the half-block `my * 2` conversion for every
/// center-anchored hover box, so the `* 2` can't be dropped at one site.
fn center_hit(pos: pixtuoid_scene::layout::Point, size: Size, mx: u16, my: u16) -> bool {
    let tl_x = pos.x.saturating_sub(size.w / 2);
    let tl_y = pos.y.saturating_sub(size.h / 2);
    let cell_y = my * 2;
    mx >= tl_x
        && mx < tl_x.saturating_add(size.w)
        && cell_y >= tl_y
        && cell_y < tl_y.saturating_add(size.h)
}

/// True if `(mx, my)` (terminal cell coords) falls on the gateway mascot's
/// `w`×`h`-px sprite, centered at `pos` (pixel coords). `w`/`h` must come from the
/// PAINTED frame (`MascotFrame`, which reads the pack's real size), so a re-tuned
/// or custom-pack mascot keeps its click box aligned with what's drawn.
pub fn hit_test_mascot(
    pos: pixtuoid_scene::layout::Point,
    w: u16,
    h: u16,
    mx: u16,
    my: u16,
) -> bool {
    center_hit(pos, pixtuoid_scene::layout::Size { w, h }, mx, my)
}

#[cfg(test)]
mod tests;
