//! Zone-based scene layout for the top-down office — primitive geometry
//! only, no terminal deps. Computed once per (buf_w, buf_h, num_agents)
//! triple; serializable / wire-shippable (no out-of-process consumer today).
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
//!   * `coarse` — the SHARED coarse routing-grid primitives (`cell_walkable`/`snap`/
//!     `NEIGHBORS_8`/`COARSE_CELL_SIZE`) that BOTH `reach` and `crate::pathfind` ride.
//!   * `reach` — `ReachSet`: coarse-cell BFS (over `coarse`) mirroring `crate::pathfind`'s A* grid.

mod approach;
mod coarse;
mod compute;
mod decor;
mod mask;
mod placement;
mod reach;
mod rooms;

pub use approach::{approach_point, stand_point};
pub use compute::PANTRY_COUNTER_LARGE_W;
pub(crate) use decor::DESK_GROUND_H;
pub use decor::{
    desk_furniture_def, desk_walk_anchor, furniture_def, seated_foot_cell, ApproachSides,
    DwellWindow, Facing, Furniture, FurnitureDef, PlantKind, PodDecor, WallDecor, WaypointKind,
    DESK_APPROACH, SEAT_RENDER_Y_OFF, WALKING_Y_OFF,
};
pub use placement::{anchored_top_left, z_sort_row, Anchor};
pub use reach::ReachSet;
pub use rooms::{MeetingRoom, MeetingTrio, PantryRoom};
// Wall geometry half (`rooms::walls`): the thickness consts + `Doorway`, plus
// the vertical-wall joint solver `stitch_vertical_wall` and its shared input
// `crossing_h_rows` — the last two SHARED with the pixel painter's
// `enqueue_room_walls_v`, so the blocked ground and the drawn glass meet the band
// / crossing walls at the same joints AND over the same crossing-wall inputs
// (cross-module, not cross-crate, hence pub(crate)). Kept as its own
// comment-delimited group so rustfmt's import sort can't orphan this note.
pub(crate) use rooms::walls::{crossing_h_rows, stitch_vertical_wall};
pub use rooms::walls::{Doorway, WALL_THICK_H, WALL_THICK_V};
// The shared coarse routing-grid primitives (crate-internal — no semver surface):
// `crate::pathfind`'s A* and `reach`'s BFS both ride these ONE definitions.
pub(crate) use coarse::{cell_walkable, snap, COARSE_CELL_SIZE, NEIGHBORS_8};

use pixtuoid_core::state::FloorLocalDeskIndex;
use pixtuoid_core::walkable::WalkableMask;

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
    /// `MeetingSofa` / `MeetingChair`, `None` otherwise). Slots sharing a
    /// `room_id` form one group-chitchat venue.
    pub room_id: Option<usize>,
}

/// Backwards-compat alias — existing call sites construct `Layout::compute()`
/// (the pre-move façade name this crate re-exported `SceneLayout` under).
pub type Layout = SceneLayout;

#[derive(Debug, Clone)]
pub struct SceneLayout {
    pub buf_w: u16,
    pub buf_h: u16,
    pub cubicle_band: Bounds,
    /// The cubicle-band-width horizontal aisle at the bottom of the desk pods
    /// (x = the cubicle columns' extent). This is the appliance-placement region
    /// (vending/printer). NOT the full-width `corridor` below — that one (widened
    /// to the whole buffer) is the A\* router's preferred zone + the pet/mascot
    /// path. Keep the two distinct: same y/height, different x-extent.
    pub cubicle_aisle: Bounds,
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
    /// Aquarium center, east of the floor lamp against the north wall band.
    /// Rides the lounge gate (couch/lamp/table/tank live and die together)
    /// plus an east-clearance gate against the elevator door; `None` when
    /// either fails.
    pub fish_tank: Option<Point>,
    pub door: Option<Point>,
    pub door_threshold: Option<Point>,
    /// Meeting rooms in floor order — index IS the `room_id` every waypoint
    /// and painter joins on (room 1 exists only on the dual-meeting Dense
    /// floor). Each element carries the room's bounds AND its trio (see
    /// [`MeetingRoom`] for why the two live in one element). Join through
    /// [`Self::meeting_room_bounds`] or index directly.
    pub meeting_rooms: Vec<MeetingRoom>,
    /// The pantry aggregate (bounds + counter footprint + island) — `None`
    /// on floors without a pantry (Dense dual-meeting).
    pub pantry: Option<PantryRoom>,
    pub room_walls: Vec<WallSegment>,
    /// The openings the wall resolver cut into `room_walls` — the painter
    /// draws door frames from these instead of re-inferring gaps from
    /// segment adjacency (the resolver is the one place that knows every
    /// door; see `rooms/walls.rs`).
    pub doorways: Vec<Doorway>,
    pub top_margin: u16,
    pub corridor: Option<Bounds>,
    /// Centre point of the lounge couch sprite (the middle of its 3 seats).
    /// The couch is 3 separate seat waypoints; the sprite + rug + side table
    /// paint once, centred here. `None` when no couch fits.
    pub couch_sprite_center: Option<Point>,
    pub walkable: WalkableMask,
    /// Coarse-cell reachable component (the walkable area an agent can A\*-route
    /// to). Computed once from a known in-component seed; consumed by
    /// `approach_point` to prefer a *reachable* approach side over a merely-
    /// walkable-but-walled-off one. Mirrors `crate::pathfind`'s coarsening.
    pub reachable: ReachSet,
}

