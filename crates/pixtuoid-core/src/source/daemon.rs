//! The shared, daemon-AGNOSTIC presence layer. A "daemon" source (the OpenClaw
//! gateway is instance #1) produces NO agent activity — it has no desk, no
//! `AgentSlot`. Instead each running daemon INSTANCE earns one presence-gated
//! wandering mascot whose motion encodes that instance's liveness (idle ambles,
//! busy shuttles, down walks out). This module owns the state machine + lifecycle
//! that is identical for EVERY daemon; the per-daemon WIRE decode (e.g.
//! `openclaw::decode_openclaw_hook_payload`, which maps a gateway envelope →
//! [`DecodedPresence`](crate::source::daemon::DecodedPresence)) stays in the daemon's own module, exactly like an agent
//! source owns its own line/hook decoder.
//!
//! Presence rides a SIBLING channel (invariant #2: NOT the one `AgentEvent`
//! channel), carrying `PresenceMsg { key: DaemonInstanceKey, delta }` so N daemons
//! AND N instances of one daemon land in DISTINCT `SceneState::daemons` entries.
//! The reducer task merges them via
//! [`apply_presence`](crate::source::daemon::apply_presence), NEVER through
//! `Reducer::apply` (which is `AgentId`-pure). See
//! `docs/superpowers/specs/2026-06-15-source-kind-daemon-agent-decouple-design.md`.
//!
//! **The two identity concepts are deliberately separate.** The
//! [`DaemonInstanceId`](crate::state::DaemonInstanceId) is STABLE — a gateway restarting on the same port keeps
//! its mascot — while `DaemonPresence::current_pid` is the PROCESS incarnation,
//! rebound by each `GatewayUp`. That split is what makes a late exit receipt for
//! a replaced process a no-op instead of a kill of its replacement, and it is why
//! neither the pid nor the profile name is the identity (a pid changes on every
//! restart; a profile is an install scope OpenClaw never puts on the wire).

use std::time::SystemTime;

use crate::state::{DaemonInstanceId, DaemonLiveness, DaemonPresence, SceneState};

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
/// consumed by [`apply_presence`]. Identity-agnostic ON PURPOSE: a 2nd daemon —
/// or a 2nd instance of one — needs ZERO new variants, because the routing
/// [`DaemonInstanceKey`] rides the channel message, not the enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonPresenceUpdate {
    /// `gateway_start` — the daemon is up; `pid` (its `process.pid`) is armed
    /// for `ExitWatch`. UP-winning + idempotent; resets the session count.
    GatewayUp {
        /// The gateway's `process.pid`, armed for the abrupt-down exit watch (`None` if unknown).
        pid: Option<i32>,
    },
    /// `gateway_stop` — clean shutdown.
    GatewayDown,
    /// `session_start` — a multiplexed session began (bumps the bubble count).
    SessionStarted,
    /// `session_end` — a session ended.
    SessionEnded,
    /// `before_agent_run` — a turn entered flight, keyed for self-healing busy.
    RunStarted {
        /// Correlates this turn with its later `RunEnded`/`RunFailed`.
        run_key: String,
    },
    /// `agent_end` with `success: true` — a turn completed OK.
    RunEnded {
        /// The completed turn's correlation key (matches its `RunStarted`).
        run_key: String,
    },
    /// `agent_end` with `success: false` (#317) — a turn FAILED (the model
    /// backend is broken: auth revoked, provider down). Drives `Degraded`.
    RunFailed {
        /// The failed turn's correlation key (matches its `RunStarted`).
        run_key: String,
    },
    /// A live gateway pid OBSERVED on any event carrying `_pid` (#318) — adopted
    /// into `current_pid` ONLY when it was `None`, so a MID-ATTACH or a
    /// reconnect-while-alive can still arm the abrupt-down exit watch even though
    /// it never saw the `gateway_start` that carries the pid via `GatewayUp`.
    /// Does NOT change `DaemonState` (it's a pure pid adoption). `GatewayUp` still
    /// owns restart-rebinds (overwrites), so `PidSeen` never clobbers a known pid.
    PidSeen {
        /// The live gateway pid observed on the event.
        pid: i32,
    },
    /// The armed gateway pid died (from the `ExitWatch` drain, not a decoder).
    PidExited {
        /// The gateway pid that exited.
        pid: i32,
    },
}

