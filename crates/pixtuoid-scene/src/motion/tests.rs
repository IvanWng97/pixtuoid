use super::*;
use crate::pose::{is_aimless_cycle, waypoint_index_for_cycle};
use pixtuoid_core::{AgentId, GlobalDeskIndex};

fn id() -> AgentId {
    AgentId::from_parts("test", "motion-test-agent")
}

// --- MotionState::new -------------------------------------------------

#[test]
fn motion_state_new_default_fields() {
    let ms = MotionState::new(id());
    assert!(ms.entry.is_none());
    assert!(ms.exit.is_none());
    assert!(ms.snap_back.is_none());
    assert_eq!(ms.wander.cycle_n, 0);
    assert_eq!(ms.wander.phase, WanderPhase::Seated);
    assert_eq!(ms.wander.phase_started_at, SystemTime::UNIX_EPOCH);
    assert_eq!(ms.wander.last_advanced_at, SystemTime::UNIX_EPOCH);
    // No separate profile field now — a fresh state is Seated (no walk leg).
    assert!(matches!(ms.wander.phase, WanderPhase::Seated));
    assert!(matches!(ms.wander.target.kind, WanderKind::Aimless));
    assert!(ms.walk_path.is_none());
}

// --- octile_path_len --------------------------------------------------

#[test]
fn path_len_empty_is_zero() {
    assert_eq!(octile_path_len(&[]), 0);
}

#[test]
fn path_len_single_point_is_zero() {
    let p = Point { x: 10, y: 20 };
    assert_eq!(octile_path_len(&[p]), 0);
}

#[test]
fn path_len_orthogonal_segment() {
    // 5 px right: octile = 10*5 = 50
    let a = Point { x: 0, y: 0 };
    let b = Point { x: 5, y: 0 };
    assert_eq!(octile_path_len(&[a, b]), 50);
}

#[test]
fn path_len_diagonal_segment() {
    // 3 px diagonal: octile = 14*3 = 42
    let a = Point { x: 0, y: 0 };
    let b = Point { x: 3, y: 3 };
    assert_eq!(octile_path_len(&[a, b]), 42);
}

#[test]
fn path_len_multi_segment_sums() {
    // right 4 (40) + down 3 (30) = 70
    let a = Point { x: 0, y: 0 };
    let b = Point { x: 4, y: 0 };
    let c = Point { x: 4, y: 3 };
    assert_eq!(octile_path_len(&[a, b, c]), 70);
}

// =========================================================================
// advance_wander tests
// =========================================================================

use crate::layout::Layout;
use crate::pathfind::Router;
use crate::pose::{
    dwell_ms, est_wander_cycle_ms, seated_dwell_ms, stale_resume_gap_ms, takes_trip,
    WANDER_DWELL_EST_MS,
};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};
use std::path::PathBuf;
use std::sync::Arc;

// -----------------------------------------------------------------------
// Stub routers
// -----------------------------------------------------------------------

/// Straight-line stub: always returns `[from, to]`.
struct Straight;
impl Router for Straight {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point> {
        vec![from, to]
    }
    fn invalidate(&mut self) {}
}

/// Recording stub: captures every `(from, to)` route request, returning the
/// straight line. Pins WHICH goal the profile snapshots route to.
struct Recording {
    calls: Vec<(Point, Point)>,
}
impl Router for Recording {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point> {
        self.calls.push((from, to));
        vec![from, to]
    }
    fn invalidate(&mut self) {}
}

/// Fixed-octile-length stub: synthesises a horizontal path of the requested
/// octile length starting at `from`, ignoring `to`. Used to test phase
/// transitions with predictable walk durations.
struct FixedLen {
    octile_len: u32,
}
impl Router for FixedLen {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &OccupancyOverlay,
        from: Point,
        _to: Point,
    ) -> Vec<Point> {
        // Horizontal path: each step is 10 octile units (1 px orthogonal).
        // octile_len / 10 px ≈ requested length.
        let steps = (self.octile_len / 10) as u16;
        let mid = Point {
            x: from.x + steps / 2,
            y: from.y,
        };
        let end = Point {
            x: from.x + steps,
            y: from.y,
        };
        vec![from, mid, end]
    }
    fn invalidate(&mut self) {}
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn idle_slot(path: &str, state_started: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: AgentId::from_transcript_path(path),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: "cc".into(),
        state: ActivityState::Idle,
        state_started_at: state_started,
        created_at: state_started
            .checked_sub(Duration::from_secs(90))
            .unwrap_or(state_started),
        last_event_at: state_started
            .checked_sub(Duration::from_secs(90))
            .unwrap_or(state_started),
        exiting_at: None,
        pending_idle_at: None,
        desk_index: GlobalDeskIndex(0),
        floor_idx: 0,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
        pid: None,
        model: None,
        effort: None,
        tokens_used: 0,
        last_usage: None,
    }
}

