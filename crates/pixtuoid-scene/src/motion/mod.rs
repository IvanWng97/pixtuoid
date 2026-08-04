//! Per-agent walk-timing state owned by each `FloorCtx` in this crate.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::physics::{walk_arrived, walk_profile, WalkIntent, WalkProfile};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};
use pixtuoid_core::AgentId;

use crate::layout::{Layout, Point, WaypointKind};
use crate::pathfind::Router;
use crate::pose::{desk_leg_endpoint, octile_distance, route_jittered};
use crate::pose::{
    dwell_ms, est_wander_cycle_ms, seated_dwell_ms, stale_resume_gap_ms, takes_trip, SpotClaims,
    WANDER_DWELL_EST_MS,
};

/// Frozen A* polyline for one in-flight walk leg. Snapshotted when the leg's
/// `(from, to)` first appear and reused unchanged: per-frame occupancy churn
/// invalidates the A* cache, and mapping the frozen profile's progress `t` onto
/// a re-routed shape mid-stride makes the sprite visibly jump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkPathSnapshot {
    /// Leg start point.
    pub from: Point,
    /// Leg end point.
    pub to: Point,
    /// The frozen A* polyline from `from` to `to`.
    pub path: Vec<Point>,
}

/// Phase the wander cycle is currently in for a given agent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WanderPhase {
    /// Sitting at the desk between trips.
    Seated,
    /// Walking from desk to the chosen waypoint, on this frozen out-leg profile.
    WalkingOut(WalkProfile),
    /// Standing/sitting at the waypoint during the dwell beat, holding the
    /// return-leg profile snapshotted at out-leg arrival.
    AtWaypoint(WalkProfile),
    /// Walking from the waypoint back to the desk, on the return-leg profile.
    WalkingBack(WalkProfile),
}

/// The wander frame `advance_wander` resolves, snapshotted off `MotionState` at
/// return time so the caller never re-reads (or re-borrows) `motion`.
#[derive(Debug, Clone, Copy)]
pub struct WanderFrame {
    /// The resolved phase this frame — selects the pose builder's arm.
    pub phase: WanderPhase,
    /// Physics walk progress 0–1000, meaningful ONLY in `WalkingOut`/
    /// `WalkingBack` (0 otherwise).
    pub t_x1000: u16,
    /// The current trip's destination cell (the walk `to` / waypoint `dest`).
    pub dest: Point,
    /// The current trip's kind — the seat cell + waypoint identity the walk /
    /// at-waypoint arms need.
    pub kind: WanderKind,
    /// The current phase's start instant — the pose builder's walk-frame clock.
    pub phase_started_at: SystemTime,
}

/// A one-shot walk leg (entry / exit / snap-back).
#[derive(Debug, Clone)]
pub struct WalkLeg {
    /// Wall-clock instant the leg armed.
    pub started_at: SystemTime,
    /// Frozen physics profile for the leg.
    pub profile: WalkProfile,
    /// Frozen leg origin, recorded at arm-time so the leg doesn't drift.
    pub from: Point,
}

/// A resolved wander destination: the walkable target cell plus WHAT it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WanderTarget {
    /// Destination pixel of the current trip (the walkable approach/amble cell).
    pub dest: Point,
    /// Whether `dest` is a named waypoint (with optional seat) or an aimless amble.
    pub kind: WanderKind,
}

/// What KIND of wander destination [`WanderTarget::dest`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WanderKind {
    /// A named lounge waypoint. `seat = Some(S)` ⇒ the walk SETTLES from the
    /// approach point `dest` onto `S` (and rises from `S` on the way back) so
    /// arrival/departure don't pop; `seat = None` ⇒ the agent stands AT `dest`.
    Named {
        /// Index into `layout.waypoints`.
        wp_idx: usize,
        /// Kind of the target waypoint.
        kind: WaypointKind,
        /// Seat foot cell to settle onto; `None` for a stand-at obstacle.
        seat: Option<Point>,
    },
    /// An aimless amble to a random walkable point (no named waypoint, no seat).
    Aimless,
}

impl WanderKind {
    pub(crate) fn seat(self) -> Option<Point> {
        match self {
            WanderKind::Named { seat, .. } => seat,
            WanderKind::Aimless => None,
        }
    }

    /// Shared by the pure `idle_pose` overlay and the routed
    /// `derive_with_routing` so the two can't drift.
    pub(crate) fn at_pose(self, dest: Point) -> crate::pose::Pose {
        match self {
            WanderKind::Named { wp_idx, kind, .. } => {
                crate::pose::Pose::AtWaypoint { wp: wp_idx, kind }
            }
            WanderKind::Aimless => crate::pose::Pose::AimlessAt { dest },
        }
    }

