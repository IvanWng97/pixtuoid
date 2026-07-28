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
/// `[p]ause`) into the effective mute; floating passes `false`. `respawn` is
/// injected so the transition is testable without a device.
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
                // volume-up IS the un-mute gesture too
                st.muted = false;
                persist.muted = true;
            }
            persist.volume_nudged = true;
        }
    }
    if !st.muted && !st.handle.is_enabled() {
        // lazy (re)spawn IN PLACE: muted costs nothing, so the device/thread/
        // buffers only come up when sound is wanted — and swapping the sender
        // into the SAME handle keeps every cached clone live (no re-sync).
        respawn(&st.handle, st.volume);
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
    /// Construct the controller AND own the device thread's whole lifecycle:
    /// boot-spawn here (iff a persisted unmute wants sound), tear down in `Drop`.
    /// Because the controller is built AFTER each painter's fallible boot steps
    /// (TUI after the pack load; floating after pack/runtime/event-loop build),
    /// no device thread ever exists before its Drop-owner — so `Drop` alone
    /// covers EVERY exit path (q / Ctrl-C / terminate / error / a boot `?`),
    /// with no manual shutdown wiring. `muted`/`volume` come pre-resolved from
    /// config; a muted boot stays at zero cost (no device/thread/buffers) until
    /// the first `m`/`+` lazy-respawns in place.
    pub(crate) fn new(muted: bool, volume: f32, config_path: std::path::PathBuf) -> Self {
        Self::new_with(muted, volume, config_path, respawn)
    }

    /// [`new`] with the boot-spawn INJECTED, mirroring [`apply`]'s `respawn`
    /// seam: production passes the real [`respawn`] free fn; a test passes a
    /// device-free closure to pin the boot decision (muted ⇒ no spawn) and that
    /// `Drop` joins a spawned thread — without opening an output device.
    fn new_with(
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

    /// Run one gesture: the shared transition, then persist — mute NOW, volume
    /// debounced (dirty + readout armed). A lazy respawn fills the live handle
    /// IN PLACE — consumers' cached clones stay valid, so no read-back.
    pub(crate) fn apply(
        &mut self,
        action: AudioAction,
        paused: bool,
        now: std::time::Instant,
        respawn: impl FnOnce(&AudioHandle, f32),
    ) {
        let persist = apply_audio_action(&mut self.ui, action, paused, respawn);
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

    /// Flush any pending debounced volume on exit (a nudge-then-quit). Called
    /// from `Drop` now — the ONE exit path — not per painter.
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

    /// Re-apply the effective mute when the external pause toggles (a frozen
    /// office must not keep clacking; unpause restores the user's own m-state).
    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.ui.handle.set_muted(paused || self.ui.muted);
    }

    /// The live audio handle — the renderer/window feeds frames to it. Stable
    /// across a lazy respawn (the sender is swapped in place), so a consumer's
    /// cached clone never goes stale — hand it out ONCE, no re-sync.
    pub(crate) fn handle(&self) -> &AudioHandle {
        &self.ui.handle
    }
}

/// RAII teardown — the ONE exit verb for BOTH halves of the audio protocol:
/// PERSIST a pending debounced volume, THEN stop the device thread. Each painter
/// holds exactly ONE controller (TUI a `run_tui` local; floating a `FloatingApp`
/// field), so this fires once on EVERY exit — the compiler guarantees it runs
/// where a hand-wired call could be forgotten on some `?`/early-return path.
/// (Release is `panic="abort"`, so a panic — a crash — is the one exit that
/// skips Drop; losing a sub-second unsaved volume nudge there is acceptable.)
///
/// Ordering is persist-before-stop, and both run unconditionally: before #752
/// the volume flush sat on the TUI `q` branch only, so Ctrl-C / terminate / error
/// lost a nudge that landed inside the debounce window — moving it here fixes that
/// (Drop already ran on every exit for the device stop). `flush_on_exit` is a
/// no-op unless a nudge is pending; both halves are panic-free (save + join log,
/// never unwrap), safe to run during unwind. `AudioHandle::shutdown` drops the
/// sole sender (closing the device thread's channel) and JOINS the thread so its
/// `RodioSink` Drop — the OS device close — completes before the process exits;
/// idempotent + a no-op for a muted session that never spawned. See the
/// `AudioHandle::shutdown` docs for why the join is load-bearing on macOS CoreAudio.
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
        // The REAL constructor with a no-op boot-spawn: these tests drive the
        // mute/volume/persist logic and inject their own respawn via `apply`, so
        // the boot must not open a device. The no-op leaves the handle disabled
        // (its Drop teardown is then a no-op) while still exercising `new_with`.
        let c = AudioController::new_with(muted, volume, path, |_, _| {});
        (c, dir)
    }

    #[test]
    fn new_boot_spawns_only_when_unmuted_and_drop_joins_the_device_thread() {
        // The two things the RAII refactor must guarantee, pinned device-free:
        // (1) a MUTED boot spawns nothing (zero-cost until the first `m`/`+`);
        // (2) an UNMUTED boot spawns AND the controller's Drop JOINS that thread
        //     (the teardown-on-quit guarantee this PR exists to deliver).
        use std::sync::atomic::{AtomicBool, Ordering};
        // A measurable teardown makes (2) a DETERMINISTIC red: without the join,
        // Drop returns while the thread is still tearing down → `done` is false.
        const TEARDOWN_MS: u64 = 300;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\n").unwrap();

        // (1) muted: the injected respawn must never run.
        let muted_spawned = std::cell::Cell::new(false);
        let c = AudioController::new_with(true, 0.4, path.clone(), |_, _| muted_spawned.set(true));
        assert!(!muted_spawned.get(), "a muted boot spawns no device thread");
        assert!(!c.handle().is_enabled());
        drop(c); // disabled handle → Drop is a no-op, must not hang

        // (2) unmuted: respawn runs at the kept volume and installs a joinable
        // fake device thread (same shape as run_loop: block on the channel, then
        // take TEARDOWN_MS to finish); dropping the controller must JOIN it.
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

        drop(c); // AudioController Drop → shutdown() → join_with_timeout
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
        // #752 Ctrl-C bug: the flush used to run only on the TUI `q` branch, so
        // an external terminate (Ctrl-C / signal / error) lost a debounced volume
        // nudge. Now `AudioController::drop` flushes on EVERY exit — modelled by
        // dropping a dirtied controller WITHOUT the q path or an explicit flush.
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
        // Drop now does config I/O on EVERY exit — the ONLY guard against a
        // needless rewrite (+ `.bak` churn) on every quit is `volume_dirty`. Pin
        // it: an un-nudged controller's drop must leave the config byte-identical
        // (a mutant flipping the guard to `if true` would else survive).
        let (c, _dir) = ctl(false, 0.50);
        let path = c.config_path.clone();
        let before = std::fs::read_to_string(&path).unwrap();
        drop(c); // no nudge → flush_on_exit is a no-op
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
        // The quit bug: the device thread is detached, so on exit its RodioSink
        // Drop (which closes the OS output device) races process teardown and
        // usually loses. shutdown() must (a) close the channel and (b) JOIN the
        // thread, so the teardown COMPLETES before shutdown() returns.
        //
        // Modelled device-free: a fake device thread that, once its channel
        // closes (mirroring run_loop's `Disconnected => return`), takes a
        // measurable TEARDOWN_MS to finish before flagging done. The delay is
        // what makes the test a DETERMINISTIC red: WITHOUT the join, shutdown()
        // returns while the thread is still tearing down → `done` is false → the
        // assert fails. WITH the join it waits → `done` is true. (A zero-cost
        // teardown would let the detached thread win the race by luck, so the
        // test would pass even unfixed — the false-green this delay removes.)
        use std::sync::atomic::{AtomicBool, Ordering};
        const TEARDOWN_MS: u64 = 300;

        let handle = AudioHandle::disabled();
        let rx = handle.install_test_channel(); // fills the shared tx with a sender
        let done = std::sync::Arc::new(AtomicBool::new(false));

        let flag = std::sync::Arc::clone(&done);
        let thread = std::thread::spawn(move || {
            // Block until the sole sender drops (channel closed), exactly as
            // run_loop returns on RecvError::Disconnected.
            while rx.recv().is_ok() {}
            // The teardown a real RodioSink Drop does takes time (closing the OS
            // device). Simulate it so an un-joined shutdown provably returns too
            // early.
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
        // muting back must NOT spawn again
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
        // the sticky (unmuted, disabled) state: '+' must re-attempt the spawn
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
        // volume-down while unmuted: no mute persist, no respawn
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
        // the TUI's [p]ause term: the handle is silenced but the user's own
        // mute flag is preserved (unpause restores it)
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
        // The drift the in-place swap fixes: apply() USED to replace the
        // handle, stranding every clone a renderer cached (frames fed a dead
        // channel until a manual re-sync). Now the sender is swapped INTO the
        // shared handle, so a clone taken BEFORE the respawn stays live and the
        // re-sync obligation is gone.
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
    /// The live device sender, swappable IN PLACE behind a shared cell. A lazy
    /// respawn ([`AudioHandle::respawn_in_place`]) fills it, and because every
    /// clone shares this `Arc`, a consumer's cached clone never goes stale — no
    /// re-sync after [`AudioController::apply`]. `None` = disabled (no device,
    /// or sound not requested yet).
    tx: std::sync::Arc<std::sync::Mutex<Option<mpsc::SyncSender<AudioFrame>>>>,
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
    /// The device thread's join handle, so [`shutdown`](Self::shutdown) can
    /// WAIT for `run_loop` to drop its `RodioSink` (which closes the OS output
    /// device) BEFORE the process exits. Shared like `tx` so any clone can shut
    /// down. Without this the device thread is detached and its teardown races
    /// process exit — on macOS CoreAudio the loser strands the output (audio
    /// keeps playing; `sudo killall coreaudiod` to recover).
    join: std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl AudioHandle {
    /// The inert handle: sound not requested yet (muted — the default —
    /// with the lazy spawn untriggered) or no usable output device. Every
    /// call is a no-op.
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

    /// Push one frame of audio intent. `try_send` — a saturated audio
    /// thread drops frames rather than ever stalling the render loop.
    pub(crate) fn frame(&self, frame: AudioFrame) {
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
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

    /// Whether audio would actually be heard — enabled, not effective-muted
    /// (m OR pause), and above zero volume (a 0% ♩ would advertise sound that
    /// isn't playing). The ONE audibility predicate both painters' footer ♩
    /// indicators read; a method on the handle (which owns all three reads)
    /// keeps the caller's disjoint borrow of the rest of the renderer.
    pub(crate) fn is_audible(&self) -> bool {
        self.is_enabled() && !self.is_muted() && self.volume() > 0.0
    }

    /// (Re)open the output device + audio thread and swap the live sender INTO
    /// this handle in place. Because `tx` is shared across clones, every
    /// consumer that cached a clone (the renderers' per-frame feed) keeps
    /// working — there is NO re-sync obligation after a lazy respawn. A
    /// no-device / feature-off system leaves the handle disabled. Injected into
    /// [`apply_audio_action`] via [`respawn`] so the transition stays
    /// device-free in tests.
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
                    // Swap the live sender in place; every cached clone follows.
                    // Replacing the sole sender also CLOSES any prior thread's
                    // channel, so retire that thread (join it) rather than leak
                    // it — a re-spawn ('+' retry / re-open) must not orphan a
                    // device thread still holding the output. Locks are dropped
                    // before the join so the keypress path never blocks holding
                    // one.
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

    /// Stop the device thread synchronously — THE quit-path teardown, called
    /// from [`AudioController`]'s `Drop` (so it runs on every painter exit
    /// without a hand-wired call). Drops the sole sender so `run_loop` sees the
    /// channel close and returns, dropping its `RodioSink` (which closes the OS
    /// output device), then JOINS the thread so that Drop actually completes
    /// BEFORE the process exits.
    ///
    /// Without the join the device thread is detached and its Drop races
    /// process teardown — it usually loses, and on macOS CoreAudio a half-closed
    /// output strands playback (music keeps going; `sudo killall coreaudiod` to
    /// recover). The reference lofi TUI (lowfi) stops its sink synchronously on
    /// quit for the same reason; this is that, adapted to the off-thread device.
    /// Bounded so a pathological rodio/cpal Drop can't hang the exit — a timeout
    /// is no worse than today's always-detached behaviour.
    ///
    /// INVARIANT: this runs from `AudioController::drop`, i.e. only after the
    /// painter's input/render loop has ended and the controller is being torn
    /// down — so `shutdown` / `respawn_in_place` / `frame` never run
    /// concurrently on the shared `tx`/`join` cells. Idempotent (`take()`-based)
    /// regardless.
    pub(crate) fn shutdown(&self) {
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let handle = self.join.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            join_with_timeout(handle, SHUTDOWN_JOIN_TIMEOUT);
        }
    }

    /// Test seam: a live handle whose receiver the test drains — the ONE
    /// way to observe what the render path actually feeds the audio thread
    /// (the online-review HIGH: the floor-scoping wiring needs a pin).
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

    /// Test seam: fill the shared sender in place (as a lazy respawn would),
    /// returning the receiver — lets a test show a clone taken BEFORE the
    /// respawn stays live (the staleness the in-place swap fixes).
    #[cfg(test)]
    pub(crate) fn install_test_channel(&self) -> mpsc::Receiver<AudioFrame> {
        let (tx, rx) = mpsc::sync_channel(256);
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        rx
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

/// Upper bound on how long [`AudioHandle::shutdown`] waits for the device
/// thread to finish its teardown.
///
/// The COMMON case is fast: once the office is running, `run_loop` sits in a
/// `recv_timeout` and returns within one `TICK_MS` (50ms) of the channel
/// closing. But `run_loop` is BLIND to the closed channel while it is inside a
/// synthesis build — the startup `AssetBank` build and each `TrackBeds::build`
/// (~2s each in release, far more in debug; see the `run_loop` synth-window
/// comment). Quitting during that window (unmute-then-immediately-quit, or a
/// mood swap) forces the thread through the in-flight build before it can
/// observe the disconnect, so the ceiling must comfortably exceed a release
/// build — otherwise the join times out and the device thread falls back to
/// DETACHED (the very bug this fixes). 8s covers the realistic
/// startup+first-frame worst case (~4s release) with load headroom, and still
/// bounds a genuinely hung cpal/CoreAudio device-close. A debug build's longer
/// synth can still exceed it — accepted: debug is not shipped, and a slow-quit
/// leak in a dev build is the mild failure, not a user one.
const SHUTDOWN_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Join `handle`, but give up after `timeout` so a hung device-close (or a
/// still-in-flight multi-second synth build, see [`SHUTDOWN_JOIN_TIMEOUT`])
/// can't block the exit forever. The join runs on a helper thread whose
/// completion we wait on with a timeout; on timeout the helper is left detached,
/// still blocked on the real join. That is harmless for the `shutdown` caller
/// (the process is exiting). For the `respawn_in_place` caller — where the
/// session CONTINUES — a timeout instead leaves the retired device thread to
/// finish on its own; no worse than the pre-fix always-detached behaviour, and
/// that path is near-unreachable in normal flow anyway. std has no timed
/// `JoinHandle::join`, hence the channel dance.
fn join_with_timeout(handle: std::thread::JoinHandle<()>, timeout: std::time::Duration) {
    let (done_tx, done_rx) = mpsc::channel();
    // `Builder::spawn` (not `thread::spawn`) so an OS thread-exhaustion failure
    // returns `Err` instead of PANICKING: this runs from `AudioController::drop`,
    // which can execute during unwind, where a panic would double-panic → abort.
    // On that extreme failure the retired thread simply detaches (its `handle` was
    // moved into the un-spawned closure and drops un-joined) — no worse than the
    // pre-fix always-detached behaviour, and we never block the calling thread.
    let spawned = std::thread::Builder::new().spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    if spawned.is_ok() {
        let _ = done_rx.recv_timeout(timeout);
    }
}

/// The production lazy-respawn injected into [`apply_audio_action`] /
/// [`AudioController::apply`]: (re)open the device + thread and swap the live
/// sender into `handle` in place, so every cached clone keeps working. A named
/// free fn (not an inline closure at each call site) so the two callers can't
/// drift, and device-free-injectable in tests.
pub(crate) fn respawn(handle: &AudioHandle, volume: f32) {
    handle.respawn_in_place(volume);
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
        // The recorder rides a `(Mutex, Condvar)` pair so the frame-1 barrier
        // below can BLOCK on the sink's own progress instead of polling a
        // wall clock: an in-test deadline is the one flakiness knob
        // `.config/nextest.toml` is structurally powerless over (an `assert!`
        // on `Instant::now()` can't be relaxed by `slow-timeout`), and a trip
        // here also fails cargo-mutants' unmutated BASELINE, which then tests
        // zero mutants. Synthesis is MEASURED at >10s debug on M-series, so
        // this barrier is genuinely machine-speed-bound.
        let recorder = Arc::new((
            std::sync::Mutex::new(sink::NullSink::default()),
            std::sync::Condvar::new(),
        ));
        // `.1` flips when the device (the `Box<dyn AudioSink>` run_loop owns) is
        // DROPPED — i.e. when run_loop returns on Disconnect. In production that
        // Drop is the RodioSink closing the OS device; the recorder is a SEPARATE
        // shared handle that outlives the thread, so it can't observe the drop —
        // this flag can. Guards the exact teardown quit relies on against a future
        // run_loop refactor that moved or `mem::forget`-ed the device out.
        struct Probe(
            Arc<(std::sync::Mutex<sink::NullSink>, std::sync::Condvar)>,
            Arc<std::sync::atomic::AtomicBool>,
        );
        impl Probe {
            /// Record, then wake anyone waiting on the sink's progress.
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

        // rain stays 0 so no scheduler one-shot can race the count —
        // only the two frame events are audible
        tx.send(AudioFrame {
            stems: StemLevels::default(),
            events: vec![OneShot::DoorChime, OneShot::PrinterWhir],
            track: Default::default(),
        })
        .unwrap();
        // Wait until the loop has processed frame 1 (the bank build delays it
        // by seconds) so the mute below deterministically lands BETWEEN the
        // frames, not before both. UNBOUNDED on purpose — the only way this
        // never wakes is run_loop failing to play frame 1's one-shots at all,
        // which nextest's `terminate-after` reports as this named test hanging.
        {
            let (lock, progress) = &*recorder;
            let mut rec = lock.lock().unwrap();
            while rec.one_shots < 2 {
                rec = progress.wait(rec).unwrap();
            }
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
        let beds = TrackBeds::build(&mut rng, TrackId::GenDay(0));
        let night = TrackBeds::build(&mut rng, TrackId::GenNight(0));
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
            let buf = render_tier(&bank, &beds, &rain, TrackId::GenDay(0), stems, events, 60.0);
            assert!(
                buf.iter().any(|&s| s.abs() > 0.01),
                "{name}: every tier is audible in Phase 2"
            );
            write_wav(&out.join(format!("{name}.wav")), &buf);
        }
        // the NIGHT track (#644): the runtime approximation of the v4 take
        // (no bus glue — rodio has no insert; the owner re-verifies by ear)
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
