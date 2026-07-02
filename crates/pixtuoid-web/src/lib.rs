//! `pixtuoid-web` — the WebAssembly canvas painter over the `pixtuoid-scene`
//! engine. The THIRD painter (alongside the binary's `tui` + `floating`): it
//! runs the real render+sim engine in the browser and blits each frame of the
//! shared `pixtuoid_scene::floor::render_floor` seam (#423) into a `<canvas>`
//! — the live office hero, NOT a gif.
//!
//! A sibling thin caller of the same seam as the binary's painters (no window,
//! no terminal): an [`Office`]
//! handle owns everything cross-frame so motion/pose stay continuous, and
//! `step(now_ms, w, h)` renders one frame into an RGBA staging buffer JS reads
//! zero-copy via [`Office::frame_ptr`]/[`Office::frame_len`] → `ImageData`.
//!
//! Time is a PARAMETER (`now_ms` from JS): the engine never calls
//! `SystemTime::now()` (it panics on wasm32-unknown-unknown).

mod script;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use wasm_bindgen::prelude::*;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;

use crate::script::{hero_script, Beat, LOOP_MS};

use pixtuoid_scene::chitchat::{ActiveChitchat, VenueKey};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::{render_floor, CoffeeState, FloorCtx, FloorMeta};
use pixtuoid_scene::layout::TEST_DEFAULT_DESKS;
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
    coffee: CoffeeState,
    seed: u64,
    /// The REAL reducer + the looped hero script driving it — the office is
    /// populated by the same state machine the app uses, not a hand-rolled fake.
    reducer: Reducer,
    beats: Vec<Beat>,
    /// Next un-fired beat in the current loop.
    cursor: usize,
    /// t0 of the current loop; set on the first `step` call.
    epoch: Option<SystemTime>,
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
            coffee: CoffeeState::new(),
            seed: seed as u64,
            reducer: Reducer::new(),
            beats: hero_script(),
            cursor: 0,
            epoch: None,
        })
    }

    /// Advance to `now_ms` and render at `w`×`h` pixels into the RGBA staging
    /// buffer.
    ///
    /// CONTRACT: `now_ms` must be UNIX-epoch milliseconds — `Date.now()`, NOT
    /// `performance.now()` and NOT a `requestAnimationFrame` timestamp (both
    /// are ms-since-page-load: motion still animates, but the office's
    /// day/night cycle and wall clock decode `now` as calendar time, so a
    /// page-relative clock pins the scene at 1970 — permanently 00:00,
    /// defeating the browser-timezone support entirely).
    pub fn step(&mut self, now_ms: f64, w: u32, h: u32) {
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(now_ms.max(0.0) as u64);
        self.advance_script(now);
        // The per-frame sweep: Active→Idle debounce, exit GC, walkouts.
        self.reducer.tick(&mut self.scene, now);
        // Evict per-agent render state for agents the sweep removed (#423: the
        // shared scene seam — see `FloorCtx::evict_missing`'s doc for why this
        // is load-bearing here: the looped script REUSES agent ids, and a
        // returning cast member with stale walk legs teleports in).
        self.floor.evict_missing(&self.scene);
        self.coffee.evict_missing(&self.scene);
        let buf_w = w.clamp(1, u16::MAX as u32) as u16;
        let buf_h = h.clamp(1, u16::MAX as u32) as u16;
        self.render(now, buf_w, buf_h);
        self.expand_rgba();
    }

    /// Pointer to the RGBA frame in wasm linear memory (`w*h*4` bytes).
    ///
    /// CONTRACT: re-read this (and rebuild any `Uint8ClampedArray` view) after
    /// EVERY `step` — a canvas resize reallocates the staging buffer (the
    /// pointer moves), and any wasm `memory.grow` invalidates existing JS
    /// views into linear memory even when the pointer value is unchanged.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// Byte length of the RGBA frame (`w*h*4`).
    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }
}

impl Office {
    /// Fire every scripted beat due by `now`, each applied at its SCHEDULED
    /// time (not `now`) so the reducer's time-based semantics — the 1.5s
    /// Active debounce, exit grace — hold even when a hidden tab's rAF pauses
    /// and a resumed step has to catch up a large gap. Wraps the loop epoch;
    /// a gap past one full loop is re-anchored instead of replayed N times.
    fn advance_script(&mut self, now: SystemTime) {
        // Track the loop epoch in a local and write it back at each mutation —
        // no Option re-read (and no unreachable-expect) inside the loop.
        let mut epoch = *self.epoch.get_or_insert(now);
        let mut elapsed = now
            .duration_since(epoch)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;

        // A long-hidden tab: skip whole missed loops, keep the phase — the
        // replayed SessionStarts of the kept phase re-seat the cast.
        if elapsed >= 2 * LOOP_MS {
            let skip = (elapsed / LOOP_MS - 1) * LOOP_MS;
            epoch += Duration::from_millis(skip);
            self.epoch = Some(epoch);
            elapsed -= skip;
        }

        loop {
            while let Some(beat) = self.beats.get(self.cursor) {
                if beat.at_ms > elapsed {
                    break;
                }
                let at = epoch + Duration::from_millis(beat.at_ms);
                self.reducer
                    .apply(&mut self.scene, beat.event.clone(), at, beat.transport);
                self.cursor += 1;
            }
            if elapsed < LOOP_MS {
                break;
            }
            // Loop wrap: restart the script one LOOP_MS later.
            epoch += Duration::from_millis(LOOP_MS);
            self.epoch = Some(epoch);
            self.cursor = 0;
            elapsed -= LOOP_MS;
        }
    }

