//! A tile as a kitty image, shown through Unicode placeholders: the image is
//! then ordinary text in cells, which a host like tmux stores and redraws
//! (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>, "Unicode placeholders").

use ratatui::style::Color;
use ratatui_image::picker::cap_parser::Parser;

use super::tiles::{Tile, TileImage};

const PLACEHOLDER: char = '\u{10EEEE}';

/// The numbers 0.. as row/column diacritics: the first rows of kitty's
/// `gen/rowcolumn-diacritics.txt`, one per cell of a tile's longer side.
const DIACRITICS: [char; 4] = ['\u{305}', '\u{30D}', '\u{30E}', '\u{310}'];

/// One past the largest id a placeholder carries without the most-significant
/// byte's diacritic: the foreground colour's 24 bits.
const ID_LIMIT: u32 = 1 << 24;

/// Base64 bytes per escape ("Remote client").
const CHUNK: usize = 4096;

/// A placeholder cell, in cells from the image's top-left.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) struct Placeholder {
    /// Its column.
    pub(crate) col: u16,
    /// Its row.
    pub(crate) row: u16,
    /// The placeholder and its row and column diacritics.
    pub(crate) symbol: String,
    /// The image id, as a 24-bit colour.
    pub(crate) fg: Color,
}

/// `tile`'s image id, the same every frame so a re-send replaces it in place;
/// `None` past what a placeholder can carry. 0 is no id.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) fn image_id(tile: Tile) -> Option<u32> {
    tile.index.checked_add(1).filter(|&id| id < ID_LIMIT)
}

/// The escapes that (re)transmit `image` as image `id` and make its virtual
/// placement over the tile's cells, each wrapped for tmux's passthrough
/// (tmux(1), `allow-passthrough`) when `tmux`.
///
/// `q=2` on every chunk: a reply would arrive as input mid-frame.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) fn transmit(id: u32, image: &TileImage, tmux: bool) -> Vec<u8> {
    let (start, esc, end) = Parser::tmux_start_escape_end(tmux);
    let data = base64_simd::STANDARD.encode_to_string(&image.rgb);
    let chunks = data.as_bytes().chunks(CHUNK);
    let last = chunks.len().saturating_sub(1);
    let mut out = Vec::with_capacity(data.len() + 128);
    for (i, chunk) in chunks.enumerate() {
        out.extend_from_slice(format!("{start}{esc}_G").as_bytes());
        if i == 0 {
            let TileImage {
                tile,
                width,
                height,
                ..
            } = image;
            out.extend_from_slice(
                format!(
                    "a=T,U=1,i={id},f=24,s={width},v={height},c={},r={},",
                    tile.cols, tile.rows
                )
                .as_bytes(),
            );
        }
        out.extend_from_slice(format!("q=2,m={};", u8::from(i < last)).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(format!("{esc}\\{end}").as_bytes());
    }
    out
}

