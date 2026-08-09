//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass, in the order the
//! orchestrator (`pixel_painter/mod.rs`) calls it.

mod celestial;
mod lighting;
mod sky;

use celestial::{
    compute_disc, golden_hour_blaze, night_star_strength, star_exists, star_twinkle, Disc,
    GLOW_ALPHA, GLOW_PX, MOON_SHADOW, STAR_ALPHA_MAX, STAR_COLOR, STAR_MIN, STAR_SKY_BAND_FRAC,
};
pub(super) use lighting::{
    paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_panel, paint_radial_falloff, paint_shadow, paint_warm_halo, Ellipse, RadialFalloff,
};
pub(super) use sky::{
    beam_strength, daylight_floor_overlay, dim_floor_overlay, hour_is_day, set_weather_override,
    sun_on_wall, time_of_day_look, weather_state, TimeOfDayLook, WallSide, Weather,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_pixel, blend_rgb, mix_lab};

/// Fractional local hour (`hour + minute/60`, in `0.0..24.0`) for `now`. The
/// ambient/sky clock-decode funnel; `paint_clock`'s analog hands keep their own
/// decode because they need raw `hour % 12` / `minute`, not this value.
pub(in crate::pixel_painter) fn local_hour_frac(now: std::time::SystemTime) -> f32 {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    local.hour() as f32 + local.minute() as f32 / 60.0
}

use crate::layout::{Layout, ELEVATOR_W};
use crate::theme::Theme;

/// Floor-to-ceiling window width + inter-pane gap. [`window_columns`] owns the
/// tiling LAW (start / stride / edge-margin / door-skip) both the spill pass and
/// the floor pass ride, so the pane x-positions can't drift between them.
const WINDOW_W: u16 = 22;
const WINDOW_GAP: u16 = 3;
/// Left edge of the first window — the ONE start [`window_columns`] begins at,
/// and the source `celestial::FIRST_WINDOW_X` derives its f32 form from.
const FIRST_WINDOW_X: u16 = 3;
/// The tiling stops when the next pane wouldn't leave this many px before the
/// right buffer edge (`x + WINDOW_W + WINDOW_EDGE_MARGIN <= buf_w`).
const WINDOW_EDGE_MARGIN: u16 = 2;
/// Vertical depth of the warm spill band below each window.
const SPILL_DEPTH: u16 = 12;

/// Lightning strike cadence (Storm only): a flash fires on average every
/// `LIGHTNING_PERIOD_MS` — a much faster cadence reads as a hyperactive storm —
/// lasting `LIGHTNING_FLASH_MS`.
const LIGHTNING_PERIOD_MS: u64 = 15000;
const LIGHTNING_FLASH_MS: u64 = 90;

/// Intensity envelope (0..1) of a lightning flash given ms since the strike
/// began: primary strike → brief dim → after-flash, so it reads as a real
/// flicker rather than a single on/off blink. Returns 0 outside the flash.
fn lightning_envelope(since_strike_ms: u64) -> f32 {
    match since_strike_ms {
        0..=24 => 1.0,   // primary strike
        25..=39 => 0.15, // dim between flickers
        40..=69 => 0.55, // after-flash
        _ => 0.0,
    }
}

/// Per-bucket strike offset (ms into the bucket) so strikes don't fire on a
/// fixed metronome. Each `LIGHTNING_PERIOD_MS`-long bucket hashes to its own
/// offset in `[0, PERIOD - FLASH)`, keeping the whole flash inside the bucket.
//
// splitmix64 is open-coded here (and in `sky::weather_state` +
// `ambient::dust_mote_positions`) by DELIBERATE choice: each is an independent
// noise source over a disjoint input domain, so no two sites need equal output.
fn strike_offset(bucket: u64) -> u64 {
    let mut h = bucket.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    h % (LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS)
}

/// `lightning_envelope` for the current clock, or 0 when not mid-strike.
/// Shared by the window bolt and the room bounce so they fire together.
fn lightning_flash_level(now: SystemTime) -> f32 {
    let elapsed_ms = epoch_ms(now);
    let bucket = elapsed_ms / LIGHTNING_PERIOD_MS;
    let phase = elapsed_ms % LIGHTNING_PERIOD_MS;
    match phase.checked_sub(strike_offset(bucket)) {
        Some(since) if since < LIGHTNING_FLASH_MS => lightning_envelope(since),
        _ => 0.0,
    }
}

