//! A tile as an iTerm2 inline image, one PNG each
//! (<https://iterm2.com/documentation-images.html>).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the compositor wires the tiles")
)]

use ratatui::layout::Position;

use super::tiles::TileImage;

/// The cursor move to `image`'s top-left cell and the inline image that
/// draws it there, the image's own top-left cell being `origin`.
///
/// Sized in cells with the aspect ratio unkept, so iTerm2 fits the image to
/// exactly the tile's cells, which it already spans pixel for pixel.
pub(crate) fn transmit(image: &TileImage, origin: Position) -> Result<Vec<u8>, png::EncodingError> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgb)?;
    let mut out = image.tile.cursor_to(origin).into_bytes();
    out.extend_from_slice(
        format!(
            "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=0:",
            image.tile.cols, image.tile.rows
        )
        .as_bytes(),
    );
    out.extend_from_slice(base64_simd::STANDARD.encode_to_string(&png).as_bytes());
    out.extend_from_slice(b"\x1b\\");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::tiles::Tile;

    fn image() -> TileImage {
        TileImage {
            tile: Tile {
                index: 0,
                col: 8,
                row: 4,
                cols: 2,
                rows: 1,
            },
            width: 4,
            height: 3,
            rgb: (0..4 * 3 * 3).map(|i| i * 7).collect(),
        }
    }

    /// The escape around the payload, split there.
    fn parts(bytes: &[u8]) -> (String, String) {
        let text = String::from_utf8(bytes.to_vec()).expect("ascii");
        let (head, payload) = text.split_once(':').expect("payload");
        let payload = payload.strip_suffix("\x1b\\").expect("ST");
        (head.to_string(), payload.to_string())
    }

    #[test]
    fn a_tile_is_a_cursor_move_and_an_inline_image_sized_in_its_cells() {
        let bytes = transmit(&image(), Position::new(1, 2)).expect("encodes");
        assert_eq!(
            parts(&bytes).0,
            "\x1b[7;10H\x1b]1337;File=inline=1;width=2;height=1;preserveAspectRatio=0"
        );
    }

    #[test]
    fn the_payload_is_a_png_of_the_tiles_pixels() {
        let img = image();
        let bytes = transmit(&img, Position::new(0, 0)).expect("encodes");
        let png = base64_simd::STANDARD
            .decode_to_vec(parts(&bytes).1)
            .expect("base64");
        let decoded = ::image::load_from_memory_with_format(&png, ::image::ImageFormat::Png)
            .expect("png")
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (img.width, img.height));
        assert_eq!(decoded.into_raw(), img.rgb);
    }
}
