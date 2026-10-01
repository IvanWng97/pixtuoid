//! The enriched orthographic cutaway profile — a SIBLING of the classic
//! half-block painter, not a fidelity knob on it. Both read
//! `pixel_painter::SimFrame`; nothing here is wired to a painter yet.
//!
//! `render_cutaway` and `CutawayCanvas` are `#[doc(hidden)] pub` as seams for
//! out-of-crate drivers (the snapshot example, the render bench, the painters
//! to come) — MECHANISM, not a promise to a crates.io consumer. `shade` has no
//! cross-crate caller and stays `pub(crate)`: a `pub` item on a published crate
//! is the one thing a follow-up cannot quietly undo.
#[doc(hidden)]
pub mod canvas;
pub(crate) mod light;
pub(crate) mod order;
#[doc(hidden)]
pub mod paint;
pub(crate) mod pen;
pub(crate) mod shade;
