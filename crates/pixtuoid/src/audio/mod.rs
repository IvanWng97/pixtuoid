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
use pixtuoid_scene::audio::mixer::LoopStem;
use pixtuoid_scene::audio::AudioFrame;
#[cfg(feature = "audio")]
use pixtuoid_scene::audio::{dsp, synth, AudioEngine, BUILD_SEED, MAX_DT_S};
// OneShot + TrackId are named only in the test fixtures now (run_loop infers
// both — `frame.events` / `frame.track`), so import them test-side to keep the
// prod build warning-free.
#[cfg(all(feature = "audio", test))]
use pixtuoid_scene::audio::{OneShot, TrackId};
#[cfg(feature = "audio")]
use sink::AudioSink;

// AssetBank / TrackBeds / TRACK_STEMS MOVED to `pixtuoid_scene::audio::bank`
// (web-audio #633): pure builders, so the wasm WebAudio painter builds
// byte-identical banks from the SAME source. The per-tick mixing/scheduling
// (mixer, schedulers, the pool/gain consts) now lives behind `AudioEngine`.
#[cfg(feature = "audio")]
use pixtuoid_scene::audio::bank::{AssetBank, TrackBeds, TRACK_STEMS};

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

/// Owns the whole mute/volume PERSIST protocol both painters used to duplicate:
/// the pure [`apply_audio_action`] transition PLUS its side effects — mute saves
/// NOW, a volume nudge marks dirty + arms the `♩ N%` readout, the debounced
/// volume save fires once that window elapses (a held `+`/`-` writes once, not
/// per repeat), and a flush on shutdown. The TUI keeps ONE of these instead of
/// five loop locals + `run_audio_action`; floating keeps one instead of its own
/// duplicated `volume_flash`/`volume_dirty`/`flush_volume`. `now` is injected so
/// the debounce is unit-testable without a clock.
pub(crate) struct AudioController {
    ui: AudioUi,
    config_path: std::path::PathBuf,
    /// A volume nudge awaits its debounced `save_audio_volume`.
    volume_dirty: bool,
    /// When the transient `♩ N%` readout was armed (volume nudges only). Doubles
    /// as the debounce clock: the volume save lands once this window elapses.
    flash_at: Option<std::time::Instant>,
}

impl AudioController {
    pub(crate) fn new(ui: AudioUi, config_path: std::path::PathBuf) -> Self {
        Self {
            ui,
            config_path,
            volume_dirty: false,
            flash_at: None,
        }
    }

    /// Run one gesture: the shared transition, then persist — mute NOW, volume
    /// debounced (dirty + readout armed). A lazy spawn may mint a new handle;
    /// read it back via [`Self::handle`].
    pub(crate) fn apply(
        &mut self,
        action: AudioAction,
        paused: bool,
        now: std::time::Instant,
        spawn: impl FnOnce(f32) -> AudioHandle,
    ) {
        let persist = apply_audio_action(&mut self.ui, action, paused, spawn);
        if persist.muted {
            // persist like a theme commit: next launch boots as the user left it
            if let Err(e) = crate::config::save_audio_muted(&self.config_path, self.ui.muted) {
                tracing::warn!("failed to persist audio mute: {e}");
            }
        }
        if persist.volume_nudged {
            self.volume_dirty = true;
            self.flash_at = Some(now);
        }
    }

    /// The transient readout window is still fresh.
    fn flashing(&self, now: std::time::Instant) -> bool {
        self.flash_at
            .is_some_and(|t| now.duration_since(t).as_millis() < VOLUME_FLASH_MS)
    }

    /// The `♩ N%` volume readout, `Some` iff the window is fresh (volume nudges
    /// only — mute state is shown by the persistent footer indicator, not this).
    pub(crate) fn volume_flash(&self, now: std::time::Instant) -> Option<u8> {
        self.flashing(now)
            .then(|| (self.ui.volume * 100.0).round() as u8)
    }

