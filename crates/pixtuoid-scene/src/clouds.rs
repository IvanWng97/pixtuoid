//! The clouds the windows show: masses of lobes over a flat base, a deck of
//! them far to near, lit by the sky body, drifting on the beat, and a storm's
//! bolt dropping from a base. They draw on a window's glass between the sky
//! and the city ([`Outside::through`](crate::outside::Outside::through)), so
//! the city stands in front and the joinery is never theirs.
//!
//! Every length is in layout units: x from the window run's west end, y from
//! the glass's top.

use pixtuoid_core::sprite::Rgb;

use crate::atmosphere::Moment;
use crate::outside::{Cell, WindowView};
use crate::sky::Weather;

/// A deterministic stream of `0..1` draws.
struct Rng(u64);

impl Rng {
    fn u(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(crate::GOLDEN_GAMMA);
        (pixtuoid_core::id::splitmix64(self.0) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn between(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.u()
    }
}

/// `0..1` value noise along `x`, one lattice point per unit, smoothstepped.
fn noise(seed: u64, x: f32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let f = f * f * (3.0 - 2.0 * f);
    let at = |k: f32| {
        (pixtuoid_core::id::splitmix64(seed ^ (k as i64 as u64).wrapping_mul(crate::GOLDEN_GAMMA))
            % 1000) as f32
            / 1000.0
    };
    at(i) + (at(i + 1.0) - at(i)) * f
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Lobe {
    x: f32,
    y: f32,
    r: f32,
}

/// Which band of its ramp a cloud cell is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Band {
    Lit,
    Body,
    Shade,
}

/// How deep in the deck a mass hangs: a far one leans most to the sky behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Far,
    Mid,
    Near,
}

impl Layer {
    /// How much a mass in it leans to its row's sky.
    fn sky_mix(self) -> f32 {
        match self {
            Layer::Far => 0.55,
            Layer::Mid => 0.25,
            Layer::Near => 0.0,
        }
    }

    /// Its drift, in units per second of loop time: the far deck slowest.
    fn drift(self) -> f32 {
        match self {
            Layer::Far => 0.05,
            Layer::Mid => 0.08,
            Layer::Near => 0.12,
        }
    }
}

/// One mass: its lobes over a flat base, at a weather's full share.
#[derive(Debug, Clone, PartialEq)]
struct Mass {
    layer: Layer,
    lobes: Vec<Lobe>,
    base: f32,
    /// The weather it belongs to, whose tones it takes.
    weather: Weather,
    /// Its seed for the base's waver.
    seed: u64,
    /// Its place in its weather's deck.
    id: usize,
    /// Its west reach at full share, undrifted: what its own noise keys on,
    /// so its waver and shading ride with it as it drifts.
    anchor: f32,
    /// How far east it has drifted this frame, in units.
    off: f32,
}

impl Mass {
    /// Its west and east reach, in units.
    fn reach(&self) -> (f32, f32) {
        self.lobes.iter().fold((f32::MAX, f32::MIN), |(w, e), l| {
            (w.min(l.x - l.r), e.max(l.x + l.r))
        })
    }

    /// Its top row, in units.
    fn top(&self) -> f32 {
        self.lobes
            .iter()
            .map(|l| l.y - l.r)
            .fold(f32::MAX, f32::min)
    }

    /// The flat base at `x`, wavering a little by column.
    fn base_at(&self, x: f32) -> f32 {
        let local = (x - self.anchor) / BASE_WAVER_PERIOD;
        self.base + BASE_WAVER * noise(self.seed, local) - BASE_WAVER / 2.0
    }

    /// The mass `share` of the way grown from its base, drifted `off` east.
    fn grown(&self, share: f32, off: f32) -> Mass {
        let s = share.clamp(0.0, 1.0);
        Mass {
            lobes: self
                .lobes
                .iter()
                .map(|l| Lobe {
                    x: l.x,
                    y: self.base - (self.base - l.y) * s,
                    r: l.r * s,
                })
                .collect(),
            off,
            ..self.clone()
        }
    }

    /// Whether any lobe has grown in at all.
    fn shows(&self) -> bool {
        self.lobes.iter().any(|l| l.r > 0.0)
    }

    /// In a lobe, in a skirt under one, or in a short crease between two in
    /// one column; never under the base.
    fn solid(&self, x: f32, y: f32) -> bool {
        if y > self.base_at(x) {
            return false;
        }
        let lobes = &self.lobes;
        if lobes
            .iter()
            .any(|l| (x - l.x).powi(2) + (y - l.y).powi(2) <= l.r * l.r)
        {
            return true;
        }
        // a skirt down to the base, narrowing slowly: the base is flat, its
        // ends slant (no bowl, no wall)
        if lobes
            .iter()
            .any(|l| y >= l.y && (x - l.x).abs() <= l.r * (1.0 - SKIRT * (y - l.y) / l.r))
        {
            return true;
        }
        let (mut above, mut below) = (false, false);
        for l in lobes {
            if (x - l.x).abs() < l.r {
                let half = (l.r * l.r - (x - l.x).powi(2)).sqrt();
                above |= y - CREASE < l.y + half && l.y + half < y;
                below |= y < l.y - half && l.y - half < y + CREASE;
            }
        }
        above && below
    }

    /// Solid, with any slit narrower than [`SLIT`] closed: a hairline gap
    /// through a mass reads as a wisp of smoke.
    fn inside(&self, x: f32, y: f32) -> bool {
        self.solid(x, y) || (self.solid(x - SLIT, y) && self.solid(x + SLIT, y))
    }

