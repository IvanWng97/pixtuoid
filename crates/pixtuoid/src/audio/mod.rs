//! Ambient office audio — the ONE consumer of the scene's
//! `pixtuoid_scene::audio::AudioFrame` model and the only owner of any
//! audio-device dependency (#633; the plan's single-gateway rule). Pure
//! synthesis (`dsp`/`synth`) pre-renders every sample buffer at startup;
//! playback rides its own thread behind a bounded channel — the render
//! loop only ever `try_send`s (drop-on-backpressure, never blocks).
//! (Phase 2's AI-generated stem ASSETS will live in `crates/pixtuoid/
//! sounds/`, the `sprites/` twin; Phase 1 ships zero asset files.)

// Everything below the handle/spawn seam is feature-gated WITH the rodio
// dep (lens-2 MEDIUM: an ungated pure half is ~40 dead_code warnings in
// every --no-default-features build — the shipped Linux artifacts).
#[cfg(feature = "audio")]
pub(crate) mod dsp;
#[cfg(feature = "audio")]
pub(crate) mod mixer;
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

/// Everything the audio thread plays, synthesized once at spawn.
#[cfg(feature = "audio")]
struct AssetBank {
    keystrokes: Vec<Arc<Vec<f32>>>,
    drops: Vec<Arc<Vec<f32>>>,
    door_chime: Arc<Vec<f32>>,
    printer_whir: Arc<Vec<f32>>,
    vending_drop: Arc<Vec<f32>>,
    rain_bed: Arc<Vec<f32>>,
}

#[cfg(feature = "audio")]
impl AssetBank {
    fn build() -> Self {
        // fixed seed: assets are identical run-to-run (reproducible audio)
        let mut rng = dsp::NoiseStream::new(0xC0FF_EE01);
        Self {
            keystrokes: (0..KEYSTROKE_POOL)
                .map(|_| Arc::new(synth::keystroke(&mut rng)))
                .collect(),
            drops: (0..DROP_POOL)
                .map(|_| Arc::new(synth::rain_drop(&mut rng)))
                .collect(),
            door_chime: Arc::new(synth::door_chime()),
            printer_whir: Arc::new(synth::printer_whir(&mut rng)),
            vending_drop: Arc::new(synth::vending_drop(&mut rng)),
            rain_bed: Arc::new(synth::rain_bed(&mut rng)),
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

// without the audio feature the handle's tx is always None, so the
// payloads are provably unread — not a bug, the inert path
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(crate) enum Msg {
    Frame(AudioFrame),
    Muted(bool),
}

/// The painters' handle — clone-cheap, non-blocking. A disabled handle
/// (audio off in config, or no device) swallows everything.
#[derive(Clone)]
pub(crate) struct AudioHandle {
    tx: Option<mpsc::SyncSender<Msg>>,
}

impl AudioHandle {
    /// The inert handle: `[audio] enabled = false` (the default) or no
    /// usable output device. Every call is a no-op.
    pub(crate) fn disabled() -> Self {
        Self { tx: None }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Push one frame of audio intent. `try_send` — a saturated audio
    /// thread drops frames rather than ever stalling the render loop.
    pub(crate) fn frame(&self, frame: AudioFrame) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Msg::Frame(frame));
        }
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(Msg::Muted(muted));
        }
    }

    /// Test seam: a live handle whose receiver the test drains — the ONE
    /// way to observe what the render path actually feeds the audio thread
    /// (the online-review HIGH: the floor-scoping wiring needs a pin).
    #[cfg(test)]
    pub(crate) fn test_pair() -> (Self, mpsc::Receiver<Msg>) {
        let (tx, rx) = mpsc::sync_channel(256);
        (Self { tx: Some(tx) }, rx)
    }
}

/// Drain every queued frame, returning them in order (test helper).
#[cfg(test)]
pub(crate) fn drain_frames(rx: &mpsc::Receiver<Msg>) -> Vec<AudioFrame> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Msg::Frame(f) = msg {
            out.push(f);
        }
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
        std::thread::Builder::new()
            .name("pixtuoid-audio".into())
            .spawn(move || run_loop(rx, Box::new(device), volume))
            .map(|_| AudioHandle { tx: Some(tx) })
            .unwrap_or_else(|e| {
                tracing::warn!("audio: thread spawn failed, running silent: {e}");
                AudioHandle::disabled()
            })
    }
}

