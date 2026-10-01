//! The sun, the moon and the stars behind the windows: where the disc stands,
//! and what a pane's glass shows at a point, as a few tones resolved once a
//! frame and picked per pixel by ordered dither.

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;

use crate::anim::epoch_ms;
use crate::atmosphere::Moment;
use crate::composite::{WHITE, blend, blend_rgb};
use crate::dither::FALLOFF_TONES;
use crate::layout::window_run;
use crate::sky::{Body, Sky};
use crate::theme::Theme;

/// One frame's disc (sun by day, moon by night), arcing across the window
/// wall. `cx` is an ABSOLUTE buffer x, not a per-window offset: one disc across
/// the whole wall, shown by the one pane it stands over ([`Disc::hosted_by`]).
#[derive(Clone, Copy)]
pub(crate) struct Disc {
    pub(crate) cx: f32,
    pub(crate) cy: f32,
    pub(crate) r: f32,
    vis: f32,
    /// Illuminated fraction (0 new..1 full) — `1.0` for the sun; for the moon it
    /// drives the elliptical terminator.
    lit_frac: f32,
    /// The lit limb is on the right, as a northern-hemisphere sky shows a waxing
    /// moon; `false` puts it on the left ([`Sky::moon_waxing`]).
    lit_right: bool,
    body: Body,
}

/// What of the disc lands at a point of glass.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DiscPart {
    Lit,
    /// The moon's dark limb, past the terminator.
    Dark,
    /// Its halo, `1` at the rim fading to `0` at [`GLOW_PX`] past it.
    Halo(f32),
}

const DISC_RADIUS_PX: f32 = 5.0;
const GLOW_PX: f32 = 3.0;
const GLOW_ALPHA: f32 = 0.55;
/// The moon's dark (un-illuminated) limb, near the night sky's own base colour
/// so the shadowed side recedes into the backdrop instead of reading as a
/// hard-edged bite out of the disc.
pub(crate) const MOON_SHADOW: Rgb = Rgb {
    r: 30,
    g: 34,
    b: 52,
};
// "Real low window": the horizon sits low in the band and the apex climbs off
// the glass entirely rather than tracking the full window height.
const HORIZON_FRAC: f32 = 0.55;
const ARC_RISE_FRAC: f32 = 0.80;
/// Below this [`Transmission::disc`](crate::sky::Transmission::disc), thick cloud
/// swallows the disc entirely.
pub(crate) const MIN_DISC_VIS: f32 = 0.08;

impl Disc {
    /// This frame's disc over a wall band `top_wall_h` tall, or `None` under
    /// thick cloud.
    pub(crate) fn of(sky: &Sky, buf_w: u16, top_wall_h: u16) -> Option<Self> {
        let e = sky.emitter();
        let vis = match e.body {
            Body::Sun => sky.transmission().disc,
            // A moon below the horizon shows no disc; one up fades in with the night.
            Body::Moon if e.altitude <= 0.0 => return None,
            Body::Moon => sky.transmission().disc * sky.nightfall(),
        };
        if vis < MIN_DISC_VIS {
            return None;
        }
        // Sweep across [first pane, last pane] inset by the radius. NOT the pane
        // CENTERS (bit-identical to the mullion columns, so the span bisected the
        // disc at its most visible low-altitude moment and froze `cx` there on a
        // single-window buffer) and NOT a linear `buf_w - WINDOW_W` bound (that
        // only coincidentally lands inside a window).
        let run = window_run(buf_w);
        let span_left = f32::from(run.start) + DISC_RADIUS_PX;
        let span_right = (f32::from(run.end) - DISC_RADIUS_PX).max(span_left);
        let cx = span_left + e.azimuth * (span_right - span_left);
        let horizon_y = top_wall_h as f32 * HORIZON_FRAC;
        let cy = horizon_y - e.altitude * (top_wall_h as f32 * ARC_RISE_FRAC);
        let (lit_frac, lit_right) = match e.body {
            Body::Sun => (1.0, true),
            Body::Moon => (sky.moon_phase(), sky.moon_waxing()),
        };
        Some(Self {
            cx,
            cy,
            r: DISC_RADIUS_PX,
            vis,
            lit_frac,
            lit_right,
            body: e.body,
        })
    }

