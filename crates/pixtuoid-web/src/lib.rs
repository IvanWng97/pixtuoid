//! `pixtuoid-web` — the WebAssembly canvas painter over the `pixtuoid-scene`
//! engine: an [`Office`] handle owns everything cross-frame so motion/pose stay
//! continuous, and `step(now_ms, w, h)` renders one frame into an RGBA staging
//! buffer JS reads zero-copy via [`Office::frame_ptr`]/[`Office::frame_len`].
//!
//! Time is a PARAMETER (`now_ms` from JS): the engine never calls
//! `SystemTime::now()` (it panics on wasm32-unknown-unknown).

mod audio;
mod script;

use std::time::{Duration, SystemTime};

use wasm_bindgen::prelude::*;

use pixtuoid_core::source::daemon::apply_presence;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::{AgentEvent, AgentId, Transport};

use crate::script::{
    hero_gateway, hero_script, hire_beats, lobster_beats, Beat, PresenceBeat, LOOP_MS,
};

use pixtuoid_scene::audio::OneShotPool;
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::{floor_capacity, FloorMeta, FloorSession, FrameInputs};
use pixtuoid_scene::layout::{Size, CHARACTER_SPRITE_W};
use pixtuoid_scene::theme::{Theme, ALL_THEMES};

/// A visitor hire's one-shot event, queued OUTSIDE the loop machinery so a
/// hire's lifecycle never replays on wrap.
struct ScheduledEvent {
    at: SystemTime,
    event: AgentEvent,
}

/// The visitor-hire lane — grouped so the cap invariant lives in ONE place
/// across the enqueue (`try_hire`) and drain (`drain_due`) sides.
#[derive(Default)]
struct VisitorHires {
    /// Kept sorted by time; drained from the front.
    pending: Vec<ScheduledEvent>,
    ids: Vec<AgentId>,
    seq: u32,
}

impl VisitorHires {
    /// Cap on concurrently-alive hires: enough that repeat clicks visibly stack,
    /// few enough that click-spam can't crowd out the cast.
    const MAX_LIVE: usize = 3;

    /// Queue one more hire's lifecycle. Refused (`false`, a no-op) when the cap
    /// is reached or the canvas-synced office has no free desk to seat one.
    fn try_hire(&mut self, base: SystemTime, scene: &SceneState) -> bool {
        // Prune only ids that are neither LIVE (in the scene) nor still QUEUED
        // (SessionStart pending): pruning a queued id loses it permanently, and a
        // click one frame after a burst would overshoot the cap.
        self.ids.retain(|id| {
            scene.agents.contains_key(id)
                || self.pending.iter().any(
                    |ev| matches!(&ev.event, AgentEvent::SessionStart { agent_id, .. } if agent_id == id),
                )
        });
        if self.ids.len() >= Self::MAX_LIVE {
            return false;
        }
        // A hire the office can't SEAT would hold a MAX_LIVE slot for its whole
        // stay with zero visual feedback. Live agents keep their desks through
        // exit grace and each queued SessionStart will claim one, so count both
        // against the canvas-synced capacity.
        let queued_starts = self
            .pending
            .iter()
            .filter(|ev| matches!(ev.event, AgentEvent::SessionStart { .. }))
            .count();
        if scene.agents.len() + queued_starts >= scene.total_capacity() {
            return false;
        }
        self.seq += 1;
        let session = format!("hire-{}", self.seq);
        let id = AgentId::from_parts(pixtuoid_core::source::claude_code::SOURCE_NAME, &session);
        self.ids.push(id);
        for (at_ms, event) in hire_beats(id, session) {
            self.pending.push(ScheduledEvent {
                at: base + Duration::from_millis(at_ms),
                event,
            });
        }
        self.pending.sort_by_key(|ev| ev.at);
        true
    }

    /// Fire every queued hire event due by `now`, each applied at its SCHEDULED
    /// time (not `now`) so the reducer's time-based semantics hold.
    fn drain_due(&mut self, now: SystemTime, reducer: &mut Reducer, scene: &mut SceneState) {
        while self.pending.first().is_some_and(|ev| ev.at <= now) {
            let ev = self.pending.remove(0);
            reducer.apply(scene, ev.event, ev.at, Transport::Hook);
        }
    }
}

/// A live office rendered to a reusable RGBA buffer across frames. Keeping ONE
/// handle alive across `step` calls is what keeps motion/pose continuous.
#[wasm_bindgen]
pub struct Office {
    scene: SceneState,
    session: FloorSession,
    /// RGBA staging (the render buffer is packed RGB) — its ptr/len back a JS
    /// view into wasm memory, so blitting is zero-copy on the JS side.
    rgba: Vec<u8>,
    pack: Pack,
    theme: &'static Theme,
    seed: u64,
    reducer: Reducer,
    beats: Vec<Beat>,
    cursor: usize,
    /// Scripted gateway presence deltas — own cursor, same loop clock as `beats`.
    presence_beats: Vec<PresenceBeat>,
    presence_cursor: usize,
    /// t0 of the current loop; set on the first `step` call.
    epoch: Option<SystemTime>,
    /// Visitor hires — outside the loop machinery, so one never replays on wrap.
    hires: VisitorHires,
    /// The clock of the most recent `step` — `hire()` has no clock parameter, so
    /// it schedules relative to this.
    last_now: Option<SystemTime>,
    /// The buffer size `floor_capacities` was last synced for — lets
    /// `sync_capacity` skip the layout recompute on every other frame.
    caps_size: Option<(u16, u16)>,
    weather_override: Option<String>,
    /// The WebAudio engine — `None` until the visitor clicks ♩ (browser autoplay
    /// policy: no sound without a gesture).
    audio: Option<audio::WebAudioDriver>,
    /// An in-flight worker-prewarm handoff — staged by `audio_adopt_begin`,
    /// promoted to `audio` by `audio_adopt_finish`.
    adopting: Option<audio::Adoption>,
}

