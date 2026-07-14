//! `run_tui`'s per-surface UI state, lifted out of the event loop's loose
//! mutable locals into ONE struct: [`UiState`] owns each modal surface's
//! open/close transitions, projects the dispatch-facing [`ModalState`]
//! ([`UiState::modal`] — one source of truth instead of an ad-hoc literal per
//! key event), and computes the per-frame renderer mirrors
//! ([`UiState::build_frames`] → [`RenderFrames`]). `run_tui` keeps the event
//! loop, the terminal lifecycle, and every renderer/config/install side
//! effect; the blocking-I/O sites (`build_rows`, connect/disconnect, the
//! onboarding apply) stay at the loop as brief inline stalls (block_in_place
//! removed in #603 — inert on the block_on thread; see the tui/CLAUDE.md sharp
//! edge) and reach the state through these methods' closure parameters or the
//! `pub(crate)` fields.

use std::time::{Instant, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::SceneState;
use pixtuoid_scene::theme;

use super::{connection, dashboard, welcome, widgets, ModalState};
use connection::{ConnectionFrame, ConnectionRow, ConnectionUi};
use dashboard::{DashboardFrame, DashboardUi};
use welcome::{OnboardingFrame, WelcomeUi};

/// The throttled (≤ every 15s) decode-drift re-scan that drives the footer
/// nudge: reuses doctor's tested scanner over the warn-floor log. The ONE
/// deliberate exception to "no scan-the-history" — it derives a passive
/// diagnostic nudge from the log artifact, NOT lifecycle state. A `None` log
/// path is a no-op (headless = no surfacing).
///
/// INCREMENTAL: log rotation is startup-only, so within a session the file is
/// append-only and unbounded (a sustained drift regime warns per tool call).
/// Re-reading the WHOLE file every 15s inline in the ~30fps loop turns that
/// growth into monotonically-worsening frame hitches + a log-sized transient
/// allocation — so the scan keeps persistent state (byte offset + accumulated
/// prefixes) and each pass reads ONLY the appended bytes. Drift breadcrumbs are
/// monotone within a session (they never un-happen), so prefixes accumulate.
#[derive(Default)]
struct DriftScan {
    last_scan: Option<Instant>,
    /// End of the last fully-scanned LINE — never mid-line: a read boundary can
    /// split a breadcrumb, so a partial trailing line waits for the next pass.
    offset: u64,
    /// Accumulated drifted label prefixes — what the footer merge reads.
    drifted: Vec<String>,
}

impl DriftScan {
    /// The per-frame call site: throttled to at most one scan per interval.
    fn rescan(&mut self, log_path: &Option<std::path::PathBuf>) {
        let Some(lp) = log_path else { return };
        // Throttle: rescan the log for decode-drift breadcrumbs at most this often.
        const DRIFT_RESCAN_INTERVAL_SECS: u64 = 15;
        let due = self
            .last_scan
            .is_none_or(|t| t.elapsed().as_secs() >= DRIFT_RESCAN_INTERVAL_SECS);
        if due {
            self.last_scan = Some(Instant::now());
            self.scan_appended(lp);
        }
    }

    /// One unthrottled incremental pass: read from the stored offset to EOF,
    /// scan the COMPLETE new lines, merge the drifted prefixes. A file shrunk
    /// out from under us (external rotation/truncation) rescans from the top.
    /// Any I/O error leaves the state unchanged for the next pass.
    fn scan_appended(&mut self, lp: &std::path::Path) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(lp) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut new = Vec::with_capacity((len - self.offset) as usize);
        if f.take(len - self.offset).read_to_end(&mut new).is_err() {
            return;
        }
        // Only complete lines: a partial tail (mid-write) stays for next pass.
        let Some(last_nl) = new.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let text = String::from_utf8_lossy(&new[..=last_nl]);
        self.offset += (last_nl + 1) as u64;
        for p in crate::doctor::drifted_sources(&text) {
            if !self.drifted.contains(&p) {
                self.drifted.push(p);
            }
        }
    }
}

