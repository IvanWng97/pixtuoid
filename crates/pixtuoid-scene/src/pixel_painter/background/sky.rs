//! Time-of-day derived state — the sky emitter (sun/moon), weather-as-atmosphere
//! transmission, glass colors, sunlight spill, and the floor tint overlays.

use std::cell::Cell;
use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::pixel_painter::palette::{blend_rgb, mix_lab, RgbLut};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::pixel_painter) enum Weather {
    Clear,
    Rain,
    Storm,
    Snow,
    Fog,
    Overcast,
    Windy,
    Smog,
}

impl Weather {
    /// All variants, in canonical order. The site's gallery manifest
    /// (site/src/weather.json) mirrors it; `weather_gallery_manifest_matches_the_weather_enum`
    /// fails on any add/rename here until the manifest (+ gen-media art) follows.
    pub(in crate::pixel_painter) const ALL: [Weather; 8] = [
        Weather::Clear,
        Weather::Rain,
        Weather::Storm,
        Weather::Snow,
        Weather::Fog,
        Weather::Overcast,
        Weather::Windy,
        Weather::Smog,
    ];

    /// Lowercase CLI name (`Weather::Rain` → `"rain"`).
    pub(in crate::pixel_painter) const fn name(self) -> &'static str {
        match self {
            Weather::Clear => "clear",
            Weather::Rain => "rain",
            Weather::Storm => "storm",
            Weather::Snow => "snow",
            Weather::Fog => "fog",
            Weather::Overcast => "overcast",
            Weather::Windy => "windy",
            Weather::Smog => "smog",
        }
    }

    /// Parse a CLI name (case-insensitive) back to a variant.
    pub(in crate::pixel_painter) fn from_name(s: &str) -> Option<Weather> {
        let s = s.trim().to_ascii_lowercase();
        Weather::ALL.into_iter().find(|w| w.name() == s)
    }
}

thread_local! {
    /// Screenshot/test affordance: when `Some`, every `weather_state` call on
    /// this thread returns it. Production never sets it (only
    /// `snapshot --weather`), so live rendering is byte-identical.
    static WEATHER_OVERRIDE: Cell<Option<Weather>> = const { Cell::new(None) };
}

pub(in crate::pixel_painter) fn set_weather_override(w: Option<Weather>) {
    WEATHER_OVERRIDE.with(|c| c.set(w));
}

pub(in crate::pixel_painter) fn weather_state(now: SystemTime) -> Weather {
    if let Some(forced) = WEATHER_OVERRIDE.with(Cell::get) {
        return forced;
    }
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    const WEATHER_CYCLE_SECS: u64 = 600;
    let cycle = secs / WEATHER_CYCLE_SECS;
    // splitmix64 finalizer, open-coded by deliberate choice (see `strike_offset`
    // in background/mod.rs).
    let mut h = cycle.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    match h % 15 {
        0..=5 => Weather::Clear,
        6..=7 => Weather::Rain,
        8 => Weather::Storm,
        9 => Weather::Snow,
        10 => Weather::Fog,
        11..=12 => Weather::Overcast,
        13 => Weather::Windy,
        _ => Weather::Smog,
    }
}

// Weights folding the two transmission channels into one interior illuminance,
// calibrated so a CLEAR noon lands at full brightness (K_BEAM + 0.55·K_FILL ≈ 1).
const K_BEAM: f32 = 0.70;
const K_FILL: f32 = 0.55;
// Max window-spill horizontal lean (px/row) at the low-sun extremes.
const SPILL_SLANT_MAX: f32 = 0.7;

/// Direct-beam strength reaching the interior = emitter luminance carried by the
/// weather's DIRECT transmission. Zero at night (the moon casts no usable beam)
/// and under thick cloud.
pub(in crate::pixel_painter) fn beam_strength(now: SystemTime) -> f32 {
    let sky = emitter(now);
    match sky.body {
        Body::Sun => sky.emitter_lum * atmo(weather_state(now)).direct,
        Body::Moon => 0.0,
    }
}