#[wasm_bindgen]
impl Office {
    /// Build an office seeded with `seed` (drives the layout variant). Errors
    /// only if the compile-time-embedded sprite pack fails to parse.
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32) -> Result<Office, JsError> {
        let pack = load_sprite_pack(None).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Office {
            // Capacity starts empty and is synced from the CANVAS's own layout
            // on every `step` before any beat fires, so the reducer only admits
            // agents the rendered office can seat.
            scene: SceneState::default(),
            session: FloorSession::new(),
            rgba: Vec::new(),
            pack,
            theme: ALL_THEMES[0],
            seed: seed as u64,
            reducer: Reducer::new(),
            beats: hero_script(),
            cursor: 0,
            presence_beats: lobster_beats(),
            presence_cursor: 0,
            epoch: None,
            hires: VisitorHires::default(),
            last_now: None,
            caps_size: None,
            weather_override: None,
            audio: None,
            adopting: None,
        })
    }

    /// Advance to `now_ms` and render at `w`×`h` pixels into the RGBA staging
    /// buffer.
    ///
    /// CONTRACT: `now_ms` must be UNIX-epoch milliseconds — `Date.now()`, NOT
    /// `performance.now()` and NOT a `requestAnimationFrame` timestamp: those are
    /// ms-since-page-load, which pins the day/night cycle and wall clock at 1970.
    pub fn step(&mut self, now_ms: f64, w: u32, h: u32) {
        // `f64 as u64` saturates (negatives/NaN → 0), so no pre-clamp is needed.
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(now_ms as u64);
        self.last_now = Some(now);
        let buf_w = w.clamp(1, u16::MAX as u32) as u16;
        let buf_h = h.clamp(1, u16::MAX as u32) as u16;
        // `force_weather` is a thread-local shared by every Office in the module, so
        // each office must set its own value right before rendering. An unknown name
        // leaves that thread-local UNTOUCHED, which would silently render whatever
        // the last writer forced — hence the Err path clearing it.
        if pixtuoid_scene::pixel_painter::force_weather(self.weather_override.as_deref()).is_err() {
            let _ = pixtuoid_scene::pixel_painter::force_weather(None);
        }
        // Capacity BEFORE the script advances: the SessionStarts due this
        // frame must allocate desks against the canvas this frame renders.
        self.sync_capacity(buf_w, buf_h);
        self.advance_script(now);
        self.reducer.tick(&mut self.scene, now);
        // The DAEMON sweep the app's reducer task runs: without it a `Down` row
        // lives until the next loop's `GatewayUp` and the wall board shows a stale
        // `gw down` chip. Registry-driven so an Nth daemon row can't decay in
        // production yet linger here.
        for (source, ttl) in pixtuoid_core::source::registry::daemon_sources() {
            pixtuoid_core::source::daemon::sweep_presence_ttl(&mut self.scene, source, ttl, now);
        }
        // `render` evicts per-agent render state for the agents the sweep removed
        // — load-bearing: the looped script REUSES agent ids, and a returning cast
        // member with stale walk legs teleports in.
        self.render(now, buf_w, buf_h);
        self.expand_rgba();
    }

    /// Pointer to the RGBA frame in wasm linear memory (`w*h*4` bytes).
    ///
    /// CONTRACT: re-read this (and rebuild any `Uint8ClampedArray` view) after
    /// EVERY `step` — a canvas resize reallocates the staging buffer, and any
    /// wasm `memory.grow` invalidates existing JS views into linear memory even
    /// when the pointer value is unchanged.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// Byte length of the RGBA frame (`w*h*4`).
    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Hire one more agent: a new coworker walks into the background office,
    /// works a few spells, and heads out ~70s later. Refused (`false`) before
    /// the first `step` (no clock yet), while `MAX_LIVE` hires are already
    /// alive, and when the canvas-sized office has no free desk. Never throws.
    pub fn hire(&mut self) -> bool {
        let Some(base) = self.last_now else {
            return false;
        };
        self.hires.try_hire(base, &self.scene)
    }

    /// Force the office's weather (`"clear"|"rain"|"storm"|"snow"|"fog"|
    /// "overcast"|"windy"|"smog"`), or `None` to follow the clock-based cycle.
    /// An unrecognized name renders as the clock-based cycle.
    pub fn set_weather(&mut self, name: Option<String>) {
        self.weather_override = name;
    }

    /// Recolor the whole office to a theme by name (`"normal"|"cyberpunk"|
    /// "dracula"|"tokyo-night"|"catppuccin"|"gruvbox"`). Unknown name = no-op.
    pub fn set_theme(&mut self, name: &str) {
        if let Some(t) = pixtuoid_scene::theme::theme_by_name(name) {
            self.theme = t;
            self.session.reset_frame_cache();
        }
    }

    /// Whether the office's sky shows the SUN at hour-of-day `hour` (0..24) — the
    /// site's sky-slider thumb reads this, so it delegates to the engine's ONE
    /// day/night boundary rather than restating it.
    pub fn is_day(&self, hour: f32) -> bool {
        pixtuoid_scene::pixel_painter::hour_is_day(hour)
    }

    /// Export the current frame's name-badge labels + neon wall-board TEXT as a
    /// small JSON string for the site's DOM overlay.
    ///
    /// The wasm office renders at a SMALL buffer that CSS upscales with
    /// `image-rendering: pixelated`, so anti-aliased text CANNOT be baked into the
    /// pixels — the site lays crisp DOM spans over the canvas from this model
    /// instead. Coordinates are OFFICE-BUFFER px (a label's `x` is the sprite
    /// CENTER, `y` its head-top; the board `rect` is the neon-panel interior).
    /// Colors are RESOLVED against the CURRENT theme. Call right after `step` (it
    /// reads the step's clock).
    pub fn overlay_json(&mut self) -> String {
        use pixtuoid_scene::pixel_painter::{
            NEON_PANEL_INNER_H, NEON_PANEL_INNER_W, NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y,
        };
        let Some(now) = self.last_now else {
            return r#"{"labels":[],"board":null}"#.to_string();
        };
        let theme = self.theme;

        let labels = self.session.overlay(&self.scene, now, None);
        let board = self.session.board(&self.scene, now, None);

        let mut out = String::from("{\"labels\":[");
        for (i, el) in labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let cx = el.anchor_px.x as i32 + CHARACTER_SPRITE_W as i32 / 2;
            out.push_str(&format!("{{\"x\":{cx},\"y\":{},\"text\":", el.anchor_px.y));
            push_json_string(&mut out, &format!("\u{25cf}{}", el.text));
            out.push_str(&format!(",\"color\":\"{}\"", label_hex(theme, el.tone)));
            // The registry prefix before the first '·' resolves to the source's
            // badge hue: the site paints the WHOLE name in it while the ● marker
            // stays the activity tone. An unregistered prefix emits no badge.
            if let Some(rgb) = pixtuoid_scene::overlay::badge_hue(&el.text, theme) {
                out.push_str(&format!(",\"badge\":\"{}\"", hex(rgb)));
            }
            out.push('}');
        }
        out.push_str(&format!(
            "],\"board\":{{\"rect\":{{\"x\":{NEON_PANEL_INNER_X},\"y\":{NEON_PANEL_INNER_Y},\"w\":{NEON_PANEL_INNER_W},\"h\":{NEON_PANEL_INNER_H}}},"
        ));
        push_board_segment(&mut out, "brand", &board.brand, theme);
        out.push(',');
        push_board_segment(&mut out, "star", &board.star, theme);
        out.push_str(",\"mood\":");
        push_board_segments(&mut out, &board.mood, theme);
        out.push_str(",\"context\":");
        push_board_segments(&mut out, &board.context, theme);
        out.push_str("}}");
        out
    }

    // JS drives the audio after the ♩ click (browser autoplay policy):
    // audio_begin → pump audio_warmup_step off setTimeout(0) → upload the
    // buffers via the ptr/len getters → audio_tick per rAF.

    /// Create the audio engine for the CURRENT day/night + weather. Idempotent —
    /// a second call is ignored. Costs nothing until `audio_warmup_step`.
    pub fn audio_begin(&mut self) {
        if self.audio.is_some() {
            return;
        }
        let track = self.current_track();
        self.audio = Some(audio::WebAudioDriver::new(track));
    }

    /// Build ONE synthesis piece; returns pieces REMAINING (0 = ready to upload
    /// buffers + tick). JS loops it off `setTimeout(0)` so the multi-second
    /// synthesis never blocks the main thread in one shot.
    pub fn audio_warmup_step(&mut self) -> u32 {
        self.audio.as_mut().map_or(0, |a| a.warmup_step())
    }

    /// The engine's sample rate (Hz) — JS builds its `AudioBuffer`s at this rate
    /// (the browser resamples to the AudioContext rate).
    pub fn audio_sample_rate(&self) -> u32 {
        pixtuoid_scene::audio::dsp::SAMPLE_RATE
    }

    /// Zero-copy pointer/length into the looping bed samples for stem `idx`
    /// (0=Pad … 5=Rain). RE-READ after warmup completes AND whenever a tick
    /// reports `swapped`.
    pub fn audio_loop_ptr(&self, idx: usize) -> *const f32 {
        self.audio
            .as_ref()
            .map_or(std::ptr::null(), |a| a.loop_buffer(idx).as_ptr())
    }
    pub fn audio_loop_len(&self, idx: usize) -> usize {
        self.audio.as_ref().map_or(0, |a| a.loop_buffer(idx).len())
    }

    /// Zero-copy pointer/length into a one-shot buffer: `pool` is the wire index
    /// (0=keystroke, 1=raindrop, 2=door chime, 3=printer, 4=vending), `idx` the
    /// pool slot.
    pub fn audio_oneshot_ptr(&self, pool: u8, idx: usize) -> *const f32 {
        self.audio.as_ref().map_or(std::ptr::null(), |a| {
            OneShotPool::from_wire(pool)
                .map_or(std::ptr::null(), |p| a.oneshot_buffer(p, idx).as_ptr())
        })
    }
    pub fn audio_oneshot_len(&self, pool: u8, idx: usize) -> usize {
        self.audio.as_ref().map_or(0, |a| {
            OneShotPool::from_wire(pool).map_or(0, |p| a.oneshot_buffer(p, idx).len())
        })
    }

    /// Advance the audio one tick at `now_ms` (the site's pause-shifted clock,
    /// same as `step`) and return the JS glue commands as JSON:
    /// `{"gains":[g0..g5],"plays":[[poolWire,idx,gain],…],"swapped":bool}` — JS
    /// ramps each GainNode to its gain, spawns the one-shots, and on `swapped`
    /// re-reads the loop buffers.
    pub fn audio_tick(&mut self, now_ms: f64) -> String {
        let Some(now) = self.last_now else {
            return r#"{"gains":[0,0,0,0,0,0],"plays":[],"swapped":false}"#.to_string();
        };
        if self.audio.as_ref().map(|a| a.is_ready()) != Some(true) {
            return r#"{"gains":[0,0,0,0,0,0],"plays":[],"swapped":false}"#.to_string();
        }
        // The shared observer composes the whole AudioFrame, single-sourced with
        // the desktop painters. Single-floor hero → floor 0.
        let frame = self.session.audio_frame(&self.scene, 0, now);
        let cmd = self
            .audio
            .as_mut()
            .expect("audio ready checked above")
            .tick(now_ms, frame);
        audio::commands_json(&cmd)
    }

    /// Stage a handoff for the worker's spawn-time track. A stale epoch at click
    /// time self-heals through the normal chunked swap. Overwrites any prior stage.
    pub fn audio_adopt_begin(&mut self, night: bool, epoch: f64) {
        let epoch = epoch as u64;
        let track = if night {
            pixtuoid_scene::audio::TrackId::GenNight(epoch)
        } else {
            pixtuoid_scene::audio::TrackId::GenDay(epoch)
        };
        self.adopting = Some(audio::Adoption::new(track));
    }

    /// Copy in one one-shot buffer (the worker's pool-discovery order).
    /// `false` = refused (no stage / bad pool / overflow).
    pub fn audio_adopt_oneshot(&mut self, pool: u8, samples: &[f32]) -> bool {
        let Some(ad) = self.adopting.as_mut() else {
            return false;
        };
        match OneShotPool::from_wire(pool) {
            Some(p) => ad.push_oneshot(p, samples),
            None => false,
        }
    }

    /// Copy in loop stem `idx` (`LoopStem::ALL` order). Same `false` contract as
    /// `audio_adopt_oneshot`.
    pub fn audio_adopt_loop(&mut self, idx: usize, samples: &[f32]) -> bool {
        match self.adopting.as_mut() {
            Some(ad) => ad.push_loop(idx, samples),
            None => false,
        }
    }

    /// Promote a COMPLETE handoff to the live driver. Refuses a torn handoff, and
    /// refuses to stomp a driver a click already warmed (first ready wins).
    pub fn audio_adopt_finish(&mut self) -> bool {
        let Some(ad) = self.adopting.take() else {
            return false;
        };
        if self.audio.is_some() {
            return false;
        }
        match ad.finish() {
            Some(driver) => {
                self.audio = Some(driver);
                true
            }
            None => false,
        }
    }
}

