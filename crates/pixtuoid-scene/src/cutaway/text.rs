//! The cutaway's pixel font: text painted on the art grid, one glyph pixel per
//! art pixel, never anti-aliased.
//!
//! ASCII and the signs' symbols are [`hand_drawn`]; every other character
//! comes from the [`fallback`] font, two open bitmap fonts
//! `scripts/gen-fallback-font.py` aligns to the same lines. The workspace's
//! other face (the binary's `aa_text`) is an anti-aliased OTF this wasm-clean
//! crate must not embed. A character takes the terminal cells
//! [`unicode-width`](unicode_width) gives it, each [`ADVANCE`] wide: at the
//! pack's 4x art one logical column, the classic badge's terminal column.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use unicode_width::UnicodeWidthChar;

use crate::cutaway::pen::{ArtPx, ArtRect, Pen};

/// A hand-drawn glyph's width in art pixels.
const GLYPH_W: u16 = 3;
/// A cell's width in art pixels: one glyph and its gap.
const ADVANCE: u16 = GLYPH_W + 1;
/// The rows above the capitals, which only accents and CJK ink.
const ACCENT_ROWS: u16 = 2;
/// A capital's height in art pixels.
const CAP_H: u16 = 5;
/// A line's height in art pixels: the accent rows, the capitals, and a row for
/// descenders, Fusion Pixel 8px's own box. Pinned to the fallback font's
/// header.
pub(crate) const LINE_H: u16 = ACCENT_ROWS + CAP_H + 1;

/// A glyph: each line row's ink, the high bit its leftmost pixel.
type Rows = [u8; LINE_H as usize];
const LEFTMOST_PIXEL: u8 = 1 << (u8::BITS - 1);

/// The fallback font, written by `scripts/gen-fallback-font.py` (its format);
/// license and sources in `fonts/`.
static FALLBACK: &[u8] = include_bytes!("../../fonts/fallback.bin");

/// The fallback font's code points, ascending, and their glyphs; `None` when
/// it was generated for another [`LINE_H`].
fn fallback_font() -> Option<(&'static [[u8; 2]], &'static [Rows])> {
    let (&[rows, n_lo, n_hi], rest) = FALLBACK.split_first_chunk::<3>()?;
    if u16::from(rows) != LINE_H {
        return None;
    }
    let n = usize::from(u16::from_le_bytes([n_lo, n_hi]));
    let (points, glyphs) = rest.split_at_checked(n * size_of::<u16>())?;
    Some((points.as_chunks().0, glyphs.as_chunks().0))
}

/// `c`'s glyph in the fallback font, if it has one.
fn fallback(c: char) -> Option<Rows> {
    let (points, glyphs) = fallback_font()?;
    let point = u16::try_from(u32::from(c)).ok()?;
    let i = points
        .binary_search_by_key(&point, |b| u16::from_le_bytes(*b))
        .ok()?;
    glyphs.get(i).copied()
}

/// `c`'s glyph, `None` when neither font draws it.
fn glyph(c: char) -> Option<Rows> {
    hand_drawn(c).map(rows_of).or_else(|| fallback(c))
}

/// A hand-drawn glyph's [`Rows`], under the accent rows.
fn rows_of(drawing: &str) -> Rows {
    let mut rows = Rows::default();
    let under_accents = rows.iter_mut().skip(usize::from(ACCENT_ROWS));
    for (row, cells) in under_accents.zip(drawing.split(' ')) {
        for (dx, cell) in cells.bytes().enumerate() {
            if cell == b'#' {
                *row |= LEFTMOST_PIXEL >> dx;
            }
        }
    }
    rows
}

/// What a character neither font draws is: a solid box `n` cells wide and a
/// capital high, so a run never collapses.
fn tofu(n: u16) -> Rows {
    let ink = !u8::MAX
        .checked_shr(u32::from(columns(n).0.saturating_sub(1)))
        .unwrap_or(0);
    let mut rows = Rows::default();
    let capitals = rows.iter_mut().skip(usize::from(ACCENT_ROWS));
    for row in capitals.take(usize::from(CAP_H)) {
        *row = ink;
    }
    rows
}

/// `c`'s rows from the capital line down, each [`GLYPH_W`] cells of `#` (ink)
/// or `.`, separated by spaces; rows not given are blank. Lowercase stands
/// four rows tall under a one-row ascender.
fn hand_drawn(c: char) -> Option<&'static str> {
    Some(match c {
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
        'w' => "... #.# #.# ### ###",
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
        '\u{25cb}' => "... ### #.# ###",
        '\u{25b2}' => "... .#. ### ###",
        '\u{25bc}' => "... ### ### .#.",
        '\u{2605}' => ".#. ### .#. #.#",
        '\u{2191}' => ".#. #.# .#. .#. .#.",
        '\u{2014}' => "... ... ###",
        '\u{2b22}' => "... .#. ### ### .#.",
        _ => return None,
    })
}

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

