//! Zone-based scene layout for the top-down office — primitive geometry
//! only, no terminal deps.
//!
//! Splits a buf-pixel rectangle into quadrants (meeting / pantry /
//! cubicles / lounge), then computes per-agent home desks, named lounge
//! waypoints, decor positions, and a per-pixel walkability mask.

mod approach;
mod coarse;
mod compute;
mod decor;
mod mask;
mod placement;
mod reach;
mod rooms;

// The deep interface is `SceneLayout::{stand_point,approach_point}`; these free
// fns stay for this crate's own synthetic-mask unit tests.
pub(crate) use approach::{approach_point, first_reachable_on_side, stand_point};
pub use compute::PANTRY_COUNTER_LARGE_W;
pub(crate) use decor::repels_plants;
pub use decor::{
    desk_ceiling_pool_center, desk_furniture_def, desk_walk_anchor_facing, furniture_def,
    seated_foot_cell, ApproachSides, DwellWindow, Facing, Furniture, FurnitureDef, PlantKind,
    PodDecor, WallDecor, WaypointKind, DESK_APPROACH, SEAT_RENDER_Y_OFF, WALKING_Y_OFF,
};
pub use placement::{anchored_top_left, z_sort_row, Anchor};
pub use reach::ReachSet;
pub use rooms::{MeetingRoom, MeetingTrio, PantryRoom};
// Both SHARED with the pixel painter's `enqueue_room_walls_v`, so the blocked
// ground and the drawn glass meet the band / crossing walls at the same joints
// and over the same crossing-wall inputs.
pub(crate) use rooms::walls::{crossing_h_rows, stitch_vertical_wall};
pub use rooms::walls::{Doorway, WALL_THICK_H, WALL_THICK_V};
// `crate::pathfind`'s A* and `reach`'s BFS both ride these ONE definitions.
pub(crate) use coarse::{cell_walkable, snap, COARSE_CELL_SIZE, NEIGHBORS_8};

use pixtuoid_core::state::FloorLocalDeskIndex;
use pixtuoid_core::walkable::WalkableMask;

/// Primitive rectangle — same shape as `ratatui::layout::Rect` so the binary
/// converts field-by-field without core paying for the ratatui dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounds {
    /// Left edge x, in buffer pixels.
    pub x: u16,
    /// Top edge y, in buffer pixels.
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
}

/// A position in buffer-pixel space (screen-space: east = +x, south = +y,
/// north = −y = the buffer top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    /// Pixel column (east-positive).
    pub x: u16,
    /// Pixel row (south-positive; north is the buffer top).
    pub y: u16,
}

/// A width×height extent in pixels — named axes so a (w,h) tuple can't be
/// silently transposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size {
    /// Width in pixels.
    pub w: u16,
    /// Height in pixels.
    pub h: u16,
}

/// An interior room-wall segment — the two endpoints of a straight (horizontal
/// or vertical) wall run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WallSegment {
    /// One endpoint of the straight wall run (pixel-space).
    pub start: Point,
    /// The other endpoint (pixel-space).
    pub end: Point,
}

/// A placed plant: its kind paired with its centre position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlantItem {
    /// Which plant species/sprite.
    pub kind: PlantKind,
    /// Centre position in buffer pixels.
    pub pos: Point,
}

/// A placed wall decoration: its kind paired with its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallDecorItem {
    /// Which wall decoration.
    pub kind: WallDecor,
    /// Position in buffer pixels.
    pub pos: Point,
}

/// A placed aisle/pod decoration: its kind paired with its centre position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodDecorItem {
    /// Which aisle/pod decoration.
    pub kind: PodDecor,
    /// Centre position in buffer pixels.
    pub pos: Point,
}

