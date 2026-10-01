//! The cutaway image cut into a grid of cell-aligned tiles, and which of them a
//! frame changed: an encoder re-sends only those.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

#[cfg(feature = "graphics")]
use crossterm::{Command, cursor::MoveTo};
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_scene::cutaway::canvas::Dirty;
use pixtuoid_scene::layout::Bounds;
#[cfg(feature = "graphics")]
use ratatui::layout::Position;

use super::{CellSize, Fit, ImageProtocol};

/// A tile's extent in cells; [`ImageProtocol::tile`] is the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileShape {
    /// Cells across.
    pub(crate) cols: u16,
    /// Cells down.
    pub(crate) rows: u16,
}

/// One tile, in cells from the image's top-left cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) struct Tile {
    /// Row-major, stable while the buffer keeps its size.
    pub(crate) index: u32,
    /// Its left cell.
    pub(crate) col: u16,
    /// Its top cell.
    pub(crate) row: u16,
    /// Cells across: fewer than the shape's in the right-hand column.
    pub(crate) cols: u16,
    /// Cells down: fewer than the shape's in the bottom row.
    pub(crate) rows: u16,
}

#[cfg(feature = "graphics")]
impl Tile {
    /// The escape that moves the cursor to this tile's top-left cell, the
    /// image's own being `origin`: where a SIXEL or iTerm2 image is drawn.
    pub(crate) fn cursor_to(self, origin: Position) -> String {
        let mut out = String::new();
        // Writing to a `String` cannot fail.
        let _ = MoveTo(
            origin.x.saturating_add(self.col),
            origin.y.saturating_add(self.row),
        )
        .write_ansi(&mut out);
        out
    }
}

/// A tile's pixels as the terminal shows them: upscaled, in whole cells.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) struct TileImage {
    /// The tile it is.
    pub(crate) tile: Tile,
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    /// Row-major 8-bit RGB.
    pub(crate) rgb: Vec<u8>,
}

/// The tile grid over one image and the hash each tile was last emitted with.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
pub(crate) struct Tiles {
    shape: TileShape,
    cell: CellSize,
    upscale: u32,
    /// The buffer size `sent` describes.
    size: (u16, u16),
    /// Per tile, its pixels' hash as last emitted; `None` until then.
    sent: Vec<Option<u64>>,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]
impl Tiles {
    /// The grid `protocol` re-sends in, over an image `fit` lays on `cell`.
    pub(crate) fn new(protocol: ImageProtocol, cell: CellSize, fit: Fit) -> Self {
        Self {
            shape: protocol.tile(),
            cell,
            upscale: u32::from(fit.upscale()),
            size: (0, 0),
            sent: Vec::new(),
        }
    }

    /// The tiles whose pixels differ from what was last emitted for them,
    /// among those `dirty` reaches, each now counted as emitted.
    pub(crate) fn changed(&mut self, buf: &RgbBuffer, dirty: &Dirty) -> Vec<Tile> {
        let size = (buf.width(), buf.height());
        let regrid = size != self.size;
        if regrid {
            self.size = size;
            let (across, down) = self.across_down();
            self.sent = vec![None; (across * down) as usize];
        }
        let candidates: Vec<u32> = match dirty {
            Dirty::Rects(rects) if !regrid => {
                let mut v: Vec<u32> = rects.iter().flat_map(|&r| self.reached(r)).collect();
                v.sort_unstable();
                v.dedup();
                v
            }
            _ => (0..self.sent.len() as u32).collect(),
        };
        let mut changed = Vec::new();
        for index in candidates {
            let tile = self.tile(index);
            let hash = Some(self.hash(buf, tile));
            if self.sent[index as usize] != hash {
                self.sent[index as usize] = hash;
                changed.push(tile);
            }
        }
        changed
    }

