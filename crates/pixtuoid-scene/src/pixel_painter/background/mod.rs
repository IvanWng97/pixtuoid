//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass, in the order the
//! orchestrator (`pixel_painter/mod.rs`) calls it.

mod celestial;
mod floor_wash;
mod lighting;

use celestial::{
    compute_disc, star_exists, star_twinkle, Disc, GLOW_ALPHA, GLOW_PX, MOON_SHADOW,
    STAR_ALPHA_MAX, STAR_COLOR,
};
pub(super) use floor_wash::paint_floor_wash;
pub(crate) use lighting::{
    clock_reading, neon_look, octant_offset, ClockReading, RUNNER_LATTICE_STRIDE,
};
pub(super) use lighting::{
    paint_clock, paint_corridor_runner, paint_light, paint_neon_panel, paint_radial_falloff,
    paint_shadow, RadialFalloff,
};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_pixel, blend_rgb, RgbLut, WHITE};

use crate::atmosphere::{Look, Moment};
use crate::layout::{
    glass_rows, wall_trim_row, window_frame, window_rows, window_run, Bounds, Layout, WindowBay,
    WINDOW_W,
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
    let (sky, look) = (&moment.sky, &moment.look);
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
    let sky_row = sky_rows(window_h, look);
    let run = window_run(buf_w);
    let city = CityStrip::draw(
        pack,
        (run.end - run.start, glass_rows(window_h)),
        moment,
        theme,
        std::num::NonZeroU16::MIN,
    );
    let view = GlassView {
        city: &city,
        run_x0: run.start,
        sky_row: &sky_row,
        disc: compute_disc(sky, buf_w, top_wall_h, theme),
    };
    for w in bays {
        paint_floor_to_ceiling_window(
            buf,
            Bounds {
                x: w.x,
                y: window_y,
                width: WINDOW_W,
                height: window_h,
            },
            window_frame,
            w.idx,
            moment,
            view,
        );
    }

    let trim_y = wall_trim_row(top_wall_h);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, wall_trim_color);
        }
    }
}

/// One weather's falling particle on the glass. Rain/Storm/Windy are `Streak`s;
/// Snow is a `Flake`.
#[derive(Clone, Copy)]
enum Particle {
    /// A vertical streak `len_base + seed % len_mod` px long, alpha fading from
    /// `alpha_base` by `alpha_falloff` over its length, blended over the glass;
    /// `drift` slants it +x by `dy/2` per row (the wind lean).
    Streak {
        len_base: u16,
        len_mod: u64,
        alpha_base: f32,
        alpha_falloff: f32,
        drift: bool,
    },
    /// A single opaque pixel with a 0/1 horizontal wiggle (snow — no falloff,
    /// no length, written flat rather than blended).
    Flake,
}

/// Per-weather constants for the shared particle loop.
struct StreakSpec {
    count: u64,
    seed_mult: u64,
    sx_mult: u64,
    speed_base: u64,
    speed_span: u64,
    color: Rgb,
    particle: Particle,
}

/// The drawable glass interior of a window — the frame inset by 1px on each
/// side (`x0 = x+1`, `w = window_w - 2`).
#[derive(Clone, Copy)]
struct GlassRect {
    x0: u16,
    y0: u16,
    w: u16,
    h: u16,
}

/// Paint one weather's falling particles onto the glass interior. The seed→
/// position math is shared across weathers; `spec` supplies the per-weather
/// constants.
fn paint_streaks(
    buf: &mut RgbBuffer,
    spec: &StreakSpec,
    window_idx: u16,
    glass: GlassRect,
    elapsed_ms: u64,
) {
    let GlassRect {
        x0: glass_x0,
        y0: glass_y0,
        w: gw,
        h: gh,
    } = glass;
    for i in 0..spec.count {
        let seed = window_idx as u64 * spec.seed_mult + i;
        let sx = (seed.wrapping_mul(spec.sx_mult) % gw as u64) as u16;
        let speed = spec.speed_base + (seed.wrapping_mul(0x4f6c_dd1d) % spec.speed_span);
        let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
        let phase = (elapsed_ms / speed + offset) % gh as u64;
        match spec.particle {
            Particle::Streak {
                len_base,
                len_mod,
                alpha_base,
                alpha_falloff,
                drift,
            } => {
                let len = len_base + (seed % len_mod) as u16;
                for dy in 0..len {
                    let dx = if drift { dy / 2 } else { 0 };
                    let px = glass_x0 + (sx + dx) % gw;
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    let alpha = alpha_base - (dy as f32 / len as f32) * alpha_falloff;
                    blend_pixel(buf, px, py, spec.color, alpha);
                }
            }
            Particle::Flake => {
                let wiggle = if (elapsed_ms / 400 + seed.wrapping_mul(0x9e37)).is_multiple_of(2) {
                    0
                } else {
                    1
                };
                let px = glass_x0 + (sx + wiggle) % gw;
                let py = glass_y0 + phase as u16;
                if px < buf.width() && py < buf.height() {
                    buf.put(px, py, spec.color);
                }
            }
        }
    }
}