    /// The mass as a surface: each lobe a dome, joined by a soft max, so the
    /// creases between lobes are valleys, not cracks.
    fn height(&self, x: f32, y: f32) -> f32 {
        let acc: f32 = self
            .lobes
            .iter()
            .filter_map(|l| {
                let h = l.r * l.r - (x - l.x).powi(2) - (y - l.y).powi(2);
                (h > 0.0).then(|| {
                    let e = DOME_EASE * l.r;
                    (SMOOTH * ((h + e * e).sqrt() - e)).exp()
                })
            })
            .sum();
        if acc > 0.0 { acc.ln() / SMOOTH } else { 0.0 }
    }
}

const BASE_WAVER: f32 = 1.2;
const BASE_WAVER_PERIOD: f32 = 5.0;
/// How fast a skirt narrows per lobe radius down.
const SKIRT: f32 = 0.3;
/// The longest gap between two lobes in one column that fills as a crease.
const CREASE: f32 = 1.5;
/// Within one mass, a gap this narrow is a notch, not sky.
const SLIT: f32 = 2.0;
/// A lobe this big carries smaller bumps on its upper rim.
const BUMP_FROM: f32 = 3.5;
/// A dome eased at its rim, as a share of its radius: a finite slope where a
/// bump meets its lobe.
const DOME_EASE: f32 = 0.3;
/// The soft max's sharpness: lower melts lobes together.
const SMOOTH: f32 = 3.0;
/// The surface normal's depth: lower rounds the lobes.
const RELIEF: f32 = 0.5;
/// The closing's reach: a notch, tooth or slit narrower fills.
const CLOSE: f32 = 1.0;

/// A heap: one or two big top lobes, smaller side lobes, a filled base.
fn cumulus(r: &mut Rng, cx: f32, base: f32, width: f32, height: f32) -> Vec<Lobe> {
    let mut lobes = Vec::new();
    for _ in 0..1 + usize::from(r.u() < 0.5) {
        let rad = width * r.between(0.26, 0.36);
        lobes.push(Lobe {
            x: cx + width * r.between(-0.18, 0.18),
            y: base - height + rad * 0.9,
            r: rad,
        });
    }
    for side in [-1.0, 1.0] {
        for _ in 0..1 + (r.u() * 2.0) as usize {
            let rad = width * r.between(0.14, 0.24);
            let x = cx + side * width * r.between(0.22, 0.46);
            // down past the base: the base cuts it flat
            lobes.push(Lobe {
                x,
                y: base - rad * r.between(0.4, 0.8),
                r: rad,
            });
        }
    }
    let n = ((width / 3.0) as usize).max(2);
    for i in 0..n {
        let rad = width * r.between(0.12, 0.2);
        let x = cx - width * 0.45 + width * 0.9 * (i as f32 + r.u() * 0.6) / n as f32;
        lobes.push(Lobe {
            x,
            y: base - rad * 0.6,
            r: rad,
        });
    }
    lobes
}

/// A cumulonimbus: a broad heap, a column narrowing as it climbs, and an anvil
/// spread flat across its top at `anvil_y`.
fn tower(r: &mut Rng, cx: f32, base: f32, width: f32, glass_h: f32) -> Vec<Lobe> {
    let mut lobes = cumulus(r, cx, base, width, glass_h * 0.22);
    let anvil_y = glass_h * 0.10;
    let mut y = base - glass_h * 0.22;
    let mut climb = 0.0;
    while y > anvil_y + 1.5 {
        let taper = 1.0 - 0.45 * climb / (base - anvil_y).max(1.0);
        // a column as broad as the heap: a thin one reads as smoke
        let rad = width * taper * r.between(0.28, 0.36);
        let side = r.between(-0.12, 0.12) * width * taper;
        lobes.push(Lobe {
            x: cx + side,
            y,
            r: rad,
        });
        let step = rad * r.between(0.7, 1.1);
        y -= step;
        climb += step;
    }
    for i in 0..6 {
        // the anvil: flat on top, ragged underneath
        let rad = width * r.between(0.12, 0.2);
        lobes.push(Lobe {
            x: cx + width * (-0.95 + 1.9 * (i as f32 + r.u()) / 6.0),
            y: anvil_y + r.between(-0.4, 0.6),
            r: rad,
        });
    }
    lobes
}

/// Every big lobe's arc broken by two or three smaller bumps on its upper rim,
/// so no silhouette is one clean circle.
fn cauliflower(lobes: Vec<Lobe>, seed: u64) -> Vec<Lobe> {
    let mut r = Rng(seed);
    let mut out = lobes.clone();
    for l in lobes.iter().filter(|l| l.r >= BUMP_FROM) {
        for _ in 0..2 + usize::from(r.u() < 0.5) {
            // the upper half, y down
            let a = std::f32::consts::PI * r.between(1.1, 1.9);
            let b = l.r * r.between(0.3, 0.45);
            out.push(Lobe {
                x: l.x + a.cos() * (l.r - b * 0.4),
                y: l.y + a.sin() * (l.r - b * 0.4),
                r: b,
            });
        }
    }
    out
}

/// The deck a weather hangs over a run `span` units wide and glass `glass_h`
/// tall, far first: every mass at its full share, undrifted.
fn deck(weather: Weather, span: f32, glass_h: f32) -> Vec<Mass> {
    let mut r = Rng(0x0c10_0d5e ^ weather as u64);
    let mut out = Vec::new();
    let mut row = |r: &mut Rng,
                   layer: Layer,
                   n: usize,
                   base_h: f32,
                   (w0, w1): (f32, f32),
                   (h0, h1): (f32, f32),
                   jitter: f32| {
        let mut xs: Vec<f32> = (0..n)
            .map(|i| span * (i as f32 + r.u() * jitter + (1.0 - jitter) / 2.0) / n as f32)
            .collect();
        xs.sort_by(f32::total_cmp);
        for x in xs {
            let width = r.between(w0, w1);
            let base = glass_h * base_h + r.between(-2.0, 2.0);
            let height = r.between(h0, h1);
            let seed = (r.u() * 1e6) as u64;
            out.push(Mass {
                layer,
                lobes: cauliflower(cumulus(r, x, base, width, height), seed),
                base,
                weather,
                seed,
                id: 0,
                anchor: 0.0,
                off: 0.0,
            });
        }
    };
    match deck_of(weather) {
        Deck::Scattered => {
            row(&mut r, Layer::Far, 2, 0.20, (5.0, 8.0), (2.5, 3.5), 0.6);
            row(&mut r, Layer::Near, 2, 0.36, (11.0, 16.0), (5.0, 7.0), 0.6);
        }
        Deck::Cover => {
            row(&mut r, Layer::Far, 4, 0.16, (10.0, 18.0), (3.0, 4.5), 0.9);
            row(&mut r, Layer::Mid, 4, 0.26, (12.0, 18.0), (4.0, 6.0), 0.6);
            row(&mut r, Layer::Near, 3, 0.32, (14.0, 20.0), (5.0, 7.0), 0.6);
        }
        Deck::Rain => {
            row(&mut r, Layer::Far, 4, 0.20, (12.0, 20.0), (3.5, 5.5), 0.9);
            row(&mut r, Layer::Mid, 5, 0.30, (14.0, 20.0), (5.0, 7.0), 0.6);
            row(&mut r, Layer::Near, 3, 0.38, (16.0, 22.0), (6.0, 8.0), 0.6);
        }
        Deck::Storm => {
            row(&mut r, Layer::Far, 4, 0.22, (12.0, 20.0), (4.0, 6.0), 0.9);
            row(&mut r, Layer::Mid, 4, 0.32, (14.0, 20.0), (5.0, 7.0), 0.6);
            let x = span * r.between(0.35, 0.65);
            let base = glass_h * 0.34;
            let width = r.between(20.0, 26.0);
            let seed = (r.u() * 1e6) as u64;
            out.push(Mass {
                layer: Layer::Near,
                lobes: cauliflower(tower(&mut r, x, base, width, glass_h), seed),
                base,
                weather,
                seed,
                id: 0,
                anchor: 0.0,
                off: 0.0,
            });
        }
    }
    for (id, m) in out.iter_mut().enumerate() {
        m.id = id;
        m.anchor = m.reach().0;
    }
    out
}

/// The kind of deck a weather hangs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deck {
    Scattered,
    Cover,
    Rain,
    Storm,
}

