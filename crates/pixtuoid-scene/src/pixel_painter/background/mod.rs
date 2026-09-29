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
    STAR_ALPHA_MAX, STAR_COLOR, STAR_SKY_BAND_FRAC,
};
pub(super) use floor_wash::paint_floor_wash;
pub(super) use lighting::{
    neon_look, paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_glow, paint_neon_panel, paint_radial_falloff, paint_shadow, paint_warm_halo,
    RadialFalloff,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_pixel, blend_rgb, RgbLut, WHITE};

use crate::atmosphere::Look;
use crate::layout::{wall_trim_row, window_rows, Layout, WindowBay, WINDOW_W};
use crate::sky::{Sky, Weather};
use crate::theme::Theme;

/// Vertical depth of the warm spill band below each window.
const SPILL_DEPTH: u16 = 12;

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
/// starting at the floor row, so the motes drift through the same warm spill
/// the floor pass paints.
pub(in crate::pixel_painter) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout.wall_band_h();
    layout
        .window_bays()
        .map(|w| SunbeamColumn {
            x: w.center_x(),
            top_y: top_wall_h,
            depth: SPILL_DEPTH,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_floor_and_walls(
    base_fill: &mut BaseFillCache,
    buf: &mut RgbBuffer,
    buf_w: u16,
    buf_h: u16,
    now: SystemTime,
    sky: &Sky,
    look: &Look,
    top_wall_h: u16,
    bays: impl IntoIterator<Item = WindowBay>,
    theme: &Theme,
    altitude: f32,
) {
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
    let (lit_colors, building, sky_row) = window_glass_invariants(window_h, look, theme);
    let disc = compute_disc(sky, buf_w, top_wall_h, theme);
    for w in bays {
        let x = w.x;
        // The disc paints ONLY in the window its centre sits over. Ungated, a
        // disc near an inter-window gap is wide enough (radius+glow) to reach
        // BOTH neighbours' glass and render twice, bleeding through the solid
        // wall pillar between them.
        let span = w.span();
        let win_disc = disc.filter(|d| d.cx >= f32::from(span.start) && d.cx < f32::from(span.end));
        paint_floor_to_ceiling_window(
            buf,
            x,
            window_y,
            WINDOW_W,
            window_h,
            window_frame,
            w.idx,
            now,
            sky,
            altitude,
            &lit_colors,
            building,
            &sky_row,
            win_disc,
            look,
        );
        // `look.sunlight` already includes atmospheric attenuation, so heavy
        // weather automatically dims the spill below windows.
        if look.sunlight > 0.0 {
            paint_window_light_spill(
                buf,
                x,
                WINDOW_W,
                top_wall_h,
                look.sunlight,
                look.spill_slant,
                theme,
            );
        }
    }

    let trim_y = wall_trim_row(top_wall_h);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, wall_trim_color);
        }
    }
}

