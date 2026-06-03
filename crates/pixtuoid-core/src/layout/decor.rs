//! Decor vocabulary used by `SceneLayout` — the enums describing every
//! piece of furniture and waypoint kind in the office. Kept separate from
//! geometry so adding a new sprite kind doesn't churn the layout math.

use super::{Point, Size, DESK_H, DESK_W};

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

/// Per-spot idle dwell window. `range_ms == 0` is the DECOR sentinel (not a
/// wander destination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DwellWindow {
    pub base_ms: u64,
    pub range_ms: u64,
}
impl DwellWindow {
    pub const DECOR: DwellWindow = DwellWindow {
        base_ms: 0,
        range_ms: 0,
    };
}

/// Plant GROUND footprint — the one geometry VALUE shared by the ficus + tall
/// plant rows in [`furniture_def`]: a shallow 6×3 POT strip the mask
/// south-anchors to the sprite's base. The 7-10px leafy canopy overhangs it
/// (top-down rule, invariant #6) and a walker parking north of the pot is
/// occluded by that canopy's own y-sort — no synthetic cap. Read only THROUGH
/// the table (`furniture_def(_).footprint`), never directly.
pub(crate) const PLANT_FOOTPRINT: Size = Size { w: 6, h: 3 };

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
/// choices (sprite name) deliberately stay in the tui crate — `pixtuoid-core`
/// has no terminal deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureDef {
    /// Ground footprint `(w, h)` the walkable mask stamps (top-down z=0
    /// rect), or `None` for slots that add no obstacle of their own
    /// (MeetingSofa/MeetingStand sit on sofa/table furniture stamped
    /// elsewhere). NB: `Pantry` is also `None` here because its footprint is
    /// runtime-sized (`pantry_counter_size`); `obstacle_footprint`
    /// special-cases it — the one kind whose shape isn't a static literal.
    pub footprint: Option<Size>,
    /// Visual sprite size `(w, h)` in buffer px — the SECOND geometry axis,
    /// kept distinct from `footprint` (the top-down ground rule, invariant #6):
    /// a sprite legitimately overhangs its ground base (a tall plant's leaves, a
    /// floor lamp's shade). Render centering + the z-sort south row derive from
    /// this; the mask derives from `footprint`. Conflating the two is exactly the
    /// canopy-over-aisle bug this split prevents. For furniture rendered
    /// procedurally with its own anchors (couch / pantry counter / meeting sofa)
    /// this is unused — set to the footprint or `(0, 0)`.
    pub visual: Size,
    /// The agent occupies `pos` DIRECTLY (sprite renders ON the furniture),
    /// so `stand_point` passes `pos` through unchanged instead of resolving a
    /// walkable cell beside the furniture (A* then snaps the walk adjacent).
    /// NOT "a human can sit here": `MeetingStand` is *standing* yet sets this
    /// true (the agent still occupies its `pos`). Opposite case (Pantry/
    /// vending/printer/phone-booth/standing-desk): `pos` = blocked obstacle
    /// CENTER, approached from a side. True set: {Couch, MeetingSofa,
    /// MeetingStand}. (Desks are NOT rows here — home workstation is separate.)
    pub occupies_pos: bool,
    /// Per-spot idle dwell window. `range_ms == 0` (the `DECOR` rows) marks a
    /// kind that is NOT a wander destination and is never fed to
    /// `pose::dwell_ms`; `range_ms > 0` marks a destination.
    /// `dwell_ms` guards with `% range_ms.max(1)`, so a zero range is safe — it
    /// IS the decor sentinel, not a bug. Do not "fix" a decor row to a non-zero
    /// range (that silently turns it into a wander destination).
    pub dwell: DwellWindow,
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
        visual: Size { w: 0, h: 0 },
        occupies_pos: false,
        dwell: DwellWindow::DECOR,
        approach: ApproachSides::ALL,
    };
    match kind {
        Furniture::Couch => FurnitureDef {
            footprint: Some(Size { w: 8, h: 7 }),
            visual: Size { w: 8, h: 7 }, // procedural render; visual unused
            occupies_pos: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
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
            footprint: None,             // runtime-sized — see obstacle_footprint
            visual: Size { w: 0, h: 0 }, // runtime-sized; procedural render
            occupies_pos: false,
            dwell: DwellWindow {
                base_ms: 10_000,
                range_ms: 8_000,
            },
            approach: ApproachSides::ALL,
        },
        Furniture::PhoneBooth => FurnitureDef {
            // Ground contact = the door/base (the bottom ~3 rows); the 12px booth
            // column overhangs north (invariant #6). A walker behind it is hidden
            // by the booth's own y-sorted sprite — no cap. stand_point parks the
            // USER clear of the full `visual`, not this shallow strip.
            footprint: Some(Size { w: 6, h: 3 }),
            visual: Size { w: 6, h: 12 },
            occupies_pos: false,
            dwell: DwellWindow {
                base_ms: 8_000,
                range_ms: 22_000,
            },
            approach: ApproachSides::ALL,
        },
        Furniture::StandingDesk => FurnitureDef {
            // Ground contact = the legs/base (bottom ~3 rows); the desktop
            // overhangs north and occludes a walker behind it (its own y-sort).
            footprint: Some(Size { w: 8, h: 3 }),
            visual: Size { w: 8, h: 8 },
            occupies_pos: false,
            dwell: DwellWindow {
                base_ms: 8_000,
                range_ms: 22_000,
            },
            approach: ApproachSides::ALL,
        },
        Furniture::VendingMachine => FurnitureDef {
            footprint: Some(Size { w: 4, h: 6 }),
            visual: Size { w: 4, h: 6 },
            occupies_pos: false,
            dwell: DwellWindow {
                base_ms: 4_000,
                range_ms: 4_000,
            },
            approach: ApproachSides::ALL,
        },
        Furniture::Printer => FurnitureDef {
            footprint: Some(Size { w: 5, h: 4 }),
            visual: Size { w: 5, h: 4 },
            occupies_pos: false,
            dwell: DwellWindow {
                base_ms: 4_000,
                range_ms: 4_000,
            },
            approach: ApproachSides::ALL,
        },
        Furniture::MeetingSofa => FurnitureDef {
            footprint: None,
            visual: Size { w: 0, h: 0 }, // procedural render
            occupies_pos: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
            approach: SEAT_APPROACH,
        },
        Furniture::MeetingStand => FurnitureDef {
            footprint: None,
            visual: Size { w: 0, h: 0 }, // procedural render
            occupies_pos: true,
            dwell: DwellWindow {
                base_ms: 20_000,
                range_ms: 20_000,
            },
            approach: SEAT_APPROACH,
        },
        // Plants: all share the tight PLANT_FOOTPRINT ground (leaves overhang,
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
        // De-shared: a 2px terracotta pot at the sprite's south; the bloom
        // overhangs it. The mask south-anchors this shallow pot strip (invariant
        // #6); the bloom's own y-sort occludes a walker behind it.
        Furniture::PlantFlower => FurnitureDef {
            footprint: Some(Size { w: 2, h: 2 }),
            visual: Size { w: 6, h: 6 },
            ..DECOR
        },
        // 3px pot at the sprite's south; the leaf cluster overhangs it. The mask
        // south-anchors this shallow pot strip (invariant #6).
        Furniture::PlantSucculent => FurnitureDef {
            footprint: Some(Size { w: 3, h: 2 }),
            visual: Size { w: 5, h: 4 },
            ..DECOR
        },
        // The rolling whiteboard is an ELEVATED obstacle: only its wheels/stand
        // (the bottom 3 sprite rows — legs at rows 8-9, wheels at row 10) touch
        // the floor; the 8-px board panel above them overhangs (invariant #6, the
        // canopy rule). GROUND footprint = the 10-px wheel span × the 3-px base
        // ONLY. `mask.rs` SOUTH-anchors this strip to the sprite's base (the
        // `Center`/`TopLeft` stamp would otherwise center the short strip on the
        // panel, lifting the block off the wheels), so a walker can pass BEHIND
        // the panel and is occluded by it — the 8-px panel overhang paints over
        // them via its own y-sort, no cap. Stamped via PodDecor (aisle) or
        // WallDecor (the free-standing board in the room).
        Furniture::Whiteboard => FurnitureDef {
            footprint: Some(Size { w: 10, h: 3 }),
            visual: Size { w: 14, h: 11 },
            ..DECOR
        },
        Furniture::Tv => FurnitureDef {
            // Ground contact = the wide base (bottom 2 rows); the monitor + mount
            // column overhang north and occlude a walker behind the stand.
            footprint: Some(Size { w: 6, h: 2 }),
            visual: Size { w: 10, h: 10 },
            ..DECOR
        },
        // Bookshelf stands ON the floor against the back wall (its base dips
        // below the window band into the room). It needs a ground footprint or
        // a walker clips through its base. The shelves above overhang that base
        // (invariant #6), so the mask south-anchors the shallow 3px base strip
        // to the sprite bottom (`stamp_south_strip`, the wall-decor loop) — the
        // upper shelves sit in the already-blocked window band.
        Furniture::Bookshelf => FurnitureDef {
            footprint: Some(Size { w: 8, h: 3 }),
            visual: Size { w: 8, h: 12 },
            ..DECOR
        },
        // Truly wall-HUNG decor — flush against the wall up in the band, no part
        // touches the floor, so footprint stays None and only `.visual` matters.
        Furniture::BulletinBoard => FurnitureDef {
            visual: Size { w: 10, h: 6 },
            ..DECOR
        },
        Furniture::ExitSign => FurnitureDef {
            visual: Size { w: 5, h: 3 },
            ..DECOR
        },
        // The big meeting "TV"/presentation screen stands on a soundbar base on
        // the floor (same as the bookshelf — its base dips below the window band
        // into the room). Block the 3px floor base (south-anchored to the sprite
        // bottom); the monitor panel above overhangs it and sits in the blocked
        // window band.
        Furniture::MeetingScreen => FurnitureDef {
            footprint: Some(Size { w: 14, h: 3 }),
            visual: Size { w: 14, h: 12 },
            ..DECOR
        },
        // Singleton / per-room furniture (rendered procedurally, so `visual` is
        // mostly informational; the mask stamps `footprint`). Both axes are
        // sized so `footprint + 2·OBSTACLE_PAD` lands exactly on the 20×7 sprite:
        // width `16 + 4 = 20`, height `3 + 4 = 7`. The width is 16 (not 20) ON
        // PURPOSE — a literal 20-wide footprint becomes 24 with pad and
        // disconnects the narrowest meeting room (caught by the connectivity
        // test). The height was 7 (→ 11 blocked with pad), over-blocking 2px of
        // walkable floor off the sofa's front and back; 3 tightens the blocked
        // rect to the sprite so the red debug footprint hugs the sofa.
        Furniture::MeetingSofaBody => FurnitureDef {
            footprint: Some(Size { w: 16, h: 3 }),
            visual: Size { w: 20, h: 7 }, // == the real meeting_sofa.sprite (20w × 7 data rows)
            ..DECOR
        },
        // 11×5 = the real coffee-table sprite (paint_coffee_table). footprint ==
        // visual so the mask blocks exactly what's drawn; the MeetingStand west
        // offset (compute.rs, t.x-9) still clears (padded west edge = cx-7).
        Furniture::MeetingTable => FurnitureDef {
            footprint: Some(Size { w: 11, h: 5 }),
            visual: Size { w: 11, h: 5 },
            ..DECOR
        },
        Furniture::PantryTable => FurnitureDef {
            footprint: Some(Size { w: 7, h: 4 }),
            visual: Size { w: 7, h: 4 },
            ..DECOR
        },
        // 2×2 = exactly the four pixels the painter draws (no back / legs). Both
        // footprint AND visual match the draw; mask.rs stamps it CENTERED so it
        // sits on the visible stool, not 1px north/west of it (the old 3×3 +
        // left/top-biased stamp blocked floor where nothing was drawn).
        Furniture::PantryChair => FurnitureDef {
            footprint: Some(Size { w: 2, h: 2 }),
            visual: Size { w: 2, h: 2 },
            ..DECOR
        },
        // Width 2 = the 2px base disc (was 4, over-blocking the 1px pole + empty
        // margins). Height 7 is deliberate, NOT the disc's 1px: the disc sits at
        // the sprite SOUTH, so the centered stamp + pad must REACH down to
        // lamp.y+4 — a height shrink would lift the block off the disc entirely.
        Furniture::FloorLamp => FurnitureDef {
            footprint: Some(Size { w: 2, h: 7 }),
            visual: Size { w: 4, h: 10 },
            ..DECOR
        },
        Furniture::LoungeSideTable => FurnitureDef {
            footprint: Some(Size { w: 7, h: 4 }),
            visual: Size { w: 7, h: 4 },
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
            footprint: Some(Size {
                w: DESK_W + 4,
                h: DESK_H,
            }),
            visual: Size {
                w: DESK_W + 4,
                h: DESK_H + 2,
            },
            occupies_pos: true,
            dwell: DwellWindow {
                base_ms: 15_000,
                range_ms: 15_000,
            },
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
        // return handled every obstacle kind. A FUTURE occupies_pos seat that
        // forgets its arm here must fail loud, not silently settle the occupant
        // on the blocked furniture centre (the walk-through-desk class of bug).
        _ => unreachable!("{kind:?} sets occupies_pos but lacks a seated_foot_cell arm"),
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
    /// as `footprint`/`visual`). Every value is in `OPTIONAL_FURNITURE_ANIMATIONS`,
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
        assert_eq!(
            d.footprint,
            Some(Size {
                w: DESK_W + 4,
                h: DESK_H
            }),
            "desk footprint"
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
            // dwell is either the decor sentinel (range_ms 0) or a real window
            // (range>0); never half-broken like (n, 0) — see FurnitureDef::dwell.
            assert!(
                d.dwell == DwellWindow::DECOR || d.dwell.range_ms > 0,
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
                    d.footprint.is_some_and(|s| s.w < PLANT_FOOTPRINT.w),
                    "{f:?}: de-shared plant must be narrower than PLANT_FOOTPRINT"
                );
            }
            // Footprint never exceeds the sprite — the mask blocks only the
            // ground projection, never more than is drawn (invariant #6). The
            // succulent inversion (6×6 footprint under a 5×4 sprite) is the bug
            // this guards against recurring.
            if let Some(Size { w: fw, h: fh }) = d.footprint {
                assert!(
                    fw <= d.visual.w && fh <= d.visual.h,
                    "{f:?}: footprint {:?} exceeds visual {:?} (invariant #6)",
                    d.footprint,
                    d.visual
                );
            }
            // Occlusion is emergent now (no `occludes_behind` field): an
            // overhanging obstacle is south-anchored by the mask so its own sprite
            // y-sorts over a walker behind it. The plants (incl. the de-shared
            // flower/succulent NOT in `PodDecor::ALL`, so `every_pod_occludes_via_
            // overhang` misses them) must STRICTLY overhang their pot — visual
            // taller than the shallow footprint — else a walker behind them isn't
            // hidden. `≤` above isn't enough for these; assert strict `<`.
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
