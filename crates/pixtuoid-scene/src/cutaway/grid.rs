//! Screen text in pixels: a [`CellGrid`] painted cell by cell in a [`Face`],
//! for the painters a terminal draws no text for (the floating window).
//!
//! Two faces of Fusion Pixel (`scripts/gen-fonts.py`, licenses in
//! `fonts/`): [`Face::World`], 8px, the cutaway's own (`text.rs`), whose cell
//! is one layout column of the art, and [`Face::Screen`], 12px, which reads at
//! a screen's size. A symbol Fusion Pixel draws only full-width is drawn here
//! half-width, in the one cell a terminal gives it.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::text::{self, LEFTMOST_PIXEL as WORLD_LEFTMOST, Rows};
use crate::display::cells::CellGrid;
use crate::display::text::{ADVANCE, LINE_H, clusters};

/// A glyph in either face: rows of ink from the cell's top, the high bit its
/// leftmost pixel, [`SCREEN_LINE_H`] rows deep.
type Bitmap = [u16; SCREEN_LINE_H as usize];
const LEFTMOST: u16 = 1 << (u16::BITS - 1);

/// The screen face's cell, in its pixels.
const SCREEN_CELL_W: u16 = 6;
const SCREEN_LINE_H: u16 = 12;
/// Its capitals' top row and height: the box a hand-drawn symbol centres in.
const SCREEN_CAP_TOP: u16 = 2;
const SCREEN_CAP_H: u16 = 8;

/// The screen face, written by `scripts/gen-fonts.py`: a byte, the
/// rows per glyph; a u16 LE count n; n u16 LE code points, ascending; n
/// glyphs of u16 LE rows.
#[cfg(feature = "cutaway-assets")]
static SCREEN: &[u8] = include_bytes!("../../fonts/screen.bin");
#[cfg(not(feature = "cutaway-assets"))]
static SCREEN: &[u8] = &[];

/// Which glyphs a grid draws in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Fusion Pixel 8px, the cutaway's own, in a cell one layout column of
    /// the art wide.
    World,
    /// Fusion Pixel 12px, for text that reads at a screen's size.
    Screen,
}

impl Face {
    /// Its cell, one glyph pixel to a pixel.
    fn unit(self) -> CellPx {
        match self {
            Self::World => CellPx {
                w: ADVANCE,
                h: LINE_H,
            },
            Self::Screen => CellPx {
                w: SCREEN_CELL_W,
                h: SCREEN_LINE_H,
            },
        }
    }

    /// The cell that draws a glyph pixel `scale` pixels square.
    pub fn cell(self, scale: u16) -> CellPx {
        let unit = self.unit();
        CellPx {
            w: unit.w * scale,
            h: unit.h * scale,
        }
    }

    /// The whole-number scale its glyphs draw at to fit `cell`, at least 1:
    /// pixel art scales by whole numbers only (Godot, "Multiple
    /// resolutions": stretch scale mode `integer`).
    pub fn scale(self, cell: CellPx) -> u16 {
        let unit = self.unit();
        (cell.w / unit.w).min(cell.h / unit.h).max(1)
    }

    /// The scale its glyphs draw at in a cell `w` pixels wide, at least 1.
    pub fn scale_across(self, w: u16) -> u16 {
        (w / self.unit().w).max(1)
    }

    /// Whether it draws `c`, rather than tofu.
    pub fn draws(self, c: char) -> bool {
        self.glyph(c).is_some()
    }

    fn glyph(self, c: char) -> Option<Bitmap> {
        match self {
            Self::World => text::glyph(c).map(widen),
            Self::Screen => text::ruled(c)
                .map(widen)
                .or_else(|| screen_font(c))
                .or_else(|| screen_drawn(c)),
        }
    }

    /// A box `n` cells wide and a capital high, for a character it lacks.
    fn tofu(self, n: u16) -> Bitmap {
        let (top, h) = match self {
            Self::World => (
                crate::display::text::ACCENT_ROWS,
                crate::display::text::CAP_H,
            ),
            Self::Screen => (SCREEN_CAP_TOP, SCREEN_CAP_H),
        };
        let w = self.unit().w * n - 1;
        let ink = !u16::MAX.checked_shr(u32::from(w)).unwrap_or(0);
        let mut rows = Bitmap::default();
        for row in rows.iter_mut().skip(usize::from(top)).take(usize::from(h)) {
            *row = ink;
        }
        rows
    }
}

/// The world face's rows, in a [`Bitmap`].
fn widen(rows: Rows) -> Bitmap {
    let mut out = Bitmap::default();
    for (o, r) in out.iter_mut().zip(rows) {
        *o = u16::from(r) << (u16::BITS - u8::BITS);
    }
    debug_assert_eq!(u16::from(WORLD_LEFTMOST) << 8, LEFTMOST);
    out
}

