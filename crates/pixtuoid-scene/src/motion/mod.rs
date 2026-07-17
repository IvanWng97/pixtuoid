//! Per-agent walk-timing state owned by each `FloorCtx` in this crate.
//!
//! `MotionState` is the single source of truth for in-flight walk profiles
//! (entry, exit, snap-back, and wander phases). It is keyed on `AgentId`
//! inside `FloorCtx::motion` and evicted when the agent leaves the scene.
//!
//! `octile_path_len` converts an A*-routed `&[Point]` slice into the same
//! octile distance metric the router uses, delegating to the already-
//! promoted `pose::octile_distance`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::physics::{walk_arrived, walk_profile, WalkIntent, WalkProfile};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::layout::{Layout, Point, WaypointKind};
use crate::pathfind::Router;
use crate::pose::{desk_leg_endpoint, octile_distance, route_jittered};
use crate::pose::{
    dwell_ms, est_wander_cycle_ms, seated_dwell_ms, stale_resume_gap_ms, takes_trip, SpotClaims,
    WANDER_DWELL_EST_MS,
};

/// Frozen A* polyline for one in-flight walk leg.
///
/// Snapshotted the first frame a walk leg's `(from, to)` endpoints appear and
/// reused unchanged for the rest of the leg. Per-frame occupancy-overlay churn
/// (e.g. another agent toggling a waypoint obstacle) invalidates the A* path
/// cache and would otherwise re-route a walker onto a differently-shaped
/// polyline mid-stride — mapping the frozen-profile progress `t` onto a new
/// shape makes the sprite visibly jump (the "flash"/teleport). Freezing the
/// shape makes the walk smooth; the trade is that a walker no longer dodges
/// agents that step into its path mid-leg (rare, cosmetic, legs are seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkPathSnapshot {
    pub from: Point,
    pub to: Point,
    pub path: Vec<Point>,
}

/// Phase the wander cycle is currently in for a given agent. The three walk
/// phases CARRY their frozen `WalkProfile` so the type makes "in a walk leg
/// with no profile" unrepresentable — the old `WanderState.profile: Option`
/// + its "should be unreachable" recovery path are gone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WanderPhase {
    /// Sitting at the desk between trips.
    Seated,
    /// Walking from desk to the chosen waypoint, on this frozen out-leg profile.
    WalkingOut(WalkProfile),
    /// Standing/sitting at the waypoint during the dwell beat, holding the
    /// pre-snapshotted return-leg profile (computed at out-leg arrival).
    AtWaypoint(WalkProfile),
    /// Walking from the waypoint back to the desk, on the return-leg profile.
    WalkingBack(WalkProfile),
}

/// A one-shot walk leg (exit / snap-back): the wall-clock instant the leg
/// armed, its frozen physics profile, and the FROZEN origin recorded at
/// arm-time (reused every frame so the leg doesn't drift). Names the fields
/// of what was a `(SystemTime, WalkProfile, Point)` tuple.
#[derive(Debug, Clone)]
pub struct WalkLeg {
    pub started_at: SystemTime,
    pub profile: WalkProfile,
    pub from: Point,
}

/// A resolved wander destination: the walkable target cell plus WHAT it is (a
/// named lounge waypoint — with an optional seat to settle onto — or an aimless
/// amble). `dest` stays outside the enum because it is always present; only the
/// waypoint/seat metadata is variant-specific. Produced by
/// [`crate::pose::resolve_wander_target`] (the ONE stateless resolver both the
/// motion authority and `idle_pose` share).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WanderTarget {
    /// Destination pixel of the current trip (the walkable approach/amble cell).
    pub dest: Point,
    pub kind: WanderKind,
}

/// What KIND of wander destination [`WanderTarget::dest`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WanderKind {
    /// A named lounge waypoint: its `layout.waypoints` index + kind, plus the
    /// seat foot cell `S` when it's an occupied seat. `seat = Some` ⇒ the walk
    /// SETTLES from the approach point `dest` onto `S` (and rises from `S` on the
    /// way back) so arrival/departure don't pop; `seat = None` for obstacles
    /// (the agent stands AT `dest`).
    Named {
        wp_idx: usize,
        kind: WaypointKind,
        seat: Option<Point>,
    },
    /// An aimless amble to a random walkable point (no named waypoint, no seat).
    Aimless,
}

