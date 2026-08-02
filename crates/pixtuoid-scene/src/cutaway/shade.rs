//! The cutaway profile's shading vocabulary.
//!
//! Ratified visually before it was written. The first mock pass was flat fills
//! and read as a diagram; adding exactly two things — a three-tone ramp per
//! material under one key light from the north windows, and an ordered dither
//! for the floor falloff — is what made it read as a room. Those two are what
//! this module is.
//!
//! Both are constrained by the indexed-palette decision: a palette cannot
//! blend, so "lit" is its own colour and a gradient IS a dither. That is not a
//! limitation being worked around — it is what keeps theme recolour an index
//! swap and SIXEL encoding free of a quantisation pass.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

/// A material's three tones under the cutaway's single key light.
///
/// Every surface in the profile carries one. Uniformity is the point: the room
/// reads as lit from one direction only because nothing opts out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ramp {
    /// The face turned toward the key light — the mass's north edge.
    pub lit: Rgb,
    /// The material's own colour, covering its body.
    pub base: Rgb,
    /// The face turned away — the mass's south edge, where it meets what's below.
    pub shade: Rgb,
}

impl Ramp {
    /// A ramp whose three tones are all `c` — an unshaded material.
    ///
    /// For genuinely self-lit surfaces (a screen, an LED) where a key-light
    /// gradient would be wrong, not for skipping the shading pass.
    pub fn flat(c: Rgb) -> Self {
        Self {
            lit: c,
            base: c,
            shade: c,
        }
    }

    /// Derive a ramp from one theme colour by tinting toward white and shading
    /// toward black.
    ///
    /// This is why the cutaway needs no theme edits: all six existing palettes
    /// gain a lit/shade pair for free, and a NEW theme cannot ship half-shaded.
    /// Adding two explicit roles per material to `Theme` instead would be ~90
    /// hand-picked colours per theme, and every one a chance to drift from the
    /// base it belongs to.
    ///
    /// Proportional rather than a flat per-channel add: moving a fraction of
    /// the distance to the endpoint keeps the hue, where `saturating_add` on an
    /// already-bright channel clips and skews it.
    pub fn from_base(base: Rgb, tint_pct: u8, shade_pct: u8) -> Self {
        Self {
            lit: toward(base, 255, tint_pct),
            base,
            shade: toward(base, 0, shade_pct),
        }
    }
}

/// Move each channel `pct` of the way to `target` (0 or 255).
fn toward(c: Rgb, target: u8, pct: u8) -> Rgb {
    let mix = |v: u8| -> u8 {
        let (v16, t16, p) = (u16::from(v), u16::from(target), u16::from(pct.min(100)));
        let moved = if t16 >= v16 {
            v16 + (t16 - v16) * p / 100
        } else {
            v16 - (v16 - t16) * p / 100
        };
        moved as u8
    };
    Rgb {
        r: mix(c.r),
        g: mix(c.g),
        b: mix(c.b),
    }
}

/// Fill a rect as a top-lit mass: one `lit` row at the top, one `shade` row at
/// the bottom, `base` between.
///
/// A mass shorter than two rows is painted `base` only — at one row the top and
/// bottom edges are the same pixels, and picking either tone would make a 1px
/// detail read as a highlight or a shadow rather than as the material.
pub fn slab(buf: &mut RgbBuffer, x: u16, y: u16, w: u16, h: u16, ramp: &Ramp) {
    if w == 0 || h == 0 {
        return;
    }
    fill(buf, x, y, w, h, ramp.base);
    if h >= 2 {
        fill(buf, x, y, w, 1, ramp.lit);
        fill(buf, x, y + h - 1, w, 1, ramp.shade);
    }
}

