//! The furniture roster: every physical thing the office stands on its floor
//! or hangs on its walls, placed and depth-keyed once, for every consumer —
//! the painters, the hover hit-test, the visual-clearance probe and the
//! placement sweep all map over [`SceneLayout::fixtures`].

use super::{
    anchored_top_left, coat_rack_rect_at, furniture_def, z_sort_row, Anchor, Bounds, Facing,
    Furniture, PlantItem, PlantKind, PodDecor, PodDecorItem, Point, SceneLayout, Size, WallDecor,
    WallDecorItem, WaypointKind, ELEVATOR_H, ELEVATOR_W,
};
use pixtuoid_core::state::FloorLocalDeskIndex;

/// One placed fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixture {
    /// What it is, carrying the ids a painter joins live state on.
    pub kind: FixtureKind,
    /// The box its art covers, in logical px.
    pub visual: Bounds,
    /// Where it sorts among everything else painted.
    pub depth: Depth,
}

/// Where a fixture sorts. The derived order is the paint order: every
/// [`Depth::Backdrop`] fixture paints under every [`Depth::Sorted`] one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Depth {
    /// Painted flat under the sorted scene, in roster order: floor coverings
    /// and wall fixtures nothing stands behind.
    Backdrop,
    /// Painted in the y-sort at this row, roster order breaking ties.
    Sorted(u16),
}

/// The waypoint kinds that are furniture in their own right. The others ride
/// a piece placed elsewhere — a seat on its sofa or chair, an island stand on
/// the island, a promoted pod-decor slot on its decor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Station {
    /// The pantry counter.
    PantryCounter,
    /// A corridor vending machine.
    VendingMachine,
    /// A corridor printer.
    Printer,
    /// The pantry's snack shelf.
    SnackShelf,
}

/// Every kind of fixture. Consumers match it exhaustively, so a new kind is a
/// compile error in each until it decides how to draw, hover and clear it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FixtureKind {
    /// A home desk.
    Desk(FloorLocalDeskIndex),
    /// The filing cabinet beside a home desk.
    FilingCabinet(FloorLocalDeskIndex),
    /// The task chair of a desk whose occupant sits with their back to us.
    DeskChair(FloorLocalDeskIndex),
    /// A waypoint that is furniture itself.
    Station {
        /// Its index in [`SceneLayout::waypoints`].
        waypoint: usize,
        /// Which station.
        station: Station,
    },
    /// A free-standing plant.
    Plant {
        /// Its index in [`SceneLayout::plants`].
        item: usize,
        /// Which plant.
        kind: PlantKind,
    },
    /// Decor in the aisles between desk pods.
    Pod {
        /// Its index in [`SceneLayout::pod_decor`].
        item: usize,
        /// Which decor.
        kind: PodDecor,
    },
    /// Decor standing against or hung on a wall.
    Wall {
        /// Its index in [`SceneLayout::wall_decor`].
        item: usize,
        /// Which decor.
        kind: WallDecor,
    },
    /// The rug under a meeting room's trio.
    MeetingRug {
        /// Its room.
        room: usize,
    },
    /// A meeting-room sofa.
    MeetingSofa {
        /// Its room.
        room: usize,
        /// `0` north, `1` south.
        seat: usize,
        /// South of the table: we see its back.
        faces_away: bool,
    },
    /// A meeting room's table.
    MeetingTable {
        /// Its room.
        room: usize,
    },
    /// A head-of-table meeting chair.
    MeetingChair {
        /// Its index in [`SceneLayout::waypoints`].
        waypoint: usize,
        /// Which way its sitter faces.
        facing: Facing,
    },
    /// A meeting room's coat rack.
    CoatRack {
        /// Its room.
        room: usize,
    },
    /// The doormat outside a meeting room.
    Doormat {
        /// Its room.
        room: usize,
    },
    /// A meeting room's notice board.
    NoticeBoard {
        /// Its room.
        room: usize,
    },
    /// The rug the lounge couch stands on.
    LoungeRug,
    /// The lounge couch.
    LoungeCouch,
    /// The lounge side table.
    SideTable,
    /// The lounge floor lamp.
    FloorLamp,
    /// The lounge aquarium.
    FishTank,
    /// The pantry's kitchen island.
    KitchenIsland,
    /// The mat inside the pantry's north doorway.
    PantryMat,
    /// The bar mat under the kitchen island's serving front.
    IslandMat,
    /// The pantry's water cooler.
    WaterCooler,
    /// The pantry's trash bin.
    TrashBin,
    /// The elevator door.
    Door,
    /// The corridor's runner carpet.
    Runner,
    /// The neon sign the wall board shows its text on.
    NeonSign,
    /// The wall clock.
    Clock,
}

