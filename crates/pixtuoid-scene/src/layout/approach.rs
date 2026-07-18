//! Per-furniture approach geometry: where does an agent stand when it
//! visits a waypoint?
//!
//! The waypoint's `pos` is the furniture's geometric CENTER, which for
//! obstacle furniture (pantry counter, vending machine, …) is a blocked
//! cell — an agent can't stand on it. [`stand_point`] resolves the actual
//! stand cell: the walkable cell just off the footprint, on the side
//! nearest the agent's home desk. Walls naturally constrain it (a machine
//! flush against a wall only exposes its open side), so a counter is
//! approachable from any open side ("360°") while a recessed appliance is
//! approached from the front — matching the real world.
//!
//! Pure geometry over [`WalkableMask`] — no A*, no terminal deps — so the
//! stateless `pose::pure::idle_pose` and the stateful `motion` walk
//! destinations stay in lockstep with the render anchor (all three call
//! this with the same `origin = home desk`).

use super::decor::{furniture_def, Facing, Furniture, WaypointKind};
use super::reach::ReachSet;
use super::{Point, Size};
use pixtuoid_core::walkable::WalkableMask;

/// First clear pixel beyond a footprint half-extent. `mask.rs` stamps
/// waypoint furniture with `pad = 1`, so `half + 2` is the first
/// guaranteed-unblocked pixel; we then scan a few more out for robustness.
const STAND_CLEARANCE: u16 = 2;
/// How far past `half + STAND_CLEARANCE` to keep probing for a walkable
/// cell before giving up on a side.
const STAND_SCAN: i32 = 4;

/// The four cardinal unit axes (N, S, W, E) both `stand_point` and
/// `approach_point` scan over. `debug_overlay`'s DIRS is a different order and
/// stays separate.
const CARDINAL_DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

/// Squared Euclidean distance between two cells (computed in `i64` to avoid
/// `u16` overflow before squaring). Used to pick the candidate stand/approach
/// cell nearest the agent's `origin`; never sqrt'd, only compared.
fn squared_distance(from: Point, to: Point) -> u64 {
    let ex = from.x as i64 - to.x as i64;
    let ey = from.y as i64 - to.y as i64;
    (ex * ex + ey * ey) as u64
}

/// Ground-footprint `(w, h)` the walkable mask stamps for a waypoint, or
/// `None` for slots that add no obstacle (meeting sofa/stand sit on the
/// sofa/table furniture, already stamped elsewhere). Reads [`furniture_def`]
/// — the single source of truth for furniture shape — and special-cases the
/// one runtime-sized kind (`Pantry`, whose counter scales with terminal width
/// and so isn't a static table row). Consumed by `mask::build_walkable_mask`
/// and [`stand_point`], so the footprint can't drift between them.
pub(super) fn obstacle_footprint(kind: WaypointKind, pantry_counter_size: Size) -> Option<Size> {
    if matches!(kind, WaypointKind::Pantry) {
        return Some(pantry_counter_size);
    }
    furniture_def(kind.furniture()).footprint
}

/// The walkable cell where an agent should stand to use the OBSTACLE furniture
/// of `kind` centered at `pos`, given the agent's `origin` (home desk) — the
/// RENDER anchor for an `AtWaypoint` obstacle sprite. It MUST equal the walk goal
/// [`approach_point`] returns for the same furniture (else the sprite pops from
/// the side the agent walked to onto a different face on arrival), so it simply
/// DELEGATES there rather than re-scanning: seat furniture (`occupies_pos`)
/// renders ON its `pos` (the seat cell) and short-circuits; every obstacle
/// forwards to `approach_point` (keyed on `kind.furniture()`), whose obstacle
/// branch is the single source of the "stand just off the VISUAL, on the
/// reachable allowed side nearest `origin`" scan. Falls back to `pos` (the "no
/// valid approach" sentinel) exactly where `approach_point` does.
///
/// Equality is now true BY CONSTRUCTION — the old parallel scan is gone, so the
/// stand-vs-approach equality a dedicated guard once asserted is a tautology now;
/// that guard was repurposed to pin the obstacle branch's REAL invariants
/// (walkable / reachable / allowed-side / off-visual) in
/// `real_layout_obstacle_stand_is_a_reachable_allowed_off_visual_cell`.
pub(crate) fn stand_point(
    kind: WaypointKind,
    pos: Point,
    pantry_counter_size: Size,
    mask: &WalkableMask,
    origin: Point,
    facing: Facing,
    reachable: &ReachSet,
) -> Point {
    // Seat furniture (Couch / meeting slots) renders ON its `pos`; approach_point
    // would return the off-side approach cell, not the seat, so short-circuit —
    // matching the old `half_extents` `occupies_pos` early return.
    if furniture_def(kind.furniture()).occupies_pos {
        return pos;
    }
    // Obstacle: render anchor == walk goal. NB `approach_point` takes `facing`
    // BEFORE `pantry_counter_size` — the reverse of `stand_point`'s order.
    approach_point(
        kind.furniture(),
        pos,
        facing,
        pantry_counter_size,
        mask,
        origin,
        reachable,
    )
}

