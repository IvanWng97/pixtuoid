//! The art pixel, the grid every look places and paints on: sprites are
//! authored `d` art pixels per logical unit, and a render at scale `s` blits
//! each `s / d` buffer pixels square, so a frame mixes no pixel sizes. Painting
//! through a [`Pen`] holds that, because a pen can't address a buffer pixel.

use std::num::NonZeroU16;

use pixtuoid_core::sprite::format::Pack;

use crate::render_scale::RenderScale;

/// A length or coordinate on the art grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ArtPx(pub(crate) u16);

/// A rect on the art grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtRect {
    pub(crate) x: ArtPx,
    pub(crate) y: ArtPx,
    pub(crate) w: ArtPx,
    pub(crate) h: ArtPx,
}

/// A render's art grid (the module doc): `k` buffer pixels make one
/// art pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Pen {
    d: NonZeroU16,
    k: NonZeroU16,
}

impl Pen {
    /// The classic painter's grid: one buffer pixel per logical unit.
    pub(crate) const UNIT: Self = Self {
        d: NonZeroU16::MIN,
        k: NonZeroU16::MIN,
    };

    /// The pen for art authored at density `d`, painted at `scale`; `None` when
    /// `d` does not divide it, since an art pixel would then straddle buffer
    /// pixels.
    pub(crate) fn new(scale: RenderScale, d: u16) -> Option<Self> {
        let d = NonZeroU16::new(d)?;
        let s = scale.get();
        if !s.is_multiple_of(d.get()) {
            return None;
        }
        Some(Self {
            d,
            k: NonZeroU16::new(s / d.get())?,
        })
    }

    /// The pen for `pack` at `scale`: the densest of its variant densities that
    /// divides `scale`, else the base art's. [`densest_frame`](crate::pack::densest_frame)
    /// applies the same rule per piece, so the room shares every piece's grid
    /// only while the pack draws its variants at one density
    /// (`the_bundled_pack_draws_every_variant_at_one_density`).
    pub(crate) fn for_pack(scale: RenderScale, pack: &Pack) -> Self {
        pack.density_variants()
            .into_iter()
            .find_map(|d| Self::new(scale, d.get()))
            .unwrap_or(Self {
                d: NonZeroU16::MIN,
                k: scale.factor(),
            })
    }

    /// `logical` layout units, as art pixels: the one conversion from the
    /// layout's units onto the grid.
    pub(crate) fn art(self, logical: u16) -> ArtPx {
        ArtPx(logical.saturating_mul(self.d.get()))
    }

    /// The logical unit art pixel `a` lies in.
    pub(crate) fn logical(self, a: ArtPx) -> u16 {
        a.0 / self.d.get()
    }

    /// `a` art pixels, as buffer pixels.
    pub(crate) fn buffer(self, a: ArtPx) -> u16 {
        a.0.saturating_mul(self.k.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "density-art")]
    fn the_bundled_pack_draws_every_variant_at_one_density() {
        let pack = crate::pack::test_default_pack();
        assert_eq!(
            pack.density_variants().len(),
            1,
            "{:?}",
            pack.density_variants()
        );
    }

    fn pen(s: u16, d: u16) -> Pen {
        Pen::new(RenderScale::new(s).expect("nonzero"), d).expect("d divides s")
    }

    #[test]
    fn a_pen_needs_its_density_to_divide_the_scale() {
        let s = RenderScale::new(8).expect("nonzero");
        assert_eq!(Pen::new(s, 4).map(|p| p.k.get()), Some(2));
        assert_eq!(
            Pen::new(s, 3),
            None,
            "an art pixel would straddle buffer pixels"
        );
        assert_eq!(Pen::new(s, 0), None);
    }

    #[test]
    fn a_pens_density_is_the_densest_the_pack_draws_at_that_scale() {
        let pack = crate::pack::test_default_pack();
        let d = pack.max_density_variant().get();
        let at = |s: u16| Pen::for_pack(RenderScale::new(s).expect("nonzero"), &pack);
        assert_eq!(at(d * 2), pen(d * 2, d), "the variant's grid");
        assert_eq!(at(1), pen(1, 1), "no variant fits: the base art's grid");
    }
}
