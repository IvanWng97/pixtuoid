use std::num::NonZeroU16;

use crate::sprite::{Frame, RgbBuffer};

/// Blit a sprite frame into `dst` with top-left at `(dst_x, dst_y)`.
/// Transparent (None) pixels leave `dst` unchanged. Out-of-bounds pixels
/// are silently clipped.
pub fn blit_frame(frame: &Frame, dst_x: u16, dst_y: u16, dst: &mut RgbBuffer) {
    for fy in 0..frame.height {
        for fx in 0..frame.width {
            let i = (fy as usize) * (frame.width as usize) + (fx as usize);
            let Some(rgb) = frame.as_slice()[i] else {
                continue;
            };
            let x = dst_x.saturating_add(fx);
            let y = dst_y.saturating_add(fy);
            if x >= dst.width || y >= dst.height {
                continue;
            }
            dst.put(x, y, rgb);
        }
    }
}

/// Blit a sprite frame with each source pixel expanded to a `scale × scale`
/// block, top-left at `(dst_x, dst_y)`.
///
/// This is the FALLBACK for art authored at a lower density than the buffer is
/// being painted at: it makes a 1x pack fill a scaled render honestly, and a
/// pack authored at the render scale blits through [`blit_frame`] untouched, so
/// swapping in richer art removes the upscale rather than fighting it.
///
/// Nearest-neighbour and integer-only, deliberately — pixel art must land on
/// exact pixel boundaries, and any filtering blurs the very edges the style is
/// made of. Not `image::imageops::resize`: that would pull a large dependency
/// into the headless core for a doubled loop, and it cannot composite
/// transparency into an existing buffer, which is the whole job here.
///
/// `scale` is [`NonZeroU16`] so a zero factor — which would silently paint
/// nothing — is unrepresentable rather than merely documented.
pub fn blit_frame_scaled(
    frame: &Frame,
    dst_x: u16,
    dst_y: u16,
    scale: NonZeroU16,
    dst: &mut RgbBuffer,
) {
    let s = scale.get();
    for fy in 0..frame.height {
        for fx in 0..frame.width {
            let i = (fy as usize) * (frame.width as usize) + (fx as usize);
            let Some(rgb) = frame.as_slice()[i] else {
                continue;
            };
            let bx = dst_x.saturating_add(fx.saturating_mul(s));
            let by = dst_y.saturating_add(fy.saturating_mul(s));
            for sy in 0..s {
                for sx in 0..s {
                    let x = bx.saturating_add(sx);
                    let y = by.saturating_add(sy);
                    if x >= dst.width || y >= dst.height {
                        continue;
                    }
                    dst.put(x, y, rgb);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::Rgb;

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const BG: Rgb = Rgb { r: 1, g: 2, b: 3 };

    /// A 2x2 frame: opaque on the diagonal, transparent off it — so the test
    /// sees both that colour expands and that transparency is not painted.
    fn diagonal() -> Frame {
        Frame::from_pixels(2, 2, vec![Some(RED), None, None, Some(RED)])
    }

    /// The load-bearing property: the classic path must be untouched by the
    /// existence of the scaled one.
    #[test]
    fn scale_one_is_byte_identical_to_the_unscaled_blit() {
        let f = diagonal();
        let mut plain = RgbBuffer::filled(6, 6, BG);
        let mut scaled = RgbBuffer::filled(6, 6, BG);
        blit_frame(&f, 1, 1, &mut plain);
        blit_frame_scaled(&f, 1, 1, NonZeroU16::MIN, &mut scaled);
        assert_eq!(plain.as_slice(), scaled.as_slice());
    }

    #[test]
    fn each_source_pixel_becomes_a_scale_by_scale_block() {
        let f = diagonal();
        let mut buf = RgbBuffer::filled(8, 8, BG);
        blit_frame_scaled(
            &f,
            0,
            0,
            NonZeroU16::new(3).expect("3 is nonzero"),
            &mut buf,
        );

        for y in 0..3u16 {
            for x in 0..3u16 {
                assert_eq!(buf.get(x, y), RED, "top-left block at ({x},{y})");
                assert_eq!(buf.get(x + 3, y + 3), RED, "bottom-right block");
                // The transparent off-diagonal cells must be left alone.
                assert_eq!(buf.get(x + 3, y), BG, "transparent top-right stayed bg");
                assert_eq!(buf.get(x, y + 3), BG, "transparent bottom-left stayed bg");
            }
        }
    }

    #[test]
    fn a_block_running_past_the_edge_is_clipped_not_wrapped() {
        let f = Frame::from_pixels(1, 1, vec![Some(RED)]);
        let mut buf = RgbBuffer::filled(4, 4, BG);
        // Anchored so only the block's top-left corner is inside the buffer.
        blit_frame_scaled(
            &f,
            3,
            3,
            NonZeroU16::new(4).expect("4 is nonzero"),
            &mut buf,
        );
        assert_eq!(buf.get(3, 3), RED);
        // Nothing wrapped around to the origin.
        assert_eq!(buf.get(0, 0), BG);
    }
}
