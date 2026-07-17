//! Pose layer: pure derivation + the routed authority, split file-level.
//!
//! `pure` (sibling file) is the stateless state → pose derivation — a
//! function of the snapshot inputs only, no routing, no per-frame history.
//! This module re-exports its whole surface and adds the routed machinery:
//!   * `PoseHistory` — per-agent cache of the last rendered position.
//!   * `derive_with_routing` — the routed variant of `derive` that consults
//!     a `&mut dyn Router` so walking poses follow A*-routed polylines and
//!     so state transitions are smoothed with a snap-back walk instead of
//!     teleporting back to the desk.

mod pure;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::AgentId;

use crate::motion::{
    advance_wander, measured_leg_len, walking_position, MotionState, WalkLeg, WalkPathSnapshot,
    WanderKind, WanderPhase,
};
use crate::physics::{walk_arrived, walk_profile, walk_progress, WalkIntent};
use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};

pub use pure::{
    aimless_wander_seed, derive, derive_state_only, dwell_ms, est_wander_cycle_ms,
    is_aimless_cycle, personality_for, pick_aimless_dest, seated_dwell_ms, stale_resume_gap_ms,
    takes_trip, walking_frame, waypoint_index_for_cycle, Personality, Pose, ENTRY_ANIMATION_MS,
    STALE_RESUME_GAP_BASE_MS, STALE_RESUME_GAP_RANGE_MS, THINKING_WINDOW_SECS, TYPING_FRAMES,
    TYPING_FRAME_MS, WALKING_FRAMES, WALKING_FRAME_MS, WANDER_DWELL_EST_MS, WANDER_WALK_EST_MS,
};
// `resolve_wander_target` + its `SpotClaims` input stay crate-internal (the
// motion authority delegates to it and builds the claims); a `pub use` would try
// to widen their `pub(crate)` visibility.
pub(crate) use pure::{resolve_wander_target, SpotClaims};

use crate::layout::{desk_walk_anchor, Layout, Point, WaypointKind};
use crate::pathfind::Router;

/// The per-frame routing engine state threaded through pose derivation,
/// character anchoring, hit-testing and label placement. `now`/`layout` stay
/// separate args (frame inputs, not engine state). `overlay` is shared (&) —
/// none of these fns mutate it; router/history/motion are &mut.
pub struct RouteCtx<'a> {
    pub router: &'a mut dyn Router,
    pub overlay: &'a OccupancyOverlay,
    pub history: &'a mut PoseHistory,
    pub motion: &'a mut HashMap<AgentId, MotionState>,
}

/// Per-agent rendered position cache. Updated each frame by
/// `derive_with_routing`, consulted on state transitions so an agent
/// who was mid-walk when their state flipped can complete the walk
/// visually instead of teleporting back to their desk.
#[derive(Debug, Default, Clone)]
pub struct PoseHistory {
    last: std::collections::HashMap<AgentId, (Point, SystemTime)>,
}