/// The worker-side synthesizer: the audio-prewarm Web Worker instantiates its
/// OWN wasm module (memories can't be shared), pumps [`SynthTake::step`] to 0,
/// then transfers each buffer to the main thread for `Office::audio_adopt_*`.
#[wasm_bindgen]
pub struct SynthTake {
    driver: audio::WebAudioDriver,
    night: bool,
    epoch: u64,
}

#[wasm_bindgen]
impl SynthTake {
    /// `now_ms` = UNIX-epoch milliseconds (the `Office::step` contract) — selects
    /// the same day/night + weather track the office would at that instant.
    #[wasm_bindgen(constructor)]
    pub fn new(now_ms: f64) -> SynthTake {
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(now_ms as u64);
        // `floor::track_for` is the ONE track-pick authority; its TrackId payload
        // IS the track epoch, so the adopt wire's (night, epoch) recovers from it.
        let track = pixtuoid_scene::floor::track_for(now);
        let (night, epoch) = match track {
            pixtuoid_scene::audio::TrackId::GenNight(e) => (true, e),
            pixtuoid_scene::audio::TrackId::GenDay(e) => (false, e),
        };
        SynthTake {
            driver: audio::WebAudioDriver::new(track),
            night,
            epoch,
        }
    }

    /// Build ONE synthesis piece; pieces remaining (0 = done).
    pub fn step(&mut self) -> u32 {
        self.driver.warmup_step()
    }

