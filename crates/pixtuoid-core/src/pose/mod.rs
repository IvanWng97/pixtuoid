//! Pure state → pose derivation. Lives in core so non-TUI renderers
//! (snapshot tooling, future PNG/GIF capture, web canvas) get identical
//! pose semantics without depending on the binary crate.
//!
//! `derive(slot, now, layout)` is a function of the snapshot inputs only —
//! no routing, no per-frame history. The routed variant (which composes
//! against a `Router` and a `PoseHistory` cache) is `tui::pose` on the
//! binary side, since the router is terminal-rendering-adjacent.
//!
//! Variation knobs:
//!  * `cycle_ms_for(agent_id)` — per-agent wander cycle length so the office
//!    stops feeling clockwork-synchronized. Range 7..13s.
//!  * Waypoint choice XORs `agent_id` with the current cycle number, so each
//!    cycle the same agent picks a (likely) different waypoint.

use std::time::{Duration, SystemTime};

use crate::layout::{
    desk_furniture_def, desk_walk_anchor, furniture_def, Bounds, DwellWindow, Point, SceneLayout,
    WaypointKind,
};
use crate::state::{ActivityState, AgentSlot};
use crate::AgentId;

/// How long after the last event an Idle agent stays in the "thinking"
/// pose (seated, awake, no z's) before entering the wander/sleep cycle.
/// 20s covers typical CC thinking pauses between tool bursts.
///
/// `pub` so `tui::pose::derive_with_routing`'s wander dispatch references the
/// same window — otherwise the thinking gate could silently drift between
/// core's `derive` and the tui-side dispatch.
pub const THINKING_WINDOW_SECS: u64 = 20;

/// Base cycle length used only as the stale-resume / off-screen-gap sentinel
/// in `tui::motion::advance_wander` (a few seconds, above on-screen frame
/// cadence and below a floor-switch-away gap). NOT the wander dwell anymore —
/// see `dwell_ms` / `seated_dwell_ms` for the absolute per-spot timeline.
pub const WANDER_CYCLE_BASE_MS: u64 = 7_000;
/// Maximum extra time added per agent — jitter range is `[0, RANGE)`.
pub const WANDER_CYCLE_RANGE_MS: u64 = 6_000;

/// Stateless-overlay wander-timeline estimates. The render authority
/// (`tui::motion::advance_wander`) drives the at-waypoint beat with the
/// per-spot `dwell_ms`; `idle_pose` only needs an approximate timeline to
/// place the occupancy overlay, so it uses these fixed estimates plus a
/// constant per-agent cycle period (`est_wander_cycle_ms`). Exact coherence
/// with the routed timeline is impossible — core has no router and walk legs
/// are physics-timed only in the tui path — and #62's frozen leg paths make
/// approximate overlay timing safe (a new leg's shape is snapshotted once).
pub const WANDER_WALK_EST_MS: u64 = 3_500;
pub const WANDER_DWELL_EST_MS: u64 = 18_000;

/// Frame-cycle period for animated poses.
pub const TYPING_FRAME_MS: u64 = 140;
pub const WALKING_FRAME_MS: u64 = 220;
pub const TYPING_FRAMES: usize = 2;
pub const WALKING_FRAMES: usize = 2;

/// Spawn-window guard for entry routing in `tui::pose::derive_with_routing`.
/// After `physics::walk_profile` took over motion timing this constant is no
/// longer used to compute walk duration — it is only the *upper bound* on the
/// time window during which the tui layer will attempt to route an entry walk
/// and (via `FloorCtx::door_anim_max_ms`) drive door-open cosmetics. The
/// actual walk completes when `physics::walk_arrived` returns true.
pub const ENTRY_ANIMATION_MS: u64 = 4000;

/// Deterministic wander-cycle length for one agent. Each agent picks a
/// different speed so walkers don't move in lockstep.
pub fn cycle_ms_for(agent_id: AgentId) -> u64 {
    WANDER_CYCLE_BASE_MS + (agent_id.raw() >> 16) % WANDER_CYCLE_RANGE_MS
}

