//! The modal panels both painters show: the dashboard, the Sources panel, the
//! help, the theme picker, the version popup and the onboarding, with the one
//! key dispatch that drives them ([`dispatch_key`] decodes, [`apply_key_action`]
//! applies). A painter draws them through [`widgets`] into a ratatui frame and
//! carries out what only it can through [`Host`].

pub(crate) mod connection;
pub(crate) mod dashboard;
pub(crate) mod ui_state;
pub(crate) mod welcome;
pub(crate) mod widgets;

use std::time::{Instant, SystemTime};

use crossterm::event::{KeyCode, KeyModifiers};
use pixtuoid_scene::theme;
use ratatui::layout::Rect;

/// How many rows at the bottom of a surface of cells the status footer owns.
pub(crate) const FOOTER_ROWS: u16 = 1;

/// `full` less its footer rows: where the office and the panels are.
pub(crate) fn scene_rect(full: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: full.width,
        height: full.height.saturating_sub(FOOTER_ROWS),
    }
}

/// Clip a widget rect to fit inside `bounds`; `None` when nothing survives.
/// Prevents ratatui's "index outside of buffer" panic when label/notice widgets
/// land near the right or bottom edge.
pub(crate) fn clip_widget_rect(rect: Rect, bounds: Rect) -> Option<Rect> {
    if rect.x >= bounds.x + bounds.width || rect.y >= bounds.y + bounds.height {
        return None;
    }
    if rect.x + rect.width <= bounds.x || rect.y + rect.height <= bounds.y {
        return None;
    }
    let x = rect.x.max(bounds.x);
    let y = rect.y.max(bounds.y);
    let right = (rect.x + rect.width).min(bounds.x + bounds.width);
    let bot = (rect.y + rect.height).min(bounds.y + bounds.height);
    if right <= x || bot <= y {
        return None;
    }
    Some(Rect {
        x,
        y,
        width: right - x,
        height: bot - y,
    })
}

/// One frame's open panels, as [`paint_overlays`] draws them.
pub(crate) struct OverlayFrame<'a> {
    pub(crate) theme_picker: Option<usize>,
    pub(crate) dashboard: &'a crate::panels::dashboard::DashboardFrame,
    pub(crate) connection: &'a crate::panels::connection::ConnectionFrame,
    pub(crate) popup_scale: f32,
    pub(crate) help_open: bool,
    pub(crate) onboarding: &'a crate::panels::welcome::OnboardingFrame,
}

/// The modal-overlay dispatch, centralized so the draw paths can't drift in
/// ordering or args. `bounds` is the FULL terminal area — a modal is centered over
/// the whole frame, and `PanelGeometry` keeps it off the footer row itself.
pub(crate) fn paint_overlays(
    f: &mut ratatui::Frame<'_>,
    ov: &OverlayFrame<'_>,
    now: SystemTime,
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let &OverlayFrame {
        theme_picker,
        dashboard,
        connection,
        popup_scale,
        help_open,
        onboarding,
    } = ov;
    if let Some(idx) = theme_picker {
        widgets::paint_theme_picker(f, idx, bounds, theme);
    }
    if dashboard.open {
        widgets::paint_dashboard(f, dashboard, now, bounds, theme);
    }
    if connection.open {
        widgets::paint_connection_panel(f, connection, now, bounds, theme);
    }
    if popup_scale > 0.0 {
        widgets::paint_version_popup(f, env!("CARGO_PKG_VERSION"), bounds, theme, popup_scale);
    }
    if help_open {
        widgets::paint_help_overlay(f, bounds, theme);
    }
    if onboarding.open {
        widgets::paint_welcome(f, onboarding, bounds, theme);
    }
}