/// Paint a solid rect, clipped to the buffer.
pub fn fill(buf: &mut RgbBuffer, x: u16, y: u16, w: u16, h: u16, c: Rgb) {
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
/// the floor falloff needs two palette slots instead of a dozen.
const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Dither a horizontal band from `light` at its top to `dark` at its bottom.
///
/// The transition an indexed palette cannot express as a blend. `y1` is
/// exclusive; a band with no height paints nothing.
pub fn dither_band(buf: &mut RgbBuffer, y0: u16, y1: u16, dark: Rgb, light: Rgb) {
    if y1 <= y0 {
        return;
    }
    let span = u32::from(y1 - y0);
    for y in y0..y1.min(buf.height()) {
        // How far through the transition this row sits, on the matrix's scale.
        let level = (u32::from(y - y0) * 16 / span) as u8;
        for x in 0..buf.width() {
            let threshold = BAYER_4X4[usize::from(y % 4)][usize::from(x % 4)];
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
        slab(&mut buf, 1, 1, 4, 4, &ramp());
        assert_eq!(buf.get(1, 1), LIT, "top row catches the key light");
        assert_eq!(buf.get(1, 2), BASE, "the body is the material itself");
        assert_eq!(buf.get(1, 3), BASE);
        assert_eq!(buf.get(1, 4), SHADE, "the base row turns away");
        assert_eq!(buf.get(0, 1), BG, "nothing painted outside the rect");
        assert_eq!(buf.get(5, 1), BG);
    }

    #[test]
    fn a_derived_ramp_brackets_its_base_and_keeps_the_hue() {
        // A saturated blue: the lit tone must stay blue-dominant. A flat
        // per-channel add would push it toward white and lose the material.
        let blue = Rgb {
            r: 40,
            g: 70,
            b: 200,
        };
        let r = Ramp::from_base(blue, 30, 30);
        assert_eq!(r.base, blue, "the base is the theme's own colour");
        assert!(r.lit.r > blue.r && r.lit.g > blue.g && r.lit.b > blue.b);
        assert!(r.shade.r < blue.r && r.shade.g < blue.g && r.shade.b < blue.b);
        assert!(
            r.lit.b > r.lit.r && r.lit.b > r.lit.g,
            "the lit tone is still blue, not washed toward white: {:?}",
            r.lit
        );
    }

    #[test]
    fn a_derived_ramp_never_leaves_the_channel_range() {
        // The endpoints are where a naive add/sub wraps or clips wrongly.
        for c in [
            Rgb { r: 0, g: 0, b: 0 },
            Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
        ] {
            let r = Ramp::from_base(c, 100, 100);
            assert_eq!(
                r.lit,
                Rgb {
                    r: 255,
                    g: 255,
                    b: 255
                }
            );
            assert_eq!(r.shade, Rgb { r: 0, g: 0, b: 0 });
        }
        // A zero-percent ramp is the flat material, not a shifted one.
        let c = Rgb {
            r: 90,
            g: 20,
            b: 60,
        };
        assert_eq!(Ramp::from_base(c, 0, 0), Ramp::flat(c));
    }

    #[test]
    fn a_one_row_slab_is_all_base() {
        // Top and bottom are the same pixels here, so either tone would make a
        // 1px detail read as a highlight or a shadow instead of as the material.
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 0, 0, 4, 1, &ramp());
        for x in 0..4 {
            assert_eq!(buf.get(x, 0), BASE);
        }
    }

    #[test]
    fn a_slab_clips_at_the_buffer_edge_instead_of_wrapping() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 3, 3, 8, 8, &ramp());
        // The one in-bounds pixel is the slab's OWN top row, so it is lit — the
        // shade row sits at y+h-1, far outside, and is dropped rather than
        // wrapping back into the buffer.
        assert_eq!(buf.get(3, 3), LIT, "the one in-bounds pixel is the lit row");
        assert_eq!(buf.get(0, 0), BG, "nothing wrapped to the origin");
    }

    #[test]
    fn an_empty_slab_paints_nothing() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        slab(&mut buf, 0, 0, 0, 4, &ramp());
        slab(&mut buf, 0, 0, 4, 0, &ramp());
        assert!(buf.as_slice().iter().all(|&c| c == BG));
    }

    #[test]
    fn a_dither_band_runs_light_at_the_top_to_dark_at_the_bottom() {
        let mut buf = RgbBuffer::filled(16, 32, BG);
        dither_band(&mut buf, 0, 32, SHADE, LIT);

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
        // The whole point of a dither under an indexed palette: it introduces
        // no third colour, so it costs no extra slot.
        let mut buf = RgbBuffer::filled(8, 8, BG);
        dither_band(&mut buf, 0, 8, SHADE, LIT);
        assert!(buf.as_slice().iter().all(|&c| c == SHADE || c == LIT));
    }

    #[test]
    fn an_inverted_or_empty_band_paints_nothing() {
        let mut buf = RgbBuffer::filled(4, 4, BG);
        dither_band(&mut buf, 3, 3, SHADE, LIT);
        dither_band(&mut buf, 3, 1, SHADE, LIT);
        assert!(buf.as_slice().iter().all(|&c| c == BG));
    }
}
