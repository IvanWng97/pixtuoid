//! The seam between LAYOUT space and BUFFER space.
//!
//! Were every layout coordinate a buffer pixel, the office's SIZE and its
//! RESOLUTION would be one axis: doubling the buffer would not draw the same
//! room sharper, it would build a room with several times the desks, and
//! "render the same office with more detail" would be unexpressible. Pinned by
//! `floor_capacity_is_invariant_under_render_scale` and its negative control
//! `without_the_seam_a_bigger_buffer_builds_a_bigger_office`.
//!
//! [`RenderScale`] splits the axis. Layout keeps computing in logical units, so
//! a floor's capacity, desk assignment and walkable mask are untouched; the
//! painter multiplies by the scale on its way to pixels. A richer visual
//! profile then buys detail per object rather than more objects.

use std::num::NonZeroU16;

/// How many buffer pixels one layout unit paints as.
///
/// `ONE` is the classic path: layout units ARE buffer pixels, byte-identical to
/// the pre-seam behaviour. A larger scale paints the same office into a bigger
/// buffer, which is the only thing that makes richer art expressible without
/// silently changing floor capacity or desk assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderScale(NonZeroU16);

impl RenderScale {
    /// The classic path — one layout unit per buffer pixel.
    pub const ONE: Self = Self(NonZeroU16::MIN);

    /// A scale of `n` buffer pixels per layout unit, or `None` for zero
    /// (a zero scale would divide the office away).
    pub fn new(n: u16) -> Option<Self> {
        NonZeroU16::new(n).map(Self)
    }

    /// The multiple of `max_density` nearest the surface's `natural` scale,
    /// measured by RATIO, or `None` when none lies within the ratio bound
    /// `FIT_MAX_RATIO_SQUARED` sets.
    ///
    /// Only a multiple of the pack's densest art lands every sprite
    /// pixel-exact: a scale the density does not divide falls back to coarser
    /// art, and a painter mixing densities draws a room whose pieces disagree
    /// about what a pixel is. The multiple may lie ABOVE `natural` — a painter
    /// sizes the office from its surface's pixels, so rounding changes how much
    /// office fits, never whether the art is resampled — and ratio, not
    /// difference, is the measure because framing is perceived
    /// multiplicatively: at a natural 12, 8 and 16 are equally far by
    /// difference, but 16 (×4/3) is nearer than 8 (×3/2).
    /// Two adjacent multiples `kd` and `(k+1)d` never tie, since that needs
    /// `k(k+1)` to be a square.
    ///
    /// The rule meets HERE, in the engine, because `max_density` is a property
    /// of the PACK and every painter that picks a scale needs it; the terminal
    /// only contributes `natural`.
    ///
    /// A pack with no variants passes `max_density = 1` (what
    /// `Pack::max_density_variant` reports), so every scale is a multiple and
    /// this is exactly [`RenderScale::new`] there.
    pub fn fit(natural: u16, max_density: u16) -> Option<Self> {
        let d = u32::from(max_density.max(1));
        let n = u32::from(natural);
        let below = n / d * d;
        let above = below + d;
        // `n/below < above/n` ⇔ `n² < below·above`, compared exactly in integers.
        let nearest = if below > 0 && n * n < below * above {
            below
        } else {
            above
        };
        let (lo, hi) = (nearest.min(n), nearest.max(n));
        let within = hi * hi <= FIT_MAX_RATIO_SQUARED * lo * lo;
        if !within {
            return None;
        }
        u16::try_from(nearest).ok().and_then(Self::new)
    }

    /// Buffer pixels per layout unit.
    pub fn get(self) -> u16 {
        self.0.get()
    }

    /// The factor as a [`NonZeroU16`], for the core blitter.
    ///
    /// `pixtuoid-core` owns the pixel primitives but sits BELOW this crate in
    /// the DAG, so it cannot name `RenderScale`; it takes the bare non-zero
    /// factor, which this module and the density-variant picker make. Core gets
    /// "repeat each pixel N times", the layout↔buffer meaning stays here.
    pub fn factor(self) -> NonZeroU16 {
        self.0
    }

    /// The logical extent a buffer of `buf_px` covers. Truncating is deliberate:
    /// a buffer that is not a whole multiple of the scale leaves a sub-unit
    /// remainder that no layout unit could occupy anyway.
    pub fn logical(self, buf_px: u16) -> u16 {
        buf_px / self.0.get()
    }

    /// The buffer pixel a layout unit paints at. Saturates rather than wrapping
    /// so an oversized layout clips at the buffer edge (`blit_frame` already
    /// discards out-of-bounds pixels) instead of aliasing to the top-left.
    pub fn to_buffer(self, logical: u16) -> u16 {
        logical.saturating_mul(self.0.get())
    }
}

/// The square of how far, as a ratio, [`RenderScale::fit`] may move a scale
/// off its natural value — squared so the comparison stays in integers.
///
/// It only bites below the densest art, where `max_density` is the one
/// candidate: a ratio of √2 is a factor of 2 in AREA, so past it the office
/// would keep under half the logical area the surface's natural scale gives
/// it, and the classic profile draws more office than that.
const FIT_MAX_RATIO_SQUARED: u32 = 2;

