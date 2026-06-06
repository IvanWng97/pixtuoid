pub mod anim;
pub mod chitchat;
pub mod embedded_pack;
pub mod floor;
pub mod frame_cache;
pub mod hit_test;
pub mod layout;
pub mod motion;
pub mod pathfind;
pub mod pet;
pub mod pixel_painter;
pub mod pose;
pub mod renderer;
pub mod theme;
pub mod tui_renderer;
pub mod widgets;

use std::io::{stdout, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use pixtuoid_core::Renderer;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use tui_renderer::TuiRenderer;

use crate::runtime::SceneRx;

/// The modal + floor state the key dispatcher needs. Pulled out so the dispatch
/// decision is a pure function of (key, state) and can be unit-tested without a
/// TTY — the crossterm `read()` and all renderer/config side effects stay in the
/// event loop. The modal priority is help > version-popup > theme-picker > normal.
#[derive(Clone, Copy)]
struct KeyCtx {
    help_open: bool,
    version_popup: bool,
    theme_picker: Option<usize>,
    n_themes: usize,
    n_floors: usize,
    current_floor: usize,
    in_transition: bool,
}

/// The decision a key press resolves to. The event loop maps each variant to the
/// concrete renderer/config side effect; keeping the decision data-only is what
/// makes the modal precedence and the floor-nav / theme-picker guards testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    None,
    Quit,
    TogglePause,
    ToggleHelp,
    CloseHelp,
    DismissVersionPopup,
    OpenThemePicker,
    /// Preview the theme at this index (picker navigation; index is pre-clamped).
    ThemePreview(usize),
    /// Enter in the picker: persist + close on this index.
    ThemeCommit(usize),
    /// Esc in the picker: revert to the saved theme + close.
    ThemeCancel,
    /// Navigate to this (already validated, in-range, no-transition) floor.
    NavigateFloor(usize),
    /// Toggle the live walkable / approach / route debug layer (`w`).
    /// Dev-only: the `w` dispatch arm is `#[cfg(debug_assertions)]`-gated, so in
    /// release this variant is never constructed — silence the dead-code lint
    /// there. The match arm in `run_tui` stays unconditional for exhaustiveness.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    ToggleWalkableDebug,
}

/// Left-click pin toggle: if an agent is pinned, clear it; otherwise hit-test
/// the click against the desk layout and pin whatever it lands on. Identical
/// in both the pet-present and pet-absent click branches, so it lives here.
fn toggle_pin<B: ratatui::backend::Backend<Error: Send + Sync + 'static>>(
    renderer: &mut TuiRenderer<B>,
    scene_rx: &SceneRx,
    col: u16,
    row: u16,
) {
    let pinned = renderer.pinned_agent();
    if pinned.is_some() {
        renderer.set_pinned_agent(None);
    } else {
        let snap = scene_rx.borrow().clone();
        let hit = renderer
            .cached_layout()
            .and_then(|layout| renderer::hit_test_from_tui(&snap, layout, col, row));
        renderer.set_pinned_agent(hit);
    }
}