/// Wash a flat translucent color over `pane`'s glass INTERIOR, one pixel in
/// from each edge. This is NOT the streaks' `x+1/y+1` inset: it takes the raw
/// window rect and does its own offset math.
fn wash_glass(buf: &mut RgbBuffer, pane: Bounds, color: Rgb, alpha: f32) {
    for dy in 1..pane.height.saturating_sub(1) {
        for dx in 1..pane.width.saturating_sub(1) {
            blend_pixel(buf, pane.x + dx, pane.y + dy, color, alpha);
        }
    }
}

/// The sky's colour on each row of the glass, shared by every window: all panes
/// in a frame have the same height and `look`.
fn sky_rows(h: u16, look: &Look) -> Vec<Rgb> {
    let glass_h = glass_rows(h);
    (0..glass_h)
        .map(|gy| {
            look.glass_b.mix(
                look.glass_a,
                crate::atmosphere::sky_share(gy as f32, glass_h),
            )
        })
        .collect()
}

/// What one frame's windows look out on, the same through every pane.
#[derive(Clone, Copy)]
struct GlassView<'a> {
    /// The city along the whole run of windows.
    city: &'a CityStrip,
    /// The column the city strip's west end stands at.
    run_x0: u16,
    /// The sky's colour on each glass row ([`sky_rows`]).
    sky_row: &'a [Rgb],
    /// The sun or moon, where it is up.
    disc: Option<Disc>,
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
    let (sky, look, now) = (&moment.sky, &moment.look, moment.now);
    let GlassView {
        city,
        run_x0,
        sky_row,
        disc,
    } = view;
    // The disc paints ONLY in the window its centre sits over. Ungated, a disc
    // near an inter-window gap is wide enough (radius+glow) to reach BOTH
    // neighbours' glass and render twice, bleeding through the solid wall pillar
    // between them.
    let disc = disc.filter(|d| d.cx >= f32::from(x) && d.cx < f32::from(x + w));
    let glass_h = glass_rows(h);
    let clear_sky = crate::skyline::clear_sky_rows(glass_h);
    let building_at = |px: u16, glass_dy: u16| city.at(px.wrapping_sub(run_x0), glass_dy);

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width() || py >= buf.height() {
                continue;
            }
            if window_frame(dx, dy, h) {
                buf.put(px, py, frame);
                continue;
            }
            let glass_dy = dy - 1;
            if let Some(building) = building_at(px, glass_dy) {
                buf.put(px, py, building);
            } else {
                let mut col = sky_row[glass_dy as usize];
                // Stars paint into the sky BEFORE the disc, so an overlapping
                // disc pixel always wins (painted next, below).
                if look.star_strength > 0.0
                    && glass_dy < clear_sky
                    && star_exists(px, py)
                    && star_twinkle(px, py, now)
                {
                    col = blend_rgb(col, STAR_COLOR, look.star_strength * STAR_ALPHA_MAX);
                }
                if let Some(d) = disc {
                    let dx = px as f32 - d.cx;
                    let dy = py as f32 - d.cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist <= d.r {
                        // The sun is always lit; the moon darkens its
                        // un-illuminated side via an elliptical terminator.
                        let target = if d.lit_frac >= 1.0 {
                            d.core
                        } else {
                            let terminator_x =
                                (1.0 - 2.0 * d.lit_frac) * (d.r * d.r - dy * dy).max(0.0).sqrt();
                            let toward_lit_limb = if d.lit_right { dx } else { -dx };
                            if toward_lit_limb >= terminator_x {
                                d.core
                            } else {
                                MOON_SHADOW
                            }
                        };
                        col = blend_rgb(col, target, d.vis);
                    } else if dist <= d.r + GLOW_PX {
                        let falloff = 1.0 - (dist - d.r) / GLOW_PX;
                        // Scaling by `lit_frac` keeps a new moon's near-dark
                        // core from casting a full-bright halo.
                        col = blend_rgb(col, d.glow, d.vis * falloff * GLOW_ALPHA * d.lit_frac);
                    }
                }
                buf.put(px, py, col);
            }
        }
    }

    let weather = sky.weather();

    // The veil goes on BEFORE the streaks and the bolt, so rain and lightning
    // still read on top of the murk.
    if let Some((color, alpha)) = look.glass_veil {
        wash_glass(buf, pane, color, alpha);
    }

    let elapsed_ms = epoch_ms(now);

    // The streak arms (Rain/Storm/Snow/Windy) all paint into the same glass-
    // interior inset; build it ONCE so the four rects can't drift apart.
    let glass = GlassRect {
        x0: x + 1,
        y0: y + 1,
        w: w.saturating_sub(2),
        h: glass_h,
    };

    match weather {
        Weather::Rain => paint_streaks(
            buf,
            &StreakSpec {
                count: 4,
                seed_mult: 7,
                sx_mult: 0x9e37_79b9,
                speed_base: 60,
                speed_span: 50,
                color: Rgb {
                    r: 210,
                    g: 220,
                    b: 240,
                },
                particle: Particle::Streak {
                    len_base: 3,
                    len_mod: 2,
                    alpha_base: 0.35,
                    alpha_falloff: 0.15,
                    drift: false,
                },
            },
            window_idx,
            glass,
            elapsed_ms,
        ),
        Weather::Storm => {
            paint_streaks(
                buf,
                &StreakSpec {
                    count: 6,
                    seed_mult: 7,
                    sx_mult: 0x9e37_79b9,
                    speed_base: 40,
                    speed_span: 40,
                    color: Rgb {
                        r: 210,
                        g: 220,
                        b: 245,
                    },
                    particle: Particle::Streak {
                        len_base: 4,
                        len_mod: 3,
                        alpha_base: 0.6,
                        alpha_falloff: 0.3,
                        drift: false,
                    },
                },
                window_idx,
                glass,
                elapsed_ms,
            );
            // The bright on-glass bolt — the strike's source. Rides the shared
            // flash level so it fires in lockstep with `paint_lightning_flash`.
            let level = sky.flash();
            if level > 0.0 {
                wash_glass(buf, pane, WHITE, 0.6 * level);
            }
        }
        Weather::Snow => paint_streaks(
            buf,
            &StreakSpec {
                count: 3,
                seed_mult: 11,
                sx_mult: 0x517c_c1b7,
                speed_base: 150,
                speed_span: 100,
                color: Rgb {
                    r: 240,
                    g: 240,
                    b: 250,
                },
                particle: Particle::Flake,
            },
            window_idx,
            glass,
            elapsed_ms,
        ),
        Weather::Windy => paint_streaks(
            buf,
            &StreakSpec {
                count: 5,
                seed_mult: 7,
                sx_mult: 0x9e37_79b9,
                speed_base: 50,
                speed_span: 40,
                color: Rgb {
                    r: 210,
                    g: 220,
                    b: 240,
                },
                particle: Particle::Streak {
                    len_base: 3,
                    len_mod: 2,
                    alpha_base: 0.35,
                    alpha_falloff: 0.15,
                    drift: true,
                },
            },
            window_idx,
            glass,
            elapsed_ms,
        ),
        Weather::Fog | Weather::Overcast | Weather::Smog | Weather::Clear => {}
    }

    let sunset = look.golden_hour;
    if sunset > 0.05 {
        for dy in 1..h.saturating_sub(1) {
            let glass_dy = dy.saturating_sub(1);
            for dx in 1..w.saturating_sub(1) {
                let px = x + dx;
                let py = y + dy;
                if px < buf.width() && py < buf.height() && building_at(px, glass_dy).is_none() {
                    let cur = buf.get(px, py);
                    let s = sunset * 0.35;
                    buf.put(
                        px,
                        py,
                        Rgb {
                            r: blend(cur.r, 255, s * 0.4),
                            g: blend(cur.g, 160, s * 0.25),
                            b: blend(cur.b, 60, s * 0.1),
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