impl WanderKind {
    /// The seat foot cell to settle onto, when this destination is an occupied
    /// seat — `None` for obstacles (`Named { seat: None }`) and aimless ambles.
    pub(crate) fn seat(self) -> Option<Point> {
        match self {
            WanderKind::Named { seat, .. } => seat,
            WanderKind::Aimless => None,
        }
    }
}

/// The elastic cyclic-wander timeline state machine for one agent (desk → waypoint
/// → desk, repeating). The fields are one unit: `advance_wander` transitions them
/// together, and the whole thing is idempotent per `now` via `last_advanced_at`.
/// (Was nine flat `wander_*` / `last_advanced_at` fields on `MotionState`; the
/// trip destination `dest`/`dest_kind`/`dest_wp_idx`/`seat` are now one
/// [`WanderTarget`].)
#[derive(Debug, Clone)]
pub struct WanderState {
    /// Monotonically increasing wander cycle counter, incremented each time
    /// `WalkingBack` completes — selects the waypoint destination (mirrors
    /// `pose::pure`'s `cycle_n` derivation).
    pub cycle_n: u64,
    /// Current phase of the wander cycle.
    pub phase: WanderPhase,
    /// Wall-clock instant the current phase began (reset every transition, so each
    /// leg has its own clock). Sentinel `UNIX_EPOCH` ⇒ a fresh agent `advance_wander`
    /// bootstraps.
    pub phase_started_at: SystemTime,
    /// The current trip's resolved destination (dest cell + named/aimless kind +
    /// optional seat). Set on each new `WalkingOut`; its `kind` resets to
    /// `Aimless` when a cycle completes (`WalkingBack` cleanup).
    pub target: WanderTarget,
    /// Last `now` at which `advance_wander` performed a transition — idempotency:
    /// `now <= last_advanced_at` ⇒ a no-op on mutable state. Sentinel `UNIX_EPOCH`
    /// ⇒ never advanced.
    pub last_advanced_at: SystemTime,
}

/// Per-agent walk-timing state owned by each `FloorCtx` in this crate.
///
/// One `MotionState` exists per live agent (per floor). Fields are `Option`
/// so the struct can be default-initialised for new agents and populated
/// lazily on the first relevant walk-start frame.
#[derive(Debug, Clone)]
pub struct MotionState {
    pub agent_id: AgentId,

    // --- entry / exit / snap-back one-shot walks ---
    /// `(walk_started_at, profile)` snapshotted once at door-crossing.
    pub entry: Option<(SystemTime, WalkProfile)>,
    /// `(walk_started_at, profile, from)` snapshotted once when `exiting_at`
    /// fires. `from` is the agent's position at that moment — its current
    /// wander position if it was out, else the desk anchor — so the exit walk
    /// starts where the sprite actually is instead of teleporting to the desk.
    pub exit: Option<WalkLeg>,
    /// `(walk_started_at, profile, from)` for the state-transition snap-back
    /// walk (replaces the old `since_state < SNAP_BACK_MS` guard). `from` is
    /// the FROZEN walk origin — the position recorded when the leg armed —
    /// reused every frame so the walk doesn't drift toward the desk (mirrors
    /// `exit`).
    pub snap_back: Option<WalkLeg>,

    /// The elastic cyclic-wander timeline state machine — the values move as a
    /// unit (`advance_wander` transitions them together, idempotent per `now`
    /// via `wander.last_advanced_at`). See [`WanderState`]. (Was nine flat
    /// `wander_*` / `last_advanced_at` fields.)
    pub wander: WanderState,

    /// Frozen A* polyline for the current walk leg (entry/exit/wander/snap-back).
    /// `None` while not walking. Re-snapshotted when the leg's `(from, to)`
    /// endpoints change. See [`WalkPathSnapshot`].
    pub walk_path: Option<WalkPathSnapshot>,
}