/// The cells that show image `id` over `tile`. Every cell names its row and
/// column, so none leans on the cell to its left being there.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) fn placeholders(id: u32, tile: Tile) -> impl Iterator<Item = Placeholder> {
    let [_, r, g, b] = id.to_be_bytes();
    (0..tile.rows).flat_map(move |y| {
        (0..tile.cols).map(move |x| Placeholder {
            col: tile.col + x,
            row: tile.row + y,
            symbol: [
                PLACEHOLDER,
                DIACRITICS[usize::from(y)],
                DIACRITICS[usize::from(x)],
            ]
            .iter()
            .collect(),
            fg: Color::Rgb(r, g, b),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::tiles::Tiles;
    use crate::graphics::{CellSize, Fit, ImageProtocol};
    use pixtuoid_core::sprite::format::Density;
    use pixtuoid_core::sprite::{Rgb, RgbBuffer};
    use pixtuoid_scene::cutaway::canvas::Dirty;
    use ratatui::layout::Size as TermSize;

    fn tile(index: u32, cols: u16, rows: u16) -> Tile {
        Tile {
            index,
            col: 0,
            row: 0,
            cols,
            rows,
        }
    }

    fn image(rgb: Vec<u8>) -> TileImage {
        TileImage {
            tile: tile(0, 1, 1),
            width: 2,
            height: 1,
            rgb,
        }
    }

    fn escapes(bytes: &[u8]) -> Vec<String> {
        String::from_utf8(bytes.to_vec())
            .expect("ascii")
            .split_inclusive("\x1b\\")
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_tile_is_one_quiet_rgb_transmit_with_a_virtual_placement() {
        assert_eq!(
            transmit(1, &image(vec![1, 2, 3, 4, 5, 6]), false),
            b"\x1b_Ga=T,U=1,i=1,f=24,s=2,v=1,c=1,r=1,q=2,m=0;AQIDBAUG\x1b\\"
        );
    }

    /// Every escape but the last carries a whole chunk and says more follow;
    /// the rest name nothing but `m` and `q`.
    #[test]
    fn the_payload_splits_at_the_chunk_size() {
        let raw = CHUNK / 4 * 3;
        assert_eq!(escapes(&transmit(1, &image(vec![0; raw]), false)).len(), 1);
        let two = escapes(&transmit(1, &image(vec![0; raw + 1]), false));
        assert_eq!(two.len(), 2);
        let payload = |e: &str| e[e.find(';').expect("payload") + 1..e.len() - 2].len();
        assert!(two[0].contains(",m=1;"));
        assert_eq!(payload(&two[0]), CHUNK);
        assert_eq!(two[1], "\x1b_Gq=2,m=0;AA==\x1b\\");
    }

    /// Inside tmux each escape rides passthrough, its own ESCs doubled.
    #[test]
    fn inside_tmux_each_escape_is_wrapped() {
        assert_eq!(
            transmit(1, &image(vec![1, 2, 3, 4, 5, 6]), true),
            b"\x1bPtmux;\x1b\x1b_Ga=T,U=1,i=1,f=24,s=2,v=1,c=1,r=1,q=2,m=0;AQIDBAUG\x1b\x1b\\\x1b\\"
        );
    }

    /// The spec's own 2x2 example, for image 42.
    #[test]
    fn placeholders_carry_the_id_and_each_cells_row_and_column() {
        let at = Tile {
            col: 4,
            row: 2,
            ..tile(41, 2, 2)
        };
        let id = image_id(at).expect("in range");
        assert_eq!(id, 42);
        let cells: Vec<_> = placeholders(id, at)
            .map(|p| (p.col, p.row, p.symbol, p.fg))
            .collect();
        let fg = Color::Rgb(0, 0, 42);
        assert_eq!(
            cells,
            [
                (4, 2, "\u{10EEEE}\u{305}\u{305}".to_string(), fg),
                (5, 2, "\u{10EEEE}\u{305}\u{30D}".to_string(), fg),
                (4, 3, "\u{10EEEE}\u{30D}\u{305}".to_string(), fg),
                (5, 3, "\u{10EEEE}\u{30D}\u{30D}".to_string(), fg),
            ]
        );
    }

    /// Against the rows of `gen/rowcolumn-diacritics.txt` (kitty@b2cb871).
    #[test]
    fn the_diacritics_number_every_cell_of_a_kitty_tile() {
        assert_eq!(DIACRITICS, ['\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}']);
        let shape = ImageProtocol::Kitty.tile();
        assert!(usize::from(shape.cols.max(shape.rows)) <= DIACRITICS.len());
    }

    #[test]
    fn ids_stop_where_the_foreground_colour_does() {
        assert_eq!(image_id(tile(ID_LIMIT - 2, 1, 1)), Some(ID_LIMIT - 1));
        assert_eq!(image_id(tile(ID_LIMIT - 1, 1, 1)), None);
        assert_eq!(image_id(tile(u32::MAX, 1, 1)), None);
    }

    /// An idle office sends the terminal nothing.
    #[test]
    fn an_idle_office_sends_zero_bytes() {
        let cell = CellSize { w: 8, h: 16 };
        let area = TermSize {
            width: 80,
            height: 24,
        };
        let fit = Fit::new(cell, area, Density::new(4).expect("nonzero")).expect("fits");
        let mut tiles = Tiles::new(ImageProtocol::Kitty, cell, fit);
        let buf = RgbBuffer::filled(64, 40, Rgb { r: 1, g: 2, b: 3 });
        let mut sent = |dirty: &Dirty| -> usize {
            tiles
                .changed(&buf, dirty)
                .into_iter()
                .map(|t| transmit(image_id(t).expect("id"), &tiles.image(&buf, t), false).len())
                .sum()
        };
        assert!(sent(&Dirty::All) > 0);
        assert_eq!(sent(&Dirty::Rects(vec![])), 0);
        assert_eq!(sent(&Dirty::All), 0);
    }
}
