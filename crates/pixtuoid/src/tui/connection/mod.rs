//! The Sources panel: a modal listing every agent CLI with its connection
//! state (bound / unbound) and its live activity. This module is the PURE model
//! — no ratatui. The painter lives in `tui::widgets::connection`.
//!
//! Rows are the UNION of install targets and registry sources, keyed on the
//! source id (`SourceDescriptor.name`, joined to an install target via
//! `Target.core_source` — NOT `Target.name`, which differs for Claude). A row's
//! `state` is driven by the live connected-set (the persisted per-source intent),
//! NOT by whether hooks happen to be installed.

use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::SceneState;

use crate::install::{InstallOutcome, InstallReport, UninstallOutcome, UninstallReport};

// Re-exported so this module, the painter and the harness keep their
// `connection::…` paths; the model itself lives in `crate::sources`.
pub use crate::sources::{
    build_rows, build_rows_from, ConnState, ConnectionRow, RowFacts, RowInput,
};

/// WHAT is live for one row — a TYPED split, because the two source classes have
/// nothing to count in common. An `Agent` source's liveness is its `AgentSlot`s; a
/// `Daemon`'s is its entries in `SceneState::daemons`, and it never creates a slot
/// at all — so the absent capability is typed here rather than stubbed with a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveFacet {
    Agents {
        agents: usize,
        last_event_age: Option<Duration>,
    },
    /// A `Daemon` source: its running instances, or `None` when none is present
    /// (the panel's `no gateway seen` cell).
    ///
    /// ATOMIC on purpose: a separate `instances: usize` + `state: Option<_>` pair
    /// admitted `{0, Some(_)}` and `{N, None}`, neither of which `live_for` can
    /// produce, and forced the painter to invent a state (`state.unwrap_or(Idle)`).
    Daemon(Option<DaemonRollup>),
}

/// N ≥ 1 running instances of a daemon source and the state they roll up to.
///
/// `NonZeroUsize` is what makes "present" and "how many" the same fact: the
/// `no gateway seen` case is `Daemon(None)`, so a zero count is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonRollup {
    pub instances: std::num::NonZeroUsize,
    /// Their worst-of state via the shared `board::gateway_rollup` — the same
    /// worst-of the footer's `⬢gw` chip and the wall board read.
    pub state: pixtuoid_core::state::DaemonState,
}

impl Default for LiveFacet {
    fn default() -> Self {
        LiveFacet::Agents {
            agents: 0,
            last_event_age: None,
        }
    }
}

/// Live-connection facet, derived per frame from the scene snapshot. Aligned by
/// index to `ConnectionUi.rows`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveInfo {
    pub facet: LiveFacet,
    /// The source's transport exited. The `HookRouter`'s shared-socket death is
    /// attributed in the FOOTER under `hook-router`, deliberately not on any row:
    /// one router death would otherwise falsely mark every healthy transcript
    /// watcher dead.
    pub dead: bool,
}

/// The per-tick Sources-panel render frame the event loop hands the renderer via
/// `set_connection_frame` — one snapshot the painter reads. Mirrors
/// `OnboardingFrame`.
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

/// Only `open` flips on close, so the cached rows + selection survive
/// close/reopen.
#[derive(Debug, Default)]
pub struct ConnectionUi {
    pub open: bool,
    /// Index into the registry-stable `rows` — a plain `usize` is sound precisely
    /// because the row set doesn't churn frame-to-frame (unlike the dashboard,
    /// which is AgentId-keyed).
    pub selected: usize,
    /// Cached CONNECTION facet — rebuilt on open + after each toggle, NEVER per
    /// frame. The LIVE facet is recomputed per frame instead.
    pub rows: Vec<ConnectionRow>,
    /// `Some(row_idx)` ⇒ a disconnect is armed on that row, awaiting y/n.
    pub confirm: Option<usize>,
    pub last_result: Option<String>,
}

/// `now` is the frame's clock (not `SystemTime::now()`) so the age is
/// deterministic and honors the paused-clock path.
pub fn live_for(
    now: SystemTime,
    source_id: &str,
    scene: &SceneState,
    health: &[SourceDeath],
) -> LiveInfo {
    let dead = health.iter().any(|d| d.source == source_id);
    // Registry-driven, so a second daemon source inherits this the day its row
    // lands — no name match here.
    let is_daemon =
        pixtuoid_core::source::registry::descriptor_for(source_id).is_some_and(|d| d.is_daemon());
    if is_daemon {
        // ONE walk, so the count and the rollup cannot disagree.
        let mine: Vec<_> = scene
            .daemons()
            .filter(|(s, _, _)| *s == source_id)
            .map(|(_, _, p)| p)
            .collect();
        let rollup = std::num::NonZeroUsize::new(mine.len())
            .zip(pixtuoid_scene::board::gateway_rollup(mine.into_iter()))
            .map(|(instances, state)| DaemonRollup { instances, state });
        return LiveInfo {
            facet: LiveFacet::Daemon(rollup),
            dead,
        };
    }
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
        facet: LiveFacet::Agents {
            agents,
            last_event_age: max_evt.map(|t| now.duration_since(t).unwrap_or_default()),
        },
        dead,
    }
}

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

pub fn move_selection(rows: &[ConnectionRow], sel: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    (sel as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize
}

pub fn no_action_hint(row: &ConnectionRow) -> String {
    match row.state {
        ConnState::NoCli { .. } => format!("{} not detected on this machine", row.display_name),
        _ => format!("nothing to do for {}", row.display_name),
    }
}

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
    // Connecting is not always the last step: OpenClaw's `plugins.load` is
    // `kind: "restart"` upstream, so a RUNNING gateway keeps serving without our
    // plugin until it restarts.
    if let Some(hint) = r.post_install_hint {
        s.push_str(" \u{00b7} ");
        s.push_str(hint);
    }
    s
}

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

/// WHICH bind/unbind step a panel failure line reports. `HookRemoval` is not a
/// failed disconnect: the flag IS persisted false and only the hook removal
/// didn't land, so it words the residual rather than the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedOp {
    /// A connect that left the source disconnected (the core rolled the flag back).
    Connect,
    /// A disconnect that wrote nothing (the persist itself aborted).
    Disconnect,
    /// A disconnect that persisted, with the hooks left behind.
    HookRemoval,
}

/// THE Sources-panel failure line, `{display_name}: {what failed} — {reason}`.
///
/// Every site that words a failed bind/unbind rides this — the panel's own `t`
/// toggle and the onboarding apply's surfacing, which routes its failures onto
/// this very panel — so a retry on the row reads the sentence the failure first
/// gave.
pub fn format_failure(op: FailedOp, display_name: &str, reason: &str) -> String {
    let what = match op {
        FailedOp::Connect => "connect failed",
        FailedOp::Disconnect => "disconnect failed",
        FailedOp::HookRemoval => crate::sources::HOOK_REMOVAL_FAILED_PHRASE,
    };
    format!("{display_name}: {what} \u{2014} {reason}")
}

#[cfg(test)]
mod tests;
