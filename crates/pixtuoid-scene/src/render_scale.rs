//! The seam between LAYOUT space and BUFFER space.
//!
//! Every layout coordinate is a buffer pixel today, so the office's SIZE and
//! its RESOLUTION are one axis: doubling the buffer does not draw the same room
//! sharper, it builds a room with four times the desks. Measured on the real
//! layout — 25 desks at 192x160, 1554 at 1536x1280 — which is why "render the
//! same office with more detail" is currently unexpressible.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::{floor_capacity, floor_capacity_scaled, floor_seed};

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
