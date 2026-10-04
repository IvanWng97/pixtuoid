//! The display list: one frame of the office as pieces in paint order,
//! composed model-side from an [`Office`] and what it is [`Showing`], for a
//! rasterizer to draw.

pub(crate) mod compose;
pub(crate) mod effects;
mod hover;
pub(crate) mod light;
mod list;
mod order;
pub(crate) mod pen;
pub mod text;

pub use crate::creatures::GatewayCard;
pub use compose::{Office, Showing};
pub(crate) use compose::{PLATE_PAD, compose, desk_span, face_rows, run_rect};
pub(crate) use hover::Hover;
pub use hover::{HoverTarget, Hovers, PetHover};
pub(crate) use list::{
    Art, DisplayList, Emits, Figure, Flip, LightPiece, Piece, PieceKind, Screen, StoodProp,
    WindowView, fingerprint,
};
#[cfg(test)]
pub(crate) use order::check_order;
pub(crate) use order::{Layer, Span, depth_sort};
pub use text::{Align, TextRole, TextRun, TextSpan};
