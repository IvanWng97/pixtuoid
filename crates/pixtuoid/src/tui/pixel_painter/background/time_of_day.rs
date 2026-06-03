//! Time-of-day derived state — glass colors, sunlight spill, weather,
//! sunset strength, and nighttime floor dim overlay.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::pixel_painter::palette::{blend_rgb, lerp_rgb};
use crate::tui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::tui::pixel_painter) enum Weather {
    Clear,
    Rain,
    Storm,
    Snow,
    Fog,
    Overcast,
    Windy,
    Smog,
}

pub(in crate::tui::pixel_painter) fn weather_state(now: SystemTime) -> Weather {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cycle = secs / 600;
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

/// The outdoor-light contribution of a weather, reaching the interior —
/// physically grounded per weather. (Renamed from `AtmoAttenuation`: `night_sky`
/// is an additive emission source, not an attenuation, so "attenuation"
/// over-promised.) Three independent 0..1 channels:
/// - `intensity` (0..1): DAYTIME diffuse sunlight — drives window spill, glass
///   day-tint, and (via `darkness = 1 - day_eff`) the artificial-light balance.
/// - `beam_strength` (0..1): the DIRECT sun beam that casts the wall sun-spot +
///   dust motes. A clear sky beams hard (1.0); thin cloud / haze / snow-glare
///   still throw a faint spot (small >0); thick overcast/rain/storm scatter it
///   to nothing (0.0). Replaces the old all-or-nothing `has_direct_beam` bool.
/// - `night_sky` (0..1): NIGHT-side luminance (moon / stars / snow-albedo glow /
///   sodium haze) reaching the interior when the sun is down. Without it every
///   weather rendered an identical pitch-black night; now a clear/snowy night
///   reads brighter than a storm night.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::tui::pixel_painter) struct WeatherLight {
    pub intensity: f32,
    pub beam_strength: f32,
    pub night_sky: f32,
}

pub(in crate::tui::pixel_painter) fn weather_light(w: Weather) -> WeatherLight {
    // (intensity, beam_strength, night_sky)
    //
    // `night_sky` was dialed down ~35% from its original tuning (Clear 0.55→0.35,
    // Snow 0.60→0.40, …): the moonlit nights read too bright — the interior
    // barely darkened after sunset. The *relative* ordering is unchanged (snow ≥
    // clear > … > rain > storm), so the weather-night character is preserved;
    // only the absolute moonlight strength drops. Guarded by
    // `night_sky_brightness_varies_by_weather`.
    let (intensity, beam_strength, night_sky) = match w {
        // Full sun, hard beam, faintly moonlit/starry night.
        Weather::Clear => (1.0, 1.0, 0.35),
        // Clear but blustery — near-full sun, beam softened slightly by haze/cloud scud.
        Weather::Windy => (1.0, 0.9, 0.32),
        // Bright grey glare (high snow albedo) + a faint beam through thin cloud;
        // brightest night of all — city light bounces off the snow.
        Weather::Snow => (0.75, 0.25, 0.40),
        // Hazy: sun reduced to a dim disc (small beam), warm sodium-lit night.
        Weather::Smog => (0.55, 0.30, 0.22),
        // Luminous white-out: bright + near-shadowless (tiny beam), hazy dim night.
        // (Daytime `intensity` was 0.30 — wrongly the 2nd-darkest; real fog is a bright veil.)
        Weather::Fog => (0.55, 0.05, 0.20),
        // Thick cloud: diffuse only, dull night.
        Weather::Overcast => (0.45, 0.0, 0.14),
        // Daytime storm lifted from 0.25 (read as dusk) to a gloomy-but-clearly-
        // daytime 0.42 (just under Overcast). Deliberately a hair ABOVE plain
        // Rain (0.40): the lightning flash repeatedly lifts a storm's perceived
        // average brightness above uniformly-gloomy steady rain. Fully diffuse
        // (no beam); darkest night of all (no moon) — lightning is the only punch.
        // The Storm>Rain ordering is guarded by `atmo_storm_brighter_than_rain_by_design`.
        Weather::Storm => (0.42, 0.0, 0.08),
        // Steady rain: dark diffuse, dark night.
        Weather::Rain => (0.40, 0.0, 0.12),
    };
    // All three channels are 0..1 multipliers/weights — enforce it at the one
    // place values are minted so a future tuning typo (e.g. pushing `night_sky`
    // past 1.0, which would over-lerp glass past the day sky / drive `darkness`
    // negative) trips in dev/test builds. Zero cost in release.
    debug_assert!(
        (0.0..=1.0).contains(&intensity)
            && (0.0..=1.0).contains(&beam_strength)
            && (0.0..=1.0).contains(&night_sky),
        "WeatherLight channels must be 0..=1: {w:?} -> ({intensity}, {beam_strength}, {night_sky})"
    );
    WeatherLight {
        intensity,
        beam_strength,
        night_sky,
    }
}

