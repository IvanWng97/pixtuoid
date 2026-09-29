//! The city seen through the office's windows, pixel-free: which buildings stand
//! where along the window run, far to near, which of their windows burn, and each
//! depth's colours under the sky. The classic painter draws it at 1x and the
//! cutaway on its art grid, so both look out on one city.
//!
//! Everything is laid out in logical units from the run's west end and the
//! glass's top, and every choice is a hash of where it is, so a wider window
//! shows more of the same city rather than another one.

use std::ops::RangeInclusive;
use std::time::SystemTime;

use pixtuoid_core::sprite::format::{Building, CityPlane, Material, Pack};
use pixtuoid_core::sprite::Rgb;

use crate::atmosphere::Look;
use crate::layout::pct;
use crate::theme::Theme;

/// A depth of the city.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Plane {
    /// Plain blocks on the horizon: the painter's own, behind every building a
    /// pack draws.
    Far,
    /// The pack's middle distance.
    Mid,
    /// The pack's nearest buildings, their feet sunk below the sill.
    Near,
}

impl Plane {
    /// Far to near: the order the planes are drawn in.
    pub(crate) const ALL: [Plane; 3] = [Plane::Far, Plane::Mid, Plane::Near];

    /// Its place in [`Plane::ALL`].
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    fn depth(self) -> &'static Depth {
        match self {
            Plane::Far => &FAR,
            Plane::Mid => &MID,
            Plane::Near => &NEAR,
        }
    }

    /// The pack's plane of the same depth; the far plane is the painter's own.
    fn pack_plane(self) -> Option<CityPlane> {
        match self {
            Plane::Far => None,
            Plane::Mid => Some(CityPlane::Mid),
            Plane::Near => Some(CityPlane::Near),
        }
    }
}

/// How a depth looks: distance reads as value, the far plane lifted and hazed
/// toward the horizon, the near one darkest.
struct Depth {
    /// Tells this plane's hashes apart from the others'.
    salt: u32,
    /// How many ramp stops its facades sit over the hour's building tone.
    lift: i8,
    /// How far everything in it fades toward the horizon: aerial perspective.
    haze: f32,
    /// The percent of its windows lit at full dark.
    lit_night_pct: u32,
    /// How far its buildings' feet sink below the sill, in percent of the
    /// glass: the nearer, the more of each the sill hides.
    sunk_pct: u16,
    /// The gap after each of its buildings, in logical units: the nearer, the
    /// wider, so the planes behind show between its towers.
    gap: RangeInclusive<u16>,
}

const FAR: Depth = Depth {
    salt: 3,
    lift: 2,
    haze: 0.5,
    lit_night_pct: 15,
    sunk_pct: 0,
    gap: 0..=1,
};
const MID: Depth = Depth {
    salt: 41,
    lift: 1,
    haze: 0.25,
    lit_night_pct: 25,
    sunk_pct: 8,
    gap: 0..=2,
};
const NEAR: Depth = Depth {
    salt: 97,
    lift: 0,
    haze: 0.0,
    lit_night_pct: 30,
    sunk_pct: 20,
    gap: 2..=7,
};

/// A far block's width, in logical units.
const FAR_BLOCK_W: RangeInclusive<u16> = 2..=5;
/// A far block's height, in percent of the glass.
const FAR_BLOCK_H_PCT: RangeInclusive<u16> = 18..=45;
/// How far the city sinks below the sill at the top floor, in percent of the
/// glass: the higher the office, the lower the city.
const ALTITUDE_SINK_PCT: u16 = 30;

/// The percent of windows lit at noon: some offices keep their lights on by day.
const LIT_DAY_PCT: u32 = 4;
/// How strongly a lit window glows at noon, where daylight washes it out.
const LIT_DAY_GLOW: f32 = 0.25;
/// A lit window's turn, in milliseconds: on each, it may go dark for the whole
/// turn.
const BLINK_CYCLE_MS: RangeInclusive<u64> = 6_000..=14_000;
/// One lit window in this many goes dark on a given turn.
const BLINK_OFF_IN: u32 = 12;

/// One building standing in a plane, its top-left in logical units from the
/// run's west end and the glass's top.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stand<'p> {
    /// A building the pack draws.
    Kit {
        building: &'p Building,
        x: i32,
        top: i32,
        hash: u32,
    },
    /// A plain block the painter draws, its windows on every other row and
    /// column.
    Block { x: i32, w: u16, top: i32, hash: u32 },
}

impl Stand<'_> {
    /// The hash every choice about it keys off.
    pub(crate) fn hash(&self) -> u32 {
        match *self {
            Stand::Kit { hash, .. } | Stand::Block { hash, .. } => hash,
        }
    }
}

/// A window of a [`Stand::Block`] at `(x, y)` from its top-left: every other
/// row and column, the lit-dot grid a city's windows are read by.
pub(crate) fn block_window(x: u16, y: u16) -> bool {
    x % 2 == 1 && y % 2 == 1
}