/// Integer percentage of `v` (floor semantics — `pct(40, 65) == 26`); the
/// layout code's one percent helper, shared by placement (`compute`) and the
/// wall resolver so their arithmetic can't diverge. Computed in u32: a bare
/// `buf_h * 30` overflows u16 once `buf_h > 2184`.
pub(crate) fn pct(v: u16, n: u16) -> u16 {
    ((v as u32 * n as u32) / 100) as u16
}

/// Padding (in pixels) added around every obstacle when building the
/// walkable mask. Reserves a buffer zone so characters route AROUND
/// furniture rather than scraping along its edge.
pub const OBSTACLE_PAD_PX: u16 = 2;

/// The SMALLER mask pad the waypoint (seat/appliance) stamp uses — a walkable
/// seat/venue sits IN the open floor and only needs a 1px cushion, not the full
/// `OBSTACLE_PAD_PX` routing buffer. THE single source for that pad, read by
/// `mask.rs`'s waypoint stamp AND the #566 couch↔door clearance gate
/// (`compute.rs`), so the gate can never assume a different pad than the mask
/// actually stamps.
pub(super) const WAYPOINT_STAMP_PAD_PX: u16 = 1;

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

/// The desk BODY size in SLOT units — the grid-pitch pricing (pod stride,
/// intra-pod gaps) counts `DESK_W`×`DESK_H`, and the sprite/visual is
/// `DESK_W+4` wide × `DESK_H+2` tall. SLOT ≠ GROUND: the desk's blocked
/// GROUND is the full `DESK_W+4`-px sprite width ([`decor::DESK_GROUND_W`],
/// side cabinets included) — the +4 overhang rides the aisle, so every
/// band-EDGE clamp reads `DESK_GROUND_W`, not `DESK_W` (the #549 2px-overflow
/// drift).
pub const DESK_W: u16 = 10;
pub const DESK_H: u16 = 5;
/// The desk's ground-CONTACT depth (rows) — only the front edge / legs touch
/// the floor; the surface + monitor OVERHANG north (`ground_y: End`), so a
/// walker passes BEHIND the monitor and is occluded by the desk's own y-sort
/// (invariant #6, the plant-canopy pattern applied to the desk — owner
/// taste-picked "h2" from the shallow-footprint renders). Distinct from
/// `DESK_H` (the body/pitch height): `DESK_H` prices the slot, `DESK_FOOT_H`
/// is the real blocked ground depth. The 5-px body still z-sorts by the full
/// `visual.h`, so the monitor paints over the walker behind it.
/// `pub(crate)`: no cross-crate consumer (unlike `DESK_W`/`DESK_H`, which the
/// binary's hit-test reads) — least-privilege on the semver surface.
pub(crate) const DESK_FOOT_H: u16 = 2;
/// Default character sprite width (px). The bundled pack is 8×12; this is the
/// ONE authority every out-of-pixel_painter consumer centers/hit-tests on
/// (anchors' LABEL fallback, `layout::decor::DESK_WALK_X_OFF`, the tui hit-test
/// pin box, the floating label centering) — a bare `8` copied into those sites
/// drifts from the painted sprite the moment the pack width changes. The sprite
/// BLIT sites still pass the pack's REAL `frame.width` (a custom pack may be
/// wider, e.g. the robot pack's 10); this const is the width-unknown fallback.
/// Lives in `layout` (not `pixel_painter`) so `layout::decor` can read it
/// without a module cycle. Pinned to the embedded pack by
/// `character_sprite_w_matches_the_embedded_pack`.
pub const CHARACTER_SPRITE_W: u16 = 8;
/// Default character sprite height in terminal CELLS (the 12 px sprite is 6
/// half-block rows). Used by the tui hit-test pin box (cell space); the pixel
/// pose offsets (8/12/7 px) are a SEPARATE vertical-anchor concern, NOT this.
pub const CHARACTER_SPRITE_H_CELLS: u16 = 6;
/// Elevator-door sprite size in buffer px — the single source for the door's
/// width (the layout slots the sprite into the back wall and the renderer skips
/// the window glass it covers) and height (the z-sort anchor row). Both the
/// layout (`compute`) and the renderer (`pixel_painter` / `background`) read
/// these so the door footprint can't drift between them.
pub const ELEVATOR_W: u16 = 16;
pub const ELEVATOR_H: u16 = 14;
/// NOT a cap anymore — production layouts fill the buffer's physical space
/// (`compute_with_seed(.., max_desks: None, ..)`), so desk count scales with
/// the canvas. This is the historical 16-desk ceiling kept as a stable "one
/// classic office worth of desks" reference, not a limit the layout enforces.
/// It is a load-bearing PRODUCTION input too (the `snapshot` example that
/// renders the docs/CI media baselines pins its scene to it), hence the
/// production name; `TEST_DEFAULT_DESKS` below is the test-facing alias.
pub const CLASSIC_OFFICE_DESKS: usize = 16;
/// Test-facing alias for [`CLASSIC_OFFICE_DESKS`] — the NAMED DEFAULT
/// deterministic tests/snapshots pass as `Some(TEST_DEFAULT_DESKS)`. Same
/// value by definition; production consumers use the production name.
/// (Published as `MAX_VISIBLE_DESKS` through 0.11.x; that deprecated alias was
/// dropped at 0.12.0 exactly as its own comment scheduled — don't re-add it.)
pub const TEST_DEFAULT_DESKS: usize = CLASSIC_OFFICE_DESKS;
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
/// Horizontal (E-W) gap between adjacent pod COLUMNS — wider than the
/// intra-pod gap so the pod boundary stays visually distinct, while hosting
/// the rolling whiteboard's 10-px GROUND footprint (the 14-px board panel
/// overhangs it, invariant #6) in the aisle. Deliberately > the N-S gap:
/// screens are landscape, so spread wider horizontally (where there's room)
/// and pack tighter vertically. 20 clears the 10-px board + pads. The
/// walkable-connectivity + decor-overlap + approach tests guard routability.
pub const INTER_POD_AISLE_X: u16 = 20;
/// Vertical (N-S) gap between adjacent pod ROWS. INTENTIONALLY < the E-W
/// gap (landscape screens — see `INTER_POD_AISLE_X`). The floor USED to be
/// EXACTLY 20 (18 AND 19 broke `every_home_desk_has_a_reachable_north_approach`:
/// the seat's north approach cell collided with the full-body desk in the row
/// above). The walk-behind change RELAXED it — the desk's shallow
/// `DESK_FOOT_H` footprint (`ground_y: End`) freed the monitor/north zone the
/// approach lands in, dropping the floor to 16 (18/16 pass, 14 breaks). 18
/// keeps a 2-px margin above the floor.
pub const INTER_POD_AISLE_Y: u16 = 18;

