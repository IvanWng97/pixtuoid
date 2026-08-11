//! Pose layer: re-exports the stateless state → pose derivation from the `pure`
//! sibling and adds the routed machinery — `PoseHistory` plus
//! `derive_with_routing`, which consults a `&mut dyn Router` so walking poses
//! follow A*-routed polylines and state transitions are smoothed with a
//! snap-back walk instead of teleporting back to the desk.

mod pure;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::AgentId;

use crate::motion::{
    advance_wander, snapshot_leg_profile, walking_position, MotionState, WalkLeg, WalkPathSnapshot,
    WanderKind, WanderPhase,
};
use crate::physics::{walk_arrived, walk_progress, WalkIntent, WalkProfile};
use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};

pub use pure::{
    aimless_wander_seed, derive, derive_state_only, dwell_ms, est_wander_cycle_ms,
    is_aimless_cycle, personality_for, pick_aimless_dest, seated_dwell_ms, stale_resume_gap_ms,
    takes_trip, walking_frame, waypoint_index_for_cycle, Personality, Pose, ENTRY_ANIMATION_MS,
    STALE_RESUME_GAP_BASE_MS, STALE_RESUME_GAP_RANGE_MS, THINKING_WINDOW_SECS, TYPING_FRAMES,
    TYPING_FRAME_MS, WALKING_FRAMES, WALKING_FRAME_MS, WANDER_DWELL_EST_MS, WANDER_WALK_EST_MS,
};
// These stay crate-internal: a `pub use` would try to widen their `pub(crate)`
// visibility.
pub(crate) use pure::{resolve_wander_target, SpotClaims};

use crate::layout::{desk_walk_anchor_facing, Layout, Point};
use crate::pathfind::Router;

/// The per-frame routing engine state threaded through pose derivation,
/// character anchoring, hit-testing and label placement. `now`/`layout` stay
/// separate args — frame inputs, not engine state.
pub struct RouteCtx<'a> {
    /// The A* router for this frame.
    pub router: &'a mut dyn Router,
    /// Live occupancy overlay (shared, read-only here).
    pub overlay: &'a OccupancyOverlay,
    /// Per-agent rendered-position cache.
    pub history: &'a mut PoseHistory,
    /// Per-agent walk-timing state, keyed by `AgentId`.
    pub motion: &'a mut HashMap<AgentId, MotionState>,
}

/// Per-agent rendered position cache, consulted on state transitions so an agent
/// who was mid-walk when their state flipped can complete the walk visually
/// instead of teleporting back to their desk.
#[derive(Debug, Default, Clone)]
pub struct PoseHistory {
    last: std::collections::HashMap<AgentId, (Point, SystemTime)>,
}

impl PoseHistory {
    /// A new, empty history.
    pub fn new() -> Self {
        Self::default()
    }
    /// Record where an agent was visually placed this frame.
    pub fn record(&mut self, agent_id: AgentId, anchor: Point, now: SystemTime) {
        self.last.insert(agent_id, (anchor, now));
    }
    /// Drop entries for agents no longer in `scene`. Without this, every AgentId
    /// ever rendered leaks an entry for the process lifetime, per floor.
    pub fn evict_missing(&mut self, scene: &pixtuoid_core::state::SceneState) {
        self.last.retain(|id, _| scene.agents.contains_key(id));
    }
    /// Whether an entry exists for `agent_id`.
    pub fn contains(&self, agent_id: AgentId) -> bool {
        self.last.contains_key(&agent_id)
    }
    /// Latest recorded position if it's at most `max_age_ms` old.
    pub fn recent(&self, agent_id: AgentId, max_age_ms: u64, now: SystemTime) -> Option<Point> {
        let (pt, when) = self.last.get(&agent_id).copied()?;
        let age = now.duration_since(when).ok()?.as_millis() as u64;
        if age <= max_age_ms {
            Some(pt)
        } else {
            None
        }
    }
}

