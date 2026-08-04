//! Ambient office audio — the only owner of any audio-device dependency, over
//! the pure synthesis in `pixtuoid_scene::audio`. Every sample buffer is
//! pre-rendered at startup and ALL-PROCEDURAL (no committed assets, no decoder
//! dep). Playback rides its own thread behind a bounded channel — the render
//! loop only ever `try_send`s (drop-on-backpressure, never blocks).

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
#[cfg(all(feature = "audio", test))]
use pixtuoid_scene::audio::{OneShot, TrackId};
#[cfg(feature = "audio")]
use sink::AudioSink;

#[cfg(feature = "audio")]
use pixtuoid_scene::audio::bank::{AssetBank, TrackBeds, TRACK_STEMS};

/// The +/- keys' volume increment — one definition for BOTH painters, which are
/// siblings that must not import from each other.
pub(crate) const VOLUME_STEP: f32 = 0.05;
/// How long the transient volume readout stays up after a nudge — also the
/// volume-persist debounce window on both painters.
pub(crate) const VOLUME_FLASH_MS: u128 = 1000;

/// The two audio gestures both painters drive; the KEY→action map is
/// painter-specific, the state transition ([`apply_audio_action`]) is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioAction {
    ToggleMute,
    /// `true` = volume up.
    Volume(bool),
}

pub(crate) struct AudioUi {
    pub(crate) handle: AudioHandle,
    pub(crate) muted: bool,
    pub(crate) volume: f32,
}

/// What the caller persists after [`apply_audio_action`] — the side effects stay
/// painter-side so the transition itself is pure.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Persist {
    /// The mute flag CHANGED — persist now.
    pub(crate) muted: bool,
    /// The volume changed — flash the readout and persist DEBOUNCED (the +/-
    /// keys autorepeat).
    pub(crate) volume_nudged: bool,
}

/// THE audio mute/volume transition — the single authority both painters run.
/// Semantics: mute toggles; volume-up from muted IS the un-mute gesture; the
/// lazy spawn (re)fires whenever sound is wanted but the system is down, so
/// `+`/`m` are never dead keys (boot-muted and failed-spawn both recover).
/// `paused` folds an external hold (the TUI's `[p]ause`) into the effective
/// mute; floating passes `false`.
pub(crate) fn apply_audio_action(
    st: &mut AudioUi,
    action: AudioAction,
    paused: bool,
    respawn: impl FnOnce(&AudioHandle, f32),
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
                st.muted = false;
                persist.muted = true;
            }
            persist.volume_nudged = true;
        }
    }
    if !st.muted && !st.handle.is_enabled() {
        respawn(&st.handle, st.volume);
    }
    st.handle.set_muted(paused || st.muted);
    st.handle.set_volume(st.volume);
    persist
}

/// The mute/volume PERSIST protocol both painters share: the pure
/// [`apply_audio_action`] transition plus its side effects — mute saves NOW, a
/// volume nudge arms the `♩ N%` readout and a save debounced behind it (a held
/// `+`/`-` writes once, not per repeat), flushed on shutdown.
pub(crate) struct AudioController {
    ui: AudioUi,
    config_path: std::path::PathBuf,
    volume_dirty: bool,
    /// When the `♩ N%` readout was armed. Doubles as the debounce clock: the
    /// volume save lands once this window elapses.
    flash_at: Option<std::time::Instant>,
}

impl AudioController {
    /// Construct the controller AND own the device thread's whole lifecycle:
    /// boot-spawn here (iff a persisted unmute wants sound), tear down in `Drop`.
    /// Each painter builds it AFTER its fallible boot steps, so no device thread
    /// can exist before its Drop-owner and `Drop` alone covers EVERY exit path.
    pub(crate) fn new(muted: bool, volume: f32, config_path: std::path::PathBuf) -> Self {
        Self::new_with(muted, volume, config_path, respawn)
    }

