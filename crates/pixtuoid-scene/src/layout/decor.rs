//! Decor vocabulary used by `SceneLayout` — the enums describing every piece of
//! furniture and waypoint kind in the office, plus THE table giving each its
//! geometry. Kept separate so a new sprite kind doesn't churn the layout math.

use super::{Anchor, Point, Size, CHARACTER_SPRITE_W, DESK_FOOT_H, DESK_H, DESK_W};

/// Wander destinations the Idle state machine can pick — each kind controls the
/// pose + sprite an arriving agent takes. Plants/lamps are decor, not waypoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaypointKind {
    /// Top-of-cubicle viewing couch facing the city windows.
    Couch,
    /// Pantry counter — kitchen + coffee.
    Pantry,
    /// Aisle phone booth — agent stands at the door (private call).
    PhoneBooth,
    /// Aisle standing desk (alternate workstation).
    StandingDesk,
    /// Corridor vending machine — agent stands in front to grab a drink.
    VendingMachine,
    /// Corridor printer — agent stands in front while "printing."
    Printer,
    /// Meeting-room sofa seat, facing the table. Multiple seats per sofa.
    MeetingSofa,
    /// Meeting-room spot beside the table, facing it.
    MeetingChair,
    /// Kitchen-island spot at the island edge (coffee-and-chat).
    Island,
    /// Snack shelf — agent stands in front browsing the shelves.
    SnackShelf,
}

/// Per-spot idle dwell window. `range_ms == 0` is the DECOR sentinel (not a
/// wander destination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DwellWindow {
    /// Baseline dwell time at the spot, in milliseconds.
    pub base_ms: u64,
    /// Extra randomized dwell added on top of `base_ms`, in milliseconds
    /// (`0` marks the [`Self::DECOR`] non-destination sentinel).
    pub range_ms: u64,
}
impl DwellWindow {
    /// The decor sentinel — scenery, not a wander destination.
    pub const DECOR: DwellWindow = DwellWindow {
        base_ms: 0,
        range_ms: 0,
    };
}

/// Plant GROUND footprint — the one geometry VALUE shared by the ficus + tall
/// plant rows in [`furniture_def`]: a shallow POT strip the mask south-anchors to
/// the sprite's base, with the leafy canopy overhanging it (invariant #6). Read
/// only THROUGH the table (`furniture_def(_).footprint`), never directly.
pub(crate) const PLANT_FOOTPRINT: Size = Size { w: 6, h: 3 };

/// Which sides an agent may approach a piece of furniture from, in the
/// CANONICAL frame (furniture facing South, toward the viewer). [`Self::allows`]
/// rotates this to the live `facing`, so one stored set works for
/// variable-facing furniture. **To add/remove an entry side, flip one bool.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproachSides {
    /// Approachable from the north (−y) in the canonical frame?
    pub n: bool,
    /// Approachable from the south (+y, the canonical front)?
    pub s: bool,
    /// Approachable from the east (+x)?
    pub e: bool,
    /// Approachable from the west (−x)?
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

    /// This canonical (facing-South) set rotated to the live `facing`.
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

/// Approach sides for the home desk. Canonical: exclude the south front (the
/// monitor faces the viewer; the agent sits behind it), so reachable from N/E/W.
pub const DESK_APPROACH: ApproachSides = ApproachSides {
    n: true,
    s: false,
    e: true,
    w: true,
};

/// Definition record for a furniture kind — the single source of truth for its
/// ground shape, occupancy semantics, and dwell. Reshaping a piece of furniture is
/// editing ONE row of [`furniture_def`]; the walkable mask, stand-point, hit-test
/// hitbox and render depth baseline all DERIVE from these fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureDef {
    /// Ground footprint `(w, h)` the walkable mask stamps (top-down z=0 rect), or
    /// `None` for slots that add no obstacle of their own (MeetingSofa/MeetingChair
    /// sit on furniture stamped elsewhere). NB: `Pantry` is also `None` because its
    /// footprint is runtime-sized — `obstacle_footprint` special-cases it, the one
    /// kind whose shape isn't a static literal.
    pub footprint: Option<Size>,
    /// Visual sprite size `(w, h)` in buffer px — the SECOND geometry axis, kept
    /// distinct from `footprint` (the top-down ground rule, invariant #6): a sprite
    /// legitimately overhangs its ground base, and conflating the two is the
    /// canopy-over-aisle bug this split prevents. Render centering + the z-sort
    /// south row derive from this; the mask derives from `footprint`.
    pub visual: Size,
    /// The agent occupies `pos` DIRECTLY (sprite renders ON the furniture), so
    /// `stand_point` passes `pos` through unchanged instead of resolving a walkable
    /// cell beside the furniture. NOT "a human can sit here", and NOT capacity: a
    /// phone booth renders stand-beside yet holds exactly one caller, so capacity
    /// lives on the separate [`exclusive`](Self::exclusive) field. This is the set
    /// `seated_foot_cell` switches on (its `unreachable!` arm keeps the two in step).
    pub occupies_pos: bool,
    /// A single-occupancy DESTINATION: at most one agent is assigned here at a
    /// time. SUPERSET of `occupies_pos` — every seat, PLUS the enclosed
    /// stand-beside singles (`PhoneBooth`, `StandingDesk`) that render at a SIDE
    /// cell yet still hold exactly one person. Queue spots (pantry / vending /
    /// printer / snack shelf) are NOT exclusive — agents share and step aside.
    pub exclusive: bool,
    /// Per-spot idle dwell window. `range_ms == 0` (the `DECOR` rows) marks a kind
    /// that is NOT a wander destination; `dwell_ms` guards with
    /// `% range_ms.max(1)`, so a zero range is safe. Do not "fix" a decor row to a
    /// non-zero range — that silently turns it into a wander destination.
    pub dwell: DwellWindow,
    /// Canonical (facing-South) sides an agent may approach from. Obstacle
    /// furniture against walls keeps `ALL` (walls already constrain the open side);
    /// seats use "front + sides, no back" so a walker never paths in through the
    /// sofa back.
    pub approach: ApproachSides,
    /// Where `footprint` sits inside the VISUAL box horizontally. Every current row
    /// is `Center`; the field exists so a future sideways-overhanging piece declares
    /// `Start`/`End` instead of needing a new stamp path.
    pub ground_x: GroundAlign,
    /// Where `footprint` sits inside the VISUAL box vertically: `End` for the
    /// overhang canopy/panel/column pieces AND the desk (invariant #6, the
    /// walk-behind shape), `Center` for the meeting sofa body + floor lamp.
    /// Resolves to a pixel offset from `visual − footprint` at stamp time.
    pub ground_y: GroundAlign,
}

