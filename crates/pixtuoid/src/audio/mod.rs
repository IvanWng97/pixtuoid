//! Ambient office audio — the ONE consumer of the scene's
//! `pixtuoid_scene::audio::AudioFrame` model and the only owner of any
//! audio-device dependency (#633; the plan's single-gateway rule). Pure
//! synthesis (`dsp`/`synth`) pre-renders every sample buffer at startup —
//! including the Phase 2 musical stems (`score` + `synth`), which are
//! ALL-PROCEDURAL by owner decision (no committed assets, no decoder dep;
//! the ratified composition is frozen data in `score.rs`). Playback rides
//! its own thread behind a bounded channel — the render loop only ever
//! `try_send`s (drop-on-backpressure, never blocks).

// Everything below the handle/spawn seam is feature-gated WITH the rodio
// dep (lens-2 MEDIUM: an ungated pure half is ~40 dead_code warnings in
// every --no-default-features build — the shipped Linux artifacts).
#[cfg(feature = "audio")]
pub(crate) mod dsp;
#[cfg(feature = "audio")]
pub(crate) mod mixer;
#[cfg(feature = "audio")]
mod score;
#[cfg(feature = "audio")]
pub(crate) mod sink;
#[cfg(feature = "audio")]
pub(crate) mod synth;

use std::sync::mpsc;
#[cfg(feature = "audio")]
use std::sync::Arc;
#[cfg(feature = "audio")]
use std::time::Instant;

#[cfg(feature = "audio")]
use mixer::{DropScheduler, LoopStem, Mixer, TypingScheduler};
use pixtuoid_scene::audio::AudioFrame;
#[cfg(feature = "audio")]
use pixtuoid_scene::audio::OneShot;
#[cfg(feature = "audio")]
use sink::AudioSink;

/// Per-key / per-drop pre-rendered variant pools: playback picks randomly so
/// typing/rain never sound repeated, while runtime stays synthesis-free.
#[cfg(feature = "audio")]
const KEYSTROKE_POOL: usize = 16;
#[cfg(feature = "audio")]
const DROP_POOL: usize = 12;

/// One-shot playback gains relative to master — the loudness-matched Phase 0
/// unit levels (±2.2dB across the set), with typing under the beds.
#[cfg(feature = "audio")]
const KEYSTROKE_GAIN: f32 = 0.35;
#[cfg(feature = "audio")]
const ONE_SHOT_GAIN: f32 = 0.5;
/// Foreground raindrops sit 12-14dB ABOVE the wash per the reference — the
/// bed peaks well under 1.0, so drops ride at the rain level itself.
#[cfg(feature = "audio")]
const DROP_GAIN: f32 = 0.9;

/// The ONE-SHOT pools the audio thread keeps for its whole life,
/// synthesized once at spawn. The loop beds live in [`LoopBeds`] instead —
/// they are handed to the sink at registration and NOT retained.
#[cfg(feature = "audio")]
struct AssetBank {
    keystrokes: Vec<Arc<Vec<f32>>>,
    drops: Vec<Arc<Vec<f32>>>,
    door_chime: Arc<Vec<f32>>,
    printer_whir: Arc<Vec<f32>>,
    vending_drop: Arc<Vec<f32>>,
}

#[cfg(feature = "audio")]
impl AssetBank {
    /// `rng` is the ONE asset stream — `LoopBeds::build` continues it, so
    /// the draw order (and thus every buffer) is byte-identical to the
    /// LISTEN-ratified renders. Don't reorder the synth calls.
    fn build(rng: &mut dsp::NoiseStream) -> Self {
        Self {
            keystrokes: (0..KEYSTROKE_POOL)
                .map(|_| Arc::new(synth::keystroke(rng)))
                .collect(),
            drops: (0..DROP_POOL)
                .map(|_| Arc::new(synth::rain_drop(rng)))
                .collect(),
            door_chime: Arc::new(synth::door_chime()),
            printer_whir: Arc::new(synth::printer_whir(rng)),
            vending_drop: Arc::new(synth::vending_drop(rng)),
        }
    }