fn layout() -> Layout {
    Layout::compute(120, 96, Some(4)).expect("fits")
}

/// Find an agent whose cycle_n=0 is a trip cycle, using the given path prefix.
fn trip_agent(prefix: &str) -> AgentId {
    (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/{prefix}_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("should find a trip agent quickly")
}

/// The dwell the machine will apply at the agent's current wander destination
/// (per-spot for a named waypoint, the estimate for an aimless trip). Read
/// after the agent has picked a destination (WalkingOut onward).
fn current_dwell_dur(motion: &HashMap<AgentId, MotionState>, id: AgentId) -> u64 {
    match motion.get(&id).map(|ms| ms.wander.target.kind) {
        Some(WanderKind::Named { kind, .. }) => dwell_ms(kind, id),
        _ => WANDER_DWELL_EST_MS,
    }
}

/// The variant KIND of a `WanderPhase`, ignoring the carried `WalkProfile` —
/// `advance_until_leaves` polls until the phase's KIND changes, and the per-leg
/// profile payload is not what these transition tests key on (they assert a
/// walk began/ended, not which frozen profile it froze). Since the walk
/// variants now carry a non-`Eq` `WalkProfile`, this is the phase's identity for
/// the "has the agent left this phase?" question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    Seated,
    WalkingOut,
    AtWaypoint,
    WalkingBack,
}

fn phase_kind(phase: WanderPhase) -> PhaseKind {
    match phase {
        WanderPhase::Seated => PhaseKind::Seated,
        WanderPhase::WalkingOut(_) => PhaseKind::WalkingOut,
        WanderPhase::AtWaypoint(_) => PhaseKind::AtWaypoint,
        WanderPhase::WalkingBack(_) => PhaseKind::WalkingBack,
    }
}

/// Poll `advance_wander` in ~1 s steps (well under the `stale_resume_gap_ms`
/// stale-resume trigger, so a long seated/dwell beat is crossed exactly as
/// real per-frame rendering would, never looking like an off-screen gap)
/// until the agent's phase KIND is no longer `from_phase`. Returns the new
/// `now`. Panics if the transition doesn't happen within `timeout_ms`.
#[allow(clippy::too_many_arguments)]
fn advance_until_leaves(
    slot: &AgentSlot,
    l: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    motion: &mut HashMap<AgentId, MotionState>,
    mut now: SystemTime,
    from_phase: PhaseKind,
    timeout_ms: u64,
) -> SystemTime {
    const STEP_MS: u64 = 1_000;
    let start = now;
    while motion
        .get(&slot.agent_id)
        .map(|m| phase_kind(m.wander.phase))
        == Some(from_phase)
    {
        let elapsed = now
            .duration_since(start)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        assert!(
            elapsed <= timeout_ms,
            "phase {from_phase:?} did not transition within {timeout_ms}ms"
        );
        now += Duration::from_millis(STEP_MS);
        advance_wander(slot, now, l, router, overlay, motion);
    }
    now
}

// -----------------------------------------------------------------------
// T1: Fresh idle agent initialises into Seated phase
// -----------------------------------------------------------------------
#[test]
fn fresh_idle_inits_to_seated_phase() {
    let now = t0();
    let slot = idle_slot("/p/a.jsonl", now);
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&slot.agent_id).expect("state inserted");
    assert!(
        matches!(ms.wander.phase, WanderPhase::Seated),
        "fresh idle should init to Seated, got {:?}",
        ms.wander.phase
    );
    assert_eq!(ms.wander.cycle_n, 0);
}

// -----------------------------------------------------------------------
// T2: Seated phase transitions to WalkingOut after the seated dwell elapses
//     on a trip cycle.
// -----------------------------------------------------------------------
#[test]
fn seated_transitions_to_walking_out_on_trip_cycle() {
    let trip_id = trip_agent("trip");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state present");
    assert!(
        matches!(ms.wander.phase, WanderPhase::WalkingOut(_)),
        "after seated dwell on trip cycle, expected WalkingOut, got {:?}",
        ms.wander.phase
    );
}

