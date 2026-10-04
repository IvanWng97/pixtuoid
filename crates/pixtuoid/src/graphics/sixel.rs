//! A tile as a DEC SIXEL image (VT330/VT340 Programmer Reference, ch. 14:
//! <https://vt100.net/docs/vt3xx-gp/chapter14.html>): every pixel its own
//! colour up to [`REGISTERS`] distinct colours, a fixed cube beyond, and no
//! error diffusion, so a still office never shimmers.

use std::collections::HashMap;
use std::fmt::Write as _;

use ratatui::layout::Position;

use super::tiles::TileImage;

/// The RGB colour introducer's unit: `#Pc;2;Px;Py;Pz` takes percentages.
const PERCENT: u16 = 100;

/// The colour registers a tile may define: as many as ratatui-image's own
/// SIXEL encoder uses (`icy_sixel` 0.5.0 `encoder.rs:126`).
const REGISTERS: usize = 256;

/// One step of the fixed colour cube a tile of more colours than
/// [`REGISTERS`] falls back to.
const CUBE_STEP: u8 = 20;
const _: () = assert!((PERCENT as usize / CUBE_STEP as usize + 1).pow(3) <= REGISTERS);

/// Pixel rows per sixel.
const BAND: usize = 6;

/// A sixel's character is its six bits over `?`.
const SIXEL_BIAS: u8 = b'?';

/// A run this long or longer is shorter as `!Pn` than spelled out.
const MIN_REPEAT: usize = 4;

/// The cursor move to `image`'s top-left cell and the SIXEL that draws it
/// there, the image's own top-left cell being `origin`.
///
/// `P1=9` and the raster attributes' `1;1` are square pixels. `P2=1` leaves
/// an unset pixel as it was, where `0` would paint it background: a tile
/// whose height is not a multiple of [`BAND`] has unset rows at the foot of
/// its last band, over the next tile's top.
pub(crate) fn transmit(image: &TileImage, origin: Position) -> Vec<u8> {
    let percent: Vec<[u8; 3]> = image
        .rgb
        .as_chunks()
        .0
        .iter()
        .map(|p| p.map(to_percent))
        .collect();
    let (palette, indices) = match indexed(&percent) {
        exact if exact.0.len() <= REGISTERS => exact,
        _ => indexed(&percent.iter().map(|c| c.map(to_cube)).collect::<Vec<_>>()),
    };
    let width = image.width as usize;
    let mut out = image.tile.cursor_to(origin);
    let _ = write!(out, "\x1bP9;1q\"1;1;{};{}", image.width, image.height);
    for (i, [r, g, b]) in palette.iter().enumerate() {
        let _ = write!(out, "#{i};2;{r};{g};{b}");
    }
    let rows: Vec<&[usize]> = indices.chunks(width.max(1)).collect();
    for (n, band) in rows.chunks(BAND).enumerate() {
        if n > 0 {
            out.push('-');
        }
        let mut colours: Vec<usize> = band.iter().flat_map(|row| row.iter().copied()).collect();
        colours.sort_unstable();
        colours.dedup();
        for (m, &colour) in colours.iter().enumerate() {
            if m > 0 {
                out.push('$');
            }
            let _ = write!(out, "#{colour}");
            let sixel = |x: usize| {
                band.iter()
                    .enumerate()
                    .fold(0, |bits, (y, row)| bits | u8::from(row[x] == colour) << y)
            };
            let mut x = 0;
            while x < width {
                let bits = sixel(x);
                let run = (x..width).take_while(|&x| sixel(x) == bits).count();
                let ch = char::from(SIXEL_BIAS + bits);
                if run >= MIN_REPEAT {
                    let _ = write!(out, "!{run}{ch}");
                } else {
                    out.extend(std::iter::repeat_n(ch, run));
                }
                x += run;
            }
        }
    }
    out.push_str("\x1b\\");
    out.into_bytes()
}

/// An 8-bit channel as the nearest whole percent.
fn to_percent(v: u8) -> u8 {
    let (v, max) = (u16::from(v), u16::from(u8::MAX));
    // At most `PERCENT`, so it fits.
    ((v * PERCENT + max / 2) / max) as u8
}

/// A percent channel as the nearest level of the fallback cube.
fn to_cube(v: u8) -> u8 {
    (v + CUBE_STEP / 2) / CUBE_STEP * CUBE_STEP
}

