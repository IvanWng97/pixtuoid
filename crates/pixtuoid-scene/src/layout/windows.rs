//! The north wall's floor-to-ceiling windows: where each one stands, in the
//! layout's own units, so every painter tiles the same windows and the elevator
//! door stands in the last one's slot, on their bottom line.

use std::ops::Range;

use super::{Bounds, CLOCK, ELEVATOR_H, ELEVATOR_W, NEON_PANEL, SceneLayout};

/// A window's width, frame included — fixed, so the skyline detail reads the
/// same on every terminal.
pub(crate) const WINDOW_W: u16 = 22;
/// The narrowest post between two window slots.
const MIN_POST_W: u16 = 3;
/// The run's west end: the neon sign hangs on the plain wall west of it.
const NEON_EAST: u16 = NEON_PANEL.x + NEON_PANEL.width;
/// The door's inset from its slot's west edge, centring it in the slot.
const DOOR_INSET: u16 = (WINDOW_W - ELEVATOR_W) / 2;
/// The wall east of the door's slot.
const WINDOW_EDGE_MARGIN: u16 = 2;
/// The windows' top frame row; one wall row stands above it.
pub(crate) const WINDOW_TOP: u16 = 1;

/// One window on the north wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowBay {
    /// The window's left edge.
    pub(crate) x: u16,
    /// The window's place in the run, counted from the west.
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

    /// The glass columns of its two panes, west then east of the centre
    /// mullion ([`window_frame`]).
    pub(crate) fn panes(self) -> [Range<u16>; 2] {
        let mullion = self.x + WINDOW_W / 2;
        [self.x + 1..mullion, mullion + 1..self.span().end - 1]
    }
}

/// The door's slot's left edge on a wall `buf_w` wide: flush with the east
/// margin.
fn door_slot_x(buf_w: u16) -> Option<u16> {
    buf_w.checked_sub(WINDOW_EDGE_MARGIN + WINDOW_W)
}

/// Every window slot a wall `buf_w` wide fits, left to right, ending in the
/// door's: as many windows as leave every post at least [`MIN_POST_W`], with
/// the wall between the neon and the door's slot shared out between the
/// posts ([`slot_gaps`]).
pub(crate) fn window_slots(buf_w: u16) -> impl Iterator<Item = WindowBay> {
    let mut x = NEON_EAST;
    slot_gaps(buf_w)
        .into_iter()
        .zip(0..)
        .map(move |(gap, idx)| {
            x += gap;
            let slot = WindowBay { x, idx };
            x += WINDOW_W;
            slot
        })
}

/// The wall west of each slot: the neon's gap, then the post before each
/// slot after the first. Shared out evenly, the easternmost taking the
/// columns that don't divide — but where no post between two slots is as
/// wide as the [`CLOCK`], the one nearest the wall's middle is widened to
/// hang it on, from the spare wall, if that leaves the rest [`MIN_POST_W`].
fn slot_gaps(buf_w: u16) -> Vec<u16> {
    let Some(span) = door_slot_x(buf_w).and_then(|x| x.checked_sub(NEON_EAST)) else {
        return Vec::new();
    };
    let windows = span.saturating_sub(MIN_POST_W) / (WINDOW_W + MIN_POST_W);
    let wall = span - windows * WINDOW_W;
    let even = spread(wall, windows + 1);
    let has_clock_post = even.iter().skip(1).any(|&g| g >= CLOCK.w);
    if windows == 0 || has_clock_post || wall < CLOCK.w + windows * MIN_POST_W {
        return even;
    }
    let middle = buf_w / 2;
    let mut west = NEON_EAST;
    let nearest = even
        .iter()
        .enumerate()
        .map(|(k, &gap)| {
            let centre = west + gap / 2;
            west += gap + WINDOW_W;
            (k, centre)
        })
        .skip(1)
        .min_by_key(|&(_, centre)| centre.abs_diff(middle))
        .map_or(1, |(k, _)| k);
    let mut gaps = spread(wall - CLOCK.w, windows);
    gaps.insert(nearest, CLOCK.w);
    gaps
}

/// `wall` shared out between `n` gaps, the easternmost taking the columns
/// that don't divide.
fn spread(wall: u16, n: u16) -> Vec<u16> {
    let (base, narrow) = (wall / n, n - wall % n);
    (0..n).map(|k| base + u16::from(k >= narrow)).collect()
}

/// The windows a wall `buf_w` wide shows, left to right: the tiling less every
/// window `door` overlaps, whose glass would otherwise show through the
/// elevator's frame.
pub(crate) fn window_bays(buf_w: u16, door: Range<u16>) -> impl Iterator<Item = WindowBay> {
    window_slots(buf_w).filter(move |b| !(b.x < door.end && b.x + WINDOW_W > door.start))
}

/// The columns from the first slot's left edge to the last one's right, the
/// door's included; one window's width past the narrowest post when none fits.
pub(crate) fn window_run(buf_w: u16) -> Range<u16> {
    let mut slots = window_slots(buf_w);
    let first = slots.next().map_or(NEON_EAST + MIN_POST_W, |s| s.x);
    let last = slots.last().map_or(first, |s| s.x);
    first..last + WINDOW_W
}

/// The frame posts between neighbouring window slots, left to right.
pub(crate) fn window_posts(buf_w: u16) -> impl Iterator<Item = Range<u16>> {
    window_slots(buf_w)
        .zip(window_slots(buf_w).skip(1))
        .map(|(west, east)| west.span().end..east.x)
}