// -----------------------------------------------------------------------
// T3: Non-trip cycle stays Seated even after the seated dwell elapses
// -----------------------------------------------------------------------
#[test]
fn non_trip_cycle_stays_seated() {
    let stay_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/stay_{i}.jsonl")))
        .find(|id| !takes_trip(*id, 0))
        .expect("should find a stay-seated agent");

    let now = t0();
    let slot = AgentSlot {
        agent_id: stay_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    // Poll well past the longest seated dwell (30 s) — a non-trip cycle must
    // never leave Seated (it bumps cycle_n in place instead).
    let mut t = now;
    for _ in 0..40 {
        t += Duration::from_millis(1_000);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
        assert!(
            matches!(
                motion.get(&stay_id).unwrap().wander.phase,
                WanderPhase::Seated
            ),
            "non-trip cycle must stay Seated"
        );
    }
}

// -----------------------------------------------------------------------
// T4: WalkingOut transitions to AtWaypoint when walk_arrived fires
// -----------------------------------------------------------------------
#[test]
fn walking_out_transitions_to_at_waypoint_on_arrival() {
    let trip_id = trip_agent("wp");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    // → WalkingOut
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::WalkingOut(_)
    ));
    // → AtWaypoint (short walk, arrives within a couple of 1 s steps)
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        PhaseKind::WalkingOut,
        20_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander.phase, WanderPhase::AtWaypoint(_)),
        "expected AtWaypoint after walk-out arrival, got {:?}",
        ms.wander.phase
    );
}

// -----------------------------------------------------------------------
// T5: AtWaypoint dwell transitions to WalkingBack after the per-spot dwell
// -----------------------------------------------------------------------
#[test]
fn at_waypoint_transitions_to_walking_back_after_dwell() {
    let trip_id = trip_agent("dwell");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    let t2 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        PhaseKind::WalkingOut,
        20_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::AtWaypoint(_)
    ));
    // Cross the (long) per-spot dwell.
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t2,
        PhaseKind::AtWaypoint,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander.phase, WanderPhase::WalkingBack(_)),
        "expected WalkingBack after dwell, got {:?}",
        ms.wander.phase
    );
}

// -----------------------------------------------------------------------
// T6: WalkingBack arrival increments cycle_n and resets to Seated
// -----------------------------------------------------------------------
#[test]
fn walking_back_arrival_increments_cycle_n_and_resets_to_seated() {
    let trip_id = trip_agent("cyc");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    let t = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        PhaseKind::WalkingOut,
        20_000,
    );
    let t = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        PhaseKind::AtWaypoint,
        60_000,
    );
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        PhaseKind::WalkingBack,
        20_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander.phase, WanderPhase::Seated),
        "completed cycle must reset to Seated, got {:?}",
        ms.wander.phase
    );
    assert_eq!(ms.wander.cycle_n, 1, "cycle_n must increment once");
}

// -----------------------------------------------------------------------
// T7: Dwell time is independent of path length (it is per-spot, not per-walk)
// -----------------------------------------------------------------------
#[test]
fn dwell_time_independent_of_path_length() {
    let trip_id = trip_agent("dwell2");
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", t0())
    };
    let l = layout();
    let overlay = OccupancyOverlay::new();

    let mut measured: Vec<u64> = Vec::new();
    for short_len in [150u32, 800u32] {
        let now = t0();
        let mut router = FixedLen {
            octile_len: short_len,
        };
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
        let t1 = advance_until_leaves(
            &slot,
            &l,
            &mut router,
            &overlay,
            &mut motion,
            now,
            PhaseKind::Seated,
            60_000,
        );
        let at_wp_enter = advance_until_leaves(
            &slot,
            &l,
            &mut router,
            &overlay,
            &mut motion,
            t1,
            PhaseKind::WalkingOut,
            20_000,
        );
        let walk_back_enter = advance_until_leaves(
            &slot,
            &l,
            &mut router,
            &overlay,
            &mut motion,
            at_wp_enter,
            PhaseKind::AtWaypoint,
            60_000,
        );
        let dwell = walk_back_enter
            .duration_since(at_wp_enter)
            .unwrap()
            .as_millis() as u64;
        measured.push(dwell);
    }

    // Same destination both runs (same agent, same cycle_n) → the dwell is the
    // same regardless of how long the walk leg was. Allow one 1 s poll step of
    // slack.
    let diff = measured[0].abs_diff(measured[1]);
    assert!(
        diff <= 1_000,
        "dwell must be path-length-independent: {measured:?}"
    );
}