/// Base dwell plus deterministic per-agent jitter within `window`. The `tag` is
/// NOT a cryptographic salt — it just disambiguates the two callers (`dwell_ms`
/// vs `seated_dwell_ms`) so their jitter is decorrelated from each other and
/// from `speed_mult` / `pause_ms` / `cycle_ms` (which slice raw id bits). No
/// security relevance.
fn jittered_dwell(window: DwellWindow, agent_id: AgentId, tag: u64) -> u64 {
    let DwellWindow { base_ms, range_ms } = window;
    base_ms + crate::id::splitmix64(agent_id.raw() ^ tag) % range_ms.max(1)
}

/// Absolute dwell (ms) an agent lingers at a waypoint, per spot kind, with
/// per-agent jitter. A sofa / meeting seat is a long lounge; a vending grab
/// is quick. The render authority (`tui::motion::advance_wander`) uses this
/// for the AtWaypoint beat.
pub fn dwell_ms(kind: WaypointKind, agent_id: AgentId) -> u64 {
    jittered_dwell(
        furniture_def(kind.furniture()).dwell,
        agent_id,
        0xd1b5_4a32_d192_ed03,
    )
}

/// Absolute dwell (ms) an agent sits at its desk between wander trips.
pub fn seated_dwell_ms(agent_id: AgentId) -> u64 {
    // Single source: the desk's own FurnitureDef.dwell (no separate constant).
    jittered_dwell(desk_furniture_def().dwell, agent_id, 0x9e37_79b9_7f4a_7c15)
}

/// Estimated full wander-cycle wall-time for an agent (desk dwell + two walk
/// legs + one waypoint dwell). Used by `idle_pose` (stateless overlay) for
/// `cycle_n` / `phase_t` and by `advance_wander`'s bootstrap fast-forward, so
/// both place a long-idle agent on the same approximate cycle. Approximate —
/// the real cycle is physics-timed and per-spot.
pub fn est_wander_cycle_ms(agent_id: AgentId) -> u64 {
    seated_dwell_ms(agent_id) + 2 * WANDER_WALK_EST_MS + WANDER_DWELL_EST_MS
}

/// Per-agent wander personality derived from the agent's id hash.
/// Controls how often the agent leaves their desk and whether they prefer
/// aimless wandering vs heading to a named lounge waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Personality {
    /// Probability (in percent) that this agent takes a trip on any given
    /// cycle. Range: 25..=60. Restless agents wander more.
    pub trip_chance_pct: u8,
    /// Probability (in percent) that a trip is aimless wander vs heading to
    /// a lounge waypoint. Range: 0..=50.
    pub aimless_pref_pct: u8,
}

pub fn personality_for(agent_id: AgentId) -> Personality {
    let h = agent_id.raw();
    Personality {
        trip_chance_pct: (25 + (h % 36)) as u8,  // 25..=60
        aimless_pref_pct: ((h >> 8) % 51) as u8, // 0..=50
    }
}

/// Deterministic per-(agent, cycle) decision: does this agent take a
/// wander trip on this cycle, or stay seated? Trip frequency is driven by
/// per-agent Personality so different agents wander at different rates.
pub fn takes_trip(agent_id: AgentId, cycle_n: u64) -> bool {
    let p = personality_for(agent_id);
    let mix = agent_id.raw() ^ cycle_n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (mix % 100) < p.trip_chance_pct as u64
}

/// Per-(agent, cycle) decision: when the agent takes a trip, is it an
/// aimless wander (random walkway point) or a directed visit to a named
/// waypoint? Used by `idle_pose` AND by the snapshot example to find
/// agent_ids whose cycle deterministically lands at a target waypoint.
pub fn is_aimless_cycle(agent_id: AgentId, cycle_n: u64) -> bool {
    let p = personality_for(agent_id);
    let type_mix = agent_id.raw() ^ cycle_n.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    (type_mix % 100) < p.aimless_pref_pct as u64
}

/// Per-(agent, cycle) waypoint index. Only meaningful when `takes_trip` is
/// true AND `is_aimless_cycle` is false. Returns 0 if `num_waypoints` is 0.
pub fn waypoint_index_for_cycle(agent_id: AgentId, cycle_n: u64, num_waypoints: usize) -> usize {
    if num_waypoints == 0 {
        return 0;
    }
    ((agent_id.raw() ^ cycle_n) as usize) % num_waypoints
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    SeatedIdle,
    /// Seated at desk, awake but not typing. Used when the agent
    /// recently finished a tool call and the LLM is likely thinking.
    SeatedThinking,
    SeatedTyping {
        frame: usize,
    },
    StandingAtDesk,
    /// At a lounge waypoint. Concrete render depends on the kind:
    ///   Couch    → sit on couch sprite
    ///   Coffee   → standing + holding-coffee sprite
    ///   Others   → plain standing
    AtWaypoint {
        wp: usize,
        kind: WaypointKind,
    },
    Walking {
        from: Point,
        to: Point,
        t_x1000: u16,
        frame: usize,
        carrying_coffee: bool,
    },
    /// Standing at a random walkway point (not at any waypoint). The dest field
    /// is the buf-pixel target the agent walked to. Used by aimless wander.
    AimlessAt {
        dest: Point,
    },
}