/// A seat body (sofa) is wider than an appliance footprint, so its side
/// approach cells sit farther out than [`STAND_SCAN`] would reach. Scan this
/// far from the seat centre to clear the furniture and land on the floor.
const SEAT_APPROACH_SCAN: i32 = 14;

/// Extent [`approach_point`] scans past for obstacle furniture — the VISUAL
/// (whole sprite), so the approach cell (and the [`stand_point`] render anchor
/// that delegates here) lands clear of everything drawn. (NOT the mask footprint,
/// now a shallow south strip — deriving the stand distance from it would pull the
/// user INSIDE the sprite.) The SINGLE extent source. `None` for kinds with no
/// ground footprint (seats / wall decor) — those never reach the obstacle branch.
/// The runtime-sized `Pantry` counter (visual (0,0)) is its own clearance.
///
/// INVARIANT: the `visual/2` half-extent `approach_point` derives from this
/// assumes `Anchor::Center` placement (pos = sprite center, so visual/2 reaches
/// the edges) — true for every obstacle WAYPOINT today (Pantry/PhoneBooth/
/// StandingDesk/VendingMachine/Printer all stamp `Anchor::Center` in `mask.rs`).
/// A future `TopLeft`-placed obstacle waypoint would compute the stand cell off a
/// wrong center and must pass a center, not the raw origin. The one TopLeft piece
/// that reaches the approach machinery (the home Desk) sidesteps this by being
/// `occupies_pos` (seat branch, no extent) + `desk_walk_anchor` scanning from the
/// CHAIR, not the desk's TopLeft origin.
fn approach_clearance_extent(kind: Furniture, pantry_counter_size: Size) -> Option<Size> {
    if matches!(kind, Furniture::Pantry) {
        return Some(pantry_counter_size);
    }
    let def = furniture_def(kind);
    def.footprint.map(|_| def.visual)
}

