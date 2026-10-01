//! What the frame's [`Sky`] does to the office's colour and light, as values
//! rather than pixels, so the painters share one answer; where each lands on
//! screen stays with the painter.

use pixtuoid_core::sprite::Rgb;

use crate::sky::{BodyKind, Sky, SkyBody, Transmission, Weather};
use crate::theme::Theme;

/// One frame's sky, resolved against the theme: [`SkyTones::resolve`] once per
/// frame, every field a frame-wide answer.
pub(crate) struct SkyTones {
    /// The window sky near the horizon.
    pub(crate) glass_horizon: Rgb,
    /// The window sky near the top of the pane.
    pub(crate) glass_zenith: Rgb,
    /// `1 - exterior`: the darkness the artificial lights fight.
    pub(crate) darkness: f32,
    /// The sun's light reaching the interior, 0..=1; none under the moon.
    pub(crate) sunlight: f32,
    /// The window spill's lean going down, as a slope: columns per row.
    pub(crate) spill_slant: f32,
    /// The ground's night dim then its daylight lift, as `(tint, strength)`
    /// blends applied in order.
    pub(crate) ground_wash: [(Rgb, f32); 2],
    /// The cast this sky puts on a LIT OBJECT: the cool night term then the
    /// warm day one, applied in order like the ground's two washes.
    pub(crate) object_wash: [(Rgb, f32); 2],
    /// The weather's cast on the ground, as `(tint, strength)`.
    pub(crate) ground_tint: (Rgb, f32),
    /// The weather's veil over the window glass, lit for this frame, as
    /// `(color, alpha)`, or `None` where the city shows crisp.
    pub(crate) glass_veil: Option<(Rgb, f32)>,
    /// How strongly the golden hour blazes in the sky around the city, 0..=1.
    pub(crate) golden_hour: f32,
    /// How brightly the star field shows, 0..=1; zero wherever it would be too
    /// faint to read.
    pub(crate) star_strength: f32,
    /// The sun's direct beam through the weather ([`Sky::beam`]), 0..=1.
    pub(crate) beam: f32,
}

/// The moment a frame shows, to either painter: the sky at `now` and its
/// [`SkyTones`], seen from `altitude` ([`FloorMeta::altitude`](crate::floor::FloorMeta::altitude)).
pub(crate) struct Moment {
    pub(crate) sky: Sky,
    /// Always `sky`'s, resolved against the frame's theme.
    pub(crate) look: SkyTones,
    pub(crate) altitude: f32,
    pub(crate) now: std::time::SystemTime,
}

impl Moment {
    /// `sky`, looked at under `theme` from `altitude` at `now`. The sky is
    /// given, not derived from `now`, so a forced weather or flash carries.
    pub(crate) fn resolve(
        sky: Sky,
        theme: &Theme,
        altitude: f32,
        now: std::time::SystemTime,
    ) -> Self {
        Self {
            look: SkyTones::resolve(&sky, theme),
            sky,
            altitude,
            now,
        }
    }
}

/// The steepest the window spill leans, at the low-sun extremes.
const SPILL_SLANT_MAX: f32 = 0.7;

/// How far a fully dark hour dims the interior.
const NIGHT_GROUND_DIM: f32 = 0.45;

/// How far a fully lit hour lifts the interior.
const DAYLIGHT_GROUND_LIFT: f32 = 0.22;

/// Pale warm midday sunlight, the same under every theme.
const SUN_TINT: Rgb = Rgb {
    r: 255,
    g: 246,
    b: 224,
};

/// Below the ground's own share: a sprite carries art contrast a full-strength
/// wash would swallow.
const OBJECT_WASH_SHARE: f32 = 0.55;

/// How far the weather's cast pulls the ground toward its tint.
const GROUND_TINT_SHARE: f32 = 0.15;

/// Below this strength the star field is too faint to read, so none shows —
/// by day and under thick cloud or fog; above it the stars ramp in from nothing.
const STAR_MIN: f32 = 0.15;

