use super::*;
use crate::layout::{Layout, WallSegment};

fn make_layout() -> Layout {
    Layout::compute(160, 200, Some(4)).expect("layout fits")
}

#[test]
fn straight_line_when_unobstructed() {
    let l = make_layout();
    let overlay = OccupancyOverlay::new();
    let from = Point {
        x: l.corridor.unwrap().x + 10,
        y: l.corridor.unwrap().y + 2,
    };
    let to = Point {
        x: l.corridor.unwrap().x + 60,
        y: l.corridor.unwrap().y + 2,
    };
    let path = find_path(&l.walkable, &overlay, None, from, to).expect("path");
    assert!(path.len() >= 2);
    assert_eq!(path[0], from);
    assert_eq!(*path.last().unwrap(), to);
}

#[test]
fn simplify_collapses_collinear() {
    let pts = vec![
        Point { x: 0, y: 0 },
        Point { x: 4, y: 0 },
        Point { x: 8, y: 0 },
        Point { x: 12, y: 0 },
        Point { x: 12, y: 4 },
    ];
    let s = simplify_polyline(pts);
    assert_eq!(s.len(), 3);
}

// A DIAGONAL run (nonzero dx AND dy) exercises the collinearity determinant's
// x-delta terms, which an axis-aligned run zeroes out (dy_in=dy_out=0 makes
// the determinant 0 regardless of dx). Off-origin so the `here.x - prev.x`
// subtraction can't be hidden by a `prev` of 0. A 3-point input also pins the
// `len < 3` boundary: a `<= 3`/`== 3` early-return would leave this uncollapsed.
#[test]
fn simplify_collapses_diagonal_collinear() {
    let pts = vec![
        Point { x: 1, y: 1 },
        Point { x: 3, y: 3 },
        Point { x: 5, y: 5 },
    ];
    assert_eq!(simplify_polyline(pts).len(), 2);
}

// The complement: a genuine 3-point corner must SURVIVE (determinant != 0),
// so the collapse above can't be a degenerate "always drops the midpoint".
#[test]
fn simplify_keeps_genuine_corner() {
    let pts = vec![
        Point { x: 0, y: 0 },
        Point { x: 2, y: 0 },
        Point { x: 2, y: 2 },
    ];
    assert_eq!(simplify_polyline(pts).len(), 3);
}

#[test]
fn routes_around_meeting_room_wall() {
    let l = make_layout();
    let overlay = OccupancyOverlay::new();
    let from = l.home_desks[0];
    let pantry = l
        .waypoints
        .iter()
        .find(|w| w.kind == crate::layout::WaypointKind::Pantry)
        .expect("pantry wp")
        .pos;
    let path = find_path(&l.walkable, &overlay, None, from, pantry).expect("path");
    assert!(path.len() >= 3, "expected routed path, got {path:?}");
}

#[test]
fn vertical_wall_is_impassable_except_through_the_door() {
    // Regression: a vertical (N-S) room divider blocks its full
    // `WALL_THICK_V` (4px) footprint, but 4px at a bad 4-alignment still
    // splits into two coarse cells at exactly the 8/16 threshold — both
    // stay "walkable" and A* threads STRAIGHT THROUGH. The X-only
    // `WALL_ROUTING_MARGIN_X` widens the stamp to 6px so a full cell column
    // drops under the threshold; this test pins that the wall is a real
    // barrier (crossable only through the door gap).
    let l = make_layout();
    let overlay = OccupancyOverlay::new();
    let WallSegment { start, end } = l
        .room_walls
        .iter()
        .copied()
        .find(|w| w.start.x == w.end.x)
        .expect("layout has a vertical wall");
    let wall_x = start.x;
    // A y inside the wall body, near its top — clear of the mid door gap.
    let y = start.y.min(end.y) + 3;
    let from = Point {
        x: wall_x.saturating_sub(12),
        y,
    };
    let to = Point { x: wall_x + 12, y };
    let path = find_path(&l.walkable, &overlay, None, from, to)
        .expect("rooms stay connected through the door gap");
    let direct = crate::pose::octile_distance(from, to);
    let routed: u32 = path
        .windows(2)
        .map(|w| crate::pose::octile_distance(w[0], w[1]))
        .sum();
    // A straight crossing is ~24px; detouring through the mid door is far
    // longer. A passable wall would yield a near-straight path (≈ direct).
    assert!(
        routed > direct * 2,
        "expected a detour around the wall (routed {routed} vs direct {direct}); \
         a near-direct path means A* crossed the wall. path={path:?}"
    );
}

