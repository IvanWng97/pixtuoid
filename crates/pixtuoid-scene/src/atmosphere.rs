//! What the frame's [`Sky`] does to the office's colour and light, as values
//! rather than pixels, so the painters share one answer; where each lands on
//! screen stays with the painter.

use pixtuoid_core::sprite::Rgb;

use crate::sky::{Atmo, Body, Emitter, Sky, Weather};
use crate::theme::Theme;

/// One frame's sky, resolved against the theme: [`Look::resolve`] once per
/// frame, every field a frame-wide answer.
pub(crate) struct Look {
    /// The window sky near the horizon.
    pub(crate) glass_a: Rgb,
    /// The window sky near the top of the pane.
    pub(crate) glass_b: Rgb,
    /// `1 - exterior`: the darkness the artificial lights fight.
    pub(crate) darkness: f32,
    /// The sun's light reaching the interior, 0..=1; none under the moon.
    pub(crate) sunlight: f32,
    /// The window spill's lean going down, as a slope: columns per row.
    pub(crate) spill_slant: f32,
    /// The floor's night dim then its daylight lift, as `(tint, strength)`
    /// blends applied in order.
    pub(crate) floor_wash: [(Rgb, f32); 2],
    /// The cast this sky puts on a LIT OBJECT: the cool night term then the
    /// warm day one, applied in order like the floor's two washes.
    pub(crate) object_wash: [(Rgb, f32); 2],
    /// The weather's cast on the floor, as `(tint, strength)`.
    pub(crate) floor_tint: (Rgb, f32),
    /// The weather's veil over the window glass, lit for this frame, as
    /// `(color, alpha)`, or `None` where the city shows crisp.
    pub(crate) glass_veil: Option<(Rgb, f32)>,
    /// How strongly the golden hour blazes on the city, 0..=1.
    pub(crate) golden_hour: f32,
    /// How brightly the star field shows, 0..=1; zero wherever it would be too
    /// faint to read.
    pub(crate) star_strength: f32,
    /// Where the sun lands on the office walls, while it is up.
    pub(crate) sun_spot: Option<SunSpot>,
}

/// The steepest the window spill leans, at the low-sun extremes.
const SPILL_SLANT_MAX: f32 = 0.7;

/// How far a fully dark hour dims the interior.
const NIGHT_FLOOR_DIM: f32 = 0.45;

/// How far a fully lit hour lifts the interior.
const DAYLIGHT_FLOOR_LIFT: f32 = 0.22;

/// Pale warm midday sunlight, the same under every theme.
const SUN_TINT: Rgb = Rgb {
    r: 255,
    g: 246,
    b: 224,
};

/// Below the floor's own share: a sprite carries art contrast a full-strength
/// wash would swallow.
const OBJECT_WASH_SHARE: f32 = 0.55;

/// How far the weather's cast pulls the floor toward its tint.
const FLOOR_TINT_SHARE: f32 = 0.15;

/// Below this strength the star field is too faint to read, so none shows —
/// by day and under thick cloud or fog.
const STAR_MIN: f32 = 0.15;

impl Look {
    pub(crate) fn resolve(sky: &Sky, theme: &Theme) -> Look {
        let light = sky.light();
        let (interior, exterior) = (light.interior, light.exterior);
        let e = sky.emitter();

        let day_a = theme.lighting.day_sky_a;
        let day_b = theme.lighting.day_sky_b;
        let night_a = theme.lighting.night_sky_a;
        let night_b = theme.lighting.night_sky_b;
        let twilight_a = theme.lighting.twilight_a;
        let twilight_b = theme.lighting.twilight_b;

        let warm = (e.warmth * interior).clamp(0.0, 1.0);
        let glass_a = night_a.mix(day_a, exterior).mix(twilight_a, warm * 0.5);
        let glass_b = night_b.mix(day_b, exterior).mix(twilight_b, warm * 0.5);

        // Leans away from the disc, which the painters place off this same
        // azimuth; `the_wall_spot_and_the_spill_fall_away_from_the_disc` pins the
        // sign.
        let (sunlight, spill_slant) = match e.body {
            Body::Sun => (interior, (0.5 - e.azimuth) * 2.0 * SPILL_SLANT_MAX),
            Body::Moon => (0.0, 0.0),
        };

        let darkness = 1.0 - exterior;
        let floor_wash = [
            (theme.lighting.night_tint, darkness * NIGHT_FLOOR_DIM),
            (SUN_TINT, sunlight * DAYLIGHT_FLOOR_LIFT),
        ];
        // SUPERPOSED, never chosen between: the floor runs both washes every
        // frame, so switching arms on `interior >= darkness` would step every
        // object in the frame that crossed it while the floor slid smoothly under
        // them.
        let object_wash = [
            (
                theme.lighting.night_tint,
                darkness * NIGHT_FLOOR_DIM * OBJECT_WASH_SHARE,
            ),
            (SUN_TINT, interior * DAYLIGHT_FLOOR_LIFT * OBJECT_WASH_SHARE),
        ];

        let weather = sky.weather();
        let veil = veil_lum(e);
        let star_strength = night_star_strength(sky, darkness);

        Look {
            glass_a,
            glass_b,
            darkness,
            sunlight,
            spill_slant,
            floor_wash,
            object_wash,
            floor_tint: (weather_floor_tint(weather), FLOOR_TINT_SHARE),
            glass_veil: glass_veil(weather).map(|(color, alpha)| (lit(color, veil), alpha)),
            golden_hour: golden_hour_blaze(e, &sky.atmo()),
            star_strength: if star_strength > STAR_MIN {
                star_strength
            } else {
                0.0
            },
            sun_spot: sun_on_wall(sky),
        }
    }
}

