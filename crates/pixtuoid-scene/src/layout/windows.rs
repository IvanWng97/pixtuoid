//! The north wall's floor-to-ceiling windows: where each pane stands, in the
//! layout's own units, so every painter tiles the same panes and the door lines
//! up with them.

use std::ops::Range;

use super::{Layout, ELEVATOR_W};

/// A pane's width, frame included.
pub(crate) const WINDOW_W: u16 = 22;
/// The wall between two panes.
pub(crate) const WINDOW_GAP: u16 = 3;
/// The first pane's left edge.
const FIRST_WINDOW_X: u16 = 3;
/// The tiling stops once the next pane would leave less wall than this before
/// the right edge.
const WINDOW_EDGE_MARGIN: u16 = 2;
/// The wall row above the panes.
const WINDOW_TOP: u16 = 1;
/// The panes' least height, however short the band.
const MIN_WINDOW_H: u16 = 8;

/// One pane on the north wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowBay {
    /// The pane's left edge.
    pub(crate) x: u16,
    /// The pane's place in the tiling, counted from the left over every pane,
    /// those the door covers included, so a pane keeps its index whether or not
    /// a door takes the one before it.
    pub(crate) idx: u16,
}

impl WindowBay {
    /// The pane's middle column.
    pub(crate) fn center_x(self) -> u16 {
        self.x + WINDOW_W / 2
    }

    /// The columns the pane spans, frame included.
    pub(crate) fn span(self) -> Range<u16> {
        self.x..self.x + WINDOW_W
    }
}

/// Every pane a wall `buf_w` wide fits, the door ignored.
fn tiling(buf_w: u16) -> impl Iterator<Item = WindowBay> {
    (0u16..).map_while(move |idx| {
        let x = idx
            .checked_mul(WINDOW_W + WINDOW_GAP)?
            .checked_add(FIRST_WINDOW_X)?;
        let fits = u32::from(x) + u32::from(WINDOW_W + WINDOW_EDGE_MARGIN) <= u32::from(buf_w);
        fits.then_some(WindowBay { x, idx })
    })
}

/// The panes a wall `buf_w` wide shows, left to right: the tiling less every
/// pane `door` overlaps.
pub(crate) fn window_bays(buf_w: u16, door: Option<Range<u16>>) -> impl Iterator<Item = WindowBay> {
    tiling(buf_w).filter(move |b| {
        !door
            .as_ref()
            .is_some_and(|d| b.x < d.end && b.x + WINDOW_W > d.start)
    })
}

/// The columns from the first pane's left edge to the last one's right, the
/// panes a door covers included; the first pane alone when none fits.
pub(crate) fn window_run(buf_w: u16) -> Range<u16> {
    let end = tiling(buf_w)
        .last()
        .map_or(FIRST_WINDOW_X + WINDOW_W, |b| b.span().end);
    FIRST_WINDOW_X..end
}

/// The rows a pane spans on a wall band `band_h` tall, frame included: one wall
/// row above it, and the band's trim below.
pub(crate) fn window_rows(band_h: u16) -> Range<u16> {
    WINDOW_TOP..WINDOW_TOP + band_h.saturating_sub(2).max(MIN_WINDOW_H)
}

impl Layout {
    /// The panes this office's north wall shows, left to right.
    pub(crate) fn window_bays(&self) -> impl Iterator<Item = WindowBay> {
        window_bays(self.buf_w, self.door_span())
    }

    /// The columns the elevator door takes out of the window wall.
    pub(crate) fn door_span(&self) -> Option<Range<u16>> {
        self.door.map(|d| d.x..d.x + ELEVATOR_W)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panes_tile_from_the_start_and_keep_their_index_across_a_door() {
        let buf_w = FIRST_WINDOW_X + 4 * (WINDOW_W + WINDOW_GAP) + WINDOW_W + WINDOW_EDGE_MARGIN;
        let all: Vec<_> = window_bays(buf_w, None).collect();
        assert_eq!(
            all.len(),
            5,
            "exactly five panes fit, the last flush with the margin"
        );
        for (k, b) in all.iter().enumerate() {
            assert_eq!(b.idx, k as u16);
            assert_eq!(b.x, FIRST_WINDOW_X + k as u16 * (WINDOW_W + WINDOW_GAP));
            assert!(b.span().end + WINDOW_EDGE_MARGIN <= buf_w);
        }
        assert!(
            window_bays(buf_w - 1, None).count() == 4,
            "one column less and the last pane no longer clears the margin"
        );

        let doomed = all[2];
        let kept: Vec<_> = window_bays(buf_w, Some(doomed.span())).collect();
        assert_eq!(kept.len(), 4, "the door takes exactly the pane it overlaps");
        assert!(kept.iter().all(|b| b.idx != doomed.idx));
        assert_eq!(kept[2].idx, 3, "the pane after the door keeps its index");
        assert_eq!(window_run(buf_w), all[0].x..all[4].span().end);
    }

    #[test]
    fn a_wall_too_narrow_for_a_pane_runs_the_first_pane_alone() {
        assert_eq!(window_bays(WINDOW_W, None).count(), 0);
        assert_eq!(
            window_run(WINDOW_W),
            FIRST_WINDOW_X..FIRST_WINDOW_X + WINDOW_W
        );
    }

    #[test]
    fn a_pane_leaves_a_wall_row_above_and_the_trim_below_but_never_shrinks_past_its_least() {
        let band_h = 30;
        let rows = window_rows(band_h);
        assert_eq!(rows.start, WINDOW_TOP);
        assert_eq!(
            rows.end,
            band_h - 1,
            "the pane stops at the band's trim row"
        );
        assert_eq!(window_rows(4).len(), usize::from(MIN_WINDOW_H));
    }

    #[test]
    fn the_widest_wall_tiles_without_overflowing() {
        let last = window_bays(u16::MAX, None).last().expect("panes fit");
        assert!(
            u32::from(last.x) + u32::from(WINDOW_W + WINDOW_EDGE_MARGIN) <= u32::from(u16::MAX)
        );
    }
}