impl FixtureKind {
    /// Its hover label.
    pub fn name(self) -> &'static str {
        match self {
            FixtureKind::Desk(_) => "Desk",
            FixtureKind::FilingCabinet(_) => "Filing Cabinet",
            FixtureKind::DeskChair(_) => "Desk Chair",
            FixtureKind::Station { station, .. } => match station {
                Station::PantryCounter => "Pantry Counter",
                Station::VendingMachine => "Vending Machine",
                Station::Printer => "Printer",
                Station::SnackShelf => "Snack Shelf",
            },
            FixtureKind::Plant { kind, .. } => match kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            },
            FixtureKind::Pod { kind, .. } => match kind {
                PodDecor::PlantTall => "Tall Plant",
                PodDecor::Whiteboard => "Whiteboard",
                PodDecor::Tv => "TV Stand",
                PodDecor::PhoneBooth => "Phone Booth",
                PodDecor::StandingDesk => "Standing Desk",
            },
            FixtureKind::Wall { kind, .. } => match kind {
                WallDecor::Whiteboard => "Whiteboard",
                WallDecor::Bookshelf => "Bookshelf",
                WallDecor::BulletinBoard => "Bulletin Board",
                WallDecor::ExitSign => "Exit Sign",
                WallDecor::MeetingScreen => "Meeting Screen",
            },
            FixtureKind::MeetingRug { .. } | FixtureKind::LoungeRug => "Rug",
            FixtureKind::MeetingSofa { .. } => "Meeting Sofa",
            FixtureKind::MeetingTable { .. } => "Meeting Table",
            FixtureKind::MeetingChair { .. } => "Meeting Chair",
            FixtureKind::CoatRack { .. } => "Coat Rack",
            FixtureKind::Doormat { .. } | FixtureKind::PantryMat => "Doormat",
            FixtureKind::NoticeBoard { .. } => "Notice Board",
            FixtureKind::LoungeCouch => "Lounge Sofa",
            FixtureKind::SideTable => "Side Table",
            FixtureKind::FloorLamp => "Floor Lamp",
            FixtureKind::FishTank => "Fish Tank",
            FixtureKind::KitchenIsland => "Kitchen Island",
            FixtureKind::IslandMat => "Bar Mat",
            FixtureKind::WaterCooler => "Water Cooler",
            FixtureKind::TrashBin => "Trash Bin",
            FixtureKind::Door => "Elevator",
            FixtureKind::Runner => "Runner",
            FixtureKind::NeonSign => "Neon Sign",
            FixtureKind::Clock => "Clock",
        }
    }
}

/// The wall clock's face.
pub(crate) const CLOCK: Size = Size { w: 7, h: 7 };
/// The wall clock's top row.
const CLOCK_Y: u16 = 1;

/// The neon sign's outer box, frame included. A pixel column is a terminal
/// cell column in the half-block flush, so these widths are cell widths too.
pub(crate) const NEON_PANEL_X: u16 = 1;
pub(crate) const NEON_PANEL_Y: u16 = 1;
/// The neon sign's OUTER width in pixels (frame included).
pub const NEON_PANEL_W: u16 = 30;
pub(crate) const NEON_PANEL_H: u16 = 8;
/// The frame thickness `paint_neon_panel` lights on every side — it reads THIS,
/// so the interior derivations below match the pixels it leaves dark.
pub(crate) const NEON_PANEL_BORDER: u16 = 1;
/// The dark interior's left cell-origin — where board text starts. The board's
/// text pins to the interior, not the outer box, or the lit text overruns the
/// glowing frame by the border on each side.
pub const NEON_PANEL_INNER_X: u16 = NEON_PANEL_X + NEON_PANEL_BORDER;
/// The dark interior's cell WIDTH — the board's usable text width.
pub const NEON_PANEL_INNER_W: u16 = NEON_PANEL_W - 2 * NEON_PANEL_BORDER;
/// The dark interior's top pixel-origin — where the floating / wasm painters
/// anchor the board's first text row.
pub const NEON_PANEL_INNER_Y: u16 = NEON_PANEL_Y + NEON_PANEL_BORDER;
/// The dark interior's pixel HEIGHT.
pub const NEON_PANEL_INNER_H: u16 = NEON_PANEL_H - 2 * NEON_PANEL_BORDER;
// The interior must be a non-empty strict subset of the outer frame (catches a
// degenerate BORDER=0 / oversized-border config at compile time).
const _: () = assert!(NEON_PANEL_INNER_W > 0 && NEON_PANEL_INNER_W < NEON_PANEL_W);
const _: () = assert!(NEON_PANEL_INNER_H > 0 && NEON_PANEL_INNER_H < NEON_PANEL_H);