    /// Whether the pane over columns `x..x + w` shows the disc: only the pane
    /// its centre sits over. A disc near an inter-window gap is wide enough to
    /// reach both neighbours' glass, and would render twice across the pillar.
    pub(crate) fn hosted_by(&self, x: u16, w: u16) -> bool {
        self.cx >= f32::from(x) && self.cx < f32::from(x + w)
    }

    fn at(&self, x: f32, y: f32) -> Option<DiscPart> {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist <= self.r {
            if self.lit_frac >= 1.0 {
                return Some(DiscPart::Lit);
            }
            let terminator_x =
                (1.0 - 2.0 * self.lit_frac) * (self.r * self.r - dy * dy).max(0.0).sqrt();
            let toward_lit_limb = if self.lit_right { dx } else { -dx };
            Some(if toward_lit_limb >= terminator_x {
                DiscPart::Lit
            } else {
                DiscPart::Dark
            })
        } else if dist <= self.r + GLOW_PX {
            Some(DiscPart::Halo(1.0 - (dist - self.r) / GLOW_PX))
        } else {
            None
        }
    }

    /// Its halo's strength at the rim. Scaling by `lit_frac` keeps a new moon's
    /// near-dark face from casting a full-bright halo.
    fn halo_peak(&self) -> f32 {
        self.vis * GLOW_ALPHA * self.lit_frac
    }
}

/// Roughly 1-in-`STAR_SPARSITY` sky pixels host a star — prime so the
/// hash-modulo grid can't line up into a visible lattice.
const STAR_SPARSITY: u64 = 47;
const STAR_COLOR: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Cap on the star blend alpha — a faint glimmer, not a bright dot.
const STAR_ALPHA_MAX: f32 = 0.55;
/// Per-star twinkle cycle length range (ms), hashed per position so the field
/// doesn't blink in unison.
const STAR_TWINKLE_CYCLE_BASE_MS: u64 = 2000;
const STAR_TWINKLE_CYCLE_SPAN_MS: u64 = 3000;

/// Deterministic sparse star field, hashed on the ABSOLUTE buffer `(px, py)`
/// so it reads as one continuous sky rather than a per-window reseed.
fn star_exists(px: u16, py: u16) -> bool {
    let mut h = (px as u64).wrapping_mul(crate::GOLDEN_GAMMA);
    h ^= (py as u64).wrapping_mul(crate::MURMUR64A_M);
    h = (h ^ (h >> 17)).wrapping_mul(pixtuoid_core::id::SPLITMIX64_M2);
    h.is_multiple_of(STAR_SPARSITY)
}

/// Per-star twinkle: a hashed per-star cycle length, rerolled on/off each cycle.
fn star_twinkle(px: u16, py: u16, now: SystemTime) -> bool {
    let now_ms = epoch_ms(now);
    let seed = (px as u64).wrapping_mul(131) ^ (py as u64).wrapping_mul(521);
    let cycle_ms = STAR_TWINKLE_CYCLE_BASE_MS + (seed % STAR_TWINKLE_CYCLE_SPAN_MS);
    let phase = now_ms / cycle_ms;
    let hash = seed.wrapping_add(phase).wrapping_mul(crate::GOLDEN_GAMMA);
    (hash % 10) < 7
}

/// The flat bands the window sky steps through from zenith to horizon.
const SKY_TONES: usize = 4;

/// A colour for each band of the sky, zenith first.
type Tones = [Rgb; SKY_TONES];

