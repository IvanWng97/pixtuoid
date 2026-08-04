pub mod connection;
pub mod dashboard;
pub mod hit_test;
pub mod renderer;
pub mod tui_renderer;
mod ui_state;
pub mod welcome;
pub mod widgets;

use std::io::{stdout, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use tui_renderer::TuiRenderer;

use crate::runtime::SceneRx;
use pixtuoid_scene::{embedded_pack, floor, pet, theme};

/// Which overlay (if any) currently owns input, plus the one count the picker needs.
/// An open overlay swallows keys and the normal-scene bindings are suspended; the
/// precedence chain itself lives in [`dispatch_key`].
#[derive(Clone, Copy)]
struct ModalState {
    onboarding_open: bool,
    help_open: bool,
    version_popup: bool,
    theme_picker: Option<usize>,
    dashboard_open: bool,
    connection_open: bool,
    /// A disconnect is armed on the Sources panel, awaiting y/n.
    connection_confirm: bool,
    n_themes: usize,
}

#[derive(Clone, Copy)]
struct FloorNav {
    n_floors: usize,
    current_floor: usize,
    in_transition: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
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
    /// `true` = up. Volume-up from muted also unmutes.
    AdjustVolume(bool),
    /// The `w` dispatch arm is `#[cfg(debug_assertions)]`-gated, so in release this
    /// variant is never constructed; the `run_tui` match arm stays unconditional for
    /// exhaustiveness.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
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
    /// Open/close the Sources panel — the variant and module keep the historical
    /// `Connection` name.
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
}

fn focus_clicked_agent<B: ratatui::backend::Backend<Error: Send + Sync + 'static>>(
    renderer: &mut TuiRenderer<B>,
    scene_rx: &SceneRx,
    focus_roots: &(Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    col: u16,
    row: u16,
    now: SystemTime,
) -> bool {
    let snap = scene_rx.borrow().clone();
    // Project to the VISIBLE floor first: hit_test_agent_at → character_anchor reads
    // floor-local desk indices.
    let floor_scene = floor::project_floor_scene(&snap, renderer.current_floor());
    let hit = renderer.hit_test_agent_at(&floor_scene, now, col, row);
    if let Some(slot) = hit.and_then(|id| snap.agents.get(&id)) {
        crate::focus::focus_slot(slot, focus_roots);
        true
    } else {
        false
    }
}

/// The core persists the flag FIRST and rolls it back if the install fails, so on `Err`
/// the live gate was never opened — no shown-but-broken source survives a restart.
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

/// Reflect the onboarding apply's outcomes into the LIVE connected-set, and hand back
/// one presentable failure per failed row.
///
/// `choices` and `outcomes` are index-aligned. `NoOp` means "already in the DESIRED
/// state — nothing written", so it sets the gate to the desired flag rather than
/// hardcoding it closed: a NoOp for a CHECKED row must leave the gate OPEN, else an
/// already-connected source the user just confirmed loses its live agents.
///
/// The `FailedOp` branches because `Failed` covers all three operations: connect,
/// disconnect (an UNCHECKED row, which `freeze_for_skip` makes the common case) and the
/// `HOOK_REMOVAL_FAILED_PREFIX` fold — an otherwise SUCCESSFUL disconnect with a
/// residual.
///
/// The RETURN is the surfacing half: in TUI mode the alternate screen owns the terminal,
/// so the warn-floor log is not a user surface.
fn reflect_onboarding_outcomes(
    connected: &crate::runtime::ConnectedSources,
    choices: &[(&'static str, bool)],
    outcomes: &[(String, crate::sources::ChangeOutcome)],
) -> Vec<OnboardingFailure> {
    use crate::sources::ChangeOutcome;
    let mut failures = Vec::new();
    for ((_, want), (id, oc)) in choices.iter().zip(outcomes) {
        match oc {
            ChangeOutcome::Connected => connected.set(id, true),
            ChangeOutcome::Disconnected => connected.set(id, false),
            ChangeOutcome::NoOp => connected.set(id, *want),
            ChangeOutcome::Failed(e) => {
                connected.set(id, false);
                let (op, verb) = if *want {
                    (connection::FailedOp::Connect, "connect")
                } else {
                    (connection::FailedOp::Disconnect, "disconnect")
                };
                tracing::warn!("onboarding: {id} failed to {verb}: {e}");
                let name =
                    crate::install::target::by_source(id).map_or(id.as_str(), |t| t.display_name);
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
/// retry is one keystroke away on the right source. The explicit selection is
/// load-bearing: `open_connection` alone keeps the PREVIOUS index — 0 on a fresh
/// `UiState` — so the offered `t` would act on whatever sorts first.
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

fn is_quit_chord(code: KeyCode, mods: KeyModifiers) -> bool {
    matches!(
        (code, mods),
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

/// Windows delivers Press AND Release per keystroke, so without this guard every key
/// double-fires there — `p` would pause then instantly unpause. Inert on Unix.
fn should_dispatch_key(kind: KeyEventKind) -> bool {
    kind == KeyEventKind::Press
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

/// The per-floor desk-capacity sweep, memoized on its own inputs.
///
/// `floor_capacity` runs a FULL `Layout::compute_with_seed` — walkable-mask stamp plus
/// coarse BFS, quadratic in buffer area — once per floor, and keeps only
/// `home_desks.len()`. It is a pure function of `(buf_w, buf_h, desk_cap)` and the
/// publish is a monotone `fetch_max`, so a repeat with identical inputs could only
/// rewrite the same values.
struct FloorCapacitySweep {
    last: Option<(u16, u16, Option<usize>)>,
}

impl FloorCapacitySweep {
    fn new() -> Self {
        Self { last: None }
    }

    /// Returns whether it actually recomputed (`false` = served from the memo).
    fn publish(
        &mut self,
        buf_w: u16,
        buf_h: u16,
        desk_cap: Option<usize>,
        caps: &[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS],
    ) -> bool {
        if self.last == Some((buf_w, buf_h, desk_cap)) {
            return false;
        }
        self.last = Some((buf_w, buf_h, desk_cap));
        for (floor_idx, cap_slot) in caps.iter().enumerate() {
            let seed = pixtuoid_scene::floor::floor_seed(floor_idx);
            let mut capacity = pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed);
            if let Some(cap) = desk_cap {
                capacity = capacity.min(cap);
            }
            if capacity > 0 {
                cap_slot.fetch_max(capacity, std::sync::atomic::Ordering::Relaxed);
            }
        }
        true
    }
}

/// Modal precedence, highest first: onboarding > help > version popup > connection >
/// dashboard > theme picker > normal scene. The body's early returns are that chain's
/// single source of truth.
fn dispatch_key(
    code: KeyCode,
    mods: KeyModifiers,
    modal: ModalState,
    floor: FloorNav,
) -> KeyAction {
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
            // Safe to quit mid-preview: the run_tui quit arm reverts the previewed
            // theme before breaking.
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

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Term> {
    // On the WinAPI fallback (no VT), crossterm maps Color::Rgb to console attribute 0
    // and the office renders black-on-black invisible. Gate, don't degrade.
    #[cfg(windows)]
    if !crossterm::ansi_support::supports_ansi() {
        anyhow::bail!(
            "pixtuoid needs a VT-capable terminal — use Windows Terminal \
             (or Windows 10 1703+ with VT processing enabled)"
        );
    }
    enable_raw_mode()?;
    let mut out = stdout();
    // Mouse capture drives the hover tooltip: terminals emit MouseEventKind::Moved on
    // cursor motion only while it is on.
    //
    // Keep setup ATOMIC — a failure after raw mode is on must roll the terminal all the
    // way back, else the error path strands the user's shell in raw mode (no echo)
    // and/or the alt screen. `Terminal::new`'s `.size()` query can fail too.
    if let Err(e) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
        let _ = unwind_terminal_modes(&mut out, disable_raw_mode);
        return Err(e.into());
    }
    Terminal::new(CrosstermBackend::new(out)).map_err(|e| {
        let mut out = stdout();
        let _ = unwind_terminal_modes(&mut out, disable_raw_mode);
        e.into()
    })
}

/// THE terminal-mode unwind: the ONE definition of the order every exit path takes.
/// `pub` for the panic hook in `crash.rs`, a module of the BIN crate.
///
/// Every step runs even when an earlier one fails, and the FIRST error is returned: a
/// `?` after the escape-sequence write would skip `disable_raw` exactly when it is
/// needed most and strand the user's shell echo-less.
pub fn unwind_terminal_modes<W: std::io::Write>(
    out: &mut W,
    disable_raw: impl FnOnce() -> std::io::Result<()>,
) -> Result<()> {
    // DisableMouseCapture must run while raw mode is still ON: on Windows it restores
    // the input mode snapshotted at Enable time (raw-era), so running it after
    // disable_raw_mode re-raws the console and leaves the user's shell echo-less.
    let seq = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
    let raw = disable_raw();
    seq?;
    raw?;
    Ok(())
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    let modes = unwind_terminal_modes(term.backend_mut(), disable_raw_mode);
    // Unconditional: a failed mode restore must not ALSO leave the cursor hidden.
    let cursor = term.show_cursor();
    modes?;
    cursor?;
    Ok(())
}

/// Persists the current version so the popup shows at most once per upgrade regardless
/// of how the run exits. Re-loads the config for `last_seen_version` only — any config
/// warning was already surfaced by `main`'s pre-altscreen pass.
fn resolve_version_popup(config_path: &std::path::Path) -> bool {
    let current_ver = env!("CARGO_PKG_VERSION");
    let cfg = crate::config::load(config_path, &mut Vec::new());
    let decision = crate::version::boot_decision(current_ver, cfg.last_seen_version.as_deref());
    if decision.should_persist {
        if let Err(e) = crate::config::save_version(config_path, current_ver) {
            tracing::warn!("failed to persist version: {e}");
        }
    }
    decision.should_show_popup
}

pub(crate) struct TuiSession {
    pub scene_rx: SceneRx,
    pub pack_dir: Option<std::path::PathBuf>,
    pub floor_caps: Arc<[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS]>,
    pub theme: &'static theme::Theme,
    pub config_path: std::path::PathBuf,
    pub desk_cap: Option<usize>,
    pub pets: Vec<pet::Pet>,
    pub source_health:
        tokio::sync::watch::Receiver<Vec<pixtuoid_core::source::manager::SourceDeath>>,
    /// The hook socket (Unix) / named pipe (Windows) the daemon bound.
    pub socket_path: std::path::PathBuf,
    /// The Sources panel's mutation seam: a toggle calls `connected.set(src, on)`, which
    /// the reducer task's reconciler observes (gate + graceful evict).
    pub connected: crate::runtime::ConnectedSources,
    /// The warn-floor log, throttle-scanned for decode-drift breadcrumbs to drive the
    /// footer nudge. `None` = no surfacing.
    pub log_path: Option<std::path::PathBuf>,
    /// `muted` seeds the m-toggle; `volume` the boot and the lazy spawn.
    pub audio_cfg: crate::config::AudioConfig,
    /// Focus-jump pid point-query roots: (CC projects root, Codex sessions root).
    pub focus_roots: (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    pub first_run: bool,
}

/// Whether a left-click at `(col, row)` landed on the wall's star/repo link,
/// given the terminal's `(cols, rows)`.
///
/// Callers MUST gate this on `renderer.cached_layout().is_some()` — the wall
/// display only paints with a layout, so an ungated hit phantom-launches a
/// browser on a too-small frame or mid floor-slide.
///
/// Note the asymmetry with [`version_popup_url_clicked`]: this hit-tests the
/// SCENE rect (footer excluded), that one the full terminal bounds.
fn star_clicked(col: u16, row: u16, term: (u16, u16)) -> bool {
    let scene = renderer::scene_rect(ratatui::layout::Rect::new(0, 0, term.0, term.1));
    widgets::star_hit_rect(scene)
        .is_some_and(|s| s.contains(ratatui::layout::Position { x: col, y: row }))
}

/// Whether a left-click at `(col, row)` landed on the version popup's URL,
/// given the terminal's `(cols, rows)`.
///
/// `scale` is the PAINTER's own last frame-scale, so the hit geometry matches
/// what was actually painted rather than the popup's resting size — the popup
/// is clickable mid-animation.
fn version_popup_url_clicked(col: u16, row: u16, scale: f32, term: (u16, u16)) -> bool {
    let bounds = ratatui::layout::Rect::new(0, 0, term.0, term.1);
    let notes = crate::version::release_notes(env!("CARGO_PKG_VERSION")).unwrap_or(&[]);
    widgets::version_popup_url_rect(notes, bounds, scale)
        .is_some_and(|rect| rect.contains(ratatui::layout::Position { x: col, y: row }))
}

/// Everything an applied [`KeyAction`] may touch: three `&mut` surfaces plus
/// the read-only context. A parameter object, not an abstraction — it exists so
/// the arm list takes one argument instead of nine.
struct KeyCtx<'a, B: ratatui::backend::Backend<Error: Send + Sync + 'static>> {
    ui: &'a mut ui_state::UiState,
    renderer: &'a mut TuiRenderer<B>,
    audio_ctl: &'a mut crate::audio::AudioController,
    config_path: &'a std::path::Path,
    connected: &'a crate::runtime::ConnectedSources,
    snapshot: &'a pixtuoid_core::state::SceneState,
    focus_roots: &'a (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    now: SystemTime,
    /// Injected for the same reason `AudioController::apply` takes it: the real
    /// one opens an output device, so a test firing an audio arm would grab the
    /// machine's sound hardware.
    respawn: fn(&crate::audio::AudioHandle, f32),
}

/// Apply one decoded [`KeyAction`], returning whether it asked to QUIT — the
/// single piece of control flow the caller's event loop keeps.
///
/// Paired with [`dispatch_key`], which decodes; this applies. Splitting them
/// is what makes the arms reachable from a test at all — `run_tui` needs a
/// real terminal.
fn apply_key_action<B: ratatui::backend::Backend<Error: Send + Sync + 'static>>(
    action: KeyAction,
    cx: &mut KeyCtx<'_, B>,
) -> bool {
    match action {
        KeyAction::None => {}
        KeyAction::Quit => return true,
        KeyAction::TogglePause => {
            cx.ui.toggle_pause();
            // Unpause restores the user's own m-key state rather than
            // clobbering it.
            cx.audio_ctl.set_paused(cx.ui.paused());
        }
        KeyAction::ToggleHelp => cx.ui.toggle_help(),
        KeyAction::CloseHelp => cx.ui.close_help(),
        KeyAction::DismissVersionPopup => cx.ui.dismiss_version_popup(),
        KeyAction::OpenThemePicker => cx.ui.open_theme_picker(),
        KeyAction::ThemePreview(i) => {
            cx.ui.preview_theme(i);
            cx.renderer.set_theme(theme::ALL_THEMES[i]);
        }
        KeyAction::ThemeCommit(i) => {
            cx.ui.commit_theme(i);
            let name = theme::ALL_THEMES[i].name;
            if let Err(e) = crate::config::save(cx.config_path, name) {
                tracing::warn!("failed to persist theme: {e}");
            }
        }
        KeyAction::ThemeCancel => {
            let saved = cx.ui.cancel_theme();
            cx.renderer.set_theme(theme::ALL_THEMES[saved]);
        }
        KeyAction::NavigateFloor(target) => {
            cx.renderer.navigate_floor(target, cx.now);
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
            let on = cx.renderer.debug_walkable();
            cx.renderer.set_debug_walkable(!on);
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
                cx.renderer.navigate_floor(floor, cx.now);
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
                // FS reads happen on open and after each toggle, never per
                // frame.
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
        KeyAction::OnboardingSkip => {
            // Skip marks onboarding done WITHOUT changing any hooks: freeze each
            // detected source to its REAL current state — live-gate connected OR
            // already carrying installed hooks (a pre-0.12 upgrader has hooks but
            // no `[sources]` flag). The apply below re-installs those idempotently
            // and leaves the rest disconnected, so `[sources]` becomes non-empty —
            // onboarding won't re-trigger — yet NO hooks are added or removed.
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
            // The freeze persists connected=true for a pre-0.12 upgrader's hooked
            // sources, so the in-process gate must open THIS session too — else
            // their office stays empty until the next restart re-seeds it from the
            // flags.
            let failed = reflect_onboarding_outcomes(cx.connected, &freeze, &outcomes);
            cx.ui.close_onboarding();
            surface_onboarding_failures(cx.ui, cx.connected, failed);
        }
    }
    false
}

pub(crate) async fn run_tui(session: TuiSession) -> Result<()> {
    let TuiSession {
        mut scene_rx,
        pack_dir,
        floor_caps,
        theme,
        config_path,
        desk_cap,
        pets,
        mut source_health,
        socket_path,
        connected,
        log_path,
        focus_roots,
        first_run,
        audio_cfg,
    } = session;
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term, theme, pets);
    // The controller OWNS the audio device thread. Built HERE, after the pack-load `?`
    // above, so a bad --pack-dir never leaves a spawned thread un-joined; and it is a
    // `run_tui` local, so EVERY exit below (q / Ctrl-C / terminate / error) drops it and
    // joins the device thread before the process exits — no manual shutdown call.
    let mut audio_ctl =
        crate::audio::AudioController::new(audio_cfg.muted, audio_cfg.volume, config_path.clone());
    renderer.set_audio(audio_ctl.handle().clone());
    // With no agent CLIs detected there is nothing to connect, so the overlay stays
    // closed and the office shows normally.
    let detected_clis = if first_run {
        crate::sources::detect()
    } else {
        Vec::new()
    };
    let onboarding_ui = welcome::WelcomeUi::from_detected(&detected_clis);

    // The version popup yields to onboarding, but still STAMPS `last_seen_version` so
    // it won't pop later. Gating on the overlay SHOWING rather than on bare `first_run`
    // is load-bearing: `first_run` stays true forever for a no-CLI user, which would
    // mute the popup for good.
    let version_popup = if !onboarding_ui.is_empty() {
        let _ = resolve_version_popup(&config_path);
        false
    } else {
        resolve_version_popup(&config_path)
    };
    let mut ui = ui_state::UiState::new(theme, onboarding_ui, version_popup, socket_path, log_path);
    let mut last_layout_sig: Option<(u16, u16)> = None;
    let mut cap_sweep = FloorCapacitySweep::new();

    const FRAME_TICK_MS: u64 = 33;
    let tick = Duration::from_millis(FRAME_TICK_MS);
    let result: Result<()> = (async {
        // An EXTERNAL SIGINT/SIGTERM would otherwise hit the default disposition and
        // kill the process mid-altscreen with mouse reporting on, leaving the shell
        // unusable until `reset`. Pinned ONCE outside the loop — a per-iteration
        // `ctrl_c()` drops the subscription mid-gap — and boxed so a registration
        // FAILURE can disarm the arm by swapping in a pending future, since a resolved
        // future must never be polled again.
        let mut ctrl_c: std::pin::Pin<
            Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>,
        > = Box::pin(tokio::signal::ctrl_c());
        #[cfg(unix)]
        let terminate = {
            let sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
            async move {
                match sig {
                    Ok(mut s) => {
                        if s.recv().await.is_none() {
                            // Stream closed without a signal — never quit on that.
                            std::future::pending::<()>().await;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            %e,
                            "SIGTERM handler registration failed — an external \
                             SIGTERM will not restore the terminal"
                        );
                        std::future::pending::<()>().await;
                    }
                }
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        tokio::pin!(terminate);
        loop {
            let now = ui.now();
            let snapshot = scene_rx.borrow_and_update().clone();
            renderer.evict_missing(&snapshot);
            let sig = (renderer.buf().width(), renderer.buf().height());
            if last_layout_sig != Some(sig) {
                renderer.invalidate_routes();
                renderer.cancel_transition();
                last_layout_sig = Some(sig);
            }
            let health = source_health.borrow_and_update().clone();
            ui.build_frames(now, &snapshot, &health)
                .apply_to(&mut renderer, now);
            let audio_now = std::time::Instant::now();
            audio_ctl.tick(audio_now);
            renderer.set_volume_flash(audio_ctl.volume_flash(audio_now));
            renderer.render(&snapshot, &pack, now)?;

            // The sweep's `fetch_max` keeps capacity monotone: a shrink would otherwise
            // shift the cumulative offsets and remap floor-1+ agents onto the wrong
            // desks. Agents past the current layout's capacity go invisible but stay
            // alive, and reappear when the terminal grows back.
            if let Some(layout) = renderer.cached_layout() {
                cap_sweep.publish(layout.buf_w, layout.buf_h, desk_cap, &floor_caps);
            }

            let start = Instant::now();
            let mut polled = event::poll(tick)?;
            let mut quit = false;
            while polled {
                match event::read()? {
                    Event::Key(k) if should_dispatch_key(k.kind) => {
                        let floor = FloorNav {
                            n_floors: pixtuoid_scene::floor::num_floors(&snapshot),
                            current_floor: renderer.current_floor(),
                            in_transition: renderer.transition().is_some(),
                        };
                        let action = dispatch_key(k.code, k.modifiers, ui.modal(), floor);
                        quit |= apply_key_action(
                            action,
                            &mut KeyCtx {
                                ui: &mut ui,
                                renderer: &mut renderer,
                                audio_ctl: &mut audio_ctl,
                                config_path: &config_path,
                                connected: &connected,
                                snapshot: &snapshot,
                                focus_roots: &focus_roots,
                                now,
                                respawn: crate::audio::respawn,
                            },
                        );
                    }
                    Event::Mouse(_) if ui.onboarding_open() => {
                        // Modal for the mouse too: swallow every event so nothing leaks
                        // to the scene behind the overlay.
                    }
                    Event::Mouse(m) if ui.help_open() => {
                        // Every mouse event is swallowed so nothing leaks to the scene
                        // behind it (a coffee-machine / branding click launches a
                        // browser). Placed before the popup guard so help wins even mid
                        // popup-dismiss animation.
                        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                            ui.close_help();
                        }
                    }
                    Event::Mouse(m) if renderer.last_popup_scale() > 0.0 => {
                        // Only the URL link is clickable while the popup is animating
                        // or visible. The painter's own frame-scale is used so the click
                        // geometry matches what was actually painted.
                        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                            && crossterm::terminal::size().is_ok_and(|t| {
                                version_popup_url_clicked(
                                    m.column,
                                    m.row,
                                    renderer.last_popup_scale(),
                                    t,
                                )
                            })
                        {
                            let _ = open::that(widgets::VERSION_POPUP_URL);
                        }
                    }
                    Event::Mouse(_)
                        if ui.theme_picker.is_some() || ui.dashboard.open || ui.connection.open =>
                    {
                        // These paint centered over the scene, so a click on an exposed
                        // edge must not fall through and launch a browser. Inert by
                        // design: they have explicit close keys (Tab / s / t / Esc), so
                        // a click does NOT dismiss them.
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                            let on_star = renderer.cached_layout().is_some()
                                && crossterm::terminal::size()
                                    .is_ok_and(|t| star_clicked(m.column, m.row, t));
                            if on_star {
                                let _ = open::that(widgets::REPO_URL);
                            } else if focus_clicked_agent(
                                &mut renderer,
                                &scene_rx,
                                &focus_roots,
                                m.column,
                                m.row,
                                now,
                            ) {
                                // Empty on purpose: the click was consumed. An agent
                                // wins over the coffee Easter egg and the pet, matching
                                // the hover ladder in renderer.rs.
                            } else if renderer.cached_layout().is_some_and(|layout| {
                                renderer::hit_test_coffee_machine(layout, m.column, m.row)
                            }) {
                                let _ = open::that("https://buymeacoffee.com/IvanWng97");
                            } else if let Some(pixtuoid_scene::pet::PetFrame {
                                pos: pet_pos,
                                anim,
                                kind,
                            }) = renderer.cached_pet_pos()
                            {
                                if renderer.active_pet_ref().is_none_or(|p| !p.is_active(now))
                                    && renderer::hit_test_pet(kind, pet_pos, anim, m.column, m.row)
                                {
                                    renderer.set_active_pet(Some(renderer::PetState {
                                        petted_at: now,
                                        pet_pos,
                                        kind,
                                        floor_idx: renderer.current_floor(),
                                    }));
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                polled = event::poll(Duration::from_millis(0))?;
            }
            if quit {
                if ui.theme_picker.is_some() {
                    renderer.set_theme(theme::ALL_THEMES[ui.saved_theme_idx]);
                }
                break;
            }
            // The frame-pacing sleep doubles as the signal-listen window: the crossterm
            // poll above is synchronous, so this is the loop's only await point.
            let rem = tick.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
            tokio::select! {
                _ = tokio::time::sleep(rem) => {}
                res = &mut ctrl_c => match res {
                    Ok(()) => break,
                    Err(e) => {
                        tracing::error!(
                            %e,
                            "SIGINT handler registration failed — an external \
                             Ctrl-C will not restore the terminal"
                        );
                        ctrl_c = Box::pin(std::future::pending());
                    }
                },
                _ = &mut terminate => break,
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
mod capacity_sweep_tests {
    use super::FloorCapacitySweep;
    use pixtuoid_core::state::MAX_FLOORS;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn caps() -> [AtomicUsize; MAX_FLOORS] {
        std::array::from_fn(|_| AtomicUsize::new(0))
    }

    // A 192x80 terminal, i.e. a 192x158 buffer.
    const W: u16 = 192;
    const H: u16 = 158;

    #[test]
    fn a_repeat_frame_serves_the_memo_instead_of_recomputing() {
        let caps = caps();
        let mut sweep = FloorCapacitySweep::new();
        assert!(sweep.publish(W, H, None, &caps), "first frame computes");
        let published: Vec<usize> = caps.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        assert!(
            !sweep.publish(W, H, None, &caps),
            "an unchanged frame must skip the whole 10-floor layout sweep"
        );
        let after: Vec<usize> = caps.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        assert_eq!(
            published, after,
            "the memo hit must publish the same values"
        );
    }

    #[test]
    fn a_resize_or_a_new_cap_recomputes() {
        let caps = caps();
        let mut sweep = FloorCapacitySweep::new();
        sweep.publish(W, H, None, &caps);
        assert!(sweep.publish(W, H - 2, None, &caps), "a resize recomputes");
        assert!(
            sweep.publish(W, H - 2, Some(4), &caps),
            "a different desk cap recomputes"
        );
    }

    #[test]
    fn published_capacities_are_the_per_floor_auto_capacity_clamped_by_the_cap() {
        let caps = caps();
        let mut sweep = FloorCapacitySweep::new();
        sweep.publish(W, H, None, &caps);
        for (i, slot) in caps.iter().enumerate() {
            let want =
                pixtuoid_scene::floor::floor_capacity(W, H, pixtuoid_scene::floor::floor_seed(i));
            assert_eq!(slot.load(Ordering::Relaxed), want, "floor {i}");
            assert!(want > 0, "floor {i} must seat someone at 192x80");
        }
        let capped: [AtomicUsize; MAX_FLOORS] = std::array::from_fn(|_| AtomicUsize::new(0));
        let mut sweep = FloorCapacitySweep::new();
        sweep.publish(W, H, Some(3), &capped);
        for (i, slot) in capped.iter().enumerate() {
            assert_eq!(
                slot.load(Ordering::Relaxed),
                3,
                "floor {i} clamped to the cap"
            );
        }
    }
}

#[cfg(test)]
mod teardown_tests {
    use super::unwind_terminal_modes;
    use std::cell::Cell;

    struct FailingWriter;
    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("terminal gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("terminal gone"))
        }
    }

    #[test]
    fn raw_mode_is_disabled_even_when_the_escape_write_fails() {
        let disabled = Cell::new(false);
        let err = unwind_terminal_modes(&mut FailingWriter, || {
            disabled.set(true);
            Ok(())
        })
        .expect_err("the write failure still propagates");
        assert!(
            disabled.get(),
            "raw mode must be disabled even when the escape-sequence write failed \
             — a `?` there strands the user's shell echo-less: {err:#}"
        );
    }

    /// Unix-only: with crossterm's ANSI flag false — as under `windows-test` (piped
    /// stdout, no console, no `TERM`) — these sequences go to the console API and no
    /// writer ever sees a byte to assert on.
    #[cfg(unix)]
    const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

    #[cfg(unix)]
    #[test]
    fn the_unwind_writes_the_leave_sequence_into_the_writer_it_is_given() {
        let mut buf: Vec<u8> = Vec::new();
        unwind_terminal_modes(&mut buf, || Ok(())).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains(LEAVE_ALT_SCREEN),
            "the unwind must reach the writer handed to it, not a fixed stream: {s:?}"
        );
    }

    /// Unix-only for the same console-API reason as `LEAVE_ALT_SCREEN` above.
    #[cfg(unix)]
    #[test]
    fn raw_mode_is_disabled_only_after_the_escape_bytes_are_written() {
        struct Recorder<'a>(&'a Cell<bool>);
        impl std::io::Write for Recorder<'_> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.set(true);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let wrote = Cell::new(false);
        let raw_saw_write = Cell::new(false);
        unwind_terminal_modes(&mut Recorder(&wrote), || {
            raw_saw_write.set(wrote.get());
            Ok(())
        })
        .unwrap();
        assert!(
            raw_saw_write.get(),
            "DisableMouseCapture must reach the terminal while raw mode is still ON"
        );
    }

    #[test]
    fn the_escape_write_error_outranks_a_later_raw_mode_error() {
        let err = unwind_terminal_modes(&mut FailingWriter, || {
            Err(std::io::Error::other("raw mode gone"))
        })
        .expect_err("both steps failed");
        assert!(
            err.to_string().contains("terminal gone"),
            "the first failure is reported, got: {err:#}"
        );
    }
}

#[cfg(test)]
mod runtime_model {
    // Pins why the `block_in_place` wraps were removed — see the tui/CLAUDE.md
    // `block_on` sharp edge.
    #[test]
    fn block_in_place_is_inert_on_the_block_on_thread() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("multi-thread runtime");
        rt.block_on(async {
            let (tx, rx) = std::sync::mpsc::channel::<u8>();
            tokio::spawn(async move {
                tx.send(1).expect("send");
            });
            let got = tokio::task::block_in_place(|| rx.recv().expect("recv"));
            assert_eq!(
                got, 1,
                "the spawned worker progressed while the loop blocked"
            );
            // Without the wrap: observably identical, so the wrap was a no-op.
            let (tx2, rx2) = std::sync::mpsc::channel::<u8>();
            tokio::spawn(async move {
                tx2.send(2).expect("send");
            });
            assert_eq!(rx2.recv().expect("recv"), 2);
        });
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{
        connect_source, connection, disconnect_source, dispatch_key, FloorNav, KeyAction,
        ModalState,
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
        use super::{toggle_intent, ToggleIntent};
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

    /// The popup is DISMISS-ONLY, which is why `widgets::version_popup` marks its
    /// overflowing notes band with a non-scrolling marker instead of the shared
    /// `⋮ N more ▾`. Binding a scroll key here would make that marker wrong.
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
    fn only_press_events_dispatch() {
        use crossterm::event::KeyEventKind;
        assert!(super::should_dispatch_key(KeyEventKind::Press));
        assert!(!super::should_dispatch_key(KeyEventKind::Release));
        assert!(!super::should_dispatch_key(KeyEventKind::Repeat));
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
    fn onboarding_noop_outcome_keeps_the_desired_gate_state() {
        use crate::sources::ChangeOutcome;
        let connected = crate::runtime::ConnectedSources::new(
            std::iter::once("antigravity".to_string()).collect(),
        );
        let choices: Vec<(&'static str, bool)> = vec![("antigravity", true), ("codex", false)];
        let outcomes = vec![
            ("antigravity".to_string(), ChangeOutcome::NoOp),
            ("codex".to_string(), ChangeOutcome::NoOp),
        ];
        super::reflect_onboarding_outcomes(&connected, &choices, &outcomes);
        assert!(
            connected.is_connected("antigravity"),
            "NoOp on a checked row must leave the gate open"
        );
        assert!(
            !connected.is_connected("codex"),
            "NoOp on an unchecked row keeps the gate closed"
        );
    }

    #[test]
    fn onboarding_outcomes_map_connected_disconnected_failed() {
        use crate::sources::ChangeOutcome;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> =
            vec![("antigravity", true), ("codex", false), ("cursor", true)];
        let outcomes = vec![
            ("antigravity".to_string(), ChangeOutcome::Connected),
            ("codex".to_string(), ChangeOutcome::Disconnected),
            ("cursor".to_string(), ChangeOutcome::Failed("boom".into())),
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
        use crate::sources::ChangeOutcome;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> = vec![("cursor", true), ("antigravity", true)];
        let outcomes = vec![
            (
                "cursor".to_string(),
                ChangeOutcome::Failed("settings is valid JSON but not an object".into()),
            ),
            ("antigravity".to_string(), ChangeOutcome::Connected),
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
        use crate::sources::ChangeOutcome;
        let connected = crate::runtime::ConnectedSources::default();
        let choices: Vec<(&'static str, bool)> = vec![("cursor", false), ("openclaw", false)];
        let outcomes = vec![
            (
                "cursor".to_string(),
                ChangeOutcome::Failed("config is not writable".into()),
            ),
            (
                "openclaw".to_string(),
                ChangeOutcome::Failed(format!(
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
        let mut ui = crate::tui::ui_state::UiState::new(
            pixtuoid_scene::theme::ALL_THEMES[0],
            crate::tui::welcome::WelcomeUi::from_detected(&[]),
            false,
            std::path::PathBuf::from("/tmp/sock"),
            None,
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
        use crate::sources::ChangeOutcome;
        // `apply_choices` maps every want to Connect/Disconnect, never NoOp, so the
        // skip path's semantic-no-op re-install really does emit `Connected`.
        let connected = crate::runtime::ConnectedSources::default();
        assert!(!connected.is_connected("antigravity"), "gate starts empty");
        let freeze: Vec<(&'static str, bool)> = vec![("antigravity", true)];
        let outcomes = vec![("antigravity".to_string(), ChangeOutcome::Connected)];
        super::reflect_onboarding_outcomes(&connected, &freeze, &outcomes);
        assert!(
            connected.is_connected("antigravity"),
            "skip must open the live gate for a frozen-connected source"
        );
    }
}

/// Tests for the APPLIER half of the key path. `dispatch_key` (the decoder) is
/// covered by `dispatch_tests` above; before the #830 split these arms lived
/// inside `run_tui`, which needs a real terminal, so nothing could reach them.
#[cfg(test)]
mod apply_key_action_tests {
    use super::{apply_key_action, KeyAction, KeyCtx};
    use crate::tui::tui_renderer::TuiRenderer;
    use pixtuoid_scene::theme;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::HashSet;
    use std::time::SystemTime;

    /// Stands in for `crate::audio::respawn`, which opens a real output device.
    fn no_respawn(_: &crate::audio::AudioHandle, _: f32) {}

    struct Harness {
        ui: super::ui_state::UiState,
        renderer: TuiRenderer<TestBackend>,
        audio_ctl: crate::audio::AudioController,
        connected: crate::runtime::ConnectedSources,
        snapshot: pixtuoid_core::state::SceneState,
        focus_roots: (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
        _tmp: tempfile::TempDir,
        config_path: std::path::PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let config_path = tmp.path().join("config.toml");
            Self {
                ui: super::ui_state::UiState::new(
                    &theme::NORMAL,
                    super::welcome::WelcomeUi::from_detected(&[]),
                    false,
                    tmp.path().join("sock"),
                    None,
                ),
                renderer: TuiRenderer::new(
                    Terminal::new(TestBackend::new(80, 24)).expect("test backend"),
                    &theme::NORMAL,
                    Vec::new(),
                ),
                // UNMUTED via `new_with` + a no-op spawn: an unmuted `new`
                // would open the machine's real output device, and a MUTED
                // controller makes pause unobservable (`set_paused` ORs the
                // mute flag in, so the handle stays muted either way).
                audio_ctl: crate::audio::AudioController::new_with(
                    false,
                    1.0,
                    config_path.clone(),
                    no_respawn,
                ),
                connected: crate::runtime::ConnectedSources::new(HashSet::new()),
                snapshot: pixtuoid_core::state::SceneState::uniform(4),
                focus_roots: (None, None),
                _tmp: tmp,
                config_path,
            }
        }

        /// Register `n` agents so the dashboard has rows to move through.
        fn seed_agents(&mut self, n: usize) {
            use pixtuoid_core::source::{AgentEvent, Transport};
            let mut r = pixtuoid_core::Reducer::new();
            for i in 0..n {
                let path = format!("/p/agent-{i}.jsonl");
                r.apply(
                    &mut self.snapshot,
                    AgentEvent::SessionStart {
                        agent_id: pixtuoid_core::AgentId::from_transcript_path(&path),
                        source: "claude-code".into(),
                        session_id: path.clone(),
                        cwd: std::path::PathBuf::from("/repo"),
                        parent_id: None,
                    },
                    SystemTime::UNIX_EPOCH,
                    Transport::Jsonl,
                );
            }
        }

        fn apply(&mut self, action: KeyAction) -> bool {
            apply_key_action(
                action,
                &mut KeyCtx {
                    ui: &mut self.ui,
                    renderer: &mut self.renderer,
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
    }

    /// The highest-blast-radius pair: `Quit` must be the ONLY action that ends
    /// the loop. A mutant returning a constant makes every keypress quit (or
    /// makes `q` inert), and nothing else in the suite would notice.
    #[test]
    fn only_quit_returns_true() {
        let mut h = Harness::new();
        assert!(h.apply(KeyAction::Quit), "Quit must end the loop");
        for action in [
            KeyAction::None,
            KeyAction::TogglePause,
            KeyAction::ToggleHelp,
            KeyAction::CloseHelp,
            KeyAction::DismissVersionPopup,
            KeyAction::OpenThemePicker,
            KeyAction::ToggleWalkableDebug,
            KeyAction::ToggleDashboard,
            KeyAction::DashboardClose,
            KeyAction::ConnectionClose,
        ] {
            assert!(
                !h.apply(action),
                "only Quit may end the loop, but {action:?} did"
            );
        }
    }

    /// `set_paused` must read `paused()` AFTER the toggle. Asserting only
    /// `ui.paused()` is NOT enough — that survives swapping the two statements,
    /// because the UI flag flips either way. The audio handle is what goes out
    /// of sync, so assert THAT: pausing must mute, unpausing must unmute.
    #[test]
    fn toggle_pause_drives_the_audio_handle_in_step_with_the_ui() {
        let mut h = Harness::new();
        assert!(!h.ui.paused());
        assert!(
            !h.audio_ctl.handle().is_muted(),
            "harness precondition: the controller starts unmuted"
        );

        h.apply(KeyAction::TogglePause);
        assert!(h.ui.paused(), "p must pause the UI");
        assert!(
            h.audio_ctl.handle().is_muted(),
            "p must mute audio in the SAME apply — a set_paused read before the \
             toggle leaves the handle a step behind"
        );

        h.apply(KeyAction::TogglePause);
        assert!(!h.ui.paused(), "p again must unpause the UI");
        assert!(
            !h.audio_ctl.handle().is_muted(),
            "unpause must unmute in the same apply"
        );
    }

    /// Pins the `!` the mutation run flagged: dropping it makes the toggle a
    /// no-op that always writes the value already there.
    #[test]
    fn toggle_walkable_debug_flips_rather_than_sets() {
        let mut h = Harness::new();
        let before = h.renderer.debug_walkable();
        h.apply(KeyAction::ToggleWalkableDebug);
        assert_eq!(
            h.renderer.debug_walkable(),
            !before,
            "w must FLIP the overlay, not assign a constant"
        );
        h.apply(KeyAction::ToggleWalkableDebug);
        assert_eq!(h.renderer.debug_walkable(), before, "w must flip back");
    }

    /// The click predicates were made pure so they COULD be tested; these are
    /// that. Both were previously unreachable — they called
    /// `crossterm::terminal::size()` internally, which under `cargo test` has no
    /// tty and returned `Err` -> `false` unconditionally.
    ///
    /// Deliberately NOT asserted: the scene-rect-vs-full-bounds asymmetry
    /// between the two. `star_hit_rect` places the star at `scene.y + 1` height
    /// 1 and `scene_rect` shrinks only HEIGHT, so both framings yield an
    /// identical star rect on any terminal taller than two rows — an assertion
    /// there would be near-unfalsifiable.
    #[test]
    fn star_clicked_hits_only_the_star_span() {
        use crate::tui::widgets::star_hit_rect;
        let term = (120u16, 44u16);
        let scene = super::renderer::scene_rect(ratatui::layout::Rect::new(0, 0, term.0, term.1));
        let star = star_hit_rect(scene).expect("the star fits at 120x44");

        assert!(
            super::star_clicked(star.x, star.y, term),
            "a click on the star's first column must hit"
        );
        assert!(
            super::star_clicked(star.x + star.width - 1, star.y, term),
            "a click on the star's last column must hit"
        );
        assert!(
            !super::star_clicked(star.x - 1, star.y, term),
            "one column LEFT of the star must miss"
        );
        assert!(
            !super::star_clicked(star.x, star.y + 1, term),
            "one row BELOW the star must miss — the rect is height 1"
        );
        // Too narrow to paint any of the star ⇒ no click target, no phantom
        // browser launch.
        assert!(
            !super::star_clicked(1, 1, (10, 44)),
            "a terminal too narrow for the star must never register a hit"
        );
    }

    /// The popup URL is clickable only while the popup is actually painted —
    /// `version_popup_url_rect` returns `None` below the clickable scale, and a
    /// predicate that ignored `scale` would launch a browser on a click landing
    /// where the popup merely USED to be.
    #[test]
    fn version_popup_url_clicked_respects_the_rect_and_the_scale() {
        use crate::tui::widgets::version_popup_url_rect;
        let term = (120u16, 44u16);
        let bounds = ratatui::layout::Rect::new(0, 0, term.0, term.1);
        let notes = crate::version::release_notes(env!("CARGO_PKG_VERSION")).unwrap_or(&[]);
        let Some(rect) = version_popup_url_rect(notes, bounds, 1.0) else {
            // The shipped notes must produce a link rect at full scale; if this
            // ever changes the assertions below would pass vacuously.
            panic!("the shipped release notes must yield a URL rect at scale 1.0");
        };

        assert!(
            super::version_popup_url_clicked(rect.x, rect.y, 1.0, term),
            "a click inside the URL rect at full scale must hit"
        );
        assert!(
            !super::version_popup_url_clicked(rect.x, rect.y.saturating_sub(1), 1.0, term),
            "a click one row above the URL must miss"
        );
        assert!(
            !super::version_popup_url_clicked(rect.x, rect.y, 0.5, term),
            "mid-animation (below the clickable scale) there is no rect, so no hit"
        );
    }

    /// `delete -` on either `-1` makes Up behave as Down. The panels are
    /// independent, so both pairs need pinning — and each needs at least two
    /// rows, or the move is a no-op in both directions and the assertion is
    /// vacuous.
    #[test]
    fn dashboard_and_connection_up_move_opposite_to_down() {
        let mut h = Harness::new();
        h.seed_agents(3);

        h.apply(KeyAction::ToggleDashboard);
        let start = h.ui.dashboard.selected;
        h.apply(KeyAction::DashboardDown);
        let after_down = h.ui.dashboard.selected;
        assert_ne!(after_down, start, "precondition: Down actually moves");
        h.apply(KeyAction::DashboardUp);
        // Must return to `start`, not merely DIFFER from `after_down` — with the
        // `-1` deleted, Up moves further DOWN, which also differs.
        assert_eq!(
            h.ui.dashboard.selected, start,
            "DashboardUp must undo DashboardDown, not advance further"
        );

        h.apply(KeyAction::ToggleConnection);
        assert!(
            h.ui.connection.rows.len() > 1,
            "precondition: the Sources panel lists the registry, so it has rows"
        );
        let start = h.ui.connection.selected;
        h.apply(KeyAction::ConnectionDown);
        let after_down = h.ui.connection.selected;
        assert_ne!(after_down, start, "precondition: Down actually moves");
        h.apply(KeyAction::ConnectionUp);
        assert_eq!(
            h.ui.connection.selected, start,
            "ConnectionUp must undo ConnectionDown, not advance further"
        );
    }

    /// `ThemeCommit` persists; `ThemeCancel` restores the last COMMITTED theme,
    /// not merely the previewed one.
    #[test]
    fn theme_commit_persists_and_cancel_restores_the_saved_theme() {
        let mut h = Harness::new();
        h.apply(KeyAction::OpenThemePicker);
        h.apply(KeyAction::ThemeCommit(1));
        let saved = std::fs::read_to_string(&h.config_path).expect("config written");
        assert!(
            saved.contains(theme::ALL_THEMES[1].name),
            "commit must persist the theme: {saved:?}"
        );

        h.apply(KeyAction::OpenThemePicker);
        h.apply(KeyAction::ThemePreview(3));
        h.apply(KeyAction::ThemeCancel);
        assert_eq!(
            h.ui.saved_theme_idx, 1,
            "cancel must restore the COMMITTED theme, not the preview"
        );
    }
}
