//! The weather on the windows' glass, pixel-free: the veil over the view and
//! the rain or snow running down the panes, placed on a grid of `d`
//! cells to the layout unit, so each painter draws one design at its own
//! density.

use pixtuoid_core::sprite::Rgb;

use crate::atmosphere::Moment;
use crate::layout::Size;
use crate::sky::Weather;

/// One frame's weather on every window: [`GlassWeather::of`] once per frame.
/// Its weather and `tick` are its whole key: two equal keys place equal marks.
#[derive(Clone, Copy)]
pub(crate) struct GlassWeather {
    /// [`SkyTones::glass_veil`](crate::atmosphere::SkyTones::glass_veil).
    pub(crate) veil: Option<(Rgb, f32)>,
    /// Each falling weather's [`Fall`] and how many of its particles show:
    /// its count, scaled by that weather's share.
    falls: [Option<(&'static Fall, u64)>; 2],
    /// The clock the marks move by, in ms.
    tick: u64,
}

/// One weather's falling particles.
#[derive(PartialEq)]
struct Fall {
    count: u64,
    seed_mult: u64,
    sx_mult: u64,
    speed_base: u64,
    speed_span: u64,
    colour: Rgb,
    particle: Particle,
}

#[derive(Clone, Copy, PartialEq)]
enum Particle {
    /// A vertical streak `len_base + seed % len_mod` units long, its alpha
    /// falling from `alpha_base` by `alpha_falloff` over its length; `drift`
    /// leans it one cell east per two down (the wind).
    Streak {
        len_base: u16,
        len_mod: u64,
        alpha_base: f32,
        alpha_falloff: f32,
        drift: bool,
    },
    /// An opaque flake wiggling one unit east and back every [`WIGGLE_MS`].
    Flake,
}

const STREAK_COLOUR: Rgb = Rgb {
    r: 210,
    g: 220,
    b: 240,
};

const RAIN: Fall = Fall {
    count: 4,
    seed_mult: 7,
    sx_mult: crate::GOLDEN_GAMMA_32 as u64,
    speed_base: 60,
    speed_span: 50,
    colour: STREAK_COLOUR,
    particle: Particle::Streak {
        len_base: 3,
        len_mod: 2,
        alpha_base: 0.35,
        alpha_falloff: 0.15,
        drift: false,
    },
};

const STORM: Fall = Fall {
    count: 6,
    speed_base: 40,
    speed_span: 40,
    colour: Rgb {
        r: 210,
        g: 220,
        b: 245,
    },
    particle: Particle::Streak {
        len_base: 4,
        len_mod: 3,
        alpha_base: 0.6,
        alpha_falloff: 0.3,
        drift: false,
    },
    ..RAIN
};

const WINDY: Fall = Fall {
    count: 5,
    speed_base: 50,
    speed_span: 40,
    particle: Particle::Streak {
        len_base: 3,
        len_mod: 2,
        alpha_base: 0.35,
        alpha_falloff: 0.15,
        drift: true,
    },
    ..RAIN
};

const SNOW: Fall = Fall {
    count: 3,
    seed_mult: 11,
    sx_mult: 0x517c_c1b7,
    speed_base: 150,
    speed_span: 100,
    colour: Rgb {
        r: 240,
        g: 240,
        b: 250,
    },
    particle: Particle::Flake,
};

/// How long a flake holds each side of its wiggle.
const WIGGLE_MS: u64 = 400;

/// Spreads each particle's speed over its fall's span.
const SPEED_MULT: u64 = 0x4f6c_dd1d;

fn fall(w: Weather) -> Option<&'static Fall> {
    match w {
        Weather::Rain => Some(&RAIN),
        Weather::Storm => Some(&STORM),
        Weather::Windy => Some(&WINDY),
        Weather::Snow => Some(&SNOW),
        Weather::Clear | Weather::Fog | Weather::Overcast | Weather::Smog => None,
    }
}

/// One cell a particle covers on a painter's grid, from the glass's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mark {
    pub(crate) x: u16,
    pub(crate) y: u16,
    colour: Rgb,
    ink: Ink,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ink {
    Solid,
    /// A streak's cell: `level` of its `peak` alpha, drawn in
    /// [`FALLOFF_TONES`](crate::dither::FALLOFF_TONES) flat tones.
    Fade {
        level: f32,
        peak: f32,
    },
}

impl Mark {
    /// The mark over `under`, at `at` on the painter's grid, which the falloff
    /// dithers by.
    pub(crate) fn over(self, under: Rgb, at: (u16, u16)) -> Rgb {
        match self.ink {
            Ink::Solid => self.colour,
            Ink::Fade { level, peak } => crate::composite::blend_rgb(
                under,
                self.colour,
                crate::dither::stepped(level, peak, crate::dither::FALLOFF_TONES, at.0, at.1),
            ),
        }
    }
}

impl GlassWeather {
    pub(crate) fn of(moment: &Moment) -> Self {
        let mut falls = moment.sky.weather().parts().map(|(w, share)| {
            fall(w).map(|fall| (fall, (fall.count as f32 * share).round() as u64))
        });
        Self {
            veil: moment.look.glass_veil,
            falls: [falls.next().flatten(), falls.next().flatten()],
            tick: moment.beat.ms(),
        }
    }

