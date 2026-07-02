//! Standalone furniture paint helpers — meeting table, area rug,
//! side table, pantry bistro table, pantry chair, and the procedural
//! room-fill decor (notice board, doormat, water cooler, trash bin).
//!
//! Extracted from `mod.rs` to keep the orchestrator focused on
//! the render pipeline rather than individual furniture geometry.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::layout::Bounds;

/// Low meeting-room table between the sofas. Wood top with darker
/// trim along the front edge so it reads as a real piece of furniture,
/// not just a brown rectangle.
pub(super) fn paint_meeting_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let min_x = cx.saturating_sub(w / 2);
    let max_x = (cx + w / 2 + (w & 1)).min(buf.width());
    let min_y = cy.saturating_sub(h / 2);
    let max_y = (cy + h / 2 + (h & 1)).min(buf.height());
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_front = y + 1 == max_y;
            buf.put(x, y, if on_front { trim } else { top });
        }
    }
}

/// Meeting-room area rug — warm Persian-tone rectangle painted under
/// the meeting table. Border ring in a darker shade so the rug reads as
/// having a fringe/binding rather than a flat blob. Centred on `cx,cy`.
pub(super) fn paint_area_rug(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::theme::Theme,
) {
    let rug_field = theme.furniture.rug_field;
    let rug_trim = theme.furniture.rug_trim;
    let rug_accent = theme.furniture.rug_accent;
    let half_w = w as i32 / 2;
    let half_h = h as i32 / 2;
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let px = cx as i32 - half_w + dx;
            let py = cy as i32 - half_h + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            let on_border = dx == 0 || dx == w as i32 - 1 || dy == 0 || dy == h as i32 - 1;
            let on_inner_border = dx == 1 || dx == w as i32 - 2 || dy == 1 || dy == h as i32 - 2;
            let color = if on_border {
                rug_trim
            } else if on_inner_border {
                rug_accent
            } else {
                rug_field
            };
            buf.put(px as u16, py as u16, color);
        }
    }
}

/// Lounge side table — 7×4 wood block next to the viewing couch
/// (opposite side from the floor lamp). Bumped from 5×3 to clear the
/// skill's ~5-cell-wide subzone threshold. Carries a 3-cell magazine
/// stack on top so the silhouette reads as "side table with a book".
pub(super) fn paint_side_table(buf: &mut RgbBuffer, cx: u16, cy: u16, theme: &crate::theme::Theme) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let mag = theme.furniture.magazine;
    let mag_trim = theme.furniture.magazine_trim;
    // Sprite dimensions from the one furniture table (== the mask footprint for
    // the side table) so the painted block can't drift from the blocked ground.
    let (w, h) = crate::layout::furniture_def(crate::layout::Furniture::LoungeSideTable)
        .footprint
        .map_or((7, 4), |s| (s.w as i32, s.h as i32));
    for dy in 0..h {
        for dx in 0..w {
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            let on_bottom = dy == h - 1;
            buf.put(px as u16, py as u16, if on_bottom { trim } else { top });
        }
    }
    let mag_pixels: &[((i32, i32), Rgb)] = &[
        ((-1, -1), mag),
        ((0, -1), mag),
        ((1, -1), mag),
        ((-1, 0), mag_trim),
        ((0, 0), mag_trim),
        ((1, 0), mag_trim),
    ];
    for ((dx, dy), c) in mag_pixels {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width() && (py as u16) < buf.height() {
            buf.put(px as u16, py as u16, *c);
        }
    }
}

/// Pantry bistro table — round-ish wood top (rounded corners by skipping
/// the 4 corner pixels) painted with the same warm wood palette as the
/// meeting table so they read as the same furniture family.
pub(super) fn paint_pantry_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let (w, h) = crate::layout::furniture_def(crate::layout::Furniture::PantryTable)
        .footprint
        .map_or((7, 4), |s| (s.w as i32, s.h as i32));
    for dy in 0..h {
        for dx in 0..w {
            let on_corner = (dx == 0 || dx == w - 1) && (dy == 0 || dy == h - 1);
            if on_corner {
                continue;
            }
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            let on_edge = dy == h - 1;
            buf.put(px as u16, py as u16, if on_edge { trim } else { top });
        }
    }
}

