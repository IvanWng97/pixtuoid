//! The time of day as the classic painter shows it — glass colours, sunlight
//! spill, the sun's spot on a wall, and the floor tint overlays — resolved from
//! the frame's [`Sky`] and the theme.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::pixel_painter::palette::{blend_rgb, mix_lab, RgbLut};
use crate::sky::{Body, Sky};
use crate::theme::Theme;

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

pub(in crate::pixel_painter) fn time_of_day_look(sky: &Sky, theme: &Theme) -> TimeOfDayLook {
    let light = sky.light();
    let (interior, exterior) = (light.interior, light.exterior);

    let day_a = theme.lighting.day_sky_a;
    let day_b = theme.lighting.day_sky_b;
    let night_a = theme.lighting.night_sky_a;
    let night_b = theme.lighting.night_sky_b;
    let twilight_a = theme.lighting.twilight_a;
    let twilight_b = theme.lighting.twilight_b;

    let warm = (sky.emitter().warmth * interior).clamp(0.0, 1.0);
    let glass_a = mix_lab(mix_lab(night_a, day_a, exterior), twilight_a, warm * 0.5);
    let glass_b = mix_lab(mix_lab(night_b, day_b, exterior), twilight_b, warm * 0.5);

    let spill_strength = match sky.emitter().body {
        Body::Sun => interior,
        Body::Moon => 0.0,
    };
    let spill_slant = light.spill_slant;

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

pub(in crate::pixel_painter) fn sun_on_wall(sky: &Sky) -> Option<SunSpot> {
    let sky = sky.emitter();
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
/// byte-identical to per-pixel [`blend_rgb`] (#900).
fn blend_floor_band(buf: &mut RgbBuffer, top_y: u16, bottom_y: u16, tint: Rgb, s: f32) {
    if s <= 0.0 {
        return;
    }
    let w = buf.width() as usize;
    let start = (top_y.min(buf.height()) as usize) * w;
    let end = (bottom_y.min(buf.height()) as usize) * w;
    if start >= end {
        return;
    }
    let lut = RgbLut::tabulate(|c| blend_rgb(c, tint, s));
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

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::sky::{atmo, set_weather_override, Weather};

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
    fn blend_floor_band_clamps_degenerate_bounds() {
        let tint = Rgb {
            r: 200,
            g: 180,
            b: 120,
        };
        let mut buf = RgbBuffer::filled(
            6,
            5,
            Rgb {
                r: 40,
                g: 50,
                b: 60,
            },
        );
        let before = buf.clone();
        blend_floor_band(&mut buf, 4, 2, tint, 0.5);
        assert_eq!(
            buf.as_slice(),
            before.as_slice(),
            "top >= bottom is a no-op"
        );
        blend_floor_band(&mut buf, 9, 12, tint, 0.5);
        assert_eq!(
            buf.as_slice(),
            before.as_slice(),
            "a band entirely past the bottom edge is a no-op"
        );
        blend_floor_band(&mut buf, 3, 12, tint, 0.5);
        let mut expected = before.clone();
        for y in 3..5u16 {
            for x in 0..6u16 {
                expected.put(x, y, blend_rgb(expected.get(x, y), tint, 0.5));
            }
        }
        assert_eq!(
            buf.as_slice(),
            expected.as_slice(),
            "bottom_y clamps to the buffer height"
        );
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
        let clear = time_of_day_look(&Sky::at(night), theme).darkness;
        set_weather_override(Some(Weather::Storm));
        let storm = time_of_day_look(&Sky::at(night), theme).darkness;
        set_weather_override(None);
        assert!(
            clear < storm,
            "clear night brighter than storm night at equal phase: {clear} vs {storm}"
        );
        assert!(storm < 1.0, "storm night keeps some city glow: {storm}");
        set_weather_override(Some(Weather::Clear));
        let noon = time_of_day_look(&Sky::at(at_hour(12, 0)), theme).darkness;
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
        let noon = time_of_day_look(&Sky::at(at_hour(12, 0)), theme).darkness;
        let dusk = time_of_day_look(&Sky::at(at_hour(18, 0)), theme).darkness;
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
                Sky::at(night_on(a))
                    .moon_phase()
                    .partial_cmp(&Sky::at(night_on(b)).moon_phase())
                    .expect("moon_phase is never NaN")
            })
            .expect("January has days");
        let full_moon_midnight = midnight_on(full_moon_day);

        set_weather_override(Some(Weather::Storm));
        let storm_noon = time_of_day_look(&Sky::at(at_hour(12, 0)), theme).darkness;

        set_weather_override(Some(Weather::Clear));
        let clear_full_moon = time_of_day_look(&Sky::at(full_moon_midnight), theme).darkness;

        set_weather_override(Some(Weather::Snow));
        let snow_full_moon = time_of_day_look(&Sky::at(full_moon_midnight), theme).darkness;

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
        let s = sun_on_wall(&Sky::at(at_hour(7, 0))).expect("sun should be up at 07:00");
        assert_eq!(s.wall, WallSide::East);
        assert!(s.warmth > 0.5, "morning sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_overhead_at_noon() {
        let s = sun_on_wall(&Sky::at(at_hour(12, 0))).expect("sun should be up at 12:00");
        assert_eq!(s.wall, WallSide::South);
        assert!(
            s.intensity > 0.85,
            "noon sun should be intense: {}",
            s.intensity
        );
    }

    #[test]
    fn sun_on_wall_west_at_evening() {
        let s = sun_on_wall(&Sky::at(at_hour(18, 0))).expect("sun should be up at 18:00");
        assert_eq!(s.wall, WallSide::West);
        assert!(s.warmth > 0.55, "evening sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_none_at_midnight() {
        assert!(sun_on_wall(&Sky::at(at_hour(0, 0))).is_none());
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
}
