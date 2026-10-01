//! The cutaway's pixel font: text painted on the art grid, one glyph pixel per
//! art pixel, never anti-aliased.
//!
//! Hand-drawn because the workspace's one face (the binary's `aa_text`) is an
//! anti-aliased OTF this wasm-clean crate must not embed. Glyphs are
//! [`GLYPH_W`] wide on an [`ADVANCE`] pitch: at the pack's 4x art that is one
//! logical column per character, the classic badge's terminal column.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::pen::{ArtPx, ArtRect, Pen};

/// A glyph's width in art pixels.
const GLYPH_W: u16 = 3;
/// Art pixels from one glyph's left edge to the next's.
pub(crate) const ADVANCE: u16 = GLYPH_W + 1;
/// A line's height in art pixels: five rows for capitals, ascenders and
/// digits, and a sixth for descenders.
pub(crate) const LINE_H: u16 = 6;

/// `c`'s rows from the top, each [`GLYPH_W`] cells of `#` (ink) or `.`,
/// separated by spaces; rows not given are blank. Lowercase stands four rows
/// tall under a one-row ascender. A character the font lacks is a solid box,
/// so a run never collapses.
fn rows(c: char) -> &'static str {
    match c {
        'A' => ".#. #.# ### #.# #.#",
        'B' => "##. #.# ##. #.# ##.",
        'C' => ".## #.. #.. #.. .##",
        'D' => "##. #.# #.# #.# ##.",
        'E' => "### #.. ##. #.. ###",
        'F' => "### #.. ##. #.. #..",
        'G' => ".## #.. #.# #.# .##",
        'H' => "#.# #.# ### #.# #.#",
        'I' => "### .#. .#. .#. ###",
        'J' => "..# ..# ..# #.# .#.",
        'K' => "#.# #.# ##. #.# #.#",
        'L' => "#.. #.. #.. #.. ###",
        'M' => "#.# ### ### #.# #.#",
        'N' => "##. #.# #.# #.# #.#",
        'O' => ".#. #.# #.# #.# .#.",
        'P' => "##. #.# ##. #.. #..",
        'Q' => ".#. #.# #.# ##. .##",
        'R' => "##. #.# ##. #.# #.#",
        'S' => ".## #.. .#. ..# ##.",
        'T' => "### .#. .#. .#. .#.",
        'U' => "#.# #.# #.# #.# ###",
        'V' => "#.# #.# #.# #.# .#.",
        'W' => "#.# #.# ### ### #.#",
        'X' => "#.# #.# .#. #.# #.#",
        'Y' => "#.# #.# .#. .#. .#.",
        'Z' => "### ..# .#. #.. ###",
        'a' => "... .## #.# #.# .##",
        'b' => "#.. ##. #.# #.# ##.",
        'c' => "... .## #.. #.. .##",
        'd' => "..# .## #.# #.# .##",
        'e' => "... .#. ### #.. .##",
        'f' => ".## .#. ### .#. .#.",
        'g' => "... .## #.# .## ..# ##.",
        'h' => "#.. ##. #.# #.# #.#",
        'i' => ".#. ... .#. .#. .#.",
        'j' => "..# ... ..# ..# ..# ##.",
        'k' => "#.. #.# ##. #.# #.#",
        'l' => "##. .#. .#. .#. .##",
        'm' => "... ##. ### #.# #.#",
        'n' => "... ##. #.# #.# #.#",
        'o' => "... .#. #.# #.# .#.",
        'p' => "... ##. #.# #.# ##. #..",
        'q' => "... .## #.# #.# .## ..#",
        'r' => "... #.# ##. #.. #..",
        's' => "... .## ##. ..# ##.",
        't' => ".#. ### .#. .#. .##",
        'u' => "... #.# #.# #.# .##",
        'v' => "... #.# #.# #.# .#.",
        'w' => "... #.# #.# ### #.#",
        'x' => "... #.# .#. #.# #.#",
        'y' => "... #.# #.# .## ..# ##.",
        'z' => "... ### .#. #.. ###",
        '0' => "### #.# #.# #.# ###",
        '1' => ".#. ##. .#. .#. ###",
        '2' => "##. ..# .#. #.. ###",
        '3' => "##. ..# .#. ..# ##.",
        '4' => "#.# #.# ### ..# ..#",
        '5' => "### #.. ##. ..# ##.",
        '6' => ".## #.. ### #.# ###",
        '7' => "### ..# .#. .#. .#.",
        '8' => "### #.# ### #.# ###",
        '9' => "### #.# ### ..# ##.",
        ' ' => "",
        '!' => ".#. .#. .#. ... .#.",
        '"' => "#.# #.#",
        '#' => "#.# ### #.# ### #.#",
        '$' => ".## ##. .#. .## ##.",
        '%' => "#.. ..# .#. #.. ..#",
        '&' => ".#. #.# .#. #.# .##",
        '\'' => ".#. .#.",
        '(' => "..# .#. .#. .#. ..#",
        ')' => "#.. .#. .#. .#. #..",
        '*' => "... #.# .#. #.#",
        '+' => "... .#. ### .#.",
        ',' => "... ... ... .#. #..",
        '-' => "... ... ###",
        '.' => "... ... ... ... .#.",
        '/' => "..# ..# .#. #.. #..",
        ':' => "... .#. ... .#.",
        ';' => "... .#. ... .#. #..",
        '<' => "..# .#. #.. .#. ..#",
        '=' => "... ### ... ###",
        '>' => "#.. .#. ..# .#. #..",
        '?' => "##. ..# .#. ... .#.",
        '@' => ".#. #.# ### #.. .##",
        '[' => "##. #.. #.. #.. ##.",
        '\\' => "#.. #.. .#. ..# ..#",
        ']' => ".## ..# ..# ..# .##",
        '^' => ".#. #.#",
        '_' => "... ... ... ... ###",
        '`' => "#.. .#.",
        '{' => ".## .#. ##. .#. .##",
        '|' => ".#. .#. .#. .#. .#.",
        '}' => "##. .#. .## .#. ##.",
        '~' => "... ##. .##",
        '\u{b7}' => "... ... .#.",
        '\u{25cf}' => "... ### ### ###",
        _ => TOFU,
    }
}

