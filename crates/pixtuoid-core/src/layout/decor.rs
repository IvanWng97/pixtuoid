//! Decor vocabulary used by `SceneLayout` — the enums describing every
//! piece of furniture and waypoint kind in the office. Kept separate from
//! geometry so adding a new sprite kind doesn't churn the layout math.

use super::{Point, DESK_H, DESK_W};

/// Wander destinations the Idle state machine can pick. Each kind controls
/// the pose + sprite an arriving agent takes. Plants/lamps are decor, not
/// waypoints. Coffee folded into Pantry — the pantry sprite already has
/// a coffee machine on its counter, so visiting the pantry covers both
/// "kitchen" and "coffee break".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaypointKind {
    /// Top-of-cubicle viewing couch facing the city windows.
    Couch,
    /// Pantry counter — kitchen + coffee.
    Pantry,
    /// Aisle phone booth — agent stands at the door (private call).
    PhoneBooth,
    /// Aisle standing desk — agent stands at the desk (alternate
    /// workstation). Random which exact StandingDesk slot is used.
    StandingDesk,
    /// Corridor vending machine — agent stands in front to grab a drink.
    VendingMachine,
    /// Corridor printer — agent stands in front while "printing."
    Printer,
    /// Meeting-room sofa seat — agent sits, facing the table. Multiple
    /// seats per sofa; a group conversation runs when ≥2 share the room.
    MeetingSofa,
    /// Meeting-room standing spot beside the table — agent stands, facing
    /// the table. Part of the same room conversation venue as MeetingSofa.
    MeetingStand,
}

/// The booth/desk dimensions — named so each appears once as BOTH the
/// `footprint` and `visual` of its single [`Furniture`] row (these two kinds are
/// box-like: ground base == sprite). Reached via either `WaypointKind` (wander
/// destination) or `PodDecor` (aisle decor), but it's one row, so nothing drifts.
pub(crate) const PHONE_BOOTH_FOOTPRINT: (u16, u16) = (6, 12);
pub(crate) const STANDING_DESK_FOOTPRINT: (u16, u16) = (8, 8);

/// Plant GROUND footprint — the one geometry VALUE shared by all four plant
/// rows in [`furniture_def`], named here so it's declared once instead of
/// duplicated across the rows. Deliberately tighter than the taller VISUAL
/// sprite (top-down rule: the leaves overhang the pot's ground base). Read only
/// THROUGH the table (`furniture_def(_).footprint`), never directly.
pub(crate) const PLANT_FOOTPRINT: (u16, u16) = (6, 6);

/// Which sides an agent may approach a piece of furniture from, in the
/// CANONICAL frame (furniture facing South, toward the viewer). [`Self::allows`]
/// rotates this to the live `facing`, so one stored set works for
/// variable-facing furniture (a sofa's "front + sides, no back" rotates to the
/// correct absolute sides whether it faces north or south). **To add/remove an
/// entry side, flip one bool** — single place, greppable. `n`/`s`/`e`/`w` are
/// the canonical absolute sides (north = −y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproachSides {
    pub n: bool,
    pub s: bool,
    pub e: bool,
    pub w: bool,
}

impl ApproachSides {
    /// 360° — approachable from every open side (pantry counter).
    pub const ALL: Self = Self {
        n: true,
        s: true,
        e: true,
        w: true,
    };

    /// This canonical (facing-South) set rotated to the live `facing`. South is
    /// the canonical front, so e.g. a "no back" set (front+sides) rotates to
    /// exclude whichever absolute side is now the back.
    pub fn rotated(self, facing: Facing) -> Self {
        let s = self;
        match facing {
            Facing::South => s,
            Facing::North => Self {
                n: s.s,
                s: s.n,
                e: s.w,
                w: s.e,
            },
            Facing::East => Self {
                n: s.e,
                s: s.w,
                e: s.s,
                w: s.n,
            },
            Facing::West => Self {
                n: s.w,
                s: s.e,
                e: s.n,
                w: s.s,
            },
        }
    }

    /// Is the absolute unit dir `(dx, dy)` (north = (0,−1), south = (0,1),
    /// east = (1,0), west = (−1,0)) an allowed approach under the live `facing`?
    pub fn allows(self, facing: Facing, dir: (i32, i32)) -> bool {
        let r = self.rotated(facing);
        match dir {
            (0, -1) => r.n,
            (0, 1) => r.s,
            (1, 0) => r.e,
            (-1, 0) => r.w,
            _ => false,
        }
    }
}

/// Approach sides for the home desk (the assigned workstation — NOT a
/// `furniture_def` row). Canonical: exclude the south front (the monitor faces
/// the viewer; the agent sits behind it), so reachable from N/E/W. Editing one
/// bool here changes the home-desk entry sides (e.g. drop east → `e: false`).
pub const DESK_APPROACH: ApproachSides = ApproachSides {
    n: true,
    s: false,
    e: true,
    w: true,
};

