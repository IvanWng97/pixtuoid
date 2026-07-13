//! The pantry aggregate: bounds + the counter footprint + the island.

use crate::layout::{Bounds, Point, Size};

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
