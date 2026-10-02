//! The display list: one frame of the office as pieces in paint order,
//! composed model-side from an [`Office`] and what it is [`Showing`], for a
//! rasterizer to draw, with the hovers and text set beside it.

pub(crate) mod compose;
#[expect(dead_code, reason = "the scene's Look entry and 3b's builders fill it")]
pub(crate) mod hover;
mod list;
mod order;
#[expect(dead_code, reason = "the scene's Look entry and 3b's builders fill it")]
pub(crate) mod text;

pub use compose::{Office, Showing};
pub(crate) use compose::{PLATE_PAD, board_runs, compose, desk_span, face_rows, indicator_plate};
pub(crate) use list::{
    Art, Badge, DisplayList, Figure, Flip, Ground, LightPiece, Piece, PieceKind, Screen, StoodProp,
    WindowView, fingerprint,
};
#[cfg(test)]
pub(crate) use order::check_order;
pub(crate) use order::{Layer, Span, depth_sort};
