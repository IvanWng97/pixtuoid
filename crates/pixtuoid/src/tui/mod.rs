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

/// Which overlay (if any) currently owns input, plus the one count the picker
/// needs. The two together drive the modal precedence chain (onboarding > help >
/// version > connection > dashboard > theme-picker > normal); when one is open it
/// swallows keys and the normal-scene bindings are suspended.
///
/// Pulled out (with [`FloorNav`]) so the dispatch decision is a pure function of
/// (key, state) and can be unit-tested without a TTY — the crossterm `read()` and
/// all renderer/config side effects stay in the event loop. Modal state and floor
/// navigation are ORTHOGONAL, so they're separate arguments to [`dispatch_key`]
/// rather than one bundle: a key is gated by the modals first, and only the
/// normal-scene floor keys ever read [`FloorNav`].
#[derive(Clone, Copy)]
struct ModalState {
    /// First-run onboarding overlay open — the TOP of the modal precedence chain.
    onboarding_open: bool,
    help_open: bool,
    version_popup: bool,
    theme_picker: Option<usize>,
    dashboard_open: bool,
    connection_open: bool,
    /// Whether the Sources panel has a disconnect armed (awaiting y/n). Splits the
    /// open-connection dispatch into the armed (y/n only) vs unarmed (nav/toggle) sub-tiers.
    connection_confirm: bool,
    n_themes: usize,
}

/// Floor-navigation state the normal-scene PageUp/PageDown arms read to clamp a
/// move (can't go past the ends, and a slide already in flight blocks a new one).
/// Independent of [`ModalState`] — see its doc.
#[derive(Clone, Copy)]
struct FloorNav {
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
    /// Open/close the agent dashboard (`Tab`, from the normal scene only).
    ToggleDashboard,
    /// Dashboard list navigation.
    DashboardUp,
    DashboardDown,
    /// `←/h`: collapse the selected root (on a child, collapse its parent).
    DashboardFoldLeft,
    /// `→/l`: expand the selected root.
    DashboardFoldRight,
    /// `z`: fold-all / unfold-all toggle across every root.
    DashboardFoldAll,
    /// `Enter`: jump to the selected agent's floor + close.
    DashboardJump,
    /// `f`: bring the selected agent's terminal app to the foreground.
    DashboardFocus,
    /// `Esc`/`Tab`: close without jumping.
    DashboardClose,
    /// Open/close the Sources panel (`s`, from the normal scene; `s`/Esc closes).
    /// (Variant + module keep the historical `Connection`/`connection` names.)
    ToggleConnection,
    /// Connection list navigation.
    ConnectionUp,
    ConnectionDown,
    /// `t`: toggle the selected CLI's connection. Connecting is immediate;
    /// disconnecting arms a confirm first (it removes hooks + walks characters out).
    ConnectionToggle,
    /// `y` while armed: run the disconnect.
    ConnectionConfirm,
    /// `n`/`Esc` while armed: cancel the arm (panel stays open).
    ConnectionCancelConfirm,
    /// `s`/`Esc` while unarmed: close the panel.
    ConnectionClose,
    /// First-run onboarding roster navigation (`↑↓`/`jk`).
    OnboardingUp,
    OnboardingDown,
    /// `space`: toggle the selected CLI's checkbox.
    OnboardingToggle,
    /// `Enter`: apply the checked sources (connect) + close onboarding.
    OnboardingConfirm,
    /// `Esc`: skip onboarding (mark done without connecting) + close.
    OnboardingSkip,
}

/// Left-click focus-jump: hit-test the click against the desk layout and bring
/// the clicked agent's terminal APP to the foreground (the click-to-pin
/// tooltip this replaced is gone — hover still shows the dossier). Identical
/// in both the pet-present and pet-absent click branches, so it lives here.
/// Every miss is a silent no-op (`focus`'s ONE failure rule).
fn focus_clicked_agent<B: ratatui::backend::Backend<Error: Send + Sync + 'static>>(
    renderer: &mut TuiRenderer<B>,
    scene_rx: &SceneRx,
    focus_roots: &(Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    col: u16,
    row: u16,
    now: SystemTime,
) {
    let snap = scene_rx.borrow().clone();
    // Project to the VISIBLE floor first — hit_test_agent_at → character_anchor
    // reads floor-local desk indices; the click now follows the LIVE sprite (like hover).
    let floor_scene = floor::project_floor_scene(&snap, renderer.current_floor());
    let hit = renderer.hit_test_agent_at(&floor_scene, now, col, row);
    if let Some(slot) = hit.and_then(|id| snap.agents.get(&id)) {
        crate::focus::focus_slot(slot, focus_roots);
    }
}

