//! The generative placement-invariant sweep — the MECHANISM for testing
//! furniture placement, not another pile of per-piece tests.
//!
//! Every invariant here is derived from the `FurnitureDef` table + the
//! `SceneLayout` collections, swept across a sizes × seeds grid that folds in
//! every corner size the retired hand-written tests encoded. Two teeth make
//! this a harness rather than more tests:
//!
//! 1. **Single-source geometry** — piece rects come from the SAME
//!    `mask::ground_rect` / `mask::pantry_ground_rect` the walkable mask
//!    stamps, so the sweep can never drift from the collision truth.
//! 2. **Exhaustive enumeration** — [`pieces`] destructures `SceneLayout`
//!    field-by-field with NO `..`: adding a new furniture collection fails
//!    compilation HERE until the new field is either fed into the sweep or
//!    explicitly exempted with a WHY. New furniture cannot ship unpinned.
//!
//! What deliberately does NOT live here: the FurnitureDef table's own axioms
//! (decor.rs tests), stamp/anchor algebra (mask.rs/placement.rs tests),
//! approach/reach/pathfind semantics on synthetic masks, and the render
//! layer's z-sort/occlusion suite — re-asserting those would make one table
//! edit fail two suites. Position-STABILITY stays with the insta goldens.

use super::mask::pantry_ground_rect;
use super::placement::rects_overlap;
use super::*;

