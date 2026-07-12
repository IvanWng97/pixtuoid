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

use super::mask::{ground_rect, pantry_ground_rect};
use super::placement::rects_overlap;
use super::*;

/// The sweep's size axis. A union of: the corner sizes the retired tests
/// encoded (34–41 forced-single-pod widths; 48×60 decor-vs-wall corner;
/// 96×{60,115} the #551 Y-overflow windows), the golden/hero sizes, and a
/// spread up to a wide-corridor floor so the appliance kinds appear.
const SWEEP_SIZES: &[(u16, u16)] = &[
    (34, 60),
    (36, 100),
    (38, 120),
    (40, 70),
    (41, 160),
    (48, 60),
    (50, 80),
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
            ground: def
                .footprint
                .map(|fp| ground_rect(anchor, pos, fp, def.visual, def.ground_x, def.ground_y)),
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
        door: _, // wall-band architecture, not furniture: it PUNCHES walkability
        //                    through the blocked band (DOOR_CUT); pinned by the
        //                    connectivity invariant + door_threshold below
        door_threshold: _, // a walkable POINT, asserted by the connectivity tests
        meeting_room: _,   // container (room 0)
        meeting_room_2: _, // container (room 1)
        pantry_room: _,    // container
        meeting_furniture,
        room_walls: _, // the containers' edges; overlap-vs-walls is its own invariant
        top_margin: _, // wall-band geometry, read via wall_band_h() in invariants
        pantry_counter_size,
        kitchen_island,
        corridor: _, // router/pet zone, spans the full width by design
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
        // Per-ITEM container: the two corridor plants live in the band, the
        // two meeting plants in room 0 — same kinds, different homes, so the
        // container is picked by position (inside room 0 → room 0).
        let in_meeting = l
            .meeting_room
            .map(|mr| contains_point(mr, p.pos))
            .unwrap_or(false);
        out.push(Piece::table(
            format!("plant[{i}] {:?}", p.kind),
            Anchor::Center,
            p.pos,
            p.kind.furniture(),
            if in_meeting {
                Container::MeetingRoom(0)
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
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {}
            WaypointKind::Pantry => {
                // Runtime-sized: geometry comes from the shared
                // pantry_ground_rect, not the (deliberately empty) table row.
                let counter = *pantry_counter_size;
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

    for (room, mf) in meeting_furniture.iter().enumerate() {
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
    // couch_sprite_center: geometry comes from the 3 seat waypoints above
    // (the mask's truth); presence still feeds the every-kind coverage test.
    let _ = couch_sprite_center;

    if let Some(p) = kitchen_island {
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
        Container::Pantry => l.pantry_room,
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
        let walls: Vec<(Point, Size)> = l
            .room_walls
            .iter()
            .map(|seg| {
                if seg.start.x == seg.end.x {
                    // Vertical: WALL_THICK_V wide, seen edge-on.
                    (
                        seg.start,
                        Size {
                            w: WALL_THICK_V,
                            h: seg.end.y - seg.start.y + 1,
                        },
                    )
                } else {
                    // Horizontal: WALL_THICK_H tall (the glass face).
                    (
                        seg.start,
                        Size {
                            w: seg.end.x - seg.start.x + 1,
                            h: WALL_THICK_H,
                        },
                    )
                }
            })
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

#[test]
fn walkable_is_one_connected_region() {
    // ONE pixel-BFS (the strongest connectivity truth), swept across the full
    // grid — retires the two hand-rolled BFS copies that each swept a slice.
    sweep(|w, h, seed, l| {
        let Some(start) = l.door_threshold else {
            panic!("{w}x{h} seed {seed}: layout has no door threshold");
        };
        let total: usize = (0..l.buf_h)
            .map(|y| {
                (0..l.buf_w)
                    .filter(|&x| l.walkable.is_walkable(x, y))
                    .count()
            })
            .sum();
        let mut seen = vec![false; l.buf_w as usize * l.buf_h as usize];
        let idx = |x: u16, y: u16| y as usize * l.buf_w as usize + x as usize;
        let mut stack = vec![start];
        seen[idx(start.x, start.y)] = true;
        assert!(
            l.walkable.is_walkable(start.x, start.y),
            "{w}x{h} seed {seed}: door threshold {start:?} is not walkable"
        );
        let mut reached = 0usize;
        while let Some(p) = stack.pop() {
            reached += 1;
            let (px, py) = (p.x as i32, p.y as i32);
            for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
                let (nx, ny) = (px + dx, py + dy);
                if nx < 0 || ny < 0 || nx >= l.buf_w as i32 || ny >= l.buf_h as i32 {
                    continue;
                }
                let (nx, ny) = (nx as u16, ny as u16);
                if !seen[idx(nx, ny)] && l.walkable.is_walkable(nx, ny) {
                    seen[idx(nx, ny)] = true;
                    stack.push(Point { x: nx, y: ny });
                }
            }
        }
        assert_eq!(
            reached,
            total,
            "{w}x{h} seed {seed}: {} of {total} walkable px unreachable from the door \
             (a sealed pocket)",
            total - reached
        );
    });
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
    // a placement-site regression). Allowlist: BulletinBoard has NO push site
    // in compute (declared, never placed — documented in decor.rs's
    // role-mapping test); it stays registered for pack authors.
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
        if l.kitchen_island.is_some() {
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
        // Ficus: allowlisted like BulletinBoard — compute has NO push site
        // for it (registered kind, sized row, never placed). A polish-arc
        // candidate; when a site lands, delete this line and the sweep
        // starts guarding it.
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
    for fixed in ["floor_lamp", "lounge_side_table", "kitchen_island", "couch"] {
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
            l.meeting_room.is_some(),
            l.pantry_room.is_some(),
            l.meeting_room_2.is_some(),
            l.cubicle_band.x,
        ));
    }
    assert!(
        shapes.len() >= 5,
        "sweep seeds reach only {} distinct floor shapes: {shapes:?} — widen SWEEP_SEEDS",
        shapes.len()
    );
}