    pub(crate) fn carries_coffee(self) -> bool {
        matches!(
            self,
            WanderKind::Named {
                kind: WaypointKind::Pantry,
                ..
            }
        )
    }
}

/// The elastic cyclic-wander timeline state machine for one agent (desk →
/// waypoint → desk, repeating). `advance_wander` transitions the fields as a
/// unit.
#[derive(Debug, Clone)]
pub struct WanderState {
    /// Wander cycle counter, incremented each time `WalkingBack` completes —
    /// selects the waypoint destination (mirrors `pose::pure`'s derivation).
    pub cycle_n: u64,
    /// Current phase of the wander cycle.
    pub phase: WanderPhase,
    /// Wall-clock instant the current phase began, reset every transition.
    /// Sentinel `UNIX_EPOCH` ⇒ a fresh agent `advance_wander` bootstraps.
    pub phase_started_at: SystemTime,
    /// The current trip's resolved destination. Set on each new `WalkingOut`;
    /// its `kind` resets to `Aimless` when a cycle completes.
    pub target: WanderTarget,
    /// Last `now` at which `advance_wander` performed a transition — idempotency:
    /// `now <= last_advanced_at` ⇒ a no-op on mutable state. Sentinel
    /// `UNIX_EPOCH` ⇒ never advanced.
    pub last_advanced_at: SystemTime,
}

/// Walk-timing state for one live agent on one floor.
#[derive(Debug, Clone)]
pub struct MotionState {
    /// The agent this motion state belongs to.
    pub agent_id: AgentId,

    /// The arrival walk, snapshotted once at door-crossing. Carries its own
    /// `from` because a resurrect that cancels an IN-FLIGHT walkout re-enters
    /// from wherever the sprite is; a hardcoded door origin teleports it.
    pub entry: Option<WalkLeg>,
    /// The walkout, snapshotted once when `exiting_at` fires. `from` is where
    /// the sprite actually is (wander position if it was out, else the desk
    /// anchor), so the exit doesn't teleport to the desk.
    pub exit: Option<WalkLeg>,
    /// The state-transition snap-back walk. `from` is the FROZEN origin recorded
    /// when the leg armed, reused every frame so the walk doesn't drift toward
    /// the desk (mirrors `exit`).
    pub snap_back: Option<WalkLeg>,

    /// The elastic cyclic-wander timeline state machine. See [`WanderState`].
    pub wander: WanderState,

    /// Frozen A* polyline for the current walk leg, `None` while not walking.
    /// Re-snapshotted when the leg's `(from, to)` change. See
    /// [`WalkPathSnapshot`].
    pub walk_path: Option<WalkPathSnapshot>,
}