impl MotionState {
    /// Construct a fresh `MotionState` for `agent_id`.
    ///
    /// All optional fields are `None`; wander starts in `Seated` phase with
    /// both `wander.phase_started_at` and `wander.last_advanced_at` set to
    /// `SystemTime::UNIX_EPOCH` so `advance_wander` can detect a bootstrap
    /// agent on the first call via the epoch sentinel.
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            entry: None,
            exit: None,
            snap_back: None,
            wander: WanderState {
                cycle_n: 0,
                phase: WanderPhase::Seated,
                phase_started_at: SystemTime::UNIX_EPOCH,
                // Placeholder — replaced on first WalkingOut transition.
                target: WanderTarget {
                    dest: Point { x: 0, y: 0 },
                    kind: WanderKind::Aimless,
                },
                last_advanced_at: SystemTime::UNIX_EPOCH,
            },
            walk_path: None,
        }
    }
}

/// Advance the wander state machine by one frame for the given idle agent.
///
/// # Idempotency (Correction F)
/// Phase transitions (re-anchor `wander.phase_started_at`, increment
/// `wander.cycle_n`, snapshot a new leg profile) are performed ONLY when
/// `now > wander.last_advanced_at`. When `now <= wander.last_advanced_at` the function
/// computes the pose from the existing phase state WITHOUT mutating any
/// wander fields — safe to call 2+ times per frame (seated-overlay pass +
/// character loop + `character_anchor`).
///
/// # Bootstrap catch-up (Correction M)
/// On first call for a fresh Idle slot (detected via epoch sentinel on
/// `wander.phase_started_at`), `cycle_n` is fast-forwarded by integer
/// division so destination selection is consistent with what core's
/// stateless `idle_pose` would have derived for an agent that was Idle
/// before the first render.
///
/// Returns `(phase, t_x1000)` where `t_x1000` is meaningful only in
/// the `WalkingOut` / `WalkingBack` phases (0–1000 physics progress).
pub fn advance_wander(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    motion: &mut HashMap<AgentId, MotionState>,
) -> (WanderPhase, u16) {
    let id = slot.agent_id;
    // Snapshot the other agents' seat claims BEFORE taking this agent's `&mut`
    // (the two borrows of `motion` can't overlap). Cheap: one pass over the
    // floor's motion map, and the Vec stays empty unless someone is actually out
    // on a seat trip.
    let claimed = spot_claims(motion, id);
    let ms = motion.entry(id).or_insert_with(|| MotionState::new(id));

    // ---- INIT / BOOTSTRAP --------------------------------------------------
    // A fresh MotionState has `wander.phase_started_at == UNIX_EPOCH`, which
    // is guaranteed to be less than any real `state_started_at`. We also
    // re-seed when the slot (re-)entered Idle after a different state (the
    // stored phase_started predates state_started_at by more than 1 ms).
    let is_fresh = ms
        .wander
        .phase_started_at
        .checked_add(Duration::from_millis(1))
        .map(|t| t <= slot.state_started_at)
        .unwrap_or(true);

    // Stale resume: this agent was advanced before (non-epoch last_advanced_at)
    // but more than a full wander cycle has elapsed since — its floor was
    // off-screen (only the current floor renders each frame) or `now` was
    // frozen (pause). Treat it like a fresh agent so the bootstrap fast-forward
    // below snaps it to the correct cycle analytically (O(1), no per-leg
    // routing) instead of the phase machine replaying the whole backlog one
    // transition per frame — the visible "fast-forward all the movement in a
    // second" bug. The trigger (`stale_resume_gap_ms`, 7–13 s) is a frame-cadence vs
    // frozen-floor detector, NOT a dwell detector: on-screen, `advance_wander`
    // runs every frame even DURING a 40 s lounge dwell, so `last_advanced_at`
    // updates each ~33 ms and the gap never approaches 7 s — only an off-screen
    // floor or a pause (frozen `now`) lets the gap exceed it. (Don't raise this
    // to "max dwell" — that would let 13–60 s off-screen gaps replay.)
    // `unwrap_or(false)`: `duration_since` only errs if `now < last_advanced_at`
    // (clock stepped backward — NTP/suspend). The per-frame render clock is
    // monotone so this is unreachable in practice; treating a backward step as
    // "not stale" avoids snapping every agent to Seated on a tiny clock adjust.
    let is_stale_resume = ms.wander.last_advanced_at != SystemTime::UNIX_EPOCH
        && now
            .duration_since(ms.wander.last_advanced_at)
            .map(|d| d.as_millis() as u64 > stale_resume_gap_ms(id))
            .unwrap_or(false);

    if is_fresh || is_stale_resume {
        let elapsed_idle = crate::anim::elapsed_ms(now, slot.state_started_at);
        // Use the estimated full cycle (matches idle_pose) so the bootstrapped
        // cycle_n agrees with what the stateless overlay derived for the same
        // long-idle agent — NOT stale_resume_gap_ms (the stale-resume sentinel).
        let cycle = est_wander_cycle_ms(id);

        // Fast-forward `cycle_n` by integer division so destination selection
        // matches what an agent idle this long would have reached (0 when idle
        // < one cycle), but ALWAYS (re)start the phase clock cleanly in Seated
        // at `now`. Anchoring mid-cycle (`now - partial_ms`) made the phase
        // machine rush through the partial cycle's already-expired legs one
        // transition per frame on the first few frames — a desk↔waypoint
        // teleport. The agent was unobserved before this frame, so starting
        // fresh-Seated is equally valid and leaves no dangling walk profile.
        ms.wander.phase = WanderPhase::Seated;
        ms.wander.cycle_n = elapsed_idle / cycle;
        ms.wander.phase_started_at = now;
    }

    // ---- IDEMPOTENCY CHECK (Correction F) ----------------------------------
    // Transitions mutate wander state; we must only do them once per unique `now`.
    let may_transition = now > ms.wander.last_advanced_at;

    // ---- PHASE MACHINE -----------------------------------------------------
    let elapsed_phase = crate::anim::elapsed_ms(now, ms.wander.phase_started_at);

    // Absolute per-spot timeline (the render authority). Seated-at-desk beat is
    // a long, per-agent dwell; the at-waypoint beat is keyed on the spot kind so
    // a sofa lounges far longer than a vending grab. Aimless trips (no named
    // kind) fall back to the average dwell estimate.
    let seated_dur = seated_dwell_ms(id);
    let dwell_dur = match ms.wander.target.kind {
        WanderKind::Named { kind, .. } => dwell_ms(kind, id),
        WanderKind::Aimless => WANDER_DWELL_EST_MS,
    };

    let result = match ms.wander.phase {
        WanderPhase::Seated => {
            if may_transition && elapsed_phase >= seated_dur {
                // Check whether this cycle is a trip.
                if !takes_trip(id, ms.wander.cycle_n) || layout.waypoints.is_empty() {
                    // Non-trip: skip forward one cycle in Seated.
                    ms.wander.cycle_n += 1;
                    ms.wander.phase_started_at = ms
                        .wander
                        .phase_started_at
                        .checked_add(Duration::from_millis(seated_dur))
                        .unwrap_or(now);
                } else {
                    // Trip: pick destination, snapshot walk-out profile.
                    // Resolve the stand cell off the agent's home desk (the
                    // origin must match core::idle_pose's `desk` so the
                    // stateless/stateful destinations stay in lockstep).
                    let desk_pt = layout.home_desk(slot.desk_index.single_floor_local());
                    let origin = desk_pt.unwrap_or(Point { x: 0, y: 0 });
                    let target = pick_wander_dest(id, ms.wander.cycle_n, layout, origin, &claimed);
                    ms.wander.target = target;
                    let dest = target.dest;
                    let seat = target.kind.seat();

                    let desk = desk_pt.unwrap_or(dest);
                    // Leave via the desk approach cell (rise off the chair),
                    // mirroring pose's WalkingOut leg. The profile duration must
                    // cover the FULL polyline: chair-glide + route + seat settle —
                    // else t reaches 1000 before the sprite arrives and it pops.
                    // Routed via the SAME jittered goal the render's walk-path
                    // freeze uses (route_jittered), so the measured length and
                    // the router-cache key match the rendered leg.
                    let (from, chair_settle) = desk_leg_endpoint(desk, layout);
                    let path = route_jittered(router, &layout.walkable, overlay, id, from, dest);
                    // Rise off the desk chair (start), glide onto the waypoint seat (end).
                    let len = measured_leg_len(&path, chair_settle, seat);
                    ms.wander.phase =
                        WanderPhase::WalkingOut(walk_profile(len, WalkIntent::WanderOut, id));
                    ms.wander.phase_started_at = ms
                        .wander
                        .phase_started_at
                        .checked_add(Duration::from_millis(seated_dur))
                        .unwrap_or(now);
                }
            }
            (ms.wander.phase, 0)
        }

        WanderPhase::WalkingOut(profile) => {
            match poll_walk_leg(&profile, elapsed_phase, may_transition) {
                WalkLegStatus::InFlight(t) => (WanderPhase::WalkingOut(profile), t),
                WalkLegStatus::Arrived {
                    t_x1000,
                    walk_total,
                } => {
                    // Divergent on-arrival: snapshot the walk-back profile (the
                    // overlay may differ now) into the AtWaypoint variant, which
                    // carries it to the WalkingBack transition. `t_x1000` is 1000
                    // at arrival (the walk-out leg's terminal progress) —
                    // preserves the old hardcoded `1000` return byte-for-byte.
                    let back = snapshot_back_profile(slot, ms, layout, router, overlay);
                    ms.wander.phase = WanderPhase::AtWaypoint(back);
                    advance_phase_clock(ms, walk_total, now);
                    (WanderPhase::AtWaypoint(back), t_x1000)
                }
            }
        }

        WanderPhase::AtWaypoint(back) => {
            if may_transition && elapsed_phase >= dwell_dur {
                // The return-leg profile rode the AtWaypoint variant from the
                // out-leg's arrival — carry it straight into WalkingBack.
                ms.wander.phase = WanderPhase::WalkingBack(back);
                ms.wander.phase_started_at = ms
                    .wander
                    .phase_started_at
                    .checked_add(Duration::from_millis(dwell_dur))
                    .unwrap_or(now);
            }
            (ms.wander.phase, 0)
        }

        WanderPhase::WalkingBack(profile) => {
            match poll_walk_leg(&profile, elapsed_phase, may_transition) {
                WalkLegStatus::InFlight(t) => (WanderPhase::WalkingBack(profile), t),
                WalkLegStatus::Arrived { walk_total, .. } => {
                    // Divergent on-arrival: a cycle completed — advance the cycle
                    // counter and clear the trip kind back to Aimless (drops the
                    // named waypoint + its seat; `target.dest` is left as-is — the
                    // Seated arm never reads it and the next WalkingOut overwrites
                    // it — matching the pre-`WanderTarget` fields, which reset
                    // kind/idx/seat but not `dest`).
                    ms.wander.cycle_n += 1;
                    ms.wander.target.kind = WanderKind::Aimless;
                    ms.wander.phase = WanderPhase::Seated;
                    advance_phase_clock(ms, walk_total, now);
                    (WanderPhase::Seated, 0)
                }
            }
        }
    };

    // Record that transitions have been applied for this `now` (idempotency).
    if may_transition {
        ms.wander.last_advanced_at = now;
    }

    result
}

