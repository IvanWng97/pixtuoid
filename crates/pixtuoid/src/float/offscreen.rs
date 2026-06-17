//! Headless office → `RgbBuffer` rendering for the `pixtuoid float` desktop window.
//!
//! This renders the office to a raw pixel `RgbBuffer` via `render_to_rgb_buffer` — NOT the
//! half-block terminal emulation `examples/snapshot.rs` saves (snapshot writes the ratatui
//! `TestBackend` → a ▀-compressed PNG via `save_backend_as_png`). A float-only seam: no
//! `draw_scene`, no `Terminal`, no shared output with snapshot. `float::window` renders at
//! a DOWNSCALED buffer (~window/SCALE) and nearest-neighbor upscales it, so the pixel-art
//! office stays chunky/legible instead of 8×12-px-tiny at 1:1. This module just paints the
//! buffer at whatever dims it's handed. It mirrors `tui_renderer::render_transition_floor`
//! (the established headless pixel pattern), owning the per-frame caches plus the persistent
//! office state (coffee cups, group chitchat) across frames so motion stays continuous.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use crate::tui::chitchat::{ActiveChitchat, VenueKey};
use crate::tui::floor::{FloorCtx, FloorMeta};
use crate::tui::layout::{Layout, MAX_VISIBLE_DESKS};
use crate::tui::pathfind::Router;
use crate::tui::pixel_painter::{render_to_rgb_buffer, PixelCtx};
use crate::tui::theme::Theme;

/// Owns everything needed to render the live office to a reusable `RgbBuffer` across
/// frames: the per-floor render caches (`FloorCtx`) plus the persistent office state
/// the pixel pass reads and updates (`coffee_holders`/`coffee_fetched_at` drive desk
/// cups + steam; `chitchat` drives group speech bubbles). One per window — keeping it
/// alive across frames is what keeps motion/pose continuous (no walk-flash).
pub struct OfficeRenderer {
    floor: FloorCtx,
    buf: RgbBuffer,
    chitchat: HashMap<VenueKey, ActiveChitchat>,
    coffee_holders: HashSet<AgentId>,
    coffee_fetched_at: HashMap<AgentId, SystemTime>,
}

impl OfficeRenderer {
    pub fn new() -> Self {
        Self {
            floor: FloorCtx::new(),
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            chitchat: HashMap::new(),
            coffee_holders: HashSet::new(),
            coffee_fetched_at: HashMap::new(),
        }
    }

    /// Render `scene`'s floor (per `floor_meta`) into the owned buffer at `buf_w`×`buf_h`
    /// PIXELS — the caller maps window px → cells → pixels (`buf_w = cols`,
    /// `buf_h = rows * 2`, the half-block 1:2 cell aspect; float has no footer row to
    /// subtract, unlike `draw_scene`). Returns the rendered buffer (a borrow of the
    /// reused allocation). On a too-small / uncomputable layout it returns the buffer
    /// unchanged — never panics.
    #[allow(clippy::too_many_arguments)] // the render inputs are genuinely flat (scene/pack/theme/clock/size/floor)
    pub fn render(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        theme: &'static Theme,
        now: SystemTime,
        buf_w: u16,
        buf_h: u16,
        floor_meta: FloorMeta,
        floor_pet: Option<&crate::tui::pet::Pet>,
    ) -> &RgbBuffer {
        self.buf
            .ensure_size(buf_w, buf_h, theme.surface.bg_fallback);
        let Some(layout) =
            Layout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, floor_meta.floor_seed)
        else {
            return &self.buf;
        };
        self.floor.router.set_preferred_zone(layout.corridor);
        let result = render_to_rgb_buffer(&mut PixelCtx {
            scene,
            layout: &layout,
            pack,
            now,
            buf: &mut self.buf,
            cache: &mut self.floor.cache,
            router: &mut self.floor.router,
            overlay: &mut self.floor.overlay,
            history: &mut self.floor.history,
            motion: &mut self.floor.motion,
            door_anim_max_ms: self.floor.door_anim_max_ms,
            theme,
            floor: floor_meta,
            // active_pet is the click-to-pet heart animation — needs window pointer
            // hit-testing (deferred); the WANDERING floor pet is wired.
            active_pet: None,
            floor_pet,
            chitchat_state: &mut self.chitchat,
            coffee_holders: &self.coffee_holders,
            coffee_fetched_at: &self.coffee_fetched_at,
            light: &mut self.floor.light,
            debug_walkable: false,
        });
        // Persist desk cups: a pantry trip completed this frame stamps the carrier so the
        // cup lands on the desk + steams (mirrors TuiRenderer's coffee bookkeeping, which
        // the transition path threads via `new_coffee_carriers`).
        for id in result.new_coffee_carriers {
            self.coffee_holders.insert(id);
            self.coffee_fetched_at.entry(id).or_insert(now);
        }
        // render_to_rgb_buffer may have snapshotted new entry/exit profiles into motion;
        // refresh the door-cosmetic clamp for next frame (same as the transition path).
        self.floor.recompute_door_anim_max_ms(now);
        &self.buf
    }
}

impl Default for OfficeRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_sized_nonblank_office_buffer() {
        // A fresh empty office still paints floor/walls/windows → never all-black, and the
        // buffer is sized to the requested pixel dims. Pins the float render seam end-to-end.
        let scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let pack = crate::tui::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = crate::tui::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();
        let buf = renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        assert_eq!((buf.width, buf.height), (160, 96));
        // Assert PAINTED content, not the pre-fill: `ensure_size` fills the buffer with
        // `bg_fallback` (non-black) BEFORE the painter runs, so "any non-black pixel" would
        // pass even if the painter no-op'd. Require a pixel that is neither black NOR
        // `bg_fallback` → the floor/walls/windows pass actually ran.
        let bg = theme.surface.bg_fallback;
        assert!(
            buf.pixels
                .iter()
                .any(|p| *p != Rgb { r: 0, g: 0, b: 0 } && *p != bg),
            "the painter draws office content beyond the cleared background"
        );
    }
}
