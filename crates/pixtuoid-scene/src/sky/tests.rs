use super::*;
use crate::localclock::{at_hour_min, on_day};

#[test]
fn the_clock_picks_every_weather_within_a_week() {
    use std::collections::HashSet;
    use std::time::Duration;
    let start = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    const WEEK_SECS: u64 = 7 * 24 * 3600;
    let seen: HashSet<Weather> = (0..WEEK_SECS / WEATHER_CYCLE_SECS)
        .map(|slot| weather_at(start + Duration::from_secs(slot * WEATHER_CYCLE_SECS)))
        .collect();
    for w in Weather::ALL {
        assert!(
            seen.contains(&w),
            "the clock never picked {w:?} in a week of slots"
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
        let s = at_hour_min(h, m);
        let e = *Sky::at(s).emitter();
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
    let noon = Sky::at(at_hour_min(12, 30)).emitter().altitude;
    let dawn = Sky::at(at_hour_min(6, 30)).emitter().altitude;
    let dusk = Sky::at(at_hour_min(18, 0)).emitter().altitude;
    assert!(noon > 0.8, "midday sun rides high: {noon}");
    // The two thresholds differ because 06:30 and 18:00 sit unequally far from
    // [`SUN_RISE_H`] and [`SUN_SET_H`].
    assert!(
        dawn < 0.4 && dusk < 0.5,
        "dawn/dusk sit low: {dawn} / {dusk}"
    );
}

#[test]
fn warmth_is_high_low_on_the_horizon_and_neutral_at_apex() {
    assert!(
        Sky::at(at_hour_min(6, 30)).emitter().warmth > 0.6,
        "low sun is warm/red"
    );
    assert!(
        Sky::at(at_hour_min(12, 30)).emitter().warmth < 0.3,
        "apex sun is neutral"
    );
}

#[test]
fn azimuth_advances_from_dawn_to_dusk() {
    let a = Sky::at(at_hour_min(7, 0)).emitter().azimuth;
    let b = Sky::at(at_hour_min(12, 0)).emitter().azimuth;
    let c = Sky::at(at_hour_min(18, 0)).emitter().azimuth;
    assert!(
        a < b && b < c,
        "azimuth grows through the day: {a} < {b} < {c}"
    );
}

#[test]
fn moon_luminance_tracks_phase() {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut lo_lum, mut hi_lum) = (0.0, 0.0);
    for day in 1..=30u32 {
        let s = on_day(day, 2);
        let frac = moon_phase_at(s);
        let lum = Sky::at(s).emitter().emitter_lum;
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
    let t = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let natural = weather_at(t);
    // Force a variant that differs from the natural pick so the assert is real.
    let forced = Weather::ALL
        .into_iter()
        .find(|&w| w != natural)
        .expect("8 variants");
    let guard = ForcedWeather::new(forced);
    assert_eq!(weather_at(t), forced);
    assert_eq!(
        weather_at(t + Duration::from_secs(987_654)),
        forced,
        "override is time-independent"
    );
    drop(guard);
    assert_eq!(
        weather_at(t),
        natural,
        "dropping the guard restores time-based selection"
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
    assert!(offsets
        .iter()
        .all(|&o| o < LIGHTNING_PERIOD_MS - LIGHTNING_FLASH_MS));
}

#[test]
fn the_weather_is_deterministic_and_changes_across_slots() {
    let base = std::time::UNIX_EPOCH;
    let at = |slot: u64| base + std::time::Duration::from_secs(slot * WEATHER_CYCLE_SECS);
    assert_eq!(weather_at(at(17)), weather_at(at(17)));
    let unique: std::collections::HashSet<_> = (0..20).map(|slot| weather_at(at(slot))).collect();
    assert!(unique.len() >= 2, "weather should vary across slots");
}

/// The one pin on the clock-to-flash path through [`Sky::at`]; painter tests
/// inject the flash with [`Sky::with_flash`].
#[test]
fn a_strike_flashes_at_its_bucket_offset_and_ends_with_the_flash() {
    for bucket in 0..24u64 {
        let off = strike_offset(bucket);
        let at = |ms: u64| {
            std::time::UNIX_EPOCH
                + std::time::Duration::from_millis(bucket * LIGHTNING_PERIOD_MS + ms)
        };
        assert_eq!(
            Sky::at(at(off)).flash(),
            lightning_envelope(0),
            "bucket {bucket}"
        );
        assert_eq!(
            Sky::at(at(off + LIGHTNING_FLASH_MS)).flash(),
            0.0,
            "bucket {bucket}"
        );
        if off > 0 {
            assert_eq!(Sky::at(at(off - 1)).flash(), 0.0, "bucket {bucket}");
        }
    }
}

#[test]
fn night_exterior_tracks_weather_at_a_fixed_phase() {
    // One instant, so one moon phase: only the weather varies.
    let night = on_day(1, 2);
    let clear = Sky::at_with(night, Weather::Clear).light().exterior;
    let storm = Sky::at_with(night, Weather::Storm).light().exterior;
    assert!(
        clear > storm,
        "clear night brighter than storm night at equal phase: {clear} vs {storm}"
    );
    assert!(storm > 0.0, "storm night keeps some city glow: {storm}");
    let noon = Sky::at_with(at_hour_min(12, 0), Weather::Clear)
        .light()
        .exterior;
    assert!(noon > 0.9, "clear noon ~fully lit: {noon}");
}

#[test]
fn interior_brightness_is_altitude_coupled() {
    let noon = Sky::at_with(at_hour_min(12, 0), Weather::Storm)
        .light()
        .exterior;
    let dusk = Sky::at_with(at_hour_min(18, 0), Weather::Storm)
        .light()
        .exterior;
    assert!(
        noon > dusk,
        "a stormy noon out-lights a stormy dusk: {noon} vs {dusk}"
    );
}

#[test]
fn solar_noon_outshines_the_brightest_night() {
    // Snow/Clear at the FULLEST moon are the two brightest cases night can
    // offer — the highest `city_bounce` floor plus peak lunar illumination.
    let full_moon_day = (1..=31u32)
        .max_by(|&a, &b| {
            moon_phase_at(on_day(a, 2))
                .partial_cmp(&moon_phase_at(on_day(b, 2)))
                .expect("moon_phase is never NaN")
        })
        .expect("January has days");
    // Near the night arc's apex, so close to that night's brightest instant.
    let full_moon_midnight = on_day(full_moon_day, 0);

    let storm_noon = Sky::at_with(at_hour_min(12, 0), Weather::Storm)
        .light()
        .exterior;
    for w in [Weather::Clear, Weather::Snow] {
        let full_moon = Sky::at_with(full_moon_midnight, w).light().exterior;
        assert!(
            storm_noon > full_moon,
            "a stormy solar noon must outshine a {w:?} full-moon midnight: \
             storm_noon={storm_noon} vs {full_moon}"
        );
    }
}

/// `y-mo-d h:mi` UTC.
fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
    let secs = chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .and_then(|date| date.and_hms_opt(h, mi, 0))
        .expect("a valid instant")
        .and_utc()
        .timestamp();
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(secs).expect("after 1970"))
}

/// How far a true lunation strays from the mean one: under a day.
const MAX_MISS_DAYS: f32 = 1.0;

/// 2026's full moons, `(month, day, hour, minute)` UTC, from NASA GSFC's "Phases
/// of the Moon 2001 to 2100"
/// (<https://eclipse.gsfc.nasa.gov/phase/phases2001.html>, Universal Time).
const FULL_MOONS_2026: [(u32, u32, u32, u32); 13] = [
    (1, 3, 10, 3),
    (2, 1, 22, 9),
    (3, 3, 11, 38),
    (4, 2, 2, 12),
    (5, 1, 17, 23),
    (5, 31, 8, 45),
    (6, 29, 23, 57),
    (7, 29, 14, 36),
    (8, 28, 4, 18),
    (9, 26, 16, 49),
    (10, 26, 4, 12),
    (11, 24, 14, 53),
    (12, 24, 1, 28),
];

/// The phase lands on every 2026 new and full moon in the same NASA table as
/// [`FULL_MOONS_2026`]; the tolerance is the illuminated fraction a
/// [`MAX_MISS_DAYS`] miss leaves.
#[test]
fn the_moon_phase_lands_on_published_lunations() {
    let tolerance = (1.0 - (std::f32::consts::TAU * MAX_MISS_DAYS / SYNODIC_DAYS).cos()) / 2.0;
    let new_moons = [
        (1, 18, 19, 52),
        (2, 17, 12, 1),
        (3, 19, 1, 23),
        (4, 17, 11, 52),
        (5, 16, 20, 1),
        (6, 15, 2, 54),
        (7, 14, 9, 43),
        (8, 12, 17, 37),
        (9, 11, 3, 27),
        (10, 10, 15, 50),
        (11, 9, 7, 2),
        (12, 9, 0, 52),
    ];
    for (mo, d, h, mi) in new_moons {
        let lit = moon_phase_at(utc(2026, mo, d, h, mi));
        assert!(lit < tolerance, "new moon 2026-{mo}-{d} {h}:{mi} lit {lit}");
    }
    for (mo, d, h, mi) in FULL_MOONS_2026 {
        let lit = moon_phase_at(utc(2026, mo, d, h, mi));
        assert!(
            lit > 1.0 - tolerance,
            "full moon 2026-{mo}-{d} {h}:{mi} lit {lit}"
        );
    }
}

/// [`NEW_MOON_EPOCH_UNIX_DAYS`]'s literal and the instant its doc names are one
/// fact: a date that drifts from its number is how the old epoch went wrong.
#[test]
fn the_new_moon_epoch_is_the_instant_its_doc_names() {
    let named = utc(2019, 11, 27, 2, 56)
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs_f64()
        / 86_400.0;
    // A few minutes: above the literal's f32 step, below any mistyped date.
    assert!(
        (f64::from(NEW_MOON_EPOCH_UNIX_DAYS) - named).abs() < 0.005,
        "{NEW_MOON_EPOCH_UNIX_DAYS} is not 2019-11-27 02:56 ({named})"
    );
}

/// The moon waxes from new to full and wanes after, on the First and Last
/// Quarter columns of NASA GSFC's "Phases of the Moon 2001 to 2100"
/// (<https://eclipse.gsfc.nasa.gov/phase/phases2001.html>, Universal Time).
#[test]
fn the_moon_waxes_to_full_and_wanes_after() {
    for (mo, d, h, mi) in [(1, 26, 4, 47), (7, 21, 11, 6)] {
        assert!(
            Sky::at(utc(2026, mo, d, h, mi)).moon_waxing(),
            "first quarter 2026-{mo}-{d} {h}:{mi} waxes"
        );
    }
    for (mo, d, h, mi) in [(1, 10, 15, 48), (8, 6, 2, 21)] {
        assert!(
            !Sky::at(utc(2026, mo, d, h, mi)).moon_waxing(),
            "last quarter 2026-{mo}-{d} {h}:{mi} wanes"
        );
    }
    // The flip itself sits within a miss of every full moon, not just somewhere
    // between the quarters.
    let miss = std::time::Duration::from_secs_f32(MAX_MISS_DAYS * 86_400.0);
    for (mo, d, h, mi) in FULL_MOONS_2026 {
        let full = utc(2026, mo, d, h, mi);
        assert!(
            Sky::at(full - miss).moon_waxing(),
            "waxes a miss before full 2026-{mo}-{d}"
        );
        assert!(
            !Sky::at(full + miss).moon_waxing(),
            "wanes a miss after full 2026-{mo}-{d}"
        );
    }
}