    /// The selected track, split for the adopt wire (`audio_adopt_begin`).
    pub fn night(&self) -> bool {
        self.night
    }
    pub fn epoch(&self) -> f64 {
        self.epoch as f64
    }

    /// The loop-stem count — loops aren't self-terminating like the one-shot
    /// pools, so JS can't discover the copy-out bound itself.
    pub fn loop_count(&self) -> usize {
        pixtuoid_scene::audio::mixer::LoopStem::ALL.len()
    }

    /// Zero-copy reads, same contract as the `Office::audio_*` getters — the
    /// worker copies before its next wasm call.
    pub fn loop_ptr(&self, idx: usize) -> *const f32 {
        self.driver.loop_buffer(idx).as_ptr()
    }
    pub fn loop_len(&self, idx: usize) -> usize {
        self.driver.loop_buffer(idx).len()
    }
    pub fn oneshot_ptr(&self, pool: u8, idx: usize) -> *const f32 {
        OneShotPool::from_wire(pool).map_or(std::ptr::null(), |p| {
            self.driver.oneshot_buffer(p, idx).as_ptr()
        })
    }
    pub fn oneshot_len(&self, pool: u8, idx: usize) -> usize {
        OneShotPool::from_wire(pool).map_or(0, |p| self.driver.oneshot_buffer(p, idx).len())
    }
}

impl Office {
    fn current_track(&self) -> pixtuoid_scene::audio::TrackId {
        match self.last_now {
            Some(now) => pixtuoid_scene::floor::track_for(now),
            None => pixtuoid_scene::audio::TrackId::GenDay(0),
        }
    }
}

impl Office {
    /// The rendered RGBA frame (`w*h*4`, opaque alpha) — the safe NATIVE accessor
    /// for rlib consumers; the wasm-JS boundary keeps the zero-copy
    /// [`Office::frame_ptr`]/[`Office::frame_len`] pair instead, because a
    /// `&[u8]` doesn't cross wasm-bindgen without copying.
    pub fn frame(&self) -> &[u8] {
        &self.rgba
    }

    /// Keep the reducer's desk capacity in lockstep with the office actually
    /// rendered at this buffer size. Without it an admitted agent's desk index
    /// can exceed the canvas layout's desk count, so it paints NOWHERE (its
    /// anchors return `None`) while staying alive in the scene. A shrink lowers
    /// capacity for FUTURE admissions only; already-seated excess agents stay
    /// alive-but-offscreen, same as the TUI on terminal shrink.
    fn sync_capacity(&mut self, buf_w: u16, buf_h: u16) {
        if self.caps_size == Some((buf_w, buf_h)) {
            return;
        }
        // The SAME (size, cap=None, seed) computation `render` feeds
        // `render_floor`, so reducer capacity and painted layout can't drift.
        let cap = floor_capacity(buf_w, buf_h, self.seed);
        self.scene.floor_capacities = std::array::from_fn(|i| if i == 0 { cap } else { 0 });
        self.caps_size = Some((buf_w, buf_h));
    }

    /// Fire every scripted beat due by `now`, each applied at its SCHEDULED time
    /// (not `now`) so the reducer's time-based semantics hold even when a hidden
    /// tab's rAF pauses and a resumed step has to catch up a large gap. A gap
    /// past one full loop is re-anchored instead of replayed N times.
    fn advance_script(&mut self, now: SystemTime) {
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
            // Each presence delta lands at its SCHEDULED time, so enter/busy/leave
            // motion anchors correctly even on a catch-up step.
            while let Some(pb) = self.presence_beats.get(self.presence_cursor) {
                if pb.at_ms > elapsed {
                    break;
                }
                let at = epoch + Duration::from_millis(pb.at_ms);
                apply_presence(&mut self.scene, &hero_gateway(), pb.update.clone(), at);
                self.presence_cursor += 1;
            }
            if elapsed < LOOP_MS {
                break;
            }
            epoch += Duration::from_millis(LOOP_MS);
            self.epoch = Some(epoch);
            self.cursor = 0;
            self.presence_cursor = 0;
            elapsed -= LOOP_MS;
        }