    /// `tile`'s pixels from `buf`, upscaled. A cell the image only partly
    /// covers is filled out with the image's edge pixels, so every tile is
    /// whole cells and no terminal resamples it.
    pub(crate) fn image(&self, buf: &RgbBuffer, tile: Tile) -> TileImage {
        let (cw, ch) = (u32::from(self.cell.w), u32::from(self.cell.h));
        let (width, height) = (u32::from(tile.cols) * cw, u32::from(tile.rows) * ch);
        let src = |px: u32, edge: u16| {
            u16::try_from(px / self.upscale).map_or(edge - 1, |b| b.min(edge - 1))
        };
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            let by = src(u32::from(tile.row) * ch + y, buf.height());
            for x in 0..width {
                let p = buf.get(src(u32::from(tile.col) * cw + x, buf.width()), by);
                rgb.extend_from_slice(&[p.r, p.g, p.b]);
            }
        }
        TileImage {
            tile,
            width,
            height,
            rgb,
        }
    }

    /// One tile's extent in image pixels on each axis.
    fn tile_px(&self) -> (u32, u32) {
        (
            u32::from(self.shape.cols) * u32::from(self.cell.w),
            u32::from(self.shape.rows) * u32::from(self.cell.h),
        )
    }

    fn image_px(&self) -> (u32, u32) {
        (
            u32::from(self.size.0) * self.upscale,
            u32::from(self.size.1) * self.upscale,
        )
    }

    fn across_down(&self) -> (u32, u32) {
        let ((w, h), (tw, th)) = (self.image_px(), self.tile_px());
        (w.div_ceil(tw), h.div_ceil(th))
    }

    fn tile(&self, index: u32) -> Tile {
        let (across, _) = self.across_down();
        let (iw, ih) = self.image_px();
        let (cells_w, cells_h) = (
            iw.div_ceil(u32::from(self.cell.w)),
            ih.div_ceil(u32::from(self.cell.h)),
        );
        let (col, row) = (
            index % across * u32::from(self.shape.cols),
            index / across * u32::from(self.shape.rows),
        );
        // The image is at most `u16::MAX` px a side (`Fit::new`), so its
        // cells are too.
        let cells = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
        Tile {
            index,
            col: cells(col),
            row: cells(row),
            cols: cells(u32::from(self.shape.cols).min(cells_w - col)),
            rows: cells(u32::from(self.shape.rows).min(cells_h - row)),
        }
    }

    /// The tiles a buffer-pixel rect overlaps.
    fn reached(&self, r: Bounds) -> impl Iterator<Item = u32> + use<> {
        let (across, down) = self.across_down();
        let (tw, th) = self.tile_px();
        let k = self.upscale;
        let span = |start: u16, len: u16, tile: u32, limit: u32| {
            let (a, b) = (u32::from(start), u32::from(start) + u32::from(len));
            if len == 0 {
                0..0
            } else {
                (a * k / tile).min(limit)..(b * k).div_ceil(tile).min(limit)
            }
        };
        let (xs, ys) = (
            span(r.x, r.width, tw, across),
            span(r.y, r.height, th, down),
        );
        ys.flat_map(move |ty| xs.clone().map(move |tx| ty * across + tx))
    }

    /// The buffer pixels `tile`'s upscaled pixels come from, on one axis.
    fn source(&self, cell0: u16, cells: u16, cell_px: u16, image_px: u32) -> Range<usize> {
        let a = u32::from(cell0) * u32::from(cell_px);
        let b = (a + u32::from(cells) * u32::from(cell_px)).min(image_px);
        (a / self.upscale) as usize..b.div_ceil(self.upscale) as usize
    }

    fn hash(&self, buf: &RgbBuffer, tile: Tile) -> u64 {
        let (iw, ih) = self.image_px();
        let xs = self.source(tile.col, tile.cols, self.cell.w, iw);
        let pixels = buf.as_slice();
        let mut h = DefaultHasher::new();
        for y in self.source(tile.row, tile.rows, self.cell.h, ih) {
            let row = y * usize::from(buf.width());
            Hash::hash_slice(&pixels[row + xs.start..row + xs.end], &mut h);
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::sprite::Rgb;
    use pixtuoid_core::sprite::format::Density;
    use ratatui::layout::Size as TermSize;

    const AREA: TermSize = TermSize {
        width: 200,
        height: 100,
    };

    /// A grid whose upscaled pixels straddle cells: a 5-px cell fits scale 6
    /// on 2x art, so each art pixel is 3 image pixels.
    fn misaligned() -> Tiles {
        let cell = CellSize { w: 5, h: 10 };
        let fit = Fit::new(cell, AREA, Density::new(2).expect("nonzero")).expect("fits");
        assert_eq!(fit.upscale(), 3);
        Tiles::new(ImageProtocol::Kitty, cell, fit)
    }

    fn buffer(w: u16, h: u16) -> RgbBuffer {
        let px = (0..u32::from(w) * u32::from(h))
            .map(|i| Rgb {
                r: i as u8,
                g: (i >> 8) as u8,
                b: 7,
            })
            .collect();
        RgbBuffer::from_pixels(w, h, px)
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Dirty {
        Dirty::Rects(vec![Bounds {
            x,
            y,
            width,
            height,
        }])
    }

    fn poke(buf: &mut RgbBuffer, x: u16, y: u16) {
        let p = buf.get(x, y);
        buf.put(x, y, Rgb { r: !p.r, ..p });
    }

    fn indices(tiles: &[Tile]) -> Vec<u32> {
        tiles.iter().map(|t| t.index).collect()
    }

    /// 13x7 art px upscaled 3x is 39x21 image px: 8x3 cells of 5x10, the
    /// last column and row only partly covered, in kitty's 4x2-cell tiles.
    #[test]
    fn the_grid_covers_the_image_in_whole_cells_with_short_edge_tiles() {
        let mut t = misaligned();
        let all = t.changed(&buffer(13, 7), &Dirty::All);
        let shape = |t: &Tile| (t.index, t.col, t.row, t.cols, t.rows);
        assert_eq!(
            all.iter().map(shape).collect::<Vec<_>>(),
            [
                (0, 0, 0, 4, 2),
                (1, 4, 0, 4, 2),
                (2, 0, 2, 4, 1),
                (3, 4, 2, 4, 1)
            ]
        );
        let mut t = misaligned();
        let all = t.changed(&buffer(14, 7), &Dirty::All);
        assert_eq!(
            all.iter().map(shape).collect::<Vec<_>>()[2],
            (2, 8, 0, 1, 2),
            "42 image px is 9 cells: a third, one-cell column"
        );
    }

    #[test]
    fn the_tile_shape_is_the_protocols() {
        assert_eq!(ImageProtocol::Kitty.tile(), TileShape { cols: 4, rows: 2 });
        for p in [ImageProtocol::Sixel, ImageProtocol::Iterm2] {
            assert_eq!(p.tile(), TileShape { cols: 8, rows: 4 });
        }
    }

    /// An idle office emits nothing: a frame the canvas did not paint names
    /// no tile, whatever the buffer holds, and a repaint of the same pixels
    /// re-sends none.
    #[test]
    fn an_unchanged_frame_emits_no_tile() {
        let mut t = misaligned();
        let mut buf = buffer(13, 7);
        assert_eq!(t.changed(&buf, &Dirty::All).len(), 4);
        assert_eq!(t.changed(&buf, &Dirty::All), []);
        assert_eq!(t.changed(&buf, &rect(0, 0, 13, 7)), []);
        poke(&mut buf, 0, 0);
        assert_eq!(t.changed(&buf, &Dirty::Rects(vec![])), []);
    }

    /// A rect re-sends the tiles it overlaps once their pixels changed, and
    /// only those: art column 6 is image px 18..21, which straddles kitty's
    /// 20-px tile edge.
    #[test]
    fn a_dirty_rect_reaches_exactly_the_tiles_it_overlaps() {
        let mut t = misaligned();
        let mut buf = buffer(13, 7);
        t.changed(&buf, &Dirty::All);
        poke(&mut buf, 6, 0);
        poke(&mut buf, 12, 6);
        assert_eq!(indices(&t.changed(&buf, &rect(6, 0, 1, 1))), [0, 1]);
        assert_eq!(indices(&t.changed(&buf, &rect(0, 0, 6, 7))), [0u32; 0]);
        assert_eq!(indices(&t.changed(&buf, &rect(12, 6, 0, 0))), [0u32; 0]);
        assert_eq!(indices(&t.changed(&buf, &rect(12, 6, 1, 1))), [3]);
    }

    /// A buffer of a new size is a new grid: every tile is re-sent, even when
    /// the frame names only a rect.
    #[test]
    fn a_new_buffer_size_re_sends_every_tile() {
        let mut t = misaligned();
        t.changed(&buffer(13, 7), &Dirty::All);
        assert_eq!(t.changed(&buffer(14, 7), &rect(0, 0, 1, 1)).len(), 6);
    }

    /// The hash reads art pixels, before the upscale, which is k² fewer: it is
    /// exact because a tile changes on screen precisely when an art pixel it
    /// samples does.
    #[test]
    fn hashing_before_the_upscale_misses_no_change_on_screen() {
        let base = buffer(13, 7);
        for (x, y) in (0..13).flat_map(|x| (0..7).map(move |y| (x, y))) {
            let (mut t, mut buf) = (misaligned(), base.clone());
            let before: Vec<TileImage> = t
                .changed(&buf, &Dirty::All)
                .into_iter()
                .map(|tile| t.image(&buf, tile))
                .collect();
            poke(&mut buf, x, y);
            let on_screen: Vec<u32> = before
                .iter()
                .filter(|was| t.image(&buf, was.tile) != **was)
                .map(|was| was.tile.index)
                .collect();
            assert_eq!(
                indices(&t.changed(&buf, &Dirty::All)),
                on_screen,
                "({x},{y})"
            );
        }
    }

    /// Each image pixel is the art pixel it upscales; past the image's edge,
    /// the edge pixel.
    #[test]
    fn a_tile_image_is_whole_cells_of_upscaled_art() {
        let (mut t, buf) = (misaligned(), buffer(13, 7));
        let corner = t.changed(&buf, &Dirty::All)[3];
        let img = t.image(&buf, corner);
        assert_eq!((img.width, img.height), (20, 10));
        assert_eq!(img.rgb.len(), 20 * 10 * 3);
        let at = |x: u32, y: u32| {
            let i = ((y * img.width + x) * 3) as usize;
            Rgb {
                r: img.rgb[i],
                g: img.rgb[i + 1],
                b: img.rgb[i + 2],
            }
        };
        // Image px (20, 20) is art (6, 6).
        assert_eq!(at(0, 0), buf.get(6, 6));
        assert_eq!(at(1, 0), buf.get(7, 6));
        assert_eq!(at(18, 0), buf.get(12, 6), "image px 38, the last");
        assert_eq!(at(19, 9), buf.get(12, 6), "past the edge");
    }
}
