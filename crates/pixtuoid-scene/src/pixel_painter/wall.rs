//! The classic painter's room walls: each of the layout's
//! [`wall_pieces`](crate::layout::wall_pieces) queued into the y-sort, and
//! painted through the frosted [`Glass`] the cutaway draws too.

use pixtuoid_core::sprite::RgbBuffer;

use super::drawable::{Drawable, DrawableKind};
use crate::glass::Glass;
use crate::layout::{wall_pieces, Layout, WallPiece};

/// Paint one room wall: its glass over what is already drawn behind it, then
/// the jamb posts where a doorway cuts it.
pub(super) fn paint_partition(buf: &mut RgbBuffer, theme: &crate::theme::Theme, piece: WallPiece) {
    let (at, size) = piece.visual();
    let glass = Glass::of(theme, piece, 1);
    for dy in 0..size.h {
        for dx in 0..size.w {
            let (x, y) = (at.x + dx, at.y + dy);
            if x < buf.width() && y < buf.height() {
                buf.put(x, y, glass.over(buf.get(x, y), dx, dy));
            }
        }
    }
    let post = theme.office.room_wall_trim_dark;
    for (p, s) in piece.jambs() {
        for y in p.y..(p.y + s.h).min(buf.height()) {
            for x in p.x..(p.x + s.w).min(buf.width()) {
                buf.put(x, y, post);
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
        if let WallPiece::Horizontal { .. } = piece {
            drawables.push(Drawable {
                anchor_y: piece.sort_row(),
                kind: DrawableKind::RoomWall(piece),
            });
        }
    }
}

/// Vertical (N-S, edge-on) room dividers join the y-sort on
/// [`WallPiece::sort_row`], each painted over its stitched `[y_top, y_bot]`.
pub(super) fn enqueue_room_walls_v<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for piece in wall_pieces(&layout.room_walls, &layout.doorways, layout.top_margin) {
        if let WallPiece::Vertical { .. } = piece {
            drawables.push(Drawable {
                anchor_y: piece.sort_row(),
                kind: DrawableKind::RoomWall(piece),
            });
        }
    }
}
