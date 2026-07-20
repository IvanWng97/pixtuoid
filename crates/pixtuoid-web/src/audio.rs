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
//! SYNTHESIS PATHS (#705): the primary path is a Web Worker — at page idle it
//! runs [`crate::SynthTake`] in its OWN wasm instance and the buffers are
//! adopted here ([`Adoption`]) so the ♩ click is upload-only, near-instant.
//! The chunked `warmup_step` pump (one bed per step, off `setTimeout(0)`)
//! remains the click-time FALLBACK (dead worker / no module workers /
//! reduced-motion). A mid-visit track switch rebuilds the 5 beds one per
//! tick under the ramped-to-silence hold ([`PendingBuild`]) — inaudible, so
//! it deliberately stays on the main thread rather than the worker.

use std::sync::Arc;

use pixtuoid_scene::audio::bank::{AssetBank, TrackBeds, DROP_POOL, KEYSTROKE_POOL, TRACK_STEMS};
use pixtuoid_scene::audio::compose::{compose, GeneratedScore, Mood};
use pixtuoid_scene::audio::dsp::NoiseStream;
use pixtuoid_scene::audio::mixer::LoopStem;
use pixtuoid_scene::audio::{
    synth, AudioEngine, AudioFrame, OneShotPool, TickCommands, TrackId, BUILD_SEED, MAX_DT_S,
};

/// The web audio engine. Native-constructible + unit-testable (the rlib target);
/// the wasm-bindgen surface in `lib.rs` wraps it for JS.
pub(crate) struct WebAudioDriver {
    // --- built during warmup (in the native rng draw order) ---
    rng: NoiseStream,
    bank: Option<AssetBank>,
    rain: Option<Arc<Vec<f32>>>,
    beds: Option<TrackBeds>,
    /// 0=bank, 1=rain, 2..=6=one track bed each, 7=ready — the initial
    /// warmup cursor (see [`WARMUP_STAGES`]).
    stage: u8,
    /// The warmup's per-lane staging (#705) — the shared [`LaneBuild`]
    /// machine, drained into `beds` on the last lane.
    warm: Option<LaneBuild>,
    /// An in-flight CHUNKED rebuild (one bed per tick): a committed swap
    /// no longer synthesizes all five beds in one rAF tick — at the
    /// 10-minute song cadence that hitched the page every switch. While
    /// pending, the incoming frame's track stems are silenced (the
    /// caller-hold pattern), and JS learns of the swap only when the
    /// last bed lands.
    pending: Option<PendingBuild>,
    /// The track the CURRENT `beds` were built for (also the warmup target).
    track: TrackId,

