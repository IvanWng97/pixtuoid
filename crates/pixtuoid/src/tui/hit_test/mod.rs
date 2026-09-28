//! Mouse hit-testing: which agent, pet, mascot or piece of furniture is painted
//! under a terminal cell.

use std::time::SystemTime;

use pixtuoid_core::{AgentId, SceneState};

use pixtuoid_scene::layout::{anchored_top_left, Anchor, Bounds, Layout, Point, Size};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::character_anchor;
use pixtuoid_scene::pose;

use crate::tui::geometry::CellArea;

/// Hover and click box: the default sprite `character_anchor` places, not a
/// custom pack's frame.
const AGENT_BOX: Size = Size {
    w: pixtuoid_scene::layout::CHARACTER_SPRITE_W,
    h: pixtuoid_scene::layout::CHARACTER_SPRITE_H,
};

/// Hit-test `cell` against each agent's current sprite box, anchored on
/// `character_anchor`.
pub(crate) fn hit_test_agent(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
    cell: CellArea,
) -> Option<AgentId> {
    scene
        .agents
        .values()
        .find(|agent| {
            character_anchor(agent, layout, now, rctx)
                .is_some_and(|anchor| box_hit(Anchor::TopLeft, anchor, AGENT_BOX, cell))
        })
        .map(|agent| agent.agent_id)
}

