//! The shared, daemon-AGNOSTIC presence layer. A "daemon" source (the OpenClaw
//! gateway is instance #1) produces NO agent activity — it has no desk, no
//! `AgentSlot`. Instead it earns ONE presence-gated wandering mascot whose
//! motion encodes the daemon's liveness (idle ambles, busy shuttles, down walks
//! out). This module owns the state machine + lifecycle that is identical for
//! EVERY daemon; the per-daemon WIRE decode (e.g. `openclaw::decode_openclaw_
//! hook_payload`, which maps a gateway envelope → `Vec<DaemonPresenceUpdate>`)
//! stays in the daemon's own module, exactly like an agent source owns its own
//! line/hook decoder.
//!
//! Presence rides a SIBLING channel (invariant #2: NOT the one `AgentEvent`
//! channel), carrying `PresenceMsg { source, delta }` so N
//! daemons land in DISTINCT `SceneState::daemons` entries. The reducer task
//! merges them via [`apply_presence`], NEVER through `Reducer::apply` (which is
//! `AgentId`-pure). See `docs/superpowers/specs/2026-06-15-source-kind-daemon-
//! agent-decouple-design.md`.

use std::time::SystemTime;

use crate::state::{DaemonPresence, DaemonState, SceneState};

// The runtime half (the tokio presence side channel + `PresenceExitWatch`) —
// ONE gate for the whole `native` layer of this module; the re-export keeps
// the pre-split `source::daemon::{PresenceSender, PresenceExitWatch,
// spawn_presence_exit_watch}` paths.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{spawn_presence_exit_watch, PresenceExitWatch, PresenceSender};

/// One presence delta for a daemon mascot — the SHARED vocabulary every daemon
/// emits (a daemon's wire decoder maps its own envelope onto these). The decode
/// arms produce the hook-derived variants; `PidExited` is emitted by the
/// [`PresenceExitWatch`] drain (the reducer wiring), never by a decoder. All
/// consumed by [`apply_presence`]. Source-agnostic ON PURPOSE: a 2nd daemon
/// needs ZERO new variants — the routing key rides the channel tuple, not the
/// enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonPresenceUpdate {
    /// `gateway_start` — the daemon is up; `pid` (its `process.pid`) is armed
    /// for `ExitWatch`. UP-winning + idempotent; resets the session count.
    GatewayUp { pid: Option<i32> },
    /// `gateway_stop` — clean shutdown.
    GatewayDown,
    /// `session_start` — a multiplexed session began (bumps the bubble count).
    SessionStarted,
    /// `session_end` — a session ended.
    SessionEnded,
    /// `before_agent_run` — a turn entered flight, keyed for self-healing busy.
    RunStarted { run_key: String },
    /// `agent_end` with `success: true` — a turn completed OK.
    RunEnded { run_key: String },
    /// `agent_end` with `success: false` (#317) — a turn FAILED (the model
    /// backend is broken: auth revoked, provider down). Drives `Degraded`.
    RunFailed { run_key: String },
    /// A live gateway pid OBSERVED on any event carrying `_pid` (#318) — adopted
    /// into `current_pid` ONLY when it was `None`, so a MID-ATTACH or a
    /// reconnect-while-alive can still arm the abrupt-down exit watch even though
    /// it never saw the `gateway_start` that carries the pid via `GatewayUp`.
    /// Does NOT change `DaemonState` (it's a pure pid adoption). `GatewayUp` still
    /// owns restart-rebinds (overwrites), so `PidSeen` never clobbers a known pid.
    PidSeen { pid: i32 },
    /// The armed gateway pid died (from the `ExitWatch` drain, not a decoder).
    PidExited { pid: i32 },
}

/// A presence delta tagged with WHICH daemon it belongs to — the routing key for
/// N daemons. Both producers (the `handle_conn` demux and the exit-watch drain)
/// emit this, so a daemon's deltas always reach the right `daemons[source]`. A
/// named struct (not a `(String, Update)` tuple) so the routing key can't be
/// read positionally at the seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceMsg {
    pub source: String,
    pub delta: DaemonPresenceUpdate,
}

