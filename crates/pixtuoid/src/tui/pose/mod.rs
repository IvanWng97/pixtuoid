//! TUI-side pose layer.
//!
//! Re-exports the pure pose-derivation surface from `pixtuoid_core::pose`
//! and adds the binary-side machinery:
//!   * `PoseHistory` — per-agent cache of the last rendered position.
//!   * `derive_with_routing` — the routed variant of `derive` that consults
//!     a `&mut dyn Router` so walking poses follow A*-routed polylines and
//!     so state transitions are smoothed with a snap-back walk instead of
//!     teleporting back to the desk.
//!
//! Keeping the routed code on this side means `pixtuoid-core` does not
//! depend on the pathfinder — the trait lives in the binary because A* is
//! TUI-rendering-adjacent and may differ for non-terminal renderers.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::physics::{walk_arrived, walk_profile, walk_progress, WalkIntent};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::tui::motion::{
    advance_wander, octile_path_len, MotionState, WalkPathSnapshot, WanderPhase,
};

pub use pixtuoid_core::pose::{
    aimless_wander_seed, cycle_ms_for, derive, derive_state_only, dwell_ms, est_wander_cycle_ms,
    is_aimless_cycle, personality_for, pick_aimless_dest, seated_dwell_ms, takes_trip,
    waypoint_index_for_cycle, Personality, Pose, ENTRY_ANIMATION_MS, THINKING_WINDOW_SECS,
    TYPING_FRAMES, TYPING_FRAME_MS, WALKING_FRAMES, WALKING_FRAME_MS, WANDER_CYCLE_BASE_MS,
    WANDER_CYCLE_RANGE_MS, WANDER_DWELL_EST_MS, WANDER_WALK_EST_MS,
};

use crate::tui::layout::{desk_walk_anchor, Layout, Point, WaypointKind};
use crate::tui::pathfind::Router;

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

