//! Shared coarse routing-grid primitives — the ONE definition of the cell
//! coarsening the A\* router (`crate::pathfind`) and the reachability BFS
//! (`super::reach`) both ride. Coarsening both against the SAME cell size,
//! walkability threshold, 8-neighbour set, and snap is what makes "reachable
//! here" (`ReachSet`) agree with "routable here" (A\*) — an agreement that used
//! to be pinned only by a 2-const compile assert across two hand-kept copies of
//! this logic.

use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};

/// Coarse-cell edge in px. Smaller = more accurate paths, more work per query.
/// 4 px gives a ~40×60 grid on a typical 160×240 buffer — A\* finishes well under
/// 1 ms uncached. `pathfind::CELL_SIZE` re-exports this value.
pub(crate) const COARSE_CELL_SIZE: u16 = 4;

/// Min walkable px (of `COARSE_CELL_SIZE²` = 16) for a coarse cell to count as
/// walkable. At 8 (50%) the coarsened grid can squeeze through 2px corridors,
/// which the meeting-room interior needs after furniture padding; tighter (12 =
/// 75%) made the meeting room unreachable, looser (4 = 25%) grazed furniture
/// edges. 50% is the sweet spot.
const COARSE_CELL_WALKABLE_MIN: u16 = 8;

/// The 8-connected neighbour offsets both the A\* expansion and the reach BFS
/// step over.
pub(crate) const NEIGHBORS_8: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Is coarse cell `(cx, cy)` walkable — ≥ `COARSE_CELL_WALKABLE_MIN` of its
/// `COARSE_CELL_SIZE²` pixels open on the static `mask` AND clear of the per-frame
/// `overlay`? The reach BFS passes an EMPTY overlay (static geometry only); the
/// router passes the live occupancy overlay.
pub(crate) fn cell_walkable(
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    cx: u16,
    cy: u16,
) -> bool {
    let px_start = cx.saturating_mul(COARSE_CELL_SIZE);
    let py_start = cy.saturating_mul(COARSE_CELL_SIZE);
    let mut walk_count = 0u16;
    for dy in 0..COARSE_CELL_SIZE {
        for dx in 0..COARSE_CELL_SIZE {
            let px = px_start + dx;
            let py = py_start + dy;
            if mask.is_walkable(px, py) && !overlay.blocks(px, py) {
                walk_count += 1;
            }
        }
    }
    walk_count >= COARSE_CELL_WALKABLE_MIN
}

/// Snap coarse `cell` to the nearest walkable coarse cell within `max_radius`
/// rings (Chebyshev), or `None` when none is walkable in range (or the cell is
/// out of the `cell_w × cell_h` grid). The A\* start/goal snap passes
/// `MAX_SNAP_RADIUS`; the reach seed snap passes its shorter `SEED_SNAP_CELLS`.
pub(crate) fn snap(
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    cell: (u16, u16),
    cell_w: u16,
    cell_h: u16,
    max_radius: u16,
) -> Option<(u16, u16)> {
    if cell.0 < cell_w && cell.1 < cell_h && cell_walkable(mask, overlay, cell.0, cell.1) {
        return Some(cell);
    }
    for r in 1..=max_radius {
        let r_i = r as i32;
        for dy in -r_i..=r_i {
            for dx in -r_i..=r_i {
                if dx.abs() != r_i && dy.abs() != r_i {
                    continue; // ring only
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
