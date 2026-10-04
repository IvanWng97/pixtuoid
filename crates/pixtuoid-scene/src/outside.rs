//! What the windows show: everything outside — the sky, the city and the
//! weather on the panes — drawn onto one window's [`WindowView`] at a time on
//! a painter's grid, which holds no cell on the window's joinery.

use std::ops::Range;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::sprite::format::{Density, Pack};

use crate::atmosphere::Moment;
use crate::celestial::SkyView;
use crate::glass_weather::GlassWeather;
use crate::layout::{Bounds, Size, WindowBay, window_frame, window_rows, window_run};
use crate::skyline::CityStrip;
use crate::theme::Theme;

/// One cell of a window's glass on a painter's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    /// Where it stands on the painter's grid, which a dither keys on.
    pub(crate) at: (u16, u16),
    /// Its offset from the glass's top-left ([`WindowBay::glass_box`]).
    pub(crate) glass_offset: (u16, u16),
}

/// One window on a grid `d` cells to the layout unit, and what its glass
/// shows: the only surface a sky effect draws on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WindowView {
    bay: WindowBay,
    /// The window's top row, in units.
    top: u16,
    /// The window's height in units, its frame included.
    h: u16,
    glass_box: Bounds,
    d: u16,
    /// The window's box row by row from its top-left, `None` on its joinery
    /// ([`window_frame`]).
    px: Vec<Option<Rgb>>,
}

