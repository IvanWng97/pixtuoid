//! `pixtuoid-web` — the WebAssembly canvas painter over the `pixtuoid-scene`
//! engine. The THIRD painter (alongside the binary's `tui` + `floating`): it
//! runs the real render+sim engine in the browser and blits
//! `render_to_rgb_buffer` into a `<canvas>` — the live office hero, NOT a gif.
//!
//! It ports `pixtuoid::floating::offscreen::OfficeRenderer` (already
//! winit/softbuffer-free — "scene → RgbBuffer"), minus the window: an [`Office`]
//! handle owns everything cross-frame so motion/pose stay continuous, and
//! `step(now_ms, w, h)` renders one frame into an RGBA staging buffer JS reads
//! zero-copy via [`Office::frame_ptr`]/[`Office::frame_len`] → `ImageData`.
//!
//! Time is a PARAMETER (`now_ms` from JS): the engine never calls
//! `SystemTime::now()` (it panics on wasm32-unknown-unknown).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use wasm_bindgen::prelude::*;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use pixtuoid_scene::chitchat::{ActiveChitchat, VenueKey};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::{FloorCtx, FloorMeta};
use pixtuoid_scene::layout::{Layout, TEST_DEFAULT_DESKS};
use pixtuoid_scene::pathfind::Router;
use pixtuoid_scene::pixel_painter::{render_to_rgb_buffer, PixelCtx};
use pixtuoid_scene::theme::{Theme, ALL_THEMES};

/// A live office rendered to a reusable RGBA buffer across frames. Owns the
/// per-floor render caches (`FloorCtx`) + the persistent office state
/// (coffee/chitchat) so keeping ONE handle alive across `step` calls is what
/// keeps motion/pose continuous (no walk-flash) — same contract as
/// `OfficeRenderer`.
#[wasm_bindgen]
pub struct Office {
    scene: SceneState,
    floor: FloorCtx,
    buf: RgbBuffer,
    /// RGBA staging (the render buffer is packed RGB, no alpha) — its ptr/len
    /// back a JS `Uint8ClampedArray` view into wasm memory, so blitting is
    /// zero-copy on the JS side.
    rgba: Vec<u8>,
    pack: Pack,
    theme: &'static Theme,
    chitchat: HashMap<VenueKey, ActiveChitchat>,
    coffee_holders: HashSet<AgentId>,
    coffee_fetched_at: HashMap<AgentId, SystemTime>,
    seed: u64,
}

#[wasm_bindgen]
impl Office {
    /// Build an office seeded with `seed` (drives the layout variant). Errors
    /// only if the compile-time-embedded sprite pack fails to parse (a build
    /// bug), surfaced to JS as an exception.
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32) -> Result<Office, JsError> {
        let pack = load_sprite_pack(None).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Office {
            // Slot capacity for the SCRIPTED agents (one classic office worth
            // is plenty for the hero's cast) — the LAYOUT below fills the
            // canvas independently of this.
            scene: SceneState::uniform(TEST_DEFAULT_DESKS),
            floor: FloorCtx::new(),
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            rgba: Vec::new(),
            pack,
            theme: ALL_THEMES[0],
            chitchat: HashMap::new(),
            coffee_holders: HashSet::new(),
            coffee_fetched_at: HashMap::new(),
            seed: seed as u64,
        })
    }

    /// Advance to `now_ms` (JS `performance.now()`/`Date.now()`) and render at
    /// `w`×`h` pixels into the RGBA staging buffer.
    pub fn step(&mut self, now_ms: f64, w: u32, h: u32) {
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(now_ms.max(0.0) as u64);
        let buf_w = w.clamp(1, u16::MAX as u32) as u16;
        let buf_h = h.clamp(1, u16::MAX as u32) as u16;
        self.render(now, buf_w, buf_h);
        self.expand_rgba();
    }

    /// Pointer to the RGBA frame in wasm linear memory (`w*h*4` bytes).
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// Byte length of the RGBA frame (`w*h*4`).
    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }
}

impl Office {
    fn render(&mut self, now: SystemTime, buf_w: u16, buf_h: u16) {
        self.buf
            .ensure_size(buf_w, buf_h, self.theme.surface.bg_fallback);
        // `None` = fill: the office packs as many desk pods as the canvas
        // physically fits (the Phase-1 desk-cap refactor — the point of it
        // for a web-scale hero background).
        let Some(layout) = Layout::compute_with_seed(buf_w, buf_h, None, self.seed) else {
            return; // too small to lay out — leave the cleared buffer, never panic
        };
        self.floor.router.set_preferred_zone(layout.corridor);
        let floor_meta = FloorMeta::for_floor(0, 1);
        let result = render_to_rgb_buffer(&mut PixelCtx {
            scene: &self.scene,
            layout: &layout,
            pack: &self.pack,
            now,
            buf: &mut self.buf,
            cache: &mut self.floor.cache,
            router: &mut self.floor.router,
            overlay: &mut self.floor.overlay,
            history: &mut self.floor.history,
            motion: &mut self.floor.motion,
            door_anim_max_ms: self.floor.door_anim_max_ms,
            theme: self.theme,
            floor: floor_meta,
            active_pet: None,
            floor_pet: None,
            chitchat_state: &mut self.chitchat,
            coffee_holders: &self.coffee_holders,
            coffee_fetched_at: &self.coffee_fetched_at,
            light: &mut self.floor.light,
            debug_walkable: false,
        });
        for id in result.new_coffee_carriers {
            self.coffee_holders.insert(id);
            self.coffee_fetched_at.entry(id).or_insert(now);
        }
        self.floor.recompute_door_anim_max_ms(now);
    }

    /// Expand the packed-RGB render buffer into the RGBA staging vec (opaque
    /// alpha). `Rgb` is not `repr(C)`, so expand per-pixel — don't cast.
    fn expand_rgba(&mut self) {
        let px = self.buf.as_slice();
        self.rgba.clear();
        self.rgba.reserve(px.len() * 4);
        for c in px {
            self.rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
        }
    }
}
