//! What the frame's [`Sky`] does to the office's colour and light, resolved
//! against the theme: the glass colours, the floor's night dim and daylight
//! lift, the weather tints and veils, the golden hour, the star gate and the
//! sun's spot on a wall.
//!
//! Pixel-free, so every painter reads one answer; where each lands on screen
//! stays with the painter.

use pixtuoid_core::sprite::Rgb;

use crate::sky::{Atmo, Body, Emitter, Sky, Weather};
use crate::theme::Theme;

/// [`time_of_day_look`]'s output for one [`Sky`].
pub(crate) struct TimeOfDayLook {
    pub(crate) glass_a: Rgb,
    pub(crate) glass_b: Rgb,
    pub(crate) spill_strength: f32,
    /// The floor spill's x-shift per row going down.
    pub(crate) spill_slant: f32,
    /// `1 - exterior`: the darkness the artificial lights fight.
    pub(crate) darkness: f32,
    /// The cast this sky puts on a LIT OBJECT: the cool night term then the
    /// warm day one, applied in order like the floor's two overlays.
    pub(crate) object_wash: [(Rgb, f32); 2],
}

// Max window-spill horizontal lean (px/row) at the low-sun extremes.
const SPILL_SLANT_MAX: f32 = 0.7;

pub(crate) fn time_of_day_look(sky: &Sky, theme: &Theme) -> TimeOfDayLook {
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

    // Leans away from the disc, which `compute_disc` places off this same azimuth;
    // `the_wall_spot_and_the_spill_fall_away_from_the_disc` pins the sign.
    let (spill_strength, spill_slant) = match e.body {
        Body::Sun => (interior, (0.5 - e.azimuth) * 2.0 * SPILL_SLANT_MAX),
        Body::Moon => (0.0, 0.0),
    };

    // Below the floor's own share: a sprite carries art contrast a full-strength pass would swallow.
    const OBJECT_WASH_SHARE: f32 = 0.55;
    let darkness = 1.0 - exterior;
    // SUPERPOSED, never chosen between: the floor runs both overlays every frame,
    // so switching arms on `interior >= darkness` would step every object in the
    // frame that crossed it while the floor slid smoothly under them.
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

/// How far a fully dark hour dims the interior.
pub(crate) const NIGHT_FLOOR_DIM: f32 = 0.45;

/// How far a fully lit hour lifts it.
pub(crate) const DAYLIGHT_FLOOR_LIFT: f32 = 0.22;

/// Pale warm midday sunlight — theme-agnostic, since daylight is daylight.
pub(crate) const SUN_TINT: Rgb = Rgb {
    r: 255,
    g: 246,
    b: 224,
};

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

pub(crate) fn sun_on_wall(sky: &Sky) -> Option<SunSpot> {
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
pub(crate) fn weather_floor_tint(w: Weather) -> Rgb {
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
pub(crate) fn glass_veil(w: Weather) -> Option<(Rgb, f32)> {
    match w {
        Weather::Fog => Some((
            Rgb {
                r: 201,
                g: 204,
                b: 211,
            },
            0.66,
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
            0.45,
        )),
        Weather::Overcast => Some((
            Rgb {
                r: 131,
                g: 135,
                b: 141,
            },
            0.30,
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
/// `atmo`/`look.darkness`: those already carry the weather (the veil colour does
/// too), and folding them in would darken a stormy noon twice.
pub(crate) fn veil_lum(e: &Emitter) -> f32 {
    NIGHT_VEIL_FLOOR + (1.0 - NIGHT_VEIL_FLOOR) * e.emitter_lum.clamp(0.0, 1.0)
}

/// Golden-hour blaze strength on the city silhouette — SUN-only: a low moon
/// must never paint an orange cast, however warm/lit it computes, so the gate
/// is absolute rather than incidental.
pub(crate) fn golden_hour_blaze(e: &Emitter, a: &Atmo) -> f32 {
    match e.body {
        Body::Sun => (e.warmth * e.emitter_lum * a.disc).clamp(0.0, 1.0),
        Body::Moon => 0.0,
    }
}

/// How brightly the star field shows this frame. Stars only appear once the
/// emitter is the MOON: dawn/dusk twilight has a high `darkness` yet the
/// brightening sky washes stars out, so gating on `darkness` alone paints a
/// full starfield at ~7am.
pub(crate) fn night_star_strength(sky: &Sky, darkness: f32) -> f32 {
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