/// Snap-back ARM window (ms): only trigger a snap-back walk if the desk-bound
/// state flip happened within this long. NOT a render cap — the armed walk runs
/// to completion by physics (`walk_arrived`).
const SNAP_BACK_MS: u64 = 900;
/// Minimum manhattan distance (px) from the current rendered position to the
/// desk before animating the snap-back; below this the teleport is invisible.
const SNAP_BACK_MIN_DIST: i32 = 8;
/// Max age (ms) for a recorded `PoseHistory` position to count as "where the
/// agent is now".
const HISTORY_RECENT_MS: u64 = 300;
/// Safety margin (ms) shaved off `EXIT_GRACE_WINDOW` so the time-compressed exit
/// walk reaches the door before the reducer GCs the slot. Equal to
/// `HISTORY_RECENT_MS` by coincidence, not by motivation — don't merge them.
const EXIT_BUDGET_MARGIN_MS: u64 = 300;

/// The home desk's ARRIVAL target: a reachable cell on an ALLOWED side
/// (`DESK_APPROACH` = N/E/W, excluding the south front), so an arriving agent
/// walks AROUND to sit behind the desk instead of straight through its front.
/// The chair itself is blocked, so targeting it directly made A\* fall back to a
/// straight `door→chair` line THROUGH the desk body.
///
/// Scans from the CHAIR, not the desk's top-left origin: the footprint is
/// anchored top-left, so a corner scan is lopsided and can't clear the body to
/// the EAST — the east side would read as walled-off. `None` only in a
/// degenerate layout where every allowed side is walled off.
pub(crate) fn desk_approach_cell(desk: Point, layout: &Layout) -> Option<Point> {
    use crate::layout::{desk_walk_anchor_facing, Furniture};
    // The desk's OWN facing, not a constant: `ApproachSides` is canonical (facing-South) and
    // rotated by it, so a back-turned desk is approached from its south front, not walled off there.
    let facing = layout.desk_facing_at(desk);
    let chair = desk_walk_anchor_facing(desk, facing);
    let cell = layout.approach_point(Furniture::Desk, chair, chair, facing);
    // `approach_point` returns the scanned pos (== chair) as its "no approach"
    // sentinel when no allowed+reachable side exists.
    (cell != chair).then_some(cell)
}

/// The desk-side endpoint of a desk-bound walk leg, resolved the ONE unified way
/// so no leg can regress to aiming A\* at the blocked chair: aiming there makes
/// `find_path` snap to the NEAREST walkable cell — the south front for a
/// south-facing chair — so the agent arrives through the desk front.
///
/// Returns `(routing_endpoint, chair_settle)`: the reachable N/E/W cell to hand
/// A\*, plus `Some(chair)` to prepend/append via [`Settle`] (the short glide
/// on/off the seat the router never plans), or `None` in the degenerate boxed-in
/// layout where the leg reverts to the direct chair target.
pub(crate) fn desk_leg_endpoint(desk: Point, layout: &Layout) -> (Point, Option<Point>) {
    let chair = crate::layout::desk_walk_anchor_facing(desk, layout.desk_facing_at(desk));
    match desk_approach_cell(desk, layout) {
        Some(approach) => (approach, Some(chair)),
        None => (chair, None),
    }
}
/// Where a cancelled walkout must re-enter FROM, or `None` when there was no
/// walkout to cancel.
enum ReEnter {
    /// The walkout ARRIVED — the sprite is off-floor, so the door is the only
    /// honest origin.
    Door,
    Live(Point),
}

/// Take a spent EXIT leg off the non-exiting path and say where entry re-arms.
///
/// An exit leg surviving onto this path means `exiting_at` was cleared under us
/// without re-stamping `created_at`, so the entry branch's spawn-window gate can
/// never re-arm on its own and the agent would pop onto its chair. The take is
/// load-bearing on its own: a retained arrived leg is replayed by the NEXT exit,
/// vanishing the sprite on its first frame instead of walking out.
fn take_cancelled_walkout(
    ms: Option<&mut MotionState>,
    now: SystemTime,
    live: Option<Point>,
) -> Option<ReEnter> {
    let ms = ms?;
    let leg = ms.exit.take()?;
    ms.entry = None;
    let elapsed = crate::anim::elapsed_ms(now, leg.started_at);
    Some(
        if walk_arrived(&leg.profile, exit_elapsed_ms(&leg.profile, elapsed)) {
            ReEnter::Door
        } else {
            live.map_or(ReEnter::Door, ReEnter::Live)
        },
    )
}

