//! The generative placement-invariant sweep over a sizes × seeds grid.
//!
//! Piece rects come from the SAME `mask::ground_rect` / `pantry_ground_rect`
//! the walkable mask stamps, so the sweep can never drift from the collision
//! truth, and [`pieces`] destructures `SceneLayout` with NO `..` so a new
//! furniture collection fails compilation here until it is swept or exempted.

use super::decor::DESK_GROUND_H;
use super::mask::pantry_ground_rect;
use super::placement::rects_overlap;
use super::*;

/// The sweep's size axis: the forced-single-pod widths (floored by
/// `MIN_LAYOUT_W`), the SHORT band the derived height floor opened (46-58 buffer
/// px = 24-30 rows, which the old hand-written floor refused sight-unseen), the
/// decor-vs-wall and Y-overflow corners, the golden and live wasm hero buffers,
/// and a spread wide enough that the appliance kinds appear.
///
/// Module-private on purpose: the routability guards that need this axis live
/// HERE so the placement axis and the routability axis can't disagree — they
/// did, and the narrow band was swept for placement but never for routability.
const SWEEP_SIZES: &[(u16, u16)] = &[
    (super::compute::MIN_LAYOUT_W, super::compute::MIN_LAYOUT_H),
    (super::compute::MIN_LAYOUT_W, 60),
    (super::compute::MIN_LAYOUT_W + 2, 100),
    (48, 46),
    (64, 48),
    (80, 46),
    (96, 52),
    (120, 46),
    (160, 50),
    (200, 46),
    (128, 56),
    (38, 120),
    (40, 70),
    (41, 160),
    (48, 60),
    (50, 80),
    // The only size here that reaches the #566 guard, so every invariant below gets one
    // layout whose decor the guard degraded.
    (59, 148),
    (64, 130),
    (96, 60),
    (96, 70),
    (96, 100),
    (96, 115),
    // The only size here with a partial BOTTOM row and a partial COLUMN beside it — a
    // `compute_pod_desks` branch no other size reaches.
    (117, 146),
    (120, 96),
    (128, 80),
    (128, 100),
    (150, 68),
    (160, 120),
    (192, 158),
    (200, 116),
    (231, 130),
    (240, 160),
    (320, 180),
];

/// Seeds swept per size. 0..12 reaches all five `FloorVariant`s through the
/// Fibonacci hash, pinned by `the_sweep_reaches_every_floor_variant`.
const SWEEP_SEEDS: std::ops::Range<u64> = 0..12;

/// Run `f` over `SWEEP_SIZES` × `seeds` at production fill (`max_desks: None`,
/// the densest desk grid — the strictest placement case). A `None` layout is
/// asserted to be a legitimate refusal, never silently skipped.
fn sweep_over(
    seeds: impl Iterator<Item = u64> + Clone,
    mut f: impl FnMut(u16, u16, u64, &SceneLayout),
) {
    for &(w, h) in SWEEP_SIZES {
        for seed in seeds.clone() {
            match SceneLayout::compute_with_seed(w, h, None, seed) {
                Some(l) => {
                    // The dual of the None arm below: that one prices a refusal, this
                    // one prices an acceptance.
                    assert!(
                        !l.home_desks.is_empty(),
                        "{w}x{h} seed {seed}: a layout above the advertised minimum with \
                         no desk to seat anyone"
                    );
                    f(w, h, seed, &l);
                }
                None => assert!(
                    w < super::compute::MIN_LAYOUT_W || h < super::compute::MIN_LAYOUT_H,
                    "{w}x{h} seed {seed}: compute returned None at a size above the \
                     documented minimum ({}x{})",
                    super::compute::MIN_LAYOUT_W,
                    super::compute::MIN_LAYOUT_H
                ),
            }
        }
    }
}

fn sweep(f: impl FnMut(u16, u16, u64, &SceneLayout)) {
    sweep_over(SWEEP_SEEDS, f);
}

/// The PRODUCTION axis: `SWEEP_SIZES` × `floor::floor_seed(0..MAX_FLOORS)`,
/// the seeds a running app actually lays out. Disjoint from [`SWEEP_SEEDS`] but
/// for seed 0, so a guard swept on `SWEEP_SEEDS` alone never visits a floor a
/// user sees.
fn sweep_production_floors(f: impl FnMut(u16, u16, u64, &SceneLayout)) {
    sweep_over(
        (0..crate::floor::MAX_FLOORS).map(crate::floor::floor_seed),
        f,
    );
}

/// Which Bounds a piece's rect must stay inside — one honest container per
/// piece, NOT one rule for all.
#[derive(Clone, Copy, Debug)]
enum Container {
    Band,
    Aisle,
    MeetingRoom(usize),
    Pantry,
    /// The carpet apron at the wall base — the straddling wall decor's ground
    /// strip.
    WallApron,
    WallBand,
}

struct Piece {
    label: String,
    /// `None` = the piece stamps no obstacle of its own (wall-hung decor).
    ground: Option<(Point, Size)>,
    visual: (Point, Size),
    /// For `Anchor::Center` pieces: the unclamped center + visual size, to catch
    /// a west/north spill that `anchored_top_left`'s `saturating_sub` silently
    /// clamps to 0 (a centered piece "fits" iff `pos >= visual/2` per axis).
    center_fit: Option<(Point, Size)>,
    container: Container,
    /// Also require the VISUAL box inside the container: the pod-decor sites
    /// SKIP a slot whose whole sprite wouldn't fit the band, and that skip is
    /// part of the contract, not just the ground.
    visual_in_container: bool,
    /// Pieces sharing a group id are ONE physical cluster, exempt from the
    /// pairwise-overlap invariant WITHIN the group. The harness's one exemption
    /// with no compile tooth: a NEW id needs a WHY at the declaration naming the
    /// authored composition AND a golden pinning the cluster's internal
    /// geometry — never a way to silence a real overlap finding.
    overlap_group: Option<u8>,
}

impl Piece {
    fn table(
        label: String,
        anchor: Anchor,
        pos: Point,
        kind: Furniture,
        container: Container,
        overlap_group: Option<u8>,
    ) -> Piece {
        let def = furniture_def(kind);
        let vis_tl = anchored_top_left(anchor, pos, def.visual.w, def.visual.h);
        Piece {
            label,
            ground: def.ground_rect(anchor, pos),
            visual: (vis_tl, def.visual),
            center_fit: matches!(anchor, Anchor::Center).then_some((pos, def.visual)),
            container,
            visual_in_container: false,
            overlap_group,
        }
    }
}

