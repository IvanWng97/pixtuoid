//! Ambient office audio — the ONE consumer of the scene's
//! `pixtuoid_scene::audio::AudioFrame` model and the only owner of any
//! audio-device dependency (#633; the plan's single-gateway rule). Pure
//! synthesis (`dsp`/`synth`) pre-renders every sample buffer at startup —
//! including the Phase 2 musical stems (`score` + `synth`), which are
//! ALL-PROCEDURAL by owner decision (no committed assets, no decoder dep;
//! the ratified composition is frozen data in `score.rs`). Playback rides
//! its own thread behind a bounded channel — the render loop only ever
//! `try_send`s (drop-on-backpressure, never blocks).

// The PURE synth stack (dsp/mixer/score/synth) MOVED to `pixtuoid_scene::audio`
// (#633 web-audio) so the native device gateway here AND the wasm WebAudio
// painter build the SAME buffers. Only the DEVICE half stays here (sink +
// spawn + run_loop), still behind the `audio` feature with the rodio dep.
#[cfg(feature = "audio")]
pub(crate) mod sink;

use std::sync::mpsc;
#[cfg(feature = "audio")]
use std::sync::Arc;
#[cfg(feature = "audio")]
use std::time::Instant;

#[cfg(feature = "audio")]
use pixtuoid_scene::audio::mixer::{DropScheduler, LoopStem, Mixer, TypingScheduler};
use pixtuoid_scene::audio::AudioFrame;
#[cfg(feature = "audio")]
use pixtuoid_scene::audio::{dsp, synth};
// OneShot + TrackId are named only in the test fixtures now (run_loop infers
// both — `frame.events` / `switch.init(frame.track)`), so import them test-side
// to keep the prod build warning-free.
#[cfg(all(feature = "audio", test))]
use pixtuoid_scene::audio::{OneShot, TrackId};
#[cfg(feature = "audio")]
use sink::AudioSink;

// AssetBank / TrackBeds / TRACK_STEMS + the pool/gain consts MOVED to
// `pixtuoid_scene::audio::bank` (web-audio #633): pure builders, so the wasm
// WebAudio painter builds byte-identical banks from the SAME source. run_loop
// imports them below.
#[cfg(feature = "audio")]
use pixtuoid_scene::audio::bank::{
    AssetBank, TrackBeds, DROP_GAIN, KEYSTROKE_GAIN, ONE_SHOT_GAIN, TRACK_STEMS,
};

/// The +/- keys' volume increment — ONE definition for BOTH painters' key
/// handlers (`tui/mod.rs` dispatch + `floating::input`), so the two surfaces
/// can't drift on feel. Lives here (the shared gateway) because `tui` and
/// `floating` are siblings that must not import from each other.
pub(crate) const VOLUME_STEP: f32 = 0.05;
/// How long the transient volume readout stays up after a nudge (the lowfi
/// volume-timer pattern) — the TUI footer flash, the floating overlay, and
/// the volume-persist debounce window on both painters all read this one.
pub(crate) const VOLUME_FLASH_MS: u128 = 1000;

/// The two audio gestures both painters drive — the `m` toggle and the
/// `+`/`-` nudge. The KEY→action map is painter-specific (crossterm vs winit,
/// in each painter), but the STATE TRANSITION is shared: see
/// [`apply_audio_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioAction {
    ToggleMute,
    /// `true` = volume up.
    Volume(bool),
}

/// The audio UI state both painters keep — the TUI as loop locals it marshals
/// in/out per keypress, floating as an owned field.
pub(crate) struct AudioUi {
    pub(crate) handle: AudioHandle,
    pub(crate) muted: bool,
    pub(crate) volume: f32,
}

/// What the caller persists after [`apply_audio_action`] — the side effects
/// (config path, wall-clock flash) stay painter-side so the transition itself
/// is pure and unit-tested.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Persist {
    /// The mute flag changed — persist NOW (`save_audio_muted`, like a theme
    /// commit).
    pub(crate) muted: bool,
    /// The volume changed — flash the readout and persist DEBOUNCED (the +/-
    /// keys autorepeat; per-press ConfigLock rounds were a review MEDIUM).
    pub(crate) volume_nudged: bool,
}