/// The distinct colours of `pixels` in first-seen order, and each pixel's
/// index among them.
fn indexed(pixels: &[[u8; 3]]) -> (Vec<[u8; 3]>, Vec<usize>) {
    let mut palette = Vec::new();
    let mut seen = HashMap::new();
    let indices = pixels
        .iter()
        .map(|&c| {
            *seen.entry(c).or_insert_with(|| {
                palette.push(c);
                palette.len() - 1
            })
        })
        .collect();
    (palette, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::tiles::Tile;

    fn image(width: u32, height: u32, rgb: Vec<u8>) -> TileImage {
        TileImage {
            tile: Tile {
                index: 0,
                col: 2,
                row: 1,
                cols: 1,
                rows: 1,
            },
            width,
            height,
            rgb,
        }
    }

    /// The pixels the terminal would show, through a decoder we did not
    /// write, in whole bands: `None` where the SIXEL left the screen as it was.
    fn decode(sixel: &[u8]) -> (usize, usize, Vec<Option<[u8; 3]>>) {
        let dcs = &sixel[sixel.windows(2).position(|w| w == b"\x1bP").expect("dcs")..];
        let img = icy_sixel::SixelImage::decode(dcs).expect("decodes");
        let px = img
            .pixels
            .as_chunks()
            .0
            .iter()
            .map(|&[r, g, b, a]| (a == u8::MAX).then_some([r, g, b]))
            .collect();
        (img.width, img.height, px)
    }

    /// `pixels` drawn, and the rest of their last band untouched.
    fn shown(width: usize, pixels: &[[u8; 3]]) -> (usize, usize, Vec<Option<[u8; 3]>>) {
        let height = pixels.len() / width;
        let mut px: Vec<_> = pixels.iter().copied().map(Some).collect();
        px.resize(height.next_multiple_of(BAND) * width, None);
        (width, height.next_multiple_of(BAND), px)
    }

    fn registers(sixel: &[u8]) -> usize {
        String::from_utf8(sixel.to_vec())
            .expect("ascii")
            .matches(";2;")
            .count()
    }

    #[test]
    fn a_tile_is_a_cursor_move_and_one_transparent_square_pixel_sixel() {
        assert_eq!(
            transmit(
                &image(2, 1, vec![255, 0, 0, 0, 0, 255]),
                Position::new(10, 5)
            ),
            b"\x1b[7;13H\x1bP9;1q\"1;1;2;1#0;2;100;0;0#1;2;0;0;100#0@?$#1?@\x1b\\"
        );
    }

    /// Runs of [`MIN_REPEAT`] or more collapse; shorter ones are spelled out.
    #[test]
    fn long_runs_use_the_repeat_introducer() {
        let sixel = |n: usize| {
            String::from_utf8(transmit(
                &image(n as u32, 1, vec![0; 3 * n]),
                Position::new(0, 0),
            ))
            .expect("ascii")
        };
        assert!(sixel(MIN_REPEAT - 1).ends_with("#0@@@\x1b\\"));
        assert!(sixel(MIN_REPEAT).ends_with(&format!("#0!{MIN_REPEAT}@\x1b\\")));
    }

    /// 13 rows are two whole six-pixel bands and a one-row band, whose other
    /// five rows stay as they were; colours that are whole percents survive
    /// the round trip exactly.
    #[test]
    fn the_terminal_decodes_every_band_to_the_tiles_pixels() {
        let levels = [0u8, 51, 102, 153, 204, 255];
        let (w, h) = (3, 13);
        let pixels: Vec<[u8; 3]> = (0..w * h)
            .map(|i| [levels[i % 6], levels[i / 6 % 6], levels[i / 36 % 6]])
            .collect();
        let sixel = transmit(
            &image(w as u32, h as u32, pixels.concat()),
            Position::new(0, 0),
        );
        assert_eq!(registers(&sixel), w * h);
        assert_eq!(decode(&sixel), shown(w, &pixels));
    }

    /// Two shades a quantizer would merge (`icy_sixel`'s Wu does) stay two,
    /// and the checker comes back cell for cell: nothing diffused.
    #[test]
    fn a_two_colour_checker_keeps_exactly_its_two_colours() {
        let (a, b) = ([100u8, 60, 40], [102u8, 61, 41]);
        let (w, h) = (8, 8);
        let pixels: Vec<[u8; 3]> = (0..w * h)
            .map(|i| if (i % w + i / w) % 2 == 0 { a } else { b })
            .collect();
        let sixel = transmit(
            &image(w as u32, h as u32, pixels.concat()),
            Position::new(0, 0),
        );
        assert_eq!(registers(&sixel), 2);
        let percent = |(_, _, px): (usize, usize, Vec<Option<[u8; 3]>>)| -> Vec<_> {
            px.into_iter()
                .map(|p| p.map(|c| c.map(to_percent)))
                .collect()
        };
        assert_eq!(percent(decode(&sixel)), percent(shown(w, &pixels)));
        assert_ne!(a.map(to_percent), b.map(to_percent));
    }

    /// Past [`REGISTERS`] colours every pixel takes its nearest cube colour,
    /// the same in every tile, so neighbouring tiles agree.
    #[test]
    fn more_colours_than_registers_fall_back_to_the_cube() {
        let level = |n: usize| (n % 16 * 17) as u8;
        let pixels: Vec<[u8; 3]> = (0..=REGISTERS)
            .map(|i| [level(i), level(i / 16), level(i / 256)])
            .collect();
        let n = pixels.len() as u32;
        let sixel = transmit(&image(n, 1, pixels.concat()), Position::new(0, 0));
        let text = String::from_utf8(sixel).expect("ascii");
        let defs: Vec<&str> = text.split('#').filter(|d| d.contains(";2;")).collect();
        assert!(defs.len() <= REGISTERS);
        for def in defs {
            for v in def.split(';').skip(2) {
                let v: u8 = v.parse().expect("percent");
                assert_eq!(v % CUBE_STEP, 0, "{def}");
            }
        }
    }

    /// At exactly [`REGISTERS`] colours none is approximated.
    #[test]
    fn a_full_palette_is_kept_exact() {
        let level = |n: usize| (n % 16 * 17) as u8;
        let pixels: Vec<[u8; 3]> = (0..REGISTERS)
            .map(|i| [level(i), level(i / 16), 0])
            .collect();
        let sixel = transmit(
            &image(REGISTERS as u32, 1, pixels.concat()),
            Position::new(0, 0),
        );
        assert_eq!(registers(&sixel), REGISTERS);
    }

    #[test]
    fn channels_round_to_the_nearest_percent() {
        assert_eq!(
            [0, 1, 2, 3, 127, 128, 254, 255].map(to_percent),
            [0, 0, 1, 1, 50, 50, 100, 100]
        );
        assert_eq!(
            [0, 9, 10, 29, 30, 100].map(to_cube),
            [0, 0, 20, 20, 40, 100]
        );
    }
}