/// Enumerate EVERY placed piece of a layout. The destructure below has no `..`
/// on purpose: a field that contributes no piece is bound and discarded with
/// the WHY on the same line.
fn pieces(l: &SceneLayout) -> Vec<Piece> {
    let SceneLayout {
        buf_w: _, // the Buffer container, read by the invariants directly
        buf_h: _,
        cubicle_band: _,  // container, not a piece
        cubicle_aisle: _, // container, not a piece
        home_desks,
        desk_facings: _, // a desk ATTRIBUTE, not a piece; length pinned by `every_desk_has_a_facing`
        waypoints,
        plants,
        wall_decor,
        pod_decor,
        lounge,
        door: _, // architecture, not furniture: it PUNCHES walkability through the band
        door_threshold: _, // a walkable POINT, asserted by the connectivity tests
        meeting_rooms,
        pantry,
        room_walls: _, // the containers' edges; overlap-vs-walls is its own invariant
        doorways: _,   // architectural openings, not furniture
        top_margin: _, // wall-band geometry, read via wall_band_h() in invariants
        corridor: _,   // router/pet zone, spans the full width by design
        walkable: _,   // probed directly by the connectivity invariant
        reachable: _,  // conservative routing truth, exercised by pathfind tests
    } = l;

    let mut out = Vec::new();

    for (i, &d) in home_desks.iter().enumerate() {
        out.push(Piece::table(
            format!("desk[{i}]"),
            Anchor::TopLeft,
            d,
            Furniture::Desk,
            Container::Band,
            None,
        ));
    }

    for (i, pd) in pod_decor.iter().enumerate() {
        let mut piece = Piece::table(
            format!("pod_decor[{i}] {:?}", pd.kind),
            Anchor::Center,
            pd.pos,
            pd.kind.furniture(),
            Container::Band,
            None,
        );
        piece.visual_in_container = true;
        out.push(piece);
    }

    for (i, p) in plants.iter().enumerate() {
        // Per-ITEM container, picked by POSITION: a plant that settle_plant
        // moved beside a corner appliance adopts the blocker's AISLE row.
        let in_meeting = l
            .meeting_room_bounds(0)
            .map(|mr| contains_point(mr, p.pos))
            .unwrap_or(false);
        let in_aisle = contains_point(l.cubicle_aisle, p.pos);
        out.push(Piece::table(
            format!("plant[{i}] {:?}", p.kind),
            Anchor::Center,
            p.pos,
            p.kind.furniture(),
            if in_meeting {
                Container::MeetingRoom(0)
            } else if in_aisle {
                Container::Aisle
            } else {
                Container::Band
            },
            None,
        ));
    }

    for (i, wd) in wall_decor.iter().enumerate() {
        let container = match wd.kind {
            // Free-standing floor furniture despite living in the wall_decor
            // vec: the container is keyed on the KIND, not on the Vec.
            WallDecor::Whiteboard => Container::Band,
            // Straddlers: tall sprite on the wall, shallow ground strip on the
            // carpet apron at the wall base.
            WallDecor::Bookshelf | WallDecor::MeetingScreen => Container::WallApron,
            WallDecor::ExitSign | WallDecor::BulletinBoard => Container::WallBand,
        };
        out.push(Piece::table(
            format!("wall_decor[{i}] {:?}", wd.kind),
            Anchor::TopLeft,
            wd.pos,
            wd.kind.furniture(),
            container,
            None,
        ));
    }

    for (i, wp) in waypoints.iter().enumerate() {
        match wp.kind {
            // Each seat stamps its own body into the mask and their union IS
            // the couch's blocked ground, so model the seats and group them.
            // `couch_sprite_center` is NOT used here — it under-models the union.
            WaypointKind::Couch => {
                out.push(Piece::table(
                    format!("waypoint[{i}] Couch seat"),
                    Anchor::Center,
                    wp.pos,
                    Furniture::Couch,
                    Container::Band,
                    Some(2),
                ));
            }
            // Promoted pod_decor slots at the same pos — the pod_decor entry
            // above carries their geometry.
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            // No obstacle of their own; containment is the pos-in-room check in
            // `every_meeting_slot_sits_in_its_room`.
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair => {}
            WaypointKind::Pantry => {
                // Runtime-sized: geometry comes from the shared
                // pantry_ground_rect, not the (deliberately empty) table row.
                let counter = l.pantry_counter_size();
                out.push(Piece {
                    label: format!("waypoint[{i}] Pantry counter"),
                    ground: Some(pantry_ground_rect(wp.pos, counter)),
                    visual: (
                        anchored_top_left(Anchor::Center, wp.pos, counter.w, counter.h),
                        counter,
                    ),
                    center_fit: Some((wp.pos, counter)),
                    container: Container::Pantry,
                    visual_in_container: false,
                    overlap_group: None,
                });
            }
            WaypointKind::VendingMachine | WaypointKind::Printer => {
                out.push(Piece::table(
                    format!("waypoint[{i}] {:?}", wp.kind),
                    Anchor::Center,
                    wp.pos,
                    wp.kind.furniture(),
                    Container::Aisle,
                    None,
                ));
            }
            WaypointKind::SnackShelf => {
                out.push(Piece::table(
                    format!("waypoint[{i}] SnackShelf"),
                    Anchor::Center,
                    wp.pos,
                    wp.kind.furniture(),
                    Container::Pantry,
                    None,
                ));
            }
            // Stands carry no ground: the island BODY registers via
            // `kitchen_island` below.
            WaypointKind::Island => {}
        }
    }

    for (room, r) in meeting_rooms.iter().enumerate() {
        // No `..` here either, so a NEW field on either struct is a compile
        // error until its pieces are registered.
        let MeetingRoom { bounds: _, trio } = r;
        let Some(MeetingTrio { sofas, table }) = trio else {
            continue;
        };
        let mf = MeetingTrio {
            sofas: *sofas,
            table: *table,
        };
        for (s, &sofa) in mf.sofas.iter().enumerate() {
            out.push(Piece::table(
                format!("meeting[{room}].sofa[{s}]"),
                Anchor::Center,
                sofa,
                Furniture::MeetingSofaBody,
                Container::MeetingRoom(room),
                None,
            ));
        }
        out.push(Piece::table(
            format!("meeting[{room}].table"),
            Anchor::Center,
            mf.table,
            Furniture::MeetingTable,
            Container::MeetingRoom(room),
            None,
        ));
    }

    // ONE authored cluster: the table tucks against the couch's west armrest
    // and the lamp hugs its east side BY DESIGN, so they share overlap group 2
    // and the goldens (not the overlap invariant) pin their internal geometry.
    if let Some(lounge) = lounge {
        out.push(Piece::table(
            "floor_lamp".into(),
            Anchor::Center,
            lounge.floor_lamp,
            Furniture::FloorLamp,
            Container::Band,
            Some(2),
        ));
        out.push(Piece::table(
            "lounge_side_table".into(),
            Anchor::Center,
            lounge.side_table,
            Furniture::LoungeSideTable,
            Container::Band,
            Some(2),
        ));
        if let Some(tank) = lounge.fish_tank {
            out.push(Piece::table(
                "fish_tank".into(),
                Anchor::Center,
                tank,
                Furniture::FishTank,
                Container::Band,
                Some(2),
            ));
        }
        // lounge.couch_center contributes no Piece: its geometry comes from the
        // seat waypoints above, the mask's truth.
    }

    let island = pantry.as_ref().and_then(|p| {
        let PantryRoom {
            bounds: _,       // container, asserted by Container::Pantry below
            counter_size: _, // its counter piece registers via the Pantry arm above
            kitchen_island,
        } = p;
        kitchen_island.as_ref()
    });
    if let Some(p) = island {
        out.push(Piece::table(
            "kitchen_island".into(),
            Anchor::Center,
            *p,
            Furniture::KitchenIsland,
            Container::Pantry,
            None,
        ));
    }

    out
}