/// City-light bounce reaching the interior at night — a small, weather-keyed
/// FLOOR so the room is never pitch black even at a new moon. Snow albedo bounces
/// the most; a storm swallows it. Independent of the moon's (date-varying) phase,
/// so the night weather-ordering is phase-stable.
fn city_bounce(w: Weather) -> f32 {
    // Magnitudes stay low enough that a moonlit night's floor can never
    // out-light a stormy solar noon (`solar_noon_outshines_the_brightest_night`).
    let v = match w {
        Weather::Snow => 0.08,
        Weather::Clear => 0.055,
        Weather::Windy => 0.05,
        Weather::Fog => 0.045,
        Weather::Smog => 0.045,
        Weather::Overcast => 0.035,
        Weather::Rain => 0.03,
        Weather::Storm => 0.015,
    };
    debug_assert!(
        (0.0..=1.0).contains(&v),
        "city_bounce out of range: {w:?} -> {v}"
    );
    v
}

/// Weather as an ATMOSPHERE: how much of the emitter's light survives to the
/// interior, split into a hard directional beam, a flat diffuse fill, and the
/// disc's own visibility through the medium.
#[derive(Debug, Clone, Copy)]
pub(in crate::pixel_painter) struct Atmo {
    pub(in crate::pixel_painter) direct: f32,
    pub(in crate::pixel_painter) diffuse: f32,
    pub(in crate::pixel_painter) disc: f32,
}

pub(in crate::pixel_painter) fn atmo(w: Weather) -> Atmo {
    // Storm < Rain in BOTH transmission channels (denser cloud), and
    // Overcast/Rain/Storm share one near-zero disc (below `MIN_DISC_VIS`) so a
    // thicker cloud never shows MORE of the disc than a thinner one.
    let (direct, diffuse, disc) = match w {
        Weather::Clear => (1.00, 0.55, 1.00),
        Weather::Windy => (0.90, 0.55, 0.95),
        Weather::Snow => (0.25, 0.70, 0.30),
        Weather::Smog => (0.30, 0.45, 0.45),
        Weather::Fog => (0.05, 0.75, 0.10),
        Weather::Overcast => (0.00, 0.50, 0.05),
        Weather::Rain => (0.00, 0.40, 0.05),
        Weather::Storm => (0.00, 0.28, 0.05),
    };
    debug_assert!(
        [direct, diffuse, disc]
            .iter()
            .all(|c| (0.0..=1.0).contains(c)),
        "Atmo channels must be 0..=1: {w:?} -> ({direct}, {diffuse}, {disc})"
    );
    Atmo {
        direct,
        diffuse,
        disc,
    }
}

/// Window glass color + spill intensity + spill slant for the current local
/// hour. `spill_slant` is x-shift per row going down; `darkness` is
/// 1 - daylight, which drives the artificial-light effects.
pub(in crate::pixel_painter) struct TimeOfDayLook {
    pub(in crate::pixel_painter) glass_a: Rgb,
    pub(in crate::pixel_painter) glass_b: Rgb,
    pub(in crate::pixel_painter) spill_strength: f32,
    pub(in crate::pixel_painter) spill_slant: f32,
    pub(in crate::pixel_painter) darkness: f32,
    /// The cast this hour + weather puts on a LIT OBJECT: the cool night term
    /// then the warm day one, applied in order like the floor's two overlays.
    pub(in crate::pixel_painter) object_wash: [(Rgb, f32); 2],
}

