//! Celestial bodies + the night sky: the sun/moon disc (placement, per-theme
//! core color, thick-cloud gating, the "real low window" arc) and the
//! deterministic night star field.

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;

use super::epoch_ms;
use super::{window_columns, WINDOW_W};
use crate::sky::{Body, Sky};
use crate::theme::Theme;

/// One frame's celestial disc (sun by day, moon by night), arcing across the
/// window wall. `cx` is an ABSOLUTE buffer x, not a per-window offset, so the
/// body paints only in whichever window it currently sits over — one disc
/// across the whole wall, not one per window.
#[derive(Clone, Copy)]
pub(super) struct Disc {
    pub(super) cx: f32,
    pub(super) cy: f32,
    pub(super) r: f32,
    pub(super) core: Rgb,
    pub(super) glow: Rgb,
    pub(super) vis: f32,
    /// Illuminated fraction (0 new..1 full) — `1.0` for the sun; for the moon it
    /// drives the elliptical terminator in the disc-core render.
    pub(super) lit_frac: f32,
    /// The lit limb is on the right, as a northern-hemisphere sky shows a waxing
    /// moon; `false` puts it on the left ([`Sky::moon_waxing`]). The sun is
    /// fully lit, so it never reads this.
    pub(super) lit_right: bool,
}

const DISC_RADIUS_PX: f32 = 5.0;
pub(super) const GLOW_PX: f32 = 3.0;
pub(super) const GLOW_ALPHA: f32 = 0.55;
/// The moon's dark (un-illuminated) limb, blended in over the terminator —
/// near the night sky's own base color so the shadowed side recedes into the
/// backdrop instead of reading as a hard-edged bite out of the disc.
pub(super) const MOON_SHADOW: Rgb = Rgb {
    r: 30,
    g: 34,
    b: 52,
};
const FIRST_WINDOW_X: f32 = super::FIRST_WINDOW_X as f32;
// "Real low window": the horizon sits low in the band and the apex climbs off
// the glass entirely rather than tracking the full window height.
const HORIZON_FRAC: f32 = 0.55; // horizon_y = top_wall_h * HORIZON_FRAC
const ARC_RISE_FRAC: f32 = 0.80; // apex lifts top_wall_h * ARC_RISE_FRAC above horizon
/// Below this atmo `disc` visibility, thick cloud swallows the disc entirely.
pub(super) const MIN_DISC_VIS: f32 = 0.08;

/// This frame's disc placement, or `None` under thick cloud.
pub(super) fn compute_disc(sky: &Sky, buf_w: u16, top_wall_h: u16, theme: &Theme) -> Option<Disc> {
    let e = sky.emitter();
    let vis = sky.atmo().disc;
    if vis < MIN_DISC_VIS {
        return None;
    }
    // Sweep across [first pane, last pane] inset by the radius. NOT the pane
    // CENTERS (bit-identical to the mullion columns, so the span bisected the
    // disc at its most visible low-altitude moment and froze `cx` there on a
    // single-window buffer) and NOT a linear `buf_w - WINDOW_W` bound (that only
    // coincidentally lands inside a window). An empty buffer falls back to the
    // first pane's nominal right edge.
    let last_window_right = window_columns(buf_w, None)
        .last()
        .map_or(FIRST_WINDOW_X + WINDOW_W as f32, |w| {
            (w.x_left + WINDOW_W) as f32
        });
    let span_left = FIRST_WINDOW_X + DISC_RADIUS_PX;
    let span_right = (last_window_right - DISC_RADIUS_PX).max(span_left);
    let cx = span_left + e.azimuth * (span_right - span_left);
    let horizon_y = top_wall_h as f32 * HORIZON_FRAC;
    let cy = horizon_y - e.altitude * (top_wall_h as f32 * ARC_RISE_FRAC);
    // glow reuses the SAME hue as core — the soft halo is a lower-alpha ring
    // of the same color, so each theme's disc reads as one coherent body.
    let (core, glow) = match e.body {
        Body::Sun => (theme.lighting.sun_core, theme.lighting.sun_core),
        Body::Moon => (theme.lighting.moon_core, theme.lighting.moon_core),
    };
    let (lit_frac, lit_right) = match e.body {
        Body::Sun => (1.0, true),
        Body::Moon => (sky.moon_phase(), sky.moon_waxing()),
    };
    Some(Disc {
        cx,
        cy,
        r: DISC_RADIUS_PX,
        core,
        glow,
        vis,
        lit_frac,
        lit_right,
    })
}

/// Roughly 1-in-`STAR_SPARSITY` sky pixels host a star — prime so the
/// hash-modulo grid can't line up into a visible lattice.
const STAR_SPARSITY: u64 = 47;
/// Stars stay in the top fraction of the glass, clear of any building
/// silhouette: `paint_floor_to_ceiling_window`'s `max_bh` tops out at 50% of
/// `glass_h`, so 0.45 leaves comfortable margin above the tallest roofline.
pub(super) const STAR_SKY_BAND_FRAC: f32 = 0.45;
pub(super) const STAR_COLOR: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Cap on the star blend alpha — a faint glimmer, not a bright dot.
pub(super) const STAR_ALPHA_MAX: f32 = 0.55;
/// Per-star twinkle cycle length range (ms), hashed per position so the field
/// doesn't blink in unison.
const STAR_TWINKLE_CYCLE_BASE_MS: u64 = 2000;
const STAR_TWINKLE_CYCLE_SPAN_MS: u64 = 3000;

/// Deterministic sparse star field, hashed on the ABSOLUTE buffer `(px, py)`
/// so it reads as one continuous sky rather than a per-window reseed.
pub(super) fn star_exists(px: u16, py: u16) -> bool {
    let mut h = (px as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= (py as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    h = (h ^ (h >> 17)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h.is_multiple_of(STAR_SPARSITY)
}

/// Per-star twinkle: a hashed per-star cycle length, rerolled on/off each cycle.
pub(super) fn star_twinkle(px: u16, py: u16, now: SystemTime) -> bool {
    let now_ms = epoch_ms(now);
    let seed = (px as u64).wrapping_mul(131) ^ (py as u64).wrapping_mul(521);
    let cycle_ms = STAR_TWINKLE_CYCLE_BASE_MS + (seed % STAR_TWINKLE_CYCLE_SPAN_MS);
    let phase = now_ms / cycle_ms;
    let hash = seed.wrapping_add(phase).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash % 10) < 7
}
