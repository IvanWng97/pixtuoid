//! The `native`-only runtime half of the daemon presence layer: the tokio
//! presence side channel + the shared gateway-pid exit watcher.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::{DaemonInstanceKey, DaemonPresenceUpdate, PresenceMsg};

/// The daemon-presence SIDE channel (invariant #2: NOT the one `AgentEvent`
/// channel). Unbounded — presence deltas are tiny + rare.
pub type PresenceSender = tokio::sync::mpsc::UnboundedSender<PresenceMsg>;

/// A handle to arm gateway-pid exit watches across ALL daemon instances. A dying
/// gateway pid converts to an instance-tagged `PidExited` presence delta — the
/// instant abrupt-down rung — reusing the AGNOSTIC `ExitWatch` (pid → channel,
/// no `AgentId` coupling), NOT `HookPidWatch` (which emits an AgentSlot-shaped
/// `SessionEnd` the non-slot mascot can't consume).
pub struct PresenceExitWatch {
    inner: crate::source::exit_watch::ExitWatch,
    /// pid → the daemon instances to take Down when it dies. SET-valued so a
    /// transient A→B pid recycle binds BOTH: take-on-death ends all, and a
    /// spurious cross-instance down self-heals on that instance's next presence
    /// event. Last-writer-wins on a lone value would instead flip a still-live
    /// instance's binding.
    pids: Arc<Mutex<HashMap<i32, HashSet<DaemonInstanceKey>>>>,
}

impl PresenceExitWatch {
    /// Watch one daemon instance's gateway pid; its death emits `(key,
    /// PidExited)` for every bound instance. Idempotent per (pid, key).
    pub fn watch(&self, key: &DaemonInstanceKey, pid: i32) {
        note_key(&self.pids, pid, key);
        self.inner.watch(pid);
    }
}

/// Spawn the shared gateway-pid exit watcher: pid deaths drain into
/// instance-tagged `PidExited` on `presence_tx`. `None` where the platform has
/// no exit-watch backend (then the `presence_ttl_ms` sweep is the only
/// abrupt-down signal). Call in a tokio runtime.
pub fn spawn_presence_exit_watch(presence_tx: PresenceSender) -> Option<PresenceExitWatch> {
    let pids: Arc<Mutex<HashMap<i32, HashSet<DaemonInstanceKey>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (pid_tx, mut pid_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
    let inner = crate::source::exit_watch::ExitWatch::spawn(pid_tx)?;
    let pids_drain = Arc::clone(&pids);
    tokio::spawn(async move {
        while let Some(pid) = pid_rx.recv().await {
            for key in take_keys(&pids_drain, pid) {
                if presence_tx
                    .send(PresenceMsg {
                        key,
                        delta: DaemonPresenceUpdate::PidExited { pid },
                    })
                    .is_err()
                {
                    // Receiver gone — `return` exits the whole drain task, not
                    // just this for-loop (a bare `break` wouldn't).
                    return;
                }
            }
        }
    });
    Some(PresenceExitWatch { inner, pids })
}

type PresencePidMap = Mutex<HashMap<i32, HashSet<DaemonInstanceKey>>>;

/// Registry ops, split from the `ExitWatch` side so they're unit-testable
/// without spawning the platform watcher thread.
fn note_key(pids: &PresencePidMap, pid: i32, key: &DaemonInstanceKey) {
    pids.lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(pid)
        .or_default()
        .insert(key.clone());
}

/// Remove `pid`'s entry and return the daemon instances bound to it. The pid
/// dies exactly once, taking its whole set with it.
fn take_keys(pids: &PresencePidMap, pid: i32) -> Vec<DaemonInstanceKey> {
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
    use crate::state::DaemonInstanceId;

    fn k(source: &str, instance: &str) -> DaemonInstanceKey {
        DaemonInstanceKey::new(
            source,
            DaemonInstanceId::new(instance).expect("non-empty test instance id"),
        )
    }

    #[test]
    fn recycled_pid_binds_both_daemons_and_take_ends_all() {
        let pids: PresencePidMap = Mutex::new(HashMap::new());
        note_key(&pids, 4242, &k("openclaw", "18789"));
        note_key(&pids, 4242, &k("secondd", "1"));
        let mut taken = take_keys(&pids, 4242);
        taken.sort();
        assert_eq!(taken, vec![k("openclaw", "18789"), k("secondd", "1")]);
        // The pid dies once — its whole entry is gone.
        assert!(take_keys(&pids, 4242).is_empty());
    }

    #[test]
    fn single_daemon_rearm_dedups_and_take_yields_one() {
        let pids: PresencePidMap = Mutex::new(HashMap::new());
        note_key(&pids, 7, &k("openclaw", "18789"));
        note_key(&pids, 7, &k("openclaw", "18789"));
        assert_eq!(take_keys(&pids, 7), vec![k("openclaw", "18789")]);
        // An unbound pid is a stale receipt — empty, skipped by the drain.
        assert!(take_keys(&pids, 99).is_empty());
    }

    #[test]
    fn two_instances_of_one_source_bind_separately() {
        let pids: PresencePidMap = Mutex::new(HashMap::new());
        note_key(&pids, 100, &k("openclaw", "18789"));
        note_key(&pids, 200, &k("openclaw", "19789"));
        assert_eq!(take_keys(&pids, 100), vec![k("openclaw", "18789")]);
        assert_eq!(
            take_keys(&pids, 200),
            vec![k("openclaw", "19789")],
            "the sibling's binding survives its neighbour's death"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[tokio::test]
    async fn spawning_the_presence_exit_watch_is_none_without_a_backend() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(spawn_presence_exit_watch(tx).is_none());
    }

    // Watches our OWN pid on purpose: the binding is observable immediately and
    // no process has to die, keeping this off the wall-clock/OS-timing flake
    // class. The death→PidExited half is e2e-covered.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn watch_records_the_binding_that_routes_a_death_to_its_instance() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // ASSERTED, not `let else`-skipped: a graceful skip is indistinguishable
        // from `spawn_presence_exit_watch` returning None outright, which would
        // silently disarm the whole instant-down rung.
        let watch = spawn_presence_exit_watch(tx)
            .expect("macOS/Linux have an exit-watch backend (kqueue / pidfd)");
        let me = std::process::id() as i32;
        let key = k("openclaw", "18789");
        watch.watch(&key, me);
        assert_eq!(
            take_keys(&watch.pids, me),
            vec![key],
            "watch must bind (pid → instance) or a death routes nowhere"
        );
    }
}
