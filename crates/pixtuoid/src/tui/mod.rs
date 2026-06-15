pub mod anim;
pub mod chitchat;
pub mod connection;
pub mod dashboard;
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
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
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
    dashboard_open: bool,
    connection_open: bool,
    /// Whether the Sources panel has a disconnect armed (awaiting y/n). Splits the
    /// open-connection dispatch into the armed (y/n only) vs unarmed (nav/toggle) sub-tiers.
    connection_confirm: bool,
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
        // Project to the VISIBLE floor first: `hit_test_from_tui` requires a
        // single-floor scene matching `cached_layout` (a raw multi-floor
        // snapshot would test other floors' agents against this floor's
        // desks — the global/local desk-index confusion, see its doc).
        let floor_scene = floor::project_floor_scene(&snap, renderer.current_floor());
        let hit = renderer
            .cached_layout()
            .and_then(|layout| renderer::hit_test_from_tui(&floor_scene, layout, col, row));
        renderer.set_pinned_agent(hit);
    }
}

/// Connect a source from the panel: PERSIST the intent FIRST (so it survives
/// restart), then — only if that succeeded — open the live gate (`connected.set`
/// → the reducer task's reconciler stops evicting it + its events flow) and, for
/// target-bearing sources, install its hooks. If the persist fails we abort
/// loudly and flip NOTHING: opening a gate the next restart can't remember would
/// silently re-evict the sprites the user just connected. Returns the panel result.
fn connect_source(
    config_path: &std::path::Path,
    connected: &crate::runtime::ConnectedSources,
    source_id: &'static str,
    target: Option<&'static crate::install::target::Target>,
    display_name: &str,
) -> String {
    if let Err(e) = crate::config::save_source_connected(config_path, source_id, true) {
        tracing::warn!("failed to persist connection for {source_id}: {e:#}");
        return format!(
            "{display_name}: connect failed \u{2014} couldn't save config \u{2014} {e:#}"
        );
    }
    connected.set(source_id, true);
    match target {
        Some(t) => match crate::install::install_target(t, None, None) {
            Ok(r) => connection::format_connect_result(&r, display_name),
            // The hooks did NOT install — roll back the flag + gate so the next
            // restart doesn't honor a persisted "connected" with no integration
            // behind it (a hook-only source would show connected yet never
            // produce an agent). The rollback save writes the same path the
            // first save just succeeded on, so it's reliable.
            Err(e) => {
                let _ = crate::config::save_source_connected(config_path, source_id, false);
                connected.set(source_id, false);
                format!("{display_name}: connect failed \u{2014} {e:#}")
            }
        },
        None => format!("\u{2713} {display_name} connected"),
    }
}

