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
