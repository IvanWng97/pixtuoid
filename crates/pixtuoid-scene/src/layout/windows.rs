//! The north wall's floor-to-ceiling windows: where each one stands, in the
//! layout's own units, so every painter tiles the same windows and the elevator
//! door stands on their bottom line.

use std::ops::Range;

use super::{ELEVATOR_W, SceneLayout};

/// A window's width, frame included — fixed, so the skyline detail reads the
/// same on every terminal.
pub(crate) const WINDOW_W: u16 = 22;
/// The wall between two windows.
const WINDOW_GAP: u16 = 3;
/// The first window's left edge.
const FIRST_WINDOW_X: u16 = 3;
/// The tiling stops once the next window would leave less wall than this
/// before the right edge.
const WINDOW_EDGE_MARGIN: u16 = 2;
/// The windows' top frame row; one wall row stands above it.
const WINDOW_TOP: u16 = 1;

/// One window on the north wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowBay {
    /// The window's left edge.
    pub(crate) x: u16,
    /// The window's place in the tiling, counted from the left over every
    /// window, those the door covers included, so a window's weather seed
    /// stays put whether or not a door takes the one before it.
    pub(crate) idx: u16,
}

impl WindowBay {
    /// The window's middle column.
    pub(crate) fn center_x(self) -> u16 {
        self.x + WINDOW_W / 2
    }

    /// The columns the window spans, frame included.
    pub(crate) fn span(self) -> Range<u16> {
        self.x..self.x + WINDOW_W
    }
}

/// Every window a wall `buf_w` wide fits, the door ignored.
fn tiling(buf_w: u16) -> impl Iterator<Item = WindowBay> {
    (0u16..).map_while(move |idx| {
        // Widened, so the fit check near `u16::MAX` cannot overflow.
        let x = u32::from(FIRST_WINDOW_X) + u32::from(idx) * u32::from(WINDOW_W + WINDOW_GAP);
        let fits = x + u32::from(WINDOW_W + WINDOW_EDGE_MARGIN) <= u32::from(buf_w);
        fits.then_some(())?;
        Some(WindowBay {
            x: u16::try_from(x).ok()?,
            idx,
        })
    })
}

/// The windows a wall `buf_w` wide shows, left to right: the tiling less every
/// window `door` overlaps, whose glass would otherwise show through the
/// elevator's frame.
pub(crate) fn window_bays(buf_w: u16, door: Option<Range<u16>>) -> impl Iterator<Item = WindowBay> {
    tiling(buf_w).filter(move |b| {
        !door
            .as_ref()
            .is_some_and(|d| b.x < d.end && b.x + WINDOW_W > d.start)
    })
}

/// The columns from the first window's left edge to the last one's right, the
/// windows a door covers included; the first window alone when none fits.
pub(crate) fn window_run(buf_w: u16) -> Range<u16> {
    let end = tiling(buf_w)
        .last()
        .map_or(FIRST_WINDOW_X + WINDOW_W, |b| b.span().end);
    FIRST_WINDOW_X..end
}

/// The wall band's trim row, where the band meets the floor.
pub(crate) fn wall_trim_row(band_h: u16) -> u16 {
    band_h.saturating_sub(1)
}

/// The rows a window spans on a wall band `band_h` tall, frame included: from
/// [`WINDOW_TOP`] down to the band's trim row, so a taller terminal gets taller
/// glass.
pub(crate) fn window_rows(band_h: u16) -> Range<u16> {
    WINDOW_TOP..wall_trim_row(band_h).max(WINDOW_TOP)
}

/// The glass rows of a window `window_h` tall: all but its top and bottom
/// frame rows.
pub(crate) fn glass_rows(window_h: u16) -> u16 {
    window_h.saturating_sub(2)
}

/// Where a window's transom crosses it, in percent of its height from the top.
const TRANSOM_PCT: u16 = 70;

/// Whether the cell `(dx, dy)` of a window `h` rows tall is its frame — its
/// edge, its centre mullion or its transom — which no glass shows through.
pub(crate) fn window_frame(dx: u16, dy: u16, h: u16) -> bool {
    dx == 0
        || dx == WINDOW_W - 1
        || dy == 0
        || dy + 1 == h
        || dx == WINDOW_W / 2
        || dy == h * TRANSOM_PCT / 100
}

impl SceneLayout {
    /// The windows this office's north wall shows, left to right: the door
    /// and its exit sign take out any they cover.
    pub(crate) fn window_bays(&self) -> impl Iterator<Item = WindowBay> + use<> {
        window_bays(
            self.buf_w,
            self.door.map(|d| super::exit_sign_x(d.x)..d.x + ELEVATOR_W),
        )
    }