/// Time-compressed elapsed for an EXIT leg, so the walk REACHES the door before
/// the reducer's `EXIT_GRACE_WINDOW` reaps the slot; without it the slot is GC'd
/// mid-walk and the sprite vanishes in the corridor. (Entry has no such cap —
/// nothing GCs an entering agent.)
///
/// `floor::recompute_door_anim_max_ms` asks "has the walkout finished?" too and
/// deliberately stays UNCOMPRESSED: it wants the physics window for a door
/// cosmetic, not the render's deadline.
fn exit_elapsed_ms(profile: &WalkProfile, elapsed_ms: u64) -> u64 {
    let budget = (pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64)
        .saturating_sub(EXIT_BUDGET_MARGIN_MS);
    if profile.duration_ms.saturating_add(profile.pause_ms) > budget {
        (elapsed_ms.saturating_mul(profile.duration_ms) / budget.max(1)).max(elapsed_ms)
    } else {
        elapsed_ms
    }
}

/// Routed variant of `derive`: Walking poses trace an A*-routed polyline
/// (layout mask + per-frame `overlay`) corner-by-corner instead of cutting
/// through obstacles or other agents.
///
/// `motion` drives entry/exit physics — the A* path length is snapshotted into a
/// `WalkProfile` on first sighting (commit-to-route), and later frames compute
/// `t_x1000` against that frozen profile. `history` is consulted on state
/// transitions so an agent whose pose flipped mid-wander walks back to the desk
/// instead of teleporting.
pub fn derive_with_routing(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    rctx: &mut RouteCtx<'_>,
) -> Option<Pose> {
    let desk = layout.home_desk(slot.desk_index.single_floor_local())?;

    if let Some(exit_time) = slot.exiting_at {
        let Some(door_target) = layout.door_threshold else {
            // No door in this layout (a very narrow terminal). Returning None
            // here would VANISH the exiting agent on its first frame; hold at the
            // desk and let the reducer's grace window GC the slot.
            let raw = derive_state_only(slot, now, layout)?;
            return match raw {
                Pose::Walking { .. } => {
                    route_walking_pose(slot, now, layout, rctx, raw, Settle::None)
                }
                other => Some(other),
            };
        };

        let mstate = rctx
            .motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        if mstate.exit.is_none() {
            // Start the exit from wherever the agent actually is; without this an
            // agent mid-coffee-run when its session ends teleports back to the
            // desk before walking to the door.
            let desk_anchor = desk_walk_anchor_facing(desk, layout.desk_facing_at(desk));
            let from = rctx
                .history
                .recent(slot.agent_id, HISTORY_RECENT_MS, now)
                .unwrap_or(desk_anchor);
            let (route_from, chair_rise) = if from == desk_anchor {
                desk_leg_endpoint(desk, layout)
            } else {
                (from, None)
            };
            let profile = snapshot_leg_profile(
                rctx.router,
                &layout.walkable,
                rctx.overlay,
                slot.agent_id,
                route_from,
                door_target,
                chair_rise,
                None,
                WalkIntent::Exit,
            );
            // Store the ORIGIN so the render can detect a desk departure and
            // re-derive the same approach + settle.
            mstate.exit = Some(WalkLeg {
                started_at: exit_time,
                profile,
                from,
            });
        }

        let e = mstate.exit.as_ref()?;
        let started_at = e.started_at;
        let profile = &e.profile;
        let stored_from = e.from;

        let elapsed_ms = crate::anim::elapsed_ms(now, started_at);

        let eff_elapsed = exit_elapsed_ms(profile, elapsed_ms);

        if walk_arrived(profile, eff_elapsed) {
            return None;
        }

        let t_x1000 = walk_progress(profile, eff_elapsed);
        let frame = walking_frame(eff_elapsed);

        // Must reproduce the snapshotted profile's endpoints, or the leg's
        // duration no longer matches the distance it renders.
        let (from, exit_settle) =
            if stored_from == desk_walk_anchor_facing(desk, layout.desk_facing_at(desk)) {
                let (approach, chair) = desk_leg_endpoint(desk, layout);
                (approach, chair.map_or(Settle::None, Settle::Start))
            } else {
                (stored_from, Settle::None)
            };

        return route_walking_pose(
            slot,
            now,
            layout,
            rctx,
            Pose::Walking {
                from,
                to: door_target,
                t_x1000,
                frame,
                carrying_coffee: false,
            },
            exit_settle,
        );
    }

    let live_now = rctx.history.recent(slot.agent_id, HISTORY_RECENT_MS, now);
    let re_enter = take_cancelled_walkout(rctx.motion.get_mut(&slot.agent_id), now, live_now);

    // ENTRY_ANIMATION_MS bounds only how long we try to ROUTE; the physics
    // duration is the real walk time.
    let since_spawn = now
        .duration_since(slot.created_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    if let Some(door) = layout.door_threshold {
        let (approach, chair_settle) = desk_leg_endpoint(desk, layout);
        let settle = chair_settle.map_or(Settle::None, Settle::End);

        let mstate = rctx
            .motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        let entry_from = match re_enter {
            Some(ReEnter::Live(p)) => p,
            _ => door,
        };
        if mstate.entry.is_none() && (since_spawn < ENTRY_ANIMATION_MS || re_enter.is_some()) {
            let profile = snapshot_leg_profile(
                rctx.router,
                &layout.walkable,
                rctx.overlay,
                slot.agent_id,
                entry_from,
                approach,
                None,
                chair_settle,
                WalkIntent::Entry,
            );
            mstate.entry = Some(WalkLeg {
                started_at: if re_enter.is_some() {
                    now
                } else {
                    slot.created_at
                },
                profile,
                from: entry_from,
            });
        }

        if let Some(WalkLeg {
            started_at,
            profile,
            from,
        }) = mstate.entry
        {
            let elapsed_ms = crate::anim::elapsed_ms(now, started_at);

            if !walk_arrived(&profile, elapsed_ms) {
                let t_x1000 = walk_progress(&profile, elapsed_ms);
                let frame = walking_frame(elapsed_ms);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    rctx,
                    Pose::Walking {
                        from,
                        to: approach,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                    settle,
                );
            }
            // DO NOT call `derive()` here — it re-fires the linear entry override
            // and causes a double-walk. Fall through to the state-driven pose.
        }
    }

    // Wander dispatch gates on Idle, NOT on `since_spawn >= ENTRY_ANIMATION_MS`:
    // that fixed gate made a near-desk agent who physically arrived in ~1s sit in
    // core's fixed-fraction idle_pose for seconds and then snap to physics wander.
    let is_idle = matches!(slot.state, pixtuoid_core::state::ActivityState::Idle);
    if is_idle && slot.exiting_at.is_none() {
        // Before `advance_wander`, so the routed render and the pure overlay share
        // the ONE `pure::in_thinking_window` gate and can't drift.
        if pure::in_thinking_window(slot, now) {
            return Some(Pose::SeatedThinking);
        }

        // A per-frame snapshot, so the arms below never re-borrow `rctx.motion`.
        let wf = advance_wander(slot, now, layout, rctx.router, rctx.overlay, rctx.motion);

        match wf.phase {
            WanderPhase::WalkingOut(_) => {
                let desk_point = layout.home_desk(slot.desk_index.single_floor_local())?;
                let dest = wf.dest;
                let seat = wf.kind.seat();
                let (from, chair_settle) = desk_leg_endpoint(desk_point, layout);
                let settle = settle_from_pair(chair_settle, seat);
                let elapsed_phase = crate::anim::elapsed_ms(now, wf.phase_started_at);
                let frame = walking_frame(elapsed_phase);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    rctx,
                    Pose::Walking {
                        from,
                        to: dest,
                        t_x1000: wf.t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                    settle,
                );
            }
            WanderPhase::AtWaypoint(_) => {
                let pose = wf.kind.at_pose(wf.dest);
                // The RENDERED position, which snap-back/exit read as their walk
                // origin: the seat cell for a seat waypoint (the sprite sits ON the
                // furniture, offset from `dest`), else `dest`. Recording `dest` for
                // a seat popped the sprite to the off-side approach cell on the
                // first snap-back/exit frame.
                let pt = wf.kind.seat().unwrap_or(wf.dest);
                rctx.history.record(slot.agent_id, pt, now);
                return Some(pose);
            }
            WanderPhase::WalkingBack(_) => {
                let desk_point = layout.home_desk(slot.desk_index.single_floor_local())?;
                let carrying_coffee = wf.kind.carries_coffee();
                let seat = wf.kind.seat();
                let (snap_target, chair_settle) = desk_leg_endpoint(desk_point, layout);
                let settle = settle_from_pair(seat, chair_settle);
                let elapsed_phase = crate::anim::elapsed_ms(now, wf.phase_started_at);
                let frame = walking_frame(elapsed_phase);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    rctx,
                    Pose::Walking {
                        from: wf.dest,
                        to: snap_target,
                        t_x1000: wf.t_x1000,
                        frame,
                        carrying_coffee,
                    },
                    settle,
                );
            }
            WanderPhase::Seated => {
                // Render SeatedIdle DIRECTLY rather than falling through to
                // derive_state_only: that re-runs core's STATELESS wander
                // (`idle_pose`), whose independent timeline can return
                // AtWaypoint/AimlessAt at an unrelated location and teleport the
                // sprite between desk and waypoint as the two clocks drift.
                return Some(Pose::SeatedIdle);
            }
        }
    }

    // Went Active/Waiting: drop any exclusive-spot claim, else the frozen
    // `wander.target` blocks that spot for the whole burst.
    if let Some(ms) = rctx.motion.get_mut(&slot.agent_id) {
        ms.wander.target.kind = WanderKind::Aimless;
    }

    // derive_state_only, NOT derive: derive() re-triggers the linear entry/exit
    // overrides and produces a double-walk.
    let raw = derive_state_only(slot, now, layout)?;

    // Snap-back override: a desk-bound state pose would teleport an agent who was
    // mid-wander when state changed, so replace it with a walk from the previous
    // rendered position.
    let desk_pose = matches!(
        raw,
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } | Pose::StandingAtDesk
    );
    let since_state = crate::anim::elapsed_ms(now, slot.state_started_at);
    let mut final_settle = Settle::None;
    let pose = if desk_pose {
        let ms_entry = rctx
            .motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));
        // ARM ONCE per state transition: `route_walking_pose` records the advancing
        // walker position into history on every call, so re-checking the distance
        // gate on a second `derive` in the same frame would see a CLOSER `prev` and
        // drop the agent back to Seated mid-walk. Keying the arm on
        // `slot.state_started_at` (not `now`) lets a NEW desk-bound transition
        // re-arm with a fresh clock.
        let already_armed =
            matches!(&ms_entry.snap_back, Some(leg) if leg.started_at == slot.state_started_at);
        if !already_armed {
            ms_entry.snap_back = None;
            if since_state < SNAP_BACK_MS {
                if let Some(prev) = rctx.history.recent(slot.agent_id, HISTORY_RECENT_MS, now) {
                    // Distance to the CHAIR, not the desk origin: the chair is offset
                    // from the origin, so a desk-origin gate would re-fire forever
                    // once the agent has settled ON the chair.
                    let chair = desk_walk_anchor_facing(desk, layout.desk_facing_at(desk));
                    let dist = (prev.x as i32 - chair.x as i32).abs()
                        + (prev.y as i32 - chair.y as i32).abs();
                    if dist >= SNAP_BACK_MIN_DIST {
                        // Route ONCE at arm time, against the same jittered goal the
                        // render route uses: the leg renders through
                        // `route_walking_pose`'s A* polyline, so the profile must
                        // measure THAT polyline (+ the chair-glide) or a detouring
                        // return traverses a longer path inside a straight-line
                        // duration, and arrives too fast.
                        let (snap_target, chair_settle) = desk_leg_endpoint(desk, layout);
                        let p = snapshot_leg_profile(
                            rctx.router,
                            &layout.walkable,
                            rctx.overlay,
                            slot.agent_id,
                            prev,
                            snap_target,
                            None,
                            chair_settle,
                            WalkIntent::SnapBack,
                        );
                        ms_entry.snap_back = Some(WalkLeg {
                            started_at: slot.state_started_at,
                            profile: p,
                            from: prev,
                        });
                    }
                }
            }
        }
        match ms_entry.snap_back.clone() {
            Some(WalkLeg {
                started_at,
                profile,
                from: snap_prev,
            }) if started_at == slot.state_started_at => {
                let elapsed_ms = crate::anim::elapsed_ms(now, started_at);
                // PURE physics, no time-compression: `WalkIntent::SnapBack`'s higher
                // accel keeps the urgent return brisk on its own.
                if walk_arrived(&profile, elapsed_ms) {
                    ms_entry.snap_back = None;
                    raw
                } else {
                    let t_x1000 = walk_progress(&profile, elapsed_ms);
                    let frame = walking_frame(elapsed_ms);
                    // Deterministic, so the rendered leg reproduces the armed
                    // profile's endpoint.
                    let (snap_target, chair_settle) = desk_leg_endpoint(desk, layout);
                    final_settle = chair_settle.map_or(Settle::None, Settle::End);
                    // Walk from the FROZEN origin captured when the leg armed, not the
                    // per-frame `prev`: `route_walking_pose` re-records the advancing
                    // walker position into history every frame, so reading it back as
                    // the origin would creep `from` toward the desk and break the
                    // walk-path freeze's `wp.from == from` reuse guard.
                    Pose::Walking {
                        from: snap_prev,
                        to: snap_target,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    }
                }
            }
            _ => raw,
        }
    } else {
        // Clear any stale snap-back so the next transition snapshots afresh rather
        // than replaying a previous one.
        if let Some(ms) = rctx.motion.get_mut(&slot.agent_id) {
            if ms.snap_back.is_some() {
                ms.snap_back = None;
            }
        }
        raw
    };

    route_walking_pose(slot, now, layout, rctx, pose, final_settle)
}