/// Definition record for a waypoint-addressable furniture kind — the single
/// source of truth for its ground shape, occupancy semantics, and dwell.
/// Reshaping a piece of furniture is editing ONE row of [`furniture_def`];
/// the walkable mask, stand-point, hit-test hitbox, and the render depth
/// baseline all DERIVE from these fields, so they cannot drift. Render-only
/// choices (sprite name, back-cap policy) deliberately stay in the tui crate
/// — `pixtuoid-core` has no terminal deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureDef {
    /// Ground footprint `(w, h)` the walkable mask stamps (top-down z=0
    /// rect), or `None` for slots that add no obstacle of their own
    /// (MeetingSofa/MeetingStand sit on sofa/table furniture stamped
    /// elsewhere). NB: `Pantry` is also `None` here because its footprint is
    /// runtime-sized (`pantry_counter_size`); `obstacle_footprint`
    /// special-cases it — the one kind whose shape isn't a static literal.
    pub footprint: Option<(u16, u16)>,
    /// Visual sprite size `(w, h)` in buffer px — the SECOND geometry axis,
    /// kept distinct from `footprint` (the top-down ground rule, invariant #6):
    /// a sprite legitimately overhangs its ground base (a tall plant's leaves, a
    /// floor lamp's shade). Render centering + the z-sort south row derive from
    /// this; the mask derives from `footprint`. Conflating the two is exactly the
    /// canopy-over-aisle bug this split prevents. For furniture rendered
    /// procedurally with its own anchors (couch / pantry counter / meeting sofa)
    /// this is unused — set to the footprint or `(0, 0)`.
    pub visual: (u16, u16),
    /// The agent occupies `pos` DIRECTLY (sprite renders ON the furniture),
    /// so `stand_point` passes `pos` through unchanged instead of resolving a
    /// walkable cell beside the furniture (A* then snaps the walk adjacent).
    /// NOT "a human can sit here": `MeetingStand` is *standing* yet sets this
    /// true (the agent still occupies its `pos`). Opposite case (Pantry/
    /// vending/printer/phone-booth/standing-desk): `pos` = blocked obstacle
    /// CENTER, approached from a side. True set: {Couch, MeetingSofa,
    /// MeetingStand}. (Desks are NOT rows here — home workstation is separate.)
    pub occupies_pos: bool,
    /// Behind-occlusion DEPTH. `Some(px)` if a character standing or passing
    /// BEHIND (north of) this object is occluded by it — `px` is the height of
    /// the "back face" the renderer extrudes north of the sprite, i.e. HOW MUCH
    /// of the character's body is hidden, scaled to the object's tallness (phone
    /// booth 6, counter 5, vending/standing-desk 4, printer 2). The body reads as
    /// *behind* the object (¾-view depth) — the same overlap the sofa back gives
    /// its sitter. `None` for flat / wall-flush decor (plants, TV / whiteboard
    /// panels, wall art) and for `occupies_pos` seats (their sitter is occluded
    /// by the seat's own y-sort, not a back face). Both the depth AND the on/off
    /// live here so a new furniture row can't silently forget behind-occlusion —
    /// every row declares it and the policy test asserts the exact values.
    pub occludes_behind: Option<u16>,
    /// Per-spot idle dwell window `(base_ms, range_ms)`. `range == 0` (the
    /// `DECOR` rows) marks a kind that is NOT a wander destination and is never
    /// fed to `pose::dwell_ms`; `range > 0` marks a destination. `dwell_ms`
    /// guards with `% range.max(1)`, so a zero range is safe — it IS the decor
    /// sentinel, not a bug. Do not "fix" a decor row to a non-zero range (that
    /// silently turns it into a wander destination).
    pub dwell: (u64, u64),
    /// Canonical (facing-South) sides an agent may approach from. Obstacle
    /// furniture against walls keeps `ALL` (walls already constrain the open
    /// side); seats use "front + sides, no back" so a walker never paths in
    /// through the sofa back. Edit one bool to change an entry side.
    pub approach: ApproachSides,
}

/// Canonical seat approach: front + sides, exclude the back. Rotates with
/// facing so a north- or south-facing sofa each exclude their own back.
const SEAT_APPROACH: ApproachSides = ApproachSides {
    n: false,
    s: true,
    e: true,
    w: true,
};

impl WaypointKind {
    /// Every variant, for exhaustive invariant tests (mirrors
    /// [`PodDecor::ALL`]). Iteration-only — order is not load-bearing.
    pub const ALL: &'static [WaypointKind] = &[
        WaypointKind::Couch,
        WaypointKind::Pantry,
        WaypointKind::PhoneBooth,
        WaypointKind::StandingDesk,
        WaypointKind::VendingMachine,
        WaypointKind::Printer,
        WaypointKind::MeetingSofa,
        WaypointKind::MeetingStand,
    ];

    /// This waypoint's geometry kind in the unified [`Furniture`] table. The
    /// waypoint enum carries ROLE (a wander destination); geometry lives in the
    /// one table so it can't drift from the decor/render side.
    pub const fn furniture(self) -> Furniture {
        match self {
            WaypointKind::Couch => Furniture::Couch,
            WaypointKind::Pantry => Furniture::Pantry,
            WaypointKind::PhoneBooth => Furniture::PhoneBooth,
            WaypointKind::StandingDesk => Furniture::StandingDesk,
            WaypointKind::VendingMachine => Furniture::VendingMachine,
            WaypointKind::Printer => Furniture::Printer,
            WaypointKind::MeetingSofa => Furniture::MeetingSofa,
            WaypointKind::MeetingStand => Furniture::MeetingStand,
        }
    }
}

