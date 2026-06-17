//! Walkability primitives — pure data, no terminal deps.
//!
//! `WalkableMask` is a per-pixel boolean grid stating which positions are
//! open floor (`true`) vs obstacle (`false`). Built once by the layout
//! engine, queried by any router implementation.
//!
//! `OccupancyOverlay` is the dynamic counterpart — a small list of blocked
//! rects added/cleared each frame so routers can avoid live agents. Kept
//! separate from the static mask so the mask can be cached / shipped over
//! the wire (serializable; no out-of-process consumer today) while occupancy
//! stays per-frame.
//!
//! Both types are sprite-pack-agnostic and have no terminal dependencies,
//! so they're safe to live in core and reuse from any future renderer
//! (web, native canvas, etc.).
//!
//! Coordinates are `(x, y)` u16 pixel positions; origin top-left.

use crate::grid::Grid;

/// Static obstacle mask sized `width × height` pixels — a `Grid<bool>`
/// (`true` = open floor, `false` = obstacle). An ALIAS, not a wrapper: the
/// mask's dims ARE the grid's `pub width`/`height` (no mirrored fields), and
/// the obstacle ops live in the `impl Grid<bool>` below. This is the clean
/// endpoint of the #333 `Grid<T>` extraction — it landed in the 0.10.0 break
/// because removing the old `pub struct WalkableMask` is a cargo-semver-checks
/// `struct_missing`.
pub type WalkableMask = Grid<bool>;

// Obstacle ops on the concrete `Grid<bool>` instantiation. ACCEPTED RESIDUAL of
// the alias form (review LOW): these become visible on EVERY `Grid<bool>`,
// including `ReachSet`'s private inner grid where `is_walkable`/`mark_blocked`
// are semantically off — but that grid is private (no external surface) and the
// methods are never called there, so the leak is harmless. An extension trait
// would scope them at the cost of an import at every call site (the churn the
// alias exists to avoid); not worth it.
impl Grid<bool> {
    /// Create a fully-open mask. Caller fills obstacles via `mark_blocked`.
    pub fn new_open(width: u16, height: u16) -> Self {
        Grid::filled(width, height, true)
    }

    /// Mark a rect (with `pad` extra pixels on each side) as blocked.
    /// Out-of-bounds pixels are clipped — caller doesn't need to bounds-check.
    pub fn mark_blocked(&mut self, x: u16, y: u16, w: u16, h: u16, pad: u16) {
        let min_x = x.saturating_sub(pad);
        let max_x = x.saturating_add(w).saturating_add(pad).min(self.width);
        let min_y = y.saturating_sub(pad);
        let max_y = y.saturating_add(h).saturating_add(pad).min(self.height);
        for yy in min_y..max_y {
            for xx in min_x..max_x {
                self.set(xx, yy, false);
            }
        }
    }

    /// Carve `true` back into a rect — used for door cutouts in the wall band.
    pub fn mark_walkable(&mut self, x: u16, y: u16, w: u16, h: u16) {
        let max_x = x.saturating_add(w).min(self.width);
        let max_y = y.saturating_add(h).min(self.height);
        for yy in y..max_y {
            for xx in x..max_x {
                self.set(xx, yy, true);
            }
        }
    }

    /// O(1) walkability lookup. Out-of-bounds queries return `false` so
    /// routers can probe near the edges without bounds checks.
    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.get_or(x, y, false)
    }
}

/// Dynamic per-frame occupancy — rebuilt each render tick from current
/// agent positions. Composed on top of `WalkableMask` so routers can
/// avoid live agents without modifying the static mask.
#[derive(Debug, Clone, Default)]
pub struct OccupancyOverlay {
    /// `(x, y, width, height)` of each blocked rect.
    rects: Vec<(u16, u16, u16, u16)>,
}

impl OccupancyOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn add(&mut self, x: u16, y: u16, w: u16, h: u16) {
        self.rects.push((x, y, w, h));
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// True if (`x`, `y`) falls inside any current occupancy rect.
    /// Linear scan — fine while N stays in the low tens.
    pub fn blocks(&self, x: u16, y: u16) -> bool {
        self.rects.iter().any(|&(rx, ry, rw, rh)| {
            x >= rx && x < rx.saturating_add(rw) && y >= ry && y < ry.saturating_add(rh)
        })
    }

    /// Order-stable hash of the current occupancy set. Rects are sorted
    /// before hashing so two overlays containing the same rects in
    /// different push order produce the same signature — important for
    /// the router cache, which uses signature equality to decide whether
    /// to invalidate.
    pub fn signature(&self) -> u64 {
        let mut sorted: Vec<(u16, u16, u16, u16)> = self.rects.clone();
        sorted.sort_unstable();
        let mut hash: u64 = crate::id::FNV_OFFSET_BASIS;
        for &(x, y, w, h) in &sorted {
            for v in [x, y, w, h] {
                hash ^= v as u64;
                hash = hash.wrapping_mul(crate::id::FNV_PRIME);
            }
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_open_is_all_walkable() {
        let m = WalkableMask::new_open(8, 4);
        for y in 0..4 {
            for x in 0..8 {
                assert!(m.is_walkable(x, y));
            }
        }
    }

    #[test]
    fn mark_blocked_pads_and_clips() {
        let mut m = WalkableMask::new_open(10, 10);
        m.mark_blocked(4, 4, 2, 2, 1);
        // Padded rect: x=3..7, y=3..7 are blocked.
        for y in 3..7 {
            for x in 3..7 {
                assert!(!m.is_walkable(x, y), "({x},{y}) should be blocked");
            }
        }
        // Outside still walkable.
        assert!(m.is_walkable(2, 4));
        assert!(m.is_walkable(8, 4));
    }

    #[test]
    fn mark_walkable_carves_a_cutout() {
        let mut m = WalkableMask::new_open(10, 10);
        m.mark_blocked(0, 0, 10, 4, 0);
        assert!(!m.is_walkable(5, 2));
        m.mark_walkable(4, 0, 3, 4);
        assert!(m.is_walkable(5, 2));
    }

    #[test]
    fn out_of_bounds_query_is_not_walkable() {
        let m = WalkableMask::new_open(4, 4);
        assert!(!m.is_walkable(4, 0));
        assert!(!m.is_walkable(0, 4));
    }

    #[test]
    fn overlay_blocks_inside_rects() {
        let mut o = OccupancyOverlay::new();
        assert_eq!(o.len(), 0);
        o.add(10, 10, 5, 5);
        o.add(20, 20, 3, 3);
        assert_eq!(o.len(), 2);
        assert!(o.blocks(12, 12));
        assert!(!o.blocks(9, 10));
        assert!(!o.blocks(15, 10));
        o.clear();
        assert_eq!(o.len(), 0);
    }

    #[test]
    fn overlay_signature_changes_with_contents() {
        let mut o = OccupancyOverlay::new();
        let s_empty = o.signature();
        o.add(1, 2, 3, 4);
        let s_one = o.signature();
        assert_ne!(s_empty, s_one);
        o.clear();
        assert_eq!(o.signature(), s_empty);
    }

    #[test]
    fn overlay_signature_is_order_independent() {
        let mut a = OccupancyOverlay::new();
        a.add(10, 20, 5, 5);
        a.add(30, 40, 8, 8);
        let mut b = OccupancyOverlay::new();
        b.add(30, 40, 8, 8);
        b.add(10, 20, 5, 5);
        assert_eq!(a.signature(), b.signature());
    }
}