/// A named stop an agent can walk to and occupy — a lounge seat, appliance,
/// or meeting-room slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waypoint {
    /// Where the occupant stands or sits (pixel-space).
    pub pos: Point,
    /// What kind of stop this is (seat, appliance, meeting slot, …).
    pub kind: WaypointKind,
    /// Direction the occupant faces here — `South` for the facing-neutral
    /// single-point waypoints, toward the table for meeting-room slots.
    pub facing: Facing,
    /// Meeting-room id this slot belongs to (`Some` for `MeetingSofa` /
    /// `MeetingChair`). Slots sharing a `room_id` form one group-chitchat venue.
    pub room_id: Option<usize>,
}

/// Backwards-compat alias for [`SceneLayout`].
pub type Layout = SceneLayout;

/// The lounge vignette placed as one unit. Couch + floor lamp + side table
/// share the one `lounge_fits` gate (hence non-optional here); the aquarium
/// carries an EXTRA east-clearance gate against the elevator door, so it
/// stays `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lounge {
    /// Centre of the 3-seat couch sprite — the sprite + rug + side table
    /// paint once, centred here.
    pub couch_center: Point,
    /// Floor lamp, just east of the couch.
    pub floor_lamp: Point,
    /// Side table on the couch's opposite (west) flank.
    pub side_table: Point,
    /// Aquarium centre, east of the lamp against the north wall band — `None`
    /// when the elevator-door east clearance fails.
    pub fish_tank: Option<Point>,
}

/// The computed office geometry for one floor — quadrant bounds, per-agent
/// desks, waypoints, decor, walls, and the walkability mask. Built once per
/// `(buf_w, buf_h, max_desks)`.
#[derive(Debug, Clone)]
pub struct SceneLayout {
    /// Buffer width in pixels this layout was computed for.
    pub buf_w: u16,
    /// Buffer height in pixels this layout was computed for.
    pub buf_h: u16,
    /// The desk-pod quadrant — the bounds enclosing the cubicle grid.
    pub cubicle_band: Bounds,
    /// The cubicle-band-width horizontal aisle at the bottom of the desk pods —
    /// the appliance-placement region (vending/printer). Keep it distinct from
    /// `corridor`: same y/height, different x-extent.
    pub cubicle_aisle: Bounds,
    /// Per-agent home-desk anchor positions, indexed floor-locally (read via
    /// [`Self::home_desk`]).
    pub home_desks: Vec<Point>,
    /// Facing per home desk, parallel to [`Self::home_desks`].
    pub desk_facings: Vec<Facing>,
    /// Named stops (lounge seats, appliances, meeting slots) agents walk to.
    pub waypoints: Vec<Waypoint>,
    /// Placed potted plants.
    pub plants: Vec<PlantItem>,
    /// Decorations mounted on the walls (whiteboards, TVs, exit signs).
    pub wall_decor: Vec<WallDecorItem>,
    /// Decor items placed in the aisles between 2×2 desk pods.
    pub pod_decor: Vec<PodDecorItem>,
    /// The lounge vignette as ONE unit — `None` when it doesn't fit. Read the
    /// individual pieces via the accessors ([`Self::couch_sprite_center`],
    /// [`Self::floor_lamp`], …).
    pub lounge: Option<Lounge>,
    /// The office entry-door position, or `None` if none fits.
    pub door: Option<Point>,
    /// The walkable cell just inside the door — the entry/exit waypoint.
    pub door_threshold: Option<Point>,
    /// Meeting rooms in floor order — the index IS the `room_id` every
    /// waypoint and painter joins on.
    pub meeting_rooms: Vec<MeetingRoom>,
    /// The pantry aggregate (bounds + counter footprint + island) — `None`
    /// on floors without a pantry (Dense dual-meeting).
    pub pantry: Option<PantryRoom>,
    /// Interior room-wall segments the painter draws and the mask blocks.
    pub room_walls: Vec<WallSegment>,
    /// The openings the wall resolver cut into `room_walls` — the painter
    /// draws door frames from these instead of re-inferring gaps from
    /// segment adjacency.
    pub doorways: Vec<Doorway>,
    /// Top offset in px reserved above the floor for the north wall+window
    /// band (and its carpet apron).
    pub top_margin: u16,
    /// The full-width horizontal corridor below the desk pods — the A\* router's
    /// preferred zone and the pet/mascot path; `None` when it doesn't fit.
    pub corridor: Option<Bounds>,
    /// Per-pixel walkability mask — the ground footprint every obstacle stamps,
    /// the surface routing runs over.
    pub walkable: WalkableMask,
    /// Coarse-cell reachable component (the walkable area an agent can A\*-route
    /// to) — consumed by `approach_point` to prefer a *reachable* approach side
    /// over a merely-walkable-but-walled-off one.
    pub reachable: ReachSet,
}

