//! Home of the `test-renderer` fixture.
//!
//! The pre-scene-split `Renderer` trait that used to live here was retired in
//! #483: its only two impls (`TuiRenderer`, `TestRenderer`) rode it
//! non-polymorphically, so the trait was deleted and each impl kept its own
//! inherent method. `TuiRenderer` still has an inherent `render`; `TestRenderer`
//! now just captures scenes via `record` (its mirrored inherent `render` was
//! dropped — a dead duplicate driven only by its own self-test). New render
//! targets go through `pixtuoid_scene::floor::render_floor` /
//! `pixel_painter::render_to_rgb_buffer` (workspace invariant #1), never a core
//! render trait.

#[cfg(feature = "test-renderer")]
pub mod test_renderer;
