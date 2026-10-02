//! What the windows show, held to their glass: everything outside — the sky,
//! the city and the weather on the panes — is drawn onto one window's
//! [`SkyLayer`] at a time, on a painter's grid, and the layer has no cell on
//! the window's joinery for anything to land on.

use std::ops::Range;

use pixtuoid_core::sprite::Rgb;

use crate::celestial::SkyView;
use crate::glass_weather::GlassWeather;
use crate::layout::{Size, WindowBay, glass_rows, window_frame};
use crate::skyline::CityStrip;

/// One cell of a window's glass on a painter's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    /// Where it stands on the painter's grid, which a dither keys on.
    pub(crate) at: (u16, u16),
    /// Its offset from the glass's top-left, inside the window's frame.
    pub(crate) glass: (u16, u16),
}

/// One window's glass on a grid `d` cells to the layout unit, showing what
/// lies outside: the only surface a sky effect draws on. Only the glass
/// [`window_frame`] leaves holds a cell; the frame, mullion and transom hold
/// none, so no write reaches them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SkyLayer {
    bay: WindowBay,
    /// The window's top row, in units.
    top: u16,
    /// The window's height in units, its frame included.
    h: u16,
    d: u16,
    /// The window's box row by row from its top-left, `None` on its joinery.
    px: Vec<Option<Rgb>>,
}

impl SkyLayer {
    /// `bay`'s glass over the window `rows`, on a grid `d` cells to the unit,
    /// each cell in `base`.
    pub(crate) fn new(
        bay: WindowBay,
        rows: Range<u16>,
        d: u16,
        mut base: impl FnMut(Cell) -> Rgb,
    ) -> Self {
        let h = rows.end.saturating_sub(rows.start);
        let mut layer = Self {
            bay,
            top: rows.start,
            h,
            d: d.max(1),
            px: Vec::new(),
        };
        let size = Size { w: bay.w, h };
        let d = layer.d;
        layer.px = (0..usize::from(layer.cols()) * usize::from(layer.rows()))
            .map(|i| {
                let (ax, ay) = layer.offset(i);
                let glass = !window_frame(ax / d, ay / d, size);
                glass.then(|| base(layer.cell(ax, ay)))
            })
            .collect();
        layer
    }

    /// Recolour every glass cell: `f` takes the cell and what it shows.
    pub(crate) fn paint(&mut self, mut f: impl FnMut(Cell, Rgb) -> Rgb) {
        for i in 0..self.px.len() {
            let (ax, ay) = self.offset(i);
            let cell = self.cell(ax, ay);
            if let Some(c) = &mut self.px[i] {
                *c = f(cell, *c);
            }
        }
    }

    /// Recolour the cell at `glass` from the glass's top-left, as
    /// [`paint`](Self::paint) does; one on the joinery or past the window
    /// takes nothing.
    pub(crate) fn paint_at(&mut self, glass: (u16, u16), f: impl FnOnce(Cell, Rgb) -> Rgb) {
        let (ax, ay) = (
            glass.0.saturating_add(self.d),
            glass.1.saturating_add(self.d),
        );
        if ax >= self.cols() || ay >= self.rows() {
            return;
        }
        let cell = self.cell(ax, ay);
        let i = usize::from(ay) * usize::from(self.cols()) + usize::from(ax);
        if let Some(Some(c)) = self.px.get_mut(i) {
            *c = f(cell, *c);
        }
    }

    /// The window's place in the run, counted from the west.
    pub(crate) fn idx(&self) -> u16 {
        self.bay.idx
    }

    /// The glass inside the window's frame, in units: its mullion and transom
    /// included.
    pub(crate) fn glass(&self) -> Size {
        Size {
            w: self.bay.w.saturating_sub(2),
            h: glass_rows(self.h),
        }
    }

    /// Cells to the layout unit.
    pub(crate) fn d(&self) -> u16 {
        self.d
    }

    /// Whether the glass shows at `at` on the grid: inside the window and off
    /// its joinery.
    pub(crate) fn shows(&self, at: (u16, u16)) -> bool {
        let d = self.d;
        let (Some(ax), Some(ay)) = (
            at.0.checked_sub(self.bay.x.saturating_mul(d)),
            at.1.checked_sub(self.top.saturating_mul(d)),
        ) else {
            return false;
        };
        ax < self.cols()
            && ay < self.rows()
            && self.px[usize::from(ay) * usize::from(self.cols()) + usize::from(ax)].is_some()
    }

    /// Each glass cell's place on the grid, and what it shows.
    pub(crate) fn cells(&self) -> impl Iterator<Item = ((u16, u16), Rgb)> + '_ {
        (0..self.px.len()).filter_map(|i| {
            let (ax, ay) = self.offset(i);
            self.px[i].map(|c| (self.cell(ax, ay).at, c))
        })
    }

    /// Each joinery cell's place on the grid.
    pub(crate) fn joinery(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        (0..self.px.len())
            .filter(|&i| self.px[i].is_none())
            .map(|i| {
                let (ax, ay) = self.offset(i);
                self.cell(ax, ay).at
            })
    }

    fn cols(&self) -> u16 {
        self.bay.w.saturating_mul(self.d)
    }

    fn rows(&self) -> u16 {
        self.h.saturating_mul(self.d)
    }

    /// Cell `i` of [`px`](Self::px), from the window's top-left.
    fn offset(&self, i: usize) -> (u16, u16) {
        let cols = usize::from(self.cols()).max(1);
        // Both below the window's extent, a u16.
        ((i % cols) as u16, (i / cols) as u16)
    }

    /// The cell `(ax, ay)` from the window's top-left.
    fn cell(&self, ax: u16, ay: u16) -> Cell {
        let d = self.d;
        Cell {
            at: (
                self.bay.x.saturating_mul(d).saturating_add(ax),
                self.top.saturating_mul(d).saturating_add(ay),
            ),
            glass: (ax.saturating_sub(d), ay.saturating_sub(d)),
        }
    }
}