/// The audio thread body — device-agnostic over [`AudioSink`], so the test
/// probe and the LISTEN-gate wav renderer drive the SAME loop.
#[cfg(feature = "audio")]
fn run_loop(rx: mpsc::Receiver<Msg>, mut device: Box<dyn AudioSink>, volume: f32) {
    let bank = AssetBank::build();
    device.start_loop(LoopStem::Rain, Arc::clone(&bank.rain_bed));
    // pad/sparkle/keys/drums AND texture await Phase 2: the vinyl/room
    // texture only makes sense UNDER music ("底噪没有音乐" — owner call),
    // so Phase 1's sound is entirely event-driven (typing/rain/one-shots)
    // and an empty office is truly silent. Levels still compute and ramp;
    // no loop is registered to hear them.

    let mut mixer = Mixer::new(volume);
    let mut typing = TypingScheduler::new(0xBEEF);
    let mut drops = DropScheduler::new(0xFACE);
    let mut pick = dsp::NoiseStream::new(0xDEAD);
    let mut typing_level = 0.0f32;
    let mut rain_level = 0.0f32;
    let started = Instant::now();
    let mut last_step = started;

    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(TICK_MS)) {
            Ok(Msg::Frame(frame)) => {
                typing_level = frame.stems.typing;
                rain_level = frame.stems.rain;
                mixer.set_target(frame.stems);
                for event in frame.events {
                    device.play_once(bank.one_shot(event), ONE_SHOT_GAIN * mixer.one_shot_gain());
                }
            }
            Ok(Msg::Muted(m)) => mixer.set_muted(m),
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
        let join = std::thread::spawn(move || run_loop(rx, Box::new(probe), 1.0));

        tx.send(Msg::Frame(AudioFrame {
            stems: StemLevels {
                rain: 0.5,
                ..Default::default()
            },
            events: vec![OneShot::DoorChime, OneShot::PrinterWhir],
        }))
        .unwrap();
        drop(tx);
        join.join().unwrap();

        let rec = recorder.lock().unwrap();
        assert!(
            rec.loops_started.contains(&LoopStem::Rain),
            "the rain bed registered"
        );
        assert!(
            !rec.loops_started.contains(&LoopStem::Texture),
            "texture waits for Phase 2's music (owner call: no floor noise without music)"
        );
        assert!(rec.one_shots >= 2, "the two frame events played");
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
        stems: StemLevels,
        events_at: &[(f32, OneShot)],
        secs: f32,
    ) -> Vec<f32> {
        let mut sink = OfflineSink::new(secs);
        sink.start_loop(LoopStem::Rain, Arc::clone(&bank.rain_bed));
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
        let bank = AssetBank::build();
        let quiet = StemLevels {
            texture: 0.28,
            ..Default::default()
        };
        let moderate = StemLevels {
            texture: 0.30,
            typing: 0.5,
            ..Default::default()
        };
        let busy = StemLevels {
            texture: 0.28,
            typing: 0.8,
            ..Default::default()
        };
        let rainy = StemLevels { rain: 0.55, ..busy };
        // the busy tier carries a scripted one-shot volley (incl. the
        // un-auditioned vending drop — flagged in the synth doc)
        let volley = [
            (5.0, OneShot::DoorChime),
            (10.0, OneShot::PrinterWhir),
            (15.0, OneShot::VendingDrop),
        ];
        for (name, stems, events, expect_sound) in [
            // Phase 1: an empty office is truly SILENT (texture waits for
            // Phase 2's music — owner call)
            ("tier_1_empty", quiet, &[][..], false),
            ("tier_2_moderate", moderate, &[][..], true),
            ("tier_3_busy_oneshot_volley", busy, &volley[..], true),
            ("tier_4_rainy_busy", rainy, &[][..], true),
        ] {
            let buf = render_tier(&bank, stems, events, 30.0);
            assert_eq!(
                buf.iter().any(|&s| s.abs() > 0.01),
                expect_sound,
                "{name}: unexpected audibility"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        println!("LISTEN GATE wavs at: {}", out.display());
    }
}
