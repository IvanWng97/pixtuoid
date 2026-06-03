//! Pathfinding façade — `Router` trait + `AStarRouter` impl.
//!
//! `Router` is the abstraction the renderer codes against: give it a static
//! `WalkableMask` and a per-frame `OccupancyOverlay`, ask for a polyline
//! from A to B, get back the route. The trait stays small so future impls
//! (Theta*, HPA*, navmesh) can drop in without touching `pose.rs` or
//! `renderer.rs`.
//!
//! `AStarRouter` is the concrete impl: A* on a coarsened 4×4 cell grid
//! with a permissive cell-walkability threshold (≥12/16 px walkable).
//! Memoizes results in a per-(from, to) cache; auto-invalidates when
//! the overlay signature changes so per-frame agent movement still routes
//! around live agents.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};

use crate::tui::layout::{Bounds, Point};

/// Cell size in pixels. Smaller = more accurate paths, more work per query.
/// 4 px gives a ~40×60 grid on a typical 160×240 buffer — A* finishes in
/// well under 1 ms uncached.
pub const CELL_SIZE: u16 = 4;
/// Cell-walkable threshold (out of CELL_SIZE^2 = 16 pixels). At 8 (50%) the
/// coarsened grid can squeeze through 2-pixel-wide corridors, which is
/// what the meeting-room interior needs after furniture obstacle padding.
/// Tighter (e.g. 12 = 75%) made the meeting room unreachable; looser (e.g.
/// 4 = 25%) lets paths graze furniture edges. 50% is the sweet spot.
const CELL_WALKABLE_MIN: u16 = 8;

// The core-side `ReachSet` MUST coarsen identically to this router, or
// "reachable" in `approach_point` would diverge from what A* actually routes.
// Locked at compile time so a CELL_SIZE / threshold edit can't silently desync.
const _: () = assert!(CELL_SIZE == pixtuoid_core::layout::REACH_CELL_SIZE);
const _: () = assert!(CELL_WALKABLE_MIN == pixtuoid_core::layout::REACH_CELL_WALKABLE_MIN);