// -----------------------------------------------------------------------
// T8: Far waypoint full-cycle wall-time longer than near
// -----------------------------------------------------------------------
#[test]
fn far_waypoint_full_cycle_is_longer() {
    use crate::physics::{walk_profile, WalkIntent};

    let trip_id = trip_agent("far");
    let seated_dur = seated_dwell_ms(trip_id);
    // Dwell is per-spot but constant across path lengths, so it cancels out of
    // the near-vs-far comparison — use the estimate as a fixed stand-in.
    let dwell_dur = WANDER_DWELL_EST_MS;

    let cycle_wall_ms = |path_len: u32| -> u64 {
        let out = walk_profile(path_len, WalkIntent::WanderOut, trip_id);
        let back = walk_profile(path_len, WalkIntent::WanderBack, trip_id);
        seated_dur
            + (out.duration_ms + out.pause_ms)
            + dwell_dur
            + (back.duration_ms + back.pause_ms)
    };

    let near_ms = cycle_wall_ms(100);
    let far_ms = cycle_wall_ms(1200);
    assert!(
        far_ms > near_ms,
        "far cycle ({far_ms}ms) must be longer than near cycle ({near_ms}ms)"
    );

    let out_near = walk_profile(100, WalkIntent::WanderOut, trip_id);
    let out_far = walk_profile(1200, WalkIntent::WanderOut, trip_id);
    assert!(
        out_far.duration_ms > out_near.duration_ms,
        "far walk must take longer"
    );
}

// -----------------------------------------------------------------------
// snapshot_leg_profile: the one-shot route→measure→freeze composition the
// five entry/exit/snap-back/wander legs share
// -----------------------------------------------------------------------
#[test]
fn snapshot_leg_profile_measures_the_routed_leg_plus_settles() {
    use crate::physics::{walk_profile, WalkIntent};

    let mask = WalkableMask::new_open(64, 64);
    let overlay = OccupancyOverlay::new();
    let id = AgentId::from_parts("test", "leg-profile");
    let from = Point { x: 4, y: 8 };
    let to = Point { x: 40, y: 8 };
    let seat = Point { x: 42, y: 10 };

    // The straight-line router returns `[from, to]` and route_jittered restores
    // the endpoint, so the composer must equal the manual route→measure→freeze it
    // replaced — pinning that measurement runs on the ROUTED polyline + settles.
    let got = snapshot_leg_profile(
        &mut Straight,
        &mask,
        &overlay,
        id,
        from,
        to,
        None,
        Some(seat),
        WalkIntent::WanderOut,
    );
    let path = route_jittered(&mut Straight, &mask, &overlay, id, from, to);
    let expect = walk_profile(
        measured_leg_len(&path, None, Some(seat)),
        WalkIntent::WanderOut,
        id,
    );
    assert_eq!(got.path_len_octile, expect.path_len_octile);
    assert_eq!(got.duration_ms, expect.duration_ms);

    // The end-settle glide onto the seat must LENGTHEN the measured leg vs no
    // settle — pins the settle segments are folded in, not dropped.
    let no_settle = snapshot_leg_profile(
        &mut Straight,
        &mask,
        &overlay,
        id,
        from,
        to,
        None,
        None,
        WalkIntent::WanderOut,
    );
    assert!(
        got.path_len_octile > no_settle.path_len_octile,
        "an end settle must add octile length to the measured leg"
    );
}

// -----------------------------------------------------------------------
// T9: Arrival pause holds WalkingOut phase during [T, T+pause)
// -----------------------------------------------------------------------
#[test]
fn arrival_pause_holds_walking_out_phase() {
    use crate::physics::{walk_arrived, walk_profile, WalkIntent};

    let trip_id = trip_agent("pause");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let profile = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
    let mid_pause_elapsed = profile.duration_ms + profile.pause_ms / 2;
    assert!(
        !walk_arrived(&profile, mid_pause_elapsed),
        "walk_arrived must be false mid-pause"
    );

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    let out_started = motion.get(&trip_id).unwrap().wander.phase_started_at;
    // The out-leg profile now rides the WalkingOut variant (no separate field).
    let actual_profile = match motion.get(&trip_id).unwrap().wander.phase {
        WanderPhase::WalkingOut(p) => p,
        other => panic!("expected WalkingOut, got {other:?}"),
    };
    let actual_mid_elapsed = actual_profile.duration_ms + actual_profile.pause_ms / 2;

    // Mid-pause: still WalkingOut (walk_arrived returns false). This sample is
    // within ~1 s of t1, far below the stale trigger.
    let _ = t1;
    let mid = out_started + Duration::from_millis(actual_mid_elapsed);
    advance_wander(&slot, mid, &l, &mut router, &overlay, &mut motion);
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander.phase,
            WanderPhase::WalkingOut(_)
        ),
        "must stay WalkingOut during arrival pause"
    );
}