/// Status of an in-flight wander walk leg (`WalkingOut` / `WalkingBack`) for the
/// current frame, the result of the scaffold those two arms share.
enum WalkLegStatus {
    /// Still walking: the physics progress `t_x1000` (0..1000).
    InFlight(u16),
    /// The walk (incl. its pause) has completed. `walk_total` = `duration_ms +
    /// pause_ms` for the shared phase-clock advance; `t_x1000` is the progress at
    /// arrival (1000), exposed for completeness.
    Arrived { t_x1000: u16, walk_total: u64 },
}

/// The scaffold the `WalkingOut` and `WalkingBack` arms share: compute physics
/// progress from the phase's frozen `profile` and classify the leg as in-flight
/// or arrived. The arms run their OWN divergent on-arrival cleanup (WalkingOut:
/// snapshot the back profile; WalkingBack: bump the cycle + reset
/// `wander.target.kind` to Aimless) — only the progress/arrival check is here.
/// The profile is the phase variant's payload, so the old "missing profile"
/// recovery is gone — the type guarantees it.
fn poll_walk_leg(profile: &WalkProfile, elapsed_phase: u64, may_transition: bool) -> WalkLegStatus {
    let t_x1000 = crate::physics::walk_progress(profile, elapsed_phase);
    if may_transition && walk_arrived(profile, elapsed_phase) {
        WalkLegStatus::Arrived {
            t_x1000,
            walk_total: profile.duration_ms + profile.pause_ms,
        }
    } else {
        WalkLegStatus::InFlight(t_x1000)
    }
}