    /// Per frame: flush the debounced volume save once the readout window has
    /// elapsed.
    pub(crate) fn tick(&mut self, now: std::time::Instant) {
        if self.volume_dirty && !self.flashing(now) {
            self.save_volume();
        }
    }

    /// Flush any pending volume on shutdown (a nudge-then-quit).
    pub(crate) fn flush_on_exit(&mut self) {
        if self.volume_dirty {
            self.save_volume();
        }
    }

    fn save_volume(&mut self) {
        self.volume_dirty = false;
        if let Err(e) = crate::config::save_audio_volume(&self.config_path, self.ui.volume) {
            tracing::warn!("failed to persist audio volume: {e}");
        }
    }

    /// Re-apply the effective mute when the external pause toggles (a frozen
    /// office must not keep clacking; unpause restores the user's own m-state).
    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.ui.handle.set_muted(paused || self.ui.muted);
    }

    pub(crate) fn muted(&self) -> bool {
        self.ui.muted
    }

    pub(crate) fn volume(&self) -> f32 {
        self.ui.volume
    }

    /// The live audio handle — the renderer/window feeds frames to it. Re-sync
    /// the consumer after [`Self::apply`], which may have lazily spawned a new one.
    pub(crate) fn handle(&self) -> &AudioHandle {
        &self.ui.handle
    }
}

#[cfg(test)]
mod controller_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ctl(muted: bool, volume: f32) -> (AudioController, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\n").unwrap();
        let ui = AudioUi {
            handle: AudioHandle::disabled(),
            muted,
            volume,
        };
        (AudioController::new(ui, path), dir)
    }

    #[test]
    fn mute_persists_immediately_and_does_not_arm_the_volume_flash() {
        let (mut c, _d) = ctl(false, 0.4);
        let t0 = Instant::now();
        c.apply(AudioAction::ToggleMute, false, t0, |_| {
            AudioHandle::disabled()
        });
        assert!(
            std::fs::read_to_string(&c.config_path)
                .unwrap()
                .contains("muted = true"),
            "mute toggled on AND persists NOW (like a theme commit)"
        );
        assert_eq!(c.volume_flash(t0), None, "mute does not flash ♩ N%");
    }

    #[test]
    fn volume_flashes_now_and_debounces_the_save_until_the_window_elapses() {
        let (mut c, _d) = ctl(false, 0.50);
        let t0 = Instant::now();
        let saved = |c: &AudioController| std::fs::read_to_string(&c.config_path).unwrap();
        c.apply(AudioAction::Volume(true), false, t0, |_| {
            AudioHandle::disabled()
        });
        assert_eq!(c.volume_flash(t0), Some(55), "readout armed immediately");
        assert!(
            !saved(&c).contains("volume"),
            "volume NOT persisted mid-flash (debounced, not per-repeat)"
        );
        c.tick(t0 + Duration::from_millis(500));
        assert!(
            !saved(&c).contains("volume"),
            "still within the window → no flush"
        );
        let after = t0 + Duration::from_millis(VOLUME_FLASH_MS as u64 + 50);
        c.tick(after);
        assert!(
            saved(&c).contains("volume"),
            "window elapsed → debounced save flushes"
        );
        assert_eq!(c.volume_flash(after), None, "readout expired");
    }

    #[test]
    fn flush_on_exit_writes_a_pending_nudge() {
        let (mut c, _d) = ctl(false, 0.50);
        c.apply(AudioAction::Volume(false), false, Instant::now(), |_| {
            AudioHandle::disabled()
        });
        c.flush_on_exit();
        assert!(
            std::fs::read_to_string(&c.config_path)
                .unwrap()
                .contains("volume"),
            "a nudge-then-quit persists on exit"
        );
    }
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