/// Connect a source from the panel: delegate the persist + install + rollback to
/// the shared `crate::sources::connect` core, then — only on success — open the
/// live gate (`connected.set` → the reducer task's reconciler stops evicting it +
/// its events flow). The core persists the flag FIRST and rolls it back if the
/// install fails, so on `Err` the gate was never opened (no shown-but-broken
/// source surviving a restart). The panel adds the live-gate line the CLI omits.
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
        Err(e) => format!("{display_name}: connect failed \u{2014} {e:#}"),
    }
}

/// Disconnect a source from the panel: delegate to `crate::sources::disconnect`
/// (persist the flag false FIRST, then remove hooks), then close the live gate.
/// The core reserves `Err` for the persist-failure abort (flip NOTHING — a
/// runtime hide the next restart reverts is a lie); a hook-removal failure is
/// folded into the `Ok` outcome, so the gate STILL closes (the flag is false).
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
                    format!("{display_name}: disconnected, but hook removal failed \u{2014} {e}")
                }
            }
        }
        Err(e) => format!("{display_name}: disconnect failed \u{2014} {e:#}"),
    }
}

/// Reflect the onboarding apply's outcomes into the LIVE connected-set.
/// `choices` and `outcomes` are index-aligned (`apply_choices` maps each choice
/// in order). `NoOp` means "already in the DESIRED state — nothing written"
/// (`sources::ChangeOutcome`), so it sets the gate to the desired flag rather
/// than hardcoding it closed: a NoOp for a CHECKED row must leave the gate OPEN
/// (an already-connected source the user just confirmed must not have its live
/// agents evicted). A failed connect must NOT go live, and leaves a trace on
/// the warn-floor log (doctor + the footer nudge read it).
fn reflect_onboarding_outcomes(
    connected: &crate::runtime::ConnectedSources,
    choices: &[(&'static str, bool)],
    outcomes: &[(String, crate::sources::ChangeOutcome)],
) {
    use crate::sources::ChangeOutcome;
    for ((_, want), (id, oc)) in choices.iter().zip(outcomes) {
        match oc {
            ChangeOutcome::Connected => connected.set(id, true),
            ChangeOutcome::Disconnected => connected.set(id, false),
            ChangeOutcome::NoOp => connected.set(id, *want),
            ChangeOutcome::Failed(e) => {
                connected.set(id, false);
                tracing::warn!("onboarding: {id} failed to connect: {e}");
            }
        }
    }
}

fn is_quit_chord(code: KeyCode, mods: KeyModifiers) -> bool {
    matches!(
        (code, mods),
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

/// Windows delivers Press AND Release events per keystroke; only Press may
/// dispatch (Release/Repeat double-fire keys there — `p` would pause then
/// instantly unpause). Unix only ever delivers Press, so this is a no-op there.
fn should_dispatch_key(kind: KeyEventKind) -> bool {
    kind == KeyEventKind::Press
}

/// What pressing `t` on a Sources-panel row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToggleIntent {
    /// Bound, OR connected-but-CLI-absent → arm the disconnect confirm.
    ArmConfirm,
    /// Unbound present CLI → connect immediately.
    Connect,
    /// Absent CLI that was never connected → inert "not detected" hint.
    Hint,
}

/// The Sources-panel toggle decision, factored OUT of the `run_tui` event loop
/// (which is codecov-excluded and undriveable headlessly) so it is unit-testable —
/// mirroring how `dispatch_key` factors key→action. The load-bearing arm is
/// `NoCli { connected: true }` → `ArmConfirm`: a source whose CLI vanished is still
/// disconnectable (its hooks live in the config, not the missing binary), while a
/// never-connected absent CLI (`NoCli { connected: false }`) stays inert.
fn toggle_intent(state: connection::ConnState) -> ToggleIntent {
    match state {
        connection::ConnState::Connected | connection::ConnState::NoCli { connected: true } => {
            ToggleIntent::ArmConfirm
        }
        connection::ConnState::Disconnected => ToggleIntent::Connect,
        connection::ConnState::NoCli { connected: false } => ToggleIntent::Hint,
    }
}

/// Pure key-dispatch: resolve a key press to a `KeyAction` given the current
/// modal + floor state. Modal precedence (highest first): onboarding > help >
/// version popup > connection > dashboard > theme picker > normal scene (the
/// same chain the `ModalState` doc above pins — the body's early-returns are its
/// single source of truth).
fn dispatch_key(
    code: KeyCode,
    mods: KeyModifiers,
    modal: ModalState,
    floor: FloorNav,
) -> KeyAction {
    // Onboarding is modal and the TOP of the precedence chain — it swallows every
    // other key (no other overlay can open while it's up) except the quit chord.
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
        // Armed sub-tier: a uninstall is awaiting confirmation — only y/n/Esc
        // (and the quit chord) act; nav/action keys are swallowed.
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
            // Bare `s` (the Sources panel's key) toggles it closed; Esc too.
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
            // The quit chord passes through like every other modal tier — the
            // run_tui quit arm already reverts the previewed theme on break.
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
    // Normal scene.
    if is_quit_chord(code, mods) || code == KeyCode::Esc {
        return KeyAction::Quit;
    }
    match code {
        KeyCode::Char('p') => KeyAction::TogglePause,
        KeyCode::Char('t') => KeyAction::OpenThemePicker,
        KeyCode::Char('?') => KeyAction::ToggleHelp,
        KeyCode::Tab => KeyAction::ToggleDashboard,
        // `s` opens the Sources panel (connection + health + live). Renamed from
        // `c`/"Connection" once the panel grew past bind/unbind into a per-source
        // board; `Ctrl+C` stays the quit chord (handled above).
        KeyCode::Char('s') => KeyAction::ToggleConnection,
        // Dev-only walkable/approach/route overlay — gated out of release builds.
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

// --- Terminal lifecycle ---------------------------------------------------
// Lives here (not renderer.rs) because raw mode + the alternate screen are
// owned by the event loop, and this file is already excluded from headless
// coverage — no test can exercise a real TTY (issue #103).

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Term> {
    // On the WinAPI fallback (no VT), crossterm maps Color::Rgb to console
    // attribute 0 — the office renders black-on-black invisible. Gate, don't
    // degrade (Windows Terminal is the supported terminal).
    #[cfg(windows)]
    if !crossterm::ansi_support::supports_ansi() {
        anyhow::bail!(
            "pixtuoid needs a VT-capable terminal — use Windows Terminal \
             (or Windows 10 1703+ with VT processing enabled)"
        );
    }
    enable_raw_mode()?;
    let mut out = stdout();
    // EnableMouseCapture turns on the terminal's mouse-event reporting.
    // Modern terminals emit MouseEventKind::Moved on cursor motion (no
    // button required), which is how we drive the hover tooltip.
    //
    // Keep setup ATOMIC: if entering the alt screen / mouse capture — or building
    // the Terminal (whose `.size()` query can fail) — errors after raw mode is on,
    // roll the terminal all the way back before propagating. Otherwise the error
    // path strands the user's shell in raw mode (no echo) and/or the alt screen.
    if let Err(e) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
        // Mirror the teardown order in case EnterAlternateScreen took effect
        // before EnableMouseCapture failed — leave the alt screen too, not just
        // raw mode, so the rollback is truly "all the way back".
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    Terminal::new(CrosstermBackend::new(out)).map_err(|e| {
        let mut out = stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        e.into()
    })
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    // DisableMouseCapture must run while raw mode is still ON: on Windows it
    // restores the input mode snapshotted at Enable time (which was raw-era),
    // so running it after disable_raw_mode re-raws the console and leaves
    // the user's shell echo-less. Raw mode goes off LAST.
    execute!(
        term.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    term.show_cursor()?;
    Ok(())
}

/// Decide whether the version popup shows this boot, persisting the current
/// version so the popup shows at most once per upgrade regardless of how the run
/// exits. Re-loads the config post-altscreen for `last_seen_version` only — any
/// config warning was already surfaced by `main`'s pre-altscreen pass.
/// A corrupted/hand-edited `last_seen_version` is overwritten so the popup can't
/// be silently disabled forever.
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

/// The bundled inputs to [`run_tui`] — the runtime (`driver.rs`) builds it once
/// and hands it over. Grouping these into a named struct kills the positional-arg
/// transposition hazard (it had grown to 11 args of mostly `Option`/`PathBuf`/
/// `bool`, several interchangeable by type) and gives new features a named home
/// instead of another positional argument.
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
    /// The resolved hook socket (Unix) / named pipe (Windows) the daemon bound,
    /// shown in the Sources panel's connection line.
    pub socket_path: std::path::PathBuf,
    /// The live connected-source set — the Sources panel's mutation seam: a
    /// toggle calls `connected.set(src, on)`, which the reducer task's reconciler
    /// observes (gate + graceful evict). Shared `Arc<Mutex<…>>` with the reducer.
    pub connected: crate::runtime::ConnectedSources,
    /// The warn-floor log path — throttle-scanned for decode-drift breadcrumbs to
    /// drive the footer nudge (`main` owns the resolution; `None` = no surfacing).
    pub log_path: Option<std::path::PathBuf>,
    /// Focus-jump pid point-query roots: (CC projects root, Codex sessions
    /// root) — threaded from RunConfig so a sprite click can resolve a
    /// transcript-family agent's pid at click time.
    pub focus_roots: (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    /// First launch ever → seed the one-time onboarding overlay open.
    pub first_run: bool,
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
    } = session;
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term, theme, pets);
    // First-run onboarding "move-in" overlay (TOP of the modal precedence chain).
    // The roster is built only on first run; if no agent CLIs are detected there's
    // nothing to connect, so it stays closed and the office shows normally.
    let detected_clis = if first_run {
        crate::sources::detect()
    } else {
        Vec::new()
    };
    let onboarding_ui = welcome::WelcomeUi::from_detected(&detected_clis);

    // The "what's new in vX" version popup yields to onboarding ONLY when the
    // overlay actually takes the screen (a fresh install WITH detected CLIs): both
    // are centered cards, and it's noise to a first-time user. Suppress + still
    // STAMP `last_seen_version` (so it won't pop later — onboarding is the one
    // welcome). Gating on the overlay SHOWING (not bare `first_run`) is load-bearing:
    // a no-CLI first-run user gets the version popup normally, incl. on a later
    // upgrade (otherwise `first_run` stays true forever and would mute it for good).
    let version_popup = if !onboarding_ui.is_empty() {
        let _ = resolve_version_popup(&config_path);
        false
    } else {
        resolve_version_popup(&config_path)
    };
    // The per-surface UI state — open/close transitions + the ModalState
    // projection + the per-frame renderer mirrors all live on UiState; this
    // loop keeps the terminal lifecycle and the renderer/config/install side
    // effects (see ui_state.rs).
    let mut ui = ui_state::UiState::new(theme, onboarding_ui, version_popup, socket_path, log_path);
    let mut last_layout_sig: Option<(u16, u16)> = None;

    // Render/event-loop tick (~30fps).
    const FRAME_TICK_MS: u64 = 33;
    let tick = Duration::from_millis(FRAME_TICK_MS);
    let result: Result<()> = (async {
        // External-signal teardown: raw mode delivers keyboard Ctrl-C as a key
        // event (the quit chord), but an EXTERNAL SIGINT/SIGTERM (`kill <pid>`,
        // logout) would hit the default disposition and kill the process
        // mid-altscreen with mouse reporting on — the shell is left unusable
        // until `reset`. Route both into the loop so the normal teardown path
        // runs. Pinned ONCE outside the loop (same rationale as headless_loop:
        // a per-iteration ctrl_c() drops the subscription mid-gap); boxed so a
        // registration FAILURE can disarm the arm by swapping in a pending
        // future — a resolved future must never be polled again, and quitting
        // on Err would exit the TUI at boot.
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
            // Capture the health snapshot ONCE this frame — both the footer
            // warning and the Sources panel's per-source `dead` flag read it.
            let health = source_health.borrow_and_update().clone();
            // Compute + push this frame's renderer mirrors (dashboard /
            // connection / onboarding frames, theme-picker/version/help flags,
            // the drift-scan-fed footer warning) — see UiState::build_frames.
            ui.build_frames(now, &snapshot, &health)
                .apply_to(&mut renderer, now);
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
                use pixtuoid_core::state::MAX_FLOORS;
                let buf_w = layout.buf_w;
                let buf_h = layout.buf_h;
                for floor_idx in 0..MAX_FLOORS {
                    let seed = pixtuoid_scene::floor::floor_seed(floor_idx);
                    let mut capacity = pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed);
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
                    // Windows delivers Press AND Release events per
                    // keystroke; without this guard every key double-fires
                    // there (e.g. `p` pauses then instantly unpauses).
                    Event::Key(k) if should_dispatch_key(k.kind) => {
                        let floor = FloorNav {
                            n_floors: pixtuoid_scene::floor::num_floors(&snapshot),
                            current_floor: renderer.current_floor(),
                            in_transition: renderer.transition().is_some(),
                        };
                        match dispatch_key(k.code, k.modifiers, ui.modal(), floor) {
                            KeyAction::None => {}
                            KeyAction::Quit => quit = true,
                            KeyAction::TogglePause => ui.toggle_pause(),
                            KeyAction::ToggleHelp => ui.toggle_help(),
                            KeyAction::CloseHelp => ui.close_help(),
                            KeyAction::DismissVersionPopup => ui.dismiss_version_popup(),
                            KeyAction::OpenThemePicker => ui.open_theme_picker(),
                            KeyAction::ThemePreview(i) => {
                                ui.preview_theme(i);
                                renderer.set_theme(theme::ALL_THEMES[i]);
                            }
                            KeyAction::ThemeCommit(i) => {
                                ui.commit_theme(i);
                                let name = theme::ALL_THEMES[i].name;
                                if let Err(e) = crate::config::save(&config_path, name) {
                                    tracing::warn!("failed to persist theme: {e}");
                                }
                            }
                            KeyAction::ThemeCancel => {
                                let saved = ui.cancel_theme();
                                renderer.set_theme(theme::ALL_THEMES[saved]);
                            }
                            KeyAction::NavigateFloor(target) => {
                                renderer.navigate_floor(target, now);
                            }
                            KeyAction::ToggleWalkableDebug => {
                                let on = renderer.debug_walkable();
                                renderer.set_debug_walkable(!on);
                            }
                            KeyAction::ToggleDashboard => ui.toggle_dashboard(&snapshot),
                            KeyAction::DashboardClose => ui.close_dashboard(),
                            KeyAction::DashboardUp => ui.dashboard_move(&snapshot, -1),
                            KeyAction::DashboardDown => ui.dashboard_move(&snapshot, 1),
                            KeyAction::DashboardFoldLeft => ui.dashboard_fold_left(&snapshot),
                            KeyAction::DashboardFoldRight => ui.dashboard_fold_right(&snapshot),
                            KeyAction::DashboardFoldAll => ui.dashboard_fold_all(&snapshot),
                            KeyAction::DashboardJump => {
                                if let Some(floor) = ui.dashboard_jump(&snapshot) {
                                    renderer.navigate_floor(floor, now);
                                }
                            }
                            KeyAction::DashboardFocus => {
                                if let Some(slot) =
                                    ui.dashboard_focus().and_then(|id| snapshot.agents.get(&id))
                                {
                                    crate::focus::focus_slot(slot, &focus_roots);
                                }
                            }
                            KeyAction::ToggleConnection => {
                                if ui.connection.open {
                                    ui.close_connection();
                                } else {
                                    // FS reads + the connected-set snapshot happen on open
                                    // + after each toggle, never per frame (a brief stall, #603).
                                    let rows = connection::build_rows(
                                        &connected.snapshot(),
                                        &ui.read_conn_log(),
                                    );
                                    ui.open_connection(rows);
                                }
                            }
                            KeyAction::ConnectionUp => ui.connection_move(-1),
                            KeyAction::ConnectionDown => ui.connection_move(1),
                            KeyAction::ConnectionToggle => {
                                // Copy the fields out before any rebuild of `rows`
                                // (which would invalidate a `&ConnectionRow` borrow).
                                let action =
                                    ui.connection.rows.get(ui.connection.selected).map(|r| {
                                        (
                                            r.state,
                                            r.source_id,
                                            r.display_name,
                                            connection::no_action_hint(r),
                                        )
                                    });
                                if let Some((state, source_id, name, hint)) = action {
                                    match toggle_intent(state) {
                                        // Bound, or connected-but-CLI-absent → arm the
                                        // disconnect confirm (it removes hooks + walks
                                        // characters out). A connected NoCli row is still
                                        // disconnectable: its hooks live in the config,
                                        // not the missing binary.
                                        ToggleIntent::ArmConfirm => {
                                            ui.connection.confirm = Some(ui.connection.selected);
                                        }
                                        // Unbound present CLI → connect immediately
                                        // (additive, reversible: flag, live gate, install
                                        // hooks). connect_source's flock+fsync+FS reads are
                                        // a brief inline stall (#603).
                                        ToggleIntent::Connect => {
                                            ui.connection.last_result = Some(connect_source(
                                                &config_path,
                                                &connected,
                                                source_id,
                                                name,
                                            ));
                                            ui.connection.rows = connection::build_rows(
                                                &connected.snapshot(),
                                                &ui.read_conn_log(),
                                            );
                                        }
                                        // Absent CLI, never connected → inert hint (the
                                        // "panel refuses an absent CLI" rule is CONNECT-side).
                                        ToggleIntent::Hint => {
                                            ui.connection.last_result = Some(hint);
                                        }
                                    }
                                }
                            }
                            KeyAction::ConnectionConfirm => {
                                if let Some(idx) = ui.connection.confirm {
                                    let action = ui
                                        .connection
                                        .rows
                                        .get(idx)
                                        .map(|r| (r.source_id, r.display_name));
                                    if let Some((source_id, name)) = action {
                                        // disconnect_source takes a flock + fsync + FS
                                        // reads — a brief inline stall (#603).
                                        ui.connection.last_result = Some(disconnect_source(
                                            &config_path,
                                            &connected,
                                            source_id,
                                            name,
                                        ));
                                        ui.connection.rows = connection::build_rows(
                                            &connected.snapshot(),
                                            &ui.read_conn_log(),
                                        );
                                    }
                                }
                                ui.connection.confirm = None;
                            }
                            KeyAction::ConnectionCancelConfirm => ui.cancel_connection_confirm(),
                            KeyAction::ConnectionClose => ui.close_connection(),
                            KeyAction::OnboardingUp => ui.onboarding_ui.move_up(),
                            KeyAction::OnboardingDown => ui.onboarding_ui.move_down(),
                            KeyAction::OnboardingToggle => ui.onboarding_ui.toggle_selected(),
                            KeyAction::OnboardingConfirm => {
                                // Apply the roster: connect the checked, disconnect
                                // the unchecked — SCOPED to the detected sources, so
                                // an undetected source's flag is never written. The
                                // ConfigLock I/O is a brief inline stall (#603).
                                let choices = ui.onboarding_ui.decisions();
                                let outcomes =
                                    crate::sources::apply_choices(&config_path, &choices);
                                // Reflect each into the LIVE connected-set off its
                                // ACTUAL outcome (a failed connect must NOT go live;
                                // a NoOp keeps the DESIRED state) — see the helper.
                                reflect_onboarding_outcomes(&connected, &choices, &outcomes);
                                ui.close_onboarding();
                            }
                            KeyAction::OnboardingSkip => {
                                // Skip = mark onboarding done WITHOUT changing any
                                // hooks. Freeze each detected source to its REAL
                                // current state — connected in the live gate OR
                                // already carrying installed hooks (a pre-0.12
                                // upgrader: hooks present, no `[sources]` flag — the
                                // population onboarding replays to re-connect). Apply
                                // then re-installs the hooked/connected ones
                                // idempotently (a SEMANTIC no-op → hooks survive) and
                                // leaves the rest disconnected (uninstall of an absent
                                // hook is a no-op), so `[sources]` becomes non-empty
                                // (onboarding won't re-trigger) yet NO hooks are
                                // added/removed. `skip_freeze` reads each target's
                                // config (has_hooks) through the sources facade so the
                                // event loop doesn't reach into install; a brief inline
                                // stall, like apply_choices (#603).
                                let snap = connected.snapshot();
                                let ids: Vec<&'static str> =
                                    ui.onboarding_ui.rows.iter().map(|r| r.source_id).collect();
                                let freeze = crate::sources::skip_freeze(ids, &snap);
                                let outcomes = crate::sources::apply_choices(&config_path, &freeze);
                                // Reflect the freeze into the LIVE gate, exactly
                                // like the Confirm arm — skip PERSISTS connected=true
                                // for a pre-0.12 upgrader's hooked sources, so the
                                // in-process gate must open THIS session too, else
                                // their office stays empty until the next restart
                                // re-seeds the gate from the (now true) flags. Also
                                // logs any persist failure (its Failed arm).
                                reflect_onboarding_outcomes(&connected, &freeze, &outcomes);
                                ui.close_onboarding();
                            }
                        }
                    }
                    Event::Mouse(_) if ui.onboarding_open() => {
                        // Onboarding is modal for the mouse too — swallow every
                        // event so nothing leaks to the scene behind the overlay
                        // (it's keyboard-driven; there are no clickable targets).
                    }
                    Event::Mouse(m) if ui.help_open() => {
                        // The help overlay is modal for the mouse: a left
                        // click dismisses it and every mouse event is
                        // swallowed so nothing leaks to the scene behind it
                        // (e.g. coffee-machine / branding clicks launching a
                        // browser). Placed before the popup guard so help
                        // wins even mid popup-dismiss animation.
                        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                            ui.close_help();
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
                    Event::Mouse(_)
                        if ui.theme_picker.is_some() || ui.dashboard.open || ui.connection.open =>
                    {
                        // The dashboard / Sources panel / theme picker are modal for
                        // the mouse too: they paint centered over the scene, so swallow
                        // every mouse event — a click on an exposed scene edge (the
                        // top-left branding region, the coffee machine) must not fall
                        // through to launch a browser or pin a hidden agent (the same
                        // phantom-click class the help/version guards above prevent).
                        // Inert by design: these modals have explicit close keys
                        // (Tab / s / t / Esc), so a click does NOT dismiss them.
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            renderer.set_mouse_pos(Some((m.column, m.row)));
                            // Gate on cached_layout (the wall display only paints
                            // with a layout) so a too-small frame / floor-slide
                            // transition can't phantom-launch — mirrors the arms below.
                            // The ★ CTA's click target is the PRECISE painted span
                            // (`star_hit_rect`, derived from the same board geometry),
                            // not the old loose top-left row (C9).
                            let on_star = renderer.cached_layout().is_some()
                                && crossterm::terminal::size()
                                    .ok()
                                    .is_some_and(|(cols, rows)| {
                                        let scene = renderer::scene_rect(ratatui::layout::Rect {
                                            x: 0,
                                            y: 0,
                                            width: cols,
                                            height: rows,
                                        });
                                        widgets::star_hit_rect(scene).is_some_and(|s| {
                                            s.contains(ratatui::layout::Position {
                                                x: m.column,
                                                y: m.row,
                                            })
                                        })
                                    });
                            if on_star {
                                let _ = open::that(widgets::REPO_URL);
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
                                } else {
                                    focus_clicked_agent(
                                        &mut renderer,
                                        &scene_rx,
                                        &focus_roots,
                                        m.column,
                                        m.row,
                                        now,
                                    );
                                }
                            } else {
                                focus_clicked_agent(
                                    &mut renderer,
                                    &scene_rx,
                                    &focus_roots,
                                    m.column,
                                    m.row,
                                    now,
                                );
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
            // The frame-pacing sleep doubles as the signal-listen window (the
            // crossterm poll above is synchronous; this is the loop's only
            // await point). An external signal breaks to the SAME teardown
            // below that `q` reaches.
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
mod runtime_model {
    // Pins WHY #603 removed the wraps — see the tui/CLAUDE.md `block_on` sharp
    // edge for the full model. Deterministic: worker_threads(1) + a std channel
    // happens-before, no sleeps.
    #[test]
    fn block_in_place_is_inert_on_the_block_on_thread() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("multi-thread runtime");
        rt.block_on(async {
            // A spawned task = the reducer/sources analog on its OWN worker thread.
            let (tx, rx) = std::sync::mpsc::channel::<u8>();
            tokio::spawn(async move {
                tx.send(1).expect("send");
            });
            // The block_on thread (= run_tui) blocks SYNCHRONOUSLY like an inline
            // flock. WRAPPED in block_in_place: must not panic, returns the value.
            let got = tokio::task::block_in_place(|| rx.recv().expect("recv"));
            assert_eq!(
                got, 1,
                "the spawned worker progressed while the loop blocked"
            );
            // WITHOUT the wrap — observably identical, so the wrap is a no-op.
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
    use super::{connect_source, disconnect_source, dispatch_key, FloorNav, KeyAction, ModalState};
    use crossterm::event::{KeyCode, KeyModifiers};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    // Default: no overlay open, theme-picker count of 6.
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

    // Default: normal scene, mid-stack floor (1 of 3), no transition.
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
        // The arc fix (#3): a connected-but-CLI-absent NoCli row stays disconnectable.
        assert_eq!(
            toggle_intent(ConnState::NoCli { connected: true }),
            ToggleIntent::ArmConfirm
        );
        // A never-connected absent CLI is inert — the connect-side refusal only.
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
            dispatch_key(KeyCode::Char('t'), NONE, modal(), nav()),
            KeyAction::OpenThemePicker
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('?'), NONE, modal(), nav()),
            KeyAction::ToggleHelp
        );
        // `w` only maps in debug builds; in release it falls through to None.
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
        // Mid-stack: up and down both valid.
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
        // Top floor: no up.
        let top = FloorNav {
            current_floor: 2,
            ..nav()
        };
        assert_eq!(
            dispatch_key(KeyCode::Up, NONE, modal(), top),
            KeyAction::None
        );
        // Bottom floor: no down.
        let bottom = FloorNav {
            current_floor: 0,
            ..nav()
        };
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, modal(), bottom),
            KeyAction::None
        );
        // A transition in flight blocks navigation in both directions.
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
        // help wins even when the version popup is also flagged.
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
        // Up does not leak to the floor-nav / picker handlers while help is open.
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c, nav()), KeyAction::None);
    }

    #[test]
    fn onboarding_is_top_precedence_and_maps_its_keys() {
        // Onboarding sits ABOVE every other overlay (help/version/connection all
        // flagged) — the version-popup-lockstep precedence class.
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
        // The quit chord still escapes; every other key is SWALLOWED (it must not
        // leak to the help / connection handlers flagged open underneath).
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
        // A floor key while the popup is up is swallowed, not navigated.
        assert_eq!(dispatch_key(KeyCode::Up, NONE, c, nav()), KeyAction::None);
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
        // The quit chord passes through like EVERY other modal tier (the run_tui
        // quit arm reverts the previewed theme before breaking) — the picker
        // used to be the one tier that swallowed Ctrl+C entirely.
        assert_eq!(
            dispatch_key(KeyCode::Char('q'), NONE, c, nav()),
            KeyAction::Quit
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, c, nav()),
            KeyAction::Quit
        );
        // Non-chord keys are still swallowed (modal).
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, c, nav()),
            KeyAction::None
        );

        // Clamp at the ends.
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
        // help / version / theme-picker tiers precede the normal Tab binding.
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
        // Bare `c` is now UNbound (the panel moved to `s`); Ctrl+C stays quit.
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
        // `t` is the single connect/disconnect toggle (replaced i/u, then Enter).
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, s, nav()),
            KeyAction::ConnectionToggle
        );
        // The old install/uninstall keys + Enter are unbound in the panel now.
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
        // Quit chord passes through; unarmed swallows y/n.
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
        // Armed swallows navigation + action keys.
        for k in [
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('i'),
            KeyCode::Char('u'),
        ] {
            assert_eq!(dispatch_key(k, NONE, s, nav()), KeyAction::None);
        }
        // Quit chord still quits even while armed.
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, s, nav()),
            KeyAction::Quit
        );
    }

    #[test]
    fn connection_precedence_help_version_win_and_connection_swallows_tab() {
        // help / version tiers precede the connection tier — bare `c` does nothing.
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
        // connection precedes dashboard: with connection open, Tab is swallowed.
        let s = ModalState {
            connection_open: true,
            ..modal()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, s, nav()), KeyAction::None);
    }

    // --- connect/disconnect persist-or-abort (no-target Antigravity path) ------

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
        // A regular file used as a directory component → the config write's
        // create-parent-dir fails → save_source_connected errs.
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

    // The install-failure rollback is now tested at the core
    // (`sources::connect_target_rolls_the_flag_back_when_install_fails`) — the
    // panel just delegates to `crate::sources::connect`.

    // --- onboarding outcome → live-gate mapping --------------------------------

    #[test]
    fn onboarding_noop_outcome_keeps_the_desired_gate_state() {
        use crate::sources::ChangeOutcome;
        // A NoOp for a CHECKED row means "already connected — nothing written":
        // the live gate must stay OPEN, never be hardcoded closed (which would
        // evict the source's live agents the user just confirmed).
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
    fn onboarding_skip_reflects_its_freeze_into_the_live_gate() {
        use crate::sources::ChangeOutcome;
        // Regression: the SKIP arm must reflect its freeze into the live gate
        // like Confirm. A pre-0.12 upgrader boots with an EMPTY gate; skip
        // freezes their hooked source `true`, and apply_choices issues a Connect
        // whose semantic-no-op re-install yields `Connected` — the outcome the
        // skip path actually emits (apply_choices maps every want to
        // Connect/Disconnect, never NoOp). Reflecting it must OPEN the gate this
        // session, else their office stays empty until the next restart re-seeds
        // from the (now true) flags. (The arm's one-line call to reflect lives
        // in the codecov-excluded run_tui loop; this pins the reflect branch it
        // drives, as the Confirm-arm tests do.)
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
