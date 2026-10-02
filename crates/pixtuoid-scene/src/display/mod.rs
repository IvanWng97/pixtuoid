//! The display list: one frame of the office as pieces in paint order,
//! composed model-side from an [`Office`] and what it is [`Showing`], for a
//! rasterizer to draw.

pub(crate) mod compose;
mod list;
mod order;

pub(crate) use compose::{
    DOOR_SPRITE, PLATE_PAD, board_runs, compose, desk_span, drawn_in, face_rows, indicator_plate,
};
pub use compose::{Office, Showing};
pub(crate) use list::{
    Art, Badge, DisplayList, Figure, Flip, Ground, Layer, LightPiece, Piece, PieceKind, Screen,
    StoodProp, WindowView, fingerprint,
};
#[cfg(test)]
pub(crate) use order::check_order;
pub(crate) use order::{Span, depth_sort};
