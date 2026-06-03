//! Zone-based scene layout for the top-down office — primitive geometry
//! only, no terminal deps. Computed once per (buf_w, buf_h, num_agents)
//! triple; serializable / wire-shippable for the future v2 daemon split.
//!
//! Splits a buf-pixel rectangle into quadrants (meeting / pantry /
//! cubicles / lounge), then computes per-agent home desks, named lounge
//! waypoints, decor positions, and a per-pixel walkability mask.
//!
//! Submodules:
//!   * `decor` — the furniture/decor vocabulary: the role enums
//!     (`WaypointKind`/`PodDecor`/`PlantKind`/`WallDecor`) plus the unified
//!     `Furniture` geometry table they map onto.
//!   * `compute` — `compute_with_seed`: desk/decor/wall/waypoint placement.
//!   * `placement` — the `Anchor` convention (where a box sits vs its `pos`).
//!   * `mask` — `build_walkable_mask`: stamps obstacle footprints for routing.
//!   * `approach` — `stand_point`/`approach_point`: where an agent stands to use a piece.
//!   * `reach` — `ReachSet`: coarse-cell BFS mirroring the tui A* grid.

mod approach;
mod compute;
mod decor;
mod mask;
mod placement;
mod reach;

pub use approach::{approach_point, stand_point};
pub use decor::{
    desk_furniture_def, desk_walk_anchor, furniture_def, seated_foot_cell, ApproachSides,
    DwellWindow, Facing, Furniture, FurnitureDef, PlantKind, PodDecor, WallDecor, WaypointKind,
    DESK_APPROACH, SEAT_RENDER_Y_OFF, WALKING_Y_OFF,
};
pub use mask::{WALL_THICK_H, WALL_THICK_V};
pub use placement::{anchored_top_left, z_sort_row, Anchor};
pub use reach::{ReachSet, REACH_CELL_SIZE, REACH_CELL_WALKABLE_MIN};

use crate::walkable::WalkableMask;

/// Primitive rectangle. Same shape as `ratatui::layout::Rect` so the
/// binary can convert with a one-line field-by-field copy without paying
/// for the ratatui dep in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounds {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

/// A width×height extent in pixels. Names the axes so a (w,h) tuple can't be
/// silently transposed. Distinct from Point (a position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size {
    pub w: u16,
    pub h: u16,
}

/// An interior room-wall segment — the two endpoints of a straight (horizontal
/// or vertical) wall run. Names the endpoints of what was a `(Point, Point)`
/// tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WallSegment {
    pub start: Point,
    pub end: Point,
}

/// A placed plant: its kind paired with its centre position. Names what was a
/// `(PlantKind, Point)` tuple in `SceneLayout::plants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlantItem {
    pub kind: PlantKind,
    pub pos: Point,
}

/// A placed wall decoration: its kind paired with its position. Names what was a
/// `(WallDecor, Point)` tuple in `SceneLayout::wall_decor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallDecorItem {
    pub kind: WallDecor,
    pub pos: Point,
}

