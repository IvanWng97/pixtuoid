//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass. Helpers are
//! `pub(super)` so the orchestrator (`pixel_painter/mod.rs`) can call
//! them in the order it wants.

mod lighting;
mod time_of_day;

// Re-export everything the parent pixel_painter/mod.rs imports.
pub(super) use lighting::{
    paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_panel, paint_shadow, Ellipse,
};
pub(super) use time_of_day::{
    daylight_floor_overlay, dim_floor_overlay, sun_on_wall, sunset_strength, time_of_day_look,
    weather_light, weather_state, TimeOfDayLook, WallSide, Weather,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::ambient::SunbeamColumn;
use super::epoch_ms;
use super::palette::{blend, blend_rgb, lerp_rgb};

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

use crate::tui::layout::{Layout, ELEVATOR_W};
use crate::tui::theme::Theme;

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
    for y in 0..buf.height {
        for x in 0..buf.width {
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
pub(in crate::tui::pixel_painter) fn window_spill_columns(layout: &Layout) -> Vec<SunbeamColumn> {
    let top_wall_h = layout
        .top_margin
        .saturating_sub(pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN);
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
                look,
                idx as u16,
                now,
                theme,
                weather,
                altitude,
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
    (h % 100) < 75
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
        let end_x = (base_x + window_w + widen).min(buf.width);
        let y = top_y + dy;
        if y >= buf.height {
            break;
        }
        let strength = fade_start * (1.0 - dy as f32 / SPILL_DEPTH as f32);
        for x in start_x..end_x {
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, warm, strength));
        }
    }
}