/// The sweep's size axis. A union of: the corner sizes the retired tests
/// encoded (34–41 forced-single-pod widths; 48×60 decor-vs-wall corner;
/// 96×{60,115} the #551 Y-overflow windows), the golden sizes, the live wasm
/// hero buffers (desktop 231×130 and the 64px portrait floor — the most-seen
/// layouts since #568), and a spread up to a wide-corridor floor so the
/// appliance kinds appear.
const SWEEP_SIZES: &[(u16, u16)] = &[
    (34, 60),
    (36, 100),
    (38, 120),
    (40, 70),
    (41, 160),
    (48, 60),
    (50, 80),
    (64, 130),
    (96, 60),
    (96, 70),
    (96, 100),
    (96, 115),
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
/// Fibonacci hash (pinned observationally by `the_sweep_reaches_every_floor_variant`
/// — if the hash or variant count changes, that test names the gap instead of
/// the coverage silently shrinking).
const SWEEP_SEEDS: std::ops::Range<u64> = 0..12;

/// Run `f` over every layout in the sweep. Production fill (`max_desks: None`)
/// so the desk grid is at its densest — the strictest placement case. A `None`
/// layout is asserted to be a legitimate refusal (below the documented
/// minimum), never silently skipped: the silent `continue` in the old sweeps
/// let small-size regressions hide.
fn sweep(mut f: impl FnMut(u16, u16, u64, &SceneLayout)) {
    for &(w, h) in SWEEP_SIZES {
        for seed in SWEEP_SEEDS {
            match SceneLayout::compute_with_seed(w, h, None, seed) {
                Some(l) => f(w, h, seed, &l),
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

/// Which Bounds a piece's rect must stay inside (the per-kind container map —
/// one honest container per piece, NOT one rule for all: wall decor straddles
/// the wall band by design, appliances live in the aisle, room furniture in
/// its own room).
#[derive(Clone, Copy, Debug)]
enum Container {
    /// The cubicle band (desks, pod decor, lounge pieces, the free-standing
    /// whiteboard, corridor plants).
    Band,
    /// The appliance strip south of the band (vending machine, printer).
    Aisle,
    /// Meeting room `room_id` — resolved via `meeting_room_bounds`, the one
    /// join point.
    MeetingRoom(usize),
    Pantry,
    /// The carpet apron rows `[wall_band_h(), top_margin)` at the wall base —
    /// the straddling wall decor's ground strip (bookshelf, meeting screen).
    WallApron,
    /// The window-wall band rows `[0, top_margin)` (truly wall-hung decor:
    /// exit sign).
    WallBand,
}

/// One placed piece, with its mask-true geometry and its containment class.
struct Piece {
    /// Failure-message identity: kind + placement site.
    label: String,
    /// Blocked-ground rect from THE shared formula. `None` = the piece stamps
    /// no obstacle of its own (wall-hung decor).
    ground: Option<(Point, Size)>,
    /// The anchored visual box (sprite extent) — must stay inside the buffer.
    visual: (Point, Size),
    /// For `Anchor::Center` pieces: the unclamped center position + visual
    /// size, to catch a west/north spill that `anchored_top_left`'s
    /// `saturating_sub` silently clamps to 0 (a centered piece "fits" iff
    /// `pos >= visual/2` on each axis).
    center_fit: Option<(Point, Size)>,
    container: Container,
    /// Also require the VISUAL box inside the container (pod decor: the
    /// placement sites SKIP a slot whose whole sprite wouldn't fit the band —
    /// that skip semantic is part of the contract, not just the ground).
    visual_in_container: bool,
    /// Pieces sharing a group id are one physical cluster (the lounge
    /// vignette: the 3-seat couch's overlapping body stamps + its lamp and
    /// side table) — exempt from the pairwise-overlap invariant WITHIN the
    /// group.
    /// DISCIPLINE (this is the harness's one exemption with no compile
    /// tooth): a NEW group id requires (a) a WHY comment at the declaration
    /// naming the authored composition, and (b) the cluster's internal
    /// geometry pinned by a golden — a group is one designed vignette, never
    /// a way to silence a real overlap finding.
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

/// Enumerate EVERY placed piece of a layout. The destructure below has no
/// `..` on purpose — see the module doc's tooth #2. A field that contributes
/// no piece is bound and discarded with the WHY on the same line.
fn pieces(l: &SceneLayout) -> Vec<Piece> {
    let SceneLayout {
        buf_w: _,         // the Buffer container — read by the invariants directly
        buf_h: _,         // ditto
        cubicle_band: _,  // container, not a piece
        cubicle_aisle: _, // container, not a piece
        home_desks,
        waypoints,
        plants,
        wall_decor,
        pod_decor,
        floor_lamp,
        lounge_side_table,
        fish_tank,
        door: _, // wall-band architecture, not furniture: it PUNCHES walkability
        //                    through the blocked band (DOOR_CUT); pinned by the
        //                    connectivity invariant + door_threshold below
        door_threshold: _, // a walkable POINT, asserted by the connectivity tests
        meeting_rooms,
        pantry,
        room_walls: _, // the containers' edges; overlap-vs-walls is its own invariant
        doorways: _,   // architectural openings, not furniture — the connectivity
        //                invariant proves every room drains through them
        top_margin: _, // wall-band geometry, read via wall_band_h() in invariants
        corridor: _,   // router/pet zone, spans the full width by design
        couch_sprite_center,
        walkable: _,  // probed directly by the connectivity invariant
        reachable: _, // conservative routing truth, exercised by pathfind tests
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
        // push_slot skips any slot whose CENTERED SPRITE would overflow the
        // band — the whole visual stays in-band, not just the ground strip.
        piece.visual_in_container = true;
        out.push(piece);
    }

    for (i, p) in plants.iter().enumerate() {
        // Per-ITEM container: picked by POSITION — meeting plants in room 0,
        // corridor plants in the band, and a plant that settle_plant moved
        // beside a corner appliance lives on the AISLE like the appliance
        // itself (the beside-spot adopts the blocker's row).
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
            // vec (the kind is dual-homed; as a pod-decor twin it's centered,
            // HERE it's TopLeft) — the container is keyed on the KIND, not on
            // which Vec the item came from.
            WallDecor::Whiteboard => Container::Band,
            // Straddlers: tall sprite on the wall, shallow ground strip on the
            // carpet apron at the wall base.
            WallDecor::Bookshelf | WallDecor::MeetingScreen => Container::WallApron,
            // Truly hung: no ground of their own.
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
            // Each couch seat stamps its own 8×7 body into the mask (the
            // union of the 3 ±6dx stamps IS the couch's true blocked ground,
            // ~20 wide) — model exactly that, grouped as one physical object
            // so the by-design mutual overlap is exempt. couch_sprite_center
            // is NOT used for geometry: it under-models the union by 12px.
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
            // Duplicates of the pod_decor items at the same pos (promoted
            // slots) — the pod_decor entry above carries their geometry.
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            // Seats on meeting furniture: no obstacle of their own
            // (footprint: None); their containment is the pos-in-room check
            // in `every_meeting_slot_sits_in_its_room`.
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
            // The snack shelf is an approachable obstacle living in the
            // pantry (vending-machine class, pantry container).
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
            // Island stands carry no ground of their own: the island BODY
            // registers via `kitchen_island` below. Their pos-in-room
            // containment is covered by the kind→zone arm in
            // compute_places_all_waypoint_kinds.
            WaypointKind::Island => {}
        }
    }

    for (room, r) in meeting_rooms.iter().enumerate() {
        // Tooth #2 EXTENDS into the aggregate: destructure MeetingRoom (and
        // its trio) with no `..`, so a NEW field on either struct is a
        // compile error here until its pieces are registered — the same
        // force the SceneLayout destructure above exerts on flat fields.
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

    // The lounge vignette (couch seats above + lamp + side table) is ONE
    // authored cluster — the table tucks against the couch's west armrest and
    // the lamp hugs its east side BY DESIGN, so they share overlap group 2
    // (like the pantry cluster). Their internal geometry is pinned by the
    // layout goldens, not the overlap invariant.
    if let Some(p) = floor_lamp {
        out.push(Piece::table(
            "floor_lamp".into(),
            Anchor::Center,
            *p,
            Furniture::FloorLamp,
            Container::Band,
            Some(2),
        ));
    }
    if let Some(p) = lounge_side_table {
        out.push(Piece::table(
            "lounge_side_table".into(),
            Anchor::Center,
            *p,
            Furniture::LoungeSideTable,
            Container::Band,
            Some(2),
        ));
    }
    if let Some(p) = fish_tank {
        // Joins the lounge cluster: it backs onto the wall band beside the
        // lamp by design, so it shares the vignette's overlap group.
        out.push(Piece::table(
            "fish_tank".into(),
            Anchor::Center,
            *p,
            Furniture::FishTank,
            Container::Band,
            Some(2),
        ));
    }
    // couch_sprite_center: geometry comes from the 3 seat waypoints above
    // (the mask's truth); presence still feeds the every-kind coverage test.
    let _ = couch_sprite_center;

    // Tooth #2 on the pantry aggregate (same rationale as MeetingRoom above).
    let island = pantry.as_ref().and_then(|p| {
        let PantryRoom {
            bounds: _,       // container, asserted by Container::Pantry below
            counter_size: _, // the runtime-sized counter piece registers via
            //                  the Pantry waypoint arm above
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
/// legitimately doesn't exist for this layout, which is itself a failure —
/// a piece can't be placed in a room the floor doesn't have.
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

// ─── The invariants ─────────────────────────────────────────────────────────

/// Collect violations across the WHOLE sweep, then fail once with the full
/// list (capped) — a fail-fast assert reports only the first cell and hides
/// the pattern (one bug vs a systemic clamp miss look identical).
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
    // Ground + visual, all FOUR edges. East/south overflow shows as
    // rect-past-buffer; west/north overflow is sneakier — `saturating_sub`
    // clamps a spilling centered piece to 0 so the rect LOOKS in-bounds —
    // hence the center_fit check (`pos >= visual/2` per axis).
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
    // Mask ≡ pieces parity: a piece whose ground rect is NOT blocked in the
    // walkable mask is a MISSING STAMP — the piece renders (and the sweep's
    // rect invariants pass) while agents walk straight through it. Caught
    // live: the kitchen island's mask stamp was silently dropped by a bad
    // edit; every rect invariant stayed green because none of them read the
    // MASK. Interior-only probe (no pad assumptions).
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
    // Nothing asserted this anywhere before the harness: two pieces whose
    // BLOCKED GROUNDS intersect are physically inside each other (sprite
    // overhangs may overlap freely — that's occlusion, not placement).
    // Same-group pieces (couch stamps, pantry cluster) are one physical
    // object and exempt.
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
    // The generalization of the retired freestanding-decor test: EVERY
    // piece's unpadded ground vs every wall segment's physical rect.
    // (Padded rects legitimately touch walls — pad is routing slack.)
    let mut v = Vec::new();
    sweep(|w, h, seed, l| {
        // THE mask's own wall-rect derivation (incl. the north-band seg_top
        // raise a hand-rolled copy here once missed) — the sweep and the mask
        // cannot disagree on wall geometry.
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

/// The strongest connectivity truth: the door threshold is walkable AND every
/// walkable pixel is reachable from it (4-connected). Reuses the PRODUCTION
/// `compute::unreachable_walkable_cells` — the SAME flood the #566 connectivity
/// guard runs — so the guard and its strongest test can't drift (the two-copies
/// class the module doc + `ground_rect` warn about). The threshold-walkable
/// assert is SEPARATE + first: `unreachable_walkable_cells` returns empty on a
/// BLOCKED seed (a no-op failsafe), so without it a sealed CLASS-A threshold
/// would pass vacuously.
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
    // ONE pixel-BFS (the strongest connectivity truth), swept across the full
    // grid — retires the two hand-rolled BFS copies that each swept a slice.
    sweep(assert_walkable_connected);
}

#[test]
fn no_walkable_hole_where_a_vertical_wall_meets_a_horizontal_one() {
    // A divider crossed by an E-W wall is TWO vertical segments (upper room's
    // east wall, lower room's / pantry's) meeting at the cross wall. The honest
    // 4px wall + the render's stitch each open a gap the mask must also close, or
    // the walkable map shows a bite out of the corner (a walker could stand IN
    // the divider): (a) the lower segment's north walk-behind cap left a notch
    // ABOVE it, and (b) the upper segment's south end left the L-notch on the
    // wall's east columns UNfilled. Both are gone now that `wall_segment_rect`
    // shares `stitch_vertical_wall` with the painter — assert every corner row is
    // solid across the wall's whole width.
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
/// grid structurally skips (#566). A step-1 scan here can't hide a sealed pocket
/// at a width the grid happens to miss (door_threshold sealed by the lounge couch
/// at a band split to exactly 30; the appliance strip sealed by a scatter plant
/// on the sole inter-pod drain at a single-pod-column band). Upper bound 76 (not
/// 64) covers the FULL single-pod-column window: the desk grid stays one column
/// through buf_w≈70 and only splits to two (two drains, robust) at ≈71, so 65-76
/// must be swept too — the discrete grid's nearest points are 64 and 96.
const NARROW_BAND: std::ops::RangeInclusive<u16> = 32..=76;

#[test]
fn narrow_band_connectivity_boundary_scan() {
    // Step-1 width sweep across the degradation band — the discrete SWEEP_SIZES
    // grid can't cover every width, so a pocket at a skipped width (39/59/…)
    // shipped silently before #566. Heights span the tall floors where the
    // aisle-seal manifests; all 12 seeds reach every FloorVariant.
    for w in NARROW_BAND {
        for &h in &[80u16, 100, 120, 160] {
            for seed in SWEEP_SEEDS {
                if let Some(l) = SceneLayout::compute_with_seed(w, h, None, seed) {
                    assert_walkable_connected(w, h, seed, &l);
                }
            }
        }
    }
}

#[test]
fn door_threshold_walkable_at_a_band_split_to_thirty() {
    // #566 CLASS A pin: at a cubicle band exactly 30 px wide the lounge couch's
    // east seat sealed the spawn threshold's own column (39x160 seed 1 is one
    // such split). The couch↔door clearance gate drops the couch there.
    let l = SceneLayout::compute_with_seed(39, 160, None, 1).expect("39x160 lays out");
    let dt = l.door_threshold.expect("has a door threshold");
    assert!(
        l.walkable.is_walkable(dt.x, dt.y),
        "door threshold {dt:?} must be walkable — the couch may not seal the spawn column"
    );
}

#[test]
fn appliance_strip_not_sealed_at_a_single_pod_band() {
    // #566 CLASS B pin: at 59x160 seed 3 the band fits ONE pod column, so the
    // only aisle drain is the intra-pod gap — a scatter plant settling onto the
    // printer's row plugged it, sealing a 182-px appliance strip. The
    // connectivity guard drops the aisle-resident plant.
    let l = SceneLayout::compute_with_seed(59, 160, None, 3).expect("59x160 lays out");
    assert_walkable_connected(59, 160, 3, &l);
}

#[test]
fn free_standing_whiteboard_yields_when_it_seals_the_west_aisle() {
    // #566 CLASS C pin (honest-wall follow-up): at 32x120 seed 3 the divider is
    // two stacked 7px meeting rooms whose east wall is the vertical divider. The
    // free-standing whiteboard sits +3px east of that wall — fine against the old
    // 1px wall (the N-S drain ran through the cols the thin wall left open), but
    // the honest 4px wall now sits flush against the board's west edge, and the
    // desk column closes the east side, sealing the entire south (~985 px). The
    // whiteboard is decor: the connectivity guard drops it and the office
    // reconnects — WITHOUT sacrificing the (innocent, far-south) scatter plants.
    let l = SceneLayout::compute_with_seed(32, 120, None, 3).expect("32x120 lays out");
    assert_walkable_connected(32, 120, 3, &l);
    assert!(
        !l.wall_decor
            .iter()
            .any(|d| matches!(d.kind, super::WallDecor::Whiteboard)),
        "the sealing whiteboard must be dropped, not merely un-blocked (else the \
         painter draws a whiteboard a walker passes straight through)"
    );
    assert_eq!(
        l.plants.len(),
        2,
        "the two innocent far-south plants survive — only the whiteboard yields"
    );
}

#[test]
fn couch_survives_a_narrow_band_that_clears_the_door() {
    // Over-drop guard for the CLASS A gate (the boundary scan can't catch an
    // over-drop — dropping the couch only IMPROVES connectivity). 40x160 seed 1
    // is the KNIFE-EDGE: door_threshold.x == couch_x+11 (the real east edge), so
    // the couch clears by exactly 1 px — this pins couch_east_ground on the seat
    // pad (WAYPOINT_STAMP_PAD_PX=1), not OBSTACLE_PAD_PX=2 (which over-dropped it).
    // 48x160 seed 0 is a comfortable clearer.
    for &(w, h, seed) in &[(40u16, 160u16, 1u64), (48, 160, 0)] {
        let l = SceneLayout::compute_with_seed(w, h, None, seed).expect("lays out");
        assert!(
            l.couch_sprite_center.is_some(),
            "{w}x{h} seed {seed}: the lounge couch must survive a band that clears the door"
        );
    }
}

#[test]
fn desk_capacity_obeys_the_request_law() {
    // The universal law the three one-shot capacity tests sampled:
    // `None` ⇒ the physical fill; `Some(n)` ⇒ exactly min(n, capacity).
    // Probed on a sub-grid (capacity re-computes the layout per n).
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
    // Existential coverage: every registered role-enum variant must appear in
    // at least ONE swept layout — a kind that never places is dead weight (or
    // a placement-site regression). Allowlist: BulletinBoard stays unplaced by
    // design (registered for pack authors).
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
        if l.floor_lamp.is_some() {
            seen.insert("floor_lamp".into());
        }
        if l.lounge_side_table.is_some() {
            seen.insert("lounge_side_table".into());
        }
        if l.fish_tank.is_some() {
            seen.insert("fish_tank".into());
        }
        if l.pantry.is_some_and(|p| p.kitchen_island.is_some()) {
            seen.insert("kitchen_island".into());
        }
        if l.couch_sprite_center.is_some() {
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
        // BulletinBoard: allowlisted — no push site in compute, see above.
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
    // Seat waypoints carry no ground (the sofa body does) — their honest
    // containment is pos-in-room, joined through meeting_room_bounds.
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
    // Guard the sweep's own coverage: the seeds must reach all five floor
    // shapes (observationally — variant internals are private). If the
    // variant hash or count changes, THIS names the gap instead of the other
    // invariants silently narrowing.
    // The observable signature needs mid_x (= cubicle_band.x − 1): Senior
    // differs from Standard, and Lounge from OpenPlan, ONLY by the left-column
    // percent — room presence alone collapses the 5 variants to 3 shapes.
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
        shapes.len() >= 5,
        "sweep seeds reach only {} distinct floor shapes: {shapes:?} — widen SWEEP_SEEDS",
        shapes.len()
    );
}

#[test]
fn plant_obstacle_census_honors_repels_plants() {
    // The ONE census (`compute::plant_obstacle_rects`) includes a singleton IFF
    // its kind `repels_plants`: the fish tank + kitchen island (solid bodies) are
    // IN; the lounge lamp + side table (the owner-ratified Ficus hug) are OUT.
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
    // with none of the repelling singletons present, the census is empty
    assert!(
        super::compute::plant_obstacle_rects(None, Some(p(1, 1)), Some(p(2, 2)), None, &[])
            .is_empty(),
        "only repels_plants singletons enter the census"
    );
}

#[test]
fn scatter_plants_keep_obstacle_clearance_and_survive_by_sliding() {
    // Yield-by-DELETION was
    // structurally universal (the corner appliances share the plants'
    // authored corners at most sizes), stripping greenery office-wide and
    // leaving OpenPlan floors with zero plants. Plants now SLIDE inward
    // along the aisle before giving up. Ridden on the full sweep grid
    // (sizes x seeds x production fill), both halves:
    //   (a) clearance invariant — no plant box within
    //       PLANT_OBSTACLE_CLEARANCE_PX of an obstacle waypoint's box;
    //   (b) greenery pin — an appliance never costs the corridor its plant
    //       (vending present => a corridor-row Flower exists; printer
    //       present => a corridor-row Succulent exists).
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
    // Does the plant box come within PLANT_OBSTACLE_CLEARANCE_PX of the obstacle
    // box (obs_pos, obs_v)? Inflate the obstacle box by the clearance on every side
    // and test overlap. The center-anchored predicate the WAYPOINT family needs;
    // the fixed non-waypoint singletons instead ride the SAME `plant_obstacle_rects`
    // census the production path uses (TL rects → `overlaps_within_clearance`).
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
            // The fixed non-waypoint singletons — the SAME `plant_obstacle_rects`
            // census the production settle path uses (fish tank / meeting-trio
            // bodies / kitchen island, filtered by `repels_plants`), no longer a
            // hand-re-derived second copy that shipped each interpenetration bug.
            // A census miss here IS a plant interpenetration.
            for (otl, osz) in super::compute::plant_obstacle_rects(
                l.fish_tank,
                l.floor_lamp,
                l.lounge_side_table,
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
        // Greenery pin only where room exists: on bands narrower than 60 the
        // slide has nowhere to land (appliance + clearance + plant + the
        // packed desk field exceed the corner) and tiny-floor degradation is
        // the house norm — the pin's job is preventing the office-WIDE loss
        // the first cut shipped at flagship sizes.
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