fn contains_point(b: Bounds, p: Point) -> bool {
    p.x >= b.x && p.x < b.x + b.width && p.y >= b.y && p.y < b.y + b.height
}

fn rect_in_bounds(tl: Point, sz: Size, b: Bounds) -> bool {
    tl.x >= b.x && tl.y >= b.y && tl.x + sz.w <= b.x + b.width && tl.y + sz.h <= b.y + b.height
}

/// Resolve a piece's container to concrete Bounds. `None` = the container
/// doesn't exist for this layout, itself a failure: a piece can't be placed in
/// a room the floor doesn't have.
fn container_bounds(l: &SceneLayout, c: Container) -> Option<Bounds> {
    match c {
        Container::Band => Some(l.cubicle_band),
        Container::Aisle => Some(l.cubicle_aisle),
        Container::MeetingRoom(i) => l.meeting_room_bounds(i),
        Container::Pantry => l.pantry.map(|p| p.bounds),
        Container::WallApron => Some(Bounds {
            x: 0,
            y: l.wall_band_h(),
            width: l.buf_w,
            height: l.top_margin - l.wall_band_h(),
        }),
        Container::WallBand => Some(Bounds {
            x: 0,
            y: 0,
            width: l.buf_w,
            height: l.top_margin,
        }),
    }
}

/// Cap on the reported violations: the invariants collect across the WHOLE
/// sweep and fail once, because a fail-fast assert reports only the first cell
/// and hides the pattern (one bug and a systemic clamp miss look identical).
const MAX_REPORTED: usize = 25;

fn assert_no_violations(what: &str, violations: Vec<String>) {
    assert!(
        violations.is_empty(),
        "{} {what} violations across the sweep (first {}):\n{}",
        violations.len(),
        violations.len().min(MAX_REPORTED),
        violations
            .iter()
            .take(MAX_REPORTED)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_piece_stays_inside_the_buffer() {
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        let buffer = Bounds {
            x: 0,
            y: 0,
            width: l.buf_w,
            height: l.buf_h,
        };
        for p in pieces(l) {
            for (what, rect) in [("ground", p.ground), ("visual", Some(p.visual))] {
                if let Some((tl, sz)) = rect {
                    if !rect_in_bounds(tl, sz, buffer) {
                        v.push(format!(
                            "{w}x{h} seed {seed}: {} {what} {tl:?}+{sz:?} leaves the buffer",
                            p.label
                        ));
                    }
                }
            }
            if let Some((pos, vis)) = p.center_fit {
                if pos.x < vis.w / 2 || pos.y < vis.h / 2 {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} centered at {pos:?} spills its {vis:?} \
                         visual west/north (silently clamped by saturating_sub)",
                        p.label
                    ));
                }
            }
        }
    });
    assert_no_violations("buffer-containment", v);
}

#[test]
fn every_piece_ground_stays_in_its_container() {
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        for p in pieces(l) {
            let Some(b) = container_bounds(l, p.container) else {
                if p.ground.is_some() {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} placed but its container {:?} doesn't exist",
                        p.label, p.container
                    ));
                }
                continue;
            };
            if let Some((tl, sz)) = p.ground {
                if !rect_in_bounds(tl, sz, b) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} ground {tl:?}+{sz:?} leaves its {:?} {b:?}",
                        p.label, p.container
                    ));
                }
            }
            if p.visual_in_container {
                let (tl, sz) = p.visual;
                if !rect_in_bounds(tl, sz, b) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} visual {tl:?}+{sz:?} leaves its {:?} {b:?}",
                        p.label, p.container
                    ));
                }
            }
        }
    });
    assert_no_violations("container", v);
}

#[test]
fn every_piece_ground_is_blocked_in_the_mask() {
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        for p in pieces(l) {
            let Some((tl, sz)) = p.ground else { continue };
            let (cx, cy) = (tl.x + sz.w / 2, tl.y + sz.h / 2);
            if l.walkable.is_walkable(cx, cy) {
                v.push(format!(
                    "{w}x{h} seed {seed}: {} ground centre ({cx},{cy}) is WALKABLE —                      missing mask stamp",
                    p.label
                ));
            }
        }
    });
    assert_no_violations("mask-parity", v);
}

#[test]
fn no_two_furniture_grounds_overlap() {
    // Only the BLOCKED grounds: sprite overhangs may overlap freely — that is
    // occlusion, not placement.
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        let ps: Vec<Piece> = pieces(l)
            .into_iter()
            .filter(|p| p.ground.is_some())
            .collect();
        for i in 0..ps.len() {
            for j in i + 1..ps.len() {
                let (a, b) = (&ps[i], &ps[j]);
                if a.overlap_group.is_some() && a.overlap_group == b.overlap_group {
                    continue;
                }
                if rects_overlap(a.ground.unwrap(), b.ground.unwrap()) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} {:?} overlaps {} {:?}",
                        a.label,
                        a.ground.unwrap(),
                        b.label,
                        b.ground.unwrap()
                    ));
                }
            }
        }
    });
    assert_no_violations("furniture-overlap", v);
}

#[test]
fn no_furniture_ground_overlaps_a_wall() {
    // UNPADDED grounds only — a padded rect legitimately touches a wall,
    // because the pad is routing slack.
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        let walls: Vec<(Point, Size)> = l
            .room_walls
            .iter()
            .map(|seg| super::rooms::walls::wall_segment_rect(seg, l.top_margin, &l.room_walls))
            .collect();
        for p in pieces(l) {
            let Some(g) = p.ground else { continue };
            for &wrect in &walls {
                if rects_overlap(g, wrect) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: {} ground {g:?} overlaps wall {wrect:?}",
                        p.label
                    ));
                }
            }
        }
    });
    assert_no_violations("wall-overlap", v);
}

/// The door threshold is walkable AND every walkable pixel is reachable from it
/// (4-connected), through the PRODUCTION `unreachable_walkable_cells` so the
/// guard and its test can't drift. The threshold-walkable assert is SEPARATE and
/// FIRST: `unreachable_walkable_cells` returns empty on a BLOCKED seed, so
/// without it a sealed threshold passes vacuously.
fn assert_walkable_connected(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    let Some(start) = l.door_threshold else {
        panic!("{w}x{h} seed {seed}: layout has no door threshold");
    };
    assert!(
        l.walkable.is_walkable(start.x, start.y),
        "{w}x{h} seed {seed}: door threshold {start:?} is not walkable"
    );
    let pocket = super::compute::unreachable_walkable_cells(&l.walkable, start);
    assert!(
        pocket.is_empty(),
        "{w}x{h} seed {seed}: {} walkable px unreachable from the door (a sealed \
         pocket), e.g. {:?}",
        pocket.len(),
        pocket.first()
    );
}