/// What an applied key changes in the painter that shows the office.
pub(crate) trait Host {
    fn set_theme(&mut self, theme: &'static theme::Theme);
    fn navigate_floor(&mut self, floor: usize, now: SystemTime);
    fn toggle_walkable_debug(&mut self);
    /// Repaint the whole surface.
    fn redraw(&mut self) -> anyhow::Result<()>;
}

/// Which overlay (if any) currently owns input, plus the one count the picker needs.
/// An open overlay swallows keys and the normal-scene bindings are suspended; the
/// precedence chain itself lives in [`dispatch_key`].
#[derive(Clone, Copy)]
pub(crate) struct ModalState {
    pub(crate) onboarding_open: bool,
    pub(crate) help_open: bool,
    pub(crate) version_popup: bool,
    pub(crate) theme_picker: Option<usize>,
    pub(crate) dashboard_open: bool,
    pub(crate) connection_open: bool,
    /// A disconnect is armed on the Sources panel, awaiting y/n.
    pub(crate) connection_confirm: bool,
    pub(crate) n_themes: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct FloorNav {
    pub(crate) n_floors: usize,
    pub(crate) current_floor: usize,
    pub(crate) in_transition: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyAction {
    None,
    Quit,
    TogglePause,
    ToggleHelp,
    CloseHelp,
    DismissVersionPopup,
    OpenThemePicker,
    /// The index is pre-clamped by the dispatch.
    ThemePreview(usize),
    ThemeCommit(usize),
    ThemeCancel,
    /// Already validated: in range, and no transition in flight.
    NavigateFloor(usize),
    ToggleAudioMute,
    /// `true` = up.
    AdjustVolume(bool),
    #[cfg_attr(
        all(not(debug_assertions), not(test)),
        expect(
            dead_code,
            reason = "only the debug-build `w` dispatch arm builds it; the apply arm stays unconditional for exhaustiveness"
        )
    )]
    ToggleWalkableDebug,
    ToggleDashboard,
    DashboardUp,
    DashboardDown,
    DashboardFoldLeft,
    DashboardFoldRight,
    DashboardFoldAll,
    DashboardJump,
    DashboardFocus,
    DashboardClose,
    /// Open/close the Sources panel.
    ToggleConnection,
    ConnectionUp,
    ConnectionDown,
    /// Connecting is immediate; disconnecting arms a confirm first, since it removes
    /// hooks and walks characters out.
    ConnectionToggle,
    ConnectionConfirm,
    ConnectionCancelConfirm,
    ConnectionClose,
    OnboardingUp,
    OnboardingDown,
    OnboardingToggle,
    OnboardingConfirm,
    OnboardingSkip,
    /// Ctrl-L: repaint the whole screen, images included.
    Redraw,
}

/// Opens the live gate only on `Ok`, matching [`crate::sources::connect`]'s flag rollback
/// — no shown-but-broken source survives a restart.
fn connect_source(
    config_path: &std::path::Path,
    connected: &crate::runtime::ConnectedSources,
    source_id: &str,
    display_name: &str,
) -> String {
    match crate::sources::connect(config_path, source_id) {
        Ok(outcome) => {
            connected.set(source_id, true);
            match outcome {
                crate::sources::ConnectOutcome::Installed(r) => {
                    connection::format_connect_result(&r, display_name)
                }
                crate::sources::ConnectOutcome::FlagOnly => {
                    format!("\u{2713} {display_name} connected")
                }
            }
        }
        Err(e) => connection::format_failure(
            connection::FailedOp::Connect,
            display_name,
            &format!("{e:#}"),
        ),
    }
}

/// The core reserves `Err` for the persist-failure abort — a runtime hide the next
/// restart reverts is a lie. A hook-removal failure is folded into the `Ok` outcome, so
/// the gate STILL closes.
fn disconnect_source(
    config_path: &std::path::Path,
    connected: &crate::runtime::ConnectedSources,
    source_id: &str,
    display_name: &str,
) -> String {
    match crate::sources::disconnect(config_path, source_id) {
        Ok(outcome) => {
            connected.set(source_id, false);
            match outcome {
                crate::sources::DisconnectOutcome::Uninstalled(r) => {
                    connection::format_disconnect_result(&r, display_name)
                }
                crate::sources::DisconnectOutcome::FlagOnly => {
                    format!("\u{2713} {display_name} disconnected")
                }
                crate::sources::DisconnectOutcome::HookRemovalFailed(e) => {
                    connection::format_failure(connection::FailedOp::HookRemoval, display_name, &e)
                }
            }
        }
        Err(e) => connection::format_failure(
            connection::FailedOp::Disconnect,
            display_name,
            &format!("{e:#}"),
        ),
    }
}

/// The source id rides along so the panel can put its selection — and the offered `t`
/// retry — on the row that actually failed.
#[derive(Debug)]
struct OnboardingFailure {
    source_id: String,
    line: String,
}

/// Reflect the onboarding apply's outcomes into the LIVE connected-set, and hand back one
/// presentable failure per failed row. `choices` and `outcomes` are index-aligned.
///
/// The RETURN is the surfacing half: in TUI mode the alternate screen owns the terminal,
/// so the warn-floor log is not a user surface.
fn reflect_onboarding_outcomes(
    connected: &crate::runtime::ConnectedSources,
    choices: &[(&'static str, bool)],
    outcomes: &[(String, crate::sources::AppliedChange)],
) -> Vec<OnboardingFailure> {
    use crate::sources::AppliedChange;
    let mut failures = Vec::new();
    for ((_, want), (id, oc)) in choices.iter().zip(outcomes) {
        match oc {
            AppliedChange::Connected => connected.set(id, true),
            AppliedChange::Disconnected => connected.set(id, false),
            AppliedChange::Failed(e) => {
                connected.set(id, false);
                // `Failed` covers all three operations: connect, disconnect (an UNCHECKED
                // row, which `freeze_for_skip` makes the common case), and the fold below.
                let op = if *want {
                    connection::FailedOp::Connect
                } else {
                    connection::FailedOp::Disconnect
                };
                tracing::warn!(source = %id, ?op, error = ?e, "onboarding: hook change failed");
                let name =
                    crate::install::target::by_source(id).map_or(id.as_str(), |t| t.display_name);
                // The fold: an otherwise SUCCESSFUL disconnect that left a residual, so it
                // presents as its own op rather than as a failed disconnect.
                let line = match e.strip_prefix(crate::sources::HOOK_REMOVAL_FAILED_PREFIX) {
                    Some(reason) => {
                        connection::format_failure(connection::FailedOp::HookRemoval, name, reason)
                    }
                    None => connection::format_failure(op, name, e),
                };
                failures.push(OnboardingFailure {
                    source_id: id.clone(),
                    line,
                });
            }
        }
    }
    failures
}

/// Open the Sources panel ON the first failed row and seed its result line, so the `t`
/// retry is one keystroke away on the right source.
fn surface_onboarding_failures(
    ui: &mut ui_state::UiState,
    connected: &crate::runtime::ConnectedSources,
    failures: Vec<OnboardingFailure>,
) {
    let Some(first) = failures.first() else {
        return;
    };
    let first_id = first.source_id.clone();
    let rows = connection::build_rows(&connected.snapshot(), &ui.read_conn_log());
    ui.open_connection(rows);
    ui.select_connection_source(&first_id);
    ui.connection.last_result = Some(
        failures
            .into_iter()
            .map(|f| f.line)
            .collect::<Vec<_>>()
            .join("  \u{b7}  "),
    );
}

pub(crate) fn is_quit_chord(code: KeyCode, mods: KeyModifiers) -> bool {
    matches!(
        (code, mods),
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

/// What pressing `t` on a Sources-panel row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToggleIntent {
    ArmConfirm,
    Connect,
    /// Absent CLI that was never connected — an inert "not detected" hint.
    Hint,
}

/// The load-bearing arm is `NoCli { connected: true }` → `ArmConfirm`: a source whose
/// CLI vanished is still disconnectable, since its hooks live in the config, not in the
/// missing binary.
fn toggle_intent(state: connection::ConnState) -> ToggleIntent {
    match state {
        connection::ConnState::Connected | connection::ConnState::NoCli { connected: true } => {
            ToggleIntent::ArmConfirm
        }
        connection::ConnState::Disconnected => ToggleIntent::Connect,
        connection::ConnState::NoCli { connected: false } => ToggleIntent::Hint,
    }
}

/// Modal precedence, highest first, is the body's early-return order.
pub(crate) fn dispatch_key(
    code: KeyCode,
    mods: KeyModifiers,
    modal: ModalState,
    floor: FloorNav,
) -> KeyAction {
    // Above every modal: a garbled screen is no less garbled under one.
    if (code, mods) == (KeyCode::Char('l'), KeyModifiers::CONTROL) {
        return KeyAction::Redraw;
    }
    if modal.onboarding_open {
        return match (code, mods) {
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => KeyAction::OnboardingUp,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => KeyAction::OnboardingDown,
            (KeyCode::Char(' '), _) => KeyAction::OnboardingToggle,
            (KeyCode::Enter, _) => KeyAction::OnboardingConfirm,
            (KeyCode::Esc, _) => KeyAction::OnboardingSkip,
            _ => KeyAction::None,
        };
    }
    if modal.help_open {
        return match (code, mods) {
            (KeyCode::Enter, _) | (KeyCode::Esc, _) | (KeyCode::Char('?'), _) => {
                KeyAction::CloseHelp
            }
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            _ => KeyAction::None,
        };
    }
    if modal.version_popup {
        return match (code, mods) {
            (KeyCode::Enter, _) => KeyAction::DismissVersionPopup,
            (KeyCode::Esc, _) => KeyAction::Quit,
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            _ => KeyAction::None,
        };
    }
    if modal.connection_open {
        if modal.connection_confirm {
            return match (code, mods) {
                _ if is_quit_chord(code, mods) => KeyAction::Quit,
                (KeyCode::Char('y'), _) => KeyAction::ConnectionConfirm,
                (KeyCode::Char('n'), _) | (KeyCode::Esc, _) => KeyAction::ConnectionCancelConfirm,
                _ => KeyAction::None,
            };
        }
        return match (code, mods) {
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            (KeyCode::Esc, _) | (KeyCode::Char('s'), _) => KeyAction::ConnectionClose,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => KeyAction::ConnectionUp,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => KeyAction::ConnectionDown,
            (KeyCode::Char('t'), _) => KeyAction::ConnectionToggle,
            _ => KeyAction::None,
        };
    }
    if modal.dashboard_open {
        return match (code, mods) {
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            (KeyCode::Esc, _) | (KeyCode::Tab, _) => KeyAction::DashboardClose,
            (KeyCode::Enter, _) => KeyAction::DashboardJump,
            (KeyCode::Char('f'), _) => KeyAction::DashboardFocus,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => KeyAction::DashboardUp,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => KeyAction::DashboardDown,
            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => KeyAction::DashboardFoldLeft,
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => KeyAction::DashboardFoldRight,
            (KeyCode::Char('z'), _) => KeyAction::DashboardFoldAll,
            _ => KeyAction::None,
        };
    }
    if let Some(idx) = modal.theme_picker {
        return match (code, mods) {
            // Safe to quit mid-preview: only a commit saves the theme.
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            (KeyCode::Up | KeyCode::Char('k'), _) => KeyAction::ThemePreview(idx.saturating_sub(1)),
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                KeyAction::ThemePreview((idx + 1).min(modal.n_themes.saturating_sub(1)))
            }
            (KeyCode::Enter, _) => KeyAction::ThemeCommit(idx),
            (KeyCode::Esc, _) => KeyAction::ThemeCancel,
            _ => KeyAction::None,
        };
    }
    if is_quit_chord(code, mods) || code == KeyCode::Esc {
        return KeyAction::Quit;
    }
    match code {
        KeyCode::Char('p') => KeyAction::TogglePause,
        KeyCode::Char('m') => KeyAction::ToggleAudioMute,
        KeyCode::Char('+') | KeyCode::Char('=') => KeyAction::AdjustVolume(true),
        KeyCode::Char('-') | KeyCode::Char('_') => KeyAction::AdjustVolume(false),
        KeyCode::Char('t') => KeyAction::OpenThemePicker,
        KeyCode::Char('?') => KeyAction::ToggleHelp,
        KeyCode::Tab => KeyAction::ToggleDashboard,
        KeyCode::Char('s') => KeyAction::ToggleConnection,
        #[cfg(debug_assertions)]
        KeyCode::Char('w') => KeyAction::ToggleWalkableDebug,
        KeyCode::PageUp | KeyCode::Up | KeyCode::Char('k') => {
            if floor.current_floor + 1 < floor.n_floors && !floor.in_transition {
                KeyAction::NavigateFloor(floor.current_floor + 1)
            } else {
                KeyAction::None
            }
        }
        KeyCode::PageDown | KeyCode::Down | KeyCode::Char('j') => {
            if floor.current_floor > 0 && !floor.in_transition {
                KeyAction::NavigateFloor(floor.current_floor - 1)
            } else {
                KeyAction::None
            }
        }
        _ => KeyAction::None,
    }
}

/// Everything an applied [`KeyAction`] may touch — a parameter object, so the arm list
/// takes one argument.
pub(crate) struct KeyCtx<'a, H: Host> {
    pub(crate) ui: &'a mut ui_state::UiState,
    pub(crate) host: &'a mut H,
    pub(crate) audio_ctl: &'a mut crate::audio::AudioController,
    pub(crate) config_path: &'a std::path::Path,
    pub(crate) connected: &'a crate::runtime::ConnectedSources,
    pub(crate) snapshot: &'a pixtuoid_core::state::SceneState,
    pub(crate) focus_roots: &'a (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    pub(crate) now: SystemTime,
    /// Injected because the real `crate::audio::respawn` opens an output device, so a
    /// test firing an audio arm would grab the machine's sound hardware.
    pub(crate) respawn: fn(&crate::audio::AudioHandle, f32),
}

/// Apply one decoded [`KeyAction`], returning whether it asked to QUIT — the single piece
/// of control flow the caller's event loop keeps. Paired with [`dispatch_key`], which
/// decodes; splitting them is what makes the arms reachable from a test at all, since
/// a painter's event loop needs a real terminal or window.
pub(crate) fn apply_key_action<H: Host>(action: KeyAction, cx: &mut KeyCtx<'_, H>) -> bool {
    match action {
        KeyAction::None => {}
        KeyAction::Quit => return true,
        KeyAction::TogglePause => {
            cx.ui.toggle_pause();
            // Unpause restores the user's own m-key state rather than clobbering it.
            cx.audio_ctl.set_paused(cx.ui.paused());
        }
        KeyAction::ToggleHelp => cx.ui.toggle_help(),
        KeyAction::CloseHelp => cx.ui.close_help(),
        KeyAction::DismissVersionPopup => cx.ui.dismiss_version_popup(),
        KeyAction::OpenThemePicker => cx.ui.open_theme_picker(),
        KeyAction::ThemePreview(i) => {
            cx.ui.preview_theme(i);
            cx.host.set_theme(theme::ALL_THEMES[i]);
        }
        KeyAction::ThemeCommit(i) => {
            cx.ui.commit_theme(i);
            let name = theme::ALL_THEMES[i].name;
            if let Err(e) = crate::config::save(cx.config_path, name) {
                tracing::warn!(error = ?e, "failed to persist theme");
            }
        }
        KeyAction::ThemeCancel => {
            let saved = cx.ui.cancel_theme();
            cx.host.set_theme(theme::ALL_THEMES[saved]);
        }
        KeyAction::NavigateFloor(target) => {
            cx.host.navigate_floor(target, cx.now);
        }
        KeyAction::ToggleAudioMute => {
            cx.audio_ctl.apply(
                crate::audio::AudioAction::ToggleMute,
                cx.ui.paused(),
                Instant::now(),
                cx.respawn,
            );
        }
        KeyAction::AdjustVolume(up) => {
            cx.audio_ctl.apply(
                crate::audio::AudioAction::Volume(up),
                cx.ui.paused(),
                Instant::now(),
                cx.respawn,
            );
        }
        KeyAction::ToggleWalkableDebug => {
            cx.host.toggle_walkable_debug();
        }
        KeyAction::ToggleDashboard => cx.ui.toggle_dashboard(cx.snapshot),
        KeyAction::DashboardClose => cx.ui.close_dashboard(),
        KeyAction::DashboardUp => cx.ui.dashboard_move(cx.snapshot, -1),
        KeyAction::DashboardDown => cx.ui.dashboard_move(cx.snapshot, 1),
        KeyAction::DashboardFoldLeft => cx.ui.dashboard_fold_left(cx.snapshot),
        KeyAction::DashboardFoldRight => cx.ui.dashboard_fold_right(cx.snapshot),
        KeyAction::DashboardFoldAll => cx.ui.dashboard_fold_all(cx.snapshot),
        KeyAction::DashboardJump => {
            if let Some(floor) = cx.ui.dashboard_jump(cx.snapshot) {
                cx.host.navigate_floor(floor, cx.now);
            }
        }
        KeyAction::DashboardFocus => {
            if let Some(slot) = cx
                .ui
                .dashboard_focus()
                .and_then(|id| cx.snapshot.agents.get(&id))
            {
                crate::focus::focus_slot(slot, cx.focus_roots);
            }
        }
        KeyAction::ToggleConnection => {
            if cx.ui.connection.open {
                cx.ui.close_connection();
            } else {
                // FS reads happen on open and after each toggle, never per frame.
                let rows = connection::build_rows(&cx.connected.snapshot(), &cx.ui.read_conn_log());
                cx.ui.open_connection(rows);
            }
        }
        KeyAction::ConnectionUp => cx.ui.connection_move(-1),
        KeyAction::ConnectionDown => cx.ui.connection_move(1),
        KeyAction::ConnectionToggle => {
            // Copy the fields out before any rebuild of `rows` (which would
            // invalidate a `&ConnectionRow` borrow).
            let action = cx
                .ui
                .connection
                .rows
                .get(cx.ui.connection.selected)
                .map(|r| {
                    (
                        r.state,
                        r.source_id,
                        r.display_name,
                        connection::no_action_hint(r),
                    )
                });
            if let Some((state, source_id, name, hint)) = action {
                match toggle_intent(state) {
                    ToggleIntent::ArmConfirm => {
                        cx.ui.connection.confirm = Some(cx.ui.connection.selected);
                    }
                    ToggleIntent::Connect => {
                        cx.ui.connection.last_result = Some(connect_source(
                            cx.config_path,
                            cx.connected,
                            source_id,
                            name,
                        ));
                        cx.ui.connection.rows = connection::build_rows(
                            &cx.connected.snapshot(),
                            &cx.ui.read_conn_log(),
                        );
                    }
                    ToggleIntent::Hint => {
                        cx.ui.connection.last_result = Some(hint);
                    }
                }
            }
        }
        KeyAction::ConnectionConfirm => {
            if let Some(idx) = cx.ui.connection.confirm {
                let action = cx
                    .ui
                    .connection
                    .rows
                    .get(idx)
                    .map(|r| (r.source_id, r.display_name));
                if let Some((source_id, name)) = action {
                    cx.ui.connection.last_result = Some(disconnect_source(
                        cx.config_path,
                        cx.connected,
                        source_id,
                        name,
                    ));
                    cx.ui.connection.rows =
                        connection::build_rows(&cx.connected.snapshot(), &cx.ui.read_conn_log());
                }
            }
            cx.ui.connection.confirm = None;
        }
        KeyAction::ConnectionCancelConfirm => cx.ui.cancel_connection_confirm(),
        KeyAction::ConnectionClose => cx.ui.close_connection(),
        KeyAction::OnboardingUp => cx.ui.onboarding_ui.move_up(),
        KeyAction::OnboardingDown => cx.ui.onboarding_ui.move_down(),
        KeyAction::OnboardingToggle => cx.ui.onboarding_ui.toggle_selected(),
        KeyAction::OnboardingConfirm => {
            // SCOPED to the detected sources, so an undetected source's flag is
            // never written.
            let choices = cx.ui.onboarding_ui.decisions();
            let outcomes = crate::sources::apply_choices(cx.config_path, &choices);
            let failed = reflect_onboarding_outcomes(cx.connected, &choices, &outcomes);
            cx.ui.close_onboarding();
            surface_onboarding_failures(cx.ui, cx.connected, failed);
        }
        KeyAction::OnboardingSkip => apply_onboarding_skip(cx),
        KeyAction::Redraw => {
            if let Err(e) = cx.host.redraw() {
                tracing::warn!(error = %e, "redraw failed");
            }
        }
    }
    false
}

/// Skip marks onboarding done WITHOUT changing any hooks: `skip_freeze` pins each detected
/// source to its REAL current state — live-gate connected OR already carrying installed
/// hooks (a pre-0.12 upgrader has hooks but no `[sources]` flag). The apply re-installs
/// those idempotently and leaves the rest disconnected, so `[sources]` becomes non-empty
/// (onboarding won't re-trigger) yet NO hooks are added or removed.
fn apply_onboarding_skip<H: Host>(cx: &mut KeyCtx<'_, H>) {
    let snap = cx.connected.snapshot();
    let ids: Vec<&'static str> = cx
        .ui
        .onboarding_ui
        .rows
        .iter()
        .map(|r| r.source_id)
        .collect();
    let freeze = crate::sources::skip_freeze(ids, &snap);
    let outcomes = crate::sources::apply_choices(cx.config_path, &freeze);
    // The freeze persists connected=true for a pre-0.12 upgrader's hooked sources, so the
    // in-process gate must open THIS session too, or their office stays empty until restart.
    let failed = reflect_onboarding_outcomes(cx.connected, &freeze, &outcomes);
    cx.ui.close_onboarding();
    surface_onboarding_failures(cx.ui, cx.connected, failed);
}

#[cfg(test)]
mod dispatch_tests {
    use super::{
        FloorNav, KeyAction, ModalState, connect_source, connection, disconnect_source,
        dispatch_key,
    };
    use crossterm::event::{KeyCode, KeyModifiers};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    fn modal() -> ModalState {
        ModalState {
            onboarding_open: false,
            help_open: false,
            version_popup: false,
            theme_picker: None,
            dashboard_open: false,
            connection_open: false,
            connection_confirm: false,
            n_themes: 6,
        }
    }

    fn nav() -> FloorNav {
        FloorNav {
            n_floors: 3,
            current_floor: 1,
            in_transition: false,
        }
    }

    #[test]
    fn toggle_intent_covers_the_four_arms() {
        use super::connection::ConnState;
        use super::{ToggleIntent, toggle_intent};
        assert_eq!(
            toggle_intent(ConnState::Connected),
            ToggleIntent::ArmConfirm
        );
        assert_eq!(
            toggle_intent(ConnState::Disconnected),
            ToggleIntent::Connect
        );
        assert_eq!(
            toggle_intent(ConnState::NoCli { connected: true }),
            ToggleIntent::ArmConfirm
        );
        assert_eq!(
            toggle_intent(ConnState::NoCli { connected: false }),
            ToggleIntent::Hint
        );
    }

    #[test]
    fn normal_quit_pause_picker_help() {
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, modal(), nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, modal(), nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, modal(), nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, modal(), nav()),
            KeyAction::TogglePause
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('m'), NONE, modal(), nav()),
            KeyAction::ToggleAudioMute
        );
        for up in ['+', '='] {
            assert_eq!(
                dispatch_key(KeyCode::Char(up), NONE, modal(), nav()),
                KeyAction::AdjustVolume(true)
            );
        }
        for down in ['-', '_'] {
            assert_eq!(
                dispatch_key(KeyCode::Char(down), NONE, modal(), nav()),
                KeyAction::AdjustVolume(false)
            );
        }
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, modal(), nav()),
            KeyAction::OpenThemePicker
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, modal(), nav()),
            KeyAction::ToggleHelp
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            dispatch_key(KeyCode::Char('w'), NONE, modal(), nav()),
            KeyAction::ToggleWalkableDebug
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('x'), NONE, modal(), nav()),
            KeyAction::None
        );
    }

    #[test]
    fn floor_nav_guards() {
        for code in [KeyCode::PageUp, KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(
                dispatch_key(code, NONE, modal(), nav()),
                KeyAction::NavigateFloor(2)
            );
        }
        for code in [KeyCode::PageDown, KeyCode::Down, KeyCode::Char('j')] {
            assert_eq!(
                dispatch_key(code, NONE, modal(), nav()),
                KeyAction::NavigateFloor(0)
            );
        }
        let top = FloorNav {
            current_floor: 2,
            ..nav()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, modal(), top),
            KeyAction::None
        );
        let bottom = FloorNav {
            current_floor: 0,
            ..nav()
        };
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, modal(), bottom),
            KeyAction::None
        );
        let mid_trans = FloorNav {
            in_transition: true,
            ..nav()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, modal(), mid_trans),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, modal(), mid_trans),
            KeyAction::None
        );
    }

    #[test]
    fn help_overlay_has_priority_and_dismisses() {
        let c = ModalState {
            help_open: true,
            version_popup: true,
            theme_picker: Some(2),
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, c, nav()),
            KeyAction::CloseHelp
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, c, nav()),
            KeyAction::CloseHelp
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, c, nav()),
            KeyAction::CloseHelp
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c, nav()), KeyAction::None);
    }

    #[test]
    fn onboarding_is_top_precedence_and_maps_its_keys() {
        let on = ModalState {
            onboarding_open: true,
            help_open: true,
            version_popup: true,
            connection_open: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, on, nav()),
            KeyAction::OnboardingUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, on, nav()),
            KeyAction::OnboardingUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, on, nav()),
            KeyAction::OnboardingDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, on, nav()),
            KeyAction::OnboardingDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char(' '), NONE, on, nav()),
            KeyAction::OnboardingToggle
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, on, nav()),
            KeyAction::OnboardingConfirm
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, on, nav()),
            KeyAction::OnboardingSkip
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, on, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('s'), NONE, on, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, on, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, on, nav()),
            KeyAction::None
        );
    }

    #[test]
    fn version_popup_enter_dismisses_esc_quits() {
        let c = ModalState {
            version_popup: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, c, nav()),
            KeyAction::DismissVersionPopup
        );
        assert_eq!(dispatch_key(KeyCode::Esc, NONE, c, nav()), KeyAction::Quit);
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c, nav()), KeyAction::None);
    }

    /// The popup is DISMISS-ONLY: a fixed body and a link, nothing to scroll.
    /// A scroll key bound here would promise rows that do not exist.
    #[test]
    fn the_version_popup_binds_no_scroll_key() {
        let c = ModalState {
            version_popup: true,
            ..modal()
        };
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::PageDown,
            KeyCode::PageUp,
        ] {
            assert_eq!(
                dispatch_key(code, NONE, c, nav()),
                KeyAction::None,
                "{code:?} must stay unbound while the version popup is up"
            );
        }
    }

    #[test]
    fn theme_picker_preview_commit_cancel_and_clamps() {
        let c = ModalState {
            theme_picker: Some(2),
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, c, nav()),
            KeyAction::ThemePreview(1)
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, c, nav()),
            KeyAction::ThemePreview(1)
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, c, nav()),
            KeyAction::ThemePreview(3)
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, c, nav()),
            KeyAction::ThemePreview(3)
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, c, nav()),
            KeyAction::ThemeCommit(2)
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, c, nav()),
            KeyAction::ThemeCancel
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, c, nav()),
            KeyAction::None
        );

        let lo = ModalState {
            theme_picker: Some(0),
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, lo, nav()),
            KeyAction::ThemePreview(0)
        );
        let hi = ModalState {
            theme_picker: Some(5),
            n_themes: 6,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, hi, nav()),
            KeyAction::ThemePreview(5)
        );
    }

    #[test]
    fn ctrl_l_redraws_under_any_modal() {
        let dashboard = ModalState {
            dashboard_open: true,
            ..modal()
        };
        let onboarding = ModalState {
            onboarding_open: true,
            ..modal()
        };
        for m in [modal(), dashboard, onboarding] {
            assert_eq!(
                dispatch_key(KeyCode::Char('l'), CTRL, m, nav()),
                KeyAction::Redraw
            );
        }
        assert_eq!(
            dispatch_key(KeyCode::Char('l'), NONE, dashboard, nav()),
            KeyAction::DashboardFoldRight
        );
    }

    #[test]
    fn tab_toggles_dashboard_from_normal_scene() {
        assert_eq!(
            dispatch_key(KeyCode::Tab, NONE, modal(), nav()),
            KeyAction::ToggleDashboard
        );
    }

    #[test]
    fn dashboard_tier_maps_nav_fold_jump_close() {
        let d = ModalState {
            dashboard_open: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, d, nav()),
            KeyAction::DashboardUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, d, nav()),
            KeyAction::DashboardUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, d, nav()),
            KeyAction::DashboardDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, d, nav()),
            KeyAction::DashboardDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Left, NONE, d, nav()),
            KeyAction::DashboardFoldLeft
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('h'), NONE, d, nav()),
            KeyAction::DashboardFoldLeft
        );
        assert_eq!(
            dispatch_key(KeyCode::Right, NONE, d, nav()),
            KeyAction::DashboardFoldRight
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('l'), NONE, d, nav()),
            KeyAction::DashboardFoldRight
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('z'), NONE, d, nav()),
            KeyAction::DashboardFoldAll
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, d, nav()),
            KeyAction::DashboardJump
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('f'), NONE, d, nav()),
            KeyAction::DashboardFocus,
            "f focuses the selected agent's terminal"
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, d, nav()),
            KeyAction::DashboardClose
        );
        assert_eq!(
            dispatch_key(KeyCode::Tab, NONE, d, nav()),
            KeyAction::DashboardClose
        );
    }

    #[test]
    fn dashboard_modal_passes_quit_chord_but_swallows_other_keys() {
        let d = ModalState {
            dashboard_open: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, d, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, d, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, d, nav()),
            KeyAction::None,
            "modal swallows pause"
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, d, nav()),
            KeyAction::None,
            "modal swallows theme picker"
        );
    }

    #[test]
    fn tab_swallowed_while_other_overlays_open() {
        let h = ModalState {
            help_open: true,
            ..modal()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, h, nav()), KeyAction::None);
        let v = ModalState {
            version_popup: true,
            ..modal()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, v, nav()), KeyAction::None);
        let p = ModalState {
            theme_picker: Some(0),
            ..modal()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, p, nav()), KeyAction::None);
    }

    #[test]
    fn s_opens_sources_panel_from_normal_scene() {
        assert_eq!(
            dispatch_key(KeyCode::Char('s'), NONE, modal(), nav()),
            KeyAction::ToggleConnection
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), NONE, modal(), nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, modal(), nav()),
            KeyAction::Quit
        );
    }

    #[test]
    fn connection_tier_maps_nav_toggle_close() {
        let s = ModalState {
            connection_open: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, s, nav()),
            KeyAction::ConnectionUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, s, nav()),
            KeyAction::ConnectionUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, s, nav()),
            KeyAction::ConnectionDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, s, nav()),
            KeyAction::ConnectionDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, s, nav()),
            KeyAction::ConnectionToggle
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('i'), NONE, s, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('u'), NONE, s, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, s, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('s'), NONE, s, nav()),
            KeyAction::ConnectionClose
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, s, nav()),
            KeyAction::ConnectionClose
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, s, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, s, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('y'), NONE, s, nav()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('n'), NONE, s, nav()),
            KeyAction::None
        );
    }

    #[test]
    fn connection_armed_tier_maps_yn_and_swallows_nav() {
        let s = ModalState {
            connection_open: true,
            connection_confirm: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Char('y'), NONE, s, nav()),
            KeyAction::ConnectionConfirm
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('n'), NONE, s, nav()),
            KeyAction::ConnectionCancelConfirm
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, s, nav()),
            KeyAction::ConnectionCancelConfirm
        );
        for k in [
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('i'),
            KeyCode::Char('u'),
        ] {
            assert_eq!(dispatch_key(k, NONE, s, nav()), KeyAction::None);
        }
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, s, nav()),
            KeyAction::Quit
        );
    }

    #[test]
    fn connection_precedence_help_version_win_and_connection_swallows_tab() {
        let h = ModalState {
            help_open: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), NONE, h, nav()),
            KeyAction::None
        );
        let v = ModalState {
            version_popup: true,
            ..modal()
        };
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), NONE, v, nav()),
            KeyAction::None
        );
        let s = ModalState {
            connection_open: true,
            ..modal()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, s, nav()), KeyAction::None);
    }

    #[test]
    fn connect_source_persists_then_flips_the_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        let connected = crate::runtime::ConnectedSources::default();

        let res = connect_source(&cfg, &connected, "antigravity", "Antigravity");
        assert!(res.contains("connected"), "result: {res}");
        assert!(connected.is_connected("antigravity"), "gate opened");
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("antigravity") && written.contains("true"),
            "the flag was persisted: {written}"
        );
    }

    #[test]
    fn disconnect_source_persists_then_closes_the_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        let connected = crate::runtime::ConnectedSources::new(
            std::iter::once("antigravity".to_string()).collect(),
        );

        let res = disconnect_source(&cfg, &connected, "antigravity", "Antigravity");
        assert!(res.contains("disconnected"), "result: {res}");
        assert!(!connected.is_connected("antigravity"), "gate closed");
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("antigravity") && written.contains("false"),
            "the flag was persisted: {written}"
        );
    }

    #[test]
    fn connect_source_aborts_without_flipping_the_gate_when_persist_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A regular file used as a directory component makes the config write's
        // create-parent-dir fail.
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let cfg = blocker.join("config.toml");
        let connected = crate::runtime::ConnectedSources::default();

        let res = connect_source(&cfg, &connected, "antigravity", "Antigravity");
        assert!(res.contains("failed"), "must report the failure: {res}");
        assert!(
            !connected.is_connected("antigravity"),
            "a failed persist must NOT open the gate (else restart re-evicts)"
        );
    }

    #[test]
    fn onboarding_outcomes_map_connected_disconnected_failed() {
        use crate::sources::AppliedChange;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> =
            vec![("antigravity", true), ("codex", false), ("cursor", true)];
        let outcomes = vec![
            ("antigravity".to_string(), AppliedChange::Connected),
            ("codex".to_string(), AppliedChange::Disconnected),
            ("cursor".to_string(), AppliedChange::Failed("boom".into())),
        ];
        super::reflect_onboarding_outcomes(&connected, &choices, &outcomes);
        assert!(connected.is_connected("antigravity"));
        assert!(!connected.is_connected("codex"));
        assert!(
            !connected.is_connected("cursor"),
            "a failed connect must NOT go live"
        );
    }

    #[test]
    fn a_failed_onboarding_connect_reports_the_reason_to_the_caller() {
        use crate::sources::AppliedChange;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> = vec![("cursor", true), ("antigravity", true)];
        let outcomes = vec![
            (
                "cursor".to_string(),
                AppliedChange::Failed("settings is valid JSON but not an object".into()),
            ),
            ("antigravity".to_string(), AppliedChange::Connected),
        ];
        let failures = super::reflect_onboarding_outcomes(&connected, &choices, &outcomes);
        assert_eq!(
            failures.len(),
            1,
            "only the failed row reports: {failures:?}"
        );
        let line = &failures[0].line;
        assert!(
            line.contains("settings is valid JSON but not an object"),
            "the REASON must survive — it is the whole point: {line}"
        );
        let display_name = crate::install::target::by_source("cursor")
            .expect("cursor is a target-bearing source")
            .display_name;
        assert!(
            line.contains(display_name),
            "the row must be named the way every other surface names it: {line}"
        );
        assert_eq!(
            line,
            &connection::format_failure(
                connection::FailedOp::Connect,
                display_name,
                "settings is valid JSON but not an object",
            ),
            "the panel's own wording, from the panel's own formatter: {line}"
        );
        assert_eq!(
            failures[0].source_id, "cursor",
            "the failure carries the row it belongs to, so the panel can select it"
        );
    }

    #[test]
    fn an_onboarding_failure_names_the_operation_that_actually_failed() {
        use crate::sources::AppliedChange;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> = vec![("cursor", false), ("openclaw", false)];
        let outcomes = vec![
            (
                "cursor".to_string(),
                AppliedChange::Failed("config is not writable".into()),
            ),
            (
                "openclaw".to_string(),
                AppliedChange::Failed(format!(
                    "{}openclaw.json is JSON5, not strict JSON",
                    crate::sources::HOOK_REMOVAL_FAILED_PREFIX
                )),
            ),
        ];
        let failures = super::reflect_onboarding_outcomes(&connected, &choices, &outcomes);
        assert_eq!(failures.len(), 2, "both rows report: {failures:?}");

        let cursor_name = crate::install::target::by_source("cursor")
            .expect("cursor is a target-bearing source")
            .display_name;
        let unchecked = &failures[0].line;
        assert_eq!(
            unchecked,
            &connection::format_failure(
                connection::FailedOp::Disconnect,
                cursor_name,
                "config is not writable",
            ),
            "an unchecked row's failure is a DISCONNECT failure: {unchecked}"
        );

        let openclaw_name = crate::install::target::by_source("openclaw")
            .expect("openclaw is a target-bearing source")
            .display_name;
        let folded = &failures[1].line;
        assert_eq!(
            folded,
            &connection::format_failure(
                connection::FailedOp::HookRemoval,
                openclaw_name,
                "openclaw.json is JSON5, not strict JSON",
            ),
            "a folded hook-removal failure keeps the panel's wording: {folded}"
        );
        assert!(
            !folded.contains(crate::sources::HOOK_REMOVAL_FAILED_PREFIX),
            "the machine token is stripped once the wording carries it: {folded}"
        );
    }

    #[test]
    fn onboarding_failures_open_the_sources_panel_on_the_failed_row() {
        let connected = crate::runtime::ConnectedSources::default();
        let mut ui = crate::panels::ui_state::UiState::new(
            pixtuoid_scene::theme::ALL_THEMES[0],
            crate::panels::welcome::WelcomeUi::from_detected(&[]),
            false,
            std::path::PathBuf::from("/tmp/sock"),
            None,
            crate::doctor::DriftSeen::default(),
        );
        assert!(!ui.modal().connection_open, "panel starts closed");

        super::surface_onboarding_failures(&mut ui, &connected, Vec::new());
        assert!(
            !ui.modal().connection_open,
            "a clean apply must not pop the panel"
        );

        super::surface_onboarding_failures(
            &mut ui,
            &connected,
            vec![super::OnboardingFailure {
                source_id: "cursor".into(),
                line: "Cursor: connect failed \u{2014} boom".into(),
            }],
        );
        assert!(ui.modal().connection_open, "a failure opens the panel");
        assert_eq!(
            ui.connection.last_result.as_deref(),
            Some("Cursor: connect failed \u{2014} boom"),
            "the reason rides the panel's own result line"
        );
        let selected = ui.connection.rows[ui.connection.selected].source_id;
        assert_eq!(
            selected, "cursor",
            "the panel must open ON the failed row — `t` acts on the SELECTED one"
        );
        assert_ne!(
            ui.connection.selected, 0,
            "cursor is not the first registry row, so this could not pass by default"
        );
    }

    #[test]
    fn onboarding_skip_reflects_its_freeze_into_the_live_gate() {
        use crate::sources::AppliedChange;
        let connected = crate::runtime::ConnectedSources::default();
        assert!(!connected.is_connected("antigravity"), "gate starts empty");
        let freeze: Vec<(&'static str, bool)> = vec![("antigravity", true)];
        let outcomes = vec![("antigravity".to_string(), AppliedChange::Connected)];
        super::reflect_onboarding_outcomes(&connected, &freeze, &outcomes);
        assert!(
            connected.is_connected("antigravity"),
            "skip must open the live gate for a frozen-connected source"
        );
    }
}