/// Integer percentage of `v`, floor semantics. Computed in u32: a bare
/// `buf_h * 30` overflows u16 once `buf_h > 2184`.
pub(crate) fn pct(v: u16, n: u16) -> u16 {
    ((v as u32 * n as u32) / 100) as u16
}

/// Padding (px) around every obstacle in the walkable mask, so characters
/// route AROUND furniture rather than scraping along its edge.
pub const OBSTACLE_PAD_PX: u16 = 2;

/// The SMALLER mask pad the waypoint (seat/appliance) stamp uses — a walkable
/// seat sits IN the open floor and needs no routing buffer. THE single source,
/// read by `mask.rs`'s waypoint stamp AND the #566 couch↔door clearance gate,
/// so the gate can never assume a different pad than the mask actually stamps.
pub(super) const WAYPOINT_STAMP_PAD_PX: u16 = 1;

/// The north wall+window band's visual bottom sits this many px ABOVE
/// `top_margin`; the rows in between render as carpet apron, not wall, so the
/// mask blocks only down to the band bottom. The renderer derives
/// `top_wall_h = top_margin - this`, so the mask and the visual MUST read this
/// one source or they drift.
pub const WALL_BAND_TO_TOP_MARGIN: u16 = 4;

/// How many pixels of the pantry counter actually sit on the floor: only the
/// southern base contacts the ground, the receding cabinet tops + backsplash
/// are overhang (invariant #6), so a character routed behind the counter is
/// occluded by its own y-sorted sprite.
pub const PANTRY_FOOTPRINT_DEPTH: u16 = 3;

/// The desk BODY size in SLOT units — the grid-pitch pricing. SLOT ≠ GROUND:
/// the desk's blocked GROUND is the full sprite width (`decor::DESK_GROUND_W`,
/// side cabinets included) and the overhang rides the aisle, so every band-EDGE
/// clamp reads `DESK_GROUND_W`, not `DESK_W` (the #549 2px-overflow drift).
pub const DESK_W: u16 = 10;
/// Rows of desk SURFACE below `desk.y`; both desk sprites are cut to it.
pub(crate) const DESK_SURFACE_ROWS: u16 = 5;
pub(crate) const DESK_FRONT_ROWS: u16 = 1;
pub(crate) const DESK_LEG_ROWS: u16 = 2;

/// Desk body height in SLOT units — the N-S pod pitch; the blocked ground is
/// only `DESK_FOOT_H` deep.
pub const DESK_H: u16 = 6;