    fn render(&mut self, now: SystemTime, buf_w: u16, buf_h: u16) {
        // The shared scene seam (#423) owns the whole frame: buffer sizing,
        // layout (`None` desk cap = fill — the office packs as many desk pods
        // as the canvas physically fits), the pixel pass, and the
        // coffee/door-anim epilogue. Too-small layouts leave the cleared
        // buffer; never panics. The layout seed is the hero's variant seed
        // (NOT floor-derived), so build the meta then override the seed.
        let floor_meta = FloorMeta {
            floor_seed: self.seed,
            ..FloorMeta::for_floor(0, 1)
        };
        render_floor(
            &mut self.floor,
            &mut self.buf,
            &mut self.coffee,
            &mut self.chitchat,
            &self.scene,
            &self.pack,
            self.theme,
            now,
            buf_w,
            buf_h,
            floor_meta,
            None,
            None,
            false,
        );
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

// The rlib half of the crate-type exists exactly for these: the full
// `Office` pipeline (script drive + reducer + render + staging) runs
// natively — the same headless-render precedent as `floating::offscreen`.
// Only the JS boundary (the wasm-bindgen glue) is wasm-only.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::cast_id;

    /// `Office::new`'s error arm constructs a `JsError` (inert off-wasm), so
    /// tests unwrap via match — the embedded pack parsing is a build-time
    /// invariant and the Ok path never touches a JS value.
    fn office() -> Office {
        match Office::new(1) {
            Ok(o) => o,
            Err(_) => panic!("embedded pack must parse"),
        }
    }

    /// Anchor sim time well past 0 so `exiting_at`-style guards never see the
    /// UNIX_EPOCH sentinel; the value itself is arbitrary.
    const T0_MS: f64 = 1_000_000_000.0;

    #[test]
    fn step_renders_a_frame_of_the_advertised_shape() {
        let mut o = office();
        let (w, h) = (160u32, 96u32);
        o.step(T0_MS, w, h);
        assert_eq!(o.frame_len(), (w * h * 4) as usize, "len is w*h*4");
        let frame = &o.rgba;
        assert!(
            frame.iter().skip(3).step_by(4).all(|&a| a == 255),
            "alpha channel is fully opaque"
        );
        assert!(
            frame.chunks(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0),
            "the office actually painted (not an all-black frame)"
        );
    }

    #[test]
    fn resize_reshapes_the_frame_and_a_tiny_canvas_never_panics() {
        let mut o = office();
        o.step(T0_MS, 320, 180);
        assert_eq!(o.frame_len(), 320 * 180 * 4);
        // Grow: the staging Vec reallocates — the documented frame_ptr
        // re-read contract's trigger.
        o.step(T0_MS + 100.0, 480, 270);
        assert_eq!(o.frame_len(), 480 * 270 * 4);
        // Too small for any layout: render early-returns, still a valid frame.
        o.step(T0_MS + 200.0, 8, 8);
        assert_eq!(o.frame_len(), 8 * 8 * 4);
    }

    #[test]
    fn beats_fire_once_at_scheduled_times_across_a_wrap() {
        // Drive PAST one loop in coarse steps: the cast must exist (beats
        // fired), and the office must stay bounded (no double-fired
        // SessionStarts duplicating agents across the wrap).
        let mut o = office();
        let step_ms = 5_000u64;
        let total = LOOP_MS + LOOP_MS / 2;
        let mut t = 0u64;
        while t <= total {
            o.step(T0_MS + t as f64, 160, 96);
            t += step_ms;
        }
        assert!(
            (5..=8).contains(&o.scene.agents.len()),
            "cast bounded across the wrap, got {}",
            o.scene.agents.len()
        );
        // Cursor is mid-loop (the wrap reset it from the end of loop 1).
        assert!(o.cursor > 0 && o.cursor < o.beats.len());
    }

    #[test]
    fn a_long_hidden_tab_reanchors_instead_of_replaying_every_missed_loop() {
        let mut o = office();
        o.step(T0_MS, 160, 96);
        let epoch_before = o.epoch.unwrap();
        // 10 simulated minutes of a hidden tab (5 whole loops).
        let gap = 10 * 60 * 1_000u64;
        o.step(T0_MS + gap as f64, 160, 96);
        let epoch_after = o.epoch.unwrap();
        let advanced = epoch_after
            .duration_since(epoch_before)
            .unwrap()
            .as_millis() as u64;
        // The epoch jumped by WHOLE loops (phase kept), leaving < 2 loops of
        // catch-up — not a 5-loop replay.
        assert_eq!(advanced % LOOP_MS, 0, "re-anchor keeps the loop phase");
        assert!(gap - advanced < 2 * LOOP_MS, "at most one wrap replays");
        assert!(
            (5..=8).contains(&o.scene.agents.len()),
            "office coherent after the gap, got {}",
            o.scene.agents.len()
        );
    }

    #[test]
    fn exited_agents_render_state_is_evicted_so_loop_two_walks_dont_teleport() {
        // The door-traffic ids (cast 5 walks out at 104s, cast 7 at wrap-2s)
        // RECUR next loop. Stale MotionState entry/exit legs gate on
        // `is_none()`, so a leftover entry from the previous life would skip
        // the new walk-in — the teleport this eviction exists to prevent.
        let mut o = office();
        let mut t = 0u64;
        // Past agent 5's SessionEnd (104s) + the 4.5s exit grace + sweep.
        while t <= 115_000 {
            o.step(T0_MS + t as f64, 160, 96);
            t += 1_000;
        }
        assert!(
            !o.scene.agents.contains_key(&cast_id(5)),
            "agent 5 exited and was GC'd"
        );
        assert!(
            !o.floor.motion.contains_key(&cast_id(5)),
            "agent 5's motion state was evicted with its slot"
        );
        assert!(
            !o.coffee.map().contains_key(&cast_id(5)),
            "agent 5's coffee state was evicted with its slot"
        );
    }
}
