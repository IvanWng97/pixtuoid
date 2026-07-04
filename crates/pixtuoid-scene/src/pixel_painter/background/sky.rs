//! Time-of-day derived state — the sky emitter (sun/moon), weather-as-
//! atmosphere transmission, glass colors, sunlight spill, and nighttime
//! floor dim overlay.

use std::cell::Cell;
use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::pixel_painter::palette::{blend_rgb, mix_lab};
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
    /// All variants, in canonical order — single source for `--weather` parsing
    /// and the valid-names list. The site's gallery manifest
    /// (site/src/weather.json) mirrors it; the bridge is the
    /// `weather_gallery_manifest_matches_the_weather_enum` test, which fails on
    /// any add/rename here until the manifest (+ gen-media art) follows.
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
    /// Screenshot/test affordance: when `Some`, every `weather_state` call on this
    /// thread returns it, bypassing the time-based selection. Production never sets
    /// it (only `snapshot --weather` via `force_weather`), so live rendering is
    /// byte-identical. `weather_state` is the single chokepoint every weather
    /// derivation (time-of-day look, floor tint, ambient beam, lightning) funnels
    /// through, so intercepting here covers them all without threading a param.
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
    // Weather re-rolls per hashed bucket, so it changes ~every 10 minutes.
    const WEATHER_CYCLE_SECS: u64 = 600;
    let cycle = secs / WEATHER_CYCLE_SECS;
    // splitmix64 finalizer, open-coded by deliberate choice (see `strike_offset`
    // in background/mod.rs for the cross-crate-copy rationale).
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

// Weights folding the two transmission channels into one interior illuminance.
// Calibrated so a CLEAR noon (emitter_lum≈1, direct=1, diffuse=0.55) lands at
// full brightness (K_BEAM + 0.55·K_FILL ≈ 1).
const K_BEAM: f32 = 0.70;
const K_FILL: f32 = 0.55;
// Max window-spill horizontal lean (px/row) at the low-sun extremes.
const SPILL_SLANT_MAX: f32 = 0.7;

/// Direct-beam strength reaching the interior = emitter luminance carried by the
/// weather's DIRECT transmission. Drives the wall sun-spot + dust motes. Zero at
/// night (the moon casts no usable beam) and under thick cloud.
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
    // Roughly half the old table (Snow 0.16→0.08, …, Storm 0.03→0.015), same
    // ordering — so a moonlit night's floor can never out-light a stormy solar
    // noon (see `solar_noon_outshines_the_brightest_night`).
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
/// disc's own visibility through the medium. (Replaces the old absolute
/// `weather_light` light-level table — magnitude now comes from the emitter.)
#[derive(Debug, Clone, Copy)]
pub(in crate::pixel_painter) struct Atmo {
    pub(in crate::pixel_painter) direct: f32,
    pub(in crate::pixel_painter) diffuse: f32,
    pub(in crate::pixel_painter) disc: f32,
}