impl SceneLayout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, max_desks: Option<usize>) -> Option<Self> {
        Self::compute_with_seed(buf_w, buf_h, max_desks, 0)
    }

    /// `max_desks` caps the desk count: `None` fills the office to the buffer's
    /// physical capacity (production — the office scales to the canvas), while
    /// `Some(n)` caps at `n` desks for deterministic tests/snapshots. The pod
    /// grid geometry is always the room's true capacity regardless of the cap.
    pub fn compute_with_seed(
        buf_w: u16,
        buf_h: u16,
        max_desks: Option<usize>,
        floor_seed: u64,
    ) -> Option<Self> {
        compute::compute_with_seed(buf_w, buf_h, max_desks, floor_seed)
    }

    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.walkable.is_walkable(x, y)
    }

    /// Typed accessor for a floor's home-desk anchor. `home_desks` is a
    /// FLOOR-LOCAL vector — index it through a `FloorLocalDeskIndex`
    /// (from `SceneState::floor_local_desk`, or
    /// `GlobalDeskIndex::single_floor_local` inside a single-floor
    /// projected scene), never with an `AgentSlot.desk_index` directly.
    /// Raw `home_desks[i]` with a loop/iteration `usize` stays fine.
    pub fn home_desk(&self, i: FloorLocalDeskIndex) -> Option<Point> {
        self.home_desks.get(i.0).copied()
    }

    /// The visible top window-wall band height in px — the wall strip between the
    /// buffer top and where the floor begins, `top_margin - WALL_BAND_TO_TOP_MARGIN`
    /// (the same quantity `compute` names `top_wall_h` at construction; saturates
    /// to 0 on a degenerate tiny margin). Post-construction render sites (wall sun
    /// spot, window spill, weather) read it here so the derivation lives once.
    pub fn wall_band_h(&self) -> u16 {
        self.top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN)
    }

    /// The Bounds of meeting room `room_id` — a thin index into
    /// [`Self::meeting_rooms`] (the id IS the Vec index), kept as the join
    /// accessor so consumers don't hand-roll the lookup.
    pub fn meeting_room_bounds(&self, room_id: usize) -> Option<Bounds> {
        self.meeting_rooms.get(room_id).map(|r| r.bounds)
    }

    /// The pantry counter's footprint, or the [`rooms::pantry::COMPACT_COUNTER`]
    /// fallback when no pantry exists — the shape every consumer of the old
    /// always-present `pantry_counter_size` field expects (the runtime-sized
    /// counter must resolve to SOME size for `approach_point`'s signature
    /// even on pantry-less floors, where it is never consulted).
    pub fn pantry_counter_size(&self) -> Size {
        self.pantry
            .map_or(rooms::pantry::COMPACT_COUNTER, |p| p.counter_size)
    }
}

#[cfg(test)]
mod placement_sweep;
#[cfg(test)]
mod tests;