#[test]
fn every_wander_waypoint_is_routable_on_the_coarse_grid() {
    // Teleport guard (#22): a waypoint A* can't reach on the coarse 4×4 grid
    // makes an idle agent SNAP/teleport there — find_path returns None and
    // route() falls back to a straight [from,to] line. The core connectivity
    // sweep only checks full-PIXEL BFS from the door; this checks COARSE-grid
    // reachability of EVERY emitted wander destination (meeting seats, pantry,
    // couch, AND the pod-aisle decor — phone booth / standing desk / vending /
    // printer, which also pins the INTER_POD_AISLE_X width: narrow the aisle
    // and the decor disconnects the grid here). Across seeds × sizes incl. the
    // 96×70 floor. It caught the narrow-meeting-room teleport (now gated).
    use crate::layout::TEST_DEFAULT_DESKS;
    let overlay = OccupancyOverlay::new();
    let sizes = [
        (96u16, 70u16),
        (128, 80),
        (160, 120),
        (192, 160),
        (240, 160),
    ];
    for (w, h) in sizes {
        for seed in 0..5u64 {
            let Some(l) = Layout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), seed) else {
                continue;
            };
            let Some(origin) = l.door_threshold else {
                continue;
            };
            for wp in &l.waypoints {
                assert!(
                    find_path(&l.walkable, &overlay, None, origin, wp.pos).is_some(),
                    "seed {seed} {w}x{h}: {:?} at ({},{}) is unreachable on the coarse \
                     routing grid — an idle agent sent there would teleport",
                    wp.kind,
                    wp.pos.x,
                    wp.pos.y
                );
            }
        }
    }
}

#[test]
fn every_approach_point_is_routable_from_its_home_desk() {
    // STRONGER routability guard for the approach model: the cell A* actually
    // targets — `approach_point` on a reachable allowed side — must be
    // find_path-routable from the agent's OWN home desk, for EVERY
    // desk × waypoint × size × seed. The test above uses the DOOR origin + the
    // blocked furniture CENTER, so it can pass while a specific desk's chosen
    // approach side is unroutable (a teleport). `reaches ⇒ routable` (the
    // ReachSet contract) makes this hold. When NO allowed+reachable side
    // exists, approach_point returns the `wp.pos` sentinel (NO fallback — the
    // wander skips the furniture), which isn't a real destination, so we
    // exclude it below.
    use crate::layout::approach_point;
    use crate::layout::TEST_DEFAULT_DESKS;
    let overlay = OccupancyOverlay::new();
    for (w, h) in [
        (96u16, 70u16),
        (128, 80),
        (160, 120),
        (192, 160),
        (240, 160),
    ] {
        for seed in 0..5u64 {
            let Some(l) = Layout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), seed) else {
                continue;
            };
            for &desk in &l.home_desks {
                for wp in &l.waypoints {
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
                        continue; // "no valid approach" sentinel — skipped, not routed to
                    }
                    assert!(
                        find_path(&l.walkable, &overlay, None, desk, a).is_some(),
                        "{w}x{h} seed {seed}: {:?} approach_point {a:?} unroutable from \
                         desk {desk:?} — the agent would teleport",
                        wp.kind,
                    );
                }
            }
        }
    }
}

