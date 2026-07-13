//! The pantry aggregate: bounds + the counter footprint + the island.

use crate::layout::{
    furniture_def, Bounds, Furniture, Point, Size, OBSTACLE_PAD_PX, PANTRY_COUNTER_LARGE_W,
    WALL_THICK_H,
};

/// Compact counter footprint — the fallback for pantries narrower than the
/// detailed 32px kitchen run, and the size consumers read when no pantry
/// exists at all (the runtime-sized `Furniture::Pantry` row is `footprint:
/// None`, so this value IS the counter's only size authority). ONE
/// definition: `SceneLayout::pantry_counter_size()` falls back to it and the
/// placement code selects it, so the two can't drift.
pub(crate) const COMPACT_COUNTER: Size = Size { w: 20, h: 8 };

/// The pantry room: its bounds plus what it owns — the counter's chosen
/// footprint (large detailed kitchen vs [`COMPACT_COUNTER`], a width-only
/// decision) and the kitchen-island body centre (`None` when the room can't
/// host it clear of walls + the counter — refuse-don't-force). The island's
/// 4 `WaypointKind::Island` stand slots and the counter/snack-shelf
/// waypoints ride in `SceneLayout::waypoints` (wander destinations — shared
/// topic, different identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PantryRoom {
    pub bounds: Bounds,
    /// Footprint (width, height) of the pantry counter sprite. (32, 10)
    /// when the pantry is wide enough for the detailed kitchen run;
    /// [`COMPACT_COUNTER`] for narrow terminals where the wide sprite
    /// wouldn't fit. The renderer reads this to pick which sprite to paint
    /// (`pantry` vs `pantry_small`).
    pub counter_size: Size,
    /// Kitchen-island body centre (pantry v2's centre piece).
    pub kitchen_island: Option<Point>,
}

/// Vertical position of the pantry counter as a percent of the room height —
/// low (65%) for the large counter, a touch higher (60%) for the small one.
/// SINGLE SOURCE: the island clamp (which keeps the island clear of the
/// counter), the counter's own waypoint placement, AND
/// [`PantryRoom::content_fit_h`]'s inverse all read it, so they cannot
/// disagree — were they to drift, the clamp would guard a phantom counter
/// position.
pub(crate) fn pantry_counter_y_pct(counter_w: u16) -> u16 {
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        65
    } else {
        60
    }
}

impl PantryRoom {
    /// The room height at which the pantry can actually HOST its content —
    /// the inverse of the island's y-clamps: the counter line sits at
    /// `pct(h, pantry_counter_y_pct)` and the island needs `island_need`
    /// rows of ceiling above it (the snack shelf's bound is identical: its
    /// half-height 5 == the island's half_h + pad). `div_ceil` is the exact
    /// inverse of the truncating `pct()`. An associated fn (not a method):
    /// the split negotiation needs the answer BEFORE any bounds exist.
    /// Drift guard vs the island block's forward clamps:
    /// `meeting_room_donates_surplus_height_to_the_pantry`.
    pub(crate) fn content_fit_h(counter: Size) -> u16 {
        let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
        let island_half_h = furniture_def(Furniture::KitchenIsland).visual.h / 2;
        let island_need =
            clr + 2 * (island_half_h + OBSTACLE_PAD_PX) + 1 + counter.h / 2 + OBSTACLE_PAD_PX;
        (u32::from(island_need) * 100).div_ceil(u32::from(pantry_counter_y_pct(counter.w))) as u16
    }
}
