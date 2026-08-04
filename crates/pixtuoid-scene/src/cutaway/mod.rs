//! The enriched orthographic cutaway profile.
//!
//! The design brief models two renderers over one shared scene frame, not one
//! renderer with a fidelity knob: the classic half-block painter stays exactly
//! as it is, and this is its sibling. Both consume `pixel_painter::SimFrame` —
//! the engine already produces that owned, immutable observation, so the seam
//! the brief asks for exists and this module simply becomes its second reader.
//!
//! Nothing here is wired to a painter yet — `examples/cutaway_snapshot` is the
//! only caller in the tree.
//!
//! Public for MECHANISM, not contract: that example lives in the `pixtuoid`
//! crate, so `render_cutaway` has to be reachable across the crate boundary,
//! but nothing here is a promise to a crates.io consumer. Hence the
//! `#[doc(hidden)]` — the same escape hatch `overlay`/`board`/`footer` use —
//! and `shade` stays `pub(crate)` outright, since its drawing primitives have
//! no cross-crate caller at all and a `pub` item on a published crate is the
//! one thing a follow-up cannot quietly undo.

#[doc(hidden)]
pub mod paint;
pub(crate) mod shade;