/// The city behind one window run.
pub(crate) struct Skyline<'p> {
    stands: Vec<(Plane, Stand<'p>)>,
}

impl<'p> Skyline<'p> {
    /// `pack`'s city across a run of glass `run_w` wide and `glass_h` tall, seen
    /// from `altitude` (0 at the ground floor, 1 at the top).
    pub(crate) fn of(pack: &'p Pack, run_w: u16, glass_h: u16, altitude: f32) -> Self {
        let sink = f32::from(pct(glass_h, ALTITUDE_SINK_PCT)) * altitude.clamp(0.0, 1.0);
        let mut stands = Vec::new();
        for plane in Plane::ALL {
            let depth = plane.depth();
            let sill = i32::from(glass_h) + i32::from(pct(glass_h, depth.sunk_pct)) + sink as i32;
            let kit: Vec<&Building> = plane.pack_plane().map_or_else(Vec::new, |p| {
                pack.buildings().filter(|b| b.stands_in(p)).collect()
            });
            let mut x = -((hash(depth.salt) % 4) as i32);
            let (mut slot, mut last) = (0u32, None);
            while x < i32::from(run_w) {
                let n = hash(depth.salt.wrapping_add(slot.wrapping_mul(131)));
                let stand = if kit.is_empty() {
                    let w = pick_in(&FAR_BLOCK_W, n);
                    let h = pct(glass_h, pick_in(&FAR_BLOCK_H_PCT, n ^ 0x5A5A));
                    Stand::Block {
                        x,
                        w,
                        top: sill - i32::from(h),
                        hash: n,
                    }
                } else {
                    let mut pick = n as usize % kit.len();
                    if kit.len() > 1 && last == Some(pick) {
                        pick = (pick + 1) % kit.len();
                    }
                    last = Some(pick);
                    let building = kit[pick];
                    Stand::Kit {
                        building,
                        x,
                        top: sill - i32::from(building.size().1),
                        hash: n,
                    }
                };
                let w = match stand {
                    Stand::Kit { building, .. } => building.size().0,
                    Stand::Block { w, .. } => w,
                };
                stands.push((plane, stand));
                x += i32::from(w) + i32::from(pick_in(&depth.gap, n ^ 0x7777));
                slot += 1;
            }
        }
        Self { stands }
    }

    /// Every building, far to near.
    pub(crate) fn stands(&self) -> impl Iterator<Item = (Plane, Stand<'p>)> + '_ {
        self.stands.iter().copied()
    }
}

/// Which of the theme's lit-window hues window `index` of a stand hashed
/// `hash` burns in, or `None` while it is dark. The share of lit windows rises
/// from [`LIT_DAY_PCT`] at noon to the plane's own by night, each window
/// keeping its own place in that order, so lights come on one by one as it
/// darkens; a lit window now and then goes dark for a turn.
pub(crate) fn lit(
    plane: Plane,
    hash: u32,
    index: usize,
    darkness: f32,
    now: SystemTime,
) -> Option<usize> {
    let depth = plane.depth();
    let r = self::hash(hash ^ depth.salt ^ (index as u32).wrapping_mul(0x2545_F491));
    let night = depth.lit_night_pct.saturating_sub(LIT_DAY_PCT) as f32;
    let share = LIT_DAY_PCT as f32 + night * darkness.clamp(0.0, 1.0);
    if (r % 1000) as f32 >= share * 10.0 {
        return None;
    }
    let ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let span = BLINK_CYCLE_MS.end() - BLINK_CYCLE_MS.start() + 1;
    let turn = ms / (BLINK_CYCLE_MS.start() + u64::from(r) % span);
    if self::hash(r ^ turn as u32).is_multiple_of(BLINK_OFF_IN) {
        return None;
    }
    Some((r / 1000 % 3) as usize)
}

/// A plane's colour for each [`Material`] under this hour's sky, its lit
/// windows' hues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlaneColours {
    facade: Rgb,
    shade: Rgb,
    roof: Rgb,
    glass: Rgb,
    mullion: Rgb,
    detail: Rgb,
    sign: Rgb,
    /// A lit window, in each of the theme's three hues.
    pub(crate) lit: [Rgb; 3],
}

impl PlaneColours {
    /// `plane`'s colours: the hour's building tone lifted by the plane's depth
    /// and hazed toward the horizon.
    pub(crate) fn of(plane: Plane, look: &Look, theme: &Theme) -> Self {
        let depth = plane.depth();
        let o = &theme.office;
        let haze = |c: Rgb| c.mix(look.glass_a, depth.haze);
        let tone = o
            .building_light
            .mix(o.building_dark, look.darkness)
            .ramp(depth.lift);
        let glass = o.city_dark_window.mix(tone, 0.5);
        let glow = look.darkness.max(LIT_DAY_GLOW);
        PlaneColours {
            facade: haze(tone),
            shade: haze(tone.ramp(-1)),
            roof: haze(tone.ramp(1)),
            glass: haze(glass),
            mullion: haze(tone.ramp(-2)),
            detail: haze(tone.ramp(-1)),
            sign: haze(tone.mix(theme.lighting.twilight_a, look.darkness)),
            lit: o.city_lit_windows.map(|c| haze(glass.mix(c, glow))),
        }
    }