    /// [`new`] with the boot-spawn injected, so a test can pin the boot decision
    /// without opening an output device. `pub(crate)` for the same reason it
    /// exists: `tui`'s key-action tests need an UNMUTED controller, and an
    /// unmuted `new` boot-spawns the real device.
    pub(crate) fn new_with(
        muted: bool,
        volume: f32,
        config_path: std::path::PathBuf,
        respawn: impl FnOnce(&AudioHandle, f32),
    ) -> Self {
        let handle = AudioHandle::disabled();
        if !muted {
            respawn(&handle, volume);
        }
        Self {
            ui: AudioUi {
                handle,
                muted,
                volume,
            },
            config_path,
            volume_dirty: false,
            flash_at: None,
        }
    }

    /// Run one gesture: the shared transition, then persist — mute NOW, volume debounced.
    pub(crate) fn apply(
        &mut self,
        action: AudioAction,
        paused: bool,
        now: std::time::Instant,
        respawn: impl FnOnce(&AudioHandle, f32),
    ) {
        let persist = apply_audio_action(&mut self.ui, action, paused, respawn);
        if persist.muted {
            if let Err(e) = crate::config::save_audio_muted(&self.config_path, self.ui.muted) {
                tracing::warn!("failed to persist audio mute: {e}");
            }
        }
        if persist.volume_nudged {
            self.volume_dirty = true;
            self.flash_at = Some(now);
        }
    }

    fn flashing(&self, now: std::time::Instant) -> bool {
        self.flash_at
            .is_some_and(|t| now.duration_since(t).as_millis() < VOLUME_FLASH_MS)
    }

    /// The `♩ N%` volume readout, `Some` iff the flash window is still fresh.
    pub(crate) fn volume_flash(&self, now: std::time::Instant) -> Option<u8> {
        self.flashing(now)
            .then(|| (self.ui.volume * 100.0).round() as u8)
    }

    /// Per frame: flush the debounced volume save once its window has elapsed.
    pub(crate) fn tick(&mut self, now: std::time::Instant) {
        if self.volume_dirty && !self.flashing(now) {
            self.save_volume();
        }
    }

    fn flush_on_exit(&mut self) {
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

    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.ui.handle.set_muted(paused || self.ui.muted);
    }

    /// The live audio handle — stable across a lazy respawn (the sender is
    /// swapped in place), so a consumer's cached clone never goes stale.
    pub(crate) fn handle(&self) -> &AudioHandle {
        &self.ui.handle
    }
}

/// RAII teardown — the ONE exit verb for BOTH halves of the audio protocol:
/// PERSIST a pending debounced volume, THEN stop the device thread. Both run
/// unconditionally, so a Ctrl-C / terminate / error can't lose a nudge that
/// landed inside the debounce window. Both halves are panic-free (save + join
/// log, never unwrap), so this is safe to run during unwind.
impl Drop for AudioController {
    fn drop(&mut self) {
        self.flush_on_exit();
        self.ui.handle.shutdown();
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
        // A no-op boot-spawn: the fixture must never open a real device.
        let c = AudioController::new_with(muted, volume, path, |_, _| {});
        (c, dir)
    }

    #[test]
    fn new_boot_spawns_only_when_unmuted_and_drop_joins_the_device_thread() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A MEASURABLE teardown is what makes the join assert a deterministic
        // red: without the join, Drop returns while the fake device thread is
        // still sleeping → `done` is false. A zero-cost teardown would pass
        // even unfixed.
        const TEARDOWN_MS: u64 = 300;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\n").unwrap();

        let muted_spawned = std::cell::Cell::new(false);
        let c = AudioController::new_with(true, 0.4, path.clone(), |_, _| muted_spawned.set(true));
        assert!(!muted_spawned.get(), "a muted boot spawns no device thread");
        assert!(!c.handle().is_enabled());
        drop(c);

        let done = std::sync::Arc::new(AtomicBool::new(false));
        let got_vol = std::cell::Cell::new(0.0f32);
        let c = AudioController::new_with(false, 0.6, path, |h, v| {
            got_vol.set(v);
            let rx = h.install_test_channel();
            let flag = std::sync::Arc::clone(&done);
            let thread = std::thread::spawn(move || {
                while rx.recv().is_ok() {}
                std::thread::sleep(std::time::Duration::from_millis(TEARDOWN_MS));
                flag.store(true, Ordering::SeqCst);
            });
            *h.join.lock().unwrap() = Some(thread);
        });
        assert_eq!(
            got_vol.get(),
            0.6,
            "an unmuted boot spawns at the kept volume"
        );
        assert!(c.handle().is_enabled());

