//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass. Helpers are
//! `pub(super)` so the orchestrator (`pixel_painter/mod.rs`) can call
//! them in the order it wants.

mod lighting;
mod sky;

// Re-export everything the parent pixel_painter/mod.rs imports.
pub(super) use lighting::{
    paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_panel, paint_shadow, Ellipse,
};
pub(super) use sky::{
    beam_strength, daylight_floor_overlay, dim_floor_overlay, set_weather_override, sun_on_wall,
    time_of_day_look, weather_state, TimeOfDayLook, WallSide, Weather,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_rgb, mix_lab};

/// Fractional local hour (`hour + minute/60`, in `0.0..24.0`) for `now`, decoded
/// via chrono. Shared by the day-ramp / sunset / window-look timers. NB:
/// `sun_on_wall` keeps its own fallible `.ok()?` decode because it returns an
/// `Option`; this infallible form (`unwrap_or_default`) suits the rest.
fn local_hour_frac(now: std::time::SystemTime) -> f32 {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    local.hour() as f32 + local.minute() as f32 / 60.0
}

use crate::layout::{Layout, ELEVATOR_W};
use crate::theme::Theme;

/// Floor-to-ceiling window stride. Mirrors `paint_floor_and_walls` —
/// kept in sync so `window_spill_columns` returns the same x positions
/// the floor pass paints.
const WINDOW_W: u16 = 22;
const WINDOW_GAP: u16 = 3;
/// Vertical depth of the warm spill band below each window. Mirrors the
/// `DEPTH` constant inside `paint_window_light_spill`.
const SPILL_DEPTH: u16 = 12;

/// Lightning strike cadence (Storm only): a flash fires on average every
/// `LIGHTNING_PERIOD_MS` (~15 s; a much faster cadence would read as a
/// hyperactive storm), lasting `LIGHTNING_FLASH_MS`. The flash shape is a two-pulse flicker
/// (`lightning_envelope`) shared by the bright on-glass bolt
/// (`paint_floor_to_ceiling_window`) and the softer room-wide ambient bounce
/// (`paint_lightning_flash`), so both stay in lockstep.
const LIGHTNING_PERIOD_MS: u64 = 15000;
const LIGHTNING_FLASH_MS: u64 = 90;

/// Intensity envelope (0..1) of a lightning flash given ms since the strike
/// began. Primary strike → brief dim → after-flash, so the strike reads as a
/// real flicker rather than a single on/off blink. Returns 0 outside the flash.
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
/// offset in `[0, PERIOD - FLASH)` (keeping the whole flash inside the bucket),
/// so inter-strike gaps wander over ~0..2·PERIOD while averaging one PERIOD.
/// splitmix64 (same mixer as `weather_state`) for a well-distributed offset.
//
// The two-multiply-xor finalizer is `pixtuoid_core::id::splitmix64`, open-coded
// here (and in `sky::weather_state` + `ambient::dust_mote_positions`) by
// DELIBERATE choice: each is an independent noise source over a disjoint input
// domain (no two sites need equal output — see the scene CLAUDE.md sharp edge).
// The canonical fn is `#[doc(hidden)] pub` (off the semver surface but shared
// cross-crate — `physics`/`pose` already call it), so the open-coding is for
// domain-independence, not a visibility barrier.
fn strike_offset(bucket: u64) -> u64 {
    let mut h = bucket.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    h % (LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS)
}

/// `lightning_envelope` for the current clock, or 0 when not mid-strike.
/// Shared by the window bolt and the room bounce so they fire together, and
/// jittered per `strike_offset` so the cadence reads organic, not clockwork.
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
/// briefly flares — the on-glass bolt alone (`paint_floor_to_ceiling_window`)
/// lit only the window strip, which barely registered. Subtler than the bolt
/// (this is bounced fill light, not the source). No-op unless mid-strike.
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
/// driven by current outdoor weather. Subtle (~15% blend); each variant
/// shifts the indoor mood without overpowering the theme palette.
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
        // overcast's, not darker (the old 200,200,205 read as dark mist).
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
/// `(haze_color, blend_alpha)` or `None` when the skyline is crisp. Fog is a
/// near-total white-out; storm/rain murk it; smog adds a brown-grey pall.
/// Applied to the glass interior before the rain/snow/lightning effects so
/// those still read on top of the murk.
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

