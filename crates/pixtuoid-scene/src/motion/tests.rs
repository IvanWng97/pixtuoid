use super::*;
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
    assert!(ms.wander.profile.is_none());
    assert!(ms.wander.dest_kind.is_none());
    assert!(ms.wander.dest_wp_idx.is_none());
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
        label: Arc::from("cc"),
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
    motion
        .get(&id)
        .and_then(|ms| ms.wander.dest_kind)
        .map_or(WANDER_DWELL_EST_MS, |k| dwell_ms(k, id))
}

/// Poll `advance_wander` in ~1 s steps (well under the `stale_resume_gap_ms`
/// stale-resume trigger, so a long seated/dwell beat is crossed exactly as
/// real per-frame rendering would, never looking like an off-screen gap)
/// until the agent's phase is no longer `from_phase`. Returns the new `now`.
/// Panics if the transition doesn't happen within `timeout_ms`.
#[allow(clippy::too_many_arguments)]
fn advance_until_leaves(
    slot: &AgentSlot,
    l: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    motion: &mut HashMap<AgentId, MotionState>,
    mut now: SystemTime,
    from_phase: WanderPhase,
    timeout_ms: u64,
) -> SystemTime {
    const STEP_MS: u64 = 1_000;
    let start = now;
    while motion.get(&slot.agent_id).map(|m| m.wander.phase) == Some(from_phase) {
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
        WanderPhase::Seated,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state present");
    assert!(
        matches!(ms.wander.phase, WanderPhase::WalkingOut),
        "after seated dwell on trip cycle, expected WalkingOut, got {:?}",
        ms.wander.phase
    );
    assert!(
        ms.wander.profile.is_some(),
        "walk-out profile must be snapshotted"
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
        WanderPhase::Seated,
        60_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::WalkingOut
    ));
    // → AtWaypoint (short walk, arrives within a couple of 1 s steps)
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        WanderPhase::WalkingOut,
        20_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander.phase, WanderPhase::AtWaypoint),
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
        WanderPhase::Seated,
        60_000,
    );
    let t2 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        WanderPhase::WalkingOut,
        20_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::AtWaypoint
    ));
    // Cross the (long) per-spot dwell.
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t2,
        WanderPhase::AtWaypoint,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander.phase, WanderPhase::WalkingBack),
        "expected WalkingBack after dwell, got {:?}",
        ms.wander.phase
    );
    assert!(
        ms.wander.profile.is_some(),
        "walk-back profile must be snapshotted"
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
        WanderPhase::Seated,
        60_000,
    );
    let t = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        WanderPhase::WalkingOut,
        20_000,
    );
    let t = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        WanderPhase::AtWaypoint,
        60_000,
    );
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t,
        WanderPhase::WalkingBack,
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
            WanderPhase::Seated,
            60_000,
        );
        let at_wp_enter = advance_until_leaves(
            &slot,
            &l,
            &mut router,
            &overlay,
            &mut motion,
            t1,
            WanderPhase::WalkingOut,
            20_000,
        );
        let walk_back_enter = advance_until_leaves(
            &slot,
            &l,
            &mut router,
            &overlay,
            &mut motion,
            at_wp_enter,
            WanderPhase::AtWaypoint,
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
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

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
// T9: Arrival pause holds WalkingOut phase during [T, T+pause)
// -----------------------------------------------------------------------
#[test]
fn arrival_pause_holds_walking_out_phase() {
    use pixtuoid_core::physics::{walk_arrived, walk_profile, WalkIntent};

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
        WanderPhase::Seated,
        60_000,
    );
    let out_started = motion.get(&trip_id).unwrap().wander.phase_started_at;
    let actual_profile = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander.profile.as_ref())
        .expect("profile snapshotted");
    let actual_mid_elapsed = actual_profile.duration_ms + actual_profile.pause_ms / 2;

    // Mid-pause: still WalkingOut (walk_arrived returns false). This sample is
    // within ~1 s of t1, far below the stale trigger.
    let _ = t1;
    let mid = out_started + Duration::from_millis(actual_mid_elapsed);
    advance_wander(&slot, mid, &l, &mut router, &overlay, &mut motion);
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander.phase,
            WanderPhase::WalkingOut
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
        WanderPhase::Seated,
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
        WanderPhase::Seated,
        60_000,
    );
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander.phase,
            WanderPhase::WalkingOut
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
        WanderPhase::Seated,
        60_000,
    );
    let t2 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        WanderPhase::WalkingOut,
        20_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::AtWaypoint
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
            WanderPhase::AtWaypoint
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
        WanderPhase::Seated,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert_eq!(ms.wander.phase, WanderPhase::WalkingOut);
    let (_, routed_to) = *router.calls.last().expect("the trip snapshot routed");
    assert_eq!(
        routed_to,
        jitter_dest(trip_id, ms.wander.dest),
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
    ms.wander.dest = Point { x: 40, y: 60 };

    let _ = snapshot_back_profile(&slot, &ms, &l, &mut router, &overlay);

    let (snap_to, _) = desk_leg_endpoint(l.home_desks[0], &l);
    assert_eq!(
        router.calls,
        vec![(ms.wander.dest, jitter_dest(trip_id, snap_to))],
        "the walk-back profile must route to the render's jittered desk goal"
    );
}