/// One frame's renderer mirrors, computed by [`UiState::build_frames`] and
/// pushed onto the [`TuiRenderer`](super::TuiRenderer) by
/// [`RenderFrames::apply_to`] — bundling them keeps the compute (here) and the
/// push (one call in the loop) from drifting apart per surface.
pub(crate) struct RenderFrames {
    theme_picker: Option<usize>,
    version_popup: bool,
    help_open: bool,
    source_warning: Option<String>,
    dashboard: DashboardFrame,
    connection: ConnectionFrame,
    onboarding: OnboardingFrame,
}

impl RenderFrames {
    pub(crate) fn apply_to<B: ratatui::backend::Backend<Error: Send + Sync + 'static>>(
        self,
        renderer: &mut super::TuiRenderer<B>,
        now: SystemTime,
    ) {
        renderer.set_theme_picker(self.theme_picker);
        renderer.set_version_popup(self.version_popup, now);
        renderer.set_help_open(self.help_open);
        renderer.set_source_warning(self.source_warning);
        renderer.set_dashboard_frame(self.dashboard);
        renderer.set_connection_frame(self.connection);
        renderer.set_onboarding_frame(self.onboarding);
    }
}

/// The per-surface UI state `run_tui` used to hold as one loose mutable-local
/// cluster per surface. Fields stay `pub(crate)` where the loop's I/O arms
/// read/write them directly (the move-only discipline: state lives here,
/// side effects stay in the loop).
pub(crate) struct UiState {
    // First-run onboarding "move-in" overlay (TOP of the modal precedence
    // chain). The overlay is "open" exactly while `onboarding_opened_at` is
    // `Some` (also the clock its painter's typewriter reads); confirm/skip
    // clears it to `None` and starts the close fade (`onboarding_closing_at`:
    // the office dims back UP over `welcome::DIM_FADE_OUT_MS` after the card
    // is gone, then clears to fully live).
    pub(crate) onboarding_ui: WelcomeUi,
    onboarding_opened_at: Option<Instant>,
    onboarding_closing_at: Option<Instant>,
    /// The "what's new in vX" version popup.
    version_popup: bool,
    /// `?` help overlay. Owned HERE (the projection's one source of truth);
    /// the renderer's copy is a per-frame mirror like every other overlay.
    help_open: bool,
    /// `[p]ause`: while paused, `now()` returns the frozen instant so every
    /// clock-driven animation (and the dashboard marquee) holds still.
    paused: bool,
    frozen_now: Option<SystemTime>,
    /// Theme picker: `Some(preview index)` while open; `saved_theme_idx` is
    /// the committed selection the quit/cancel paths revert to.
    pub(crate) theme_picker: Option<usize>,
    pub(crate) saved_theme_idx: usize,
    pub(crate) dashboard: DashboardUi,
    pub(crate) connection: ConnectionUi,
    drift_scan: DriftScan,
    /// The resolved hook socket path — the Sources panel's connection line.
    socket_path: std::path::PathBuf,
    /// The warn-floor log path the drift re-scan reads (`None` = no surfacing).
    log_path: Option<std::path::PathBuf>,
}

impl UiState {
    pub(crate) fn new(
        boot_theme: &'static theme::Theme,
        onboarding_ui: WelcomeUi,
        version_popup: bool,
        socket_path: std::path::PathBuf,
        log_path: Option<std::path::PathBuf>,
    ) -> Self {
        let onboarding_opened_at = (!onboarding_ui.is_empty()).then(Instant::now);
        let saved_theme_idx = theme::ALL_THEMES
            .iter()
            .position(|t| std::ptr::eq(*t, boot_theme))
            .unwrap_or(0);
        Self {
            onboarding_ui,
            onboarding_opened_at,
            onboarding_closing_at: None,
            version_popup,
            help_open: false,
            paused: false,
            frozen_now: None,
            theme_picker: None,
            saved_theme_idx,
            dashboard: DashboardUi::default(),
            connection: ConnectionUi::default(),
            drift_scan: DriftScan::default(),
            socket_path,
            log_path,
        }
    }

    /// The dispatch-facing modal snapshot — THE projection (`dispatch_key`'s
    /// precedence chain reads exactly this; no surface's open flag has a
    /// second home).
    pub(crate) fn modal(&self) -> ModalState {
        ModalState {
            onboarding_open: self.onboarding_open(),
            help_open: self.help_open,
            version_popup: self.version_popup,
            theme_picker: self.theme_picker,
            dashboard_open: self.dashboard.open,
            connection_open: self.connection.open,
            connection_confirm: self.connection.confirm.is_some(),
            n_themes: theme::ALL_THEMES.len(),
        }
    }