impl FurnitureDef {
    /// The blocked ground rect for an EXPLICIT footprint, placed with THIS def's
    /// visual box + ground aligns — so the runtime-footprint path (a waypoint's
    /// `approach::obstacle_footprint`) shares the def's alignment with the table
    /// path and no call site re-threads `visual`/`ground_x`/`ground_y`.
    pub(super) fn ground_rect_of(&self, anchor: Anchor, pos: Point, fp: Size) -> (Point, Size) {
        super::mask::ground_rect(anchor, pos, fp, self.visual, self.ground_x, self.ground_y)
    }

    /// The blocked ground rect from this def's OWN table footprint, or `None` when
    /// the piece has no ground footprint (wall-hung decor, runtime-sized pantry
    /// counter). THE concentrator the mask stamp / collision checks / placement
    /// sweep all read.
    pub(super) fn ground_rect(&self, anchor: Anchor, pos: Point) -> Option<(Point, Size)> {
        self.footprint
            .map(|fp| self.ground_rect_of(anchor, pos, fp))
    }
}

/// Canonical seat approach: front + sides, exclude the back. Rotates with
/// facing so a north- or south-facing sofa each exclude their own back.
const SEAT_APPROACH: ApproachSides = ApproachSides {
    n: false,
    s: true,
    e: true,
    w: true,
};

/// Canonical bar-slot approach: behind + sides, never across the FRONT — the
/// mirror of [`SEAT_APPROACH`]. A bartender slot sits INSIDE the island body, so a
/// front (south, canonical) approach would glide visibly THROUGH the counter's
/// face; behind + lateral glides stay behind the countertop for the whole settle.
const BAR_APPROACH: ApproachSides = ApproachSides {
    n: true,
    s: false,
    e: true,
    w: true,
};

impl WaypointKind {
    /// Every variant, for exhaustive invariant tests. Order is not load-bearing.
    pub const ALL: &'static [WaypointKind] = &[
        WaypointKind::Couch,
        WaypointKind::Pantry,
        WaypointKind::PhoneBooth,
        WaypointKind::StandingDesk,
        WaypointKind::VendingMachine,
        WaypointKind::Printer,
        WaypointKind::MeetingSofa,
        WaypointKind::MeetingChair,
        WaypointKind::Island,
        WaypointKind::SnackShelf,
    ];

    /// This waypoint's geometry kind in the unified [`Furniture`] table. The
    /// waypoint enum carries only ROLE — a wander destination.
    pub const fn furniture(self) -> Furniture {
        match self {
            WaypointKind::Couch => Furniture::Couch,
            WaypointKind::Pantry => Furniture::Pantry,
            WaypointKind::PhoneBooth => Furniture::PhoneBooth,
            WaypointKind::StandingDesk => Furniture::StandingDesk,
            WaypointKind::VendingMachine => Furniture::VendingMachine,
            WaypointKind::Printer => Furniture::Printer,
            WaypointKind::MeetingSofa => Furniture::MeetingSofa,
            WaypointKind::MeetingChair => Furniture::MeetingChair,
            WaypointKind::Island => Furniture::IslandStand,
            WaypointKind::SnackShelf => Furniture::SnackShelf,
        }
    }
}

/// Every geometry-bearing furniture/decor KIND. This is the unification axis: the
/// role enums ([`WaypointKind`] = wander destination, [`PodDecor`] = aisle filler,
/// [`PlantKind`], [`WallDecor`]) each `.furniture()`-map onto these, so an item's
/// shape is defined exactly once no matter how many roles reference it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Furniture {
    /// The lounge / cubicle-top viewing couch (3 seats).
    Couch,
    /// The pantry counter (kitchen + coffee).
    Pantry,
    /// An aisle phone booth.
    PhoneBooth,
    /// An aisle standing desk (alternate workstation).
    StandingDesk,
    /// A corridor vending machine.
    VendingMachine,
    /// A corridor printer.
    Printer,
    /// A meeting-room sofa SEAT (its body is [`Furniture::MeetingSofaBody`]).
    MeetingSofa,
    /// A meeting-room spot beside the table.
    MeetingChair,
    /// A potted ficus (6px pot, overhanging canopy).
    PlantFicus,
    /// A tall potted plant (shares the ficus pot footprint).
    PlantTall,
    /// A small flowering plant (2×2 pot).
    PlantFlower,
    /// A low succulent (3×2 pot).
    PlantSucculent,
    /// A rolling whiteboard (aisle filler or wall decor).
    Whiteboard,
    /// A wall-mounted TV.
    Tv,
    /// A bookshelf.
    Bookshelf,
    /// A cork bulletin board.
    BulletinBoard,
    /// An exit sign.
    ExitSign,
    /// A meeting-room presentation screen.
    MeetingScreen,
    /// The meeting-sofa BODY — the obstacle the mask stamps once per room
    /// (its seats are the [`Furniture::MeetingSofa`] rows).
    MeetingSofaBody,
    /// The meeting-room table body.
    MeetingTable,
    /// The lounge floor lamp.
    FloorLamp,
    /// The lounge side table (wood surface + magazine).
    LoungeSideTable,
    /// Kitchen-island BODY (the big center counter) — the obstacle the mask
    /// stamps once; the `IslandStand` rows are the stand slots around it.
    KitchenIsland,
    /// A standing slot at the kitchen island's edge (no obstacle of its own).
    IslandStand,
    /// Snack shelf against the pantry's west wall — an approachable obstacle
    /// (vending-machine class): tall shelf sprite, shallow walk-behind base.
    SnackShelf,
    /// The agent's OWNED home workstation. Not a [`WaypointKind`] (per-agent,
    /// forced-seat when Active, never a wander destination) but a first-class
    /// geometry row, so desk and couch share ONE table and the same
    /// `seated_foot_cell` + approach/settle path.
    Desk,
    /// Aquarium on a low cabinet against the north wall band. Pure decor: the
    /// glass tank overhangs a shallow cabinet base, idle fish animate in the
    /// paint pass.
    FishTank,
}

