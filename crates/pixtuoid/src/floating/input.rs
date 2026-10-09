//! Floating-window keyboard input: a winit key read as the crossterm key the
//! TUI's [`dispatch_key`](crate::panels::dispatch_key) decodes, so both
//! painters take one vocabulary and one precedence of panels.

use crossterm::event::{KeyCode, KeyModifiers};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::compose::Screen;
use super::offscreen::OfficeRenderer;
use pixtuoid_scene::theme::Theme;

/// `key` with `mods` held as the TUI reads it, `repeat` being winit's flag
/// for a held key; `None` for a key it binds nothing to, a held toggle, or a
/// Cmd chord, which a terminal keeps for itself and never hands the TUI.
pub(crate) fn key(
    key: &Key,
    mods: ModifiersState,
    repeat: bool,
) -> Option<(KeyCode, KeyModifiers)> {
    if mods.super_key() {
        return None;
    }
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
    if repeat && !repeats(code) {
        return None;
    }
    let mut held = KeyModifiers::NONE;
    if mods.control_key() {
        held |= KeyModifiers::CONTROL;
    }
    if mods.alt_key() {
        held |= KeyModifiers::ALT;
    }
    Some((code, held))
}

/// Whether a held `code` fires again: a step through a list, a floor or the
/// volume may, a toggle must not oscillate (winit flags repeats; a terminal
/// delivers each as a fresh press).
fn repeats(code: KeyCode) -> bool {
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

/// What an applied key changes in the window: its theme, its floor and its
/// next frame.
pub(crate) struct WindowHost<'a> {
    pub(crate) renderer: &'a mut OfficeRenderer,
    pub(crate) theme: &'a mut &'static Theme,
    pub(crate) screen: &'a mut Screen,
}

impl crate::panels::Host for WindowHost<'_> {
    fn set_theme(&mut self, theme: &'static Theme) {
        *self.theme = theme;
    }

    fn navigate_floor(&mut self, floor: usize, now: std::time::SystemTime) {
        self.renderer.navigate(floor, now);
    }

    fn toggle_walkable_debug(&mut self) {
        self.renderer.toggle_walkable_debug();
    }

    fn redraw(&mut self) -> anyhow::Result<()> {
        self.screen.stale();
        Ok(())
    }
}

