//! Screen text: a grid of terminal cells, the one form the footer, tooltips
//! and panels take before a painter draws them, as terminal cells or in the
//! pixel font ([`paint_grid`](crate::cutaway::paint_grid)). Its cells are the
//! ones ratatui's buffer writes ([`text::cells`](super::text::cells)), so
//! both draw the same layout.

use pixtuoid_core::sprite::Rgb;

use super::text::clusters;

/// How far a card's drop shadow darkens what is under it, toward black. One
/// uniform factor is the owner's preference, not an unfinished gradient.
pub const CARD_SHADOW: f32 = 0.42;

/// A rectangle of cells: its top-left and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// One cell of screen text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    /// The grapheme cluster it shows; empty for the cells after a wide one's
    /// first, which it covers.
    pub symbol: String,
    /// Its ink; `None` is the painter's own text colour.
    pub fg: Option<Rgb>,
    /// Its fill; `None` leaves what is under it.
    pub bg: Option<Rgb>,
    pub bold: bool,
}

impl GridCell {
    const BLANK: &str = " ";

    fn blank() -> Self {
        Self {
            symbol: Self::BLANK.to_owned(),
            fg: None,
            bg: None,
            bold: false,
        }
    }
}

/// A `w`×`h` grid of [`GridCell`]s, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellGrid {
    w: u16,
    h: u16,
    cells: Vec<GridCell>,
}

impl CellGrid {
    /// A `w`×`h` grid of blank cells with no ink or fill.
    pub fn new(w: u16, h: u16) -> Self {
        Self {
            w,
            h,
            cells: vec![GridCell::blank(); usize::from(w) * usize::from(h)],
        }
    }

    pub fn width(&self) -> u16 {
        self.w
    }

    pub fn height(&self) -> u16 {
        self.h
    }

    /// Its size, at the origin.
    pub fn rect(&self) -> CellRect {
        CellRect {
            w: self.w,
            h: self.h,
            ..CellRect::default()
        }
    }

    /// The cell at `(x, y)`, `None` off the grid.
    pub fn get(&self, x: u16, y: u16) -> Option<&GridCell> {
        (x < self.w && y < self.h)
            .then(|| {
                self.cells
                    .get(usize::from(y) * usize::from(self.w) + usize::from(x))
            })
            .flatten()
    }

    fn get_mut(&mut self, x: u16, y: u16) -> Option<&mut GridCell> {
        (x < self.w && y < self.h)
            .then(|| {
                self.cells
                    .get_mut(usize::from(y) * usize::from(self.w) + usize::from(x))
            })
            .flatten()
    }

    /// Fill every cell with `bg`.
    pub fn fill(&mut self, bg: Rgb) {
        for cell in &mut self.cells {
            cell.bg = Some(bg);
        }
    }

    /// Write `text` from `(x, y)` in `fg`, a cluster a cell (or two, wide),
    /// as ratatui's buffer writes it; a cluster that would cross the right
    /// edge is dropped with the rest. Returns the column after it.
    pub fn put(&mut self, (x, y): (u16, u16), text: &str, fg: Option<Rgb>, bold: bool) -> u16 {
        let mut col = x;
        for (cluster, n) in clusters(text) {
            if col.saturating_add(n) > self.w {
                break;
            }
            for i in 0..n {
                if let Some(cell) = self.get_mut(col + i, y) {
                    cell.symbol = if i == 0 {
                        cluster.to_owned()
                    } else {
                        String::new()
                    };
                    cell.fg = fg;
                    cell.bold = bold;
                }
            }
            col += n;
        }
        col
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INK: Rgb = Rgb { r: 9, g: 8, b: 7 };

    fn symbols(grid: &CellGrid, y: u16) -> Vec<String> {
        (0..grid.width())
            .map(|x| grid.get(x, y).map(|c| c.symbol.clone()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_wide_cluster_takes_two_cells_the_second_empty() {
        let mut grid = CellGrid::new(5, 1);
        assert_eq!(grid.put((0, 0), "a\u{65e5}b", Some(INK), false), 4);
        assert_eq!(symbols(&grid, 0), ["a", "\u{65e5}", "", "b", " "]);
        assert_eq!(grid.get(2, 0).and_then(|c| c.fg), Some(INK));
    }

    #[test]
    fn a_cluster_crossing_the_right_edge_is_dropped() {
        let mut grid = CellGrid::new(3, 1);
        assert_eq!(grid.put((1, 0), "a\u{65e5}", None, false), 2);
        assert_eq!(symbols(&grid, 0), [" ", "a", " "]);
    }

    #[test]
    fn writes_off_the_grid_land_nowhere() {
        let mut grid = CellGrid::new(2, 1);
        grid.put((0, 3), "ab", None, false);
        assert_eq!(grid, CellGrid::new(2, 1));
        assert!(grid.get(2, 0).is_none() && grid.get(0, 1).is_none());
    }
}
