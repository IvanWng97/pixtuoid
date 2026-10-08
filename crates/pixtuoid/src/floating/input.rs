//! Floating-window keyboard input — the winit KEY→action map for the window's
//! controls and the footer hints that name them. The audio state TRANSITION is
//! shared with the TUI in [`crate::audio::apply_audio_action`]; only this
//! key-decoding half is painter-specific (winit here, crossterm in the TUI).

use crate::audio::AudioAction;
use winit::keyboard::Key;

/// What a key does in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Audio(AudioAction),
    Pause,
    /// The next theme: the TUI's `t` opens a picker, which the window has no
    /// panel for, so it takes the next one at once.
    Theme,
    Floor(FloorStep),
}

/// Which way a floor key moves, the TUI's floor keys: PageUp, Up or `k`
/// climbs; PageDown, Down or `j` descends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloorStep {
    Up,
    Down,
}

/// A character key the window binds.
struct Binding {
    key: &'static str,
    action: Action,
    /// Whether a held key fires it again: a volume or floor step may, a
    /// toggle must not oscillate (winit flags repeats; the TUI's crossterm
    /// path can't).
    repeats: bool,
    /// What the footer names it, if it names it.
    hint: Option<&'static str>,
}

/// The window's character keys, the TUI's vocabulary (lowercase only, as the
/// TUI's `KeyCode::Char`s are): the one table [`action`] decodes and
/// [`footer_keys`] lists, so the footer names exactly what the window binds.
const BINDINGS: [Binding; 9] = [
    Binding {
        key: "p",
        action: Action::Pause,
        repeats: false,
        hint: Some("[p]ause"),
    },
    Binding {
        key: "t",
        action: Action::Theme,
        repeats: false,
        hint: Some("[t]heme"),
    },
    Binding {
        key: "m",
        action: Action::Audio(AudioAction::ToggleMute),
        repeats: false,
        hint: Some("[m]ute"),
    },
    Binding {
        key: "+",
        action: Action::Audio(AudioAction::Volume(true)),
        repeats: true,
        hint: Some("[+/-]vol"),
    },
    Binding {
        key: "=",
        action: Action::Audio(AudioAction::Volume(true)),
        repeats: true,
        hint: None,
    },
    Binding {
        key: "-",
        action: Action::Audio(AudioAction::Volume(false)),
        repeats: true,
        hint: None,
    },
    Binding {
        key: "_",
        action: Action::Audio(AudioAction::Volume(false)),
        repeats: true,
        hint: None,
    },
    Binding {
        key: "k",
        action: Action::Floor(FloorStep::Up),
        repeats: true,
        hint: None,
    },
    Binding {
        key: "j",
        action: Action::Floor(FloorStep::Down),
        repeats: true,
        hint: None,
    },
];

/// What `key` does, `repeat` being winit's flag for a held key; `None` for a
/// key the window doesn't bind.
pub(crate) fn action(key: &Key, repeat: bool) -> Option<Action> {
    use winit::keyboard::NamedKey;
    match key {
        Key::Named(NamedKey::PageUp | NamedKey::ArrowUp) => Some(Action::Floor(FloorStep::Up)),
        Key::Named(NamedKey::PageDown | NamedKey::ArrowDown) => {
            Some(Action::Floor(FloorStep::Down))
        }
        Key::Character(s) => BINDINGS
            .iter()
            .find(|b| b.key == s.as_str() && (b.repeats || !repeat))
            .map(|b| b.action),
        _ => None,
    }
}

/// The footer's keybind tail: every [`BINDINGS`] hint, in order.
pub(crate) fn footer_keys() -> &'static str {
    static KEYS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let hints: Vec<_> = BINDINGS.iter().filter_map(|b| b.hint).collect();
        format!(" {} ", hints.join(" "))
    });
    &KEYS
}

/// The theme after `theme` in [`ALL_THEMES`](pixtuoid_scene::theme::ALL_THEMES),
/// the first after the last.
pub(crate) fn next_theme(
    theme: &'static pixtuoid_scene::theme::Theme,
) -> &'static pixtuoid_scene::theme::Theme {
    use pixtuoid_scene::theme::ALL_THEMES;
    let at = ALL_THEMES
        .iter()
        .position(|t| std::ptr::eq(*t, theme))
        .unwrap_or(0);
    ALL_THEMES[(at + 1) % ALL_THEMES.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Key {
        Key::Character(s.into())
    }

    /// The TUI's vocabulary: lowercase only, toggles swallow a held key's
    /// repeats, and volume and floor steps take them.
    #[test]
    fn the_keys_are_the_tuis_and_only_steps_repeat() {
        use winit::keyboard::NamedKey;
        let cases = [
            ("m", Some(Action::Audio(AudioAction::ToggleMute)), false),
            ("+", Some(Action::Audio(AudioAction::Volume(true))), true),
            ("=", Some(Action::Audio(AudioAction::Volume(true))), true),
            ("-", Some(Action::Audio(AudioAction::Volume(false))), true),
            ("_", Some(Action::Audio(AudioAction::Volume(false))), true),
            ("p", Some(Action::Pause), false),
            ("t", Some(Action::Theme), false),
            ("k", Some(Action::Floor(FloorStep::Up)), true),
            ("j", Some(Action::Floor(FloorStep::Down)), true),
            ("M", None, false),
            ("P", None, false),
            ("T", None, false),
            ("q", None, false),
        ];
        for (k, want, repeats) in cases {
            assert_eq!(action(&key(k), false), want, "{k}");
            let held = if repeats { want } else { None };
            assert_eq!(action(&key(k), true), held, "{k} held");
        }
        for (k, step) in [
            (NamedKey::PageUp, FloorStep::Up),
            (NamedKey::ArrowUp, FloorStep::Up),
            (NamedKey::PageDown, FloorStep::Down),
            (NamedKey::ArrowDown, FloorStep::Down),
        ] {
            assert_eq!(action(&Key::Named(k), false), Some(Action::Floor(step)));
        }
        assert_eq!(action(&Key::Named(NamedKey::Enter), false), None);
    }

    /// The footer names a key only when the window binds it: every hint's
    /// `[x]` keys decode, and the hints read in the table's order.
    #[test]
    fn the_footer_names_only_bound_keys() {
        assert_eq!(footer_keys(), " [p]ause [t]heme [m]ute [+/-]vol ");
        for hint in BINDINGS.iter().filter_map(|b| b.hint) {
            let inside = &hint[1..hint.find(']').expect("a [key]")];
            for k in inside.split('/') {
                assert!(action(&key(k), false).is_some(), "{hint}: {k} unbound");
            }
        }
    }

    #[test]
    fn t_cycles_every_theme_once_round() {
        use pixtuoid_scene::theme::ALL_THEMES;
        let mut seen = vec![ALL_THEMES[0].name];
        let mut theme = ALL_THEMES[0];
        for _ in 1..ALL_THEMES.len() {
            theme = next_theme(theme);
            seen.push(theme.name);
        }
        let want: Vec<_> = ALL_THEMES.iter().map(|t| t.name).collect();
        assert_eq!(seen, want, "in order");
        assert!(std::ptr::eq(next_theme(theme), ALL_THEMES[0]), "wraps");
    }
}