/// A key read as the TUI's, through its dispatch and panels, the window
/// hosting; whether it asked to quit.
pub(crate) fn press_key(
    (code, mods): (crossterm::event::KeyCode, crossterm::event::KeyModifiers),
    cx: &mut crate::panels::KeyCtx<'_, WindowHost<'_>>,
) -> bool {
    let nav = cx.host.renderer.nav();
    let floor = crate::panels::FloorNav {
        n_floors: cx.host.renderer.n_floors(),
        current_floor: nav.current(),
        in_transition: nav.transition().is_some(),
    };
    let action = crate::panels::dispatch_key(code, mods, cx.ui.modal(), floor);
    crate::panels::apply_key_action(action, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::state::SceneState;
    use std::time::SystemTime;

    /// The window's keys reach the TUI's dispatch as its own: characters as
    /// typed, the named keys it binds, and Ctrl held through.
    #[test]
    fn a_window_key_reads_as_the_tuis() {
        let none = ModifiersState::empty();
        assert_eq!(
            key(&Key::Character("t".into()), none, false),
            Some((KeyCode::Char('t'), KeyModifiers::NONE))
        );
        assert_eq!(
            key(&Key::Character("c".into()), ModifiersState::CONTROL, false),
            Some((KeyCode::Char('c'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            key(&Key::Character("q".into()), ModifiersState::ALT, false),
            Some((KeyCode::Char('q'), KeyModifiers::ALT))
        );
        assert_eq!(
            key(&Key::Named(NamedKey::Tab), none, false),
            Some((KeyCode::Tab, KeyModifiers::NONE))
        );
        assert_eq!(
            key(&Key::Named(NamedKey::Space), none, false),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(key(&Key::Named(NamedKey::F1), none, false), None);
        assert_eq!(
            key(&Key::Character("q".into()), ModifiersState::SUPER, false),
            None,
            "Cmd+Q is the platform's, never the TUI's q"
        );
    }

    /// A held toggle fires once; a held step keeps stepping.
    #[test]
    fn only_steps_repeat() {
        let none = ModifiersState::empty();
        for k in [
            Key::Character("j".into()),
            Key::Named(NamedKey::ArrowDown),
            Key::Character("+".into()),
        ] {
            assert_eq!(key(&k, none, true), key(&k, none, false), "{k:?}");
            assert!(key(&k, none, true).is_some(), "{k:?}");
        }
        for k in [
            Key::Character("p".into()),
            Key::Character("m".into()),
            Key::Character("t".into()),
            Key::Named(NamedKey::Tab),
            Key::Named(NamedKey::Enter),
        ] {
            assert!(key(&k, none, false).is_some(), "{k:?}");
            assert_eq!(key(&k, none, true), None, "held {k:?}");
        }
    }

    /// The window's keys through the TUI's dispatch, the window hosting.
    struct Keys {
        ui: crate::panels::ui_state::UiState,
        renderer: OfficeRenderer,
        theme: &'static Theme,
        screen: Screen,
        audio_ctl: crate::audio::AudioController,
        connected: crate::runtime::ConnectedSources,
        snapshot: SceneState,
        focus_roots: (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
        config_path: std::path::PathBuf,
        _tmp: tempfile::TempDir,
    }

    /// Stands in for `crate::audio::respawn`, which opens a real output device.
    fn no_respawn(_: &crate::audio::AudioHandle, _: f32) {}

    impl Keys {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let config_path = tmp.path().join("config.toml");
            let theme = pixtuoid_scene::theme::ALL_THEMES[0];
            Self {
                ui: crate::panels::ui_state::UiState::new(
                    theme,
                    crate::panels::welcome::WelcomeUi::from_detected(&[]),
                    false,
                    tmp.path().join("sock"),
                    None,
                    crate::doctor::DriftSeen::default(),
                ),
                renderer: OfficeRenderer::new(std::sync::Arc::new(
                    pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack"),
                )),
                theme,
                screen: Screen::new(true),
                audio_ctl: crate::audio::AudioController::new_with(
                    crate::config::AudioConfig {
                        muted: true,
                        volume: 1.0,
                    },
                    config_path.clone(),
                    no_respawn,
                ),
                connected: crate::runtime::ConnectedSources::default(),
                snapshot: SceneState::uniform(4),
                focus_roots: (None, None),
                config_path,
                _tmp: tmp,
            }
        }

        /// Press `code`; whether it quit.
        fn press(&mut self, code: crossterm::event::KeyCode) -> bool {
            press_key(
                (code, crossterm::event::KeyModifiers::NONE),
                &mut crate::panels::KeyCtx {
                    ui: &mut self.ui,
                    host: &mut WindowHost {
                        renderer: &mut self.renderer,
                        theme: &mut self.theme,
                        screen: &mut self.screen,
                    },
                    audio_ctl: &mut self.audio_ctl,
                    config_path: &self.config_path,
                    connected: &self.connected,
                    snapshot: &self.snapshot,
                    focus_roots: &self.focus_roots,
                    now: SystemTime::UNIX_EPOCH,
                    respawn: no_respawn,
                },
            )
        }

        fn saved_theme(&self) -> Option<String> {
            crate::config::load(&self.config_path, &mut Vec::new()).theme
        }
    }

    /// A previewed theme shows in the window at once, and only a commit
    /// saves it: quitting or cancelling mid-preview leaves the config as it
    /// was.
    #[test]
    fn only_a_committed_theme_is_saved() {
        use crossterm::event::KeyCode;
        let first = pixtuoid_scene::theme::ALL_THEMES[0];
        let mut k = Keys::new();
        assert!(!k.press(KeyCode::Char('t')));
        assert!(k.ui.theme_picker.is_some(), "t opens the picker");
        assert!(!k.press(KeyCode::Char('j')));
        assert!(!std::ptr::eq(k.theme, first), "j previews in the window");
        assert!(k.press(KeyCode::Char('q')), "q quits mid-preview");
        assert_eq!(k.saved_theme(), None);

        let mut k = Keys::new();
        k.press(KeyCode::Char('t'));
        k.press(KeyCode::Char('j'));
        assert!(
            !k.press(KeyCode::Esc),
            "Esc closes the picker, not the window"
        );
        assert!(std::ptr::eq(k.theme, first), "and restores the theme");
        assert_eq!(k.saved_theme(), None);

        k.press(KeyCode::Char('t'));
        k.press(KeyCode::Char('j'));
        assert!(!k.press(KeyCode::Enter));
        assert_eq!(k.saved_theme().as_deref(), Some(k.theme.name));
    }

    /// An unwritable config costs only the save: the committed theme still
    /// shows.
    #[test]
    fn an_unwritable_config_keeps_the_committed_theme() {
        use crossterm::event::KeyCode;
        let mut k = Keys::new();
        let blocker = k._tmp.path().join("missing");
        std::fs::write(&blocker, "a file, not a dir").expect("write");
        k.config_path = blocker.join("config.toml");
        k.press(KeyCode::Char('t'));
        k.press(KeyCode::Char('j'));
        assert!(!k.press(KeyCode::Enter));
        assert!(!std::ptr::eq(k.theme, pixtuoid_scene::theme::ALL_THEMES[0]));
    }

    /// Esc closes an open panel before it quits the window.
    #[test]
    fn esc_closes_a_panel_before_it_quits() {
        use crossterm::event::KeyCode;
        let mut k = Keys::new();
        assert!(!k.press(KeyCode::Char('?')));
        assert!(k.ui.help_open());
        assert!(!k.press(KeyCode::Esc));
        assert!(!k.ui.help_open());
        assert!(k.press(KeyCode::Esc), "with nothing open Esc quits");
    }

    /// The walkable debug key reaches the window's renderer.
    #[test]
    fn the_walkable_debug_key_flips_the_windows_layer() {
        use crate::panels::Host;
        let mut k = Keys::new();
        let mut host = WindowHost {
            renderer: &mut k.renderer,
            theme: &mut k.theme,
            screen: &mut k.screen,
        };
        host.toggle_walkable_debug();
        assert!(host.renderer.debug_walkable);
        host.toggle_walkable_debug();
        assert!(!host.renderer.debug_walkable);
    }
}
