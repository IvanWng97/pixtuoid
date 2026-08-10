//! Liveness for HOOK-ONLY sources whose shim can supply the agent CLI's pid.
//!
//! A hook-only source has no tailable transcript and therefore none of the JSONL
//! watcher's liveness ladder; its ONLY exit signal is the best-effort
//! `session_end` hook on a CLEAN quit, so an abrupt exit ghosts the sprite until
//! the 10–30 min stale-sweep. When the shim can stamp the CLI's pid (`_pid`, an
//! ancestor walk past the runner's interposed shell), [`ExitWatch`] emits a
//! `SessionEnd` the moment that pid dies. Fed ONLY from the hook decode path,
//! so it is inert for sources whose payloads carry no `_pid`.
//!
//! A wrong pid is NOT self-healing: acting on the first sighting walked the
//! sprite out on every hook for a whole session (#896). So the watch arms only
//! on the SECOND sighting of a `(pid, agent)` pair — this crate's
//! `SHARP-EDGES.md` has why that costs nothing and what it buys.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::source::exit_watch::ExitWatch;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

/// Cloneable handle (one per hook connection task) over a shared pid→agents
/// registry + the process-exit watcher.
#[derive(Clone)]
pub(crate) struct HookPidWatch {
    exit: Arc<ExitWatch>,
    pids: Arc<Mutex<HashMap<i32, HashSet<AgentId>>>>,
}

impl HookPidWatch {
    /// Spawn the exit watcher + the drain task that turns a dead pid into a
    /// `SessionEnd` for each agent bound to it. `None` if no exit-watch backend
    /// exists on this platform (Windows, pre-5.3 Linux) — the source then falls
    /// back to `session_end` + the stale-sweep.
    pub(crate) fn spawn(tx: TaggedSender) -> Option<Self> {
        let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
        let exit = Arc::new(ExitWatch::spawn(exit_tx)?);
        let pids: Arc<Mutex<HashMap<i32, HashSet<AgentId>>>> = Arc::new(Mutex::new(HashMap::new()));
        let this = Self { exit, pids };
        let drain = this.clone();
        tokio::spawn(async move {
            while let Some(dead) = exit_rx.recv().await {
                // A SessionEnd for an already-ended agent (the clean-quit case,
                // where `session_end` ended it first) is a reducer no-op.
                for agent_id in drain.take(dead) {
                    if tx
                        .send((
                            Transport::Hook,
                            AgentEvent::SessionEnd {
                                agent_id,
                                as_child: false,
                            },
                        ))
                        .await
                        .is_err()
                    {
                        return; // reducer gone → daemon shutdown
                    }
                }
            }
        });
        Some(this)
    }

    /// Bind `agent_id` to `pid`, arming the exit watch once the pair repeats
    /// (idempotent). Callers must note at most once per hook PAYLOAD, or a
    /// multi-event batch would confirm its own pid.
    pub(crate) fn note(&self, pid: i32, agent_id: AgentId) {
        if note_pid(&self.pids, pid, agent_id) {
            self.exit.watch(pid);
        }
    }

    fn take(&self, pid: i32) -> Vec<AgentId> {
        take_pid(&self.pids, pid)
    }
}

type PidMap = Mutex<HashMap<i32, HashSet<AgentId>>>;

/// Registry ops, split from the [`ExitWatch`] side so they're unit-testable
/// without spawning the platform watcher thread. Returns whether this pairing
/// was already on file — the repeat sighting that arms the watch.
fn note_pid(pids: &PidMap, pid: i32, agent_id: AgentId) -> bool {
    !pids
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(pid)
        .or_default()
        .insert(agent_id)
}

/// Remove `pid`'s entry and return the agents bound to it. The removal keeps the
/// map from accumulating across CLI launches.
fn take_pid(pids: &PidMap, pid: i32) -> Vec<AgentId> {
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn spawn_returns_a_watch_on_first_class_platforms() {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        assert!(HookPidWatch::spawn(tx).is_some());
    }

    #[test]
    fn note_binds_agents_to_a_pid_and_take_removes_them_once() {
        let pids: PidMap = Mutex::new(HashMap::new());
        let a1 = AgentId::from_parts("codewhale", "/ws");
        let a2 = AgentId::from_parts("codewhale", "agent-child");
        note_pid(&pids, 4242, a1);
        note_pid(&pids, 4242, a2);
        note_pid(&pids, 99, AgentId::from_parts("codewhale", "/other"));

        let mut taken = take_pid(&pids, 4242);
        taken.sort_unstable();
        let mut expected = vec![a1, a2];
        expected.sort_unstable();
        assert_eq!(
            taken, expected,
            "both agents bound to the pid are ended on its death"
        );

        assert!(
            take_pid(&pids, 4242).is_empty(),
            "the entry is removed on the first take"
        );
        assert_eq!(take_pid(&pids, 99).len(), 1);
    }

    #[test]
    fn note_is_idempotent_per_agent_and_reports_the_repeat() {
        let pids: PidMap = Mutex::new(HashMap::new());
        let a = AgentId::from_parts("codewhale", "/ws");
        assert!(!note_pid(&pids, 7, a), "a first sighting is not a repeat");
        assert!(note_pid(&pids, 7, a), "the second sighting is the repeat");
        assert_eq!(
            take_pid(&pids, 7).len(),
            1,
            "a re-noted (pid, agent) is deduped"
        );
    }

    /// #896: a per-invocation wrapper reports a FRESH pid each hook, so no
    /// pairing ever repeats and nothing is ever armed.
    #[test]
    fn a_fresh_pid_per_hook_never_becomes_a_repeat() {
        let pids: PidMap = Mutex::new(HashMap::new());
        let agent = AgentId::from_parts("cursor", "sess-A");
        for wrapper_pid in [4808, 5379, 8924, 9869, 13594] {
            assert!(
                !note_pid(&pids, wrapper_pid, agent),
                "wrapper pid {wrapper_pid} must never read as corroborated"
            );
        }
    }

    #[tokio::test]
    async fn killing_a_watched_pid_emits_session_end_for_its_agent() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let Some(watch) = HookPidWatch::spawn(tx) else {
            return; // no exit-watch backend on this platform — nothing to assert
        };
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn a child to watch");
        let pid = i32::try_from(child.id()).expect("pid fits i32");
        let agent = AgentId::from_parts("codewhale", "/ws");
        watch.note(pid, agent);
        watch.note(pid, agent); // the CLI's pid rides every hook event
        child.kill().expect("kill the watched child");
        let _ = child.wait();

        let (transport, ev) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a SessionEnd within 5s of the watched pid dying")
            .expect("channel still open");
        assert_eq!(transport, Transport::Hook);
        assert!(
            matches!(ev, AgentEvent::SessionEnd { agent_id, as_child: false } if agent_id == agent),
            "the bound agent must be ended when its pid dies, got {ev:?}"
        );
    }

    /// The #896 guard end to end: one sighting, then that pid dies.
    #[tokio::test]
    async fn a_once_seen_pid_dying_ends_nothing() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let Some(watch) = HookPidWatch::spawn(tx) else {
            return; // no exit-watch backend on this platform — nothing to assert
        };
        let mut wrapper = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn a stand-in wrapper shell");
        let pid = i32::try_from(wrapper.id()).expect("pid fits i32");
        watch.note(pid, AgentId::from_parts("cursor", "sess-A"));
        wrapper.kill().expect("kill the wrapper");
        let _ = wrapper.wait();

        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .is_err(),
            "an uncorroborated pid's death must not end the session"
        );
    }
}
