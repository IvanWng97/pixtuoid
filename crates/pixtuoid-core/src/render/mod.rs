//! Home of the `test-renderer` fixture.
//!
//! There is no core render trait: `TuiRenderer` and `TestRenderer` each ride
//! their own inherent method (`render` / `record`). New render targets go
//! through `pixtuoid_scene::floor::render_floor` /
//! `pixel_painter::render_to_rgb_buffer` (workspace invariant #1), never a core
//! render trait.

#[cfg(feature = "test-renderer")]
pub mod test_renderer;