impl MotionState {
    /// Construct a fresh `MotionState`. Both wander instants are `UNIX_EPOCH` so
    /// `advance_wander` detects a bootstrap agent via the sentinel.
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
/// Transitions run ONLY when `now > wander.last_advanced_at`; otherwise the pose
/// is computed from the existing phase WITHOUT mutating any wander field, so
/// this is safe to call 2+ times per frame (seated-overlay pass, character loop,
/// `character_anchor`).
///
/// On the first call for a fresh Idle slot, `cycle_n` is fast-forwarded so
/// destination selection agrees with what core's stateless `idle_pose` would
/// have derived for an agent that was Idle before the first render.
pub fn advance_wander(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    motion: &mut HashMap<AgentId, MotionState>,
) -> WanderFrame {
    let id = slot.agent_id;
    // Claims must be snapshotted BEFORE this agent's `&mut` — the two borrows of
    // `motion` can't overlap.
    let claimed = spot_claims(motion, id);
    let ms = motion.entry(id).or_insert_with(|| MotionState::new(id));

    // A fresh MotionState's epoch `phase_started_at` is below any real
    // `state_started_at`; we also re-seed when the slot (re-)entered Idle after a
    // different state.
    let is_fresh = ms
        .wander
        .phase_started_at
        .checked_add(Duration::from_millis(1))
        .map(|t| t <= slot.state_started_at)
        .unwrap_or(true);

    // Stale resume: advanced before, but more than a full wander cycle ago — its
    // floor was off-screen (only the current floor renders) or `now` was frozen
    // (pause). Treat it as fresh so the bootstrap below snaps it to the right
    // cycle analytically instead of the phase machine replaying the backlog one
    // transition per frame (the visible "fast-forward all the movement" bug).
    // Don't raise the trigger to "max dwell": on-screen, `advance_wander` runs
    // every frame even DURING a 40 s lounge dwell, so only an off-screen floor or
    // a pause can exceed it. `unwrap_or(false)` treats a backward clock step as
    // "not stale" rather than snapping every agent to Seated.
    let is_stale_resume = ms.wander.last_advanced_at != SystemTime::UNIX_EPOCH
        && now
            .duration_since(ms.wander.last_advanced_at)
            .map(|d| d.as_millis() as u64 > stale_resume_gap_ms(id))
            .unwrap_or(false);

    if is_fresh || is_stale_resume {
        let elapsed_idle = crate::anim::elapsed_ms(now, slot.state_started_at);
        // The estimated full cycle (matches idle_pose), NOT stale_resume_gap_ms.
        let cycle = est_wander_cycle_ms(id);

        // Fast-forward `cycle_n`, but ALWAYS (re)start the phase clock cleanly in
        // Seated at `now`. Anchoring mid-cycle (`now - partial_ms`) made the phase
        // machine rush through the partial cycle's already-expired legs one
        // transition per frame — a desk↔waypoint teleport.
        ms.wander.phase = WanderPhase::Seated;
        ms.wander.cycle_n = elapsed_idle / cycle;
        ms.wander.phase_started_at = now;
    }

    let may_transition = now > ms.wander.last_advanced_at;

    let elapsed_phase = crate::anim::elapsed_ms(now, ms.wander.phase_started_at);

    let seated_dur = seated_dwell_ms(id);
    let dwell_dur = match ms.wander.target.kind {
        WanderKind::Named { kind, .. } => dwell_ms(kind, id),
        WanderKind::Aimless => WANDER_DWELL_EST_MS,
    };

    let result = match ms.wander.phase {
        WanderPhase::Seated => {
            if may_transition && elapsed_phase >= seated_dur {
                if !takes_trip(id, ms.wander.cycle_n) || layout.waypoints.is_empty() {
                    ms.wander.cycle_n += 1;
                    ms.wander.phase_started_at = ms
                        .wander
                        .phase_started_at
                        .checked_add(Duration::from_millis(seated_dur))
                        .unwrap_or(now);
                } else {
                    // The origin must match core::idle_pose's `desk` so the
                    // stateless/stateful destinations stay in lockstep.
                    let desk_pt = layout.home_desk(slot.desk_index.single_floor_local());
                    let origin = desk_pt.unwrap_or(Point { x: 0, y: 0 });
                    let target = pick_wander_dest(id, ms.wander.cycle_n, layout, origin, &claimed);
                    ms.wander.target = target;
                    let dest = target.dest;
                    let seat = target.kind.seat();

                    let desk = desk_pt.unwrap_or(dest);
                    let (from, chair_settle) = desk_leg_endpoint(desk, layout);
                    ms.wander.phase = WanderPhase::WalkingOut(snapshot_leg_profile(
                        router,
                        &layout.walkable,
                        overlay,
                        id,
                        from,
                        dest,
                        chair_settle,
                        seat,
                        WalkIntent::WanderOut,
                    ));
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
                    // Snapshot the walk-back profile now — the overlay may differ
                    // by the time the dwell ends.
                    let back = snapshot_back_profile(slot, ms, layout, router, overlay);
                    ms.wander.phase = WanderPhase::AtWaypoint(back);
                    advance_phase_clock(ms, walk_total, now);
                    (WanderPhase::AtWaypoint(back), t_x1000)
                }
            }
        }

        WanderPhase::AtWaypoint(back) => {
            if may_transition && elapsed_phase >= dwell_dur {
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
                    // `target.dest` is deliberately left as-is: the Seated arm
                    // never reads it and the next WalkingOut overwrites it.
                    ms.wander.cycle_n += 1;
                    ms.wander.target.kind = WanderKind::Aimless;
                    ms.wander.phase = WanderPhase::Seated;
                    advance_phase_clock(ms, walk_total, now);
                    (WanderPhase::Seated, 0)
                }
            }
        }
    };

    if may_transition {
        ms.wander.last_advanced_at = now;
    }

    // `result.0` == `ms.wander.phase` in every arm, so the frame's
    // `phase_started_at`/`target` always describe the returned phase.
    let (phase, t_x1000) = result;
    WanderFrame {
        phase,
        t_x1000,
        dest: ms.wander.target.dest,
        kind: ms.wander.target.kind,
        phase_started_at: ms.wander.phase_started_at,
    }
}

enum WalkLegStatus {
    /// Still walking: the physics progress `t_x1000` (0..1000).
    InFlight(u16),
    /// The walk and its pause have completed; `walk_total` = `duration_ms +
    /// pause_ms`.
    Arrived { t_x1000: u16, walk_total: u64 },
}

/// The progress/arrival check the `WalkingOut` and `WalkingBack` arms share;
/// each arm runs its OWN divergent on-arrival cleanup.
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

