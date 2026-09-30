//! The sun, the moon and the stars behind the windows, pixel-free: where the
//! disc stands, and what the glass shows at a point, as a few tones resolved
//! once a frame and picked per pixel by ordered dither.

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;

use crate::anim::epoch_ms;
use crate::atmosphere::Moment;
use crate::dither::FALLOFF_TONES;
use crate::layout::window_run;
use crate::pixel_painter::blend_rgb;
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
    pub(crate) vis: f32,
    /// Illuminated fraction (0 new..1 full) — `1.0` for the sun; for the moon it
    /// drives the elliptical terminator.
    pub(crate) lit_frac: f32,
    /// The lit limb is on the right, as a northern-hemisphere sky shows a waxing
    /// moon; `false` puts it on the left ([`Sky::moon_waxing`]). The sun is
    /// fully lit, so it never reads this.
    pub(crate) lit_right: bool,
    body: Body,
}

/// What of the disc lands at a point of glass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DiscPart {
    /// Its lit face.
    Lit,
    /// The moon's dark limb, past the terminator.
    Dark,
    /// Its halo, `1` at the rim fading to `0` at [`GLOW_PX`] past it.
    Halo(f32),
}

const DISC_RADIUS_PX: f32 = 5.0;
pub(crate) const GLOW_PX: f32 = 3.0;
pub(crate) const GLOW_ALPHA: f32 = 0.55;
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
const HORIZON_FRAC: f32 = 0.55; // horizon_y = top_wall_h * HORIZON_FRAC
const ARC_RISE_FRAC: f32 = 0.80; // apex lifts top_wall_h * ARC_RISE_FRAC above horizon
/// Below this atmo `disc` visibility, thick cloud swallows the disc entirely.
pub(crate) const MIN_DISC_VIS: f32 = 0.08;

impl Disc {
    /// This frame's disc over a wall band `top_wall_h` tall, or `None` under
    /// thick cloud.
    pub(crate) fn of(sky: &Sky, buf_w: u16, top_wall_h: u16) -> Option<Self> {
        let e = sky.emitter();
        let vis = sky.atmo().disc;
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

    /// What of the disc lands at `(x, y)`, if any.
    pub(crate) fn at(&self, x: f32, y: f32) -> Option<DiscPart> {
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
pub(crate) const STAR_COLOR: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Cap on the star blend alpha — a faint glimmer, not a bright dot.
pub(crate) const STAR_ALPHA_MAX: f32 = 0.55;
/// Per-star twinkle cycle length range (ms), hashed per position so the field
/// doesn't blink in unison.
const STAR_TWINKLE_CYCLE_BASE_MS: u64 = 2000;
const STAR_TWINKLE_CYCLE_SPAN_MS: u64 = 3000;

/// Deterministic sparse star field, hashed on the ABSOLUTE buffer `(px, py)`
/// so it reads as one continuous sky rather than a per-window reseed.
pub(crate) fn star_exists(px: u16, py: u16) -> bool {
    let mut h = (px as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= (py as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    h = (h ^ (h >> 17)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h.is_multiple_of(STAR_SPARSITY)
}

/// Per-star twinkle: a hashed per-star cycle length, rerolled on/off each cycle.
pub(crate) fn star_twinkle(px: u16, py: u16, now: SystemTime) -> bool {
    let now_ms = epoch_ms(now);
    let seed = (px as u64).wrapping_mul(131) ^ (py as u64).wrapping_mul(521);
    let cycle_ms = STAR_TWINKLE_CYCLE_BASE_MS + (seed % STAR_TWINKLE_CYCLE_SPAN_MS);
    let phase = now_ms / cycle_ms;
    let hash = seed.wrapping_add(phase).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash % 10) < 7
}

/// The flat bands the window sky steps through from zenith to horizon, each
/// seam between two dithered.
const SKY_TONES: usize = 4;

/// A colour for each band of the sky, zenith first.
type Tones = [Rgb; SKY_TONES];

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
        }
    }

    /// The disc, where one is up.
    pub(crate) fn disc(&self) -> Option<Disc> {
        self.disc
    }

    /// The colour at sample point `p` of glass whose pane shows the disc
    /// (`hosts_disc`), dithered on grid cell `g`: `share` of the way from
    /// zenith to horizon, where `open_sky` says a star may shine.
    pub(crate) fn colour(
        &self,
        p: (f32, f32),
        g: (u16, u16),
        share: f32,
        open_sky: bool,
        hosts_disc: bool,
    ) -> Rgb {
        let band = crate::dither::nearest(share * (SKY_TONES - 1) as f32, g.0, g.1);
        let i = usize::from(band).min(SKY_TONES - 1);
        if let Some(part) = self
            .disc
            .filter(|_| hosts_disc)
            .and_then(|d| d.at(p.0, p.1))
        {
            return match part {
                DiscPart::Lit => self.lit[i],
                DiscPart::Dark => self.dark[i],
                DiscPart::Halo(f) => {
                    match crate::dither::nearest(f * f32::from(FALLOFF_TONES), g.0, g.1) {
                        0 => self.sky[i],
                        k => self.halo[i][usize::from(k.min(FALLOFF_TONES)) - 1],
                    }
                }
            };
        }
        let (sx, sy) = (p.0 as u16, p.1 as u16);
        if self.stars && open_sky && star_exists(sx, sy) && star_twinkle(sx, sy, self.now) {
            return self.star[i];
        }
        self.sky[i]
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

    /// Every pixel of glass is one of the frame's resolved tones: no pixel
    /// blends its own colour.
    #[test]
    fn the_window_sky_paints_only_its_resolved_tones() {
        for hour in [6, 12, 19, 22] {
            let v = view(hour);
            let palette: Vec<Rgb> = [v.sky, v.star, v.lit, v.dark]
                .into_iter()
                .flatten()
                .chain(v.halo.into_iter().flatten())
                .collect();
            for y in 0..30u16 {
                for x in 0..160u16 {
                    let share = f32::from(y) / 29.0;
                    let c = v.colour((f32::from(x), f32::from(y)), (x, y), share, true, true);
                    assert!(palette.contains(&c), "{hour}h ({x},{y}): {c:?}");
                }
            }
        }
    }

    /// The zenith and the horizon are each one flat band; the tones between
    /// meet at dithered seams.
    #[test]
    fn the_sky_is_flat_at_its_ends() {
        let v = view(12);
        let tile = |share: f32| -> Vec<Rgb> {
            (0..crate::dither::PERIOD)
                .flat_map(|y| (0..crate::dither::PERIOD).map(move |x| (x, y)))
                .map(|(x, y)| v.colour((0.0, 0.0), (x, y), share, false, false))
                .collect()
        };
        assert!(tile(0.0).iter().all(|&c| c == v.sky[0]), "zenith");
        assert!(
            tile(1.0).iter().all(|&c| c == v.sky[SKY_TONES - 1]),
            "horizon"
        );
    }
}
