//! The `native`-only runtime half of the daemon presence layer: the tokio
//! presence side channel + the shared gateway-pid exit watcher. The pure
//! state machine (`apply_presence`, the sweeps, the vocabulary) stays in the
//! always-compiled parent module; this whole file sits behind the parent's
//! ONE `#[cfg(feature = "native")] mod native;` gate and is re-exported
//! there, so public paths don't move.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::{DaemonPresenceUpdate, PresenceMsg};

/// The daemon-presence SIDE channel (invariant #2: NOT the one `AgentEvent`
/// channel). Unbounded — presence deltas are tiny + rare.
pub type PresenceSender = tokio::sync::mpsc::UnboundedSender<PresenceMsg>;

/// A handle to arm gateway-pid exit watches across ALL daemons. A dying gateway
/// pid converts to a source-tagged `PidExited` presence delta — the instant
/// abrupt-down rung — reusing the AGNOSTIC `ExitWatch` (pid → channel, no
/// `AgentId` coupling), NOT `HookPidWatch` (which emits an AgentSlot-shaped
/// `SessionEnd` the non-slot mascot can't consume). One watcher multiplexes
/// every daemon's pid; the `pid → source` binding routes the death back.
pub struct PresenceExitWatch {
    inner: crate::source::exit_watch::ExitWatch,
    /// pid → the daemon sources to take Down when it dies. SET-valued (not a
    /// lone source) so a transient A→B pid recycle binds BOTH: take-on-death
    /// ends all, and a spurious cross-source down self-heals on that daemon's
    /// next presence event — the `HookPidWatch` pattern, keyed by pid alone but
    /// set-valued so last-writer-wins can't flip a still-live daemon's binding.
    pids: Arc<Mutex<HashMap<i32, HashSet<String>>>>,
}

impl PresenceExitWatch {
    /// Watch a daemon's gateway pid; its death emits `(source, PidExited)` for
    /// every bound source. Idempotent per (pid, source) — a re-arm just
    /// re-inserts into the set.
    pub fn watch(&self, source: &str, pid: i32) {
        note_source(&self.pids, pid, source);
        self.inner.watch(pid);
    }
}

/// Spawn the shared gateway-pid exit watcher: pid deaths drain into source-tagged
/// `PidExited` on `presence_tx`. `None` where the platform has no exit-watch
/// backend (then the `presence_ttl_ms` sweep is the only abrupt-down signal).
/// Call in a tokio runtime.
pub fn spawn_presence_exit_watch(presence_tx: PresenceSender) -> Option<PresenceExitWatch> {
    let pids: Arc<Mutex<HashMap<i32, HashSet<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let (pid_tx, mut pid_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
    let inner = crate::source::exit_watch::ExitWatch::spawn(pid_tx)?;
    let pids_drain = Arc::clone(&pids);
    tokio::spawn(async move {
        while let Some(pid) = pid_rx.recv().await {
            // Unbound pid = stale receipt (already routed): the empty Vec
            // iterates zero times. Each bound source gets its own PidExited.
            for source in take_sources(&pids_drain, pid) {
                if presence_tx
                    .send(PresenceMsg {
                        source,
                        delta: DaemonPresenceUpdate::PidExited { pid },
                    })
                    .is_err()
                {
                    // Receiver (the reducer) gone — `return` exits the whole drain
                    // task, not just this for-loop (a bare `break` wouldn't).
                    return;
                }
            }
        }
    });
    Some(PresenceExitWatch { inner, pids })
}

type PresencePidMap = Mutex<HashMap<i32, HashSet<String>>>;

/// Registry ops, split from the [`ExitWatch`] side so they're unit-testable
/// without spawning the platform watcher thread (the `pid_watch` precedent).
fn note_source(pids: &PresencePidMap, pid: i32, source: &str) {
    pids.lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(pid)
        .or_default()
        .insert(source.to_string());
}

/// Remove `pid`'s entry and return the daemon sources bound to it (empty if
/// none). The pid dies exactly once, taking its whole set with it.
fn take_sources(pids: &PresencePidMap, pid: i32) -> Vec<String> {
    pids.lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&pid)
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // N-daemon pid recycle: P bound to A, re-armed for B (B reused P before A's
    // death drained). take must return BOTH; the old lone-String map lost one.
    #[test]
    fn recycled_pid_binds_both_daemons_and_take_ends_all() {
        let pids: PresencePidMap = Mutex::new(HashMap::new());
        note_source(&pids, 4242, "openclaw");
        note_source(&pids, 4242, "secondd");
        let mut taken = take_sources(&pids, 4242);
        taken.sort();
        assert_eq!(taken, vec!["openclaw".to_string(), "secondd".to_string()]);
        // The pid dies once — its whole entry is gone.
        assert!(take_sources(&pids, 4242).is_empty());
    }

    // Single-daemon path (today's only reality) is byte-identical to the old
    // lone-source map: a re-arm dedups, take yields exactly one source.
    #[test]
    fn single_daemon_rearm_dedups_and_take_yields_one() {
        let pids: PresencePidMap = Mutex::new(HashMap::new());
        note_source(&pids, 7, "openclaw");
        note_source(&pids, 7, "openclaw");
        assert_eq!(take_sources(&pids, 7), vec!["openclaw".to_string()]);
        // An unbound pid is a stale receipt — empty, skipped by the drain.
        assert!(take_sources(&pids, 99).is_empty());
    }
}