pub(in crate::tui::pixel_painter) fn sunset_strength(now: SystemTime) -> f32 {
    let h = super::local_hour_frac(now);
    crate::tui::pixel_painter::palette::bell(h, 18.0, 1.5)
        .max(crate::tui::pixel_painter::palette::bell(h, 6.5, 1.0))
}

/// Window glass color + spill intensity + spill slant for the current local
/// hour. `spill_slant` is x-shift per row going down: positive = rightward
/// (morning sun in the east), negative = leftward (evening sun in the west).
/// `darkness` is 1 - daylight, used to drive artificial-light effects.
pub(in crate::tui::pixel_painter) struct TimeOfDayLook {
    pub(in crate::tui::pixel_painter) glass_a: Rgb,
    pub(in crate::tui::pixel_painter) glass_b: Rgb,
    pub(in crate::tui::pixel_painter) spill_strength: f32,
    pub(in crate::tui::pixel_painter) spill_slant: f32,
    pub(in crate::tui::pixel_painter) darkness: f32,
    /// Raw twilight bell (dawn ~6.5 / dusk ~18.5), pre-atmosphere. Exposed so
    /// the window painter reads it instead of re-decoding the local hour and
    /// recomputing the identical expression per window per frame.
    pub(in crate::tui::pixel_painter) twilight: f32,
}

