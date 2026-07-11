//! Vibe dressing — the zone rugs and the north-wall art prints +
//! window-sill succulents (2026-07-11 vibe package). Floor/wall FIXTURES
//! like the clock, neon panel, and corridor runner live in `lighting.rs`;
//! this file owns the decorative layer added on top.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::super::palette::blend_rgb;

/// Tint-blend a zone rug over the already-painted carpet (keeps the tile
/// texture — a flat fill reads as recolored floor, not a rug). 2px inset from
/// the room bounds; the 1px edge band blends harder so the rug reads bordered.
/// Colors come from `SurfaceColors::{rug_meeting, rug_pantry}` per theme.
pub(in crate::pixel_painter) fn paint_zone_rug(
    buf: &mut RgbBuffer,
    room: crate::layout::Bounds,
    rug: Rgb,
) {
    let (x0, y0) = (room.x + 2, room.y + 2);
    let (x1, y1) = (
        (room.x + room.width).saturating_sub(2),
        (room.y + room.height).saturating_sub(2),
    );
    for y in y0..y1.min(buf.height()) {
        for x in x0..x1.min(buf.width()) {
            let c = buf.get(x, y);
            let edge = x == x0 || x + 1 == x1 || y == y0 || y + 1 == y1;
            // WHY 0.45/0.65: 45% keeps carpet texture visible through the tint;
            // the 65% edge band draws the rug border (taste-pinned 2026-07-11).
            let t = if edge { 0.65 } else { 0.45 };
            buf.put(x, y, blend_rgb(c, rug, t));
        }
    }
}

/// Wall dressing over the window band: two framed art prints on the window
/// mullions + two window-sill succulents. Positions are buf-width fractions
/// (they must land BETWEEN fixed wall fixtures — stats board left, clock
/// centre, elevator right); the prints bridge a mullion the way a real frame
/// hangs on the structural column. Print hues are deliberately theme-fixed
/// (art is art — it doesn't repaint with the walls); the succulents are the
/// pack's own sprite so custom packs restyle them.
pub(in crate::pixel_painter) fn paint_wall_dressing(
    buf: &mut RgbBuffer,
    buf_w: u16,
    top_wall_h: u16,
    pack: &pixtuoid_core::sprite::format::Pack,
) {
    const FRAME: Rgb = Rgb {
        r: 0x3a,
        g: 0x2a,
        b: 0x1c,
    };
    // Band triplets top-to-bottom per print (warm + cool so one of the two
    // pops on any theme's wall).
    const PRINTS: [(u32, [Rgb; 3]); 2] = [
        (
            38,
            [
                Rgb {
                    r: 0x7e,
                    g: 0xd4,
                    b: 0xe2,
                },
                Rgb {
                    r: 0xa8,
                    g: 0x32,
                    b: 0x32,
                },
                Rgb {
                    r: 0xc8,
                    g: 0xa6,
                    b: 0x4c,
                },
            ],
        ),
        (
            83,
            [
                Rgb {
                    r: 0xc2,
                    g: 0x73,
                    b: 0x4f,
                },
                Rgb {
                    r: 0x4a,
                    g: 0x8f,
                    b: 0x3a,
                },
                Rgb {
                    r: 0x2e,
                    g: 0x62,
                    b: 0xcf,
                },
            ],
        ),
    ];
    for (pct, cols) in PRINTS {
        let px0 = (buf_w as u32 * pct / 100) as u16;
        let py0 = top_wall_h.saturating_sub(10);
        for dy in 0..8u16 {
            for dx in 0..6u16 {
                let (x, y) = (px0 + dx, py0 + dy);
                if x >= buf.width() || y >= buf.height() {
                    continue;
                }
                let edge = dx == 0 || dx == 5 || dy == 0 || dy == 7;
                let c = if edge {
                    FRAME
                } else {
                    cols[(((dy - 1) / 2).min(2)) as usize]
                };
                buf.put(x, y, c);
            }
        }
    }
    // Sill spots sit between the prints (38/83) and clear of the stats board /
    // clock / elevator, same buf-width-fraction scheme as PRINTS.
    const SILL_PCTS: [u32; 2] = [28, 62];
    if let Some(p) = pack
        .animation("plant_succulent")
        .and_then(|a| a.frames.first())
    {
        for pct in SILL_PCTS {
            let px = (buf_w as u32 * pct / 100) as u16;
            let py = top_wall_h.saturating_sub(p.height() + 1);
            if px + p.width() <= buf.width() {
                pixtuoid_core::sprite::blit::blit_frame(p, px, py, buf);
            }
        }
    }
}
