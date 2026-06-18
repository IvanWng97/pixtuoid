//! The Sources panel: a modal listing every agent CLI with its connection
//! state (bound / unbound) and its live activity. This module is the PURE model
//! — no ratatui. The painter lives in `tui::widgets::connection`; the event-loop
//! wiring lives in `tui::mod`.
//!
//! Rows are the UNION of install targets and registry sources, keyed on the
//! source id (`SourceDescriptor.name`, joined to an install target via
//! `Target.core_source` — NOT `Target.name`, which differs for Claude). A row's
//! `state` is driven by the live connected-set (the persisted per-source intent),
//! NOT by whether hooks happen to be installed: connecting a source opens its
//! gate (characters appear); disconnecting closes it (characters walk out) AND,
//! for target-bearing sources, removes its hooks. Users bind/unbind a source;
//! they never think in terms of hooks vs JSONL.

use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::SceneState;

use crate::install::{InstallOutcome, InstallReport, UninstallOutcome, UninstallReport};

// The source-status MODEL (rows, state, builders) lives in `crate::sources` (the
// TUI-free control core shared by the panel, the CLI, and onboarding). Re-export
// it here so this module + the painter + harness keep their `connection::…` paths.
pub use crate::sources::{
    build_rows, build_rows_from, ConnState, ConnectionRow, RowFacts, RowInput,
};

/// Live-connection facet, derived per frame from the scene snapshot. Aligned by
/// index to `ConnectionUi.rows`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveInfo {
    pub agents: usize,
    pub last_event_age: Option<Duration>,
    /// The source's transport exited (only ever true for sources with a `Source`
    /// impl that can die — hook-only sources ride CC's socket and never die here).
    pub dead: bool,
}

/// The per-tick Sources-panel render frame the event loop hands the renderer via
/// `set_connection_frame` — one snapshot the painter reads. The event loop builds
/// the cached HOOK facet (`rows`) + the per-frame LIVE facet (`live`) then hands
/// both over together, so they ride as one struct rather than seven parallel
/// `connection_*` fields/params through `TuiRenderer` → `DrawCtx` → `paint_overlays`.
/// (The two-facet BUILD lifecycle is unchanged — this bundles only the snapshot the
/// renderer mirrors.) Mirrors `OnboardingFrame`.
#[derive(Debug, Clone, Default)]
pub struct ConnectionFrame {
    pub open: bool,
    pub rows: Vec<ConnectionRow>,
    pub live: Vec<LiveInfo>,
    pub selected: usize,
    pub confirm: Option<usize>,
    pub result: Option<String>,
    pub socket_line: String,
}

/// Session-persistent Connection UI state, owned by the event loop. Only `open`
/// flips on close, so the cached rows + selection survive close/reopen.
#[derive(Debug, Default)]
pub struct ConnectionUi {
    pub open: bool,
    /// Index into the registry-stable `rows` (fixed order, rebuilt in place on
    /// open/action) — a plain `usize` is sound precisely because the row set
    /// doesn't churn frame-to-frame (unlike the dashboard, which is AgentId-keyed).
    pub selected: usize,
    /// Cached CONNECTION facet — rebuilt on open + after each toggle (filesystem
    /// reads + a connected-set read), NEVER per frame. The LIVE facet is
    /// recomputed per frame instead.
    pub rows: Vec<ConnectionRow>,
    /// `Some(row_idx)` ⇒ a disconnect is armed on that row, awaiting y/n.
    pub confirm: Option<usize>,
    pub last_result: Option<String>,
}

/// The live facet for one source, derived purely from the scene snapshot +
/// health list. `now` is the frame's clock (not `SystemTime::now()`) so the age
/// is deterministic + honors the paused-clock path.
pub fn live_for(
    now: SystemTime,
    source_id: &str,
    scene: &SceneState,
    health: &[SourceDeath],
) -> LiveInfo {
    let mut agents = 0usize;
    let mut max_evt: Option<SystemTime> = None;
    for slot in scene.agents.values() {
        if slot.source.as_ref() == source_id {
            agents += 1;
            max_evt = Some(max_evt.map_or(slot.last_event_at, |m: SystemTime| {
                m.max(slot.last_event_at)
            }));
        }
    }
    LiveInfo {
        agents,
        last_event_age: max_evt.map(|t| now.duration_since(t).unwrap_or_default()),
        dead: health.iter().any(|d| d.source == source_id),
    }
}

/// The per-frame parallel `LiveInfo` vec aligned to `rows`.
pub fn live_view(
    now: SystemTime,
    rows: &[ConnectionRow],
    scene: &SceneState,
    health: &[SourceDeath],
) -> Vec<LiveInfo> {
    rows.iter()
        .map(|r| live_for(now, r.source_id, scene, health))
        .collect()
}

/// Move the selection one row up (`-1`) or down (`+1`), clamped at the ends.
pub fn move_selection(rows: &[ConnectionRow], sel: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    (sel as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize
}

/// Detail-line hint when the toggle lands on a row that can't be acted on.
pub fn no_action_hint(row: &ConnectionRow) -> String {
    match row.state {
        ConnState::NoCli => format!("{} not detected on this machine", row.display_name),
        _ => format!("nothing to do for {}", row.display_name),
    }
}

/// Render an `InstallReport` into the panel's one-line "connected" result.
pub fn format_connect_result(r: &InstallReport, display_name: &str) -> String {
    let mut s = match r.outcome {
        InstallOutcome::AlreadyUpToDate | InstallOutcome::Installed => {
            format!("\u{2713} {display_name} connected")
        }
    };
    if r.backup.is_some() {
        s.push_str(" \u{00b7} backup saved");
    }
    if r.path_warning {
        s.push_str(" \u{00b7} \u{26a0} pixtuoid-hook not on PATH");
    }
    s
}

/// Render an `UninstallReport` into the panel's one-line "disconnected" result.
pub fn format_disconnect_result(r: &UninstallReport, display_name: &str) -> String {
    let mut s = match r.outcome {
        UninstallOutcome::NothingToRemove | UninstallOutcome::Removed => {
            format!("\u{2713} {display_name} disconnected")
        }
    };
    if r.removed_backup.is_some() {
        s.push_str(" \u{00b7} backup cleared");
    }
    s
}

#[cfg(test)]
mod tests;