/// The neon sign's outer box.
pub(crate) const NEON_PANEL: Bounds = Bounds {
    x: NEON_PANEL_X,
    y: NEON_PANEL_Y,
    width: NEON_PANEL_W,
    height: NEON_PANEL_H,
};

/// How far north of the couch centre the lounge rug sorts: under the couch,
/// which then sits on it.
const LOUNGE_RUG_Z_LEAD: u16 = 2;

/// A centre-pinned `size` box at `pos`.
fn centred(pos: Point, size: Size) -> Bounds {
    boxed(anchored_top_left(Anchor::Center, pos, size.w, size.h), size)
}

fn boxed(tl: Point, size: Size) -> Bounds {
    Bounds {
        x: tl.x,
        y: tl.y,
        width: size.w,
        height: size.h,
    }
}

/// A centre-pinned furniture row at `pos`, sorting at its south row.
fn centred_row(kind: FixtureKind, pos: Point, row: Furniture) -> Fixture {
    let size = furniture_def(row).visual;
    Fixture {
        kind,
        visual: centred(pos, size),
        depth: Depth::Sorted(z_sort_row(Anchor::Center, pos, size.h)),
    }
}

impl SceneLayout {
    /// Every fixture of this office, backdrop first and then in the order that
    /// breaks the y-sort's ties.
    ///
    /// The ONE destructure of the layout with no `..`: a new collection is a
    /// compile error here until it is rostered or bound away with its reason,
    /// and every consumer then meets its kinds in an exhaustive match.
    pub fn fixtures(&self) -> Vec<Fixture> {
        let SceneLayout {
            // Read through `clock_pos`.
            buf_w: _,
            buf_h,
            // Containers the pieces were placed in, not pieces.
            cubicle_band: _,
            cubicle_aisle: _,
            home_desks,
            // A desk attribute, read through `desk_facing`.
            desk_facings: _,
            waypoints,
            plants,
            wall_decor,
            pod_decor,
            lounge,
            door,
            // A walkable point, not a thing.
            door_threshold: _,
            meeting_rooms,
            pantry,
            // Architecture, which the painters draw from `wall_pieces`; the
            // pantry mat reads `doorways` through `pantry_entry_mat`.
            room_walls: _,
            doorways: _,
            wall_pieces: _,
            // Wall-band geometry, read through `wall_band_h`.
            top_margin: _,
            corridor,
            // What the fixtures stamp, not fixtures.
            walkable: _,
            reachable: _,
        } = self;

        let backdrop = |kind, visual| Fixture {
            kind,
            visual,
            depth: Depth::Backdrop,
        };
        let mut out = vec![
            backdrop(FixtureKind::NeonSign, NEON_PANEL),
            backdrop(FixtureKind::Clock, boxed(self.clock_pos(), CLOCK)),
        ];
        out.extend(corridor.map(|b| backdrop(FixtureKind::Runner, b)));
        for (room, r) in meeting_rooms.iter().enumerate() {
            out.extend(
                r.notice_board_rect()
                    .map(|b| backdrop(FixtureKind::NoticeBoard { room }, b)),
            );
            out.extend(
                r.doormat_rect()
                    .map(|b| backdrop(FixtureKind::Doormat { room }, b)),
            );
        }
        // Mats before the upright pantry fixtures: on a narrow pantry the entry
        // mat reaches the water cooler's column.
        out.extend(
            self.pantry_entry_mat()
                .map(|b| backdrop(FixtureKind::PantryMat, b)),
        );
        out.extend(
            self.island_bar_mat()
                .map(|b| backdrop(FixtureKind::IslandMat, b)),
        );
        if let Some(p) = pantry {
            out.extend(
                p.water_cooler_rect()
                    .map(|b| backdrop(FixtureKind::WaterCooler, b)),
            );
            out.extend(
                p.trash_bin_rect()
                    .map(|b| backdrop(FixtureKind::TrashBin, b)),
            );
        }

        let desk = furniture_def(Furniture::Desk).visual;
        for (i, &at) in home_desks.iter().enumerate() {
            let local = FloorLocalDeskIndex(i);
            let depth = Depth::Sorted(at.y + desk.h);
            if let Some(tl) = self.filing_cabinet_top_left(local) {
                out.push(Fixture {
                    kind: FixtureKind::FilingCabinet(local),
                    visual: boxed(tl, furniture_def(Furniture::FilingCabinet).visual),
                    depth,
                });
            }
            out.push(Fixture {
                kind: FixtureKind::Desk(local),
                visual: boxed(at, desk),
                depth,
            });
        }

        let trios = || {
            meeting_rooms
                .iter()
                .enumerate()
                .filter_map(|(room, r)| r.trio.map(|t| (room, t)))
        };
        for (room, trio) in trios() {
            let rug = trio.rug(*buf_h);
            out.push(Fixture {
                kind: FixtureKind::MeetingRug { room },
                visual: rug,
                depth: Depth::Sorted(rug.y),
            });
        }
        for (room, trio) in trios() {
            for (seat, sofa) in trio.sofas.into_iter().enumerate() {
                let faces_away = sofa.y >= trio.table.y;
                out.push(Fixture {
                    kind: FixtureKind::MeetingSofa {
                        room,
                        seat,
                        faces_away,
                    },
                    visual: centred(sofa, furniture_def(Furniture::MeetingSofaBody).visual),
                    // A sofa we see the back of sorts past its sitters to hide
                    // them; a front one ties them, and they sit on it.
                    depth: Depth::Sorted(super::seated_z_key(sofa) + u16::from(faces_away)),
                });
            }
        }
        for (room, trio) in trios() {
            out.push(centred_row(
                FixtureKind::MeetingTable { room },
                trio.table,
                Furniture::MeetingTable,
            ));
        }

        if let Some(island) = pantry.and_then(|p| p.kitchen_island) {
            out.push(centred_row(
                FixtureKind::KitchenIsland,
                island,
                Furniture::KitchenIsland,
            ));
        }
        if let Some(l) = lounge {
            out.push(Fixture {
                kind: FixtureKind::LoungeRug,
                visual: l.rug(),
                depth: Depth::Sorted(l.couch_center.y.saturating_sub(LOUNGE_RUG_Z_LEAD)),
            });
            out.push(centred_row(
                FixtureKind::LoungeCouch,
                l.couch_center,
                Furniture::MeetingSofaBody,
            ));
            out.push(centred_row(
                FixtureKind::SideTable,
                l.side_table,
                Furniture::LoungeSideTable,
            ));
        }

        for (waypoint, wp) in waypoints.iter().enumerate() {
            let station = match wp.kind {
                WaypointKind::Pantry => Station::PantryCounter,
                WaypointKind::VendingMachine => Station::VendingMachine,
                WaypointKind::Printer => Station::Printer,
                WaypointKind::SnackShelf => Station::SnackShelf,
                // Seats on the lounge couch, a meeting sofa, a meeting chair
                // (rostered below, after the lamp) or the island.
                WaypointKind::Couch
                | WaypointKind::MeetingSofa
                | WaypointKind::MeetingChair
                | WaypointKind::Island => continue,
                // Promoted pod-decor slots: their decor is the fixture.
                WaypointKind::PhoneBooth | WaypointKind::StandingDesk => continue,
            };
            let kind = FixtureKind::Station { waypoint, station };
            out.push(match station {
                // Runtime-sized: the furniture row is empty on purpose.
                Station::PantryCounter => {
                    let size = self.pantry_counter_size();
                    Fixture {
                        kind,
                        visual: centred(wp.pos, size),
                        depth: Depth::Sorted(z_sort_row(Anchor::Center, wp.pos, size.h)),
                    }
                }
                Station::VendingMachine | Station::Printer | Station::SnackShelf => {
                    centred_row(kind, wp.pos, wp.kind.furniture())
                }
            });
        }

        for (item, &PodDecorItem { kind, pos }) in pod_decor.iter().enumerate() {
            out.push(centred_row(
                FixtureKind::Pod { item, kind },
                pos,
                kind.furniture(),
            ));
        }
        for (item, &PlantItem { kind, pos }) in plants.iter().enumerate() {
            out.push(centred_row(
                FixtureKind::Plant { item, kind },
                pos,
                kind.furniture(),
            ));
        }

        if let Some(l) = lounge {
            out.push(centred_row(
                FixtureKind::FloorLamp,
                l.floor_lamp,
                Furniture::FloorLamp,
            ));
        }
        for (waypoint, wp) in waypoints.iter().enumerate() {
            if wp.kind != WaypointKind::MeetingChair {
                continue;
            }
            out.push(Fixture {
                kind: FixtureKind::MeetingChair {
                    waypoint,
                    facing: wp.facing,
                },
                visual: centred(wp.pos, furniture_def(Furniture::MeetingChair).visual),
                // One row under its sitter, who sits on it.
                depth: Depth::Sorted(super::seated_z_key(wp.pos) - 1),
            });
        }
        if let Some(tank) = lounge.and_then(|l| l.fish_tank) {
            out.push(centred_row(
                FixtureKind::FishTank,
                tank,
                Furniture::FishTank,
            ));
        }
        for (room, r) in meeting_rooms.iter().enumerate() {
            if let Some(visual) = r.coat_rack_pos().map(coat_rack_rect_at) {
                out.push(Fixture {
                    kind: FixtureKind::CoatRack { room },
                    visual,
                    depth: Depth::Sorted(visual.y + visual.height - 1),
                });
            }
        }
        if let Some(at) = door {
            let size = Size {
                w: ELEVATOR_W,
                h: ELEVATOR_H,
            };
            out.push(Fixture {
                kind: FixtureKind::Door,
                visual: boxed(*at, size),
                depth: Depth::Sorted(at.y + ELEVATOR_H),
            });
        }
        for (item, &WallDecorItem { kind, pos }) in wall_decor.iter().enumerate() {
            let size = furniture_def(kind.furniture()).visual;
            out.push(Fixture {
                kind: FixtureKind::Wall { item, kind },
                visual: boxed(pos, size),
                depth: Depth::Sorted(z_sort_row(Anchor::TopLeft, pos, size.h)),
            });
        }
        // Last: they tie with their sitters, whom the painters queue before them.
        for (i, &at) in home_desks.iter().enumerate() {
            let local = FloorLocalDeskIndex(i);
            let facing = self.desk_facing(local);
            if let Some(tl) = desk_chair_top_left(at, facing) {
                out.push(Fixture {
                    kind: FixtureKind::DeskChair(local),
                    visual: boxed(tl, furniture_def(Furniture::DeskChair).visual),
                    depth: Depth::Sorted(desk_chair_z_key(at, facing)),
                });
            }
        }
        out
    }

