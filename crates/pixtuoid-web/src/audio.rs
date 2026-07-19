//! The THIRD audio painter (#633 web-audio): a `WebAudioDriver` that runs the
//! SAME `pixtuoid_scene::audio` mixer / schedulers / [`TrackSwitch`] the native
//! rodio thread runs, but instead of writing to a device it RECORDS the sink
//! commands each tick for the site's WebAudio glue to flush — the audio twin of
//! `Office::overlay_json` (all feel logic stays in Rust; JS is dumb glue).
//!
//! Time is a PARAMETER (`now_ms` from JS), like the render `step`: the driver
//! never reads a clock. dt is derived from the passed timestamps and CLAMPED,
//! and the scheduler clock advances by that clamped dt — so a backgrounded tab
//! whose `now_ms` jumps seconds neither ramp-snaps the crossfade nor replays a
//! keystroke backlog (the stall-clock class, web-side).
//!
//! MOOD-TRACK SWITCH: the INITIAL bank/rain/beds synthesis is chunked across
//! `warmup_step` calls (JS pumps it off `setTimeout(0)` after the ♩ click so
//! the main thread never blocks seconds at once). A mid-visit day↔night switch
//! rebuilds only the 5 track beds in ONE tick once the stems have ramped to
//! silence — rare (a real-clock boundary / 10-min weather flip) and inaudible
//! (it happens under the hold), so it isn't chunked.

use std::sync::Arc;

use pixtuoid_scene::audio::bank::{self, AssetBank, TrackBeds};
use pixtuoid_scene::audio::dsp::NoiseStream;
use pixtuoid_scene::audio::mixer::{DropScheduler, LoopStem, Mixer, TypingScheduler};
use pixtuoid_scene::audio::{
    synth, AudioFrame, OneShot, StemLevels, TrackId, TrackSwitch, BUILD_SEED, DROP_SEED, PICK_SEED,
    TYPING_SEED,
};

/// dt ceiling (s): a bigger inter-tick gap (backgrounded tab, GC pause) is
/// clamped so the crossfade can't snap and the scheduler can't burst-replay.
const MAX_DT_S: f32 = 0.10;

/// Which pre-uploaded one-shot pool a [`PlayCmd`] draws from — JS holds one
/// `AudioBuffer` per (pool, index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OneShotPool {
    Keystroke,
    Drop,
    DoorChime,
    PrinterWhir,
    VendingDrop,
}

impl OneShotPool {
    /// Stable index for the JSON wire (JS maps it back to its buffer bank).
    pub(crate) fn wire(self) -> u8 {
        match self {
            OneShotPool::Keystroke => 0,
            OneShotPool::Drop => 1,
            OneShotPool::DoorChime => 2,
            OneShotPool::PrinterWhir => 3,
            OneShotPool::VendingDrop => 4,
        }
    }
}

/// One one-shot to spawn this tick: a fresh source from `(pool, index)` at `gain`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayCmd {
    pub(crate) pool: OneShotPool,
    pub(crate) index: usize,
    pub(crate) gain: f32,
}

/// What one [`WebAudioDriver::tick`] produces for the JS glue.
pub(crate) struct TickCommands {
    /// Target gain per LOOP stem, in `LoopStem::ALL` order (0=Pad … 5=Rain) —
    /// JS ramps each stem's `GainNode` toward it.
    pub(crate) gains: [f32; LoopStem::ALL.len()],
    /// One-shots to fire this tick (keystrokes / raindrops / appliance cues).
    pub(crate) plays: Vec<PlayCmd>,
    /// The mood track just swapped — JS re-reads every loop buffer (the 5 track
    /// stems changed) and hot-swaps its looping sources.
    pub(crate) swapped: bool,
}

/// The web audio engine. Native-constructible + unit-testable (the rlib target);
/// the wasm-bindgen surface in `lib.rs` wraps it for JS.
pub(crate) struct WebAudioDriver {
    // --- built during warmup (in the native rng draw order) ---
    rng: NoiseStream,
    bank: Option<AssetBank>,
    rain: Option<Arc<Vec<f32>>>,
    beds: Option<TrackBeds>,
    /// 0=bank, 1=rain, 2=beds, 3=ready — the initial warmup cursor.
    stage: u8,
    /// The track the CURRENT `beds` were built for (also the warmup target).
    track: TrackId,

