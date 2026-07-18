//! Pure state → pose derivation — every renderer (tui, floating, snapshot
//! tooling, web canvas) gets identical pose semantics from this one file.
//!
//! `derive(slot, now, layout)` is a function of the snapshot inputs only —
//! no routing, no per-frame history. The routed variant (which composes
//! against a `Router` and a `PoseHistory` cache) is the sibling
//! `pose/mod.rs` (`derive_with_routing`) — the pure-vs-routed split is
//! file-level within this module.
//!
//! Variation knobs:
//!  * `stale_resume_gap_ms(agent_id)` — per-agent off-screen-gap sentinel (NOT the
//!    wander cycle; that's `est_wander_cycle_ms`): if `advance_wander` hasn't run
//!    for this long the floor was off-screen/paused, so re-bootstrap analytically.
//!    Range 7..13s, jittered so floors don't all re-bootstrap in lockstep.
//!  * Waypoint choice XORs `agent_id` with the current cycle number, so each
//!    cycle the same agent picks a (likely) different waypoint.

use std::time::{Duration, SystemTime};

use crate::layout::{
    desk_furniture_def, desk_walk_anchor, furniture_def, Bounds, DwellWindow, Point, SceneLayout,
    WaypointKind,
};
use crate::motion::{WanderKind, WanderTarget};
use pixtuoid_core::state::{ActivityState, AgentSlot};
use pixtuoid_core::AgentId;

/// How long after the last event an Idle agent stays in the "thinking"
/// pose (seated, awake, no z's) before entering the wander/sleep cycle.
/// 20s covers typical CC thinking pauses between tool bursts.
///
/// `pub` so `pose::derive_with_routing`'s wander dispatch references the
/// same window — otherwise the thinking gate could silently drift between
/// the stateless `derive` and the routed dispatch.
pub const THINKING_WINDOW_SECS: u64 = 20;

/// Base cycle length used only as the stale-resume / off-screen-gap sentinel
/// in `motion::advance_wander` (a few seconds, above on-screen frame
/// cadence and below a floor-switch-away gap). NOT the wander dwell anymore —
/// see `dwell_ms` / `seated_dwell_ms` for the absolute per-spot timeline.
pub const STALE_RESUME_GAP_BASE_MS: u64 = 7_000;
/// Maximum extra time added per agent — jitter range is `[0, RANGE)`.
pub const STALE_RESUME_GAP_RANGE_MS: u64 = 6_000;

/// Stateless-overlay wander-timeline estimates. The render authority
/// (`motion::advance_wander`) drives the at-waypoint beat with the
/// per-spot `dwell_ms`; `idle_pose` only needs an approximate timeline to
/// place the occupancy overlay, so it uses these fixed estimates plus a
/// constant per-agent cycle period (`est_wander_cycle_ms`). Exact coherence
/// with the routed timeline is impossible — core has no router and walk legs
/// are physics-timed only in the routed motion path — and #62's frozen leg paths make
/// approximate overlay timing safe (a new leg's shape is snapshotted once).
pub const WANDER_WALK_EST_MS: u64 = 3_500;
pub const WANDER_DWELL_EST_MS: u64 = 18_000;

/// Frame-cycle period for animated poses.
pub const TYPING_FRAME_MS: u64 = 140;
pub const WALKING_FRAME_MS: u64 = 220;
pub const TYPING_FRAMES: usize = 2;
pub const WALKING_FRAMES: usize = 2;

/// The walking sprite's frame index at `elapsed_ms` into the walk — the one
/// `(elapsed / WALKING_FRAME_MS) % WALKING_FRAMES` cadence, named once so the
/// the stateless overlay and the routed motion authority can't recompute it
/// differently (it was open-coded at 8 sites across both crates).
pub fn walking_frame(elapsed_ms: u64) -> usize {
    (elapsed_ms / WALKING_FRAME_MS) as usize % WALKING_FRAMES
}

/// Spawn-window guard for entry routing in `pose::derive_with_routing`.
/// After `physics::walk_profile` took over motion timing this constant is no
/// longer used to compute walk duration — it is only the *upper bound* on the
/// time window during which the routed motion layer will attempt to route an entry walk
/// and (via `FloorCtx::door_anim_max_ms`) drive door-open cosmetics. The
/// actual walk completes when `physics::walk_arrived` returns true.
pub const ENTRY_ANIMATION_MS: u64 = 4000;