    /// Whether desk `i` stands a filing cabinet beside it.
    pub(crate) fn desk_has_cabinet(&self, i: FloorLocalDeskIndex) -> bool {
        i.0.is_multiple_of(2)
    }

    /// Where desk `i`'s filing cabinet stands — one clear column west of the
    /// desk — or `None` for a desk without one, or whose cabinet would run off
    /// the office's south edge.
    pub(crate) fn filing_cabinet_top_left(&self, i: FloorLocalDeskIndex) -> Option<Point> {
        let desk = *self.home_desks.get(i.0)?;
        let cab = furniture_def(Furniture::FilingCabinet).visual;
        (self.desk_has_cabinet(i) && desk.y + cab.h <= self.buf_h).then(|| Point {
            x: desk.x.saturating_sub(cab.w + 1),
            y: desk.y,
        })
    }

    /// The wall clock's top-left: centred on the wall, clear of the neon
    /// sign's east edge by a column.
    pub(crate) fn clock_pos(&self) -> Point {
        Point {
            x: (self.buf_w / 2)
                .saturating_sub(CLOCK.w / 2)
                .max(NEON_PANEL_X + NEON_PANEL_W + 1),
            y: CLOCK_Y,
        }
    }

    /// The mat inside the pantry's north doorway, one clear floor row south of
    /// the wall face, or `None` without a pantry or that doorway.
    pub(crate) fn pantry_entry_mat(&self) -> Option<Bounds> {
        const ENTRY_MAT: Size = Size { w: 16, h: 5 };
        let p = self.pantry?;
        let dw = self
            .doorways
            .iter()
            .find(|d| d.start.y == d.end.y && d.start.y == p.bounds.y)?;
        Some(centred(
            Point {
                x: (dw.start.x + dw.end.x) / 2,
                y: dw.start.y + super::WALL_THICK_H + 1 + ENTRY_MAT.h / 2,
            },
            ENTRY_MAT,
        ))
    }