    // --- runtime ---
    mixer: Mixer,
    typing: TypingScheduler,
    drops: DropScheduler,
    pick: NoiseStream,
    switch: TrackSwitch,
    typing_level: f32,
    rain_level: f32,
    wanted: StemLevels,
    /// Monotonic scheduler clock (s), advanced by the CLAMPED dt — gap-immune.
    sched_s: f64,
    last_ms: Option<f64>,
}

impl WebAudioDriver {
    /// A driver primed to warm up `initial_track` (day/night at the hero's
    /// boot clock). Master is fixed 1.0 (the ratified trimmed bus, via
    /// `mixer::master_amp`); the web has no volume UI — mute = JS suspends the
    /// AudioContext, so the mixer always runs unmuted.
    pub(crate) fn new(initial_track: TrackId) -> Self {
        Self {
            rng: NoiseStream::new(BUILD_SEED),
            bank: None,
            rain: None,
            beds: None,
            stage: 0,
            track: initial_track,
            mixer: Mixer::new(1.0),
            typing: TypingScheduler::new(TYPING_SEED),
            drops: DropScheduler::new(DROP_SEED),
            pick: NoiseStream::new(PICK_SEED),
            switch: TrackSwitch::new(),
            typing_level: 0.0,
            rain_level: 0.0,
            wanted: StemLevels::default(),
            sched_s: 0.0,
            last_ms: None,
        }
    }

    /// Build ONE synthesis piece (bank → rain → beds, the native rng order),
    /// returning pieces REMAINING. JS pumps this off `setTimeout(0)` after the
    /// ♩ click so the main thread never blocks on the multi-second synthesis.
    /// A no-op (returns 0) once ready.
    pub(crate) fn warmup_step(&mut self) -> u32 {
        match self.stage {
            0 => {
                self.bank = Some(AssetBank::build(&mut self.rng));
                self.stage = 1;
            }
            1 => {
                self.rain = Some(Arc::new(synth::rain_bed(&mut self.rng)));
                self.stage = 2;
            }
            2 => {
                self.beds = Some(TrackBeds::build(&mut self.rng, self.track));
                self.switch.init(self.track);
                self.stage = 3;
            }
            _ => {}
        }
        (3u8.saturating_sub(self.stage)) as u32
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.stage >= 3
    }

    /// Advance one tick and record the JS commands. `frame` is built office-side
    /// (the SAME `stem_levels` / cue tracker / `select_track` the desktop
    /// painters use). `now_ms` is the site's pause-shifted clock (so pause
    /// freezes the sound coherently). A no-op-ish empty command set before the
    /// beds are ready.
    pub(crate) fn tick(&mut self, now_ms: f64, frame: AudioFrame) -> TickCommands {
        let dt = match self.last_ms {
            Some(prev) => (((now_ms - prev) / 1000.0) as f32).clamp(0.0, MAX_DT_S),
            None => 0.0,
        };
        self.last_ms = Some(now_ms);
        self.sched_s += dt as f64;

        self.typing_level = frame.stems.typing;
        self.rain_level = frame.stems.rain;
        self.wanted = frame.stems;

        // Track machine: after warmup the switch is already `init`ed, so a
        // changed track only ever `request`s (never re-inits). The BEDS rebuild
        // happens caller-side in `try_swap` below.
        if self.is_ready() {
            self.switch.request(frame.track);
        }

        // hold the 5 track stems silent while a switch settles
        let mut target = self.wanted;
        if self.switch.is_holding() {
            target.silence_track_stems();
        }
        self.mixer.set_target(target);
        let gain_pairs = self.mixer.step(dt);

        // once the held stems reach silence, rebuild + swap this ONE tick
        let swapped = if let Some(to) = self.switch.try_swap(bank::track_stems_silent(&gain_pairs))
        {
            self.beds = Some(TrackBeds::build(&mut self.rng, to));
            self.track = to;
            true
        } else {
            false
        };

        let mut gains = [0.0f32; LoopStem::ALL.len()];
        for (i, (_, g)) in gain_pairs.iter().enumerate() {
            gains[i] = *g;
        }

        // one-shots: the scene's fired events + the typing/rain schedulers
        let mut plays = Vec::new();
        let os_gain = self.mixer.one_shot_gain();
        if let Some(bank) = &self.bank {
            for event in &frame.events {
                plays.push(PlayCmd {
                    pool: match event {
                        OneShot::DoorChime => OneShotPool::DoorChime,
                        OneShot::PrinterWhir => OneShotPool::PrinterWhir,
                        OneShot::VendingDrop => OneShotPool::VendingDrop,
                    },
                    index: 0,
                    gain: bank::ONE_SHOT_GAIN * os_gain,
                });
            }
            for _ in 0..self.typing.tick(self.sched_s, self.typing_level) {
                let index = (self.pick.unit() * bank.keystrokes.len() as f32) as usize
                    % bank.keystrokes.len();
                plays.push(PlayCmd {
                    pool: OneShotPool::Keystroke,
                    index,
                    gain: bank::KEYSTROKE_GAIN * os_gain,
                });
            }
            for _ in 0..self.drops.tick(self.sched_s, self.rain_level) {
                let index =
                    (self.pick.unit() * bank.drops.len() as f32) as usize % bank.drops.len();
                plays.push(PlayCmd {
                    pool: OneShotPool::Drop,
                    index,
                    gain: bank::DROP_GAIN * self.rain_level * os_gain,
                });
            }
        }

        TickCommands {
            gains,
            plays,
            swapped,
        }
    }