fn is_quit_chord(code: KeyCode, mods: KeyModifiers) -> bool {
    matches!(
        (code, mods),
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

/// Pure key-dispatch: resolve a key press to a `KeyAction` given the current
/// modal + floor state. Modal precedence (highest first): help overlay,
/// version popup, theme picker, then the normal scene.
fn dispatch_key(code: KeyCode, mods: KeyModifiers, ctx: KeyCtx) -> KeyAction {
    if ctx.help_open {
        return match (code, mods) {
            (KeyCode::Enter, _) | (KeyCode::Esc, _) | (KeyCode::Char('?'), _) => {
                KeyAction::CloseHelp
            }
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            _ => KeyAction::None,
        };
    }
    if ctx.version_popup {
        return match (code, mods) {
            (KeyCode::Enter, _) => KeyAction::DismissVersionPopup,
            (KeyCode::Esc, _) => KeyAction::Quit,
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            _ => KeyAction::None,
        };
    }
    if let Some(idx) = ctx.theme_picker {
        return match code {
            KeyCode::Up | KeyCode::Char('k') => KeyAction::ThemePreview(idx.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                KeyAction::ThemePreview((idx + 1).min(ctx.n_themes.saturating_sub(1)))
            }
            KeyCode::Enter => KeyAction::ThemeCommit(idx),
            KeyCode::Esc => KeyAction::ThemeCancel,
            _ => KeyAction::None,
        };
    }
    // Normal scene.
    if is_quit_chord(code, mods) || code == KeyCode::Esc {
        return KeyAction::Quit;
    }
    match code {
        KeyCode::Char('p') => KeyAction::TogglePause,
        KeyCode::Char('t') => KeyAction::OpenThemePicker,
        KeyCode::Char('?') => KeyAction::ToggleHelp,
        // Dev-only walkable/approach/route overlay — gated out of release builds.
        #[cfg(debug_assertions)]
        KeyCode::Char('w') => KeyAction::ToggleWalkableDebug,
        KeyCode::PageUp | KeyCode::Up | KeyCode::Char('k') => {
            if ctx.current_floor + 1 < ctx.n_floors && !ctx.in_transition {
                KeyAction::NavigateFloor(ctx.current_floor + 1)
            } else {
                KeyAction::None
            }
        }
        KeyCode::PageDown | KeyCode::Down | KeyCode::Char('j') => {
            if ctx.current_floor > 0 && !ctx.in_transition {
                KeyAction::NavigateFloor(ctx.current_floor - 1)
            } else {
                KeyAction::None
            }
        }
        _ => KeyAction::None,
    }
}

// --- Terminal lifecycle ---------------------------------------------------
// Lives here (not renderer.rs) because raw mode + the alternate screen are
// owned by the event loop, and this file is already excluded from headless
// coverage — no test can exercise a real TTY (issue #103).

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    // EnableMouseCapture turns on the terminal's mouse-event reporting.
    // Modern terminals emit MouseEventKind::Moved on cursor motion (no
    // button required), which is how we drive the hover tooltip.
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    term.show_cursor()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_tui(
    mut scene_rx: SceneRx,
    pack_dir: Option<std::path::PathBuf>,
    floor_caps: Arc<[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS]>,
    theme: &'static theme::Theme,
    config_path: std::path::PathBuf,
    desk_cap: Option<usize>,
    pets: Vec<pet::Pet>,
) -> Result<()> {
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term, theme, pets);
    let mut version_popup = {
        let current_ver = env!("CARGO_PKG_VERSION");
        let cfg = crate::config::load(&config_path);
        let decision = crate::version::boot_decision(current_ver, cfg.last_seen_version.as_deref());
        // Persist the current version immediately so the popup shows at
        // most once per upgrade, regardless of how the user exits this run
        // (Enter to dismiss, Esc/q/Ctrl+C to quit, or terminal close).
        // Also overwrites a corrupted/hand-edited last_seen_version so the
        // popup can't be silently disabled forever.
        if decision.should_persist {
            if let Err(e) = crate::config::save_version(&config_path, current_ver) {
                tracing::warn!("failed to persist version: {e}");
            }
        }
        decision.should_show_popup
    };
    let mut last_layout_sig: Option<(u16, u16)> = None;
    let mut paused = false;
    let mut frozen_now: Option<SystemTime> = None;
    let mut theme_picker: Option<usize> = None;
    let mut saved_theme_idx: usize = theme::ALL_THEMES
        .iter()
        .position(|t| std::ptr::eq(*t, theme))
        .unwrap_or(0);

    let tick = Duration::from_millis(33);
    let result: Result<()> = (async {
        loop {
            let now = if paused {
                *frozen_now.get_or_insert(SystemTime::now())
            } else {
                frozen_now = None;
                SystemTime::now()
            };
            let snapshot = scene_rx.borrow_and_update().clone();
            renderer.evict_missing(&snapshot);
            let sig = (renderer.buf().width, renderer.buf().height);
            if last_layout_sig != Some(sig) {
                renderer.invalidate_routes();
                renderer.cancel_transition();
                last_layout_sig = Some(sig);
            }
            renderer.set_theme_picker(theme_picker);
            renderer.set_version_popup(version_popup, now);
            renderer.render(&snapshot, &pack, now)?;

            // Auto-compute per-floor desk capacity from the current
            // terminal dimensions. Each floor uses its own layout seed, so
            // different variants may have different desk counts. fetch_max
            // ensures capacity only grows (monotone) to prevent shifting
            // cumulative offsets that would remap agents on floor 1+ to
            // wrong desk positions. On terminal shrink, agents beyond the
            // layout's capacity become invisible but stay alive; they
            // reappear when the terminal grows back.
            if let Some(layout) = renderer.cached_layout() {
                use pixtuoid_core::layout::{SceneLayout, MAX_VISIBLE_DESKS};
                use pixtuoid_core::state::MAX_FLOORS;
                let buf_w = layout.buf_w;
                let buf_h = layout.buf_h;
                for floor_idx in 0..MAX_FLOORS {
                    let seed =
                        (floor_idx as u64).wrapping_mul(crate::tui::floor::FLOOR_SEED_MULTIPLIER);
                    let mut capacity =
                        SceneLayout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, seed)
                            .map(|l| l.home_desks.len())
                            .unwrap_or(0);
                    if let Some(cap) = desk_cap {
                        capacity = capacity.min(cap);
                    }
                    if capacity > 0 {
                        floor_caps[floor_idx]
                            .fetch_max(capacity, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            let start = Instant::now();
            let mut polled = event::poll(tick)?;
            let mut quit = false;
            while polled {
                match event::read()? {
                    Event::Key(k) => {
                        let ctx = KeyCtx {
                            help_open: renderer.help_open(),
                            version_popup,
                            theme_picker,
                            n_themes: theme::ALL_THEMES.len(),
                            n_floors: crate::tui::floor::num_floors(&snapshot),
                            current_floor: renderer.current_floor(),
                            in_transition: renderer.transition().is_some(),
                        };
                        match dispatch_key(k.code, k.modifiers, ctx) {
                            KeyAction::None => {}
                            KeyAction::Quit => quit = true,
                            KeyAction::TogglePause => paused = !paused,
                            KeyAction::ToggleHelp => {
                                let open = renderer.help_open();
                                renderer.set_help_open(!open);
                            }
                            KeyAction::CloseHelp => renderer.set_help_open(false),
                            KeyAction::DismissVersionPopup => version_popup = false,
                            KeyAction::OpenThemePicker => theme_picker = Some(saved_theme_idx),
                            KeyAction::ThemePreview(i) => {
                                theme_picker = Some(i);
                                renderer.set_theme(theme::ALL_THEMES[i]);
                            }
                            KeyAction::ThemeCommit(i) => {
                                saved_theme_idx = i;
                                theme_picker = None;
                                let name = theme::ALL_THEMES[i].name;
                                if let Err(e) = crate::config::save(&config_path, name) {
                                    tracing::warn!("failed to persist theme: {e}");
                                }
                            }
                            KeyAction::ThemeCancel => {
                                renderer.set_theme(theme::ALL_THEMES[saved_theme_idx]);
                                theme_picker = None;
                            }
                            KeyAction::NavigateFloor(target) => {
                                renderer.navigate_floor(target, now);
                            }
                            KeyAction::ToggleWalkableDebug => {
                                let on = renderer.debug_walkable();
                                renderer.set_debug_walkable(!on);
                            }
                        }
                    }
                    Event::Mouse(m) if renderer.help_open() => {
                        // The help overlay is modal for the mouse: a left
                        // click dismisses it and every mouse event is
                        // swallowed so nothing leaks to the scene behind it
                        // (e.g. coffee-machine / branding clicks launching a
                        // browser). Placed before the popup guard so help
                        // wins even mid popup-dismiss animation.
                        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                            renderer.set_help_open(false);
                        }
                    }
                    Event::Mouse(m) if renderer.last_popup_scale() > 0.0 => {
                        // While the popup is animating or fully visible, only
                        // the URL link is clickable; all other clicks are
                        // swallowed so they don't fall through to the scene.
                        // Uses the painter's frame-scale (last_popup_scale) so
                        // the click geometry matches what was actually painted.
                        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                            if let Ok((cols, rows)) = crossterm::terminal::size() {
                                let bounds = ratatui::layout::Rect {
                                    x: 0,
                                    y: 0,
                                    width: cols,
                                    height: rows,
                                };
                                let notes_len =
                                    crate::version::release_notes(env!("CARGO_PKG_VERSION"))
                                        .map(|n| n.len())
                                        .unwrap_or(0);
                                let scale = renderer.last_popup_scale();
                                if let Some(rect) =
                                    widgets::version_popup_url_rect(notes_len, bounds, scale)
                                {
                                    if m.column >= rect.x
                                        && m.column < rect.x + rect.width
                                        && m.row >= rect.y
                                        && m.row < rect.y + rect.height
                                    {
                                        let _ = open::that(widgets::VERSION_POPUP_URL);
                                    }
                                }
                            }
                        }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                            if m.row <= 1 && m.column >= 1 && m.column < 31 {
                                let _ = open::that("https://github.com/IvanWng97/pixtuoid");
                            } else if renderer.cached_layout().is_some_and(|layout| {
                                renderer::hit_test_coffee_machine(layout, m.column, m.row)
                            }) {
                                let _ = open::that("https://buymeacoffee.com/IvanWng97");
                            } else if let Some(crate::tui::pet::PetFrame {
                                pos: pet_pos,
                                anim,
                                kind,
                            }) = renderer.cached_pet_pos()
                            {
                                if renderer
                                    .active_pet_ref()
                                    .map_or(true, |p| !p.is_active(now))
                                    && renderer::hit_test_pet(kind, pet_pos, anim, m.column, m.row)
                                {
                                    renderer.set_active_pet(Some(renderer::PetState {
                                        petted_at: now,
                                        pet_pos,
                                        kind,
                                        floor_idx: renderer.current_floor(),
                                    }));
                                } else {
                                    toggle_pin(&mut renderer, &scene_rx, m.column, m.row);
                                }
                            } else {
                                toggle_pin(&mut renderer, &scene_rx, m.column, m.row);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                polled = event::poll(Duration::from_millis(0))?;
            }
            if quit {
                if theme_picker.is_some() {
                    renderer.set_theme(theme::ALL_THEMES[saved_theme_idx]);
                }
                break;
            }
            let elapsed = start.elapsed();
            if let Some(rem) = tick.checked_sub(elapsed) {
                tokio::time::sleep(rem).await;
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    })
    .await;

    teardown_terminal(&mut renderer.terminal)?;
    result
}

#[cfg(test)]
mod dispatch_tests {
    use super::{dispatch_key, KeyAction, KeyCtx};
    use crossterm::event::{KeyCode, KeyModifiers};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    // Default: normal scene, mid-stack floor (1 of 3), no transition.
    fn ctx() -> KeyCtx {
        KeyCtx {
            help_open: false,
            version_popup: false,
            theme_picker: None,
            n_themes: 6,
            n_floors: 3,
            current_floor: 1,
            in_transition: false,
        }
    }

    #[test]
    fn normal_quit_pause_picker_help() {
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, ctx()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, ctx()),
            KeyAction::Quit
        );
        assert_eq!(dispatch_key(KeyCode::Esc, NONE, ctx()), KeyAction::Quit);
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, ctx()),
            KeyAction::TogglePause
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, ctx()),
            KeyAction::OpenThemePicker
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, ctx()),
            KeyAction::ToggleHelp
        );
        // `w` only maps in debug builds; in release it falls through to None.
        #[cfg(debug_assertions)]
        assert_eq!(
            dispatch_key(KeyCode::Char('w'), NONE, ctx()),
            KeyAction::ToggleWalkableDebug
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('x'), NONE, ctx()),
            KeyAction::None
        );
    }

    #[test]
    fn floor_nav_guards() {
        // Mid-stack: up and down both valid.
        for code in [KeyCode::PageUp, KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(dispatch_key(code, NONE, ctx()), KeyAction::NavigateFloor(2));
        }
        for code in [KeyCode::PageDown, KeyCode::Down, KeyCode::Char('j')] {
            assert_eq!(dispatch_key(code, NONE, ctx()), KeyAction::NavigateFloor(0));
        }
        // Top floor: no up.
        let top = KeyCtx {
            current_floor: 2,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Up, NONE, top), KeyAction::None);
        // Bottom floor: no down.
        let bottom = KeyCtx {
            current_floor: 0,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Down, NONE, bottom), KeyAction::None);
        // A transition in flight blocks navigation in both directions.
        let mid_trans = KeyCtx {
            in_transition: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Up, NONE, mid_trans), KeyAction::None);
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, mid_trans),
            KeyAction::None
        );
    }

    #[test]
    fn help_overlay_has_priority_and_dismisses() {
        // help wins even when the version popup is also flagged.
        let c = KeyCtx {
            help_open: true,
            version_popup: true,
            theme_picker: Some(2),
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Enter, NONE, c), KeyAction::CloseHelp);
        assert_eq!(dispatch_key(KeyCode::Esc, NONE, c), KeyAction::CloseHelp);
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, c),
            KeyAction::CloseHelp
        );
        assert_eq!(dispatch_key(KeyCode::Char('q'), NONE, c), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('c'), CTRL, c), KeyAction::Quit);
        // Up does not leak to the floor-nav / picker handlers while help is open.
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c), KeyAction::None);
    }

    #[test]
    fn version_popup_enter_dismisses_esc_quits() {
        let c = KeyCtx {
            version_popup: true,
            ..ctx()
        };
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, c),
            KeyAction::DismissVersionPopup
        );
        assert_eq!(dispatch_key(KeyCode::Esc, NONE, c), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('q'), NONE, c), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('c'), CTRL, c), KeyAction::Quit);
        // A floor key while the popup is up is swallowed, not navigated.
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c), KeyAction::None);
    }

    #[test]
    fn theme_picker_preview_commit_cancel_and_clamps() {
        let c = KeyCtx {
            theme_picker: Some(2),
            ..ctx()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, c),
            KeyAction::ThemePreview(1)
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, c),
            KeyAction::ThemePreview(1)
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, c),
            KeyAction::ThemePreview(3)
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, c),
            KeyAction::ThemePreview(3)
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, c),
            KeyAction::ThemeCommit(2)
        );
        assert_eq!(dispatch_key(KeyCode::Esc, NONE, c), KeyAction::ThemeCancel);
        // q does NOT quit while the picker is open (must Esc/Enter out first).
        assert_eq!(dispatch_key(KeyCode::Char('q'), NONE, c), KeyAction::None);

        // Clamp at the ends.
        let lo = KeyCtx {
            theme_picker: Some(0),
            ..ctx()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, lo),
            KeyAction::ThemePreview(0)
        );
        let hi = KeyCtx {
            theme_picker: Some(5),
            n_themes: 6,
            ..ctx()
        };
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, hi),
            KeyAction::ThemePreview(5)
        );
    }
}
