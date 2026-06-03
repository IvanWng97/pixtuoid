//! Lighting effects — ceiling pools, lamp halos, shadows, corridor
//! runner texture, neon sign panel, and wall clock.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::pixel_painter::epoch_ms;
use crate::tui::pixel_painter::palette::blend_rgb;
use crate::tui::theme::Theme;

/// An axis-aligned ellipse for the radial floor pools (light + shadow).
#[derive(Clone, Copy)]
pub(in crate::tui::pixel_painter) struct Ellipse {
    pub cx: u16,
    pub cy: u16,
    pub half_w: u16,
    pub half_h: u16,
}

/// Blend `color` over an elliptical region with a quadratic falloff from the
/// center (full `strength`) to the edge (0), so it reads as a soft round patch
/// rather than a stamped oval. Shared by the ceiling light pool and the
/// furniture shadow — same math, different tint.
fn paint_ellipse_blend(buf: &mut RgbBuffer, e: Ellipse, strength: f32, color: Rgb) {
    if e.half_w == 0 || e.half_h == 0 || strength <= 0.0 {
        return;
    }
    let min_x = e.cx.saturating_sub(e.half_w);
    let max_x = (e.cx + e.half_w).min(buf.width);
    let min_y = e.cy.saturating_sub(e.half_h);
    let max_y = (e.cy + e.half_h).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let nx = (x as f32 - e.cx as f32) / e.half_w as f32;
            let ny = (y as f32 - e.cy as f32) / e.half_h as f32;
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let t = (1.0 - r2) * strength;
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, color, t));
        }
    }
}

/// Elliptical "ceiling fluorescent" pool of pale warm light on the floor.
pub(in crate::tui::pixel_painter) fn paint_ceiling_pool(
    buf: &mut RgbBuffer,
    ellipse: Ellipse,
    strength: f32,
    theme: &Theme,
) {
    paint_ellipse_blend(buf, ellipse, strength, theme.lighting.ceiling_pool);
}

/// Warm radial halo around the floor lamp — only visible at night.
pub(in crate::tui::pixel_painter) fn paint_floor_lamp_halo(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    strength: f32,
    theme: &Theme,
) {
    let warm = theme.lighting.floor_lamp_halo;
    const RADIUS: u16 = 11;
    if strength <= 0.0 {
        return;
    }
    let min_x = cx.saturating_sub(RADIUS);
    let max_x = (cx + RADIUS).min(buf.width);
    let min_y = cy.saturating_sub(RADIUS);
    let max_y = (cy + RADIUS).min(buf.height);
    let r2max = (RADIUS as f32) * (RADIUS as f32);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = x as f32 - cx as f32;
            let dy = y as f32 - cy as f32;
            let r2 = dx * dx + dy * dy;
            if r2 > r2max {
                continue;
            }
            let t = (1.0 - (r2 / r2max).sqrt()) * strength;
            let cur = buf.get(x, y);
            buf.put(x, y, blend_rgb(cur, warm, t));
        }
    }
}

/// Neon sign panel — dark background with colored glow border, painted in
/// the wall band. The ratatui text widget renders on top with bright colors.
pub(in crate::tui::pixel_painter) fn paint_neon_panel(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    now: SystemTime,
    theme: &Theme,
) {
    let elapsed_ms = epoch_ms(now);
    let pulse = 0.7 + 0.3 * ((elapsed_ms as f32 / 1200.0).sin() * 0.5 + 0.5);

    let panel_bg = theme.office.neon_panel_bg;
    let base = theme.office.neon_frame_base;

    let clamp = |v: f32| v.clamp(0.0, 255.0) as u8;
    let frame_color = Rgb {
        r: clamp(base.r as f32 + 25.0 * pulse),
        g: clamp(base.g as f32 + 50.0 * pulse),
        b: clamp(base.b as f32 + 50.0 * pulse),
    };

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
                continue;
            }
            let on_border = dx == 0 || dx == w - 1 || dy == 0 || dy == h - 1;
            if on_border {
                buf.put(px, py, frame_color);
            } else {
                buf.put(px, py, panel_bg);
            }
        }
    }
}

