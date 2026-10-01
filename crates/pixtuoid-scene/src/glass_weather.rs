//! The weather on the windows' glass, pixel-free: the veil over the view and
//! the rain or snow running down the panes, placed on a grid of `d`
//! cells to the layout unit, so each painter draws one design at its own
//! density.

use pixtuoid_core::sprite::Rgb;

use crate::atmosphere::Moment;
use crate::dither::Dithered;
use crate::layout::Size;
use crate::sky::{Element, Weather, WeatherPolicy};

/// One frame's weather on every window: [`GlassWeather::of`] once per frame.
/// Its policy and `tick` are its whole key: two equal keys place equal marks.
#[derive(Clone, Copy)]
pub(crate) struct GlassWeather {
    /// [`SkyTones::glass_veil`](crate::atmosphere::SkyTones::glass_veil).
    pub(crate) veil: Dithered<Option<(Rgb, f32)>>,
    /// The weather at any instant: a particle shows by its weather's share
    /// when its fall began.
    policy: WeatherPolicy,
    /// The clock the marks move by, in ms.
    tick: u64,
}

/// One weather's falling particles.
struct Fall {
    count: u64,
    seed_mult: u64,
    sx_mult: u64,
    speed_base: u64,
    speed_span: u64,
    colour: Rgb,
    particle: Particle,
}

#[derive(Clone, Copy)]
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

/// One particle's way down the glass, at a tick.
struct Descent {
    seed: u64,
    /// Its column, in units.
    sx: u16,
    /// Its row, in units.
    sy: u16,
    /// The tick, in ms, its current fall down the glass began.
    began: u64,
}

impl Descent {
    /// The `i`th particle of `fall` on the glass of the window `idx`th in the
    /// run, `glass` units big, at `tick`.
    fn of(fall: &Fall, idx: u16, i: u64, glass: Size, tick: u64) -> Self {
        let seed = u64::from(idx) * fall.seed_mult + i;
        let gh = u64::from(glass.h);
        let speed = fall.speed_base + seed.wrapping_mul(SPEED_MULT) % fall.speed_span;
        let offset = seed.wrapping_mul(u64::from(crate::MURMUR3_FMIX32_M1)) % gh;
        let fallen = tick / speed + offset;
        let top = fallen - fallen % gh;
        Self {
            seed,
            sx: (seed.wrapping_mul(fall.sx_mult) % u64::from(glass.w)) as u16,
            // Below `gh`, a u16.
            sy: (fallen % gh) as u16,
            began: top.saturating_sub(offset) * speed,
        }
    }
}

/// The precipitation share at which the `i`th of `count` particles falls: the
/// middle of its even slice of the share, so a share shows its own count.
fn threshold(i: u64, count: u64) -> f32 {
    (i as f32 + 0.5) / count as f32
}

impl GlassWeather {
    pub(crate) fn of(moment: &Moment) -> Self {
        Self {
            veil: moment.look.glass_veil,
            policy: moment.sky.policy(),
            tick: crate::anim::epoch_ms(moment.now),
        }
    }

    /// Whether `descent`, the `i`th of `w`'s `count`, falls: decided where its
    /// fall began, so it never appears or vanishes partway down.
    fn shows(&self, w: Weather, i: u64, count: u64, descent: &Descent) -> bool {
        self.policy
            .weather_at_ms(descent.began)
            .share(Element::Precipitation, w)
            >= threshold(i, count)
    }