    /// The thin bar mat under the kitchen island: the island covers most of
    /// it, leaving a sliver along the bar's south serving front.
    pub(crate) fn island_bar_mat(&self) -> Option<Bounds> {
        const BAR_MAT: Size = Size { w: 26, h: 4 };
        // Drops the mat's centre from the island's to the seat row, so the
        // sliver clears the body's south edge (mock-verified).
        const BAR_MAT_Y_OFF: u16 = 4;
        let isl = self.pantry.and_then(|p| p.kitchen_island)?;
        Some(centred(
            Point {
                x: isl.x,
                y: isl.y + BAR_MAT_Y_OFF,
            },
            BAR_MAT,
        ))
    }

    /// The floor lamp's base — the row its sprite stands on, where its halo
    /// pools — or `None` without a lounge.
    pub(crate) fn floor_lamp_base(&self) -> Option<Point> {
        let lamp = self.floor_lamp()?;
        Some(Point {
            x: lamp.x,
            y: z_sort_row(
                Anchor::Center,
                lamp,
                furniture_def(Furniture::FloorLamp).visual.h,
            ),
        })
    }

    /// The coffee machine's box on the pantry counter, or `None` without one.
    pub fn coffee_machine(&self) -> Option<Bounds> {
        let wp = self
            .waypoints
            .iter()
            .find(|w| w.kind == WaypointKind::Pantry)?;
        let size = self.pantry_counter_size();
        let counter = centred(wp.pos, size);
        let (x0, x1) = coffee_machine_cols(size.w);
        Some(Bounds {
            x: counter.x + x0,
            y: counter.y,
            width: x1 - x0,
            height: size.h,
        })
    }
}

