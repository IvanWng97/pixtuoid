//! Liveness for HOOK-ONLY sources whose shim can supply the agent CLI's pid.
//!
//! A hook-only source (CodeWhale, and the same gap exists for Reasonix) has no
//! tailable transcript and therefore none of the JSONL watcher's liveness ladder
//! (the FD/registry probe, the negative vouch, the instant exit). Its ONLY exit
//! signal is the best-effort `session_end` hook on a CLEAN quit — an abrupt exit
//! (kill/crash/terminal-close) fires nothing, so the sprite ghosts until the
//! 10–30 min stale-sweep.
//!
//! When the shim can stamp the CLI's pid (`_pid` — CodeWhale's Unix hooks run
//! under `sh -c`, which EXEC's the command, so the shim's `getppid()` IS the CLI;
//! see `pixtuoid-hook`), this watch closes that gap WITHOUT touching the
//! JSONL-bound ladder: it reuses the standalone [`ExitWatch`] (kqueue
//! `NOTE_EXIT` / Linux `pidfd`) to emit a `SessionEnd` for every agent bound to a
//! pid the moment that pid dies — clean OR abrupt. It is fed ONLY from the hook
//! decode path (`handle_conn`), so it is inert for sources whose payloads carry
//! no `_pid`.
//!
//! Self-healing, mirroring the negative vouch: if the supplied pid is ever wrong
//! (a shell that forks instead of exec's, so `_pid` is the shell not the CLI),
//! the false `SessionEnd` just walks the sprite out and the CLI's next hook event
//! walks it back in (the `message_submit`→`SessionStart` resurrect path).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::source::exit_watch::ExitWatch;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

/// Cloneable handle (one per hook connection task) over a shared pid→agents
/// registry + the process-exit watcher. `None` from [`HookPidWatch::spawn`] on
/// platforms without an exit-watch backend (Windows, pre-5.3 Linux) — the source
/// then falls back to `session_end` + the stale-sweep.
#[derive(Clone)]
pub(crate) struct HookPidWatch {
    exit: Arc<ExitWatch>,
    /// pid → the agents to end when it dies. Self-cleans: the pid dies either way
    /// (clean or abrupt) → the entry is removed on the death event, so it never
    /// accumulates across CLI launches.
    pids: Arc<Mutex<HashMap<i32, HashSet<AgentId>>>>,
}

impl HookPidWatch {
    /// Spawn the exit watcher + the drain task that turns a dead pid into a
    /// `SessionEnd` for each agent bound to it. `tx` is the reducer's event
    /// channel (the synthesized end is Hook-tagged, like the source's own
    /// events). `None` if no exit-watch backend exists on this platform.
    pub(crate) fn spawn(tx: TaggedSender) -> Option<Self> {
        let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
        let exit = Arc::new(ExitWatch::spawn(exit_tx)?);
        let pids: Arc<Mutex<HashMap<i32, HashSet<AgentId>>>> = Arc::new(Mutex::new(HashMap::new()));
        let this = Self { exit, pids };
        let drain = this.clone();
        tokio::spawn(async move {
            while let Some(dead) = exit_rx.recv().await {
                // Take the entry (the pid is gone — clean or abrupt) and end each
                // agent it carried. A SessionEnd for an already-ended agent (the
                // clean-quit case, where `session_end` ended it first) is a
                // reducer no-op, so the redundant end is harmless.
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

    /// Bind `agent_id` to `pid` and start watching the pid (idempotent). Called
    /// from the hook decode path for every registration carrier whose payload
    /// carried a `_pid` — `SessionStart` AND `Identity` (`pid_bind_target`), the
    /// latter being the only carrier for a mid-attached session whose
    /// `SessionStart` predates the daemon.
    pub(crate) fn note(&self, pid: i32, agent_id: AgentId) {
        note_pid(&self.pids, pid, agent_id);
        self.exit.watch(pid);
    }

    /// Remove `pid`'s entry and return the agents bound to it.
    fn take(&self, pid: i32) -> Vec<AgentId> {
        take_pid(&self.pids, pid)
    }
}

type PidMap = Mutex<HashMap<i32, HashSet<AgentId>>>;

/// Registry ops, split from the [`ExitWatch`] side so they're unit-testable
/// without spawning the platform watcher thread.
fn note_pid(pids: &PidMap, pid: i32, agent_id: AgentId) {
    pids.lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(pid)
        .or_default()
        .insert(agent_id);
}

/// Remove `pid`'s entry and return the agents bound to it (empty if none). The
/// removal keeps the map from accumulating across CLI launches — the pid dies
/// (clean or abrupt) exactly once, taking its entry with it.
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

        // Self-cleaning: a second death of the same (recycled) pid carries nothing.
        assert!(
            take_pid(&pids, 4242).is_empty(),
            "the entry is removed on the first take"
        );
        // An unrelated pid is untouched.
        assert_eq!(take_pid(&pids, 99).len(), 1);
    }

    #[test]
    fn note_is_idempotent_per_agent() {
        let pids: PidMap = Mutex::new(HashMap::new());
        let a = AgentId::from_parts("codewhale", "/ws");
        note_pid(&pids, 7, a);
        note_pid(&pids, 7, a); // message_submit re-fires every prompt → same (pid, agent)
        assert_eq!(
            take_pid(&pids, 7).len(),
            1,
            "a re-noted (pid, agent) is deduped"
        );
    }

    // End-to-end through the REAL exit watcher: a watched process dying emits a
    // SessionEnd for its bound agent. Gated on the platform backend (kqueue on
    // macOS / pidfd on Linux — `spawn` returns None on Windows / pre-5.3 Linux,
    // where the test is a no-op). Mirrors the JSONL path's instant-exit test.
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
}