fn deck_of(weather: Weather) -> Deck {
    match weather {
        Weather::Clear | Weather::Windy => Deck::Scattered,
        Weather::Overcast | Weather::Snow | Weather::Fog | Weather::Smog => Deck::Cover,
        Weather::Rain => Deck::Rain,
        Weather::Storm => Deck::Storm,
    }
}

/// How far a weather leans its clouds to their storm grey.
fn heaviness(weather: Weather) -> f32 {
    match weather {
        Weather::Clear => 0.0,
        Weather::Windy => 0.1,
        Weather::Snow => 0.3,
        Weather::Overcast => 0.35,
        Weather::Fog => 0.4,
        Weather::Smog => 0.45,
        Weather::Rain => 0.6,
        Weather::Storm => 0.85,
    }
}

/// How the sky body lights the clouds at one time of day.
#[derive(Debug, Clone, Copy)]
struct Lighting {
    /// The light's lean on the glass, x east and y down.
    dir: (f32, f32),
    /// Its height: a low sun or moon catches only the edges facing it.
    z: f32,
    /// How much of the lit band a low body keeps to the rim.
    rim: f32,
    /// How deep the base's own shadow is.
    occlude: f32,
    /// Light, body and shade.
    tones: [Rgb; 3],
}

const DAY: Lighting = Lighting {
    dir: (-0.35, -1.0),
    z: 0.8,
    rim: 0.0,
    occlude: 0.5,
    tones: [
        Rgb {
            r: 255,
            g: 251,
            b: 242,
        },
        Rgb {
            r: 231,
            g: 235,
            b: 243,
        },
        Rgb {
            r: 176,
            g: 189,
            b: 214,
        },
    ],
};
const SUNSET: Lighting = Lighting {
    dir: (-1.0, 0.15),
    z: 0.35,
    rim: 0.22,
    occlude: 0.15,
    tones: [
        Rgb {
            r: 255,
            g: 194,
            b: 146,
        },
        Rgb {
            r: 222,
            g: 168,
            b: 168,
        },
        Rgb {
            r: 122,
            g: 100,
            b: 146,
        },
    ],
};
const NIGHT: Lighting = Lighting {
    dir: (0.55, -1.0),
    z: 0.3,
    rim: 0.1,
    occlude: 0.5,
    tones: [
        Rgb {
            r: 178,
            g: 188,
            b: 214,
        },
        Rgb {
            r: 66,
            g: 74,
            b: 98,
        },
        Rgb {
            r: 34,
            g: 38,
            b: 56,
        },
    ],
};
/// A heavy sky's tones, which each weather leans to by its [`heaviness`].
const STORM_GREY: [Rgb; 3] = [
    Rgb {
        r: 118,
        g: 122,
        b: 134,
    },
    Rgb {
        r: 84,
        g: 88,
        b: 102,
    },
    Rgb {
        r: 40,
        g: 42,
        b: 54,
    },
];
/// A storm's base, near black.
const STORM_BASE: Rgb = Rgb {
    r: 22,
    g: 20,
    b: 30,
};
/// The city's warm light on a night base: one warm-dark step, no more.
const CITY_GLOW: Rgb = Rgb {
    r: 104,
    g: 74,
    b: 62,
};
const CITY_GLOW_STEP: f32 = 0.16;
/// What a strike's light tints a deck toward, ring by ring.
const FLASH_TINT: Rgb = Rgb {
    r: 214,
    g: 220,
    b: 246,
};
const FLASH_RINGS: [f32; 3] = [0.0, 0.22, 0.42];
/// How much of its first ring a strike's faint edge lifts.
const DIM_SHARE: f32 = 0.6;
/// The bolt's near-white core.
const BOLT_CORE: Rgb = Rgb {
    r: 244,
    g: 244,
    b: 255,
};
/// At 1x one cell is a 4x's sixteen: a dimmer core keeps the bolt far away.
const BOLT_1X: f32 = 0.6;

impl Lighting {
    /// Day, sunset and night mixed by their weights.
    fn mixed(day: f32, sunset: f32, night: f32) -> Lighting {
        let total = (day + sunset + night).max(f32::EPSILON);
        let (sw, nw) = (sunset / total, night / total);
        let lerp = |d: f32, s: f32, n: f32| d + (s - d) * sw + (n - d) * nw;
        let tone = |i: usize| {
            let ds = DAY.tones[i].mix(SUNSET.tones[i], sw / (1.0 - nw).max(f32::EPSILON));
            ds.mix(NIGHT.tones[i], nw)
        };
        Lighting {
            dir: (
                lerp(DAY.dir.0, SUNSET.dir.0, NIGHT.dir.0),
                lerp(DAY.dir.1, SUNSET.dir.1, NIGHT.dir.1),
            ),
            z: lerp(DAY.z, SUNSET.z, NIGHT.z),
            rim: lerp(DAY.rim, SUNSET.rim, NIGHT.rim),
            occlude: lerp(DAY.occlude, SUNSET.occlude, NIGHT.occlude),
            tones: [tone(0), tone(1), tone(2)],
        }
    }
}