    /// This frame's wall clock: real time, or the frozen instant while paused.
    pub(crate) fn now(&mut self) -> SystemTime {
        if self.paused {
            *self.frozen_now.get_or_insert(SystemTime::now())
        } else {
            self.frozen_now = None;
            SystemTime::now()
        }
    }

    // --- open/close + per-surface transitions ------------------------------

    pub(crate) fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    pub(crate) fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
    }

    pub(crate) fn close_help(&mut self) {
        self.help_open = false;
    }

    pub(crate) fn help_open(&self) -> bool {
        self.help_open
    }

    pub(crate) fn dismiss_version_popup(&mut self) {
        self.version_popup = false;
    }

    pub(crate) fn open_theme_picker(&mut self) {
        self.theme_picker = Some(self.saved_theme_idx);
    }

    pub(crate) fn preview_theme(&mut self, idx: usize) {
        self.theme_picker = Some(idx);
    }

    pub(crate) fn commit_theme(&mut self, idx: usize) {
        self.saved_theme_idx = idx;
        self.theme_picker = None;
    }

    /// Esc in the picker: close and return the saved index to revert to.
    pub(crate) fn cancel_theme(&mut self) -> usize {
        self.theme_picker = None;
        self.saved_theme_idx
    }

    pub(crate) fn toggle_dashboard(&mut self, scene: &SceneState) {
        self.dashboard.open = !self.dashboard.open;
        if self.dashboard.open {
            let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
            self.dashboard.selected = dashboard::reanchor_selection(&rows, self.dashboard.selected);
        }
    }

    pub(crate) fn close_dashboard(&mut self) {
        self.dashboard.open = false;
    }

    pub(crate) fn dashboard_move(&mut self, scene: &SceneState, delta: i32) {
        let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
        self.dashboard.selected = dashboard::move_selection(&rows, self.dashboard.selected, delta);
    }

    /// `←/h`: on a child, collapse its parent and move the cursor up to the
    /// (now collapsed) root so it stays visible; on a root, collapse it.
    pub(crate) fn dashboard_fold_left(&mut self, scene: &SceneState) {
        let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
        if let Some(sel) = self.dashboard.selected {
            if let Some(row) = rows.iter().find(|r| r.agent_id == sel) {
                let root = row.parent_id.unwrap_or(sel);
                self.dashboard.folds.fold_all([root]);
                self.dashboard.selected = Some(root);
            }
        }
    }

    /// `→/l`: only roots are collapsible; expand the selected one.
    pub(crate) fn dashboard_fold_right(&mut self, scene: &SceneState) {
        let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
        if let Some(sel) = self.dashboard.selected {
            if rows
                .iter()
                .any(|r| r.agent_id == sel && r.parent_id.is_none())
            {
                self.dashboard.folds.unfold_all([sel]);
            }
        }
    }

    /// `z`: fold-all / unfold-all toggle across every root.
    pub(crate) fn dashboard_fold_all(&mut self, scene: &SceneState) {
        let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
        let roots: Vec<_> = rows
            .iter()
            .filter(|r| r.parent_id.is_none())
            .map(|r| r.agent_id)
            .collect();
        let any_expanded = rows.iter().any(|r| r.parent_id.is_none() && !r.collapsed);
        if any_expanded {
            self.dashboard.folds.fold_all(roots);
        } else {
            self.dashboard.folds.unfold_all(roots);
        }
    }

    /// `Enter`: close and return the selected agent's floor (if resolvable)
    /// for the loop to navigate to.
    pub(crate) fn dashboard_jump(&mut self, scene: &SceneState) -> Option<usize> {
        let floor = self.dashboard.selected.and_then(|sel| {
            let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
            dashboard::resolve_floor(&rows, sel)
        });
        self.dashboard.open = false;
        floor
    }

    /// `f` in the dashboard: the selected row's agent, for the focus-jump
    /// (the panel STAYS open — focusing a terminal is a glance-and-return
    /// action, unlike Enter's floor navigation which closes to show it).
    pub(crate) fn dashboard_focus(&self) -> Option<pixtuoid_core::AgentId> {
        self.dashboard.selected
    }

    /// `s` on a closed panel: open with the freshly rebuilt connection facet.
    /// The rows' blocking I/O (`build_rows` — FS probes + per-source `diagnose`)
    /// runs at the loop inline (a brief stall, #603) and is handed in; it happens
    /// ON OPEN only, never per frame.
    pub(crate) fn open_connection(&mut self, rows: Vec<ConnectionRow>) {
        self.connection.open = true;
        self.connection.confirm = None;
        self.connection.rows = rows;
        self.connection.selected =
            connection::move_selection(&self.connection.rows, self.connection.selected, 0);
        self.connection.last_result = None;
    }

    pub(crate) fn connection_move(&mut self, delta: i32) {
        self.connection.selected =
            connection::move_selection(&self.connection.rows, self.connection.selected, delta);
        self.connection.last_result = None;
    }

    pub(crate) fn cancel_connection_confirm(&mut self) {
        self.connection.confirm = None;
    }

    pub(crate) fn close_connection(&mut self) {
        self.connection.open = false;
        self.connection.confirm = None;
    }

    pub(crate) fn onboarding_open(&self) -> bool {
        self.onboarding_opened_at.is_some()
    }

    /// Confirm/skip both end the overlay the same way: card gone, close fade
    /// armed (the office keeps dimming back up for a beat).
    pub(crate) fn close_onboarding(&mut self) {
        self.onboarding_opened_at = None;
        self.onboarding_closing_at = Some(Instant::now());
    }

    // --- the per-frame renderer mirrors -------------------------------------

    /// Compute this frame's renderer mirrors from the live scene + health
    /// snapshot. Mutates the pieces that are themselves per-frame state: the
    /// throttled drift re-scan, the dashboard reanchor/scroll clamp, the
    /// connection selection clamp, and the onboarding close-fade expiry.
    pub(crate) fn build_frames(
        &mut self,
        now: SystemTime,
        scene: &SceneState,
        health: &[SourceDeath],
    ) -> RenderFrames {
        // Throttled drift re-scan (≤ every 15s) — reuse doctor's tested
        // scanner; the source-death warning still preempts it in the merge.
        // This is the ONE deliberate exception to "no scan-the-history": it
        // derives a passive diagnostic nudge from the log artifact, NOT
        // lifecycle state (the no-history rule guards the reducer). A counting
        // tracing::Layer was rejected — it would add stateful blast radius to
        // the single global file subscriber for a hint the 15s scan covers.
        self.drift_scan.rescan(&self.log_path);
        let source_warning = crate::doctor::footer_warning(
            widgets::source_warning_message(health).as_deref(),
            &self.drift_scan.drifted,
        );

        // Mirror the dashboard frame: while open, rebuild the rows from the
        // live snapshot, re-anchor the selection by AgentId (an agent may
        // have exited), and keep it in the scroll viewport. Closed → an
        // empty frame (the painter reads rows only when open).
        let dashboard_frame = if self.dashboard.open {
            let rows = dashboard::build_dashboard_rows(scene, &self.dashboard.folds);
            self.dashboard.selected = dashboard::reanchor_selection(&rows, self.dashboard.selected);
            self.dashboard.scroll = dashboard::clamp_scroll(
                &rows,
                self.dashboard.selected,
                self.dashboard.scroll,
                dashboard::DASHBOARD_VIEWPORT_ROWS,
            );
            DashboardFrame {
                open: true,
                rows,
                selected: self.dashboard.selected,
                scroll: self.dashboard.scroll,
            }
        } else {
            DashboardFrame::default()
        };

        // Mirror the Connection frame: the HOOK facet (`connection.rows`) is
        // cached (rebuilt on open + after actions, NOT per frame — it does FS
        // reads); only the LIVE facet + socket line recompute here from the
        // snapshot.
        let connection_frame = if self.connection.open {
            self.connection.selected =
                connection::move_selection(&self.connection.rows, self.connection.selected, 0);
            let live = connection::live_view(now, &self.connection.rows, scene, health);
            let socket_line = format!("socket  {}  (listening)", self.socket_path.display());
            ConnectionFrame {
                open: true,
                rows: self.connection.rows.clone(),
                live,
                selected: self.connection.selected,
                confirm: self.connection.confirm,
                result: self.connection.last_result.clone(),
                socket_line,
            }
        } else {
            ConnectionFrame::default()
        };

        // Onboarding frame: while OPEN, paint the card + dim ramps in (the
        // painter's `elapsed_ms` drives the typewriter). While CLOSING, the
        // card is gone but the office keeps fading back UP for a beat; once
        // the fade completes, drop the closing state to fully live.
        let onboarding_frame = if let Some(opened) = self.onboarding_opened_at {
            let e = opened.elapsed().as_millis() as u64;
            OnboardingFrame {
                open: true,
                rows: self.onboarding_ui.rows.clone(),
                selected: self.onboarding_ui.selected,
                elapsed_ms: e,
                dim: welcome::dim_opening(e),
            }
        } else if let Some(closing) = self.onboarding_closing_at {
            match welcome::dim_closing(closing.elapsed().as_millis() as u64) {
                Some(dim) => OnboardingFrame {
                    dim,
                    ..Default::default()
                },
                None => {
                    self.onboarding_closing_at = None;
                    OnboardingFrame::default()
                }
            }
        } else {
            OnboardingFrame::default()
        };

        RenderFrames {
            theme_picker: self.theme_picker,
            version_popup: self.version_popup,
            help_open: self.help_open,
            source_warning,
            dashboard: dashboard_frame,
            connection: connection_frame,
            onboarding: onboarding_frame,
        }
    }

    /// The Sources panel's cached rows carry a per-source HEALTH summary
    /// (install soundness + drift) computed on open/toggle; it scans the
    /// warn-floor log, so read it fresh at each (infrequent) rebuild. `""`
    /// when no log path.
    pub(crate) fn read_conn_log(&self) -> String {
        self.log_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_scene::theme::ALL_THEMES;

    fn ui() -> UiState {
        UiState::new(
            ALL_THEMES[0],
            WelcomeUi::from_detected(&[]),
            false,
            std::path::PathBuf::from("/tmp/sock"),
            None,
        )
    }

    /// The projection is the ONE source of truth for the dispatch chain: each
    /// surface's open flag must round-trip through `modal()` exactly, so
    /// `dispatch_key`'s precedence (onboarding > help > version > connection >
    /// dashboard > theme-picker — pinned per-tier in `dispatch_tests`) is fed
    /// by real state, not an ad-hoc literal that can drift.
    #[test]
    fn modal_projection_mirrors_each_surface_open_flag() {
        let mut ui = ui();
        let m = ui.modal();
        assert!(
            !m.onboarding_open
                && !m.help_open
                && !m.version_popup
                && m.theme_picker.is_none()
                && !m.dashboard_open
                && !m.connection_open
                && !m.connection_confirm,
            "everything closed at boot (no detected CLIs, no version popup)"
        );
        assert_eq!(m.n_themes, ALL_THEMES.len());

        ui.toggle_help();
        assert!(ui.modal().help_open);
        ui.close_help();
        assert!(!ui.modal().help_open);

        ui.open_theme_picker();
        assert_eq!(ui.modal().theme_picker, Some(ui.saved_theme_idx));
        ui.preview_theme(2);
        assert_eq!(ui.modal().theme_picker, Some(2));
        ui.commit_theme(2);
        assert_eq!(ui.modal().theme_picker, None);
        assert_eq!(ui.saved_theme_idx, 2);

        let scene = SceneState::new([4, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        ui.toggle_dashboard(&scene);
        assert!(ui.modal().dashboard_open);
        ui.close_dashboard();
        assert!(!ui.modal().dashboard_open);

        ui.open_connection(Vec::new());
        assert!(ui.modal().connection_open);
        assert!(!ui.modal().connection_confirm);
        ui.connection.confirm = Some(0);
        assert!(ui.modal().connection_confirm);
        ui.close_connection();
        let m = ui.modal();
        assert!(!m.connection_open && !m.connection_confirm);
    }

    /// Onboarding is open exactly while `onboarding_opened_at` is `Some` —
    /// seeded by a NON-empty detected roster, cleared by close (which arms the
    /// fade-out, not a reopen).
    #[test]
    fn onboarding_opens_only_with_a_roster_and_closes_for_good() {
        // No CLIs detected → nothing to connect → stays closed.
        assert!(!ui().modal().onboarding_open);

        let mut ui = UiState::new(
            ALL_THEMES[0],
            WelcomeUi::from_detected(&["codex", "claude-code"]),
            false,
            std::path::PathBuf::from("/tmp/sock"),
            None,
        );
        assert!(ui.modal().onboarding_open, "a roster opens the overlay");
        ui.close_onboarding();
        assert!(!ui.modal().onboarding_open);
        // The close fade is armed: the next frame still carries a dim value.
        let scene = SceneState::new([4, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let frames = ui.build_frames(SystemTime::now(), &scene, &[]);
        assert!(!frames.onboarding.open, "the card itself is gone");
    }

    /// Pause freezes `now()` at one instant; unpausing releases it.
    #[test]
    fn pause_freezes_the_clock() {
        let mut ui = ui();
        ui.toggle_pause();
        let a = ui.now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(a, ui.now(), "paused: the same frozen instant");
        ui.toggle_pause();
        assert_ne!(a, ui.now(), "unpaused: live time again");
    }

    // --- DriftScan: the incremental warn-floor log scan ------------------------

    // A real tracing-fmt drift breadcrumb line (the shape doctor's scanner parses).
    fn drift_line(source: &str, name: &str) -> String {
        format!(
            "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source={source} kind=\"unknown_event\" name={name}\n"
        )
    }

    #[test]
    fn drift_scan_reads_incrementally_and_accumulates_prefixes() {
        use std::io::Write;
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("log");
        std::fs::write(&log, drift_line("claude-code", "X")).unwrap();

        let mut scan = DriftScan::default();
        scan.scan_appended(&log);
        assert_eq!(scan.drifted, vec!["cc".to_string()]);
        let after_first = scan.offset;
        assert_eq!(
            after_first,
            std::fs::metadata(&log).unwrap().len(),
            "offset advances past the scanned lines"
        );

        // Append a SECOND source's breadcrumb: the next pass reads only the
        // appended bytes (offset moved by exactly the new line) and MERGES the
        // new prefix — the first one survives without re-reading its bytes.
        let codex_line = drift_line("codex", "Y");
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(codex_line.as_bytes()).unwrap();
        drop(f);
        scan.scan_appended(&log);
        assert_eq!(scan.drifted, vec!["cc".to_string(), "cx".to_string()]);
        assert_eq!(
            scan.offset,
            after_first + codex_line.len() as u64,
            "second pass consumed only the appended bytes"
        );

        // A no-growth pass changes nothing (and re-appending the SAME source
        // never duplicates its prefix).
        scan.scan_appended(&log);
        assert_eq!(scan.drifted.len(), 2);
    }

    #[test]
    fn drift_scan_leaves_a_partial_trailing_line_for_the_next_pass() {
        use std::io::Write;
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("log");
        let full = drift_line("claude-code", "X");
        // Write the line WITHOUT its terminating newline (mid-write).
        std::fs::write(&log, full.trim_end_matches('\n')).unwrap();

        let mut scan = DriftScan::default();
        scan.scan_appended(&log);
        assert_eq!(scan.offset, 0, "a partial line is not consumed");
        assert!(scan.drifted.is_empty(), "…nor scanned: {:?}", scan.drifted);

        // The newline lands → the completed line scans on the next pass.
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);
        scan.scan_appended(&log);
        assert_eq!(scan.drifted, vec!["cc".to_string()]);
    }

    #[test]
    fn drift_scan_resets_on_external_truncation_and_tolerates_missing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("log");

        // Missing file: a quiet no-op.
        let mut scan = DriftScan::default();
        scan.scan_appended(&log);
        assert_eq!(scan.offset, 0);

        std::fs::write(&log, drift_line("claude-code", "X")).unwrap();
        scan.scan_appended(&log);
        assert!(scan.offset > 0);

        // Externally truncated/rotated below the offset → rescan from the top.
        std::fs::write(&log, drift_line("codex", "Y")).unwrap();
        scan.scan_appended(&log);
        assert!(
            scan.drifted.contains(&"cx".to_string()),
            "post-truncation content is scanned from offset 0: {:?}",
            scan.drifted
        );
    }
}
