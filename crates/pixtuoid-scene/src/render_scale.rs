//! The seam between LAYOUT space and BUFFER space.
//!
//! Every layout coordinate is a buffer pixel today, so the office's SIZE and
//! its RESOLUTION are one axis: doubling the buffer does not draw the same room
//! sharper, it builds a room with several times the desks, which is why "render
//! the same office with more detail" is currently unexpressible. Pinned by
//! `floor_capacity_is_invariant_under_render_scale` and its negative control
//! `without_the_seam_a_bigger_buffer_builds_a_bigger_office`.
//!
//! [`RenderScale`] splits the axis. Layout keeps computing in logical units, so
//! a floor's capacity, desk assignment and walkable mask are untouched; the
//! painter multiplies by the scale on its way to pixels. A richer visual
//! profile then buys detail per object rather than more objects.

use std::num::NonZeroU16;

use pixtuoid_core::sprite::format::{density_variant_name_into, variant_redraws, Pack};
use pixtuoid_core::sprite::{Frame, RecolorableFrame};

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

    /// The densest scale that `available` pixels allow AND the pack's art can
    /// land on — rounded DOWN to a multiple of `max_density`.
    ///
    /// A density variant is only usable at a scale its density divides, so the
    /// two facts have to meet somewhere. They meet HERE, in the engine, because
    /// `max_density` is a property of the PACK: every painter that picks a scale
    /// needs this rule, and only one of them is a terminal. Putting it in the
    /// terminal's capability module (where it was first written) would make the
    /// window and canvas painters re-derive it.
    ///
    /// Found on a real Retina Ghostty: a 17px cell makes 17 the natural scale,
    /// 17 is PRIME, and every `@4x` sprite in the pack sits unused while the
    /// base art block-scales 17x. Giving up at most `max_density - 1` px of
    /// office (17 -> 16, ~6%) is not a close call — not being upscaled is the
    /// whole point of the variants.
    ///
    /// A pack with no variants passes `max_density = 1`, so this is exactly
    /// [`RenderScale::new`] there and the classic path is untouched.
    pub fn fit(available: u16, max_density: u16) -> Option<Self> {
        let usable = if max_density >= 2 && available >= max_density {
            (available / max_density) * max_density
        } else {
            available
        };
        Self::new(usable)
    }

    /// Buffer pixels per layout unit.
    pub fn get(self) -> u16 {
        self.0.get()
    }

    /// The factor as a [`NonZeroU16`], for the core blitter.
    ///
    /// `pixtuoid-core` owns the pixel primitives but sits BELOW this crate in
    /// the DAG, so it cannot name `RenderScale`; it takes the bare non-zero
    /// factor and this is the one conversion. Core gets "repeat each pixel N
    /// times", the layout↔buffer meaning stays here.
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

impl Default for RenderScale {
    fn default() -> Self {
        Self::ONE
    }
}

/// One frame of art as a renderer at some [`RenderScale`] should draw it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DenseFrame<'a> {
    /// The art in the pack's own colours.
    pub(crate) frame: &'a Frame,
    /// The same art as palette indices, for a per-agent recolor.
    pub(crate) recolorable: RecolorableFrame<'a>,
    /// The grid the art is authored on: 1 for the base, `N` for `<name>@<N>x`.
    pub(crate) density: NonZeroU16,
    /// The factor still left to blit at: the scale divided by `density`.
    pub(crate) blit_at: NonZeroU16,
}

