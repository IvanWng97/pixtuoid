use std::time::SystemTime;

use pixtuoid_core::layout::WALKING_Y_OFF;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::epoch_ms;
use super::palette::blend_rgb;
use crate::tui::layout::Point;
use crate::tui::theme::Theme;

pub(super) fn paint_screen_glow(
    buf: &mut RgbBuffer,
    desk_x: u16,
    desk_y: u16,
    now: SystemTime,
    tint: Rgb,
    theme: &Theme,
) {
    let frame_lit = theme.effects.monitor_frame_lit;
    let glow = tint;
    let white = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    let glow_bright = blend_rgb(tint, white, 0.4);
    let scanline = blend_rgb(tint, white, 0.7);
    let put = |buf: &mut RgbBuffer, dx: u16, dy: u16, c: Rgb| {
        let px = desk_x + dx;
        let py = desk_y + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, c);
        }
    };
    for dx in 3..=10 {
        put(buf, dx, 0, frame_lit);
    }
    for dx in 4..=9 {
        put(buf, dx, 1, glow_bright);
        put(buf, dx, 2, glow);
    }
    for dx in 4..=9 {
        put(buf, dx, 3, frame_lit);
    }
    let elapsed_ms = epoch_ms(now);
    let phase = (elapsed_ms / 120) as u16 + desk_x;
    let scan_col = 4 + (phase % 6);
    put(buf, scan_col, 1, scanline);
    put(buf, scan_col, 2, scanline);
}

pub(super) fn paint_sleep_z(
    buf: &mut RgbBuffer,
    head_anchor: Point,
    now: SystemTime,
    seed: u64,
    theme: &Theme,
) {
    let z_color = theme.effects.sleep_z;
    const CYCLE_MS: u64 = 2400;
    let elapsed_ms = epoch_ms(now);
    let phase_ms = elapsed_ms.wrapping_add(seed % CYCLE_MS) % CYCLE_MS;
    if phase_ms >= CYCLE_MS - 400 {
        return;
    }
    let rise = (phase_ms / 180) as u16;
    let z_x = head_anchor.x + 5;
    let z_y = head_anchor.y.saturating_sub(rise + 3);
    let pixels: &[(u16, u16)] = &[(0, 0), (1, 0), (1, 1), (0, 2), (1, 2)];
    for (dx, dy) in pixels {
        let px = z_x + dx;
        let py = z_y + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, z_color);
        }
    }
}

pub(super) fn paint_coffee_steam(buf: &mut RgbBuffer, base: Point, now: SystemTime, theme: &Theme) {
    let steam = theme.effects.coffee_steam;
    let elapsed_ms = epoch_ms(now);
    for offset in 0..3u64 {
        let phase = (elapsed_ms + offset * 600) % 1800;
        let rise = (phase / 140) as u16;
        let alpha = 1.0 - phase as f32 / 1800.0;
        if alpha < 0.15 {
            continue;
        }
        let wiggle = if (phase / 200) % 2 == 0 { 0 } else { 1 };
        let px = base.x + wiggle;
        let py = base.y.saturating_sub(rise + 2);
        if px < buf.width && py < buf.height {
            let cur = buf.get(px, py);
            buf.put(px, py, blend_rgb(cur, steam, alpha * 0.55));
        }
    }
}

pub(super) fn paint_walking_dust(
    buf: &mut RgbBuffer,
    walker_anchor: Point,
    frame_idx: usize,
    theme: &Theme,
) {
    let dust = theme.effects.walking_dust;
    let foot_y = walker_anchor.y + WALKING_Y_OFF;
    let foot_x = walker_anchor.x + if frame_idx == 0 { 6 } else { 1 };
    if foot_x < buf.width && foot_y < buf.height {
        let cur = buf.get(foot_x, foot_y);
        buf.put(foot_x, foot_y, blend_rgb(cur, dust, 0.45));
    }
}

pub(super) fn paint_thinking_dots(
    buf: &mut RgbBuffer,
    anchor: Point,
    now: SystemTime,
    theme: &Theme,
) {
    let fg = theme.ui.label_active;
    let elapsed_ms = epoch_ms(now);
    let phase = (elapsed_ms / 800) % 4;
    let bx = anchor.x + 2;
    let by = anchor.y.saturating_sub(3);
    for i in 0..phase {
        let px = bx + (i as u16) * 2;
        if px < buf.width && by < buf.height {
            buf.put(px, by, fg);
        }
    }
}

/// Floating heart particles for the "pet the cat" interaction.
/// 4 hearts, staggered 150ms apart, each rising 6px over 1550ms and
/// fading via alpha blend toward the background. Last heart starts at
/// 450ms so all 4 complete within PET_DURATION_MS (2000ms).
pub(super) fn paint_pet_hearts(buf: &mut RgbBuffer, cat_pos: Point, elapsed_ms: u64) {
    const STAGGER_MS: u64 = 150;
    const HEART_LIFE_MS: u64 = 1550;
    let heart_color = Rgb {
        r: 255,
        g: 100,
        b: 100,
    };
    for i in 0..4u64 {
        let stagger = i * STAGGER_MS;
        if elapsed_ms < stagger {
            continue;
        }
        let local_ms = elapsed_ms - stagger;
        if local_ms >= HEART_LIFE_MS {
            continue;
        }
        let t = local_ms as f32 / HEART_LIFE_MS as f32;
        let rise = (t * 6.0) as u16;
        let alpha = 1.0 - t;
        if alpha < 0.05 {
            continue;
        }
        // Spread hearts horizontally: offsets -3, -1, +1, +3
        let dx: i16 = (i as i16) * 2 - 3;
        let hx = (cat_pos.x as i32 + dx as i32).max(0) as u16;
        let hy = cat_pos.y.saturating_sub(4 + rise);
        // 2x2 pixel heart
        for dy in 0..2u16 {
            for ddx in 0..2u16 {
                let px = hx + ddx;
                let py = hy + dy;
                if px < buf.width && py < buf.height {
                    let cur = buf.get(px, py);
                    buf.put(px, py, blend_rgb(cur, heart_color, alpha * 0.8));
                }
            }
        }
    }
}

pub(super) fn paint_waiting_bubble(buf: &mut RgbBuffer, anchor: Point, theme: &Theme) {
    let fg = theme.effects.waiting_bubble;
    const GLYPH: &[&[u8]] = &[b".YYY.", b"...Y.", b"..Y..", b"..Y.."];
    let bx = anchor.x + 1;
    let by = anchor.y.saturating_sub(5) & !1u16;
    for (dy, row) in GLYPH.iter().enumerate() {
        for (dx, byte) in row.iter().enumerate() {
            if *byte != b'Y' {
                continue;
            }
            let px = bx + dx as u16;
            let py = by + dy as u16;
            if px < buf.width && py < buf.height {
                buf.put(px, py, fg);
            }
        }
    }
}
