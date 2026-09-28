//! Where the office sits under the terminal's cells.

use pixtuoid_scene::layout::Point;

/// The office pixels one terminal cell shows, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellArea {
    x0: u16,
    x1: u16,
    y0: u16,
    y1: u16,
}

impl CellArea {
    /// Under the half-block flush (`renderer::flush_buffer_to_term_at_offset`,
    /// from `renderer::scene_rect`'s origin) a cell shows one pixel column and
    /// two rows: its upper half and its lower half.
    pub(crate) fn half_block(col: u16, row: u16) -> Self {
        let y0 = row.saturating_mul(2);
        Self {
            x0: col,
            x1: col,
            y0,
            y1: y0.saturating_add(1),
        }
    }

    /// Whether this cell shows any pixel of the `w`×`h` box whose top-left is
    /// `tl`, so a box whose edge falls inside the cell is hit from it, whichever
    /// part of the cell shows it. An empty box shows nowhere.
    pub(crate) fn overlaps(self, tl: Point, w: u16, h: u16) -> bool {
        // u32: a box's exclusive end can lie one past `u16::MAX`.
        let (end_x, end_y) = (
            u32::from(tl.x) + u32::from(w),
            u32::from(tl.y) + u32::from(h),
        );
        w > 0
            && h > 0
            && u32::from(self.x0) < end_x
            && tl.x <= self.x1
            && u32::from(self.y0) < end_y
            && tl.y <= self.y1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box starting on a cell's LOWER half is shown by that cell, and a box
    /// ending on a cell's upper half by that one: every cell the box touches
    /// hits, and only those.
    #[test]
    fn a_cell_hits_a_box_whenever_either_half_shows_it() {
        let tl = Point { x: 3, y: 5 };
        let (w, h) = (2, 4); // rows 5..=8: cells 2 (lower half), 3, 4 (upper half)
        let hits: Vec<(u16, u16)> = (0..8)
            .flat_map(|row| (0..8).map(move |col| (col, row)))
            .filter(|&(col, row)| CellArea::half_block(col, row).overlaps(tl, w, h))
            .collect();
        assert_eq!(hits, [(3, 2), (4, 2), (3, 3), (4, 3), (3, 4), (4, 4)]);
    }

    #[test]
    fn an_empty_box_is_hit_from_nowhere() {
        let at = Point { x: 3, y: 4 };
        assert!(!CellArea::half_block(3, 2).overlaps(at, 0, 4));
        assert!(!CellArea::half_block(3, 2).overlaps(at, 2, 0));
    }

    #[test]
    fn the_far_edge_saturates_rather_than_wrapping() {
        let cell = CellArea::half_block(u16::MAX, u16::MAX);
        assert!(cell.overlaps(
            Point {
                x: u16::MAX - 1,
                y: u16::MAX - 1
            },
            2,
            2
        ));
    }
}
