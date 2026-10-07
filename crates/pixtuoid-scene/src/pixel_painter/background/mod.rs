//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, lamp halo,
//! floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass, in the order the
//! orchestrator (`pixel_painter/mod.rs`) calls it; the backdrop fixtures
//! among it (clock, runner, mats) in roster order.

mod ground_wash;
mod lighting;

pub(super) use ground_wash::paint_ground_wash;
pub(super) use lighting::{
    paint_clock, paint_corridor_runner, paint_light, paint_neon_panel, paint_shadows,
};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::palette::{RgbLut, WHITE, blend_rgb};

use crate::atmosphere::Moment;
use crate::dither::Dithered;
use crate::glass_weather::GlassWeather;
use crate::layout::{WindowBay, wall_trim_row, window_posts, window_rows};
use crate::sky::Sky;
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

/// The base fill's complete input set: the [`CachedLayer`](crate::cached_layer::CachedLayer)
/// key, and so its correctness boundary.
#[derive(Debug, PartialEq)]
struct BaseFillKey {
    band_h: u16,
    carpet: Dithered<[Rgb; 3]>,
    wall: Rgb,
}

/// Memoized carpet-noise + wall-band base fill — the full-buffer fills were
/// the largest single cost of a frame (#900's profile) yet their inputs
/// ([`BaseFillKey`]) change only on a resize, theme swap, or weather-tint
/// change.
#[derive(Debug, Default)]
pub(crate) struct BaseFillCache(crate::cached_layer::CachedLayer<BaseFillKey>);

impl BaseFillCache {
    /// Empty cache — no fill retained yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Stamp the memoized fill over `buf`, refilling on any key change.
    fn blit_into(&mut self, buf: &mut RgbBuffer, key: BaseFillKey) {
        let fill = |key: &BaseFillKey, filled: &mut RgbBuffer| {
            let BaseFillKey {
                band_h,
                carpet,
                wall,
            } = *key;
            let (buf_w, buf_h) = (filled.width(), filled.height());
            filled.resize_fill(buf_w, buf_h, wall);
            for y in band_h..buf_h {
                for x in 0..buf_w {
                    let hash = u32::from(x)
                        .wrapping_mul(73)
                        .wrapping_add(u32::from(y).wrapping_mul(151))
                        ^ (u32::from(x).wrapping_mul(11) ^ u32::from(y).wrapping_mul(37));
                    let carpet = carpet.at(x, y);
                    let color = match hash % 17 {
                        0 | 1 => carpet[0],
                        2 | 3 => carpet[1],
                        _ => carpet[2],
                    };
                    filled.put(x, y, color);
                }
            }
        };
        self.0.stamp(key, fill, buf);
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
    let wall = theme.surface.wall;
    let wall_trim_color = theme.surface.wall_trim;

    let carpet = look.carpet(theme).map(|c| [c.lit, c.dark, c.base]);
    base_fill.blit_into(
        buf,
        BaseFillKey {
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

/// The windows `bays` on a wall band `top_wall_h` tall, at `moment`.
pub(super) fn paint_windows(
    buf: &mut RgbBuffer,
    top_wall_h: u16,
    bays: impl IntoIterator<Item = WindowBay>,
    moment: &Moment,
    pack: &Pack,
    theme: &Theme,
    outside: &mut crate::outside::OutsideCache,
) {
    let wall = crate::outside::Wall {
        size: (buf.width(), top_wall_h),
        bays: bays.into_iter().collect(),
    };
    let views = outside.views(
        moment,
        pack,
        theme,
        wall,
        pixtuoid_core::sprite::format::Density::ONE,
        GlassWeather::of(moment),
    );
    // The bolt, the strike's source, lights the glass in lockstep with
    // `paint_lightning_flash`, over all it shows.
    let bolt = BOLT_ALPHA * moment.sky.flash();
    for (_, mut view) in views {
        if bolt > 0.0 {
            std::sync::Arc::make_mut(&mut view).paint(|_, c| blend_rgb(c, WHITE, bolt));
        }
        for ((x, y), c) in view.every() {
            buf.put_checked(x, y, c.unwrap_or(theme.surface.window_frame));
        }
    }
}

#[cfg(test)]
mod tests;