    /// Whether cell `(x, y)` is a window's glass, where the outside shows
    /// rather than the room: inside a bay and off its [`window_frame`].
    pub(crate) fn glass_at(&self, x: u16, y: u16) -> bool {
        let rows = window_rows(self.wall_band_h());
        rows.contains(&y)
            && self.window_bays().any(|b| {
                (b.x..b.x + WINDOW_W).contains(&x)
                    && !window_frame(x - b.x, y - rows.start, rows.end - rows.start)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_tile_from_the_start_and_keep_their_index_across_a_door() {
        let buf_w = FIRST_WINDOW_X + 4 * (WINDOW_W + WINDOW_GAP) + WINDOW_W + WINDOW_EDGE_MARGIN;
        let all: Vec<_> = window_bays(buf_w, None).collect();
        assert_eq!(
            all.len(),
            5,
            "exactly five windows fit, the last flush with the margin"
        );
        for (k, b) in all.iter().enumerate() {
            assert_eq!(b.idx, k as u16);
            assert_eq!(b.x, FIRST_WINDOW_X + k as u16 * (WINDOW_W + WINDOW_GAP));
            assert_eq!(b.center_x(), b.x + WINDOW_W / 2);
            assert!(b.span().end + WINDOW_EDGE_MARGIN <= buf_w);
        }
        assert_eq!(
            window_bays(buf_w - 1, None).count(),
            4,
            "one column less and the last window no longer clears the margin"
        );

        let doomed = all[2];
        let kept: Vec<_> = window_bays(buf_w, Some(doomed.span())).collect();
        assert_eq!(
            kept.len(),
            4,
            "the door takes exactly the window it overlaps"
        );
        assert!(kept.iter().all(|b| b.idx != doomed.idx));
        assert_eq!(kept[2].idx, 3, "the window after the door keeps its index");
        assert_eq!(window_run(buf_w), all[0].x..all[4].span().end);
    }

    #[test]
    fn a_wall_too_narrow_for_a_window_runs_the_first_window_alone() {
        assert_eq!(window_bays(WINDOW_W, None).count(), 0);
        assert_eq!(
            window_run(WINDOW_W),
            FIRST_WINDOW_X..FIRST_WINDOW_X + WINDOW_W
        );
    }

    #[test]
    fn a_window_runs_from_its_top_row_to_the_trim() {
        let band_h = 30;
        assert_eq!(window_rows(band_h), WINDOW_TOP..wall_trim_row(band_h));
        assert!(
            window_rows(0).is_empty(),
            "a band with no room has no window"
        );
    }

    /// The fit check near `u16::MAX` overflowed a `u16` sum before it was
    /// widened.
    #[test]
    fn the_widest_wall_tiles_every_window_it_fits() {
        let fits = (u16::MAX - FIRST_WINDOW_X - WINDOW_W - WINDOW_EDGE_MARGIN)
            / (WINDOW_W + WINDOW_GAP)
            + 1;
        assert_eq!(window_bays(u16::MAX, None).count(), usize::from(fits));
    }

    /// The door is placed from the window rows, and nothing else pins where it
    /// ends up.
    #[test]
    fn the_door_stands_on_the_wall_trim() {
        for (w, h) in [(192, 80), (140, 60), (250, 90)] {
            let layout = SceneLayout::compute(w, h, Some(4)).expect("layout fits");
            let door = layout.door.expect("a door at this size");
            assert_eq!(
                door.y + super::super::ELEVATOR_H - 1,
                wall_trim_row(layout.wall_band_h()),
                "{w}x{h}"
            );
        }
    }

    #[test]
    fn glass_is_a_bays_cells_off_its_frame() {
        let layout =
            crate::layout::Layout::compute(192, 160, Some(crate::layout::TEST_DEFAULT_DESKS))
                .expect("192x160 fits");
        let rows = window_rows(layout.wall_band_h());
        let h = rows.end - rows.start;
        let bay = layout.window_bays().next().expect("a window");
        for dy in 0..h {
            for dx in 0..WINDOW_W {
                assert_eq!(
                    layout.glass_at(bay.x + dx, rows.start + dy),
                    !window_frame(dx, dy, h),
                    "({dx}, {dy}) of the first bay"
                );
            }
        }
        assert!(
            !layout.glass_at(bay.x + WINDOW_W, rows.start + 2),
            "the pillar past it"
        );
        assert!(!layout.glass_at(bay.x + 2, rows.end), "the trim below it");
    }
}