/// After the first-frame `TrackBeds::build` (~2s release / >10s debug) the
/// channel holds a backlog. Adopt its freshest LEVELS (they re-send every
/// render frame) but drop its event backlog — a replayed stack of chimes is a
/// clank pile — while KEEPING the first frame's own events, which haven't
/// played yet. The scheduler re-anchor the old `resync_after_stall` also did is
/// now inherent: the engine owns the (clamped) clock, so the build can't burst it.
#[cfg(feature = "audio")]
fn merge_backlog_levels(rx: &mpsc::Receiver<AudioFrame>, mut first: AudioFrame) -> AudioFrame {
    while let Ok(f) = rx.try_recv() {
        first.stems = f.stems;
    }
    first
}

/// The per-tick dt, CLAMPED to `MAX_DT_S` — the shell's half of the engine's
/// gap-immunity (the wasm painter clamps its `now_ms` delta the same way). A
/// ~2s track-build stall or a scheduler-starvation gap can't cover seconds and
/// snap the crossfade (the "bot HIGH" pop) or burst the schedulers; this is
/// what REPLACED `resync_after_stall`'s clock re-anchor. Pure so the clamp has
/// teeth without a device or thread.
#[cfg(feature = "audio")]
fn clamped_dt(prev: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(prev)
        .as_secs_f32()
        .min(MAX_DT_S)
}

