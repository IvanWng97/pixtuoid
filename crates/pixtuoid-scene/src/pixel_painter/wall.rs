//! The room walls: each of the layout's
//! [`wall_pieces`](crate::layout::SceneLayout::wall_pieces), sorted in its
//! [bands](WallPiece::sort_bands) and painted through the [`Glass`] by
//! both painters, each on its own [`Pen`].

use std::ops::Range;

use pixtuoid_core::sprite::RgbBuffer;

use super::drawable::{Drawable, DrawableKind};
use crate::cutaway::pen::{ArtRect, Pen};
use crate::glass::Glass;
use crate::layout::{Layout, WallPiece};
use crate::theme::Theme;

/// Paint the logical `rows` of one room wall: its glass over what is already
/// drawn behind them, then the jamb posts where a doorway cuts it. The glass is
/// laid from the wall's own top, so its rhythm runs on across the bands.
pub(crate) fn paint_wall(
    buf: &mut RgbBuffer,
    theme: &Theme,
    piece: WallPiece,
    rows: Range<u16>,
    pen: Pen,
) {
    let (at, size) = piece.visual();
    let mut glass = Glass::of(theme, piece, pen.art(1).0);
    let band = ArtRect {
        x: pen.art(at.x),
        y: pen.art(rows.start),
        w: pen.art(size.w),
        h: pen.art(rows.end - rows.start),
    };
    let top = pen.art(rows.start - at.y).0;
    pen.recolour(buf, band, |dx, dy, under| glass.over(under, dx, top + dy));
    for (post, s) in piece.jambs() {
        let (from, to) = (post.y.max(rows.start), (post.y + s.h).min(rows.end));
        if from < to {
            let r = ArtRect {
                x: pen.art(post.x),
                y: pen.art(from),
                w: pen.art(s.w),
                h: pen.art(to - from),
            };
            pen.fill(buf, r, theme.office.room_wall_trim_dark);
        }
    }
}

/// Queue every room wall's bands into the y-sort, emitted LAST so a character
/// tied with a band's row still paints behind the glass.
pub(super) fn enqueue_room_walls<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &piece in &layout.wall_pieces {
        for (rows, depth) in piece.sort_bands() {
            drawables.push(Drawable {
                anchor_y: depth,
                kind: DrawableKind::RoomWall { piece, rows },
            });
        }
    }
}
