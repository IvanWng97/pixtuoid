//! Backend-agnostic 8×8 bitmap-font rasterizer.
//!
//! The shared glyph source for surfaces that draw crisp pixel text — the
//! `snapshot` example's PNG/RGBA rasterizers and the floating window's
//! name-badge labels. Custom status dots (●/○/◐) come first, then font8x8's
//! sets. `draw_text` is surface-agnostic: it calls a `put(px, py)` closure for
//! every lit foreground pixel, so the caller writes whatever buffer it owns.

/// 8×8 bitmaps for the few status glyphs font8x8 lacks but the popups lean on
/// for at-a-glance state — the dashboard/connection dots `●` `○` `◐`. Without
/// these all three collapse to the same fallback block, distinct only by color.
/// Rows top→bottom, bit 0 (LSB) = leftmost pixel — same convention as font8x8.
fn custom_glyph(ch: char) -> Option<[u8; 8]> {
    match ch {
        '\u{25CF}' => Some([0x3C, 0x7E, 0xFF, 0xFF, 0xFF, 0xFF, 0x7E, 0x3C]), // ● filled disc
        '\u{25CB}' => Some([0x3C, 0x42, 0x81, 0x81, 0x81, 0x81, 0x42, 0x3C]), // ○ ring
        '\u{25D0}' => Some([0x3C, 0x4E, 0x8F, 0x8F, 0x8F, 0x8F, 0x4E, 0x3C]), // ◐ left-half disc
        _ => None,
    }
}

/// 8×8 bitmap per char: custom status dots (●/○/◐) first, then font8x8's sets
/// (ASCII, then Latin/box/block/misc/greek). Rows top→bottom; within a row bit 0
/// (LSB) = leftmost pixel. `None` for an uncovered glyph.
pub fn glyph8x8(ch: char) -> Option<[u8; 8]> {
    use font8x8::{
        UnicodeFonts, BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, LATIN_FONTS, MISC_FONTS,
    };
    custom_glyph(ch)
        .or_else(|| BASIC_FONTS.get(ch))
        .or_else(|| LATIN_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| MISC_FONTS.get(ch))
        .or_else(|| GREEK_FONTS.get(ch))
}

/// Rasterize `text` as 8×8 glyphs left-to-right from (x,y), `scale`× in both axes,
/// calling `put(px,py)` for each lit foreground pixel. Backend-agnostic — caller
/// writes its surface. 8·scale px advance per char; uncovered glyphs advance but
/// draw nothing.
pub fn draw_text(text: &str, x: i32, y: i32, scale: i32, mut put: impl FnMut(i32, i32)) {
    let mut cx = x;
    for ch in text.chars() {
        if let Some(rows) = glyph8x8(ch) {
            for (ry, &bits) in rows.iter().enumerate() {
                for col in 0..8i32 {
                    if bits & (1u8 << col) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                put(cx + col * scale + sx, y + ry as i32 * scale + sy);
                            }
                        }
                    }
                }
            }
        }
        cx += 8 * scale;
    }
}

/// Pixel width of `text` rendered at `scale` (chars × 8 × scale).
pub fn text_width(text: &str, scale: i32) -> i32 {
    text.chars().count() as i32 * 8 * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph8x8_covers_ascii_and_custom_dots_but_not_arbitrary_symbols() {
        // ASCII resolves via font8x8's BASIC set; · (U+00B7) via LATIN — both
        // are the legibility win, so pin them.
        assert!(glyph8x8('A').is_some());
        assert!(glyph8x8('z').is_some());
        assert!(
            glyph8x8('\u{00b7}').is_some(),
            "middle-dot separator must be legible"
        );
        // The status dots resolve via our custom_glyph table (added so they're
        // shape-distinct, not merely color-distinct).
        assert!(glyph8x8('\u{25cf}').is_some(), "\u{25cf} filled disc");
        assert!(glyph8x8('\u{25cb}').is_some(), "\u{25cb} ring");
        assert!(glyph8x8('\u{25d0}').is_some(), "\u{25d0} left-half disc");
        // ...and the three are DISTINCT bitmaps — the whole point of custom_glyph.
        let (filled, ring, half) = (
            glyph8x8('\u{25cf}').unwrap(),
            glyph8x8('\u{25cb}').unwrap(),
            glyph8x8('\u{25d0}').unwrap(),
        );
        assert_ne!(filled, ring);
        assert_ne!(filled, half);
        assert_ne!(ring, half);
        // A glyph nothing covers falls through to None → the caller's block fallback.
        assert!(
            glyph8x8('\u{2713}').is_none(),
            "\u{2713} check has no glyph"
        );
    }

    #[test]
    fn draw_text_rasterizes_within_the_glyph_cell() {
        let mut hits: Vec<(i32, i32)> = Vec::new();
        draw_text("A", 0, 0, 1, |px, py| hits.push((px, py)));
        assert!(!hits.is_empty(), "'A' draws lit pixels");
        for (px, py) in &hits {
            assert!((0..8).contains(px), "x in glyph cell: {px}");
            assert!((0..8).contains(py), "y in glyph cell: {py}");
        }
    }

    #[test]
    fn text_width_is_chars_times_eight_times_scale() {
        assert_eq!(text_width("AB", 1), 16);
        assert_eq!(text_width("AB", 2), 32);
        assert_eq!(text_width("", 3), 0);
    }
}