        drop(c);
        assert!(
            done.load(Ordering::SeqCst),
            "dropping the controller must JOIN the boot-spawned device thread so \
             its teardown completes — the RAII teardown-on-quit guarantee"
        );
    }

    #[test]
    fn mute_persists_immediately_and_does_not_arm_the_volume_flash() {
        let (mut c, _d) = ctl(false, 0.4);
        let t0 = Instant::now();
        c.apply(AudioAction::ToggleMute, false, t0, |_, _| {});
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
        c.apply(AudioAction::Volume(true), false, t0, |_, _| {});
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
        c.apply(AudioAction::Volume(false), false, Instant::now(), |_, _| {});
        c.flush_on_exit();
        assert!(
            std::fs::read_to_string(&c.config_path)
                .unwrap()
                .contains("volume"),
            "a nudge-then-quit persists on exit"
        );
    }

    #[test]
    fn drop_persists_a_pending_nudge_even_without_the_q_path() {
        let (mut c, _dir) = ctl(false, 0.50);
        let path = c.config_path.clone();
        c.apply(AudioAction::Volume(false), false, Instant::now(), |_, _| {});
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("volume"),
            "not yet persisted — still inside the debounce window"
        );
        drop(c); // the ONLY exit signal: no `q`, no explicit flush_on_exit()
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("volume"),
            "AudioController::drop must persist a pending nudge (the #752 Ctrl-C fix)"
        );
    }

    #[test]
    fn a_clean_drop_does_not_rewrite_the_config() {
        let (c, _dir) = ctl(false, 0.50);
        let path = c.config_path.clone();
        let before = std::fs::read_to_string(&path).unwrap();
        drop(c);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "an un-dirtied drop must not touch the user's config"
        );
    }
}

#[cfg(test)]
mod controls_tests {
    use super::*;

    #[test]
    fn shutdown_joins_the_device_thread_so_its_teardown_runs_before_return() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A fake device thread that mirrors run_loop (block on the channel, exit
        // on disconnect) but takes a MEASURABLE time to finish. That delay is
        // what makes this a deterministic red: a zero-cost teardown would let an
        // un-joined thread win the race by luck and pass even unfixed.
        const TEARDOWN_MS: u64 = 300;

        let handle = AudioHandle::disabled();
        let rx = handle.install_test_channel();
        let done = std::sync::Arc::new(AtomicBool::new(false));

        let flag = std::sync::Arc::clone(&done);
        let thread = std::thread::spawn(move || {
            while rx.recv().is_ok() {}
            std::thread::sleep(std::time::Duration::from_millis(TEARDOWN_MS));
            flag.store(true, Ordering::SeqCst);
        });
        *handle.join.lock().unwrap() = Some(thread);

        let t0 = std::time::Instant::now();
        handle.shutdown();

        assert!(
            done.load(Ordering::SeqCst),
            "shutdown() must JOIN the device thread so its teardown (the RodioSink \
             Drop that closes the OS device) completes before it returns — without \
             the join it returns mid-teardown and the OS output is stranded"
        );
        assert!(
            t0.elapsed() >= std::time::Duration::from_millis(TEARDOWN_MS),
            "shutdown() returned before the thread's teardown could finish — it did \
             not actually wait"
        );
        assert!(
            !handle.is_enabled(),
            "shutdown() drops the sole sender — the handle is inert afterwards"
        );
    }