/// Advance the phase clock by `walk_total` ms from its current anchor (so the
/// next phase starts exactly when this one's wall-time budget elapsed), falling
/// back to `now` if the add overflows. The shared clock-advance both wander walk
/// arms run after their divergent on-arrival cleanup.
fn advance_phase_clock(ms: &mut MotionState, walk_total: u64, now: SystemTime) {
    ms.wander.phase_started_at = ms
        .wander
        .phase_started_at
        .checked_add(Duration::from_millis(walk_total))
        .unwrap_or(now);
}

/// Pick the wander destination for a given agent and cycle — a thin delegate to
/// the ONE stateless resolver [`crate::pose::resolve_wander_target`], which
/// `pose::pure::idle_pose` also calls, so the routed motion path and the
/// stateless overlay can never drift to different destinations for the same
/// `(agent, cycle)` and equal claims. `origin` is the agent's home desk (the
/// stand-side tiebreaker), kept identical to `idle_pose`'s `desk`.
fn pick_wander_dest(
    id: AgentId,
    cycle_n: u64,
    layout: &Layout,
    origin: Point,
    claimed: &SpotClaims,
) -> WanderTarget {
    crate::pose::resolve_wander_target(id, cycle_n, layout, origin, claimed)
}

/// The exclusive-spot waypoints every OTHER agent on this floor is currently out
/// on a trip to — the exclusion set that keeps a single-occupancy spot to one
/// occupant (see [`SpotClaims`]). Read from the live wander targets in `motion`,
/// which is why it must be built BEFORE the caller takes its own `&mut
/// MotionState`.
///
/// Two gates, both load-bearing:
///  * **phase ≠ Seated** — a Seated agent is at its desk. `target.kind` is
///    normally reset to `Aimless` on the walk-back's arrival, but the
///    bootstrap / stale-resume path re-seats an agent WITHOUT touching its
///    target, so the phase (not the kind) is the honest "is this agent actually
///    out at the spot" signal.
///  * **`exclusive`** — reuses the one authority for "single-occupancy
///    destination" rather than re-listing kinds, so seats AND the stand-beside
///    singles (phone booth, standing desk) are covered, and a future exclusive
///    kind inherits it. Shareable waypoints (pantry counter, vending, printer,
///    snack shelf) are NOT claimed: the painter's rank offset is a genuine
///    step-aside queue there.
///
/// An agent that goes Active mid-trip releases its claim in
/// `pose::derive_with_routing`, so a typing agent can't hold a spot it isn't at.
/// The other two holds are bounded and deliberate: an EXITING agent keeps its
/// spot for the ≤`EXIT_GRACE_WINDOW` walkout (it IS still there until it leaves)
/// until eviction drops the whole `MotionState`; and a `SeatedThinking` agent
/// keeps it for ≤`THINKING_WINDOW_SECS` — reachable without ever going Active,
/// since a stale `ActivityEnd` stamps `last_event_at` with no state change —
/// after which the stale-resume bootstrap re-seats the phase machine and the
/// `phase != Seated` gate drops the claim.
fn spot_claims(motion: &HashMap<AgentId, MotionState>, exclude: AgentId) -> SpotClaims {
    let mut claims = SpotClaims::default();
    for (id, ms) in motion {
        if *id == exclude || matches!(ms.wander.phase, WanderPhase::Seated) {
            continue;
        }
        if let WanderKind::Named { wp_idx, kind, .. } = ms.wander.target.kind {
            if crate::layout::furniture_def(kind.furniture()).exclusive {
                claims.claim(wp_idx);
            }
        }
    }
    claims
}