/// How a walk leg extends its polyline onto a seat — a short terminal motion the
/// A* router never plans (the seat cell may be blocked). `End` = sit down on
/// arrival (append the seat); `Start` = stand up on departure (prepend it).
/// Makes walk-end ≡ render-feet so seat arrival/departure don't pop.
#[derive(Clone, Copy)]
enum Settle {
    None,
    End(Point),
    Start(Point),
    Both { start: Point, end: Point },
}

/// Collapse a leg's `(start_settle, end_settle)` pair into a single [`Settle`].
/// Argument order encodes direction: `start` = the seat to rise OFF (prepended),
/// `end` = the seat to glide ONTO (appended).
fn settle_from_pair(start: Option<Point>, end: Option<Point>) -> Settle {
    match (start, end) {
        (Some(start), Some(end)) => Settle::Both { start, end },
        (Some(start), None) => Settle::Start(start),
        (None, Some(end)) => Settle::End(end),
        (None, None) => Settle::None,
    }
}

fn route_walking_pose(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    rctx: &mut RouteCtx<'_>,
    pose: Pose,
    settle: Settle,
) -> Option<Pose> {
    let router = &mut *rctx.router;
    let overlay = rctx.overlay;
    let history = &mut *rctx.history;
    let motion = &mut *rctx.motion;
    let Pose::Walking {
        from,
        to,
        t_x1000,
        frame,
        carrying_coffee,
    } = pose
    else {
        if let Some(ms) = motion.get_mut(&slot.agent_id) {
            ms.walk_path = None;
        }
        // AtWaypoint / AimlessAt positions are a valid "previous position" for a
        // subsequent snap-back walk, so record them too.
        let pt = match &pose {
            Pose::AtWaypoint { wp, .. } => layout.waypoints.get(*wp).map(|w| w.pos),
            Pose::AimlessAt { dest } => Some(*dest),
            _ => None,
        };
        if let Some(p) = pt {
            history.record(slot.agent_id, p, now);
        }
        return Some(pose);
    };

    // Freeze the leg's polyline: snapshot the A* route the first frame this
    // (from, to) leg appears, then reuse it unchanged until the endpoints change.
    // Without this, per-frame occupancy-overlay churn invalidates the A* cache and
    // re-routes the walker onto a differently-shaped path, making the
    // frozen-profile progress `t` land on a new pixel — the visible "flash" — and
    // re-routing every frame is what spikes a frame's A* cost.
    let path = {
        let ms = motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));
        match &ms.walk_path {
            Some(wp) if wp.from == from && wp.to == to => wp.path.clone(),
            _ => {
                let mut p =
                    route_jittered(router, &layout.walkable, overlay, slot.agent_id, from, to);
                match settle {
                    Settle::End(s) if p.last() != Some(&s) => p.push(s),
                    Settle::Start(s) if p.first() != Some(&s) => p.insert(0, s),
                    Settle::Both { start, end } => {
                        // Append the end first so the prepend can't shift it.
                        if p.last() != Some(&end) {
                            p.push(end);
                        }
                        if p.first() != Some(&start) {
                            p.insert(0, start);
                        }
                    }
                    _ => {}
                }
                // Only freeze genuinely CORNERED routes (>2 points). Freezing a
                // straight 2-point walk would stick a transient A* fallback
                // (`find_path` returned None this frame) — a walk through walls —
                // for the whole leg; unfrozen, the next frame recovers the route.
                if p.len() > 2 {
                    ms.walk_path = Some(WalkPathSnapshot {
                        from,
                        to,
                        path: p.clone(),
                    });
                } else {
                    ms.walk_path = None;
                }
                p
            }
        }
    };
    if path.len() <= 2 {
        // Record the interpolated position for the next frame's snap-back lookup.
        history.record(slot.agent_id, walking_position(from, to, t_x1000), now);
        return Some(Pose::Walking {
            from,
            to,
            t_x1000,
            frame,
            carrying_coffee,
        });
    }
    // Map global t to (segment_idx, t_within_segment) by cumulative octile
    // distance — the same metric A* planned with, so timing stays uniform along
    // diagonals.
    let mut leg_lens: Vec<u32> = Vec::with_capacity(path.len() - 1);
    for w in path.windows(2) {
        leg_lens.push(octile_distance(w[0], w[1]));
    }
    let total: u32 = leg_lens.iter().sum();
    if total == 0 {
        return Some(pose);
    }
    let traveled = (t_x1000 as u32 * total) / 1000;
    let mut acc: u32 = 0;
    for (i, &leg) in leg_lens.iter().enumerate() {
        if acc + leg >= traveled {
            let into_leg = traveled - acc;
            let seg_t = (into_leg * 1000)
                .checked_div(leg)
                .map(|t| t.min(1000) as u16)
                .unwrap_or(1000);
            let cur_pos = walking_position(path[i], path[i + 1], seg_t);
            history.record(slot.agent_id, cur_pos, now);
            return Some(Pose::Walking {
                from: path[i],
                to: path[i + 1],
                t_x1000: seg_t,
                frame,
                carrying_coffee,
            });
        }
        acc += leg;
    }
    let last = path.len() - 1;
    history.record(slot.agent_id, path[last], now);
    Some(Pose::Walking {
        from: path[last - 1],
        to: path[last],
        t_x1000: 1000,
        frame,
        carrying_coffee,
    })
}