pub(in crate::pixel_painter) fn atmo(w: Weather) -> Atmo {
    // (direct, diffuse, disc). Storm < Rain in BOTH transmission channels
    // (denser cloud); lightning adds transient punch elsewhere, not here.
    // Overcast/Rain/Storm all sit at the SAME near-zero disc (0.05, below
    // `MIN_DISC_VIS`) — thick cloud hides the disc uniformly, so a thicker
    // cloud (Storm) never shows MORE of the disc than a thinner one (Rain).
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
/// hour. `spill_slant` is x-shift per row going down: positive = rightward
/// (morning sun in the east), negative = leftward (evening sun in the west).
/// `darkness` is 1 - daylight, used to drive artificial-light effects.
pub(in crate::pixel_painter) struct TimeOfDayLook {
    pub(in crate::pixel_painter) glass_a: Rgb,
    pub(in crate::pixel_painter) glass_b: Rgb,
    pub(in crate::pixel_painter) spill_strength: f32,
    pub(in crate::pixel_painter) spill_slant: f32,
    pub(in crate::pixel_painter) darkness: f32,
}

pub(in crate::pixel_painter) fn time_of_day_look(now: SystemTime, theme: &Theme) -> TimeOfDayLook {
    let sky = emitter(now);
    let a = atmo(weather_state(now));
    // The moon casts no USABLE direct beam (mirrors `beam_strength`'s own
    // Sun/Moon gate) — a moonlit night must never out-light a cloudy solar
    // noon, so only the sun feeds the hard-beam term; the moon's illuminance
    // is diffuse-fill only.
    let direct_eff = match sky.body {
        Body::Sun => a.direct,
        Body::Moon => 0.0,
    };
    // One illuminance for sun OR moon: luminance carried by beam + diffuse fill.
    let interior = (sky.emitter_lum * (direct_eff * K_BEAM + a.diffuse * K_FILL)).clamp(0.0, 1.0);
    // At night the city-bounce floor keeps the room from going pitch black.
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

    // Base sky lerps night→day by the exterior light (a moonlit night lifts a touch);
    // then a low, LIT sun/moon warms it toward twilight (warmth is high near the horizon).
    let warm = (sky.warmth * interior).clamp(0.0, 1.0);
    let glass_a = mix_lab(mix_lab(night_a, day_a, exterior), twilight_a, warm * 0.5);
    let glass_b = mix_lab(mix_lab(night_b, day_b, exterior), twilight_b, warm * 0.5);

    // Directional cues share the emitter azimuth (0=east/dawn .. 1=west/dusk):
    // morning sun casts light westward (leftward, negative), evening eastward (right).
    let (spill_strength, spill_slant) = match sky.body {
        Body::Sun => (interior, (sky.azimuth - 0.5) * 2.0 * SPILL_SLANT_MAX),
        Body::Moon => (0.0, 0.0),
    };

    TimeOfDayLook {
        glass_a,
        glass_b,
        spill_strength,
        spill_slant,
        darkness: 1.0 - exterior,
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

/// Time-of-day sun position projected onto an office wall, derived from the
/// SAME emitter arc that drives `time_of_day_look`'s spill lean — so the
/// wall the sun-spot lands on, the direction the floor spill leans, and the
/// disc's own position (Task 5) can never disagree. Returns `None` at night
/// (the moon casts no wall spot).
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
    // Azimuth bands map the arc onto the office walls: east wall in the morning,
    // the south (window) wall around midday, west wall in the evening — the SAME
    // azimuth that places the disc + leans the floor spill, so all three agree.
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

/// Multiplicative dim applied to floor pixels at night. Pulls everything
/// toward a dark navy so the artificial-light pools have something to
/// stand out against. `strength` is 0..1 (no dim..full dim).
pub(in crate::pixel_painter) fn dim_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
    theme: &Theme,
) {
    let night_tint = theme.lighting.night_tint;
    let s = strength.clamp(0.0, 0.55);
    // Skip the full-floor blend on a clear-sky day (s == 0): blend_rgb(cur, _, 0.0)
    // is a per-pixel no-op, so the early return is byte-identical (mirrors
    // daylight_floor_overlay) and saves a full floor-area pass every clear daytime frame.
    if s <= 0.0 {
        return;
    }
    for y in top_y..bottom_y.min(buf.height()) {
        for x in 0..buf.width() {
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, night_tint, s));
        }
    }
}

/// Warm sunlight LIFT on the floor — the daytime mirror of [`dim_floor_overlay`].
/// Blends floor pixels toward a warm midday tint so a sunny day reads bright and
/// warm instead of flat carpet. Needed because the model otherwise has only a
/// night *dim* and no positive day term: `intensity` maxes at 1.0, so at clear
/// noon `darkness` is 0 and the floor sat at its plain (brownish) base color.
/// `strength` is `day_eff`-driven (0 at night / full-dark weather, full at clear
/// noon), so cloudy days lift proportionally less. Sun enters regardless of
/// occupancy, so — unlike the dim — this is NOT scaled by the empty-floor boost.
pub(in crate::pixel_painter) fn daylight_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
) {
    // Pale warm midday sunlight. Theme-agnostic (daylight is daylight); applied
    // at low strength so it warms/brightens the floor without washing it out.
    const SUN_TINT: Rgb = Rgb {
        r: 255,
        g: 246,
        b: 224,
    };
    let s = strength.clamp(0.0, 0.40);
    if s <= 0.0 {
        return;
    }
    for y in top_y..bottom_y.min(buf.height()) {
        for x in 0..buf.width() {
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, SUN_TINT, s));
        }
    }
}

/// The physical sky emitter — sun by day, moon by night — resolved from the
/// local clock. Position rides an arc (§ altitude/azimuth); luminance + warmth
/// follow altitude (low body = longer air path = dimmer + warmer). This is the
/// ONE source the interior light + the disc derive from.
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
/// Moon luminance is a small fraction of full sun even at a full phase — low
/// enough that a full-moon midnight (plus the `city_bounce` floor) still
/// stays dimmer than the dimmest cloudy daytime (a stormy solar noon); see
/// `solar_noon_outshines_the_brightest_night`.
const MOON_PEAK_LUM: f32 = 0.12;
/// Synodic month (days) + a known new-moon epoch (unix days) for the phase calc.
const SYNODIC_DAYS: f32 = 29.530_588;
const NEW_MOON_EPOCH_UNIX_DAYS: f32 = 18_231.0; // 2019-11-27 new moon (unix day index)