/// Every geometry-bearing furniture/decor KIND. This is the unification axis:
/// the role enums ([`WaypointKind`] = wander destination, [`PodDecor`] = aisle
/// filler, [`PlantKind`], [`WallDecor`]) each `.furniture()`-map onto these, so
/// overlapping items collapse to ONE row — a phone booth, a rolling whiteboard,
/// or a tall plant has its shape defined exactly once no matter how many roles
/// reference it. (The home desk is per-agent, not a fixed kind, so it keeps the
/// [`desk_furniture_def`] accessor.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Furniture {
    Couch,
    Pantry,
    PhoneBooth,
    StandingDesk,
    VendingMachine,
    Printer,
    MeetingSofa,
    MeetingStand,
    PlantFicus,
    PlantTall,
    PlantFlower,
    PlantSucculent,
    Whiteboard,
    Tv,
    Bookshelf,
    BulletinBoard,
    ExitSign,
    MeetingScreen,
    // Singleton / per-room furniture (not keyed by a role enum — placed
    // directly in the layout). The meeting sofa/table BODIES are distinct from
    // the `MeetingSofa`/`MeetingStand` SEAT rows above (3 seats sit on 1 body):
    // the seat rows carry `None` footprint, these carry the obstacle the mask
    // stamps once per room.
    MeetingSofaBody,
    MeetingTable,
    PantryTable,
    PantryChair,
    FloorLamp,
    LoungeSideTable,
    /// The agent's OWNED home workstation. Not a [`WaypointKind`] (N per-agent
    /// desks, forced-seat when Active, never a wander destination) — but a
    /// first-class geometry row so desk and couch share ONE table and the same
    /// `seated_foot_cell` + approach/settle path. Replaces the old standalone
    /// `desk_furniture_def` literal (now a shim over this row).
    Desk,
}

impl Furniture {
    /// Every variant — the iteration handle for the exhaustive row-invariant
    /// test. Adding a variant is a compile error in [`furniture_def`]'s match
    /// AND fails the `ALL.len()` count assert until listed here, so no row can
    /// slip in unverified (the singleton/decor rows have no other guard).
    pub const ALL: &'static [Furniture] = &[
        Furniture::Couch,
        Furniture::Pantry,
        Furniture::PhoneBooth,
        Furniture::StandingDesk,
        Furniture::VendingMachine,
        Furniture::Printer,
        Furniture::MeetingSofa,
        Furniture::MeetingStand,
        Furniture::PlantFicus,
        Furniture::PlantTall,
        Furniture::PlantFlower,
        Furniture::PlantSucculent,
        Furniture::Whiteboard,
        Furniture::Tv,
        Furniture::Bookshelf,
        Furniture::BulletinBoard,
        Furniture::ExitSign,
        Furniture::MeetingScreen,
        Furniture::MeetingSofaBody,
        Furniture::MeetingTable,
        Furniture::PantryTable,
        Furniture::PantryChair,
        Furniture::FloorLamp,
        Furniture::LoungeSideTable,
        Furniture::Desk,
    ];
}