    fn one_shot(&self, event: OneShot) -> Arc<Vec<f32>> {
        match event {
            OneShot::DoorChime => Arc::clone(&self.door_chime),
            OneShot::PrinterWhir => Arc::clone(&self.printer_whir),
            OneShot::VendingDrop => Arc::clone(&self.vending_drop),
        }
    }
}

/// The six loop beds — built once, registered with the sink, then DROPPED:
/// `RodioSink::start_loop` copies each bed into its own `SamplesBuffer`, so
/// retaining these Arcs would double the ~23MB bed footprint for nothing
/// (review finding). The four musical beds share ONE sample count
/// (`musical_stems_share_one_loop_length`) and register together, so they
/// tile phase-locked; texture/rain are noise beds with independent lengths
/// (no musical phase to keep).
#[cfg(feature = "audio")]
struct LoopBeds {
    beds: [Arc<Vec<f32>>; LoopStem::ALL.len()],
}

#[cfg(feature = "audio")]
impl LoopBeds {
    /// Continues `AssetBank::build`'s rng stream — same consumption order
    /// as the ratified renders: rain, then drums, then texture (the pure
    /// musical stems draw nothing).
    fn build(rng: &mut dsp::NoiseStream) -> Self {
        let rain = Arc::new(synth::rain_bed(rng));
        let pad = Arc::new(synth::stem_pad());
        let sparkle = Arc::new(synth::stem_sparkle());
        let keys = Arc::new(synth::stem_keys());
        let drums = Arc::new(synth::stem_drums(rng));
        let texture = Arc::new(synth::texture_bed(rng));
        // index order = LoopStem::ALL order
        Self {
            beds: [pad, sparkle, keys, drums, texture, rain],
        }
    }

    fn bed(&self, stem: LoopStem) -> Arc<Vec<f32>> {
        let i = LoopStem::ALL
            .iter()
            .position(|s| *s == stem)
            .expect("every LoopStem has a bed");
        Arc::clone(&self.beds[i])
    }
}

