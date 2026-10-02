//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, lamp halo,
//! floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass, in the order the
//! orchestrator (`pixel_painter/mod.rs`) calls it; the backdrop fixtures
//! among it (clock, runner, mats) in roster order.

mod ground_wash;
mod lighting;

use crate::celestial::SkyView;
pub(super) use ground_wash::paint_ground_wash;
pub(super) use lighting::{
    paint_clock, paint_corridor_runner, paint_light, paint_neon_halo, paint_neon_panel,
    paint_shadows,
};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::palette::{RgbLut, WHITE, blend_rgb};

use crate::atmosphere::Moment;
use crate::glass_weather::GlassWeather;
use crate::layout::{WindowBay, glass_rows, wall_trim_row, window_posts, window_rows, window_run};
use crate::sky::Sky;
use crate::sky_layer::Outside;
use crate::skyline::CityStrip;
use crate::theme::Theme;

/// How far a strike's peak washes the window glass white.
const BOLT_ALPHA: f32 = 0.6;

/// Room-wide ambient bounce from a lightning strike, at [`Sky::flash`].
pub(super) fn paint_lightning_flash(buf: &mut RgbBuffer, sky: &Sky) {
    let level = sky.flash();
    if level <= 0.0 {
        return;
    }
    let alpha = 0.20 * level;
    let lut = RgbLut::tabulate(|c| blend_rgb(c, WHITE, alpha));
    for px in buf.as_mut_slice() {
        *px = lut.apply(*px);
    }
}

/// The base fill's complete input set. Every value the fill loops read is a
/// named field here; a stale hit is invisible to every other gate, so this
/// key IS the correctness boundary — a new input into the fill loops must
/// join it.
#[derive(PartialEq)]
struct BaseFillKey {
    buf_w: u16,
    buf_h: u16,
    band_h: u16,
    carpet: [Rgb; 3],
    wall: Rgb,
}

/// Memoized carpet-noise + wall-band base fill — the full-buffer fills were
/// the largest single cost of a frame (#900's profile) yet their inputs
/// ([`BaseFillKey`]) change only on a resize, theme swap, or weather-tint
/// change.
pub(crate) struct BaseFillCache {
    key: Option<BaseFillKey>,
    filled: RgbBuffer,
}

impl BaseFillCache {
    /// Empty cache — no fill retained yet.
    pub(crate) fn new() -> Self {
        Self {
            key: None,
            filled: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
        }
    }

    /// Stamp the memoized fill over `buf`, refilling on any key change.
    fn blit_into(&mut self, buf: &mut RgbBuffer, key: BaseFillKey) {
        if self.key.as_ref() != Some(&key) {
            self.filled.resize_fill(key.buf_w, key.buf_h, key.wall);
            for y in key.band_h..key.buf_h {
                for x in 0..key.buf_w {
                    let hash = (x as u32)
                        .wrapping_mul(73)
                        .wrapping_add((y as u32).wrapping_mul(151))
                        ^ ((x as u32).wrapping_mul(11) ^ (y as u32).wrapping_mul(37));
                    let color = match hash % 17 {
                        0 | 1 => key.carpet[0],
                        2 | 3 => key.carpet[1],
                        _ => key.carpet[2],
                    };
                    self.filled.put(x, y, color);
                }
            }
            self.key = Some(key);
        }
        debug_assert_eq!(buf.as_slice().len(), self.filled.as_slice().len());
        buf.as_mut_slice().copy_from_slice(self.filled.as_slice());
    }
}

/// The ground and the north wall band `top_wall_h` tall, the posts between
/// its windows included, over the whole of `buf`, at `moment`.
pub(super) fn paint_ground_and_walls(
    base_fill: &mut BaseFillCache,
    buf: &mut RgbBuffer,
    top_wall_h: u16,
    moment: &Moment,
    theme: &Theme,
) {
    let (buf_w, buf_h) = (buf.width(), buf.height());
    let look = &moment.look;
    let window_frame = theme.surface.window_frame;
    let carpet_base = theme.surface.carpet_base;
    let carpet_light = theme.surface.carpet_light;
    let carpet_dark = theme.surface.carpet_dark;
    let wall = theme.surface.wall;
    let wall_trim_color = theme.surface.wall_trim;

    let (tint, share) = look.ground_tint;

    // The noise picks one of THREE colours and the tint is fixed for the frame,
    // so resolve the blend once, not per pixel.
    let carpet = [
        blend_rgb(carpet_light, tint, share),
        blend_rgb(carpet_dark, tint, share),
        blend_rgb(carpet_base, tint, share),
    ];
    base_fill.blit_into(
        buf,
        BaseFillKey {
            buf_w,
            buf_h,
            band_h: top_wall_h.min(buf_h),
            carpet,
            wall,
        },
    );

    let rows = window_rows(top_wall_h);
    for post in window_posts(buf_w) {
        for x in post {
            for y in rows.clone() {
                buf.put_checked(x, y, window_frame);
            }
        }
    }

    let trim_y = wall_trim_row(top_wall_h);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, wall_trim_color);
        }
    }
}

/// The windows `bays` on a wall band `top_wall_h` tall: what each one's glass
/// looks out on at `moment`, its frame over it.
pub(super) fn paint_windows(
    buf: &mut RgbBuffer,
    top_wall_h: u16,
    bays: impl IntoIterator<Item = WindowBay>,
    moment: &Moment,
    pack: &Pack,
    theme: &Theme,
) {
    let buf_w = buf.width();
    let rows = window_rows(top_wall_h);
    let run = window_run(buf_w);
    let city = CityStrip::draw(
        pack,
        (run.end - run.start, glass_rows(rows.end - rows.start)),
        moment,
        theme,
        pixtuoid_core::sprite::format::Density::ONE,
    );
    let outside = Outside {
        sky: &SkyView::of(moment, buf_w, top_wall_h, theme),
        city: &city,
        run_x0: run.start,
        weather: &GlassWeather::of(moment),
    };
    // The bolt, the strike's source, lights the glass in lockstep with
    // `paint_lightning_flash`, over all it shows.
    let bolt = BOLT_ALPHA * moment.sky.flash();
    for bay in bays {
        let mut view = outside.through(bay, rows.clone(), 1);
        if bolt > 0.0 {
            view.paint(|_, c| blend_rgb(c, WHITE, bolt));
        }
        for ((x, y), c) in view.cells() {
            buf.put_checked(x, y, c);
        }
        for (x, y) in view.joinery() {
            buf.put_checked(x, y, theme.surface.window_frame);
        }
    }
}

#[cfg(test)]
mod tests;