/// Live wall clock — reads system local time and renders hour + minute hands.
/// 7x7 clock face with a circular rim. Hands quantize to 8 cardinal/inter-
/// cardinal directions and are drawn as multi-pixel rays from the center
/// (hour 1 px, minute 2 px) so they read clearly at this size.
pub(in crate::tui::pixel_painter) fn paint_clock(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    now: SystemTime,
    theme: &Theme,
) {
    let rim = theme.office.clock_rim;
    let face = theme.office.clock_face;
    let hand_color = theme.office.clock_hand;
    let hand_min = hand_color;

    // 7x7 disc — `R` rim, `F` face, `.` transparent. Center at x+3, y+3.
    let rows: &[&[u8]] = &[
        b"..RRR..", b".RFFFR.", b"RFFFFFR", b"RFFFFFR", b"RFFFFFR", b".RFFFR.", b"..RRR..",
    ];
    for (dy, row) in rows.iter().enumerate() {
        for (dx, ch) in row.iter().enumerate() {
            let c = match ch {
                b'R' => rim,
                b'F' => face,
                _ => continue,
            };
            let px = x + dx as u16;
            let py = y + dy as u16;
            if px < buf.width && py < buf.height {
                buf.put(px, py, c);
            }
        }
    }

    // Decompose `now` into local hour + minute via chrono.
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    use chrono::Timelike;
    let hour = local.hour() % 12;
    let minute = local.minute();

    // Fractional positions around the clock (0.0 = 12 o'clock, 0.25 = 3 o'clock).
    let hour_turns = (hour as f32 + minute as f32 / 60.0) / 12.0;
    let min_turns = minute as f32 / 60.0;

    let put = |buf: &mut RgbBuffer, ox: i32, oy: i32, color: Rgb| {
        let px = x as i32 + 3 + ox;
        let py = y as i32 + 3 + oy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, color);
        }
    };

    // Center pin (always painted).
    put(buf, 0, 0, hand_color);

    // Hour hand: ray of length 1 from center.
    let (hdx, hdy) = octant_offset(hour_turns);
    put(buf, hdx, hdy, hand_color);

    // Minute hand: ray of length 2 from center at cardinals, 1 at
    // diagonals. The 7x7 disc has 3-px face at cardinals but only 1-px
    // face at diagonals — a length-2 diagonal hand would overwrite the
    // rim and leave a gap in the border.
    let (mdx, mdy) = octant_offset(min_turns);
    let max_step = if mdx != 0 && mdy != 0 { 1 } else { 2 };
    for step in 1..=max_step {
        put(buf, mdx * step, mdy * step, hand_min);
    }
}

/// Quantize a fractional turn (0.0..1.0, 0.0 = north) to one of 8 octant
/// (dx, dy) unit offsets.
fn octant_offset(turn: f32) -> (i32, i32) {
    let oct = ((turn * 8.0).round() as i32).rem_euclid(8);
    match oct {
        0 => (0, -1),
        1 => (1, -1),
        2 => (1, 0),
        3 => (1, 1),
        4 => (0, 1),
        5 => (-1, 1),
        6 => (-1, 0),
        7 => (-1, -1),
        _ => (0, 0),
    }
}

/// Office corridor runner — a darker wood strip with subtle lighter stripes,
/// painted along the walkway band so the eye traces a path connecting the
/// door, meeting room, pantry, cubicles, and lounge. Just texture over the
/// floor — walls and decor paint on top.
pub(in crate::tui::pixel_painter) fn paint_corridor_runner(
    buf: &mut RgbBuffer,
    rect: crate::tui::layout::Bounds,
    theme: &Theme,
) {
    let runner_base = theme.office.runner_base;
    let runner_stripe = theme.office.runner_stripe;
    let runner_edge = theme.office.runner_edge;
    let max_x = (rect.x + rect.width).min(buf.width);
    let max_y = (rect.y + rect.height).min(buf.height);
    for y in rect.y..max_y {
        for x in rect.x..max_x {
            let is_edge = y == rect.y || y + 1 == max_y;
            let is_inner_edge = y == rect.y + 1 || y + 2 == max_y;
            let dy = (y - rect.y) as i32;
            let dx = (x - rect.x) as i32;
            let diamond = ((dx + dy) % 6 == 0) || ((dx - dy).rem_euclid(6) == 0);
            let color = if is_edge {
                runner_edge
            } else if is_inner_edge || diamond {
                runner_stripe
            } else {
                runner_base
            };
            buf.put(x, y, color);
        }
    }
}

/// Elliptical drop-shadow blended toward black at the floor level.
/// Grounds floating sprites so they look like they're standing/sitting
/// on the floor instead of hovering in mid-air. `strength` 0..1 controls
/// the darken amount at the center; falls off quadratically to 0 at edge.
/// Soft elliptical contact shadow under furniture / characters.
pub(in crate::tui::pixel_painter) fn paint_shadow(
    buf: &mut RgbBuffer,
    ellipse: Ellipse,
    strength: f32,
    theme: &Theme,
) {
    paint_ellipse_blend(buf, ellipse, strength, theme.office.shadow);
}
