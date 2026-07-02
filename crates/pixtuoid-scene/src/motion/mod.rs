//! Per-agent walk-timing state owned by the TUI layer.
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

use pixtuoid_core::physics::{walk_arrived, walk_profile, WalkIntent, WalkProfile};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::layout::{Layout, Point, WaypointKind};
use crate::pathfind::Router;
use crate::pose::{
    aimless_wander_seed, dwell_ms, est_wander_cycle_ms, is_aimless_cycle, pick_aimless_dest,
    seated_dwell_ms, stale_resume_gap_ms, takes_trip, waypoint_index_for_cycle,
    WANDER_DWELL_EST_MS,
};
use crate::pose::{desk_leg_endpoint, octile_distance, route_jittered};

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

/// Phase the wander cycle is currently in for a given agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WanderPhase {
    /// Sitting at the desk between trips.
    Seated,
    /// Walking from desk to the chosen waypoint.
    WalkingOut,
    /// Standing/sitting at the waypoint during the dwell beat.
    AtWaypoint,
    /// Walking from the waypoint back to the desk.
    WalkingBack,
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

/// The elastic cyclic-wander timeline state machine for one agent (desk → waypoint
/// → desk, repeating). The nine fields are one unit: `advance_wander` transitions
/// them together, and the whole thing is idempotent per `now` via `last_advanced_at`.
/// (Was nine flat `wander_*` / `last_advanced_at` fields on `MotionState`.)
#[derive(Debug, Clone)]
pub struct WanderState {
    /// Monotonically increasing wander cycle counter, incremented each time
    /// `WalkingBack` completes — selects the waypoint destination (mirrors
    /// `core::pose`'s `cycle_n` derivation).
    pub cycle_n: u64,
    /// Current phase of the wander cycle.
    pub phase: WanderPhase,
    /// Wall-clock instant the current phase began (reset every transition, so each
    /// leg has its own clock). Sentinel `UNIX_EPOCH` ⇒ a fresh agent `advance_wander`
    /// bootstraps.
    pub phase_started_at: SystemTime,
    /// Walk profile for the current out-/back-leg, snapshotted at the phase
    /// transition. `None` while `Seated` or `AtWaypoint`.
    pub profile: Option<WalkProfile>,
    /// Destination pixel of the current trip. Reset on each new `WalkingOut`.
    pub dest: Point,
    /// Kind of the current wander waypoint, if it is a named waypoint.
    pub dest_kind: Option<WaypointKind>,
    /// Index into `layout.waypoints` for the current destination, if named.
    pub dest_wp_idx: Option<usize>,
    /// Seat foot cell `S` for the current waypoint (where the seated sprite
    /// renders), when it's an occupied seat. `Some` ⇒ the walk SETTLES from the
    /// approach point `dest` onto `S` (and rises from `S` on the way back) so
    /// arrival/departure don't pop; `None` for obstacles/aimless (stands AT `dest`).
    pub seat: Option<Point>,
    /// Last `now` at which `advance_wander` performed a transition — idempotency:
    /// `now <= last_advanced_at` ⇒ a no-op on mutable state. Sentinel `UNIX_EPOCH`
    /// ⇒ never advanced.
    pub last_advanced_at: SystemTime,
}