/// `color` at `lum` of its own brightness, its hue kept.
fn lit(color: Rgb, lum: f32) -> Rgb {
    let scale = |c: u8| (f32::from(c) * lum).round() as u8;
    Rgb {
        r: scale(color.r),
        g: scale(color.g),
        b: scale(color.b),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WallSide {
    East,
    North,
    West,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SunSpot {
    pub(crate) wall: WallSide,
    /// 0.0..=1.0 along the wall (left→right for North, top→bottom for East/West).
    pub(crate) along: f32,
    /// 0.0=dim, 1.0=brightest at noon.
    pub(crate) intensity: f32,
    /// 0.0=neutral white (noon), 1.0=very warm gold (sunrise/sunset).
    pub(crate) warmth: f32,
}

/// The azimuths where [`sun_on_wall`]'s spot hands off east wall → window
/// wall → west wall.
const AZ_EAST_MAX: f32 = 0.30;
const AZ_WEST_MIN: f32 = 0.70;

fn sun_on_wall(sky: &Sky) -> Option<SunSpot> {
    let e = sky.emitter();
    if !matches!(e.body, Body::Sun) {
        return None;
    }
    // The SAME azimuth that places the disc and leans the floor spill;
    // `the_wall_spot_and_the_spill_fall_away_from_the_disc` pins their sides.
    let az = e.azimuth;
    let (wall, along) = if az < AZ_EAST_MAX {
        (WallSide::East, az / AZ_EAST_MAX)
    } else if az < AZ_WEST_MIN {
        (
            WallSide::North,
            (az - AZ_EAST_MAX) / (AZ_WEST_MIN - AZ_EAST_MAX),
        )
    } else {
        (WallSide::West, (az - AZ_WEST_MIN) / (1.0 - AZ_WEST_MIN))
    };
    Some(SunSpot {
        wall,
        along,
        intensity: e.altitude,
        warmth: e.warmth,
    })
}

/// The cast the current weather lends the floor.
fn weather_floor_tint(w: Weather) -> Rgb {
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

/// The veil a weather lays over the window glass, as `(color, alpha)` before
/// [`veil_lum`] lights it, or `None` where the city shows crisp.
fn glass_veil(w: Weather) -> Option<(Rgb, f32)> {
    match w {
        Weather::Fog => Some((
            Rgb {
                r: 201,
                g: 204,
                b: 211,
            },
            0.6625,
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
                r: 170,
                g: 153,
                b: 110,
            },
            0.454,
        )),
        Weather::Overcast => Some((
            Rgb {
                r: 131,
                g: 135,
                b: 141,
            },
            0.296,
        )),
        Weather::Clear | Weather::Snow | Weather::Windy => None,
    }
}

/// The least of a veil's own colour [`veil_lum`] brings up: the city-light
/// scatter that keeps fog reading as fog after dark.
const NIGHT_VEIL_FLOOR: f32 = 0.35;

/// How much of a weather VEIL's own colour the frame's sky brings up (0..1).
///
/// The day term is the emitter's OWN luminance, deliberately NOT
/// [`Sky::atmo`] or [`Look::darkness`]: those already carry the weather (the veil
/// colour does too), and folding them in would darken a stormy noon twice.
fn veil_lum(e: &Emitter) -> f32 {
    NIGHT_VEIL_FLOOR + (1.0 - NIGHT_VEIL_FLOOR) * e.emitter_lum.clamp(0.0, 1.0)
}

/// Golden-hour blaze strength on the city silhouette — SUN-only: a low moon
/// must never paint an orange cast, however warm/lit it computes, so the gate
/// is absolute rather than incidental.
fn golden_hour_blaze(e: &Emitter, a: &Atmo) -> f32 {
    match e.body {
        Body::Sun => (e.warmth * e.emitter_lum * a.disc).clamp(0.0, 1.0),
        Body::Moon => 0.0,
    }
}

/// How brightly the star field would show this frame, before [`STAR_MIN`]'s
/// gate. Stars only appear once the emitter is the MOON: dawn/dusk twilight has
/// a high `darkness` yet the brightening sky washes stars out, so gating on
/// `darkness` alone paints a full starfield at ~7am.
fn night_star_strength(sky: &Sky, darkness: f32) -> f32 {
    match sky.emitter().body {
        Body::Moon => (darkness * sky.atmo().disc).clamp(0.0, 1.0),
        Body::Sun => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localclock::at_hour_min;

    // Hand-built Emitter/Atmo values, not real clock times: a real moon's low
    // altitude/luminance could never produce these, so a maximally warm/lit MOON
    // proves the gate is absolute rather than merely well-behaved in practice.
    #[test]
    fn golden_hour_blaze_is_sun_only() {
        let full_atmo = Atmo {
            direct: 1.0,
            diffuse: 1.0,
            disc: 1.0,
        };
        let moon = Emitter {
            body: Body::Moon,
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
        let sun = Emitter {
            body: Body::Sun,
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
        let fog = weather_floor_tint(Weather::Fog);
        let oc = weather_floor_tint(Weather::Overcast);
        let lum = |c: Rgb| c.r as u16 + c.g as u16 + c.b as u16;
        assert!(
            lum(fog) > lum(oc),
            "fog {fog:?} should outshine overcast {oc:?}"
        );
    }

    #[test]
    fn glass_veil_obscures_fog_and_storm_only_when_expected() {
        let fog = glass_veil(Weather::Fog).expect("fog veils").1;
        let storm = glass_veil(Weather::Storm).expect("storm veils").1;
        assert!(fog > storm, "fog should obscure more than storm");
        assert!(
            glass_veil(Weather::Clear).is_none(),
            "clear skyline is crisp"
        );
        assert!(glass_veil(Weather::Snow).is_none(), "snow skyline is crisp");
    }

    #[test]
    fn stars_gate_on_night_not_darkness_alone() {
        // A HIGH darkness passed at an hour when the sun is up: the pale dawn sky
        // is dark enough to pass a darkness gate, yet washes the stars out.
        let at = crate::localclock::at_hour;
        assert_eq!(
            night_star_strength(&Sky::at_with(at(7), Weather::Clear), 0.6),
            0.0,
            "no stars at 7am while the sun is up"
        );
        let stars = |w| Look::resolve(&Sky::at_with(at(2), w), &crate::theme::NORMAL).star_strength;
        assert!(
            stars(Weather::Clear) > STAR_MIN,
            "a clear night should light the stars"
        );
        assert_eq!(
            stars(Weather::Overcast),
            0.0,
            "overcast should hide the stars even at night"
        );
    }

    #[test]
    fn the_floor_is_dimmed_by_the_dark_and_lifted_by_the_sun() {
        let at = crate::localclock::at_hour;
        let wash = |h| {
            Look::resolve(&Sky::at_with(at(h), Weather::Clear), &crate::theme::NORMAL).floor_wash
        };
        let [(_, noon_dim), (tint, noon_lift)] = wash(12);
        let [(dim_tint, night_dim), (_, night_lift)] = wash(0);
        assert_eq!(dim_tint, crate::theme::NORMAL.lighting.night_tint);
        assert_eq!(tint, SUN_TINT);
        assert!(noon_lift > 0.0, "a clear noon lifts the floor");
        assert_eq!(night_lift, 0.0, "the moon lifts nothing");
        assert!(night_dim > noon_dim, "midnight dims the floor past noon");
    }

    /// The veil keeps the weather reading after dark: dimmer than by day, but
    /// never below [`NIGHT_VEIL_FLOOR`] of its own colour.
    #[test]
    fn a_veil_dims_after_dark_but_keeps_its_floor() {
        let at = crate::localclock::at_hour;
        let veil = |h| {
            Look::resolve(&Sky::at_with(at(h), Weather::Fog), &crate::theme::NORMAL)
                .glass_veil
                .expect("fog veils the glass")
                .0
        };
        let lum = |c: Rgb| f32::from(c.r) + f32::from(c.g) + f32::from(c.b);
        let unlit = glass_veil(Weather::Fog).expect("fog veils the glass").0;
        let (noon, midnight) = (veil(12), veil(0));
        assert!(lum(midnight) < lum(noon), "{midnight:?} vs {noon:?}");
        // Each channel rounds by at most half a level.
        assert!(
            lum(midnight) >= lum(unlit) * NIGHT_VEIL_FLOOR - 1.5,
            "{midnight:?} fell below the night floor of {unlit:?}"
        );
    }

    #[test]
    fn sun_on_wall_east_at_morning() {
        let s = sun_on_wall(&Sky::at(at_hour_min(7, 0))).expect("sun should be up at 07:00");
        assert_eq!(s.wall, WallSide::East);
        assert!(s.warmth > 0.5, "morning sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_overhead_at_noon() {
        let s = sun_on_wall(&Sky::at(at_hour_min(12, 0))).expect("sun should be up at 12:00");
        assert_eq!(s.wall, WallSide::North);
        assert!(
            s.intensity > 0.85,
            "noon sun should be intense: {}",
            s.intensity
        );
    }

    #[test]
    fn sun_on_wall_west_at_evening() {
        let s = sun_on_wall(&Sky::at(at_hour_min(18, 0))).expect("sun should be up at 18:00");
        assert_eq!(s.wall, WallSide::West);
        assert!(s.warmth > 0.55, "evening sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_none_at_midnight() {
        assert!(sun_on_wall(&Sky::at(at_hour_min(0, 0))).is_none());
    }
}