/// Per-agent stale-resume gap (ms): the `now - last_advanced_at` threshold above
/// which `motion::advance_wander` treats the floor as off-screen/paused and
/// re-bootstraps analytically instead of replaying the backlog one transition per
/// frame. This is NOT the wander cycle length (`est_wander_cycle_ms` is) — it's a
/// frame-cadence-vs-frozen-floor sentinel (7..13s), jittered per agent so floors
/// don't re-bootstrap in lockstep.
pub fn stale_resume_gap_ms(agent_id: AgentId) -> u64 {
    STALE_RESUME_GAP_BASE_MS + (agent_id.raw() >> 16) % STALE_RESUME_GAP_RANGE_MS
}

/// Base dwell plus deterministic per-agent jitter within `window`. The `tag` is
/// NOT a cryptographic salt — it just disambiguates the two callers (`dwell_ms`
/// vs `seated_dwell_ms`) so their jitter is decorrelated from each other and
/// from `speed_mult` / `pause_ms` / `cycle_ms` (which slice raw id bits). No
/// security relevance.
fn jittered_dwell(window: DwellWindow, agent_id: AgentId, tag: u64) -> u64 {
    let DwellWindow { base_ms, range_ms } = window;
    base_ms + pixtuoid_core::id::splitmix64(agent_id.raw() ^ tag) % range_ms.max(1)
}

/// Absolute dwell (ms) an agent lingers at a waypoint, per spot kind, with
/// per-agent jitter. A sofa / meeting seat is a long lounge; a vending grab
/// is quick. The render authority (`motion::advance_wander`) uses this
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

/// Widths of the hashed personality ranges: trip-chance lands in 25..=60 (span
/// 36), aimless-preference in 0..=50 (span 51).
const TRIP_CHANCE_SPAN_PCT: u64 = 36;
const AIMLESS_PREF_SPAN_PCT: u64 = 51;