/// THE audio mute/volume transition — the single authority BOTH painters run
/// (the TUI's `ToggleAudioMute`/`AdjustVolume` arms and floating's key
/// handler), so the two surfaces can't drift on feel (the duplicated-logic
/// review MEDIUM; VOLUME_STEP lives here for the same reason). Semantics:
/// mute toggles; volume-up from muted IS the un-mute gesture; the lazy spawn
/// (re)fires whenever sound is wanted but the system is down (`+`/`m` are
/// never dead keys — boot-muted and failed-spawn both recover); volume clamps
/// to [0, 1] by [`VOLUME_STEP`]. `paused` folds an external hold (the TUI's
/// `[p]ause`) into the effective mute; floating passes `false`. `spawn` is
/// injected so the transition is testable without a device.
pub(crate) fn apply_audio_action(
    st: &mut AudioUi,
    action: AudioAction,
    paused: bool,
    spawn: impl FnOnce(f32) -> AudioHandle,
) -> Persist {
    let mut persist = Persist {
        muted: false,
        volume_nudged: false,
    };
    match action {
        AudioAction::ToggleMute => {
            st.muted = !st.muted;
            persist.muted = true;
        }
        AudioAction::Volume(up) => {
            let delta = if up { VOLUME_STEP } else { -VOLUME_STEP };
            st.volume = (st.volume + delta).clamp(0.0, 1.0);
            if up && st.muted {
                // volume-up IS the un-mute gesture too
                st.muted = false;
                persist.muted = true;
            }
            persist.volume_nudged = true;
        }
    }
    if !st.muted && !st.handle.is_enabled() {
        // lazy (re)spawn: muted costs nothing, so the device/thread/buffers
        // only come up when sound is actually wanted
        st.handle = spawn(st.volume);
    }
    st.handle.set_muted(paused || st.muted);
    st.handle.set_volume(st.volume);
    persist
}

#[cfg(test)]
mod controls_tests {
    use super::*;