    #[test]
    fn unmute_lazy_spawns_and_mute_back_does_not() {
        let mut st = AudioUi {
            handle: AudioHandle::disabled(),
            muted: true,
            volume: 0.4,
        };
        let mut spawned_at = None;
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, false, |h, v| {
            spawned_at = Some(v);
            h.install_test_channel();
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
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, false, |_, _| {
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
        let mut st = AudioUi {
            handle: AudioHandle::disabled(),
            muted: true,
            volume: 0.5,
        };
        let mut spawns = 0;
        let p = apply_audio_action(&mut st, AudioAction::Volume(true), false, |h, _| {
            spawns += 1;
            h.install_test_channel();
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
        let p = apply_audio_action(&mut st, AudioAction::Volume(false), false, |_, _| {
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
            |_, _| unreachable!(),
        );
        assert_eq!(st.volume, 1.0, "top rail");
        st.volume = 0.0;
        apply_audio_action(
            &mut st,
            AudioAction::Volume(false),
            false,
            |_, _| unreachable!(),
        );
        assert_eq!(st.volume, 0.0, "bottom rail");
    }

    #[test]
    fn paused_forces_effective_mute_without_touching_the_flag() {
        let (live, _rx) = AudioHandle::test_pair();
        let mut st = AudioUi {
            handle: live,
            muted: false,
            volume: 0.5,
        };
        apply_audio_action(
            &mut st,
            AudioAction::Volume(true),
            true,
            |_, _| unreachable!(),
        );
        assert!(!st.muted, "the user's flag stays unmuted");
        assert!(st.handle.is_muted(), "but paused silences the handle");
    }

    #[test]
    fn a_consumer_clone_survives_a_lazy_respawn_in_place() {
        let handle = AudioHandle::disabled();
        let cached = handle.clone(); // what a renderer caches once, at init
        assert!(!cached.is_enabled());
        let rx = handle.install_test_channel(); // a lazy respawn fills the shared tx
        assert!(
            cached.is_enabled(),
            "the pre-respawn clone must see the swapped-in channel"
        );
        cached.frame(AudioFrame::default());
        assert_eq!(
            drain_frames(&rx).len(),
            1,
            "frames from the pre-respawn clone reach the live channel"
        );
    }
}

/// The painters' handle — clone-cheap, non-blocking. A disabled handle
/// (audio off in config, or no device) swallows everything.
#[derive(Clone)]
pub(crate) struct AudioHandle {
    /// The live device sender, swappable IN PLACE behind a shared cell: every
    /// clone shares this `Arc`, so a consumer's cached clone survives a lazy
    /// respawn. `None` = disabled (no device, or sound not requested yet).
    tx: std::sync::Arc<std::sync::Mutex<Option<mpsc::SyncSender<AudioFrame>>>>,
    /// Mute is STATE, not an event: it rides this atomic instead of the
    /// droppable frame channel. During the bank-synthesis window the channel
    /// saturates and try_sends drop — an `m`/`p` keypress there must still
    /// land, or the beds fade in unmuted against a footer that says muted.
    muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Master volume (f32 bits) — same state-not-event rationale as `muted`.
    volume: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// The device thread's join handle, so [`shutdown`](Self::shutdown) can WAIT
    /// for `run_loop` to drop its `RodioSink` (the OS device close) before the
    /// process exits. Without it the thread is detached and its teardown races
    /// exit — on macOS CoreAudio the loser strands the output (audio keeps
    /// playing; `sudo killall coreaudiod` to recover).
    join: std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl AudioHandle {
    /// The inert handle: sound not requested yet, or no usable output device.
    pub(crate) fn disabled() -> Self {
        Self {
            tx: std::sync::Arc::new(std::sync::Mutex::new(None)),
            muted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            volume: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
            join: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.tx.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// Push one frame of audio intent. `try_send` — a saturated audio thread
    /// drops frames rather than ever stalling the render loop.
    pub(crate) fn frame(&self, frame: AudioFrame) {
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.try_send(frame);
        }
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        self.muted
            .store(muted, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        self.volume.store(
            volume.clamp(0.0, 1.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The EFFECTIVE silence state — the m-toggle OR'd with pause, since the
    /// caller stores the combined value here.
    pub(crate) fn is_muted(&self) -> bool {
        self.muted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The ONE audibility predicate both painters' footer ♩ indicators read.
    pub(crate) fn is_audible(&self) -> bool {
        self.is_enabled() && !self.is_muted() && self.volume() > 0.0
    }

    /// (Re)open the output device + audio thread and swap the live sender INTO
    /// this handle in place, so every consumer that cached a clone keeps
    /// working. A no-device / feature-off system leaves the handle disabled.
    pub(crate) fn respawn_in_place(&self, volume: f32) {
        self.set_volume(volume);
        #[cfg(feature = "audio")]
        {
            let Some(device) = sink::rodio_sink::RodioSink::open() else {
                return;
            };
            let (tx, rx) = mpsc::sync_channel(64);
            let muted_for_loop = std::sync::Arc::clone(&self.muted);
            let vol_for_loop = std::sync::Arc::clone(&self.volume);
            match std::thread::Builder::new()
                .name("pixtuoid-audio".into())
                .spawn(move || run_loop(rx, Box::new(device), muted_for_loop, vol_for_loop))
            {
                Ok(join) => {
                    // Replacing the sole sender CLOSES any prior thread's
                    // channel, so retire that thread rather than leak a device
                    // thread still holding the output. Both locks are dropped
                    // before the join, so the keypress path never blocks
                    // holding one.
                    *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
                    let prior = self
                        .join
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .replace(join);
                    if let Some(prior) = prior {
                        join_with_timeout(prior, SHUTDOWN_JOIN_TIMEOUT);
                    }
                }
                Err(e) => tracing::warn!("audio: thread spawn failed, running silent: {e}"),
            }
        }
    }

    /// Stop the device thread SYNCHRONOUSLY: drop the sole sender so `run_loop`
    /// sees the channel close and returns, dropping its `RodioSink` (the OS
    /// device close), then JOIN so that Drop completes before the process exits.
    /// Without the join the thread is detached and its Drop races process
    /// teardown — on macOS CoreAudio a half-closed output strands playback
    /// (music keeps going; `sudo killall coreaudiod` to recover). Bounded so a
    /// pathological rodio/cpal Drop can't hang the exit.
    ///
    /// INVARIANT: this runs from `AudioController::drop`, only after the
    /// painter's loop has ended — so `shutdown` / `respawn_in_place` / `frame`
    /// never run concurrently on the shared `tx`/`join` cells. Idempotent
    /// (`take()`-based) regardless.
    pub(crate) fn shutdown(&self) {
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let handle = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            join_with_timeout(handle, SHUTDOWN_JOIN_TIMEOUT);
        }
    }

    /// Test seam: a live handle whose receiver the test drains — the one way to
    /// see what the render path feeds the audio thread.
    #[cfg(test)]
    pub(crate) fn test_pair() -> (Self, mpsc::Receiver<AudioFrame>) {
        let (tx, rx) = mpsc::sync_channel(256);
        (
            Self {
                tx: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
                muted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                volume: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
                join: std::sync::Arc::new(std::sync::Mutex::new(None)),
            },
            rx,
        )
    }

    /// Test seam: fill the shared sender in place, as a lazy respawn would.
    #[cfg(test)]
    pub(crate) fn install_test_channel(&self) -> mpsc::Receiver<AudioFrame> {
        let (tx, rx) = mpsc::sync_channel(256);
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        rx
    }
}

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

/// Upper bound on how long [`AudioHandle::shutdown`] waits for the device
/// thread's teardown. The common case returns within one `TICK_MS`, but
/// `run_loop` is BLIND to the closed channel while inside a multi-second
/// synthesis build, so a quit landing in that window must wait the build out:
/// a ceiling under a release build's worst case times the join out and leaves
/// the device thread DETACHED — the very failure the join exists to prevent. A
/// debug build's longer synth can still exceed this; accepted, debug isn't
/// shipped.
const SHUTDOWN_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Join `handle`, but give up after `timeout` so a hung device-close (or a
/// still-in-flight synth build, see [`SHUTDOWN_JOIN_TIMEOUT`]) can't block the
/// exit forever. On timeout the helper thread is left detached, still blocked on
/// the real join — no worse than the always-detached behaviour this replaces.
/// std has no timed `JoinHandle::join`, hence the channel dance.
fn join_with_timeout(handle: std::thread::JoinHandle<()>, timeout: std::time::Duration) {
    let (done_tx, done_rx) = mpsc::channel();
    // `Builder::spawn` (not `thread::spawn`) so an OS thread-exhaustion failure
    // returns `Err` instead of PANICKING: this runs from `AudioController::drop`,
    // which can execute during unwind, where a panic would double-panic → abort.
    let spawned = std::thread::Builder::new().spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    if spawned.is_ok() {
        let _ = done_rx.recv_timeout(timeout);
    }
}

/// The production lazy-respawn injected into [`apply_audio_action`] /
/// [`AudioController::apply`] — a named fn so the two callers can't drift.
pub(crate) fn respawn(handle: &AudioHandle, volume: f32) {
    handle.respawn_in_place(volume);
}

/// After the first-frame `TrackBeds::build` the channel holds a backlog. Adopt
/// its freshest LEVELS (they re-send every render frame) but drop its event
/// backlog — a replayed stack of chimes is a clank pile — while KEEPING the
/// first frame's own events, which haven't played yet.
#[cfg(feature = "audio")]
fn merge_backlog_levels(rx: &mpsc::Receiver<AudioFrame>, mut first: AudioFrame) -> AudioFrame {
    while let Ok(f) = rx.try_recv() {
        first.stems = f.stems;
    }
    first
}

/// The per-tick dt, CLAMPED to `MAX_DT_S` — the shell's half of the engine's
/// gap-immunity (the wasm painter clamps its `now_ms` delta the same way). A
/// track-build stall or a scheduler-starvation gap must not cover seconds and
/// snap the crossfade or burst the schedulers.
#[cfg(feature = "audio")]
fn clamped_dt(prev: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(prev)
        .as_secs_f32()
        .min(MAX_DT_S)
}

/// The audio thread body — the DEVICE shell over the shared [`AudioEngine`],
/// which owns all mixing/crossfade/scheduling.
#[cfg(feature = "audio")]
fn run_loop(
    rx: mpsc::Receiver<AudioFrame>,
    mut device: Box<dyn AudioSink>,
    muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    volume: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    use std::sync::atomic::Ordering::Relaxed;

    // The synthesis window: frames try_sent meanwhile drop harmlessly (levels
    // are re-sent every render frame), and mute rides the atomic so a keypress
    // landing here can never be lost.
    let built_at = Instant::now();
    let mut rng = dsp::NoiseStream::new(BUILD_SEED);
    let bank = AssetBank::build(&mut rng);
    // Rain is weather — track-independent, registered once. The TRACK beds wait
    // for the FIRST frame, which names the right mood for the office's current
    // hour/weather (booting Day at night would synthesize a track just to
    // crossfade it away).
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

        let now = Instant::now();
        let dt = clamped_dt(last_step, now);
        last_step = now;

        let frame = match msg {
            Ok(frame) => {
                if !inited {
                    // The first frame's synth stalls the thread: drop the
                    // backlog it queued and re-anchor the clock, so the build's
                    // seconds ramp nothing.
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
        h.set_muted(true);
    }

    #[test]
    fn run_loop_registers_beds_plays_events_and_exits_on_disconnect() {
        let (tx, rx) = mpsc::sync_channel(8);
        // The recorder rides a `(Mutex, Condvar)` pair so the frame-1 barrier
        // below BLOCKS on the sink's own progress instead of polling a wall
        // clock: the wait is machine-speed-bound (the synth bank build
        // dominates it), and an in-test deadline is the one flakiness knob
        // `.config/nextest.toml` is structurally powerless over.
        let recorder = Arc::new((
            std::sync::Mutex::new(sink::NullSink::default()),
            std::sync::Condvar::new(),
        ));
        // `.1` flips when the device run_loop owns is DROPPED. The recorder is a
        // SEPARATE shared handle that outlives the thread, so it cannot observe
        // that drop — this flag can.
        struct Probe(
            Arc<(std::sync::Mutex<sink::NullSink>, std::sync::Condvar)>,
            Arc<std::sync::atomic::AtomicBool>,
        );
        impl Probe {
            fn record(&self, f: impl FnOnce(&mut sink::NullSink)) {
                let (lock, progress) = &*self.0;
                f(&mut lock.lock().unwrap());
                progress.notify_all();
            }
        }
        impl AudioSink for Probe {
            fn start_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.record(|r| r.start_loop(stem, s));
            }
            fn swap_loop(&mut self, stem: LoopStem, s: Arc<Vec<f32>>) {
                self.record(|r| r.swap_loop(stem, s));
            }
            fn set_loop_gain(&mut self, stem: LoopStem, g: f32) {
                self.record(|r| r.set_loop_gain(stem, g));
            }
            fn play_once(&mut self, s: Arc<Vec<f32>>, g: f32) {
                self.record(|r| r.play_once(s, g));
            }
        }
        impl Drop for Probe {
            fn drop(&mut self) {
                self.1.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let device_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = Probe(Arc::clone(&recorder), Arc::clone(&device_dropped));
        let muted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let muted_ctl = std::sync::Arc::clone(&muted);
        let vol = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits()));
        let join = std::thread::spawn(move || run_loop(rx, Box::new(probe), muted, vol));

        // Rain stays 0 so no scheduler one-shot can race the count — only the
        // two frame events are audible.
        tx.send(AudioFrame {
            stems: StemLevels::default(),
            events: vec![OneShot::DoorChime, OneShot::PrinterWhir],
            track: Default::default(),
        })
        .unwrap();
        // Wait until the loop has processed frame 1 (the bank build delays it by
        // seconds) so the mute below deterministically lands BETWEEN the frames,
        // not before both. UNBOUNDED on purpose — the only way this never wakes
        // is run_loop failing to play frame 1's one-shots at all, which nextest's
        // `terminate-after` reports as this named test hanging.
        {
            let (lock, progress) = &*recorder;
            let mut rec = lock.lock().unwrap();
            while rec.one_shots < 2 {
                rec = progress.wait(rec).unwrap();
            }
        }
        muted_ctl.store(true, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioFrame {
            stems: StemLevels::default(),
            events: vec![OneShot::DoorChime, OneShot::VendingDrop],
            track: Default::default(),
        })
        .unwrap();
        drop(tx);
        join.join().unwrap();

        assert!(
            device_dropped.load(std::sync::atomic::Ordering::SeqCst),
            "run_loop must DROP its device when the channel closes — that Drop is \
             the RodioSink closing the OS output; a refactor that leaked it would \
             re-strand audio on quit (the bug shutdown()'s join exists to force)"
        );

        let rec = recorder.0.lock().unwrap();
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
    }

    #[test]
    fn clamped_dt_caps_a_build_stall_gap_but_passes_a_normal_tick() {
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

/// The LISTEN gate: renders each busy-ness tier through the REAL
/// mixer/schedulers/synth into wav files for the owner's audition.
/// `#[ignore]` — run explicitly:
/// `cargo test -p pixtuoid --lib audio::listen_gate -- --ignored --nocapture`
#[cfg(all(test, feature = "audio"))]
mod listen_gate {
    use super::*;
    use pixtuoid_scene::audio::StemLevels;
    use std::io::Write;

    /// Sample-accurate mixdown of loops and one-shots into one master buffer.
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
        // mixes exactly what ships (incl. the production bus trim).
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
        let beds = TrackBeds::build(&mut rng, TrackId::GenDay(0));
        let night = TrackBeds::build(&mut rng, TrackId::GenNight(0));
        // Tier levels come from the PRODUCTION mapping, not hand-rolled
        // literals — the wavs must audition what the app will mix.
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
        let volley = [
            (5.0, OneShot::DoorChime),
            (10.0, OneShot::PrinterWhir),
            (15.0, OneShot::VendingDrop),
        ];
        for (name, stems, events) in [
            ("tier_1_empty", quiet, &[][..]),
            ("tier_2_moderate", moderate, &[][..]),
            ("tier_3_busy_oneshot_volley", busy, &volley[..]),
            ("tier_4_rainy_busy", rainy, &[][..]),
        ] {
            let buf = render_tier(&bank, &beds, &rain, TrackId::GenDay(0), stems, events, 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: every tier is audible in Phase 2"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        // The NIGHT track carries no bus glue — rodio has no insert, so the
        // owner re-verifies it by ear.
        for (name, stems) in [("night_moderate", moderate), ("night_rainy", rainy)] {
            let buf = render_tier(&bank, &night, &rain, TrackId::GenNight(0), stems, &[], 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: the night track is audible"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        println!("LISTEN GATE wavs at: {}", out.display());
    }
}