impl SkyTones {
    pub(crate) fn resolve(sky: &Sky, theme: &Theme) -> SkyTones {
        let light = sky.light();
        let (interior, exterior) = (light.interior, light.exterior);
        let e = sky.body();

        let day_a = theme.lighting.day_sky_a;
        let day_b = theme.lighting.day_sky_b;
        let night_a = theme.lighting.night_sky_a;
        let night_b = theme.lighting.night_sky_b;
        let twilight_a = theme.lighting.twilight_a;
        let twilight_b = theme.lighting.twilight_b;

        let warm = (e.warmth * interior).clamp(0.0, 1.0);
        let glass_horizon = night_a.mix(day_a, exterior).mix(twilight_a, warm * 0.5);
        let glass_zenith = night_b.mix(day_b, exterior).mix(twilight_b, warm * 0.5);

        // Leans away from the disc, which the painters place off this same
        // azimuth; `the_spill_leans_away_from_the_disc` pins the
        // sign.
        let (sunlight, spill_slant) = match e.kind {
            BodyKind::Sun => (interior, (0.5 - e.azimuth) * 2.0 * SPILL_SLANT_MAX),
            BodyKind::Moon => (0.0, 0.0),
        };

        let darkness = 1.0 - exterior;
        let ground_wash = [
            (theme.lighting.night_tint, darkness * NIGHT_GROUND_DIM),
            (SUN_TINT, sunlight * DAYLIGHT_GROUND_LIFT),
        ];
        // SUPERPOSED, never chosen between: the ground runs both washes every
        // frame, so switching arms on `interior >= darkness` would step every
        // object in the frame that crossed it while the ground slid smoothly under
        // them.
        let object_wash = [
            (
                theme.lighting.night_tint,
                darkness * NIGHT_GROUND_DIM * OBJECT_WASH_SHARE,
            ),
            (
                SUN_TINT,
                sunlight * DAYLIGHT_GROUND_LIFT * OBJECT_WASH_SHARE,
            ),
        ];

        let weather = sky.weather();
        let veil = veil_lum(e);
        let star_strength = night_star_strength(sky, darkness);

        SkyTones {
            glass_horizon,
            glass_zenith,
            darkness,
            sunlight,
            spill_slant,
            ground_wash,
            object_wash,
            ground_tint: (weather_ground_tint(weather), GROUND_TINT_SHARE),
            glass_veil: glass_veil(weather).map(|(color, alpha)| (lit(color, veil), alpha)),
            golden_hour: golden_hour_blaze(e, &sky.transmission()),
            star_strength: ((star_strength - STAR_MIN) / (1.0 - STAR_MIN)).max(0.0),
            beam: sky.beam(),
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

/// The cast the current weather lends the ground.
fn weather_ground_tint(w: Weather) -> Rgb {
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
        // Fog is a luminous white-out — its ground tint must be brighter than
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
const NIGHT_VEIL_MIN: f32 = 0.35;

/// How much of a weather VEIL's own colour the frame's sky brings up (0..1).
///
/// The day term is the sky body's OWN luminance, deliberately NOT
/// [`Sky::transmission`] or [`SkyTones::darkness`]: those already carry the weather (the veil
/// colour does too), and folding them in would darken a stormy noon twice.
fn veil_lum(e: &SkyBody) -> f32 {
    NIGHT_VEIL_MIN + (1.0 - NIGHT_VEIL_MIN) * e.lum.clamp(0.0, 1.0)
}

/// Golden-hour blaze strength in the sky around the city — SUN-only: a low moon
/// must never paint an orange cast, however warm/lit it computes, so the gate
/// is absolute rather than incidental.
fn golden_hour_blaze(e: &SkyBody, a: &Transmission) -> f32 {
    match e.kind {
        BodyKind::Sun => (e.warmth * e.lum * a.disc).clamp(0.0, 1.0),
        BodyKind::Moon => 0.0,
    }
}

/// How brightly the star field would show this frame, before [`STAR_MIN`]'s
/// ramp. Stars ride [`Sky::nightfall`], not `darkness` alone: the dawn and dusk
/// sky is dark enough to pass a darkness gate, yet washes stars out.
fn night_star_strength(sky: &Sky, darkness: f32) -> f32 {
    (darkness * sky.transmission().disc * sky.nightfall()).clamp(0.0, 1.0)
}

/// How far down the glass, as a share of it, the sky reaches its horizon
/// colour ([`SkyTones::glass_horizon`]) from its zenith's ([`SkyTones::glass_zenith`]).
const HORIZON_AT: f32 = 0.7;

/// How far from its zenith colour toward its horizon's the sky is `gy` rows
/// down glass `glass_h` tall, 0..=1.
pub(crate) fn sky_share(gy: f32, glass_h: u16) -> f32 {
    (gy / (f32::from(glass_h) * HORIZON_AT)).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localclock::at_hour_min;

    // Hand-built SkyBody/Transmission values, not real clock times: a real moon's low
    // altitude/luminance could never produce these, so a maximally warm/lit MOON
    // proves the gate is absolute rather than merely well-behaved in practice.
    #[test]
    fn golden_hour_blaze_is_sun_only() {
        let full = Transmission {
            direct: 1.0,
            diffuse: 1.0,
            disc: 1.0,
        };
        let moon = SkyBody {
            kind: BodyKind::Moon,
            altitude: 1.0,
            azimuth: 0.5,
            warmth: 1.0,
            lum: 1.0,
        };
        assert_eq!(
            golden_hour_blaze(&moon, &full),
            0.0,
            "a moon must never blaze, even at maximal warmth/luminance"
        );
        let sun = SkyBody {
            kind: BodyKind::Sun,
            ..moon
        };
        assert!(
            golden_hour_blaze(&sun, &full) > 0.9,
            "a maximal sun should blaze near-full"
        );
    }

    /// Every value the sky hands the painters moves by at most a step a minute,
    /// through every hour, weather and moon age: none flips at dusk or dawn.
    #[test]
    fn the_look_moves_without_a_step_minute_by_minute() {
        const MAX_STEP: f32 = 0.04;
        const MAX_CHANNEL_STEP: i16 = 4;
        let scalars = |l: &SkyTones| {
            [
                ("darkness", l.darkness),
                ("sunlight", l.sunlight),
                ("ground dim", l.ground_wash[0].1),
                ("ground lift", l.ground_wash[1].1),
                ("object dim", l.object_wash[0].1),
                ("object lift", l.object_wash[1].1),
                ("golden hour", l.golden_hour),
                ("stars", l.star_strength),
                ("beam", l.beam),
            ]
        };
        let channels = |l: &SkyTones| {
            let veil = l.glass_veil.map_or(Rgb { r: 0, g: 0, b: 0 }, |v| v.0);
            [
                ("glass horizon", l.glass_horizon),
                ("glass zenith", l.glass_zenith),
                ("veil", veil),
            ]
        };
        let weathers = [Weather::Clear, Weather::Fog, Weather::Snow, Weather::Storm];
        for day in [0, 4, 8, 11, 15, 19, 23, 26] {
            let midnight = crate::localclock::on_day(day, 0);
            for w in weathers {
                let look = |m: u64| {
                    let now = midnight + std::time::Duration::from_secs(60 * m);
                    SkyTones::resolve(&Sky::at_with(now, w), &crate::theme::NORMAL)
                };
                let mut prev = look(0);
                for m in 1..24 * 60 {
                    let next = look(m);
                    let at = format!("day {day} {w:?} {:02}:{:02}", m / 60, m % 60);
                    for ((name, a), (_, b)) in scalars(&prev).into_iter().zip(scalars(&next)) {
                        assert!(
                            (b - a).abs() <= MAX_STEP,
                            "{name} stepped {a} -> {b} at {at}"
                        );
                    }
                    for ((name, a), (_, b)) in channels(&prev).into_iter().zip(channels(&next)) {
                        let step = [(a.r, b.r), (a.g, b.g), (a.b, b.b)]
                            .map(|(x, y)| (i16::from(y) - i16::from(x)).abs())
                            .into_iter()
                            .max()
                            .unwrap_or(0);
                        assert!(
                            step <= MAX_CHANNEL_STEP,
                            "{name} stepped {a:?} -> {b:?} at {at}"
                        );
                    }
                    prev = next;
                }
            }
        }
    }

    #[test]
    fn weather_ground_tint_differs_by_variant() {
        let clear = weather_ground_tint(Weather::Clear);
        let rain = weather_ground_tint(Weather::Rain);
        let fog = weather_ground_tint(Weather::Fog);
        assert_ne!(clear, rain, "rain biases the ground cooler");
        assert_ne!(clear, fog, "fog desaturates");
        assert!(
            rain.b >= rain.r,
            "rain tint should be cool (blue >= red), got {:?}",
            rain
        );
    }

    #[test]
    fn weather_ground_tint_clear_is_near_neutral() {
        let clear = weather_ground_tint(Weather::Clear);
        assert!(
            clear.r > 200 && clear.g > 200 && clear.b > 200,
            "clear should be a near-white slight-warm tint, got {:?}",
            clear
        );
    }

    #[test]
    fn fog_ground_tint_is_brighter_than_overcast() {
        let fog = weather_ground_tint(Weather::Fog);
        let oc = weather_ground_tint(Weather::Overcast);
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
        let stars =
            |w| SkyTones::resolve(&Sky::at_with(at(2), w), &crate::theme::NORMAL).star_strength;
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
    fn the_ground_is_dimmed_by_the_dark_and_lifted_by_the_sun() {
        let at = crate::localclock::at_hour;
        let wash = |h| {
            SkyTones::resolve(&Sky::at_with(at(h), Weather::Clear), &crate::theme::NORMAL)
                .ground_wash
        };
        let [(_, noon_dim), (tint, noon_lift)] = wash(12);
        let [(dim_tint, night_dim), (_, night_lift)] = wash(0);
        assert_eq!(dim_tint, crate::theme::NORMAL.lighting.night_tint);
        assert_eq!(tint, SUN_TINT);
        assert!(noon_lift > 0.0, "a clear noon lifts the ground");
        assert_eq!(night_lift, 0.0, "the moon lifts nothing");
        assert!(night_dim > noon_dim, "midnight dims the ground past noon");
        assert!(
            night_dim < NIGHT_GROUND_DIM,
            "the dim rides the darkness, which the city's glow keeps short of full"
        );
    }

    #[test]
    fn a_lit_object_takes_the_grounds_daylight_at_its_share() {
        for (h, m) in [(0, 0), (3, 0), (7, 0), (12, 30), (19, 30), (22, 0)] {
            let look = SkyTones::resolve(
                &Sky::at_with(at_hour_min(h, m), Weather::Clear),
                &crate::theme::NORMAL,
            );
            let [_, (_, ground_lift)] = look.ground_wash;
            let [_, (_, object_lift)] = look.object_wash;
            assert_eq!(
                object_lift,
                ground_lift * OBJECT_WASH_SHARE,
                "{h:02}:{m:02}"
            );
        }
    }

    /// The veil keeps the weather reading after dark: dimmer than by day, but
    /// never below [`NIGHT_VEIL_MIN`] of its own colour.
    #[test]
    fn a_veil_dims_after_dark_but_keeps_its_minimum() {
        let at = crate::localclock::at_hour;
        let veil = |h| {
            SkyTones::resolve(&Sky::at_with(at(h), Weather::Fog), &crate::theme::NORMAL)
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
            lum(midnight) >= lum(unlit) * NIGHT_VEIL_MIN - 1.5,
            "{midnight:?} fell below the night minimum of {unlit:?}"
        );
    }
}