#[test]
fn reachset_never_claims_an_unroutable_cell() {
    // The core ReachSet must never be a FALSE POSITIVE vs the real router:
    // every cell it reports reachable MUST be find_path-routable from the
    // door. (Conservative false negatives at coarse boundaries are fine —
    // approach_point simply won't pick those.) Pins the core↔router
    // coarsening agreement on REAL layouts, not just synthetic masks, so
    // approach_point can never select an unroutable approach side.
    use crate::layout::TEST_DEFAULT_DESKS;
    let overlay = OccupancyOverlay::new();
    for (w, h) in [(160u16, 120u16), (200, 80), (96, 70)] {
        for seed in 0..3u64 {
            let Some(l) = Layout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), seed) else {
                continue;
            };
            let Some(door) = l.door_threshold else {
                continue;
            };
            let mut y = 0;
            while y < l.buf_h {
                let mut x = 0;
                while x < l.buf_w {
                    let p = Point { x, y };
                    if l.reachable.reaches(p) {
                        assert!(
                            find_path(&l.walkable, &overlay, None, door, p).is_some(),
                            "{w}x{h} seed {seed}: ReachSet claims {p:?} reachable but \
                             find_path can't route there from the door {door:?}",
                        );
                    }
                    x += 8;
                }
                y += 8;
            }
        }
    }
}

#[test]
fn router_caches_until_overlay_changes() {
    let l = make_layout();
    let mut router = AStarRouter::new();
    let mut overlay = OccupancyOverlay::new();
    let from = Point { x: 30, y: 80 };
    let to = Point { x: 30, y: 120 };
    let _ = router.route(&l.walkable, &overlay, from, to);
    assert_eq!(router.len(), 1);
    let _ = router.route(&l.walkable, &overlay, from, to);
    assert_eq!(router.len(), 1, "should hit cache");

    // Push an occupancy rect — cache should drop.
    overlay.add(100, 100, 8, 8);
    let _ = router.route(&l.walkable, &overlay, from, to);
    assert_eq!(router.len(), 1, "cache rebuilt after overlay change");
}

#[test]
fn path_cache_is_bounded_and_still_routes_after_the_clear() {
    // Regression: aimless wander destinations + live-position snap-back/
    // exit origins mint ever-new (from, to) keys, so without the cap the
    // cache (and the per-overlay retain scan over it) grew without bound
    // in an always-on office.
    let mask = WalkableMask::new_open(400, 400);
    let overlay = OccupancyOverlay::new();
    let mut router = AStarRouter::new();
    // On a cell-center (x % 4 == 2) so same-row routes collapse to the
    // straight 2-point polyline (see routes_around_dynamic_obstacle).
    let from = Point { x: 10, y: 50 };
    // Strictly more distinct (from, to) pairs than the cap holds, so the
    // overflow clear provably fires at least once.
    let distinct_routes = PATH_CACHE_CAP + 100;
    for i in 0..distinct_routes {
        let to = Point {
            x: (8 + (i % 90) * 4) as u16,
            y: (8 + (i / 90) * 4) as u16,
        };
        let _ = router.route(&mask, &overlay, from, to);
        assert!(
            router.len() <= PATH_CACHE_CAP,
            "cache must stay bounded: {} entries after {} distinct routes",
            router.len(),
            i + 1
        );
    }
    // Routing stays correct after the overflow clear: a same-row route on
    // an open mask yields the straight [from, to], recomputed
    // bit-identically.
    let to = Point { x: 90, y: 50 };
    assert_eq!(
        router.route(&mask, &overlay, from, to),
        vec![from, to],
        "post-clear routing must still return correct paths"
    );
}

#[test]
fn routes_around_dynamic_obstacle() {
    // Synthetic open mask isolates the routing behaviour from the
    // production layout's obstacle clutter.
    let mask = pixtuoid_core::walkable::WalkableMask::new_open(100, 100);
    let mut overlay = OccupancyOverlay::new();
    let from = Point { x: 10, y: 50 };
    let to = Point { x: 90, y: 50 };
    let baseline = find_path(&mask, &overlay, None, from, to).expect("baseline");
    assert_eq!(baseline.len(), 2, "open mask should yield straight line");

    overlay.add(40, 40, 20, 20);
    let detour = find_path(&mask, &overlay, None, from, to).expect("detour");
    assert!(
        detour.len() > 2,
        "detour must add at least one corner around the dynamic block, got {detour:?}"
    );
}

