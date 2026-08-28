//! The enriched orthographic cutaway profile — a SIBLING of the classic
//! half-block painter, not a fidelity knob on it. Both read
//! `pixel_painter::SimFrame`; nothing here is wired to a painter yet, and
//! `examples/cutaway_snapshot` is the only caller in the tree.
//!
//! `render_cutaway` is `#[doc(hidden)] pub` because that example lives in the
//! `pixtuoid` crate and has to reach it — MECHANISM, not a promise to a
//! crates.io consumer. `shade` has no cross-crate caller and stays
//! `pub(crate)`: a `pub` item on a published crate is the one thing a follow-up
//! cannot quietly undo.
pub(crate) mod order;
#[doc(hidden)]
pub mod paint;
pub(crate) mod shade;