pub(in crate::pixel_painter) fn time_of_day_look(now: SystemTime, theme: &Theme) -> TimeOfDayLook {
    let sky = emitter(now);
    let a = atmo(weather_state(now));
    // The moon casts no USABLE direct beam (mirrors `beam_strength`'s gate) — a
    // moonlit night must never out-light a cloudy solar noon, so the moon's
    // illuminance is diffuse-fill only.
    let direct_eff = match sky.body {
        Body::Sun => a.direct,
        Body::Moon => 0.0,
    };
    let interior = (sky.emitter_lum * (direct_eff * K_BEAM + a.diffuse * K_FILL)).clamp(0.0, 1.0);
    let night_floor = match sky.body {
        Body::Moon => city_bounce(weather_state(now)),
        Body::Sun => 0.0,
    };
    let exterior = (interior + night_floor).min(1.0);

    let day_a = theme.lighting.day_sky_a;
    let day_b = theme.lighting.day_sky_b;
    let night_a = theme.lighting.night_sky_a;
    let night_b = theme.lighting.night_sky_b;
    let twilight_a = theme.lighting.twilight_a;
    let twilight_b = theme.lighting.twilight_b;

    let warm = (sky.warmth * interior).clamp(0.0, 1.0);
    let glass_a = mix_lab(mix_lab(night_a, day_a, exterior), twilight_a, warm * 0.5);
    let glass_b = mix_lab(mix_lab(night_b, day_b, exterior), twilight_b, warm * 0.5);

    // Azimuth runs 0=east/dawn .. 1=west/dusk, so the morning sun casts light
    // leftward (negative slant) and the evening sun rightward.
    let (spill_strength, spill_slant) = match sky.body {
        Body::Sun => (interior, (sky.azimuth - 0.5) * 2.0 * SPILL_SLANT_MAX),
        Body::Moon => (0.0, 0.0),
    };

    // Below the floor's own share: a sprite carries art contrast a full-strength pass would swallow.
    const OBJECT_WASH_SHARE: f32 = 0.55;
    let darkness = 1.0 - exterior;
    // SUPERPOSED, never chosen between: the floor runs both overlays every frame,
    // so picking one arm on `interior >= darkness` stepped every object 16-27 luma
    // in the frame that crossed it while the floor slid smoothly under them.
    let object_wash = [
        (
            theme.lighting.night_tint,
            darkness * NIGHT_FLOOR_DIM * OBJECT_WASH_SHARE,
        ),
        (SUN_TINT, interior * DAYLIGHT_FLOOR_LIFT * OBJECT_WASH_SHARE),
    ];

    TimeOfDayLook {
        glass_a,
        glass_b,
        spill_strength,
        spill_slant,
        darkness,
        object_wash,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pixel_painter) enum WallSide {
    East,
    South,
    West,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::pixel_painter) struct SunSpot {
    pub wall: WallSide,
    /// 0.0..=1.0 along the wall (left→right for South, top→bottom for East/West).
    pub along: f32,
    /// 0.0=dim, 1.0=brightest at noon.
    pub intensity: f32,
    /// 0.0=neutral white (noon), 1.0=very warm gold (sunrise/sunset).
    pub warmth: f32,
}

/// Azimuth band boundaries partitioning the sun's E->W arc onto the office
/// walls: `0.0..AZ_EAST_MAX` = east wall (morning), `AZ_EAST_MAX..AZ_WEST_MIN`
/// = south/window wall (midday), `AZ_WEST_MIN..1.0` = west wall (evening).
const AZ_EAST_MAX: f32 = 0.30;
const AZ_WEST_MIN: f32 = 0.70;

pub(in crate::pixel_painter) fn sun_on_wall(now: SystemTime) -> Option<SunSpot> {
    let sky = emitter(now);
    if !matches!(sky.body, Body::Sun) {
        return None;
    }
    // The SAME azimuth that places the disc and leans the floor spill, so the
    // wall, the disc, and the spill direction can never disagree.
    let az = sky.azimuth;
    let (wall, along) = if az < AZ_EAST_MAX {
        (WallSide::East, az / AZ_EAST_MAX)
    } else if az < AZ_WEST_MIN {
        (
            WallSide::South,
            (az - AZ_EAST_MAX) / (AZ_WEST_MIN - AZ_EAST_MAX),
        )
    } else {
        (WallSide::West, (az - AZ_WEST_MIN) / (1.0 - AZ_WEST_MIN))
    };
    Some(SunSpot {
        wall,
        along,
        intensity: sky.altitude,
        warmth: sky.warmth,
    })
}

/// Blend `tint` over every floor pixel in the band `top_y..bottom_y` at an
/// ALREADY-CLAMPED strength `s`. `s <= 0.0` early-returns: byte-identical to
/// blending, but it skips the whole pass every clear frame. Tint and strength
/// are constant across the band, so the blend runs through an [`RgbLut`] —
/// byte-identical to per-pixel [`blend_rgb`], measured ~28% of a frame down
/// to a fraction (#900's profile).
fn blend_floor_band(buf: &mut RgbBuffer, top_y: u16, bottom_y: u16, tint: Rgb, s: f32) {
    if s <= 0.0 {
        return;
    }
    let lut = RgbLut::tabulate(|c| blend_rgb(c, tint, s));
    let w = buf.width() as usize;
    let start = (top_y.min(buf.height()) as usize) * w;
    let end = (bottom_y.min(buf.height()) as usize) * w;
    if start >= end {
        return;
    }
    for px in &mut buf.as_mut_slice()[start..end] {
        *px = lut.apply(*px);
    }
}

/// Multiplicative dim applied to floor pixels at night — pulls everything toward
/// a dark navy so the artificial-light pools have something to stand out against.
pub(in crate::pixel_painter) fn dim_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
    theme: &Theme,
) {
    let s = strength.clamp(0.0, 0.55);
    blend_floor_band(buf, top_y, bottom_y, theme.lighting.night_tint, s);
}