/// A strike: where its light gathers, and its bolt.
#[derive(Debug, Clone, PartialEq)]
struct Strike {
    /// The flash's centre, its radius, and whether it lights its mass from
    /// inside (no bolt).
    at: (f32, f32),
    radius: f32,
    /// The mass it lights from inside, or `None` for a bolt's base.
    intra: Option<usize>,
    /// The bolt's polylines, trunk first, in units.
    bolt: Vec<Vec<(f32, f32)>>,
    /// How far up its ramp of rings the flash reaches this frame.
    step: u8,
    /// A strike's faint edge: only its brightest ring lifts, and less.
    dim: bool,
}

/// One mass's bands on a grid `d` cells to the unit, in its own undrifted
/// frame: what a frame translates by its drift and composites, and what the
/// office's [`CloudCache`] keeps.
#[derive(Debug, PartialEq)]
pub(crate) struct MassRaster {
    /// Its first column and row, in grid cells from the run's west end and
    /// the glass's top, undrifted.
    x0: i32,
    y0: i32,
    w: usize,
    h: usize,
    bands: Vec<Option<Band>>,
}

impl MassRaster {
    fn at(&self, col: i32, row: i32) -> Option<Band> {
        let (x, y) = (
            usize::try_from(col - self.x0).ok()?,
            usize::try_from(row - self.y0).ok()?,
        );
        (x < self.w && y < self.h)
            .then(|| self.bands[y * self.w + x])
            .flatten()
    }
}

/// What a [`MassRaster`] is drawn from: one mass of one weather's deck over
/// one office's glass, at one density, grown to one share, under one light.
/// Every input it reads is here, quantized, so a cached raster is the one an
/// uncached frame would draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RasterKey {
    weather: Weather,
    id: usize,
    span: u16,
    glass_h: u16,
    d: u16,
    share: u8,
    light: LightKey,
}

/// The light a mass's bands are cut under, in [`LIGHT_STEPS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LightKey {
    night: u8,
    sunset: u8,
    heaviness: u8,
    storm: u8,
}

/// Each light input's steps: the deck's bands change with the hour and the
/// weather at most this often.
const LIGHT_STEPS: f32 = 16.0;
/// A weather's share's steps: a mass grows in or out in this many.
const SHARE_STEPS: f32 = 32.0;

fn step(v: f32, steps: f32) -> u8 {
    (v.clamp(0.0, 1.0) * steps).round() as u8
}

fn unstep(k: u8, steps: f32) -> f32 {
    f32::from(k) / steps
}

/// The office's mass rasters, the most recently used first and at most
/// [`CloudCache::CAPACITY`] of them: drawing a mass's bands is most of a
/// cloudy frame's cost, and a drifting mass's bands don't change.
#[derive(Debug, Default)]
pub(crate) struct CloudCache {
    entries: std::collections::VecDeque<(RasterKey, std::sync::Arc<MassRaster>)>,
}

impl CloudCache {
    /// Room for two weathers' decks across a transition, at both densities.
    const CAPACITY: usize = 64;

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn get_or_draw(
        &mut self,
        key: RasterKey,
        draw: impl FnOnce() -> MassRaster,
    ) -> std::sync::Arc<MassRaster> {
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            let hit = self.entries.remove(i).expect("found above");
            let raster = std::sync::Arc::clone(&hit.1);
            self.entries.push_front(hit);
            return raster;
        }
        let raster = std::sync::Arc::new(draw());
        self.entries
            .push_front((key, std::sync::Arc::clone(&raster)));
        self.entries.truncate(Self::CAPACITY);
        raster
    }
}

/// What the windows' clouds are this frame.
pub(crate) struct Clouds {
    masses: Vec<Mass>,
    /// Each mass's bands, by index.
    rasters: Vec<std::sync::Arc<MassRaster>>,
    /// The grid the rasters are drawn on, cells to the unit.
    d: u16,
    lighting: Lighting,
    /// Each weather's tones under this light, by the weather's index.
    tones: Vec<(Weather, [Rgb; 3])>,
    diffuse: bool,
    heaviness: f32,
    /// The storm's share: its base shades deeper.
    storm: f32,
    night: f32,
    rain: f32,
    strike: Option<Strike>,
}

impl Clouds {
    /// The clouds of `moment` over a window run `span` units wide and glass
    /// `glass_h` tall, drawn on a grid `d` to the unit; `cache` keeps the
    /// masses' bands across frames, `None` draws them afresh.
    pub(crate) fn of(
        moment: &Moment,
        (span, glass_h): (u16, u16),
        d: u16,
        mut cache: Option<&mut CloudCache>,
    ) -> Self {
        let (span_f, glass_h_f) = (f32::from(span), f32::from(glass_h));
        let d = d.max(1);
        let weather = moment.sky.weather();
        let secs = moment.timing.beat.ms() as f32 / 1000.0;
        let body = moment.sky.body();
        let night = match body.kind {
            crate::sky::BodyKind::Sun => 1.0 - ease(body.altitude / FULL_DAY_ALTITUDE),
            crate::sky::BodyKind::Moon => 1.0,
        };
        let light = LightKey {
            night: step(night, LIGHT_STEPS),
            sunset: step(moment.look.golden_hour, LIGHT_STEPS),
            heaviness: step(weather.lerp(heaviness), LIGHT_STEPS),
            storm: step(weather.share(Weather::Storm), LIGHT_STEPS),
        };
        let night = unstep(light.night, LIGHT_STEPS);
        let sunset = unstep(light.sunset, LIGHT_STEPS);
        let day = (1.0 - night - sunset).max(0.0);
        let lighting = Lighting::mixed(day, sunset, night);
        let heavy = unstep(light.heaviness, LIGHT_STEPS);
        let tones = weather
            .parts()
            .map(|(w, _)| {
                let k = heaviness(w);
                let mut t = [0, 1, 2].map(|i| lighting.tones[i].mix(STORM_GREY[i], k));
                if w == Weather::Storm {
                    t[2] = t[2].mix(STORM_BASE, 0.6);
                }
                (w, t)
            })
            .collect();
        let mut clouds = Self {
            masses: Vec::new(),
            rasters: Vec::new(),
            d,
            lighting,
            tones,
            diffuse: heavy >= if night > 0.5 { 0.6 } else { 0.35 } && day < 0.5,
            heaviness: heavy,
            storm: unstep(light.storm, LIGHT_STEPS),
            night,
            rain: weather.share(Weather::Rain) + 0.8 * weather.share(Weather::Storm),
            strike: None,
        };
        for (w, share) in weather.parts() {
            let full = deck(w, span_f, glass_h_f);
            let widest = full
                .iter()
                .map(|m| {
                    let (west, east) = m.reach();
                    east - west
                })
                .fold(0.0, f32::max);
            let share = step(ease(share), SHARE_STEPS);
            for m in full {
                let west = drifted_west(m.anchor, secs * m.layer.drift(), span_f, widest);
                let mass = m.grown(unstep(share, SHARE_STEPS), west - m.anchor);
                let key = RasterKey {
                    weather: w,
                    id: m.id,
                    span,
                    glass_h,
                    d,
                    share,
                    light,
                };
                let draw = || clouds.draw(&mass, glass_h_f, d);
                let raster = match cache.as_deref_mut() {
                    Some(cache) => cache.get_or_draw(key, draw),
                    None => std::sync::Arc::new(draw()),
                };
                clouds.masses.push(mass);
                clouds.rasters.push(raster);
            }
        }
        // far to near: the nearest wins
        let mut order: Vec<usize> = (0..clouds.masses.len()).collect();
        order.sort_by_key(|&i| clouds.masses[i].layer);
        let (masses, rasters) = order
            .iter()
            .map(|&i| {
                (
                    clouds.masses[i].clone(),
                    std::sync::Arc::clone(&clouds.rasters[i]),
                )
            })
            .unzip();
        (clouds.masses, clouds.rasters) = (masses, rasters);
        let flash = moment.sky.flash();
        if flash > 0.0 {
            clouds.strike = clouds.strike_at(
                crate::sky::strike_bucket(moment.timing.beat),
                flash,
                span_f,
                glass_h_f,
            );
        }
        clouds
    }