/// Per-daemon decay/stale knobs. A daemon has no per-session pid, so silence is
/// the only abrupt-exit signal — these bound how long busy/up linger without
/// fresh deltas. Carried per-daemon (today every daemon uses [`PresenceTtl::
/// DEFAULT`]; a future faster/slower daemon sets its own without touching the
/// sweep, which already takes `ttl` as a parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceTtl {
    /// Grace before busy → idle when no `before_agent_run`/`agent_end` arrives
    /// (a dropped `agent_end` must self-heal, never strand perpetual busy).
    pub busy_decay_ms: u64,
    /// With no activity for this long the daemon is presumed DOWN (covers
    /// SIGTERM, where neither `session_end` nor `gateway_stop` fires).
    pub presence_ttl_ms: u64,
    /// How long a `Down` presence lingers (drawn walking out) before it is
    /// REMOVED (back to absent) — generously past the renderer's elevator walk-
    /// out so the leave animation always completes first.
    pub down_remove_ms: u64,
}

impl PresenceTtl {
    /// The default decay profile (OpenClaw's). 30s busy decay, 5min presence
    /// TTL, 5s Down linger.
    pub const DEFAULT: PresenceTtl = PresenceTtl {
        busy_decay_ms: 30_000,
        presence_ttl_ms: 5 * 60 * 1_000,
        down_remove_ms: 5_000,
    };
}

impl DaemonPresenceUpdate {
    /// The gateway pid this update arms the abrupt-down `ExitWatch` on, if any.
    /// The variant→pid mapping lives HERE (one place) so the driver's watch-arm
    /// and `apply_presence`'s `current_pid` adoption can't drift: `GatewayUp`
    /// carries the restart-rebind pid, `PidSeen` the mid-attach (#318) adopted pid.
    pub fn armable_pid(&self) -> Option<i32> {
        match self {
            DaemonPresenceUpdate::GatewayUp { pid } => *pid,
            DaemonPresenceUpdate::PidSeen { pid } => Some(*pid),
            _ => None,
        }
    }
}

impl DaemonPresence {
    /// Zero the "live work" pair (multiplexed-session bubble count + in-flight
    /// run keys) — one concept, always reset together on every restart-or-down
    /// path. (The busy-decay arm deliberately clears only `in_flight_run_keys`
    /// — keeping the session count — so it does NOT use this.)
    fn clear_concurrency(&mut self) {
        self.active_sessions = 0;
        self.in_flight_run_keys.clear();
    }

    /// Transition to `Down` + clear the live-work pair AND the armed pid — the
    /// daemon mirror of `fsm::accumulate_active_ms`-style single-owner
    /// transitions, so a must-clear-on-down field can't be forgotten at one of
    /// the four down sites. `current_pid` is cleared because a Down daemon has no
    /// live gateway pid: leaving it set strands the binding on the dead pid, so a
    /// reconnect-as-a-new-pid whose `gateway_start` is missed can't re-adopt via
    /// `PidSeen` (None-only) and the instant abrupt-down rung silently disarms on
    /// the SECOND cycle until the 5-min presence sweep. Does NOT touch
    /// `last_seen`: the `apply_presence` callers ride its top-level stamp, and the
    /// sweep / `mark_presence_down` re-anchor it explicitly for the walk-out timer.
    fn enter_down(&mut self) {
        self.state = DaemonState::Down;
        self.clear_concurrency();
        self.current_pid = None;
    }
}

