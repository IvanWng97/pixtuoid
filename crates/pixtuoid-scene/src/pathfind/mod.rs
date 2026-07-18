//! Pathfinding façade — `Router` trait + `AStarRouter` impl.
//!
//! `Router` is the abstraction the renderer codes against: give it a static
//! `WalkableMask` and a per-frame `OccupancyOverlay`, ask for a polyline
//! from A to B, get back the route. The trait stays small so future impls
//! (Theta*, HPA*, navmesh) can drop in without touching `pose.rs` or
//! `renderer.rs`.
//!
//! `AStarRouter` is the concrete impl: A* on a coarsened 4×4 cell grid
//! with a permissive cell-walkability threshold (≥8/16 px walkable, 50% —
//! see `layout::coarse::COARSE_CELL_WALKABLE_MIN` for why tighter thresholds
//! were rejected). The coarse-grid primitives (`cell_walkable`/`snap`/
//! `NEIGHBORS_8`/`CELL_SIZE`) are the SHARED `layout::coarse` ones `layout::reach`
//! also rides, so router reachability can't drift from `ReachSet`. Memoizes
//! results in a per-(from, to) cache; auto-invalidates when the overlay
//! signature changes so per-frame agent movement still routes around live agents.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};

use crate::layout::{cell_walkable, snap, Bounds, Point, COARSE_CELL_SIZE, NEIGHBORS_8};

/// Cell size in pixels — the coarse routing-grid edge, re-exported from the
/// SHARED `layout::coarse` (the single source `layout::reach`'s BFS also rides,
/// so the router coarsening and the reachability coarsening can't drift — the
/// agreement the two old `const _: () = assert!` checks pinned is now structural).
/// Public name kept for this module's helpers + tests.
pub const CELL_SIZE: u16 = COARSE_CELL_SIZE;

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

/// Path-cache entry cap. The (from, to) key space is unbounded in steady
/// state: aimless wander mints a fresh pseudo-random destination every cycle
/// and snap-back/exit legs route from live interpolated origins, so an
/// always-on office accumulates keys forever — and the per-overlay-change
/// `retain` scan inside `route` grows linearly with the map. Keys are
/// per-agent jittered (from, to) PAIRS, not shared anchors: a fully loaded
/// floor (16 agents × ~12-16 waypoints × 2 directions) recurs ~400-500
/// keys, which 512 covers. If the working set ever cycles past the cap
/// anyway, the failure mode is graceful: on overflow the whole map is
/// cleared, and each evicted route is a sub-ms uncached A* on its next
/// request — at most #walking-agents re-misses per frame. A mid-leg clear
/// is safe by construction: cornered in-flight legs are frozen on
/// `MotionState.walk_path` (they never re-consult the router), and a
/// straight 2-point leg recomputes bit-identically only while the overlay
/// is unchanged — after a clear it re-routes under the CURRENT overlay,
/// the same self-healing re-route class the design already accepts for
/// unfrozen legs.
const PATH_CACHE_CAP: usize = 512;

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
        // Cache ONLY real routes. The no-path straight [from, to] fallback is
        // returned UNCACHED: `path_clear_under` validates cached entries against
        // the OVERLAY only (never the static mask), so a fallback minted while a
        // transient blocker severed the grid would survive every retain() and
        // serve a walk-through-walls line for that (from, to) key forever. Left
        // uncached it re-routes next call — the same self-healing re-route class
        // the walk-path freeze already accepts for 2-point legs.
        match find_path(mask, overlay, self.preferred_zone, from, to) {
            Some(path) => {
                self.paths.insert((from, to), path.clone());
                if self.paths.len() > PATH_CACHE_CAP {
                    self.paths.clear();
                }
                path
            }
            None => vec![from, to],
        }
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

/// Octile-distance step costs (integer, so the heuristic stays admissible): a
/// diagonal move costs `OCTILE_DIAGONAL_COST`, an orthogonal one
/// `OCTILE_STRAIGHT_COST` — the classic 14/10 ≈ √2 : 1 ratio. Shared with
/// `pose::octile_distance` so the heuristic and the path metric can't drift.
pub(crate) const OCTILE_STRAIGHT_COST: u32 = 10;
pub(crate) const OCTILE_DIAGONAL_COST: u32 = 14;

fn heuristic(a: (u16, u16), b: (u16, u16)) -> u32 {
    let dx = (a.0 as i32 - b.0 as i32).unsigned_abs();
    let dy = (a.1 as i32 - b.1 as i32).unsigned_abs();
    OCTILE_DIAGONAL_COST * dx.min(dy) + OCTILE_STRAIGHT_COST * (dx.max(dy) - dx.min(dy))
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
    let cell_w = mask.width() / CELL_SIZE;
    let cell_h = mask.height() / CELL_SIZE;
    if cell_w == 0 || cell_h == 0 {
        return None;
    }
    Some((cell_w, cell_h))
}

/// Max rings the A\* start/goal snap probes for a walkable coarse cell (the reach
/// seed snap uses a shorter radius). Passed to the shared `layout::snap`.
const MAX_SNAP_RADIUS: u16 = 12;

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

    // Preferred-corridor discount: a step inside the preferred zone costs
    // `NUM/DEN` (= 7/10, a 30% discount) so A* is biased to hug the corridor
    // without it being a hard constraint.
    const PREFERRED_ZONE_COST_NUM: u32 = 7;
    const PREFERRED_ZONE_COST_DEN: u32 = 10;

    let start = snap(
        mask,
        overlay,
        cell_of(from),
        cell_w,
        cell_h,
        MAX_SNAP_RADIUS,
    )?;
    let goal = snap(mask, overlay, cell_of(to), cell_w, cell_h, MAX_SNAP_RADIUS)?;

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
            let base_step = if dx.abs() + dy.abs() == 2 {
                OCTILE_DIAGONAL_COST
            } else {
                OCTILE_STRAIGHT_COST
            };
            let step = if cell_in_zone(preferred, nx, ny) {
                base_step * PREFERRED_ZONE_COST_NUM / PREFERRED_ZONE_COST_DEN
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
/// expands on — ≥`COARSE_CELL_WALKABLE_MIN`/16 px open)? This is the granularity the
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
    let (cx, cy) = snap(mask, &empty, cell_of(p), cell_w, cell_h, MAX_SNAP_RADIUS)?;
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
mod tests;