/// The audio thread body — the DEVICE shell over the shared [`AudioEngine`]:
/// the clamped clock, the mute/volume atomics, the caller-side bed BUILD, and
/// forwarding each tick's `TickCommands` to the [`AudioSink`] (the test probe
/// drives the SAME loop). All mixing/crossfade/scheduling lives in the engine.
#[cfg(feature = "audio")]
fn run_loop(
    rx: mpsc::Receiver<AudioFrame>,
    mut device: Box<dyn AudioSink>,
    muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    volume: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    use std::sync::atomic::Ordering::Relaxed;

    // the ~2s (release; >10s debug) synthesis window: frames try_sent
    // meanwhile drop harmlessly (levels are re-sent every render frame),
    // and mute rides the atomic so a keypress here can never be lost
    let built_at = Instant::now();
    let mut rng = dsp::NoiseStream::new(BUILD_SEED);
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

    let mut engine = AudioEngine::new(f32::from_bits(volume.load(Relaxed)));
    let mut inited = false;
    let mut last_step = Instant::now();

    loop {
        let msg = rx.recv_timeout(std::time::Duration::from_millis(TICK_MS));
        engine.set_muted(muted.load(Relaxed));
        engine.set_master(f32::from_bits(volume.load(Relaxed)));

        // dt is CLAMPED (like the wasm shell): a build stall or scheduler
        // hiccup can neither snap the crossfade nor burst the schedulers, so
        // the old per-build clock re-anchor is no longer needed.
        let now = Instant::now();
        let dt = clamped_dt(last_step, now);
        last_step = now;

        let frame = match msg {
            Ok(frame) => {
                if !inited {
                    // First frame: build + register the RIGHT mood's beds, then
                    // init the engine's switch. The ~2s synth stalls the thread;
                    // drop the backlog it queued (keep the freshest levels) and
                    // re-anchor the clock so the build's seconds ramp nothing.
                    let beds = TrackBeds::build(&mut rng, frame.track);
                    for stem in TRACK_STEMS {
                        device.start_loop(stem, beds.bed(stem));
                    }
                    engine.init_track(frame.track);
                    inited = true;
                    let fresh = merge_backlog_levels(&rx, frame);
                    last_step = Instant::now();
                    Some(fresh)
                } else {
                    Some(frame)
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };

        let cmds = engine.tick(dt, frame);

        for (stem, gain) in LoopStem::ALL.into_iter().zip(cmds.gains) {
            device.set_loop_gain(stem, gain);
        }
        // A committed mood switch: build + swap the five track beds under the
        // silence, then drain the backlog + re-anchor (as on the first build).
        if let Some(to) = cmds.swap {
            let beds = TrackBeds::build(&mut rng, to);
            for stem in TRACK_STEMS {
                device.swap_loop(stem, beds.bed(stem));
            }
            for _ in rx.try_iter() {}
            last_step = Instant::now();
        }
        for play in cmds.plays {
            device.play_once(bank.sample(play.pool, play.index), play.gain);
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
        // Bed IDENTITY (each stem got the RIGHT bed, not just A bed) is pinned
        // by the fast, device-free `track_beds_sit_in_the_ratified_centroid_order`
        // in `pixtuoid_scene::audio::bank`; the per-tick mixing / crossfade /
        // scheduling correctness by the `AudioEngine` value tests. This smoke
        // proves only the run_loop WIRING: registration, one-shots, mute, exit.
    }

    #[test]
    fn clamped_dt_caps_a_build_stall_gap_but_passes_a_normal_tick() {
        // The native shell's gap-immunity (what replaced `resync_after_stall`'s
        // clock re-anchor): a ~2s track-build stall clamps to the ceiling so the
        // next `mixer.step` can't snap the crossfade (the "bot HIGH" pop). If the
        // `.min(MAX_DT_S)` is ever dropped, this fails; the engine's
        // `the_dt_clamp_ceiling_stays_a_slew_*` test proves the ceiling VALUE is
        // small enough to keep that clamped step a slew.
        let t0 = Instant::now();
        assert_eq!(
            clamped_dt(t0, t0 + std::time::Duration::from_secs(2)),
            MAX_DT_S,
            "a multi-second build stall clamps to the ceiling"
        );
        let dt = clamped_dt(t0, t0 + std::time::Duration::from_millis(20));
        assert!(
            (dt - 0.020).abs() < 1e-4,
            "a normal tick passes through: {dt}"
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
        track: TrackId,
        stems: StemLevels,
        events_at: &[(f32, OneShot)],
        secs: f32,
    ) -> Vec<f32> {
        let mut sink = OfflineSink::new(secs);
        sink.start_loop(LoopStem::Rain, Arc::clone(rain));
        for stem in TRACK_STEMS {
            sink.start_loop(stem, beds.bed(stem));
        }
        // Drive the SAME shared `AudioEngine` the app runs, so the audition
        // mixes exactly what ships (incl. the production bus trim on the
        // one-shots — the old hand-rolled loop played them untrimmed).
        let mut engine = AudioEngine::new(1.0);
        engine.init_track(track);
        let step_s = 0.05f32;
        let step_n = (step_s * dsp::SAMPLE_RATE as f32) as usize;
        let mut fired = vec![false; events_at.len()];
        let mut now_s = 0.0f64;
        while now_s < secs as f64 {
            let mut events = Vec::new();
            for (i, (at, ev)) in events_at.iter().enumerate() {
                if !fired[i] && now_s >= *at as f64 {
                    fired[i] = true;
                    events.push(*ev);
                }
            }
            let cmds = engine.tick(
                step_s,
                Some(AudioFrame {
                    stems,
                    events,
                    track,
                }),
            );
            for (stem, gain) in LoopStem::ALL.into_iter().zip(cmds.gains) {
                sink.set_loop_gain(stem, gain);
            }
            for play in cmds.plays {
                sink.play_once(bank.sample(play.pool, play.index), play.gain);
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
        let mut rng = dsp::NoiseStream::new(BUILD_SEED);
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
            let buf = render_tier(&bank, &beds, &rain, TrackId::Day, stems, events, 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: every tier is audible in Phase 2"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        // the NIGHT track (#644): the runtime approximation of the v4 take
        // (no bus glue — rodio has no insert; the owner re-verifies by ear)
        for (name, stems) in [("night_moderate", moderate), ("night_rainy", rainy)] {
            let buf = render_tier(&bank, &night, &rain, TrackId::Night, stems, &[], 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: the night track is audible"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        println!("LISTEN GATE wavs at: {}", out.display());
    }
}