/// THE furniture table — one row per [`Furniture`] kind, the **single** source
/// of truth for ground shape (`footprint`) AND sprite size (`visual`) plus
/// occupancy / dwell / approach. Every geometric dependent (walkable mask,
/// stand-point half-extents, hit-test box, render centering + depth baseline)
/// derives from this row; do not re-type these numbers anywhere else. `dwell`
/// with `range == 0` marks a kind that is NOT a wander destination (pure decor).
pub const fn furniture_def(kind: Furniture) -> FurnitureDef {
    // Decor that isn't a wander destination: no dwell, approachable from
    // anywhere (unused — decor never runs stand_point). Spelled once.
    const DECOR: FurnitureDef = FurnitureDef {
        footprint: None,
        visual: (0, 0),
        occupies_pos: false,
        occludes_behind: None,
        dwell: (0, 0),
        approach: ApproachSides::ALL,
    };
    match kind {
        Furniture::Couch => FurnitureDef {
            footprint: Some((8, 7)),
            visual: (8, 7), // procedural render; visual unused
            occupies_pos: true,
            occludes_behind: None, // sitter occluded by the couch's own y-sort
            dwell: (20_000, 20_000),
            // SEAT_APPROACH rotated by the SEATED facing (North = looks at the
            // window → back_couch sprite) resolves to {N, E, W} — the natural
            // sides, EXCLUDING the south backrest. The agent comes from whichever
            // of those is reachable + nearest its (always-south) desk. A couch
            // seat walled in on ALL of N/E/W is un-sittable — approach_point
            // returns the `pos` sentinel and the wander SKIPS it (no fallback,
            // never the backrest). Near the window the N side is normally open,
            // so the couch is reachable; that N approach is the FAR side in
            // 2.5D, so the walk-in passes behind the couch until it settles.
            approach: SEAT_APPROACH,
        },
        Furniture::Pantry => FurnitureDef {
            footprint: None, // runtime-sized — see obstacle_footprint
            visual: (0, 0),  // runtime-sized; procedural render
            occupies_pos: false,
            occludes_behind: Some(3), // counter back; hides ~3px of a north-stander (waist-high, subtle)
            dwell: (10_000, 8_000),
            approach: ApproachSides::ALL,
        },
        Furniture::PhoneBooth => FurnitureDef {
            footprint: Some(PHONE_BOOTH_FOOTPRINT),
            visual: PHONE_BOOTH_FOOTPRINT,
            occupies_pos: false,
            occludes_behind: Some(6), // tall booth; hides most of a north-stander
            dwell: (8_000, 22_000),
            approach: ApproachSides::ALL,
        },
        Furniture::StandingDesk => FurnitureDef {
            footprint: Some(STANDING_DESK_FOOTPRINT),
            visual: STANDING_DESK_FOOTPRINT,
            occupies_pos: false,
            occludes_behind: Some(4), // desktop back; hides a north-stander's legs
            dwell: (8_000, 22_000),
            approach: ApproachSides::ALL,
        },
        Furniture::VendingMachine => FurnitureDef {
            footprint: Some((4, 6)),
            visual: (4, 6),
            occupies_pos: false,
            // None: sits flush against the corridor's NORTH edge (pos.y =
            // walkway.y+3), so there's no walkable cell behind it — a back-cap
            // would extrude onto the cubicle floor and occlude nothing.
            occludes_behind: None,
            dwell: (4_000, 4_000),
            approach: ApproachSides::ALL,
        },
        Furniture::Printer => FurnitureDef {
            footprint: Some((5, 4)),
            visual: (5, 4),
            occupies_pos: false,
            occludes_behind: None, // same as vending: corridor north-edge, nothing behind it
            dwell: (4_000, 4_000),
            approach: ApproachSides::ALL,
        },
        Furniture::MeetingSofa => FurnitureDef {
            footprint: None,
            visual: (0, 0), // procedural render
            occupies_pos: true,
            occludes_behind: None, // sitter occluded by the sofa's own y-sort
            dwell: (20_000, 20_000),
            approach: SEAT_APPROACH,
        },
        Furniture::MeetingStand => FurnitureDef {
            footprint: None,
            visual: (0, 0), // procedural render
            occupies_pos: true,
            occludes_behind: None, // stands beside the table; nothing behind to occlude
            dwell: (20_000, 20_000),
            approach: SEAT_APPROACH,
        },
        // Plants: all share the tight PLANT_FOOTPRINT ground (leaves overhang,
        // invariant #6) but each has a distinct sprite height.
        Furniture::PlantFicus => FurnitureDef {
            footprint: Some(PLANT_FOOTPRINT),
            visual: (6, 7),
            occludes_behind: None, // no back-cap: the 1px cap rendered as an ugly dark line across the foliage top; plants are thin decor, nothing meaningful sits behind them
            ..DECOR
        },
        Furniture::PlantTall => FurnitureDef {
            footprint: Some(PLANT_FOOTPRINT),
            visual: (6, 10),
            occludes_behind: None, // no back-cap: the 1px cap rendered as an ugly dark line across the foliage top; plants are thin decor, nothing meaningful sits behind them
            ..DECOR
        },
        // De-shared from PLANT_FOOTPRINT: the bloom overhangs a 2px pot, so the
        // ground footprint is the pot WIDTH only (height stays full so the
        // centered stamp still covers the bottom pot rows; invariant #6).
        Furniture::PlantFlower => FurnitureDef {
            footprint: Some((2, 6)),
            visual: (6, 6),
            occludes_behind: None, // no back-cap: the 1px cap rendered as an ugly dark line across the foliage top; plants are thin decor, nothing meaningful sits behind them
            ..DECOR
        },
        // Pot is 3px wide; the leaf cluster overhangs it. Width-only ground
        // footprint (NOT the 5px sprite, NOT the shared 6×6); invariant #6.
        Furniture::PlantSucculent => FurnitureDef {
            footprint: Some((3, 4)),
            visual: (5, 4),
            occludes_behind: None, // no back-cap: the 1px cap rendered as an ugly dark line across the foliage top; plants are thin decor, nothing meaningful sits behind them
            ..DECOR
        },
        // The rolling whiteboard is an ELEVATED obstacle: only its wheels/stand
        // (the bottom 3 sprite rows — legs at rows 8-9, wheels at row 10) touch
        // the floor; the 8-px board panel above them overhangs (invariant #6, the
        // canopy rule). GROUND footprint = the 10-px wheel span × the 3-px base
        // ONLY. `mask.rs` SOUTH-anchors this strip to the sprite's base (the
        // `Center`/`TopLeft` stamp would otherwise center the short strip on the
        // panel, lifting the block off the wheels), so a walker can pass BEHIND
        // the panel and is occluded by it (the 8-px overhang via z-sort + the
        // `occludes_behind` back-cap). Stamped via PodDecor (aisle) or WallDecor
        // (the free-standing board in the room).
        Furniture::Whiteboard => FurnitureDef {
            footprint: Some((10, 3)),
            visual: (14, 11),
            occludes_behind: Some(2), // board panel occludes a walker behind it
            ..DECOR
        },
        Furniture::Tv => FurnitureDef {
            footprint: Some((6, 10)),
            visual: (10, 10),
            occludes_behind: Some(2), // monitor occludes a walker behind the stand
            ..DECOR
        },
        // Wall-mounted decor — hung in the wall band, never stamped into the
        // mask, so footprint stays None and only `.visual` matters.
        Furniture::Bookshelf => FurnitureDef {
            visual: (8, 12),
            ..DECOR
        },
        Furniture::BulletinBoard => FurnitureDef {
            visual: (10, 6),
            ..DECOR
        },
        Furniture::ExitSign => FurnitureDef {
            visual: (5, 3),
            ..DECOR
        },
        Furniture::MeetingScreen => FurnitureDef {
            visual: (14, 12),
            ..DECOR
        },
        // Singleton / per-room furniture (rendered procedurally, so `visual` is
        // mostly informational; the mask stamps `footprint`). The meeting sofa
        // body's width is 16 ON PURPOSE: `16 + 2·OBSTACLE_PAD = 20` reproduces
        // the 20px sprite's X footprint while the pad gives the vertical sit-
        // access clearance — a literal 20-wide block disconnects the narrowest
        // meeting room (caught by the connectivity test).
        Furniture::MeetingSofaBody => FurnitureDef {
            footprint: Some((16, 7)),
            visual: (20, 8),
            ..DECOR
        },
        // 11×5 = the real coffee-table sprite (paint_coffee_table). footprint ==
        // visual so the mask blocks exactly what's drawn; the MeetingStand west
        // offset (compute.rs, t.x-9) still clears (padded west edge = cx-7).
        Furniture::MeetingTable => FurnitureDef {
            footprint: Some((11, 5)),
            visual: (11, 5),
            ..DECOR
        },
        Furniture::PantryTable => FurnitureDef {
            footprint: Some((7, 4)),
            visual: (7, 4),
            ..DECOR
        },
        // 2×2 = exactly the four pixels the painter draws (no back / legs). Both
        // footprint AND visual match the draw; mask.rs stamps it CENTERED so it
        // sits on the visible stool, not 1px north/west of it (the old 3×3 +
        // left/top-biased stamp blocked floor where nothing was drawn).
        Furniture::PantryChair => FurnitureDef {
            footprint: Some((2, 2)),
            visual: (2, 2),
            ..DECOR
        },
        // Width 2 = the 2px base disc (was 4, over-blocking the 1px pole + empty
        // margins). Height 7 is deliberate, NOT the disc's 1px: the disc sits at
        // the sprite SOUTH, so the centered stamp + pad must REACH down to
        // lamp.y+4 — a height shrink would lift the block off the disc entirely.
        Furniture::FloorLamp => FurnitureDef {
            footprint: Some((2, 7)),
            visual: (4, 10),
            ..DECOR
        },
        Furniture::LoungeSideTable => FurnitureDef {
            footprint: Some((7, 4)),
            visual: (7, 4),
            ..DECOR
        },
        // The home desk — the agent's OWNED workstation, now a first-class row
        // (was the standalone `desk_furniture_def` literal). `occupies_pos` = the
        // agent renders ON it (`seated_anchor`); its seat cell is
        // [`desk_walk_anchor`] (= `seated_foot_cell(Desk)`). `footprint = DESK_W+4`
        // (the solid 16px sprite, no overhang) is stamped TOP-LEFT in `mask.rs`,
        // not centered. `dwell` is the SEATED window (`pose::seated_dwell_ms`).
        // `approach = DESK_APPROACH` (no south front — sit behind the monitor).
        // Not a `WaypointKind`, so `stand_point` never runs on it; entry/wander/
        // exit reach its seat via `approach_point(Furniture::Desk)` (the N/E/W
        // `desk_approach_cell`) + the unified `seated_foot_cell` settle.
        Furniture::Desk => FurnitureDef {
            footprint: Some((DESK_W + 4, DESK_H)),
            visual: (DESK_W + 4, DESK_H + 2),
            occupies_pos: true,
            occludes_behind: None,
            dwell: (15_000, 15_000),
            approach: DESK_APPROACH,
        },
    }
}

