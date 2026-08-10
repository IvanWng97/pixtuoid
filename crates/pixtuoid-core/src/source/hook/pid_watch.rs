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
    bindings: Arc<Mutex<Bindings>>,
}

/// The two halves of a sighting, in ONE lock so they cannot contradict: a pid is
/// either a candidate or armed, never both.
#[derive(Default)]
struct Bindings {
    /// Armed pid → the agents ended when it dies. Drained by [`Bindings::take`],
    /// so it holds only pids the exit watch is actually watching.
    armed: HashMap<i32, HashSet<AgentId>>,
    /// The ONE uncorroborated sighting per agent. A runner whose wrapper pid is
    /// fresh every hook never corroborates, and nothing would ever evict those —
    /// only the exit watch removes entries, and it never sees them — so keying
    /// by AGENT bounds this at one per live session instead of one per tool call.
    candidate: HashMap<AgentId, i32>,
}

impl HookPidWatch {
    /// Spawn the exit watcher + the drain task that turns a dead pid into a
    /// `SessionEnd` for each agent bound to it. `None` if no exit-watch backend
    /// exists on this platform (Windows, pre-5.3 Linux) — the source then falls
    /// back to `session_end` + the stale-sweep.
    pub(crate) fn spawn(tx: TaggedSender) -> Option<Self> {
        let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
        let exit = Arc::new(ExitWatch::spawn(exit_tx)?);
        let this = Self {
            exit,
            bindings: Arc::new(Mutex::new(Bindings::default())),
        };
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
        if self.lock().sight(pid, agent_id) {
            self.exit.watch(pid);
        }
    }

    fn take(&self, pid: i32) -> Vec<AgentId> {
        self.lock().take(pid)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Bindings> {
        self.bindings.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Split from the [`ExitWatch`] side so it is unit-testable without spawning the
/// platform watcher thread.
impl Bindings {
    /// Record a sighting; `true` when it CORROBORATES this agent's previous one,
    /// which is what arms the watch. A different pid for the same agent replaces
    /// the candidate rather than joining it.
    fn sight(&mut self, pid: i32, agent_id: AgentId) -> bool {
        if self.armed.get(&pid).is_some_and(|a| a.contains(&agent_id)) {
            return false;
        }
        if self.candidate.insert(agent_id, pid) != Some(pid) {
            return false;
        }
        self.candidate.remove(&agent_id);
        self.armed.entry(pid).or_default().insert(agent_id);
        true
    }

    /// Remove `pid`'s entry and return the agents bound to it. The removal keeps
    /// the map from accumulating across CLI launches.
    fn take(&mut self, pid: i32) -> Vec<AgentId> {
        self.armed.remove(&pid).into_iter().flatten().collect()
    }
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

    /// Two sightings each, so both agents are ARMED on the shared pid.
    #[test]
    fn take_returns_every_agent_armed_on_a_pid_and_only_once() {
        let mut b = Bindings::default();
        let a1 = AgentId::from_parts("codewhale", "/ws");
        let a2 = AgentId::from_parts("codewhale", "agent-child");
        let other = AgentId::from_parts("codewhale", "/other");
        for _ in 0..2 {
            b.sight(4242, a1);
            b.sight(4242, a2);
            b.sight(99, other);
        }

        let mut taken = b.take(4242);
        taken.sort_unstable();
        let mut expected = vec![a1, a2];
        expected.sort_unstable();
        assert_eq!(
            taken, expected,
            "both agents armed on the pid are ended on its death"
        );

        assert!(
            b.take(4242).is_empty(),
            "the entry is removed on the first take"
        );
        assert_eq!(b.take(99).len(), 1);
    }

    #[test]
    fn the_second_sighting_arms_and_further_ones_do_not_rearm() {
        let mut b = Bindings::default();
        let a = AgentId::from_parts("codewhale", "/ws");
        assert!(!b.sight(7, a), "a first sighting is not a repeat");
        assert!(b.sight(7, a), "the second sighting is the repeat");
        assert!(!b.sight(7, a), "an already-armed pairing must not re-arm");
        assert_eq!(b.take(7).len(), 1, "a re-sighted (pid, agent) is deduped");
    }

    /// #896: a per-invocation wrapper reports a FRESH pid each hook, so no
    /// pairing ever repeats and nothing is ever armed.
    #[test]
    fn a_fresh_pid_per_hook_never_arms_and_never_accumulates() {
        let mut b = Bindings::default();
        let agent = AgentId::from_parts("cursor", "sess-A");
        for wrapper_pid in [4808, 5379, 8924, 9869, 13594] {
            assert!(
                !b.sight(wrapper_pid, agent),
                "wrapper pid {wrapper_pid} must never read as corroborated"
            );
        }
        assert!(b.armed.is_empty(), "nothing may be armed");
        assert_eq!(
            b.candidate.len(),
            1,
            "only the LATEST candidate is kept — one entry per agent, not per hook"
        );
    }

    /// Two agents behind one wrapper pid still bound by AGENT, not by sighting.
    #[test]
    fn candidates_are_bounded_by_live_agents() {
        let mut b = Bindings::default();
        for hook in 0..50 {
            for n in 0..3 {
                b.sight(
                    10_000 + hook,
                    AgentId::from_parts("cursor", &format!("s{n}")),
                );
            }
        }
        assert!(b.armed.is_empty());
        assert_eq!(
            b.candidate.len(),
            3,
            "one candidate per agent, 150 sightings"
        );
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