/// The coffee machine's columns `[start, end)` within the large pantry
/// counter's sprite — the steam and the click target both sit inside them.
const PANTRY_COFFEE_COLS_LARGE: (u16, u16) = (11, 18);
/// The coffee machine's columns within the compact `pantry_small` sprite.
const PANTRY_COFFEE_COLS_SMALL: (u16, u16) = (9, 12);

/// The coffee machine's columns within a `counter_w`-wide counter's sprite.
pub(crate) fn coffee_machine_cols(counter_w: u16) -> (u16, u16) {
    if counter_w >= super::PANTRY_COUNTER_LARGE_W {
        PANTRY_COFFEE_COLS_LARGE
    } else {
        PANTRY_COFFEE_COLS_SMALL
    }
}

/// Where the task chair stands at `desk`, or `None` for a desk that does not
/// face north: a viewer-facing occupant sits behind their desk, in front of
/// their chair.
pub(crate) fn desk_chair_top_left(desk: Point, facing: Facing) -> Option<Point> {
    /// The backrest crosses the occupant's lower torso deliberately — clearing
    /// the sprite would leave a detached slab at their feet.
    const CHAIR_BACK_TOP_DY: u16 = 6;
    (facing == Facing::North).then(|| Point {
        x: super::desk_walk_anchor_facing(desk, facing)
            .x
            .saturating_sub(furniture_def(Furniture::DeskChair).visual.w / 2),
        y: desk.y + CHAIR_BACK_TOP_DY,
    })
}

/// The depth a desk's task chair sorts at: its seat's own z-key, which the sim
/// gives the occupant arriving at, sitting in and leaving the seat alike. A
/// painter that draws chairs after people on a tie therefore draws the chair
/// over its occupant throughout — no flip where the walk ends and the sit
/// begins.
pub(crate) fn desk_chair_z_key(desk: Point, facing: Facing) -> u16 {
    super::desk_walk_anchor_facing(desk, facing).y
}

#[cfg(test)]
mod tests;
