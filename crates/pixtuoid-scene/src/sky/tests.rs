use std::time::SystemTime;

use super::*;
use crate::localclock::{at_hour_min, on_day};

/// Local `h:m` on the reference day — `localclock` owns the construction.
fn at_hour(h: u32, m: u32) -> SystemTime {
    at_hour_min(h, m)
}

/// Local 02:00 (always night) on a given January day.
fn night_on(day: u32) -> SystemTime {
    on_day(day, 2)
}

#[test]
fn weather_state_emits_every_variant_within_a_week() {
    use std::collections::HashSet;
    use std::time::Duration;
    let start = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut seen: HashSet<Weather> = HashSet::new();
    for slot in 0..(7u64 * 24 * 6) {
        seen.insert(weather_at(start + Duration::from_secs(slot * 600)));
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
    let noon = Sky::at(at_hour(12, 30)).emitter().altitude;
    let dawn = Sky::at(at_hour(6, 30)).emitter().altitude;
    let dusk = Sky::at(at_hour(18, 0)).emitter().altitude;
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
    assert!(
        Sky::at(at_hour(6, 30)).emitter().warmth > 0.6,
        "low sun is warm/red"
    );
    assert!(
        Sky::at(at_hour(12, 30)).emitter().warmth < 0.3,
        "apex sun is neutral"
    );
}

#[test]
fn azimuth_advances_west_across_the_day() {
    let a = Sky::at(at_hour(7, 0)).emitter().azimuth;
    let b = Sky::at(at_hour(12, 0)).emitter().azimuth;
    let c = Sky::at(at_hour(18, 0)).emitter().azimuth;
    assert!(a < b && b < c, "azimuth marches E->W: {a} < {b} < {c}");
}

#[test]
fn moon_luminance_tracks_phase() {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut lo_lum, mut hi_lum) = (0.0, 0.0);
    for day in 1..=30u32 {
        let s = night_on(day);
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
    let natural = weather_at(t);
    // Force a variant that differs from the natural pick so the assert is real.
    let forced = Weather::ALL
        .into_iter()
        .find(|&w| w != natural)
        .expect("8 variants");
    set_weather_override(Some(forced));
    assert_eq!(weather_at(t), forced);
    assert_eq!(
        weather_at(t + Duration::from_secs(987_654)),
        forced,
        "override is time-independent"
    );
    set_weather_override(None);
    assert_eq!(weather_at(t), natural, "None restores time-based selection");
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