/// Returns `None` if the slot's desk_index is out of range for `layout`.
///
/// Priority chain (first match wins, walks down):
///   1. **Exit override** — slot has `exiting_at` set → Walking to door.
///      Once the exit window passes, returns `None` (slot will be GC'd).
///   2. **Entry override** — `now - created_at < ENTRY_ANIMATION_MS` →
///      Walking from door to desk. Plays for the first 4 s of the slot's
///      life regardless of state changes.
///   3. **State-driven pose** — Active → SeatedTyping, Waiting →
///      StandingAtDesk, Idle → idle_pose (which itself decides between
///      SeatedIdle / Walking / AtWaypoint / AimlessAt based on the
///      wander state machine).
pub fn derive(slot: &AgentSlot, now: SystemTime, layout: &SceneLayout) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;

    // Exit takes priority — once SessionEnd fires we always walk to the
    // door regardless of entry-window or normal state. Use door_threshold
    // (on-floor point below the door) as the walk target so the character
    // doesn't paint through the wall trim.
    if let (Some(exit_time), Some(target)) = (slot.exiting_at, layout.door_threshold) {
        let since_exit = now
            .duration_since(exit_time)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        if since_exit < ENTRY_ANIMATION_MS {
            return Some(linear_walk_pose(since_exit, desk_walk_anchor(desk), target));
        }
        // Past exit window: nothing to render, slot will be GC'd shortly.
        return None;
    }

    // Entry animation overrides everything for the first ENTRY_ANIMATION_MS
    // after creation — agent walks in from the door threshold to their desk.
    // Target is offset (+6, +4) from the desk top-left so the walk ends at
    // the seated anchor position, not inside the desk obstacle. Without this
    // the A* router detours around the desk and always approaches from one side.
    if let Some(from) = layout.door_threshold {
        let since_spawn = now
            .duration_since(slot.created_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        if since_spawn < ENTRY_ANIMATION_MS {
            return Some(linear_walk_pose(since_spawn, from, desk_walk_anchor(desk)));
        }
    }

    state_driven_pose(slot, desk, layout, now)
}

