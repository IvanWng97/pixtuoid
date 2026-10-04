//! Text as the display list lays it out: a character takes the terminal cells
//! [`unicode-width`](unicode_width) gives it, each [`ADVANCE`] art pixels wide
//! on a line [`LINE_H`] tall. At the pack's 4x art a cell is one logical
//! column, the classic badge's terminal column. The rasterizer's font draws
//! into these cells.

use unicode_width::UnicodeWidthChar;

use crate::display::pen::ArtPx;

/// A hand-drawn glyph's width in art pixels.
pub(crate) const GLYPH_W: u16 = 3;
/// A cell's width in art pixels: one glyph and its gap.
pub(crate) const ADVANCE: u16 = GLYPH_W + 1;
/// The rows above the capitals, which only accents and CJK ink.
pub(crate) const ACCENT_ROWS: u16 = 2;
/// A capital's height in art pixels.
pub(crate) const CAP_H: u16 = 5;
/// A line's height in art pixels: the accent rows, the capitals, and a row for
/// descenders, Fusion Pixel 8px's own box. Pinned to the fallback font's
/// header.
pub(crate) const LINE_H: u16 = ACCENT_ROWS + CAP_H + 1;

/// `text`'s width in art pixels, from its first ink column to its last.
pub(crate) fn width(text: &str) -> ArtPx {
    ArtPx(advance(text).0.saturating_sub(1))
}

/// Art pixels from a run's left edge to where a run `n` cells on starts.
pub(crate) fn columns(n: u16) -> ArtPx {
    ArtPx(n.saturating_mul(ADVANCE))
}

/// The terminal cells `c` takes: two for a wide character, none for a
/// combining mark or a control.
pub(crate) fn char_cells(c: char) -> u16 {
    c.width()
        .map_or(0, |n| u16::try_from(n).unwrap_or(u16::MAX))
}

/// The cells `text` takes, the one width every run of it is laid out by.
pub(crate) fn cells(text: &str) -> u16 {
    text.chars().fold(0, |n, c| n.saturating_add(char_cells(c)))
}

/// [`columns`] past all of `text`: where the run after it starts.
pub(crate) fn advance(text: &str) -> ArtPx {
    columns(cells(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_its_advances_less_the_trailing_gap() {
        assert_eq!(width(""), ArtPx(0));
        assert_eq!(width("cc\u{b7}a"), ArtPx(4 * ADVANCE - 1));
    }
}