impl PoseHistory {
    pub fn new() -> Self {
        Self::default()
    }
    /// Record where an agent was visually placed this frame.
    pub fn record(&mut self, agent_id: AgentId, anchor: Point, now: SystemTime) {
        self.last.insert(agent_id, (anchor, now));
    }
    /// Drop entries for agents no longer in `scene` (mirrors
    /// `FrameCache::evict_missing`). Without this, one `(Point, SystemTime)`
    /// per AgentId ever rendered lived for the process lifetime, per floor.
    pub fn evict_missing(&mut self, scene: &pixtuoid_core::state::SceneState) {
        self.last.retain(|id, _| scene.agents.contains_key(id));
    }
    /// Whether an entry exists for `agent_id` (test seam for eviction checks).
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
/// state flip happened within this long. It is NOT a render cap — the walk runs
/// to completion by physics (`walk_arrived`), kept brisk by the snap-back's
/// higher accel (`physics::WALK_ACCEL_SNAPBACK`). Past this window we just show
/// the seated pose directly (too late to bother animating a return).
const SNAP_BACK_MS: u64 = 900;
/// Minimum manhattan distance (px) from current rendered position to
/// the desk before we bother animating the snap-back. Below this the
/// teleport is invisible and animating wastes a frame.
const SNAP_BACK_MIN_DIST: i32 = 8;
/// Max age (ms) for a recorded `PoseHistory` position to count as "where the
/// agent is now" — the start point for both the exit walk and the snap-back.
const HISTORY_RECENT_MS: u64 = 300;
/// Safety margin (ms) shaved off `EXIT_GRACE_WINDOW` so the time-compressed
/// exit walk reaches the door before the reducer GCs the slot. Coincides with
/// `HISTORY_RECENT_MS` in value but is independently motivated (GC-timing slack,
/// not a recency window) — the two may diverge, so they stay separate consts.
const EXIT_BUDGET_MARGIN_MS: u64 = 300;

/// The home desk's ARRIVAL target: a reachable cell on an ALLOWED side
/// (`DESK_APPROACH` = N/E/W, excluding the south front) via the SAME
/// `approach_point` the wander seats use — so an arriving agent walks AROUND to
/// sit behind the desk instead of straight through its front. The chair
/// (`desk_walk_anchor`) is inside the blocked desk footprint, so targeting it
/// directly made A\* fall back to a straight `door→chair` line THROUGH the desk
/// body (the "walk through the table" bug). `None` only in a degenerate layout
/// where every allowed side is walled off — the caller then falls back to the
/// old direct target. The chair is the SETTLE endpoint, appended after this.
///
/// Scans from the CHAIR, not the desk's top-left origin: the footprint is
/// anchored top-left, so a scan from the corner is lopsided and can't clear the
/// 16px-wide body to the EAST (the east side would read as walled-off). From the
/// chair (≈ footprint centre) all three allowed sides are within reach, so the
/// approach cell sits directly off the seat and the settle glide is a short
/// straight hop onto the chair.
pub(crate) fn desk_approach_cell(desk: Point, layout: &Layout) -> Option<Point> {
    use crate::layout::{approach_point, desk_walk_anchor, Facing, Furniture};
    let chair = desk_walk_anchor(desk);
    let cell = approach_point(
        Furniture::Desk,
        chair,
        // The desk sitter faces the camera (South); DESK_APPROACH then allows
        // N/E/W (the south front is excluded — that is the bug-prone side).
        Facing::South,
        layout.pantry_counter_size(),
        &layout.walkable,
        chair,
        &layout.reachable,
    );
    // approach_point returns the scanned `pos` (== chair) as the "no valid
    // approach" sentinel when no allowed+reachable side exists.
    (cell != chair).then_some(cell)
}

/// The desk-side endpoint of a desk-bound walk leg, resolved the ONE unified way
/// so no leg can regress to aiming A\* at the blocked chair. Used by EVERY leg
/// that arrives at or departs from the chair — entry, wander-out, wander-back,
/// the exit DEPARTURE, AND snap-back — so "approach via an allowed side, then
/// settle onto the chair" is defined exactly once.
///
/// Returns `(routing_endpoint, chair_settle)`:
///   * `routing_endpoint` — the cell to hand A\* as the leg's desk-side `from`/`to`
///     (a reachable N/E/W [`desk_approach_cell`], NEVER the chair: aiming A\* at
///     the blocked chair makes `find_path` snap to the *nearest* walkable cell —
///     the SOUTH front for a south-facing chair — so the agent arrives through
///     the desk front).
///   * `chair_settle` — `Some(chair)` to prepend/append via [`Settle`] (the short
///     glide on/off the seat the router never plans), or `None` in the degenerate
///     boxed-in layout where every allowed side is walled off and the leg reverts
///     to the direct chair target (resolved by `find_path`'s coarse-cell `snap`).
///
/// NOTE: snap-back is now a caller too (the urgent Idle→Active return routes via
/// the approach cell + settle like the rest, run by pure physics with a brisk
/// profile — no fixed-time compression). The ONLY non-caller is a mid-wander EXIT:
/// there the agent departs from its live wander position, not the chair.
pub(crate) fn desk_leg_endpoint(desk: Point, layout: &Layout) -> (Point, Option<Point>) {
    let chair = crate::layout::desk_walk_anchor(desk);
    match desk_approach_cell(desk, layout) {
        Some(approach) => (approach, Some(chair)),
        None => (chair, None),
    }
}

/// Routed variant of `derive`. For Walking poses, asks `router` for an
/// A*-routed polyline (composed against the layout's static mask + the
/// per-frame `overlay`) and converts the global t (0..1000) into a
/// per-segment Walking pose so the character traces the path
/// corner-by-corner instead of cutting through obstacles or other agents.
///
/// `motion` drives entry/exit physics: on first sighting an entering or
/// exiting agent the A* path length is snapshotted into a `WalkProfile`
/// (commit-to-route); subsequent frames compute `t_x1000` from
/// `walk_progress` against the frozen profile.
///
/// `history` is consulted on state transitions: if the agent's pose
/// flipped from a wander walk (or from AtWaypoint) to a desk-bound
/// pose (SeatedTyping / SeatedIdle / StandingAtDesk), we override the
/// instant teleport with a brief walk from the recorded previous
/// position to the desk.
pub fn derive_with_routing(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    rctx: &mut RouteCtx<'_>,
) -> Option<Pose> {
    // The four engine borrows stay behind `rctx`: direct `rctx.router` /
    // `rctx.overlay` / `rctx.history` / `rctx.motion` accesses are disjoint
    // field borrows, and `rctx` auto-reborrows (`&mut *rctx`) at each
    // `route_walking_pose` call — no per-branch `RouteCtx { .. }` rebuild.
    let desk = layout.home_desk(slot.desk_index.single_floor_local())?;

    // ---- EXIT branch -------------------------------------------------------
    // Takes priority over entry and state-driven poses.
    if let Some(exit_time) = slot.exiting_at {
        let Some(door_target) = layout.door_threshold else {
            // No door in this layout (very narrow terminal — Layout can
            // return door_threshold: None). Skip the physics exit walk and
            // let the reducer's grace window GC the slot. Returning None
            // here would make the exiting agent VANISH on its first frame
            // instead of holding at the desk; the old linear exit code
            // handled this gracefully too.
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

        // Snapshot the exit profile on first sighting.
        if mstate.exit.is_none() {
            // Start the exit from wherever the agent actually is: its current
            // wander position if it was out on a trip (fresh history), else the
            // desk anchor (the common case — exiting from a seated state).
            // Without this, an agent that's mid-coffee-run when its session
            // ends teleports back to the desk before walking to the door.
            let desk_anchor = desk_walk_anchor(desk);
            let from = rctx
                .history
                .recent(slot.agent_id, HISTORY_RECENT_MS, now)
                .unwrap_or(desk_anchor);
            // Exit is a desk DEPARTURE: when leaving the seated chair, rise off it
            // via the N/E/W approach cell so the walk to the (NE) door doesn't dip
            // SOUTH first — aiming A* from the blocked chair snaps it to the
            // nearest (south) cell, sending the agent the wrong way around the
            // desk. When already out on a wander trip, start from the live
            // position. The profile covers the chair-glide so duration matches.
            let (route_from, chair_rise) = if from == desk_anchor {
                desk_leg_endpoint(desk, layout)
            } else {
                (from, None)
            };
            // route_jittered restores the polyline's endpoint to the TRUE target,
            // so the measured octile length matches the rendered leg — the same
            // contract the wander legs use. A raw router.route(to_jittered) would
            // measure the ±jitter-perturbed polyline, a small duration/speed skew.
            let path = route_jittered(
                rctx.router,
                &layout.walkable,
                rctx.overlay,
                slot.agent_id,
                route_from,
                door_target,
            );
            // Rise off the chair (start settle); the door has no seat (end None).
            let path_len = measured_leg_len(&path, chair_rise, None);
            let profile = walk_profile(path_len, WalkIntent::Exit, slot.agent_id);
            // Store the ORIGIN (chair when a desk exit) so the render can detect
            // the desk-departure and re-derive the approach+settle.
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

        // Compress the exit walk so it REACHES the door before the reducer's
        // EXIT_GRACE_WINDOW reaps the slot. Physics exit duration for far/slow
        // desks can exceed 4500ms; without this the slot is GC'd mid-walk and
        // the sprite vanishes in the corridor instead of reaching the door.
        // (Entry has no such cap — nothing GCs an entering agent.)
        let exit_budget = (pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64)
            .saturating_sub(EXIT_BUDGET_MARGIN_MS);
        let eff_elapsed = if profile.duration_ms.saturating_add(profile.pause_ms) > exit_budget {
            (elapsed_ms.saturating_mul(profile.duration_ms) / exit_budget.max(1)).max(elapsed_ms)
        } else {
            elapsed_ms
        };

        // GC: walk fully done including pause → return None so the slot
        // disappears (same as old ENTRY_ANIMATION_MS gate).
        if walk_arrived(profile, eff_elapsed) {
            return None;
        }

        let t_x1000 = walk_progress(profile, eff_elapsed);
        let frame = walking_frame(eff_elapsed);

        // Desk departure: when the stored origin is the chair, rise off it via
        // the approach cell (matching the snapshotted profile + every other
        // desk-touching leg). Mid-wander exit starts straight from the live pos.
        let (from, exit_settle) = if stored_from == desk_walk_anchor(desk) {
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

    // ---- ENTRY branch ------------------------------------------------------
    // Gate: spawn window check reuses ENTRY_ANIMATION_MS only as a bound on
    // how long we try to route. Physics duration is the real walk time.
    let since_spawn = now
        .duration_since(slot.created_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    if let Some(door) = layout.door_threshold {
        // Unified ARRIVAL: walk to a reachable allowed-side cell (approach_point),
        // then Settle::End glides onto the chair — the SAME path the wander seats
        // use. Degenerate (every N/E/W side walled off): fall back to the old
        // direct target with no settle.
        let (approach, chair_settle) = desk_leg_endpoint(desk, layout);
        let settle = chair_settle.map_or(Settle::None, Settle::End);

        let mstate = rctx
            .motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        // Snapshot on first sighting if we're within the spawn window.
        if mstate.entry.is_none() && since_spawn < ENTRY_ANIMATION_MS {
            // route_jittered (endpoint restored) so the measured length matches
            // the rendered leg — same as the wander legs.
            let path = route_jittered(
                rctx.router,
                &layout.walkable,
                rctx.overlay,
                slot.agent_id,
                door,
                approach,
            );
            // Profile covers door→approach PLUS the short settle glide onto the chair
            // (end settle; the door start has no seat).
            let path_len = measured_leg_len(&path, None, chair_settle);
            let profile = walk_profile(path_len, WalkIntent::Entry, slot.agent_id);
            mstate.entry = Some((slot.created_at, profile));
        }

        if let Some((started_at, profile)) = mstate.entry {
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
                        from: door,
                        to: approach,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                    settle,
                );
            }
            // walk_arrived — fall through to state-driven pose (Correction C).
            // DO NOT call `derive()` here as that would re-fire the linear
            // entry override and cause a double-walk.
        }
    }

    // ---- WANDER DISPATCH (Idle agents whose entry walk is done) ------------
    // Reaching this line means the entry branch above already returned for any
    // in-flight entry walk, so the agent's entry is complete (arrived early for
    // near desks, or never started). Gate on Idle, NOT on `since_spawn >=
    // ENTRY_ANIMATION_MS` — that fixed 4000ms gate made a near-desk agent that
    // physically arrived in ~1s sit in core's fixed-fraction idle_pose until 4s
    // and then snap to physics wander. Drive `advance_wander` right away.
    // SeatedThinking still takes priority so the thinking-pose window is intact.
    let is_idle = matches!(slot.state, pixtuoid_core::state::ActivityState::Idle);
    if is_idle && slot.exiting_at.is_none() {
        // Check thinking-pose seam: if the agent recently finished active
        // work and is within the thinking window, return SeatedThinking now
        // regardless of wander phase.  This keeps the existing thinking-pose
        // behaviour entirely intact (no regression).
        let was_active = slot.last_event_at > slot.created_at;
        let since_last_event = now
            .duration_since(slot.last_event_at)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        if was_active && since_last_event < THINKING_WINDOW_SECS {
            // Thinking window active — return SeatedThinking directly.
            return Some(Pose::SeatedThinking);
        }

        let (wander_phase, t_phys) =
            advance_wander(slot, now, layout, rctx.router, rctx.overlay, rctx.motion);

        match wander_phase {
            WanderPhase::WalkingOut(_) => {
                let ms = rctx.motion.get(&slot.agent_id)?;
                let desk_point = layout.home_desk(slot.desk_index.single_floor_local())?;
                let dest = ms.wander.target.dest;
                let seat = ms.wander.target.kind.seat();
                // Leave the desk via the approach cell (a reachable N/E/W side),
                // never straight through the south front: `from` is the approach
                // cell and the chair is PREPENDED via Settle so the sprite first
                // glides off the seat. Mirrors the entry/walk-back legs — all
                // desk-touching legs share `desk_leg_endpoint`. The profile in
                // advance_wander adds the same chair-glide so duration matches.
                let (from, chair_settle) = desk_leg_endpoint(desk_point, layout);
                // Rise off the desk chair (start), glide onto the waypoint seat (end).
                let settle = settle_from_pair(chair_settle, seat);
                let elapsed_phase = crate::anim::elapsed_ms(now, ms.wander.phase_started_at);
                let frame = walking_frame(elapsed_phase);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    rctx,
                    Pose::Walking {
                        from,
                        to: dest,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee: false,
                    },
                    settle,
                );
            }
            WanderPhase::AtWaypoint(_) => {
                let ms = rctx.motion.get(&slot.agent_id)?;
                let pose = match ms.wander.target.kind {
                    WanderKind::Named { wp_idx, kind, .. } => Pose::AtWaypoint { wp: wp_idx, kind },
                    WanderKind::Aimless => Pose::AimlessAt {
                        dest: ms.wander.target.dest,
                    },
                };
                // Record the RENDERED position so snap-back/exit (which read
                // history.recent as their walk origin) start where the sprite
                // actually is: the seat cell for a seat waypoint (the sprite sits
                // ON the furniture, ~half_extent off `dest`), else `dest` (an
                // obstacle stands AT its approach cell, seat = None). Recording
                // `dest` for a seat popped the sprite ~10px to the off-side
                // approach cell on the first snap-back/exit frame — the one seated
                // departure the WalkingBack Settle machinery didn't cover.
                let pt = ms
                    .wander
                    .target
                    .kind
                    .seat()
                    .unwrap_or(ms.wander.target.dest);
                rctx.history.record(slot.agent_id, pt, now);
                return Some(pose);
            }
            WanderPhase::WalkingBack(_) => {
                let ms = rctx.motion.get(&slot.agent_id)?;
                let desk_point = layout.home_desk(slot.desk_index.single_floor_local())?;
                // Copy the fields off `ms` so the immutable `motion` borrow ends
                // before `route_walking_pose` takes `&mut motion`.
                let wander_dest = ms.wander.target.dest;
                let wander_phase_started_at = ms.wander.phase_started_at;
                let carrying_coffee = matches!(
                    ms.wander.target.kind,
                    WanderKind::Named {
                        kind: WaypointKind::Pantry,
                        ..
                    }
                );
                let seat = ms.wander.target.kind.seat();
                // Arrive at the desk via the approach cell (a reachable N/E/W
                // side), never up through the south front: `to` is the approach
                // cell and the chair is APPENDED via Settle so the sprite glides
                // onto the seat. The waypoint seat (if any) is the leg's START
                // settle (stand up off it). Shares `desk_leg_endpoint` with the
                // entry/walk-out legs; advance_wander adds the matching glide len.
                let (snap_target, chair_settle) = desk_leg_endpoint(desk_point, layout);
                // Rise off the waypoint seat (start), glide onto the desk chair (end).
                let settle = settle_from_pair(seat, chair_settle);
                let elapsed_phase = crate::anim::elapsed_ms(now, wander_phase_started_at);
                let frame = walking_frame(elapsed_phase);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    rctx,
                    Pose::Walking {
                        from: wander_dest,
                        to: snap_target,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee,
                    },
                    settle,
                );
            }
            WanderPhase::Seated => {
                // The tui motion machine is the wander authority: during its
                // Seated phase the agent is at its desk. Render SeatedIdle
                // DIRECTLY rather than falling through to derive_state_only —
                // that re-runs core's *stateless* wander (`idle_pose`), whose
                // independent fixed-fraction timeline can disagree with this
                // machine and return AtWaypoint/AimlessAt at an unrelated
                // location, teleporting the sprite between desk and waypoint
                // as the two clocks drift. (SeatedThinking is already handled
                // above, before advance_wander.)
                return Some(Pose::SeatedIdle);
            }
        }
    }

    // Went Active/Waiting: drop any exclusive-spot claim, else the frozen
    // `wander.target` blocks that spot all burst. Idempotent, non-Idle only, and
    // NOT reached by exits (see `motion::spot_claims` + the SpotClaims sharp edge).
    if let Some(ms) = rctx.motion.get_mut(&slot.agent_id) {
        ms.wander.target.kind = WanderKind::Aimless;
    }

    // ---- STATE-DRIVEN pose -------------------------------------------------
    // Use derive_state_only (not derive) to avoid re-triggering the linear
    // entry/exit overrides in core's derive() (Correction C — no double-walk).
    let raw = derive_state_only(slot, now, layout)?;

    // Snap-back override: state-driven poses (SeatedTyping etc.) at the
    // desk would teleport the agent if they were mid-wander when state
    // changed. Replace them with a Walking pose from the previous
    // rendered position over SNAP_BACK_MS.
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
        // ARM ONCE per state transition. The distance gate is checked ONLY when not
        // already armed for this `state_started_at`; once armed, the leg renders to
        // completion (by physics — `walk_arrived`) from the FROZEN origin. This is
        // what makes the override idempotent within a frame: `route_walking_pose`
        // records the advancing walker position into history every call, so a second
        // `derive` in the same frame sees a CLOSER `prev` — re-checking the distance
        // gate there would drop the agent back to Seated mid-walk (the K-call
        // desync). Keying the arm on `slot.state_started_at` (not `now`) lets a NEW
        // desk-bound transition re-arm with a fresh clock. `SNAP_BACK_MS` is now just
        // the ARM window (only snap-back for a RECENT flip) — NOT a render cap; the
        // render runs until the physics walk arrives, so it never teleports.
        let already_armed =
            matches!(&ms_entry.snap_back, Some(leg) if leg.started_at == slot.state_started_at);
        if !already_armed {
            // A snap_back here is STALE (a previous transition) — clear it, then arm
            // a fresh leg, but only for a recent flip (the arm window).
            ms_entry.snap_back = None;
            if since_state < SNAP_BACK_MS {
                if let Some(prev) = rctx.history.recent(slot.agent_id, HISTORY_RECENT_MS, now) {
                    // Distance to the CHAIR (where the agent actually sits), NOT the
                    // desk origin: the chair is offset (+6,+4) from the origin, so a
                    // desk-origin gate would re-fire forever once the agent settles ON
                    // the chair (10px from the origin ≥ MIN). Gating on the seat makes
                    // the snap-back stop the instant the walk reaches it.
                    let chair = desk_walk_anchor(desk);
                    let dist = (prev.x as i32 - chair.x as i32).abs()
                        + (prev.y as i32 - chair.y as i32).abs();
                    if dist >= SNAP_BACK_MIN_DIST {
                        // Snap-back joins the unified desk-leg path: route to the N/E/W
                        // approach cell and SETTLE onto the chair, so the correction
                        // arrives from an allowed side instead of the south front
                        // (aiming A* at the blocked chair would snap the goal to the
                        // nearest — south — cell). Route ONCE here at arm time —
                        // mirroring the entry/exit arms, same jittered goal as the
                        // render route — because the leg renders through
                        // route_walking_pose's A* polyline: the profile must measure
                        // that polyline (+ the chair-glide), or a detouring return
                        // traverses a longer path inside a straight-line duration
                        // (too fast). The arm stays once-per-transition; the rendered
                        // leg itself is still frozen (no per-frame re-route). Higher
                        // accel (WalkIntent::SnapBack → WALK_ACCEL_SNAPBACK) keeps the
                        // urgent return brisk under pure physics.
                        let (snap_target, chair_settle) = desk_leg_endpoint(desk, layout);
                        // route_jittered (endpoint restored) so the measured length
                        // matches the rendered leg — same as the wander legs.
                        let path = route_jittered(
                            rctx.router,
                            &layout.walkable,
                            rctx.overlay,
                            slot.agent_id,
                            prev,
                            snap_target,
                        );
                        // Glide onto the chair (end settle); `prev` start has no seat.
                        let len = measured_leg_len(&path, None, chair_settle);
                        let p = walk_profile(len, WalkIntent::SnapBack, slot.agent_id);
                        ms_entry.snap_back = Some(WalkLeg {
                            started_at: slot.state_started_at,
                            profile: p,
                            from: prev,
                        });
                    }
                }
            }
        }
        // Render the armed leg (idempotent — NO per-frame gate). A stale snap_back
        // (different `started_at`) fails the guard → `raw`; it is re-armed or cleared
        // on a later frame.
        match ms_entry.snap_back.clone() {
            Some(WalkLeg {
                started_at,
                profile,
                from: snap_prev,
            }) if started_at == slot.state_started_at => {
                let elapsed_ms = crate::anim::elapsed_ms(now, started_at);
                // PURE physics — no time-compression. Snap-back's higher accel
                // (WALK_ACCEL_SNAPBACK) keeps the urgent return brisk on its own, so
                // we render the eased walk to completion by `walk_arrived` rather than
                // compressing it into a fixed 900ms window. It never teleports because
                // physics drives the whole walk (a far snap-back is a real ~1s walk,
                // not a hard-compressed dash).
                if walk_arrived(&profile, elapsed_ms) {
                    // Completed + paused — clear so the next transition re-snapshots.
                    ms_entry.snap_back = None;
                    raw
                } else {
                    let t_x1000 = walk_progress(&profile, elapsed_ms);
                    let frame = walking_frame(elapsed_ms);
                    // Recompute the desk endpoint (deterministic) so the rendered leg
                    // matches the armed profile: route to the approach cell and SETTLE
                    // onto the chair (`desk_walk_anchor`, == seated_foot_cell(Desk), so
                    // the walk ends on the exact seated anchor — no transition flash),
                    // like every other desk leg. Degenerate (no approach) → direct.
                    let (snap_target, chair_settle) = desk_leg_endpoint(desk, layout);
                    final_settle = chair_settle.map_or(Settle::None, Settle::End);
                    // Walk from the FROZEN origin captured when the leg armed, not the
                    // per-frame `prev`: route_walking_pose re-records the advancing
                    // walker position into history every frame, so reading it back as
                    // the origin would creep `from` toward the desk and break the
                    // walk_path freeze's `wp.from == from` reuse guard. Mirrors exit.
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
        // Hard wall: clear any stale snap-back profile so the next state transition
        // gets a fresh snapshot rather than replaying a previous one.
        if let Some(ms) = rctx.motion.get_mut(&slot.agent_id) {
            if ms.snap_back.is_some() {
                ms.snap_back = None;
            }
        }
        raw
    };

    route_walking_pose(slot, now, layout, rctx, pose, final_settle)
}

/// Apply A*-based polyline routing to a `Pose::Walking`, recording
/// history with `now`. For non-Walking poses, records waypoint/aimless
/// positions to history and returns `Some(pose)`.
///
/// This is the single shared helper for entry, exit, snap-back, and
/// state-driven walks (Correction B). Records history with `now` (not
/// `slot.last_event_at`) so snap-back lookups are fresh.
/// How a walk leg extends its polyline onto a seat — a short terminal motion the
/// A* router never plans (the seat cell may be blocked). `End` = sit down on
/// arrival (append the seat); `Start` = stand up on departure (prepend it);
/// `Both` = a leg that BOTH rises off one seat and glides onto another (a
/// wander-out rises off the desk chair then sits on the waypoint seat; a
/// wander-back rises off the waypoint seat then glides onto the desk chair).
/// Makes walk-end ≡ render-feet so seat arrival/departure don't pop.
#[derive(Clone, Copy)]
enum Settle {
    None,
    End(Point),
    Start(Point),
    Both { start: Point, end: Point },
}

/// Collapse a leg's `(start_settle, end_settle)` pair into a single [`Settle`].
/// The wander legs both rise off one seat and glide onto another, so the
/// presence of each end decides `Both`/`Start`/`End`/`None`. Argument order
/// encodes direction: `start` = the seat to rise OFF (prepended), `end` = the
/// seat to glide ONTO (appended).
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
        // Not walking any more — drop the frozen leg path so the next walk
        // re-snapshots a fresh polyline.
        if let Some(ms) = motion.get_mut(&slot.agent_id) {
            ms.walk_path = None;
        }
        // Record AtWaypoint / AimlessAt positions too — they're a valid
        // "previous position" for a subsequent snap-back walk.
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
    // (from, to) leg appears, then reuse it unchanged until the endpoints
    // change. Without this, per-frame occupancy-overlay churn invalidates the
    // A* cache and re-routes the walker onto a differently-shaped path, making
    // the frozen-profile progress `t` land on a new pixel — the visible
    // "flash"/teleport. Re-routing every frame is also what spikes a frame's
    // A* cost (the periodic stutter). See [`WalkPathSnapshot`].
    //
    // Per-agent path personality: perturb the routing destination by a few
    // pixels hashed from the agent_id so converging agents take visibly
    // different polylines (breaks the "ant trail"). The last polyline point is
    // restored to the true `to` so the walker ends at the canonical
    // destination, not the jittered approximation.
    let path = {
        let ms = motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));
        match &ms.walk_path {
            Some(wp) if wp.from == from && wp.to == to => wp.path.clone(),
            _ => {
                let mut p =
                    route_jittered(router, &layout.walkable, overlay, slot.agent_id, from, to);
                // Settle: extend the polyline onto/off the seat (terminal "sit
                // down" / "stand up" the router never plans). walk-end ≡ render.
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
                // Only freeze genuinely CORNERED routes (>2 points). A straight
                // 2-point walk has no interior corners to remap `t` onto, so it
                // can't flash — and re-routing it each frame is cheap AND
                // self-healing: if A* transiently fell back to a straight
                // `[from, to]` (find_path returned None this frame), freezing it
                // would stick that "walk through walls" for the whole leg;
                // leaving it unfrozen lets the next frame recover the real route.
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
        // Straight-line walk — record the interpolated position for next
        // frame's snap-back lookup. Use `now` not `last_event_at`.
        history.record(slot.agent_id, walking_position(from, to, t_x1000), now);
        return Some(Pose::Walking {
            from,
            to,
            t_x1000,
            frame,
            carrying_coffee,
        });
    }
    // Map global t to a (segment_idx, t_within_segment) using cumulative
    // octile distance — same metric A* used to plan the path, so timing
    // stays uniform along diagonals.
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
            // Record the walker's current position for the next frame's
            // snap-back lookup. Use `now` not `last_event_at`.
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
    // Past the last segment — snap to final.
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
    use crate::pathfind::{OCTILE_DIAGONAL_COST, OCTILE_STRAIGHT_COST};
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    OCTILE_DIAGONAL_COST * dx.min(dy) + OCTILE_STRAIGHT_COST * (dx.max(dy) - dx.min(dy))
}

/// Half-range (px) of the per-agent destination jitter — the offset spans
/// `-JITTER_MAX_PX..=JITTER_MAX_PX`, so the modulus span is `2*MAX+1`.
const JITTER_MAX_PX: i32 = 4;
const JITTER_SPAN: u64 = (2 * JITTER_MAX_PX + 1) as u64;

/// Per-agent ±4px routing-destination jitter, hashed from the agent_id, so
/// converging agents take visibly different polylines (breaks the "ant trail")
/// — the entry/exit/snap-back walk targets, the wander walk-path freeze, and
/// the motion-side wander profile snapshots (via [`route_jittered`]) all
/// perturb the GOAL the same way. Output must stay bit-identical across every
/// call site (same hash, same `saturating_add_signed`) — a site routing the
/// RAW goal measures a differently-shaped polyline than the one rendered AND
/// mints a second router-cache key per leg.
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
/// polyline's endpoint to the true `to` — the routing call shared by EVERY walk
/// leg: the render-side walk-path freeze ([`route_walking_pose`]), the wander
/// profile snapshots (`advance_wander` / `snapshot_back_profile`), AND the
/// one-shot entry/exit/snap-back profile snapshots (which measure the restored
/// polyline so their duration matches the rendered leg), so the rendered shape,
/// the measured profile length, and the router-cache key can't diverge (the
/// [`jitter_dest`] lockstep contract).
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