/// Static "is this building window lit?" decision — a time-independent hash of
/// (window_idx, dx, dy) so each building's pattern is stable across frames;
/// only `city_dot_twinkle` animates on top.
fn city_dot_lit(window_idx: u16, dx: u16, dy: u16) -> bool {
    let mut h = (window_idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= (dx as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    h ^= (dy as u64).wrapping_mul(0x1656_67b1_9e37_79b9);
    h ^= h >> 17;
    // Enough of the grid lit that the skyline reads as alive at night.
    const CITY_WINDOW_LIT_PERCENT: u64 = 75;
    (h % 100) < CITY_WINDOW_LIT_PERCENT
}

/// Per-dot twinkle: each city-window dot rerolls on/off on its own cycle,
/// biased toward "on" so only the occasional dot blinks off.
fn city_dot_twinkle(window_idx: u16, dx: u16, dy: u16, now: SystemTime) -> bool {
    let now_ms = epoch_ms(now);
    let dot_seed = (window_idx as u64).wrapping_mul(31)
        ^ (dx as u64).wrapping_mul(131)
        ^ (dy as u64).wrapping_mul(521);
    let cycle_ms = 6000 + (dot_seed % 8000);
    let phase = now_ms / cycle_ms;
    let hash = dot_seed
        .wrapping_add(phase)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash % 10) < 7
}

/// Warm sunlight tint spilling onto the floor below a window — a trapezoid
/// blended with the existing floor so it reads as "light through window", not
/// "yellow rectangle". `slant_per_row` shifts the band +x per row going down.
fn paint_window_light_spill(
    buf: &mut RgbBuffer,
    window_x: u16,
    window_w: u16,
    top_y: u16,
    intensity: f32,
    slant_per_row: f32,
    theme: &Theme,
) {
    let warm = theme.lighting.sun_spill;
    let fade_start = 0.32 * intensity;
    for dy in 0..SPILL_DEPTH {
        let widen = i32::from((dy / 2).min(3));
        let shift = (slant_per_row * dy as f32).round() as i32;
        let base_x = i32::from(window_x) + shift;
        // Both edges clip to the canvas, so a band leaning off either side is
        // trimmed there rather than pushed back on whole.
        let clip = |x: i32| x.clamp(0, i32::from(buf.width())) as u16;
        let start_x = clip(base_x - widen);
        let end_x = clip(base_x + i32::from(window_w) + widen);
        let y = top_y + dy;
        if y >= buf.height() {
            break;
        }
        let strength = fade_start * (1.0 - dy as f32 / SPILL_DEPTH as f32);
        for x in start_x..end_x {
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, warm, strength));
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

/// Wash a flat translucent color over the glass INTERIOR — the inset rect
/// `(x0+1 .. x0+w-1, y0+1 .. y0+h-1)`. This is NOT the streaks' `x+1/y+1`
/// inset: it takes the raw window rect and does its own offset math.
fn wash_glass(buf: &mut RgbBuffer, x0: u16, y0: u16, w: u16, h: u16, color: Rgb, alpha: f32) {
    for dy in 1..h.saturating_sub(1) {
        for dx in 1..w.saturating_sub(1) {
            blend_pixel(buf, x0 + dx, y0 + dy, color, alpha);
        }
    }
}

/// Window-invariant glass colors, computed ONCE per frame in
/// `paint_floor_and_walls` and shared by every window: all panes in a frame have
/// the same height, `look`, and theme. The per-window skyline-HEIGHT math is NOT
/// here — it rides `altitude` and stays in `paint_floor_to_ceiling_window`.
fn window_glass_invariants(h: u16, look: &Look, theme: &Theme) -> ([Rgb; 3], Rgb, Vec<Rgb>) {
    let building_dark = theme.office.building_dark;
    let building_light = theme.office.building_light;
    let cw = theme.office.city_lit_windows;
    let dark_window = theme.office.city_dark_window;

    // A floor this LOW keeps only a faint window structure visible by day and
    // lets the city windows glow toward dusk; a 0.5 floor left buildings ~50%
    // lit at noon.
    let lit_strength = look.darkness.max(0.12).clamp(0.0, 1.0);
    let lit_colors: [Rgb; 3] = [
        dark_window.mix(cw[0], lit_strength),
        dark_window.mix(cw[1], lit_strength),
        dark_window.mix(cw[2], lit_strength),
    ];
    let building = building_light.mix(building_dark, look.darkness);

    let glass_h = h.saturating_sub(2);
    let sky_norm = (glass_h as f32) * 0.7;
    let sky_row: Vec<Rgb> = (0..glass_h)
        .map(|gy| {
            let sky_t = (gy as f32 / sky_norm).min(1.0);
            look.glass_b.mix(look.glass_a, sky_t)
        })
        .collect();

    (lit_colors, building, sky_row)
}

/// Floor-to-ceiling window with frame, mullion, and a procedural city view
/// inside the glass. `lit_colors` / `building` / `sky_row` are window-invariant
/// (see `window_glass_invariants`) and passed in by reference.
#[allow(clippy::too_many_arguments)]
fn paint_floor_to_ceiling_window(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    frame: Rgb,
    window_idx: u16,
    now: SystemTime,
    sky: &Sky,
    altitude: f32,
    lit_colors: &[Rgb; 3],
    building: Rgb,
    sky_row: &[Rgb],
    disc: Option<Disc>,
    look: &Look,
) {
    // Skyline silhouette as a 0..PATTERN_MAX ratio, not pixels — the height is
    // computed per-window so the skyline auto-scales with the glass.
    const SKYLINE_PATTERN: &[u8] = &[8, 14, 11, 15, 6, 13, 9, 12, 7, 15, 10, 13];
    const PATTERN_MAX: u16 = 15;
    let glass_h = h.saturating_sub(2);
    let alt_shrink = (glass_h as f32 * 0.3 * altitude) as u16;
    let min_bh = (glass_h / 5).saturating_sub(alt_shrink).max(2);
    let max_bh = (glass_h * 50 / 100)
        .saturating_sub(alt_shrink)
        .max(min_bh + 3);
    let bh_range = max_bh.saturating_sub(min_bh);

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width() || py >= buf.height() {
                continue;
            }
            let on_edge = dx == 0 || dx == w - 1 || dy == 0 || dy == h - 1;
            let on_mullion = dx == w / 2 || dy == h * 7 / 10;
            if on_edge || on_mullion {
                buf.put(px, py, frame);
                continue;
            }
            let glass_dx = dx - 1;
            let glass_dy = dy - 1;
            let pat_idx = ((glass_dx + window_idx * 3) % SKYLINE_PATTERN.len() as u16) as usize;
            let pat = SKYLINE_PATTERN[pat_idx] as u16;
            let building_h = min_bh + (pat * bh_range) / PATTERN_MAX;
            let in_building = glass_dy >= glass_h.saturating_sub(building_h);

            if in_building {
                let bldg_y = glass_dy - (glass_h - building_h);
                // Lit-window dots sit on a 2-px grid — every other column and
                // every other row of the building.
                let on_grid = glass_dx % 2 == 1 && bldg_y % 2 == 1;
                let lit_base = on_grid && city_dot_lit(window_idx, glass_dx, bldg_y);
                if lit_base && city_dot_twinkle(window_idx, glass_dx, bldg_y, now) {
                    let dot_color = match (glass_dx.wrapping_add(bldg_y)) % 5 {
                        0 => lit_colors[1],
                        1 => lit_colors[2],
                        _ => lit_colors[0],
                    };
                    buf.put(px, py, dot_color);
                } else {
                    buf.put(px, py, building);
                }
            } else {
                let mut col = sky_row[glass_dy as usize];
                // Stars paint into the sky BEFORE the disc, so an overlapping
                // disc pixel always wins (painted next, below).
                if look.star_strength > 0.0
                    && (glass_dy as f32) < glass_h as f32 * STAR_SKY_BAND_FRAC
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
        wash_glass(buf, x, y, w, h, color, alpha);
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
                wash_glass(buf, x, y, w, h, WHITE, 0.6 * level);
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
        let min_building_h = (glass_h / 5).max(3);
        for dy in 1..h.saturating_sub(1) {
            let glass_dy = dy.saturating_sub(1);
            if glass_dy >= glass_h.saturating_sub(min_building_h) {
                continue;
            }
            for dx in 1..w.saturating_sub(1) {
                let px = x + dx;
                let py = y + dy;
                if px < buf.width() && py < buf.height() {
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