impl Furniture {
    /// Every variant — the iteration handle for the exhaustive row-invariant test.
    /// A new variant fails the `ALL.len()` count assert until it is listed here,
    /// so no row can slip in unverified.
    pub const ALL: &'static [Furniture] = &[
        Furniture::Couch,
        Furniture::Pantry,
        Furniture::PhoneBooth,
        Furniture::StandingDesk,
        Furniture::VendingMachine,
        Furniture::Printer,
        Furniture::MeetingSofa,
        Furniture::MeetingChair,
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
        Furniture::FloorLamp,
        Furniture::LoungeSideTable,
        Furniture::KitchenIsland,
        Furniture::IslandStand,
        Furniture::SnackShelf,
        Furniture::Desk,
        Furniture::FishTank,
    ];
}

/// Whether a scatter plant must keep clearance from this furniture kind when it
/// appears as a NON-waypoint singleton — the plant-obstacle census's per-kind
/// authority (`plant_obstacle_rects` filters every singleton by it). An EXHAUSTIVE
/// match, no `_`: a new [`Furniture`] kind can't compile until it declares a
/// stance, because curating this set by OMISSION let bodies ship with no
/// repulsion and plants grew through them.
pub(crate) const fn repels_plants(kind: Furniture) -> bool {
    match kind {
        // Solid non-waypoint singleton BODIES a scatter plant routes around.
        Furniture::FishTank
        | Furniture::MeetingSofaBody
        | Furniture::MeetingTable
        | Furniture::KitchenIsland => true,
        // The lounge lamp + side table ARE non-waypoint singletons too, but the
        // owner-ratified mock has the lounge Ficus hug them (1px) — deliberately
        // NOT repelled.
        Furniture::FloorLamp | Furniture::LoungeSideTable => false,
        // Never a non-waypoint singleton in the plant census — waypoint furniture
        // is repelled via `first_blocking_waypoint` instead.
        Furniture::Couch
        | Furniture::Pantry
        | Furniture::PhoneBooth
        | Furniture::StandingDesk
        | Furniture::VendingMachine
        | Furniture::Printer
        | Furniture::MeetingSofa
        | Furniture::MeetingChair
        | Furniture::PlantFicus
        | Furniture::PlantTall
        | Furniture::PlantFlower
        | Furniture::PlantSucculent
        | Furniture::Whiteboard
        | Furniture::Tv
        | Furniture::Bookshelf
        | Furniture::BulletinBoard
        | Furniture::ExitSign
        | Furniture::MeetingScreen
        | Furniture::IslandStand
        | Furniture::SnackShelf
        | Furniture::Desk => false,
    }
}