/// Snapshot the WanderBack `WalkProfile`: route `wander.target.dest → desk
/// approach cell`, add the seat-rise (`settle_len(target.dest, target.kind.seat())`)
/// and the chair-glide settle, then freeze a `WanderBack` profile over that full
/// polyline length (no pop on arrival).
///
/// Endpoint is the desk approach cell (matching `seated_anchor` via the
/// chair-glide) so there's no jump on arrival; this intentionally differs from
/// `core::idle_pose`'s raw `to: desk` (only the routed TUI path is
/// user-visible). Shared by the WalkingOut-arrival snapshot and the AtWaypoint
/// "shouldn't happen" fallback so the two can't drift.
fn snapshot_back_profile(
    slot: &AgentSlot,
    ms: &MotionState,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
) -> WalkProfile {
    let desk = layout
        .home_desk(slot.desk_index.single_floor_local())
        .unwrap_or(ms.wander.target.dest);
    // Arrive via the desk approach cell (glide onto the chair), mirroring pose's
    // WalkingBack leg; add the chair-glide so the profile covers the full
    // polyline (no pop on arrival). Routed via the SAME jittered goal the
    // render's walk-path freeze uses (route_jittered), so the measured length
    // and the router-cache key match the rendered leg.
    let (snap_to, chair_settle) = desk_leg_endpoint(desk, layout);
    let back_path = route_jittered(
        router,
        &layout.walkable,
        overlay,
        slot.agent_id,
        ms.wander.target.dest,
        snap_to,
    );
    // Rise off the waypoint seat (start), glide onto the desk chair (end).
    let back_len = measured_leg_len(&back_path, ms.wander.target.kind.seat(), chair_settle);
    walk_profile(back_len, WalkIntent::WanderBack, slot.agent_id)
}