/// How far a fully dark hour dims the interior.
pub(in crate::pixel_painter) const NIGHT_FLOOR_DIM: f32 = 0.45;

/// How far a fully lit hour lifts it.
pub(in crate::pixel_painter) const DAYLIGHT_FLOOR_LIFT: f32 = 0.22;

/// Pale warm midday sunlight — theme-agnostic, since daylight is daylight.
const SUN_TINT: Rgb = Rgb {
    r: 255,
    g: 246,
    b: 224,
};

/// Warm sunlight LIFT on the floor — the daytime mirror of [`dim_floor_overlay`],
/// and the model's only positive day term (without it a clear noon leaves the
/// floor at its plain brownish base). Sun enters regardless of occupancy, so —
/// unlike the dim — this is NOT scaled by the empty-floor boost.
pub(in crate::pixel_painter) fn daylight_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
) {
    let s = strength.clamp(0.0, 0.40);
    blend_floor_band(buf, top_y, bottom_y, SUN_TINT, s);
}

/// The physical sky emitter — sun by day, moon by night. Luminance + warmth
/// follow altitude (low body = longer air path = dimmer + warmer). The ONE
/// source the interior light + the disc derive from.
pub(in crate::pixel_painter) enum Body {
    Sun,
    Moon,
}

pub(in crate::pixel_painter) struct SkyState {
    pub(in crate::pixel_painter) body: Body,
    pub(in crate::pixel_painter) altitude: f32, // 0 horizon .. 1 apex
    pub(in crate::pixel_painter) azimuth: f32,  // 0 (east/dawn) .. 1 (west/dusk)
    pub(in crate::pixel_painter) warmth: f32,   // 0 neutral(apex) .. 1 warm/red(horizon)
    pub(in crate::pixel_painter) emitter_lum: f32, // 0..1 luminance reaching the atmosphere
}

// Sun rides the arc over its up-span; the moon owns the complementary night span.
const SUN_RISE_H: f32 = 5.0;
const SUN_SET_H: f32 = 20.0;
/// Moon luminance at a full phase — low enough that a full-moon midnight (plus
/// the `city_bounce` floor) still stays dimmer than a stormy solar noon; see
/// `solar_noon_outshines_the_brightest_night`.
const MOON_PEAK_LUM: f32 = 0.12;
/// Synodic month (days) + a known new-moon epoch (unix days) for the phase calc.
const SYNODIC_DAYS: f32 = 29.530_588;
const NEW_MOON_EPOCH_UNIX_DAYS: f32 = 18_231.0; // 2019-11-27 new moon (unix day index)

fn arc_progress(h: f32, rise: f32, set: f32) -> f32 {
    ((h - rise) / (set - rise)).clamp(0.0, 1.0)
}

/// Whether the sky shows the SUN (not the moon) at hour-of-day `h` (0..24) — the
/// ONE definition of the day/night boundary, so `emitter` and any external
/// consumer can't drift from a second hardcoded copy.
pub(crate) fn hour_is_day(h: f32) -> bool {
    (SUN_RISE_H..SUN_SET_H).contains(&h)
}

pub(in crate::pixel_painter) fn emitter(now: SystemTime) -> SkyState {
    let h = super::local_hour_frac(now);
    let is_day = hour_is_day(h);
    let (rise, set) = if is_day {
        (SUN_RISE_H, SUN_SET_H)
    } else {
        // Night span wraps midnight: dusk(20:00) -> next dawn(05:00) = 9h.
        (SUN_SET_H, SUN_RISE_H + 24.0)
    };
    let h_lin = if is_day || h >= SUN_SET_H {
        h
    } else {
        h + 24.0
    };
    let t = arc_progress(h_lin, rise, set);
    let altitude = (std::f32::consts::PI * t).sin();
    let warmth = (1.0 - altitude).clamp(0.0, 1.0);
    let (body, emitter_lum) = if is_day {
        (Body::Sun, altitude)
    } else {
        (Body::Moon, MOON_PEAK_LUM * altitude * moon_phase(now))
    };
    SkyState {
        body,
        altitude,
        azimuth: t,
        warmth,
        emitter_lum,
    }
}