/// The **home desk** descriptor — sugar over the [`Furniture::Desk`] table row
/// (kept because the desk is per-agent, not a `WaypointKind`, and ~10 call sites
/// read it). The geometry now lives in ONE place: `furniture_def(Furniture::Desk)`.
pub const fn desk_furniture_def() -> FurnitureDef {
    furniture_def(Furniture::Desk)
}

/// Vertical offset baked into the TUI walking / waypoint sprite anchor
/// (`p.y - WALKING_Y_OFF`) — the 12-px standing/walking sprite height. Owned
/// here in core (not just as a tui literal) so `seated_foot_cell` and the tui
/// anchor reference ONE value: the "invert the render anchor to the settle
/// cell" identity then holds by construction, not by two crates keeping a
/// literal in sync. See [`seated_foot_cell`].
pub const WALKING_Y_OFF: u16 = 12;
/// Vertical offset of the back-view seat sprite anchor (`pos.y - SEAT_RENDER_Y_OFF`).
/// The seat's settle cell is `WALKING_Y_OFF - SEAT_RENDER_Y_OFF = 5` px south of
/// `pos` (where `walking_anchor` lands exactly on `back_couch_anchor`).
pub const SEAT_RENDER_Y_OFF: u16 = 7;

/// Offsets from a home desk's top-left to the agent's WALK anchor (the cell the
/// agent walks to/from for its desk). Chosen so the TUI `walking_anchor` of this
/// point equals the TUI `seated_anchor` of the desk — the agent settles exactly
/// onto its north seat with no arrival pop, just clear of the desk obstacle.
/// The `walking_anchor(desk_walk_anchor(d)) == seated_anchor(d)` identity is
/// locked by a tui-side test; if `DESK_W` or those anchors change they move
/// together (X tracks `DESK_W`; `8` is the character sprite width).
pub const DESK_WALK_X_OFF: u16 = (DESK_W - 8) / 2 + 4;
pub const DESK_WALK_Y_OFF: u16 = 4;

/// The cell an agent walks to/from for its home `desk` (top-left Point). The
/// single source for what were ~10 scattered `desk + (6, 4)` literals across the
/// entry / exit / wander / snap-back walks.
pub fn desk_walk_anchor(desk: Point) -> Point {
    Point {
        x: desk.x + DESK_WALK_X_OFF,
        y: desk.y + DESK_WALK_Y_OFF,
    }
}