#[test]
fn walkable_is_one_connected_region() {
    sweep(assert_walkable_connected);
}

/// The elevator spawn must land on OPEN FLOOR — at or south of `top_margin` —
/// never on the carpet apron where the straddling wall decor stamps its ground.
///
/// Deliberately a one-sided bound, not `assert_eq!` against the spawn formula,
/// which would pin nothing. Walkability is NOT the discriminator and a
/// routability sweep is blind to this: with the spawn moved north of the floor
/// line `is_walkable(dt)` stays true and every routing assertion still passes
/// (both `find_path` and `ReachSet::from_mask` SNAP a displaced seed back into
/// the component) — only this floor-line bound fails.
fn assert_spawn_stands_on_open_floor(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    let Some(dt) = l.door_threshold else {
        panic!("{w}x{h} seed {seed}: layout has no door threshold");
    };
    assert!(
        dt.y >= l.top_margin,
        "{w}x{h} seed {seed}: spawn {dt:?} sits on the wall apron (rows {}..{}) \
         instead of open floor — an entering character starts inside the strip \
         the straddling wall decor stamps its ground into",
        l.wall_band_h(),
        l.top_margin
    );
}

#[test]
fn the_spawn_threshold_stands_on_the_floor_not_the_wall_apron() {
    sweep(assert_spawn_stands_on_open_floor);
    sweep_production_floors(assert_spawn_stands_on_open_floor);
}

/// Every destination the wander can hand out must be `find_path`-routable from
/// the leg's real ORIGIN, or the leg degrades to a straight line through
/// furniture.
///
/// Routes from `pose::desk_leg_endpoint(desk, l).0`, the endpoint the production
/// wander-out leg starts at — NOT from the door. Routing from the door made a
/// desk an origin-free bystander, so a desk stranded on the severed side of a
/// coarse cut never appeared as a route START and the whole class passed
/// through. The `(origin, approach)` pair is deduped because the desk loop
/// otherwise re-routes it once per desk sharing an approach cell.
#[test]
fn every_wander_destination_is_routable_from_its_desk() {
    use crate::pathfind::find_path;
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    sweep(|w, h, seed, l| {
        let mut seen: std::collections::HashSet<(Point, Point)> = std::collections::HashSet::new();
        for &desk in &l.home_desks {
            let (origin, _) = crate::pose::desk_leg_endpoint(desk, l);
            for wp in &l.waypoints {
                let a = super::approach_point(
                    wp.kind.furniture(),
                    wp.pos,
                    wp.facing,
                    l.pantry_counter_size(),
                    &l.walkable,
                    desk,
                    &l.reachable,
                );
                // A `wp.pos` sentinel is the documented "no valid approach"
                // answer — `resolve_wander_target` ambles that cycle instead.
                if a == wp.pos || !seen.insert((origin, a)) {
                    continue;
                }
                assert!(
                    find_path(&l.walkable, &overlay, None, origin, a).is_some(),
                    "{w}x{h} seed {seed}: {:?} approach {a:?} unroutable from desk \
                     {desk:?}'s leg origin {origin:?}",
                    wp.kind
                );
            }
        }
    });
}

/// Nothing in the type system pins `desk_facings` parallel to `home_desks`; a short one
/// makes the tail desks silently fall back to `South`, passing every geometry invariant.
fn assert_every_desk_has_a_facing(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    assert_eq!(
        l.desk_facings.len(),
        l.home_desks.len(),
        "{w}x{h} seed {seed}: {} desks but {} facings — the tail would silently \
         fall back to South",
        l.home_desks.len(),
        l.desk_facings.len()
    );
}

/// The COARSE twin of [`assert_walkable_connected`]. That one is a 4-connected
/// PIXEL flood, but the router runs on the 4×4 grid, so a ≤3px channel is
/// pixel-connected and coarse-IMPASSABLE — the office reads as ONE region at
/// pixel granularity and TWO at router granularity. When that happens,
/// `desk_approach_cell` returns its no-valid-approach sentinel and every leg for
/// the severed desks degrades to a straight `door→chair` line through furniture.
///
/// A free fn, not a closure, because THREE sweeps need it: the placement seed
/// axis, the production floor seeds, and the step-1 `NARROW_BAND` width scan.
fn assert_home_desk_approaches_are_routable(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    use crate::pathfind::find_path;
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let Some(door) = l.door_threshold else {
        panic!("{w}x{h} seed {seed}: layout has no door threshold");
    };
    for (i, &desk) in l.home_desks.iter().enumerate() {
        let approach = crate::pose::desk_approach_cell(desk, l).unwrap_or_else(|| {
            panic!(
                "{w}x{h} seed {seed}: home desk {i} at {desk:?} has NO reachable approach \
                 side — every leg to it falls back to a straight line through the desk"
            )
        });
        assert!(
            find_path(&l.walkable, &overlay, None, door, approach).is_some(),
            "{w}x{h} seed {seed}: home desk {i} at {desk:?} has approach {approach:?} \
             unroutable from the door {door:?} — the coarse grid is severed"
        );
    }
}

#[test]
fn every_home_desk_approach_is_routable_from_the_door() {
    sweep(assert_home_desk_approaches_are_routable);
    sweep_production_floors(assert_home_desk_approaches_are_routable);
}

#[test]
fn every_desk_has_a_facing() {
    sweep(assert_every_desk_has_a_facing);
    sweep_production_floors(assert_every_desk_has_a_facing);
}

/// A back-turned desk exists only to face a viewer-facing one across the pod's inner
/// gap. Stated as the PAIRING, not as `pod_row_facing`'s formula a test could only copy.
fn assert_back_turned_desks_face_a_partner(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    let south: std::collections::HashSet<Point> = l
        .home_desks
        .iter()
        .zip(&l.desk_facings)
        .filter(|(_, &f)| f == Facing::South)
        .map(|(&d, _)| d)
        .collect();
    for (&d, &f) in l.home_desks.iter().zip(&l.desk_facings) {
        if f != Facing::North {
            continue;
        }
        let partner = Point {
            x: d.x,
            y: d.y.saturating_sub(DESK_H + INTRA_POD_GAP_Y),
        };
        assert!(
            south.contains(&partner),
            "{w}x{h} seed {seed}: back-turned desk {d:?} has no viewer-facing desk at \
             {partner:?} — it turns its back on empty floor instead of on a pod partner"
        );
    }
}

#[test]
fn back_turned_desks_face_a_partner_across_the_pod_gap() {
    sweep(assert_back_turned_desks_face_a_partner);
    sweep_production_floors(assert_back_turned_desks_face_a_partner);
}