    /// Mass `m`'s bands over glass `glass_h` tall on a grid `d` to the unit,
    /// in its own undrifted frame, closed: a notch narrower than
    /// [`CLOSE`] fills.
    fn draw(&self, m: &Mass, glass_h: f32, d: u16) -> MassRaster {
        let df = f32::from(d);
        let k = ((CLOSE * df).round() as i32).max(1);
        let (west, east) = m.reach();
        let x0 = ((west - SLIT) * df).floor() as i32 - k;
        let x1 = ((east + SLIT) * df).ceil() as i32 + k;
        let y0 = (m.top().min(0.0) * df).floor() as i32 - k;
        let y1 = (glass_h * df).ceil() as i32;
        let (w, h) = (
            usize::try_from(x1 - x0).unwrap_or(0),
            usize::try_from(y1 - y0).unwrap_or(0),
        );
        let unit = |i: usize| {
            (
                ((x0 + (i % w.max(1)) as i32) as f32 + 0.5) / df,
                ((y0 + (i / w.max(1)) as i32) as f32 + 0.5) / df,
            )
        };
        let raw: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = unit(i);
                m.shows() && m.inside(x, y)
            })
            .collect();
        let k = k as usize;
        let solid = erode(&dilate(&raw, w, h, k), w, h, k);
        let bands = (0..w * h)
            .map(|i| {
                solid[i].then(|| {
                    let (x, y) = unit(i);
                    let band = self.band(m, x, y);
                    if m.layer == Layer::Far && self.heaviness >= 0.6 {
                        band.max(Band::Body)
                    } else {
                        band
                    }
                })
            })
            .collect();
        MassRaster {
            x0,
            y0,
            w,
            h,
            bands,
        }
    }

    /// The flat base of the nearest mass over column `x`, where a bolt from it
    /// starts; `None` with no mass overhead.
    pub(crate) fn base_at(&self, x: f32) -> Option<f32> {
        self.nearest_over(x)
            .map(|i| self.masses[i].base_at(x - self.masses[i].off))
    }

    fn nearest_over(&self, x: f32) -> Option<usize> {
        (0..self.masses.len()).rev().find(|&i| {
            let m = &self.masses[i];
            m.lobes
                .iter()
                .any(|l| l.r > 0.0 && (x - m.off - l.x).abs() < l.r)
        })
    }

    fn strike_at(&self, bucket: u64, flash: f32, span: f32, glass_h: f32) -> Option<Strike> {
        let mut r = Rng(bucket.wrapping_mul(31).wrapping_add(7));
        let x = span * r.between(0.15, 0.85);
        let i = self.nearest_over(x)?;
        let base = self.base_at(x)?;
        let intra = r.u() < 0.4;
        let step = if flash > 0.9 {
            2
        } else {
            u8::from(flash >= 0.5)
        };
        Some(Strike {
            at: (x, base - if intra { 3.0 } else { 0.8 }),
            radius: if intra { 3.5 } else { 8.0 },
            intra: intra.then_some(i),
            bolt: if intra {
                Vec::new()
            } else {
                bolt(&mut r, (x, base), span, glass_h)
            },
            step,
            dim: flash < 0.5,
        })
    }

    /// The clouds on `view`'s glass, its run starting `run_x0` units in.
    pub(crate) fn paint(&self, view: &mut WindowView, run_x0: u16) {
        if self.masses.is_empty() {
            return;
        }
        let d = view.d();
        debug_assert_eq!(d, self.d, "the clouds were drawn for another grid");
        let df = f32::from(d);
        let unit = |cell: Cell| {
            (
                (f32::from(cell.at.0) + 0.5) / df - f32::from(run_x0),
                (f32::from(cell.glass.1) + 0.5) / df,
            )
        };
        let size = view.glass();
        let (cols, rows) = (usize::from(size.w * d), usize::from(size.h * d));
        if cols == 0 || rows == 0 {
            return;
        }
        // The glass's grid, by glass offset: its west edge's column on the run's.
        let mut west = None;
        view.paint(|cell, c| {
            if west.is_none() {
                west = Some(i32::from(cell.at.0) - i32::from(cell.glass.0) - i32::from(run_x0 * d));
            }
            c
        });
        let Some(west) = west else { return };
        let drift: Vec<i32> = self
            .masses
            .iter()
            .map(|m| (m.off * df).round() as i32)
            .collect();
        let mut px: Vec<Option<(Band, usize)>> = (0..rows * cols)
            .map(|i| {
                let (col, row) = (west + (i % cols) as i32, (i / cols) as i32);
                (0..self.masses.len())
                    .rev()
                    .find_map(|m| self.rasters[m].at(col - drift[m], row).map(|b| (b, m)))
            })
            .collect();
        if d == 1 {
            despeckle(&mut px, cols, rows);
        } else {
            close_thin_runs(&mut px, cols, rows, usize::from(d));
        }
        let mut row_sky = vec![(0u32, [0u32; 3]); rows];
        view.paint(|cell, c| {
            let gy = usize::from(cell.glass.1);
            if let Some(acc) = row_sky.get_mut(gy) {
                acc.0 += 1;
                acc.1[0] += u32::from(c.r);
                acc.1[1] += u32::from(c.g);
                acc.1[2] += u32::from(c.b);
            }
            c
        });
        let row_sky: Vec<Option<Rgb>> = row_sky
            .iter()
            .map(|&(n, s)| {
                (n > 0).then(|| Rgb {
                    r: (s[0] / n) as u8,
                    g: (s[1] / n) as u8,
                    b: (s[2] / n) as u8,
                })
            })
            .collect();
        let bolt = self.bolt_cells(d, run_x0);
        view.paint(|cell, c| {
            let (gx, gy) = (usize::from(cell.glass.0), usize::from(cell.glass.1));
            let (x, y) = unit(cell);
            let mut c = match px.get(gy * cols + gx).copied().flatten() {
                Some((band, m)) => {
                    let mass = &self.masses[m];
                    let mut tone = self.tone(mass.weather, band);
                    if band == Band::Shade && self.night > 0.5 && noise(0x617, x / 9.0) > 0.5 {
                        tone = tone.mix(CITY_GLOW, CITY_GLOW_STEP);
                    }
                    if let Some(lift) = self.flash_lift(m, x, y) {
                        tone = tone.mix(FLASH_TINT, lift);
                    }
                    let sky = row_sky.get(gy).copied().flatten().unwrap_or(c);
                    tone.mix(sky, mass.layer.sky_mix())
                }
                None => self.virga(cell, d, (x, y), c),
            };
            if bolt.contains(&(cell.at.0, cell.glass.1)) {
                c = c.mix(BOLT_CORE, if d > 1 { 1.0 } else { BOLT_1X });
            }
            c
        });
    }

    fn tone(&self, weather: Weather, band: Band) -> Rgb {
        let t = self
            .tones
            .iter()
            .find(|(w, _)| *w == weather)
            .map_or(self.lighting.tones, |(_, t)| *t);
        t[band as usize]
    }

    /// The band the light leaves `(x, y)` of `m` in: the surface's normal
    /// against the light, the mass shading itself toward its base.
    fn band(&self, m: &Mass, x: f32, y: f32) -> Band {
        const E: f32 = 0.25;
        let hc = m.height(x, y);
        // a sample off every lobe (a closed slit, the fill) is level with
        // here: no cliff at its edge
        let at = |sx: f32, sy: f32| {
            let h = m.height(sx, sy);
            if h > 0.0 { h } else { hc }
        };
        let gx = (at(x + E, y) - at(x - E, y)) / (2.0 * E);
        let gy = (at(x, y + E) - at(x, y - E)) / (2.0 * E);
        let (nx, ny, nz) = (-gx, -gy, RELIEF);
        let nn = (nx * nx + ny * ny + nz * nz).sqrt();
        let Lighting {
            dir: (mut lx, mut ly),
            z: mut lz,
            rim,
            occlude,
            ..
        } = self.lighting;
        if self.diffuse {
            // a heavy deck hides the sky body: the light is diffuse, from above
            (lx, ly, lz) = (lx * 0.3, -1.0, 0.8);
        }
        let ln = (lx * lx + ly * ly + lz * lz).sqrt();
        let mut v = (nx * lx + ny * ly + nz * lz) / (nn * ln);
        let t = (m.base_at(x) - y) / (m.base - m.top()).max(1.0);
        // how far up the base's shadow climbs, by the mass's own column: two
        // octaves, never a ruled line
        let lx = x - m.anchor;
        let reach =
            0.12 + 0.22 * noise(0x1b5b, lx * 2.3 + 40.0) + 0.14 * noise(0x1b5b, lx * 6.1 + 13.0);
        v -= (occlude + STORM_OCCLUDE * self.storm) * (1.0 - t / reach).max(0.0);
        let cuts = (
            0.55 + 0.12 * self.heaviness + rim,
            0.05 + 0.1 * self.heaviness,
        );
        let mut band = if v > cuts.0 {
            Band::Lit
        } else if v > cuts.1 {
            Band::Body
        } else {
            Band::Shade
        };
        if band == Band::Shade && t >= reach {
            // shade is the base's alone: a flank turned away stays body
            band = Band::Body;
        }
        if band == Band::Lit && rim > 0.01 && !self.diffuse {
            // a rim: only within reach of the edge it faces
            let l = (lx * lx + ly * ly).sqrt().max(f32::EPSILON);
            if m.inside(x + lx / l * RIM_REACH, y + ly / l * RIM_REACH) {
                band = Band::Body;
            }
        }
        band
    }

    /// How far a strike lifts mass `m` at `(x, y)` toward its flash: by rings
    /// around its light, the rings' edges ragged.
    fn flash_lift(&self, m: usize, x: f32, y: f32) -> Option<f32> {
        let s = self.strike.as_ref()?;
        if s.intra.is_some_and(|i| i != m) {
            return None;
        }
        let ry = s.radius * if s.intra.is_some() { 0.6 } else { 0.5 };
        let mut dist = ((x - s.at.0) / s.radius).hypot((y - s.at.1) / ry);
        dist += 0.3 * (noise(0xf1a5, x * 4.3 + y * 7.1) - 0.5);
        let ring = if dist < 0.55 { 2 } else { u8::from(dist < 1.0) };
        let lift = if s.dim {
            if ring == 2 {
                FLASH_RINGS[1] * DIM_SHARE
            } else {
                0.0
            }
        } else {
            FLASH_RINGS[usize::from(ring.min(s.step))]
        };
        (lift > 0.0).then_some(lift)
    }

    /// Faint shafts of rain under the darkest masses, by the rain's share.
    fn virga(&self, cell: Cell, d: u16, (x, y): (f32, f32), c: Rgb) -> Rgb {
        if self.rain <= 0.0 {
            return c;
        }
        let Some(m) = self.masses.iter().find(|m| {
            m.layer >= Layer::Mid
                && m.base < y
                && y <= m.base + VIRGA_DEPTH
                && m.lobes.iter().any(|l| (x - m.off - l.x).abs() < l.r * 0.7)
        }) else {
            return c;
        };
        let (gx, gy) = (u32::from(cell.at.0), u32::from(cell.at.1));
        let fade = 1.0 - (y - m.base) / VIRGA_DEPTH;
        // sparse, slanted streaks, half their cells
        let streak = (gx + gy / 2) % (VIRGA_PITCH * u32::from(d)) == 0;
        if streak && gx % 2 == gy % 2 {
            c.mix(
                self.tone(m.weather, Band::Shade),
                0.35 * self.rain.min(1.0) * fade,
            )
        } else {
            c
        }
    }

    /// The cells the bolt's core covers on a grid `d` to the unit, as each
    /// cell's grid column and glass row.
    fn bolt_cells(&self, d: u16, run_x0: u16) -> std::collections::HashSet<(u16, u16)> {
        let mut out = std::collections::HashSet::new();
        let Some(s) = &self.strike else { return out };
        let df = f32::from(d);
        for (li, line) in s.bolt.iter().enumerate() {
            // 1 cell wide on every grid; at 4x the trunk's top few segments 2
            let parts: Vec<(&[(f32, f32)], u16)> = if d > 1 && li == 0 {
                vec![
                    (&line[..line.len().min(4)], 2),
                    (&line[line.len().min(3)..], 1),
                ]
            } else {
                vec![(&line[..], 1)]
            };
            for (seg, width) in parts {
                for pair in seg.windows(2) {
                    let (ax, ay) = ((pair[0].0 + f32::from(run_x0)) * df, pair[0].1 * df);
                    let (bx, by) = ((pair[1].0 + f32::from(run_x0)) * df, pair[1].1 * df);
                    let n = (bx - ax).abs().max((by - ay).abs()) as u32 + 1;
                    for i in 0..=n {
                        let t = i as f32 / n as f32;
                        let (x, y) = (ax + (bx - ax) * t, ay + (by - ay) * t);
                        for o in 0..width {
                            out.insert((x as u16 + o, y as u16));
                        }
                    }
                }
            }
        }
        out
    }
}

