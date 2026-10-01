//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, lamp halo,
//! floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass, in the order the
//! orchestrator (`pixel_painter/mod.rs`) calls it; the backdrop fixtures
//! among it (clock, runner, mats) in roster order.

mod floor_wash;
mod lighting;

use crate::celestial::SkyView;
pub(super) use floor_wash::paint_floor_wash;
pub(crate) use lighting::{
    ClockReading, RUNNER_LATTICE_STRIDE, clock_reading, neon_look, octant_offset,
};
pub(super) use lighting::{
    NeonLook, paint_clock, paint_corridor_runner, paint_light, paint_neon_halo, paint_neon_panel,
    paint_shadows,
};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::palette::{RgbLut, WHITE, blend_pixel, blend_rgb};

use crate::atmosphere::Moment;
use crate::glass_weather::GlassWeather;
use crate::layout::{
    Bounds, Layout, Size, WindowBay, glass_rows, wall_trim_row, window_frame, window_posts,
    window_rows, window_run,
};
use crate::sky::{Sky, Weather};
use crate::skyline::CityStrip;
use crate::theme::Theme;

/// Room-wide ambient bounce from a Storm lightning strike, at [`Sky::flash`].
pub(super) fn paint_lightning_flash(buf: &mut RgbBuffer, sky: &Sky) {
    if sky.weather() != Weather::Storm {
        return;
    }
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

/// Returns one `SunbeamColumn` per painted window, centred on the pane and
/// starting at the floor row, so the motes drift through the window's
/// [`Light::Spill`](crate::lighting::Light::Spill).
pub(in crate::pixel_painter) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout.wall_band_h();
    layout
        .window_bays()
        .map(|w| SunbeamColumn {
            x: w.center_x(),
            top_y: top_wall_h,
            depth: crate::lighting::SPILL_DEPTH,
        })
        .collect()
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

/// The floor and the north wall band `top_wall_h` tall with the windows `bays`,
/// over the whole of `buf`, at `moment`.
pub(super) fn paint_floor_and_walls(
    base_fill: &mut BaseFillCache,
    buf: &mut RgbBuffer,
    top_wall_h: u16,
    bays: impl IntoIterator<Item = WindowBay>,
    moment: &Moment,
    pack: &Pack,
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

    let (tint, share) = look.floor_tint;

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
    let (window_y, window_h) = (rows.start, rows.end - rows.start);
    let run = window_run(buf_w);
    let city = CityStrip::draw(
        pack,
        (run.end - run.start, glass_rows(window_h)),
        moment,
        theme,
        pixtuoid_core::sprite::format::Density::ONE,
    );
    let view = GlassView {
        city: &city,
        run_x0: run.start,
        sky: &SkyView::of(moment, buf_w, top_wall_h, theme),
        weather: GlassWeather::of(moment),
    };
    for w in bays {
        paint_floor_to_ceiling_window(
            buf,
            Bounds {
                x: w.x,
                y: window_y,
                width: w.w,
                height: window_h,
            },
            window_frame,
            w.idx,
            moment,
            view,
        );
    }
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

/// Wash a flat translucent color over `pane`'s glass INTERIOR, one pixel in
/// from each edge: it takes the raw window rect and does its own offset math.
fn wash_glass(buf: &mut RgbBuffer, pane: Bounds, color: Rgb, alpha: f32) {
    for dy in 1..pane.height.saturating_sub(1) {
        for dx in 1..pane.width.saturating_sub(1) {
            blend_pixel(buf, pane.x + dx, pane.y + dy, color, alpha);
        }
    }
}

/// What one frame's windows look out on, the same through every pane.
#[derive(Clone, Copy)]
struct GlassView<'a> {
    /// The city along the whole run of windows.
    city: &'a CityStrip,
    /// The column the city strip's west end stands at.
    run_x0: u16,
    sky: &'a SkyView,
    weather: GlassWeather,
}

/// Floor-to-ceiling window `pane`, framed in `frame` and seeded by its tiling
/// index `window_idx`, with mullion, and `view` behind its glass at `moment`.
fn paint_floor_to_ceiling_window(
    buf: &mut RgbBuffer,
    pane: Bounds,
    frame: Rgb,
    window_idx: u16,
    moment: &Moment,
    view: GlassView<'_>,
) {
    let Bounds {
        x,
        y,
        width: w,
        height: h,
    } = pane;
    let sky = &moment.sky;
    let GlassView {
        city,
        run_x0,
        sky: sky_view,
        weather: glass_weather,
    } = view;
    let glass_h = glass_rows(h);
    let glass = sky_view.pane(x, w, glass_h, 1);
    let building_at = |px: u16, glass_dy: u16| city.at(px.wrapping_sub(run_x0), glass_dy);

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width() || py >= buf.height() {
                continue;
            }
            if window_frame(dx, dy, Size { w, h }) {
                buf.put(px, py, frame);
                continue;
            }
            let glass_dy = dy - 1;
            if let Some(building) = building_at(px, glass_dy) {
                buf.put(px, py, building);
            } else {
                buf.put(px, py, glass.colour((px, py), glass_dy));
            }
        }
    }

    // The veil goes on BEFORE the marks and the bolt, so rain and lightning
    // still read on top of the murk.
    if let Some((color, alpha)) = glass_weather.veil {
        wash_glass(buf, pane, color, alpha);
    }

    // Unclipped by the mullion and transom, which the classic's marks run over.
    let marks = glass_weather.marks(
        window_idx,
        Size {
            w: w.saturating_sub(2),
            h: glass_h,
        },
        1,
    );
    for m in marks {
        let (px, py) = (x + 1 + m.x, y + 1 + m.y);
        if px < buf.width() && py < buf.height() {
            buf.put(px, py, m.over(buf.get(px, py), (px, py)));
        }
    }

    // The bright on-glass bolt — the strike's source. Rides the shared flash
    // level so it fires in lockstep with `paint_lightning_flash`.
    if sky.weather() == Weather::Storm {
        let level = sky.flash();
        if level > 0.0 {
            wash_glass(buf, pane, WHITE, 0.6 * level);
        }
    }

    if let Some(blaze) = sky_view.blaze() {
        for dy in 1..h.saturating_sub(1) {
            let glass_dy = dy.saturating_sub(1);
            for dx in 1..w.saturating_sub(1) {
                let px = x + dx;
                let py = y + dy;
                if px < buf.width() && py < buf.height() && building_at(px, glass_dy).is_none() {
                    buf.put(px, py, blaze.over(buf.get(px, py)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