/// `c`'s glyph in the generated screen face, if it has one.
fn screen_font(c: char) -> Option<Bitmap> {
    let (&rows, rest) = SCREEN.split_first()?;
    if u16::from(rows) != SCREEN_LINE_H {
        return None;
    }
    let (n, rest) = rest.split_first_chunk::<2>()?;
    let n = usize::from(u16::from_le_bytes(*n));
    let (points, glyphs) = rest.split_at_checked(n * size_of::<u16>())?;
    let point = u16::try_from(u32::from(c)).ok()?;
    let i = points
        .as_chunks::<2>()
        .0
        .binary_search_by_key(&point, |b| u16::from_le_bytes(*b))
        .ok()?;
    let stride = usize::from(SCREEN_LINE_H) * size_of::<u16>();
    let glyph = glyphs.get(i * stride..(i + 1) * stride)?;
    let mut out = Bitmap::default();
    for (o, b) in out.iter_mut().zip(glyph.as_chunks::<2>().0) {
        *o = u16::from_le_bytes(*b);
    }
    Some(out)
}

/// The symbols the office writes that Fusion Pixel draws only full-width, by
/// code point: each row its cells' width less the gap of `#` (ink) or `.`,
/// rows separated by spaces, centred on the capitals.
const SCREEN_DRAWN: &[(char, &str)] = &[
    ('\u{2190}', "..#.. .#... ##### .#... ..#.."),
    ('\u{2191}', "..#.. .###. #.#.# ..#.. ..#.. ..#.."),
    ('\u{2192}', "..#.. ...#. ##### ...#. ..#.."),
    ('\u{2193}', "..#.. ..#.. ..#.. #.#.# .###. ..#.."),
    ('\u{2197}', ".#### ...## ..#.# .#... #...."),
    ('\u{21b3}', "#.... #.... #..#. ##### ...#."),
    ('\u{22ee}', "..#.. ..... ..#.. ..... ..#.."),
    ('\u{23ce}', "....# ....# .#..# ##### .#..."),
    ('\u{25a4}', "##### #...# ##### #...# #####"),
    ('\u{25ae}', ".###. .###. .###. .###. .###. .###. .###."),
    ('\u{25af}', ".###. .#.#. .#.#. .#.#. .#.#. .#.#. .###."),
    ('\u{25b2}', "..#.. .###. .###. #####"),
    ('\u{25b8}', "#.... ##... ###.. ##... #...."),
    ('\u{25bc}', "##### .###. .###. ..#.."),
    ('\u{25be}', "##### .###. ..#.."),
    ('\u{25cb}', ".###. #...# #...# #...# .###."),
    ('\u{25cc}', ".#.#. #...# ..... #...# .#.#."),
    ('\u{25cf}', ".###. ##### ##### ##### .###."),
    ('\u{25d0}', ".###. ##..# ##..# ##..# .###."),
    ('\u{25f7}', ".###. #.#.# #.### #...# .###."),
    ('\u{2605}', "..#.. ##### .###. .#.#. #...#"),
    (
        '\u{2615}',
        "...#..#.... ..#..#..... ........... #########.. ##########. #########.# ##########. .#######... ..#####....",
    ),
    ('\u{2669}', "...#. ...#. ...#. ...#. .###. ####. .##.."),
    ('\u{26a0}', "..#.. .#.#. .#.#. #.#.# #...# #.#.# #####"),
    ('\u{2713}', "....# ...#. #.#.. .#..."),
    ('\u{2b22}', "..#.. .###. ##### ##### .###. ..#.."),
];

/// `c`'s [`SCREEN_DRAWN`] glyph, centred on the capitals.
fn screen_drawn(c: char) -> Option<Bitmap> {
    let i = SCREEN_DRAWN.binary_search_by_key(&c, |&(k, _)| k).ok()?;
    let (_, drawing) = SCREEN_DRAWN.get(i)?;
    let rows: Vec<&str> = drawing.split(' ').collect();
    let h = u16::try_from(rows.len()).unwrap_or(SCREEN_LINE_H);
    let top = if h > SCREEN_CAP_H {
        (SCREEN_CAP_TOP + SCREEN_CAP_H).saturating_sub(h)
    } else {
        SCREEN_CAP_TOP + (SCREEN_CAP_H - h) / 2
    };
    let mut out = Bitmap::default();
    for (o, row) in out.iter_mut().skip(usize::from(top)).zip(rows) {
        for (dx, b) in row.bytes().enumerate() {
            if b == b'#' {
                *o |= LEFTMOST >> dx;
            }
        }
    }
    Some(out)
}