/// Merge one presence delta into `scene.daemons[source]`. Called by the reducer
/// task off the SIBLING channel — NEVER through `Reducer::apply` (which is
/// `AgentId`-pure). Every update refreshes `last_seen` (any event is proof of
/// life) and "any event implies UP" resurrects a wrongly-DOWN daemon.
pub fn apply_presence(
    scene: &mut SceneState,
    source: &str,
    update: DaemonPresenceUpdate,
    now: SystemTime,
) {
    use DaemonPresenceUpdate::*;
    let p = scene
        .daemons_mut()
        .entry(source.to_string())
        .or_insert_with(|| DaemonPresence {
            state: DaemonState::Idle,
            active_sessions: 0,
            last_seen: now,
            entered_at: now,
            in_flight_run_keys: Default::default(),
            current_pid: None,
        });
    // A transition out of Down (or a fresh GatewayUp) re-anchors the enter
    // animation — the mascot scuttles back in from the elevator. Idle↔Busy
    // does NOT reset it, so the steady wander clock stays continuous.
    let was_down = p.state == DaemonState::Down;
    p.last_seen = now;
    match update {
        // UP-winning + idempotent. A (re)start resets the multiplexed-session
        // count + in-flight runs and rebinds the armed pid — so a later stale
        // `PidExited` for the OLD pid is ignored (restart rebind).
        GatewayUp { pid } => {
            p.current_pid = pid;
            p.clear_concurrency();
            p.state = DaemonState::Idle;
        }
        GatewayDown => {
            p.enter_down();
        }
        SessionStarted => {
            p.active_sessions = p.active_sessions.saturating_add(1);
            if p.state == DaemonState::Down {
                p.state = DaemonState::Idle; // any event ⇒ up
            }
        }
        SessionEnded => {
            // saturating: a pre-attach session_start we never saw must not underflow.
            p.active_sessions = p.active_sessions.saturating_sub(1);
            if p.state == DaemonState::Down {
                p.state = DaemonState::Idle;
            }
        }
        RunStarted { run_key } => {
            p.in_flight_run_keys.insert(run_key);
            p.state = DaemonState::Busy;
        }
        RunEnded { run_key } => {
            p.in_flight_run_keys.remove(&run_key);
            if p.in_flight_run_keys.is_empty() {
                // A successful run ending heals a prior Degraded back to Idle.
                p.state = DaemonState::Idle;
            }
        }
        // A FAILED run (#317): the gateway is alive but its model backend broke.
        // Degraded overrides Busy/Idle and persists until the next SUCCESSFUL run
        // (RunEnded → Idle) or a new attempt (RunStarted → Busy) or a restart
        // (GatewayUp → Idle). Remove this run from the in-flight set (it ended).
        RunFailed { run_key } => {
            p.in_flight_run_keys.remove(&run_key);
            p.state = DaemonState::Degraded;
        }
        // Pure pid adoption (#318): bootstrap `current_pid` for a live daemon we
        // never saw `gateway_start` for (mid-attach / reconnect-while-alive), so
        // the abrupt-down exit watch can arm. ONLY when None — `GatewayUp` owns
        // restart-rebinds, so this never clobbers a known LIVE pid; `enter_down`
        // clears `current_pid` so a reconnect after a Down re-adopts here. No
        // state change.
        PidSeen { pid } => {
            if p.current_pid.is_none() {
                p.current_pid = Some(pid);
            }
        }
        // Only the CURRENTLY-armed pid dying takes the daemon down. A stale
        // `PidExited` for an old pid after a restart (`current_pid` already
        // rebound to the new pid) is a no-op — the live daemon stays up.
        // `current_pid` is armed by `GatewayUp` (restart-rebind) AND adopted by
        // `PidSeen` (#318 mid-attach) — the gateway plugin now stamps `_pid` on
        // EVERY event, so a daemon pixtuoid attaches to AFTER its `gateway_start`
        // still arms this instant abrupt-down rung off the next event's `PidSeen`.
        PidExited { pid } => {
            if p.current_pid == Some(pid) {
                p.enter_down();
            }
        }
    }
    // Re-anchor the enter animation on a Down → up resurrection (the entry was
    // not yet TTL-swept). A fresh insert already stamped `entered_at = now`.
    if was_down && p.state != DaemonState::Down {
        p.entered_at = now;
    }
}

/// Decay one daemon's stale presence on the reducer's sweep tick: BUSY → IDLE
/// after `ttl.busy_decay_ms` of silence (a dropped `agent_end` self-heals —
/// never a latch), any live state → DOWN after `ttl.presence_ttl_ms` (covers
/// SIGTERM), and a `Down` entry is REMOVED after `ttl.down_remove_ms` (back to
/// absent, so it doesn't leak forever). Single-source so the reducer iterates
/// `registry::daemon_sources()` and each daemon decays on its own profile.
pub fn sweep_presence_ttl(scene: &mut SceneState, source: &str, ttl: PresenceTtl, now: SystemTime) {
    let map = scene.daemons_mut();
    let remove = {
        let Some(p) = map.get_mut(source) else {
            return;
        };
        let idle_ms = now
            .duration_since(p.last_seen)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if p.state == DaemonState::Down {
            // Keep the Down entry only until the walk-out has had time to finish.
            idle_ms >= ttl.down_remove_ms
        } else {
            if idle_ms >= ttl.presence_ttl_ms {
                p.enter_down();
                // Re-anchor `last_seen` to NOW so the renderer's `now - last_seen`
                // walk-out timer starts at 0 and the mascot plays the elevator
                // leave (without this the entry is ≥TTL stale → it vanishes with
                // no walk-out). The apply-path GatewayDown/PidExited ride
                // `apply_presence`'s top-level stamp instead.
                p.last_seen = now;
            } else if p.state == DaemonState::Busy && idle_ms >= ttl.busy_decay_ms {
                p.state = DaemonState::Idle;
                p.in_flight_run_keys.clear();
            }
            false
        }
    };
    if remove {
        map.remove(source);
    }
}