    // --- runtime ---
    /// The shared per-tick engine (mixer, schedulers, switch machine) — the
    /// SAME `pixtuoid_scene::audio::AudioEngine` the native gateway runs, so the
    /// two soundtracks can't drift. dt is clamped to `MAX_DT_S` before it here.
    engine: AudioEngine,
    /// Last JS timestamp (ms); the clamped delta feeds the engine's clock.
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
            warm: None,
            pending: None,
            stage: 0,
            track: initial_track,
            engine: AudioEngine::new(1.0),
            last_ms: None,
        }
    }

    /// A driver assembled from worker-synthesized pieces (#705) — ready
    /// immediately, state-identical to a locally-warmed one, so the upload /
    /// tick / swap paths downstream never know the difference. The rng starts
    /// at position 0 where a locally-warmed driver sits post-warmup: only
    /// FUTURE swap-bed noise textures differ (the score itself is
    /// seed-deterministic), and nothing compares those bytes cross-path.
    pub(crate) fn adopted(
        track: TrackId,
        bank: AssetBank,
        rain: Arc<Vec<f32>>,
        beds: [Arc<Vec<f32>>; TRACK_STEMS.len()],
    ) -> Self {
        let mut engine = AudioEngine::new(1.0);
        engine.init_track(track);
        Self {
            rng: NoiseStream::new(BUILD_SEED),
            bank: Some(bank),
            rain: Some(rain),
            beds: Some(TrackBeds::from_arcs(beds)),
            warm: None,
            pending: None,
            stage: WARMUP_STAGES,
            track,
            engine,
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
            // one bed per step (#705): the old build-all-five stage was
            // the longest single main-thread block of the whole warmup.
            // The range derives from WARMUP_STAGES (review finding: a
            // hardcoded 2..=6 would silently strand is_ready() if
            // TRACK_STEMS ever resized), and the per-lane machine is the
            // SAME LaneBuild the swap rebuild runs.
            s if (2..WARMUP_STAGES).contains(&s) => {
                let build = self.warm.get_or_insert_with(|| LaneBuild::new(self.track));
                if let Some(beds) = build.step(&mut self.rng) {
                    self.beds = Some(TrackBeds::from_arcs(beds));
                    self.warm = None;
                    self.engine.init_track(self.track);
                }
                self.stage = s + 1;
            }
            _ => {}
        }
        (WARMUP_STAGES.saturating_sub(self.stage)) as u32
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.stage >= WARMUP_STAGES
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

        // While a chunked rebuild is in flight, keep the track stems
        // silent at the SOURCE (the caller-hold pattern the pre-fold
        // players used): the engine's mixer then ramps toward zero
        // targets and ramps back smoothly when the swap lands — no
        // output clamping, no pop.
        let mut frame = frame;
        if self.pending.is_some() {
            frame.stems.silence_track_stems();
        }
        let mut cmds = self.engine.tick(dt, Some(frame));

        // The BUILD stays caller-side (the engine's sharp edge), but NOT
        // in one tick: a committed swap stages a per-lane build (the
        // 10-minute cadence made the one-tick five-bed synthesis a
        // recurring main-thread hitch). A newer swap mid-build restarts
        // the stage (latest wins).
        if let Some(to) = cmds.swap.take() {
            self.pending = Some(PendingBuild {
                to,
                build: LaneBuild::new(to),
            });
        }
        // advance ONE lane per tick; on the last lane, hot-swap and tell JS
        let finished = match self.pending.as_mut() {
            Some(p) => p.build.step(&mut self.rng),
            None => None,
        };
        if let Some(beds) = finished {
            if let Some(p) = self.pending.take() {
                self.beds = Some(TrackBeds::from_arcs(beds));
                self.track = p.to;
                cmds.swap = Some(p.to);
            }
        }
        cmds
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
/// Warmup piece count: bank + rain + one step per track bed (#705 —
/// keeps every main-thread block to a single bed).
const WARMUP_STAGES: u8 = 2 + TRACK_STEMS.len() as u8;

/// The ONE per-lane build state machine (review finding: warmup and the
/// swap rebuild had two hand-copies of it): compose once, then each
/// [`LaneBuild::step`] renders the next lane; the final step hands back
/// the assembled bed array.
struct LaneBuild {
    score: GeneratedScore,
    beds: Vec<Arc<Vec<f32>>>,
}

impl LaneBuild {
    fn new(track: TrackId) -> Self {
        let (mood, seed) = match track {
            TrackId::GenDay(seed) => (Mood::Day, seed),
            TrackId::GenNight(seed) => (Mood::Night, seed),
        };
        Self {
            score: compose(mood, seed),
            beds: Vec::new(),
        }
    }

    /// Render ONE more lane; `Some(beds)` on the last.
    fn step(&mut self, rng: &mut NoiseStream) -> Option<[Arc<Vec<f32>>; TRACK_STEMS.len()]> {
        let lane = self.beds.len();
        self.beds
            .push(Arc::new(synth::gen_bed(&self.score, lane, rng)));
        if self.beds.len() == TRACK_STEMS.len() {
            <[Arc<Vec<f32>>; TRACK_STEMS.len()]>::try_from(std::mem::take(&mut self.beds)).ok()
        } else {
            None
        }
    }
}

/// One chunked track rebuild in flight (the mid-visit swap path).
struct PendingBuild {
    to: TrackId,
    build: LaneBuild,
}

/// The worker-prewarm handoff (#705): buffers synthesized in a Web Worker's
/// OWN wasm instance are copied here piece-by-piece (a postMessage transfer
/// can't cross wasm memories), then [`Adoption::finish`] assembles a fully
/// ready driver. Every push validates order/bounds and `finish` validates
/// completeness against the bank consts, so a torn handoff (worker died
/// mid-stream) yields `None` and the click-time warmup runs instead.
pub(crate) struct Adoption {
    track: TrackId,
    keystrokes: Vec<Arc<Vec<f32>>>,
    drops: Vec<Arc<Vec<f32>>>,
    door_chime: Option<Arc<Vec<f32>>>,
    printer_whir: Option<Arc<Vec<f32>>>,
    vending_drop: Option<Arc<Vec<f32>>>,
    beds: Vec<Arc<Vec<f32>>>,
    rain: Option<Arc<Vec<f32>>>,
}

impl Adoption {
    pub(crate) fn new(track: TrackId) -> Self {
        Self {
            track,
            keystrokes: Vec::new(),
            drops: Vec::new(),
            door_chime: None,
            printer_whir: None,
            vending_drop: None,
            beds: Vec::new(),
            rain: None,
        }
    }

    /// Append one one-shot buffer (the worker's pool-discovery order).
    /// `false` — overflow, duplicate, or empty — aborts the handoff.
    pub(crate) fn push_oneshot(&mut self, pool: OneShotPool, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }
        let arc = Arc::new(samples.to_vec());
        let slot = |o: &mut Option<Arc<Vec<f32>>>| {
            if o.is_some() {
                return false;
            }
            *o = Some(arc.clone());
            true
        };
        match pool {
            OneShotPool::Keystroke => {
                if self.keystrokes.len() >= KEYSTROKE_POOL {
                    return false;
                }
                self.keystrokes.push(arc.clone());
                true
            }
            OneShotPool::Drop => {
                if self.drops.len() >= DROP_POOL {
                    return false;
                }
                self.drops.push(arc.clone());
                true
            }
            OneShotPool::DoorChime => slot(&mut self.door_chime),
            OneShotPool::PrinterWhir => slot(&mut self.printer_whir),
            OneShotPool::VendingDrop => slot(&mut self.vending_drop),
        }
    }

    /// Loop stem `idx` in `LoopStem::ALL` order: the track beds (sequential —
    /// out-of-order aborts) then rain last.
    pub(crate) fn push_loop(&mut self, idx: usize, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }
        let arc = Arc::new(samples.to_vec());
        if idx < TRACK_STEMS.len() {
            if idx != self.beds.len() {
                return false;
            }
            self.beds.push(arc);
            true
        } else if idx == TRACK_STEMS.len() && self.rain.is_none() {
            self.rain = Some(arc);
            true
        } else {
            false
        }
    }

    /// A COMPLETE handoff becomes a ready driver; anything torn → `None`.
    pub(crate) fn finish(self) -> Option<WebAudioDriver> {
        if self.keystrokes.len() != KEYSTROKE_POOL || self.drops.len() != DROP_POOL {
            return None;
        }
        let bank = AssetBank {
            keystrokes: self.keystrokes,
            drops: self.drops,
            door_chime: self.door_chime?,
            printer_whir: self.printer_whir?,
            vending_drop: self.vending_drop?,
        };
        let beds = <[Arc<Vec<f32>>; TRACK_STEMS.len()]>::try_from(self.beds).ok()?;
        Some(WebAudioDriver::adopted(self.track, bank, self.rain?, beds))
    }
}

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
    out.push_str(&format!("],\"swapped\":{}}}", cmd.swap.is_some()));
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
    use pixtuoid_scene::audio::{bank, PlayCmd};

    #[test]
    fn every_oneshot_pool_has_a_finite_end_the_js_discovery_loop_can_find() {
        // The site reads oneshot_buffer(pool, j) for j=0,1,… until len==0 to
        // discover the pool size; a pool that returns non-empty for EVERY index
        // would spin the browser's main thread forever (the review HIGH). Pin
        // that every pool terminates, at its true size.
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
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
            swap: Some(TrackId::GenNight(0)),
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
    fn warmup_builds_bank_rain_then_one_bed_per_step_then_is_ready() {
        // #705: the bed stage is per-lane so no single warmup step blocks
        // longer than one bed — bank, rain, then exactly five bed steps
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
        assert!(!d.is_ready());
        assert_eq!(d.warmup_step(), 6); // bank built
        assert!(!d.oneshot_buffer(OneShotPool::DoorChime, 0).is_empty());
        assert_eq!(d.warmup_step(), 5); // rain built
        assert!(!d.loop_buffer(5).is_empty(), "rain bed (stem 5) ready");
        for remaining in (0..5).rev() {
            assert!(!d.is_ready(), "not ready before the last bed");
            assert_eq!(d.warmup_step(), remaining); // one track bed each
        }
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
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
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
            track: TrackId::GenDay(0),
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
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
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
            track: TrackId::GenDay(0),
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
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
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
            track: TrackId::GenDay(0),
        };
        // settle the day mix up
        let mut now = 0.0;
        for _ in 0..40 {
            now += 50.0;
            d.tick(now, day.clone());
        }
        // request night: the stems must ramp DOWN to silence, then swap once
        let mut night = day.clone();
        night.track = TrackId::GenNight(0);
        let mut swapped_seen = false;
        for _ in 0..80 {
            now += 50.0;
            let cmd = d.tick(now, night.clone());
            if cmd.swap.is_some() {
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

    /// Feed every piece of a warmed `src` through an [`Adoption`] — the
    /// worker-handoff round trip, minus the JS/postMessage hop.
    fn adopt_all_of(src: &WebAudioDriver, track: TrackId) -> Adoption {
        let mut ad = Adoption::new(track);
        for i in 0..LoopStem::ALL.len() {
            assert!(ad.push_loop(i, src.loop_buffer(i)), "loop {i}");
        }
        for (pool, size) in ONESHOT_POOL_SIZES {
            for j in 0..size {
                assert!(
                    ad.push_oneshot(pool, src.oneshot_buffer(pool, j)),
                    "{pool:?}[{j}]"
                );
            }
        }
        ad
    }

    const ONESHOT_POOL_SIZES: [(OneShotPool, usize); 5] = [
        (OneShotPool::Keystroke, KEYSTROKE_POOL),
        (OneShotPool::Drop, DROP_POOL),
        (OneShotPool::DoorChime, 1),
        (OneShotPool::PrinterWhir, 1),
        (OneShotPool::VendingDrop, 1),
    ];

    #[test]
    fn adoption_round_trips_a_warmed_driver_byte_for_byte() {
        let track = TrackId::GenNight(3);
        let mut src = WebAudioDriver::new(track);
        while src.warmup_step() > 0 {}
        let dst = adopt_all_of(&src, track)
            .finish()
            .expect("complete handoff");
        assert!(dst.is_ready(), "adopted driver is ready with zero warmup");
        for i in 0..LoopStem::ALL.len() {
            assert_eq!(dst.loop_buffer(i), src.loop_buffer(i), "stem {i} bytes");
        }
        for (pool, size) in ONESHOT_POOL_SIZES {
            for j in 0..size {
                assert_eq!(dst.oneshot_buffer(pool, j), src.oneshot_buffer(pool, j));
            }
            assert!(
                dst.oneshot_buffer(pool, size).is_empty(),
                "{pool:?} keeps the discovery-loop terminator"
            );
        }
    }

    #[test]
    fn an_adopted_driver_ticks_and_swaps_like_a_warmed_one() {
        // downstream must never know the difference: gains ramp, and a track
        // change still walks the driver's OWN chunked LaneBuild swap
        let track = TrackId::GenDay(0);
        let mut src = WebAudioDriver::new(track);
        while src.warmup_step() > 0 {}
        let mut d = adopt_all_of(&src, track).finish().expect("handoff");
        let mk = |track| AudioFrame {
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
            track,
        };
        let mut now = 0.0;
        let mut pad = 0.0;
        for _ in 0..40 {
            now += 50.0;
            pad = d.tick(now, mk(track)).gains[0];
        }
        assert!(pad > 0.0, "adopted mix ramps up");
        let mut swapped = false;
        for _ in 0..200 {
            now += 50.0;
            if d.tick(now, mk(TrackId::GenNight(9))).swap.is_some() {
                swapped = true;
                break;
            }
        }
        assert!(swapped, "the chunked swap path works post-adoption");
        assert_eq!(d.track, TrackId::GenNight(9));
    }

    #[test]
    fn a_torn_adoption_yields_none_and_bad_pushes_are_refused() {
        let track = TrackId::GenDay(1);
        let mut src = WebAudioDriver::new(track);
        while src.warmup_step() > 0 {}
        // missing rain → torn
        let mut ad = adopt_all_of(&src, track);
        ad.rain = None;
        assert!(ad.finish().is_none(), "missing rain is a torn handoff");
        // out-of-order bed / empty buffer / pool overflow / duplicate cue
        let mut ad = Adoption::new(track);
        assert!(!ad.push_loop(1, src.loop_buffer(1)), "beds are sequential");
        assert!(!ad.push_loop(0, &[]), "empty buffers are refused");
        let mut ad = adopt_all_of(&src, track);
        assert!(
            !ad.push_oneshot(
                OneShotPool::Keystroke,
                src.oneshot_buffer(OneShotPool::Keystroke, 0)
            ),
            "keystroke overflow refused"
        );
        assert!(
            !ad.push_oneshot(
                OneShotPool::DoorChime,
                src.oneshot_buffer(OneShotPool::DoorChime, 0)
            ),
            "duplicate cue refused"
        );
        // a short keystroke pool → torn
        let mut ad = Adoption::new(track);
        for i in 0..LoopStem::ALL.len() {
            ad.push_loop(i, src.loop_buffer(i));
        }
        assert!(ad.finish().is_none(), "missing one-shots is a torn handoff");
    }

    #[test]
    fn a_swap_builds_one_bed_per_tick_and_signals_js_only_when_done() {
        // the chunked-rebuild contract (the 10-min cadence fix): the tick
        // that COMMITS the swap must not hand JS `swapped` yet — the five
        // beds land one per tick, stems stay silent throughout, and JS
        // learns of the swap exactly once, when the last bed is in
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
        while d.warmup_step() > 0 {}
        let mk = |track| AudioFrame {
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
            track,
        };
        let mut now = 0.0;
        for _ in 0..40 {
            now += 50.0;
            d.tick(now, mk(TrackId::GenDay(0)));
        }
        // drive the switch until the engine commits (stems reach silence)
        let mut ticks_after_commit = None;
        let mut swap_tick = None;
        for i in 0..200 {
            now += 50.0;
            let cmd = d.tick(now, mk(TrackId::GenNight(7)));
            if d.pending.is_some() && ticks_after_commit.is_none() {
                ticks_after_commit = Some(i);
                assert!(cmd.swap.is_none(), "the commit tick must not signal JS");
            }
            if cmd.swap.is_some() {
                swap_tick = Some(i);
                break;
            }
        }
        let (commit, swap) = (
            ticks_after_commit.expect("a rebuild staged"),
            swap_tick.expect("the swap eventually signalled"),
        );
        // one lane per tick: commit tick builds lane 0, the swap lands on
        // the tick that builds lane 4 — exactly TRACK_STEMS.len() ticks
        assert_eq!(
            swap - commit,
            TRACK_STEMS.len() - 1,
            "five beds, one per tick"
        );
        assert_eq!(d.track, TrackId::GenNight(7));
        assert!(d.pending.is_none(), "the stage is consumed by the swap");
    }
}