#[test]
fn path_clear_under_empty_overlay_always_true() {
    let overlay = OccupancyOverlay::new();
    let path = vec![Point { x: 0, y: 0 }, Point { x: 100, y: 100 }];
    assert!(path_clear_under(&path, &overlay));
}

#[test]
fn path_clear_under_blocked_returns_false() {
    let mut overlay = OccupancyOverlay::new();
    overlay.add(50, 50, 10, 10);
    let path = vec![Point { x: 0, y: 0 }, Point { x: 100, y: 100 }];
    assert!(!path_clear_under(&path, &overlay));
}

#[test]
fn path_clear_under_misses_obstacle_returns_true() {
    let mut overlay = OccupancyOverlay::new();
    overlay.add(50, 50, 10, 10);
    let path = vec![Point { x: 0, y: 0 }, Point { x: 40, y: 0 }];
    assert!(path_clear_under(&path, &overlay));
}

#[test]
fn snap_to_walkable_returns_cell_when_already_walkable() {
    let l = make_layout();
    let overlay = OccupancyOverlay::new();
    let corridor = l.corridor.unwrap();
    let cell_w = l.buf_w / 4;
    let cell_h = l.buf_h / 4;
    let cx = (corridor.x + corridor.width / 2) / 4;
    let cy = (corridor.y + corridor.height / 2) / 4;
    let result = snap(
        &l.walkable,
        &overlay,
        (cx, cy),
        cell_w,
        cell_h,
        MAX_SNAP_RADIUS,
    );
    assert_eq!(result, Some((cx, cy)));
}

#[test]
fn snap_to_walkable_finds_nearby_cell_when_blocked() {
    let l = make_layout();
    let cell_w = l.buf_w / 4;
    let cell_h = l.buf_h / 4;
    let wall_cell_y = l.top_margin / CELL_SIZE;
    let result = snap(
        &l.walkable,
        &OccupancyOverlay::new(),
        (0, wall_cell_y),
        cell_w,
        cell_h,
        MAX_SNAP_RADIUS,
    );
    assert!(result.is_some(), "should snap to a nearby walkable cell");
}

#[test]
fn heuristic_zero_for_same_cell() {
    assert_eq!(heuristic((5, 5), (5, 5)), 0);
}

#[test]
fn heuristic_straight_horizontal() {
    assert_eq!(heuristic((0, 0), (3, 0)), 30);
}

#[test]
fn heuristic_diagonal_uses_octile() {
    let h = heuristic((0, 0), (2, 2));
    assert_eq!(h, 28);
}

#[test]
fn cell_of_maps_pixel_to_cell() {
    assert_eq!(cell_of(Point { x: 0, y: 0 }), (0, 0));
    assert_eq!(cell_of(Point { x: 7, y: 11 }), (1, 2));
    assert_eq!(cell_of(Point { x: 4, y: 4 }), (1, 1));
}

#[test]
fn cell_center_is_midpoint_of_cell() {
    let c = cell_center(0, 0);
    assert_eq!(c, Point { x: 2, y: 2 });
    let c = cell_center(3, 5);
    assert_eq!(c, Point { x: 14, y: 22 });
}

#[test]
fn cell_in_zone_false_when_none() {
    assert!(!cell_in_zone(None, 5, 5));
}

#[test]
fn cell_in_zone_true_when_inside() {
    let zone = Bounds {
        x: 0,
        y: 0,
        width: 40,
        height: 40,
    };
    assert!(cell_in_zone(Some(zone), 2, 2));
}

#[test]
fn cell_in_zone_false_when_outside() {
    let zone = Bounds {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    };
    assert!(!cell_in_zone(Some(zone), 20, 20));
}

