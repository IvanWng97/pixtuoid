//! Floating-window keyboard input: a winit key read as the crossterm key the
//! TUI's [`dispatch_key`](crate::panels::dispatch_key) decodes, so both
//! painters take one vocabulary and one precedence of panels.

use crossterm::event::{KeyCode, KeyModifiers};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// `key` with `mods` held as the TUI reads it; `None` for a key it binds
/// nothing to.
pub(crate) fn key(key: &Key, mods: ModifiersState) -> Option<(KeyCode, KeyModifiers)> {
    let code = match key {
        Key::Named(named) => match named {
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            _ => return None,
        },
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
        _ => return None,
    };
    let mut held = KeyModifiers::NONE;
    if mods.control_key() {
        held |= KeyModifiers::CONTROL;
    }
    Some((code, held))
}

/// Whether a held `code` fires again: a step through a list, a floor or the
/// volume may, a toggle must not oscillate (winit flags repeats; a terminal
/// delivers each as a fresh press).
pub(crate) fn repeats(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Char('j' | 'k' | 'h' | 'l' | '+' | '=' | '-' | '_')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's keys reach the TUI's dispatch as its own: characters as
    /// typed, the named keys it binds, and Ctrl held through.
    #[test]
    fn a_window_key_reads_as_the_tuis() {
        let none = ModifiersState::empty();
        assert_eq!(
            key(&Key::Character("t".into()), none),
            Some((KeyCode::Char('t'), KeyModifiers::NONE))
        );
        assert_eq!(
            key(&Key::Character("c".into()), ModifiersState::CONTROL),
            Some((KeyCode::Char('c'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            key(&Key::Named(NamedKey::Tab), none),
            Some((KeyCode::Tab, KeyModifiers::NONE))
        );
        assert_eq!(
            key(&Key::Named(NamedKey::Space), none),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(key(&Key::Named(NamedKey::F1), none), None);
    }

    /// A held toggle fires once; a held step keeps stepping.
    #[test]
    fn only_steps_repeat() {
        for code in [KeyCode::Char('j'), KeyCode::Down, KeyCode::Char('+')] {
            assert!(repeats(code), "{code:?}");
        }
        for code in [
            KeyCode::Char('p'),
            KeyCode::Char('m'),
            KeyCode::Char('t'),
            KeyCode::Tab,
            KeyCode::Enter,
        ] {
            assert!(!repeats(code), "{code:?}");
        }
    }
}