/// Frame `frame_idx` of `name` from the densest `<name>@<N>x` variant whose `N`
/// divides `scale`, else the base art. `frame_idx` resolves through the classic
/// painter's own `frame_index`.
///
/// A variant is the SAME art on an N-times grid, so it is taken only when it
/// can honour that: its density divides the scale (4x art still halves the
/// upscale at 8x rather than being discarded), and it redraws its base whole
/// ([`variant_redraws`]), so an animation never mixes densities mid-cycle. A
/// variant failing either is SKIPPED rather than drawn wrong —
/// `validate_pack_animations` reports each as a hard error, so this is the
/// render-time backstop for a pack that was never validated.
///
/// A pack with no variants renders exactly as it did before they existed, so
/// richer art lands one piece at a time rather than as a flag day.
pub(crate) fn densest_frame<'a>(
    pack: &'a Pack,
    name: &str,
    frame_idx: usize,
    scale: RenderScale,
) -> Option<DenseFrame<'a>> {
    let base_anim = pack.animation(name)?;
    let idx = crate::pixel_painter::frame_index(base_anim, frame_idx);
    let base = base_anim.frames().get(idx)?;
    let s = scale.get();
    // ONE buffer, reused: this runs per divisor, per piece, per frame, and a
    // fresh `String` each time is an allocation for a map probe that drops it.
    let mut key = String::with_capacity(name.len() + 4);
    for density in (2..=s).rev() {
        if !s.is_multiple_of(density) {
            continue;
        }
        key.clear();
        density_variant_name_into(&mut key, name, density);
        let Some(anim) = pack.animation(&key) else {
            continue;
        };
        if !variant_redraws(base_anim, density, anim) {
            continue;
        }
        let (Some(art), Some(recolorable)) = (anim.frames().get(idx), anim.recolorable(idx)) else {
            continue;
        };
        // `density` divides `s` and both are >= 2 here, so both are nonzero.
        if let (Some(density), Some(blit_at)) =
            (NonZeroU16::new(density), NonZeroU16::new(s / density))
        {
            return Some(DenseFrame {
                frame: art,
                recolorable,
                density,
                blit_at,
            });
        }
    }
    Some(DenseFrame {
        frame: base,
        recolorable: base_anim.recolorable(idx)?,
        density: NonZeroU16::MIN,
        blit_at: scale.factor(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::{floor_capacity, floor_capacity_scaled, floor_seed};

    /// `typing@4x` claims 4x but is drawn at 2x; `walking@2x` has one frame
    /// against its base's two; `seated@2x`'s second frame is drawn at 3x.
    fn variant_pack() -> Pack {
        pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\"B\"=\"#040506\"\n\
             [animations.typing]\nframes=[\"a1.sprite\", \"b1.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@2x\"]\nframes=[\"a2.sprite\", \"b2.sprite\"]\nframe_ms=100\n\
             [animations.\"typing@4x\"]\nframes=[\"a2.sprite\", \"b2.sprite\"]\nframe_ms=100\n\
             [animations.walking]\nframes=[\"a1.sprite\", \"b1.sprite\"]\nframe_ms=100\n\
             [animations.\"walking@2x\"]\nframes=[\"a2.sprite\"]\nframe_ms=100\n\
             [animations.seated]\nframes=[\"a1.sprite\", \"b1.sprite\"]\nframe_ms=100\n\
             [animations.\"seated@2x\"]\nframes=[\"a2.sprite\", \"c3.sprite\"]\nframe_ms=100\n",
            &[
                ("a1.sprite", "@frame 0\nA"),
                ("b1.sprite", "@frame 0\nB"),
                ("a2.sprite", "@frame 0\nA A\nA A"),
                ("b2.sprite", "@frame 0\nB B\nB B"),
                ("c3.sprite", "@frame 0\nB B B\nB B B\nB B B"),
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
    fn the_densest_honest_variant_wins_and_blits_the_remainder() {
        let pack = variant_pack();
        assert_eq!(
            at(&pack, "typing", 0, 4),
            (2, 2, 2),
            "4x lies, so 4 falls to 2"
        );
        assert_eq!(at(&pack, "typing", 0, 2), (2, 2, 1));
        assert_eq!(at(&pack, "typing", 0, 3), (1, 1, 3), "2 does not divide 3");
        assert_eq!(at(&pack, "typing", 0, 1), (1, 1, 1));
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

    /// Frame 0 exists and fits in `walking@2x`, so only its frame count can
    /// reject it.
    #[test]
    fn a_variant_with_the_wrong_frame_count_is_skipped() {
        let pack = variant_pack();
        assert_eq!(at(&pack, "walking", 0, 2), (1, 1, 2));
        assert_eq!(at(&pack, "walking", 1, 2), (1, 1, 2));
    }

    /// Past the end reads the first frame, like the classic painter.
    /// Frame 0 of `seated@2x` fits; its frame 1 does not, so the variant is
    /// skipped at every frame rather than drawn at 2x for half the cycle.
    #[test]
    fn a_variant_with_one_wrong_frame_is_skipped_whole() {
        let pack = variant_pack();
        assert_eq!(at(&pack, "seated", 0, 2), (1, 1, 2));
        assert_eq!(at(&pack, "seated", 1, 2), (1, 1, 2));
    }

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

    #[test]
    fn an_animation_the_pack_lacks_has_no_art() {
        assert!(densest_frame(&variant_pack(), "standing", 0, RenderScale::ONE).is_none());
    }

    /// A pack ships art at ONE density; a painter picks a scale from its own
    /// surface. `fit` is where those meet, and the rule is that the art must
    /// divide the scale or it is simply never drawn.
    #[test]
    fn a_scale_rounds_down_so_the_packs_densest_art_can_land_on_it() {
        assert_eq!(RenderScale::fit(17, 4).map(|s| s.get()), Some(16));
        // A pack with no variants must be untouched — this rule may only ever
        // COST office area when there is richer art to spend it on.
        assert_eq!(RenderScale::fit(17, 1).map(|s| s.get()), Some(17));
        // Already a multiple: nothing to give up.
        assert_eq!(RenderScale::fit(16, 4).map(|s| s.get()), Some(16));
    }

    /// Rounding must never round the office away.
    #[test]
    fn art_denser_than_the_whole_scale_does_not_round_to_nothing() {
        // 4x art at a 3px scale can never land however we round, so the honest
        // answer is the unrounded scale (the art is skipped), NOT zero.
        assert_eq!(RenderScale::fit(3, 4).map(|s| s.get()), Some(3));
        assert_eq!(
            RenderScale::fit(0, 4),
            None,
            "zero pixels is still no scale"
        );
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

    /// The invariant the whole seam exists for, and the spec's acceptance
    /// criterion #10: raising render fidelity must not change the office.
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