/// Drive a source's presence to `Down` (arming the renderer's walk-out) iff it
/// exists and is not already Down — idempotent, so the `down_remove_ms` removal
/// timer in [`sweep_presence_ttl`] isn't reset on every tick. The runtime calls
/// this to walk a mascot out when its source is DISCONNECTED in the Sources
/// panel: the presence side-channel is separate from the `AgentEvent`
/// connection gate, so a disconnect must reconcile presence too (mirrors the
/// reducer's `reconcile_connected` for agents).
pub fn mark_presence_down(scene: &mut SceneState, source: &str, now: SystemTime) {
    if let Some(p) = scene.daemons_mut().get_mut(source) {
        if p.state != DaemonState::Down {
            p.enter_down();
            p.last_seen = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The presence state machine is daemon-AGNOSTIC: every assertion runs
    // against TWO synthetic sources to PROVE a 2nd daemon needs zero new
    // state-machine code (the multi-daemon directive's structural guarantee).
    const SOURCES: [&str; 2] = ["openclaw", "daemon2"];

    fn ms(m: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(m)
    }
    fn st(s: &SceneState, src: &str) -> DaemonState {
        s.daemons()[src].state
    }
    fn sessions(s: &SceneState, src: &str) -> u32 {
        s.daemons()[src].active_sessions
    }
    fn entered_at(s: &SceneState, src: &str) -> SystemTime {
        s.daemons()[src].entered_at
    }
    fn last_seen(s: &SceneState, src: &str) -> SystemTime {
        s.daemons()[src].last_seen
    }
    fn up(s: &mut SceneState, src: &str, pid: i32, at: u64) {
        apply_presence(
            s,
            src,
            DaemonPresenceUpdate::GatewayUp { pid: Some(pid) },
            ms(at),
        );
    }

    #[test]
    fn gateway_up_sets_idle_and_records_pid() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 4242, 0);
            assert_eq!(st(&s, src), DaemonState::Idle);
            assert_eq!(s.daemons()[src].current_pid, Some(4242));
        }
    }

    #[test]
    fn gateway_up_resets_sessions_and_in_flight_runs() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(0));
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(2),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            up(&mut s, src, 1, 3);
            assert_eq!(st(&s, src), DaemonState::Idle);
            assert_eq!(sessions(&s, src), 0);
            assert!(s.daemons()[src].in_flight_run_keys.is_empty());
        }
    }

    #[test]
    fn gateway_down_sets_down() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(1));
            assert_eq!(st(&s, src), DaemonState::Down);
        }
    }

    #[test]
    fn session_count_increments_and_saturates_at_zero() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(0));
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
            assert_eq!(sessions(&s, src), 2);
            for i in 0..3 {
                apply_presence(&mut s, src, DaemonPresenceUpdate::SessionEnded, ms(2 + i));
            }
            assert_eq!(
                sessions(&s, src),
                0,
                "saturating — a pre-attach miss never underflows"
            );
        }
    }

    #[test]
    fn busy_holds_until_the_last_run_ends() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "a".into(),
                },
                ms(0),
            );
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "b".into(),
                },
                ms(1),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunEnded {
                    run_key: "a".into(),
                },
                ms(2),
            );
            assert_eq!(st(&s, src), DaemonState::Busy, "b still in flight");
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunEnded {
                    run_key: "b".into(),
                },
                ms(3),
            );
            assert_eq!(st(&s, src), DaemonState::Idle);
        }
    }

    // ---- #317: the Degraded (model-error) arm ----

    #[test]
    fn failed_run_degrades_the_daemon() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "r".into(),
                },
                ms(1),
            );
            assert_eq!(
                st(&s, src),
                DaemonState::Degraded,
                "agent_end.success:false ⇒ degraded"
            );
            assert!(
                s.daemons()[src].in_flight_run_keys.is_empty(),
                "the failed run leaves the in-flight set"
            );
        }
    }

    #[test]
    fn a_new_run_clears_degraded_back_to_busy() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "a".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Degraded);
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "b".into(),
                },
                ms(1),
            );
            assert_eq!(
                st(&s, src),
                DaemonState::Busy,
                "a fresh attempt re-enters Busy (the gateway is trying again)"
            );
        }
    }

    #[test]
    fn a_successful_run_heals_degraded_to_idle() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "a".into(),
                },
                ms(0),
            );
            // The next attempt enters flight, then SUCCEEDS.
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "b".into(),
                },
                ms(1),
            );
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunEnded {
                    run_key: "b".into(),
                },
                ms(2),
            );
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "a clean run drains the in-flight set ⇒ heals to idle"
            );
        }
    }

    #[test]
    fn gateway_restart_clears_degraded() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "a".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Degraded);
            up(&mut s, src, 9, 1);
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "a restart (re-auth, provider back) clears the degraded latch"
            );
        }
    }

    // ---- #318: the PidSeen mid-attach pid adoption ----

    #[test]
    fn pid_seen_adopts_when_current_pid_is_none() {
        for src in SOURCES {
            // Mid-attach: pixtuoid never saw `gateway_start`, so the entry is
            // first created by a plain activity event carrying `_pid`.
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::PidSeen { pid: 555 },
                ms(0),
            );
            assert_eq!(
                s.daemons()[src].current_pid,
                Some(555),
                "the live pid is adopted so the instant abrupt-down rung can arm"
            );
            // And it does NOT change the state (pure pid adoption).
            assert_eq!(st(&s, src), DaemonState::Idle);
            // The adopted pid dying now takes the daemon down (the #318 payoff).
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::PidExited { pid: 555 },
                ms(1),
            );
            assert_eq!(st(&s, src), DaemonState::Down);
        }
    }

    #[test]
    fn pid_seen_never_clobbers_a_known_pid() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 100, 0);
            // A later event re-stamps a (possibly stale) pid — must NOT overwrite
            // the authoritative `GatewayUp` binding (restart-rebind owns that).
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::PidSeen { pid: 999 },
                ms(1),
            );
            assert_eq!(
                s.daemons()[src].current_pid,
                Some(100),
                "PidSeen is adopt-only-when-None; GatewayUp owns rebinds"
            );
        }
    }

    #[test]
    fn pid_seen_is_pure_adoption_and_does_not_change_state() {
        // PidSeen adopts the pid but is intentionally state-NEUTRAL — the decoder
        // ALWAYS prepends it to a state-bearing update (`out.insert(0, PidSeen)`
        // only when `out` is non-empty), so resurrection rides on that sibling
        // update, never on PidSeen alone. Verify the state-neutrality directly.
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply_presence(&mut s, src, DaemonPresenceUpdate::PidSeen { pid: 7 }, ms(1));
            assert_eq!(
                st(&s, src),
                DaemonState::Down,
                "PidSeen is pure pid adoption — it does NOT resurrect by itself"
            );
            assert_eq!(s.daemons()[src].current_pid, Some(7));
        }
    }

    #[test]
    fn armable_pid_is_only_gateway_up_some_and_pid_seen() {
        // The ONE variant→exit-watch-pid mapping the driver arms on must match the
        // pids apply_presence adopts into current_pid.
        use DaemonPresenceUpdate::*;
        assert_eq!(GatewayUp { pid: Some(7) }.armable_pid(), Some(7));
        assert_eq!(GatewayUp { pid: None }.armable_pid(), None);
        assert_eq!(PidSeen { pid: 9 }.armable_pid(), Some(9));
        assert_eq!(GatewayDown.armable_pid(), None);
        assert_eq!(SessionStarted.armable_pid(), None);
        assert_eq!(
            RunStarted {
                run_key: "r".into()
            }
            .armable_pid(),
            None
        );
        assert_eq!(PidExited { pid: 3 }.armable_pid(), None);
    }

    #[test]
    fn pid_seen_re_adopts_after_an_abrupt_down_so_the_second_cycle_arms() {
        // #318 fixed the FIRST (None) adoption, but an abrupt-down must also let
        // the rung RE-arm. After PidExited takes the daemon Down, a reconnect as a
        // NEW pid whose gateway_start is missed is learned only via PidSeen — which
        // must adopt it (current_pid was stranded on the dead pid before this fix),
        // so the next PidExited takes the daemon down INSTANTLY rather than waiting
        // for the 5-min presence_ttl sweep.
        use DaemonPresenceUpdate::*;
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 100, 0); // P1 live
            apply_presence(&mut s, src, PidExited { pid: 100 }, ms(1)); // P1 dies → Down
            assert_eq!(st(&s, src), DaemonState::Down);
            // Reconnect as P2; gateway_start missed → only a normal event + PidSeen.
            apply_presence(&mut s, src, PidSeen { pid: 200 }, ms(2));
            apply_presence(&mut s, src, SessionStarted, ms(3)); // any event ⇒ up
            assert_eq!(
                s.daemons()[src].current_pid,
                Some(200),
                "PidSeen must re-adopt the live pid after a Down"
            );
            // P2 dying now takes the daemon down instantly (the rung re-armed).
            apply_presence(&mut s, src, PidExited { pid: 200 }, ms(4));
            assert_eq!(
                st(&s, src),
                DaemonState::Down,
                "the second-cycle PidExited re-armed the instant abrupt-down rung"
            );
        }
    }

    #[test]
    fn pid_exit_matching_current_takes_the_daemon_down() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 7, 0);
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::PidExited { pid: 7 },
                ms(1),
            );
            assert_eq!(st(&s, src), DaemonState::Down);
        }
    }

    #[test]
    fn stale_pid_exit_after_restart_leaves_the_daemon_up() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            up(&mut s, src, 2, 1);
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::PidExited { pid: 1 },
                ms(2),
            );
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "P2 stays up; stale P1 exit ignored"
            );
            assert_eq!(s.daemons()[src].current_pid, Some(2));
        }
    }

    #[test]
    fn any_event_resurrects_from_down() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "any presence event implies up"
            );
        }
    }

    #[test]
    fn session_ended_resurrects_from_down() {
        // The SessionEnded arm ALSO carries the "any event ⇒ up" resurrect — the
        // sibling test exercises it only via SessionStarted. From a Down entry
        // (active_sessions already zeroed by enter_down) a session_end resurrects
        // to Idle, and the saturating_sub of a never-seen session must not
        // underflow on the pre-attach miss.
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionEnded, ms(1));
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "a session_end event implies up (resurrects from Down)"
            );
            assert_eq!(
                sessions(&s, src),
                0,
                "saturating — the pre-attach session_start miss must not underflow"
            );
        }
    }

    #[test]
    fn entered_at_reanchors_on_resurrection_but_not_on_idle_busy() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            assert_eq!(entered_at(&s, src), ms(0));
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(2000),
            );
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunEnded {
                    run_key: "r".into(),
                },
                ms(3000),
            );
            assert_eq!(
                entered_at(&s, src),
                ms(0),
                "idle↔busy must not move entered_at"
            );
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(4000));
            apply_presence(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(9000));
            assert_eq!(st(&s, src), DaemonState::Idle);
            assert_eq!(
                entered_at(&s, src),
                ms(9000),
                "resurrection re-anchors the walk-in"
            );
        }
    }

    #[test]
    fn mark_presence_down_arms_the_walkout_idempotently() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            mark_presence_down(&mut s, src, ms(1000));
            assert_eq!(st(&s, src), DaemonState::Down);
            assert_eq!(
                last_seen(&s, src),
                ms(1000),
                "Down re-anchors last_seen for the walk-out"
            );
            mark_presence_down(&mut s, src, ms(5000));
            assert_eq!(
                last_seen(&s, src),
                ms(1000),
                "idempotent: already-Down is untouched"
            );
        }
        // Unknown source is a no-op (no panic / no phantom entry).
        let mut s = SceneState::default();
        up(&mut s, "openclaw", 1, 0);
        mark_presence_down(&mut s, "not-a-source", ms(6000));
        assert_eq!(s.daemons().len(), 1);
    }

    #[test]
    fn sweep_takes_the_daemon_down_after_presence_ttl() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.presence_ttl_ms + 1));
            assert_eq!(
                st(&s, src),
                DaemonState::Down,
                "silence past the TTL ⇒ down (covers SIGTERM)"
            );
            assert_eq!(sessions(&s, src), 0);
            assert_eq!(
                last_seen(&s, src),
                ms(ttl.presence_ttl_ms + 1),
                "walk-out anchor re-stamped"
            );
        }
    }

    #[test]
    fn sweep_removes_a_down_entry_after_the_walkout_window() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.down_remove_ms - 1));
            assert!(s.daemons().contains_key(src), "still present mid walk-out");
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.down_remove_ms + 1));
            assert!(
                !s.daemons().contains_key(src),
                "removed once the walk-out window elapsed"
            );
        }
    }

    #[test]
    fn sweep_self_heals_a_stranded_busy_after_the_grace_window() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "stranded".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.busy_decay_ms + 1));
            assert_eq!(
                st(&s, src),
                DaemonState::Idle,
                "stranded busy self-heals to idle"
            );
            assert!(s.daemons()[src].in_flight_run_keys.is_empty());
        }
    }

    #[test]
    fn sweep_does_not_busy_decay_a_degraded_daemon_but_ttl_takes_it_down() {
        // #317: a Degraded gateway is NOT a stale Busy — the busy_decay arm only
        // matches Busy, so a broken gateway can't silently "heal" to Idle on a
        // dropped event (only a real RunEnded/RunStarted/GatewayUp heals). It does
        // still go Down on the presence_ttl silence (covers a SIGTERM'd broken gateway).
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "r".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Degraded);
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.busy_decay_ms + 1));
            assert_eq!(
                st(&s, src),
                DaemonState::Degraded,
                "Degraded must NOT busy-decay to Idle (only Busy does)"
            );
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.presence_ttl_ms + 1));
            assert_eq!(
                st(&s, src),
                DaemonState::Down,
                "silence past the TTL takes even a Degraded daemon down"
            );
        }
    }

    #[test]
    fn sweep_on_an_unknown_source_is_a_noop() {
        // The `let Some(p) = map.get_mut(source) else { return }` guard: a sweep
        // tick for a source with NO presence entry must not mint a phantom
        // Down/Idle entry (a mutation to or_insert_with would). The map is empty
        // before and stays empty after, even far past every TTL.
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        assert!(s.daemons().is_empty());
        sweep_presence_ttl(&mut s, "never-seen", ttl, ms(ttl.presence_ttl_ms + 1));
        assert!(
            s.daemons().is_empty(),
            "sweeping an unknown source mints no phantom entry"
        );
        assert!(!s.daemons().contains_key("never-seen"));
    }

    #[test]
    fn sweep_within_the_grace_window_keeps_busy() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply_presence(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(0),
            );
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.busy_decay_ms - 1));
            assert_eq!(
                st(&s, src),
                DaemonState::Busy,
                "still within the decay grace"
            );
        }
    }

    // The cross-daemon isolation proof: two daemons coexist in one scene with
    // INDEPENDENT state — a delta for one never touches the other's entry. This
    // is the structural guarantee behind "register a 2nd daemon = one row".
    #[test]
    fn two_daemons_coexist_with_independent_presence() {
        let mut s = SceneState::default();
        up(&mut s, "openclaw", 1, 0);
        apply_presence(
            &mut s,
            "daemon2",
            DaemonPresenceUpdate::RunStarted {
                run_key: "r".into(),
            },
            ms(1),
        );
        assert_eq!(st(&s, "openclaw"), DaemonState::Idle);
        assert_eq!(st(&s, "daemon2"), DaemonState::Busy);
        // Taking openclaw down leaves daemon2 untouched.
        apply_presence(&mut s, "openclaw", DaemonPresenceUpdate::GatewayDown, ms(2));
        assert_eq!(st(&s, "openclaw"), DaemonState::Down);
        assert_eq!(
            st(&s, "daemon2"),
            DaemonState::Busy,
            "daemon2 unaffected by openclaw down"
        );
        assert_eq!(s.daemons().len(), 2);
    }
}