/// Reuses `pose::octile_distance` (the same metric A* uses) so the
/// snapshotted path length is consistent with per-segment timing.
///
/// Returns 0 for a path with fewer than 2 points (no segments).
pub fn octile_path_len(path: &[Point]) -> u32 {
    if path.len() < 2 {
        return 0;
    }
    path.windows(2).map(|w| octile_distance(w[0], w[1])).sum()
}

/// Octile length of the settle segment `approach → seat`, or 0 when there is no
/// seat (obstacle/aimless). Added to a wander leg's profile length so its
/// DURATION covers the full walk including the short sit-down/stand-up settle.
pub(crate) fn settle_len(approach: Point, seat: Option<Point>) -> u32 {
    seat.map_or(0, |s| octile_distance(approach, s))
}

/// Rendered-polyline length of a walk leg: the octile length of the routed
/// polyline plus the short settle segments the router never plans (rise off the
/// `start_settle` seat at `route`'s FIRST point, glide onto the `end_settle`
/// seat at its LAST), floored at 1. The walk profile's DURATION is derived from
/// this so it covers the FULL rendered leg — chair-glide + route + seat settle —
/// and `t` can't reach 1000 before the sprite arrives (no pop). The ONE place
/// the ~5 hand-assembled "profile length == rendered polyline length" sites
/// agree; it takes the SAME start/end settle `Option`s `settle_from_pair` feeds
/// the render.
///
/// `route` is a [`route_jittered`](crate::pose::route_jittered) polyline, whose
/// first point is the leg source and last is the leg target by construction
/// (`find_path`'s `reconstruct` restores both raw endpoints, and `route_jittered`
/// re-pins the last to the true `to`) — so `settle_len(route.first, start_settle)`
/// measures the same rise the render prepends and `settle_len(route.last,
/// end_settle)` the same glide it appends.
pub(crate) fn measured_leg_len(
    route: &[Point],
    start_settle: Option<Point>,
    end_settle: Option<Point>,
) -> u32 {
    let start = route.first().map_or(0, |&p| settle_len(p, start_settle));
    let end = route.last().map_or(0, |&p| settle_len(p, end_settle));
    (octile_path_len(route) + start + end).max(1)
}

/// Pure linear interpolation along the walk segment `from → to` at
/// `t_x1000` (0..=1000). Deterministic walk-leg geometry: the pose history
/// records with it (snap-back lookups need the breath-free position) and
/// `pixel_painter` re-imports it to place the walking sprite/label anchors.
pub(crate) fn walking_position(from: Point, to: Point, t_x1000: u16) -> Point {
    let t = t_x1000 as i32;
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    // Clamp at zero before casting to u16 — left-walking agents (to.x <
    // from.x) cross through negative x partway through their walk if the
    // animation interpolation overshoots, and a bare `as u16` cast wraps
    // silently to ~65k, blitting the sprite off-screen invisibly.
    Point {
        x: (from.x as i32 + dx * t / 1000).clamp(0, u16::MAX as i32) as u16,
        y: (from.y as i32 + dy * t / 1000).clamp(0, u16::MAX as i32) as u16,
    }
}

#[cfg(test)]
mod tests;