/// Returns one `SunbeamColumn` per floor-to-ceiling window, centred on
/// the window and starting at the floor row (just below the wall band).
/// Elevator-door windows are excluded — mirroring the `overlaps_door`
/// guard in `paint_floor_and_walls`. Used by `paint_dust_motes` so the
/// motes drift through the same warm spill the floor pass paints.
pub(in crate::pixel_painter) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout
        .top_margin
        .saturating_sub(crate::layout::WALL_BAND_TO_TOP_MARGIN);
    let skip = layout.door.map(|d| (d.x, d.x + ELEVATOR_W));
    let mut out = Vec::new();
    let mut x = 3u16;
    while x + WINDOW_W + 2 <= layout.buf_w {
        let overlaps_door = skip.is_some_and(|(dx0, dx1)| x < dx1 && x + WINDOW_W > dx0);
        if !overlaps_door {
            out.push(SunbeamColumn {
                x: x + WINDOW_W / 2,
                top_y: top_wall_h,
                depth: SPILL_DEPTH,
            });
        }
        x += WINDOW_W + WINDOW_GAP;
    }
    out
}

/// One frame's celestial disc (sun by day, moon by night), arcing across the
/// window wall. Computed ONCE per frame in `paint_floor_and_walls` — it needs
/// only the emitter + atmosphere + buffer geometry, no per-window state — and
/// passed BY VALUE (it's `Copy`) into every window. `cx` is an ABSOLUTE
/// buffer x (not a per-window offset), so the same body paints only in
/// whichever window it currently sits over — one disc across the whole wall,
/// not one per window.
#[derive(Clone, Copy)]
struct Disc {
    cx: f32,
    cy: f32,
    r: f32,
    core: Rgb,
    glow: Rgb,
    vis: f32,
}

// Placeholder celestial colors — Task 7 replaces these with the theme's
// `theme.lighting.sun_core`/`moon_core`.
const SUN_CORE: Rgb = Rgb {
    r: 255,
    g: 240,
    b: 170,
};
const SUN_GLOW: Rgb = Rgb {
    r: 255,
    g: 205,
    b: 120,
};
const MOON_CORE: Rgb = Rgb {
    r: 236,
    g: 242,
    b: 255,
};
const MOON_GLOW: Rgb = Rgb {
    r: 180,
    g: 200,
    b: 240,
};
const DISC_RADIUS_PX: f32 = 3.5;
const GLOW_PX: f32 = 2.5;
const GLOW_ALPHA: f32 = 0.55;
// "Real low window": the horizon sits low in the band, and the apex climbs
// high enough to leave the glass entirely (clipped) rather than the disc
// tracking the full window height.
const HORIZON_FRAC: f32 = 0.55; // horizon_y = top_wall_h * HORIZON_FRAC
const ARC_RISE_FRAC: f32 = 0.80; // apex lifts top_wall_h * ARC_RISE_FRAC above horizon
/// Below this atmo `disc` visibility, thick cloud swallows the disc entirely
/// (no point painting a body no one can see through the murk).
const MIN_DISC_VIS: f32 = 0.08;