/// How far a low body's rim light reaches in from the edge it faces.
const RIM_REACH: f32 = 1.5;
/// How much deeper a storm's base shades.
const STORM_OCCLUDE: f32 = 0.4;
/// How far under a base its rain shafts fall.
const VIRGA_DEPTH: f32 = 4.5;
/// The units between two rain shafts.
const VIRGA_PITCH: u32 = 3;

/// The sun's altitude, from the horizon's 0, above which its clouds wear
/// their full day tones.
const FULL_DAY_ALTITUDE: f32 = 0.3;

/// Where a mass whose west edge began at `x0` has drifted after `travelled`
/// units east over a run `span` wide, its deck's widest mass `widest` wide: it
/// wraps a period the run plus two of the widest long, so it leaves the run's
/// east end whole before it comes back round its west.
fn drifted_west(x0: f32, travelled: f32, span: f32, widest: f32) -> f32 {
    (x0 + widest + travelled).rem_euclid(span + 2.0 * widest) - widest
}

/// A share eased in and out: a mass forms slowly, then fills, then settles.
fn ease(share: f32) -> f32 {
    let s = share.clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

/// A strike's bolt from `(x, base)`: a zig-zag trunk a quarter to two-fifths
/// of the glass long, and one fork.
fn bolt(r: &mut Rng, (mut x, base): (f32, f32), span: f32, glass_h: f32) -> Vec<Vec<(f32, f32)>> {
    let mut y = base;
    let depth = y + glass_h * r.between(0.25, 0.40);
    let mut trunk = vec![(x, y)];
    let lean = if r.u() < 0.5 { 1.0 } else { -1.0 };
    while y < depth {
        y += r.between(0.7, 1.3);
        x += lean * r.between(0.15, 0.65) * if r.u() < 0.7 { 1.0 } else { -1.0 };
        x = x.clamp(1.5, span - 2.5);
        trunk.push((x, y));
    }
    let k = 1 + (r.u() * (trunk.len().saturating_sub(3)).max(1) as f32) as usize;
    let (mut fx, mut fy) = trunk[k.min(trunk.len() - 1)];
    let mut fork = vec![(fx, fy)];
    for _ in 0..2 + (r.u() * 2.0) as usize {
        fy += r.between(0.6, 1.1);
        fx = (fx - lean * r.between(0.4, 0.9)).clamp(1.0, span - 2.0);
        fork.push((fx, fy));
    }
    vec![trunk, fork]
}

/// A square grow by `k` cells.
fn dilate(m: &[bool], cols: usize, rows: usize, k: usize) -> Vec<bool> {
    morph(m, cols, rows, k, true)
}

/// A square shrink by `k` cells.
fn erode(m: &[bool], cols: usize, rows: usize, k: usize) -> Vec<bool> {
    morph(m, cols, rows, k, false)
}

fn morph(m: &[bool], cols: usize, rows: usize, k: usize, grow: bool) -> Vec<bool> {
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        (0..rows * cols)
            .map(|i| {
                let (x, y) = (i % cols, i / cols);
                let (along, len) = if horizontal { (x, cols) } else { (y, rows) };
                let at = |j: usize| {
                    if horizontal {
                        y * cols + j
                    } else {
                        j * cols + x
                    }
                };
                let mut window =
                    (along.saturating_sub(k)..=(along + k).min(len - 1)).map(|j| src[at(j)]);
                if grow {
                    window.any(|b| b)
                } else {
                    window.all(|b| b)
                }
            })
            .collect()
    };
    pass(&pass(m, true), false)
}