/// Everything one frame's windows look out on, the same through every one.
#[derive(Clone, Copy)]
pub(crate) struct Outside<'a> {
    pub(crate) sky: &'a SkyView,
    /// The city along the whole run of windows.
    pub(crate) city: &'a CityStrip,
    /// The column, in units, the city's west end stands at.
    pub(crate) run_x0: u16,
    pub(crate) weather: &'a GlassWeather,
}

impl Outside<'_> {
    /// What `bay`'s glass shows over the window `rows`, on a grid `d` cells to
    /// the unit: each layer of the outside, back to front, in the one order
    /// every painter draws it in.
    pub(crate) fn through(&self, bay: WindowBay, rows: Range<u16>, d: u16) -> SkyLayer {
        let mut layer = self.sky.layer(bay, rows, d);
        self.city.paint(&mut layer, self.run_x0);
        self.weather.paint(&mut layer);
        layer
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sky::{Sky, Weather};

    /// One sky a painter's frame is drawn under, beside the no-weather sky of
    /// its instant, which its windows show in the frame it is held against.
    pub(crate) struct Weathered {
        pub(crate) name: String,
        pub(crate) now: std::time::SystemTime,
        pub(crate) sky: Sky,
        pub(crate) bare: Sky,
        /// Whether its outside differs from the bare one's.
        pub(crate) shows: bool,
    }

    /// Every weather by day, at sunset and by night, a storm mid-strike too.
    pub(crate) fn every_sky() -> Vec<Weathered> {
        let mut skies = Vec::new();
        for hour in [12, 19, 23] {
            let now = crate::localclock::at_hour(hour);
            for w in Weather::ALL {
                let strikes = if w == Weather::Storm {
                    [0.0, 1.0].as_slice()
                } else {
                    &[0.0]
                };
                for &flash in strikes {
                    skies.push(Weathered {
                        name: format!("{w:?} at {hour}h, flash {flash}"),
                        now,
                        sky: Sky::at_with(now, w).with_flash(flash),
                        bare: Sky::at_with(now, Weather::Clear),
                        shows: w != Weather::Clear,
                    });
                }
            }
        }
        assert!(
            skies.iter().any(|s| {
                crate::atmosphere::SkyTones::resolve(&s.sky, &crate::theme::NORMAL).golden_hour
                    > 0.1
            }),
            "the sweep reaches a sunset's blaze"
        );
        skies
    }

    fn bay() -> WindowBay {
        WindowBay {
            x: 5,
            w: crate::layout::WINDOW_W,
            idx: 0,
        }
    }

    const ROWS: Range<u16> = 1..21;

    /// The layer's cells are the glass [`window_frame`] leaves and nothing
    /// else, at every grid density, so its mask is the window geometry's.
    #[test]
    fn a_layer_holds_the_glass_and_none_of_the_joinery() {
        let h = ROWS.end - ROWS.start;
        for d in [1, 2, 4] {
            let layer = SkyLayer::new(bay(), ROWS, d, |_| Rgb { r: 1, g: 2, b: 3 });
            let glass: std::collections::HashSet<_> = layer.cells().map(|(at, _)| at).collect();
            let joinery: std::collections::HashSet<_> = layer.joinery().collect();
            for ay in ROWS.start * d..ROWS.end * d {
                for ax in bay().x * d..bay().span().end * d {
                    let frame = window_frame(
                        ax / d - bay().x,
                        ay / d - ROWS.start,
                        Size { w: bay().w, h },
                    );
                    assert_eq!(glass.contains(&(ax, ay)), !frame, "d={d} ({ax}, {ay})");
                    assert_eq!(joinery.contains(&(ax, ay)), frame, "d={d} ({ax}, {ay})");
                }
            }
        }
    }

    /// No write path reaches the joinery: painting every cell, and painting
    /// at every offset the glass and its surround span, leaves it empty.
    #[test]
    fn no_write_reaches_the_joinery() {
        const INK: Rgb = Rgb { r: 9, g: 9, b: 9 };
        for d in [1, 4] {
            let mut layer = SkyLayer::new(bay(), ROWS, d, |_| Rgb { r: 1, g: 2, b: 3 });
            let joinery: Vec<_> = layer.joinery().collect();
            layer.paint(|_, _| INK);
            let glass = layer.glass();
            for gy in 0..(glass.h + 2) * d {
                for gx in 0..(glass.w + 2) * d {
                    layer.paint_at((gx, gy), |_, _| INK);
                }
            }
            assert_eq!(layer.joinery().collect::<Vec<_>>(), joinery, "d={d}");
            assert!(layer.cells().all(|(_, c)| c == INK), "d={d}");
        }
    }

    /// A cell's glass offset counts from the glass's top-left, inside the
    /// window's one-unit frame.
    #[test]
    fn a_glass_offset_counts_from_inside_the_frame() {
        let d = 4;
        let mut layer = SkyLayer::new(bay(), ROWS, d, |_| Rgb { r: 0, g: 0, b: 0 });
        let mut seen = None;
        layer.paint_at((0, 0), |cell, c| {
            seen = Some(cell);
            c
        });
        assert_eq!(
            seen,
            Some(Cell {
                at: ((bay().x + 1) * d, (ROWS.start + 1) * d),
                glass: (0, 0),
            })
        );
    }
}
