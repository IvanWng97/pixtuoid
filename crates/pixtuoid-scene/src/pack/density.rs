//! The density-variant picker: `densest_frame` chooses which art a painter at a
//! render scale draws a piece from — the base sprite, or a `<name>@<N>x`
//! variant of it — and how much upscale is left to blit it at.

use std::num::NonZeroU16;

#[cfg(doc)]
use pixtuoid_core::sprite::format::variant_redraws;
use pixtuoid_core::sprite::format::{Density, Pack, density_variant_name_into};
use pixtuoid_core::sprite::{Frame, RecolorableFrame};

use crate::render_scale::RenderScale;

/// One frame of art as a renderer at some [`RenderScale`] should draw it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DenseFrame<'a> {
    /// The art in the pack's own colours.
    pub(crate) frame: &'a Frame,
    /// The same art as palette indices, for a per-agent recolor.
    pub(crate) recolorable: RecolorableFrame<'a>,
    /// The piece's size in layout units: its base frame's, since a variant
    /// redraws that frame's box on a finer grid ([`variant_redraws`]).
    pub(crate) logical: (u16, u16),
    /// The grid the art is authored on: 1 for the base, `N` for `<name>@<N>x`.
    pub(crate) density: Density,
    /// The factor still left to blit at: the scale divided by `density`.
    pub(crate) blit_at: NonZeroU16,
    /// Where the frame's head is, if the art marks it: where a hairstyle
    /// dresses it ([`crate::character::dress_for`]).
    pub(crate) head: Option<pixtuoid_core::sprite::HeadMark>,
    /// Every point the art marks, on its own grid.
    pub(crate) marks: &'a [pixtuoid_core::sprite::Mark],
}

