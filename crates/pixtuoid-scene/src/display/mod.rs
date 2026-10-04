//! The display list: one frame of the office as pieces in paint order,
//! composed model-side from an [`Office`] and what it is [`Showing`], for a
//! rasterizer to draw.

pub(crate) mod compose;
mod hover;
mod list;
mod order;
pub(crate) mod pen;
pub(crate) mod text;

pub use crate::creatures::GatewayCard;
pub use compose::{Office, Showing};
pub(crate) use compose::{PLATE_PAD, board_runs, compose, desk_span, face_rows, indicator_plate};
pub(crate) use hover::Hover;
pub use hover::{HoverTarget, Hovers, PetHover};
pub(crate) use list::{
    Art, Badge, DisplayList, Figure, Flip, Ground, LightPiece, Piece, PieceKind, Screen, StoodProp,
    WindowView, fingerprint,
};
#[cfg(test)]
pub(crate) use order::check_order;
pub(crate) use order::{Layer, Span, depth_sort};