const _: () = assert!(
    DESK_H + 2 == DESK_SURFACE_ROWS + DESK_FRONT_ROWS + DESK_LEG_ROWS,
    "the desk's VISUAL height (DESK_H + 2) must equal the rows its art draws: \
     surface + front + legs"
);
/// The desk's ground-CONTACT depth (rows) — only the front edge / legs touch
/// the floor; the surface + monitor OVERHANG north (`ground_y: End`), so a
/// walker passes BEHIND the monitor and is occluded by the desk's own y-sort
/// (invariant #6). Distinct from `DESK_H`, which prices the slot.
pub(crate) const DESK_FOOT_H: u16 = 2;
/// Default character sprite width (px) — the ONE authority every
/// out-of-pixel_painter consumer centers/hit-tests on. Sprite BLIT sites still
/// pass the pack's REAL `frame.width`; this is the width-unknown fallback.
/// Lives in `layout` so `layout::decor` can read it without a module cycle.
pub const CHARACTER_SPRITE_W: u16 = 8;
/// Default character sprite height in terminal CELLS — used by the tui hit-test
/// pin box; the pixel pose offsets are a SEPARATE vertical-anchor concern.
pub const CHARACTER_SPRITE_H_CELLS: u16 = 6;
/// Elevator-door sprite width in buffer px. Both the layout and the renderer
/// read this, so the door footprint can't drift between them.
pub const ELEVATOR_W: u16 = 16;
/// Elevator-door sprite height in buffer px — the door's z-sort anchor row.
pub const ELEVATOR_H: u16 = 14;
/// NOT a cap — production layouts fill the buffer's physical space
/// (`max_desks: None`). This is the stable "one classic office worth of desks"
/// reference, and the `snapshot` example that renders the docs/CI media
/// baselines pins its scene to it.
pub const CLASSIC_OFFICE_DESKS: usize = 16;
/// Test-facing alias for [`CLASSIC_OFFICE_DESKS`] — the named default
/// deterministic tests/snapshots pass as `Some(TEST_DEFAULT_DESKS)`.
pub const TEST_DEFAULT_DESKS: usize = CLASSIC_OFFICE_DESKS;
/// Minimum horizontal gap (px) flanking the desk grid — sizes `MIN_LAYOUT_W`
/// (`DESK_W` plus one gap on each side).
pub const DESK_GAP_X: u16 = 11;
/// The N-S counterpart to [`DESK_GAP_X`] — the desk-grid vertical gap unit (px).
pub const DESK_GAP_Y: u16 = 14;
/// Floor (px) for the layout's `top_margin` — the north wall band never
/// shrinks below this (`top_margin = max(30% of buffer height, this)`).
pub const MIN_TOP_MARGIN: u16 = 20;
const MIN_DUAL_MEETING_H: u16 = 80;

/// Number of desks per side in a pod (`POD_SIDE * POD_SIDE` total).
pub const POD_SIDE: u16 = 2;
/// Gap between two desks inside the same pod — big enough that each desk
/// reads as its own workstation, not a merged blob.
pub const INTRA_POD_GAP_X: u16 = 12;
/// N-S gap between the two desks stacked in one pod (vertical counterpart to
/// [`INTRA_POD_GAP_X`]); sets the pod's inner height. Rows step by
/// `DESK_H + this`, and that STEP must stay EVEN or the pod's two rows land on
/// different half-block parities — so retuning either side needs both checked.
pub const INTRA_POD_GAP_Y: u16 = 6;
const _: () = assert!((DESK_H + INTRA_POD_GAP_Y).is_multiple_of(2));
/// Horizontal (E-W) gap between adjacent pod COLUMNS — wide enough to keep the
/// pod boundary visually distinct AND to host the rolling whiteboard's GROUND
/// footprint in the aisle. Deliberately > the N-S gap: screens are landscape,
/// so spread wider horizontally and pack tighter vertically.
pub const INTER_POD_AISLE_X: u16 = 20;
/// Vertical (N-S) gap between adjacent pod ROWS. INTENTIONALLY < the E-W gap
/// (landscape screens — see `INTER_POD_AISLE_X`). Shrinking it breaks
/// `every_home_desk_has_a_reachable_approach_on_its_own_far_side`: the seat's
/// far-side approach cell collides with the desk in the row above.
pub const INTER_POD_AISLE_Y: u16 = 18;