/// Below this [`Look::golden_hour`](crate::atmosphere::Look::golden_hour) the
/// blaze is too faint to paint.
const BLAZE_MIN: f32 = 0.05;
/// How far a full golden hour pulls open sky toward [`BLAZE`].
const BLAZE_SHARE: f32 = 0.35;
/// The golden hour's colour, and how far each channel leans toward it: red
/// most, so the cast reads orange over any sky tone.
const BLAZE: Rgb = Rgb {
    r: 255,
    g: 160,
    b: 60,
};
const BLAZE_LEAN: [f32; 3] = [0.4, 0.25, 0.1];

/// The golden hour's warm cast over open sky, at one strength a frame.
#[derive(Clone, Copy)]
pub(crate) struct Blaze(f32);

impl Blaze {
    fn of(golden_hour: f32) -> Option<Self> {
        (golden_hour > BLAZE_MIN).then_some(Self(golden_hour * BLAZE_SHARE))
    }

    /// `cur` under the blaze.
    pub(crate) fn over(self, cur: Rgb) -> Rgb {
        let [r, g, b] = BLAZE_LEAN.map(|lean| self.0 * lean);
        Rgb {
            r: blend(cur.r, BLAZE.r, r),
            g: blend(cur.g, BLAZE.g, g),
            b: blend(cur.b, BLAZE.b, b),
        }
    }
}

/// How far a sun at its apex pales the wall spot from the theme's warm spill
/// toward white.
const WALL_SPOT_PALE: f32 = 0.6;

/// The sun's spot on a side wall at `warmth`: the theme's warm spill, paling
/// toward white as the sun climbs.
pub(crate) fn wall_spot_colour(warmth: f32, theme: &Theme) -> Rgb {
    blend_rgb(
        theme.lighting.sun_spill,
        WHITE,
        (1.0 - warmth) * WALL_SPOT_PALE,
    )
}

/// The window sky one frame shows: its disc and stars, and every tone they
/// paint in, resolved once so a pixel only picks among them.
pub(crate) struct SkyView {
    disc: Option<Disc>,
    stars: bool,
    now: SystemTime,
    sky: Tones,
    star: Tones,
    lit: Tones,
    dark: Tones,
    halo: [[Rgb; FALLOFF_TONES as usize]; SKY_TONES],
    blaze: Option<Blaze>,
}

impl SkyView {
    /// `moment`'s sky behind a wall band `top_wall_h` tall, in `theme`.
    pub(crate) fn of(moment: &Moment, buf_w: u16, top_wall_h: u16, theme: &Theme) -> Self {
        let look = &moment.look;
        let sky: Tones = std::array::from_fn(|k| {
            look.glass_b
                .mix(look.glass_a, k as f32 / (SKY_TONES - 1) as f32)
        });
        let disc = Disc::of(&moment.sky, buf_w, top_wall_h);
        let core = match disc.map_or(Body::Sun, |d| d.body) {
            Body::Sun => theme.lighting.sun_core,
            Body::Moon => theme.lighting.moon_core,
        };
        let (vis, peak) = disc.map_or((0.0, 0.0), |d| (d.vis, d.halo_peak()));
        let over = |c: Rgb, alpha: f32| sky.map(|s| blend_rgb(s, c, alpha));
        let tone = |k: usize| peak * (k + 1) as f32 / f32::from(FALLOFF_TONES);
        Self {
            disc,
            stars: look.star_strength > 0.0,
            now: moment.now,
            sky,
            star: over(STAR_COLOR, look.star_strength * STAR_ALPHA_MAX),
            lit: over(core, vis),
            dark: over(MOON_SHADOW, vis),
            halo: sky.map(|s| std::array::from_fn(|k| blend_rgb(s, core, tone(k)))),
            blaze: Blaze::of(look.golden_hour),
        }
    }

    /// The golden hour's cast over the open sky, while it shows.
    pub(crate) fn blaze(&self) -> Option<Blaze> {
        self.blaze
    }