/// THE furniture table — one row per [`Furniture`] kind, the **single** source of
/// truth for ground shape (`footprint`) AND sprite size (`visual`) plus occupancy /
/// dwell / approach. Every geometric dependent (walkable mask, stand-point
/// half-extents, hit-test box, render centering + depth baseline) derives from this
/// row; do not re-type these numbers anywhere else.
pub const fn furniture_def(kind: Furniture) -> FurnitureDef {
    // Decor that isn't a wander destination: no dwell, approachable from anywhere
    // (unused — decor never runs stand_point). `ground_y: End` because the rows
    // that spread `..DECOR` are the overhang pieces, whose ground strip pins to the
    // sprite base; the flat singletons resolve `End` to offset 0 anyway, and the
    // two CENTERED exceptions override `ground_y` explicitly below.
    const DECOR: FurnitureDef = FurnitureDef {
        footprint: None,
        visual: Size { w: 0, h: 0 },
        occupies_pos: false,
        exclusive: false,
        dwell: DwellWindow::DECOR,
        approach: ApproachSides::ALL,
        ground_x: GroundAlign::Center,
        ground_y: GroundAlign::End,
    };
    match kind {
        Furniture::Couch => FurnitureDef {
            footprint: Some(Size { w: 8, h: 7 }),
            visual: Size { w: 8, h: 7 }, // procedural render; visual unused
            occupies_pos: true,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
            // Rotated by the SEATED facing (North, looking at the window) this
            // resolves to {N, E, W}, EXCLUDING the south backrest. A couch seat
            // walled in on ALL of N/E/W is un-sittable: `approach_point` returns
            // the `pos` sentinel and the wander SKIPS it — never the backrest.
            approach: SEAT_APPROACH,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        Furniture::Pantry => FurnitureDef {
            footprint: None,             // runtime-sized — see obstacle_footprint
            visual: Size { w: 0, h: 0 }, // runtime-sized; procedural render
            occupies_pos: false,
            exclusive: false,
            dwell: DwellWindow {
                base_ms: 10_000,
                range_ms: 8_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        Furniture::PhoneBooth => FurnitureDef {
            // Ground contact = the door/base; the booth column overhangs north
            // (invariant #6) and hides a walker behind it by its own y-sort.
            // `stand_point` parks the USER clear of the full `visual`.
            footprint: Some(Size { w: 6, h: 3 }),
            visual: Size { w: 6, h: 12 },
            occupies_pos: false,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 8_000,
                range_ms: 22_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::End,
        },
        Furniture::StandingDesk => FurnitureDef {
            // Ground contact = the legs/base; the desktop overhangs north and
            // occludes a walker behind it by its own y-sort.
            footprint: Some(Size { w: 8, h: 3 }),
            visual: Size { w: 8, h: 8 },
            occupies_pos: false,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 8_000,
                range_ms: 22_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::End,
        },
        Furniture::VendingMachine => FurnitureDef {
            footprint: Some(Size { w: 4, h: 6 }),
            visual: Size { w: 4, h: 6 },
            occupies_pos: false,
            exclusive: false,
            dwell: DwellWindow {
                base_ms: 4_000,
                range_ms: 4_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        Furniture::Printer => FurnitureDef {
            footprint: Some(Size { w: 5, h: 4 }),
            visual: Size { w: 5, h: 4 },
            occupies_pos: false,
            exclusive: false,
            dwell: DwellWindow {
                base_ms: 4_000,
                range_ms: 4_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        Furniture::MeetingSofa => FurnitureDef {
            footprint: None,
            visual: Size { w: 0, h: 0 }, // procedural render
            occupies_pos: true,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
            approach: SEAT_APPROACH,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        Furniture::MeetingChair => FurnitureDef {
            // No footprint DELIBERATELY: the chair has no furniture body to sit
            // inside (unlike the island's in-body slots), so its cell stays
            // walkable; blocking it would ripple mask/approach for a 7x7 body
            // walkers can at worst visually clip.
            footprint: None,
            visual: Size { w: 7, h: 7 },
            occupies_pos: true,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
            approach: SEAT_APPROACH,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        // Ficus + tall share the tight PLANT_FOOTPRINT ground (leaves overhang,
        // invariant #6) but each has a distinct sprite height.
        Furniture::PlantFicus => FurnitureDef {
            footprint: Some(PLANT_FOOTPRINT),
            visual: Size { w: 6, h: 7 },
            ..DECOR
        },
        Furniture::PlantTall => FurnitureDef {
            footprint: Some(PLANT_FOOTPRINT),
            visual: Size { w: 6, h: 10 },
            ..DECOR
        },
        // De-shared: a 2px terracotta pot at the sprite's south, the bloom
        // overhanging it (invariant #6).
        Furniture::PlantFlower => FurnitureDef {
            footprint: Some(Size { w: 2, h: 2 }),
            visual: Size { w: 6, h: 6 },
            ..DECOR
        },
        // 3px pot at the sprite's south, the leaf cluster overhanging it.
        Furniture::PlantSucculent => FurnitureDef {
            footprint: Some(Size { w: 3, h: 2 }),
            visual: Size { w: 5, h: 4 },
            ..DECOR
        },
        // An ELEVATED obstacle: only the wheels/stand touch the floor, the board
        // panel above them overhangs (invariant #6), and the mask SOUTH-anchors
        // the ground strip — a centered stamp lifts the block off the wheels.
        Furniture::Whiteboard => FurnitureDef {
            footprint: Some(Size { w: 10, h: 3 }),
            visual: Size { w: 14, h: 11 },
            ..DECOR
        },
        Furniture::Tv => FurnitureDef {
            // Ground contact = the wide base; the monitor + mount column overhang
            // north and occlude a walker behind the stand.
            footprint: Some(Size { w: 6, h: 2 }),
            visual: Size { w: 10, h: 10 },
            ..DECOR
        },
        // Its base dips below the window band into the room, so it needs a ground
        // footprint or a walker clips through it. The shelves above overhang that
        // base (invariant #6) and sit in the already-blocked band.
        Furniture::Bookshelf => FurnitureDef {
            footprint: Some(Size { w: 8, h: 3 }),
            visual: Size { w: 8, h: 12 },
            ..DECOR
        },
        // Truly wall-HUNG decor: no part touches the floor, so footprint stays
        // None and only `.visual` matters.
        Furniture::BulletinBoard => FurnitureDef {
            visual: Size { w: 10, h: 6 },
            ..DECOR
        },
        Furniture::ExitSign => FurnitureDef {
            visual: Size { w: 5, h: 3 },
            ..DECOR
        },
        // Stands on a soundbar base on the floor (bookshelf-class): block the
        // floor base only; the monitor panel above overhangs it and sits in the
        // already-blocked window band.
        Furniture::MeetingScreen => FurnitureDef {
            footprint: Some(Size { w: 14, h: 3 }),
            visual: Size { w: 14, h: 12 },
            ..DECOR
        },
        // Both axes are sized so `footprint + 2·OBSTACLE_PAD` lands exactly on the
        // sprite. The width is narrower than the sprite ON PURPOSE — a full-width
        // footprint plus pad disconnects the narrowest meeting room.
        Furniture::MeetingSofaBody => FurnitureDef {
            footprint: Some(Size { w: 16, h: 3 }),
            visual: Size { w: 20, h: 7 }, // == the real meeting_sofa.sprite
            // CENTERED (not south): seat settle clearance + narrowest-room
            // connectivity are tuned to the strip sitting on the sofa pos.
            ground_y: GroundAlign::Center,
            ..DECOR
        },
        // footprint == visual so the mask blocks exactly what's drawn.
        Furniture::MeetingTable => FurnitureDef {
            footprint: Some(Size { w: 11, h: 5 }),
            visual: Size { w: 11, h: 5 },
            ..DECOR
        },
        // The sprite's top rows are countertop that OVERHANGS the south-anchored
        // base (invariant #6). The bartender slots stand ON the body's center row
        // — blocked cells reached via BAR_APPROACH + the settle glide, with the
        // body's south-row z-key occluding their legs.
        Furniture::KitchenIsland => FurnitureDef {
            footprint: Some(Size { w: 20, h: 5 }),
            visual: Size { w: 20, h: 7 },
            ..DECOR
        },
        // Two shapes share this row: the E/W FLANKS, pre-positioned CLEAR of the
        // body's padded footprint, and the BARTENDER pair, whose `pos` is INSIDE
        // the island body. A blocked pos is fine for `occupies_pos` — A* routes to
        // a BAR_APPROACH cell and the settle glide bridges on.
        Furniture::IslandStand => FurnitureDef {
            footprint: None,
            visual: Size { w: 0, h: 0 },
            occupies_pos: true,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 9_000,
                range_ms: 9_000,
            },
            approach: BAR_APPROACH,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::Center,
        },
        // Tall shelf sprite, shallow walk-behind base (bookshelf-class overhang).
        // Approachable obstacle like the vending machine — `stand_point` delegates
        // to `approach_point`, which finds the open side via reachability.
        Furniture::SnackShelf => FurnitureDef {
            footprint: Some(Size { w: 7, h: 2 }),
            visual: Size { w: 7, h: 10 },
            occupies_pos: false,
            exclusive: false,
            dwell: DwellWindow {
                base_ms: 5_000,
                range_ms: 5_000,
            },
            approach: ApproachSides::ALL,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::End,
        },
        // Width 2 = the base disc only, not the pole and its empty margins.
        Furniture::FloorLamp => FurnitureDef {
            footprint: Some(Size { w: 2, h: 7 }),
            visual: Size { w: 4, h: 10 },
            // CENTERED, and far taller than the 1px disc on purpose: from a
            // centered stamp the tall footprint is what REACHES down to the disc
            // at the sprite south. `End`, or a shorter one, lifts the block off it.
            ground_y: GroundAlign::Center,
            ..DECOR
        },
        Furniture::LoungeSideTable => FurnitureDef {
            footprint: Some(Size { w: 7, h: 4 }),
            visual: Size { w: 7, h: 4 },
            ..DECOR
        },
        Furniture::FishTank => FurnitureDef {
            // Ground rule (invariant #6): only the cabinet base blocks; the glass
            // tank above it is visual overhang.
            footprint: Some(Size { w: 14, h: 3 }),
            visual: Size { w: 14, h: 11 },
            ..DECOR
        },
        // The desk's footprint is stamped TOP-LEFT in `mask.rs`, not centered, and
        // its `dwell` is the SEATED window (`pose::seated_dwell_ms`).
        Furniture::Desk => FurnitureDef {
            // Ground = the full sprite width (the side cabinets touch the floor) ×
            // the shallow front-contact depth. `End` south-anchors it so the
            // surface + monitor OVERHANG north; a walker passes behind the monitor,
            // occluded by the desk's own y-sort (invariant #6).
            footprint: Some(Size {
                w: DESK_W + 4,
                h: DESK_FOOT_H,
            }),
            visual: Size {
                w: DESK_W + 4,
                h: DESK_H + 2,
            },
            occupies_pos: true,
            exclusive: true,
            dwell: DwellWindow {
                base_ms: 15_000,
                range_ms: 15_000,
            },
            approach: DESK_APPROACH,
            ground_x: GroundAlign::Center,
            ground_y: GroundAlign::End,
        },
    }
}

/// Where a footprint sits inside its VISUAL box on ONE axis. Each variant declares
/// INTENT and resolves its pixel offset from `visual − footprint` at stamp time, so
/// it can NEVER drift when a sprite is resized — which is the whole point of this
/// type over a stored `dx: u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundAlign {
    /// Flush to the box's LOW edge — North (y) / West (x): offset 0.
    Start,
    /// Centered ON the sprite center (== the placement `pos` for a Center anchor):
    /// `floor(v/2) − floor(f/2)`, NOT `floor((v−f)/2)` — the two differ by 1px when
    /// visual and footprint have opposite parity, which lifts the floor lamp's
    /// block off its disc.
    Center,
    /// Flush to the box's HIGH edge — South (y) / East (x): offset
    /// `visual − footprint`. THE walk-behind shape (invariant #6) every overhang
    /// piece uses: a walker parks deep behind the overhang and the sprite's own
    /// y-sort occludes them.
    End,
}

impl GroundAlign {
    /// The pixel offset from the visual box's low edge for a `footprint`-long
    /// span inside a `visual`-long box. `saturating_sub` keeps a (malformed)
    /// footprint-larger-than-visual row at offset 0 rather than wrapping.
    pub(crate) const fn offset(self, visual: u16, footprint: u16) -> u16 {
        match self {
            GroundAlign::Start => 0,
            GroundAlign::Center => (visual / 2).saturating_sub(footprint / 2),
            GroundAlign::End => visual.saturating_sub(footprint),
        }
    }
}

/// The desk's blocked-GROUND width — the full sprite width (side cabinets
/// included), read from the ONE table row so the pod-grid's band-EDGE clamps
/// (`compute.rs`) price the honest ground, not the `DESK_W` SLOT width. The two
/// diverge by the side-cabinet overhang that rides the aisle, and a clamp on
/// `DESK_W` lets a desk's ground poke past the buffer edge.
pub(crate) const DESK_GROUND_W: u16 = match desk_furniture_def().footprint {
    Some(s) => s.w,
    None => panic!("Desk must carry a static footprint"),
};

/// The desk's blocked-GROUND SOUTH edge, measured from its NW corner (the desk
/// `Point`) — the Y twin of [`DESK_GROUND_W`], but deliberately NOT the footprint
/// HEIGHT: the desk is `ground_y: End` (walk-behind), so its shallow `DESK_FOOT_H`
/// strip anchors to the sprite BASE and its south edge sits the full VISUAL height
/// below the corner. `compute.rs`'s `desk_y_max` clamps on THIS so a bottom-row
/// desk's ground can't spill south into the cubicle aisle.
pub(crate) const DESK_GROUND_H: u16 = match desk_furniture_def().footprint {
    Some(fp) => {
        let def = desk_furniture_def();
        def.ground_y.offset(def.visual.h, fp.h) + fp.h
    }
    None => panic!("Desk must carry a static footprint"),
};

/// The **home desk** descriptor — sugar over the [`Furniture::Desk`] table row,
/// kept because the desk is per-agent rather than a `WaypointKind`.
pub const fn desk_furniture_def() -> FurnitureDef {
    furniture_def(Furniture::Desk)
}

/// Vertical offset baked into the walking / waypoint sprite anchor
/// (`p.y - WALKING_Y_OFF`) — the standing/walking sprite height. Owned here rather
/// than duplicated as a painter literal, so [`seated_foot_cell`] and the anchor
/// invert each other by construction instead of by two modules staying in sync.
pub const WALKING_Y_OFF: u16 = 12;
/// Vertical offset of the back-view seat sprite anchor (`pos.y - SEAT_RENDER_Y_OFF`).
/// The seat's settle cell is `WALKING_Y_OFF - SEAT_RENDER_Y_OFF` px south of `pos`,
/// where `walking_anchor` lands exactly on `back_couch_anchor`.
pub const SEAT_RENDER_Y_OFF: u16 = 7;

/// Offsets from a home desk's top-left to the agent's WALK anchor. Chosen so
/// `walking_anchor(desk_walk_anchor_facing(d, f)) == seated_anchor_facing(d, f)` — the agent settles
/// exactly onto its seat with no arrival pop, just clear of the desk obstacle.
///
/// That identity is now STRUCTURAL: `seated_anchor` derives itself from this
/// offset rather than restating the difference, so the two cannot drift. It used
/// to be two independent literals held together by a test.
pub(crate) const DESK_WALK_X_OFF: u16 = (DESK_W - CHARACTER_SPRITE_W) / 2 + 4;
/// The walk-anchor offset for a desk whose occupant sits NORTH of it, facing the
/// viewer — the only arrangement the office had before desks carried a facing.
pub(crate) const DESK_WALK_Y_OFF: u16 = 4;

/// The walk-anchor offset for a desk whose occupant sits on the NEAR side, back
/// to the viewer.
///
/// DERIVED, not chosen: at `WALKING_Y_OFF` the occupant's sprite TOP lands on the
/// desk's own row, so they overlap the desk body's lower half and their z-key —
/// which is this chair row — puts them in front of it. That is a statable
/// geometric fact; a free constant here is how a value that moved the occupant
/// ONE pixel once passed for a side swap.
pub(crate) const DESK_WALK_Y_OFF_BACK: u16 = WALKING_Y_OFF;

// A north seat's walk anchor must clear `WALKING_Y_OFF`, or `seated_anchor_facing`'s
// `saturating_sub` clamps and the desk chair's z-key tie with its occupant breaks —
// the chair then paints UNDER them with every render test green.
const _: () = assert!(DESK_WALK_Y_OFF_BACK >= WALKING_Y_OFF);

/// Where an agent walks to/from for its home `desk`, given which way that desk
/// seats its occupant.
///
/// Desks are only ever laid out along the N-S axis, so `East`/`West` are not
/// reachable; they take the viewer-facing arrangement rather than a panic,
/// because a layout bug should render a slightly wrong office, not kill the
/// render thread.
pub fn desk_walk_anchor_facing(desk: Point, facing: Facing) -> Point {
    let y_off = match facing {
        Facing::North => DESK_WALK_Y_OFF_BACK,
        Facing::South | Facing::East | Facing::West => DESK_WALK_Y_OFF,
    };
    Point {
        x: desk.x + DESK_WALK_X_OFF,
        y: desk.y + y_off,
    }
}

/// Where a desk's ceiling tube pools its light, given which way that desk seats
/// its occupant.
///
/// Derived from the SEAT, not the desk origin: the pool exists to light the
/// WORKSTATION, and which side of the desk that is became a per-desk fact the
/// day desks grew a facing. It resolves to the occupant's own vertical middle —
/// the walk anchor is their feet, so half a standing height above it is their
/// centre.
///
/// For a viewer-facing seat that is `desk.y - 2`, byte-identical to the
/// hardcoded north lift it replaced, so the historical look is untouched. A
/// back-turned seat moves the light SOUTH onto the person instead of leaving it
/// over the empty floor behind them, which is what it had been doing since the
/// pod grew a second facing. East/west follows for free, on both axes, because
/// the walk anchor is already a function of facing — no second site to remember.
pub fn desk_ceiling_pool_center(desk: Point, facing: Facing) -> Point {
    let walk = desk_walk_anchor_facing(desk, facing);
    Point {
        x: walk.x,
        y: walk.y.saturating_sub(WALKING_Y_OFF / 2),
    }
}

/// The cell where a seated agent's WALK visually ends so the seated sprite renders
/// with no arrival jump — the inverse of the render anchor under
/// [`WALKING_Y_OFF`], solving `walking_anchor(S) == render_anchor(pos)`.
///
/// `Some` for every `occupies_pos` furniture; `None` for obstacles, whose sprite
/// renders AT the approach cell rather than a fixed seat. The post-A\* settle walks
/// `approach_point → S`; when `S` is blocked (meeting sofa, desk) that final
/// segment is the "sit down" motion, not pathfinding.
pub fn seated_foot_cell(kind: Furniture, pos: Point) -> Option<Point> {
    if !furniture_def(kind).occupies_pos {
        return None;
    }
    Some(match kind {
        // Seat render (`pos.y − SEAT_RENDER_Y_OFF`): S is the one cell where
        // `walking_anchor` lands exactly on `back_couch_anchor`.
        Furniture::Couch | Furniture::MeetingSofa | Furniture::MeetingChair => Point {
            x: pos.x,
            y: pos.y + (WALKING_Y_OFF - SEAT_RENDER_Y_OFF),
        },
        // waypoint render (`== walking_anchor`): S == pos.
        Furniture::IslandStand => pos,
        // desk render is `seated_anchor`; its inverse is `desk_walk_anchor`.
        Furniture::Desk => desk_walk_anchor_facing(pos, crate::layout::Facing::South),
        // The early return handled every obstacle kind. A FUTURE occupies_pos seat
        // that forgets its arm here must fail loud, not silently settle the
        // occupant on the blocked furniture centre (walk-through-desk bugs).
        _ => unreachable!("{kind:?} sets occupies_pos but lacks a seated_foot_cell arm"),
    })
}

/// Which way a waypoint occupant faces. Drives sprite choice (back vs front view)
/// and horizontal mirroring at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facing {
    /// Facing north (−y, toward the far wall) — a back view.
    North,
    /// Facing south (+y, toward the viewer) — a front view.
    South,
    /// Facing east (+x, right).
    East,
    /// Facing west (−x, left).
    West,
}

/// Wall-mounted / wall-leaning furniture, painted as decor in the top wall area.
/// Not a wander destination — agents can't walk through their own cubicle row to
/// reach the back wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallDecor {
    /// A bookshelf against the back wall.
    Bookshelf,
    /// A wall-mounted whiteboard.
    Whiteboard,
    /// A cork bulletin board.
    BulletinBoard,
    /// An exit sign.
    ExitSign,
    /// Wall-mounted meeting-room display.
    MeetingScreen,
}

impl WallDecor {
    /// Geometry kind in the unified [`Furniture`] table. Wall decor isn't
    /// mask-stamped, so only `.visual` is read from the row.
    pub const fn furniture(self) -> Furniture {
        match self {
            WallDecor::Whiteboard => Furniture::Whiteboard,
            WallDecor::Bookshelf => Furniture::Bookshelf,
            WallDecor::BulletinBoard => Furniture::BulletinBoard,
            WallDecor::ExitSign => Furniture::ExitSign,
            WallDecor::MeetingScreen => Furniture::MeetingScreen,
        }
    }

    /// Pack-animation key for this decor's sprite. The blit lives in
    /// `pixel_painter::drawable`; the NAME lives on the enum so a new variant is a
    /// compile error HERE, not a forgotten call-site match arm. Every value must be
    /// in `OPTIONAL_FURNITURE_ANIMATIONS`.
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

/// Variety of potted plants — each renders a different sprite, so the lounge
/// doesn't read as one ficus repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlantKind {
    /// A leafy ficus in a 6px pot.
    Ficus,
    /// A tall plant (shares the ficus pot footprint).
    Tall,
    /// A small flowering plant (2×2 pot).
    Flower,
    /// A low succulent (3×2 pot).
    Succulent,
}

impl PlantKind {
    /// Geometry kind in the unified [`Furniture`] table.
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

/// Decor placed in the aisles BETWEEN 2×2 desk pods. Picked by a deterministic
/// hash of the pod index, so each office layout is varied but stable across
/// renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PodDecor {
    /// A tall plant filling the aisle.
    PlantTall,
    /// A rolling whiteboard in the aisle.
    Whiteboard,
    /// A wheeled TV cart.
    Tv,
    /// A phone booth.
    PhoneBooth,
    /// A standing desk.
    StandingDesk,
}

impl PodDecor {
    /// The randomly-picked pool. Every member's GROUND footprint has to fit the
    /// aisle width once the obstacle pad is added — the whiteboard, whose board
    /// panel overhangs its wheelbase, is the tight one.
    pub const ALL: &'static [PodDecor] = &[
        PodDecor::PlantTall,
        PodDecor::Whiteboard,
        PodDecor::Tv,
        PodDecor::PhoneBooth,
        PodDecor::StandingDesk,
    ];

    /// Geometry kind in the unified [`Furniture`] table. PlantTall resolves to the
    /// SAME row as the free-standing `PlantKind::Tall`, and PhoneBooth/StandingDesk
    /// to the same rows as their `WaypointKind` twins, so nothing drifts.
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

    #[test]
    fn repels_plants_is_exactly_the_solid_non_waypoint_bodies() {
        for k in [
            Furniture::FishTank,
            Furniture::MeetingSofaBody,
            Furniture::MeetingTable,
            Furniture::KitchenIsland,
        ] {
            assert!(
                repels_plants(k),
                "{k:?} is a solid body — must repel plants"
            );
        }
        assert!(
            !repels_plants(Furniture::FloorLamp),
            "lamp keeps the Ficus hug"
        );
        assert!(
            !repels_plants(Furniture::LoungeSideTable),
            "side table keeps the Ficus hug"
        );
        assert_eq!(
            Furniture::ALL.iter().filter(|&&k| repels_plants(k)).count(),
            4,
            "exactly four kinds repel — a new `true` must be deliberate"
        );
    }

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
        assert_eq!(allowed(SEAT_APPROACH, Facing::South), vec![S, E, W]);
    }

    #[test]
    fn seat_facing_north_rotates_to_exclude_the_south_back() {
        assert_eq!(allowed(SEAT_APPROACH, Facing::North), vec![N, E, W]);
    }

    #[test]
    fn desk_excludes_its_south_front() {
        assert_eq!(allowed(DESK_APPROACH, Facing::South), vec![N, E, W]);
        let no_east = ApproachSides {
            e: false,
            ..DESK_APPROACH
        };
        assert_eq!(allowed(no_east, Facing::South), vec![N, W]);
    }

    #[test]
    fn rotation_is_a_bijection_on_sides() {
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
        let d = desk_furniture_def();
        assert_eq!(
            d,
            furniture_def(Furniture::Desk),
            "desk_furniture_def must be sugar over the Furniture::Desk row"
        );
        assert_eq!(
            d.footprint,
            Some(Size {
                w: DESK_W + 4,
                h: DESK_FOOT_H,
            }),
            "desk footprint"
        );
        assert_eq!(
            d.ground_y,
            GroundAlign::End,
            "desk walk-behind: south-anchored"
        );
        let Size { w: fw, h: fh } = d.footprint.unwrap();
        assert!(
            fw <= d.visual.w && fh <= d.visual.h,
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
        assert!(d.dwell.range_ms > 0, "seated dwell range must be positive");
    }

    #[test]
    fn ground_align_stays_inside_the_visual_and_follows_the_declared_intent() {
        for &kind in Furniture::ALL {
            let def = furniture_def(kind);
            let Some(fp) = def.footprint else {
                continue; // no static ground
            };
            let dx = def.ground_x.offset(def.visual.w, fp.w);
            let dy = def.ground_y.offset(def.visual.h, fp.h);
            assert!(
                dx + fp.w <= def.visual.w.max(fp.w) && dy + fp.h <= def.visual.h.max(fp.h),
                "{kind:?}: blocked rect must not poke past the visual box"
            );
            let center_exception =
                matches!(kind, Furniture::MeetingSofaBody | Furniture::FloorLamp);
            if def.visual.h > fp.h && !center_exception {
                assert_eq!(
                    def.ground_y,
                    GroundAlign::End,
                    "{kind:?}: an overhang row must south-anchor (End) unless documented"
                );
            }
            assert_eq!(
                def.ground_x,
                GroundAlign::Center,
                "{kind:?}: first non-Center ground_x — update the ground_x field doc"
            );
        }
    }

    #[test]
    fn furniture_def_invariants_hold_for_every_row() {
        assert_eq!(
            Furniture::ALL.len(),
            27,
            "Furniture variant added/removed — update ALL (and this count)"
        );
        for &f in Furniture::ALL {
            let d = furniture_def(f);
            assert!(
                d.dwell == DwellWindow::DECOR || d.dwell.range_ms > 0,
                "{f:?}: half-broken dwell {:?}",
                d.dwell
            );
            let expect_occupies = matches!(
                f,
                Furniture::Couch
                    | Furniture::MeetingSofa
                    | Furniture::MeetingChair
                    | Furniture::IslandStand
                    | Furniture::Desk
            );
            assert_eq!(d.occupies_pos, expect_occupies, "{f:?}: occupies_pos");
            let expect_exclusive =
                expect_occupies || matches!(f, Furniture::PhoneBooth | Furniture::StandingDesk);
            assert_eq!(d.exclusive, expect_exclusive, "{f:?}: exclusive");
            assert!(
                !d.occupies_pos || d.exclusive,
                "{f:?}: occupies_pos implies exclusive"
            );
            if matches!(
                f,
                Furniture::MeetingSofa | Furniture::MeetingChair | Furniture::IslandStand
            ) {
                assert!(
                    d.footprint.is_none(),
                    "{f:?}: seat row must carry no footprint"
                );
            }
            if matches!(f, Furniture::PlantFicus | Furniture::PlantTall) {
                assert_eq!(
                    d.footprint,
                    Some(PLANT_FOOTPRINT),
                    "{f:?}: plant ground footprint"
                );
            }
            if matches!(f, Furniture::PlantFlower | Furniture::PlantSucculent) {
                assert!(
                    d.footprint.is_some_and(|s| s.w < PLANT_FOOTPRINT.w),
                    "{f:?}: de-shared plant must be narrower than PLANT_FOOTPRINT"
                );
            }
            if let Some(Size { w: fw, h: fh }) = d.footprint {
                assert!(
                    fw <= d.visual.w && fh <= d.visual.h,
                    "{f:?}: footprint {:?} exceeds visual {:?} (invariant #6)",
                    d.footprint,
                    d.visual
                );
            }
            // The de-shared flower/succulent are NOT in `PodDecor::ALL`, so
            // `every_pod_occludes_via_overhang` never sees them: the `≤` above is
            // the only other height check they get, and it is too weak.
            if matches!(
                f,
                Furniture::PlantFicus
                    | Furniture::PlantTall
                    | Furniture::PlantFlower
                    | Furniture::PlantSucculent
            ) {
                let Size { h: fh, .. } = d.footprint.expect("plant has a pot footprint");
                assert!(
                    d.visual.h > fh,
                    "{f:?}: plant must overhang its pot to occlude (visual.h {} > footprint.h {fh})",
                    d.visual.h
                );
            }
        }
    }

    #[test]
    fn role_enum_sprite_names_resolve_in_the_animation_registry() {
        use pixtuoid_core::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS;
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

    /// The claim the doc comment makes, proven rather than asserted in prose:
    /// deriving the pool from the seat reproduces the hardcoded north lift
    /// EXACTLY for a viewer-facing desk, so no existing render moved — while a
    /// back-turned desk's light finally follows its occupant south instead of
    /// staying over the empty floor behind them.
    #[test]
    fn the_desk_light_follows_the_seat_and_leaves_a_far_seat_where_it_was() {
        // The lift and centring the pool used before it read the facing.
        const HISTORICAL_CY_LIFT: u16 = 2;
        let desk = Point { x: 40, y: 30 };

        let far = desk_ceiling_pool_center(desk, Facing::South);
        assert_eq!(
            far,
            Point {
                x: desk.x + DESK_W / 2,
                y: desk.y - HISTORICAL_CY_LIFT,
            },
            "a viewer-facing desk must light exactly where the hardcoded lift did"
        );

        let near = desk_ceiling_pool_center(desk, Facing::North);
        assert!(
            near.y > desk.y,
            "a back-turned desk seats its occupant SOUTH, so its light must move \
             there too: {near:?} vs desk {desk:?}"
        );
        assert!(
            near.y < desk_walk_anchor_facing(desk, Facing::North).y,
            "...but stay on the body rather than drop to their feet: {near:?}"
        );
    }
}