/// The cell where a seated agent's WALK visually ends so the seated sprite
/// renders with no arrival jump — the inverse of the render anchor under
/// [`WALKING_Y_OFF`], solving `walking_anchor(S) == render_anchor(pos)`.
///
/// `Some` for every `occupies_pos` furniture (desk + the seat kinds — the agent
/// sits/stands ON `pos`); `None` for obstacles, whose sprite renders AT the
/// approach cell, not at a fixed seat. Keyed on [`Furniture`] so the home desk
/// flows through the SAME fn as the couch (the desk's `S` is [`desk_walk_anchor`],
/// its render `seated_anchor`). The post-A\* settle walks `approach_point → S`;
/// when `S` is blocked (meeting sofa, desk) that final segment is the "sit down"
/// motion, not pathfinding.
pub fn seated_foot_cell(kind: Furniture, pos: Point) -> Option<Point> {
    if !furniture_def(kind).occupies_pos {
        return None;
    }
    Some(match kind {
        // back_couch render (`pos.y − SEAT_RENDER_Y_OFF`): S is
        // `WALKING_Y_OFF − SEAT_RENDER_Y_OFF` px south of `pos`, the one cell
        // where `walking_anchor` lands exactly on `back_couch_anchor`.
        Furniture::Couch | Furniture::MeetingSofa => Point {
            x: pos.x,
            y: pos.y + (WALKING_Y_OFF - SEAT_RENDER_Y_OFF),
        },
        // waypoint render (`== walking_anchor`): S == pos.
        Furniture::MeetingStand => pos,
        // desk render is `seated_anchor`; its inverse is the bespoke
        // `desk_walk_anchor` (pinned by DESK_WALK_X/Y_OFF). ONE source.
        Furniture::Desk => desk_walk_anchor(pos),
        // `occupies_pos` is exactly {Couch, MeetingSofa, MeetingStand, Desk}
        // (guarded by `furniture_def_invariants_hold_for_every_row`); the early
        // return handled every obstacle kind.
        _ => pos,
    })
}

/// Which way a waypoint occupant faces. Drives sprite choice (back vs
/// front view) and horizontal mirroring at render time. Most waypoints
/// are `South` (facing the viewer / facing-neutral); meeting-room slots
/// face the table at the room centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facing {
    North,
    South,
    East,
    West,
}

/// Wall-mounted / wall-leaning furniture, painted as decor in the top wall
/// area. Not a wander destination — agents can't walk through their own
/// cubicle row to reach the back wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallDecor {
    Bookshelf,
    Whiteboard,
    BulletinBoard,
    ExitSign,
    /// Wall-mounted meeting-room display — paints above the meeting
    /// room interior so participants can pretend they're presenting.
    MeetingScreen,
}

impl WallDecor {
    /// Geometry kind in the unified [`Furniture`] table (sprite size via
    /// `furniture_def(self.furniture()).visual`; wall decor isn't mask-stamped).
    pub const fn furniture(self) -> Furniture {
        match self {
            WallDecor::Whiteboard => Furniture::Whiteboard,
            WallDecor::Bookshelf => Furniture::Bookshelf,
            WallDecor::BulletinBoard => Furniture::BulletinBoard,
            WallDecor::ExitSign => Furniture::ExitSign,
            WallDecor::MeetingScreen => Furniture::MeetingScreen,
        }
    }

    /// Pack-animation key for this decor's sprite. The blit lives in the tui crate
    /// (`drawable.rs`); the NAME lives on the enum so a new variant is a compile
    /// error HERE, not a forgotten call-site match arm (same data-in-core pattern
    /// as `occludes_behind`). Every value is in `OPTIONAL_FURNITURE_ANIMATIONS`,
    /// pinned by `role_enum_sprite_names_resolve_in_the_animation_registry`.
    pub const fn sprite_name(self) -> &'static str {
        match self {
            WallDecor::Bookshelf => "bookshelf",
            WallDecor::Whiteboard => "whiteboard",
            WallDecor::BulletinBoard => "bulletin_board",
            WallDecor::ExitSign => "exit_sign",
            WallDecor::MeetingScreen => "meeting_screen",
        }
    }
}

/// Variety of potted plants — each renders a different sprite. Spread
/// these around the lounge so it doesn't feel like one ficus repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlantKind {
    Ficus,
    Tall,
    Flower,
    Succulent,
}

impl PlantKind {
    /// Geometry kind in the unified [`Furniture`] table. Sprite size via
    /// `furniture_def(self.furniture()).visual`; all plants share the tight
    /// `PLANT_FOOTPRINT` ground (leaves overhang it, invariant #6).
    pub const fn furniture(self) -> Furniture {
        match self {
            PlantKind::Ficus => Furniture::PlantFicus,
            PlantKind::Tall => Furniture::PlantTall,
            PlantKind::Flower => Furniture::PlantFlower,
            PlantKind::Succulent => Furniture::PlantSucculent,
        }
    }

    /// Pack-animation key for this plant's sprite (blit in `drawable.rs`).
    pub const fn sprite_name(self) -> &'static str {
        match self {
            PlantKind::Ficus => "plant",
            PlantKind::Tall => "plant_tall",
            PlantKind::Flower => "plant_flower",
            PlantKind::Succulent => "plant_succulent",
        }
    }
}

/// Decor placed in the aisles BETWEEN 2×2 desk pods. Picked at random
/// (deterministic hash of pod index) so each office layout is varied
/// but stable across renders. Each variant maps to a distinct sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PodDecor {
    PlantTall,
    Whiteboard,
    Tv,
    PhoneBooth,
    StandingDesk,
}

