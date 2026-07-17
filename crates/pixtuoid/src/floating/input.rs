//! Floating-window keyboard input — the winit KEY→action map for the audio
//! runtime controls (#633 close-out). The state TRANSITION is shared with the
//! TUI in `crate::audio` ([`crate::audio::apply_audio_action`]); only this
//! key-decoding half is painter-specific (winit here, crossterm in the TUI),
//! so the two surfaces can't drift on feel. The winit glue in `window.rs`
//! stays thin (codecov-ignored, like the TUI event loop).

use crate::audio::AudioAction;
use winit::keyboard::Key;

/// Map a winit logical key to an [`AudioAction`] — the TUI's `m` / `+`(`=`) /
/// `-`(`_`) vocabulary (lowercase `m` only, matching the TUI's
/// `KeyCode::Char('m')`; no Shift+M).
///
/// winit delivers an explicit `repeat` flag, which the TUI's crossterm path
/// LACKS (crossterm surfaces OS autorepeat as ordinary `Press` events unless
/// the never-enabled kitty protocol is on, so a held `m` there re-dispatches).
/// We use it as floating-only hardening: volume keys accept repeats (holding
/// `-` slides the volume), the mute TOGGLE swallows them (a held `m` must not
/// oscillate).
pub(crate) fn audio_action(key: &Key, repeat: bool) -> Option<AudioAction> {
    let Key::Character(s) = key else {
        return None;
    };
    match s.as_str() {
        "m" if !repeat => Some(AudioAction::ToggleMute),
        "+" | "=" => Some(AudioAction::Volume(true)),
        "-" | "_" => Some(AudioAction::Volume(false)),
        _ => None,
    }
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
            audio_action(&key("m"), true),
            None,
            "held m must not oscillate"
        );
        assert_eq!(
            audio_action(&key("M"), false),
            None,
            "Shift+M is not mute — parity with the TUI's Char('m')"
        );
        for k in ["+", "="] {
            assert_eq!(
                audio_action(&key(k), false),
                Some(AudioAction::Volume(true))
            );
            assert_eq!(
                audio_action(&key(k), true),
                Some(AudioAction::Volume(true)),
                "up-volume keys autorepeat"
            );
        }
        for k in ["-", "_"] {
            assert_eq!(
                audio_action(&key(k), false),
                Some(AudioAction::Volume(false))
            );
            assert_eq!(
                audio_action(&key(k), true),
                Some(AudioAction::Volume(false)),
                "down-volume keys autorepeat too (symmetry with up)"
            );
        }
        assert_eq!(audio_action(&key("q"), false), None);
        assert_eq!(
            audio_action(&Key::Named(winit::keyboard::NamedKey::Enter), false),
            None
        );
    }
}
