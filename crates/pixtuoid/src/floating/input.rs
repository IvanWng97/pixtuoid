//! Floating-window keyboard input — the PURE half of the audio runtime
//! controls (#633 close-out): winit key → action mapping plus the mute/volume
//! state transitions, mirroring the TUI's `ToggleAudioMute`/`AdjustVolume`
//! dispatch arms 1:1 so the two painters can't drift on behavior. The winit
//! glue in `window.rs` stays thin (codecov-ignored, like the TUI event loop).

use winit::keyboard::Key;

/// The audio actions the floating window's keyboard drives — the TUI's
/// `m` / `+`(`=`) / `-`(`_`) vocabulary, nothing more (floating has no pause,
/// panels, or floor nav today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioAction {
    ToggleMute,
    /// `true` = volume up.
    Volume(bool),
}

/// Map a winit logical key to an [`AudioAction`]. winit delivers explicit
/// key REPEATS: volume keys accept them (holding `-` slides the volume, the
/// crossterm autorepeat feel), the mute TOGGLE swallows them (a held `m`
/// must not oscillate — the TUI never sees repeats as distinct presses for
/// toggles either, via its Press-only gate).
pub(crate) fn audio_action(key: &Key, repeat: bool) -> Option<AudioAction> {
    let Key::Character(s) = key else {
        return None;
    };
    match s.as_str() {
        "m" | "M" if !repeat => Some(AudioAction::ToggleMute),
        "+" | "=" => Some(AudioAction::Volume(true)),
        "-" | "_" => Some(AudioAction::Volume(false)),
        _ => None,
    }
}

/// The floating window's audio UI state — the same trio the TUI loop keeps as
/// locals (`audio` / `audio_muted` / `audio_volume`).
pub(crate) struct AudioUi {
    pub(crate) handle: crate::audio::AudioHandle,
    pub(crate) muted: bool,
    pub(crate) volume: f32,
}

/// What the caller must do after [`apply_audio_action`] — the side effects
/// stay in `window.rs` (config persistence needs the path; the flash needs
/// the wall clock), keeping this half pure and unit-testable.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Persist {
    /// The mute flag changed — `save_audio_muted` NOW (the TUI persists mute
    /// immediately, like a theme commit).
    pub(crate) muted: bool,
    /// The volume changed — flash the readout and persist DEBOUNCED (the
    /// +/- keys autorepeat; per-press ConfigLock rounds were a review
    /// MEDIUM on the TUI side).
    pub(crate) volume_nudged: bool,
}

/// Apply one action — the TUI arms' exact semantics: mute toggles persist;
/// volume-up from muted IS the un-mute gesture; the lazy spawn (re)fires
/// whenever sound is wanted but the system is down (`+`/`m` are never dead
/// keys — boot-muted and failed-spawn states both recover); volume clamps to
/// [0, 1] by the shared [`crate::audio::VOLUME_STEP`]. Floating has no pause
/// key, so effective mute is just `muted` (no `paused ||` term).
pub(crate) fn apply_audio_action(
    st: &mut AudioUi,
    action: AudioAction,
    spawn: impl FnOnce(f32) -> crate::audio::AudioHandle,
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
            let delta = if up {
                crate::audio::VOLUME_STEP
            } else {
                -crate::audio::VOLUME_STEP
            };
            st.volume = (st.volume + delta).clamp(0.0, 1.0);
            if up && st.muted {
                // volume-up IS the un-mute gesture too (TUI parity)
                st.muted = false;
                persist.muted = true;
            }
            persist.volume_nudged = true;
        }
    }
    if !st.muted && !st.handle.is_enabled() {
        st.handle = spawn(st.volume);
    }
    st.handle.set_muted(st.muted);
    st.handle.set_volume(st.volume);
    persist
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn key_map_is_the_tui_vocabulary_and_swallows_mute_repeats() {
        assert_eq!(
            audio_action(&key("m"), false),
            Some(AudioAction::ToggleMute)
        );
        assert_eq!(
            audio_action(&key("M"), false),
            Some(AudioAction::ToggleMute)
        );
        assert_eq!(
            audio_action(&key("m"), true),
            None,
            "held m must not oscillate"
        );
        for k in ["+", "="] {
            assert_eq!(
                audio_action(&key(k), false),
                Some(AudioAction::Volume(true))
            );
            assert_eq!(
                audio_action(&key(k), true),
                Some(AudioAction::Volume(true)),
                "volume keys autorepeat"
            );
        }
        for k in ["-", "_"] {
            assert_eq!(
                audio_action(&key(k), false),
                Some(AudioAction::Volume(false))
            );
        }
        assert_eq!(audio_action(&key("q"), false), None);
        assert_eq!(
            audio_action(&Key::Named(winit::keyboard::NamedKey::Enter), false),
            None
        );
    }

    #[test]
    fn unmute_lazy_spawns_and_mute_back_does_not() {
        // boot-muted state: disabled handle, muted config
        let mut st = AudioUi {
            handle: crate::audio::AudioHandle::disabled(),
            muted: true,
            volume: 0.4,
        };
        let (live, _rx) = crate::audio::AudioHandle::test_pair();
        let mut spawned_at = None;
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, |v| {
            spawned_at = Some(v);
            live.clone()
        });
        assert!(!st.muted);
        assert_eq!(
            spawned_at,
            Some(0.4),
            "first unmute lazy-spawns at the kept volume"
        );
        assert!(st.handle.is_enabled());
        assert!(!st.handle.is_muted());
        assert_eq!(
            p,
            Persist {
                muted: true,
                volume_nudged: false
            }
        );

        // toggling back to muted must NOT spawn again (handle already live)
        let p = apply_audio_action(&mut st, AudioAction::ToggleMute, |_| {
            panic!("mute must never spawn")
        });
        assert!(st.muted);
        assert!(st.handle.is_muted());
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
        // the sticky (unmuted, disabled) state the TUI review MEDIUM covered:
        // '+' must re-attempt the spawn, never go dead
        let mut st = AudioUi {
            handle: crate::audio::AudioHandle::disabled(),
            muted: true,
            volume: 0.5,
        };
        let (live, _rx) = crate::audio::AudioHandle::test_pair();
        let mut spawns = 0;
        let p = apply_audio_action(&mut st, AudioAction::Volume(true), |_| {
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
        assert!((st.volume - (0.5 + crate::audio::VOLUME_STEP)).abs() < 1e-6);
        assert!(
            (st.handle.volume() - st.volume).abs() < 1e-6,
            "handle sees the new volume"
        );

        // volume-down while unmuted: no mute persist, no new spawn
        let p = apply_audio_action(&mut st, AudioAction::Volume(false), |_| {
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
        let (live, _rx) = crate::audio::AudioHandle::test_pair();
        let mut st = AudioUi {
            handle: live,
            muted: false,
            volume: 1.0,
        };
        apply_audio_action(&mut st, AudioAction::Volume(true), |_| unreachable!());
        assert_eq!(st.volume, 1.0, "clamped at the top rail");
        st.volume = 0.0;
        apply_audio_action(&mut st, AudioAction::Volume(false), |_| unreachable!());
        assert_eq!(st.volume, 0.0, "clamped at the bottom rail");
    }
}