/// Illuminated fraction of the moon (0 new .. 1 full), from the synodic month.
pub(in crate::pixel_painter) fn moon_phase(now: SystemTime) -> f32 {
    let unix_days = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f32() / 86_400.0)
        .unwrap_or(0.0);
    let age = (unix_days - NEW_MOON_EPOCH_UNIX_DAYS).rem_euclid(SYNODIC_DAYS);
    (1.0 - (std::f32::consts::TAU * age / SYNODIC_DAYS).cos()) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_floor_band_tints_only_the_band_and_noops_at_zero() {
        let base = Rgb {
            r: 100,
            g: 100,
            b: 100,
        };
        let tint = Rgb { r: 0, g: 0, b: 0 };
        let mut buf = RgbBuffer::filled(3, 4, base);
        blend_floor_band(&mut buf, 1, 3, tint, 0.0);
        for y in 0..4 {
            for x in 0..3 {
                assert_eq!(buf.get(x, y), base, "s=0 leaves ({x},{y}) untouched");
            }
        }
        blend_floor_band(&mut buf, 1, 3, tint, 0.5);
        let blended = blend_rgb(base, tint, 0.5);
        for x in 0..3 {
            assert_eq!(buf.get(x, 0), base, "row above the band untouched");
            assert_eq!(buf.get(x, 1), blended);
            assert_eq!(buf.get(x, 2), blended);
            assert_eq!(buf.get(x, 3), base, "bottom_y is exclusive");
        }
    }

    #[test]
    fn blend_floor_band_matches_the_per_pixel_blend_reference() {
        let mut lcg = 0x9E3779B9u32;
        let mut next = || {
            lcg = lcg.wrapping_mul(1664525).wrapping_add(1013904223);
            Rgb {
                r: (lcg >> 24) as u8,
                g: (lcg >> 16) as u8,
                b: (lcg >> 8) as u8,
            }
        };
        let tints = [
            Rgb {
                r: 255,
                g: 244,
                b: 214,
            },
            Rgb {
                r: 24,
                g: 32,
                b: 64,
            },
            Rgb { r: 0, g: 0, b: 0 },
        ];
        for tint in tints {
            for s in [0.001f32, 0.22, 0.45, 0.999, 1.0] {
                let (w, h) = (67u16, 11u16);
                let mut buf = RgbBuffer::filled(w, h, Rgb { r: 0, g: 0, b: 0 });
                for y in 0..h {
                    for x in 0..w {
                        buf.put(x, y, next());
                    }
                }
                let mut expected = buf.clone();
                for y in 2..9u16 {
                    for x in 0..w {
                        expected.put(x, y, blend_rgb(expected.get(x, y), tint, s));
                    }
                }
                blend_floor_band(&mut buf, 2, 9, tint, s);
                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            buf.get(x, y),
                            expected.get(x, y),
                            "({x},{y}) diverged from per-pixel blend_rgb at tint {tint:?} s {s}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn daylight_floor_overlay_brightens_at_positive_strength() {
        let mut buf = RgbBuffer::filled(
            4,
            10,
            Rgb {
                r: 50,
                g: 50,
                b: 50,
            },
        );
        daylight_floor_overlay(&mut buf, 2, 10, 0.30);
        for y in 2..10u16 {
            for x in 0..4u16 {
                assert!(
                    buf.get(x, y).r > 50,
                    "floor pixel ({x},{y}) should brighten"
                );
            }
        }
    }

    #[test]
    fn daylight_floor_overlay_is_noop_at_zero_strength() {
        let mut buf = RgbBuffer::filled(
            4,
            10,
            Rgb {
                r: 80,
                g: 90,
                b: 100,
            },
        );
        daylight_floor_overlay(&mut buf, 2, 10, 0.0);
        for y in 2..10u16 {
            for x in 0..4u16 {
                assert_eq!(
                    buf.get(x, y),
                    Rgb {
                        r: 80,
                        g: 90,
                        b: 100
                    },
                    "zero strength must not mutate pixels"
                );
            }
        }
    }

    use crate::localclock::{at_hour_min, on_day};

    /// Local `h:m` on the reference day — `localclock` owns the construction.
    fn at_hour(h: u32, m: u32) -> SystemTime {
        at_hour_min(h, m)
    }

    /// Local 02:00 (always night) on a given January day. Weather varies by day
    /// at a fixed hour, so searching days finds different weathers/moon phases.
    fn night_on(day: u32) -> SystemTime {
        on_day(day, 2)
    }

    /// Local midnight on a given January day — near the night arc's apex, so
    /// it's close to the brightest instant of that night.
    fn midnight_on(day: u32) -> SystemTime {
        on_day(day, 0)
    }

    #[test]
    fn night_darkness_tracks_weather_at_fixed_phase() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        let theme = crate::theme::ALL_THEMES[0];
        let night = night_on(1); // fixed instant -> fixed moon phase; only weather varies
        set_weather_override(Some(Weather::Clear));
        let clear = time_of_day_look(night, theme).darkness;
        set_weather_override(Some(Weather::Storm));
        let storm = time_of_day_look(night, theme).darkness;
        set_weather_override(None);
        assert!(
            clear < storm,
            "clear night brighter than storm night at equal phase: {clear} vs {storm}"
        );
        assert!(storm < 1.0, "storm night keeps some city glow: {storm}");
        set_weather_override(Some(Weather::Clear));
        let noon = time_of_day_look(at_hour(12, 0), theme).darkness;
        set_weather_override(None);
        assert!(noon < 0.1, "clear noon ~fully lit: {noon}");
    }

    #[test]
    fn interior_brightness_is_altitude_coupled() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        let theme = crate::theme::ALL_THEMES[0];
        set_weather_override(Some(Weather::Storm));
        let noon = time_of_day_look(at_hour(12, 0), theme).darkness;
        let dusk = time_of_day_look(at_hour(18, 0), theme).darkness;
        set_weather_override(None);
        assert!(
            noon < dusk,
            "a stormy noon out-lights a stormy dusk: {noon} vs {dusk}"
        );
    }

    #[test]
    fn solar_noon_outshines_the_brightest_night() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        let theme = crate::theme::ALL_THEMES[0];

        // Snow/Clear at the FULLEST moon are the two brightest cases night can
        // offer — the highest `city_bounce` floor plus peak lunar illumination.
        let full_moon_day = (1..=31u32)
            .max_by(|&a, &b| {
                moon_phase(night_on(a))
                    .partial_cmp(&moon_phase(night_on(b)))
                    .expect("moon_phase is never NaN")
            })
            .expect("January has days");
        let full_moon_midnight = midnight_on(full_moon_day);

        set_weather_override(Some(Weather::Storm));
        let storm_noon = time_of_day_look(at_hour(12, 0), theme).darkness;

        set_weather_override(Some(Weather::Clear));
        let clear_full_moon = time_of_day_look(full_moon_midnight, theme).darkness;

        set_weather_override(Some(Weather::Snow));
        let snow_full_moon = time_of_day_look(full_moon_midnight, theme).darkness;

        set_weather_override(None);

        assert!(
            storm_noon < clear_full_moon,
            "a stormy solar noon must outshine even a clear full-moon midnight: \
             storm_noon darkness={storm_noon} vs clear_full_moon={clear_full_moon}"
        );
        assert!(
            storm_noon < snow_full_moon,
            "a stormy solar noon must outshine even a snow-lit full-moon midnight \
             (snow has the highest city_bounce floor): \
             storm_noon darkness={storm_noon} vs snow_full_moon={snow_full_moon}"
        );
    }

    #[test]
    fn sun_on_wall_east_at_morning() {
        let s = sun_on_wall(at_hour(7, 0)).expect("sun should be up at 07:00");
        assert_eq!(s.wall, WallSide::East);
        assert!(s.warmth > 0.5, "morning sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_overhead_at_noon() {
        let s = sun_on_wall(at_hour(12, 0)).expect("sun should be up at 12:00");
        assert_eq!(s.wall, WallSide::South);
        assert!(
            s.intensity > 0.85,
            "noon sun should be intense: {}",
            s.intensity
        );
    }

    #[test]
    fn sun_on_wall_west_at_evening() {
        let s = sun_on_wall(at_hour(18, 0)).expect("sun should be up at 18:00");
        assert_eq!(s.wall, WallSide::West);
        assert!(s.warmth > 0.55, "evening sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_none_at_midnight() {
        assert!(sun_on_wall(at_hour(0, 0)).is_none());
    }

    #[test]
    fn weather_state_emits_every_variant_within_a_week() {
        use std::collections::HashSet;
        use std::time::Duration;
        let start = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut seen: HashSet<Weather> = HashSet::new();
        for slot in 0..(7u64 * 24 * 6) {
            seen.insert(weather_state(start + Duration::from_secs(slot * 600)));
        }
        for w in [
            Weather::Clear,
            Weather::Rain,
            Weather::Storm,
            Weather::Snow,
            Weather::Fog,
            Weather::Overcast,
            Weather::Windy,
            Weather::Smog,
        ] {
            assert!(
                seen.contains(&w),
                "weather_state never emitted {w:?} in a week of slots"
            );
        }
    }

    #[test]
    fn weather_name_round_trips_for_every_variant() {
        for w in Weather::ALL {
            assert_eq!(Weather::from_name(w.name()), Some(w), "{w:?} round-trips");
        }
        assert_eq!(Weather::from_name("  SNOW "), Some(Weather::Snow));
        assert_eq!(Weather::from_name("drizzle"), None);
    }

    #[test]
    fn emitter_is_sun_by_day_moon_by_night_never_both() {
        for slot in 0..48u32 {
            let (h, m) = (slot / 2, (slot % 2) * 30);
            let s = at_hour(h, m);
            let e = emitter(s);
            match e.body {
                Body::Sun => assert!(
                    (5.0..20.0).contains(&(h as f32 + m as f32 / 60.0)),
                    "sun only during the daylight ramp, got {h}:{m:02}"
                ),
                Body::Moon => assert!(
                    !(5.0..20.0).contains(&(h as f32 + m as f32 / 60.0)),
                    "moon only when the sun is down, got {h}:{m:02}"
                ),
            }
        }
    }

    #[test]
    fn sun_altitude_peaks_near_midday_and_bottoms_at_the_horizon() {
        let noon = emitter(at_hour(12, 30)).altitude;
        let dawn = emitter(at_hour(6, 30)).altitude;
        let dusk = emitter(at_hour(18, 0)).altitude;
        assert!(noon > 0.8, "midday sun rides high: {noon}");
        // The two thresholds differ because these sample hours aren't
        // equidistant from their horizon crossings on the 5..20 day span — dusk
        // sits 2h before sunset, dawn 1.5h after sunrise.
        assert!(
            dawn < 0.4 && dusk < 0.5,
            "dawn/dusk sit low: {dawn} / {dusk}"
        );
    }

    #[test]
    fn warmth_is_high_low_on_the_horizon_and_neutral_at_apex() {
        assert!(emitter(at_hour(6, 30)).warmth > 0.6, "low sun is warm/red");
        assert!(emitter(at_hour(12, 30)).warmth < 0.3, "apex sun is neutral");
    }

    #[test]
    fn azimuth_advances_west_across_the_day() {
        let a = emitter(at_hour(7, 0)).azimuth;
        let b = emitter(at_hour(12, 0)).azimuth;
        let c = emitter(at_hour(18, 0)).azimuth;
        assert!(a < b && b < c, "azimuth marches E->W: {a} < {b} < {c}");
    }

    #[test]
    fn moon_luminance_tracks_phase() {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let (mut lo_lum, mut hi_lum) = (0.0, 0.0);
        for day in 1..=30u32 {
            let s = night_on(day);
            let frac = moon_phase(s);
            let lum = emitter(s).emitter_lum;
            if frac < lo {
                lo = frac;
                lo_lum = lum;
            }
            if frac > hi {
                hi = frac;
                hi_lum = lum;
            }
        }
        assert!(
            hi_lum > lo_lum,
            "fuller moon lights brighter ({hi_lum} vs {lo_lum})"
        );
    }

    #[test]
    fn weather_override_forces_a_fixed_variant_then_restores() {
        use std::time::Duration;
        // Clear the thread-local even if an assert below panics — plain
        // `cargo test` shares threads, so a leaked override corrupts a sibling.
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_weather_override(None);
            }
        }
        let _reset = Reset;
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let natural = weather_state(t);
        // Force a variant that differs from the natural pick so the assert is real.
        let forced = Weather::ALL
            .into_iter()
            .find(|&w| w != natural)
            .expect("8 variants");
        set_weather_override(Some(forced));
        assert_eq!(weather_state(t), forced);
        assert_eq!(
            weather_state(t + Duration::from_secs(987_654)),
            forced,
            "override is time-independent"
        );
        set_weather_override(None);
        assert_eq!(
            weather_state(t),
            natural,
            "None restores time-based selection"
        );
    }

    #[test]
    fn storm_transmits_less_than_rain_overall() {
        let s = atmo(Weather::Storm);
        let r = atmo(Weather::Rain);
        assert!(
            s.direct <= r.direct && s.diffuse < r.diffuse,
            "storm steady-state darker than rain: {s:?} vs {r:?}"
        );
    }

    #[test]
    fn clear_beams_hard_overcast_kills_the_beam() {
        assert!(atmo(Weather::Clear).direct > 0.9, "clear = hard beam");
        for w in [Weather::Overcast, Weather::Rain, Weather::Storm] {
            assert_eq!(atmo(w).direct, 0.0, "{w:?} scatters the beam to nothing");
        }
    }

    #[test]
    fn fog_is_a_luminous_diffuse_whiteout() {
        let f = atmo(Weather::Fog);
        assert!(
            f.diffuse >= atmo(Weather::Overcast).diffuse,
            "fog is a bright veil"
        );
        assert!(f.direct < 0.2, "fog is near-shadowless");
        assert!(f.disc < 0.2, "the disc is lost in fog");
    }

    #[test]
    fn disc_visibility_is_clear_then_hazy_then_gone() {
        assert!(atmo(Weather::Clear).disc > 0.9);
        assert!(
            (0.0..0.6).contains(&atmo(Weather::Smog).disc),
            "haze half-hides the disc"
        );
        assert!(
            atmo(Weather::Overcast).disc < 0.1,
            "overcast hides the disc"
        );
    }

    #[test]
    fn thick_cloud_hides_the_disc_uniformly() {
        let min_disc_vis = crate::pixel_painter::background::celestial::MIN_DISC_VIS;
        let overcast = atmo(Weather::Overcast).disc;
        let rain = atmo(Weather::Rain).disc;
        let storm = atmo(Weather::Storm).disc;
        assert!(
            overcast >= rain && rain >= storm,
            "disc visibility must not increase as cloud thickens: \
             overcast={overcast} rain={rain} storm={storm}"
        );
        assert!(
            overcast < min_disc_vis && rain < min_disc_vis && storm < min_disc_vis,
            "overcast/rain/storm should all hide the disc (below MIN_DISC_VIS={min_disc_vis}): \
             overcast={overcast} rain={rain} storm={storm}"
        );
    }

    #[test]
    fn windy_near_full_beam() {
        assert!(
            atmo(Weather::Windy).direct > 0.5,
            "windy keeps a strong beam"
        );
    }

    #[test]
    fn haze_and_snow_keep_a_faint_but_nonzero_beam() {
        for w in [Weather::Snow, Weather::Fog, Weather::Smog] {
            let d = atmo(w).direct;
            assert!(
                0.0 < d && d < 0.5,
                "{w:?} should keep a faint but nonzero beam: {d}"
            );
        }
    }

    #[test]
    fn storm_diffuse_dimmer_than_overcast() {
        assert!(
            atmo(Weather::Storm).diffuse < atmo(Weather::Overcast).diffuse,
            "storm diffuse should be dimmer than overcast"
        );
    }

    #[test]
    fn night_floor_varies_by_weather() {
        assert!(
            city_bounce(Weather::Snow) >= city_bounce(Weather::Clear),
            "snow albedo should bounce at least as much as a clear night"
        );
        assert!(
            city_bounce(Weather::Clear) > city_bounce(Weather::Overcast),
            "clear night should out-glow overcast"
        );
        assert!(
            city_bounce(Weather::Storm) < city_bounce(Weather::Overcast),
            "storm should swallow more glow than overcast"
        );
        assert!(
            city_bounce(Weather::Storm) < city_bounce(Weather::Clear),
            "storm should swallow more glow than a clear night"
        );
        for w in Weather::ALL {
            assert!(
                city_bounce(w) > 0.0,
                "{w:?} must keep a nonzero city-bounce floor (never pitch black)"
            );
        }
    }
}