/// The shared `Walking` pose for the stateless entry/exit overrides: a
/// LINEAR (not physics-timed) interpolation over `ENTRY_ANIMATION_MS`. This
/// is deliberately distinct from the tui motion path's kinematic profiles —
/// the overlay/snapshot path stays linear so it has no per-frame history.
fn linear_walk_pose(since_ms: u64, from: Point, to: Point) -> Pose {
    let t = (since_ms * 1000 / ENTRY_ANIMATION_MS).min(1000) as u16;
    let frame = ((since_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
    Pose::Walking {
        from,
        to,
        t_x1000: t,
        frame,
        carrying_coffee: false,
    }
}

/// The state→pose tail shared by `derive` and `derive_state_only`: maps
/// `slot.state` (relative to `state_started_at`) to the animated pose,
/// AFTER each caller has applied its own override guards and resolved
/// `desk`. Keeping this in one place prevents the two entry points from
/// drifting (e.g. divergent thinking-window or frame-counter logic).
fn state_driven_pose(
    slot: &AgentSlot,
    desk: Point,
    layout: &SceneLayout,
    now: SystemTime,
) -> Option<Pose> {
    let elapsed = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    match &slot.state {
        ActivityState::Active { .. } => {
            let frame = ((elapsed / TYPING_FRAME_MS) as usize) % TYPING_FRAMES;
            Some(Pose::SeatedTyping { frame })
        }
        ActivityState::Waiting { .. } => Some(Pose::StandingAtDesk),
        ActivityState::Idle => {
            let was_active = slot.last_event_at > slot.created_at;
            let since_last_event = now
                .duration_since(slot.last_event_at)
                .unwrap_or(Duration::ZERO)
                .as_secs();
            if was_active && since_last_event < THINKING_WINDOW_SECS {
                Some(Pose::SeatedThinking)
            } else {
                Some(idle_pose(slot, desk, layout, elapsed))
            }
        }
    }
}

/// Pure state → pose derivation, **excluding** the exit and entry override
/// blocks at the top of `derive`. Only the `state_driven_pose` tail is
/// evaluated (elapsed time since `state_started_at` drives the animation
/// frame counters).
///
/// This is the seam used by `tui::pose::derive_with_routing` so that a
/// physics-driven entry (already in-flight via `MotionState`) does not
/// restart a redundant linear entry walk. `derive()` itself stays
/// UNTOUCHED — its existing callers (TestRenderer, overlay pass, snapshot
/// tooling) keep identical behaviour.
///
/// Returns `None` when `slot.desk_index` is out of range for `layout`.
pub fn derive_state_only(slot: &AgentSlot, now: SystemTime, layout: &SceneLayout) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;
    state_driven_pose(slot, desk, layout, now)
}

/// Per-(agent, cycle) seed for `pick_aimless_dest`. Shared by core's
/// `idle_pose` and the tui's `pick_wander_dest` so the two paths can never
/// drift to different aimless destinations for the same (agent, cycle).
pub fn aimless_wander_seed(agent_id: AgentId, cycle_n: u64) -> u64 {
    agent_id.raw() ^ cycle_n.wrapping_mul(0xd1b5_4a32_d192_ed03)
}

/// Pick an aimless wander destination using weighted zones. Each zone
/// gets a "vibe weight" — window-viewing strip + pantry are highest
/// because that's where people naturally drift during breaks; corridor
/// and cubicle aisles are incidental; meeting room is rare. After
/// picking a zone (weighted random), rejection-sample 32 points
/// within the zone for a walkable pixel. Falls back to a randomised
/// point along the corridor if every probe fails.
pub fn pick_aimless_dest(layout: &SceneLayout, seed: u64) -> Point {
    // Build the zone list. Use small rectangles for "window strip"
    // (top of cubicle band, where viewing-the-city makes sense) and
    // larger bounding boxes for the rooms / corridor. Zones can
    // overlap — the walkable mask filters out non-walkable picks
    // either way.
    let window_strip = Bounds {
        x: layout.cubicle_band.x,
        y: layout.top_margin + 1,
        width: layout.cubicle_band.width,
        height: 10,
    };
    let zones: [(Bounds, u16); 5] = [
        // Stretch + look-at-the-view at the top of the cubicle band.
        (window_strip, 30),
        // Pantry interior — snack break, coffee, chat.
        (layout.pantry_room.unwrap_or(window_strip), 25),
        // Main corridor — incidental traffic.
        (layout.corridor.unwrap_or(layout.walkway), 20),
        // Cubicle band (pod aisles) — within own area, stretching.
        (layout.cubicle_band, 15),
        // Meeting room — occasional drift-in.
        (layout.meeting_room.unwrap_or(window_strip), 10),
    ];
    let total: u16 = zones.iter().map(|(_, w)| *w).sum();
    let mut roll = ((seed >> 32) as u16) % total.max(1);
    let zone = zones
        .iter()
        .find_map(|(b, w)| {
            if roll < *w {
                Some(b)
            } else {
                roll -= w;
                None
            }
        })
        .unwrap_or(&zones[0].0);
    for i in 0..32u64 {
        let h = seed
            .wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .wrapping_mul(0xc6a4_a793_5bd1_e995);
        let x = zone.x + (h as u16) % zone.width.max(1);
        let y = zone.y + ((h >> 16) as u16) % zone.height.max(1);
        if layout.is_walkable(x, y) {
            return Point { x, y };
        }
    }
    // Fallback — randomised point along the corridor's x-range so
    // multiple fallback agents spread out instead of clustering.
    let c = layout.corridor.unwrap_or(layout.walkway);
    let x_jitter = (seed as u16) % c.width.max(1);
    Point {
        x: c.x + x_jitter,
        y: c.y + c.height / 2,
    }
}

fn idle_pose(slot: &AgentSlot, desk: Point, layout: &SceneLayout, elapsed_ms: u64) -> Pose {
    let cycle_ms = est_wander_cycle_ms(slot.agent_id);
    let cycle_n = elapsed_ms / cycle_ms;
    let phase_t = elapsed_ms % cycle_ms;

    if !takes_trip(slot.agent_id, cycle_n) || layout.waypoints.is_empty() {
        return Pose::SeatedIdle;
    }

    // Per-cycle "trip type" roll. Personality.aimless_pref_pct shifts the
    // mix between lounge waypoint and aimless wander.
    let aimless = is_aimless_cycle(slot.agent_id, cycle_n);

    // Absolute phase boundaries (fixed overlay estimates; the routed render
    // path uses per-spot `dwell_ms`). cycle_ms == at_wp_end + WANDER_WALK_EST_MS
    // by construction, so the walk-back span below is always positive.
    let seated_end = seated_dwell_ms(slot.agent_id);
    let walk_out_end = seated_end + WANDER_WALK_EST_MS;
    let at_wp_end = walk_out_end + WANDER_DWELL_EST_MS;

    // Weighted-zone aimless wander. Instead of uniformly sampling
    // anywhere in the buffer (which clusters at the fallback because most
    // cubicle pixels are obstacles), pick a ZONE by weight first — window-
    // viewing strip, pantry, corridor, meeting room — then rejection-sample
    // within that zone. Weights tune the "vibe" of where agents drift:
    // window strip and pantry get the highest weight so the office feels
    // alive (people stretching at windows, grabbing coffee), corridor/
    // cubicle/meeting are more incidental. Shared between the explicit
    // aimless branch and the no-reachable-side waypoint fallback below.
    let amble = || {
        let seed = aimless_wander_seed(slot.agent_id, cycle_n);
        let p = pick_aimless_dest(layout, seed);
        (p, Pose::AimlessAt { dest: p })
    };

    // Destination: lounge waypoint OR aimless point.
    let (dest, at_dest_pose): (Point, Pose) = if aimless {
        amble()
    } else {
        let wp_idx = waypoint_index_for_cycle(slot.agent_id, cycle_n, layout.waypoints.len());
        let wp = layout.waypoints[wp_idx];
        // Walk DESTINATION (not the render anchor): the A*-reachable approach
        // point on an allowed side — for seats an allowed-side cell so the agent
        // never paths in through the back; the AtWaypoint sprite still renders on
        // the seat (see pixel_painter). Same `&layout.reachable` as tui::motion.
        let dest = crate::layout::approach_point(
            wp.kind.furniture(),
            wp.pos,
            wp.facing,
            layout.pantry_counter_size,
            &layout.walkable,
            desk,
            &layout.reachable,
        );
        // NO approach-side fallback (mirrors tui::motion::pick_wander_dest so the
        // overlay + render stay in lockstep): when no allowed+reachable side
        // exists, approach_point returns the blocked `wp.pos` sentinel (a seat
        // boxed in to only its backrest, or an obstacle with no open reachable
        // side). Amble aimlessly this cycle instead of routing into the furniture.
        if dest == wp.pos {
            amble()
        } else {
            (
                dest,
                Pose::AtWaypoint {
                    wp: wp_idx,
                    kind: wp.kind,
                },
            )
        }
    };

    if phase_t < seated_end {
        Pose::SeatedIdle
    } else if phase_t < walk_out_end {
        let span = walk_out_end - seated_end;
        let t = ((phase_t - seated_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking {
            from: desk,
            to: dest,
            t_x1000: t,
            frame,
            carrying_coffee: false,
        }
    } else if phase_t < at_wp_end {
        at_dest_pose
    } else {
        // span == WANDER_WALK_EST_MS by construction (cycle_ms == at_wp_end +
        // WANDER_WALK_EST_MS); assert it so a future estimate-constant change
        // that zeroed it can't silently divide-by-zero here.
        let span = cycle_ms - at_wp_end;
        debug_assert!(span > 0, "idle_pose walk-back span invariant violated");
        let t = ((phase_t - at_wp_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        let carrying_coffee = matches!(
            at_dest_pose,
            Pose::AtWaypoint {
                kind: WaypointKind::Pantry,
                ..
            }
        );
        Pose::Walking {
            from: dest,
            to: desk,
            t_x1000: t,
            frame,
            carrying_coffee,
        }
    }
}

#[cfg(test)]
mod tests;