/// The walkable, A\*-REACHABLE cell A\* routes to when an agent visits furniture
/// `kind` at `pos` — the **approach point** (A\*'s goal). Honours the furniture's
/// allowed approach sides (`ApproachSides`, rotated by `facing`) AND coarse-grid
/// `reachable`-ility from `origin` (the home desk), so A\* never targets a
/// walled-off cell ("the available approach side"). Keyed on [`Furniture`] so the
/// home desk flows through the same selector as every waypoint.
///
/// - **`occupies_pos` seat/desk:** an allowed-side walkable+reachable cell
///   adjacent to the seat, nearest `origin`. The sprite renders on its fixed
///   [`seated_foot_cell`]; the post-A\* settle bridges approach → seat. The back
///   (excluded) side is never scanned — **a seat approach is never a back-side
///   cell** (you can't sit by climbing over the backrest).
/// - **obstacle:** the first reachable cell past the footprint on the allowed
///   side nearest `origin` — the agent stands there (render == approach point).
///
/// Returns the blocked `pos` as a **"no valid approach" sentinel** when no allowed
/// reachable side exists (a seat boxed in to only its back, or a fully-blocked
/// obstacle). Callers MUST treat `== pos` as "skip this furniture this cycle"
/// rather than routing to it — routing to `pos` would let A\* snap onto the back.
pub(crate) fn approach_point(
    kind: Furniture,
    pos: Point,
    facing: Facing,
    pantry_counter_size: Size,
    mask: &WalkableMask,
    origin: Point,
    reachable: &ReachSet,
) -> Point {
    let def = furniture_def(kind);
    let mut best: Option<(u64, Point)> = None;
    if def.occupies_pos {
        // SEAT/desk: the sprite renders on the fixed seat foot cell; the agent
        // walks IN from an ALLOWED side and the post-A* settle bridges approach →
        // seat. Pick the ApproachSides-allowed side (facing-rotated: a North couch
        // ⇒ {N,E,W}, the south backrest EXCLUDED) with a reachable cell NEAREST the
        // home desk — the agent's natural side. The back side is never even scanned:
        // a seat reachable ONLY from behind is un-sittable (you'd clip through the
        // backrest), so there is NO back-side fallback — we return the blocked `pos`
        // as the "no valid approach" sentinel and the caller skips the seat rather
        // than letting A* snap onto the backrest. INVARIANT: a seat approach is
        // never a back-side cell.
        let mut allowed: Option<(u64, Point)> = None;
        for (dx, dy) in CARDINAL_DIRS {
            if !def.approach.allows(facing, (dx, dy)) {
                continue; // never approach across an excluded side (the back)
            }
            // The FIRST walkable cell off this side can be a thin EDGE whose coarse
            // routing cell straddles the furniture (the back-row desk's gap edge
            // between pod rows): walkable, yet ReachSet-rejected. Step DEEPER
            // through the CONTIGUOUS walkable run for the first cell A* can actually
            // reach, instead of dropping the whole side at that edge. The seat's own
            // footprint is skipped while `entered` is still false; once we've
            // entered the run, the first blocked pixel STOPS the scan (`entered`
            // guard) so we never hop a SECOND obstacle to a far strip — that would
            // resurrect a cross-furniture / back-side approach (Session-3 invariant
            // `seat_approach_is_never_behind_the_backrest_on_real_layouts`).
            let mut entered = false;
            for dist in 1..=SEAT_APPROACH_SCAN {
                let cx = pos.x as i32 + dx * dist;
                let cy = pos.y as i32 + dy * dist;
                if cx < 0 || cy < 0 {
                    break;
                }
                let c = Point {
                    x: cx as u16,
                    y: cy as u16,
                };
                if mask.is_walkable(c.x, c.y) {
                    entered = true;
                    if reachable.reaches(c) {
                        let d2 = squared_distance(c, origin);
                        if allowed.is_none_or(|(b, _)| d2 < b) {
                            allowed = Some((d2, c));
                        }
                        break;
                    }
                    // walkable but coarse-unreachable → keep scanning this run.
                } else if entered {
                    break;
                }
            }
        }
        return allowed.map(|(_, p)| p).unwrap_or(pos);
    } else if let Some(Size { w: fw, h: fh }) = approach_clearance_extent(kind, pantry_counter_size)
    {
        // Obstacle: stand just off the footprint, on the reachable allowed side
        // nearest the home desk (== stand_point, plus the reachability filter).
        let (hx, hy) = (fw as i32 / 2, fh as i32 / 2);
        for (dx, dy) in CARDINAL_DIRS {
            if !def.approach.allows(facing, (dx, dy)) {
                continue;
            }
            let half = if dx != 0 { hx } else { hy };
            for step in 0..=STAND_SCAN {
                let dist = half + STAND_CLEARANCE as i32 + step;
                let cx = pos.x as i32 + dx * dist;
                let cy = pos.y as i32 + dy * dist;
                if cx < 0 || cy < 0 {
                    break;
                }
                let c = Point {
                    x: cx as u16,
                    y: cy as u16,
                };
                if mask.is_walkable(c.x, c.y) {
                    if reachable.reaches(c) {
                        let d2 = squared_distance(c, origin);
                        if best.is_none_or(|(bd, _)| d2 < bd) {
                            best = Some((d2, c));
                        }
                    }
                    break;
                }
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overhanging_obstacle_stand_clearance_uses_visual_not_footprint() {
        // The decouple: stand/approach clearance is the FULL `visual` (a USER
        // parks clear of the whole sprite), NOT the shallow mask footprint. If
        // this regressed to footprint, a user would stand INSIDE the sprite. Pin
        // the magnitude so the regression can't ship green. `approach_clearance_extent`
        // is the SINGLE extent source `stand_point` now delegates through.
        // PhoneBooth: footprint (6,3) but visual (6,12).
        let booth = furniture_def(Furniture::PhoneBooth);
        let ext = approach_clearance_extent(Furniture::PhoneBooth, Size { w: 0, h: 0 })
            .expect("booth is an approachable obstacle");
        assert_eq!(
            ext.h, booth.visual.h,
            "stand clearance must use the visual height, not the footprint"
        );
        assert!(
            ext.h > booth.footprint.unwrap().h,
            "visual extent ({}) must exceed the shallow footprint's ({})",
            ext.h,
            booth.footprint.unwrap().h
        );
        // StandingDesk too (visual (8,8) vs footprint (8,3)).
        let ext = approach_clearance_extent(Furniture::StandingDesk, Size { w: 0, h: 0 }).unwrap();
        assert_eq!(ext.h, furniture_def(Furniture::StandingDesk).visual.h);
    }

    /// Build an open mask with a centred obstacle stamped exactly like
    /// `mask.rs` does for a waypoint (pad = 1).
    fn mask_with_obstacle(w: u16, h: u16, pos: Point, fw: u16, fh: u16) -> WalkableMask {
        let mut m = WalkableMask::new_open(w, h);
        m.mark_blocked(pos.x - fw / 2, pos.y - fh / 2, fw, fh, 1);
        m
    }

    #[test]
    fn pantry_stand_is_walkable_and_off_center() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        let reach = ReachSet::from_mask(&m, Point { x: 5, y: 5 });
        // Origin to the north → expect a north-side stand cell.
        let s = stand_point(
            WaypointKind::Pantry,
            pos,
            Size { w: 32, h: 10 },
            &m,
            Point { x: 50, y: 8 },
            Facing::South,
            &reach,
        );
        assert!(m.is_walkable(s.x, s.y), "stand cell must be walkable");
        assert_ne!(s, pos, "must move off the blocked center");
        assert!(s.y < pos.y, "north origin → north stand, got {s:?}");
    }

    #[test]
    fn pantry_stand_follows_origin_side() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        let reach = ReachSet::from_mask(&m, Point { x: 5, y: 5 });
        let south = stand_point(
            WaypointKind::Pantry,
            pos,
            Size { w: 32, h: 10 },
            &m,
            Point { x: 50, y: 95 },
            Facing::South,
            &reach,
        );
        assert!(south.y > pos.y, "south origin → south stand, got {south:?}");
        let east = stand_point(
            WaypointKind::Pantry,
            pos,
            Size { w: 32, h: 10 },
            &m,
            Point { x: 98, y: 50 },
            Facing::South,
            &reach,
        );
        assert!(east.x > pos.x, "east origin → east stand, got {east:?}");
    }

    #[test]
    fn seat_kinds_pass_through_pos() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 20, 7);
        let reach = ReachSet::from_mask(&m, Point { x: 10, y: 10 });
        for kind in [
            WaypointKind::Couch,
            WaypointKind::MeetingSofa,
            WaypointKind::MeetingChair,
        ] {
            assert_eq!(
                stand_point(
                    kind,
                    pos,
                    Size { w: 0, h: 0 },
                    &m,
                    Point { x: 10, y: 10 },
                    Facing::South,
                    &reach,
                ),
                pos,
                "{kind:?} pos is already the seat cell"
            );
        }
    }

    #[test]
    fn real_layout_stand_points_are_walkable() {
        // Guards against a floor whose obstacle furniture has no walkable
        // approach side (the real geometry, not a synthetic mask). Seat kinds
        // (couch/meeting) intentionally return their on-furniture seat cell —
        // which IS blocked (the sprite sits on it; A* snaps the walk adjacent)
        // — so they're excluded from the walkability assertion.
        use crate::layout::SceneLayout;
        let seat = |k| {
            matches!(
                k,
                WaypointKind::Couch
                    | WaypointKind::MeetingSofa
                    | WaypointKind::MeetingChair
                    | WaypointKind::Island
            )
        };
        // 160×120 (cubicle_aisle 8 → no vending/printer) AND 160×150 (cubicle_aisle ≥10 →
        // vending + printer spawn) so the appliance stand cells are covered too.
        for (bw, bh) in [(160u16, 120u16), (160, 150)] {
            for seed in 0..5u64 {
                let l = SceneLayout::compute_with_seed(bw, bh, Some(6), seed).unwrap();
                let origin = l
                    .home_desks
                    .first()
                    .copied()
                    .unwrap_or(Point { x: 0, y: 0 });
                for wp in &l.waypoints {
                    let s = super::stand_point(
                        wp.kind,
                        wp.pos,
                        l.pantry_counter_size(),
                        &l.walkable,
                        origin,
                        wp.facing,
                        &l.reachable,
                    );
                    if seat(wp.kind) {
                        assert_eq!(s, wp.pos, "seed {seed}: {:?} should pass through", wp.kind);
                    } else {
                        assert!(
                            l.walkable.is_walkable(s.x, s.y) && s != wp.pos,
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} not a walkable off-center cell (center {:?})",
                            wp.kind,
                            wp.pos
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn real_layout_obstacle_stand_is_a_reachable_allowed_off_visual_cell() {
        // `stand_point ≡ approach_point` is now true BY CONSTRUCTION (stand_point
        // delegates to approach_point for obstacles), so asserting their equality is
        // a tautology — `f(X) == f(X)` can't fail whatever approach_point computes.
        // Instead pin the real invariants of the obstacle branch on live layouts:
        // the resolved stand cell is walkable, coarse-reachable, on a facing-allowed
        // (non-back) side, and OUTSIDE the whole visual sprite (clearance derives
        // from the visual, not the shallow footprint — else the agent stands inside
        // the sprite). Each sub-assertion can genuinely fail under a regression.
        use crate::layout::{furniture_def, SceneLayout};
        let obstacle = |k| {
            !matches!(
                k,
                WaypointKind::Couch
                    | WaypointKind::MeetingSofa
                    | WaypointKind::MeetingChair
                    | WaypointKind::Island
            )
        };
        for (bw, bh) in [
            (160u16, 120u16),
            (192, 160),
            (240, 160),
            (256, 200),
            (200, 140),
        ] {
            for seed in 0..16u64 {
                let Some(l) = SceneLayout::compute_with_seed(bw, bh, Some(6), seed) else {
                    continue;
                };
                for &desk in &l.home_desks {
                    for wp in l.waypoints.iter().filter(|w| obstacle(w.kind)) {
                        let s = stand_point(
                            wp.kind,
                            wp.pos,
                            l.pantry_counter_size(),
                            &l.walkable,
                            desk,
                            wp.facing,
                            &l.reachable,
                        );
                        if s == wp.pos {
                            continue; // "no valid approach" sentinel — nothing to stand on
                        }
                        assert!(
                            l.walkable.is_walkable(s.x, s.y),
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} not walkable",
                            wp.kind
                        );
                        assert!(
                            l.reachable.reaches(s),
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} not coarse-reachable",
                            wp.kind
                        );
                        // A pure single-axis offset from pos → its direction MUST be
                        // an allowed (non-back) side.
                        let dx = s.x as i32 - wp.pos.x as i32;
                        let dy = s.y as i32 - wp.pos.y as i32;
                        let dir = if dx.abs() >= dy.abs() {
                            (dx.signum(), 0)
                        } else {
                            (0, dy.signum())
                        };
                        let def = furniture_def(wp.kind.furniture());
                        assert!(
                            def.approach.allows(wp.facing, dir),
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} on a FORBIDDEN side {dir:?}",
                            wp.kind
                        );
                        // Clear of the WHOLE visual, not the shallow footprint: the
                        // offset along the stand axis is ≥ visual/2 + STAND_CLEARANCE.
                        // Read the visual from an INDEPENDENT authority (furniture_def /
                        // the pantry counter size) — NOT `approach_clearance_extent`, the
                        // fn `approach_point` itself uses to place the cell — so a
                        // visual→footprint regression there fails HERE (off < visual/2 +
                        // clearance) instead of shifting both the placement AND this
                        // oracle together (which would leave it toothless).
                        let visual = if wp.kind == WaypointKind::Pantry {
                            l.pantry_counter_size()
                        } else {
                            def.visual
                        };
                        let (half, off) = if dx != 0 {
                            (visual.w as i32 / 2, dx.abs())
                        } else {
                            (visual.h as i32 / 2, dy.abs())
                        };
                        assert!(
                            off >= half + STAND_CLEARANCE as i32,
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} sits INSIDE its visual \
                             (off {off} < half {half} + clearance {STAND_CLEARANCE})",
                            wp.kind
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn real_pantry_stand_responds_to_desk_side() {
        // On the real standard floor, a desk to the NORTH must not yield a
        // stand cell *south* of one chosen for a desk to the SOUTH — the side
        // tracks the origin (the whole point: a desk above the pantry → a
        // top-down approach, not a detour below).
        use crate::layout::SceneLayout;
        let l = SceneLayout::compute(120, 96, Some(4)).unwrap();
        let p = l
            .waypoints
            .iter()
            .find(|w| w.kind == WaypointKind::Pantry)
            .expect("pantry")
            .pos;
        let cs = l.pantry_counter_size();
        let north = stand_point(
            WaypointKind::Pantry,
            p,
            cs,
            &l.walkable,
            Point { x: p.x, y: 0 },
            Facing::South,
            &l.reachable,
        );
        let south = stand_point(
            WaypointKind::Pantry,
            p,
            cs,
            &l.walkable,
            Point {
                x: p.x,
                y: l.buf_h - 1,
            },
            Facing::South,
            &l.reachable,
        );
        assert!(
            north.y <= south.y,
            "north-desk stand {north:?} should be no lower than south-desk stand {south:?}"
        );
    }

    #[test]
    fn stand_point_breaks_on_negative_coords_near_the_origin_corner() {
        // An obstacle pressed into the top-left corner: scanning N or W from a
        // `pos` this close to (0,0) drives the probe cell below 0 BEFORE any
        // walkable cell is found, so the `cx < 0 || cy < 0` break ends those
        // sides. E/S stay positive and one of them wins. Covers the negative-
        // coordinate break in stand_point's outward scan.
        let pos = Point { x: 2, y: 2 };
        let counter = Size { w: 4, h: 4 };
        let m = mask_with_obstacle(100, 100, pos, counter.w, counter.h);
        // Reach seeded from the far-open corner: the E/S stand cells are connected
        // to it (the obstacle only blocks the top-left), so the filter is a no-op.
        let reach = ReachSet::from_mask(&m, Point { x: 90, y: 90 });
        // Origin to the NW (off-buffer-ish corner) so the nearest sides (N/W) are
        // preferred — but they break on negative coords, forcing an E/S result.
        let s = stand_point(
            WaypointKind::Pantry,
            pos,
            counter,
            &m,
            Point { x: 0, y: 0 },
            Facing::South,
            &reach,
        );
        assert!(m.is_walkable(s.x, s.y), "stand cell must be walkable");
        assert!(
            s.x > pos.x || s.y > pos.y,
            "N/W scans go negative and break → an E or S cell wins, got {s:?}"
        );
    }

    #[test]
    fn picks_only_open_side_when_walled() {
        // Vending machine recessed into a wall — everything down to y=54 is
        // blocked, so N/E/W are walled and only the south side is open. Even a
        // north origin must stand south.
        let pos = Point { x: 50, y: 50 };
        let mut m = WalkableMask::new_open(100, 100);
        m.mark_blocked(0, 0, 100, 54, 0);
        // Reach seeded from the open south band (the only walkable region), so the
        // south stand cell qualifies under the reachability filter.
        let reach = ReachSet::from_mask(&m, Point { x: 50, y: 80 });
        let s = stand_point(
            WaypointKind::VendingMachine,
            pos,
            Size { w: 0, h: 0 },
            &m,
            Point { x: 50, y: 5 },
            Facing::South,
            &reach,
        );
        assert!(m.is_walkable(s.x, s.y));
        assert!(s.y > pos.y, "only south is open, got {s:?}");
    }

    #[test]
    fn approach_point_seat_never_approaches_through_the_back() {
        // Open field so the reachability filter is a no-op and we isolate the
        // allowed-side (no-back) rule. The back is excluded even when the origin
        // sits directly behind the seat (which would otherwise pull it there).
        let pos = Point { x: 50, y: 50 };
        let m = WalkableMask::new_open(100, 100);
        let reach = ReachSet::from_mask(&m, Point { x: 5, y: 5 });
        // South-facing sofa: back is north. Origin due NORTH must NOT yield a
        // due-north (back) approach.
        let s = approach_point(
            Furniture::MeetingSofa,
            pos,
            Facing::South,
            Size { w: 0, h: 0 },
            &m,
            Point { x: 50, y: 5 },
            &reach,
        );
        assert!(s != pos && reach.reaches(s));
        assert!(
            !(s.x == pos.x && s.y < pos.y),
            "south-facing sofa back is north — must not approach due-north: {s:?}"
        );
        // North-facing sofa: back is south. Origin due SOUTH must NOT yield a
        // due-south (back) approach.
        let n = approach_point(
            Furniture::MeetingSofa,
            pos,
            Facing::North,
            Size { w: 0, h: 0 },
            &m,
            Point { x: 50, y: 95 },
            &reach,
        );
        assert!(n != pos && reach.reaches(n));
        assert!(
            !(n.x == pos.x && n.y > pos.y),
            "north-facing sofa back is south — must not approach due-south: {n:?}"
        );
    }

    #[test]
    fn seat_approach_is_never_behind_the_backrest_on_real_layouts() {
        // The physics invariant on REAL layouts: for every seat, across
        // sizes × seeds × desks, approach_point is EITHER the `pos` skip-sentinel
        // OR a cell on an ALLOWED side — NEVER a cell behind the backrest (the
        // excluded side). With no fallback, a back-approach can never be chosen.
        use crate::layout::{furniture_def, SceneLayout};
        for (w, h) in [(120u16, 96u16), (160, 120), (192, 160), (240, 160)] {
            for seed in 0..4u64 {
                let Some(l) = SceneLayout::compute_with_seed(w, h, Some(4), seed) else {
                    continue;
                };
                for &desk in &l.home_desks {
                    for wp in &l.waypoints {
                        let def = furniture_def(wp.kind.furniture());
                        if !def.occupies_pos {
                            continue;
                        }
                        let a = approach_point(
                            wp.kind.furniture(),
                            wp.pos,
                            wp.facing,
                            l.pantry_counter_size(),
                            &l.walkable,
                            desk,
                            &l.reachable,
                        );
                        if a == wp.pos {
                            continue; // skip sentinel — the wander avoids it, fine
                        }
                        // The approach is a pure single-axis offset from pos; its
                        // direction MUST be an allowed (non-back) side.
                        let dx = a.x as i32 - wp.pos.x as i32;
                        let dy = a.y as i32 - wp.pos.y as i32;
                        let dir = if dx.abs() >= dy.abs() {
                            (dx.signum(), 0)
                        } else {
                            (0, dy.signum())
                        };
                        assert!(
                            def.approach.allows(wp.facing, dir),
                            "{w}x{h} seed{seed}: {:?} at {:?} (facing {:?}) approach {a:?} is on \
                             a FORBIDDEN side {dir:?} — a back-approach was chosen",
                            wp.kind,
                            wp.pos,
                            wp.facing,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn approach_point_seat_never_approaches_from_the_back_even_when_walled_in() {
        // A North-facing sofa boxed in on N/E/W (its three ALLOWED sides), with
        // ONLY the south backrest side opening onto a reachable corridor. Physics:
        // you cannot sit by climbing over the backrest, so there is NO valid
        // approach. approach_point must return the blocked `pos` (the "no valid
        // side" sentinel for the caller to skip), NOT a back-side cell — a settle
        // must never cross the backrest line, not even as a last resort.
        let pos = Point { x: 50, y: 50 };
        let mut m = WalkableMask::new_open(100, 100);
        m.mark_blocked(0, 0, 100, 100, 0); // block everything…
        m.mark_walkable(48, 54, 5, 44); // …then carve a corridor SOUTH (behind the back)
        let reach = ReachSet::from_mask(&m, Point { x: 50, y: 90 });
        let s = approach_point(
            Furniture::MeetingSofa,
            pos,
            Facing::North,
            Size { w: 0, h: 0 },
            &m,
            Point { x: 50, y: 95 }, // origin in the south corridor (behind the back)
            &reach,
        );
        assert_eq!(
            s, pos,
            "only the backrest side is reachable → no valid approach → return pos (skip), got {s:?}"
        );
    }

    #[test]
    fn obstacle_stand_point_resolves_to_a_reachable_walkable_off_center_cell() {
        // On an open field an obstacle's stand cell must be a walkable, coarse-
        // reachable cell OFF the blocked center. (stand_point ≡ approach_point is
        // now true BY CONSTRUCTION — stand_point delegates — so an equality assert
        // would be a tautology; pin the real properties instead.)
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        let reach = ReachSet::from_mask(&m, Point { x: 5, y: 5 });
        let origin = Point { x: 50, y: 8 };
        let sp = stand_point(
            WaypointKind::Pantry,
            pos,
            Size { w: 32, h: 10 },
            &m,
            origin,
            Facing::South,
            &reach,
        );
        assert_ne!(sp, pos, "must resolve off the blocked center");
        assert!(m.is_walkable(sp.x, sp.y), "the stand cell must be walkable");
        assert!(
            reach.reaches(sp),
            "the stand cell must be coarse-reachable here"
        );
    }

    // ── furniture_def single-source-of-truth invariants ────────────────────
    // These iterate `WaypointKind::ALL` so re-introducing a hardcoded shape
    // number or breaking a derivation fails here, not as a silent visual bug.

    #[test]
    fn def_footprint_matches_obstacle_footprint() {
        // furniture_def is the SoT; obstacle_footprint must agree for every
        // non-Pantry kind (Pantry is runtime-sized → special-cased).
        let dummy = Size { w: 32, h: 10 };
        for &kind in WaypointKind::ALL {
            if kind == WaypointKind::Pantry {
                continue;
            }
            assert_eq!(
                furniture_def(kind.furniture()).footprint,
                obstacle_footprint(kind, dummy),
                "{kind:?}: furniture_def.footprint must equal obstacle_footprint",
            );
        }
    }

    #[test]
    fn occupies_pos_is_exactly_the_seat_kinds() {
        for &kind in WaypointKind::ALL {
            let expected = matches!(
                kind,
                WaypointKind::Couch
                    | WaypointKind::MeetingSofa
                    | WaypointKind::MeetingChair
                    | WaypointKind::Island
            );
            assert_eq!(
                furniture_def(kind.furniture()).occupies_pos,
                expected,
                "{kind:?}: occupies_pos must be true iff the agent occupies pos directly",
            );
        }
    }

    #[test]
    fn approachable_obstacle_resolves_clearance_extent() {
        let dummy = Size { w: 32, h: 10 };
        for &kind in WaypointKind::ALL {
            let def = furniture_def(kind.furniture());
            let has_footprint = def.footprint.is_some() || kind == WaypointKind::Pantry;
            if has_footprint && !def.occupies_pos {
                assert!(
                    approach_clearance_extent(kind.furniture(), dummy).is_some(),
                    "{kind:?}: an approachable obstacle must resolve a clearance extent",
                );
            }
        }
    }

    #[test]
    fn dwell_range_is_nonzero() {
        // pose::dwell_ms does `% range.max(1)`; a zero range would silently
        // collapse the jitter. Keep every row's range positive.
        for &kind in WaypointKind::ALL {
            assert!(
                furniture_def(kind.furniture()).dwell.range_ms > 0,
                "{kind:?}: dwell range must be > 0",
            );
        }
    }

    #[test]
    fn pod_and_waypoint_twins_resolve_to_one_furniture_row() {
        // PhoneBooth/StandingDesk exist as BOTH a PodDecor and a WaypointKind;
        // after the fold they must map to the SAME Furniture row, so geometry
        // cannot drift between the two roles.
        use crate::layout::PodDecor;
        for (pod, wp) in [
            (PodDecor::PhoneBooth, WaypointKind::PhoneBooth),
            (PodDecor::StandingDesk, WaypointKind::StandingDesk),
        ] {
            assert_eq!(
                pod.furniture(),
                wp.furniture(),
                "{pod:?}/{wp:?}: pod + waypoint twins must share one Furniture row",
            );
        }
    }

    #[test]
    fn waypoint_kind_all_is_unique_and_complete() {
        use std::collections::HashSet;
        let set: HashSet<_> = WaypointKind::ALL.iter().copied().collect();
        assert_eq!(
            set.len(),
            WaypointKind::ALL.len(),
            "ALL contains duplicates"
        );
        // furniture_def's match is compiler-forced exhaustive; this count
        // catches a new variant that was added there but not to ALL.
        assert_eq!(
            WaypointKind::ALL.len(),
            10,
            "a WaypointKind variant was added/removed — update ALL",
        );
    }
}