    /// Every weather whose particles may be falling on glass `gh` units tall:
    /// the weather now's, and the weather's when the slowest fall now on the
    /// glass began.
    fn falling(&self, gh: u16) -> Vec<Weather> {
        let slowest = Weather::ALL
            .into_iter()
            .filter_map(fall)
            .map(|f| (f.speed_base + f.speed_span) * u64::from(gh))
            .max()
            .unwrap_or(0);
        let mut weathers = Vec::new();
        for w in [self.tick, self.tick.saturating_sub(slowest)]
            .into_iter()
            .flat_map(|ms| self.policy.weather_at_ms(ms).ends())
        {
            if !weathers.contains(&w) {
                weathers.push(w);
            }
        }
        weathers
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
        for w in self.falling(gh) {
            let Some(fall) = fall(w) else {
                continue;
            };
            for i in 0..fall.count {
                let descent = Descent::of(fall, idx, i, glass, self.tick);
                if !self.shows(w, i, fall.count, &descent) {
                    continue;
                }
                let Descent { seed, sx, sy, .. } = descent;
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
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localclock::at_hour;
    use crate::sky::Sky;

    fn weather_at(w: Weather, tick_ms: u64) -> GlassWeather {
        let now = at_hour(12) + std::time::Duration::from_millis(tick_ms);
        GlassWeather::of(&Moment::resolve(
            Sky::at_with(now, w),
            &crate::theme::NORMAL,
            0.0,
            now,
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

    /// The weathers that fall are the sky's: [`Weather::falls`] and [`fall`]
    /// name one set.
    #[test]
    fn the_sky_and_the_glass_agree_on_what_falls() {
        for w in Weather::ALL {
            assert_eq!(fall(w).is_some(), w.falls(), "{w:?}");
        }
    }

    /// Where each particle's fall began, and whether it shows, under the
    /// clock at `tick` on window `idx`'s `glass`.
    fn particles(tick: u64, idx: u16, glass: Size) -> Vec<((Weather, u64), u64, bool)> {
        let gw = GlassWeather {
            veil: Dithered::solid(None),
            policy: WeatherPolicy::Clock,
            tick,
        };
        let falling = gw.falling(glass.h);
        let mut out = Vec::new();
        for w in Weather::ALL {
            let Some(f) = fall(w) else {
                continue;
            };
            for i in 0..f.count {
                let descent = Descent::of(f, idx, i, glass, tick);
                let shows = falling.contains(&w) && gw.shows(w, i, f.count, &descent);
                out.push(((w, i), descent.began, shows));
            }
        }
        out
    }

    /// The start, in ms, of each clock transition that changes what falls,
    /// with its two weathers.
    fn falling_changes() -> impl Iterator<Item = (u64, Weather, Weather)> {
        let slot_ms = crate::sky::WEATHER_CYCLE_SECS * 1000;
        (0..).filter_map(move |slot: u64| {
            let start = (slot + 1) * slot_ms - crate::sky::TRANSITION_MS;
            let [here, next] = WeatherPolicy::Clock.weather_at_ms(start + 1).ends();
            (here != next && (here.falls() || next.falls())).then_some((start, here, next))
        })
    }

    /// A particle shows or hides for a whole fall: none appears or vanishes
    /// partway down, through changes of every kind and the slot boundaries
    /// after them.
    #[test]
    fn no_particle_appears_or_vanishes_mid_fall() {
        const CHANGES: usize = 16;
        const STEP_MS: usize = 37;
        const MARGIN_MS: u64 = 15_000;
        let glass = Size { w: 20, h: 28 };
        let mut kinds = std::collections::HashSet::new();
        for (start, here, next) in falling_changes().take(CHANGES) {
            kinds.insert((here, next));
            let end = start + crate::sky::TRANSITION_MS + MARGIN_MS;
            for idx in 0..3 {
                let mut prev = particles(start - MARGIN_MS, idx, glass);
                for tick in (start - MARGIN_MS..end).step_by(STEP_MS).skip(1) {
                    let now = particles(tick, idx, glass);
                    for ((id, began, was), (_, began_now, is)) in prev.iter().zip(&now) {
                        assert!(
                            began != began_now || was == is,
                            "{here:?} -> {next:?}: {id:?} on window {idx} \
                             {} mid-fall at {tick}",
                            if *is { "appeared" } else { "vanished" }
                        );
                    }
                    prev = now;
                }
            }
        }
        assert!(kinds.len() > 4, "too few kinds of change: {kinds:?}");
    }

    /// Across a rain-to-storm change the storm's particles take over from the
    /// rain's: all rain as it opens, all storm as the next slot does, both
    /// between.
    #[test]
    fn a_change_trades_one_falls_particles_for_the_others() {
        let glass = Size { w: 20, h: 28 };
        let (start, ..) = falling_changes()
            .find(|&(_, here, next)| here == Weather::Rain && next == Weather::Storm)
            .expect("the clock turns rain to storm");
        let counts = |tick| {
            let shown = |w| {
                (0..8)
                    .flat_map(|idx| particles(tick, idx, glass))
                    .filter(|&((pw, _), _, shows)| pw == w && shows)
                    .count()
            };
            (shown(Weather::Rain), shown(Weather::Storm))
        };
        let windows = 8 * usize::try_from(RAIN.count).expect("small");
        let storms = 8 * usize::try_from(STORM.count).expect("small");
        assert_eq!(counts(start), (windows, 0));
        assert_eq!(counts(start + crate::sky::TRANSITION_MS), (0, storms));
        let mid = counts(start + crate::sky::TRANSITION_MS / 3);
        assert!(mid.0 > 0 && mid.1 > 0, "{mid:?}");
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
