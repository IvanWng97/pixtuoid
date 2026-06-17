use crate::sprite::{Frame, Rgb, RgbBuffer};

/// Blit a sprite frame with a 1-pixel outline painted around its silhouette.
/// For each **transparent in-frame** pixel whose 4-neighborhood (up/down/
/// left/right) contains at least one opaque pixel, the destination buffer
/// gets an `outline` pixel painted at that position *before* the actual
/// sprite pixels are blitted on top.
///
/// The scan covers in-frame cells ONLY: a silhouette needs a transparent
/// margin inside the frame to receive its halo on that side. An opaque pixel
/// touching the frame edge gets no halo beyond the boundary, and a fully
/// opaque frame gets none at all (pinned by
/// `outlined_blit_does_not_outline_interior_opaque_pixels`) — author sprites
/// with a 1-px transparent border where the halo matters.
///
/// Result: a clean 1-pixel halo around the sprite silhouette, useful for
/// making solid-color shapes (desks, plants) pop against the floor pattern.
pub fn blit_frame_outlined(
    frame: &Frame,
    dst_x: u16,
    dst_y: u16,
    dst: &mut RgbBuffer,
    outline: Rgb,
) {
    let w = frame.width as i32;
    let h = frame.height as i32;
    let is_opaque = |fx: i32, fy: i32| -> bool {
        if fx < 0 || fy < 0 || fx >= w || fy >= h {
            return false;
        }
        let i = (fy as usize) * (frame.width as usize) + (fx as usize);
        frame.as_slice()[i].is_some()
    };

    for fy in 0..h {
        for fx in 0..w {
            if is_opaque(fx, fy) {
                continue;
            }
            // Transparent (or boundary) cell — does any opaque neighbor exist?
            let has_opaque_neighbor = is_opaque(fx - 1, fy)
                || is_opaque(fx + 1, fy)
                || is_opaque(fx, fy - 1)
                || is_opaque(fx, fy + 1);
            if !has_opaque_neighbor {
                continue;
            }
            let x = dst_x.saturating_add(fx as u16);
            let y = dst_y.saturating_add(fy as u16);
            if x < dst.width && y < dst.height {
                dst.put(x, y, outline);
            }
        }
    }
    blit_frame(frame, dst_x, dst_y, dst);
}

/// Bresenham line drawing into an RgbBuffer. Coordinates are signed so callers
/// can pass off-buffer endpoints (clipping is implicit — pixels outside the
/// buffer are silently skipped).
pub fn draw_line(buf: &mut RgbBuffer, x0: i32, y0: i32, x1: i32, y1: i32, rgb: Rgb) {
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as u16) < buf.width && (y as u16) < buf.height {
            buf.put(x as u16, y as u16, rgb);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Convenience: dotted horizontal line. `dash` painted px, then `gap` skipped.
/// A zero-length dash paints nothing. All arithmetic runs in u32 so a
/// `dash + gap == 0` step can't spin forever (the step floors at 1) and
/// coordinates near `u16::MAX` can't overflow — this is pub API, the caller
/// controls every parameter.
pub fn draw_dotted_hline(
    buf: &mut RgbBuffer,
    x0: u16,
    y: u16,
    x1: u16,
    rgb: Rgb,
    dash: u16,
    gap: u16,
) {
    let step = (dash as u32 + gap as u32).max(1);
    let x1 = x1 as u32;
    let mut x = x0 as u32;
    while x <= x1 {
        for i in 0..dash as u32 {
            let px = x + i;
            if px > x1 {
                break;
            }
            if px < buf.width as u32 && y < buf.height {
                buf.put(px as u16, y, rgb);
            }
        }
        x += step;
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HalfCell {
    pub fg: Rgb,
    pub bg: Rgb,
}

/// Convert an RGB buffer into a 2D grid of half-block cells.
/// Each row pair becomes one cell row: `fg` = upper pixel, `bg` = lower pixel.
/// Odd-height buffers pad the last cell by duplicating the final row into `bg`.
pub fn half_block_cells(buf: &RgbBuffer) -> Vec<Vec<HalfCell>> {
    let w = buf.width as usize;
    let h = buf.height as usize;
    if h == 0 || w == 0 {
        return Vec::new();
    }
    let cell_rows = h.div_ceil(2);
    let mut out: Vec<Vec<HalfCell>> = Vec::with_capacity(cell_rows);
    for cy in 0..cell_rows {
        let py_top = cy * 2;
        let py_bot = (py_top + 1).min(h - 1);
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let fg = buf.as_slice()[py_top * w + x];
            let bg = buf.as_slice()[py_bot * w + x];
            row.push(HalfCell { fg, bg });
        }
        out.push(row);
    }
    out
}