// -----------------------------------------------------------------------
// T10: Idempotency — advance_wander twice same `now` leaves state unchanged
// -----------------------------------------------------------------------
#[test]
fn idempotent_same_now_does_not_mutate_state() {
    let trip_id = trip_agent("idem");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );

    let (phase_before, cycle_before) = {
        let ms = motion.get(&trip_id).unwrap();
        (ms.wander.phase, ms.wander.cycle_n)
    };

    // Call again with the SAME `now` (t1) — must NOT mutate.
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert_eq!(
        ms.wander.phase, phase_before,
        "2nd call with same now must not change phase"
    );
    assert_eq!(
        ms.wander.cycle_n, cycle_before,
        "2nd call with same now must not change cycle_n"
    );
}

// -----------------------------------------------------------------------
// T11: Bootstrap — agent idle for N cycles before first render. cycle_n is
//      fast-forwarded by the ESTIMATED cycle (matches idle_pose), not the
//      stale-resume sentinel stale_resume_gap_ms.
// -----------------------------------------------------------------------
#[test]
fn bootstrap_fast_forwards_cycle_n() {
    let id = AgentId::from_transcript_path("/p/bootstrap.jsonl");
    let now = t0();
    let cycle = est_wander_cycle_ms(id);
    let state_started = now
        .checked_sub(Duration::from_millis(10 * cycle))
        .expect("time arithmetic ok");
    let slot = idle_slot("/p/bootstrap.jsonl", state_started);

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&id).expect("state present");
    assert_eq!(
        ms.wander.cycle_n, 10,
        "bootstrap: elapsed = 10*est_cycle => cycle_n must equal exactly 10"
    );
}

// -----------------------------------------------------------------------
// T12: Stale resume — a floor off-screen (motion frozen) must resync
//      analytically on return instead of replaying the backlog one phase per
//      frame. Trigger: gap > stale_resume_gap_ms; fast-forward divides by est cycle.
// -----------------------------------------------------------------------
#[test]
fn stale_resume_resyncs_without_replay() {
    let trip_id = trip_agent("stale");
    let now = t0();
    let est_cycle = est_wander_cycle_ms(trip_id);

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Init, then poll into a walk leg so the pre-gap phase is mid-cycle.
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander.phase,
            WanderPhase::WalkingOut(_)
        ),
        "precondition: agent should be WalkingOut before the gap"
    );

    // Floor goes off-screen for ~20 cycles; advance_wander is NOT called.
    // The gap dwarfs the stale trigger; a SINGLE call on return must resync.
    assert!(20 * est_cycle > stale_resume_gap_ms(trip_id));
    let resume = t1 + Duration::from_millis(20 * est_cycle);
    advance_wander(&slot, resume, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert!(
        matches!(ms.wander.phase, WanderPhase::Seated),
        "stale resume must resync to Seated (no per-frame replay), got {:?}",
        ms.wander.phase
    );
    assert!(
        ms.wander.cycle_n >= 18,
        "stale resume must fast-forward cycle_n across the gap, got {}",
        ms.wander.cycle_n
    );
}

// -----------------------------------------------------------------------
// T13: A long on-screen dwell (sampled every ~33 ms) never trips the
//      stale-resume resync — the guard against making the trigger a dwell
//      detector. The agent stays AtWaypoint until the dwell genuinely ends.
// -----------------------------------------------------------------------
#[test]
fn long_dwell_never_trips_stale_resume_on_screen() {
    let trip_id = trip_agent("longdwell");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );
    let t2 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        PhaseKind::WalkingOut,
        20_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::AtWaypoint(_)
    ));

    // Sample every 33 ms across (almost) the full dwell window. Even for a 40 s
    // sofa lounge the per-frame gap stays ~33 ms, so the stale-resume (gap >
    // stale_resume_gap_ms) must never fire — the agent must NOT snap to Seated.
    // Base the window on the ACTUAL AtWaypoint phase start (the poll-observed
    // `t2` can lag the real transition by up to one 1 s step), leaving a 2 s
    // margin so we stop before the dwell genuinely ends.
    let at_wp_start = motion.get(&trip_id).unwrap().wander.phase_started_at;
    let dwell_dur = current_dwell_dur(&motion, trip_id);
    let mut t = t2;
    let end = at_wp_start + Duration::from_millis(dwell_dur.saturating_sub(2_000));
    while t < end {
        t += Duration::from_millis(33);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
        assert!(
            !matches!(
                motion.get(&trip_id).unwrap().wander.phase,
                WanderPhase::Seated
            ),
            "long on-screen dwell wrongly tripped stale-resume (snapped to Seated mid-dwell)"
        );
    }
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander.phase,
            WanderPhase::AtWaypoint(_)
        ),
        "agent should still be AtWaypoint just before the dwell ends"
    );
}