/// WHICH daemon mascot a presence delta belongs to: the registry source name
/// (which mascot definition + which connection gate) plus the source-owned
/// [`DaemonInstanceId`] (WHICH running instance of it). The routing key for N
/// daemons AND for N instances of one daemon — a named struct, not a tuple, so it
/// can't be read positionally at any of the four seams it crosses (hook demux →
/// side channel → state machine → exit watch).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DaemonInstanceKey {
    source: String,
    instance: DaemonInstanceId,
}

impl DaemonInstanceKey {
    /// Bind a source name to one of its instance ids.
    pub fn new(source: impl Into<String>, instance: DaemonInstanceId) -> Self {
        Self {
            source: source.into(),
            instance,
        }
    }

    /// The registry source name (the mascot definition + the connection gate).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The instance within that source.
    pub fn instance(&self) -> &DaemonInstanceId {
        &self.instance
    }
}

/// A presence delta tagged with the exact mascot it belongs to. Both producers
/// (the `handle_conn` demux and the exit-watch drain) emit this, so a delta always
/// reaches the right instance. A named struct (not a `(Key, Update)` tuple) so the
/// routing key can't be read positionally at the seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceMsg {
    /// The exact daemon instance this delta routes to.
    pub key: DaemonInstanceKey,
    /// The presence delta to apply to that instance.
    pub delta: DaemonPresenceUpdate,
}

/// One decoded daemon envelope: WHICH instance sent it plus the presence deltas
/// it implies. The return type of every `presence_decoder` — the instance id is
/// mandatory at the wire boundary, because it is the identity a source-wide
/// bucket used to swallow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPresence {
    /// The sending instance, as the source's own decoder resolved it.
    pub instance: DaemonInstanceId,
    /// The presence deltas the envelope implies (empty for a benign skip).
    pub updates: Vec<DaemonPresenceUpdate>,
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
    /// path. (The busy-decay arm deliberately clears only `in_flight_runs`
    /// — keeping the session count — so it does NOT use this.)
    fn clear_concurrency(&mut self) {
        self.active_sessions = 0;
        self.in_flight_runs.clear();
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
        self.liveness = DaemonLiveness::Down;
        self.clear_concurrency();
        self.current_pid = None;
    }
}

