use std::time::SystemTime;

use anyhow::Result;

use crate::sprite::format::Pack;
use crate::state::SceneState;

/// LEGACY pre-scene-split seam, `#[doc(hidden)]` = off the semver surface.
/// It never became the render abstraction it was designed as: the two
/// painters built after the `pixtuoid-scene` split (floating window, web
/// canvas) bypassed it entirely — the real seam for a new render target is
/// `pixtuoid_scene::floor::render_floor` / `pixel_painter::render_to_rgb_buffer`
/// (see workspace invariant #1). It survives only because its two impls
/// still ride it non-polymorphically: `TuiRenderer` (the binary's terminal
/// flush entry point) and `TestRenderer` (the e2e harness). Neither needs
/// the trait per se, but retiring it means moving `TuiRenderer::render` to
/// an inherent impl in the binary — do that there if you touch this again;
/// don't build new render targets on it.
#[doc(hidden)]
pub trait Renderer {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()>;
}

#[cfg(feature = "test-renderer")]
pub mod test_renderer;