// -----------------------------------------------------------------------
// Jitter lockstep: the profile snapshots must route to the SAME jittered
// goal the render's walk-path freeze uses (route_walking_pose →
// jitter_dest). A raw-dest route measures a differently-shaped polyline
// than the one rendered (wrong walk speed) AND mints a second router-cache
// key per leg (2x the PATH_CACHE_CAP sizing note).
// -----------------------------------------------------------------------

use crate::pose::jitter_dest;

/// A trip agent whose ±4px goal jitter is nonzero, so the lockstep
/// assertions below have teeth.
fn jittering_trip_agent(prefix: &str) -> AgentId {
    let probe = Point { x: 50, y: 50 };
    (0u64..2000)
        .map(|i| AgentId::from_transcript_path(&format!("/p/{prefix}_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0) && jitter_dest(*id, probe) != probe)
        .expect("should find a jittering trip agent quickly")
}

#[test]
fn wander_out_profile_routes_to_the_jittered_goal_the_render_uses() {
    let trip_id = jittering_trip_agent("jout");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Recording { calls: Vec::new() };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(matches!(ms.wander.phase, WanderPhase::WalkingOut(_)));
    let (_, routed_to) = *router.calls.last().expect("the trip snapshot routed");
    assert_eq!(
        routed_to,
        jitter_dest(trip_id, ms.wander.target.dest),
        "the WalkingOut profile must route to the render's jittered goal"
    );
}

#[test]
fn back_profile_routes_to_the_jittered_desk_goal_the_render_uses() {
    let trip_id = jittering_trip_agent("jback");
    let now = t0();
    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Recording { calls: Vec::new() };
    let mut ms = MotionState::new(trip_id);
    ms.wander.target.dest = Point { x: 40, y: 60 };

    let _ = snapshot_back_profile(&slot, &ms, &l, &mut router, &overlay);

    let (snap_to, _) = desk_leg_endpoint(l.home_desks[0], &l);
    assert_eq!(
        router.calls,
        vec![(ms.wander.target.dest, jitter_dest(trip_id, snap_to))],
        "the walk-back profile must route to the render's jittered desk goal"
    );
}

// -----------------------------------------------------------------------
// Dest mirror: the motion walk destination for a furniture waypoint must
// equal layout::stand_point computed with the agent's HOME DESK as
// origin — the same call pose::pure::idle_pose and both render anchors make.
// Guards the load-bearing core↔tui dest mirror against a future origin drift.
// -----------------------------------------------------------------------
#[test]
fn wander_dest_for_pantry_is_the_home_desk_stand_point() {
    let l = layout();
    let pantry_idx = l
        .waypoints
        .iter()
        .position(|w| w.kind == WaypointKind::Pantry)
        .expect("standard floor has a pantry");
    // Find an agent whose cycle-0 trip is a non-aimless pantry visit.
    let (path, _id) = (0u64..8000)
        .find_map(|i| {
            let p = format!("/p/mirror_{i}.jsonl");
            let id = AgentId::from_transcript_path(&p);
            (takes_trip(id, 0)
                && !is_aimless_cycle(id, 0)
                && waypoint_index_for_cycle(id, 0, l.waypoints.len()) == pantry_idx)
                .then_some((p, id))
        })
        .expect("an agent lands at the pantry on cycle 0");

    let now = t0();
    let slot = idle_slot(&path, now); // desk_index 0
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let now = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        now,
        PhaseKind::Seated,
        120_000,
    );
    let _ = now;

    let ms = motion.get(&slot.agent_id).expect("state");
    assert!(matches!(
        ms.wander.target.kind,
        WanderKind::Named {
            kind: WaypointKind::Pantry,
            seat: None, // Pantry is an obstacle (stands AT, not sits ON)
            ..
        }
    ));
    let desk = l.home_desks[0];
    let expected = crate::layout::approach_point(
        WaypointKind::Pantry.furniture(),
        l.waypoints[pantry_idx].pos,
        l.waypoints[pantry_idx].facing,
        l.pantry_counter_size(),
        &l.walkable,
        desk,
        &l.reachable,
    );
    assert_eq!(
        ms.wander.target.dest, expected,
        "motion dest must equal the home-desk approach_point (core↔tui mirror)"
    );
}

// -----------------------------------------------------------------------
// pick_wander_dest aimless fallback: when approach_point returns the blocked
// `wp.pos` "no valid approach" sentinel (no allowed+reachable side), the
// directed-waypoint branch falls back to an aimless dest (kind None). Recipe:
// take a real layout, block the ENTIRE walkable mask so NO approach side is
// reachable for any waypoint, rebuild `reachable`, and drive a NON-aimless
// cycle. `pick_wander_dest` (private; sibling has access) must return kind None.
// -----------------------------------------------------------------------
#[test]
fn pick_wander_dest_falls_back_to_aimless_when_boxed_in() {
    use crate::layout::ReachSet;

    let mut l = layout();
    assert!(!l.waypoints.is_empty(), "layout must have waypoints");
    // Box EVERY waypoint in: block the whole mask so approach_point finds no
    // allowed+reachable side for any waypoint and returns the `pos` sentinel.
    l.walkable
        .mark_blocked(0, 0, l.walkable.width(), l.walkable.height(), 0);
    l.reachable = ReachSet::from_mask(&l.walkable, Point { x: 0, y: 0 });

    // Find an agent whose cycle 0 is a directed (non-aimless) trip so we reach
    // the `else` branch where approach_point is consulted.
    let id = (0u64..2000)
        .map(|i| AgentId::from_transcript_path(&format!("/p/boxed_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0) && !is_aimless_cycle(*id, 0))
        .expect("should find a directed-trip agent");

    let origin = l.home_desks[0];
    let target = pick_wander_dest(id, 0, &l, origin, &SpotClaims::default());

    assert!(
        matches!(target.kind, WanderKind::Aimless),
        "a boxed-in waypoint (no reachable approach side) must amble aimlessly \
         (no waypoint index / kind / seat)",
    );
}

/// Continuity guard for the WanderTarget reshape (the flat
/// `dest`/`dest_kind`/`dest_wp_idx`/`seat` -> `WanderKind::Named{wp_idx,kind,seat}
/// | Aimless` collapse): a `Named` destination's `seat` is `Some` IFF the
/// waypoint is one the agent sits ON (`occupies_pos`), `None` for an obstacle it
/// stands AT — the invariant `seated_foot_cell` enforces. Pins the
/// `Named{seat:None}`-vs-seat boundary across the resolver so a future
/// `WanderKind` edit can't silently drop or forge the settle cell (a mid-walk
/// pop / wrong render anchor).
#[test]
fn wander_named_seat_is_some_iff_the_destination_is_sat_on() {
    use crate::layout::furniture_def;
    // A full-size floor so BOTH obstacle (pantry/vending/printer) AND seat
    // (couch / meeting sofa) waypoints exist to cover both sides of the boundary
    // — the tiny 120x96 `layout()` fixture has no seat waypoints.
    let l = Layout::compute(240, 160, None).expect("fits");
    let origin = l.home_desks[0];
    let (mut saw_obstacle, mut saw_seat) = (false, false);
    for i in 0u64..5000 {
        let id = AgentId::from_transcript_path(&format!("/p/seatpin_{i}.jsonl"));
        if !takes_trip(id, 0) || is_aimless_cycle(id, 0) {
            continue;
        }
        if let WanderKind::Named { kind, seat, .. } =
            pick_wander_dest(id, 0, &l, origin, &SpotClaims::default()).kind
        {
            assert_eq!(
                seat.is_some(),
                furniture_def(kind.furniture()).occupies_pos,
                "Named.seat.is_some() must equal occupies_pos for {kind:?}",
            );
            if seat.is_some() {
                saw_seat = true;
            } else {
                saw_obstacle = true;
            }
        }
    }
    assert!(
        saw_obstacle,
        "sweep must resolve at least one obstacle (seat:None) waypoint"
    );
    assert!(
        saw_seat,
        "sweep must resolve at least one seat (seat:Some) waypoint"
    );
}

// --- spot_claims: who actually holds an exclusive spot ----------------

/// Build a motion state parked mid-trip at `wp_idx` of `kind`.
fn tripping_at(id: AgentId, wp_idx: usize, kind: WaypointKind) -> MotionState {
    let mut ms = MotionState::new(id);
    ms.wander.phase =
        WanderPhase::AtWaypoint(crate::physics::walk_profile(100, WalkIntent::WanderOut, id));
    ms.wander.target = WanderTarget {
        dest: Point { x: 0, y: 0 },
        kind: WanderKind::Named {
            wp_idx,
            kind,
            seat: None,
        },
    };
    ms
}

#[test]
fn spot_claims_holds_only_exclusive_spots_of_other_agents_out_on_a_trip() {
    let me = AgentId::from_transcript_path("/p/claims-me.jsonl");
    let sitter = AgentId::from_transcript_path("/p/claims-sitter.jsonl");
    let caller = AgentId::from_transcript_path("/p/claims-caller.jsonl");
    let stander = AgentId::from_transcript_path("/p/claims-stander.jsonl");
    let mut motion = HashMap::new();
    // Another agent sitting on a seat: claimed.
    motion.insert(sitter, tripping_at(sitter, 3, WaypointKind::MeetingChair));
    // Another agent in a phone booth — occupies_pos FALSE but EXCLUSIVE, so it
    // is claimed too (the whole point of the exclusive/occupies_pos split).
    motion.insert(caller, tripping_at(caller, 9, WaypointKind::PhoneBooth));
    // Another agent at a SHAREABLE queue obstacle: never claimed — the painter's
    // ±9 step-aside is the intended affordance there.
    motion.insert(stander, tripping_at(stander, 7, WaypointKind::Printer));
    // This agent's OWN spot must not block its own re-pick.
    motion.insert(me, tripping_at(me, 5, WaypointKind::Couch));

    let claims = spot_claims(&motion, me);
    assert!(claims.holds(3), "another agent's seat must be claimed");
    assert!(claims.holds(9), "a phone booth is exclusive → claimed");
    assert!(
        !claims.holds(7),
        "a shareable queue obstacle is not claimed"
    );
    assert!(
        !claims.holds(5),
        "an agent must not claim a spot against itself"
    );
}

/// The `phase != Seated` gate. `target.kind` is reset to `Aimless` on the normal
/// walk-back arrival, but the bootstrap / stale-resume path re-seats an agent at
/// its desk WITHOUT touching its target — so a stale `Named` spot would linger
/// and block it for everyone. The phase, not the kind, is the honest "is this
/// agent actually out at the spot" signal.
#[test]
fn spot_claims_ignores_a_seated_agents_stale_target() {
    let me = AgentId::from_transcript_path("/p/claims-me.jsonl");
    let resumed = AgentId::from_transcript_path("/p/claims-resumed.jsonl");
    let mut ms = tripping_at(resumed, 3, WaypointKind::MeetingChair);
    ms.wander.phase = WanderPhase::Seated; // what the stale-resume bootstrap does
    let mut motion = HashMap::new();
    motion.insert(resumed, ms);

    assert!(
        !spot_claims(&motion, me).holds(3),
        "an agent re-seated at its desk must not keep holding a meeting seat"
    );
}

/// The probe: a claimed seat is not a dead end. A venue's seats are CONTIGUOUS
/// in `layout.waypoints` (`compute_waypoints` pushes a sofa's 3 / a table's 2 in
/// one run), so probing forward from the hashed index lands on a neighbouring
/// seat of the SAME venue — "that chair's taken, I'll take the next one" —
/// instead of ambling away and leaving the room empty. Pins BOTH halves: the
/// claimed seat is refused, and the replacement is still a seat in that venue.
#[test]
fn a_claimed_seat_sends_the_agent_to_the_next_seat_of_the_same_venue() {
    let l = Layout::compute(240, 160, None).expect("fits");
    let origin = l.home_desks[0];
    // An agent whose cycle-0 pick is a MEETING seat, so the venue has siblings.
    let (id, taken) = (0u64..5000)
        .map(|i| AgentId::from_transcript_path(&format!("/p/probe_{i}.jsonl")))
        .find_map(|id| {
            if !takes_trip(id, 0) || is_aimless_cycle(id, 0) {
                return None;
            }
            match pick_wander_dest(id, 0, &l, origin, &SpotClaims::default()).kind {
                WanderKind::Named {
                    wp_idx,
                    kind: WaypointKind::MeetingSofa | WaypointKind::MeetingChair,
                    ..
                } => Some((id, wp_idx)),
                _ => None,
            }
        })
        .expect("some agent must pick a meeting seat on cycle 0");

    let mut claims = SpotClaims::default();
    claims.claim(taken);
    let target = pick_wander_dest(id, 0, &l, origin, &claims);
    let WanderKind::Named { wp_idx, kind, .. } = target.kind else {
        panic!("a claimed seat must not strand the agent into an aimless amble");
    };
    assert_ne!(wp_idx, taken, "the claimed seat must be refused");
    assert_eq!(
        l.waypoints[wp_idx].room_id, l.waypoints[taken].room_id,
        "the probe should land in the same meeting room, not walk off to another floor spot"
    );
    assert!(
        matches!(kind, WaypointKind::MeetingSofa | WaypointKind::MeetingChair),
        "the probe should find another meeting slot, got {kind:?}"
    );
}