/// Abstract pathfinder — implementations route from `from` to `to` over
/// the supplied mask + overlay, returning a polyline (first = `from`,
/// last = `to`, intermediate = corners). Renderer + pose layer use this
/// trait so the algorithm can be swapped without touching them.
pub trait Router {
    /// Compute or look up the route. The returned slice is owned by the
    /// router (cache-backed); copy if you need to outlive the next call.
    fn route(
        &mut self,
        mask: &WalkableMask,
        overlay: &OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point>;

    /// Drop any cached state — call when the static mask is replaced
    /// (terminal resize, layout shape change).
    fn invalidate(&mut self);

    /// Optional: bias the cost function toward a preferred zone (e.g. the
    /// office corridor). Cells inside `zone` get a small cost discount so
    /// paths naturally hug the hallway instead of cutting diagonally
    /// across the cubicle floor. Default impl is a no-op so a Router
    /// that doesn't care about zones can skip it.
    fn set_preferred_zone(&mut self, zone: Option<Bounds>) {
        let _ = zone;
    }
}

/// A* router with internal path cache. Cache invalidates on overlay
/// signature change so per-frame occupancy movement (live agents) still
/// produces correct routes.
#[derive(Debug, Default, Clone)]
pub struct AStarRouter {
    paths: HashMap<(Point, Point), Vec<Point>>,
    last_overlay_sig: u64,
    /// Cells inside this zone get a cost discount during A*. When `None`,
    /// every cell has uniform cost. Changing this drops the cached paths
    /// (different zone = different optimal route).
    preferred_zone: Option<Bounds>,
}

impl AStarRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

impl Router for AStarRouter {
    fn route(
        &mut self,
        mask: &WalkableMask,
        overlay: &OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point> {
        let overlay_sig = overlay.signature();
        // Per-path validity (replaces the old global cache wipe): when the
        // overlay changes, check each cached path to see if it now crosses
        // an obstacle. Only invalidate entries that actually conflict —
        // paths in unaffected corridors stay cached.
        if overlay_sig != self.last_overlay_sig {
            self.paths.retain(|_, path| path_clear_under(path, overlay));
            self.last_overlay_sig = overlay_sig;
        }
        if let Some(p) = self.paths.get(&(from, to)) {
            return p.clone();
        }
        let path = find_path(mask, overlay, self.preferred_zone, from, to)
            .unwrap_or_else(|| vec![from, to]);
        self.paths.insert((from, to), path.clone());
        path
    }

    fn invalidate(&mut self) {
        self.paths.clear();
    }

    fn set_preferred_zone(&mut self, zone: Option<Bounds>) {
        // Different zone produces different optimal paths — invalidate the
        // cache. Cheap to do unconditionally; the layout's corridor only
        // changes on terminal resize so this fires rarely.
        if self.preferred_zone != zone {
            self.paths.clear();
            self.preferred_zone = zone;
        }
    }
}

/// Is `path` still walkable under the current `overlay`? Samples each
/// segment at a small stride and checks whether any sample falls inside
/// an overlay rect. Faster than re-running A* per path; tolerates a tiny
/// overshoot (a 1-px clip into an obstacle won't invalidate, but a
/// real intersection at any corner will).
fn path_clear_under(path: &[Point], overlay: &OccupancyOverlay) -> bool {
    if overlay.is_empty() {
        return true;
    }
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let dx = b.x as i32 - a.x as i32;
        let dy = b.y as i32 - a.y as i32;
        let steps = dx.abs().max(dy.abs()).max(1) / 4;
        let n = steps.max(2);
        for i in 0..=n {
            let x = (a.x as i32 + dx * i / n).max(0) as u16;
            let y = (a.y as i32 + dy * i / n).max(0) as u16;
            if overlay.blocks(x, y) {
                return false;
            }
        }
    }
    true
}

#[derive(Eq, PartialEq)]
struct Node {
    f: u32,
    g: u32,
    cell: (u16, u16),
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.cmp(&self.f).then(other.g.cmp(&self.g))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const NEIGHBORS_8: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

fn heuristic(a: (u16, u16), b: (u16, u16)) -> u32 {
    let dx = (a.0 as i32 - b.0 as i32).unsigned_abs();
    let dy = (a.1 as i32 - b.1 as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}

/// Is the center of cell `(cx, cy)` inside `zone`? Used by the preferred-
/// zone discount: cells whose center lands in the corridor get a cheaper
/// step cost so A* hugs the hallway.
fn cell_in_zone(zone: Option<Bounds>, cx: u16, cy: u16) -> bool {
    let Some(z) = zone else {
        return false;
    };
    let cp = cell_center(cx, cy);
    cp.x >= z.x && cp.x < z.x + z.width && cp.y >= z.y && cp.y < z.y + z.height
}

fn cell_walkable(mask: &WalkableMask, overlay: &OccupancyOverlay, cx: u16, cy: u16) -> bool {
    let px_start = cx.saturating_mul(CELL_SIZE);
    let py_start = cy.saturating_mul(CELL_SIZE);
    let mut walk_count = 0u16;
    for dy in 0..CELL_SIZE {
        for dx in 0..CELL_SIZE {
            let px = px_start + dx;
            let py = py_start + dy;
            if mask.is_walkable(px, py) && !overlay.blocks(px, py) {
                walk_count += 1;
            }
        }
    }
    walk_count >= CELL_WALKABLE_MIN
}

fn cell_of(p: Point) -> (u16, u16) {
    (p.x / CELL_SIZE, p.y / CELL_SIZE)
}

fn cell_center(cx: u16, cy: u16) -> Point {
    Point {
        x: cx * CELL_SIZE + CELL_SIZE / 2,
        y: cy * CELL_SIZE + CELL_SIZE / 2,
    }
}

/// Coarse-grid dimensions (`mask` pixel size ÷ `CELL_SIZE`), or `None` when
/// either axis is 0 — a degenerate grid the A* loop can't index. Callers pick
/// their own degenerate return (straight `[from,to]`, `None`, `false`).
fn grid_dims(mask: &WalkableMask) -> Option<(u16, u16)> {
    let cell_w = mask.width / CELL_SIZE;
    let cell_h = mask.height / CELL_SIZE;
    if cell_w == 0 || cell_h == 0 {
        return None;
    }
    Some((cell_w, cell_h))
}

const MAX_SNAP_RADIUS: u16 = 12;

fn snap_to_walkable(
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    cell: (u16, u16),
    cell_w: u16,
    cell_h: u16,
) -> Option<(u16, u16)> {
    if cell.0 < cell_w && cell.1 < cell_h && cell_walkable(mask, overlay, cell.0, cell.1) {
        return Some(cell);
    }
    for r in 1..=MAX_SNAP_RADIUS {
        let r_i = r as i32;
        for dy in -r_i..=r_i {
            for dx in -r_i..=r_i {
                if dx.abs() != r_i && dy.abs() != r_i {
                    continue;
                }
                let nx = cell.0 as i32 + dx;
                let ny = cell.1 as i32 + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let (nx, ny) = (nx as u16, ny as u16);
                if nx >= cell_w || ny >= cell_h {
                    continue;
                }
                if cell_walkable(mask, overlay, nx, ny) {
                    return Some((nx, ny));
                }
            }
        }
    }
    None
}

/// Run A* on the layout's walkability mask + per-frame occupancy. When
/// `preferred` is `Some(rect)`, cells whose center falls inside the rect
/// get a 30% step-cost discount — paths naturally hug that zone (e.g.
/// the office corridor) when an off-zone diagonal cut would otherwise
/// be slightly shorter.
pub fn find_path(
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    preferred: Option<Bounds>,
    from: Point,
    to: Point,
) -> Option<Vec<Point>> {
    let Some((cell_w, cell_h)) = grid_dims(mask) else {
        return Some(vec![from, to]);
    };

    let start = snap_to_walkable(mask, overlay, cell_of(from), cell_w, cell_h)?;
    let goal = snap_to_walkable(mask, overlay, cell_of(to), cell_w, cell_h)?;

    if start == goal {
        return Some(vec![from, to]);
    }

    let mut open: BinaryHeap<Node> = BinaryHeap::new();
    let mut came_from: HashMap<(u16, u16), (u16, u16)> = HashMap::new();
    let mut g_score: HashMap<(u16, u16), u32> = HashMap::new();
    g_score.insert(start, 0);
    open.push(Node {
        f: heuristic(start, goal),
        g: 0,
        cell: start,
    });

    while let Some(current) = open.pop() {
        if current.cell == goal {
            return Some(reconstruct(&came_from, goal, from, to));
        }
        if current.g > *g_score.get(&current.cell).unwrap_or(&u32::MAX) {
            continue;
        }
        for (dx, dy) in NEIGHBORS_8.iter() {
            let nx = current.cell.0 as i32 + dx;
            let ny = current.cell.1 as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as u16, ny as u16);
            if nx >= cell_w || ny >= cell_h {
                continue;
            }
            if !cell_walkable(mask, overlay, nx, ny) {
                continue;
            }
            let base_step = if dx.abs() + dy.abs() == 2 { 14 } else { 10 };
            let step = if cell_in_zone(preferred, nx, ny) {
                base_step * 7 / 10
            } else {
                base_step
            };
            let tentative = current.g + step;
            if tentative < *g_score.get(&(nx, ny)).unwrap_or(&u32::MAX) {
                came_from.insert((nx, ny), current.cell);
                g_score.insert((nx, ny), tentative);
                open.push(Node {
                    f: tentative + heuristic((nx, ny), goal),
                    g: tentative,
                    cell: (nx, ny),
                });
            }
        }
    }
    None
}

/// Is the coarse routing cell containing `p` walkable (the SAME predicate A*
/// expands on — ≥`CELL_WALKABLE_MIN`/16 px open)? This is the granularity the
/// router actually guarantees: a position can fail a per-pixel `is_walkable`
/// (it's in the obstacle PAD band, or a transient diagonal corner-graze) yet
/// still be in a walkable routing cell — exactly like every agent sprite, which
/// rides the same coarse grid. Test/diagnostic helper.
pub fn point_in_walkable_cell(mask: &WalkableMask, p: Point) -> bool {
    let Some((cell_w, cell_h)) = grid_dims(mask) else {
        return false;
    };
    let (cx, cy) = cell_of(p);
    cx < cell_w && cy < cell_h && cell_walkable(mask, &OccupancyOverlay::new(), cx, cy)
}

/// Snap a pixel-space `Point` to the nearest walkable coarse-cell *center* on
/// the STATIC mask (no dynamic overlay). Returns `None` only when the grid is
/// degenerate or no walkable cell exists within `MAX_SNAP_RADIUS`.
///
/// This is the pet's rest/leg anchor: pass a raw furniture-adjacent spot to get
/// the nearest floor pixel it can actually stand on. Distinct from `find_path`'s
/// internal snapping, whose `reconstruct` overwrites the polyline endpoints with
/// the RAW `from`/`to` — so callers that need a guaranteed-walkable endpoint must
/// re-anchor with this.
pub fn snap_point_to_walkable(mask: &WalkableMask, p: Point) -> Option<Point> {
    let (cell_w, cell_h) = grid_dims(mask)?;
    let empty = OccupancyOverlay::new();
    let (cx, cy) = snap_to_walkable(mask, &empty, cell_of(p), cell_w, cell_h)?;
    Some(cell_center(cx, cy))
}

fn reconstruct(
    came_from: &HashMap<(u16, u16), (u16, u16)>,
    end: (u16, u16),
    from: Point,
    to: Point,
) -> Vec<Point> {
    let mut cells = vec![end];
    let mut cur = end;
    while let Some(&prev) = came_from.get(&cur) {
        cells.push(prev);
        cur = prev;
    }
    cells.reverse();
    let mut pts: Vec<Point> = cells.iter().map(|&(cx, cy)| cell_center(cx, cy)).collect();
    if pts.is_empty() {
        return vec![from, to];
    }
    pts[0] = from;
    let last = pts.len() - 1;
    pts[last] = to;
    simplify_polyline(pts)
}

fn simplify_polyline(pts: Vec<Point>) -> Vec<Point> {
    if pts.len() < 3 {
        return pts;
    }
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    out.push(pts[0]);
    for i in 1..pts.len() - 1 {
        // `out` is non-empty (pushed pts[0] above); index instead of unwrap.
        let prev = out[out.len() - 1];
        let here = pts[i];
        let next = pts[i + 1];
        let dx_in = here.x as i32 - prev.x as i32;
        let dy_in = here.y as i32 - prev.y as i32;
        let dx_out = next.x as i32 - here.x as i32;
        let dy_out = next.y as i32 - here.y as i32;
        if dx_in * dy_out != dy_in * dx_out {
            out.push(here);
        }
    }
    // `pts.len() >= 3` here (early-returned otherwise), so indexing is safe.
    out.push(pts[pts.len() - 1]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::layout::{Layout, WallSegment};

    fn make_layout() -> Layout {
        Layout::compute(160, 200, 4).expect("layout fits")
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

    #[test]
    fn routes_around_meeting_room_wall() {
        let l = make_layout();
        let overlay = OccupancyOverlay::new();
        let from = l.home_desks[0];
        let pantry = l
            .waypoints
            .iter()
            .find(|w| w.kind == crate::tui::layout::WaypointKind::Pantry)
            .expect("pantry wp")
            .pos;
        let path = find_path(&l.walkable, &overlay, None, from, pantry).expect("path");
        assert!(path.len() >= 3, "expected routed path, got {path:?}");
    }

    #[test]
    fn vertical_wall_is_impassable_except_through_the_door() {
        // Regression: a vertical (N-S) room divider has a 1px walkable
        // footprint (WALL_THICK_V, edge-on top-down rule). With NO clearance
        // pad that 1px strip is invisible to the coarse 4×4 router — only
        // 1 of a cell's 4 columns is blocked, so the cell keeps ≥12/16 px
        // walkable and stays "walkable", letting A* route STRAIGHT THROUGH
        // the wall. OBSTACLE_PAD_PX drives the wall's whole cell-column under
        // the threshold; this test pins that the wall is a real barrier.
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
        let direct = crate::tui::pose::octile_distance(from, to);
        let routed: u32 = path
            .windows(2)
            .map(|w| crate::tui::pose::octile_distance(w[0], w[1]))
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
        use crate::tui::layout::MAX_VISIBLE_DESKS;
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
                let Some(l) = Layout::compute_with_seed(w, h, MAX_VISIBLE_DESKS, seed) else {
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
        use crate::tui::layout::MAX_VISIBLE_DESKS;
        use pixtuoid_core::layout::approach_point;
        let overlay = OccupancyOverlay::new();
        for (w, h) in [
            (96u16, 70u16),
            (128, 80),
            (160, 120),
            (192, 160),
            (240, 160),
        ] {
            for seed in 0..5u64 {
                let Some(l) = Layout::compute_with_seed(w, h, MAX_VISIBLE_DESKS, seed) else {
                    continue;
                };
                for &desk in &l.home_desks {
                    for wp in &l.waypoints {
                        let a = approach_point(
                            wp.kind.furniture(),
                            wp.pos,
                            wp.facing,
                            l.pantry_counter_size,
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
        use crate::tui::layout::MAX_VISIBLE_DESKS;
        let overlay = OccupancyOverlay::new();
        for (w, h) in [(160u16, 120u16), (200, 80), (96, 70)] {
            for seed in 0..3u64 {
                let Some(l) = Layout::compute_with_seed(w, h, MAX_VISIBLE_DESKS, seed) else {
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
        let result = snap_to_walkable(&l.walkable, &overlay, (cx, cy), cell_w, cell_h);
        assert_eq!(result, Some((cx, cy)));
    }

    #[test]
    fn snap_to_walkable_finds_nearby_cell_when_blocked() {
        let l = make_layout();
        let cell_w = l.buf_w / 4;
        let cell_h = l.buf_h / 4;
        let wall_cell_y = l.top_margin / CELL_SIZE;
        let result = snap_to_walkable(
            &l.walkable,
            &OccupancyOverlay::new(),
            (0, wall_cell_y),
            cell_w,
            cell_h,
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
        // origin and accidentally cover from=(4,4). This ensures
        // snap_to_walkable succeeds on `from` but fails on the goal.
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
        let snapped = snap_point_to_walkable(&l.walkable, blocked_p)
            .expect("blocked desk should snap nearby");
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

        let result = snap_to_walkable(&mask, &overlay, (cell_w - 1, cell_h - 1), cell_w, cell_h);
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
}