    #[test]
    fn unmute_lazy_spawns_and_mute_back_does_not() {
        let mut st = AudioUi {
            handle: AudioHandle::disabled(),
            muted: true,
            volume: 0.4,
        };
        let (live, _rx) = AudioHandle::test_pair();
        let mut spawned_at = None;
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, false, |v| {
            spawned_at = Some(v);
            live.clone()
        });
        assert!(!st.muted);
        assert_eq!(
            spawned_at,
            Some(0.4),
            "first unmute spawns at the kept volume"
        );
        assert!(st.handle.is_enabled() && !st.handle.is_muted());
        assert_eq!(
            p,
            Persist {
                muted: true,
                volume_nudged: false
            }
        );
        // muting back must NOT spawn again
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, false, |_| {
            panic!("mute must never spawn")
        });
        assert!(st.muted && st.handle.is_muted());
        assert_eq!(
            p,
            Persist {
                muted: true,
                volume_nudged: false
            }
        );
    }

    #[test]
    fn volume_up_from_muted_unmutes_and_respawns_a_dead_system() {
        // the sticky (unmuted, disabled) state: '+' must re-attempt the spawn
        let mut st = AudioUi {
            handle: AudioHandle::disabled(),
            muted: true,
            volume: 0.5,
        };
        let (live, _rx) = AudioHandle::test_pair();
        let mut spawns = 0;
        let p = apply_audio_action(&mut st, AudioAction::Volume(true), false, |_| {
            spawns += 1;
            live.clone()
        });
        assert!(!st.muted, "volume-up IS the un-mute gesture");
        assert_eq!(spawns, 1);
        assert_eq!(
            p,
            Persist {
                muted: true,
                volume_nudged: true
            }
        );
        assert!((st.volume - (0.5 + VOLUME_STEP)).abs() < 1e-6);
        assert!((st.handle.volume() - st.volume).abs() < 1e-6);
        // volume-down while unmuted: no mute persist, no respawn
        let p = apply_audio_action(&mut st, AudioAction::Volume(false), false, |_| {
            panic!("live system must not respawn")
        });
        assert_eq!(
            p,
            Persist {
                muted: false,
                volume_nudged: true
            }
        );
        assert!((st.volume - 0.5).abs() < 1e-6);
    }

    #[test]
    fn volume_clamps_at_both_rails() {
        let (live, _rx) = AudioHandle::test_pair();
        let mut st = AudioUi {
            handle: live,
            muted: false,
            volume: 1.0,
        };
        apply_audio_action(
            &mut st,
            AudioAction::Volume(true),
            false,
            |_| unreachable!(),
        );
        assert_eq!(st.volume, 1.0, "top rail");
        st.volume = 0.0;
        apply_audio_action(
            &mut st,
            AudioAction::Volume(false),
            false,
            |_| unreachable!(),
        );
        assert_eq!(st.volume, 0.0, "bottom rail");
    }

    #[test]
    fn paused_forces_effective_mute_without_touching_the_flag() {
        // the TUI's [p]ause term: the handle is silenced but the user's own
        // mute flag is preserved (unpause restores it)
        let (live, _rx) = AudioHandle::test_pair();
        let mut st = AudioUi {
            handle: live,
            muted: false,
            volume: 0.5,
        };
        apply_audio_action(&mut st, AudioAction::Volume(true), true, |_| unreachable!());
        assert!(!st.muted, "the user's flag stays unmuted");
        assert!(st.handle.is_muted(), "but paused silences the handle");
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

/// After a blocking `TrackBeds::build` (~2s release / >10s debug) the
/// thread's clocks are stale and the frame channel holds a backlog. ONE
/// recovery routine for both build arms (first-frame + pending-switch):
/// reset the ramp clock (a stale `last_step` snaps gains to target — the
/// bot HIGH), re-anchor the schedulers via their level-0 clock-hold arm
/// (else they fire the stall's backlog as a burst), and drain the queued
/// frames keeping the freshest LEVELS while discarding edge-EVENTS
/// (replayed stacked they are a clank pile; losing a chime under a track
/// change is the better artifact).
#[cfg(feature = "audio")]
#[allow(clippy::too_many_arguments)] // the loop's mutable locals, passed once from two arms
fn resync_after_stall(
    rx: &mpsc::Receiver<AudioFrame>,
    started: Instant,
    last_step: &mut Instant,
    typing: &mut TypingScheduler,
    drops: &mut DropScheduler,
    typing_level: &mut f32,
    rain_level: &mut f32,
    wanted_stems: &mut pixtuoid_scene::audio::StemLevels,
) {
    *last_step = Instant::now();
    let resync = last_step.duration_since(started).as_secs_f64();
    typing.tick(resync, 0.0);
    drops.tick(resync, 0.0);
    while let Ok(f) = rx.try_recv() {
        *typing_level = f.stems.typing;
        *rain_level = f.stems.rain;
        *wanted_stems = f.stems;
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
    // Rain is weather — track-independent, registered once. The five
    // TRACK beds register on the FIRST frame (it names the right mood for
    // the office's current hour/weather — booting Day at night would
    // synthesize a track just to crossfade it away).
    device.start_loop(LoopStem::Rain, Arc::new(synth::rain_bed(&mut rng)));
    tracing::debug!(
        ms = built_at.elapsed().as_millis(),
        "audio: one-shots + rain synthesized; track beds await the first frame"
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
    // The mood-track machine (#644) — the state half is the SHARED
    // `pixtuoid_scene::audio::TrackSwitch` (native + wasm run the same
    // latch/hold/silent-gate); only the BUILD (blocking synth here) is
    // caller-side. The cycle: hold the five track stems at target 0 → when
    // their gains reach silence, synthesize + swap (the silence covers the
    // ~2s) → release the hold.
    let mut switch = pixtuoid_scene::audio::TrackSwitch::new();
    let mut wanted_stems = pixtuoid_scene::audio::StemLevels::default();

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
                wanted_stems = frame.stems;
                if let Some(track) = switch.init(frame.track) {
                    // first frame: build + register the RIGHT track
                    let beds = TrackBeds::build(&mut rng, track);
                    for stem in TRACK_STEMS {
                        device.start_loop(stem, beds.bed(stem));
                    }
                    resync_after_stall(
                        &rx,
                        started,
                        &mut last_step,
                        &mut typing,
                        &mut drops,
                        &mut typing_level,
                        &mut rain_level,
                        &mut wanted_stems,
                    );
                } else {
                    switch.request(frame.track);
                }
                for event in frame.events {
                    device.play_once(bank.one_shot(event), ONE_SHOT_GAIN * mixer.one_shot_gain());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // while a switch is pending, hold the track stems silent (rain and
        // typing keep following the scene — weather is not the track's)
        let mut target = wanted_stems;
        if switch.is_holding() {
            target.silence_track_stems();
        }
        mixer.set_target(target);

        let now = Instant::now();
        let dt = now.duration_since(last_step).as_secs_f32();
        last_step = now;
        let gains = mixer.step(dt);
        for (stem, gain) in gains {
            device.set_loop_gain(stem, gain);
        }

        if let Some(to) = switch.try_swap(pixtuoid_scene::audio::bank::track_stems_silent(&gains)) {
            // ~2s of synthesis under silence; the ramp back in is the
            // same crossfade every tier change rides
            let beds = TrackBeds::build(&mut rng, to);
            for stem in TRACK_STEMS {
                device.swap_loop(stem, beds.bed(stem));
            }
            resync_after_stall(
                &rx,
                started,
                &mut last_step,
                &mut typing,
                &mut drops,
                &mut typing_level,
                &mut rain_level,
                &mut wanted_stems,
            );
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
            fn swap_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.0.lock().unwrap().swap_loop(stem, s);
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
            track: Default::default(),
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
            track: Default::default(),
        })
        .unwrap();
        drop(tx);
        join.join().unwrap();

        let rec = recorder.lock().unwrap();
        for stem in LoopStem::ALL {
            assert!(
                rec.loops_started.contains(&stem),
                "rain at spawn + the first frame's track beds — missing {stem:?}"
            );
        }
        assert!(rec.swaps.is_empty(), "no track switch happened");
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

#[cfg(all(test, feature = "audio"))]
mod track_switch_tests {
    use super::*;
    use pixtuoid_scene::audio::StemLevels;

    #[test]
    fn track_switch_ramps_to_silence_swaps_and_restores() {
        let (tx, rx) = mpsc::sync_channel(64);
        let recorder = Arc::new(std::sync::Mutex::new(sink::NullSink::default()));
        struct Probe(Arc<std::sync::Mutex<sink::NullSink>>);
        impl AudioSink for Probe {
            fn start_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.0.lock().unwrap().start_loop(stem, s);
            }
            fn swap_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.0.lock().unwrap().swap_loop(stem, s);
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
        let vol = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits()));
        let join = std::thread::spawn(move || run_loop(rx, Box::new(probe), muted, vol));

        let day_frame = || AudioFrame {
            stems: StemLevels {
                pad: 0.7,
                ..Default::default()
            },
            events: vec![],
            track: TrackId::Day,
        };
        // first frame registers the DAY beds
        tx.send(day_frame()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            {
                let rec = recorder.lock().unwrap();
                if rec
                    .loops_started
                    .iter()
                    .filter(|s| TRACK_STEMS.contains(s))
                    .count()
                    == 5
                {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "day beds never registered"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let day_pad_len = recorder.lock().unwrap().loop_samples[&LoopStem::Pad].len();

        // keep frames flowing so the ramp progresses; request NIGHT
        let night_frame = || AudioFrame {
            stems: StemLevels {
                pad: 0.7,
                ..Default::default()
            },
            events: vec![],
            track: TrackId::Night,
        };
        loop {
            let _ = tx.try_send(night_frame());
            {
                let rec = recorder.lock().unwrap();
                if rec.swaps.len() == TRACK_STEMS.len() {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the switch never completed"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        {
            let rec = recorder.lock().unwrap();
            // every track stem swapped exactly once, rain untouched
            for stem in TRACK_STEMS {
                assert_eq!(
                    rec.swaps.iter().filter(|(s, _)| *s == stem).count(),
                    1,
                    "{stem:?} swaps exactly once"
                );
            }
            assert!(
                !rec.swaps.iter().any(|(s, _)| *s == LoopStem::Rain),
                "rain is weather — never swapped by a mood change"
            );
            // the night pad is a DIFFERENT loop length (68 vs 72 BPM)
            let night_pad_len = rec.loop_samples[&LoopStem::Pad].len();
            assert_ne!(
                night_pad_len, day_pad_len,
                "the swap installed the night bed"
            );
        }

        // after the swap the pad RAMPS back — the first nonzero gain must
        // be a slew step, not the full goal (the bot HIGH: a stalled ramp
        // clock made dt cover the ~2s synth and snap gains to target)
        let first_gain = loop {
            let _ = tx.try_send(night_frame());
            {
                let rec = recorder.lock().unwrap();
                let g = rec.last_gain.get(&LoopStem::Pad).copied().unwrap_or(0.0);
                if g > 0.0 {
                    break g;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pad never ramped back"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(
            first_gain < 0.1,
            "post-swap gain must fade in (first step {first_gain}), not snap to target"
        );
        drop(tx);
        join.join().unwrap();
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
        fn swap_loop(&mut self, stem: LoopStem, samples: Arc<Vec<f32>>) {
            if let Some(i) = self.loop_ids.iter().position(|s| *s == stem) {
                self.loops[i].0 = samples;
            }
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
        beds: &TrackBeds,
        rain: &Arc<Vec<f32>>,
        stems: StemLevels,
        events_at: &[(f32, OneShot)],
        secs: f32,
    ) -> Vec<f32> {
        let mut sink = OfflineSink::new(secs);
        sink.start_loop(LoopStem::Rain, Arc::clone(rain));
        for stem in TRACK_STEMS {
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
        let rain = Arc::new(synth::rain_bed(&mut rng));
        let beds = TrackBeds::build(&mut rng, TrackId::Day);
        let night = TrackBeds::build(&mut rng, TrackId::Night);
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
            let buf = render_tier(&bank, &beds, &rain, stems, events, 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: every tier is audible in Phase 2"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        // the NIGHT track (#644): the runtime approximation of the v4 take
        // (no bus glue — rodio has no insert; the owner re-verifies by ear)
        for (name, stems) in [("night_moderate", moderate), ("night_rainy", rainy)] {
            let buf = render_tier(&bank, &night, &rain, stems, &[], 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: the night track is audible"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        println!("LISTEN GATE wavs at: {}", out.display());
    }
}