// A cell center landing EXACTLY on the exclusive far edge (x+width / y+height)
// is OUTSIDE — the bound is a strict `<`. The zone edge is derived from
// `cell_center` so the alignment holds regardless of CELL_SIZE. Without this,
// a `<`->`<=` mutation is invisible (no on-edge cell is ever tested).
#[test]
fn cell_in_zone_false_on_exclusive_edges() {
    let right_edge = cell_center(2, 1).x;
    let zone_x = Bounds {
        x: 0,
        y: 0,
        width: right_edge,
        height: 40,
    };
    assert!(!cell_in_zone(Some(zone_x), 2, 1));

    let bottom_edge = cell_center(1, 2).y;
    let zone_y = Bounds {
        x: 0,
        y: 0,
        width: 40,
        height: bottom_edge,
    };
    assert!(!cell_in_zone(Some(zone_y), 1, 2));
}

// Inside on ONE axis but outside on the other is OUTSIDE — the four bounds
// are AND-joined. The both-axes-outside test above leaves an `&&`->`||`
// mutation on the middle joins invisible (F||F is still F); a single-axis
// miss (T on one pair, F on the other) is what makes the conjunction observable.
#[test]
fn cell_in_zone_false_when_outside_on_one_axis_only() {
    let zone = Bounds {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    };
    assert!(!cell_in_zone(Some(zone), 20, 1)); // outside x, inside y
    assert!(!cell_in_zone(Some(zone), 1, 20)); // inside x, outside y
}

// The complement of the exclusive-edge test: a cell center landing EXACTLY
// on the INCLUSIVE near edge (x / y) is INSIDE — the lower bound is `>=`.
// Without this a `>=`->`>` mutation on either lower bound survives (the
// mirror of the `<`->`<=` gap above; sibling-set-spans-axes).
#[test]
fn cell_in_zone_true_on_inclusive_lower_edges() {
    let near = cell_center(1, 1);
    let zone = Bounds {
        x: near.x,
        y: near.y,
        width: 40,
        height: 40,
    };
    assert!(cell_in_zone(Some(zone), 1, 1));
}

#[test]
fn cell_walkable_on_open_mask() {
    let mask = WalkableMask::new_open(100, 100);
    let overlay = OccupancyOverlay::new();
    assert!(cell_walkable(&mask, &overlay, 5, 5));
}

#[test]
fn cell_walkable_false_when_blocked_by_overlay() {
    let mask = WalkableMask::new_open(100, 100);
    let mut overlay = OccupancyOverlay::new();
    overlay.add(20, 20, CELL_SIZE, CELL_SIZE);
    assert!(!cell_walkable(&mask, &overlay, 5, 5));
}

#[test]
fn find_path_returns_none_when_target_completely_surrounded() {
    // 200×200 mask so the wall around (100,100) doesn't saturate to
    // origin and accidentally cover from=(4,4). This ensures the coarse-cell
    // `snap` succeeds on `from` but fails on the goal.
    let mask = WalkableMask::new_open(200, 200);
    let mut overlay = OccupancyOverlay::new();
    let target = Point { x: 100, y: 100 };
    let wall_size = (MAX_SNAP_RADIUS + 1) * CELL_SIZE * 2;
    let wall_origin = 100u16 - wall_size / 2;
    overlay.add(wall_origin, wall_origin, wall_size, wall_size);

    let from = Point { x: 4, y: 4 };
    let result = find_path(&mask, &overlay, None, from, target);
    assert!(
        result.is_none(),
        "completely surrounded target should return None, got {result:?}"
    );
}