/// Room-wide ambient bounce from a Storm lightning strike. Painted LAST in the
/// pixel pass (after floor/walls/furniture/characters) so the whole interior
/// briefly flares; the on-glass bolt alone lit only the window strip.
pub(super) fn paint_lightning_flash(buf: &mut RgbBuffer, now: SystemTime, weather: Weather) {
    if weather != Weather::Storm {
        return;
    }
    let level = lightning_flash_level(now);
    if level <= 0.0 {
        return;
    }
    let alpha = 0.20 * level;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                blend_rgb(
                    cur,
                    Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                    alpha,
                ),
            );
        }
    }
}

/// Multiplicative-ish tint applied to floor cells after the base palette,
/// driven by current outdoor weather.
pub(super) fn weather_floor_tint(w: Weather) -> Rgb {
    match w {
        Weather::Clear => Rgb {
            r: 255,
            g: 252,
            b: 240,
        },
        Weather::Rain => Rgb {
            r: 190,
            g: 200,
            b: 220,
        },
        Weather::Storm => Rgb {
            r: 140,
            g: 145,
            b: 165,
        },
        Weather::Snow => Rgb {
            r: 220,
            g: 230,
            b: 250,
        },
        // Fog is a luminous white-out — its floor tint must be brighter than
        // overcast's, not darker, or it reads as dark mist.
        Weather::Fog => Rgb {
            r: 228,
            g: 229,
            b: 233,
        },
        Weather::Overcast => Rgb {
            r: 210,
            g: 210,
            b: 215,
        },
        Weather::Windy => Rgb {
            r: 248,
            g: 248,
            b: 245,
        },
        Weather::Smog => Rgb {
            r: 215,
            g: 200,
            b: 165,
        },
    }
}

/// Haze that obscures the city skyline behind the glass, by weather. Returns
/// `(haze_color, blend_alpha)` or `None` when the skyline is crisp.
fn skyline_haze(w: Weather) -> Option<(Rgb, f32)> {
    match w {
        Weather::Fog => Some((
            Rgb {
                r: 226,
                g: 228,
                b: 233,
            },
            0.55,
        )),
        Weather::Storm => Some((
            Rgb {
                r: 120,
                g: 126,
                b: 142,
            },
            0.38,
        )),
        Weather::Rain => Some((
            Rgb {
                r: 168,
                g: 178,
                b: 198,
            },
            0.20,
        )),
        Weather::Smog => Some((
            Rgb {
                r: 150,
                g: 138,
                b: 110,
            },
            0.22,
        )),
        Weather::Overcast => Some((
            Rgb {
                r: 196,
                g: 199,
                b: 206,
            },
            0.12,
        )),
        _ => None,
    }
}

/// How much of a weather VEIL's own colour the frame's sky brings up (0..1) —
/// its floor is the city-light scatter that keeps fog reading as fog after dark.
///
/// The day term is the emitter's OWN luminance, deliberately NOT
/// `atmo`/`look.darkness`: those already carry the weather (the veil colour does
/// too), and folding them in would darken a stormy noon twice.
const NIGHT_VEIL_FLOOR: f32 = 0.35;

fn veil_lum(sky: &sky::SkyState) -> f32 {
    NIGHT_VEIL_FLOOR + (1.0 - NIGHT_VEIL_FLOOR) * sky.emitter_lum.clamp(0.0, 1.0)
}

/// A veil colour at the frame's daylight — hue preserved, luminance tracked.
fn veil_lit(color: Rgb, lum: f32) -> Rgb {
    blend_rgb(Rgb { r: 0, g: 0, b: 0 }, color, lum)
}

/// One PAINTED floor-to-ceiling window: its left edge, its centre column, and
/// its ABSOLUTE position `idx` (counted across the whole wall — door-skipped
/// panes still advance it, so a pane after the elevator keeps its true index).
#[derive(Clone, Copy)]
pub(super) struct WindowColumn {
    pub x_left: u16,
    pub center_x: u16,
    pub idx: u16,
}