pub(crate) fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    crate::pathfind::octile_cost(dx, dy)
}

/// Half-range (px) of the per-agent destination jitter — the offset spans
/// `-JITTER_MAX_PX..=JITTER_MAX_PX`, so the modulus span is `2*MAX+1`.
const JITTER_MAX_PX: i32 = 4;
const JITTER_SPAN: u64 = (2 * JITTER_MAX_PX + 1) as u64;

/// Per-agent routing-destination jitter, hashed from the agent_id, so converging
/// agents take visibly different polylines (breaks the "ant trail"). Output must
/// stay bit-identical across every call site — a site routing the RAW goal
/// measures a differently-shaped polyline than the one rendered AND mints a
/// second router-cache key per leg.
pub(crate) fn jitter_dest(id: AgentId, p: Point) -> Point {
    let h = id.raw();
    let jx = ((h % JITTER_SPAN) as i32 - JITTER_MAX_PX) as i16;
    let jy = (((h >> 16) % JITTER_SPAN) as i32 - JITTER_MAX_PX) as i16;
    Point {
        x: p.x.saturating_add_signed(jx),
        y: p.y.saturating_add_signed(jy),
    }
}

/// Route `from → to` against the per-agent JITTERED goal, then restore the
/// polyline's endpoint to the true `to`. EVERY walk leg routes through here, so
/// the rendered shape, the measured profile length and the router-cache key
/// can't diverge (the [`jitter_dest`] lockstep contract).
pub(crate) fn route_jittered(
    router: &mut dyn Router,
    mask: &WalkableMask,
    overlay: &OccupancyOverlay,
    id: AgentId,
    from: Point,
    to: Point,
) -> Vec<Point> {
    let mut p = router.route(mask, overlay, from, jitter_dest(id, to));
    if let Some(last) = p.last_mut() {
        *last = to;
    }
    p
}

#[cfg(test)]
mod tests;