/// The pixels a grid of screen text paints into; a read or write off it is
/// a no-op.
pub trait Canvas {
    fn pixel(&self, x: i32, y: i32) -> Option<Rgb>;
    fn set(&mut self, x: i32, y: i32, rgb: Rgb);
}

impl Canvas for RgbBuffer {
    fn pixel(&self, x: i32, y: i32) -> Option<Rgb> {
        let (x, y) = (u16::try_from(x).ok()?, u16::try_from(y).ok()?);
        (x < self.width() && y < self.height()).then(|| self.get(x, y))
    }

    fn set(&mut self, x: i32, y: i32, rgb: Rgb) {
        if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
            self.put_checked(x, y, rgb);
        }
    }
}

/// A screen cell's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPx {
    pub w: u16,
    pub h: u16,
}

/// How [`paint_grid`] inks what a grid leaves to the painter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridInk {
    /// The text colour of a cell with no ink of its own.
    pub text: Rgb,
    /// A one-pixel drop shadow under every glyph, for text over the office.
    pub halo: Option<Rgb>,
    /// How far a card's drop shadow darkens what is under it, a cell right
    /// and half a cell down, where the terminal's half-block shadow lands;
    /// `None` casts none.
    pub shadow: Option<f32>,
}

/// Paint `grid` in `face` with its top-left at pixel `at`, each cell `cell`
/// big: a cell's fill, then its glyph centred at [`Face::scale`], or
/// stretched to the whole cell for box-drawing lines and block elements so
/// they join; a bold one struck twice a pixel apart, as a terminal without a
/// bold face does (xterm(1): "the bold font will be produced by
/// overstriking").
pub fn paint_grid(
    canvas: &mut impl Canvas,
    grid: &CellGrid,
    at: (i32, i32),
    cell: CellPx,
    face: Face,
    ink: GridInk,
) {
    let (cw, ch) = (i32::from(cell.w), i32::from(cell.h));
    if let Some(factor) = ink.shadow {
        let (w, h) = (i32::from(grid.width()) * cw, i32::from(grid.height()) * ch);
        for y in at.1 + ch / 2..at.1 + ch / 2 + h {
            for x in at.0 + cw..at.0 + cw + w {
                if let Some(under) = canvas.pixel(x, y) {
                    canvas.set(x, y, darken(under, factor));
                }
            }
        }
    }
    for gy in 0..grid.height() {
        for gx in 0..grid.width() {
            let Some(c) = grid.get(gx, gy) else { continue };
            let (x, y) = (at.0 + i32::from(gx) * cw, at.1 + i32::from(gy) * ch);
            if let Some(bg) = c.bg {
                for py in y..y + ch {
                    for px in x..x + cw {
                        canvas.set(px, py, bg);
                    }
                }
            }
            let strikes: &[i32] = if c.bold { &[0, 1] } else { &[0] };
            if let Some(halo) = ink.halo {
                for &dx in strikes {
                    paint_cell_glyph(canvas, &c.symbol, (x + dx + 1, y + 1), cell, face, halo);
                }
            }
            for &dx in strikes {
                let fg = c.fg.unwrap_or(ink.text);
                paint_cell_glyph(canvas, &c.symbol, (x + dx, y), cell, face, fg);
            }
        }
    }
}

/// `symbol`'s glyph in `face` in the cell at `at`.
fn paint_cell_glyph(
    canvas: &mut impl Canvas,
    symbol: &str,
    at: (i32, i32),
    cell: CellPx,
    face: Face,
    ink: Rgb,
) {
    let (cw, ch) = (i32::from(cell.w), i32::from(cell.h));
    let mut chars = symbol.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Some(rows) = text::ruled(c)
    {
        for py in 0..ch {
            let row = rows
                .get(usize::try_from(py * i32::from(LINE_H) / ch).unwrap_or(0))
                .copied()
                .unwrap_or(0);
            for px in 0..cw {
                let gx = u32::try_from(px * i32::from(ADVANCE) / cw).unwrap_or(0);
                if row & WORLD_LEFTMOST.checked_shr(gx).unwrap_or(0) != 0 {
                    canvas.set(at.0 + px, at.1 + py, ink);
                }
            }
        }
        return;
    }
    let unit = face.unit();
    let s = i32::from(face.scale(cell));
    let top = at.1 + (ch - i32::from(unit.h) * s) / 2;
    let mut left = at.0;
    let glyphs = clusters(symbol).flat_map(|(cluster, n)| {
        text::glyphs_by(cluster, n, move |c| face.glyph(c), move |k| face.tofu(k))
    });
    for (rows, n) in glyphs {
        let span = cw * i32::from(n);
        let x0 = left + (span - i32::from(unit.w * n) * s) / 2;
        for (dy, mut bits) in (0i32..).zip(rows) {
            let mut dx = 0;
            while bits != 0 {
                if bits & LEFTMOST != 0 {
                    for py in 0..s {
                        for px in 0..s {
                            canvas.set(x0 + dx * s + px, top + dy * s + py, ink);
                        }
                    }
                }
                bits <<= 1;
                dx += 1;
            }
        }
        left += span;
    }
}