#[test]
fn no_walkable_hole_where_a_vertical_wall_meets_a_horizontal_one() {
    // A divider crossed by an E-W wall is TWO vertical segments meeting at the
    // cross wall; every corner row must be solid across the wall's whole width,
    // or a walker can stand IN the divider.
    sweep(|w, h, seed, l| {
        let h_walls: Vec<_> = l
            .room_walls
            .iter()
            .filter(|s| s.start.y == s.end.y)
            .collect();
        for v in l.room_walls.iter().filter(|s| s.start.x == s.end.x) {
            let vtop = v.start.y.min(v.end.y);
            for hw in &h_walls {
                let (hx0, hx1) = (hw.start.x.min(hw.end.x), hw.start.x.max(hw.end.x));
                let hr = hw.start.y;
                // Only the crossing that actually trims this segment's north end.
                if hr < vtop
                    && vtop - hr <= super::WALL_THICK_H + super::rooms::walls::WALL_BRIDGE_SLACK_PX
                    && (hx0..=hx1).contains(&v.start.x)
                {
                    for y in hr..vtop {
                        for dx in 0..super::WALL_THICK_V {
                            assert!(
                                !l.is_walkable(v.start.x + dx, y),
                                "{w}x{h} seed {seed}: walkable HOLE at ({},{y}) in the \
                                 divider corner between H wall @{hr} and V wall @{vtop}",
                                v.start.x + dx,
                            );
                        }
                    }
                }
            }
        }
    });
}

/// Widths inside the narrow-band DEGRADATION zone the discrete `SWEEP_SIZES`
/// grid structurally skips. Floored by `MIN_LAYOUT_W`; the upper bound is 76,
/// not 64, to cover the FULL single-pod-column window: the desk grid stays one
/// column through buf_w≈70 and only splits to two (two drains, robust) at ≈71,
/// and the discrete grid's nearest points either side are 64 and 80.
const NARROW_BAND: std::ops::RangeInclusive<u16> = super::compute::MIN_LAYOUT_W..=76;

/// Step-1 width sweep across the degradation band, running BOTH connectivity
/// predicates at that resolution — the discrete `SWEEP_SIZES` grid can't cover
/// every width, and a sealed pocket at a width it skips ships silently. Heights
/// span the tall floors where the aisle-seal manifests; only 148 reaches the #566 guard.
/// The Y twin of the width scan below: the derived height floor opened 45..59, where
/// `pod_rows` collapses to 1 and the appliance aisle sits at its 8px minimum. The
/// discrete grid gives that band 6 points; this walks it at step 1.
#[test]
fn short_band_connectivity_boundary_scan() {
    for h in super::compute::MIN_LAYOUT_H..60 {
        for &w in &[super::compute::MIN_LAYOUT_W, 48u16, 80, 120, 200] {
            for seed in SWEEP_SEEDS {
                let Some(l) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                    panic!("{w}x{h} seed {seed}: refused above the floor");
                };
                assert!(
                    !l.home_desks.is_empty(),
                    "{w}x{h} seed {seed}: lays out with no desk to seat anyone"
                );
                assert_walkable_connected(w, h, seed, &l);
                assert_home_desk_approaches_are_routable(w, h, seed, &l);
            }
        }
    }
}

#[test]
fn narrow_band_connectivity_boundary_scan() {
    for w in NARROW_BAND {
        // 46 reaches the SHORT band the derived floor opened, where `pod_rows`
        // collapses to 1 and the appliance aisle sits at its 8px minimum.
        for &h in &[46u16, 80, 100, 120, 148, 160] {
            for seed in SWEEP_SEEDS {
                let Some(l) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                    // Every size here is above BOTH floors, so a refusal is a bug —
                    // and `sweep_over`'s None arm covers only the discrete grid.
                    panic!("{w}x{h} seed {seed}: refused above the floor");
                };
                {
                    // `assert_home_desk_approaches_are_routable` passes VACUOUSLY on an
                    // empty one, and this scan visits the widths between the grid points.
                    assert!(
                        !l.home_desks.is_empty(),
                        "{w}x{h} seed {seed}: lays out with no desk to seat anyone"
                    );
                    assert_walkable_connected(w, h, seed, &l);
                    assert_home_desk_approaches_are_routable(w, h, seed, &l);
                }
            }
        }
    }
}

#[test]
fn door_threshold_walkable_at_a_band_split_to_thirty() {
    // At a cubicle band exactly 30 px wide the lounge couch's east seat sealed
    // the spawn threshold's own column; 39x160 seed 1 is one such split.
    let l = SceneLayout::compute_with_seed(39, 160, None, 1).expect("39x160 lays out");
    let dt = l.door_threshold.expect("has a door threshold");
    assert!(
        l.walkable.is_walkable(dt.x, dt.y),
        "door threshold {dt:?} must be walkable — the couch may not seal the spawn column"
    );
}

#[test]
fn appliance_strip_not_sealed_at_a_single_pod_band() {
    // At 59x160 seed 3 the band fits ONE pod column, so the only aisle drain is
    // the intra-pod gap — a scatter plant settling onto the printer's row plugs
    // it and seals the whole appliance strip.
    let l = SceneLayout::compute_with_seed(59, 160, None, 3).expect("59x160 lays out");
    assert_walkable_connected(59, 160, 3, &l);
}

/// Each pod's vertical extent from the desks ALONE, so the guards below test the
/// layout's output rather than the placement helper's own idea of where the pods are.
fn pod_y_extents(l: &SceneLayout) -> Vec<(u16, u16)> {
    let mut ys: Vec<u16> = l.home_desks.iter().map(|d| d.y).collect();
    ys.sort_unstable();
    ys.dedup();
    let mut pods = Vec::new();
    let mut i = 0;
    while i < ys.len() {
        let top = ys[i];
        if ys.get(i + 1) == Some(&(top + DESK_H + INTRA_POD_GAP_Y)) {
            pods.push((top, ys[i + 1] + DESK_GROUND_H));
            i += 2;
        } else {
            pods.push((top, top + DESK_GROUND_H));
            i += 1;
        }
    }
    pods
}

fn assert_no_free_standing_piece_inside_a_pod(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    let pods = pod_y_extents(l);
    for d in &l.wall_decor {
        let Some((g, gs)) = furniture_def(d.kind.furniture()).ground_rect(Anchor::TopLeft, d.pos)
        else {
            continue; // wall-hung: no ground contact, nothing to wedge into a pod
        };
        for &(top, bottom) in &pods {
            assert!(
                g.y >= bottom || g.y + gs.h <= top,
                "{w}x{h} seed {seed}: {:?}'s ground rows {}..{} fall inside the pod \
                 spanning {top}..{bottom} — free-standing furniture belongs in the \
                 aisles BETWEEN pods, never in a pod's own desk rows or intra-pod gap",
                d.kind,
                g.y,
                g.y + gs.h
            );
        }
    }
}

#[test]
fn free_standing_furniture_never_stands_inside_a_pod() {
    sweep(assert_no_free_standing_piece_inside_a_pod);
    sweep_production_floors(assert_no_free_standing_piece_inside_a_pod);
}