// -----------------------------------------------------------------------
// Missing-profile warn latch: the Missing arm fires per frame while the
// corrupt state persists; the warn must fire once per agent per episode
// (repeats downgraded to trace), re-arming when the profile recovers.
// -----------------------------------------------------------------------
#[test]
fn missing_profile_warn_latches_once_per_episode() {
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

    let now = t0();
    let slot = idle_slot("/p/latch.jsonl", now - Duration::from_secs(90));
    let mut ms = MotionState::new(slot.agent_id);
    ms.wander.phase = WanderPhase::WalkingOut;
    assert!(!ms.missing_profile_warned);

    assert!(matches!(
        poll_walk_leg(&slot, &mut ms, WanderPhase::WalkingOut, 0, true),
        WalkLegStatus::Missing
    ));
    assert!(
        ms.missing_profile_warned,
        "the first miss must latch the warn"
    );

    assert!(matches!(
        poll_walk_leg(&slot, &mut ms, WanderPhase::WalkingOut, 33, true),
        WalkLegStatus::Missing
    ));
    assert!(
        ms.missing_profile_warned,
        "repeats stay latched (trace, not warn)"
    );

    // A recovered profile ends the episode and re-arms the warn.
    ms.wander.profile = Some(walk_profile(100, WalkIntent::WanderOut, slot.agent_id));
    assert!(!matches!(
        poll_walk_leg(&slot, &mut ms, WanderPhase::WalkingOut, 33, true),
        WalkLegStatus::Missing
    ));
    assert!(
        !ms.missing_profile_warned,
        "a recovered profile must re-arm the warn for the next episode"
    );
}

// -----------------------------------------------------------------------
// Dest mirror: the motion walk destination for a furniture waypoint must
// equal core::layout::stand_point computed with the agent's HOME DESK as
// origin — the same call core::pose::idle_pose and both render anchors make.
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
        WanderPhase::Seated,
        120_000,
    );
    let _ = now;

    let ms = motion.get(&slot.agent_id).expect("state");
    assert_eq!(ms.wander.dest_kind, Some(WaypointKind::Pantry));
    let desk = l.home_desks[0];
    let expected = pixtuoid_core::layout::approach_point(
        WaypointKind::Pantry.furniture(),
        l.waypoints[pantry_idx].pos,
        l.waypoints[pantry_idx].facing,
        l.pantry_counter_size,
        &l.walkable,
        desk,
        &l.reachable,
    );
    assert_eq!(
        ms.wander.dest, expected,
        "motion dest must equal the home-desk approach_point (core↔tui mirror)"
    );
}