fn arc_progress(h: f32, rise: f32, set: f32) -> f32 {
    ((h - rise) / (set - rise)).clamp(0.0, 1.0)
}

pub(in crate::pixel_painter) fn emitter(now: SystemTime) -> SkyState {
    let h = super::local_hour_frac(now);
    let is_day = (SUN_RISE_H..SUN_SET_H).contains(&h);
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
        (Body::Sun, altitude) // full sun ∝ altitude
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
    // Illuminated fraction ≈ (1 - cos(2π·age/synodic)) / 2.
    (1.0 - (std::f32::consts::TAU * age / SYNODIC_DAYS).cos()) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn daylight_floor_overlay_brightens_at_positive_strength() {
        // The warm SUN_TINT (255,246,224) blended in at positive strength lifts a
        // dark floor on every channel (it only ever warms/brightens).
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
        // strength 0 short-circuits before any blend — pixels untouched.
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

    /// Build a `SystemTime` that corresponds to local hour `h`, minute `m`
    /// on a fixed date — keeps the tests TZ-independent because
    /// `sun_on_wall` decodes the input back into `chrono::Local`.
    fn at_hour(h: u32, m: u32) -> SystemTime {
        chrono::Local
            .with_ymd_and_hms(2026, 1, 1, h, m, 0)
            .single()
            .expect("local time should be unambiguous")
            .into()
    }

    /// Local 02:00 (always night in the day-ramp) on a given January day.
    /// Weather varies by day at a fixed hour (the hash keys on unix-secs/600),
    /// so searching days lets us find a clear vs storm night TZ-independently.
    fn night_on(day: u32) -> SystemTime {
        chrono::Local
            .with_ymd_and_hms(2026, 1, day, 2, 0, 0)
            .single()
            .expect("local time should be unambiguous")
            .into()
    }

    /// Local midnight on a given January day — near the night arc's own apex,
    /// so it's close to the brightest instant of that night regardless of
    /// weather (mirrors `night_on` but at 00:00).
    fn midnight_on(day: u32) -> SystemTime {
        chrono::Local
            .with_ymd_and_hms(2026, 1, day, 0, 0, 0)
            .single()
            .expect("local time should be unambiguous")
            .into()
    }

    // The interior illuminance now folds emitter luminance with atmo transmission
    // (`K_BEAM`/`K_FILL`), and night keeps a weather-keyed `city_bounce` floor —
    // so weather must still separate a night's darkness, but the OLD test's
    // "any clear vs any storm night" search is no longer phase-fair: two
    // different nights can land on two different moon phases, which now ALSO
    // drives interior brightness. Hold the instant fixed (only weather varies
    // via the override) so the comparison is honest.
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
        // Clear noon is ~fully lit (day dominates).
        set_weather_override(Some(Weather::Clear));
        let noon = time_of_day_look(at_hour(12, 0), theme).darkness;
        set_weather_override(None);
        assert!(noon < 0.1, "clear noon ~fully lit: {noon}");
    }

    // The property that was impossible under the old flat weather table: interior
    // brightness now tracks the emitter's ALTITUDE, so even holding weather fixed,
    // a higher sun (noon) out-lights a lower one (dusk).
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

    // The headline physics-audit fix: a moonlit night must NEVER render
    // brighter than a cloudy solar noon. Storm zeroes both `direct` channels
    // (so the moon's already-zeroed direct beam can't matter, but a Storm
    // noon still has to survive on diffuse alone), while Snow/Clear at a
    // FULL moon are the two best cases night can offer (highest `city_bounce`
    // floor + full lunar illumination). Even that best case must stay dimmer.
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

        // The fullest moon night in January 2026 (max illuminated fraction).
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
        // Was `> 0.6`: the emitter-derived warmth at 18:00 is ≈0.593 (symmetric
        // with dawn's ≈0.593 on the sin(pi*t) arc) — the old flat-table value
        // no longer applies. `> 0.55` keeps a real margin below the computed value.
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
        // case-insensitive + trimmed
        assert_eq!(Weather::from_name("  SNOW "), Some(Weather::Snow));
        assert_eq!(Weather::from_name("drizzle"), None);
    }

    #[test]
    fn emitter_is_sun_by_day_moon_by_night_never_both() {
        // Sample every 30 min across a day; exactly one body, and the switch
        // happens around the dawn/dusk ramps (no midday moon, no midnight sun).
        for slot in 0..48u32 {
            let (h, m) = (slot / 2, (slot % 2) * 30);
            let s = at_hour(h, m);
            let e = emitter(s);
            match e.body {
                Body::Sun => assert!(
                    (5.0..20.0).contains(&(h as f32 + m as f32 / 60.0)),
                    "sun only during the daylight ramp, got {h}:{m:02}"
                ),
                // Exact complement of the Sun arm's `[5,20)` band (was the
                // looser `!(8..17)`, which left [5,8)/[17,20) unchecked for the
                // moon — a SUN_RISE_H/SUN_SET_H boundary shift slipped past it).
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
        // The brief's illustrative `< 0.35` for both doesn't hold honestly: on
        // the sin(pi*t) arc over the 5..20 day span, dawn (6:30, t≈0.10) sits
        // at altitude≈0.309 but dusk (18:00, t≈0.867) sits at altitude≈0.407
        // -- dusk is only 2h before the 20:00 sunset while dawn is 1.5h after
        // the 5:00 sunrise, so the two sample hours aren't equidistant from
        // their respective horizon crossings. Not a curve bug, just an
        // artifact of these particular sample points. Thresholds below keep
        // the same ~0.09 real margin off the actual computed values (dawn
        // 0.309 vs 0.4, dusk 0.407 vs 0.5) while still reading clearly low
        // next to noon's >0.8.
        assert!(
            dawn < 0.4 && dusk < 0.5,
            "dawn/dusk sit low: {dawn} / {dusk}"
        );
    }

    #[test]
    fn warmth_is_high_low_on_the_horizon_and_neutral_at_apex() {
        // The brief's illustrative `> 0.7` doesn't hold honestly: dawn's
        // warmth = 1 - altitude(6:30) computes to ≈0.691 on the sin(pi*t)
        // arc. `> 0.6` keeps the same ~0.09 real margin as the reconciled
        // altitude thresholds above.
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
        // A near-full-moon night is brighter than a near-new-moon night.
        // Search a lunar month for the min/max illuminated fraction at 02:00.
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
        // Clear the thread-local even if an assert below panics — `cargo test` shares
        // threads across tests, so a leaked override would corrupt a sibling weather
        // test (nextest is process-per-test and immune, but the justfile falls back to
        // `cargo test` when nextest isn't installed).
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
        // The override ignores the timestamp entirely.
        assert_eq!(weather_state(t), forced);
        assert_eq!(
            weather_state(t + Duration::from_secs(987_654)),
            forced,
            "override is time-independent"
        );
        // Restore so this thread-local can't bleed into sibling tests.
        set_weather_override(None);
        assert_eq!(
            weather_state(t),
            natural,
            "None restores time-based selection"
        );
    }

    #[test]
    fn storm_transmits_less_than_rain_overall() {
        // The physical correction: cumulonimbus is optically denser than nimbostratus.
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
        // `MIN_DISC_VIS` (background/mod.rs `compute_disc`'s hide gate) is
        // 0.08 — Overcast/Rain/Storm must all sit at or below it, so thick
        // cloud hides the disc uniformly: a THICKER cloud (Storm) must never
        // show MORE of the disc than a thinner one (Rain), matching the
        // direct/diffuse ordering `storm_transmits_less_than_rain_overall`
        // already pins.
        const MIN_DISC_VIS: f32 = 0.08;
        let overcast = atmo(Weather::Overcast).disc;
        let rain = atmo(Weather::Rain).disc;
        let storm = atmo(Weather::Storm).disc;
        assert!(
            overcast >= rain && rain >= storm,
            "disc visibility must not increase as cloud thickens: \
             overcast={overcast} rain={rain} storm={storm}"
        );
        assert!(
            overcast < MIN_DISC_VIS && rain < MIN_DISC_VIS && storm < MIN_DISC_VIS,
            "overcast/rain/storm should all hide the disc (below MIN_DISC_VIS={MIN_DISC_VIS}): \
             overcast={overcast} rain={rain} storm={storm}"
        );
    }

    #[test]
    fn windy_near_full_beam() {
        // Windy scatters cloud but keeps the sky mostly clear: near-full direct beam.
        assert!(
            atmo(Weather::Windy).direct > 0.5,
            "windy keeps a strong beam"
        );
    }

    #[test]
    fn haze_and_snow_keep_a_faint_but_nonzero_beam() {
        // Snow glare and atmospheric haze (fog/smog) still let a weak directional
        // beam through — never a hard zero, but well below a clear/windy sky.
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
        // A storm's cumulonimbus is optically denser than plain overcast stratus,
        // so even the flat diffuse fill is dimmer under a storm.
        assert!(
            atmo(Weather::Storm).diffuse < atmo(Weather::Overcast).diffuse,
            "storm diffuse should be dimmer than overcast"
        );
    }

    #[test]
    fn night_floor_varies_by_weather() {
        // The city_bounce night floor is phase-independent (unlike the moon), so
        // this ordering must hold regardless of the date: snow's albedo bounces
        // the most, a storm swallows the most, and every weather keeps SOME glow
        // (a room is never truly pitch black).
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
