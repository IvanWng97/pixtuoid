//! Floating-window keyboard input — the winit KEY→action map for the audio
//! runtime controls. The state TRANSITION is shared with the TUI in
//! [`crate::audio::apply_audio_action`]; only this key-decoding half is
//! painter-specific (winit here, crossterm in the TUI).

use crate::audio::AudioAction;
use winit::keyboard::Key;

/// Map a winit logical key to an [`AudioAction`] — the TUI's `m` / `+`(`=`) /
/// `-`(`_`) vocabulary (lowercase `m` only, matching the TUI's
/// `KeyCode::Char('m')`).
///
/// winit delivers an explicit `repeat` flag the TUI's crossterm path LACKS. We
/// use it as floating-only hardening: volume keys accept repeats (holding `-`
/// slides the volume), the mute TOGGLE swallows them (a held `m` must not
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

/// Which way a floor key moves, the TUI's floor keys: PageUp, Up or `k`
/// climbs; PageDown, Down or `j` descends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloorStep {
    Up,
    Down,
}

/// The floor step `key` asks for, if it is a floor key.
pub(crate) fn floor_step(key: &Key) -> Option<FloorStep> {
    use winit::keyboard::NamedKey;
    match key {
        Key::Named(NamedKey::PageUp | NamedKey::ArrowUp) => Some(FloorStep::Up),
        Key::Named(NamedKey::PageDown | NamedKey::ArrowDown) => Some(FloorStep::Down),
        Key::Character(s) if s.as_str() == "k" => Some(FloorStep::Up),
        Key::Character(s) if s.as_str() == "j" => Some(FloorStep::Down),
        _ => None,
    }
}

/// Whether `key` toggles the pause: the TUI's `p`, its repeats swallowed as
/// mute's are, so a held key can't oscillate.
pub(crate) fn is_pause(key: &Key, repeat: bool) -> bool {
    !repeat && matches!(key, Key::Character(s) if s.as_str() == "p")
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

    #[test]
    fn floor_and_pause_keys_are_the_tuis() {
        use winit::keyboard::NamedKey;
        for k in [
            Key::Named(NamedKey::PageUp),
            Key::Named(NamedKey::ArrowUp),
            key("k"),
        ] {
            assert_eq!(floor_step(&k), Some(FloorStep::Up), "{k:?}");
        }
        for k in [
            Key::Named(NamedKey::PageDown),
            Key::Named(NamedKey::ArrowDown),
            key("j"),
        ] {
            assert_eq!(floor_step(&k), Some(FloorStep::Down), "{k:?}");
        }
        assert_eq!(floor_step(&key("m")), None);
        assert!(is_pause(&key("p"), false));
        assert!(!is_pause(&key("p"), true), "held p must not oscillate");
        assert!(!is_pause(&key("P"), false));
    }
}
