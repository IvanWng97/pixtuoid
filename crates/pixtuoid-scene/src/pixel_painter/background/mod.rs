//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass. Helpers are
//! `pub(super)` so the orchestrator (`pixel_painter/mod.rs`) can call
//! them in the order it wants.

mod celestial;
mod lighting;
mod sky;

// Re-export everything the parent pixel_painter/mod.rs imports.
pub(super) use lighting::{
    paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_panel, paint_radial_falloff, paint_shadow, Ellipse, RadialFalloff,
};
pub(super) use sky::{
    beam_strength, daylight_floor_overlay, dim_floor_overlay, hour_is_day, set_weather_override,
    sun_on_wall, time_of_day_look, weather_state, TimeOfDayLook, WallSide, Weather,
};
// The celestial disc + night-star helpers (#469) are INTERNAL to this module —
// the sky branch of `paint_floor_to_ceiling_window` (and its tests) consume
// them. Unlike sky/lighting they are NOT re-exported up to `pixel_painter` (no
// other pass reads them), so this is a plain `use`, listed explicitly to keep
// the celestial→mod surface auditable. celestial's placement-internal consts
// (DISC_RADIUS_PX/FIRST_WINDOW_X/HORIZON_FRAC/ARC_RISE_FRAC/MIN_DISC_VIS/
// STAR_SPARSITY/STAR_TWINKLE_*) stay private to celestial.rs.
use celestial::{
    compute_disc, golden_hour_blaze, night_star_strength, star_exists, star_twinkle, Disc,
    GLOW_ALPHA, GLOW_PX, MOON_SHADOW, STAR_ALPHA_MAX, STAR_COLOR, STAR_MIN, STAR_SKY_BAND_FRAC,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_rgb, mix_lab};