#[test]
fn transient_no_path_fallback_is_not_served_from_the_cache() {
    // A wall with ONE gap; an overlay blocker transiently closes the gap →
    // find_path None → route() returns the straight [from, to] fallback.
    // When the blocker leaves, the SAME (from, to) key must recover the
    // real detour: `path_clear_under` checks only the OVERLAY (never the
    // static mask), so a cached wall-crossing fallback would survive every
    // retain() and the agent would walk through the wall on every future
    // leg (see the walk-path freeze doc: 2-point legs deliberately stay
    // unfrozen precisely so "the next frame recovers the real route").
    let mut mask = WalkableMask::new_open(80, 48);
    // Blocked strip x ∈ [36, 44) for y ∈ [0, 32); gap open at y ∈ [32, 48).
    mask.mark_blocked(36, 0, 8, 32, 0);
    let from = Point { x: 10, y: 10 };
    let to = Point { x: 70, y: 10 };

    // Sanity: with the gap open the real route is a cornered detour.
    let open = find_path(&mask, &OccupancyOverlay::new(), None, from, to).expect("gap routes");
    assert!(
        open.len() > 2,
        "expected a detour via the gap, got {open:?}"
    );

    let mut router = AStarRouter::new();
    let mut blocked = OccupancyOverlay::new();
    blocked.add(36, 32, 8, 16); // close the gap → no path at all
    assert_eq!(
        router.route(&mask, &blocked, from, to),
        vec![from, to],
        "with the gap closed the router falls back to the straight line"
    );

    // Blocker leaves (overlay signature changes back). The poisoned key
    // must re-route for real instead of serving the cached fallback.
    let recovered = router.route(&mask, &OccupancyOverlay::new(), from, to);
    assert!(
        recovered.len() > 2,
        "the transient no-path fallback must not be cached; got {recovered:?}"
    );
}

#[test]
fn router_falls_back_to_straight_line_when_path_is_none() {
    let mask = WalkableMask::new_open(200, 200);
    let mut overlay = OccupancyOverlay::new();
    let from = Point { x: 4, y: 4 };
    let to = Point { x: 100, y: 100 };
    let wall_size = (MAX_SNAP_RADIUS + 1) * CELL_SIZE * 2;
    let wall_origin = 100u16 - wall_size / 2;
    overlay.add(wall_origin, wall_origin, wall_size, wall_size);

    let mut router = AStarRouter::new();
    let path = router.route(&mask, &overlay, from, to);
    assert_eq!(
        path,
        vec![from, to],
        "router should fall back to [from, to] when find_path returns None"
    );
}

#[test]
fn snap_point_to_walkable_returns_walkable_cell() {
    let l = make_layout();
    // A point inside a desk footprint (blocked, with obstacle pad).
    let desk = l.home_desks[0];
    let blocked_p = Point {
        x: desk.x + 4,
        y: desk.y + 2,
    };
    let snapped =
        snap_point_to_walkable(&l.walkable, blocked_p).expect("blocked desk should snap nearby");
    assert!(
        l.walkable.is_walkable(snapped.x, snapped.y),
        "snapped point ({},{}) must be walkable",
        snapped.x,
        snapped.y
    );
    // An already-open corridor point must also resolve to a walkable cell.
    let c = l.corridor.unwrap();
    let open_p = Point {
        x: c.x + c.width / 2,
        y: c.y + c.height / 2,
    };
    let open = snap_point_to_walkable(&l.walkable, open_p).expect("corridor center snaps");
    assert!(
        l.walkable.is_walkable(open.x, open.y),
        "open-floor snap walkable"
    );
}

// ── Router accessor / trait-default coverage ───────────────────────────

/// A Router that does NOT override `set_preferred_zone`, so calling it hits
/// the trait DEFAULT no-op body (pathfind.rs:63-65).
struct NoZoneRouter;
impl Router for NoZoneRouter {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point> {
        vec![from, to]
    }
    fn invalidate(&mut self) {}
    // set_preferred_zone intentionally NOT overridden.
}

#[test]
fn router_default_set_preferred_zone_is_a_noop() {
    let mut r = NoZoneRouter;
    // The default impl just drops the argument — calling it must not panic
    // and must leave routing unchanged.
    r.set_preferred_zone(Some(Bounds {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    }));
    r.set_preferred_zone(None);
    assert_eq!(
        r.route(
            &WalkableMask::new_open(40, 40),
            &OccupancyOverlay::new(),
            Point { x: 0, y: 0 },
            Point { x: 10, y: 0 },
        ),
        vec![Point { x: 0, y: 0 }, Point { x: 10, y: 0 }]
    );
}