    /// The colour `material` is drawn in.
    pub(crate) fn material(&self, material: Material) -> Rgb {
        match material {
            Material::Facade => self.facade,
            Material::Shade => self.shade,
            Material::Roof => self.roof,
            Material::Glass => self.glass,
            Material::Mullion => self.mullion,
            Material::Detail => self.detail,
            Material::Sign => self.sign,
        }
    }
}

/// A value in `range` chosen by `n`.
fn pick_in(range: &RangeInclusive<u16>, n: u32) -> u16 {
    let span = u32::from(range.end() - range.start()) + 1;
    range.start() + (hash(n) % span) as u16
}

/// A deterministic hash for placing the city: the same office always gets the
/// same skyline, where a per-frame reshuffle would flicker.
fn hash(n: u32) -> u32 {
    let mut v = n.wrapping_mul(0x9E37_79B9);
    v ^= v >> 15;
    v = v.wrapping_mul(0x85EB_CA6B);
    v ^ (v >> 13)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Pack {
        crate::embedded_pack::load_sprite_pack(crate::embedded_pack::PackSource::Bundled)
            .expect("the embedded pack loads")
    }

    /// Where each stand stands, for comparing two cities.
    fn places(city: &Skyline<'_>) -> Vec<(Plane, i32, i32)> {
        city.stands()
            .map(|(p, s)| match s {
                Stand::Kit { x, top, .. } | Stand::Block { x, top, .. } => (p, x, top),
            })
            .collect()
    }

    #[test]
    fn the_city_is_drawn_far_to_near_across_the_whole_run() {
        let pack = pack();
        let city = Skyline::of(&pack, 120, 20, 0.0);
        let planes: Vec<_> = city.stands().map(|(p, _)| p).collect();
        assert!(planes.windows(2).all(|w| w[0] <= w[1]), "far to near");
        for plane in Plane::ALL {
            let reach = city
                .stands()
                .filter(|&(p, _)| p == plane)
                .map(|(_, s)| match s {
                    Stand::Kit { building, x, .. } => x + i32::from(building.size().0),
                    Stand::Block { x, w, .. } => x + i32::from(w),
                })
                .max()
                .expect("every plane stands something");
            let gap = i32::from(*plane.depth().gap.end());
            assert!(
                reach + gap >= 120,
                "{plane:?} reaches the run's east end, but for a gap"
            );
        }
    }

    #[test]
    fn a_wider_run_shows_more_of_the_same_city() {
        let pack = pack();
        let narrow = places(&Skyline::of(&pack, 60, 20, 0.0));
        let wide = places(&Skyline::of(&pack, 140, 20, 0.0));
        for stand in &narrow {
            assert!(wide.contains(stand), "{stand:?} stays put");
        }
    }

    #[test]
    fn the_pack_stands_only_in_the_planes_its_buildings_name() {
        let pack = pack();
        for (plane, stand) in Skyline::of(&pack, 200, 20, 0.0).stands() {
            match (plane, stand) {
                (Plane::Far, Stand::Block { .. }) => {}
                (_, Stand::Kit { building, .. }) => {
                    let p = plane.pack_plane().expect("a pack plane");
                    assert!(building.stands_in(p), "{} in {plane:?}", building.name());
                }
                (p, s) => panic!("{p:?} stands {s:?}"),
            }
        }
    }

    #[test]
    fn a_higher_office_sees_the_city_lower() {
        let pack = pack();
        let ground = places(&Skyline::of(&pack, 80, 20, 0.0));
        let top = places(&Skyline::of(&pack, 80, 20, 1.0));
        assert!(ground
            .iter()
            .zip(&top)
            .all(|(g, t)| t.2 > g.2 && (t.0, t.1) == (g.0, g.1)));
    }

    #[test]
    fn more_windows_burn_as_it_darkens() {
        let now = SystemTime::UNIX_EPOCH;
        let count = |d: f32| {
            (0..2000)
                .filter(|&i| lit(Plane::Near, 7, i, d, now).is_some())
                .count()
        };
        let (noon, dusk, night) = (count(0.0), count(0.5), count(1.0));
        assert!(noon < dusk && dusk < night, "{noon} < {dusk} < {night}");
        assert!(noon > 0, "some lights are on by day");
        assert!(night < 2000 / 2, "most windows stay dark");
        for i in 0..2000 {
            if lit(Plane::Near, 7, i, 0.3, now).is_some() {
                assert!(
                    lit(Plane::Near, 7, i, 0.9, now).is_some(),
                    "a light that is on stays on as it darkens"
                );
            }
        }
    }
}