    /// The looping bed samples for `LoopStem::ALL[idx]` (0=Pad … 5=Rain) — JS
    /// reads this (zero-copy) to (re)build its looping source. Empty until the
    /// beds are ready; re-read every time `swapped` is set (memory.grow / a
    /// track swap moves the data).
    pub(crate) fn loop_buffer(&self, idx: usize) -> &[f32] {
        match LoopStem::ALL.get(idx) {
            Some(LoopStem::Rain) => self.rain.as_deref().map(Vec::as_slice).unwrap_or(&[]),
            Some(stem) => match &self.beds {
                Some(beds) => beds.bed_slice(*stem),
                None => &[],
            },
            None => &[],
        }
    }

    /// The `(pool, index)` one-shot samples JS pre-uploads once after warmup.
    /// Pool sizes: keystroke = `bank::KEYSTROKE_POOL`, drop = `bank::DROP_POOL`,
    /// the three appliance cues = 1 each. Empty until warmup builds the bank,
    /// AND empty for any `index` PAST the pool's end — the single-sample pools
    /// return their buffer ONLY at index 0 (the JS discovery loop grows until
    /// the first empty slot, so an unbounded non-empty would spin forever).
    pub(crate) fn oneshot_buffer(&self, pool: OneShotPool, index: usize) -> &[f32] {
        let Some(bank) = &self.bank else {
            return &[];
        };
        match pool {
            OneShotPool::Keystroke => bank.keystrokes.get(index).map(|a| a.as_slice()),
            OneShotPool::Drop => bank.drops.get(index).map(|a| a.as_slice()),
            // single-sample pools: buffer at index 0 ONLY, else empty (the JS
            // discovery loop reads until the first empty slot — an unbounded
            // non-empty would spin forever)
            OneShotPool::DoorChime => (index == 0).then(|| bank.door_chime.as_slice()),
            OneShotPool::PrinterWhir => (index == 0).then(|| bank.printer_whir.as_slice()),
            OneShotPool::VendingDrop => (index == 0).then(|| bank.vending_drop.as_slice()),
        }
        .unwrap_or(&[])
    }
}

/// The inverse of [`OneShotPool::wire`] — decode the JS-side pool index (the
/// `audio_oneshot_ptr`/`_len` getters take it). `None` for an unknown wire.
pub(crate) fn pool_from_wire(wire: u8) -> Option<OneShotPool> {
    Some(match wire {
        0 => OneShotPool::Keystroke,
        1 => OneShotPool::Drop,
        2 => OneShotPool::DoorChime,
        3 => OneShotPool::PrinterWhir,
        4 => OneShotPool::VendingDrop,
        _ => return None,
    })
}