/// Disconnect a source from the panel: PERSIST the intent FIRST, then — only if
/// that succeeded — close the live gate (`connected.set(false)` → the reducer
/// task's reconciler walks its characters out on the next tick) and remove its
/// hooks. Same persist-or-abort rule as connect: a runtime hide the next restart
/// reverts (the saved flag still says connected) is a lie, so on a persist
/// failure flip NOTHING and report it. Returns the panel result.
fn disconnect_source(
    config_path: &std::path::Path,
    connected: &crate::runtime::ConnectedSources,
    source_id: &'static str,
    target: Option<&'static crate::install::target::Target>,
    display_name: &str,
) -> String {
    if let Err(e) = crate::config::save_source_connected(config_path, source_id, false) {
        tracing::warn!("failed to persist disconnection for {source_id}: {e:#}");
        return format!(
            "{display_name}: disconnect failed \u{2014} couldn't save config \u{2014} {e:#}"
        );
    }
    connected.set(source_id, false);
    match target {
        Some(t) => match crate::install::uninstall_target(t, None) {
            Ok(r) => connection::format_disconnect_result(&r, display_name),
            Err(e) => format!("{display_name}: disconnect failed \u{2014} {e:#}"),
        },
        None => format!("\u{2713} {display_name} disconnected"),
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
    if ctx.connection_open {
        // Armed sub-tier: a uninstall is awaiting confirmation — only y/n/Esc
        // (and the quit chord) act; nav/action keys are swallowed.
        if ctx.connection_confirm {
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
    if ctx.dashboard_open {
        return match (code, mods) {
            _ if is_quit_chord(code, mods) => KeyAction::Quit,
            (KeyCode::Esc, _) | (KeyCode::Tab, _) => KeyAction::DashboardClose,
            (KeyCode::Enter, _) => KeyAction::DashboardJump,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => KeyAction::DashboardUp,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => KeyAction::DashboardDown,
            (KeyCode::Left, _) | (KeyCode::Char('h'), _) => KeyAction::DashboardFoldLeft,
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => KeyAction::DashboardFoldRight,
            (KeyCode::Char('z'), _) => KeyAction::DashboardFoldAll,
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
        KeyCode::Tab => KeyAction::ToggleDashboard,
        // `s` opens the Sources panel (connection + health + live). Renamed from
        // `c`/"Connection" once the panel grew past bind/unbind into a per-source
        // board; `Ctrl+C` stays the quit chord (handled above).
        KeyCode::Char('s') => KeyAction::ToggleConnection,
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
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
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

#[allow(clippy::too_many_arguments)]
pub async fn run_tui(
    mut scene_rx: SceneRx,
    pack_dir: Option<std::path::PathBuf>,
    floor_caps: Arc<[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS]>,
    theme: &'static theme::Theme,
    config_path: std::path::PathBuf,
    desk_cap: Option<usize>,
    pets: Vec<pet::Pet>,
    mut source_health: tokio::sync::watch::Receiver<
        Vec<pixtuoid_core::source::manager::SourceDeath>,
    >,
    // The resolved hook socket (Unix) / named pipe (Windows) the daemon bound,
    // shown in the Sources panel's connection line.
    socket_path: std::path::PathBuf,
    // The live connected-source set — the Sources panel's mutation seam: a
    // toggle calls `connected.set(src, on)`, which the reducer task's reconciler
    // observes (gate + graceful evict). Shared `Arc<Mutex<…>>` with the reducer.
    connected: crate::runtime::ConnectedSources,
    // The warn-floor log path — throttle-scanned for decode-drift breadcrumbs to
    // drive the footer nudge (`main` owns the resolution; `None` = no surfacing).
    log_path: Option<std::path::PathBuf>,
) -> Result<()> {
    let pack = embedded_pack::load_sprite_pack(pack_dir)?;
    let term = setup_terminal()?;
    let mut renderer = TuiRenderer::new(term, theme, pets);
    let mut version_popup = {
        let current_ver = env!("CARGO_PKG_VERSION");
        // Post-altscreen re-load for last_seen_version only — any config
        // warning was already surfaced by main's pre-altscreen pass.
        let cfg = crate::config::load(&config_path, &mut Vec::new());
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
    let mut dashboard_ui = dashboard::DashboardUi::default();
    let mut connection_ui = connection::ConnectionUi::default();
    // Live decode-drift footer nudge: throttle-scan the warn-floor log (reusing
    // doctor's tested scanner) at most every ~15s, NOT per frame.
    let mut last_drift_scan: Option<std::time::Instant> = None;
    let mut drifted_prefixes: Vec<String> = Vec::new();
    // The Sources panel's cached rows carry a per-source HEALTH summary
    // (install soundness + drift) computed on open/toggle; it scans the warn-floor
    // log, so read it fresh at each (infrequent) rebuild. `""` when no log path.
    let read_conn_log = || {
        log_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    };

    let tick = Duration::from_millis(33);
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
            // Capture the health snapshot ONCE this frame — both the footer
            // warning and the Sources panel's per-source `dead` flag read it.
            let health = source_health.borrow_and_update().clone();
            // Throttled drift re-scan (≤ every 15s) — reuse doctor's tested
            // scanner; the source-death warning still preempts it in the merge.
            // This is the ONE deliberate exception to "no scan-the-history": it
            // derives a passive diagnostic nudge from the log artifact, NOT
            // lifecycle state (the no-history rule guards the reducer). A counting
            // tracing::Layer was rejected — it would add stateful blast radius to
            // the single global file subscriber for a hint the 15s scan covers.
            if let Some(lp) = &log_path {
                let due = last_drift_scan.is_none_or(|t| t.elapsed().as_secs() >= 15);
                if due {
                    last_drift_scan = Some(std::time::Instant::now());
                    drifted_prefixes = std::fs::read_to_string(lp)
                        .map(|log| crate::doctor::drifted_sources(&log))
                        .unwrap_or_default();
                }
            }
            renderer.set_source_warning(crate::doctor::footer_warning(
                widgets::source_warning_message(&health).as_deref(),
                &drifted_prefixes,
            ));
            // Mirror the dashboard frame: while open, rebuild the rows from the
            // live snapshot, re-anchor the selection by AgentId (an agent may
            // have exited), and keep it in the scroll viewport. Closed → push an
            // empty frame (the painter reads rows only when open).
            if dashboard_ui.open {
                let rows = dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                dashboard_ui.selected = dashboard::reanchor_selection(&rows, dashboard_ui.selected);
                dashboard_ui.scroll = dashboard::clamp_scroll(
                    &rows,
                    dashboard_ui.selected,
                    dashboard_ui.scroll,
                    dashboard::DASHBOARD_VIEWPORT_ROWS,
                );
                renderer.set_dashboard_frame(
                    true,
                    rows,
                    dashboard_ui.selected,
                    dashboard_ui.scroll,
                );
            } else {
                renderer.set_dashboard_frame(false, Vec::new(), dashboard_ui.selected, 0);
            }
            // Mirror the Connection frame: the HOOK facet (`connection_ui.rows`) is cached
            // (rebuilt on open + after actions, NOT per frame — it does FS reads);
            // only the LIVE facet + socket line recompute here from the snapshot.
            if connection_ui.open {
                connection_ui.selected =
                    connection::move_selection(&connection_ui.rows, connection_ui.selected, 0);
                let live = connection::live_view(now, &connection_ui.rows, &snapshot, &health);
                let socket_line = format!("socket  {}  (listening)", socket_path.display());
                renderer.set_connection_frame(
                    true,
                    connection_ui.rows.clone(),
                    live,
                    connection_ui.selected,
                    connection_ui.confirm,
                    connection_ui.last_result.clone(),
                    socket_line,
                );
            } else {
                renderer.set_connection_frame(
                    false,
                    Vec::new(),
                    Vec::new(),
                    0,
                    None,
                    None,
                    String::new(),
                );
            }
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
                    // Windows delivers Press AND Release events per
                    // keystroke; without this guard every key double-fires
                    // there (e.g. `p` pauses then instantly unpauses).
                    Event::Key(k) if should_dispatch_key(k.kind) => {
                        let ctx = KeyCtx {
                            help_open: renderer.help_open(),
                            version_popup,
                            theme_picker,
                            dashboard_open: dashboard_ui.open,
                            connection_open: connection_ui.open,
                            connection_confirm: connection_ui.confirm.is_some(),
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
                            KeyAction::ToggleDashboard => {
                                dashboard_ui.open = !dashboard_ui.open;
                                if dashboard_ui.open {
                                    let rows = dashboard::build_dashboard_rows(
                                        &snapshot,
                                        &dashboard_ui.folds,
                                    );
                                    dashboard_ui.selected =
                                        dashboard::reanchor_selection(&rows, dashboard_ui.selected);
                                }
                            }
                            KeyAction::DashboardClose => dashboard_ui.open = false,
                            KeyAction::DashboardUp => {
                                let rows =
                                    dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                                dashboard_ui.selected =
                                    dashboard::move_selection(&rows, dashboard_ui.selected, -1);
                            }
                            KeyAction::DashboardDown => {
                                let rows =
                                    dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                                dashboard_ui.selected =
                                    dashboard::move_selection(&rows, dashboard_ui.selected, 1);
                            }
                            KeyAction::DashboardFoldLeft => {
                                let rows =
                                    dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                                if let Some(sel) = dashboard_ui.selected {
                                    if let Some(row) = rows.iter().find(|r| r.agent_id == sel) {
                                        // On a child, collapse its parent and move the
                                        // cursor up to the (now collapsed) root so it
                                        // stays visible; on a root, collapse it.
                                        let root = row.parent_id.unwrap_or(sel);
                                        dashboard_ui.folds.fold_all([root]);
                                        dashboard_ui.selected = Some(root);
                                    }
                                }
                            }
                            KeyAction::DashboardFoldRight => {
                                let rows =
                                    dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                                if let Some(sel) = dashboard_ui.selected {
                                    // Only roots are collapsible; expand the selected one.
                                    if rows
                                        .iter()
                                        .any(|r| r.agent_id == sel && r.parent_id.is_none())
                                    {
                                        dashboard_ui.folds.unfold_all([sel]);
                                    }
                                }
                            }
                            KeyAction::DashboardFoldAll => {
                                let rows =
                                    dashboard::build_dashboard_rows(&snapshot, &dashboard_ui.folds);
                                let roots: Vec<_> = rows
                                    .iter()
                                    .filter(|r| r.parent_id.is_none())
                                    .map(|r| r.agent_id)
                                    .collect();
                                let any_expanded =
                                    rows.iter().any(|r| r.parent_id.is_none() && !r.collapsed);
                                if any_expanded {
                                    dashboard_ui.folds.fold_all(roots);
                                } else {
                                    dashboard_ui.folds.unfold_all(roots);
                                }
                            }
                            KeyAction::DashboardJump => {
                                if let Some(sel) = dashboard_ui.selected {
                                    let rows = dashboard::build_dashboard_rows(
                                        &snapshot,
                                        &dashboard_ui.folds,
                                    );
                                    if let Some(floor) = dashboard::resolve_floor(&rows, sel) {
                                        renderer.navigate_floor(floor, now);
                                    }
                                }
                                dashboard_ui.open = false;
                            }
                            KeyAction::ToggleConnection => {
                                connection_ui.open = !connection_ui.open;
                                connection_ui.confirm = None;
                                if connection_ui.open {
                                    // Cached connection facet: FS reads + the
                                    // connected-set snapshot happen HERE (on open)
                                    // + after each toggle, never per frame.
                                    connection_ui.rows = connection::build_rows(
                                        &connected.snapshot(),
                                        &read_conn_log(),
                                    );
                                    connection_ui.selected = connection::move_selection(
                                        &connection_ui.rows,
                                        connection_ui.selected,
                                        0,
                                    );
                                    connection_ui.last_result = None;
                                }
                            }
                            KeyAction::ConnectionUp => {
                                connection_ui.selected = connection::move_selection(
                                    &connection_ui.rows,
                                    connection_ui.selected,
                                    -1,
                                );
                                connection_ui.last_result = None;
                            }
                            KeyAction::ConnectionDown => {
                                connection_ui.selected = connection::move_selection(
                                    &connection_ui.rows,
                                    connection_ui.selected,
                                    1,
                                );
                                connection_ui.last_result = None;
                            }
                            KeyAction::ConnectionToggle => {
                                // Copy the fields out before any rebuild of `rows`
                                // (which would invalidate a `&ConnectionRow` borrow).
                                let action =
                                    connection_ui.rows.get(connection_ui.selected).map(|r| {
                                        (
                                            r.state,
                                            r.target,
                                            r.source_id,
                                            r.display_name,
                                            connection::no_action_hint(r),
                                        )
                                    });
                                if let Some((state, target, source_id, name, hint)) = action {
                                    match state {
                                        // Bound → arm the disconnect confirm (it
                                        // removes hooks + walks characters out).
                                        connection::ConnState::Connected => {
                                            connection_ui.confirm = Some(connection_ui.selected);
                                        }
                                        // Unbound → connect immediately (additive,
                                        // reversible): flip the flag, open the live
                                        // gate, and install hooks for richer signal.
                                        connection::ConnState::Disconnected => {
                                            connection_ui.last_result = Some(connect_source(
                                                &config_path,
                                                &connected,
                                                source_id,
                                                target,
                                                name,
                                            ));
                                            connection_ui.rows = connection::build_rows(
                                                &connected.snapshot(),
                                                &read_conn_log(),
                                            );
                                        }
                                        connection::ConnState::NoCli => {
                                            connection_ui.last_result = Some(hint);
                                        }
                                    }
                                }
                            }
                            KeyAction::ConnectionConfirm => {
                                if let Some(idx) = connection_ui.confirm {
                                    let action = connection_ui
                                        .rows
                                        .get(idx)
                                        .map(|r| (r.target, r.source_id, r.display_name));
                                    if let Some((target, source_id, name)) = action {
                                        connection_ui.last_result = Some(disconnect_source(
                                            &config_path,
                                            &connected,
                                            source_id,
                                            target,
                                            name,
                                        ));
                                        connection_ui.rows = connection::build_rows(
                                            &connected.snapshot(),
                                            &read_conn_log(),
                                        );
                                    }
                                }
                                connection_ui.confirm = None;
                            }
                            KeyAction::ConnectionCancelConfirm => connection_ui.confirm = None,
                            KeyAction::ConnectionClose => {
                                connection_ui.open = false;
                                connection_ui.confirm = None;
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
mod dispatch_tests {
    use super::{connect_source, disconnect_source, dispatch_key, KeyAction, KeyCtx};
    use crossterm::event::{KeyCode, KeyModifiers};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    // Default: normal scene, mid-stack floor (1 of 3), no transition.
    fn ctx() -> KeyCtx {
        KeyCtx {
            help_open: false,
            version_popup: false,
            theme_picker: None,
            dashboard_open: false,
            connection_open: false,
            connection_confirm: false,
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
            dispatch_key(KeyCode::Tab, NONE, ctx()),
            KeyAction::ToggleDashboard
        );
    }

    #[test]
    fn dashboard_tier_maps_nav_fold_jump_close() {
        let d = KeyCtx {
            dashboard_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Up, NONE, d), KeyAction::DashboardUp);
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, d),
            KeyAction::DashboardUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, d),
            KeyAction::DashboardDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, d),
            KeyAction::DashboardDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Left, NONE, d),
            KeyAction::DashboardFoldLeft
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('h'), NONE, d),
            KeyAction::DashboardFoldLeft
        );
        assert_eq!(
            dispatch_key(KeyCode::Right, NONE, d),
            KeyAction::DashboardFoldRight
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('l'), NONE, d),
            KeyAction::DashboardFoldRight
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('z'), NONE, d),
            KeyAction::DashboardFoldAll
        );
        assert_eq!(
            dispatch_key(KeyCode::Enter, NONE, d),
            KeyAction::DashboardJump
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, d),
            KeyAction::DashboardClose
        );
        assert_eq!(
            dispatch_key(KeyCode::Tab, NONE, d),
            KeyAction::DashboardClose
        );
    }

    #[test]
    fn dashboard_modal_passes_quit_chord_but_swallows_other_keys() {
        let d = KeyCtx {
            dashboard_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Char('q'), NONE, d), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('c'), CTRL, d), KeyAction::Quit);
        assert_eq!(
            dispatch_key(KeyCode::Char('p'), NONE, d),
            KeyAction::None,
            "modal swallows pause"
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, d),
            KeyAction::None,
            "modal swallows theme picker"
        );
    }

    #[test]
    fn tab_swallowed_while_other_overlays_open() {
        // help / version / theme-picker tiers precede the normal Tab binding.
        let h = KeyCtx {
            help_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, h), KeyAction::None);
        let v = KeyCtx {
            version_popup: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, v), KeyAction::None);
        let p = KeyCtx {
            theme_picker: Some(0),
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, p), KeyAction::None);
    }

    #[test]
    fn s_opens_sources_panel_from_normal_scene() {
        assert_eq!(
            dispatch_key(KeyCode::Char('s'), NONE, ctx()),
            KeyAction::ToggleConnection
        );
        // Bare `c` is now UNbound (the panel moved to `s`); Ctrl+C stays quit.
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), NONE, ctx()),
            KeyAction::None
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('c'), CTRL, ctx()),
            KeyAction::Quit
        );
    }

    #[test]
    fn connection_tier_maps_nav_toggle_close() {
        let s = KeyCtx {
            connection_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Up, NONE, s), KeyAction::ConnectionUp);
        assert_eq!(
            dispatch_key(KeyCode::Char('k'), NONE, s),
            KeyAction::ConnectionUp
        );
        assert_eq!(
            dispatch_key(KeyCode::Down, NONE, s),
            KeyAction::ConnectionDown
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('j'), NONE, s),
            KeyAction::ConnectionDown
        );
        // `t` is the single connect/disconnect toggle (replaced i/u, then Enter).
        assert_eq!(
            dispatch_key(KeyCode::Char('t'), NONE, s),
            KeyAction::ConnectionToggle
        );
        // The old install/uninstall keys + Enter are unbound in the panel now.
        assert_eq!(dispatch_key(KeyCode::Char('i'), NONE, s), KeyAction::None);
        assert_eq!(dispatch_key(KeyCode::Char('u'), NONE, s), KeyAction::None);
        assert_eq!(dispatch_key(KeyCode::Enter, NONE, s), KeyAction::None);
        assert_eq!(
            dispatch_key(KeyCode::Char('s'), NONE, s),
            KeyAction::ConnectionClose
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, s),
            KeyAction::ConnectionClose
        );
        // Quit chord passes through; unarmed swallows y/n.
        assert_eq!(dispatch_key(KeyCode::Char('q'), NONE, s), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('c'), CTRL, s), KeyAction::Quit);
        assert_eq!(dispatch_key(KeyCode::Char('y'), NONE, s), KeyAction::None);
        assert_eq!(dispatch_key(KeyCode::Char('n'), NONE, s), KeyAction::None);
    }

    #[test]
    fn connection_armed_tier_maps_yn_and_swallows_nav() {
        let s = KeyCtx {
            connection_open: true,
            connection_confirm: true,
            ..ctx()
        };
        assert_eq!(
            dispatch_key(KeyCode::Char('y'), NONE, s),
            KeyAction::ConnectionConfirm
        );
        assert_eq!(
            dispatch_key(KeyCode::Char('n'), NONE, s),
            KeyAction::ConnectionCancelConfirm
        );
        assert_eq!(
            dispatch_key(KeyCode::Esc, NONE, s),
            KeyAction::ConnectionCancelConfirm
        );
        // Armed swallows navigation + action keys.
        for k in [
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('i'),
            KeyCode::Char('u'),
        ] {
            assert_eq!(dispatch_key(k, NONE, s), KeyAction::None);
        }
        // Quit chord still quits even while armed.
        assert_eq!(dispatch_key(KeyCode::Char('c'), CTRL, s), KeyAction::Quit);
    }

    #[test]
    fn connection_precedence_help_version_win_and_connection_swallows_tab() {
        // help / version tiers precede the connection tier — bare `c` does nothing.
        let h = KeyCtx {
            help_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Char('c'), NONE, h), KeyAction::None);
        let v = KeyCtx {
            version_popup: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Char('c'), NONE, v), KeyAction::None);
        // connection precedes dashboard: with connection open, Tab is swallowed.
        let s = KeyCtx {
            connection_open: true,
            ..ctx()
        };
        assert_eq!(dispatch_key(KeyCode::Tab, NONE, s), KeyAction::None);
    }

    // --- connect/disconnect persist-or-abort (no-target Antigravity path) ------

    #[test]
    fn connect_source_persists_then_flips_the_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        let connected = crate::runtime::ConnectedSources::default();

        let res = connect_source(&cfg, &connected, "antigravity", None, "Antigravity");
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

        let res = disconnect_source(&cfg, &connected, "antigravity", None, "Antigravity");
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

        let res = connect_source(&cfg, &connected, "antigravity", None, "Antigravity");
        assert!(res.contains("failed"), "must report the failure: {res}");
        assert!(
            !connected.is_connected("antigravity"),
            "a failed persist must NOT open the gate (else restart re-evicts)"
        );
    }

    // A target whose install ALWAYS fails (its default_config_path errs, so
    // install_target bails before any FS) — to exercise connect_source's
    // install-failure rollback deterministically + cross-platform.
    static FAIL_TARGET: crate::install::target::Target = crate::install::target::Target {
        name: "rollbacktest",
        core_source: "rollbacktest",
        display_name: "RollbackTest",
        restart_noun: "RollbackTest",
        default_config_path: || Err(anyhow::anyhow!("forced install failure")),
        hook_command: |_, _| Ok(String::new()),
        merge_install: |c, _| {
            Ok(crate::install::target::MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(crate::install::target::MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        needs_path_warning: false,
        needs_resolved_binary: false,
        post_install_note: None,
        presence_probe: None,
    };

    #[test]
    fn connect_source_rolls_back_gate_and_flag_when_install_target_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        let connected = crate::runtime::ConnectedSources::default();

        // Save succeeds (writable cfg) so the gate opens, THEN install_target
        // fails → connect_source must undo both.
        let res = connect_source(
            &cfg,
            &connected,
            "rollbacktest",
            Some(&FAIL_TARGET),
            "RollbackTest",
        );
        assert!(res.contains("failed"), "must report the failure: {res}");
        assert!(
            !connected.is_connected("rollbacktest"),
            "install failure must roll the gate back closed"
        );
        // The persisted flag is rolled back to false too (no shown-but-broken
        // source surviving the next restart).
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("rollbacktest") && written.contains("false"),
            "the flag was rolled back to false: {written}"
        );
    }
}