/// `c` darkened toward black by `factor` (0 is black, 1 unchanged).
fn darken(c: Rgb, factor: f32) -> Rgb {
    let f = |v: u8| (f32::from(v) * factor.clamp(0.0, 1.0)) as u8;
    Rgb {
        r: f(c.r),
        g: f(c.g),
        b: f(c.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::cells::CellGrid;
    use std::collections::BTreeSet;

    const BG: Rgb = Rgb {
        r: 200,
        g: 200,
        b: 200,
    };
    const FG: Rgb = Rgb { r: 9, g: 9, b: 9 };
    const INK: GridInk = GridInk {
        text: FG,
        halo: None,
        shadow: None,
    };
    const VS_CODE: CellPx = CellPx { w: 8, h: 18 };

    fn painted(text: &str, bold: bool, cell: CellPx, ink: GridInk) -> RgbBuffer {
        let mut grid = CellGrid::new(crate::display::text::cells(text), 1);
        grid.put((0, 0), text, None, bold);
        let (w, h) = (grid.width() * cell.w * 2, cell.h * 2);
        let mut buf = RgbBuffer::filled(w, h, BG);
        paint_grid(&mut buf, &grid, (0, 0), cell, Face::World, ink);
        buf
    }

    fn inked(buf: &RgbBuffer, colour: Rgb) -> BTreeSet<(u16, u16)> {
        (0..buf.width())
            .flat_map(|x| (0..buf.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| buf.get(x, y) == colour)
            .collect()
    }

    /// A glyph pixel is `scale` pixels square and the glyph is centred in
    /// its cell: VS Code's 8×18 cell draws at 2, a row of margin above.
    #[test]
    fn a_glyph_draws_whole_scaled_and_centred_in_its_cell() {
        assert_eq!(Face::World.scale(VS_CODE), 2);
        let marker = crate::badge::BADGE_MARKER.to_string();
        let unit = inked(&painted(&marker, false, Face::World.cell(1), INK), FG);
        let margin = (VS_CODE.h - LINE_H * 2) / 2;
        let want: BTreeSet<(u16, u16)> = unit
            .iter()
            .flat_map(|&(x, y)| (0..4).map(move |i| (x * 2 + i % 2, y * 2 + i / 2 + margin)))
            .collect();
        assert_eq!(inked(&painted(&marker, false, VS_CODE, INK), FG), want);
    }

    /// A symbol is drawn by hand only where Fusion Pixel 12px draws none in
    /// its cells: one it does is a drawing nothing paints.
    #[cfg(feature = "cutaway-assets")]
    #[test]
    fn a_screen_symbol_is_drawn_by_hand_only_where_fusion_pixel_lacks_it() {
        let shadowed: Vec<char> = SCREEN_DRAWN
            .iter()
            .map(|&(c, _)| c)
            .filter(|&c| screen_font(c).is_some() || text::ruled(c).is_some())
            .collect();
        assert_eq!(shadowed, []);
    }

    /// A box-drawing line runs the whole cell, however the cell divides,
    /// so a frame joins.
    #[test]
    fn a_rule_fills_its_cell_edge_to_edge() {
        let cell = CellPx { w: 9, h: 19 };
        let ink = inked(&painted("\u{2500}\u{2500}", false, cell, INK), FG);
        let row = ink.iter().map(|&(_, y)| y).min().expect("a rule draws");
        for x in 0..2 * cell.w {
            assert!(ink.contains(&(x, row)), "a gap at {x}");
        }
    }

    /// Bold strikes the glyph twice, a pixel apart.
    #[test]
    fn bold_strikes_twice_a_pixel_apart() {
        let plain = inked(&painted("I", false, VS_CODE, INK), FG);
        let bold = inked(&painted("I", true, VS_CODE, INK), FG);
        let struck: BTreeSet<_> = plain
            .iter()
            .flat_map(|&(x, y)| [(x, y), (x + 1, y)])
            .collect();
        assert_eq!(bold, struck);
    }

    /// Text over the office draws on a one-pixel shadow down and right.
    #[test]
    fn a_halo_lies_a_pixel_down_and_right_of_the_glyph() {
        let halo = Rgb { r: 1, g: 2, b: 3 };
        let buf = painted(
            "I",
            false,
            VS_CODE,
            GridInk {
                halo: Some(halo),
                ..INK
            },
        );
        let glyph = inked(&buf, FG);
        for (x, y) in inked(&buf, halo) {
            assert!(glyph.contains(&(x - 1, y - 1)), "({x}, {y}) shadows no ink");
        }
    }

    /// A card darkens what is a cell right and half a cell down of it,
    /// and its own fill covers the rest of the shadow.
    #[test]
    fn a_card_casts_its_shadow_a_cell_down_and_right() {
        let fill = Rgb { r: 50, g: 0, b: 0 };
        let mut grid = CellGrid::new(1, 1);
        grid.fill(fill);
        let mut buf = RgbBuffer::filled(3 * VS_CODE.w, 3 * VS_CODE.h, BG);
        let ink = GridInk {
            shadow: Some(0.5),
            ..INK
        };
        paint_grid(&mut buf, &grid, (0, 0), VS_CODE, Face::World, ink);
        assert_eq!(buf.get(0, 0), fill);
        assert_eq!(buf.get(VS_CODE.w, VS_CODE.h / 2), darken(BG, 0.5));
        assert_eq!(buf.get(VS_CODE.w, VS_CODE.h / 2 - 1), BG);
        assert_eq!(buf.get(2 * VS_CODE.w, 2 * VS_CODE.h), BG);
    }

    /// The table is sorted by code point, as its binary search needs, and
    /// each drawing fits its cells less the gap and the cell's rows.
    #[test]
    fn the_screen_drawings_ascend_and_fit_their_cells() {
        for pair in SCREEN_DRAWN.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{pair:?}");
        }
        for &(c, drawing) in SCREEN_DRAWN {
            let n = crate::display::text::cells(c.encode_utf8(&mut [0; 4]));
            let rows: Vec<&str> = drawing.split(' ').collect();
            assert!(rows.len() <= usize::from(SCREEN_LINE_H), "{c:?}");
            for row in rows {
                assert_eq!(
                    row.len(),
                    usize::from(SCREEN_CELL_W * n - 1),
                    "{c:?}: {row:?}"
                );
                assert!(row.bytes().all(|b| b == b'#' || b == b'.'), "{c:?}");
            }
        }
    }

    /// Each hand-drawn screen symbol draws its own shape.
    #[test]
    fn no_two_screen_symbols_share_a_glyph() {
        let mut seen = std::collections::HashMap::new();
        for &(c, _) in SCREEN_DRAWN {
            if let Some(other) = seen.insert(screen_drawn(c), c) {
                panic!("{c:?} draws as {other:?}");
            }
        }
    }

    /// A symbol a terminal gives one cell draws inside that cell in the
    /// screen face, though Fusion Pixel draws it full-width.
    #[test]
    fn a_symbol_draws_in_the_one_cell_a_terminal_gives_it() {
        let cell = Face::Screen.cell(2);
        let ink = inked(&painted_in("\u{25cf}", Face::Screen, cell), FG);
        assert!(!ink.is_empty());
        assert!(ink.iter().all(|&(x, _)| x < cell.w), "{ink:?}");
    }

    /// The screen face draws ASCII and CJK from Fusion Pixel 12px, each as
    /// wide as its cells.
    #[cfg(feature = "cutaway-assets")]
    #[test]
    fn the_screen_face_draws_latin_and_cjk_in_their_cells() {
        for c in ['M', 'g', '\u{5c0f}', '\u{660e}', '\u{d55c}', '\u{416}'] {
            assert!(Face::Screen.draws(c), "{c:?}");
        }
        let cell = Face::Screen.cell(1);
        let wide = inked(&painted_in("\u{5c0f}", Face::Screen, cell), FG);
        assert!(
            wide.iter().any(|&(x, _)| x >= cell.w),
            "a CJK glyph spans two cells"
        );
        assert!(wide.iter().all(|&(x, _)| x < 2 * cell.w));
    }

    fn painted_in(text: &str, face: Face, cell: CellPx) -> RgbBuffer {
        let mut grid = CellGrid::new(crate::display::text::cells(text), 1);
        grid.put((0, 0), text, None, false);
        let mut buf = RgbBuffer::filled(grid.width() * cell.w * 2, cell.h * 2, BG);
        paint_grid(&mut buf, &grid, (0, 0), cell, face, INK);
        buf
    }
}