impl Default for RenderScale {
    fn default() -> Self {
        Self::ONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::{floor_capacity, floor_capacity_scaled, floor_seed};

    fn fit(natural: u16, max_density: u16) -> Option<u16> {
        RenderScale::fit(natural, max_density).map(RenderScale::get)
    }

    /// The bundled pack's case, natural scale by natural scale: every pick is a
    /// multiple of the densest art, rounding goes whichever way is nearer by
    /// ratio, and a cell too small to reach 8 within √2 gets no scale at all.
    #[test]
    fn a_scale_snaps_to_the_multiple_of_the_densest_art_nearest_by_ratio() {
        let want: [(u16, Option<u16>); 12] = [
            (5, None),
            (6, Some(8)),
            (8, Some(8)),
            (11, Some(8)),
            (12, Some(16)),
            (15, Some(16)),
            (17, Some(16)),
            (19, Some(16)),
            (20, Some(24)),
            (27, Some(24)),
            (28, Some(32)),
            (41, Some(40)),
        ];
        for (natural, scale) in want {
            assert_eq!(fit(natural, 8), scale, "natural {natural}");
        }
    }

    /// The properties the table samples, over every natural scale a terminal
    /// could report and every density a pack could ship.
    #[test]
    fn every_pick_is_a_multiple_within_the_ratio_bound_and_the_nearest_one() {
        for d in 1..=16u16 {
            for n in 0..=256u16 {
                let Some(s) = fit(n, d) else {
                    // Only a scale with no multiple of `d` in reach goes without.
                    assert!(
                        u32::from(d) * u32::from(d) > 2 * u32::from(n) * u32::from(n),
                        "natural {n} at density {d} has a multiple in reach"
                    );
                    continue;
                };
                assert_eq!(s % d, 0, "natural {n} at density {d} picked {s}");
                let ratio = |a: u16| f64::from(a.max(n)) / f64::from(a.min(n));
                assert!(ratio(s) <= std::f64::consts::SQRT_2, "{n}@{d} → {s}");
                for other in [s.saturating_sub(d), s + d].into_iter().filter(|&o| o > 0) {
                    assert!(
                        ratio(s) < ratio(other),
                        "{n}@{d}: {other} is nearer than {s}"
                    );
                }
            }
        }
    }

    /// A pack with no variants must be untouched: every scale is a multiple of
    /// 1, so the natural scale comes back as it went in.
    #[test]
    fn a_pack_without_variants_keeps_the_natural_scale() {
        for n in 1..=64 {
            assert_eq!(fit(n, 1), Some(n));
        }
        assert_eq!(fit(0, 1), None, "zero pixels is still no scale");
    }

    #[test]
    fn zero_is_refused_because_it_would_divide_the_office_away() {
        assert!(RenderScale::new(0).is_none());
        assert_eq!(RenderScale::ONE.get(), 1);
    }

    #[test]
    fn logical_and_buffer_round_trip_on_whole_multiples() {
        let s = RenderScale::new(4).expect("4 is nonzero");
        assert_eq!(s.to_buffer(160), 640);
        assert_eq!(s.logical(640), 160);
        // A partial unit at the edge belongs to no layout unit.
        assert_eq!(s.logical(643), 160);
    }

    /// The seam reaches the core blitter unchanged — a scale that says "4" must
    /// expand a sprite by 4, or layout and paint would disagree about what a
    /// unit is while every test above still passed.
    #[test]
    fn the_factor_drives_the_core_blitter_at_the_same_scale() {
        use pixtuoid_core::sprite::blit::blit_frame_scaled;
        use pixtuoid_core::sprite::{Frame, Rgb, RgbBuffer};

        let red = Rgb { r: 255, g: 0, b: 0 };
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let s = RenderScale::new(4).expect("nonzero");
        let f = Frame::from_pixels(1, 1, vec![Some(red)]);
        let mut buf = RgbBuffer::filled(8, 8, bg);
        blit_frame_scaled(&f, 0, 0, s.factor(), &mut buf);

        // One source pixel covers exactly s.get() x s.get() buffer pixels.
        let last = s.get() - 1;
        assert_eq!(buf.get(last, last), red, "the block reaches its far corner");
        assert_eq!(buf.get(s.get(), 0), bg, "and stops there");
    }

    #[test]
    fn to_buffer_saturates_instead_of_wrapping() {
        let s = RenderScale::new(8).expect("8 is nonzero");
        assert_eq!(s.to_buffer(u16::MAX), u16::MAX);
    }

    /// The invariant the whole seam exists for: raising render fidelity must
    /// not change the office.
    #[test]
    fn floor_capacity_is_invariant_under_render_scale() {
        let seed = floor_seed(0);
        let base = floor_capacity_scaled(192, 160, RenderScale::ONE, seed);
        assert!(base > 0, "the 1x baseline must lay out at all");

        for n in [2u16, 4, 8] {
            let s = RenderScale::new(n).expect("nonzero");
            let got = floor_capacity_scaled(s.to_buffer(192), s.to_buffer(160), s, seed);
            assert_eq!(got, base, "render scale {n} changed the desk count");
        }
    }

    /// Negative control for the test above — it must be able to FAIL. Without
    /// the seam the same buffers yield a wildly bigger office, which is exactly
    /// the regression the invariant pins.
    #[test]
    fn without_the_seam_a_bigger_buffer_builds_a_bigger_office() {
        let seed = floor_seed(0);
        let base = floor_capacity(192, 160, seed);
        let unscaled = floor_capacity(192 * 8, 160 * 8, seed);
        assert!(
            unscaled > base * 8,
            "expected the un-seamed call to balloon the desk count \
             (base {base}, 8x buffer {unscaled}) — if this ever stops holding, \
             the invariant test above has lost its teeth"
        );
    }
}