/// `snap_inter_pod_ground_y` answers `None` by design and the caller drops the board
/// with no trace, so a broken snap surfaces only as one fewer whiteboard.
fn assert_the_whiteboard_lands_when_an_aisle_exists(w: u16, h: u16, seed: u64, l: &SceneLayout) {
    let has_side_rooms = !l.meeting_rooms.is_empty() || l.pantry.is_some();
    if !has_side_rooms || pod_y_extents(l).len() < 2 {
        return;
    }
    assert!(
        l.wall_decor
            .iter()
            .any(|d| matches!(d.kind, super::WallDecor::Whiteboard)),
        "{w}x{h} seed {seed}: {} pod rows leave an inter-pod aisle, but no whiteboard \
         was placed — the snap dropped it silently",
        pod_y_extents(l).len()
    );
}

#[test]
fn the_whiteboard_lands_whenever_an_inter_pod_aisle_exists() {
    sweep(assert_the_whiteboard_lands_when_an_aisle_exists);
    sweep_production_floors(assert_the_whiteboard_lands_when_an_aisle_exists);
}

/// The 32x120 seed-3 repro that forced the aisle rule — board +3px east of the divider,
/// wall flush west, desk column east, sealing the south — is under `MIN_LAYOUT_W` now, so
/// this re-pins at the width floor. Still the snap's own guard: unsnapped, the board makes
/// two desks' south approach unroutable, `severed` fires on that arm instead of a pocket,
/// and the connectivity guard spends the board — reverting `snap_inter_pod_ground_y` reds
/// the `wall_decor` assert below.
#[test]
fn free_standing_whiteboard_survives_the_west_aisle_it_used_to_seal() {
    let (w, h, seed) = (super::compute::MIN_LAYOUT_W, 120, 3);
    let l = SceneLayout::compute_with_seed(w, h, None, seed).expect("the width floor lays out");
    assert_walkable_connected(w, h, seed, &l);
    assert!(
        l.wall_decor
            .iter()
            .any(|d| matches!(d.kind, super::WallDecor::Whiteboard)),
        "the whiteboard must survive — an aisle-seated board severs nothing, so the \
         connectivity guard has no cause to spend it"
    );
    assert_eq!(
        l.plants.len(),
        2,
        "the two far-south plants survive alongside it — the guard spends nothing here"
    );
}

#[test]
fn couch_survives_a_narrow_band_that_clears_the_door() {
    // The boundary scan can't catch an over-drop: dropping the couch only
    // IMPROVES connectivity. 40x160 seed 1 is the KNIFE-EDGE — the couch clears
    // the door by exactly 1 px, so it pins couch_east_ground on the seat pad
    // (`WAYPOINT_STAMP_PAD_PX`), not `OBSTACLE_PAD_PX`. 48x160 seed 0 clears
    // comfortably.
    for &(w, h, seed) in &[(40u16, 160u16, 1u64), (48, 160, 0)] {
        let l = SceneLayout::compute_with_seed(w, h, None, seed).expect("lays out");
        assert!(
            l.couch_sprite_center().is_some(),
            "{w}x{h} seed {seed}: the lounge couch must survive a band that clears the door"
        );
    }
}

#[test]
fn desk_capacity_obeys_the_request_law() {
    // A sub-grid, because capacity re-computes the whole layout per n.
    for &(w, h) in &[(50u16, 80u16), (96, 100), (120, 96), (192, 158), (320, 180)] {
        for seed in 0..4u64 {
            let Some(full) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                continue;
            };
            let cap = full.home_desks.len();
            for n in [1usize, cap.saturating_sub(1).max(1), cap, cap + 5] {
                let l = SceneLayout::compute_with_seed(w, h, Some(n), seed).expect("fits");
                assert_eq!(
                    l.home_desks.len(),
                    n.min(cap),
                    "{w}x{h} seed {seed}: Some({n}) must yield min({n}, cap={cap}) desks"
                );
            }
        }
    }
}

#[test]
fn every_kind_is_placed_somewhere_in_the_sweep() {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    sweep(|_, _, _, l| {
        for wp in &l.waypoints {
            seen.insert(format!("wp:{:?}", wp.kind));
        }
        for pd in &l.pod_decor {
            seen.insert(format!("pod:{:?}", pd.kind));
        }
        for p in &l.plants {
            seen.insert(format!("plant:{:?}", p.kind));
        }
        for wd in &l.wall_decor {
            seen.insert(format!("wall:{:?}", wd.kind));
        }
        if l.floor_lamp().is_some() {
            seen.insert("floor_lamp".into());
        }
        if l.lounge_side_table().is_some() {
            seen.insert("lounge_side_table".into());
        }
        if l.fish_tank().is_some() {
            seen.insert("fish_tank".into());
        }
        if l.pantry.is_some_and(|p| p.kitchen_island.is_some()) {
            seen.insert("kitchen_island".into());
        }
        if l.couch_sprite_center().is_some() {
            seen.insert("couch".into());
        }
    });
    let mut missing: Vec<String> = Vec::new();
    for kind in WaypointKind::ALL {
        let k = format!("wp:{kind:?}");
        if !seen.contains(&k) {
            missing.push(k);
        }
    }
    for kind in PodDecor::ALL {
        let k = format!("pod:{kind:?}");
        if !seen.contains(&k) {
            missing.push(k);
        }
    }
    for kind in [
        PlantKind::Tall,
        PlantKind::Flower,
        PlantKind::Succulent,
        PlantKind::Ficus,
    ] {
        let k = format!("plant:{kind:?}");
        if !seen.contains(&k) {
            missing.push(k);
        }
    }
    for kind in [
        WallDecor::Bookshelf,
        WallDecor::Whiteboard,
        WallDecor::ExitSign,
        WallDecor::MeetingScreen,
        // BulletinBoard: allowlisted — unplaced by design, registered for pack
        // authors, so it has no push site in compute.
    ] {
        let k = format!("wall:{kind:?}");
        if !seen.contains(&k) {
            missing.push(k);
        }
    }
    for fixed in [
        "floor_lamp",
        "lounge_side_table",
        "fish_tank",
        "kitchen_island",
        "couch",
    ] {
        if !seen.contains(fixed) {
            missing.push(fixed.into());
        }
    }
    assert!(
        missing.is_empty(),
        "kinds never placed across the whole sweep: {missing:?}"
    );
}

#[test]
fn every_meeting_slot_sits_in_its_room() {
    // Seat waypoints carry no ground (the sofa body does), so their honest
    // containment is pos-in-room.
    sweep(|w, h, seed, l| {
        for wp in &l.waypoints {
            let Some(room_id) = wp.room_id else { continue };
            let Some(b) = l.meeting_room_bounds(room_id) else {
                panic!(
                    "{w}x{h} seed {seed}: waypoint {:?} claims room {room_id} \
                     but that room has no bounds",
                    wp.kind
                );
            };
            assert!(
                contains_point(b, wp.pos),
                "{w}x{h} seed {seed}: {:?} slot {:?} sits outside its room {room_id} {b:?}",
                wp.kind,
                wp.pos
            );
        }
    });
}