/// What a character the font lacks draws as.
const TOFU: &str = "### ### ### ### ###";

/// `text`'s width in art pixels, from its first ink column to its last.
pub(crate) fn width(text: &str) -> ArtPx {
    let n = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    ArtPx(n.saturating_mul(ADVANCE).saturating_sub(1))
}

/// Paint `text` in `ink` from its top-left `(x, y)`, clipped to the buffer.
pub(crate) fn paint(pen: Pen, buf: &mut RgbBuffer, (x, y): (ArtPx, ArtPx), text: &str, ink: Rgb) {
    for (i, c) in text.chars().enumerate() {
        let left =
            x.0.saturating_add(u16::try_from(i).unwrap_or(u16::MAX).saturating_mul(ADVANCE));
        for (dy, row) in (0u16..).zip(rows(c).split(' ')) {
            for (dx, cell) in (0u16..).zip(row.bytes()) {
                if cell == b'#' {
                    let at = ArtRect {
                        x: ArtPx(left.saturating_add(dx)),
                        y: ArtPx(y.0.saturating_add(dy)),
                        w: ArtPx(1),
                        h: ArtPx(1),
                    };
                    pen.fill(buf, at, ink);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph fits its cell: no row wider than [`GLYPH_W`], none below
    /// [`LINE_H`], and nothing but ink or blank in it.
    #[test]
    fn every_glyph_fits_its_cell() {
        let chars = (' '..='~').chain(['\u{b7}', crate::overlay::BADGE_MARKER, '\u{2603}']);
        for c in chars {
            let rows: Vec<&str> = rows(c).split(' ').collect();
            assert!(rows.len() <= usize::from(LINE_H), "{c:?}: {rows:?}");
            for row in rows {
                assert!(
                    row.is_empty() || (row.len() == usize::from(GLYPH_W)),
                    "{c:?}: row {row:?}"
                );
                assert!(row.bytes().all(|b| b == b'#' || b == b'.'), "{c:?}");
            }
        }
    }

    /// Each printable ASCII character draws its own shape: a label never
    /// reads as another.
    #[test]
    fn no_two_printable_characters_share_a_glyph() {
        let mut seen = std::collections::HashMap::new();
        for c in '!'..='~' {
            let shape = rows(c).trim_end_matches([' ', '.']);
            if let Some(other) = seen.insert(shape, c) {
                panic!("{c:?} draws as {other:?}");
            }
        }
    }

    /// One glyph pixel is one art pixel, whatever the scale: `k` buffer
    /// pixels square, all ink.
    #[test]
    fn a_glyph_pixel_is_one_art_pixel() {
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let ink = Rgb { r: 255, g: 1, b: 2 };
        for (s, d) in [(4u16, 4u16), (8, 4)] {
            let pen = Pen::new(crate::render_scale::RenderScale::new(s).expect("s"), d)
                .expect("d divides s");
            let k = s / d;
            let mut buf = RgbBuffer::filled(8 * k, 8 * k, bg);
            paint(pen, &mut buf, (ArtPx(1), ArtPx(1)), "I", ink);
            // The I's top bar: art (1..4, 1), so buffer (k..4k, k..2k).
            for y in 0..8 * k {
                for x in 0..8 * k {
                    let top_bar = (k..4 * k).contains(&x) && (k..2 * k).contains(&y);
                    let stem = (2 * k..3 * k).contains(&x) && (k..6 * k).contains(&y);
                    let bottom_bar = (k..4 * k).contains(&x) && (5 * k..6 * k).contains(&y);
                    let want = if top_bar || stem || bottom_bar {
                        ink
                    } else {
                        bg
                    };
                    assert_eq!(buf.get(x, y), want, "scale {s} at ({x}, {y})");
                }
            }
        }
    }

    /// Every painter's badge marker has a glyph: changing it can't leave the
    /// cutaway's badges leading with a tofu box.
    #[test]
    fn the_font_draws_the_badge_marker() {
        assert_ne!(rows(crate::overlay::BADGE_MARKER), TOFU);
    }

    #[test]
    fn a_run_is_its_advances_less_the_trailing_gap() {
        assert_eq!(width(""), ArtPx(0));
        assert_eq!(width("cc\u{b7}a"), ArtPx(4 * ADVANCE - 1));
    }
}