/// Duration of the snap-back walk used when state-driven pose would
/// instantly place the agent back at their desk. 600ms is short enough
/// to feel responsive (the user wants to see the tool fire) but long
/// enough to read as motion, not a pop.
const SNAP_BACK_MS: u64 = 900;
/// Minimum manhattan distance (px) from current rendered position to
/// the desk before we bother animating the snap-back. Below this the
/// teleport is invisible and animating wastes a frame.
const SNAP_BACK_MIN_DIST: i32 = 8;

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
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
    motion: &mut HashMap<AgentId, MotionState>,
) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;

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
                Pose::Walking { .. } => route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    motion,
                    raw,
                    Settle::None,
                ),
                other => Some(other),
            };
        };

        let mstate = motion
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
            let from = history
                .recent(slot.agent_id, 300, now)
                .unwrap_or(desk_anchor);
            let h = slot.agent_id.raw();
            let jx = ((h % 9) as i32 - 4) as i16;
            let jy = (((h >> 16) % 9) as i32 - 4) as i16;
            let to_jittered = Point {
                x: door_target.x.saturating_add_signed(jx),
                y: door_target.y.saturating_add_signed(jy),
            };
            let path = router.route(&layout.walkable, overlay, from, to_jittered);
            let path_len = octile_path_len(&path).max(1);
            let profile = walk_profile(path_len, WalkIntent::Exit, slot.agent_id);
            mstate.exit = Some((exit_time, profile, from));
        }

        // Destructure without moving the non-Copy profile (Correction L).
        let e = mstate.exit.as_ref()?;
        let started_at = e.0;
        let profile = &e.1;
        let from = e.2;

        let elapsed_ms = now
            .duration_since(started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;

        // Compress the exit walk so it REACHES the door before the reducer's
        // EXIT_GRACE_WINDOW reaps the slot. Physics exit duration for far/slow
        // desks can exceed 4500ms; without this the slot is GC'd mid-walk and
        // the sprite vanishes in the corridor instead of reaching the door.
        // (Entry has no such cap — nothing GCs an entering agent.)
        let exit_budget = (pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64)
            .saturating_sub(300);
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
        let frame = ((eff_elapsed / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;

        return route_walking_pose(
            slot,
            now,
            layout,
            router,
            overlay,
            history,
            motion,
            Pose::Walking {
                from,
                to: door_target,
                t_x1000,
                frame,
                carrying_coffee: false,
            },
            Settle::None,
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
        let mstate = motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        // Snapshot on first sighting if we're within the spawn window.
        if mstate.entry.is_none() && since_spawn < ENTRY_ANIMATION_MS {
            let to_desk = desk_walk_anchor(desk);
            let h = slot.agent_id.raw();
            let jx = ((h % 9) as i32 - 4) as i16;
            let jy = (((h >> 16) % 9) as i32 - 4) as i16;
            let to_jittered = Point {
                x: to_desk.x.saturating_add_signed(jx),
                y: to_desk.y.saturating_add_signed(jy),
            };
            let path = router.route(&layout.walkable, overlay, door, to_jittered);
            let path_len = octile_path_len(&path).max(1);
            let profile = walk_profile(path_len, WalkIntent::Entry, slot.agent_id);
            mstate.entry = Some((slot.created_at, profile));
        }

        if let Some((started_at, ref profile)) = mstate.entry.clone() {
            let elapsed_ms = now
                .duration_since(started_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;

            if !walk_arrived(profile, elapsed_ms) {
                let t_x1000 = walk_progress(profile, elapsed_ms);
                let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                let to_desk = desk_walk_anchor(desk);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    motion,
                    Pose::Walking {
                        from: door,
                        to: to_desk,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                    Settle::None,
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

        let (wander_phase, t_phys) = advance_wander(slot, now, layout, router, overlay, motion);

        match wander_phase {
            WanderPhase::WalkingOut => {
                let ms = motion.get(&slot.agent_id)?;
                let desk_point = *layout.home_desks.get(slot.desk_index)?;
                let dest = ms.wander_dest;
                let seat = ms.wander_seat;
                // Walk-out starts at desk+(6,4) (the seated anchor) so there's
                // no stand-up jump; symmetric with walk-back's snap_target.
                // This intentionally differs from core::idle_pose's raw
                // `from: desk` (only the routed TUI path is user-visible) and
                // matches the profile routed from desk+(6,4) in advance_wander.
                let from = desk_walk_anchor(desk_point);
                let elapsed_phase = now
                    .duration_since(ms.wander_phase_started_at)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64;
                let frame = (elapsed_phase / WALKING_FRAME_MS) as usize % WALKING_FRAMES;
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    motion,
                    Pose::Walking {
                        from,
                        to: dest,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee: false,
                    },
                    // Settle from the approach point onto the seat (sit down).
                    seat.map_or(Settle::None, Settle::End),
                );
            }
            WanderPhase::AtWaypoint => {
                let ms = motion.get(&slot.agent_id)?;
                let pose = if let (Some(wp_idx), Some(kind)) =
                    (ms.wander_dest_wp_idx, ms.wander_dest_kind)
                {
                    Pose::AtWaypoint { wp: wp_idx, kind }
                } else {
                    Pose::AimlessAt {
                        dest: ms.wander_dest,
                    }
                };
                // Record history so snap-back works if state changes now.
                let pt = ms.wander_dest;
                history.record(slot.agent_id, pt, now);
                return Some(pose);
            }
            WanderPhase::WalkingBack => {
                let ms = motion.get(&slot.agent_id)?;
                let desk_point = *layout.home_desks.get(slot.desk_index)?;
                // Copy the fields off `ms` so the immutable `motion` borrow ends
                // before `route_walking_pose` takes `&mut motion`.
                let wander_dest = ms.wander_dest;
                let wander_phase_started_at = ms.wander_phase_started_at;
                let carrying_coffee = ms.wander_dest_kind == Some(WaypointKind::Pantry);
                let seat = ms.wander_seat;
                // Endpoint is desk+(6,4) to match seated_anchor so there's no
                // jump on arrival; this intentionally differs from
                // core::idle_pose's raw `to: desk` (only the routed TUI path is
                // user-visible).
                let snap_target = desk_walk_anchor(desk_point);
                let elapsed_phase = now
                    .duration_since(wander_phase_started_at)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64;
                let frame = (elapsed_phase / WALKING_FRAME_MS) as usize % WALKING_FRAMES;
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    motion,
                    Pose::Walking {
                        from: wander_dest,
                        to: snap_target,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee,
                    },
                    // Rise off the seat before walking back (stand up).
                    seat.map_or(Settle::None, Settle::Start),
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
    let since_state = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    let pose = if desk_pose && since_state < SNAP_BACK_MS {
        if let Some(prev) = history.recent(slot.agent_id, 300, now) {
            let dist =
                (prev.x as i32 - desk.x as i32).abs() + (prev.y as i32 - desk.y as i32).abs();
            if dist >= SNAP_BACK_MIN_DIST {
                // Walk-end target is offset (+6, +4) from the desk pixel so
                // walking_anchor(target) lands on the SAME sprite anchor
                // that seated_anchor(desk) would. Without this offset the
                // sprite jumps ~6 px right + 4 px down at the moment the
                // pose flips from Walking → SeatedTyping. The agent ends
                // visually AT the desk (anchor-equivalent), so there's no
                // perceivable transition flash.
                let snap_target = desk_walk_anchor(desk);
                // Retrieve or snapshot the physics profile for this snap-back.
                // Re-arm when EITHER there's no profile yet OR a NEW state
                // transition began (stored `started_at != slot.state_started_at`):
                // a second desk-bound transition within the 900ms window would
                // otherwise reuse the stale T0 clock and jump mid-progress. The
                // stored key is `slot.state_started_at` (not `now`) so the elapsed
                // clock and the re-arm key agree. A fresh HashMap each call (tests)
                // still snapshots on first sight — `snap_back` is None.
                let ms_entry = motion
                    .entry(slot.agent_id)
                    .or_insert_with(|| MotionState::new(slot.agent_id));
                let needs_arm = match &ms_entry.snap_back {
                    Some((started_at, _, _)) => *started_at != slot.state_started_at,
                    None => true,
                };
                if needs_arm {
                    let path = [prev, snap_target];
                    let len = octile_path_len(&path);
                    let p = walk_profile(len, WalkIntent::SnapBack, slot.agent_id);
                    ms_entry.snap_back = Some((slot.state_started_at, p, prev));
                }
                // Clone out (releases the borrow) so we can clear snap_back below.
                // No `unwrap`: `needs_arm` set Some above, or the match at line
                // ~408 witnessed Some — a None here is unreachable, but fall back
                // to the seated pose gracefully rather than panic (CLAUDE.md: no
                // unwrap in non-test code).
                match ms_entry.snap_back.clone() {
                    None => raw,
                    Some((started_at, profile, snap_prev)) => {
                        let elapsed_ms = now
                            .duration_since(started_at)
                            .unwrap_or(Duration::ZERO)
                            .as_millis() as u64;
                        // Time-compress so the eased walk COMPLETES by the 900ms
                        // responsive window. Snap-back distances routinely exceed
                        // the ~13px the physics finishes within 900ms; without this
                        // the walk is cut off mid-path by the window guard and the
                        // sprite teleports the remaining distance to the desk.
                        let eff_elapsed = if profile.duration_ms > SNAP_BACK_MS {
                            (elapsed_ms.saturating_mul(profile.duration_ms) / SNAP_BACK_MS)
                                .max(elapsed_ms)
                        } else {
                            elapsed_ms
                        };
                        if walk_arrived(&profile, eff_elapsed) {
                            // Short snaps complete + pause before the window edge —
                            // clear so the next state transition re-snapshots fresh.
                            ms_entry.snap_back = None;
                            raw
                        } else {
                            let t_x1000 = walk_progress(&profile, eff_elapsed);
                            let frame =
                                ((eff_elapsed / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                            // Walk from the FROZEN origin captured when the leg
                            // armed, not the per-frame `prev`. route_walking_pose
                            // re-records the advancing walker position into the
                            // single-slot history every frame, so reading it back
                            // as the origin would creep `from` toward the desk
                            // (a contraction that finishes ahead of the frozen
                            // physics profile and breaks the walk_path freeze's
                            // `wp.from == from` reuse guard). Mirrors the exit
                            // branch, which likewise freezes its origin Point.
                            Pose::Walking {
                                from: snap_prev,
                                to: snap_target,
                                t_x1000,
                                frame,
                                carrying_coffee: false,
                            }
                        }
                    }
                }
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        // Hard wall: clear any stale snap-back profile so the next state
        // transition gets a fresh snapshot rather than replaying a previous one.
        if let Some(ms) = motion.get_mut(&slot.agent_id) {
            if ms.snap_back.is_some() {
                ms.snap_back = None;
            }
        }
        raw
    };

    route_walking_pose(
        slot,
        now,
        layout,
        router,
        overlay,
        history,
        motion,
        pose,
        Settle::None,
    )
}

/// Apply A*-based polyline routing to a `Pose::Walking`, recording
/// history with `now`. For non-Walking poses, records waypoint/aimless
/// positions to history and returns `Some(pose)`.
///
/// This is the single shared helper for entry, exit, snap-back, and
/// state-driven walks (Correction B). Records history with `now` (not
/// `slot.last_event_at`) so snap-back lookups are fresh.
#[allow(clippy::too_many_arguments)]
/// How a walk leg extends its polyline onto a seat — a short terminal motion the
/// A* router never plans (the seat cell may be blocked). `End` = sit down on
/// arrival (append the seat); `Start` = stand up on departure (prepend it).
/// Makes walk-end ≡ render-feet so seat arrival/departure don't pop.
#[derive(Clone, Copy)]
enum Settle {
    None,
    End(Point),
    Start(Point),
}

#[allow(clippy::too_many_arguments)]
fn route_walking_pose(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
    motion: &mut HashMap<AgentId, MotionState>,
    pose: Pose,
    settle: Settle,
) -> Option<Pose> {
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
                let h = slot.agent_id.raw();
                let jx = ((h % 9) as i32 - 4) as i16;
                let jy = (((h >> 16) % 9) as i32 - 4) as i16;
                let to_jittered = Point {
                    x: to.x.saturating_add_signed(jx),
                    y: to.y.saturating_add_signed(jy),
                };
                let mut p = router.route(&layout.walkable, overlay, from, to_jittered);
                if let Some(last) = p.last_mut() {
                    *last = to;
                }
                // Settle: extend the polyline onto/off the seat (terminal "sit
                // down" / "stand up" the router never plans). walk-end ≡ render.
                match settle {
                    Settle::End(s) if p.last() != Some(&s) => p.push(s),
                    Settle::Start(s) if p.first() != Some(&s) => p.insert(0, s),
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

/// Pure linear interpolation along the segment from `from` to `to`. The
/// rendering side has its own `walking_position` in renderer.rs that
/// also applies vertical breathing; this one is for history-tracking
/// only (we want the deterministic position, not the breath offset).
use crate::tui::pixel_painter::walking_position;

pub(in crate::tui) fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}

#[cfg(test)]
mod tests;