/// This frame's disc placement, or `None` under thick cloud (`atmo(weather).disc`
/// below [`MIN_DISC_VIS`]). `cx`/`cy` are absolute buffer coordinates derived
/// from the SAME `sky::emitter` arc that drives `time_of_day_look`'s spill lean
/// and `sun_on_wall`'s wall spot — so the disc's side, the floor-spill lean, and
/// the wall sun-spot can never disagree (all three read one `azimuth`).
fn compute_disc(now: SystemTime, weather: Weather, buf_w: u16, top_wall_h: u16) -> Option<Disc> {
    let sky = sky::emitter(now);
    let vis = sky::atmo(weather).disc;
    if vis < MIN_DISC_VIS {
        return None; // thick cloud swallows the disc
    }
    let cx = sky.azimuth * buf_w as f32;
    let horizon_y = top_wall_h as f32 * HORIZON_FRAC;
    let cy = horizon_y - sky.altitude * (top_wall_h as f32 * ARC_RISE_FRAC);
    let (core, glow) = match sky.body {
        sky::Body::Sun => (SUN_CORE, SUN_GLOW),
        sky::Body::Moon => (MOON_CORE, MOON_GLOW),
    };
    Some(Disc {
        cx,
        cy,
        r: DISC_RADIUS_PX,
        core,
        glow,
        vis,
    })
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

    for y in 0..buf_h {
        for x in 0..buf_w {
            let hash = (x as u32)
                .wrapping_mul(73)
                .wrapping_add((y as u32).wrapping_mul(151))
                ^ ((x as u32).wrapping_mul(11) ^ (y as u32).wrapping_mul(37));
            let color = match hash % 17 {
                0 | 1 => carpet_light,
                2 | 3 => carpet_dark,
                _ => carpet_base,
            };
            buf.put(x, y, blend_rgb(color, tint, 0.15));
        }
    }
    for y in 0..top_wall_h.min(buf_h) {
        for x in 0..buf_w {
            buf.put(x, y, wall);
        }
    }

    // Floor-to-ceiling windows: 落地窗 — height grows with the wall band so
    // taller terminals get dramatic floor-to-ceiling glass. Width stays
    // fixed (mullion every 22 px) so the skyline detail reads consistently.
    // WINDOW_W / WINDOW_GAP are module constants — kept in sync with
    // `window_spill_columns` so motes drift through the same x columns.
    let window_y: u16 = 1;
    let window_h: u16 = top_wall_h.saturating_sub(2).max(8);
    // Window-invariant glass colors: `lit_colors` / `building` / `sky_row`
    // depend only on `look` + `theme` + the (fixed-across-the-loop) window
    // height, NOT on the per-window x / window_idx / altitude — so they're
    // identical for every window in this frame. Compute them ONCE here and pass
    // by reference, instead of recomputing (3 + 1 + glass_h `mix_lab` calls and
    // a Vec alloc) inside every window. (The per-window skyline-height math —
    // alt_shrink/min_bh/max_bh — stays in the fn: it uses `altitude`.)
    let (lit_colors, building, sky_row) = window_glass_invariants(window_h, look, theme);
    // Computed once per frame (not per window) and passed by value — see
    // `compute_disc`'s doc comment for why `cx` is absolute across the wall.
    let disc = compute_disc(now, weather, buf_w, top_wall_h);
    let mut x = 3u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        // Skip any window whose x-range overlaps the elevator door —
        // the elevator sits in the wall and would otherwise show the
        // window's glass + skyline behind its frame.
        let overlaps_door =
            skip_window_x_range.is_some_and(|(dx0, dx1)| x < dx1 && x + WINDOW_W > dx0);
        if !overlaps_door {
            paint_floor_to_ceiling_window(
                buf,
                x,
                window_y,
                WINDOW_W,
                window_h,
                window_frame,
                idx as u16,
                now,
                weather,
                altitude,
                &lit_colors,
                building,
                &sky_row,
                disc,
            );
            // look.spill_strength already includes atmospheric attenuation
            // (time_of_day_look multiplies by atmo.intensity), so heavy
            // weather automatically dims the spill below windows.
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
        x += WINDOW_W + WINDOW_GAP;
        idx += 1;
    }

    // Wall trim line at the bottom of the wall band.
    let trim_y = top_wall_h.saturating_sub(1);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, wall_trim_color);
        }
    }
}