pub(super) fn paint_pantry_chair(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::theme::Theme,
) {
    let seat = theme.furniture.chair_seat;
    let trim = theme.furniture.chair_trim;
    let put = |buf: &mut RgbBuffer, dx: i32, dy: i32, c: Rgb| {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width() && (py as u16) < buf.height() {
            buf.put(px as u16, py as u16, c);
        }
    };
    put(buf, -1, -1, seat);
    put(buf, 0, -1, seat);
    put(buf, -1, 0, trim);
    put(buf, 0, 0, trim);
}

/// Notice board on the meeting room's south wall (8×5 framed rectangle).
/// Painted in the background-fill pass; no-op for rooms too small to host it.
pub(super) fn paint_notice_board(buf: &mut RgbBuffer, mr: Bounds, theme: &crate::theme::Theme) {
    if !(mr.height > 20 && mr.width > 15) {
        return;
    }
    let wall_color = theme.office.room_wall_trim_dark;
    let accent = theme.furniture.rug_accent;
    let bx = mr.x + 4;
    let by = mr.y + mr.height - 8;
    for dy in 0..5u16 {
        for dx in 0..8u16 {
            let px = bx + dx;
            let py = by + dy;
            if px < buf.width() && py < buf.height() {
                let on_edge = dx == 0 || dx == 7 || dy == 0 || dy == 4;
                buf.put(px, py, if on_edge { wall_color } else { accent });
            }
        }
    }
}

/// Small doormat at the meeting-room entrance (4×5 bordered rug, cubicle side).
pub(super) fn paint_doormat(buf: &mut RgbBuffer, mr: Bounds, theme: &crate::theme::Theme) {
    if mr.width <= 10 {
        return;
    }
    let mat_x = mr.x + mr.width;
    let mat_y = mr.y + mr.height / 2 - 2;
    let mat_color = theme.furniture.rug_trim;
    let mat_accent = theme.furniture.rug_field;
    for dy in 0..5u16 {
        for dx in 0..4u16 {
            let px = mat_x + dx + 1;
            let py = mat_y + dy;
            if px < buf.width() && py < buf.height() {
                let on_border = dx == 0 || dx == 3 || dy == 0 || dy == 4;
                buf.put(px, py, if on_border { mat_color } else { mat_accent });
            }
        }
    }
}

/// Water cooler near the pantry wall (3×6: blue bottle over a light body).
pub(super) fn paint_water_cooler(buf: &mut RgbBuffer, pr: Bounds, theme: &crate::theme::Theme) {
    if !(pr.height > 25 && pr.width > 12) {
        return;
    }
    let cooler_body = theme.office.building_light;
    let cooler_water = Rgb {
        r: 100,
        g: 180,
        b: 230,
    };
    let wx = pr.x + pr.width - 6;
    let wy = pr.y + 8;
    for dy in 0..6u16 {
        for dx in 0..3u16 {
            let px = wx + dx;
            let py = wy + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy < 2 { cooler_water } else { cooler_body };
                buf.put(px, py, color);
            }
        }
    }
}

/// Trash bin near the pantry counter (4×5 with a visible bag-liner peek). Its
/// colours are intentionally un-themed neutral greys (a semantic object, like
/// the water bottle's blue), so it takes no theme.
pub(super) fn paint_trash_bin(buf: &mut RgbBuffer, pr: Bounds) {
    if pr.height <= 20 {
        return;
    }
    let tx = pr.x + 3;
    let ty = pr.y + pr.height - 14;
    let bin_outer = Rgb {
        r: 70,
        g: 70,
        b: 78,
    };
    let bin_rim = Rgb {
        r: 100,
        g: 100,
        b: 108,
    };
    let bag_liner = Rgb {
        r: 200,
        g: 200,
        b: 210,
    };
    let bag_fill = Rgb {
        r: 160,
        g: 160,
        b: 170,
    };
    for dy in 0..5u16 {
        for dx in 0..4u16 {
            let px = tx + dx;
            let py = ty + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy == 0 {
                    // Rim row — lighter metal rim with bag liner peek
                    if dx == 0 || dx == 3 {
                        bin_rim
                    } else {
                        bag_liner
                    }
                } else if dy == 1 {
                    // Bag liner visible
                    if dx == 0 || dx == 3 {
                        bin_outer
                    } else {
                        bag_fill
                    }
                } else {
                    // Bin body
                    bin_outer
                };
                buf.put(px, py, color);
            }
        }
    }
}
