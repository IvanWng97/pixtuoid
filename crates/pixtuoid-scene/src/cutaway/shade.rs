//! The cutaway profile's shading vocabulary.
//!
//! Ratified visually before it was written. The first mock pass was flat fills
//! and read as a diagram; adding exactly two things — a three-tone ramp per
//! material under one key light from the north windows, and an ordered dither
//! for the floor falloff — is what made it read as a room. Those two are what
//! this module is.
//!
//! Neither blends: "lit" is a color of its own and a gradient is a dither — the
//! pixel-art convention the room is drawn in.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::render_scale::RenderScale;

/// How many [`Rgb::ramp`] levels a [`Ramp`]'s lit face sits above its material.
///
/// One value for every material: the room reads as lit from a single direction
/// because nothing gets its own exposure. Tuned on the ratified visual mock.
const RAMP_LIT_LEVEL: i8 = 3;
/// Shade counterpart of [`RAMP_LIT_LEVEL`], deliberately deeper — a surface
/// turning away from the only light loses more than a facing one gains.
const RAMP_SHADE_LEVEL: i8 = -4;

/// A material's three tones under the cutaway's single key light.
///
/// Every shaded mass carries one, so each reads as lit from the same direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ramp {
    /// The face turned toward the key light — the mass's north edge.
    pub lit: Rgb,
    /// The material's own color, covering its body.
    pub base: Rgb,
    /// The face turned away — the mass's south edge, where it meets what's below.
    pub shade: Rgb,
}

impl Ramp {
    /// Derive a ramp from one base color, [`RAMP_LIT_LEVEL`] and
    /// [`RAMP_SHADE_LEVEL`] steps along its [`Rgb::ramp`] — the hue-shifted ramp a
    /// pack's `[ramps]` shades use, so the room and the art are lit by one rule.
    ///
    /// This is why the cutaway needs no theme edits: every theme gains a
    /// lit/shade pair for free, and a NEW theme cannot ship half-shaded. Adding
    /// two explicit roles per material to `Theme` instead would mean hand-picking
    /// both for every material in every theme, and every one a chance to drift
    /// from the base it belongs to.
    pub(crate) fn from_base(base: Rgb) -> Self {
        Self {
            lit: base.ramp(RAMP_LIT_LEVEL),
            base,
            shade: base.ramp(RAMP_SHADE_LEVEL),
        }
    }
}

/// Fill a rect as a top-lit mass: one LOGICAL `lit` row at the top, one
/// `shade` row at the bottom, `base` between.
///
/// The edges are `scale` buffer pixels thick, not one: every caller passes a
/// height already multiplied by the scale, and a fixed 1px edge would thin to a
/// hairline as the render gets denser, the inverse of what the render-scale
/// seam exists for (richer art, not finer detail).
///
/// A mass shorter than two logical rows is painted `base` only: its top and
/// bottom edges would be the same pixels, and either tone would make a thin
/// detail read as a highlight or a shadow rather than as the material.
pub(crate) fn slab(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    ramp: &Ramp,
    scale: RenderScale,
) {
    if w == 0 || h == 0 {
        return;
    }
    fill(buf, x, y, w, h, ramp.base);
    let edge = scale.get();
    if h >= edge.saturating_mul(2) {
        fill(buf, x, y, w, edge, ramp.lit);
        fill(buf, x, y + h - edge, w, edge, ramp.shade);
    }
}

/// Paint a solid rect, clipped to the buffer.
pub(crate) fn fill(buf: &mut RgbBuffer, x: u16, y: u16, w: u16, h: u16, c: Rgb) {
    for dy in 0..h {
        let py = y.saturating_add(dy);
        if py >= buf.height() {
            break;
        }
        for dx in 0..w {
            let px = x.saturating_add(dx);
            if px >= buf.width() {
                break;
            }
            buf.put(px, py, c);
        }
    }
}