/// Merge one presence delta into `key`'s `(source, instance)` entry of
/// `scene.daemons` — never a source-wide one. Called by the reducer
/// task off the SIBLING channel — NEVER through `Reducer::apply` (which is
/// `AgentId`-pure). A proof-of-life update refreshes `last_seen` and "any event
/// implies UP" resurrects a wrongly-DOWN daemon; `PidExited` is the exception — a
/// DEATH signal that is non-creating (it never materializes an absent daemon).
pub fn apply_presence(
    scene: &mut SceneState,
    key: &DaemonInstanceKey,
    update: DaemonPresenceUpdate,
    now: SystemTime,
) {
    use DaemonPresenceUpdate::*;
    let (source, instance) = (key.source(), key.instance());
    // `PidExited` is a DEATH signal (synthesized by the exit-watch drain), NOT
    // proof of life, so — unlike every other delta — it must NEVER materialize a
    // daemon: a fresh entry has `current_pid == None`, the arm's `current_pid ==
    // Some(pid)` guard fails, and the entry is left UP — a phantom live idle mascot
    // for a gateway that is actually dead (a resurrection if it was already
    // TTL-removed; the exit watch races the removal sweep). For an absent daemon
    // the death is a no-op. Every OTHER delta is proof of life and (re)creates UP.
    let p = if matches!(update, PidExited { .. }) {
        let Some(p) = scene.daemons.get_mut(source, instance) else {
            return;
        };
        p
    } else {
        scene
            .daemons
            .get_or_insert_with(source, instance, || DaemonPresence {
                liveness: DaemonLiveness::UP,
                active_sessions: 0,
                last_seen: now,
                entered_at: now,
                in_flight_runs: Default::default(),
                current_pid: None,
            })
    };
    // Only a transition OUT of Down re-anchors the enter animation below (the
    // mascot scuttles back in from the elevator); Idle↔Busy — and a `GatewayUp` for
    // a daemon already UP — leave it, so the steady wander clock stays continuous.
    let was_down = p.liveness == DaemonLiveness::Down;
    p.last_seen = now;
    match update {
        // UP-winning + idempotent. A (re)start resets the multiplexed-session
        // count + in-flight runs and rebinds the armed pid — so a later stale
        // `PidExited` for the OLD pid is ignored (restart rebind).
        GatewayUp { pid } => {
            p.current_pid = pid;
            p.clear_concurrency();
            p.liveness = DaemonLiveness::UP;
        }
        GatewayDown => {
            p.enter_down();
        }
        SessionStarted => {
            p.active_sessions = p.active_sessions.saturating_add(1);
            if p.liveness == DaemonLiveness::Down {
                p.liveness = DaemonLiveness::UP; // any event ⇒ up
            }
        }
        SessionEnded => {
            // saturating: a pre-attach session_start we never saw must not underflow.
            p.active_sessions = p.active_sessions.saturating_sub(1);
            if p.liveness == DaemonLiveness::Down {
                p.liveness = DaemonLiveness::UP;
            }
        }
        RunStarted { run_key } => {
            // Stamped with THIS observation, so the run ages on its own clock (a
            // duplicate start is idempotent and simply refreshes the lease).
            p.in_flight_runs.insert(run_key, now);
            // A run starting means alive + not degraded (a fresh attempt clears a
            // prior model-error). Busy itself is DERIVED from the now-non-empty
            // run set by `display_state()`, never stored.
            p.liveness = DaemonLiveness::UP;
        }
        RunEnded { run_key } => {
            p.in_flight_runs.remove(&run_key);
            if p.in_flight_runs.is_empty() {
                // The set drained: a clean run heals a prior Degraded and, if the
                // daemon was Down, resurrects it. Idle is the derived projection
                // of an empty run set, so there is nothing else to set.
                p.liveness = DaemonLiveness::UP;
            }
        }
        // A FAILED run (#317): the gateway is alive but its model backend broke.
        // Degraded overrides Busy/Idle and persists until the next SUCCESSFUL run
        // (RunEnded → Idle) or a new attempt (RunStarted → Busy) or a restart
        // (GatewayUp → Idle). Remove this run from the in-flight set (it ended).
        RunFailed { run_key } => {
            p.in_flight_runs.remove(&run_key);
            // Degraded regardless of any OTHER run still in flight — the
            // projection renders Degraded over Busy (degraded checked first).
            p.liveness = DaemonLiveness::Up { degraded: true };
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
    if was_down && p.liveness != DaemonLiveness::Down {
        p.entered_at = now;
    }
}

/// Decay one daemon source's stale presence on the reducer's sweep tick,
/// INSTANCE-LOCALLY: each instance decays on its OWN `last_seen`, so traffic from
/// gateway A can never renew gateway B. Per instance: each in-flight RUN expires
/// `ttl.busy_decay_ms` after ITS OWN last observation (so a dropped `agent_end`
/// self-heals even while the gateway keeps serving other runs — never a latch),
/// any live state → DOWN after `ttl.presence_ttl_ms` of total silence (SIGTERM),
/// and a `Down` entry is REMOVED after `ttl.down_remove_ms` (back to absent, so
/// it doesn't leak forever). Expiring a run lease never clears `degraded` (a
/// separate axis — only a real `RunEnded`/`RunStarted`/`GatewayUp` heals that).
/// Source-scoped so the reducer iterates
/// `registry::daemon_sources()` and each daemon decays on its own profile.
pub fn sweep_presence_ttl(scene: &mut SceneState, source: &str, ttl: PresenceTtl, now: SystemTime) {
    let mut doomed: Vec<DaemonInstanceId> = Vec::new();
    for (instance, p) in scene.daemons.instances_of_mut(source) {
        let idle_ms = now
            .duration_since(p.last_seen)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if p.liveness == DaemonLiveness::Down {
            // Keep the Down entry only until the walk-out has had time to finish.
            if idle_ms >= ttl.down_remove_ms {
                doomed.push(instance.clone());
            }
        } else if idle_ms >= ttl.presence_ttl_ms {
            p.enter_down();
            // Re-anchor `last_seen` to NOW so the renderer's `now - last_seen`
            // walk-out timer starts at 0 and the mascot plays the elevator
            // leave (without this the entry is ≥TTL stale → it vanishes with
            // no walk-out). The apply-path GatewayDown/PidExited ride
            // `apply_presence`'s top-level stamp instead.
            p.last_seen = now;
        } else {
            // Per-RUN clocks, not the daemon-wide `last_seen` (which ANY event
            // refreshes): otherwise a gateway that keeps serving other traffic
            // latches Busy forever on one dropped `agent_end`.
            p.in_flight_runs.retain(|_, started| {
                now.duration_since(*started)
                    .map(|d| (d.as_millis() as u64) < ttl.busy_decay_ms)
                    // A clock regression keeps the lease (the repo-wide
                    // `duration_since(..).unwrap_or(0)` policy).
                    .unwrap_or(true)
            });
        }
    }
    scene.daemons.remove_instances(source, &doomed);
}

/// Drive EVERY instance of a source to `Down` (arming the renderer's walk-out),
/// skipping any already Down — idempotent, so the `down_remove_ms` removal timer
/// in [`sweep_presence_ttl`] isn't reset on every tick. The runtime calls this to
/// walk a source's mascots out when it is DISCONNECTED in the Sources panel: the
/// presence side-channel is separate from the `AgentEvent` connection gate, so a
/// disconnect must reconcile presence too (mirrors the reducer's
/// `reconcile_connected` for agents). Source-WIDE by design — disconnecting
/// OpenClaw walks out every gateway, not one.
pub fn mark_presence_down(scene: &mut SceneState, source: &str, now: SystemTime) {
    for (_, p) in scene.daemons.instances_of_mut(source) {
        if p.liveness != DaemonLiveness::Down {
            p.enter_down();
            p.last_seen = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The render vocabulary the assertions compare against — projected via
    // `display_state()`; production daemon.rs now mutates only `DaemonLiveness`.
    use crate::state::DaemonState;

    // The presence state machine is daemon-AGNOSTIC: every assertion runs
    // against TWO synthetic sources to PROVE a 2nd daemon needs zero new
    // state-machine code (the multi-daemon directive's structural guarantee).
    const SOURCES: [&str; 2] = ["openclaw", "daemon2"];

    /// Pin the DEFAULT decay profile's literal values. Every timing test here
    /// correctly derives its offsets FROM the profile (`ttl.presence_ttl_ms +
    /// 1`), so mutating `5 * 60 * 1_000` also mutates each test's own
    /// expectation — a direct pin is the only guard on the literals (the
    /// reducer's stale-timeout pin, same rationale). The values ARE the
    /// product decision; change deliberately, never to make this pass.
    #[test]
    fn default_presence_profile_has_its_intended_durations() {
        assert_eq!(PresenceTtl::DEFAULT.busy_decay_ms, 30_000); // 30 s
        assert_eq!(PresenceTtl::DEFAULT.presence_ttl_ms, 300_000); // 5 min
        assert_eq!(PresenceTtl::DEFAULT.down_remove_ms, 5_000); // 5 s
    }

    fn ms(m: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(m)
    }

    /// The canonical SINGLE instance every pre-existing state-machine test runs
    /// against — those tests are about the state machine, not identity, so they
    /// keep asserting on one mascot. The multi-INSTANCE guarantees have their own
    /// suite below (`inst`/`apply_at`).
    fn ikey(src: &str) -> DaemonInstanceKey {
        DaemonInstanceKey::new(src, inst("1"))
    }
    fn inst(id: &str) -> DaemonInstanceId {
        DaemonInstanceId::new(id).expect("non-empty test instance id")
    }
    fn apply(s: &mut SceneState, src: &str, u: DaemonPresenceUpdate, at: SystemTime) {
        apply_presence(s, &ikey(src), u, at);
    }
    /// Apply to an EXACT (source, instance) — the multi-instance suite's entry.
    fn apply_at(s: &mut SceneState, src: &str, id: &str, u: DaemonPresenceUpdate, at: u64) {
        apply_presence(s, &DaemonInstanceKey::new(src, inst(id)), u, ms(at));
    }
    fn p_opt<'a>(s: &'a SceneState, src: &str) -> Option<&'a DaemonPresence> {
        s.daemon(src, ikey(src).instance())
    }
    fn p<'a>(s: &'a SceneState, src: &str) -> &'a DaemonPresence {
        p_opt(s, src).expect("presence entry")
    }
    fn st_at(s: &SceneState, src: &str, id: &str) -> Option<DaemonState> {
        s.daemon(src, &inst(id)).map(|p| p.display_state())
    }
    fn st(s: &SceneState, src: &str) -> DaemonState {
        p(s, src).display_state()
    }
    fn sessions(s: &SceneState, src: &str) -> u32 {
        p(s, src).active_sessions
    }
    fn entered_at(s: &SceneState, src: &str) -> SystemTime {
        p(s, src).entered_at
    }
    fn last_seen(s: &SceneState, src: &str) -> SystemTime {
        p(s, src).last_seen
    }
    fn up(s: &mut SceneState, src: &str, pid: i32, at: u64) {
        apply(
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
            assert_eq!(p(&s, src).current_pid, Some(4242));
        }
    }

    #[test]
    fn gateway_up_resets_sessions_and_in_flight_runs() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(0));
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
            apply(
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
            assert!(p(&s, src).in_flight_runs.is_empty());
        }
    }

    #[test]
    fn gateway_down_sets_down() {
        for src in SOURCES {
            let mut s = SceneState::default();
            up(&mut s, src, 1, 0);
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(1));
            assert_eq!(st(&s, src), DaemonState::Down);
        }
    }

    #[test]
    fn pid_exited_never_materializes_a_daemon() {
        // A `PidExited` is a DEATH signal (synthesized by the exit-watch drain),
        // NOT proof of life. For a daemon that was never seen — or was already
        // TTL-removed (the exit watch races the removal sweep) — the death must be
        // a NO-OP, never mint a fresh UP entry: a fresh entry has `current_pid ==
        // None`, so the arm's `current_pid == Some(pid)` guard fails and the entry
        // is left UP, rendering a phantom live idle mascot for a dead gateway.
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::PidExited { pid: 100 },
                ms(1),
            );
            assert!(
                p_opt(&s, src).is_none(),
                "PidExited on an absent daemon must not create an entry"
            );
        }
    }

    #[test]
    fn session_count_increments_and_saturates_at_zero() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(0));
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
            assert_eq!(sessions(&s, src), 2);
            for i in 0..3 {
                apply(&mut s, src, DaemonPresenceUpdate::SessionEnded, ms(2 + i));
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "a".into(),
                },
                ms(0),
            );
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "b".into(),
                },
                ms(1),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunEnded {
                    run_key: "a".into(),
                },
                ms(2),
            );
            assert_eq!(st(&s, src), DaemonState::Busy, "b still in flight");
            apply(
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            apply(
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
                p(&s, src).in_flight_runs.is_empty(),
                "the failed run leaves the in-flight set"
            );
        }
    }

    #[test]
    fn a_new_run_clears_degraded_back_to_busy() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "a".into(),
                },
                ms(0),
            );
            assert_eq!(st(&s, src), DaemonState::Degraded);
            apply(
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunFailed {
                    run_key: "a".into(),
                },
                ms(0),
            );
            // The next attempt enters flight, then SUCCEEDS.
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "b".into(),
                },
                ms(1),
            );
            apply(
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
            apply(
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::PidSeen { pid: 555 },
                ms(0),
            );
            assert_eq!(
                p(&s, src).current_pid,
                Some(555),
                "the live pid is adopted so the instant abrupt-down rung can arm"
            );
            // And it does NOT change the state (pure pid adoption).
            assert_eq!(st(&s, src), DaemonState::Idle);
            // The adopted pid dying now takes the daemon down (the #318 payoff).
            apply(
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::PidSeen { pid: 999 },
                ms(1),
            );
            assert_eq!(
                p(&s, src).current_pid,
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
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply(&mut s, src, DaemonPresenceUpdate::PidSeen { pid: 7 }, ms(1));
            assert_eq!(
                st(&s, src),
                DaemonState::Down,
                "PidSeen is pure pid adoption — it does NOT resurrect by itself"
            );
            assert_eq!(p(&s, src).current_pid, Some(7));
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
            apply(&mut s, src, PidExited { pid: 100 }, ms(1)); // P1 dies → Down
            assert_eq!(st(&s, src), DaemonState::Down);
            // Reconnect as P2; gateway_start missed → only a normal event + PidSeen.
            apply(&mut s, src, PidSeen { pid: 200 }, ms(2));
            apply(&mut s, src, SessionStarted, ms(3)); // any event ⇒ up
            assert_eq!(
                p(&s, src).current_pid,
                Some(200),
                "PidSeen must re-adopt the live pid after a Down"
            );
            // P2 dying now takes the daemon down instantly (the rung re-armed).
            apply(&mut s, src, PidExited { pid: 200 }, ms(4));
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
            apply(
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
            apply(
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
            assert_eq!(p(&s, src).current_pid, Some(2));
        }
    }

    #[test]
    fn any_event_resurrects_from_down() {
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(1));
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
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            assert_eq!(st(&s, src), DaemonState::Down);
            apply(&mut s, src, DaemonPresenceUpdate::SessionEnded, ms(1));
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
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(2000),
            );
            apply(
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
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(4000));
            apply(&mut s, src, DaemonPresenceUpdate::SessionStarted, ms(9000));
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
        assert_eq!(s.daemons().count(), 1);
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
            apply(&mut s, src, DaemonPresenceUpdate::GatewayDown, ms(0));
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.down_remove_ms - 1));
            assert!(p_opt(&s, src).is_some(), "still present mid walk-out");
            sweep_presence_ttl(&mut s, src, ttl, ms(ttl.down_remove_ms + 1));
            assert!(
                p_opt(&s, src).is_none(),
                "removed once the walk-out window elapsed"
            );
        }
    }

    #[test]
    fn sweep_self_heals_a_stranded_busy_after_the_grace_window() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
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
            assert!(p(&s, src).in_flight_runs.is_empty());
        }
    }

    #[test]
    fn a_clock_regression_keeps_an_in_flight_lease() {
        // `duration_since` FAILS when the run's stamp is in the future (a wall-clock
        // step back — NTP, a suspend/resume). The lease is then KEPT
        // (`unwrap_or(true)`, the repo-wide "a regression is not evidence of age"
        // policy): with `false` a single backwards step would expire EVERY run at
        // once and drop a live busy gateway to Idle.
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
                &mut s,
                src,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                ms(10_000),
            );
            assert_eq!(st(&s, src), DaemonState::Busy);
            // Sweep with `now` BEFORE the run's own stamp.
            sweep_presence_ttl(&mut s, src, ttl, ms(0));
            assert_eq!(
                st(&s, src),
                DaemonState::Busy,
                "a backwards clock must not expire a live run"
            );
        }
    }

    #[test]
    fn sweep_does_not_busy_decay_a_degraded_daemon_but_ttl_takes_it_down() {
        // #317: a broken gateway must not silently "heal" on a dropped event.
        // `degraded` is its OWN liveness axis, so expiring run leases (the only
        // thing the decay arm does since #460 — Busy is projected, not stored)
        // cannot clear it; only a real RunEnded/RunStarted/GatewayUp does. It does
        // still go Down on the presence_ttl silence (a SIGTERM'd broken gateway).
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
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
        assert_eq!(s.daemons().count(), 0);
        sweep_presence_ttl(&mut s, "never-seen", ttl, ms(ttl.presence_ttl_ms + 1));
        assert_eq!(
            s.daemons().count(),
            0,
            "sweeping an unknown source mints no phantom entry"
        );
        assert!(p_opt(&s, "never-seen").is_none());
    }

    #[test]
    fn sweep_within_the_grace_window_keeps_busy() {
        let ttl = PresenceTtl::DEFAULT;
        for src in SOURCES {
            let mut s = SceneState::default();
            apply(
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
        apply(
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
        apply(&mut s, "openclaw", DaemonPresenceUpdate::GatewayDown, ms(2));
        assert_eq!(st(&s, "openclaw"), DaemonState::Down);
        assert_eq!(
            st(&s, "daemon2"),
            DaemonState::Busy,
            "daemon2 unaffected by openclaw down"
        );
        assert_eq!(s.daemons().count(), 2);
    }

    // ---- multi-INSTANCE of ONE source (two OpenClaw gateways) ----
    //
    // The sibling of the two-SOURCE block above, and the reason
    // `DaemonInstanceId` exists: OpenClaw officially supports several isolated
    // gateways on one host, so `daemons` must key on (source, instance) — every
    // assertion below fails against a source-only key.

    const A: &str = "18789";
    const B: &str = "19789";

    #[test]
    fn two_gateway_instances_of_one_source_hold_independent_state() {
        let src = "openclaw";
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            0,
        );
        apply_at(
            &mut s,
            src,
            B,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            0,
        );
        assert_eq!(s.daemons().count(), 2, "two ports ⇒ two mascots");
        // A goes busy; B must NOT.
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r1".into(),
            },
            1,
        );
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Busy));
        assert_eq!(
            st_at(&s, src, B),
            Some(DaemonState::Idle),
            "gateway A's run must not make gateway B busy"
        );
        // A's clean stop must not touch B.
        apply_at(&mut s, src, A, DaemonPresenceUpdate::GatewayDown, 2);
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Down));
        assert_eq!(
            st_at(&s, src, B),
            Some(DaemonState::Idle),
            "gateway A's stop must not take gateway B down"
        );
    }

    #[test]
    fn restart_on_the_same_port_reuses_the_mascot_and_rebinds_its_process() {
        // The stable-identity payoff: a gateway restarting on its port keeps ONE
        // mascot (no second lobster, no walk-out/walk-in churn) while its PROCESS
        // incarnation rebinds — the two-concept split the port key buys.
        let src = "openclaw";
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: Some(100) },
            0,
        );
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: Some(200) },
            10,
        );
        assert_eq!(s.daemons().count(), 1, "a restart is the SAME instance");
        assert_eq!(
            s.daemon(src, &inst(A)).and_then(|p| p.current_pid),
            Some(200)
        );
    }

    #[test]
    fn a_stale_exit_receipt_for_the_replaced_process_is_a_no_op() {
        // The exit watch may deliver the OLD pid's death after the replacement's
        // `gateway_start` rebound `current_pid`. That receipt must not kill the
        // live gateway — the `current_pid == Some(pid)` guard is the generation
        // check (and it needs no start-marker: the arm is rebound synchronously by
        // the restart, so only the identical pid could match).
        let src = "openclaw";
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: Some(100) },
            0,
        );
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: Some(200) },
            10,
        );
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::PidExited { pid: 100 },
            11,
        );
        assert_eq!(
            st_at(&s, src, A),
            Some(DaemonState::Idle),
            "the old generation's exit must not down the replacement"
        );
        // The CURRENT generation's exit does take it down — and only it.
        apply_at(
            &mut s,
            src,
            B,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            11,
        );
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::PidExited { pid: 200 },
            12,
        );
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Down));
        assert_eq!(
            st_at(&s, src, B),
            Some(DaemonState::Idle),
            "one gateway's process exit is instance-local"
        );
    }

    #[test]
    fn exit_receipt_for_an_unseen_instance_creates_nothing() {
        // `PidExited` is a DEATH signal, never proof of life — the non-creating
        // rule holds per INSTANCE too (a receipt for a port we never observed must
        // not mint a phantom live mascot).
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            "openclaw",
            A,
            DaemonPresenceUpdate::PidExited { pid: 7 },
            0,
        );
        assert_eq!(s.daemons().count(), 0);
    }

    #[test]
    fn ttl_decay_is_instance_local() {
        // Traffic on gateway B must not renew gateway A's presence clock: each
        // instance decays on its OWN `last_seen`. Against a source-only key this
        // is the bug where a busy sibling keeps a dead gateway's lobster alive.
        let src = "openclaw";
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            0,
        );
        let late = ttl.presence_ttl_ms + 1;
        apply_at(
            &mut s,
            src,
            B,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            late,
        );
        sweep_presence_ttl(&mut s, src, ttl, ms(late));
        assert_eq!(
            st_at(&s, src, A),
            Some(DaemonState::Down),
            "the silent gateway expires on its own clock"
        );
        assert_eq!(
            st_at(&s, src, B),
            Some(DaemonState::Idle),
            "the fresh sibling is untouched by its neighbour's expiry"
        );
        // Only the expired instance is REMOVED once its walk-out window elapses.
        sweep_presence_ttl(&mut s, src, ttl, ms(late + ttl.down_remove_ms));
        assert!(st_at(&s, src, A).is_none(), "the Down instance is pruned");
        assert_eq!(st_at(&s, src, B), Some(DaemonState::Idle));
    }

    #[test]
    fn busy_decay_is_instance_local() {
        let src = "openclaw";
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        for id in [A, B] {
            apply_at(
                &mut s,
                src,
                id,
                DaemonPresenceUpdate::RunStarted {
                    run_key: "r".into(),
                },
                0,
            );
        }
        // B keeps working; A's `agent_end` was dropped.
        let t = ttl.busy_decay_ms;
        apply_at(
            &mut s,
            src,
            B,
            DaemonPresenceUpdate::RunStarted {
                run_key: "r2".into(),
            },
            t,
        );
        sweep_presence_ttl(&mut s, src, ttl, ms(t));
        assert_eq!(
            st_at(&s, src, A),
            Some(DaemonState::Idle),
            "A's stranded run decays on A's own clock"
        );
        assert_eq!(
            st_at(&s, src, B),
            Some(DaemonState::Busy),
            "B is still genuinely busy"
        );
    }

    #[test]
    fn removing_the_last_instance_leaves_no_husk_source_entry() {
        // The nested roster's one hazard: a source level with ZERO instances would
        // be a third state ("configured but nothing running") that no consumer
        // models — `daemons()` flattens, `gateway_rollup`'s `None` means ABSENT, and
        // the floating tick gate's `.all()` is vacuously true on an empty set. The
        // sweep therefore PRUNES an emptied source, making that state unreachable.
        let src = "openclaw";
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        apply_at(&mut s, src, A, DaemonPresenceUpdate::GatewayDown, 0);
        assert_eq!(s.daemons().count(), 1);
        sweep_presence_ttl(&mut s, src, ttl, ms(ttl.down_remove_ms));
        assert_eq!(s.daemons().count(), 0, "the instance is gone");
        // …and so is the source level: a fresh event must re-create it cleanly, and
        // nothing may iterate an empty husk.
        assert!(
            s.daemon(src, &inst(A)).is_none() && st_at(&s, src, A).is_none(),
            "no husk source entry survives its last instance"
        );
        // Every accessor FLATTENS, so an empty source level is invisible through
        // them — the serialized shape is the only place the prune is observable, and
        // without this assertion disabling it passes the whole suite.
        assert_eq!(
            serde_json::to_string(&s.daemons).expect("the roster serializes"),
            "{}",
            "the emptied source level must be GONE, not an empty husk map"
        );
        apply_at(
            &mut s,
            src,
            B,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            1,
        );
        assert_eq!(s.daemons().count(), 1, "a later gateway re-creates cleanly");
    }

    #[test]
    fn a_stranded_run_expires_while_the_gateway_keeps_serving() {
        // The busy LATCH (the "self-healing, never a latch" claim only held for a
        // SILENT gateway): a dropped `agent_end` strands a run key, and the decay
        // used the daemon-wide `last_seen` — refreshed by ANY event — so on a
        // gateway that keeps serving other traffic the mascot shuttled Busy for the
        // gateway's whole life. Each run now ages on ITS OWN observation.
        let src = "openclaw";
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::RunStarted {
                run_key: "stranded".into(),
            },
            0,
        );
        // The gateway stays chatty right up to the decay boundary: sessions come and
        // go, so `last_seen` is always fresh and the OLD sweep never fired.
        let mut t = 0;
        while t < ttl.busy_decay_ms {
            t += ttl.busy_decay_ms / 4;
            apply_at(&mut s, src, A, DaemonPresenceUpdate::SessionStarted, t);
            sweep_presence_ttl(&mut s, src, ttl, ms(t));
        }
        assert_eq!(
            st_at(&s, src, A),
            Some(DaemonState::Idle),
            "the stranded run must expire on its own clock, not the daemon's"
        );
        // A run that IS still being observed is untouched by the same sweep.
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::RunStarted {
                run_key: "live".into(),
            },
            t,
        );
        sweep_presence_ttl(&mut s, src, ttl, ms(t + ttl.busy_decay_ms - 1));
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Busy));
    }

    #[test]
    fn expiring_a_stranded_run_never_heals_a_degraded_gateway() {
        // Degraded is a SEPARATE axis: only a real RunEnded/RunStarted/GatewayUp
        // clears it. The old decay was gated on `is_busy()` (false while Degraded)
        // partly for this; the per-run expiry drops that gate, so pin that the
        // gateway still reads Degraded after its stale lease lapses.
        let src = "openclaw";
        let ttl = PresenceTtl::DEFAULT;
        let mut s = SceneState::default();
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::RunStarted {
                run_key: "stranded".into(),
            },
            0,
        );
        apply_at(
            &mut s,
            src,
            A,
            DaemonPresenceUpdate::RunFailed {
                run_key: "other".into(),
            },
            1,
        );
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Degraded));
        sweep_presence_ttl(&mut s, src, ttl, ms(ttl.busy_decay_ms + 1));
        assert_eq!(
            st_at(&s, src, A),
            Some(DaemonState::Degraded),
            "a lapsed run lease must not silently heal a broken backend"
        );
    }

    #[test]
    fn source_wide_disconnect_walks_out_every_instance() {
        // Disconnect is a SOURCE-level user action (one Sources-panel row), so it
        // must reach every gateway of that source — not just one.
        let src = "openclaw";
        let mut s = SceneState::default();
        for id in [A, B] {
            apply_at(
                &mut s,
                src,
                id,
                DaemonPresenceUpdate::GatewayUp { pid: None },
                0,
            );
        }
        apply_at(
            &mut s,
            "daemon2",
            A,
            DaemonPresenceUpdate::GatewayUp { pid: None },
            0,
        );
        mark_presence_down(&mut s, src, ms(5));
        assert_eq!(st_at(&s, src, A), Some(DaemonState::Down));
        assert_eq!(st_at(&s, src, B), Some(DaemonState::Down));
        assert_eq!(
            st_at(&s, "daemon2", A),
            Some(DaemonState::Idle),
            "a different source's daemon keeps running"
        );
    }
}
