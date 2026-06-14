//! The Connection panel: a modal listing every agent CLI with its connection
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

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::SceneState;

use crate::install::target::{self, Target};
use crate::install::{InstallOutcome, InstallReport, UninstallOutcome, UninstallReport};

/// Connection state for one CLI row — the single facet the toggle acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Bound: this source's events flow and its characters show. Toggle disconnects.
    Connected,
    /// Unbound: the gate is closed; no characters. Toggle connects.
    Disconnected,
    /// A target-bearing CLI that isn't installed on this machine — nothing to bind to.
    NoCli,
}

/// One row in the Connection list = one agent CLI.
#[derive(Debug, Clone)]
pub struct ConnectionRow {
    /// The core source id (registry `SourceDescriptor.name`, e.g. "claude-code")
    /// — the unifying key; joined to an install target via `Target.core_source`.
    pub source_id: &'static str,
    /// 2-char badge id (`cc`/`cx`/…), from the source descriptor.
    pub label_prefix: &'static str,
    pub display_name: &'static str,
    pub state: ConnState,
    /// The config the hooks live in; `None` for no-target (JSONL-only) rows.
    pub config_path: Option<PathBuf>,
    /// The install target backing this row; `None` ⇒ connect/disconnect is a
    /// flag-only flip (Antigravity — no hooks to write).
    pub target: Option<&'static Target>,
}

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

/// Per-target filesystem facts, injected so `build_rows_from` is pure (the FS
/// reads — `is_present`/`default_config_path` — happen in `build_rows`). `Some`
/// exactly when the row has an install target.
#[derive(Debug, Clone)]
pub struct RowFacts {
    pub present: bool,
    pub config_path: Option<PathBuf>,
}

/// One pure input row for `build_rows_from`.
#[derive(Debug, Clone)]
pub struct RowInput {
    pub source_id: &'static str,
    pub label_prefix: &'static str,
    pub target: Option<&'static Target>,
    pub facts: Option<RowFacts>,
    /// Whether this source is in the live connected-set (the persisted intent).
    pub connected: bool,
}

/// Title-case the no-target sources (the registry deliberately omits display
/// names). Target-bearing rows use `Target.display_name`.
fn display_name_for(source_id: &'static str) -> &'static str {
    match source_id {
        "antigravity" => "Antigravity",
        "copilot" => "Copilot CLI",
        other => other,
    }
}

/// Pure row builder over injected facts — the testable core of `build_rows`.
/// A target-bearing CLI that isn't present is `NoCli` (nothing to bind to, even
/// if a stale flag says connected); otherwise the connected-set is authoritative.
pub fn build_rows_from(inputs: Vec<RowInput>) -> Vec<ConnectionRow> {
    inputs
        .into_iter()
        .map(|input| {
            let absent_cli = matches!(
                (&input.target, &input.facts),
                (Some(_), Some(f)) if !f.present
            );
            let state = if absent_cli {
                ConnState::NoCli
            } else if input.connected {
                ConnState::Connected
            } else {
                ConnState::Disconnected
            };
            ConnectionRow {
                source_id: input.source_id,
                label_prefix: input.label_prefix,
                display_name: input
                    .target
                    .map_or_else(|| display_name_for(input.source_id), |t| t.display_name),
                state,
                config_path: input.facts.and_then(|f| f.config_path),
                target: input.target,
            }
        })
        .collect()
}

/// Build the cached connection-facet rows from the registry + install targets +
/// the live connected-set. Performs filesystem reads (`is_present`/
/// `default_config_path`) — call on open + after each toggle, NEVER per frame.
pub fn build_rows(connected: &HashSet<String>) -> Vec<ConnectionRow> {
    use pixtuoid_core::source::registry::REGISTRY;
    let inputs = REGISTRY
        .iter()
        .map(|d| {
            // Join on the SOURCE id via `core_source`, NOT `by_name`: the Claude
            // target is named "claude" but its source is "claude-code", so
            // `by_name(d.name)` would miss it and render the flagship CLI as a
            // non-actionable JSONL row.
            let target = target::by_source(d.name);
            let facts = target.map(|t| RowFacts {
                present: target::is_present(t),
                config_path: (t.default_config_path)().ok(),
            });
            RowInput {
                source_id: d.name,
                label_prefix: d.label_prefix,
                target,
                facts,
                connected: connected.contains(d.name),
            }
        })
        .collect();
    build_rows_from(inputs)
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
