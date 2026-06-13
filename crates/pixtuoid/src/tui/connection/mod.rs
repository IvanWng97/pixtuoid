//! The Connection panel: a modal listing every agent CLI with its hook-setup state
//! (install/uninstall actionable) and its live-connection state. This module is
//! the PURE model — no ratatui. The painter lives in `tui::widgets::connection`; the
//! event-loop wiring lives in `tui::mod`.
//!
//! Rows are the UNION of install targets and registry sources, keyed on the
//! source id (`SourceDescriptor.name`, joined to an install target via
//! `Target.core_source` — NOT `Target.name`, which differs for Claude). A
//! source with no install target (Antigravity, JSONL-only) renders a
//! `JsonlNoHooks` row where install/uninstall no-op.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::SceneState;

use crate::install::target::{self, Target};
use crate::install::{InstallOutcome, InstallReport, UninstallOutcome, UninstallReport};

/// Hook-setup state for one CLI row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    /// Managed pixtuoid hooks are present in the config.
    On,
    /// CLI detected, hooks absent.
    Off,
    /// CLI not detected on this machine.
    NoCli,
    /// A source with no install target (Antigravity, JSONL-only) — nothing to install.
    JsonlNoHooks,
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
    pub hooks: HookState,
    /// The config the hooks live in; `None` for JSONL/no-target rows.
    pub config_path: Option<PathBuf>,
    /// The install target backing this row; `None` ⇒ actions no-op (Antigravity).
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
    /// Cached HOOK facet — rebuilt on open + after each action (filesystem
    /// reads), NEVER per frame. The LIVE facet is recomputed per frame instead.
    pub rows: Vec<ConnectionRow>,
    /// `Some(row_idx)` ⇒ uninstall is armed on that row, awaiting y/n.
    pub confirm: Option<usize>,
    pub last_result: Option<String>,
}

/// Per-target filesystem facts, injected so `build_rows_from` is pure (mirrors
/// `install::plan_targets` taking an injected `present` slice). `Some` exactly
/// when the row has an install target.
#[derive(Debug, Clone)]
pub struct HookFacts {
    pub present: bool,
    pub installed: bool,
    pub config_path: Option<PathBuf>,
}

/// One pure input row for `build_rows_from`.
#[derive(Debug, Clone)]
pub struct RowInput {
    pub source_id: &'static str,
    pub label_prefix: &'static str,
    pub target: Option<&'static Target>,
    pub facts: Option<HookFacts>,
}

/// Title-case the one no-target source (the registry deliberately omits display
/// names). Target-bearing rows use `Target.display_name`.
fn display_name_for(source_id: &'static str) -> &'static str {
    match source_id {
        "antigravity" => "Antigravity",
        other => other,
    }
}

/// Pure row builder over injected facts — the testable core of `build_rows`.
pub fn build_rows_from(inputs: Vec<RowInput>) -> Vec<ConnectionRow> {
    inputs
        .into_iter()
        .map(|input| {
            let (hooks, config_path) = match (&input.target, &input.facts) {
                (Some(_), Some(f)) => {
                    let hooks = if !f.present {
                        HookState::NoCli
                    } else if f.installed {
                        HookState::On
                    } else {
                        HookState::Off
                    };
                    (hooks, f.config_path.clone())
                }
                // No install target (Antigravity) — or facts missing (defensive).
                _ => (HookState::JsonlNoHooks, None),
            };
            ConnectionRow {
                source_id: input.source_id,
                label_prefix: input.label_prefix,
                display_name: input
                    .target
                    .map_or_else(|| display_name_for(input.source_id), |t| t.display_name),
                hooks,
                config_path,
                target: input.target,
            }
        })
        .collect()
}

/// Build the cached hook-facet rows from the registry + install targets.
/// Performs filesystem reads (`is_present`/`has_hooks`/`default_config_path`) —
/// call on open + after each action, NEVER per frame.
pub fn build_rows() -> Vec<ConnectionRow> {
    use pixtuoid_core::source::registry::REGISTRY;
    let inputs = REGISTRY
        .iter()
        .map(|d| {
            // Join on the SOURCE id via `core_source`, NOT `by_name`: the Claude
            // target is named "claude" but its source is "claude-code", so
            // `by_name(d.name)` would miss it and render the flagship CLI as a
            // non-actionable JSONL row.
            let target = target::by_source(d.name);
            let facts = target.map(|t| HookFacts {
                present: target::is_present(t),
                installed: crate::install::has_hooks(t),
                config_path: (t.default_config_path)().ok(),
            });
            RowInput {
                source_id: d.name,
                label_prefix: d.label_prefix,
                target,
                facts,
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

/// Detail-line hint when an action key lands on a row that can't be acted on.
pub fn no_action_hint(row: &ConnectionRow) -> String {
    match row.hooks {
        HookState::JsonlNoHooks => {
            format!(
                "{} connects via JSONL — no hooks to install",
                row.display_name
            )
        }
        HookState::NoCli => format!("{} not detected on this machine", row.display_name),
        _ => format!("nothing to do for {}", row.display_name),
    }
}

/// Render an `InstallReport` into the panel's one-line result string.
pub fn format_install_result(r: &InstallReport, display_name: &str) -> String {
    match r.outcome {
        InstallOutcome::AlreadyUpToDate => format!("{display_name}: already up to date"),
        InstallOutcome::Installed => {
            let mut s = format!("\u{2713} {display_name} hooks installed");
            if r.backup.is_some() {
                s.push_str(" \u{00b7} backup saved");
            }
            s.push_str(&format!(" \u{00b7} start a new {} session", r.restart_noun));
            if r.path_warning {
                s.push_str(" \u{00b7} \u{26a0} pixtuoid-hook not on PATH");
            }
            s
        }
    }
}

/// Render an `UninstallReport` into the panel's one-line result string.
pub fn format_uninstall_result(r: &UninstallReport, display_name: &str) -> String {
    match r.outcome {
        UninstallOutcome::NothingToRemove => format!("{display_name}: nothing to remove"),
        UninstallOutcome::Removed => {
            let mut s = format!("\u{2713} {display_name} hooks removed");
            if r.removed_backup.is_some() {
                s.push_str(" \u{00b7} backup cleared");
            }
            s.push_str(&format!(" \u{00b7} start a new {} session", r.restart_noun));
            s
        }
    }
}

#[cfg(test)]
mod tests;