impl SceneLayout {
    /// Returns `None` if the buffer is too small for even one cubicle plus the
    /// fixed lounge area — the caller paints a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, max_desks: Option<usize>) -> Option<Self> {
        Self::compute_with_seed(buf_w, buf_h, max_desks, 0)
    }

    /// `max_desks` caps the desk count: `None` fills the office to the buffer's
    /// physical capacity, `Some(n)` caps at `n` for deterministic
    /// tests/snapshots. The pod grid geometry is always the room's true
    /// capacity regardless of the cap.
    pub fn compute_with_seed(
        buf_w: u16,
        buf_h: u16,
        max_desks: Option<usize>,
        floor_seed: u64,
    ) -> Option<Self> {
        compute::compute_with_seed(buf_w, buf_h, max_desks, floor_seed)
    }

    /// Is buffer pixel `(x, y)` walkable?
    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.walkable.is_walkable(x, y)
    }

    /// Typed accessor for a floor's home-desk anchor. `home_desks` is a
    /// FLOOR-LOCAL vector — index it through a `FloorLocalDeskIndex`, never
    /// with an `AgentSlot.desk_index` directly.
    pub fn home_desk(&self, i: FloorLocalDeskIndex) -> Option<Point> {
        self.home_desks.get(i.0).copied()
    }

    /// Is `p` clear of the furniture sprites that PAINT OVER it? Walkable is the
    /// GROUND rule (invariant #6), so the cell in front of a desk is legitimately
    /// walkable AND legitimately covered — fine to walk THROUGH, wrong to park in.
    ///
    /// Destructured with NO `..`, the same guarantee `placement_sweep::pieces`
    /// takes: a new collection is a compile error HERE, not a finding two review
    /// rounds later. Three kinds carry a `0x0` table `visual` because their sprite
    /// is runtime-sized, so each is read from its own authority below.
    pub(crate) fn is_visually_clear(&self, p: Point) -> bool {
        let SceneLayout {
            home_desks,
            waypoints,
            plants,
            pod_decor,
            wall_decor,
            lounge,
            meeting_rooms,
            pantry,
            desk_facings: _, // a desk ATTRIBUTE, not a sprite
            room_walls: _,   // translucent glass; a creature behind it still reads
            door: _,         // architecture, and the band it punches is not walkable
            door_threshold: _,
            doorways: _,
            corridor: _,     // a zone, not a sprite
            cubicle_band: _, // containers
            cubicle_aisle: _,
            buf_w: _,
            buf_h: _,
            top_margin: _,
            walkable: _,
            reachable: _,
        } = self;
        let inside = |tl: Point, sz: Size| {
            p.x >= tl.x && p.x < tl.x + sz.w && p.y >= tl.y && p.y < tl.y + sz.h
        };
        let covered = |anchor: Anchor, pos: Point, kind: Furniture| {
            let (tl, sz) = furniture_def(kind).visual_rect(anchor, pos);
            inside(tl, sz)
        };
        let table = home_desks
            .iter()
            .any(|&d| covered(Anchor::TopLeft, d, Furniture::Desk))
            || waypoints
                .iter()
                .any(|w| covered(Anchor::Center, w.pos, w.kind.furniture()))
            || plants
                .iter()
                .any(|pl| covered(Anchor::Center, pl.pos, pl.kind.furniture()))
            || pod_decor
                .iter()
                .any(|d| covered(Anchor::Center, d.pos, d.kind.furniture()))
            // Wall decor is NOT out as a class: the whiteboard is free-standing
            // floor furniture standing in an inter-pod aisle.
            || wall_decor
                .iter()
                .any(|d| covered(Anchor::TopLeft, d.pos, d.kind.furniture()));
        let lounge = lounge.is_some_and(|l| {
            covered(Anchor::Center, l.couch_center, Furniture::Couch)
                || covered(Anchor::Center, l.floor_lamp, Furniture::FloorLamp)
                || covered(Anchor::Center, l.side_table, Furniture::LoungeSideTable)
                || l.fish_tank
                    .is_some_and(|t| covered(Anchor::Center, t, Furniture::FishTank))
        });
        let runtime = waypoints.iter().any(|w| {
            w.kind == WaypointKind::Pantry && {
                let sz = self.pantry_counter_size();
                inside(
                    placement::anchored_top_left(Anchor::Center, w.pos, sz.w, sz.h),
                    sz,
                )
            }
        }) || meeting_rooms.iter().any(|r| {
            r.trio.is_some_and(|tr| {
                tr.sofas
                    .iter()
                    .any(|&s| covered(Anchor::Center, s, Furniture::MeetingSofaBody))
                    || covered(Anchor::Center, tr.table, Furniture::MeetingTable)
            })
        }) || pantry
            .and_then(|pa| pa.kitchen_island)
            .is_some_and(|i| covered(Anchor::Center, i, Furniture::KitchenIsland));
        !(table || lounge || runtime)
    }

    /// Which way the desk AT `pos` seats its occupant (an O(desks) scan).
    pub fn desk_facing_at(&self, pos: Point) -> Facing {
        self.home_desks
            .iter()
            .position(|&d| d == pos)
            .map_or(Facing::South, |i| self.desk_facing(FloorLocalDeskIndex(i)))
    }

    /// Which way desk `i`'s occupant faces — the ONE authority painters and the
    /// approach/walk geometry share, so a seat is never drawn off its routed side.
    pub fn desk_facing(&self, i: FloorLocalDeskIndex) -> Facing {
        self.desk_facings.get(i.0).copied().unwrap_or(Facing::South)
    }

    /// The visible top window-wall band height in px (`compute` names the same
    /// quantity `top_wall_h`). Post-construction render sites read it here so
    /// the derivation lives once.
    pub fn wall_band_h(&self) -> u16 {
        self.top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN)
    }

    /// The bounds of meeting room `room_id` — the id IS the
    /// [`Self::meeting_rooms`] index.
    pub fn meeting_room_bounds(&self, room_id: usize) -> Option<Bounds> {
        self.meeting_rooms.get(room_id).map(|r| r.bounds)
    }

    /// Couch sprite centre (middle of the 3 seats) — `Some` iff the lounge
    /// vignette fits.
    pub fn couch_sprite_center(&self) -> Option<Point> {
        self.lounge.as_ref().map(|l| l.couch_center)
    }

    /// The lounge floor lamp — `Some` iff the vignette fits.
    pub fn floor_lamp(&self) -> Option<Point> {
        self.lounge.as_ref().map(|l| l.floor_lamp)
    }

    /// The lounge side table — `Some` iff the vignette fits.
    pub fn lounge_side_table(&self) -> Option<Point> {
        self.lounge.as_ref().map(|l| l.side_table)
    }

    /// The aquarium centre — `Some` only when the vignette fits AND the
    /// east-clearance gate against the elevator door passes.
    pub fn fish_tank(&self) -> Option<Point> {
        self.lounge.as_ref().and_then(|l| l.fish_tank)
    }

    /// The pantry counter's footprint, or the `rooms::pantry::COMPACT_COUNTER`
    /// fallback when no pantry exists — `approach_point`'s signature needs SOME
    /// size even on pantry-less floors, where it is never consulted.
    pub fn pantry_counter_size(&self) -> Size {
        self.pantry
            .map_or(rooms::pantry::COMPACT_COUNTER, |p| p.counter_size)
    }

    /// Where an agent's sprite RENDERS when it visits furniture `kind` at `pos`
    /// (the walk goal for an obstacle, the seat cell for a seat), on the side
    /// nearest `origin` facing `facing`.
    pub fn stand_point(
        &self,
        kind: WaypointKind,
        pos: Point,
        origin: Point,
        facing: Facing,
    ) -> Point {
        stand_point(
            kind,
            pos,
            self.pantry_counter_size(),
            &self.walkable,
            origin,
            facing,
            &self.reachable,
        )
    }

    /// A\*'s goal cell when an agent at `origin` visits furniture `kind` at
    /// `pos` facing `facing`. Callers MUST honor its `== pos` "no valid
    /// approach" sentinel — skip the furniture this cycle rather than route to it.
    pub fn approach_point(
        &self,
        kind: Furniture,
        pos: Point,
        origin: Point,
        facing: Facing,
    ) -> Point {
        approach_point(
            kind,
            pos,
            facing,
            self.pantry_counter_size(),
            &self.walkable,
            origin,
            &self.reachable,
        )
    }
}

#[cfg(test)]
mod placement_sweep;
#[cfg(test)]
mod tests;
