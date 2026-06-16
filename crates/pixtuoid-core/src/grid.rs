//! A tiny checked 2-D grid — pure data, no terminal deps.
//!
//! `Grid<T>` is a `width × height` row-major `Vec<T>` with bounds-checked
//! access. It consolidates the hand-rolled `y * width + x` indexing + edge
//! clamps that `WalkableMask` (a `Grid<bool>` pixel mask) and `ReachSet` (a
//! `Grid<bool>` coarse-cell reachability set) each re-implemented (#333). The
//! checked `get`/`set` make an off-by-one or a transposed index a `None`/clip
//! rather than a panic or a silent wrong-cell read.
//!
//! Coordinates are `(x, y)` u16, origin top-left.

/// A `width × height` row-major grid of `T`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grid<T> {
    pub width: u16,
    pub height: u16,
    data: Vec<T>,
}

impl<T: Clone> Grid<T> {
    /// A `width × height` grid with every cell set to `fill`.
    pub fn filled(width: u16, height: u16, fill: T) -> Self {
        Self {
            width,
            height,
            data: vec![fill; width as usize * height as usize],
        }
    }
}

impl<T> Grid<T> {
    /// Row-major flat index of `(x, y)`, or `None` out of bounds.
    #[inline]
    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// The cell at `(x, y)`, or `None` out of bounds.
    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Option<&T> {
        self.index(x, y).map(|i| &self.data[i])
    }

    /// Set `(x, y)` if in bounds; a no-op (clip) when out of bounds — callers
    /// stamp padded rects that may extend past the edge.
    #[inline]
    pub fn set(&mut self, x: u16, y: u16, value: T) {
        if let Some(i) = self.index(x, y) {
            self.data[i] = value;
        }
    }
}

impl<T: Copy> Grid<T> {
    /// The cell at `(x, y)`, or `default` out of bounds — the common read for
    /// `Copy` cells (e.g. a `bool` mask that reads `false` past the edge).
    #[inline]
    pub fn get_or(&self, x: u16, y: u16, default: T) -> T {
        self.get(x, y).copied().unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_then_get_set_round_trips() {
        let mut g = Grid::filled(4, 3, 0u8);
        assert_eq!(g.width, 4);
        assert_eq!(g.height, 3);
        assert_eq!(g.get(0, 0), Some(&0));
        g.set(2, 1, 7);
        assert_eq!(g.get(2, 1), Some(&7));
        // The neighbour is untouched — the index math isn't transposed.
        assert_eq!(g.get(1, 2), Some(&0));
    }

    #[test]
    fn out_of_bounds_get_is_none_and_set_is_a_noop() {
        let mut g = Grid::filled(2, 2, false);
        assert_eq!(g.get(2, 0), None);
        assert_eq!(g.get(0, 2), None);
        g.set(5, 5, true); // clipped, no panic
        assert!(!g.get_or(5, 5, false));
    }

    #[test]
    fn get_or_returns_default_past_the_edge() {
        let g = Grid::filled(2, 2, true);
        assert!(g.get_or(0, 0, false));
        assert!(!g.get_or(9, 9, false));
    }
}