/// Floor-to-ceiling window with frame, mullion, and a procedural city view
/// inside the glass. Sky gradient at top blends with time-of-day glass
/// colors; the lower portion shows building silhouettes whose "windows"
/// (1-pixel dots) light up at night and twinkle on a per-dot cycle so the
/// skyline reads as alive instead of stamped.
#[allow(clippy::too_many_arguments)]
fn paint_floor_to_ceiling_window(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    frame: Rgb,
    look: &TimeOfDayLook,
    window_idx: u16,
    now: SystemTime,
    theme: &Theme,
    weather: Weather,
    altitude: f32,
) {
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
        lerp_rgb(dark_window, cw[0], lit_strength),
        lerp_rgb(dark_window, cw[1], lit_strength),
        lerp_rgb(dark_window, cw[2], lit_strength),
    ];
    let building = lerp_rgb(building_light, building_dark, look.darkness);

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
    let sky_norm = (glass_h as f32) * 0.7;
    let sky_row: Vec<Rgb> = (0..glass_h)
        .map(|gy| {
            let sky_t = (gy as f32 / sky_norm).min(1.0);
            lerp_rgb(look.glass_b, look.glass_a, sky_t)
        })
        .collect();

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
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
                buf.put(px, py, sky_row[glass_dy as usize]);
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
                if px < buf.width && py < buf.height {
                    let cur = buf.get(px, py);
                    buf.put(px, py, blend_rgb(cur, haze, alpha));
                }
            }
        }
    }

    let elapsed_ms = epoch_ms(now);

    match weather {
        Weather::Rain => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..4u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 60 + (seed.wrapping_mul(0x4f6c_dd1d) % 50);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 3 + (seed % 2) as u16;
                let px = glass_x0 + sx;
                for dy in 0..len {
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.35 - (dy as f32 / len as f32) * 0.15;
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            blend_rgb(
                                cur,
                                Rgb {
                                    r: 210,
                                    g: 220,
                                    b: 240,
                                },
                                alpha,
                            ),
                        );
                    }
                }
            }
        }
        Weather::Storm => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..6u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 40 + (seed.wrapping_mul(0x4f6c_dd1d) % 40);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 4 + (seed % 3) as u16;
                let px = glass_x0 + sx;
                for dy in 0..len {
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.6 - (dy as f32 / len as f32) * 0.3;
                        buf.put(
                            px,
                            py,
                            Rgb {
                                r: blend(buf.get(px, py).r, 210, alpha),
                                g: blend(buf.get(px, py).g, 220, alpha),
                                b: blend(buf.get(px, py).b, 245, alpha),
                            },
                        );
                    }
                }
            }
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
                        if px < buf.width && py < buf.height {
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
        Weather::Snow => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for flake in 0..3u64 {
                let seed = window_idx as u64 * 11 + flake;
                let sx = (seed.wrapping_mul(0x517c_c1b7) % gw as u64) as u16;
                let speed = 150 + (seed.wrapping_mul(0x4f6c_dd1d) % 100);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let wiggle = if (elapsed_ms / 400 + seed.wrapping_mul(0x9e37)) % 2 == 0 {
                    0
                } else {
                    1
                };
                let px = glass_x0 + (sx + wiggle) % gw;
                let py = glass_y0 + phase as u16;
                if px < buf.width && py < buf.height {
                    buf.put(
                        px,
                        py,
                        Rgb {
                            r: 240,
                            g: 240,
                            b: 250,
                        },
                    );
                }
            }
        }
        Weather::Fog => {
            for dy in 1..h.saturating_sub(1) {
                for dx in 1..w.saturating_sub(1) {
                    let px = x + dx;
                    let py = y + dy;
                    if px < buf.width && py < buf.height {
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            blend_rgb(
                                cur,
                                Rgb {
                                    r: 160,
                                    g: 165,
                                    b: 175,
                                },
                                0.25,
                            ),
                        );
                    }
                }
            }
        }
        Weather::Overcast => {
            for dy in 1..h.saturating_sub(1) {
                for dx in 1..w.saturating_sub(1) {
                    let px = x + dx;
                    let py = y + dy;
                    if px < buf.width && py < buf.height {
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            blend_rgb(
                                cur,
                                Rgb {
                                    r: 100,
                                    g: 105,
                                    b: 110,
                                },
                                0.2,
                            ),
                        );
                    }
                }
            }
        }
        Weather::Windy => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..5u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 50 + (seed.wrapping_mul(0x4f6c_dd1d) % 40);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 3 + (seed % 2) as u16;
                for dy in 0..len {
                    let drift = dy / 2;
                    let px = glass_x0 + (sx + drift) % gw;
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.35 - (dy as f32 / len as f32) * 0.15;
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            blend_rgb(
                                cur,
                                Rgb {
                                    r: 210,
                                    g: 220,
                                    b: 240,
                                },
                                alpha,
                            ),
                        );
                    }
                }
            }
        }
        Weather::Smog => {
            // Warm-yellow desaturated haze across the full glass. Heavier
            // than Fog and noticeably warmer — pulls the city behind a
            // sodium-lit veil.
            for dy in 1..h.saturating_sub(1) {
                for dx in 1..w.saturating_sub(1) {
                    let px = x + dx;
                    let py = y + dy;
                    if px < buf.width && py < buf.height {
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            blend_rgb(
                                cur,
                                Rgb {
                                    r: 180,
                                    g: 160,
                                    b: 110,
                                },
                                0.30,
                            ),
                        );
                    }
                }
            }
        }
        Weather::Clear => {}
    }

    let raw_sunset = sunset_strength(now);
    let twilight_now = look.twilight;
    // Golden-hour blaze on the city silhouette is attenuated by atmo —
    // clouds scatter the direct warm light away (Storm at sunset reaches
    // only ~25% of Clear's strength), Smog amplifies the warm cast by 1.4×
    // for the sodium-lit "Blade Runner" sunset.
    let atmo = weather_light(weather);
    let smog_boost = if matches!(weather, Weather::Smog) {
        1.4
    } else {
        1.0
    };
    let sunset =
        (raw_sunset * (1.0 - twilight_now * 0.8) * atmo.intensity * smog_boost).clamp(0.0, 1.0);
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
                if px < buf.width && py < buf.height {
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
}