/// Home-desk-only agent hit-test (no router/overlay state) — the deterministic
/// seated-agent locator for the test harness, which has no populated `route_ctx`.
/// A seated agent's `character_anchor` is `seated_anchor_facing` of its desk,
/// which this reads directly, so the two agree.
///
/// `scene` must be a SINGLE-FLOOR scene matching `layout` (the caller projects via
/// `project_floor_scene` first): indexing `layout.home_desks` with a raw
/// multi-floor `desk_index` can pin an invisible agent from another floor.
#[cfg(test)]
pub(crate) fn hit_test_from_tui(
    scene: &SceneState,
    layout: &Layout,
    cell: CellArea,
) -> Option<AgentId> {
    for agent in scene.agents.values() {
        // `single_floor_local()`, NOT the arithmetic bridge: on an out-of-range
        // desk the bridge would wrap onto a synthetic later floor and could land
        // back in `[0..len)` — hit-testable while invisible to the renderer.
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // The painter's own anchor, not the desk box: a south-facing desk seats
        // its sitter north of the desk.
        let a = pixtuoid_scene::pixel_painter::seated_anchor_facing(
            desk,
            AGENT_BOX.w,
            layout.desk_facing(agent.desk_index.single_floor_local()),
        );
        if box_hit(Anchor::TopLeft, a, AGENT_BOX, cell) {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Whether `cell` shows the coffee-machine section of the pantry counter
/// sprite.
pub(crate) fn hit_test_coffee_machine(layout: &Layout, cell: CellArea) -> bool {
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
    cell.overlaps(
        Point {
            x: sprite_x + dx0,
            y: sprite_y,
        },
        dx1 - dx0,
        ch,
    )
}

/// A short label if `cell` shows any known furniture item. The coffee machine
/// is handled separately for its click-to-open behavior.
pub(crate) fn hit_test_furniture(layout: &Layout, cell: CellArea) -> Option<&'static str> {
    use pixtuoid_scene::layout::{
        furniture_def, Furniture, PlantItem, PlantKind, PodDecor, PodDecorItem, WallDecor,
        WallDecorItem, WaypointKind, ELEVATOR_H, ELEVATOR_W,
    };
    // Every hover box reads the size its piece is laid out and z-sorted by — the
    // furniture table's `.visual`, a room's rect, or a layout size — never a
    // literal; `every_hover_size_is_its_painted_sprite_size` pins the
    // pack-blitted ones to their sprite.
    let visual = |f| furniture_def(f).visual;
    let centered = |pos, size| box_hit(Anchor::Center, pos, size, cell);
    let on_rect = |b: Bounds| {
        box_hit(
            Anchor::TopLeft,
            Point { x: b.x, y: b.y },
            Size {
                w: b.width,
                h: b.height,
            },
            cell,
        )
    };

    let desk_vis = visual(Furniture::Desk);
    for &desk in &layout.home_desks {
        if box_hit(Anchor::TopLeft, desk, desk_vis, cell) {
            return Some("Desk");
        }
    }

    // ONE hover region on the sofa sprite, which the lounge shares with the
    // meeting rooms: it's 3 seat waypoints, so per-seat boxes would over-cover
    // and multi-fire.
    if let Some(c) = layout.couch_sprite_center() {
        if centered(c, visual(Furniture::MeetingSofaBody)) {
            return Some("Lounge Sofa");
        }
    }

    for wp in &layout.waypoints {
        let size = match wp.kind {
            // Hovers via the one-time region above.
            WaypointKind::Couch => continue,
            WaypointKind::Pantry => layout.pantry_counter_size(),
            // Meeting slots hover on their furniture below (the trio's sofas, the
            // head-of-table chairs); island stands are footprint-less slots on the
            // island body, which has its own hover region.
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair | WaypointKind::Island => {
                continue
            }
            other => visual(other.furniture()),
        };
        if centered(wp.pos, size) {
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
            if centered(sofa, visual(Furniture::MeetingSofaBody)) {
                return Some("Meeting Sofa");
            }
        }
        if centered(trio.table, visual(Furniture::MeetingTable)) {
            return Some("Meeting Table");
        }
    }

    if let Some(p) = layout.pantry.and_then(|p| p.kitchen_island) {
        if centered(p, visual(Furniture::KitchenIsland)) {
            return Some("Kitchen Island");
        }
    }

    for &PlantItem { kind, pos } in &layout.plants {
        if centered(pos, visual(kind.furniture())) {
            return Some(match kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            });
        }
    }

    if let Some(tank) = layout.fish_tank() {
        if centered(tank, visual(Furniture::FishTank)) {
            return Some("Fish Tank");
        }
    }

    // Head-of-table meeting chairs; an occupant's own hover wins, because the
    // agent pass runs before furniture.
    for wp in &layout.waypoints {
        if wp.kind == WaypointKind::MeetingChair
            && centered(wp.pos, visual(Furniture::MeetingChair))
        {
            return Some("Meeting Chair");
        }
    }

    if let Some(lamp) = layout.floor_lamp() {
        if centered(lamp, visual(Furniture::FloorLamp)) {
            return Some("Floor Lamp");
        }
    }

    for &WallDecorItem { kind, pos } in &layout.wall_decor {
        if box_hit(Anchor::TopLeft, pos, visual(kind.furniture()), cell) {
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
        if centered(pos, visual(kind.furniture())) {
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
        if centered(t, visual(Furniture::LoungeSideTable)) {
            return Some("Side Table");
        }
    }

    // EVERY room, not just room 0 (#555 left room 1 bare of decor).
    for room in &layout.meeting_rooms {
        if room.coat_rack_rect().is_some_and(on_rect) {
            return Some("Coat Rack");
        }
        if room.doormat_rect().is_some_and(on_rect) {
            return Some("Doormat");
        }
    }

    if let Some(pantry) = layout.pantry {
        if pantry.water_cooler_rect().is_some_and(on_rect) {
            return Some("Water Cooler");
        }
        if pantry.trash_bin_rect().is_some_and(on_rect) {
            return Some("Trash Bin");
        }
    }

    let door = Size {
        w: ELEVATOR_W,
        h: ELEVATOR_H,
    };
    if layout
        .door
        .is_some_and(|d| box_hit(Anchor::TopLeft, d, door, cell))
    {
        return Some("Elevator");
    }

    None
}

/// Whether `cell` shows the office pet's sprite. `pet_pos` is its center
/// anchor in pixel coordinates; `anim_name` selects the bounding-box size via
/// `PetKind::hitbox`.
pub(crate) fn hit_test_pet(kind: PetKind, pet_pos: Point, anim_name: &str, cell: CellArea) -> bool {
    box_hit(Anchor::Center, pet_pos, kind.hitbox(anim_name), cell)
}

/// Whether `cell` shows a `size`-px box placed at `pos` (pixel coords) by
/// `anchor` — through [`anchored_top_left`], the painter's own placement.
fn box_hit(anchor: Anchor, pos: Point, size: Size, cell: CellArea) -> bool {
    cell.overlaps(
        anchored_top_left(anchor, pos, size.w, size.h),
        size.w,
        size.h,
    )
}

/// True if `cell` shows the gateway mascot's `w`×`h`-px sprite, centered at
/// `pos` (pixel coords). `w`/`h` must come from the PAINTED frame
/// (`MascotFrame`, which reads the pack's real size), so a re-tuned or
/// custom-pack mascot keeps its click box aligned with what's drawn.
pub(crate) fn hit_test_mascot(pos: Point, w: u16, h: u16, cell: CellArea) -> bool {
    box_hit(Anchor::Center, pos, Size { w, h }, cell)
}

#[cfg(test)]
mod tests;
