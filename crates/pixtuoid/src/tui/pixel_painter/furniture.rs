//! Standalone furniture paint helpers — coffee table, area rug,
//! side table, pantry bistro table, pantry chair.
//!
//! Extracted from `mod.rs` to keep the orchestrator focused on
//! the render pipeline rather than individual furniture geometry.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

/// Low coffee table in front of the lounge couch. Wood top with darker
/// trim along the front edge so it reads as a real piece of furniture,
/// not just a brown rectangle.
pub(super) fn paint_coffee_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let min_x = cx.saturating_sub(w / 2);
    let max_x = (cx + w / 2 + (w & 1)).min(buf.width);
    let min_y = cy.saturating_sub(h / 2);
    let max_y = (cy + h / 2 + (h & 1)).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_front = y + 1 == max_y;
            buf.put(x, y, if on_front { trim } else { top });
        }
    }
}

/// Meeting-room area rug — warm Persian-tone rectangle painted under
/// the coffee table. Border ring in a darker shade so the rug reads as
/// having a fringe/binding rather than a flat blob. Centred on `cx,cy`.
pub(super) fn paint_area_rug(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::tui::theme::Theme,
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
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
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
pub(super) fn paint_side_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let mag = theme.furniture.magazine;
    let mag_trim = theme.furniture.magazine_trim;
    // Sprite dimensions from the one furniture table (== the mask footprint for
    // the side table) so the painted block can't drift from the blocked ground.
    let (w, h) = crate::tui::layout::furniture_def(crate::tui::layout::Furniture::LoungeSideTable)
        .footprint
        .map_or((7, 4), |s| (s.w as i32, s.h as i32));
    for dy in 0..h {
        for dx in 0..w {
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
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
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, *c);
        }
    }
}

/// Pantry bistro table — round-ish wood top (rounded corners by skipping
/// the 4 corner pixels) painted with the same warm wood palette as the
/// coffee table so they read as the same furniture family.
pub(super) fn paint_pantry_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let (w, h) = crate::tui::layout::furniture_def(crate::tui::layout::Furniture::PantryTable)
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
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
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
    theme: &crate::tui::theme::Theme,
) {
    let seat = theme.furniture.chair_seat;
    let trim = theme.furniture.chair_trim;
    let put = |buf: &mut RgbBuffer, dx: i32, dy: i32, c: Rgb| {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, c);
        }
    };
    put(buf, -1, -1, seat);
    put(buf, 0, -1, seat);
    put(buf, -1, 0, trim);
    put(buf, 0, 0, trim);
}