        self.hires
            .drain_due(now, &mut self.reducer, &mut self.scene);
    }

    fn render(&mut self, now: SystemTime, buf_w: u16, buf_h: u16) {
        // The layout seed is the hero's variant seed (NOT floor-derived), so build
        // the meta then override the seed. Too-small layouts leave the cleared
        // buffer; never panics.
        let floor_meta = FloorMeta {
            floor_seed: self.seed,
            ..FloorMeta::for_floor(0, 1)
        };
        self.session.render(FrameInputs {
            scene: &self.scene,
            pack: &self.pack,
            theme: self.theme,
            now,
            size: Size { w: buf_w, h: buf_h },
            floor_meta,
            active_pet: None,
            floor_pet: None,
            debug_walkable: false,
        });
    }

    /// `Rgb` is not `repr(C)`, so expand into the RGBA staging vec per-pixel
    /// (opaque alpha) — don't cast.
    fn expand_rgba(&mut self) {
        let px = self.session.buf().as_slice();
        self.rgba.clear();
        self.rgba.reserve(px.len() * 4);
        for c in px {
            self.rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
        }
    }
}

// Hand-built JSON — the wasm artifact ships no serde.

fn hex(c: pixtuoid_core::sprite::Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn label_hex(theme: &Theme, tone: pixtuoid_scene::overlay::LabelTone) -> String {
    hex(pixtuoid_scene::overlay::label_tone_rgb(tone, theme))
}

fn board_hex(theme: &Theme, tone: pixtuoid_scene::board::BoardTone) -> String {
    hex(pixtuoid_scene::board::tone_rgb(tone, theme))
}

/// Append `s` as a JSON string literal (quotes + escapes) to `out`. Agent labels
/// derive from arbitrary cwds, so one unescaped char would break the whole
/// frame's `JSON.parse` on the site.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_board_segment(
    out: &mut String,
    key: &str,
    seg: &pixtuoid_scene::board::BoardSegment,
    theme: &Theme,
) {
    out.push_str(&format!("\"{key}\":{{\"text\":"));
    push_json_string(out, &seg.text);
    out.push_str(&format!(",\"color\":\"{}\"}}", board_hex(theme, seg.tone)));
}