/// The elevator door's left column on a wall `buf_w` wide: centred in the
/// door's slot, which stands even where no window run fits west of it.
pub(crate) fn door_x(buf_w: u16) -> u16 {
    buf_w.saturating_sub(WINDOW_EDGE_MARGIN + WINDOW_W) + DOOR_INSET
}

/// The narrowest wall a door's slot fits, so [`door_x`] never saturates on a
/// laid-out office.
pub(crate) const DOOR_SLOT_MIN_W: u16 = WINDOW_EDGE_MARGIN + WINDOW_W;

/// The wall band's trim row, where the band meets the floor.
pub(crate) const fn wall_trim_row(band_h: u16) -> u16 {
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
    /// The windows this office's north wall shows, left to right: every slot
    /// but the door's.
    pub(crate) fn window_bays(&self) -> impl Iterator<Item = WindowBay> + use<> {
        let door = self.door_rect();
        window_bays(self.buf_w, door.x..door.x + door.width)
    }

    /// The box the elevator door's art covers.
    pub(crate) fn door_rect(&self) -> Bounds {
        Bounds {
            x: self.door.x,
            y: self.door.y,
            width: ELEVATOR_W,
            height: ELEVATOR_H,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_share_the_wall_between_the_neon_and_the_door_slot_evenly() {
        let mut walls_with_a_window = 0;
        for buf_w in 0..400 {
            let slots: Vec<_> = window_slots(buf_w).collect();
            let Some(door) = slots.last() else {
                assert!(door_slot_x(buf_w).is_none_or(|x| x < NEON_EAST), "{buf_w}");
                continue;
            };
            assert_eq!(
                Some(door.x),
                door_slot_x(buf_w),
                "{buf_w}: the door's slot ends it"
            );
            let gaps: Vec<u16> = std::iter::once(NEON_EAST)
                .chain(slots.iter().map(|s| s.span().end))
                .zip(slots.iter().map(|s| s.x))
                .map(|(west, east)| east - west)
                .collect();
            let mut rest = gaps.clone();
            if let Some(k) = rest.iter().skip(1).position(|&g| g == CLOCK.w) {
                rest.remove(k + 1);
            }
            let (min, max) = (rest.iter().min(), rest.iter().max());
            assert!(
                max.zip(min).is_none_or(|(a, b)| a - b <= 1),
                "{buf_w}: {gaps:?} evenly, but a clock's post"
            );
            for (k, s) in slots.iter().enumerate() {
                assert_eq!(usize::from(s.idx), k);
            }
            let windows = slots.len() as u16 - 1;
            if windows > 0 {
                walls_with_a_window += 1;
                assert!(gaps.iter().all(|&g| g >= MIN_POST_W), "{buf_w}: {gaps:?}");
            }
            let one_more = (windows + 1) * WINDOW_W + (windows + 2) * MIN_POST_W;
            assert!(
                NEON_EAST + one_more > door.x,
                "{buf_w}: another window fits"
            );
            assert_eq!(window_run(buf_w), slots[0].x..door.span().end);
            assert_eq!(door_x(buf_w), door.x + DOOR_INSET);
        }
        assert!(walls_with_a_window > 0);
    }

    #[test]
    fn a_door_takes_exactly_the_window_it_overlaps() {
        let all: Vec<_> = window_bays(240, 0..0).collect();
        let doomed = all[2];
        let kept: Vec<_> = window_bays(240, doomed.span()).collect();
        assert_eq!(kept.len() + 1, all.len());
        assert!(kept.iter().all(|b| b.idx != doomed.idx));
    }

    #[test]
    fn a_post_fills_each_gap_between_window_slots() {
        let buf_w = 240;
        let slots: Vec<_> = window_slots(buf_w).collect();
        let posts: Vec<_> = window_posts(buf_w).collect();
        assert_eq!(posts.len() + 1, slots.len());
        for (post, pair) in posts.iter().zip(slots.windows(2)) {
            assert_eq!(*post, pair[0].span().end..pair[1].x);
        }
    }

    #[test]
    fn a_pane_is_the_glass_between_two_frame_columns() {
        let bay = WindowBay { x: 10, idx: 0 };
        let (h, glass_row) = (20, 5);
        let frame = |x: u16| window_frame(x - bay.x, glass_row, h);
        for pane in bay.panes() {
            assert!(pane.clone().all(|x| !frame(x)), "{pane:?}");
            assert!(frame(pane.start - 1) && frame(pane.end), "{pane:?}");
        }
    }

    #[test]
    fn a_wall_too_narrow_for_a_window_runs_the_first_window_alone() {
        assert_eq!(window_bays(WINDOW_W, 0..0).count(), 0);
        let first = NEON_EAST + MIN_POST_W;
        assert_eq!(window_run(WINDOW_W), first..first + WINDOW_W);
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

    #[test]
    fn the_widest_wall_tiles_to_its_door_slot() {
        assert_eq!(
            window_slots(u16::MAX).last().map(|s| s.x),
            door_slot_x(u16::MAX)
        );
    }

    /// The door is placed from the window rows, and nothing else pins where it
    /// ends up.
    #[test]
    fn the_door_stands_on_the_wall_trim() {
        for (w, h) in [(192, 80), (140, 60), (250, 90)] {
            let layout = SceneLayout::compute(w, h, Some(4)).expect("layout fits");
            let door = layout.door;
            assert_eq!(
                door.y + super::super::ELEVATOR_H - 1,
                wall_trim_row(layout.wall_band_h()),
                "{w}x{h}"
            );
        }
    }
}
