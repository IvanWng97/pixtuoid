//! A `WebAudioDriver` that runs the SAME `pixtuoid_scene::audio` mixer the
//! native rodio thread runs, but instead of writing to a device it RECORDS the
//! sink commands each tick for the site's WebAudio glue to flush.
//!
//! Time is a PARAMETER (`now_ms` from JS): the driver never reads a clock, and
//! dt is CLAMPED — a backgrounded tab whose `now_ms` jumps seconds neither
//! ramp-snaps the crossfade nor replays a keystroke backlog.

use std::sync::Arc;

use pixtuoid_scene::audio::bank::{AssetBank, TrackBeds, DROP_POOL, KEYSTROKE_POOL, TRACK_STEMS};
use pixtuoid_scene::audio::compose::GeneratedScore;
use pixtuoid_scene::audio::dsp::NoiseStream;
use pixtuoid_scene::audio::mixer::LoopStem;
use pixtuoid_scene::audio::{
    synth, AudioEngine, AudioFrame, OneShotPool, TickCommands, TrackId, BUILD_SEED, MAX_DT_S,
};

pub(crate) struct WebAudioDriver {
    rng: NoiseStream,
    bank: Option<AssetBank>,
    rain: Option<Arc<Vec<f32>>>,
    beds: Option<TrackBeds>,
    stage: u8,
    warm: Option<LaneBuild>,
    /// An in-flight CHUNKED rebuild, one bed per tick: synthesizing all five
    /// beds in one rAF tick hitched the page at every switch.
    pending: Option<PendingBuild>,
    track: TrackId,

    engine: AudioEngine,
    last_ms: Option<f64>,
}

impl WebAudioDriver {
    /// Master is fixed 1.0: the web has no volume UI — mute = JS suspending the
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

    /// A driver assembled from worker-synthesized pieces, ready immediately.
    /// The rng starts at position 0 where a locally-warmed driver sits
    /// post-warmup: only FUTURE swap-bed noise textures differ (the score
    /// itself is seed-deterministic), and nothing compares those cross-path.
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
    /// returning the pieces REMAINING. JS pumps this off `setTimeout(0)` so the
    /// main thread never blocks on the multi-second synthesis.
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
            // The range derives from WARMUP_STAGES: a hardcoded 2..=6 would
            // silently strand is_ready() if TRACK_STEMS ever resized.
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

    /// `now_ms` is the site's PAUSE-SHIFTED clock, so pause freezes the sound
    /// coherently.
    pub(crate) fn tick(&mut self, now_ms: f64, frame: AudioFrame) -> TickCommands {
        let dt = match self.last_ms {
            Some(prev) => (((now_ms - prev) / 1000.0) as f32).clamp(0.0, MAX_DT_S),
            None => 0.0,
        };
        self.last_ms = Some(now_ms);

        // Silence at the SOURCE while a rebuild is in flight: the mixer then
        // ramps to zero and back when the swap lands — no clamping, no pop.
        let mut frame = frame;
        if self.pending.is_some() {
            frame.stems.silence_track_stems();
        }
        let mut cmds = self.engine.tick(dt, Some(frame));

        // A newer swap mid-build restarts the stage (latest wins).
        if let Some(to) = cmds.swap.take() {
            self.pending = Some(PendingBuild {
                to,
                build: LaneBuild::new(to),
            });
        }
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

    /// JS reads this zero-copy, so it must re-read every time `swapped` is set:
    /// memory.grow / a track swap moves the data.
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

    /// MUST return empty for any `index` past the pool's end: the JS discovery
    /// loop grows until the first empty slot, so an unbounded non-empty would
    /// spin the browser's main thread forever.
    pub(crate) fn oneshot_buffer(&self, pool: OneShotPool, index: usize) -> &[f32] {
        let Some(bank) = &self.bank else {
            return &[];
        };
        match pool {
            OneShotPool::Keystroke => bank.keystrokes.get(index).map(|a| a.as_slice()),
            OneShotPool::Drop => bank.drops.get(index).map(|a| a.as_slice()),
            OneShotPool::DoorChime => (index == 0).then(|| bank.door_chime.as_slice()),
            OneShotPool::PrinterWhir => (index == 0).then(|| bank.printer_whir.as_slice()),
            OneShotPool::VendingDrop => (index == 0).then(|| bank.vending_drop.as_slice()),
        }
        .unwrap_or(&[])
    }
}

const WARMUP_STAGES: u8 = 2 + TRACK_STEMS.len() as u8;

struct LaneBuild {
    score: GeneratedScore,
    beds: Vec<Arc<Vec<f32>>>,
}

impl LaneBuild {
    fn new(track: TrackId) -> Self {
        Self {
            score: pixtuoid_scene::audio::compose_track(track),
            beds: Vec::new(),
        }
    }

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

struct PendingBuild {
    to: TrackId,
    build: LaneBuild,
}

/// The worker-prewarm handoff: buffers synthesized in a Web Worker's OWN wasm
/// instance are copied here piece-by-piece, because a postMessage transfer
/// can't cross wasm memories. A torn handoff (worker died mid-stream) yields
/// `None` so the click-time warmup runs instead.
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

