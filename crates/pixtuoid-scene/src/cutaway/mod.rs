//! The enriched orthographic cutaway profile.
//!
//! The design brief models two renderers over one shared scene frame, not one
//! renderer with a fidelity knob: the classic half-block painter stays exactly
//! as it is, and this is its sibling. Both consume `pixel_painter::SimFrame` —
//! the engine already produces that owned, immutable observation, so the seam
//! the brief asks for exists and this module simply becomes its second reader.
//!
//! Nothing here is wired to a painter yet. The vocabulary lands first because
//! it is what the visual mock ratified, and because it is pure and testable in
//! a way the paint pass built on top of it will not be.

pub mod shade;
