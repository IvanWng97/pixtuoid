//! The wall's RENDER half — room-divider partitions drawn as frosted glass.
//! The painter-side counterpart to the GEOMETRY half in `layout::rooms::walls`;
//! the two stay bound by the shared `WALL_THICK_*` consts +
//! `stitch_vertical_wall` (single source, no drift).

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::drawable::{Drawable, DrawableKind};
use super::palette::blend_pixel;
use crate::layout::{crossing_h_rows, stitch_vertical_wall, Layout, WallSegment};

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
/// `paint_glass_strip(axis, alphas)` helper. They share only the tone-ladder
/// SKELETON and diverge at every load-bearing point, so the unifier's interface
/// would be as wide as the body it hides. Revisit only if a THIRD wall
/// orientation appears.
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

/// Horizontal (E-W) room dividers join the y-sort, anchored at their south
/// (front) edge so a character standing north of the wall is composited over by
/// the frosted glass rather than painting on top of it. Emitted LAST so a
/// character tied with a wall row still paints behind it.
pub(super) fn enqueue_room_walls_h<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.y == end.y {
            let (x0, x1) = (start.x.min(end.x), start.x.max(end.x));
            // A cut end abutting a doorway gets a jamb — flagged HERE because
            // the paint pass has no layout access.
            let jamb_right = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.start.x == x1);
            let jamb_left = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.end.x == x0);
            drawables.push(Drawable {
                anchor_y: start.y + (WALL_THICK_H_PX - 1),
                kind: DrawableKind::RoomWallH {
                    x0,
                    x1,
                    y_top: start.y,
                    jamb_left,
                    jamb_right,
                },
            });
        }
    }
}

/// Vertical (N-S, edge-on) room dividers join the y-sort. Each segment carries
/// its own stitched `[y_top, y_bot]` for PAINT — the layout emits raw geometry;
/// the render offsets that plug the joints live in `stitch_vertical_wall`.
pub(super) fn enqueue_room_walls_v<'a>(
    layout: &'a Layout,
    top_wall_h: u16,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.x != end.x {
            continue; // horizontal walls handled by enqueue_room_walls_h
        }
        // The SAME x-filtered crossing rows the mask footprint uses, so the
        // painted glass and the blocked ground bridge off the same H walls.
        let h_rows = crossing_h_rows(start.x, &layout.room_walls);
        let (y_top, y_bot) =
            stitch_vertical_wall(start.y, end.y, layout.top_margin, top_wall_h, &h_rows);
        // Jamb flags on the RAW cut ends — a door cut is never a stitch joint,
        // so the stitched y_top/y_bot the paint arm uses equal these.
        let jamb_south = layout
            .doorways
            .iter()
            .any(|d| d.start.x == start.x && d.end.x == start.x && d.start.y == end.y);
        let jamb_north = layout
            .doorways
            .iter()
            .any(|d| d.start.x == start.x && d.end.x == start.x && d.end.y == start.y);
        drawables.push(Drawable {
            // z-key = the RAW south end, NOT the stitched `y_bot`: at a corner
            // the stitch extends `y_bot` down into the crossing H wall to fill
            // the L-notch, and anchoring there would paint the vertical glass
            // OVER that H wall (and the pantry counter).
            anchor_y: end.y,
            kind: DrawableKind::RoomWallV {
                x: start.x,
                y_top,
                y_bot,
                jamb_north,
                jamb_south,
            },
        });
    }
}