/// A placed aisle/pod decoration: its kind paired with its centre position.
/// Names what was a `(PodDecor, Point)` tuple in `SceneLayout::pod_decor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodDecorItem {
    pub kind: PodDecor,
    pub pos: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waypoint {
    pub pos: Point,
    pub kind: WaypointKind,
    /// Direction the occupant faces while at this waypoint. `South` for
    /// all the legacy single-point waypoints (facing-neutral); set toward
    /// the table for meeting-room slots.
    pub facing: Facing,
    /// Meeting-room id this slot belongs to (`Some(idx)` for
    /// `MeetingSofa` / `MeetingStand`, `None` otherwise). Slots sharing a
    /// `room_id` form one group-chitchat venue.
    pub room_id: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SceneLayout {
    pub buf_w: u16,
    pub buf_h: u16,
    pub cubicle_band: Bounds,
    /// Horizontal corridor at the bottom of the cubicle area — the "main
    /// aisle" connecting door / meeting / pantry. Used by the cat
    /// wanderer destination.
    pub walkway: Bounds,
    pub home_desks: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    pub plants: Vec<PlantItem>,
    pub wall_decor: Vec<WallDecorItem>,
    /// Decor items placed in the aisles between 2×2 desk pods. Each
    /// item paints its sprite centred on `pos` and marks it as an obstacle
    /// in the walkable mask.
    pub pod_decor: Vec<PodDecorItem>,
    pub floor_lamp: Option<Point>,
    /// Lounge side table (7×4 wood + magazine) placed next to the
    /// viewing couch on the side opposite the floor lamp.
    pub lounge_side_table: Option<Point>,
    pub door: Option<Point>,
    pub door_threshold: Option<Point>,
    pub meeting_room: Option<Bounds>,
    pub pantry_room: Option<Bounds>,
    pub meeting_sofas: Vec<Point>,
    pub meeting_tables: Vec<Point>,
    pub room_walls: Vec<WallSegment>,
    pub top_margin: u16,
    pub pantry_table: Option<Point>,
    pub pantry_chairs: Vec<Point>,
    /// Footprint (width, height) of the pantry counter sprite. (32, 10)
    /// when the pantry is large enough for the detailed kitchen run;
    /// (20, 8) fallback for narrow terminals where the wide sprite
    /// wouldn't fit. The renderer reads this to pick which sprite to
    /// paint (`pantry` vs `pantry_small`).
    pub pantry_counter_size: Size,
    pub corridor: Option<Bounds>,
    /// Centre point of the lounge couch sprite (the middle of its 3 seats).
    /// The couch is 3 separate seat waypoints; the sprite + rug + side table
    /// paint once, centred here. `None` when no couch fits.
    pub couch_sprite_center: Option<Point>,
    pub walkable: WalkableMask,
    /// Coarse-cell reachable component (the walkable area an agent can A\*-route
    /// to). Computed once from a known in-component seed; consumed by
    /// `approach_point` to prefer a *reachable* approach side over a merely-
    /// walkable-but-walled-off one. Mirrors the tui router's coarsening.
    pub reachable: ReachSet,
}

/// Padding (in pixels) added around every obstacle when building the
/// walkable mask. Reserves a buffer zone so characters route AROUND
/// furniture rather than scraping along its edge.
pub const OBSTACLE_PAD_PX: u16 = 2;

/// The north wall+window band's visual bottom sits this many px ABOVE
/// `top_margin`; the rows in between (`[top_margin - this, top_margin)`) render
/// as carpet apron, not wall. The mask therefore blocks only down to the band
/// bottom (`top_margin - this`), NOT the full `top_margin`, so the walkable area
/// hugs the visible wall base instead of eating a strip of carpet (invariant #6,
/// the same ground-projection rule furniture footprints follow). The renderer
/// derives `top_wall_h = top_margin - this` for the wall/window/trim paint, so
/// the two MUST agree — one source here prevents the mask and the visual from
/// drifting (the relationship was a `- 4` literal duplicated across both).
pub const WALL_BAND_TO_TOP_MARGIN: u16 = 4;

/// How many pixels of the pantry counter actually sit on the floor. The
/// counter is a 3/4-perspective sprite (10 px tall in the large variant)
/// centered on its waypoint `pos`, but only the southern base contacts the
/// ground — the receding cabinet tops + backsplash are elevation that
/// overhangs (invariant #6). The mask blocks only this shallow strip,
/// anchored to the sprite's SOUTH base, so the non-walkable area hugs the
/// counter's foot instead of the full sprite height. A character routed
/// behind (north of) the counter is occluded by the counter's own y-sorted
/// sprite (the overhang paints over them), exactly like the couch — see
/// `mask::build_walkable_mask`.
pub const PANTRY_FOOTPRINT_DEPTH: u16 = 3;

pub const DESK_W: u16 = 12;
pub const DESK_H: u16 = 6;
/// Elevator-door sprite size in buffer px — the single source for the door's
/// width (the layout slots the sprite into the back wall and the renderer skips
/// the window glass it covers) and height (the z-sort anchor row). Both the
/// layout (`compute`) and the renderer (`pixel_painter` / `background`) read
/// these so the door footprint can't drift between them.
pub const ELEVATOR_W: u16 = 16;
pub const ELEVATOR_H: u16 = 14;
/// Hard cap on how many cubicles get painted regardless of how high
/// `max_desks` is set. Bumped from 8 → 16 after the lounge_band quadrant
/// was retired and the cubicle band absorbed its vertical space — more
/// rows fit, so more agents can have their own desk before falling back
/// to overflow seating.
pub const MAX_VISIBLE_DESKS: usize = 16;
pub const DESK_GAP_X: u16 = 11;
pub const DESK_GAP_Y: u16 = 14;
pub const MIN_TOP_MARGIN: u16 = 20;
const MIN_DUAL_MEETING_H: u16 = 80;

/// Number of desks per side in a pod (`POD_SIDE * POD_SIDE` total).
pub const POD_SIDE: u16 = 2;
/// Gap between two desks inside the same pod — big enough that each
/// desk reads as its own workstation (chair + monitor + space), not
/// a merged blob. 12 px ≈ a full desk width of empty floor between
/// pod-mates.
pub const INTRA_POD_GAP_X: u16 = 12;
pub const INTRA_POD_GAP_Y: u16 = 12;
/// Gap between adjacent pods — wider than the intra-pod gap so the pod
/// boundary stays visually distinct, while still hosting the rolling
/// whiteboard's 10-px GROUND footprint (the 14-px board panel overhangs it,
/// invariant #6) in the aisle. Tightened 28 → 22 to pack the 4-desk pods
/// denser (the office read too sparse — big empty aisles between clusters);
/// 22 px still clears the 10-px board + its 1-px pad. The walkable-connectivity
/// + decor-overlap + approach tests guard that the tighter aisle stays routable.
pub const INTER_POD_AISLE_X: u16 = 22;
pub const INTER_POD_AISLE_Y: u16 = 22;

impl SceneLayout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, num_agents: usize) -> Option<Self> {
        Self::compute_with_seed(buf_w, buf_h, num_agents, 0)
    }

    pub fn compute_with_seed(
        buf_w: u16,
        buf_h: u16,
        num_agents: usize,
        floor_seed: u64,
    ) -> Option<Self> {
        compute::compute_with_seed(buf_w, buf_h, num_agents, floor_seed)
    }

    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.walkable.is_walkable(x, y)
    }
}

#[cfg(test)]
mod tests;