/// Advance the phase clock from its CURRENT anchor — not from `now` — so the
/// next phase starts exactly when this one's wall-time budget elapsed.
fn advance_phase_clock(ms: &mut MotionState, walk_total: u64, now: SystemTime) {
    ms.wander.phase_started_at = ms
        .wander
        .phase_started_at
        .checked_add(Duration::from_millis(walk_total))
        .unwrap_or(now);
}

/// Delegates to the ONE stateless resolver `crate::pose::resolve_wander_target`,
/// which `idle_pose` also calls, so the routed motion path and the stateless
/// overlay can never drift to different destinations. `origin` is the agent's
/// home desk, kept identical to `idle_pose`'s `desk`.
fn pick_wander_dest(
    id: AgentId,
    cycle_n: u64,
    layout: &Layout,
    origin: Point,
    claimed: &SpotClaims,
) -> WanderTarget {
    crate::pose::resolve_wander_target(id, cycle_n, layout, origin, claimed)
}

/// The exclusive-spot waypoints every OTHER agent on this floor is out on a trip
/// to — the exclusion set that keeps a single-occupancy spot to one occupant.
/// Read from the live wander targets, so it must be built BEFORE the caller
/// takes its own `&mut MotionState`.
///
/// Gating on **phase ≠ Seated** (not on the kind) is the honest "is this agent
/// actually out" signal: the bootstrap / stale-resume path re-seats an agent
/// WITHOUT touching its target. Gating on **`exclusive`** reuses the one
/// authority for "single-occupancy destination", so a future exclusive kind
/// inherits it; shareable waypoints are NOT claimed, since the painter's rank
/// offset is a genuine step-aside queue there.
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

/// Freeze one one-shot walk leg's timing profile. Measuring the ROUTED (not raw)
/// polyline is load-bearing: the duration must cover the whole path or `t`
/// reaches 1000 before the sprite arrives and it pops.
#[allow(clippy::too_many_arguments)] // each arg is a distinct leg parameter
pub(crate) fn snapshot_leg_profile(
    router: &mut dyn Router,
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    id: AgentId,
    from: Point,
    to: Point,
    start_settle: Option<Point>,
    end_settle: Option<Point>,
    intent: WalkIntent,
) -> WalkProfile {
    let path = route_jittered(router, mask, overlay, id, from, to);
    walk_profile(
        measured_leg_len(&path, start_settle, end_settle),
        intent,
        id,
    )
}

/// Freeze the WanderBack profile. The endpoint is the desk APPROACH cell
/// (matching `seated_anchor` via the chair-glide) so there's no jump on arrival;
/// this intentionally differs from `core::idle_pose`'s raw `to: desk`, since
/// only the routed path is user-visible.
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
    let (snap_to, chair_settle) = desk_leg_endpoint(desk, layout);
    snapshot_leg_profile(
        router,
        &layout.walkable,
        overlay,
        slot.agent_id,
        ms.wander.target.dest,
        snap_to,
        ms.wander.target.kind.seat(),
        chair_settle,
        WalkIntent::WanderBack,
    )
}

/// Octile length of a routed polyline — the same metric A* uses, so the
/// snapshotted length is consistent with per-segment timing. 0 below 2 points.
pub fn octile_path_len(path: &[Point]) -> u32 {
    if path.len() < 2 {
        return 0;
    }
    path.windows(2).map(|w| octile_distance(w[0], w[1])).sum()
}

/// Octile length of the settle segment `approach → seat`, or 0 when there is no
/// seat, so a leg's DURATION covers the sit-down/stand-up settle too.
pub(crate) fn settle_len(approach: Point, seat: Option<Point>) -> u32 {
    seat.map_or(0, |s| octile_distance(approach, s))
}

/// Rendered-polyline length of a walk leg: the routed polyline plus the settle
/// segments the router never plans (rise off `start_settle` at the FIRST point,
/// glide onto `end_settle` at the LAST), floored at 1. The profile's DURATION is
/// derived from this so it covers the FULL rendered leg and `t` can't reach 1000
/// before the sprite arrives (no pop).
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
/// `t_x1000` (0..=1000).
pub(crate) fn walking_position(from: Point, to: Point, t_x1000: u16) -> Point {
    let t = t_x1000 as i32;
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    // Left-walking agents cross through negative x if the interpolation
    // overshoots, and a bare `as u16` wraps to ~65k — blitting off-screen.
    Point {
        x: (from.x as i32 + dx * t / 1000).clamp(0, u16::MAX as i32) as u16,
        y: (from.y as i32 + dy * t / 1000).clamp(0, u16::MAX as i32) as u16,
    }
}

#[cfg(test)]
mod tests;