/// Fractional local hour (`hour + minute/60`, in `0.0..24.0`) for `now`, decoded
/// via chrono. The ambient/sky clock-decode funnel: the day-ramp / sunset /
/// window-look timers and `sun_on_wall` (via `emitter`) all route through here.
/// (`paint_clock`'s analog hands keep their own decode — they need raw
/// `hour % 12` / `minute`, not this fractional value.)
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
    let top_wall_h = layout.wall_band_h();
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
    let disc = compute_disc(now, weather, buf_w, top_wall_h, theme);
    let star_strength = night_star_strength(now, look.darkness, weather);
    let mut x = 3u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        // Skip any window whose x-range overlaps the elevator door —
        // the elevator sits in the wall and would otherwise show the
        // window's glass + skyline behind its frame.
        let overlaps_door =
            skip_window_x_range.is_some_and(|(dx0, dx1)| x < dx1 && x + WINDOW_W > dx0);
        if !overlaps_door {
            // The disc paints ONLY in the window its centre currently sits over.
            // Without this gate, a disc whose `cx` lands near an inter-window gap
            // is wide enough (radius+glow) to reach the glass of BOTH neighbours,
            // so the same sun/moon rendered in two panes at once — bleeding
            // through the solid wall pillar (frame + WINDOW_GAP + frame) between
            // them. Restricting to the containing window makes that pillar occlude
            // the body correctly: it hides behind the pillar between panes and
            // re-emerges in the next window, "one disc across the wall".
            let win_disc = disc.filter(|d| d.cx >= x as f32 && d.cx < (x + WINDOW_W) as f32);
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
                win_disc,
                star_strength,
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
    star_strength: f32,
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
                        // Sun (lit_frac == 1.0) skips the terminator entirely
                        // (always lit); the moon darkens the un-illuminated
                        // side via the classic elliptical terminator.
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
                        // Scale by `lit_frac` so the glow tracks the illuminated
                        // fraction: the sun (lit_frac=1.0) is unaffected, but a
                        // new moon's near-dark core no longer casts a full-bright
                        // halo — the ring dims in step with the phase.
                        col = blend_rgb(col, d.glow, d.vis * falloff * GLOW_ALPHA * d.lit_frac);
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
        wash_glass(buf, x, y, w, h, haze, alpha);
    }

    let elapsed_ms = epoch_ms(now);

    // The streak arms (Rain/Storm/Snow/Windy) all paint into the same glass-
    // interior inset; build it ONCE (reusing `glass_h`) so the four rects can't
    // drift apart.
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
                glass,
                elapsed_ms,
            );
            // The bright on-glass bolt — the strike's source. Uses the shared,
            // jittered flash level so it fires in lockstep with the room-wide
            // bounce (paint_lightning_flash).
            let level = lightning_flash_level(now);
            if level > 0.0 {
                // The on-glass bolt is the same glass-interior wash as fog/overcast,
                // just white at the jittered flash level.
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
            glass,
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
            glass,
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
                0.0,
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
    /// does) at a forced January `day` + local `hour` + weather. Resets the
    /// weather override on drop so a mid-test panic can't leak into a
    /// sibling test's thread.
    fn render_office_on(
        day: u32,
        hour: u32,
        weather: Weather,
        buf_w: u16,
        top_wall_h: u16,
    ) -> RgbBuffer {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        set_weather_override(Some(weather));
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let now = at_local(2026, 1, day, hour, 0);
        let look = time_of_day_look(now, theme);
        let buf_h = top_wall_h + 4;
        let mut buf = RgbBuffer::filled(buf_w, buf_h, Rgb { r: 4, g: 4, b: 6 });
        paint_floor_and_walls(
            &mut buf, buf_w, buf_h, now, &look, top_wall_h, None, theme, 0.0,
        );
        buf
    }

    /// `render_office_on` pinned to January 1st — the fixed date every
    /// existing hour/weather-only test uses (the moon-phase tests below are
    /// the ones that vary the day).
    fn render_office_at(hour: u32, weather: Weather, buf_w: u16, top_wall_h: u16) -> RgbBuffer {
        render_office_on(1, hour, weather, buf_w, top_wall_h)
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
    /// sky-only region. Per-theme `moon_core` values sit closer to neutral
    /// white than each theme's warm `sun_core`, so the blue-over-red margin
    /// is smaller than `count_warm_bright`'s (10 vs 40) — still well clear of
    /// the base night-sky gradient (`theme.lighting.night_sky_a/b`), whose
    /// blue channel never approaches 200.
    fn count_cool_bright(buf: &RgbBuffer, top_wall_h: u16) -> usize {
        (1..(top_wall_h / 3).max(2))
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = buf.get(x, y);
                p.b > 200 && p.b > p.r.saturating_add(10)
            })
            .count()
    }

    /// Count faint-white STAR pixels in the same sky-only top-third band as
    /// `count_warm_bright`/`count_cool_bright` (so it can't pick up the
    /// skyline's lit city-window dots). The base night sky (`night_sky_a/b`,
    /// (18,26,52)/(28,36,70)) never gets close to this threshold on its own —
    /// only a `STAR_COLOR` blend lifts a pixel this bright.
    fn count_faint_white(buf: &RgbBuffer, top_wall_h: u16) -> usize {
        (1..(top_wall_h / 3).max(2))
            .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = buf.get(x, y);
                p.r > 90 && p.g > 90 && p.b > 90
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
    fn rain_hides_the_disc_like_overcast() {
        // Thick cloud hides the disc UNIFORMLY: Rain's disc channel (0.05,
        // same as Overcast/Storm) must hide it entirely too, not just dim it
        // — regression guard for the old 0.20 value that let Rain out-show
        // Overcast.
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let clear = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
        let rain = render_office_at(7, Weather::Rain, buf_w, top_wall_h);
        let overcast = render_office_at(7, Weather::Overcast, buf_w, top_wall_h);
        let clear_n = count_warm_bright(&clear, top_wall_h);
        let rain_n = count_warm_bright(&rain, top_wall_h);
        let overcast_n = count_warm_bright(&overcast, top_wall_h);
        assert!(
            clear_n >= 3,
            "clear should show a disc at a low sun hour, got {clear_n}"
        );
        assert_eq!(
            rain_n, 0,
            "rain should hide the disc entirely, like overcast, got {rain_n}"
        );
        assert_eq!(
            overcast_n, 0,
            "overcast should hide the disc entirely, got {overcast_n}"
        );
    }

    #[test]
    fn disc_clips_above_the_glass_at_the_arc_apex() {
        // Hold `top_wall_h` CONSTANT and vary only the hour, so the only
        // difference between the two renders is the sun's altitude:
        // `compute_disc`'s `cy` is `top_wall_h * (HORIZON_FRAC -
        // altitude*ARC_RISE_FRAC)`, and at the apex (12:00, altitude ≈0.99)
        // the bracket is solidly negative — the disc has climbed entirely
        // above the glass, regardless of `top_wall_h`'s size ("real low
        // window": the apex ALWAYS clips, by construction).
        let buf_w = 96u16;
        let top_wall_h = 40u16; // same height for both renders — only the HOUR varies
        let low = render_office_at(7, Weather::Clear, buf_w, top_wall_h); // altitude ~0.41, in-glass
        let apex = render_office_at(12, Weather::Clear, buf_w, top_wall_h); // altitude ~0.99, clipped
        let low_n = count_warm_bright(&low, top_wall_h);
        let apex_n = count_warm_bright(&apex, top_wall_h);
        assert!(low_n >= 3, "low sun should show a disc: {low_n}");
        assert_eq!(
            apex_n, 0,
            "the apex disc must clip entirely above the glass: {apex_n}"
        );
    }

    #[test]
    fn short_window_apex_does_not_panic() {
        // A SHORT window at the apex shrinks `window_h`/`glass_h` to their
        // floor while the disc's `cy` is solidly negative — must not panic.
        let _ = render_office_at(12, Weather::Clear, 96, 10);
    }

    #[test]
    fn disc_lands_in_a_window_never_on_the_wall_margin() {
        // Regression guard for the original wall-margin-vanish bug (the OLD
        // linear `cx` overshot the last painted pane onto blank wall). Now that
        // `compute_disc` maps azimuth across the real tiled span AND the disc is
        // gated to the window its centre is over, the disc can legitimately hide
        // behind an inter-window pillar at some hours — so it is NOT visible at
        // every hour. Across a sweep of low-sun hours it must, for every buffer
        // width: (a) appear inside a real window at least once (not perpetually
        // lost), and (b) NEVER paint a pixel past the last painted window (the
        // wall margin — the bug this guards).
        let top_wall_h = 40u16;
        let stride = (WINDOW_W + WINDOW_GAP) as f32;
        for buf_w in [76u16, 96, 120, 150, 192, 220, 300] {
            // Last painted window's right edge (mirrors compute_disc's tiling).
            let k_max = (((buf_w as f32) - WINDOW_W as f32 - 5.0) / stride).floor();
            let last_right = (3.0 + k_max.max(0.0) * stride + WINDOW_W as f32) as u16;
            let mut seen_in_a_window = false;
            for h in [5u32, 6, 7, 17, 18, 19] {
                let buf = render_office_at(h, Weather::Clear, buf_w, top_wall_h);
                for y in 1..(top_wall_h / 3).max(2) {
                    for x in 0..buf.width() {
                        let p = buf.get(x, y);
                        if p.r > 240 && p.r as i16 - p.b as i16 > 40 {
                            assert!(
                                x < last_right,
                                "buf_w={buf_w} h={h}: disc pixel at x={x} is past the \
                                 last window (wall margin; last right edge {last_right})"
                            );
                            seen_in_a_window = true;
                        }
                    }
                }
            }
            assert!(
                seen_in_a_window,
                "buf_w={buf_w}: the disc never appeared in a window across the low-sun sweep"
            );
        }
    }

    #[test]
    fn disc_sweeps_across_a_single_window_buffer() {
        // Regression guard for the old center-to-center mapping: with only
        // ONE window painted, `first_center == last_center` (both the same
        // window's centre), so `cx` was CONSTANT regardless of azimuth — the
        // disc froze on the shared mullion column instead of sweeping. The
        // new inset-span mapping must still sweep even on a single-window
        // buffer. buf_w=40 paints exactly one window (WINDOW_W=22 + a margin
        // too narrow for a second 22+3px pane).
        let buf_w = 40u16;
        let top_wall_h = 40u16;
        let morning = render_office_at(7, Weather::Clear, buf_w, top_wall_h);
        let evening = render_office_at(18, Weather::Clear, buf_w, top_wall_h);
        let warm_center_x = |buf: &RgbBuffer| -> f32 {
            let mut sum = 0u32;
            let mut count = 0u32;
            for y in 1..(top_wall_h / 3).max(2) {
                for x in 0..buf.width() {
                    let p = buf.get(x, y);
                    if p.r > 200 && p.r > p.b.saturating_add(40) {
                        sum += x as u32;
                        count += 1;
                    }
                }
            }
            assert!(count > 0, "expected a warm disc to render in this buffer");
            sum as f32 / count as f32
        };
        let morning_x = warm_center_x(&morning);
        let evening_x = warm_center_x(&evening);
        assert!(
            (morning_x - evening_x).abs() > 1.0,
            "the disc must sweep across a single-window buffer, not freeze on \
             the mullion: morning_x={morning_x} evening_x={evening_x}"
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

    #[test]
    fn stars_appear_on_a_clear_night_and_vanish_under_overcast() {
        // 02:00: deep night, near the moon's own night-arc apex, so its disc
        // clips (near-)entirely above the glass (see `moon_disc_shows_at_night`'s
        // doc comment on why THAT test uses 21:00 instead) — the only bright
        // thing left to find in the upper sky band is a star.
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let clear = render_office_at(2, Weather::Clear, buf_w, top_wall_h);
        let overcast = render_office_at(2, Weather::Overcast, buf_w, top_wall_h);
        let clear_n = count_faint_white(&clear, top_wall_h);
        let overcast_n = count_faint_white(&overcast, top_wall_h);
        assert!(
            clear_n >= 3,
            "a clear night should show some stars in the upper sky, got {clear_n}"
        );
        assert!(
            clear_n > overcast_n,
            "overcast (atmo.disc below STAR_MIN once multiplied by darkness) \
             should hide the stars a clear sky shows: clear={clear_n} overcast={overcast_n}"
        );
    }

    #[test]
    fn stars_gate_on_night_not_darkness_alone() {
        // The star gate must key on the sun being BELOW the horizon (night), not
        // on `darkness`. At 07:00 the sun is up but low, so a HIGH darkness (0.6)
        // is passed — yet stars must be OFF (else a full field wrongly shows at
        // dawn). Counting rendered pixels can't test this (the pale dawn sky is
        // itself "faint-white"), so assert the pure gate directly.
        let at = |h: u32| at_local(2026, 1, 1, h, 0);
        // Dawn: sun up (emitter is the Sun) → no stars regardless of darkness.
        assert_eq!(
            night_star_strength(at(7), 0.6, Weather::Clear),
            0.0,
            "no stars at 7am while the sun is up"
        );
        // Deep night, clear: sun down (emitter is the Moon) → stars visible.
        assert!(
            night_star_strength(at(2), 0.9, Weather::Clear) > STAR_MIN,
            "a clear night should light the stars"
        );
        // Night but overcast: the clear-sky factor (atmo.disc≈0.05) drops it
        // below STAR_MIN → the thick cloud hides the stars.
        assert!(
            night_star_strength(at(2), 0.9, Weather::Overcast) < STAR_MIN,
            "overcast should hide the stars even at night"
        );
    }

    #[test]
    fn disc_never_bleeds_across_a_window_pillar() {
        // Physics-audit repro: a disc whose `cx` lands near an inter-window gap
        // is wide enough (radius + glow) to reach the glass on BOTH sides of the
        // solid wall pillar (frame + WINDOW_GAP + frame). Before the per-window
        // gate it painted in both panes at once — the sun/moon showing THROUGH a
        // wall. The disc must light at most ONE window at any instant. A wide
        // buffer has many internal gaps; sweep the low-sun hours so `cx` passes
        // over one.
        let buf_w = 280u16;
        let top_wall_h = 40u16;
        let stride = (WINDOW_W + WINDOW_GAP) as i32;
        for h in [5u32, 6, 7, 17, 18, 19] {
            let buf = render_office_at(h, Weather::Clear, buf_w, top_wall_h);
            let mut wins = std::collections::HashSet::new();
            // Upper sky band only (top third) so the skyline's own lit city dots
            // can't masquerade as disc-core pixels.
            for y in 1..(top_wall_h / 3).max(2) {
                for x in 0..buf.width() {
                    let p = buf.get(x, y);
                    if !(p.r > 240 && p.r as i16 - p.b as i16 > 40) {
                        continue;
                    }
                    let rel = x as i32 - 3;
                    if rel < 0 {
                        continue;
                    }
                    if rel % stride < WINDOW_W as i32 {
                        wins.insert(rel / stride);
                    }
                }
            }
            assert!(
                wins.len() <= 1,
                "at {h}:00 the disc lit {} windows {:?} — it bled across a wall pillar",
                wins.len(),
                wins
            );
        }
    }

    #[test]
    fn crescent_moon_leaves_the_dark_limb_unlit() {
        // At 21:00 the moon disc sits low & in-glass at FULL atmo visibility
        // under Clear (`vis == atmo(Clear).disc == 1.0`), so every
        // disc-interior pixel becomes EXACTLY `theme.lighting.moon_core` (lit)
        // or EXACTLY `MOON_SHADOW` (the dark limb) — no partial blend to
        // muddy the count. The disc's (cx, cy, r) depend only on the hour
        // (not the date), so one `compute_disc` call gives the shared
        // bounding box for every day.
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let geom = compute_disc(
            at_local(2026, 1, 1, 21, 0),
            Weather::Clear,
            buf_w,
            top_wall_h,
            theme,
        )
        .expect("moon disc visible at 21:00 under Clear");

        let crescent_day = (1..=31u32)
            .find(|&d| sky::moon_phase(at_local(2026, 1, d, 21, 0)) < 0.35)
            .expect("a crescent night exists in January 2026");
        let full_day = (1..=31u32)
            .find(|&d| sky::moon_phase(at_local(2026, 1, d, 21, 0)) > 0.9)
            .expect("a near-full night exists in January 2026");

        // (dark-limb count, lit-bright count) within the disc proper.
        let count_dark_and_bright = |day: u32| -> (usize, usize) {
            let buf = render_office_on(day, 21, Weather::Clear, buf_w, top_wall_h);
            let r = geom.r.ceil() as i32;
            let (cx, cy) = (geom.cx.round() as i32, geom.cy.round() as i32);
            let mut dark = 0usize;
            let mut bright = 0usize;
            for py in (cy - r)..=(cy + r) {
                for px in (cx - r)..=(cx + r) {
                    if px < 0 || py < 0 || px as u16 >= buf.width() || py as u16 >= buf.height() {
                        continue;
                    }
                    let dx = px as f32 - geom.cx;
                    let dy = py as f32 - geom.cy;
                    if dx * dx + dy * dy > geom.r * geom.r {
                        continue; // outside the disc proper
                    }
                    let p = buf.get(px as u16, py as u16);
                    if p == MOON_SHADOW {
                        dark += 1;
                    } else if p.b > 200 && p.b > p.r.saturating_add(10) {
                        bright += 1;
                    }
                }
            }
            (dark, bright)
        };

        let (crescent_dark, crescent_bright) = count_dark_and_bright(crescent_day);
        let (full_dark, full_bright) = count_dark_and_bright(full_day);

        assert!(
            crescent_bright >= 2,
            "the crescent should still show a lit sliver, got {crescent_bright}"
        );
        assert!(
            crescent_dark >= 2,
            "the crescent should leave a dark limb unlit, got {crescent_dark}"
        );
        assert!(
            full_bright >= 2,
            "a near-full moon should be lit, got {full_bright}"
        );
        assert!(
            crescent_dark > full_dark,
            "a crescent should have strictly MORE dark-within-disc pixels than \
             a near-full moon: crescent={crescent_dark} full={full_dark}"
        );
        assert!(
            crescent_dark >= full_dark + 10,
            "assert a real margin, not a hair's-breadth win: \
             crescent={crescent_dark} full={full_dark}"
        );
    }

    #[test]
    fn moon_glow_dims_at_new_moon() {
        // The glow halo must track the phase: a new moon's near-dark core
        // should cast (almost) no ring, unlike a full moon's bright one.
        // Search January 2026 at 21:00 for the min/max illuminated fraction
        // (mirrors `moon_luminance_tracks_phase` in sky.rs).
        let buf_w = 96u16;
        let top_wall_h = 40u16;
        let (mut new_moon_day, mut new_moon_frac) = (1u32, f32::MAX);
        let (mut full_moon_day, mut full_moon_frac) = (1u32, f32::MIN);
        for day in 1..=31u32 {
            let frac = sky::moon_phase(at_local(2026, 1, day, 21, 0));
            if frac < new_moon_frac {
                new_moon_frac = frac;
                new_moon_day = day;
            }
            if frac > full_moon_frac {
                full_moon_frac = frac;
                full_moon_day = day;
            }
        }

        // Count faint cool "glow ring" pixels — a softer bar than
        // `count_cool_bright`'s core threshold, catching the halo blend
        // around the disc rather than requiring a fully-opaque core hit.
        let count_glow_ring = |buf: &RgbBuffer| -> usize {
            (1..(top_wall_h / 3).max(2))
                .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let p = buf.get(x, y);
                    p.b > 90 && p.b > p.r.saturating_add(5)
                })
                .count()
        };

        let new_moon_buf = render_office_on(new_moon_day, 21, Weather::Clear, buf_w, top_wall_h);
        let full_moon_buf = render_office_on(full_moon_day, 21, Weather::Clear, buf_w, top_wall_h);
        let new_moon_glow = count_glow_ring(&new_moon_buf);
        let full_moon_glow = count_glow_ring(&full_moon_buf);
        assert!(
            new_moon_glow < full_moon_glow,
            "a new moon's glow ring (phase={new_moon_frac}) should show fewer/dimmer \
             cool pixels than a full moon's (phase={full_moon_frac}): \
             new={new_moon_glow} full={full_moon_glow}"
        );
    }
}