/// Per-agent walk-timing state owned by the TUI layer.
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

    /// The elastic cyclic-wander timeline state machine — all nine values move as
    /// a unit (`advance_wander` transitions them together, idempotent per `now`
    /// via `wander.last_advanced_at`). See [`WanderState`]. (Was nine flat
    /// `wander_*` / `last_advanced_at` fields.)
    pub wander: WanderState,

    /// Frozen A* polyline for the current walk leg (entry/exit/wander/snap-back).
    /// `None` while not walking. Re-snapshotted when the leg's `(from, to)`
    /// endpoints change. See [`WalkPathSnapshot`].
    pub walk_path: Option<WalkPathSnapshot>,

    /// Latch for the "wander walk profile missing" warn: the Missing recover
    /// path fires every frame while the corrupt state persists (the
    /// WalkingOut/WalkingBack arms return early without restoring a profile),
    /// so only the FIRST miss of an episode warns — repeats log at trace.
    /// Reset as soon as the profile is readable again (episode over).
    pub(crate) missing_profile_warned: bool,
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
                profile: None,
                // Placeholder — replaced on first WalkingOut transition.
                dest: Point { x: 0, y: 0 },
                dest_kind: None,
                dest_wp_idx: None,
                seat: None,
                last_advanced_at: SystemTime::UNIX_EPOCH,
            },
            walk_path: None,
            missing_profile_warned: false,
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
        ms.wander.profile = None;
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
    let dwell_dur = ms
        .wander
        .dest_kind
        .map_or(WANDER_DWELL_EST_MS, |k| dwell_ms(k, id));

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
                    let (dest, dest_kind, wp_idx, seat) =
                        pick_wander_dest(id, ms.wander.cycle_n, layout, origin);
                    ms.wander.dest = dest;
                    ms.wander.dest_kind = dest_kind;
                    ms.wander.dest_wp_idx = wp_idx;
                    ms.wander.seat = seat;

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
                    let desk_glide = settle_len(from, chair_settle);
                    let len = (octile_path_len(&path) + desk_glide + settle_len(dest, seat)).max(1);
                    ms.wander.profile = Some(walk_profile(len, WalkIntent::WanderOut, id));

                    ms.wander.phase = WanderPhase::WalkingOut;
                    ms.wander.phase_started_at = ms
                        .wander
                        .phase_started_at
                        .checked_add(Duration::from_millis(seated_dur))
                        .unwrap_or(now);
                }
            }
            (ms.wander.phase, 0)
        }

        WanderPhase::WalkingOut => {
            match poll_walk_leg(
                slot,
                ms,
                WanderPhase::WalkingOut,
                elapsed_phase,
                may_transition,
            ) {
                WalkLegStatus::Missing => return (WanderPhase::WalkingOut, 0),
                WalkLegStatus::InFlight(t) => (WanderPhase::WalkingOut, t),
                WalkLegStatus::Arrived {
                    t_x1000,
                    walk_total,
                } => {
                    // Divergent on-arrival: snapshot the walk-back profile (the
                    // overlay may differ now) and store it for the AtWaypoint →
                    // WalkingBack transition. `t_x1000` is 1000 at arrival (the
                    // walk-out leg's terminal progress) — preserves the old
                    // hardcoded `1000` return byte-for-byte.
                    let back = snapshot_back_profile(slot, ms, layout, router, overlay);
                    ms.wander.phase = WanderPhase::AtWaypoint;
                    ms.wander.profile = Some(back);
                    advance_phase_clock(ms, walk_total, now);
                    (WanderPhase::AtWaypoint, t_x1000)
                }
            }
        }

        WanderPhase::AtWaypoint => {
            if may_transition && elapsed_phase >= dwell_dur {
                // Use the back-leg profile already snapshotted at WalkingOut arrival.
                // If somehow missing (shouldn't happen), re-snapshot now.
                if ms.wander.profile.is_none() {
                    let back = snapshot_back_profile(slot, ms, layout, router, overlay);
                    ms.wander.profile = Some(back);
                }

                ms.wander.phase = WanderPhase::WalkingBack;
                ms.wander.phase_started_at = ms
                    .wander
                    .phase_started_at
                    .checked_add(Duration::from_millis(dwell_dur))
                    .unwrap_or(now);
            }
            (ms.wander.phase, 0)
        }

        WanderPhase::WalkingBack => {
            match poll_walk_leg(
                slot,
                ms,
                WanderPhase::WalkingBack,
                elapsed_phase,
                may_transition,
            ) {
                WalkLegStatus::Missing => return (WanderPhase::WalkingBack, 0),
                WalkLegStatus::InFlight(t) => (WanderPhase::WalkingBack, t),
                WalkLegStatus::Arrived { walk_total, .. } => {
                    // Divergent on-arrival: a cycle completed — advance the cycle
                    // counter and clear the trip fields.
                    ms.wander.cycle_n += 1;
                    ms.wander.profile = None;
                    ms.wander.dest_kind = None;
                    ms.wander.dest_wp_idx = None;
                    // Clear the seat too (symmetry with the sibling dest fields):
                    // the Seated arm never reads it and the next WalkingOut overwrites
                    // it, but leaving it stale invites a future Seated-phase reader to
                    // mistake it for "currently on a seat".
                    ms.wander.seat = None;
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
    /// The phase profile is missing (should be unreachable — it is snapshotted at
    /// the entering transition). The caller logs + recovers in place.
    Missing,
    /// Still walking: the physics progress `t_x1000` (0..1000).
    InFlight(u16),
    /// The walk (incl. its pause) has completed. `walk_total` = `duration_ms +
    /// pause_ms` for the shared phase-clock advance; `t_x1000` is the progress at
    /// arrival (1000), exposed for completeness.
    Arrived { t_x1000: u16, walk_total: u64 },
}

/// The scaffold the `WalkingOut` and `WalkingBack` arms share: read the phase
/// profile (warn on the unreachable missing case), compute physics progress, and
/// classify the leg as in-flight or arrived. The arms run their OWN divergent
/// on-arrival cleanup (WalkingOut: snapshot the back profile; WalkingBack: bump
/// the cycle + clear trip fields incl. `wander.seat`) — only the read/progress/
/// arrival check is factored here.
fn poll_walk_leg(
    slot: &AgentSlot,
    ms: &mut MotionState,
    phase: WanderPhase,
    elapsed_phase: u64,
    may_transition: bool,
) -> WalkLegStatus {
    let profile = match &ms.wander.profile {
        Some(p) => p,
        None => {
            // Should be unreachable: an in-flight phase always has a profile
            // snapshotted at its entering transition. Log + recover (project
            // convention: never freeze silently). The Missing state persists
            // across frames (the callers' early returns don't restore a
            // profile), so the warn is LATCHED per agent per episode — the
            // first miss warns, repeats trace — or a stuck agent floods the
            // log at frame rate.
            if ms.missing_profile_warned {
                tracing::trace!(
                    agent_id = ?slot.agent_id,
                    ?phase,
                    "wander walk profile still missing — recovering"
                );
            } else {
                ms.missing_profile_warned = true;
                tracing::warn!(
                    agent_id = ?slot.agent_id,
                    ?phase,
                    "wander walk profile missing — recovering"
                );
            }
            return WalkLegStatus::Missing;
        }
    };
    // Profile readable again: the missing episode (if any) is over — re-arm.
    ms.missing_profile_warned = false;
    let t_x1000 = pixtuoid_core::physics::walk_progress(profile, elapsed_phase);
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

/// Pick the wander destination for a given agent and cycle. Mirrors the same
/// logic as `core::pose::idle_pose` so `cycle_n` produces identical
/// destination choices in both the stateless core path and the stateful tui path.
///
/// `origin` is the agent's home desk — the stand-side tiebreaker, kept
/// identical to `core::pose::idle_pose`'s `desk` so the paths can't drift.
///
/// Returns `(dest_point, waypoint_kind, waypoint_index)`.
fn pick_wander_dest(
    id: AgentId,
    cycle_n: u64,
    layout: &Layout,
    origin: Point,
) -> (Point, Option<WaypointKind>, Option<usize>, Option<Point>) {
    if is_aimless_cycle(id, cycle_n) {
        // Shared seed helper so this can never drift from core::pose::idle_pose.
        let seed = aimless_wander_seed(id, cycle_n);
        let p = pick_aimless_dest(layout, seed, origin);
        (p, None, None, None)
    } else {
        let wp_idx = waypoint_index_for_cycle(id, cycle_n, layout.waypoints.len());
        let wp = layout.waypoints[wp_idx];
        // Walk destination = the A*-reachable approach point on an allowed side
        // (NOT the raw blocked `wp.pos`, which made A* detour + the sprite pop).
        // Same `&layout.reachable` + origin as core::pose::idle_pose so the
        // stateless overlay and this routed dest stay in lockstep.
        let dest = pixtuoid_core::layout::approach_point(
            wp.kind.furniture(),
            wp.pos,
            wp.facing,
            layout.pantry_counter_size,
            &layout.walkable,
            origin,
            &layout.reachable,
        );
        // NO approach-side fallback: when no allowed+reachable side exists,
        // approach_point returns the blocked `wp.pos` sentinel (a seat boxed in to
        // only its backrest, or an obstacle with no open reachable side). Never
        // route there — A* would snap onto the furniture (the backrest, for a
        // seat). Amble aimlessly this cycle instead, matching idle_pose.
        if dest == wp.pos {
            let seed = aimless_wander_seed(id, cycle_n);
            return (pick_aimless_dest(layout, seed, origin), None, None, None);
        }
        // Seat foot cell `S`: the walk SETTLES from `dest` onto it (the sprite
        // renders here). `None` for obstacles — the agent stands AT `dest`.
        let seat = pixtuoid_core::layout::seated_foot_cell(wp.kind.furniture(), wp.pos);
        (dest, Some(wp.kind), Some(wp_idx), seat)
    }
}

/// Snapshot the WanderBack `WalkProfile`: route `wander.dest → desk approach
/// cell`, add the seat-rise (`settle_len(wander.dest, wander.seat)`) and the
/// chair-glide settle, then freeze a `WanderBack` profile over that full
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
        .unwrap_or(ms.wander.dest);
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
        ms.wander.dest,
        snap_to,
    );
    let desk_glide = settle_len(snap_to, chair_settle);
    let back_len =
        (octile_path_len(&back_path) + settle_len(ms.wander.dest, ms.wander.seat) + desk_glide)
            .max(1);
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
        x: (from.x as i32 + dx * t / 1000).max(0).min(u16::MAX as i32) as u16,
        y: (from.y as i32 + dy * t / 1000).max(0).min(u16::MAX as i32) as u16,
    }
}

#[cfg(test)]
mod tests;