/// Static "is this building window lit?" decision — independent of time.
/// Deterministic hash of (window_idx, dx, dy) so each building's window
/// pattern is stable across frames; only `city_dot_twinkle` animates
/// on top. ~75% of grid slots are lit so the city reads as "alive at
/// night" without every single window being on.
fn city_dot_lit(window_idx: u16, dx: u16, dy: u16) -> bool {
    let mut h = (window_idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= (dx as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    h ^= (dy as u64).wrapping_mul(0x1656_67b1_9e37_79b9);
    h ^= h >> 17;
    // ~75% of the city-window grid is lit at night so the skyline reads as alive.
    const CITY_WINDOW_LIT_PERCENT: u64 = 75;
    (h % 100) < CITY_WINDOW_LIT_PERCENT
}

/// Per-dot twinkle: each city-window dot has its own ~600-1400ms cycle and
/// each cycle rerolls on/off via a deterministic hash. Bias toward "on" so
/// the skyline is mostly lit with the occasional dot blinking off.
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

/// Warm sunlight tint spilling onto the floor below a window. Trapezoid
/// shape (widens by 1 px every 2 rows) blended with the existing floor so
/// it reads as "light through window" not "yellow rectangle". `intensity`
/// (0..1) scales with daylight — zero at night so no spill paints.
/// `slant_per_row` shifts the spill horizontally per row going down —
/// positive = rightward (morning sun in the east casts light right), negative
/// = leftward (evening sun in the west casts light left).
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
/// Snow is a `Flake`. Every per-weather magic number lives in [`StreakSpec`] so
/// the four hand-written loops collapse to one without changing a pixel.
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

/// Per-weather constants for the shared particle loop. Snow diverges the most
/// (`seed_mult` 11 not 7, a different `sx_mult`, `Flake` shape) — all captured
/// here so [`paint_streaks`] stays a single behavior-exact path.
struct StreakSpec {
    count: u64,
    seed_mult: u64,
    sx_mult: u64,
    speed_base: u64,
    speed_span: u64,
    color: Rgb,
    particle: Particle,
}

/// The drawable glass interior of a window — the frame inset by 1px on each side
/// (`x0 = x+1`, `w = window_w - 2`). Bundled so [`paint_streaks`] takes one rect
/// instead of four loose coords.
#[derive(Clone, Copy)]
struct GlassRect {
    x0: u16,
    y0: u16,
    w: u16,
    h: u16,
}

/// Paint one weather's falling particles onto the glass interior. The seed→
/// position math is shared across weathers; `spec` supplies the per-weather
/// constants. This replaced four structurally-identical loops
/// (Rain/Storm/Windy/Snow); the refactor is pixel-verified (#92): byte-identical
/// `snapshot --weather <w>` before/after.
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
                    if px < buf.width() && py < buf.height() {
                        let alpha = alpha_base - (dy as f32 / len as f32) * alpha_falloff;
                        let cur = buf.get(px, py);
                        buf.put(px, py, blend_rgb(cur, spec.color, alpha));
                    }
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
/// `(x0+1 .. x0+w-1, y0+1 .. y0+h-1)`, one `blend_rgb(cur, color, alpha)` per
/// in-bounds cell. The shared body of the Fog / Overcast / Smog weather arms,
/// carrying their EXACT offset math (`1..h-1`/`1..w-1`, raw `x0+dx`/`y0+dy`, the
/// `px < buf.width && py < buf.height` guard). NOT the streaks' `x+1/y+1` inset —
/// keep it byte-identical to the hand-rolled fog/overcast/smog loops (#92-class).
fn wash_glass(buf: &mut RgbBuffer, x0: u16, y0: u16, w: u16, h: u16, color: Rgb, alpha: f32) {
    for dy in 1..h.saturating_sub(1) {
        for dx in 1..w.saturating_sub(1) {
            let px = x0 + dx;
            let py = y0 + dy;
            if px < buf.width() && py < buf.height() {
                let cur = buf.get(px, py);
                buf.put(px, py, blend_rgb(cur, color, alpha));
            }
        }
    }
}

/// Window-invariant glass colors, computed ONCE per frame in
/// `paint_floor_and_walls` and shared by every window. `lit_colors` (city-dot
/// hues) and `building` (silhouette fill) are functions of `look.darkness` plus
/// the theme; `sky_row` (the per-row sky gradient) is a function of the window
/// HEIGHT plus the `look` glass colors. All windows in a frame share the same
/// height, `look`, and theme, so these are identical across the loop — hoisting
/// them out of the per-window loop is byte-identical, just fewer redundant
/// `mix_lab` calls. The per-window skyline-HEIGHT math is NOT here: it rides
/// `altitude` and stays inside `paint_floor_to_ceiling_window`.
fn window_glass_invariants(
    h: u16,
    look: &TimeOfDayLook,
    theme: &Theme,
) -> ([Rgb; 3], Rgb, Vec<Rgb>) {
    let building_dark = theme.office.building_dark;
    let building_light = theme.office.building_light;
    let cw = theme.office.city_lit_windows;
    let dark_window = theme.office.city_dark_window;

    // Floor at 0.12 (not 0.5): keeps a faint window structure visible by day
    // but lets the city windows fade toward dark in full daylight and only glow
    // toward dusk/night — tracking `darkness` like the rest of the light model
    // (the old 0.5 floor kept buildings ~50% lit even at noon).
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

/// Golden-hour blaze strength on the city silhouette — SUN-only: a low moon
/// must never paint an orange cast, however warm/lit it computes (a real
/// moon's own altitude/luminance are already too low to matter, but the gate
/// is absolute, not incidental). Warmth peaks near the horizon, scaled by the
/// emitter's own luminance (a dim sunrise/sunset blazes less than a bright
/// one) and the atmosphere's disc visibility (clouds hide the source, so no
/// blaze without a visible disc).
fn golden_hour_blaze(sky: &sky::SkyState, a: &sky::Atmo) -> f32 {
    match sky.body {
        sky::Body::Sun => (sky.warmth * sky.emitter_lum * a.disc).clamp(0.0, 1.0),
        sky::Body::Moon => 0.0,
    }
}

/// Floor-to-ceiling window with frame, mullion, and a procedural city view
/// inside the glass. Sky gradient at top blends with time-of-day glass
/// colors; the lower portion shows building silhouettes whose "windows"
/// (1-pixel dots) light up at night and twinkle on a per-dot cycle so the
/// skyline reads as alive instead of stamped. `lit_colors` / `building` /
/// `sky_row` are window-invariant (see `window_glass_invariants`) and passed in
/// by reference so they're computed once per frame, not once per window.
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
) {
    // Skyline silhouette as a 0..15 PATTERN; the actual pixel height is
    // computed per-window so the skyline auto-scales with the glass
    // height. On a 12-px-tall window the buildings are 3..7 px, on a
    // 50-px-tall window they fill 12..24 px — same visual proportion.
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
                // Lit-window dots arranged on a 2-px grid (every other
                // column + every other row of the building). Per-dot
                // lit/unlit decision is hashed from (col, row, win_idx)
                // so the same building always shows the same pattern;
                // ~70 % of grid slots are lit at night. Twinkle animates
                // the lit ones on independent cycles.
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
                if let Some(d) = disc {
                    let dist = (((px as f32 - d.cx).powi(2)) + ((py as f32 - d.cy).powi(2))).sqrt();
                    if dist <= d.r {
                        col = blend_rgb(col, d.core, d.vis);
                    } else if dist <= d.r + GLOW_PX {
                        let falloff = 1.0 - (dist - d.r) / GLOW_PX;
                        col = blend_rgb(col, d.glow, d.vis * falloff * GLOW_ALPHA);
                    }
                }
                buf.put(px, py, col);
            }
        }
    }

    // Skyline haze: fog/rain/storm/smog obscure the city behind the glass.
    // Blend the glass interior toward the weather haze BEFORE the streak/flash
    // effects, so rain/snow/lightning still read on top of the murk.
    if let Some((haze, alpha)) = skyline_haze(weather) {
        for dy in 1..h.saturating_sub(1) {
            for dx in 1..w.saturating_sub(1) {
                let px = x + dx;
                let py = y + dy;
                if px < buf.width() && py < buf.height() {
                    let cur = buf.get(px, py);
                    buf.put(px, py, blend_rgb(cur, haze, alpha));
                }
            }
        }
    }

    let elapsed_ms = epoch_ms(now);

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
            GlassRect {
                x0: x + 1,
                y0: y + 1,
                w: w.saturating_sub(2),
                h: h.saturating_sub(2),
            },
            elapsed_ms,
        ),
        Weather::Storm => {
            // Storm keeps Rain's idiom but a distinct cool-blue target (b:245 vs
            // 240), longer/darker streaks, and 6 of them — then the bolt.
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
                GlassRect {
                    x0: x + 1,
                    y0: y + 1,
                    w: w.saturating_sub(2),
                    h: h.saturating_sub(2),
                },
                elapsed_ms,
            );
            // The bright on-glass bolt — the strike's source. Uses the shared,
            // jittered flash level so it fires in lockstep with the room-wide
            // bounce (paint_lightning_flash).
            let level = lightning_flash_level(now);
            if level > 0.0 {
                let alpha = 0.6 * level;
                for dy in 1..h.saturating_sub(1) {
                    for dx in 1..w.saturating_sub(1) {
                        let px = x + dx;
                        let py = y + dy;
                        if px < buf.width() && py < buf.height() {
                            let cur = buf.get(px, py);
                            buf.put(
                                px,
                                py,
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
            }
        }
        Weather::Snow => paint_streaks(
            buf,
            &StreakSpec {
                // Snow diverges: seed_mult 11 (not 7), a different sx_mult, and a
                // flat single-pixel flake with a 0/1 wiggle (no falloff/length).
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
            GlassRect {
                x0: x + 1,
                y0: y + 1,
                w: w.saturating_sub(2),
                h: h.saturating_sub(2),
            },
            elapsed_ms,
        ),
        Weather::Fog => wash_glass(
            buf,
            x,
            y,
            w,
            h,
            Rgb {
                r: 160,
                g: 165,
                b: 175,
            },
            0.25,
        ),
        Weather::Overcast => wash_glass(
            buf,
            x,
            y,
            w,
            h,
            Rgb {
                r: 100,
                g: 105,
                b: 110,
            },
            0.2,
        ),
        Weather::Windy => paint_streaks(
            buf,
            &StreakSpec {
                // Rain's streak with a wind lean (drift) and one more streak.
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
            GlassRect {
                x0: x + 1,
                y0: y + 1,
                w: w.saturating_sub(2),
                h: h.saturating_sub(2),
            },
            elapsed_ms,
        ),
        Weather::Smog => {
            // Warm-yellow desaturated haze across the full glass. Heavier
            // than Fog and noticeably warmer — pulls the city behind a
            // sodium-lit veil.
            wash_glass(
                buf,
                x,
                y,
                w,
                h,
                Rgb {
                    r: 180,
                    g: 160,
                    b: 110,
                },
                0.30,
            )
        }
        Weather::Clear => {}
    }

    let sky_now = sky::emitter(now);
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
mod tests {
    use super::*;

    // Task 4's headline invariant for the golden-hour blaze: it must be
    // SUN-only. Uses hand-built SkyState/Atmo values (not real clock times) so
    // even a maximally warm/lit MOON — which a real moon's low altitude/luminance
    // could never actually produce — still proves the gate is absolute.
    #[test]
    fn golden_hour_blaze_is_sun_only() {
        let full_atmo = sky::Atmo {
            direct: 1.0,
            diffuse: 1.0,
            disc: 1.0,
        };
        let moon = sky::SkyState {
            body: sky::Body::Moon,
            altitude: 1.0,
            azimuth: 0.5,
            warmth: 1.0,
            emitter_lum: 1.0,
        };
        assert_eq!(
            golden_hour_blaze(&moon, &full_atmo),
            0.0,
            "a moon must never blaze, even at maximal warmth/luminance"
        );
        let sun = sky::SkyState {
            body: sky::Body::Sun,
            ..moon
        };
        assert!(
            golden_hour_blaze(&sun, &full_atmo) > 0.9,
            "a maximal sun should blaze near-full"
        );
    }

    #[test]
    fn weather_floor_tint_differs_by_variant() {
        let clear = weather_floor_tint(Weather::Clear);
        let rain = weather_floor_tint(Weather::Rain);
        let fog = weather_floor_tint(Weather::Fog);
        assert_ne!(clear, rain, "rain biases floor cooler");
        assert_ne!(clear, fog, "fog desaturates");
        assert!(
            rain.b >= rain.r,
            "rain tint should be cool (blue >= red), got {:?}",
            rain
        );
    }

    #[test]
    fn weather_floor_tint_clear_is_near_neutral() {
        let clear = weather_floor_tint(Weather::Clear);
        assert!(
            clear.r > 200 && clear.g > 200 && clear.b > 200,
            "clear should be a near-white slight-warm tint, got {:?}",
            clear
        );
    }

    #[test]
    fn fog_floor_tint_is_brighter_than_overcast() {
        // Regression for the "fog read as dark mist" bug — fog must be the
        // brighter (luminous white-out) of the two.
        let fog = weather_floor_tint(Weather::Fog);
        let oc = weather_floor_tint(Weather::Overcast);
        let lum = |c: Rgb| c.r as u16 + c.g as u16 + c.b as u16;
        assert!(
            lum(fog) > lum(oc),
            "fog {fog:?} should outshine overcast {oc:?}"
        );
    }

    #[test]
    fn skyline_haze_obscures_fog_and_storm_only_when_expected() {
        // Fog is the heaviest veil; clear/windy/snow leave the skyline crisp.
        let fog = skyline_haze(Weather::Fog).expect("fog hazes").1;
        let storm = skyline_haze(Weather::Storm).expect("storm hazes").1;
        assert!(fog > storm, "fog should obscure more than storm");
        assert!(
            skyline_haze(Weather::Clear).is_none(),
            "clear skyline is crisp"
        );
        assert!(
            skyline_haze(Weather::Snow).is_none(),
            "snow skyline is crisp"
        );
    }

    #[test]
    fn lightning_envelope_is_a_two_pulse_then_dark() {
        assert_eq!(lightning_envelope(0), 1.0, "primary strike");
        assert!(
            lightning_envelope(30) < lightning_envelope(0),
            "dim between flickers"
        );
        assert!(
            lightning_envelope(50) > lightning_envelope(30),
            "after-flash rebrightens"
        );
        assert_eq!(lightning_envelope(LIGHTNING_FLASH_MS), 0.0, "flash is over");
        assert_eq!(lightning_envelope(5000), 0.0, "dark between strikes");
    }

    #[test]
    fn lightning_flash_storm_only_and_mid_strike_only() {
        use std::time::{Duration, UNIX_EPOCH};
        // Strikes are jittered per bucket, so the flash is at `strike_offset(bucket)`
        // into the bucket, not phase 0. Pick a low-offset bucket so off+1000 (the
        // quiet probe) stays inside the same bucket.
        let bucket = (0u64..)
            .find(|&b| strike_offset(b) < 500)
            .expect("a low-offset bucket exists");
        let off = strike_offset(bucket);
        let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms);
        let mk = || {
            RgbBuffer::filled(
                8,
                4,
                Rgb {
                    r: 10,
                    g: 10,
                    b: 12,
                },
            )
        };

        let mut b = mk();
        paint_lightning_flash(&mut b, at(off), Weather::Storm);
        assert!(b.get(0, 0).r > 10, "storm strike should brighten the room");

        let mut b = mk();
        paint_lightning_flash(&mut b, at(off + 1000), Weather::Storm);
        assert_eq!(
            b.get(0, 0),
            Rgb {
                r: 10,
                g: 10,
                b: 12
            },
            "no flash between strikes"
        );

        let mut b = mk();
        paint_lightning_flash(&mut b, at(off), Weather::Clear);
        assert_eq!(
            b.get(0, 0),
            Rgb {
                r: 10,
                g: 10,
                b: 12
            },
            "flash is storm-only"
        );
    }

    #[test]
    fn lightning_strikes_are_jittered_not_metronomic() {
        let offsets: Vec<u64> = (0..24u64).map(strike_offset).collect();
        let distinct = offsets
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert!(
            distinct > 12,
            "strike offsets should vary across buckets, got {offsets:?}"
        );
        // Every offset keeps the whole flash inside its own bucket.
        assert!(offsets
            .iter()
            .all(|&o| o < LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS));
    }

    // The Storm window arm paints rain streaks plus a bright on-glass bolt that
    // fires only inside the ~90 ms lightning flash. Drive the painter directly
    // with Weather::Storm at a `now` inside a low-offset strike window (same
    // technique as lightning_flash_storm_only) and assert the glass interior is
    // markedly brighter than the same window painted one second later (no flash).
    #[test]
    fn storm_window_bolt_brightens_glass_during_the_flash() {
        use std::time::{Duration, UNIX_EPOCH};
        let bucket = (0u64..)
            .find(|&b| strike_offset(b) < 500)
            .expect("a low-offset bucket exists");
        let off = strike_offset(bucket);
        let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms);
        // Sanity: the chosen instant has a positive flash level, the next-second
        // probe does not — so the only difference between the two renders is the
        // bolt block.
        assert!(
            lightning_flash_level(at(off)) > 0.0,
            "flash at strike offset"
        );
        assert_eq!(
            lightning_flash_level(at(off + 1000)),
            0.0,
            "quiet 1 s later"
        );

        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let render_lum = |now: SystemTime| -> u64 {
            let look = time_of_day_look(now, theme);
            let (lit_colors, building, sky_row) = window_glass_invariants(30, &look, theme);
            let mut buf = RgbBuffer::filled(40, 40, Rgb { r: 8, g: 8, b: 10 });
            paint_floor_to_ceiling_window(
                &mut buf,
                0,
                0,
                WINDOW_W,
                30,
                theme.surface.window_frame,
                0,
                now,
                Weather::Storm,
                0.0,
                &lit_colors,
                building,
                &sky_row,
                None,
            );
            // Sum luminance over the glass interior (inside the 1px frame).
            let mut sum = 0u64;
            for y in 1..29u16 {
                for x in 1..(WINDOW_W - 1) {
                    let p = buf.get(x, y);
                    sum += p.r as u64 + p.g as u64 + p.b as u64;
                }
            }
            sum
        };
        let flashing = render_lum(at(off));
        let quiet = render_lum(at(off + 1000));
        assert!(
            flashing > quiet,
            "the on-glass bolt must brighten the storm glass during the flash \
             (flash={flashing}, quiet={quiet})"
        );
    }

    // The spill/window bounds clamps: a buffer barely taller than the wall band
    // forces the window-light spill trapezoid AND the floor-to-ceiling window to
    // run off the bottom edge, exercising the `break` / `continue` guards. The
    // render must not panic and the in-bounds rows must still paint.
    #[test]
    fn short_buffer_clamps_spill_and_window_without_panic() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let top_wall_h = 18u16;
        // buf_h sits just above top_wall_h so the spill (SPILL_DEPTH rows below
        // the wall band) and the window glass both straddle the bottom edge.
        let buf_h = top_wall_h + 2;
        let buf_w = 60u16;
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12 * 3600);
        // Construct the look directly with a positive spill so the spill path
        // runs regardless of the local clock.
        let look = TimeOfDayLook {
            glass_a: theme.office.building_light,
            glass_b: theme.office.building_dark,
            spill_strength: 0.8,
            spill_slant: 0.0,
            darkness: 0.2,
        };
        let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 5, g: 5, b: 5 });
        paint_floor_and_walls(
            &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
        );
        // No panic reaching here is the primary assertion (RgbBuffer::put has no
        // bounds guard). The wall band's in-bounds rows must still be painted.
        assert_ne!(
            buf.get(0, 0),
            Rgb { r: 5, g: 5, b: 5 },
            "the wall band should still paint in the in-bounds rows"
        );
    }

    /// Build a `SystemTime` for local `h:mi` on a fixed date — mirrors
    /// `sky.rs`'s private `at_hour`, TZ-independent since every derivation
    /// (`sky::emitter`/`weather_state`) decodes back into `chrono::Local`.
    fn at_local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("local time should be unambiguous")
            .into()
    }

    /// Render a full office wall (via the real `paint_floor_and_walls` path —
    /// exercises `compute_disc` + the sky-branch blend exactly as production
    /// does) at a forced local hour + weather. Resets the weather override on
    /// drop so a mid-test panic can't leak into a sibling test's thread.
    fn render_office_at(hour: u32, weather: Weather, buf_w: u16, top_wall_h: u16) -> RgbBuffer {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        set_weather_override(Some(weather));
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let now = at_local(2026, 1, 1, hour, 0);
        let look = time_of_day_look(now, theme);
        let buf_h = top_wall_h + 4;
        let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 4, g: 4, b: 6 });
        paint_floor_and_walls(
            &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
        );
        buf
    }

    /// Count "warm bright" pixels (the sun disc's signature — its core color
    /// fully replaces the sky pixel at full atmo visibility) in the sky-only
    /// top third of the window band. Restricted to the top third (not the
    /// full `1..top_wall_h`) so it can never pick up the SKYLINE's own lit
    /// city-window dots (`theme.office.city_lit_windows`), which live in the
    /// glass's bottom half regardless of time of day and would otherwise
    /// false-positive as a "disc".
    fn count_warm_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
        (1..(top_wall_h / 3).max(2))
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = buf.get(x, y);
                p.r > 200 && p.r > p.b.saturating_add(40)
            })
            .count()
    }

    /// Count "cool bright" pixels (the moon disc's signature) in the same
    /// sky-only region. `MOON_CORE`/`MOON_GLOW` sit closer to neutral white
    /// than the sun's warm palette, so the blue-over-red margin is smaller
    /// than `count_warm_bright`'s (10 vs 40) — still well clear of the base
    /// night-sky gradient (`theme.lighting.night_sky_a/b`), whose blue
    /// channel never approaches 200.
    fn count_cool_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
        (1..(top_wall_h / 3).max(2))
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = buf.get(x, y);
                p.b > 200 && p.b > p.r.saturating_add(10)
            })
            .count()
    }

    #[test]
    fn disc_appears_low_in_the_sky_at_a_low_sun_hour() {
        // 07:00: the sun sits low (altitude ≈0.41, well under the
        // HORIZON_FRAC/ARC_RISE_FRAC ≈0.69 clip threshold), so its disc
        // lands inside the glass rather than climbing off the top.
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let clear = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
        let overcast = render_office_at(7, Weather::Overcast, buf_w, top_wall_h);
        let clear_n = count_warm_bright(&clear, top_wall_h);
        let overcast_n = count_warm_bright(&overcast, top_wall_h);
        assert!(
            clear_n >= 3,
            "a warm disc should show at a low clear sun hour, got {clear_n} bright px"
        );
        assert!(
            clear_n > overcast_n,
            "overcast (atmo disc visibility below MIN_DISC_VIS) should hide the \
             disc clear shows: clear={clear_n} overcast={overcast_n}"
        );
    }

    #[test]
    fn disc_is_absent_at_apex_on_a_short_window() {
        // Low-sun baseline: same 07:00 clear render as the test above.
        let buf_w = 96u16;
        let low_top_wall_h = 40u16;
        let low = render_office_at(7, Weather::Clear, buf_w, low_top_wall_h);
        let low_n = count_warm_bright(&low, low_top_wall_h);

        // Apex (12:00, altitude ≈0.99) on a SHORT window: `compute_disc`'s
        // `cy` is `top_wall_h * (HORIZON_FRAC - altitude*ARC_RISE_FRAC)`, and
        // at this altitude the bracket is solidly negative — the disc has
        // climbed above the glass regardless of `top_wall_h`'s size ("real
        // low window": the apex ALWAYS clips, by construction). Must not
        // panic (a short window also shrinks `window_h`/`glass_h` to their
        // floor).
        let short_top_wall_h = 10u16;
        let apex = render_office_at(12, Weather::Clear, buf_w, short_top_wall_h);
        let apex_n = count_warm_bright(&apex, short_top_wall_h);

        assert!(
            apex_n <= low_n,
            "the apex disc should have clipped above the short glass: \
             apex={apex_n} low={low_n}"
        );
    }

    #[test]
    fn moon_disc_shows_at_night() {
        // 21:00: one hour past dusk, the moon's night-arc altitude is still
        // low (≈0.59, under the clip threshold) — unlike the small hours
        // (00:00-02:00), which sit near the night arc's OWN apex (the
        // dusk-to-dawn span's midpoint) and clip exactly like a midday sun.
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let clear = render_office_at(21, Weather::Clear, buf_w, top_wall_h);
        let overcast = render_office_at(21, Weather::Overcast, buf_w, top_wall_h);
        let clear_n = count_cool_bright(&clear, top_wall_h);
        let overcast_n = count_cool_bright(&overcast, top_wall_h);
        assert!(
            clear_n >= 3,
            "a cool moon disc should show at a clear night hour, got {clear_n} bright px"
        );
        assert!(
            clear_n > overcast_n,
            "overcast should hide the moon disc clear shows: \
             clear={clear_n} overcast={overcast_n}"
        );
    }
}