impl WindowView {
    /// `bay`'s window over `rows`, on a grid `d` cells to the unit, each glass
    /// cell in `base`.
    pub(crate) fn new(
        bay: WindowBay,
        rows: Range<u16>,
        d: u16,
        mut base: impl FnMut(Cell) -> Rgb,
    ) -> Self {
        let h = rows.end.saturating_sub(rows.start);
        let mut view = Self {
            bay,
            top: rows.start,
            h,
            glass_box: bay.glass_box(rows),
            d: d.max(1),
            px: Vec::new(),
        };
        let size = Size { w: bay.w, h };
        let d = view.d;
        view.px = (0..usize::from(view.cols()) * usize::from(view.rows()))
            .map(|i| {
                let (ax, ay) = view.offset(i);
                let glass = !window_frame(ax / d, ay / d, size);
                glass.then(|| base(view.cell(ax, ay)))
            })
            .collect();
        view
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

    /// Recolour the glass cell `glass_offset` cells from the glass's top-left, as
    /// [`paint`](Self::paint) does.
    pub(crate) fn paint_glass_at(
        &mut self,
        glass_offset: (u16, u16),
        f: impl FnOnce(Cell, Rgb) -> Rgb,
    ) {
        let (ix, iy) = self.inset();
        let (ax, ay) = (
            glass_offset.0.saturating_add(ix),
            glass_offset.1.saturating_add(iy),
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

    /// Its window's [`WindowBay::idx`].
    pub(crate) fn idx(&self) -> u16 {
        self.bay.idx
    }

    /// Its [`WindowBay::glass_box`], in units.
    pub(crate) fn glass_size(&self) -> Size {
        Size {
            w: self.glass_box.width,
            h: self.glass_box.height,
        }
    }

    pub(crate) fn d(&self) -> u16 {
        self.d
    }

    /// Whether its glass shows at `at` on the grid.
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

    /// The glass's top-left, in cells from the window's.
    fn inset(&self) -> (u16, u16) {
        let d = self.d;
        (
            (self.glass_box.x - self.bay.x).saturating_mul(d),
            (self.glass_box.y - self.top).saturating_mul(d),
        )
    }

    /// Cell `i` of [`px`](Self::px), from the window's top-left.
    fn offset(&self, i: usize) -> (u16, u16) {
        let cols = usize::from(self.cols()).max(1);
        // Both below the window's extent, a u16.
        ((i % cols) as u16, (i / cols) as u16)
    }

    fn cell(&self, ax: u16, ay: u16) -> Cell {
        let d = self.d;
        let (ix, iy) = self.inset();
        Cell {
            at: (
                self.bay.x.saturating_mul(d).saturating_add(ax),
                self.top.saturating_mul(d).saturating_add(ay),
            ),
            glass_offset: (ax.saturating_sub(ix), ay.saturating_sub(iy)),
        }
    }
}

/// Everything one frame's windows look out on, the same through every one,
/// on one painter's grid.
pub(crate) struct Outside {
    sky: SkyView,
    city: CityStrip,
    /// The column, in units, the city's west end stands at.
    run_x0: u16,
    weather: GlassWeather,
    rows: Range<u16>,
    d: u16,
}

impl Outside {
    /// The outside at `moment` of a wall `buf_w` wide and `band_h` tall under
    /// `weather`, at `density`.
    pub(crate) fn of(
        moment: &Moment,
        pack: &Pack,
        theme: &Theme,
        (buf_w, band_h): (u16, u16),
        density: Density,
        weather: GlassWeather,
    ) -> Self {
        let rows = window_rows(band_h);
        let run = window_run(buf_w);
        let glass_h = crate::layout::glass_rows(rows.end - rows.start);
        Self {
            sky: SkyView::of(moment, buf_w, band_h, theme),
            city: CityStrip::draw(pack, (run.end - run.start, glass_h), moment, theme, density),
            run_x0: run.start,
            weather,
            rows,
            d: density.get(),
        }
    }

    /// What `bay`'s glass shows: each part of the outside, back to front, in
    /// the one order every painter draws it in.
    pub(crate) fn through(&self, bay: WindowBay) -> WindowView {
        let mut view = self.sky.window(bay, self.rows.clone(), self.d);
        self.city.paint(&mut view, self.run_x0);
        self.weather.paint(&mut view);
        view
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sky::{Sky, Weather};
    use pixtuoid_core::sprite::RgbBuffer;

    /// One sky a painter's frame is drawn under, beside the no-weather sky of
    /// its instant, which its windows show in the frame it is held against.
    pub(crate) struct Weathered {
        pub(crate) name: String,
        pub(crate) now: std::time::SystemTime,
        pub(crate) sky: Sky,
        pub(crate) bare: Sky,
        pub(crate) changes_glass: bool,
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
                        changes_glass: w != Weather::Clear,
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

    /// What a test buffer holds where nothing painted.
    pub(crate) const UNPAINTED: Rgb = Rgb { r: 1, g: 2, b: 3 };

    /// Which pixels of a `w`×`h` buffer some of `pieces` writes, each painted
    /// alone by `paint` and looked for in the rows and columns `within` gives it.
    pub(crate) fn painted_alone<P>(
        (w, h): (u16, u16),
        pieces: impl IntoIterator<Item = P>,
        within: impl Fn(&P) -> (Range<u16>, Range<u16>),
        mut paint: impl FnMut(&P, &mut RgbBuffer),
    ) -> Vec<bool> {
        let mut over = RgbBuffer::filled(w, h, UNPAINTED);
        let mut painted = vec![false; usize::from(w) * usize::from(h)];
        for piece in pieces {
            let epoch = over.begin_writes();
            paint(&piece, &mut over);
            let (xs, ys) = within(&piece);
            for y in ys.start..ys.end.min(h) {
                for x in xs.start..xs.end.min(w) {
                    if over.written_in(x, y, epoch) {
                        painted[usize::from(y) * usize::from(w) + usize::from(x)] = true;
                    }
                }
            }
        }
        over.end_writes();
        painted
    }

    /// Under each of [`every_sky`], a painter's frame matches the one whose
    /// windows show its instant's no-weather sky everywhere but on glass
    /// nothing else paints, and a sky other than a clear one changes some
    /// glass. `frames` paints a sky's frame and that twin, `covered` gives each
    /// pixel something other than a window paints at the sky's instant (see
    /// [`painted_alone`]) and `on_glass` whether a pixel is glass. Returns the
    /// glass pixels something hangs over, for the painter's population guard.
    pub(crate) fn assert_the_outside_reaches_only_the_glass(
        painter: &str,
        (w, h): (u16, u16),
        on_glass: impl Fn(u16, u16) -> bool,
        mut frames: impl FnMut(&Weathered) -> [RgbBuffer; 2],
        mut covered: impl FnMut(&Weathered) -> Vec<bool>,
    ) -> std::collections::HashSet<(u16, u16)> {
        let mut hung_over_glass = std::collections::HashSet::new();
        let mut at_instant: Option<(std::time::SystemTime, Vec<bool>)> = None;
        for weathered in every_sky() {
            let name = format!("{painter}, {}", weathered.name);
            if at_instant
                .as_ref()
                .is_none_or(|(now, _)| *now != weathered.now)
            {
                at_instant = Some((weathered.now, covered(&weathered)));
            }
            let covered = &at_instant.as_ref().expect("filled above").1;
            let [shown, bare] = frames(&weathered);
            let mut glass = 0;
            for y in 0..h {
                for x in 0..w {
                    let hung = covered[usize::from(y) * usize::from(w) + usize::from(x)];
                    if hung && on_glass(x, y) {
                        hung_over_glass.insert((x, y));
                    }
                    if shown.get(x, y) == bare.get(x, y) {
                        continue;
                    }
                    assert!(
                        on_glass(x, y) && !hung,
                        "{name}: the outside reached ({x}, {y}), off the glass"
                    );
                    glass += 1;
                }
            }
            assert_eq!(
                glass > 0,
                weathered.changes_glass,
                "{name}: {glass} glass pixels"
            );
        }
        hung_over_glass
    }

    fn bay() -> WindowBay {
        WindowBay {
            x: 5,
            w: crate::layout::WINDOW_W,
            idx: 0,
        }
    }

    const ROWS: Range<u16> = 1..21;

    /// The view's cells are the glass [`window_frame`] leaves and nothing
    /// else, at every grid density, so its mask is the window geometry's.
    #[test]
    fn a_view_holds_the_glass_and_none_of_the_joinery() {
        let h = ROWS.end - ROWS.start;
        for d in [1, 2, 4] {
            let view = WindowView::new(bay(), ROWS, d, |_| Rgb { r: 1, g: 2, b: 3 });
            let glass: std::collections::HashSet<_> = view.cells().map(|(at, _)| at).collect();
            let joinery: std::collections::HashSet<_> = view.joinery().collect();
            for ay in ROWS.start * d..ROWS.end * d {
                for ax in bay().x * d..bay().span().end * d {
                    let frame = window_frame(
                        ax / d - bay().x,
                        ay / d - ROWS.start,
                        Size { w: bay().w, h },
                    );
                    assert_eq!(glass.contains(&(ax, ay)), !frame, "d={d} ({ax}, {ay})");
                    assert_eq!(joinery.contains(&(ax, ay)), frame, "d={d} ({ax}, {ay})");
                    assert_eq!(view.shows((ax, ay)), !frame, "d={d} ({ax}, {ay})");
                }
            }
        }
    }

    #[test]
    fn no_write_reaches_the_joinery() {
        const INK: Rgb = Rgb { r: 9, g: 9, b: 9 };
        for d in [1, 4] {
            let mut view = WindowView::new(bay(), ROWS, d, |_| Rgb { r: 1, g: 2, b: 3 });
            let joinery: Vec<_> = view.joinery().collect();
            view.paint(|_, _| INK);
            let glass = view.glass_size();
            for gy in 0..(glass.h + 2) * d {
                for gx in 0..(glass.w + 2) * d {
                    view.paint_glass_at((gx, gy), |_, _| INK);
                }
            }
            assert_eq!(view.joinery().collect::<Vec<_>>(), joinery, "d={d}");
            assert!(view.cells().all(|(_, c)| c == INK), "d={d}");
        }
    }

    #[test]
    fn a_glass_offset_counts_from_the_glass() {
        let d = 4;
        let mut view = WindowView::new(bay(), ROWS, d, |_| Rgb { r: 0, g: 0, b: 0 });
        let mut seen = None;
        view.paint_glass_at((0, 0), |cell, c| {
            seen = Some(cell);
            c
        });
        let glass = bay().glass_box(ROWS);
        assert_eq!(
            seen,
            Some(Cell {
                at: (glass.x * d, glass.y * d),
                glass_offset: (0, 0),
            })
        );
    }

    #[test]
    fn a_window_shows_the_city_strip_from_its_own_column() {
        // Overcast noon: no stars and no disc, which key on the screen column,
        // not the city's; the sky's dither does too, so the far pane sits a
        // whole number of its periods east.
        let now = crate::localclock::on_day(15, 12);
        let theme = &crate::theme::NORMAL;
        let moment = Moment::resolve(
            Sky::at_with(now, Weather::Overcast),
            theme,
            0.0,
            crate::anim::Motion::Full.timing(now),
        );
        let dx = 7;
        let far = (crate::layout::WINDOW_W + dx).next_multiple_of(crate::dither::PERIOD);
        let mut outside = Outside::of(
            &moment,
            &crate::pack::test_default_pack(),
            theme,
            (crate::layout::WINDOW_W * 3, 32),
            Density::ONE,
            GlassWeather::of(&moment),
        );
        let mut pane = |x: u16, run_x0: u16| {
            outside.run_x0 = run_x0;
            let bay = WindowBay {
                x,
                w: crate::layout::WINDOW_W,
                idx: 0,
            };
            outside
                .through(bay)
                .cells()
                .map(|(_, c)| c)
                .collect::<Vec<_>>()
        };
        let (west, east) = (pane(0, 0), pane(far, far));
        assert_eq!(west, east, "a pane shows the strip from the run's west end");
        assert_ne!(
            pane(dx, 0),
            west,
            "a pane {dx} columns east shows a different stretch of city"
        );
    }
}