/// The painters' handle — clone-cheap, non-blocking. A disabled handle
/// (audio off in config, or no device) swallows everything.
#[derive(Clone)]
pub(crate) struct AudioHandle {
    tx: Option<mpsc::SyncSender<AudioFrame>>,
    /// Mute is STATE, not an event: it rides this atomic instead of the
    /// droppable frame channel. During the bank-synthesis window the
    /// channel saturates and try_sends drop — an `m`/`p` keypress there
    /// must still land, or the beds fade in unmuted against a footer that
    /// says muted (review MEDIUM).
    muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Master volume (f32 bits) — same state-not-event rationale as `muted`:
    /// the +/- keys must land even while the synthesis window saturates the
    /// frame channel. The audio thread folds it into the mixer each tick.
    volume: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl AudioHandle {
    /// The inert handle: sound not requested yet (muted — the default —
    /// with the lazy spawn untriggered) or no usable output device. Every
    /// call is a no-op.
    pub(crate) fn disabled() -> Self {
        Self {
            tx: None,
            muted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            volume: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Push one frame of audio intent. `try_send` — a saturated audio
    /// thread drops frames rather than ever stalling the render loop.
    pub(crate) fn frame(&self, frame: AudioFrame) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(frame);
        }
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        self.muted
            .store(muted, std::sync::atomic::Ordering::Relaxed);
    }

    /// Live master-volume update (pre-clamped by the caller's key handler;
    /// clamped again defensively here).
    pub(crate) fn set_volume(&self, volume: f32) {
        self.volume.store(
            volume.clamp(0.0, 1.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// The user's master volume — the footer's audibility check reads it
    /// (0% is silence even when live and unmuted).
    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The EFFECTIVE silence state (the m-toggle OR'd with pause — run_tui
    /// stores the combined value), read by the footer's ♩ indicator.
    pub(crate) fn is_muted(&self) -> bool {
        self.muted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Test seam: a live handle whose receiver the test drains — the ONE
    /// way to observe what the render path actually feeds the audio thread
    /// (the online-review HIGH: the floor-scoping wiring needs a pin).
    #[cfg(test)]
    pub(crate) fn test_pair() -> (Self, mpsc::Receiver<AudioFrame>) {
        let (tx, rx) = mpsc::sync_channel(256);
        (
            Self {
                tx: Some(tx),
                muted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                volume: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
            },
            rx,
        )
    }
}

/// Drain every queued frame, returning them in order (test helper).
#[cfg(test)]
pub(crate) fn drain_frames(rx: &mpsc::Receiver<AudioFrame>) -> Vec<AudioFrame> {
    let mut out = Vec::new();
    while let Ok(f) = rx.try_recv() {
        out.push(f);
    }
    out
}

/// How often the audio thread wakes to ramp gains / run schedulers when no
/// frames arrive (frames themselves also wake it).
#[cfg(feature = "audio")]
const TICK_MS: u64 = 50;

/// Spawn the audio thread. `volume` arrives pre-clamped from config
/// resolve. Returns a disabled handle when the `audio` feature is off or
/// no output device exists — callers never need a cfg.
pub(crate) fn spawn(volume: f32) -> AudioHandle {
    #[cfg(not(feature = "audio"))]
    {
        let _ = volume;
        AudioHandle::disabled()
    }
    #[cfg(feature = "audio")]
    {
        let Some(device) = sink::rodio_sink::RodioSink::open() else {
            return AudioHandle::disabled();
        };
        let (tx, rx) = mpsc::sync_channel(64);
        let muted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let vol = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(
            volume.clamp(0.0, 1.0).to_bits(),
        ));
        let muted_for_loop = std::sync::Arc::clone(&muted);
        let vol_for_loop = std::sync::Arc::clone(&vol);
        std::thread::Builder::new()
            .name("pixtuoid-audio".into())
            .spawn(move || run_loop(rx, Box::new(device), muted_for_loop, vol_for_loop))
            .map(|_| AudioHandle {
                tx: Some(tx),
                muted,
                volume: vol,
            })
            .unwrap_or_else(|e| {
                tracing::warn!("audio: thread spawn failed, running silent: {e}");
                AudioHandle::disabled()
            })
    }
}

/// The audio thread body — device-agnostic over [`AudioSink`], so the test
/// probe and the LISTEN-gate wav renderer drive the SAME loop.
#[cfg(feature = "audio")]
fn run_loop(
    rx: mpsc::Receiver<AudioFrame>,
    mut device: Box<dyn AudioSink>,
    muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    volume: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    // the ~2s (release; >10s debug) synthesis window: frames try_sent
    // meanwhile drop harmlessly (levels are re-sent every render frame),
    // and mute rides the atomic so a keypress here can never be lost
    let built_at = Instant::now();
    let mut rng = dsp::NoiseStream::new(0xC0FF_EE01);
    let bank = AssetBank::build(&mut rng);
    let beds = LoopBeds::build(&mut rng);
    // Phase 2: every bed registers — the musical stems (empty office =
    // pad+sparkle+texture, the ratified "someone left the radio on") joined
    // by keys/drums as the office busies; texture re-wired WITH them (the
    // Phase 1 "no floor noise without music" owner call, now satisfied).
    // The sink copies each bed; `beds` drops here so nothing is held twice.
    for stem in LoopStem::ALL {
        device.start_loop(stem, beds.bed(stem));
    }
    drop(beds);
    tracing::debug!(
        ms = built_at.elapsed().as_millis(),
        "audio: assets synthesized and beds registered"
    );

    let mut mixer = Mixer::new(f32::from_bits(
        volume.load(std::sync::atomic::Ordering::Relaxed),
    ));
    let mut typing = TypingScheduler::new(0xBEEF);
    let mut drops = DropScheduler::new(0xFACE);
    let mut pick = dsp::NoiseStream::new(0xDEAD);
    let mut typing_level = 0.0f32;
    let mut rain_level = 0.0f32;
    let started = Instant::now();
    let mut last_step = started;

    loop {
        let msg = rx.recv_timeout(std::time::Duration::from_millis(TICK_MS));
        mixer.set_muted(muted.load(std::sync::atomic::Ordering::Relaxed));
        mixer.set_master(f32::from_bits(
            volume.load(std::sync::atomic::Ordering::Relaxed),
        ));
        match msg {
            Ok(frame) => {
                typing_level = frame.stems.typing;
                rain_level = frame.stems.rain;
                mixer.set_target(frame.stems);
                for event in frame.events {
                    device.play_once(bank.one_shot(event), ONE_SHOT_GAIN * mixer.one_shot_gain());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();
        let dt = now.duration_since(last_step).as_secs_f32();
        last_step = now;
        for (stem, gain) in mixer.step(dt) {
            device.set_loop_gain(stem, gain);
        }

        let now_s = now.duration_since(started).as_secs_f64();
        let os_gain = mixer.one_shot_gain();
        for _ in 0..typing.tick(now_s, typing_level) {
            let idx = (pick.unit() * bank.keystrokes.len() as f32) as usize % bank.keystrokes.len();
            device.play_once(Arc::clone(&bank.keystrokes[idx]), KEYSTROKE_GAIN * os_gain);
        }
        for _ in 0..drops.tick(now_s, rain_level) {
            let idx = (pick.unit() * bank.drops.len() as f32) as usize % bank.drops.len();
            device.play_once(
                Arc::clone(&bank.drops[idx]),
                DROP_GAIN * rain_level * os_gain,
            );
        }
    }
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;
    use pixtuoid_scene::audio::StemLevels;

    #[test]
    fn disabled_handle_swallows_everything() {
        let h = AudioHandle::disabled();
        assert!(!h.is_enabled());
        h.frame(AudioFrame {
            events: vec![OneShot::DoorChime],
            ..Default::default()
        });
        h.set_muted(true); // no panic, no effect — the inert path
    }

    #[test]
    fn run_loop_registers_beds_plays_events_and_exits_on_disconnect() {
        // drive the REAL thread body against a recording sink via the
        // channel, then drop the sender — the loop must exit cleanly
        let (tx, rx) = mpsc::sync_channel(8);
        let recorder = Arc::new(std::sync::Mutex::new(sink::NullSink::default()));
        struct Probe(Arc<std::sync::Mutex<sink::NullSink>>);
        impl AudioSink for Probe {
            fn start_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.0.lock().unwrap().start_loop(stem, s);
            }
            fn set_loop_gain(&mut self, stem: LoopStem, g: f32) {
                self.0.lock().unwrap().set_loop_gain(stem, g);
            }
            fn play_once(&mut self, s: Arc<Vec<f32>>, g: f32) {
                self.0.lock().unwrap().play_once(s, g);
            }
        }
        let probe = Probe(Arc::clone(&recorder));
        let muted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let muted_ctl = std::sync::Arc::clone(&muted);
        let vol = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits()));
        let join = std::thread::spawn(move || run_loop(rx, Box::new(probe), muted, vol));

        // rain stays 0 so no scheduler one-shot can race the count —
        // only the two frame events are audible
        tx.send(AudioFrame {
            stems: StemLevels::default(),
            events: vec![OneShot::DoorChime, OneShot::PrinterWhir],
        })
        .unwrap();
        // wait until the loop has processed frame 1 (the bank build delays
        // it by seconds) so the mute below deterministically lands BETWEEN
        // the frames, not before both
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while recorder.lock().unwrap().one_shots < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "frame 1 was never processed"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // mute flips the ATOMIC (not a droppable channel message): the
        // second frame's events must play at gain 0 → uncounted (the
        // review MEDIUM: a mute during the bank-build window was lost)
        muted_ctl.store(true, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioFrame {
            stems: StemLevels::default(),
            events: vec![OneShot::DoorChime, OneShot::VendingDrop],
        })
        .unwrap();
        drop(tx);
        join.join().unwrap();

        let rec = recorder.lock().unwrap();
        for stem in LoopStem::ALL {
            assert!(
                rec.loops_started.contains(&stem),
                "Phase 2 registers every bed, missing {stem:?}"
            );
        }
        assert_eq!(
            rec.one_shots, 2,
            "the unmuted frame's 2 events played; the post-mute frame's 2 did not"
        );
        // each stem got the RIGHT bed, not just A bed (a bed() arm swap
        // must fail): noise beds carry the bed-loop length, and the four
        // musical beds are told apart by their ratified centroid ordering
        // drums(215) < pad(291) < keys(350) < sparkle(608)
        let len_of = |s: LoopStem| rec.loop_samples[&s].len();
        assert_eq!(len_of(LoopStem::Rain), 1 << 19, "rain = the noise-bed loop");
        assert_eq!(
            len_of(LoopStem::Texture),
            1 << 19,
            "texture = the noise-bed loop"
        );
        let c = |s: LoopStem| dsp::centroid_hz(&rec.loop_samples[&s]);
        let (d, p, k, sp) = (
            c(LoopStem::Drums),
            c(LoopStem::Pad),
            c(LoopStem::Keys),
            c(LoopStem::Sparkle),
        );
        assert!(
            d < p && p < k && k < sp,
            "musical beds must sit in the ratified centroid order: drums {d:.0} < pad {p:.0} < keys {k:.0} < sparkle {sp:.0}"
        );
    }
}

/// The LISTEN gate (plan §7 — the audio twin of render-and-WATCH): renders
/// each busy-ness tier through the REAL mixer/schedulers/synth into wav
/// files for the owner's audition. `#[ignore]` — run explicitly:
/// `cargo test -p pixtuoid --lib audio::listen_gate -- --ignored --nocapture`
#[cfg(all(test, feature = "audio"))]
mod listen_gate {
    use super::*;
    use pixtuoid_scene::audio::StemLevels;
    use std::io::Write;

    /// Offline sink: sample-accurate mixdown of loops (per-step gain) and
    /// scheduled one-shots into one master buffer.
    struct OfflineSink {
        master: Vec<f32>,
        loops: Vec<(Arc<Vec<f32>>, f32)>, // (samples, current gain)
        loop_ids: Vec<LoopStem>,
        cursor: usize, // master write position (samples)
    }

    impl OfflineSink {
        fn new(secs: f32) -> Self {
            Self {
                master: vec![0.0; (secs * dsp::SAMPLE_RATE as f32) as usize],
                loops: Vec::new(),
                loop_ids: Vec::new(),
                cursor: 0,
            }
        }

        /// Advance offline time by `n` samples, mixing every loop at its
        /// current gain into the master.
        fn advance(&mut self, n: usize) {
            for i in 0..n {
                let at = self.cursor + i;
                if at >= self.master.len() {
                    return;
                }
                for (samples, gain) in &self.loops {
                    self.master[at] += samples[at % samples.len()] * gain;
                }
            }
            self.cursor += n;
        }
    }

    impl AudioSink for OfflineSink {
        fn start_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
            self.loops.push((samples, 0.0));
            self.loop_ids.push(stem);
        }
        fn set_loop_gain(&mut self, stem: LoopStem, gain: f32) {
            if let Some(i) = self.loop_ids.iter().position(|s| *s == stem) {
                self.loops[i].1 = gain;
            }
        }
        fn play_once(&mut self, samples: Arc<Vec<f32>>, gain: f32) {
            for (i, &s) in samples.iter().enumerate() {
                if let Some(slot) = self.master.get_mut(self.cursor + i) {
                    *slot += s * gain;
                }
            }
        }
    }

    fn write_wav(path: &std::path::Path, samples: &[f32]) {
        let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&dsp::SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(dsp::SAMPLE_RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            let clipped = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&clipped.to_le_bytes());
        }
        std::fs::File::create(path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
    }

    fn render_tier(
        bank: &AssetBank,
        beds: &LoopBeds,
        stems: StemLevels,
        events_at: &[(f32, OneShot)],
        secs: f32,
    ) -> Vec<f32> {
        let mut sink = OfflineSink::new(secs);
        for stem in LoopStem::ALL {
            sink.start_loop(stem, beds.bed(stem));
        }
        let mut mixer = Mixer::new(1.0);
        mixer.set_target(stems);
        let mut typing = TypingScheduler::new(0xBEEF);
        let mut drops = DropScheduler::new(0xFACE);
        let mut pick = dsp::NoiseStream::new(0xDEAD);
        let step_s = 0.05f32;
        let step_n = (step_s * dsp::SAMPLE_RATE as f32) as usize;
        let mut fired = vec![false; events_at.len()];
        let mut now_s = 0.0f64;
        while now_s < secs as f64 {
            for (stem, gain) in mixer.step(step_s) {
                sink.set_loop_gain(stem, gain);
            }
            for (i, (at, ev)) in events_at.iter().enumerate() {
                if !fired[i] && now_s >= *at as f64 {
                    fired[i] = true;
                    sink.play_once(bank.one_shot(*ev), ONE_SHOT_GAIN);
                }
            }
            for _ in 0..typing.tick(now_s, stems.typing) {
                let idx =
                    (pick.unit() * bank.keystrokes.len() as f32) as usize % bank.keystrokes.len();
                sink.play_once(Arc::clone(&bank.keystrokes[idx]), KEYSTROKE_GAIN);
            }
            for _ in 0..drops.tick(now_s, stems.rain) {
                let idx = (pick.unit() * bank.drops.len() as f32) as usize % bank.drops.len();
                sink.play_once(Arc::clone(&bank.drops[idx]), DROP_GAIN * stems.rain);
            }
            sink.advance(step_n);
            now_s += step_s as f64;
        }
        sink.master
    }

    #[test]
    #[ignore = "the LISTEN gate: renders audition wavs for the owner's ears"]
    fn render_listen_gate_wavs() {
        let out = std::env::temp_dir().join("pixtuoid-audio-audition");
        std::fs::create_dir_all(&out).unwrap();
        let mut rng = dsp::NoiseStream::new(0xC0FF_EE01);
        let bank = AssetBank::build(&mut rng);
        let beds = LoopBeds::build(&mut rng);
        // tier levels come from the PRODUCTION mapping, not hand-rolled
        // literals — the wavs audition exactly what the app will mix
        let counts = |active: usize| pixtuoid_scene::board::StateCounts {
            active,
            waiting: 0,
            idle: 0,
            exiting: 0,
            total: active,
        };
        let quiet = pixtuoid_scene::audio::stem_levels(&counts(0), 0.0);
        let moderate = pixtuoid_scene::audio::stem_levels(&counts(1), 0.0);
        let busy = pixtuoid_scene::audio::stem_levels(&counts(3), 0.0);
        let rainy = pixtuoid_scene::audio::stem_levels(&counts(3), 1.0);
        // the busy tier carries a scripted one-shot volley
        let volley = [
            (5.0, OneShot::DoorChime),
            (10.0, OneShot::PrinterWhir),
            (15.0, OneShot::VendingDrop),
        ];
        for (name, stems, events) in [
            // Phase 2: an empty office plays the ratified pad+sparkle+
            // texture radio-on floor (demo_1 / p3_soak_empty)
            ("tier_1_empty", quiet, &[][..]),
            ("tier_2_moderate", moderate, &[][..]),
            ("tier_3_busy_oneshot_volley", busy, &volley[..]),
            ("tier_4_rainy_busy", rainy, &[][..]),
        ] {
            let buf = render_tier(&bank, &beds, stems, events, 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: every tier is audible in Phase 2"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        println!("LISTEN GATE wavs at: {}", out.display());
    }
}