impl PodDecor {
    /// The randomly-picked pool. Whiteboard's 10-px GROUND footprint
    /// (the 14-px board panel overhangs it) fits the 22-px aisle with
    /// ~5 px clearance each side after the 1-px obstacle pad — same
    /// rolling-whiteboard sprite as the wall mount, just in an aisle slot.
    pub const ALL: &'static [PodDecor] = &[
        PodDecor::PlantTall,
        PodDecor::Whiteboard,
        PodDecor::Tv,
        PodDecor::PhoneBooth,
        PodDecor::StandingDesk,
    ];

    /// Geometry kind in the unified [`Furniture`] table — the single source for
    /// this decor's ground footprint (mask) AND sprite size (render). PlantTall
    /// resolves to the SAME row as the free-standing `PlantKind::Tall`, and
    /// PhoneBooth/StandingDesk to the same rows as their `WaypointKind` twins, so
    /// nothing drifts. (PlantTall's mask footprint is the tight 6×6 ground while
    /// its sprite is 6×10 — the fix that motivated this fold.)
    pub const fn furniture(self) -> Furniture {
        match self {
            PodDecor::PlantTall => Furniture::PlantTall,
            PodDecor::Whiteboard => Furniture::Whiteboard,
            PodDecor::Tv => Furniture::Tv,
            PodDecor::PhoneBooth => Furniture::PhoneBooth,
            PodDecor::StandingDesk => Furniture::StandingDesk,
        }
    }

    /// Pack-animation key for this pod-decor's sprite (blit in `drawable.rs`).
    pub const fn sprite_name(self) -> &'static str {
        match self {
            PodDecor::PlantTall => "plant_tall",
            PodDecor::Whiteboard => "whiteboard",
            PodDecor::Tv => "tv_stand",
            PodDecor::PhoneBooth => "phone_booth",
            PodDecor::StandingDesk => "standing_desk",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: (i32, i32) = (0, -1);
    const S: (i32, i32) = (0, 1);
    const E: (i32, i32) = (1, 0);
    const W: (i32, i32) = (-1, 0);

    fn allowed(sides: ApproachSides, facing: Facing) -> Vec<(i32, i32)> {
        [N, S, E, W]
            .into_iter()
            .filter(|&d| sides.allows(facing, d))
            .collect()
    }

    #[test]
    fn all_allows_every_side_for_any_facing() {
        for facing in [Facing::North, Facing::South, Facing::East, Facing::West] {
            assert_eq!(allowed(ApproachSides::ALL, facing), vec![N, S, E, W]);
        }
    }

    #[test]
    fn seat_facing_south_allows_front_and_sides_not_back() {
        // Sofa facing south (front toward viewer): approach S + E + W, not N.
        assert_eq!(allowed(SEAT_APPROACH, Facing::South), vec![S, E, W]);
    }

    #[test]
    fn seat_facing_north_rotates_to_exclude_the_south_back() {
        // Sofa facing north (back toward viewer/south): approach N + E + W, not S.
        assert_eq!(allowed(SEAT_APPROACH, Facing::North), vec![N, E, W]);
    }

    #[test]
    fn desk_excludes_its_south_front() {
        // Home desk faces south (monitor toward viewer): reachable N/E/W only.
        assert_eq!(allowed(DESK_APPROACH, Facing::South), vec![N, E, W]);
        // And "remove east" would be a one-bool edit:
        let no_east = ApproachSides {
            e: false,
            ..DESK_APPROACH
        };
        assert_eq!(allowed(no_east, Facing::South), vec![N, W]);
    }

    #[test]
    fn rotation_is_a_bijection_on_sides() {
        // A single-side set must map to exactly one side under any facing
        // (no side lost or duplicated by the rotation).
        for facing in [Facing::North, Facing::South, Facing::East, Facing::West] {
            for one in [N, S, E, W] {
                let sides = ApproachSides {
                    n: one == N,
                    s: one == S,
                    e: one == E,
                    w: one == W,
                };
                assert_eq!(
                    allowed(sides, facing).len(),
                    1,
                    "facing {facing:?}, side {one:?} must rotate to exactly one side",
                );
            }
        }
    }

    #[test]
    fn desk_is_a_furniture_def_with_desk_geometry() {
        // The home desk is now a first-class Furniture::Desk row; this accessor
        // is sugar over furniture_def(Furniture::Desk). occupies_pos=true — the
        // agent renders ON it (seated_anchor); its seat cell is desk_walk_anchor
        // = seated_foot_cell(Furniture::Desk), reached by the unified settle.
        let d = desk_furniture_def();
        assert_eq!(
            d,
            furniture_def(Furniture::Desk),
            "desk_furniture_def must be sugar over the Furniture::Desk row"
        );
        // Footprint matches the solid 16px sprite (DESK_W+4), not the old +2
        // under-block; footprint never exceeds the 16×8 visual.
        assert_eq!(d.footprint, Some((DESK_W + 4, DESK_H)), "desk footprint");
        let (fw, fh) = d.footprint.unwrap();
        assert!(
            fw <= d.visual.0 && fh <= d.visual.1,
            "desk footprint must not exceed its visual"
        );
        assert!(
            d.occupies_pos,
            "agent renders ON the desk (seated_anchor); seat = seated_foot_cell(Desk)"
        );
        assert_eq!(
            d.approach, DESK_APPROACH,
            "desk uses the editable DESK_APPROACH policy"
        );
        assert!(d.dwell.1 > 0, "seated dwell range must be positive");
    }

    #[test]
    fn furniture_def_invariants_hold_for_every_row() {
        // The singleton/decor rows have no other test (unlike WaypointKind::ALL),
        // so a typo in any of the 25 rows — wrong dwell sentinel, an accidental
        // occupies_pos, a wrong plant footprint — is caught HERE rather than as a
        // silent wrong-mask/wrong-render at runtime.
        assert_eq!(
            Furniture::ALL.len(),
            25,
            "Furniture variant added/removed — update ALL (and this count)"
        );
        for &f in Furniture::ALL {
            let d = furniture_def(f);
            // dwell is either the decor sentinel (0,0) or a real window (range>0);
            // never half-broken like (n, 0) — see the FurnitureDef::dwell doc.
            assert!(
                d.dwell == (0, 0) || d.dwell.1 > 0,
                "{f:?}: half-broken dwell {:?}",
                d.dwell
            );
            // occupies_pos is EXACTLY the on-furniture seat/stand kinds (incl.
            // the home Desk — agent renders ON it via seated_anchor).
            let expect_occupies = matches!(
                f,
                Furniture::Couch
                    | Furniture::MeetingSofa
                    | Furniture::MeetingStand
                    | Furniture::Desk
            );
            assert_eq!(d.occupies_pos, expect_occupies, "{f:?}: occupies_pos");
            // Meeting SEAT rows add no obstacle (3 seats sit on the 1 body row).
            if matches!(f, Furniture::MeetingSofa | Furniture::MeetingStand) {
                assert!(
                    d.footprint.is_none(),
                    "{f:?}: seat row must carry no footprint"
                );
            }
            // Ficus + Tall share the 6px PLANT_FOOTPRINT (their pots are 6px
            // wide). Flower/Succulent are de-shared: their pots are narrower than
            // the canopy, so they carry their own width-only ground footprint.
            if matches!(f, Furniture::PlantFicus | Furniture::PlantTall) {
                assert_eq!(
                    d.footprint,
                    Some(PLANT_FOOTPRINT),
                    "{f:?}: plant ground footprint"
                );
            }
            if matches!(f, Furniture::PlantFlower | Furniture::PlantSucculent) {
                assert!(
                    d.footprint.is_some_and(|(w, _)| w < PLANT_FOOTPRINT.0),
                    "{f:?}: de-shared plant must be narrower than PLANT_FOOTPRINT"
                );
            }
            // Footprint never exceeds the sprite — the mask blocks only the
            // ground projection, never more than is drawn (invariant #6). The
            // succulent inversion (6×6 footprint under a 5×4 sprite) is the bug
            // this guards against recurring.
            if let Some((fw, fh)) = d.footprint {
                assert!(
                    fw <= d.visual.0 && fh <= d.visual.1,
                    "{f:?}: footprint {:?} exceeds visual {:?} (invariant #6)",
                    d.footprint,
                    d.visual
                );
            }
            // Behind-occlusion (the renderer's back-cap) DEPTH per furniture —
            // EXACTLY the solid free-standing objects an agent stands behind,
            // each with its tuned extrusion height. Pinning the values here
            // forces a new furniture row to make the call (the ALL count assert
            // above forces the row; this forces both its on/off AND its depth).
            let expect_occludes = match f {
                Furniture::Pantry => Some(3),
                Furniture::PhoneBooth => Some(6),
                Furniture::StandingDesk => Some(4),
                // VendingMachine/Printer = None: corridor north-edge, no cell behind.
                Furniture::Whiteboard | Furniture::Tv => Some(2),
                // Plants = None: the 1px back-cap rendered an ugly dark line across
                // the foliage top; they're thin decor with nothing behind them.
                _ => None,
            };
            assert_eq!(
                d.occludes_behind, expect_occludes,
                "{f:?}: occludes_behind depth"
            );
            // A seat's sitter is occluded by the seat's own y-sort, never a back
            // face — the two occlusion mechanisms are mutually exclusive.
            assert!(
                !(d.occludes_behind.is_some() && d.occupies_pos),
                "{f:?}: occludes_behind + occupies_pos are mutually exclusive"
            );
        }
    }

    #[test]
    fn role_enum_sprite_names_resolve_in_the_animation_registry() {
        // The role enums own their pack-animation key via `sprite_name()` (the
        // blit lives in tui `drawable.rs`). Adding a variant without a name is a
        // compile error (exhaustive match); a TYPO'd name would draw nothing —
        // this catches it by checking every value is a real registered animation.
        use crate::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS;
        let names: Vec<&str> = [
            WallDecor::Bookshelf.sprite_name(),
            WallDecor::Whiteboard.sprite_name(),
            WallDecor::BulletinBoard.sprite_name(),
            WallDecor::ExitSign.sprite_name(),
            WallDecor::MeetingScreen.sprite_name(),
            PlantKind::Ficus.sprite_name(),
            PlantKind::Tall.sprite_name(),
            PlantKind::Flower.sprite_name(),
            PlantKind::Succulent.sprite_name(),
        ]
        .into_iter()
        .chain(PodDecor::ALL.iter().map(|p| p.sprite_name()))
        .collect();
        for n in names {
            assert!(
                OPTIONAL_FURNITURE_ANIMATIONS.contains(&n),
                "sprite_name {n:?} is not a registered OPTIONAL_FURNITURE_ANIMATIONS key"
            );
        }
    }
}