pub(in crate::tui::pixel_painter) fn time_of_day_look(
    now: SystemTime,
    theme: &Theme,
) -> TimeOfDayLook {
    let h = super::local_hour_frac(now);

    // Daylight intensity: full from 8 to 17, smooth ramp 5..8 and 17..20.
    let day = if !(5.0..20.0).contains(&h) {
        0.0
    } else if h < 8.0 {
        (h - 5.0) / 3.0
    } else if h < 17.0 {
        1.0
    } else {
        1.0 - (h - 17.0) / 3.0
    };

    // Twilight bell at dawn (~6.5) and dusk (~18.5) — adds orange/pink
    // tint that the cyan↔dark-blue base doesn't capture.
    let twilight = crate::tui::pixel_painter::palette::bell(h, 6.5, 1.5)
        .max(crate::tui::pixel_painter::palette::bell(h, 18.5, 1.5));

    // Atmospheric attenuation makes the sky base + twilight blaze respond
    // to outdoor weather. Storm at noon shouldn't read as full day-blue
    // with rain streaks pasted over it — the sky itself goes dim under
    // heavy weather. `day_eff` is the effective daylight reaching the
    // glass; consumers of `spill_strength` / `darkness` see weather
    // automatically applied without each caller having to multiply.
    let atmo = weather_light(weather_state(now));
    let day_eff = day * atmo.intensity;
    let twilight_eff = twilight * atmo.intensity;
    // Night-side exterior light (moon / stars / snow-albedo glow / sodium haze)
    // fades in as the sun ramps out (`1 - day`). Without it every weather
    // collapsed to an identical pitch-black night; now a clear/snowy night
    // reads brighter than a storm night. `exterior` is the effective outdoor
    // light the interior balances against — sun by day, sky-glow by night.
    let night_glow = atmo.night_sky * (1.0 - day);
    let exterior = day_eff.max(night_glow);

    let day_a = theme.lighting.day_sky_a;
    let day_b = theme.lighting.day_sky_b;
    let night_a = theme.lighting.night_sky_a;
    let night_b = theme.lighting.night_sky_b;
    let twilight_a = theme.lighting.twilight_a;
    let twilight_b = theme.lighting.twilight_b;

    // Lift the night-base glass a touch toward the day sky by night_glow so a
    // clear/snowy night shows faintly lit (moonlit) glass while a storm night
    // stays near-black. Subtle (≤ ~0.10 for the brightest skies).
    let night_lift = night_glow * 0.18;
    let glass_night_a = lerp_rgb(night_a, day_a, night_lift);
    let glass_night_b = lerp_rgb(night_b, day_b, night_lift);
    let glass_a = lerp_rgb(
        lerp_rgb(glass_night_a, day_a, day_eff),
        twilight_a,
        twilight_eff * 0.5,
    );
    let glass_b = lerp_rgb(
        lerp_rgb(glass_night_b, day_b, day_eff),
        twilight_b,
        twilight_eff * 0.5,
    );

    // Spill slant: ±0.7 px per row at peak hours (6am leftmost, 6pm
    // rightmost), zero at noon. Conventional read: morning sun on the east
    // (right of image) casts light westward (leftward shift); evening sun
    // on the west casts eastward (rightward shift).
    let slant = if h < 12.0 {
        -((12.0 - h) / 6.0).clamp(0.0, 1.0) * 0.7
    } else {
        ((h - 12.0) / 6.0).clamp(0.0, 1.0) * 0.7
    };

    TimeOfDayLook {
        glass_a,
        glass_b,
        spill_strength: day_eff,
        spill_slant: slant,
        // Artificial lights + floor-dim balance against the effective exterior
        // light, so they ride weather at night too (dark storm night → more
        // dim + brighter pools; bright clear night → less dim).
        darkness: 1.0 - exterior,
        twilight,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui::pixel_painter) enum WallSide {
    East,
    South,
    West,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::tui::pixel_painter) struct SunSpot {
    pub wall: WallSide,
    /// 0.0..=1.0 along the wall (left→right for South, top→bottom for East/West).
    pub along: f32,
    /// 0.0=dim, 1.0=brightest at noon.
    pub intensity: f32,
    /// 0.0=neutral white (noon), 1.0=very warm gold (sunrise/sunset).
    pub warmth: f32,
}

/// Time-of-day sun position projected onto an office wall. Uses local
/// hour-of-day so the sun's wall (East / West) matches what the rendered
/// wall clock shows; same pattern as `paint_clock` / `sunset_strength` /
/// `time_of_day_look`. Returns `None` outside the extended daylight
/// window 5:30–19:30; the extra 30 minutes on each end carry a fade-in
/// / fade-out ramp so the sun spot doesn't pop on/off at the boundary.
pub(in crate::tui::pixel_painter) fn sun_on_wall(now: SystemTime) -> Option<SunSpot> {
    use chrono::Timelike;
    const SUN_RAMP_HOURS: f32 = 0.5;
    const LOWER: f32 = 6.0 - SUN_RAMP_HOURS;
    const UPPER: f32 = 19.0 + SUN_RAMP_HOURS;
    let unix_now = now.duration_since(std::time::UNIX_EPOCH).ok()?;
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let hour = local.hour() as f32 + local.minute() as f32 / 60.0;
    if !(LOWER..=UPPER).contains(&hour) {
        return None;
    }
    // Wall partition uses the position-hour clamped to [6, 19] so the
    // along/warmth/noon formulas stay in their valid ranges through the
    // boundary fade.
    let position_hour = hour.clamp(6.0, 19.0);
    let (wall, along) = if position_hour < 8.5 {
        (WallSide::East, (position_hour - 6.0) / 2.5)
    } else if position_hour < 16.0 {
        (WallSide::South, (position_hour - 8.5) / 7.5)
    } else {
        (WallSide::West, (position_hour - 16.0) / 3.0)
    };
    let noon_distance = (position_hour - 12.0).abs() / 6.0;
    let boundary_fade = if hour < 6.0 {
        ((hour - LOWER) / SUN_RAMP_HOURS).clamp(0.0, 1.0)
    } else if hour > 19.0 {
        ((UPPER - hour) / SUN_RAMP_HOURS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let intensity = (1.0 - noon_distance * 0.7).clamp(0.0, 1.0) * boundary_fade;
    let warmth = noon_distance.clamp(0.0, 1.0);
    Some(SunSpot {
        wall,
        along,
        intensity,
        warmth,
    })
}

/// Multiplicative dim applied to floor pixels at night. Pulls everything
/// toward a dark navy so the artificial-light pools have something to
/// stand out against. `strength` is 0..1 (no dim..full dim).
pub(in crate::tui::pixel_painter) fn dim_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
    theme: &Theme,
) {
    let night_tint = theme.lighting.night_tint;
    let s = strength.clamp(0.0, 0.55);
    for y in top_y..bottom_y.min(buf.height) {
        for x in 0..buf.width {
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
pub(in crate::tui::pixel_painter) fn daylight_floor_overlay(
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
    for y in top_y..bottom_y.min(buf.height) {
        for x in 0..buf.width {
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, SUN_TINT, s));
        }
    }
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

    // The headline F1 logic: `darkness = 1 - max(day_eff, night_glow)` makes the
    // interior brightness track weather AT NIGHT (previously every weather gave
    // day_eff=0 → identical pitch black). A revert to `1 - day_eff` (dropping
    // night_glow), a sign flip, or a min/max swap would leave the atmo table
    // tests green but fail here.
    #[test]
    fn time_of_day_look_night_darkness_tracks_weather() {
        let theme = crate::tui::theme::ALL_THEMES[0];
        let (mut clear_t, mut storm_t) = (None, None);
        for day in 1..=28u32 {
            let t = night_on(day);
            match weather_state(t) {
                Weather::Clear if clear_t.is_none() => clear_t = Some(t),
                Weather::Storm if storm_t.is_none() => storm_t = Some(t),
                _ => {}
            }
        }
        let clear = time_of_day_look(clear_t.expect("a clear night in January"), theme);
        let storm = time_of_day_look(storm_t.expect("a storm night in January"), theme);
        assert!(
            clear.darkness < storm.darkness,
            "clear night ({}) must be brighter than storm night ({})",
            clear.darkness,
            storm.darkness
        );
        assert!(
            storm.darkness < 1.0,
            "even a storm night keeps some sky-glow (not pitch black): {}",
            storm.darkness
        );
        // Day path / `max(day_eff, night_glow)`: a CLEAR noon has day_eff≈1, so
        // night_glow (`night_sky·(1−day)`) drops out and darkness≈0 — day must
        // dominate the max regardless of the small night term.
        let clear_noon: SystemTime = (1..=28u32)
            .map(|d| {
                chrono::Local
                    .with_ymd_and_hms(2026, 1, d, 12, 0, 0)
                    .single()
                    .unwrap()
                    .into()
            })
            .find(|t| weather_state(*t) == Weather::Clear)
            .expect("a clear noon in January");
        assert!(
            time_of_day_look(clear_noon, theme).darkness < 0.1,
            "a clear noon should be ~fully lit (day_eff dominates night_glow)"
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
        assert!(s.warmth > 0.6, "evening sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_none_at_midnight() {
        assert!(sun_on_wall(at_hour(0, 0)).is_none());
    }

    #[test]
    fn atmo_clear_beams_hard() {
        let a = weather_light(Weather::Clear);
        assert_eq!(a.beam_strength, 1.0);
        assert_eq!(a.intensity, 1.0);
        // Windy is clear-but-blustery: near-full beam.
        assert!(weather_light(Weather::Windy).beam_strength > 0.5);
    }

    #[test]
    fn atmo_thick_cloud_kills_the_beam() {
        // Thick overcast / rain / storm scatter the beam to nothing.
        for w in [Weather::Rain, Weather::Storm, Weather::Overcast] {
            let a = weather_light(w);
            assert_eq!(a.beam_strength, 0.0, "{w:?} should have no direct beam");
            assert!(a.intensity < 1.0, "{w:?} should dim diffuse light");
        }
    }

    #[test]
    fn atmo_haze_and_snow_keep_a_faint_beam() {
        // Thin cloud / haze / snow-glare still throw a *faint* (not full) spot.
        for w in [Weather::Snow, Weather::Fog, Weather::Smog] {
            let b = weather_light(w).beam_strength;
            assert!(b > 0.0 && b < 0.5, "{w:?} beam should be faint, got {b}");
        }
    }

    #[test]
    fn atmo_storm_dimmer_than_overcast() {
        assert!(
            weather_light(Weather::Storm).intensity < weather_light(Weather::Overcast).intensity
        );
    }

    #[test]
    fn atmo_storm_brighter_than_rain_by_design() {
        // Counterintuitive but intentional: Storm (0.42) sits a hair ABOVE plain
        // Rain (0.40) because the lightning flash repeatedly lifts a storm's
        // perceived average brightness above uniformly-gloomy steady rain. Guard
        // it so a future intensity tune doesn't silently re-invert the ordering.
        assert!(
            weather_light(Weather::Storm).intensity > weather_light(Weather::Rain).intensity,
            "storm.intensity must exceed rain.intensity (lightning raises the average)"
        );
    }

    #[test]
    fn fog_is_a_luminous_whiteout_not_dark_mist() {
        // Real fog is a bright veil — must be brighter than overcast, not the
        // 2nd-darkest weather (the old 0.30 bug), yet still beam-faint.
        let fog = weather_light(Weather::Fog);
        assert!(fog.intensity >= weather_light(Weather::Overcast).intensity);
        assert!(fog.beam_strength < 0.2);
    }

    #[test]
    fn smog_dims_diffusely() {
        let a = weather_light(Weather::Smog);
        assert!(a.beam_strength > 0.0 && a.beam_strength < 0.5); // dim sun disc
        assert!(a.intensity > 0.4 && a.intensity < 0.7);
    }

    #[test]
    fn night_sky_brightness_varies_by_weather() {
        // Clear/snow nights (moon, stars, snow-albedo glow) are bright; a storm
        // night is the darkest. Previously every weather rendered identical night.
        let clear = weather_light(Weather::Clear).night_sky;
        let snow = weather_light(Weather::Snow).night_sky;
        let storm = weather_light(Weather::Storm).night_sky;
        let overcast = weather_light(Weather::Overcast).night_sky;
        assert!(clear > overcast, "clear night should beat overcast");
        assert!(snow >= clear, "snow-glow night should be the brightest");
        assert!(
            storm < overcast && storm < clear,
            "storm night should be the darkest"
        );
        // Every weather keeps SOME night-sky luminance (never pure black).
        for w in [
            Weather::Clear,
            Weather::Windy,
            Weather::Snow,
            Weather::Smog,
            Weather::Fog,
            Weather::Overcast,
            Weather::Rain,
            Weather::Storm,
        ] {
            assert!(weather_light(w).night_sky > 0.0, "{w:?} night_sky > 0");
        }
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
}