/// The 4x4 ordered (Bayer) threshold matrix.
///
/// Its 16 evenly-spread levels are why a dither reads as a smooth ramp rather
/// than as noise or as banding — the classic pixel-art answer, and the reason
/// the floor falloff needs two tones instead of a dozen.
const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Dither a horizontal band from `light` at its top to `dark` at its bottom.
///
/// `y1` is exclusive; a band with no height paints nothing.
///
/// One matrix cell is one LOGICAL pixel — `scale` buffer pixels square. A fixed
/// 1px cell would make the pattern finer as the render gets denser, turning a
/// bold checker matching the art's granularity into a sub-sprite stipple: dirt
/// on a deliberately chunky office, and the class most likely to moiré once a
/// terminal composites the image.
///
/// The indices stay ABSOLUTE (`/ cell % 4`, not relative to `y0`) so the
/// pattern tiles seamlessly across every band and object that shares the
/// buffer, which is what stops a seam appearing at each boundary.
pub(crate) fn dither_band(
    buf: &mut RgbBuffer,
    y0: u16,
    y1: u16,
    dark: Rgb,
    light: Rgb,
    scale: RenderScale,
) {
    if y1 <= y0 {
        return;
    }
    let cell = scale.get();
    let span = u32::from(y1 - y0);
    for y in y0..y1.min(buf.height()) {
        // How far through the transition this row sits, on the matrix's scale.
        let level = (u32::from(y - y0) * 16 / span) as u8;
        for x in 0..buf.width() {
            let threshold = BAYER_4X4[usize::from((y / cell) % 4)][usize::from((x / cell) % 4)];
            buf.put(x, y, if threshold < level { dark } else { light });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIT: Rgb = Rgb {
        r: 200,
        g: 200,
        b: 200,
    };
    const BASE: Rgb = Rgb {
        r: 100,
        g: 100,
        b: 100,
    };
    const SHADE: Rgb = Rgb {
        r: 40,
        g: 40,
        b: 40,
    };
    const BG: Rgb = Rgb { r: 1, g: 2, b: 3 };

    fn ramp() -> Ramp {
        Ramp {
            lit: LIT,
            base: BASE,
            shade: SHADE,
        }
    }

    #[test]
    fn a_slab_is_lit_on_top_and_shaded_at_its_base() {
        let mut buf = RgbBuffer::filled(8, 8, BG);
        slab(&mut buf, 1, 1, 4, 4, &ramp(), RenderScale::ONE);
        assert_eq!(buf.get(1, 1), LIT, "top row catches the key light");
        assert_eq!(buf.get(1, 2), BASE, "the body is the material itself");
        assert_eq!(buf.get(1, 3), BASE);
        assert_eq!(buf.get(1, 4), SHADE, "the base row turns away");
        assert_eq!(buf.get(0, 1), BG, "nothing painted outside the rect");
        assert_eq!(buf.get(5, 1), BG);
    }

    /// Pins [`Ramp::from_base`] to the pack's own ramp, so a cutaway shade and a
    /// sprite's `[ramps]` shade move a colour by one rule.
    #[test]
    fn a_derived_ramp_is_the_packs_hue_shifted_ramp() {
        let c = Rgb {
            r: 90,
            g: 20,
            b: 60,
        };
        assert_eq!(
            Ramp::from_base(c),
            Ramp {
                lit: c.ramp(RAMP_LIT_LEVEL),
                base: c,
                shade: c.ramp(RAMP_SHADE_LEVEL),
            }
        );
    }

    #[test]
    fn a_derived_ramp_brackets_its_base_and_stays_its_material() {
        // A saturated blue: the lit tone must stay blue-dominant, not wash
        // toward white and lose the material.
        let blue = Rgb {
            r: 40,
            g: 70,
            b: 200,
        };
        let luma = |c: Rgb| u32::from(c.r) * 299 + u32::from(c.g) * 587 + u32::from(c.b) * 114;
        let r = Ramp::from_base(blue);
        assert_eq!(r.base, blue, "the base is the theme's own color");
        assert!(
            luma(r.lit) > luma(blue) && luma(r.shade) < luma(blue),
            "{r:?}"
        );
        assert!(
            r.lit.b > r.lit.r && r.lit.b > r.lit.g,
            "the lit tone is still blue, not washed toward white: {:?}",
            r.lit
        );
    }

    #[test]
    fn a_one_row_slab_is_all_base() {
        // Top and bottom are the same pixels here, so either tone would make a
        // 1px detail read as a highlight or a shadow instead of as the material.
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 0, 0, 4, 1, &ramp(), RenderScale::ONE);
        for x in 0..4 {
            assert_eq!(buf.get(x, 0), BASE);
        }
    }

    /// Pins [`slab`]'s edge rule.
    #[test]
    fn a_slabs_edges_are_one_logical_row_at_every_scale() {
        for n in [1u16, 2, 4, 8, 16] {
            let scale = RenderScale::new(n).expect("nonzero");
            let h = 6 * n;
            let mut buf = RgbBuffer::filled(4, h + 4, BG);
            slab(&mut buf, 0, 0, 4, h, &ramp(), scale);

            let lit_rows = (0..h).filter(|&y| buf.get(0, y) == LIT).count();
            let shade_rows = (0..h).filter(|&y| buf.get(0, y) == SHADE).count();
            assert_eq!(
                lit_rows,
                usize::from(n),
                "scale {n}: lit edge is 1 logical row"
            );
            assert_eq!(
                shade_rows,
                usize::from(n),
                "scale {n}: shade edge is 1 logical row"
            );
            // ...and the edges are still AT the edges, not floating.
            assert_eq!(buf.get(0, 0), LIT, "scale {n}");
            assert_eq!(buf.get(0, h - 1), SHADE, "scale {n}");
        }
    }

    /// Pins [`dither_band`]'s cell size.
    #[test]
    fn a_dither_cell_is_one_logical_pixel_at_every_scale() {
        for n in [1u16, 4, 8] {
            let scale = RenderScale::new(n).expect("nonzero");
            let w = 16 * n;
            let mut buf = RgbBuffer::filled(w, 8 * n, BG);
            dither_band(&mut buf, 0, 8 * n, SHADE, LIT, scale);
            // Within one logical pixel every buffer pixel is the same tone —
            // that is what "the cell scales" means.
            let row = 4 * n;
            for cell in 0..4u16 {
                let x0 = cell * n;
                let first = buf.get(x0, row);
                for dx in 0..n {
                    assert_eq!(
                        buf.get(x0 + dx, row),
                        first,
                        "scale {n}: cell {cell} is not solid across its width"
                    );
                }
            }
        }
    }

    #[test]
    fn a_slab_clips_at_the_buffer_edge_instead_of_wrapping() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 3, 3, 8, 8, &ramp(), RenderScale::ONE);
        // The one in-bounds pixel is the slab's OWN top row, so it is lit — the
        // shade row sits at y+h-1, far outside, and is dropped rather than
        // wrapping back into the buffer.
        assert_eq!(buf.get(3, 3), LIT, "the one in-bounds pixel is the lit row");
        assert_eq!(buf.get(0, 0), BG, "nothing wrapped to the origin");
    }

    #[test]
    fn an_empty_slab_paints_nothing() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 0, 0, 0, 4, &ramp(), RenderScale::ONE);
        slab(&mut buf, 0, 0, 4, 0, &ramp(), RenderScale::ONE);
        assert!(buf.as_slice().iter().all(|&c| c == BG));
    }

    #[test]
    fn a_dither_band_runs_light_at_the_top_to_dark_at_the_bottom() {
        let mut buf = RgbBuffer::filled(16, 32, BG);
        dither_band(&mut buf, 0, 32, SHADE, LIT, RenderScale::ONE);

        let dark_in = |y0: u16, y1: u16| {
            (y0..y1)
                .flat_map(|y| (0..16).map(move |x| (x, y)))
                .filter(|&(x, y)| buf.get(x, y) == SHADE)
                .count()
        };
        let (top, bottom) = (dark_in(0, 4), dark_in(28, 32));
        assert_eq!(top, 0, "the first rows are entirely the light tone");
        assert!(
            bottom > top,
            "the dark tone must dominate by the bottom (top {top}, bottom {bottom})"
        );
        // Monotone: each quarter is at least as dark as the one above it.
        let quarters: Vec<usize> = (0..4).map(|q| dark_in(q * 8, q * 8 + 8)).collect();
        assert!(
            quarters.windows(2).all(|w| w[1] >= w[0]),
            "the ramp must not reverse: {quarters:?}"
        );
    }

    #[test]
    fn a_dither_band_uses_only_its_two_tones() {
        let mut buf = RgbBuffer::filled(8, 8, BG);
        dither_band(&mut buf, 0, 8, SHADE, LIT, RenderScale::ONE);
        assert!(buf.as_slice().iter().all(|&c| c == SHADE || c == LIT));
    }

    #[test]
    fn an_inverted_or_empty_band_paints_nothing() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        dither_band(&mut buf, 3, 3, SHADE, LIT, RenderScale::ONE);
        dither_band(&mut buf, 3, 1, SHADE, LIT, RenderScale::ONE);
        assert!(buf.as_slice().iter().all(|&c| c == BG));
    }
}
