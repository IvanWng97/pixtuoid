//! The wall's RENDER half — room-divider partitions drawn as frosted glass.
//! The painter-side counterpart to the GEOMETRY half in `layout::rooms::walls`,
//! whose `wall_pieces` every painter draws from.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::drawable::{Drawable, DrawableKind};
use super::palette::blend_pixel;
use crate::layout::{wall_pieces, Layout, WallPiece};

// The E-W wall shows its face while the N-S wall is seen edge-on; the 3:2
// thickness ratio sells the top-down fake-3D. Both DERIVE from the core mask
// consts so the visible glass face and the blocked ground footprint can't
// drift apart.
pub(super) const WALL_THICK_V_PX: u16 = crate::layout::WALL_THICK_V;
pub(super) const WALL_THICK_H_PX: u16 = crate::layout::WALL_THICK_H;
const GLASS_SEAM_STRIDE: u16 = 16;
/// Mullion (partition post) spacing: a 1px darker post every this-many px so a
/// long run reads as panelled partitions instead of one unbroken sheet. Offset
/// from the seam-glint stride so the two rhythms interleave.
const MULLION_STRIDE: u16 = 10;
// A visual-only "back cap" rising north of the walkable footprint, so a walker
// standing behind the wall has their legs composited behind the glass. Derived
// from the face thickness so retuning the wall moves the cap with it.
const GLASS_CAP_PX: u16 = WALL_THICK_H_PX;

fn glass_tones(theme: &crate::theme::Theme) -> (Rgb, Rgb, Rgb) {
    let tl = theme.office.room_wall_trim_light;
    (
        Rgb {
            r: tl.r.saturating_add(125),
            g: tl.g.saturating_add(135),
            b: tl.b.saturating_add(124),
        },
        Rgb {
            r: tl.r.saturating_add(70),
            g: tl.g.saturating_add(100),
            b: tl.b.saturating_add(116),
        },
        Rgb {
            r: tl.r.saturating_add(18),
            g: tl.g.saturating_add(52),
            b: tl.b.saturating_add(86),
        },
    )
}

/// The H twin of [`paint_glass_wall_v`]. They stay SEPARATE — don't hoist a
/// `paint_glass_strip(axis, ..)` helper: only the five-arm tone ladder is
/// shared, and H prepends a [`GLASS_CAP_PX`] cap V has no equivalent for, the
/// two take run/anchor arguments in opposite orders, and three of the ladder's
/// five alphas differ. Revisit only if a THIRD orientation appears.
pub(super) fn paint_glass_wall_h(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x0: u16,
    x1: u16,
    y_top: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width(), buf.height());
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    let rows = GLASS_CAP_PX + WALL_THICK_H_PX;
    for x in x0..=x1.min(bw.saturating_sub(1)) {
        let seam = (x - x0).is_multiple_of(GLASS_SEAM_STRIDE);
        // Interior posts only: a post AT a run end would double the door
        // frames / corner joints.
        let mullion = x > x0 && x < x1 && (x - x0).is_multiple_of(MULLION_STRIDE);
        for i in 0..rows {
            let y = cap_top + i;
            if y >= bh {
                continue;
            }
            let (g, a) = if mullion {
                (lo, 0.8)
            } else if seam {
                (hi, 0.55)
            } else if i == 0 {
                (hi, 0.82)
            } else if i == rows - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.58)
            };
            blend_pixel(buf, x, y, g, a);
        }
    }
}

pub(super) fn paint_glass_wall_v(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
    y_bot: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width(), buf.height());
    for y in y_top..=y_bot.min(bh.saturating_sub(1)) {
        let seam = (y - y_top).is_multiple_of(GLASS_SEAM_STRIDE);
        let mullion = y > y_top && y < y_bot && (y - y_top).is_multiple_of(MULLION_STRIDE);
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x >= bw {
                continue;
            }
            let (g, a) = if mullion {
                (lo, 0.8)
            } else if seam {
                (hi, 0.6)
            } else if dx == 0 {
                (hi, 0.85)
            } else if dx == WALL_THICK_V_PX - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.6)
            };
            blend_pixel(buf, x, y, g, a);
        }
    }
}

/// Jamb depth in px along the wall's axis — 2 reads as a solid post at
/// half-block scale without eating into the opening.
pub(super) const DOOR_JAMB_PX: u16 = 2;

pub(super) fn paint_door_jamb_h(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
) {
    let dark = theme.office.room_wall_trim_dark;
    let (bw, bh) = (buf.width(), buf.height());
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    for x in x_left..(x_left + DOOR_JAMB_PX).min(bw) {
        for i in 0..(GLASS_CAP_PX + WALL_THICK_H_PX) {
            let y = cap_top + i;
            if y < bh {
                buf.put(x, y, dark);
            }
        }
    }
}

/// `y_top` is the jamb's FIRST row, so a south jamb is passed
/// `y_bot - (DOOR_JAMB_PX - 1)`.
pub(super) fn paint_door_jamb_v(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
) {
    let dark = theme.office.room_wall_trim_dark;
    let (bw, bh) = (buf.width(), buf.height());
    for y in y_top..(y_top + DOOR_JAMB_PX).min(bh) {
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x < bw {
                buf.put(x, y, dark);
            }
        }
    }
}

/// Horizontal (E-W) room dividers join the y-sort on [`WallPiece::sort_row`],
/// so a character standing north of the wall is composited over by the frosted
/// glass rather than painting on top of it. Emitted LAST so a character tied
/// with a wall row still paints behind it.
pub(super) fn enqueue_room_walls_h<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for piece in wall_pieces(&layout.room_walls, &layout.doorways, layout.top_margin) {
        if let WallPiece::Horizontal {
            x0,
            x1,
            y_face,
            jamb_west,
            jamb_east,
        } = piece
        {
            drawables.push(Drawable {
                anchor_y: piece.sort_row(),
                kind: DrawableKind::RoomWallH {
                    x0,
                    x1,
                    y_top: y_face,
                    jamb_left: jamb_west,
                    jamb_right: jamb_east,
                },
            });
        }
    }
}

/// Vertical (N-S, edge-on) room dividers join the y-sort on
/// [`WallPiece::sort_row`], each painted over its stitched `[y_top, y_bot]`.
pub(super) fn enqueue_room_walls_v<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for piece in wall_pieces(&layout.room_walls, &layout.doorways, layout.top_margin) {
        if let WallPiece::Vertical {
            x,
            y_top,
            y_bot,
            jamb_north,
            jamb_south,
            ..
        } = piece
        {
            drawables.push(Drawable {
                anchor_y: piece.sort_row(),
                kind: DrawableKind::RoomWallV {
                    x,
                    y_top,
                    y_bot,
                    jamb_north,
                    jamb_south,
                },
            });
        }
    }
}