#[test]
fn astar_is_empty_then_invalidate_clears_cache() {
    let mask = WalkableMask::new_open(80, 80);
    let overlay = OccupancyOverlay::new();
    let mut router = AStarRouter::new();
    // Fresh router has an empty cache.
    assert!(router.is_empty(), "fresh router cache must be empty");
    assert_eq!(router.len(), 0);

    // One route populates the cache.
    let _ = router.route(
        &mask,
        &overlay,
        Point { x: 4, y: 4 },
        Point { x: 60, y: 60 },
    );
    assert!(!router.is_empty(), "cache must be non-empty after a route");
    assert_ne!(router.len(), 0);

    // invalidate() drops every cached path.
    router.invalidate();
    assert!(router.is_empty(), "invalidate must clear the cache");
    assert_eq!(router.len(), 0);
}

// ── Degenerate sub-CELL_SIZE grid ──────────────────────────────────────

#[test]
fn degenerate_grid_returns_fallbacks() {
    // A 3×3 mask: 3 / CELL_SIZE(4) == 0 on both axes ⇒ grid_dims None.
    let mask = WalkableMask::new_open(3, 3);
    let overlay = OccupancyOverlay::new();
    let a = Point { x: 0, y: 0 };
    let b = Point { x: 2, y: 2 };
    // find_path hits the grid_dims-None early return ⇒ straight [a,b].
    assert_eq!(
        find_path(&mask, &overlay, None, a, b),
        Some(vec![a, b]),
        "degenerate grid must fall back to the straight [from,to]"
    );
    // point_in_walkable_cell hits its grid_dims-None branch ⇒ false.
    assert!(
        !point_in_walkable_cell(&mask, a),
        "degenerate grid: no point is in a walkable cell"
    );
}

#[test]
fn snap_to_walkable_skips_out_of_bounds_corner_neighbours() {
    // Block the bottom-right CORNER cell so the expanding ring at r>=1 pokes
    // PAST the grid's far edge (nx>=cell_w / ny>=cell_h), forcing the
    // out-of-range `continue` (pathfind.rs:274) before it lands on an
    // interior walkable cell. Must still return Some.
    let mut mask = WalkableMask::new_open(40, 40); // 10×10 cells
    let overlay = OccupancyOverlay::new();
    let (cell_w, cell_h) = grid_dims(&mask).expect("non-degenerate");
    // Block the corner cell (cell_w-1, cell_h-1) at the pixel level.
    let corner_px = ((cell_w - 1) * CELL_SIZE, (cell_h - 1) * CELL_SIZE);
    mask.mark_blocked(corner_px.0, corner_px.1, CELL_SIZE, CELL_SIZE, 0);

    let result = snap(
        &mask,
        &overlay,
        (cell_w - 1, cell_h - 1),
        cell_w,
        cell_h,
        MAX_SNAP_RADIUS,
    );
    assert!(
        result.is_some(),
        "snap from the corner must still find an interior walkable cell"
    );
}

#[test]
fn find_path_none_when_two_regions_split_by_a_full_wall() {
    // Two open regions split by a full-height blocked strip with NO door gap.
    // `from`/`to` are each in open cells (snap succeeds) but the search
    // exhausts the open set without reaching the goal ⇒ None AFTER the loop
    // (pathfind.rs:356) — distinct from the goal-snap-fails None at 302.
    let mut mask = WalkableMask::new_open(80, 40);
    let overlay = OccupancyOverlay::new();
    // Block x ∈ [36, 44) across the full height: 2 fully-blocked cell
    // columns (cells 9,10) — impassable to the coarse diagonal stepper.
    mask.mark_blocked(36, 0, 8, 40, 0);

    let from = Point { x: 10, y: 20 }; // left region
    let to = Point { x: 70, y: 20 }; // right region
                                     // Sanity: both endpoints are in walkable cells, so snapping succeeds and
                                     // the A* loop actually runs (start != goal).
    assert!(point_in_walkable_cell(&mask, from));
    assert!(point_in_walkable_cell(&mask, to));

    assert!(
        find_path(&mask, &overlay, None, from, to).is_none(),
        "a wall with no gap must leave the two regions unconnected (loop exhausts → None)"
    );
}