    /// The marks on the glass of the window `idx`th in the run, `glass` units
    /// big, on a grid `d` cells to the unit. A streak is one cell wide and
    /// `d` cells per unit long, and wraps within the glass; a flake is a
    /// square half a unit across.
    pub(crate) fn marks(&self, idx: u16, glass: Size, d: u16) -> Vec<Mark> {
        let Size { w: gw, h: gh } = glass;
        let mut marks = Vec::new();
        if gw == 0 || gh == 0 || d == 0 {
            return marks;
        }
        let (cols, rows) = (gw * d, gh * d);
        for (fall, i) in self
            .falls
            .iter()
            .flatten()
            .flat_map(|&(fall, count)| (0..count).map(move |i| (fall, i)))
        {
            let seed = u64::from(idx) * fall.seed_mult + i;
            let sx = (seed.wrapping_mul(fall.sx_mult) % u64::from(gw)) as u16;
            let speed = fall.speed_base + seed.wrapping_mul(SPEED_MULT) % fall.speed_span;
            let offset = seed.wrapping_mul(u64::from(crate::MURMUR3_FMIX32_M1)) % u64::from(gh);
            // Below `gh`, a u16.
            let sy = ((self.tick / speed + offset) % u64::from(gh)) as u16;
            let colour = fall.colour;
            match fall.particle {
                Particle::Streak {
                    len_base,
                    len_mod,
                    alpha_base,
                    alpha_falloff,
                    drift,
                } => {
                    let len = (len_base + (seed % len_mod) as u16) * d;
                    marks.extend((0..len).map(|k| Mark {
                        x: (sx * d + d / 2 + if drift { k / 2 } else { 0 }) % cols,
                        y: (sy * d + k) % rows,
                        colour,
                        ink: Ink::Fade {
                            level: alpha_base - (f32::from(k) / f32::from(len)) * alpha_falloff,
                            peak: alpha_base,
                        },
                    }));
                }
                Particle::Flake => {
                    let wiggle = u16::from(!(self.tick / WIGGLE_MS + seed).is_multiple_of(2));
                    let side = (d / 2).max(1);
                    let x0 = (sx + wiggle) % gw * d + (d - side) / 2;
                    let y0 = sy * d + (d - side) / 2;
                    marks.extend((0..side * side).map(|k| Mark {
                        x: x0 + k % side,
                        y: y0 + k / side,
                        colour,
                        ink: Ink::Solid,
                    }));
                }
            }
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localclock::at_hour;
    use crate::sky::{Sky, TRANSITION_STEPS, WeatherMix};

    fn weather_at(w: Weather, tick_ms: u64) -> GlassWeather {
        let now = at_hour(12) + std::time::Duration::from_millis(tick_ms);
        GlassWeather::of(&Moment::resolve(
            Sky::at_with(now, w),
            &crate::theme::NORMAL,
            0.0,
            crate::anim::Motion::Full.timing(now),
        ))
    }

    #[test]
    fn every_mark_lies_on_the_glass_at_every_density() {
        let glass = Size { w: 20, h: 13 };
        for w in Weather::ALL {
            for tick in [0, 37, 1234, 99_999] {
                for d in [1, 2, 3, 4] {
                    for m in weather_at(w, tick).marks(3, glass, d) {
                        assert!(
                            m.x < glass.w * d && m.y < glass.h * d,
                            "{w:?} {tick} {d}: {m:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn only_falling_weather_marks_the_glass() {
        let glass = Size { w: 20, h: 13 };
        for w in Weather::ALL {
            let falls = matches!(
                w,
                Weather::Rain | Weather::Storm | Weather::Windy | Weather::Snow
            );
            assert_eq!(
                !weather_at(w, 0).marks(0, glass, 4).is_empty(),
                falls,
                "{w:?}"
            );
        }
    }

    /// The tick moves the marks, and is all that does within one weather.
    #[test]
    fn equal_keys_place_equal_marks_and_the_tick_moves_them() {
        let glass = Size { w: 20, h: 13 };
        for w in [Weather::Rain, Weather::Snow] {
            let marks = |tick| weather_at(w, tick).marks(1, glass, 4);
            assert_eq!(marks(5_000), marks(5_000), "{w:?}");
            assert_ne!(marks(5_000), marks(6_000), "{w:?}");
        }
    }

    /// A streak is one cell wide at every density, so it stays a line, never
    /// a bar a unit wide.
    #[test]
    fn a_streak_is_one_cell_wide_and_a_unit_per_unit_long() {
        let glass = Size { w: 20, h: 13 };
        let marks = weather_at(Weather::Rain, 0).marks(0, glass, 4);
        let one = weather_at(Weather::Rain, 0).marks(0, glass, 1);
        assert_eq!(marks.len(), one.len() * 4);
        let columns: std::collections::BTreeSet<u16> = marks.iter().map(|m| m.x).collect();
        assert!(columns.len() <= usize::try_from(RAIN.count).expect("small"));
    }

    /// Mid-change, each falling weather shows its share of its particles:
    /// rain's thin out as a storm's come in, and either end is one weather's
    /// alone.
    #[test]
    fn a_change_trades_one_falls_particles_for_the_others() {
        let now = at_hour(12);
        let counts = |step| {
            let sky = Sky::at_with(now, Weather::Rain).with_weather(WeatherMix::stepped(
                Weather::Rain,
                Weather::Storm,
                step,
            ));
            let glass = GlassWeather::of(&Moment::resolve(
                sky,
                &crate::theme::NORMAL,
                0.0,
                crate::anim::Motion::Full.timing(now),
            ));
            let count = |of: &Fall| {
                glass
                    .falls
                    .iter()
                    .flatten()
                    .find(|(fall, _)| *fall == of)
                    .map_or(0, |&(_, n)| n)
            };
            (count(&RAIN), count(&STORM))
        };
        assert_eq!(counts(0), (RAIN.count, 0));
        assert_eq!(counts(TRANSITION_STEPS + 1), (0, STORM.count));
        let mid = counts(TRANSITION_STEPS / 2);
        assert!(mid.0 > 0 && mid.1 > 0, "{mid:?}");
        for step in 0..=TRANSITION_STEPS {
            let ((rain, storm), (next_rain, next_storm)) = (counts(step), counts(step + 1));
            assert!(next_rain <= rain && next_storm >= storm, "step {step}");
        }
    }

    /// No two weathers fall alike, so finding a fall by value names one
    /// weather's: `&CONST` addresses are not unique, so identity can't.
    #[test]
    fn every_falling_weather_has_its_own_fall() {
        let falls: Vec<(Weather, &Fall)> = Weather::ALL
            .into_iter()
            .filter_map(|w| fall(w).map(|f| (w, f)))
            .collect();
        for (i, (a, fa)) in falls.iter().enumerate() {
            for (b, fb) in &falls[i + 1..] {
                assert!(fa != fb, "{a:?} and {b:?} share one fall");
            }
        }
    }

    #[test]
    fn a_streak_steps_down_through_the_falloff_tones() {
        let Particle::Streak { alpha_base, .. } = STORM.particle else {
            panic!("storm streaks");
        };
        let black = Rgb { r: 0, g: 0, b: 0 };
        let tones: Vec<Rgb> = (1..=crate::dither::FALLOFF_TONES)
            .map(|k| {
                let alpha = alpha_base * f32::from(k) / f32::from(crate::dither::FALLOFF_TONES);
                crate::composite::blend_rgb(black, STORM.colour, alpha)
            })
            .chain([black])
            .collect();
        let marks = weather_at(Weather::Storm, 12_345).marks(0, Size { w: 18, h: 28 }, 1);
        assert!(!marks.is_empty());
        for m in marks {
            let c = m.over(black, (m.x, m.y));
            assert!(tones.contains(&c), "{c:?} vs {tones:?}");
        }
    }
}