#[test]
fn the_sweep_reaches_every_floor_variant() {
    // Variant internals are private, so the shapes are compared observationally.
    // The signature needs cubicle_band.x: Senior differs from Standard, and
    // Lounge from OpenPlan, ONLY by the left-column percent — room presence
    // alone collapses the 5 variants to 3 shapes.
    use std::collections::BTreeSet;
    let mut shapes: BTreeSet<(bool, bool, bool, u16)> = BTreeSet::new();
    for seed in SWEEP_SEEDS {
        let l = SceneLayout::compute_with_seed(240, 160, None, seed).expect("fits");
        shapes.insert((
            !l.meeting_rooms.is_empty(),
            l.pantry.is_some(),
            l.meeting_rooms.len() > 1,
            l.cubicle_band.x,
        ));
    }
    assert!(
        shapes.len() >= super::compute::FloorVariant::ALL.len(),
        "sweep seeds reach only {} of {} floor shapes: {shapes:?} — widen SWEEP_SEEDS",
        shapes.len(),
        super::compute::FloorVariant::ALL.len()
    );
}

#[test]
fn plant_obstacle_census_honors_repels_plants() {
    // Positions are arbitrary — only presence/absence in the census matters.
    let p = |x: u16, y: u16| Point { x, y };
    let rects = super::compute::plant_obstacle_rects(
        Some(p(10, 10)), // fish tank      -> repels -> in
        Some(p(50, 50)), // floor lamp     -> NOT    -> out
        Some(p(60, 60)), // side table     -> NOT    -> out
        Some(p(80, 40)), // kitchen island -> repels -> in
        &[],             // no meeting rooms
    );
    assert_eq!(
        rects.len(),
        2,
        "fish tank + island repel; lamp + side table are the declared Ficus-hug exclusions"
    );
    assert!(
        super::compute::plant_obstacle_rects(None, Some(p(1, 1)), Some(p(2, 2)), None, &[])
            .is_empty(),
        "only repels_plants singletons enter the census"
    );
}

#[test]
fn scatter_plants_keep_obstacle_clearance_and_survive_by_sliding() {
    use super::compute::{PLANT_OBSTACLE_CLEARANCE_PX, ROOMY_BAND_MIN_W};
    let visual_tl = |pos: Point, v: Size| {
        (
            Point {
                x: pos.x.saturating_sub(v.w / 2),
                y: pos.y.saturating_sub(v.h / 2),
            },
            v,
        )
    };
    // Inflate the obstacle box by the clearance on every side, then test overlap.
    // The center-anchored predicate the WAYPOINT family needs; the fixed
    // non-waypoint singletons ride `plant_obstacle_rects` instead.
    let within_clearance = |plant_box: (Point, Size), obs_pos: Point, obs_v: Size| {
        let m = PLANT_OBSTACLE_CLEARANCE_PX;
        let (otl, osz) = visual_tl(obs_pos, obs_v);
        let inflated = (
            Point {
                x: otl.x.saturating_sub(m),
                y: otl.y.saturating_sub(m),
            },
            Size {
                w: osz.w + 2 * m,
                h: osz.h + 2 * m,
            },
        );
        super::placement::rects_overlap(plant_box, inflated)
    };
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        for p in &l.plants {
            let pv = furniture_def(p.kind.furniture()).visual;
            let plant_box = visual_tl(p.pos, pv);
            // The SAME `plant_obstacle_rects` census the production settle path
            // uses, never a hand-re-derived second copy: a census miss here IS a
            // plant interpenetration.
            for (otl, osz) in super::compute::plant_obstacle_rects(
                l.fish_tank(),
                l.floor_lamp(),
                l.lounge_side_table(),
                l.pantry.and_then(|pr| pr.kitchen_island),
                &l.meeting_rooms,
            ) {
                if super::placement::overlaps_within_clearance(
                    plant_box,
                    (otl, osz),
                    PLANT_OBSTACLE_CLEARANCE_PX,
                ) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: plant {:?}@{:?} within clearance of a repels-plants singleton @{:?}",
                        p.kind, p.pos, otl
                    ));
                }
            }
            for wp in &l.waypoints {
                let def = furniture_def(wp.kind.furniture());
                if def.footprint.is_none() {
                    continue;
                }
                if within_clearance(plant_box, wp.pos, def.visual) {
                    v.push(format!(
                        "{w}x{h} seed {seed}: plant {:?}@{:?} within clearance of {:?}@{:?}",
                        p.kind, p.pos, wp.kind, wp.pos
                    ));
                }
            }
        }
        // Greenery pin only where room exists: on a narrower band the slide has
        // nowhere to land and tiny-floor degradation is the house norm. The pin
        // guards against an office-WIDE loss at flagship sizes.
        if l.cubicle_band.width >= ROOMY_BAND_MIN_W {
            let has = |k: WaypointKind| l.waypoints.iter().any(|w| w.kind == k);
            let corridor_plant = |kind: PlantKind| {
                l.plants
                    .iter()
                    .any(|p| p.kind == kind && p.pos.y + 6 >= l.cubicle_aisle.y)
            };
            if has(WaypointKind::VendingMachine) && !corridor_plant(PlantKind::Flower) {
                v.push(format!(
                    "{w}x{h} seed {seed}: vending cost the corridor its Flower"
                ));
            }
            if has(WaypointKind::Printer) && !corridor_plant(PlantKind::Succulent) {
                v.push(format!(
                    "{w}x{h} seed {seed}: printer cost the corridor its Succulent"
                ));
            }
        }
    });
    assert!(
        v.is_empty(),
        "{} violations (first 6):\n{}",
        v.len(),
        v[..v.len().min(6)].join("\n")
    );
}

/// `is_visually_clear`'s completeness, checked against the enumeration that is
/// ALREADY compile-forced. The predicate destructures `SceneLayout` with no `..`,
/// so the compiler catches a new COLLECTION; this catches a member of an existing
/// one that the predicate reads with the wrong anchor, kind or size — the class
/// that shipped incomplete twice (lounge + the runtime-sized kinds, then the
/// free-standing whiteboard).
#[test]
fn every_placed_sprite_is_opaque_to_the_visual_clearance_predicate() {
    let mut checked = 0u32;
    sweep(|w, h, seed, l| {
        for p in pieces(l) {
            let (tl, sz) = p.visual;
            if sz.w == 0 || sz.h == 0 {
                continue; // runtime-sized: the table has no rect to centre on
            }
            let mid = Point {
                x: tl.x + sz.w / 2,
                y: tl.y + sz.h / 2,
            };
            assert!(
                !l.is_visually_clear(mid),
                "{w}x{h} seed {seed}: {} paints over {mid:?} but the predicate calls it clear",
                p.label
            );
            checked += 1;
        }
    });
    assert!(checked > 1000, "the sweep must reach pieces, saw {checked}");
}