/// THE window-tiling law, single-sourced: panes start at [`FIRST_WINDOW_X`],
/// stride `WINDOW_W + WINDOW_GAP`, and stop once the next pane wouldn't clear
/// [`WINDOW_EDGE_MARGIN`] before `buf_w`. Yields only panes whose x-range does
/// NOT overlap `skip` (the elevator-door range `(dx0, dx1)`) — but `idx` still
/// counts the skipped ones, so the floor pass's per-window index is stable.
pub(super) fn window_columns(
    buf_w: u16,
    skip: Option<(u16, u16)>,
) -> impl Iterator<Item = WindowColumn> {
    let mut x = FIRST_WINDOW_X;
    let mut idx: u16 = 0;
    std::iter::from_fn(move || {
        while x + WINDOW_W + WINDOW_EDGE_MARGIN <= buf_w {
            let (this_x, this_idx) = (x, idx);
            x += WINDOW_W + WINDOW_GAP;
            idx += 1;
            if !skip.is_some_and(|(dx0, dx1)| this_x < dx1 && this_x + WINDOW_W > dx0) {
                return Some(WindowColumn {
                    x_left: this_x,
                    center_x: this_x + WINDOW_W / 2,
                    idx: this_idx,
                });
            }
        }
        None
    })
}

/// Returns one `SunbeamColumn` per PAINTED floor-to-ceiling window, centred on
/// the window and starting at the floor row. Rides [`window_columns`] so the
/// motes drift through the same warm spill the floor pass paints.
pub(in crate::pixel_painter) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout.wall_band_h();
    let skip = layout.door.map(|d| (d.x, d.x + ELEVATOR_W));
    window_columns(layout.buf_w, skip)
        .map(|w| SunbeamColumn {
            x: w.center_x,
            top_y: top_wall_h,
            depth: SPILL_DEPTH,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_floor_and_walls(
    buf: &mut RgbBuffer,
    buf_w: u16,
    buf_h: u16,
    now: SystemTime,
    look: &TimeOfDayLook,
    top_wall_h: u16,
    skip_window_x_range: Option<(u16, u16)>,
    theme: &Theme,
    altitude: f32,
) {
    let window_frame = theme.surface.window_frame;
    let carpet_base = theme.surface.carpet_base;
    let carpet_light = theme.surface.carpet_light;
    let carpet_dark = theme.surface.carpet_dark;
    let wall = theme.surface.wall;
    let wall_trim_color = theme.surface.wall_trim;

    let weather = weather_state(now);
    let tint = weather_floor_tint(weather);

    // The noise picks one of THREE colours and the tint is fixed for the frame,
    // so resolve the blend once, not per pixel.
    let carpet = [
        blend_rgb(carpet_light, tint, 0.15),
        blend_rgb(carpet_dark, tint, 0.15),
        blend_rgb(carpet_base, tint, 0.15),
    ];
    // Start BELOW the wall band: the loop right after overwrites it opaquely.
    let band_h = top_wall_h.min(buf_h);
    for y in band_h..buf_h {
        for x in 0..buf_w {
            let hash = (x as u32)
                .wrapping_mul(73)
                .wrapping_add((y as u32).wrapping_mul(151))
                ^ ((x as u32).wrapping_mul(11) ^ (y as u32).wrapping_mul(37));
            let color = match hash % 17 {
                0 | 1 => carpet[0],
                2 | 3 => carpet[1],
                _ => carpet[2],
            };
            buf.put(x, y, color);
        }
    }
    for y in 0..band_h {
        for x in 0..buf_w {
            buf.put(x, y, wall);
        }
    }

    // Window HEIGHT grows with the wall band so taller terminals get dramatic
    // glass; width stays fixed so the skyline detail reads consistently.
    let window_y: u16 = 1;
    let window_h: u16 = top_wall_h.saturating_sub(2).max(8);
    let (lit_colors, building, sky_row) = window_glass_invariants(window_h, look, theme);
    let disc = compute_disc(now, weather, buf_w, top_wall_h, theme);
    let star_strength = night_star_strength(now, look.darkness, weather);
    for w in window_columns(buf_w, skip_window_x_range) {
        let x = w.x_left;
        // The disc paints ONLY in the window its centre sits over. Ungated, a
        // disc near an inter-window gap is wide enough (radius+glow) to reach
        // BOTH neighbours' glass and render twice, bleeding through the solid
        // wall pillar between them.
        let win_disc = disc.filter(|d| d.cx >= x as f32 && d.cx < (x + WINDOW_W) as f32);
        paint_floor_to_ceiling_window(
            buf,
            x,
            window_y,
            WINDOW_W,
            window_h,
            window_frame,
            w.idx,
            now,
            weather,
            altitude,
            &lit_colors,
            building,
            &sky_row,
            win_disc,
            star_strength,
        );
        // look.spill_strength already includes atmospheric attenuation, so
        // heavy weather automatically dims the spill below windows.
        if look.spill_strength > 0.0 {
            paint_window_light_spill(
                buf,
                x,
                WINDOW_W,
                top_wall_h,
                look.spill_strength,
                look.spill_slant,
                theme,
            );
        }
    }

    let trim_y = top_wall_h.saturating_sub(1);
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
/// "yellow rectangle". `slant_per_row` is positive rightward (morning sun in
/// the east), negative leftward (evening sun in the west).
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
        let widen = (dy / 2).min(3);
        let shift = (slant_per_row * dy as f32).round() as i32;
        let base_x = (window_x as i32 + shift).max(0) as u16;
        let start_x = base_x.saturating_sub(widen);
        let end_x = (base_x + window_w + widen).min(buf.width());
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
fn window_glass_invariants(
    h: u16,
    look: &TimeOfDayLook,
    theme: &Theme,
) -> ([Rgb; 3], Rgb, Vec<Rgb>) {
    let building_dark = theme.office.building_dark;
    let building_light = theme.office.building_light;
    let cw = theme.office.city_lit_windows;
    let dark_window = theme.office.city_dark_window;

    // A floor this LOW keeps only a faint window structure visible by day and
    // lets the city windows glow toward dusk; a 0.5 floor left buildings ~50%
    // lit at noon.
    let lit_strength = look.darkness.max(0.12).clamp(0.0, 1.0);
    let lit_colors: [Rgb; 3] = [
        mix_lab(dark_window, cw[0], lit_strength),
        mix_lab(dark_window, cw[1], lit_strength),
        mix_lab(dark_window, cw[2], lit_strength),
    ];
    let building = mix_lab(building_light, building_dark, look.darkness);

    let glass_h = h.saturating_sub(2);
    let sky_norm = (glass_h as f32) * 0.7;
    let sky_row: Vec<Rgb> = (0..glass_h)
        .map(|gy| {
            let sky_t = (gy as f32 / sky_norm).min(1.0);
            mix_lab(look.glass_b, look.glass_a, sky_t)
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
    weather: Weather,
    altitude: f32,
    lit_colors: &[Rgb; 3],
    building: Rgb,
    sky_row: &[Rgb],
    disc: Option<Disc>,
    star_strength: f32,
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
                if star_strength > STAR_MIN
                    && (glass_dy as f32) < glass_h as f32 * STAR_SKY_BAND_FRAC
                    && star_exists(px, py)
                    && star_twinkle(px, py, now)
                {
                    col = blend_rgb(col, STAR_COLOR, star_strength * STAR_ALPHA_MAX);
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
                            if dx >= terminator_x {
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

    let sky_now = sky::emitter(now);
    let veil = veil_lum(&sky_now);

    // The haze goes on BEFORE the streak/flash effects, so rain/snow/lightning
    // still read on top of the murk.
    if let Some((haze, alpha)) = skyline_haze(weather) {
        wash_glass(buf, x, y, w, h, veil_lit(haze, veil), alpha);
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
            let level = lightning_flash_level(now);
            if level > 0.0 {
                wash_glass(
                    buf,
                    x,
                    y,
                    w,
                    h,
                    Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                    0.6 * level,
                );
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
        Weather::Fog => wash_glass(
            buf,
            x,
            y,
            w,
            h,
            veil_lit(
                Rgb {
                    r: 160,
                    g: 165,
                    b: 175,
                },
                veil,
            ),
            0.25,
        ),
        Weather::Overcast => wash_glass(
            buf,
            x,
            y,
            w,
            h,
            veil_lit(
                Rgb {
                    r: 100,
                    g: 105,
                    b: 110,
                },
                veil,
            ),
            0.2,
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
        Weather::Smog => wash_glass(
            buf,
            x,
            y,
            w,
            h,
            veil_lit(
                Rgb {
                    r: 180,
                    g: 160,
                    b: 110,
                },
                veil,
            ),
            0.30,
        ),
        Weather::Clear => {}
    }

    let a = sky::atmo(weather);
    let sunset = golden_hour_blaze(&sky_now, &a);
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