/// Frame `frame_idx` of `name` from the densest `<name>@<N>x` variant whose `N`
/// divides `scale`, else the base art. `frame_idx` resolves through the classic
/// painter's own `frame_index`.
///
/// A variant is the same PIECE on an N-times grid — the base's logical box
/// drawn finer — so it is taken only at a scale its density divides (4x art at
/// 8x blits at 2 rather than being discarded for not matching). Validation
/// holds each to redrawing its base whole ([`variant_redraws`]), so an
/// animation never mixes densities mid-cycle.
///
/// A piece with no variant renders exactly as it did before variants existed,
/// so richer art lands one piece at a time rather than as a flag day.
pub(crate) fn densest_frame<'a>(
    pack: &'a Pack,
    name: &str,
    frame_idx: usize,
    scale: RenderScale,
) -> Option<DenseFrame<'a>> {
    let base_anim = pack.animation(name)?;
    let idx = super::lookup::frame_index(base_anim, frame_idx);
    let base = base_anim.frames().get(idx)?;
    let logical = (base.width(), base.height());
    let s = scale.get();
    // ONE buffer, reused across the divisor probes; it allocates on the first
    // probe, so a scale with no divisor allocates nothing.
    let mut key = String::new();
    for density in (2..=s).rev().filter_map(Density::new) {
        if !s.is_multiple_of(density.get()) {
            continue;
        }
        key.clear();
        density_variant_name_into(&mut key, name, density);
        let Some(anim) = pack.animation(&key) else {
            continue;
        };
        let (Some(art), Some(recolorable)) = (anim.frames().get(idx), anim.recolorable(idx)) else {
            continue;
        };
        // `density` divides `s`, so the quotient is nonzero.
        if let Some(blit_at) = NonZeroU16::new(s / density.get()) {
            return Some(DenseFrame {
                frame: art,
                recolorable,
                logical,
                density,
                blit_at,
                head: anim.head(idx),
                marks: anim.marks(idx),
            });
        }
    }
    Some(DenseFrame {
        frame: base,
        recolorable: base_anim.recolorable(idx)?,
        logical,
        density: Density::ONE,
        blit_at: scale.factor(),
        head: base_anim.head(idx),
        marks: base_anim.marks(idx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `typing` has a variant at 2x, `holding_coffee` at both 2x and 4x; `desk`
    /// has none.
    fn variant_pack() -> Pack {
        pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\"B\"=\"#040506\"\n\
             [animations.typing]\nframes=[\"a1.sprite\", \"b1.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"a2.sprite\", \"b2.sprite\"]\nframe_ms=100\n\
             [animations.holding_coffee]\nframes=[\"a1.sprite\"]\nframe_ms=100\n\
             [animations.\"holding_coffee@2x\"]\nframes=[\"a2.sprite\"]\nframe_ms=100\n\
             [animations.\"holding_coffee@4x\"]\nframes=[\"a4.sprite\"]\nframe_ms=100\n\
             [animations.desk]\nframes=[\"a1.sprite\"]\nframe_ms=100\n",
            &[
                ("a1.sprite", "@frame 0\nA"),
                ("b1.sprite", "@frame 0\nB"),
                ("a2.sprite", "@frame 0\nA A\nA A"),
                ("b2.sprite", "@frame 0\nB B\nB B"),
                ("a4.sprite", "@frame 0\nA A A A\nA A A A\nA A A A\nA A A A"),
            ],
        )
        .expect("pack builds")
    }

    /// (width, density, blit_at) of `name`'s frame `idx` at scale `s`.
    fn at(pack: &Pack, name: &str, idx: usize, s: u16) -> (u16, u16, u16) {
        let d = densest_frame(pack, name, idx, RenderScale::new(s).expect("nonzero")).expect("art");
        (d.frame.width(), d.density.get(), d.blit_at.get())
    }

    #[test]
    fn the_densest_dividing_variant_wins_and_blits_the_remainder() {
        let pack = variant_pack();
        assert_eq!(
            at(&pack, "typing", 0, 4),
            (2, 2, 2),
            "no 4x, so 4 falls to 2"
        );
        assert_eq!(at(&pack, "typing", 0, 2), (2, 2, 1));
        assert_eq!(at(&pack, "typing", 0, 3), (1, 1, 3), "2 does not divide 3");
        assert_eq!(at(&pack, "typing", 0, 1), (1, 1, 1));
    }

    /// Two variants that both divide the scale: the DENSER one wins, and
    /// blits whatever upscale is left.
    #[test]
    fn the_denser_of_two_dividing_variants_wins() {
        let pack = variant_pack();
        assert_eq!(at(&pack, "holding_coffee", 0, 4), (4, 4, 1));
        assert_eq!(at(&pack, "holding_coffee", 0, 8), (4, 4, 2));
        assert_eq!(
            at(&pack, "holding_coffee", 0, 6),
            (2, 2, 3),
            "4 does not divide 6"
        );
    }

    /// The layout size is the base's at every density.
    #[test]
    fn a_variant_keeps_its_bases_logical_size() {
        let pack = variant_pack();
        for s in [1, 2, 4, 8] {
            let d = densest_frame(
                &pack,
                "holding_coffee",
                0,
                RenderScale::new(s).expect("nonzero"),
            )
            .expect("art");
            assert_eq!(d.logical, (1, 1), "scale {s}");
        }
    }

    /// A variant is indexed frame for frame, so frame 1 of the base must come
    /// out as frame 1 of the variant, not whatever the variant starts on.
    #[test]
    fn a_variant_is_read_at_the_same_frame_as_its_base() {
        let pack = variant_pack();
        let b = pixtuoid_core::sprite::Rgb { r: 4, g: 5, b: 6 };
        let d =
            densest_frame(&pack, "typing", 1, RenderScale::new(2).expect("nonzero")).expect("art");
        assert_eq!(d.density.get(), 2);
        assert_eq!(d.frame.get(0, 0).copied().flatten(), Some(b));
        assert_eq!(
            d.recolorable.recolored(&[]).get(0, 0).copied().flatten(),
            Some(b)
        );
    }

    /// Past the end reads the first frame, like the classic painter.
    #[test]
    fn a_frame_past_the_end_reads_the_first() {
        let pack = variant_pack();
        let a = pixtuoid_core::sprite::Rgb { r: 1, g: 2, b: 3 };
        let d =
            densest_frame(&pack, "typing", 9, RenderScale::new(2).expect("nonzero")).expect("art");
        assert_eq!(
            (d.density.get(), d.frame.get(0, 0).copied().flatten()),
            (2, Some(a))
        );
    }

    /// The flip side of one piece at a time: a piece with no variant draws its
    /// base while another's lands at the same scale, or adding one `@Nx` sprite
    /// would be a flag day for all of them.
    #[test]
    fn a_piece_with_no_variant_draws_its_base_while_anothers_lands() {
        let pack = variant_pack();
        assert_eq!(at(&pack, "holding_coffee", 0, 4), (4, 4, 1));
        assert_eq!(at(&pack, "desk", 0, 4), (1, 1, 4));
    }

    #[test]
    fn an_animation_the_pack_lacks_has_no_art() {
        assert!(densest_frame(&variant_pack(), "standing", 0, RenderScale::ONE).is_none());
    }
}