/// Serialize a tick's commands as the compact JSON the site's WebAudio glue
/// parses: `{"gains":[g0..g5],"plays":[[poolWire,idx,gain],…],"swapped":bool}`.
/// Hand-built (no serde in the wasm artifact — the `overlay_json` precedent).
pub(crate) fn commands_json(cmd: &TickCommands) -> String {
    let mut out = String::from("{\"gains\":[");
    for (i, g) in cmd.gains.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&fmt_f32(*g));
    }
    out.push_str("],\"plays\":[");
    for (i, p) in cmd.plays.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "[{},{},{}]",
            p.pool.wire(),
            p.index,
            fmt_f32(p.gain)
        ));
    }
    out.push_str(&format!("],\"swapped\":{}}}", cmd.swapped));
    out
}

/// Compact finite-float formatting for the JSON payload (JS `JSON.parse` reads
/// it). Non-finite is impossible here (gains are bounded ramps) but map to 0
/// defensively so a bad frame degrades to silence, never invalid JSON.
fn fmt_f32(v: f32) -> String {
    if v.is_finite() {
        format!("{v:.5}")
    } else {
        "0".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_oneshot_pool_has_a_finite_end_the_js_discovery_loop_can_find() {
        // The site reads oneshot_buffer(pool, j) for j=0,1,… until len==0 to
        // discover the pool size; a pool that returns non-empty for EVERY index
        // would spin the browser's main thread forever (the review HIGH). Pin
        // that every pool terminates, at its true size.
        let mut d = WebAudioDriver::new(TrackId::Day);
        while d.warmup_step() > 0 {}
        let pools = [
            (OneShotPool::Keystroke, bank::KEYSTROKE_POOL),
            (OneShotPool::Drop, bank::DROP_POOL),
            (OneShotPool::DoorChime, 1),
            (OneShotPool::PrinterWhir, 1),
            (OneShotPool::VendingDrop, 1),
        ];
        for (pool, size) in pools {
            for j in 0..size {
                assert!(
                    !d.oneshot_buffer(pool, j).is_empty(),
                    "{pool:?}[{j}] present"
                );
            }
            assert!(
                d.oneshot_buffer(pool, size).is_empty(),
                "{pool:?}[{size}] must be EMPTY — the discovery-loop terminator"
            );
        }
    }

    #[test]
    fn commands_json_is_parseable_and_pool_wire_round_trips() {
        let cmd = TickCommands {
            gains: [0.1, 0.2, 0.0, 0.0, 0.0, 0.35],
            plays: vec![
                PlayCmd {
                    pool: OneShotPool::Keystroke,
                    index: 7,
                    gain: 0.12,
                },
                PlayCmd {
                    pool: OneShotPool::PrinterWhir,
                    index: 0,
                    gain: 0.5,
                },
            ],
            swapped: true,
        };
        let json = commands_json(&cmd);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["gains"].as_array().unwrap().len(), 6);
        assert_eq!(v["gains"][5].as_f64().unwrap(), 0.35);
        assert_eq!(v["plays"][0][0], 0, "keystroke wire = 0");
        assert_eq!(v["plays"][0][1], 7, "keystroke index");
        assert_eq!(v["plays"][1][0], 3, "printer wire = 3");
        assert_eq!(v["swapped"], true);
        // every pool wire decodes back to itself
        for p in [
            OneShotPool::Keystroke,
            OneShotPool::Drop,
            OneShotPool::DoorChime,
            OneShotPool::PrinterWhir,
            OneShotPool::VendingDrop,
        ] {
            assert_eq!(pool_from_wire(p.wire()), Some(p));
        }
        assert_eq!(pool_from_wire(9), None);
    }

    #[test]
    fn warmup_builds_bank_rain_beds_in_order_then_is_ready() {
        let mut d = WebAudioDriver::new(TrackId::Day);
        assert!(!d.is_ready());
        assert_eq!(d.warmup_step(), 2); // bank built
        assert!(!d.oneshot_buffer(OneShotPool::DoorChime, 0).is_empty());
        assert_eq!(d.warmup_step(), 1); // rain built
        assert!(!d.loop_buffer(5).is_empty(), "rain bed (stem 5) ready");
        assert_eq!(d.warmup_step(), 0); // beds built
        assert!(d.is_ready());
        assert_eq!(d.warmup_step(), 0, "warmup is idempotent once ready");
        for i in 0..6 {
            assert!(!d.loop_buffer(i).is_empty(), "loop stem {i} has a bed");
        }
        for i in 0..bank::KEYSTROKE_POOL {
            assert!(!d.oneshot_buffer(OneShotPool::Keystroke, i).is_empty());
        }
    }

    #[test]
    fn tick_ramps_loop_gains_and_fires_scheduled_typing() {
        let mut d = WebAudioDriver::new(TrackId::Day);
        while d.warmup_step() > 0 {}
        // a busy office: typing level high, all music stems up
        let busy = pixtuoid_scene::audio::stem_levels(
            &pixtuoid_scene::board::StateCounts {
                active: 3,
                waiting: 0,
                idle: 0,
                exiting: 0,
                total: 3,
            },
            0.0,
        );
        let frame = AudioFrame {
            stems: busy,
            events: Vec::new(),
            track: TrackId::Day,
        };
        // 200 ticks × 50ms = 10s: the crossfade climbs from 0 in the first ~2s,
        // and the typing scheduler's first burst gap (~2-3s at this rate) fires
        // well inside the window.
        let mut now = 0.0;
        let mut typed = 0;
        let mut last_pad = 0.0;
        for _ in 0..200 {
            now += 50.0;
            let cmd = d.tick(now, frame.clone());
            last_pad = cmd.gains[0];
            typed += cmd
                .plays
                .iter()
                .filter(|p| p.pool == OneShotPool::Keystroke)
                .count();
        }
        assert!(last_pad > 0.0, "pad gain ramped up");
        assert!(
            typed > 0,
            "the typing scheduler fired keystrokes for a busy office"
        );
    }

    #[test]
    fn a_big_time_gap_does_not_snap_the_ramp_or_burst_typing() {
        // the stall-clock class, web-side: a backgrounded tab whose now_ms
        // jumps must clamp dt (no ramp snap) and not replay a keystroke backlog.
        let mut d = WebAudioDriver::new(TrackId::Day);
        while d.warmup_step() > 0 {}
        let busy = pixtuoid_scene::audio::stem_levels(
            &pixtuoid_scene::board::StateCounts {
                active: 3,
                waiting: 0,
                idle: 0,
                exiting: 0,
                total: 3,
            },
            0.0,
        );
        let frame = AudioFrame {
            stems: busy,
            events: Vec::new(),
            track: TrackId::Day,
        };
        d.tick(0.0, frame.clone()); // establish last_ms
                                    // jump 30 SECONDS forward in one tick
        let cmd = d.tick(30_000.0, frame.clone());
        assert!(
            cmd.gains[0] <= pixtuoid_scene::audio::mixer::RAMP_PER_S * MAX_DT_S + 1e-4,
            "one clamped tick can't snap the pad gain to full"
        );
        assert!(
            cmd.plays
                .iter()
                .filter(|p| p.pool == OneShotPool::Keystroke)
                .count()
                < 50,
            "the scheduler clock advanced by the clamped dt, not 30s of backlog"
        );
    }

    #[test]
    fn a_track_change_holds_silent_then_swaps() {
        let mut d = WebAudioDriver::new(TrackId::Day);
        while d.warmup_step() > 0 {}
        let day = AudioFrame {
            stems: pixtuoid_scene::audio::stem_levels(
                &pixtuoid_scene::board::StateCounts {
                    active: 1,
                    waiting: 0,
                    idle: 0,
                    exiting: 0,
                    total: 1,
                },
                0.0,
            ),
            events: Vec::new(),
            track: TrackId::Day,
        };
        // settle the day mix up
        let mut now = 0.0;
        for _ in 0..40 {
            now += 50.0;
            d.tick(now, day.clone());
        }
        // request night: the stems must ramp DOWN to silence, then swap once
        let mut night = day.clone();
        night.track = TrackId::Night;
        let mut swapped_seen = false;
        for _ in 0..80 {
            now += 50.0;
            let cmd = d.tick(now, night.clone());
            if cmd.swapped {
                swapped_seen = true;
                // at the swap tick the track stems were silent
                for g in &cmd.gains[0..5] {
                    assert!(*g <= 1e-4, "track stems held silent through the swap");
                }
                break;
            }
        }
        assert!(swapped_seen, "the night switch completed a swap");
        // after the swap, the mix ramps back up
        let mut pad_after = 0.0;
        for _ in 0..40 {
            now += 50.0;
            pad_after = d.tick(now, night.clone()).gains[0];
        }
        assert!(
            pad_after > 0.0,
            "the night mix ramps back in after the swap"
        );
    }
}