    /// One pane's glass, over columns `x..x + w` and `glass_h` rows tall.
    pub(crate) fn pane(&self, x: u16, w: u16, glass_h: u16) -> PaneSky<'_> {
        PaneSky {
            view: self,
            hosts_disc: self.disc.is_some_and(|d| d.hosted_by(x, w)),
            glass_h,
            clear_rows: crate::skyline::clear_sky_rows(glass_h),
        }
    }
}

/// One pane's share of a [`SkyView`]: whether it shows the disc, and how far
/// down its glass the open sky runs, where a star may shine.
pub(crate) struct PaneSky<'a> {
    view: &'a SkyView,
    hosts_disc: bool,
    glass_h: u16,
    clear_rows: u16,
}

impl PaneSky<'_> {
    /// The colour `glass_dy` rows down this pane's glass, at sample point `p`,
    /// dithered on grid cell `g`.
    pub(crate) fn colour(&self, p: (f32, f32), g: (u16, u16), glass_dy: f32) -> Rgb {
        let v = self.view;
        let share = crate::atmosphere::sky_share(glass_dy, self.glass_h);
        let band = crate::dither::nearest(share * (SKY_TONES - 1) as f32, g.0, g.1);
        let i = usize::from(band).min(SKY_TONES - 1);
        match v
            .disc
            .filter(|_| self.hosts_disc)
            .and_then(|d| d.at(p.0, p.1))
        {
            Some(DiscPart::Lit) => return v.lit[i],
            Some(DiscPart::Dark) => return v.dark[i],
            Some(DiscPart::Halo(f)) => {
                // Flat, not seamed: at a pixel a ring, a seam's specks would
                // be all the ring there is.
                let k = (f * f32::from(FALLOFF_TONES)).round() as u8;
                if k > 0 {
                    return v.halo[i][usize::from(k.min(FALLOFF_TONES)) - 1];
                }
            }
            None => {}
        }
        let (sx, sy) = (p.0 as u16, p.1 as u16);
        if v.stars
            && glass_dy < f32::from(self.clear_rows)
            && star_exists(sx, sy)
            && star_twinkle(sx, sy, v.now)
        {
            return v.star[i];
        }
        v.sky[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::Weather;

    fn view(hour: u32) -> SkyView {
        let now = crate::localclock::at_hour(hour);
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let moment = Moment::resolve(Sky::at_with(now, Weather::Clear), theme, 0.0, now);
        SkyView::of(&moment, 160, 40, theme)
    }

    #[test]
    fn the_window_sky_paints_only_its_resolved_tones() {
        for hour in [6, 12, 19, 22] {
            let v = view(hour);
            let palette: Vec<Rgb> = [v.sky, v.star, v.lit, v.dark]
                .into_iter()
                .flatten()
                .chain(v.halo.into_iter().flatten())
                .collect();
            let pane = v.pane(0, 160, 30);
            for y in 0..30u16 {
                for x in 0..160u16 {
                    let c = pane.colour((f32::from(x), f32::from(y)), (x, y), f32::from(y));
                    assert!(palette.contains(&c), "{hour}h ({x},{y}): {c:?}");
                }
            }
        }
    }

    #[test]
    fn the_sky_is_flat_at_its_ends() {
        let v = view(12);
        let glass_h = 30;
        let pane = v.pane(0, 0, glass_h);
        let tile = |glass_dy: u16| -> Vec<Rgb> {
            (0..crate::dither::PERIOD)
                .flat_map(|y| (0..crate::dither::PERIOD).map(move |x| (x, y)))
                .map(|(x, y)| pane.colour((0.0, 0.0), (x, y), f32::from(glass_dy)))
                .collect()
        };
        assert!(tile(0).iter().all(|&c| c == v.sky[0]), "zenith");
        assert!(
            tile(glass_h - 1).iter().all(|&c| c == v.sky[SKY_TONES - 1]),
            "horizon"
        );
    }
}