fn push_board_segments(
    out: &mut String,
    segs: &[pixtuoid_scene::board::BoardSegment],
    theme: &Theme,
) {
    out.push('[');
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"text\":");
        push_json_string(out, &seg.text);
        out.push_str(&format!(",\"color\":\"{}\"}}", board_hex(theme, seg.tone)));
    }
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::cast_id;

    /// `Office::new`'s error arm constructs a `JsError`, so unwrap via match.
    fn office() -> Office {
        match Office::new(1) {
            Ok(o) => o,
            Err(_) => panic!("embedded pack must parse"),
        }
    }

    /// The worker→main handoff, minus the JS/postMessage hop.
    fn adopt_take_into(o: &mut Office, now_ms: f64) -> SynthTake {
        let mut take = SynthTake::new(now_ms);
        while take.step() > 0 {}
        o.audio_adopt_begin(take.night(), take.epoch());
        for i in 0..take.loop_count() {
            assert!(
                o.audio_adopt_loop(i, take.driver.loop_buffer(i)),
                "loop {i} adopted"
            );
        }
        for wire in 0u8..5 {
            let pool = OneShotPool::from_wire(wire).expect("known wire");
            let mut j = 0;
            while !take.driver.oneshot_buffer(pool, j).is_empty() {
                assert!(
                    o.audio_adopt_oneshot(wire, take.driver.oneshot_buffer(pool, j)),
                    "{pool:?}[{j}] adopted"
                );
                j += 1;
            }
        }
        take
    }

    #[test]
    fn a_worker_take_adopts_into_an_office_and_the_click_is_upload_only() {
        let mut o = office();
        let take = adopt_take_into(&mut o, 1_753_000_000_000.0);
        assert!(o.audio_adopt_finish(), "complete handoff promotes");
        o.audio_begin();
        assert_eq!(o.audio_warmup_step(), 0, "click-time synthesis skipped");
        for i in 0..take.loop_count() {
            assert_eq!(
                o.audio_loop_len(i),
                take.driver.loop_buffer(i).len(),
                "stem {i} upload reads the adopted bytes"
            );
        }
        assert!(
            !o.audio_adopt_finish(),
            "a second finish has nothing staged"
        );
    }

    #[test]
    fn a_click_warmed_driver_wins_over_a_late_adoption() {
        let mut o = office();
        o.audio_begin();
        while o.audio_warmup_step() > 0 {}
        let baseline = o.audio_loop_len(0);
        adopt_take_into(&mut o, 1_753_000_600_000.0);
        assert!(
            !o.audio_adopt_finish(),
            "first ready wins — a late adoption must not stomp the live driver"
        );
        assert_eq!(o.audio_loop_len(0), baseline, "live driver untouched");
        assert!(!o.audio_adopt_loop(0, &[0.1]));
        assert!(!o.audio_adopt_oneshot(0, &[0.1]));
    }

    #[test]
    fn synth_take_matches_what_the_main_thread_would_have_synthesized() {
        let mut take = SynthTake::new(1_753_000_000_000.0);
        while take.step() > 0 {}
        let mut take2 = SynthTake::new(1_753_000_000_000.0);
        while take2.step() > 0 {}
        assert_eq!(take.night(), take2.night());
        assert_eq!(take.epoch(), take2.epoch());
        for i in 0..take.loop_count() {
            assert_eq!(
                take.driver.loop_buffer(i),
                take2.driver.loop_buffer(i),
                "stem {i} is deterministic for a now_ms"
            );
        }
    }

    /// Anchor sim time well past 0 so `exiting_at`-style guards never see the
    /// UNIX_EPOCH sentinel; the value itself is arbitrary.
    const T0_MS: f64 = 1_000_000_000.0;

    #[test]
    fn overlay_json_carries_the_cli_badge_hue_for_registered_prefixes() {
        let mut o = office();
        o.step(T0_MS, 320, 180);
        o.step(T0_MS + 10_000.0, 320, 180);
        let json = o.overlay_json();
        let cc = pixtuoid_scene::theme::ALL_THEMES[0].source.claude_code;
        let expect = format!("\"badge\":\"#{:02x}{:02x}{:02x}\"", cc.r, cc.g, cc.b);
        assert!(
            json.contains(&expect),
            "labels carry the cc badge hue: {json}"
        );
        // Scope the count to the labels array: board segments also emit "text".
        let labels_json = json.split("],\"board\"").next().unwrap();
        let labels = labels_json.split("\"text\":").count() - 1;
        let badges = labels_json.split("\"badge\":").count() - 1;
        assert!(labels > 0, "the seated cast produced labels");
        assert_eq!(labels, badges, "every cast label resolves a badge hue");
    }

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
        o.step(T0_MS + 100.0, 480, 270);
        assert_eq!(o.frame_len(), 480 * 270 * 4);
        // Too small for any layout: render early-returns, still a valid frame.
        o.step(T0_MS + 200.0, 8, 8);
        assert_eq!(o.frame_len(), 8 * 8 * 4);
    }

    #[test]
    fn beats_fire_once_at_scheduled_times_across_a_wrap() {
        let mut o = office();
        let step_ms = 5_000u64;
        let total = LOOP_MS + LOOP_MS / 2;
        let mut t = 0u64;
        while t <= total {
            o.step(T0_MS + t as f64, 160, 96);
            t += step_ms;
        }
        assert!(
            (8..=11).contains(&o.scene.agents.len()),
            "cast bounded across the wrap, got {}",
            o.scene.agents.len()
        );
        assert!(o.cursor > 0 && o.cursor < o.beats.len());
    }

    #[test]
    fn overlay_json_before_first_step_is_empty_but_valid() {
        let mut o = office();
        let v: serde_json::Value = serde_json::from_str(&o.overlay_json()).expect("valid JSON");
        assert!(v["labels"].as_array().unwrap().is_empty());
        assert!(v["board"].is_null());
    }

    #[test]
    fn json_string_escapes_quotes_backslashes_and_controls() {
        let mut s = String::new();
        push_json_string(&mut s, "a\"b\\c\n\td");
        assert_eq!(s, r#""a\"b\\c\n\td""#);
        let mut ctrl = String::new();
        push_json_string(&mut ctrl, "x\u{0001}y");
        assert_eq!(ctrl, "\"x\\u0001y\"");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&s)
                .unwrap()
                .as_str()
                .unwrap(),
            "a\"b\\c\n\td"
        );
    }

    #[test]
    fn overlay_json_exports_a_board_and_a_label_per_visible_agent() {
        let mut o = office();
        // Poster-sized canvas, driven to mid-loop, so several agents are seated.
        let mut t = 0u64;
        while t <= LOOP_MS / 2 {
            o.step(T0_MS + t as f64, 288, 180);
            t += 5_000;
        }
        assert!(
            !o.scene.agents.is_empty(),
            "the hero script populated the office"
        );

        let json = o.overlay_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("overlay_json is valid JSON");

        let board = &v["board"];
        assert!(
            board["brand"]["text"]
                .as_str()
                .unwrap()
                .starts_with("pixtuoid v"),
            "brand carries the version: {board}"
        );
        assert_eq!(board["star"]["text"].as_str().unwrap(), "\u{2605} Star");
        assert!(board["mood"].is_array() && board["context"].is_array());
        assert_eq!(
            board["rect"]["w"].as_u64().unwrap(),
            pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_W as u64
        );
        assert_eq!(
            board["rect"]["h"].as_u64().unwrap(),
            pixtuoid_scene::pixel_painter::NEON_PANEL_INNER_H as u64
        );
        assert!(board["brand"]["color"].as_str().unwrap().starts_with('#'));

        let labels = v["labels"].as_array().unwrap();
        assert!(!labels.is_empty(), "visible agents produce badges");
        for l in labels {
            assert!(l["x"].is_number() && l["y"].is_number());
            assert!(l["text"].as_str().unwrap().starts_with('\u{25cf}'));
            assert!(l["color"].as_str().unwrap().starts_with('#'));
        }
    }

    #[test]
    fn overlay_json_colors_track_set_theme() {
        let mut o = office();
        o.step(T0_MS, 288, 180);
        let normal: serde_json::Value = serde_json::from_str(&o.overlay_json()).unwrap();
        o.set_theme("cyberpunk");
        o.step(T0_MS + 100.0, 288, 180);
        let cyber: serde_json::Value = serde_json::from_str(&o.overlay_json()).unwrap();
        assert_ne!(
            normal["board"]["brand"]["color"], cyber["board"]["brand"]["color"],
            "the brand hue differs between normal and cyberpunk"
        );
    }

    #[test]
    fn a_long_hidden_tab_reanchors_instead_of_replaying_every_missed_loop() {
        let mut o = office();
        o.step(T0_MS, 160, 96);
        let epoch_before = o.epoch.unwrap();
        // 10 simulated minutes of a hidden tab.
        let gap = 10 * 60 * 1_000u64;
        o.step(T0_MS + gap as f64, 160, 96);
        let epoch_after = o.epoch.unwrap();
        let advanced = epoch_after
            .duration_since(epoch_before)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(advanced % LOOP_MS, 0, "re-anchor keeps the loop phase");
        assert!(gap - advanced < 2 * LOOP_MS, "at most one wrap replays");
        assert!(
            (8..=11).contains(&o.scene.agents.len()),
            "office coherent after the gap, got {}",
            o.scene.agents.len()
        );
    }

    #[test]
    fn exited_agents_render_state_is_evicted_so_loop_two_walks_dont_teleport() {
        // The door-traffic cast ids RECUR next loop; a stale entry/exit leg gates
        // on `is_none()`, so a leftover would skip the new walk-in.
        let mut o = office();
        let mut t = 0u64;
        while t <= 60_000 {
            o.step(T0_MS + t as f64, 160, 96);
            t += 1_000;
        }
        assert!(
            o.scene.agents.contains_key(&cast_id(5))
                && o.session.floor.ctx.motion.contains_key(&cast_id(5)),
            "agent 5 must be live with motion state mid-loop (positive control)"
        );
        // Past agent 5's SessionEnd + the exit grace + sweep.
        while t <= 115_000 {
            o.step(T0_MS + t as f64, 160, 96);
            t += 1_000;
        }
        assert!(
            !o.scene.agents.contains_key(&cast_id(5)),
            "agent 5 exited and was GC'd"
        );
        assert!(
            !o.session.floor.ctx.motion.contains_key(&cast_id(5)),
            "agent 5's motion state was evicted with its slot"
        );
        assert!(
            !o.session.office.coffee.map().contains_key(&cast_id(5)),
            "agent 5's coffee state was evicted with its slot"
        );
    }

    #[test]
    fn lobster_presence_follows_the_scripted_loop_through_the_real_state_machine() {
        use pixtuoid_core::state::DaemonState;
        let mut o = office();
        let gw = hero_gateway();
        let lobster = |o: &Office| {
            o.scene
                .daemon(gw.source(), gw.instance())
                .map(|p| p.display_state())
        };
        o.step(T0_MS, 160, 96);
        o.step(T0_MS + 10_000.0, 160, 96);
        assert!(lobster(&o).is_none(), "no lobster before 25s");
        o.step(T0_MS + 30_000.0, 160, 96);
        assert_eq!(lobster(&o), Some(DaemonState::Idle));
        o.step(T0_MS + 45_000.0, 160, 96);
        assert_eq!(lobster(&o), Some(DaemonState::Busy));
        assert_eq!(
            o.scene
                .daemon(gw.source(), gw.instance())
                .map(|p| p.in_flight_runs.len()),
            Some(1)
        );
        o.step(T0_MS + 100_000.0, 160, 96);
        assert_eq!(lobster(&o), Some(DaemonState::Idle));
        // Down ≠ absent — the leave animation anchors on it.
        o.step(T0_MS + 115_000.0, 160, 96);
        assert_eq!(lobster(&o), Some(DaemonState::Down));
        o.step(T0_MS + 121_000.0, 160, 96);
        assert_eq!(
            lobster(&o),
            None,
            "a walked-out gateway must go ABSENT, not linger Down to the loop wrap"
        );
        let wrap30 = T0_MS + LOOP_MS as f64 + 30_000.0;
        o.step(wrap30, 160, 96);
        assert_eq!(lobster(&o), Some(DaemonState::Idle));
    }

    #[test]
    fn hire_walks_in_works_and_leaves_without_replaying_on_wrap() {
        let mut o = office();
        assert!(!o.hire(), "hire before the first step is refused");
        assert!(
            o.hires.pending.is_empty(),
            "hire before the first step is ignored"
        );

        o.step(T0_MS, 160, 96);
        o.step(T0_MS + 30_000.0, 160, 96);
        let baseline = o.scene.agents.len();
        assert!(o.hire(), "the hire is admitted");
        o.step(T0_MS + 31_000.0, 160, 96);
        assert_eq!(o.scene.agents.len(), baseline + 1, "the hire walked in");
        o.step(T0_MS + 80_000.0, 160, 96);
        assert_eq!(o.scene.agents.len(), baseline + 1);
        // Past SessionEnd + exit grace + GC, and across the loop wrap.
        let after = T0_MS + 31_000.0 + 70_000.0 + 20_000.0 + LOOP_MS as f64;
        o.step(after, 160, 96);
        let hired_alive = o
            .scene
            .agents
            .keys()
            .filter(|id| !(0..crate::script::CAST_LEN).map(cast_id).any(|c| c == **id))
            .count();
        assert_eq!(hired_alive, 0, "the hire left and never replays");
    }

    #[test]
    fn frame_exposes_the_same_bytes_as_the_ptr_len_contract() {
        let mut o = office();
        o.step(T0_MS, 160, 96);
        assert_eq!(o.frame().len(), o.frame_len());
        assert_eq!(o.frame().as_ptr(), o.frame_ptr());
    }

    #[test]
    fn capacity_tracks_the_canvas_layout_so_no_agent_is_stranded_unpainted() {
        use pixtuoid_scene::layout::Layout;
        // The site renders BUF_H=130; this width seats the full cast plus at
        // least one spare. The free-desk count is DERIVED, not a size literal —
        // the density pass re-tunes desks-per-buffer out from under a literal.
        let (w, h) = (192u32, 130u32);
        let mut o = office();
        let mut t = 0u64;
        while t <= 30_000 {
            o.step(T0_MS + t as f64, w, h);
            t += 1_000;
        }
        let free = o.scene.total_capacity() - o.scene.agents.len();
        assert!(free >= 1, "the layout must leave a spare desk for a hire");
        let clicks = free.min(VisitorHires::MAX_LIVE) + 1;
        let admitted: Vec<bool> = (0..clicks).map(|_| o.hire()).collect();
        assert!(
            admitted[..clicks - 1].iter().all(|&ok| ok),
            "every free desk (capped by MAX_LIVE) seats a click: {admitted:?}"
        );
        assert!(
            !admitted[clicks - 1],
            "the click past exhaustion is refused outright: {admitted:?}"
        );
        o.step(T0_MS + 32_000.0, w, h);
        let layout = Layout::compute_with_seed(w as u16, h as u16, None, o.seed)
            .expect("the portrait buffer lays out");
        assert_eq!(
            o.scene.total_capacity(),
            layout.home_desks.len(),
            "reducer capacity derives from the SAME layout the canvas renders"
        );
        for a in o.scene.agents.values() {
            let local = o.scene.floor_local_desk(a.desk_index);
            assert!(
                layout.home_desk(local).is_some(),
                "agent {:?} at desk {:?} has no paintable anchor in the canvas layout",
                a.agent_id,
                a.desk_index
            );
        }
        assert_eq!(
            o.hires.ids.len(),
            free.min(VisitorHires::MAX_LIVE),
            "hires the office can't seat are refused, not admitted-invisible"
        );
    }

    #[test]
    fn a_phone_narrow_office_the_cast_fills_refuses_hires_outright() {
        let (w, h) = (96u32, 130u32);
        let mut o = office();
        let mut t = 0u64;
        while t <= 30_000 {
            o.step(T0_MS + t as f64, w, h);
            t += 1_000;
        }
        assert!(
            o.scene.total_capacity() <= crate::script::CAST_LEN,
            "premise: this width seats no more than the cast"
        );
        assert_eq!(
            o.scene.agents.len(),
            o.scene.total_capacity(),
            "the cast fills every desk the narrow canvas lays out"
        );
        assert!(!o.hire(), "no free desk: the hire click is refused");
    }

    #[test]
    fn hire_cap_holds_under_click_spam() {
        // A roomy canvas: this exercises the MAX_LIVE cap, not desk exhaustion.
        let mut o = office();
        o.step(T0_MS, 320, 180);
        o.step(T0_MS + 30_000.0, 320, 180);
        let admitted: Vec<bool> = (0..10).map(|_| o.hire()).collect();
        assert_eq!(
            admitted.iter().filter(|&&ok| ok).count(),
            VisitorHires::MAX_LIVE,
            "only the first MAX_LIVE clicks are admitted; click spam past it is refused"
        );
        o.step(T0_MS + 32_000.0, 320, 180);
        let count_hires = |o: &Office| {
            o.scene
                .agents
                .keys()
                .filter(|id| !(0..crate::script::CAST_LEN).map(cast_id).any(|c| c == **id))
                .count()
        };
        assert_eq!(
            count_hires(&o),
            VisitorHires::MAX_LIVE,
            "click spam caps at the limit"
        );
        assert!(!o.hire(), "a post-burst click must not overshoot the cap");
        o.step(T0_MS + 33_000.0, 320, 180);
        assert_eq!(
            count_hires(&o),
            VisitorHires::MAX_LIVE,
            "a post-burst click must not overshoot the cap"
        );
    }

    #[test]
    fn hire_after_the_stay_ends_re_admits_past_the_cap() {
        use crate::script::HIRE_STAY_MS;
        let mut o = office();
        o.step(T0_MS, 320, 180);
        o.step(T0_MS + 30_000.0, 320, 180);
        let admitted: Vec<bool> = (0..VisitorHires::MAX_LIVE).map(|_| o.hire()).collect();
        assert!(
            admitted.iter().all(|&ok| ok),
            "the first MAX_LIVE clicks all seat"
        );
        assert!(
            !o.hire(),
            "a click past the cap is refused while the first three are live"
        );
        o.step(T0_MS + 32_000.0, 320, 180);

        let count_hires = |o: &Office| {
            o.scene
                .agents
                .keys()
                .filter(|id| !(0..crate::script::CAST_LEN).map(cast_id).any(|c| c == **id))
                .count()
        };
        assert_eq!(count_hires(&o), VisitorHires::MAX_LIVE);

        // Past the stay + exit grace + GC sweep.
        let after_stay = T0_MS + 32_000.0 + HIRE_STAY_MS as f64 + 20_000.0;
        o.step(after_stay, 320, 180);
        assert_eq!(count_hires(&o), 0, "the earlier hires left the office");

        assert!(
            o.hire(),
            "the cap frees up once the earlier hires pruned out"
        );
        o.step(after_stay + 1_000.0, 320, 180);
        assert_eq!(count_hires(&o), 1, "the re-admitted hire walked in");
    }

    #[test]
    fn set_weather_forces_that_weather_and_two_offices_dont_fight() {
        let mut storm = Office::new(1).unwrap();
        storm.set_weather(Some("storm".into()));
        storm.step(T0_MS, 160, 96);
        let storm_frame = storm.frame().to_vec();

        let mut clear = Office::new(1).unwrap();
        clear.set_weather(Some("clear".into()));
        clear.step(T0_MS, 160, 96);
        let clear_frame = clear.frame().to_vec();
        assert_ne!(storm_frame, clear_frame, "storm vs clear must differ");

        storm.step(T0_MS, 160, 96);
        assert_eq!(
            storm.frame(),
            &storm_frame[..],
            "storm office must keep its own weather after another office stepped"
        );

        // `force_weather` leaves the override UNTOUCHED on Err, so a swallowed Err
        // would render the PREVIOUS office's weather — here, storm.
        let mut typo = Office::new(1).unwrap();
        typo.set_weather(Some("stormy".into()));
        typo.step(T0_MS, 160, 96);
        let typo_frame = typo.frame().to_vec();

        let mut unforced = Office::new(1).unwrap();
        typo.step(T0_MS, 160, 96); // leave `typo`'s (mis)override as the last writer
        unforced.step(T0_MS, 160, 96);
        // `assert!` over the slices, not `assert_eq!`: a mismatch would otherwise
        // dump two whole frames into the failure output.
        assert!(
            typo_frame == unforced.frame(),
            "an unknown weather name must render the clock-based cycle"
        );
        assert!(
            typo_frame != storm_frame,
            "…and specifically must NOT inherit the sibling office's storm"
        );
    }

    #[test]
    fn set_theme_recolors_and_unknown_is_noop() {
        let mut a = Office::new(2).unwrap();
        a.step(T0_MS, 160, 96);
        let normal = a.frame().to_vec();

        a.set_theme("cyberpunk");
        a.step(T0_MS, 160, 96);
        assert_ne!(
            a.frame(),
            &normal[..],
            "cyberpunk must repaint the office differently from normal"
        );

        a.set_theme("nonsense");
        let before = a.frame().to_vec();
        a.step(T0_MS, 160, 96);
        assert_eq!(a.frame(), &before[..], "unknown theme is a no-op");
    }

    #[test]
    fn is_day_matches_the_engine_sun_window() {
        let o = office();
        assert!(!o.is_day(4.9), "pre-dawn is night");
        assert!(o.is_day(5.0), "sunrise is day");
        assert!(o.is_day(12.0), "noon is day");
        assert!(o.is_day(19.9), "just before sunset is day");
        assert!(!o.is_day(20.0), "sunset flips to night");
        assert!(!o.is_day(23.0), "late night is night");
    }
}