/// Paint `text` in `ink` from its top-left `(x, y)`, clipped to the buffer.
pub(crate) fn paint(pen: Pen, buf: &mut RgbBuffer, (x, y): (ArtPx, ArtPx), text: &str, ink: Rgb) {
    let mut left = x.0;
    for c in text.chars() {
        let n = char_cells(c);
        if n == 0 {
            continue;
        }
        let rows = glyph(c).unwrap_or_else(|| tofu(n));
        for (dy, mut bits) in (0u16..).zip(rows) {
            let mut dx = 0;
            while bits != 0 {
                if bits & LEFTMOST_PIXEL != 0 {
                    let at = ArtRect {
                        x: ArtPx(left.saturating_add(dx)),
                        y: ArtPx(y.0.saturating_add(dy)),
                        w: ArtPx(1),
                        h: ArtPx(1),
                    };
                    pen.fill(buf, at, ink);
                }
                bits <<= 1;
                dx += 1;
            }
        }
        left = left.saturating_add(columns(n).0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the wall board and the floor indicator write: each
    /// mood over two flap cycles, each gateway state, many floors.
    fn signs() -> std::collections::BTreeSet<char> {
        use crate::board::{StateCounts, build_board};
        use pixtuoid_core::state::DaemonState;
        let mut text = crate::layout::floor_indicator_text(12);
        let moods = [
            StateCounts::default(),
            StateCounts {
                idle: 2,
                total: 2,
                ..StateCounts::default()
            },
            StateCounts {
                active: 1,
                total: 1,
                ..StateCounts::default()
            },
            StateCounts {
                active: 3,
                total: 3,
                ..StateCounts::default()
            },
            StateCounts {
                waiting: 1,
                total: 1,
                ..StateCounts::default()
            },
            StateCounts {
                waiting: 2,
                active: 3,
                idle: 4,
                total: 9,
                ..StateCounts::default()
            },
        ];
        let gateways = [
            None,
            Some(DaemonState::Idle),
            Some(DaemonState::Busy),
            Some(DaemonState::Degraded),
            Some(DaemonState::Down),
        ];
        for (counts, gateway) in moods.into_iter().zip(gateways.into_iter().cycle()) {
            for ms in (0..32_000).step_by(20) {
                let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
                let board = build_board(counts, 3_700, Some((2, 3)), gateway, now);
                for seg in [&board.brand, &board.star]
                    .into_iter()
                    .chain(&board.mood)
                    .chain(&board.context)
                {
                    text.push_str(&seg.text);
                }
            }
        }
        text.chars().collect()
    }

    /// The font draws everything the signs write: no sign shows a tofu box.
    #[test]
    fn the_font_draws_every_character_the_signs_write() {
        let missing: Vec<char> = signs()
            .into_iter()
            .filter(|&c| glyph(c).is_none())
            .collect();
        assert_eq!(missing, []);
    }

    /// Every hand-drawn glyph fits its cell: no row wider than [`GLYPH_W`],
    /// none below [`LINE_H`], and nothing but ink or blank in it.
    #[test]
    fn every_glyph_fits_its_cell() {
        let chars =
            (' '..='~')
                .chain(signs())
                .chain(['\u{b7}', crate::overlay::BADGE_MARKER, '\u{2603}']);
        for c in chars {
            let rows: Vec<&str> = hand_drawn(c).unwrap_or_default().split(' ').collect();
            assert!(
                rows.len() <= usize::from(LINE_H - ACCENT_ROWS),
                "{c:?}: {rows:?}"
            );
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
            let shape = hand_drawn(c)
                .expect("printable ASCII")
                .trim_end_matches([' ', '.']);
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
            let side = 1 + LINE_H;
            let mut buf = RgbBuffer::filled(side * k, side * k, bg);
            paint(pen, &mut buf, (ArtPx(1), ArtPx(1)), "I", ink);
            // The I's top bar is art (1..4, top), so buffer (k..4k, top*k..(top+1)*k).
            let top = 1 + ACCENT_ROWS;
            let foot = top + CAP_H - 1;
            let rows = |from: u16, to: u16| from * k..(to + 1) * k;
            for y in 0..side * k {
                for x in 0..side * k {
                    let top_bar = (k..4 * k).contains(&x) && rows(top, top).contains(&y);
                    let stem = (2 * k..3 * k).contains(&x) && rows(top, foot).contains(&y);
                    let bottom_bar = (k..4 * k).contains(&x) && rows(foot, foot).contains(&y);
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
        assert!(hand_drawn(crate::overlay::BADGE_MARKER).is_some());
    }

    /// A lowercase `w` closes its foot where `H` stands on open legs: the
    /// board's "wait" read "Hait" with an open one.
    #[test]
    fn a_lowercase_w_closes_its_foot_unlike_an_h() {
        let foot = |c| {
            hand_drawn(c)
                .and_then(|g| g.split(' ').nth(4))
                .expect("five rows")
        };
        assert_eq!(foot('w'), "###");
        assert_ne!(foot('H'), "###");
    }

    /// Where `text` painted from the origin leaves ink, as `(x, y)` art pixels.
    fn ink(text: &str) -> std::collections::BTreeSet<(u16, u16)> {
        let (bg, fg) = (Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 9, g: 9, b: 9 });
        let w = advance(text).0.max(1);
        let mut buf = RgbBuffer::filled(w, LINE_H, bg);
        paint(Pen::UNIT, &mut buf, (ArtPx(0), ArtPx(0)), text, fg);
        (0..w)
            .flat_map(|x| (0..LINE_H).map(move |y| (x, y)))
            .filter(|&(x, y)| buf.get(x, y) == fg)
            .collect()
    }

    /// Project names in CJK, Cyrillic and accented Latin draw real glyphs,
    /// each as wide as the cells `unicode-width` gives it.
    #[test]
    fn names_beyond_ascii_draw_glyphs_as_wide_as_their_cells() {
        for (name, n) in [
            ("日本語", 6),
            ("项目", 4),
            ("проект", 6),
            ("café-api", 8),
            ("ñandú", 5),
            ("한글", 4),
        ] {
            assert_eq!(cells(name), n, "{name}");
            for c in name.chars() {
                assert!(glyph(c).is_some(), "{c:?} of {name} is tofu");
            }
        }
        let wide = ink("日");
        assert!(
            wide.iter().any(|&(x, _)| x >= ADVANCE),
            "a CJK glyph spans its second cell: {wide:?}"
        );
        assert_eq!(advance("日I"), columns(3));
        let after = ink("日I");
        assert!(
            after.contains(&(columns(2).0, ACCENT_ROWS)),
            "the I's top bar opens its third cell"
        );
    }

    /// The one fallback: a character neither font draws is a solid box,
    /// capital-high and its cells wide, so a run never collapses.
    #[test]
    fn a_character_no_font_draws_is_a_box_its_cells_wide() {
        // Thai, and CJK Extension A: in neither pinned font's subset.
        for (c, n) in [('\u{0e01}', 1), ('\u{3400}', 2)] {
            assert_eq!(glyph(c), None, "{c:?}");
            assert_eq!(char_cells(c), n, "{c:?}");
            let text = c.to_string();
            let want: std::collections::BTreeSet<_> = (0..width(&text).0)
                .flat_map(|x| (ACCENT_ROWS..ACCENT_ROWS + CAP_H).map(move |y| (x, y)))
                .collect();
            assert_eq!(ink(&text), want, "{c:?}");
        }
    }

    /// A fallback glyph stands on the hand-drawn baseline and under its
    /// capital line, so a mixed name reads as one line.
    #[test]
    fn fallback_glyphs_share_the_hand_drawn_lines() {
        let bottom = |c| glyph(c).and_then(|g| g.iter().rposition(|&row| row != 0));
        let top = |c| glyph(c).and_then(|g| g.iter().position(|&row| row != 0));
        assert_eq!(bottom('é'), bottom('e'), "Latin from Fusion Pixel");
        assert_eq!(top('П'), top('H'), "Cyrillic from 4x6");
        assert_eq!(bottom('П'), bottom('H'));
        assert_eq!(bottom('日'), bottom('g'), "CJK reaches the descender row");
    }

    /// The fallback font is well formed, and every glyph in it fits the
    /// cells its character takes.
    #[test]
    fn every_fallback_glyph_fits_its_characters_cells() {
        let (points, glyphs) = fallback_font().expect("the header matches LINE_H");
        assert_eq!(points.len(), glyphs.len());
        let points: Vec<u16> = points.iter().map(|&b| u16::from_le_bytes(b)).collect();
        assert!(
            points.is_sorted_by(|a, b| a < b),
            "binary search needs them ascending"
        );
        let misfits: Vec<(char, u16)> = points
            .iter()
            .zip(glyphs)
            .filter_map(|(&point, rows)| {
                let c = char::from_u32(u32::from(point))?;
                let n = char_cells(c);
                let past_its_cells = u8::MAX.checked_shr(u32::from(columns(n).0)).unwrap_or(0);
                (n == 0 || rows.iter().any(|row| row & past_its_cells != 0)).then_some((c, n))
            })
            .collect();
        assert_eq!(misfits, []);
    }

    #[test]
    fn a_run_is_its_advances_less_the_trailing_gap() {
        assert_eq!(width(""), ArtPx(0));
        assert_eq!(width("cc\u{b7}a"), ArtPx(4 * ADVANCE - 1));
    }
}
