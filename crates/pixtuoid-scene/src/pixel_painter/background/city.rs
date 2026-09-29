//! The classic painter's city: the shared [`Skyline`] drawn once a frame at 1x
//! into a strip as wide as the window run, which each window shows its part of.

use std::time::SystemTime;

use pixtuoid_core::sprite::format::{Material, Pack};
use pixtuoid_core::sprite::{Pixel, Rgb};

use crate::atmosphere::Look;
use crate::skyline::{block_window, lit, Plane, PlaneColours, Skyline, Stand};
use crate::theme::Theme;

/// The city behind a window run at 1x, `None` where the sky shows.
pub(super) struct CityStrip {
    w: u16,
    h: u16,
    px: Vec<Option<Rgb>>,
}

impl CityStrip {
    /// `pack`'s city across a run `run_w` wide behind glass `glass_h` tall,
    /// seen from `altitude`, under `look`'s sky at `now`.
    pub(super) fn draw(
        pack: &Pack,
        (run_w, glass_h): (u16, u16),
        altitude: f32,
        look: &Look,
        theme: &Theme,
        now: SystemTime,
    ) -> Self {
        let mut strip = CityStrip {
            w: run_w,
            h: glass_h,
            px: vec![None; usize::from(run_w) * usize::from(glass_h)],
        };
        let colours = Plane::ALL.map(|p| PlaneColours::of(p, look, theme));
        for (plane, stand) in Skyline::of(pack, run_w, glass_h, altitude).stands() {
            let c = &colours[plane.index()];
            let window = |i: usize| {
                lit(plane, stand.hash(), i, look.darkness, now)
                    .map_or(c.material(Material::Glass), |hue| c.lit[hue])
            };
            match stand {
                Stand::Block { x, w, top, .. } => {
                    for dy in 0..i32::from(glass_h) - top {
                        for dx in 0..w {
                            let (wx, wy) = (dx, u16::try_from(dy).unwrap_or(u16::MAX));
                            let colour = if block_window(wx, wy) {
                                window(usize::from(wy) * usize::from(w) + usize::from(wx))
                            } else {
                                c.material(Material::Facade)
                            };
                            strip.put(x + i32::from(dx), top + dy, colour);
                        }
                    }
                }
                Stand::Kit {
                    building, x, top, ..
                } => {
                    let (Some(materials), Some(art)) = (pack.city_materials(), building.art(1))
                    else {
                        continue;
                    };
                    let overrides: Vec<(char, Pixel)> = Material::ALL
                        .into_iter()
                        .map(|m| (materials.key(m), Some(c.material(m))))
                        .collect();
                    let Some(frame) = art.sprite().recolorable(0).map(|f| f.recolored(&overrides))
                    else {
                        continue;
                    };
                    for fy in 0..frame.height() {
                        for fx in 0..frame.width() {
                            if let Some(colour) = frame.get(fx, fy).copied().flatten() {
                                strip.put(x + i32::from(fx), top + i32::from(fy), colour);
                            }
                        }
                    }
                    for (i, pane) in art.windows().iter().enumerate() {
                        let colour = window(i);
                        for &(wx, wy) in pane {
                            strip.put(x + i32::from(wx), top + i32::from(wy), colour);
                        }
                    }
                }
            }
        }
        strip
    }

    fn put(&mut self, x: i32, y: i32, colour: Rgb) {
        if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
            if x < self.w && y < self.h {
                self.px[usize::from(y) * usize::from(self.w) + usize::from(x)] = Some(colour);
            }
        }
    }

    /// What stands at `(x, y)` from the run's west end and the glass's top, or
    /// `None` where the sky shows.
    pub(super) fn at(&self, x: u16, y: u16) -> Option<Rgb> {
        (x < self.w && y < self.h)
            .then(|| self.px[usize::from(y) * usize::from(self.w) + usize::from(x)])
            .flatten()
    }
}
