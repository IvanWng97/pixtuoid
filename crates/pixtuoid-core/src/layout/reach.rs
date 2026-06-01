//! Coarse-cell reachability over a [`WalkableMask`] — a pure-core mirror of the
//! tui A\* router's coarsening, so the geometry layer can ask "is this cell
//! actually *routable*?" without importing the router (workspace invariant #1:
//! `pixtuoid-core` has no terminal/router deps).
//!
//! [`approach_point`](super::approach) uses it to PREFER an A\*-reachable
//! approach side over a merely-walkable-but-walled-off one — the "available
//! approach side" rule. The coarsening (`REACH_CELL_SIZE` × `REACH_CELL_SIZE`
//! cells, ≥ `REACH_CELL_WALKABLE_MIN` walkable px) is kept byte-identical to
//! `tui::pathfind` (asserted by a `const` equality check on the tui side), so
//! "reachable here" means the same thing A\* will find at route time.

use std::collections::VecDeque;

use super::Point;
use crate::walkable::WalkableMask;

/// Coarse-cell edge in px. MUST equal `tui::pathfind::CELL_SIZE` (asserted there).
pub const REACH_CELL_SIZE: u16 = 4;
/// Min walkable px (of `REACH_CELL_SIZE²` = 16) for a coarse cell to count as
/// walkable. MUST equal `tui::pathfind`'s `CELL_WALKABLE_MIN` (asserted there).
pub const REACH_CELL_WALKABLE_MIN: u16 = 8;

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

/// How far (in coarse cells) to snap a blocked seed to the nearest walkable
/// coarse cell — mirrors the router's start-snap so a seed sitting on a blocked
/// pixel (a door on a wall edge, a desk) still lands in the right component.
const SEED_SNAP_CELLS: i32 = 3;

fn coarse_cell_walkable(mask: &WalkableMask, cx: u16, cy: u16) -> bool {
    let px0 = cx.saturating_mul(REACH_CELL_SIZE);
    let py0 = cy.saturating_mul(REACH_CELL_SIZE);
    let mut n = 0u16;
    for dy in 0..REACH_CELL_SIZE {
        for dx in 0..REACH_CELL_SIZE {
            if mask.is_walkable(px0 + dx, py0 + dy) {
                n += 1;
            }
        }
    }
    n >= REACH_CELL_WALKABLE_MIN
}

fn snap_seed(mask: &WalkableMask, seed: Point, cell_w: u16, cell_h: u16) -> Option<(u16, u16)> {
    let c = (seed.x / REACH_CELL_SIZE, seed.y / REACH_CELL_SIZE);
    if c.0 < cell_w && c.1 < cell_h && coarse_cell_walkable(mask, c.0, c.1) {
        return Some(c);
    }
    for r in 1..=SEED_SNAP_CELLS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue; // ring only
                }
                let nx = c.0 as i32 + dx;
                let ny = c.1 as i32 + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let (nx, ny) = (nx as u16, ny as u16);
                if nx < cell_w && ny < cell_h && coarse_cell_walkable(mask, nx, ny) {
                    return Some((nx, ny));
                }
            }
        }
    }
    None
}

/// The set of coarse cells reachable (8-connected) from a seed — i.e. the
/// agent's connected walkable component. Built once per layout from a known
/// in-component seed (the door, or a home desk); real floors are
/// connectivity-tested, so this set covers the whole walkable area.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReachSet {
    cell_w: u16,
    cell_h: u16,
    reachable: Vec<bool>,
}

impl ReachSet {
    /// 8-connected coarse BFS from `seed`'s cell (snapped to the nearest walkable
    /// coarse cell when `seed` lands on a blocked one, like the router's start
    /// snap). An empty/degenerate mask yields an all-unreachable set.
    pub fn from_mask(mask: &WalkableMask, seed: Point) -> ReachSet {
        let cell_w = mask.width / REACH_CELL_SIZE;
        let cell_h = mask.height / REACH_CELL_SIZE;
        let mut reachable = vec![false; cell_w as usize * cell_h as usize];
        if cell_w == 0 || cell_h == 0 {
            return ReachSet {
                cell_w,
                cell_h,
                reachable,
            };
        }
        let idx = |cx: u16, cy: u16| cy as usize * cell_w as usize + cx as usize;
        if let Some(start) = snap_seed(mask, seed, cell_w, cell_h) {
            let mut q = VecDeque::new();
            reachable[idx(start.0, start.1)] = true;
            q.push_back(start);
            while let Some((cx, cy)) = q.pop_front() {
                for (dx, dy) in NEIGHBORS_8 {
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || ny < 0 {
                        continue;
                    }
                    let (nx, ny) = (nx as u16, ny as u16);
                    if nx >= cell_w || ny >= cell_h || reachable[idx(nx, ny)] {
                        continue;
                    }
                    if coarse_cell_walkable(mask, nx, ny) {
                        reachable[idx(nx, ny)] = true;
                        q.push_back((nx, ny));
                    }
                }
            }
        }
        ReachSet {
            cell_w,
            cell_h,
            reachable,
        }
    }

    /// Is the coarse cell containing pixel `p` in the reachable component?
    /// Out-of-bounds or blocked → `false`.
    ///
    /// **Conservative at cell boundaries** (a lone walkable px inside a
    /// <50%-walkable coarse cell reads unreachable), but NEVER a false positive:
    /// `reaches(p) ⇒ A* can route to p`. So `approach_point` can safely drop any
    /// side `reaches` rejects — it will never pick an unroutable one.
    pub fn reaches(&self, p: Point) -> bool {
        let cx = p.x / REACH_CELL_SIZE;
        let cy = p.y / REACH_CELL_SIZE;
        if cx >= self.cell_w || cy >= self.cell_h {
            return false;
        }
        self.reachable[cy as usize * self.cell_w as usize + cx as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_field_reaches_everywhere() {
        let m = WalkableMask::new_open(64, 64);
        let r = ReachSet::from_mask(&m, Point { x: 4, y: 4 });
        assert!(r.reaches(Point { x: 4, y: 4 }));
        assert!(r.reaches(Point { x: 60, y: 60 }));
        assert!(r.reaches(Point { x: 32, y: 8 }));
    }

    #[test]
    fn walled_pocket_is_unreachable_but_main_is_reachable() {
        // Split a 64×64 field with a full-height wall at x∈[28,36): the seed on
        // the left can reach the left side but NOT the right pocket.
        let mut m = WalkableMask::new_open(64, 64);
        m.mark_blocked(28, 0, 8, 64, 0);
        let r = ReachSet::from_mask(&m, Point { x: 8, y: 32 });
        assert!(r.reaches(Point { x: 8, y: 32 }), "seed side reachable");
        assert!(
            r.reaches(Point { x: 20, y: 50 }),
            "rest of seed side reachable"
        );
        assert!(
            !r.reaches(Point { x: 50, y: 32 }),
            "walled-off pocket must be unreachable"
        );
    }

    #[test]
    fn blocked_seed_snaps_into_the_component() {
        // Seed lands on the blocked wall column; snap should pull it into the
        // adjacent walkable component rather than yield an empty set.
        let mut m = WalkableMask::new_open(64, 64);
        m.mark_blocked(30, 0, 4, 64, 0);
        let r = ReachSet::from_mask(&m, Point { x: 31, y: 32 }); // on the wall
                                                                 // Snapped to one side → that side is reachable (whichever it picked).
        assert!(
            r.reaches(Point { x: 8, y: 32 }) || r.reaches(Point { x: 56, y: 32 }),
            "a blocked seed must snap into SOME component, not vanish"
        );
    }
}