    /// Loop stem `idx` in `LoopStem::ALL` order: the track beds sequentially
    /// (out-of-order aborts), then rain last.
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

/// Non-finite is impossible here (gains are bounded ramps) but maps to 0
/// defensively, so a bad frame degrades to silence rather than invalid JSON.
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
            gains: [0.1, 0.2, 0.0, 0.0, 0.0, 0.0, 0.35],
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
        assert_eq!(v["gains"].as_array().unwrap().len(), 7);
        assert_eq!(v["gains"][6].as_f64().unwrap(), 0.35);
        assert_eq!(v["plays"][0][0], 0, "keystroke wire = 0");
        assert_eq!(v["plays"][0][1], 7, "keystroke index");
        assert_eq!(v["plays"][1][0], 3, "printer wire = 3");
        assert_eq!(v["swapped"], true);
        for p in OneShotPool::ALL {
            assert_eq!(OneShotPool::from_wire(p.wire()), Some(p));
        }
        assert_eq!(OneShotPool::from_wire(9), None);
    }

    #[test]
    fn warmup_builds_bank_rain_then_one_bed_per_step_then_is_ready() {
        let mut d = WebAudioDriver::new(TrackId::GenDay(0));
        assert!(!d.is_ready());
        assert_eq!(d.warmup_step(), 7);
        assert!(!d.oneshot_buffer(OneShotPool::DoorChime, 0).is_empty());
        assert_eq!(d.warmup_step(), 6);
        assert!(!d.loop_buffer(6).is_empty(), "rain bed (stem 6) ready");
        for remaining in (0..6).rev() {
            assert!(!d.is_ready(), "not ready before the last bed");
            assert_eq!(d.warmup_step(), remaining);
        }
        assert!(d.is_ready());
        assert_eq!(d.warmup_step(), 0, "warmup is idempotent once ready");
        for i in 0..7 {
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
        // 200 ticks × 50ms = 10s, so the typing scheduler's first burst gap
        // (~2-3s at this rate) fires well inside the window.
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
        d.tick(0.0, frame.clone());
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
        let mut now = 0.0;
        for _ in 0..40 {
            now += 50.0;
            d.tick(now, day.clone());
        }
        let mut night = day.clone();
        night.track = TrackId::GenNight(0);
        let mut swapped_seen = false;
        for _ in 0..80 {
            now += 50.0;
            let cmd = d.tick(now, night.clone());
            if cmd.swap.is_some() {
                swapped_seen = true;
                for g in &cmd.gains[0..TRACK_STEMS.len()] {
                    assert!(*g <= 1e-4, "track stems held silent through the swap");
                }
                break;
            }
        }
        assert!(swapped_seen, "the night switch completed a swap");
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
        let mut ad = adopt_all_of(&src, track);
        ad.rain = None;
        assert!(ad.finish().is_none(), "missing rain is a torn handoff");
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
        let mut ad = Adoption::new(track);
        for i in 0..LoopStem::ALL.len() {
            ad.push_loop(i, src.loop_buffer(i));
        }
        assert!(ad.finish().is_none(), "missing one-shots is a torn handoff");
    }

    #[test]
    fn a_swap_builds_one_bed_per_tick_and_signals_js_only_when_done() {
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
        // the commit tick builds lane 0, so the swap lands on the tick that
        // builds the LAST lane
        assert_eq!(
            swap - commit,
            TRACK_STEMS.len() - 1,
            "the beds land one per tick"
        );
        assert_eq!(d.track, TrackId::GenNight(7));
        assert!(d.pending.is_none(), "the stage is consumed by the swap");
    }
}