/// At 1x a lone cell is noise, not form: one whose band none of its mass's
/// 4-neighbours share takes the band most of them have.
fn despeckle(px: &mut [Option<(Band, usize)>], cols: usize, rows: usize) {
    let snapshot = px.to_vec();
    for i in 0..px.len() {
        let Some((band, m)) = snapshot[i] else {
            continue;
        };
        let (x, y) = (i % cols, i / cols);
        let nb: Vec<Band> = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ]
        .into_iter()
        .filter(|&(nx, ny)| nx < cols && ny < rows)
        .filter_map(|(nx, ny)| snapshot[ny * cols + nx])
        .filter(|&(_, nm)| nm == m)
        .map(|(b, _)| b)
        .collect();
        if nb.len() >= 3 && !nb.contains(&band) {
            let most = [Band::Lit, Band::Body, Band::Shade]
                .into_iter()
                .max_by_key(|b| nb.iter().filter(|&&n| n == *b).count())
                .unwrap_or(band);
            px[i] = Some((most, m));
        }
    }
}

/// A run no wider than `thin` cells between two cells of one mass and band
/// takes that mass and band: a hairline crease reads as smoke or a crack.
fn close_thin_runs(px: &mut [Option<(Band, usize)>], cols: usize, rows: usize, thin: usize) {
    for horizontal in [true, false] {
        let (outer, inner) = if horizontal {
            (rows, cols)
        } else {
            (cols, rows)
        };
        let at = |o: usize, i: usize| {
            if horizontal {
                o * cols + i
            } else {
                i * cols + o
            }
        };
        for o in 0..outer {
            let mut i = 1;
            while i < inner {
                let prev = px[at(o, i - 1)];
                if prev.is_none() || px[at(o, i)] == prev {
                    i += 1;
                    continue;
                }
                let run = px[at(o, i)];
                let mut n = 0;
                while i + n < inner && px[at(o, i + n)] == run && n <= thin {
                    n += 1;
                }
                if n <= thin && i + n < inner && px[at(o, i + n)] == prev {
                    for j in 0..n {
                        px[at(o, i + j)] = prev;
                    }
                }
                i += n.max(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Motion;
    use crate::sky::Sky;
    use std::time::Duration;

    const SPAN: u16 = 160;
    const GLASS_H: u16 = 20;

    fn clouds(weather: Weather, hour: u32, flash: f32) -> Clouds {
        let now = crate::localclock::at_hour(hour);
        let sky = Sky::at_with(now, weather).with_flash(flash);
        let moment = Moment::resolve(sky, &crate::theme::NORMAL, 0.0, Motion::Full.timing(now));
        Clouds::of(&moment, (SPAN, GLASS_H), 1, None)
    }

    /// A drifting mass wraps only out of sight: it has left the run's east end
    /// whole, and comes back round wholly west of its west end.
    #[test]
    fn a_mass_wraps_only_out_of_sight() {
        let (span, widest) = (f32::from(SPAN), 26.0);
        for width in [5.0, 16.0, widest] {
            let mut prev = drifted_west(10.0, 0.0, span, widest);
            for step in 1..20_000 {
                let west = drifted_west(10.0, step as f32 * 0.05, span, widest);
                if west < prev {
                    assert!(prev >= span, "wrapped at {prev}, still on the run");
                    assert!(
                        west + width <= 0.0,
                        "came back at {west}, already on the run"
                    );
                }
                prev = west;
            }
        }
    }

    /// A mass grows from its base: the base stays put, and it fills in as its
    /// weather's share rises, never jumping.
    #[test]
    fn a_mass_grows_from_its_fixed_base() {
        let full = deck(Weather::Rain, f32::from(SPAN), f32::from(GLASS_H));
        for m in &full {
            let mut prev = 0;
            for k in 0..=20 {
                let g = m.grown(ease(k as f32 / 20.0), 0.0);
                assert_eq!(g.base, m.base);
                let cells = (0..SPAN)
                    .flat_map(|x| (0..GLASS_H).map(move |y| (x, y)))
                    .filter(|&(x, y)| {
                        g.lobes.iter().any(|l| l.r > 0.0)
                            && g.inside(f32::from(x) + 0.5, f32::from(y) + 0.5)
                    })
                    .count();
                assert!(
                    cells + 2 >= prev,
                    "share {k}/20 shrank the mass: {prev} → {cells}"
                );
                prev = cells;
            }
        }
    }

    /// A strike's bolt drops from the base of the mass over it.
    #[test]
    fn the_bolt_starts_at_its_cloud_base() {
        let mut bolts = 0;
        for bucket in 0..40 {
            let c = clouds(Weather::Storm, 12, 1.0);
            let Some(s) = c.strike_at(bucket, 1.0, f32::from(SPAN), f32::from(GLASS_H)) else {
                continue;
            };
            let Some(trunk) = s.bolt.first() else {
                continue;
            };
            let (x, y) = trunk[0];
            assert_eq!(
                Some(y),
                c.base_at(x),
                "bucket {bucket}'s bolt hangs off its base"
            );
            bolts += 1;
        }
        assert!(bolts > 0, "the sample must strike a bolt");
    }

    /// A far mass leans further to the sky behind it than a near one.
    #[test]
    fn a_far_mass_leans_to_the_sky() {
        let sky = Rgb {
            r: 90,
            g: 130,
            b: 200,
        };
        let c = clouds(Weather::Overcast, 12, 0.0);
        for band in [Band::Lit, Band::Body, Band::Shade] {
            let tone = c.tone(Weather::Overcast, band);
            let off = |layer: Layer| {
                let t = tone.mix(sky, layer.sky_mix());
                (i32::from(t.r) - i32::from(sky.r)).abs()
                    + (i32::from(t.g) - i32::from(sky.g)).abs()
                    + (i32::from(t.b) - i32::from(sky.b)).abs()
            };
            assert!(
                off(Layer::Far) < off(Layer::Mid) && off(Layer::Mid) < off(Layer::Near),
                "{band:?}"
            );
        }
    }

    /// The photosensitive floor: a strike changes the clouds' light only as
    /// the flash's phase changes, never within one.
    #[test]
    fn a_strike_lights_the_deck_only_by_its_phase() {
        let lifts = |flash: f32| {
            let c = clouds(Weather::Storm, 12, flash);
            (0..SPAN * 4)
                .flat_map(|x| (0..GLASS_H * 4).map(move |y| (x, y)))
                .map(|(x, y)| {
                    (0..c.masses.len())
                        .map(|m| {
                            c.flash_lift(m, f32::from(x) / 4.0, f32::from(y) / 4.0)
                                .map(f32::to_bits)
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(lifts(0.6), lifts(0.85), "one phase, two looks");
        assert_eq!(lifts(0.1), lifts(0.4), "one faint phase, two looks");
        assert_ne!(lifts(0.0), lifts(1.0), "a peak must light the deck");
    }

    /// Clouds drift on the beat: at rest they hold still.
    #[test]
    fn a_still_office_holds_its_clouds() {
        let at = |ms: u64| {
            let now = crate::localclock::at_hour(12) + Duration::from_millis(ms);
            let sky = Sky::at_with(now, Weather::Overcast);
            let moment =
                Moment::resolve(sky, &crate::theme::NORMAL, 0.0, Motion::Still.timing(now));
            Clouds::of(&moment, (SPAN, GLASS_H), 1, None).masses
        };
        assert_eq!(at(0), at(600_000));
    }
}