pub fn personality_for(agent_id: AgentId) -> Personality {
    let h = agent_id.raw();
    Personality {
        trip_chance_pct: (25 + (h % TRIP_CHANCE_SPAN_PCT)) as u8, // 25..=60
        aimless_pref_pct: ((h >> 8) % AIMLESS_PREF_SPAN_PCT) as u8, // 0..=50
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
/// aimless wander (random cubicle_aisle point) or a directed visit to a named
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
    /// Standing at a random cubicle_aisle point (not at any waypoint). The dest field
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
    let desk = layout.home_desk(slot.desk_index.single_floor_local())?;

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
    // Target is offset (+5, +4) from the desk top-left so the walk ends at
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
/// is deliberately distinct from the routed motion path's kinematic profiles —
/// the overlay/snapshot path stays linear so it has no per-frame history.
fn linear_walk_pose(since_ms: u64, from: Point, to: Point) -> Pose {
    let t = (since_ms * 1000 / ENTRY_ANIMATION_MS).min(1000) as u16;
    let frame = walking_frame(since_ms);
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
/// This is the seam used by `pose::derive_with_routing` so that a
/// physics-driven entry (already in-flight via `MotionState`) does not
/// restart a redundant linear entry walk. `derive()` itself stays
/// UNTOUCHED — its existing callers (TestRenderer, overlay pass, snapshot
/// tooling) keep identical behaviour.
///
/// Returns `None` when `slot.desk_index` is out of range for `layout`.
pub fn derive_state_only(slot: &AgentSlot, now: SystemTime, layout: &SceneLayout) -> Option<Pose> {
    let desk = layout.home_desk(slot.desk_index.single_floor_local())?;
    state_driven_pose(slot, desk, layout, now)
}

/// Per-(agent, cycle) seed for `pick_aimless_dest`. Shared by the stateless
/// `idle_pose` and the routed `pick_wander_dest` so the two paths can never
/// drift to different aimless destinations for the same (agent, cycle).
pub fn aimless_wander_seed(agent_id: AgentId, cycle_n: u64) -> u64 {
    agent_id.raw() ^ cycle_n.wrapping_mul(0xd1b5_4a32_d192_ed03)
}

/// Pick an aimless wander destination using weighted zones. Each zone
/// gets a "vibe weight" — window-viewing strip + pantry are highest
/// because that's where people naturally drift during breaks; corridor
/// and cubicle aisles are incidental; meeting room is rare. After
/// picking a zone (weighted random), rejection-sample 32 points
/// within the zone for a walkable pixel. Falls back to the nearest
/// walkable corridor-midline cell from a randomised x if every probe
/// fails, and to `home_desk`'s walk anchor in the degenerate fully
/// blocked-corridor case — never a cell inside furniture.
///
/// `home_desk` is the caller's home desk (`idle_pose`'s `desk` /
/// `pick_wander_dest`'s `origin` — the same Point on both paths), so the
/// result stays deterministic in `(layout, seed)` across the two callers.
pub fn pick_aimless_dest(layout: &SceneLayout, seed: u64, home_desk: Point) -> Point {
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
    // The corridor zone (its cubicle-aisle fallback) is used twice — the wander
    // zone table and the fallback midline scan below — so bind it once.
    let corridor_zone = layout.corridor.unwrap_or(layout.cubicle_aisle);
    let zones: [(Bounds, u16); 5] = [
        // Stretch + look-at-the-view at the top of the cubicle band.
        (window_strip, 30),
        // Pantry interior — snack break, coffee, chat.
        (layout.pantry.map_or(window_strip, |p| p.bounds), 25),
        // Main corridor — incidental traffic.
        (corridor_zone, 20),
        // Cubicle band (pod aisles) — within own area, stretching.
        (layout.cubicle_band, 15),
        // Meeting room — occasional drift-in.
        (layout.meeting_room_bounds(0).unwrap_or(window_strip), 10),
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
    // Rejection-sampling budget: tries this many hashed cells to land on a
    // walkable one before falling back to the zone origin.
    const AIMLESS_SAMPLE_ATTEMPTS: u64 = 32;
    for i in 0..AIMLESS_SAMPLE_ATTEMPTS {
        let h = seed
            .wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .wrapping_mul(0xc6a4_a793_5bd1_e995);
        let x = zone.x + (h as u16) % zone.width.max(1);
        let y = zone.y + ((h >> 16) as u16) % zone.height.max(1);
        if layout.is_walkable(x, y) {
            return Point { x, y };
        }
    }
    // Fallback — randomised point along the corridor's x-range so multiple
    // fallback agents spread out instead of clustering. The midline is NOT
    // guaranteed open (the vending / printer footprints sit in the
    // cubicle_aisle band), so scan outward from the jittered x for the
    // nearest walkable midline cell — everywhere else this function's
    // contract is "returns a walkable pixel". Bounded by the corridor width
    // and purely (layout, seed)-deterministic, like the probes above.
    let c = corridor_zone;
    let x_jitter = (seed as u16) % c.width.max(1);
    let base_x = c.x + x_jitter;
    let mid_y = c.y + c.height / 2;
    let in_band = |x: u16| x >= c.x && x < c.x.saturating_add(c.width);
    for d in 0..c.width {
        let east = base_x.saturating_add(d);
        if in_band(east) && layout.is_walkable(east, mid_y) {
            return Point { x: east, y: mid_y };
        }
        let west = base_x.saturating_sub(d);
        if in_band(west) && layout.is_walkable(west, mid_y) {
            return Point { x: west, y: mid_y };
        }
    }
    // Whole midline blocked (degenerate layout): the agent's own desk anchor
    // is a destination every consumer already handles (A* snap / render
    // anchor) — return it rather than hand out a cell inside furniture.
    desk_walk_anchor(home_desk)
}

/// Milliseconds of the current Idle period during which `state_driven_pose`'s
/// SeatedThinking gate held the agent at its desk: from `state_started_at`
/// until `last_event_at + THINKING_WINDOW_SECS`. 0 when the agent was never
/// active (mirrors the gate's `was_active`) or when the window expired before
/// this Idle period began.
fn thinking_hold_ms(slot: &AgentSlot) -> u64 {
    if slot.last_event_at <= slot.created_at {
        return 0;
    }
    slot.last_event_at
        .checked_add(Duration::from_secs(THINKING_WINDOW_SECS))
        .and_then(|release| release.duration_since(slot.state_started_at).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The EXCLUSIVE-spot waypoints already spoken for — the input that makes a
/// single-occupancy destination single-occupancy. An exclusive spot (any
/// `exclusive` furniture: the seats couch / meeting sofa / meeting chair /
/// island stand, PLUS the stand-beside singles phone booth / standing desk) is
/// a discrete slot for exactly one occupant, and each is already its OWN
/// waypoint, so "two agents at the same exclusive waypoint" has no valid
/// rendering — the painter's rank de-collision can only slide the second
/// occupant sideways onto thin air (or, at ±9, onto the meeting table). Claims
/// are therefore resolved where the destination is CHOSEN, not where it's drawn.
///
/// Built by [`crate::motion::spot_claims`] from the live wander targets; small
/// by construction (at most one entry per agent out on an exclusive-spot trip),
/// so a linear `contains` beats hashing.
#[derive(Debug, Default, Clone)]
pub(crate) struct SpotClaims(Vec<usize>);

impl SpotClaims {
    /// Claim `wp_idx` for an exclusive-spot trip. Idempotent — re-claiming is a
    /// no-op, so an agent re-picking its own held spot can't grow the set.
    pub(crate) fn claim(&mut self, wp_idx: usize) {
        if !self.holds(wp_idx) {
            self.0.push(wp_idx);
        }
    }

    /// Whether some agent is already headed for (or occupying) this spot. Only
    /// `exclusive` waypoints are ever claimed, so this is always false for a
    /// shareable queue spot.
    pub(crate) fn holds(&self, wp_idx: usize) -> bool {
        self.0.contains(&wp_idx)
    }
}

/// Resolve the wander destination for `(agent_id, cycle_n)` on `layout`, with
/// `origin` (the home desk) as the approach-side tiebreaker. The ONE stateless
/// wander-destination resolver: the stateful motion authority
/// (`motion::advance_wander` via `pick_wander_dest`) delegates to it, and
/// `idle_pose` calls it then maps the [`WanderTarget`] to a [`Pose`]. Was
/// DUPLICATED across the two, kept in lockstep only by comment + test.
///
/// `claimed` is the seat-exclusion input (see [`SpotClaims`]): the two callers
/// stay in lockstep for equal `claimed`, and `idle_pose` passes an EMPTY set
/// because the pure half has no occupancy knowledge — the same reason its dwell
/// timeline is estimated rather than exact. The routed motion authority is what
/// actually places agents, so it is the one that must not double-book.
///
/// So the two DO diverge on a contested seat, and the consumer that sees it is
/// the A\* occupancy prepass (`pixel_painter::sim::sim_step`, which reserves the
/// stand cell of whatever the STATELESS `derive` returns) — not the label
/// overlay, which rides the routed `derive_with_routing`. Cost: on a contested
/// seat the prepass reserves the loser's cell and not the winner's, so a walker
/// may clip a real sitter. That is the SAME trade the frozen-polyline sharp edge
/// already buys (rare, cosmetic, legs are seconds), on a second axis — the two
/// timelines were already allowed to disagree. What matters is preserved: the
/// pure half stays a function of its snapshot inputs, so the overlay's cache
/// signature is still frame-stable.
///
/// A named waypoint whose only reachable side is its excluded backrest (or a
/// fully-blocked obstacle) yields `approach_point == wp.pos`, the "no valid
/// approach" sentinel — this ambles aimlessly that cycle instead of routing into
/// the furniture.
pub(crate) fn resolve_wander_target(
    id: AgentId,
    cycle_n: u64,
    layout: &SceneLayout,
    origin: Point,
    claimed: &SpotClaims,
) -> WanderTarget {
    let amble = || WanderTarget {
        dest: pick_aimless_dest(layout, aimless_wander_seed(id, cycle_n), origin),
        kind: WanderKind::Aimless,
    };
    if is_aimless_cycle(id, cycle_n) {
        return amble();
    }
    let n = layout.waypoints.len();
    let first = waypoint_index_for_cycle(id, cycle_n, n);
    // A claimed seat simply isn't a destination: probe forward to the next
    // unclaimed waypoint. A venue's seats are CONTIGUOUS in `waypoints`
    // (`compute_waypoints` pushes a sofa's 3 / a table's 2 in one run), so the
    // probe lands on a neighbouring seat of the SAME venue first — "that chair's
    // taken, I'll take the next one" — and only walks off to another spot once
    // the venue is full. Non-seat waypoints are never claimed, so with no claims
    // this resolves to `waypoints[first]` exactly as it did before.
    let Some(wp_idx) = (0..n)
        .map(|step| (first + step) % n)
        .find(|i| !claimed.holds(*i))
    else {
        return amble();
    };
    let wp = layout.waypoints[wp_idx];
    // Walk destination = the A*-reachable approach point on an allowed side (NOT
    // the raw blocked `wp.pos`, which made A* detour + the sprite pop). Same
    // `&layout.reachable` + origin on both callers so they can't drift.
    let dest = layout.approach_point(wp.kind.furniture(), wp.pos, origin, wp.facing);
    if dest == wp.pos {
        return amble();
    }
    // Seat foot cell `S`: the walk SETTLES from `dest` onto it (the sprite renders
    // here). `None` for obstacles — the agent stands AT `dest`.
    let seat = crate::layout::seated_foot_cell(wp.kind.furniture(), wp.pos);
    WanderTarget {
        dest,
        kind: WanderKind::Named {
            wp_idx,
            kind: wp.kind,
            seat,
        },
    }
}

fn idle_pose(slot: &AgentSlot, desk: Point, layout: &SceneLayout, elapsed_ms: u64) -> Pose {
    let cycle_ms = est_wander_cycle_ms(slot.agent_id);
    let cycle_n = elapsed_ms / cycle_ms;
    let phase_t = elapsed_ms % cycle_ms;

    if !takes_trip(slot.agent_id, cycle_n) || layout.waypoints.is_empty() {
        return Pose::SeatedIdle;
    }

    // Absolute phase boundaries (fixed overlay estimates; the routed render
    // path uses per-spot `dwell_ms`). cycle_ms == at_wp_end + WANDER_WALK_EST_MS
    // by construction, so the walk-back span below is always positive.
    let seated_end = seated_dwell_ms(slot.agent_id);
    let walk_out_end = seated_end + WANDER_WALK_EST_MS;
    let at_wp_end = walk_out_end + WANDER_DWELL_EST_MS;

    // Thinking-gate continuity: the SeatedThinking gate masks this timeline
    // until `thinking_hold_ms` into the Idle period. If the release lands PAST
    // this cycle's seated phase, the first pose `derive()` would emit after
    // SeatedThinking is mid-Walking / AtWaypoint — a desk→corridor pop in
    // every stateless consumer (TestRenderer, occupancy overlay, snapshot
    // tooling). Sit out the RELEASE cycle instead: SeatedThinking → SeatedIdle
    // is continuous, the cycle boundary stays continuous (Seated → Seated),
    // and the next trip's walk-out starts from its beginning. Deliberately
    // phase-only — shifting `cycle_n` would desync destination selection from
    // `motion::advance_wander`'s bootstrap (`elapsed_idle /
    // est_wander_cycle_ms`), which must stay in lockstep with this function.
    let hold = thinking_hold_ms(slot);
    if cycle_n == hold / cycle_ms && hold % cycle_ms > seated_end {
        return Pose::SeatedIdle;
    }

    // Destination: lounge waypoint OR aimless point, resolved by the ONE shared
    // `resolve_wander_target` (also driving `motion::advance_wander`), then mapped
    // to a pose. The per-cycle aimless-vs-waypoint roll, the weighted-zone aimless
    // pick, the reachable-approach selection, and the no-reachable-side amble
    // fallback all live in that resolver so the stateless overlay here and the
    // stateful render can't drift. Seat claims are EMPTY here: this half has no
    // occupancy knowledge (see `resolve_wander_target`), and its job is a
    // cache-stable overlay estimate, not the authoritative placement.
    let target =
        resolve_wander_target(slot.agent_id, cycle_n, layout, desk, &SpotClaims::default());
    let dest = target.dest;
    let at_dest_pose: Pose = match target.kind {
        WanderKind::Named { wp_idx, kind, .. } => Pose::AtWaypoint { wp: wp_idx, kind },
        WanderKind::Aimless => Pose::AimlessAt { dest: target.dest },
    };

    if phase_t < seated_end {
        Pose::SeatedIdle
    } else if phase_t < walk_out_end {
        let span = walk_out_end - seated_end;
        let t = ((phase_t - seated_end) * 1000 / span) as u16;
        let frame = walking_frame(elapsed_ms);
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
        let frame = walking_frame(elapsed_ms);
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