/// The north wall band is a WALL PLANE, not floor — nothing in it is walkable,
/// the elevator included. Its doorway is a hole in the WALL, not in the ground:
/// `door_threshold` already stands clear to the south, so a cut here only ever
/// produced walkable wall, and `creatures::walkable_target` draws wander
/// destinations straight off this mask (#902 — a cat wandering up the channel).
#[test]
fn no_cell_of_the_north_wall_band_is_walkable() {
    let mut checked = 0usize;
    sweep(|w, h, seed, l| {
        let band = l.wall_band_h();
        assert!(band > 0, "{w}x{h} seed {seed}: no wall band to check");
        for y in 0..band {
            for x in 0..l.walkable.width() {
                assert!(
                    !l.walkable.is_walkable(x, y),
                    "{w}x{h} seed {seed}: ({x},{y}) is walkable inside the wall band \
                     (rows 0..{band}), door at {:?}",
                    l.door
                );
            }
        }
        checked += 1;
    });
    assert!(checked > 0, "the sweep produced no layouts");
}

/// A pot must not stand on the same pixel of every floor. `settle_plant` was
/// already authorised to slide `MAX_PLANT_NUDGE_PX` inward to dodge an
/// obstacle; the seeded first step spends part of that same budget on variety,
/// so this asks only that the budget is actually being spent.
#[test]
fn a_plants_spot_varies_by_floor() {
    use std::collections::HashSet;
    for &(w, h) in &[(96u16, 70u16), (160, 120), (192, 158)] {
        let mut seen: HashSet<Vec<(u16, u16)>> = HashSet::new();
        for f in 0..crate::floor::MAX_FLOORS {
            if let Some(l) = SceneLayout::compute_with_seed(w, h, None, crate::floor::floor_seed(f))
            {
                seen.insert(l.plants.iter().map(|p| (p.pos.x, p.pos.y)).collect());
            }
        }
        assert!(
            seen.len() > crate::floor::MAX_FLOORS / 2,
            "{w}x{h}: only {} distinct plant arrangements over {} floors — the \
             seeded step is not reaching the pots",
            seen.len(),
            crate::floor::MAX_FLOORS
        );
    }
}

/// Each PASS of the decor bag is a permutation: a floor sees every kind before
/// any repeats. That is the property a bare rotation was picked for, and the
/// reason a plain hash was rejected before it — a shuffled bag has to keep it.
#[test]
fn each_pass_of_the_decor_bag_is_a_permutation_of_the_roster() {
    use crate::layout::PodDecor;
    let n = PodDecor::ALL.len();
    let pass_at = |seed: u64, pass: usize| -> Vec<PodDecor> {
        (0..n)
            .map(|i| super::compute::decor_for_slot(seed, pass * n + i))
            .collect()
    };
    let mut saw_two_orders = false;
    for f in 0..crate::floor::MAX_FLOORS {
        let seed = crate::floor::floor_seed(f);
        for pass in 0..4 {
            let got = pass_at(seed, pass);
            for kind in PodDecor::ALL {
                assert_eq!(
                    got.iter().filter(|k| *k == kind).count(),
                    1,
                    "floor {f} pass {pass}: {kind:?} is not exactly once in {got:?}"
                );
            }
            saw_two_orders |= got != pass_at(seed, 0);
        }
    }
    assert!(
        saw_two_orders,
        "every pass has the SAME order — that is a rotation, not a bag, and a \
         floor wide enough for a second pass repeats it verbatim"
    );
}

/// Every pod-decor kind must be able to open a floor — a one-slot floor renders
/// only whatever the bag deals first, so a kind that never leads is a sprite
/// that never appears. The rotation this replaced stranded `Tv` exactly that
/// way on all ten production floors.
#[test]
fn every_pod_decor_kind_can_open_a_floor() {
    let mut seen: Vec<crate::layout::PodDecor> = (0..crate::floor::MAX_FLOORS)
        .filter_map(|f| {
            SceneLayout::compute_with_seed(192, 80, None, crate::floor::floor_seed(f))
                .and_then(|l| l.pod_decor.first().map(|d| d.kind))
        })
        .collect();
    seen.sort_by_key(|k| format!("{k:?}"));
    seen.dedup();
    assert_eq!(
        seen.len(),
        crate::layout::PodDecor::ALL.len(),
        "only {seen:?} ever open a floor — a phase modulus that misses the \
         roster strands the rest"
    );
}

/// A wide floor's aisles must not read as one repeating cycle. A rotation can
/// only ever produce `PodDecor::ALL.len()` arrangements however many slots a
/// floor has, because one slot's kind fixes every later one.
#[test]
fn a_wide_floors_decor_order_is_not_one_fixed_cycle() {
    use std::collections::HashSet;
    for &(w, h) in &[(160u16, 120u16), (192, 158), (200, 116)] {
        let seen: HashSet<Vec<crate::layout::PodDecor>> = (0..crate::floor::MAX_FLOORS)
            .filter_map(|f| {
                SceneLayout::compute_with_seed(w, h, None, crate::floor::floor_seed(f))
                    .map(|l| l.pod_decor.iter().map(|d| d.kind).collect())
            })
            .collect();
        assert!(
            seen.len() > crate::layout::PodDecor::ALL.len(),
            "{w}x{h}: only {} distinct aisle arrangements over {} floors — no more \
             than a rotation of the roster would give",
            seen.len(),
            crate::floor::MAX_FLOORS
        );
    }
}

/// No two adjacent aisle slots share a kind. The rotation this bag replaced had
/// it by construction; a bag has to be told, because a fresh permutation may
/// open on the kind the last one closed with — two identical pieces in
/// neighbouring aisles is the failure a user sees first.
#[test]
fn no_two_adjacent_aisle_slots_share_a_kind() {
    // Enough passes that the pass BOUNDARY is exercised, not just one bag.
    let span = crate::layout::PodDecor::ALL.len() * 4;
    for f in 0..crate::floor::MAX_FLOORS {
        let seed = crate::floor::floor_seed(f);
        let kinds: Vec<_> = (0..span)
            .map(|i| super::compute::decor_for_slot(seed, i))
            .collect();
        for i in 1..kinds.len() {
            assert_ne!(
                kinds[i],
                kinds[i - 1],
                "floor {f}: slots {} and {i} are both {:?}",
                i - 1,
                kinds[i]
            );
        }
    }
}

/// The seed PACKING, frozen by value. Every other seat/plant test asserts a
/// property that survives any uniform seed — a swapped packing passes all of
/// them while moving every chair and pot away from the committed art, which only
/// the CI-only `gen-check` pixel diff would notice, and only as an opaque delta.
#[test]
fn point_seed_packing_is_frozen() {
    assert_eq!(
        super::decor::point_seed(Point { x: 1, y: 2 }),
        (1u64 << 32) | 2,
        "x occupies the high word and y the low one"
    );
    assert_eq!(super::decor::point_seed(Point { x: 0, y: 0 }), 0);
    assert_ne!(
        super::decor::point_seed(Point { x: 3, y: 5 }),
        super::decor::point_seed(Point { x: 5, y: 3 }),
        "the packing must not be symmetric in x and y"
    );
}