// -----------------------------------------------------------------------
// Missing-profile recover: a WalkingOut / WalkingBack phase with a
// `wander.profile == None` is "shouldn't happen", but the convention is to
// warn + recover (never freeze silently). Drive that arm directly by
// pre-inserting a corrupt MotionState and asserting advance_wander returns
// `(phase, 0)` without panicking.
// -----------------------------------------------------------------------

/// Insert a MotionState already in `phase` with NO walk profile, anchored so
/// the bootstrap/stale-resume re-seed does NOT fire (phase clock at `now`,
/// last_advanced just before `now`, slot Idle well before that).
fn corrupt_walking_state(
    motion: &mut HashMap<AgentId, MotionState>,
    id: AgentId,
    now: SystemTime,
    phase: WanderPhase,
) {
    let mut ms = MotionState::new(id);
    ms.wander.phase = phase;
    ms.wander.profile = None;
    ms.wander.phase_started_at = now;
    ms.wander.last_advanced_at = now - Duration::from_millis(33);
    motion.insert(id, ms);
}

#[test]
fn walking_out_missing_profile_recovers_without_panic() {
    let now = t0();
    let slot = idle_slot("/p/recover_out.jsonl", now - Duration::from_secs(90));
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    corrupt_walking_state(&mut motion, slot.agent_id, now, WanderPhase::WalkingOut);

    let (phase, t) = advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    assert_eq!(
        phase,
        WanderPhase::WalkingOut,
        "must recover, staying WalkingOut"
    );
    assert_eq!(t, 0, "missing-profile recover returns t_x1000 == 0");
}

#[test]
fn walking_back_missing_profile_recovers_without_panic() {
    let now = t0();
    let slot = idle_slot("/p/recover_back.jsonl", now - Duration::from_secs(90));
    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    corrupt_walking_state(&mut motion, slot.agent_id, now, WanderPhase::WalkingBack);

    let (phase, t) = advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    assert_eq!(
        phase,
        WanderPhase::WalkingBack,
        "must recover, staying WalkingBack"
    );
    assert_eq!(t, 0, "missing-profile recover returns t_x1000 == 0");
}

// -----------------------------------------------------------------------
// AtWaypoint re-snapshot fallback: the back profile is normally snapshotted
// at WalkingOut arrival, but if it goes missing during the dwell the
// AtWaypoint→WalkingBack transition re-snapshots it. Drive an agent to
// AtWaypoint normally, NULL its profile, then advance past the dwell and
// assert it transitions to WalkingBack with a fresh profile.
// -----------------------------------------------------------------------
#[test]
fn at_waypoint_resnapshots_back_profile_when_missing() {
    let trip_id = trip_agent("resnap");
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
        WanderPhase::Seated,
        60_000,
    );
    let t2 = advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t1,
        WanderPhase::WalkingOut,
        20_000,
    );
    assert!(matches!(
        motion.get(&trip_id).unwrap().wander.phase,
        WanderPhase::AtWaypoint
    ));

    // Simulate the back profile going missing during the dwell.
    motion.get_mut(&trip_id).unwrap().wander.profile = None;

    // Cross the per-spot dwell — the AtWaypoint arm must re-snapshot the back
    // profile rather than freeze.
    advance_until_leaves(
        &slot,
        &l,
        &mut router,
        &overlay,
        &mut motion,
        t2,
        WanderPhase::AtWaypoint,
        60_000,
    );

    let ms = motion.get(&trip_id).expect("state");
    assert_eq!(
        ms.wander.phase,
        WanderPhase::WalkingBack,
        "must transition to WalkingBack after the dwell"
    );
    assert!(
        ms.wander.profile.is_some(),
        "the back profile must be freshly re-snapshotted, not left None"
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
    use pixtuoid_core::layout::ReachSet;

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
    let (_dest, kind, wp_idx, seat) = pick_wander_dest(id, 0, &l, origin);

    assert_eq!(
        kind, None,
        "a boxed-in waypoint (no reachable approach side) must amble aimlessly"
    );
    assert_eq!(wp_idx, None, "aimless fallback carries no waypoint index");
    assert_eq!(seat, None, "aimless fallback carries no seat cell");
}
