use super::*;
use pixtuoid_core::AgentId;

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
    assert_eq!(ms.wander_cycle_n, 0);
    assert_eq!(ms.wander_phase, WanderPhase::Seated);
    assert_eq!(ms.wander_phase_started_at, SystemTime::UNIX_EPOCH);
    assert_eq!(ms.last_advanced_at, SystemTime::UNIX_EPOCH);
    assert!(ms.wander_profile.is_none());
    assert!(ms.wander_dest_kind.is_none());
    assert!(ms.wander_dest_wp_idx.is_none());
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

use crate::tui::layout::Layout;
use crate::tui::pathfind::Router;
use crate::tui::pose::{
    cycle_ms_for, dwell_ms, est_wander_cycle_ms, seated_dwell_ms, takes_trip, WANDER_DWELL_EST_MS,
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
        desk_index: 0,
        floor_idx: 0,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
    }
}

fn layout() -> Layout {
    Layout::compute(120, 96, 4).expect("fits")
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
        .and_then(|ms| ms.wander_dest_kind)
        .map_or(WANDER_DWELL_EST_MS, |k| dwell_ms(k, id))
}

/// Poll `advance_wander` in ~1 s steps (well under the `cycle_ms_for`
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
    while motion.get(&slot.agent_id).map(|m| m.wander_phase) == Some(from_phase) {
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
        matches!(ms.wander_phase, WanderPhase::Seated),
        "fresh idle should init to Seated, got {:?}",
        ms.wander_phase
    );
    assert_eq!(ms.wander_cycle_n, 0);
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
        matches!(ms.wander_phase, WanderPhase::WalkingOut),
        "after seated dwell on trip cycle, expected WalkingOut, got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_profile.is_some(),
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
                motion.get(&stay_id).unwrap().wander_phase,
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
        motion.get(&trip_id).unwrap().wander_phase,
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
        matches!(ms.wander_phase, WanderPhase::AtWaypoint),
        "expected AtWaypoint after walk-out arrival, got {:?}",
        ms.wander_phase
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
        motion.get(&trip_id).unwrap().wander_phase,
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
        matches!(ms.wander_phase, WanderPhase::WalkingBack),
        "expected WalkingBack after dwell, got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_profile.is_some(),
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
        matches!(ms.wander_phase, WanderPhase::Seated),
        "completed cycle must reset to Seated, got {:?}",
        ms.wander_phase
    );
    assert_eq!(ms.wander_cycle_n, 1, "cycle_n must increment once");
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
    let out_started = motion.get(&trip_id).unwrap().wander_phase_started_at;
    let actual_profile = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander_profile.as_ref())
        .expect("profile snapshotted");
    let actual_mid_elapsed = actual_profile.duration_ms + actual_profile.pause_ms / 2;

    // Mid-pause: still WalkingOut (walk_arrived returns false). This sample is
    // within ~1 s of t1, far below the stale trigger.
    let _ = t1;
    let mid = out_started + Duration::from_millis(actual_mid_elapsed);
    advance_wander(&slot, mid, &l, &mut router, &overlay, &mut motion);
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander_phase,
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
        (ms.wander_phase, ms.wander_cycle_n)
    };

    // Call again with the SAME `now` (t1) — must NOT mutate.
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert_eq!(
        ms.wander_phase, phase_before,
        "2nd call with same now must not change phase"
    );
    assert_eq!(
        ms.wander_cycle_n, cycle_before,
        "2nd call with same now must not change cycle_n"
    );
}

// -----------------------------------------------------------------------
// T11: Bootstrap — agent idle for N cycles before first render. cycle_n is
//      fast-forwarded by the ESTIMATED cycle (matches idle_pose), not the
//      stale-resume sentinel cycle_ms_for.
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
        ms.wander_cycle_n, 10,
        "bootstrap: elapsed = 10*est_cycle => cycle_n must equal exactly 10"
    );
}

// -----------------------------------------------------------------------
// T12: Stale resume — a floor off-screen (motion frozen) must resync
//      analytically on return instead of replaying the backlog one phase per
//      frame. Trigger: gap > cycle_ms_for; fast-forward divides by est cycle.
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
            motion.get(&trip_id).unwrap().wander_phase,
            WanderPhase::WalkingOut
        ),
        "precondition: agent should be WalkingOut before the gap"
    );

    // Floor goes off-screen for ~20 cycles; advance_wander is NOT called.
    // The gap dwarfs the stale trigger; a SINGLE call on return must resync.
    assert!(20 * est_cycle > cycle_ms_for(trip_id));
    let resume = t1 + Duration::from_millis(20 * est_cycle);
    advance_wander(&slot, resume, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert!(
        matches!(ms.wander_phase, WanderPhase::Seated),
        "stale resume must resync to Seated (no per-frame replay), got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_cycle_n >= 18,
        "stale resume must fast-forward cycle_n across the gap, got {}",
        ms.wander_cycle_n
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
        motion.get(&trip_id).unwrap().wander_phase,
        WanderPhase::AtWaypoint
    ));

    // Sample every 33 ms across (almost) the full dwell window. Even for a 40 s
    // sofa lounge the per-frame gap stays ~33 ms, so the stale-resume (gap >
    // cycle_ms_for) must never fire — the agent must NOT snap to Seated.
    // Base the window on the ACTUAL AtWaypoint phase start (the poll-observed
    // `t2` can lag the real transition by up to one 1 s step), leaving a 2 s
    // margin so we stop before the dwell genuinely ends.
    let at_wp_start = motion.get(&trip_id).unwrap().wander_phase_started_at;
    let dwell_dur = current_dwell_dur(&motion, trip_id);
    let mut t = t2;
    let end = at_wp_start + Duration::from_millis(dwell_dur.saturating_sub(2_000));
    while t < end {
        t += Duration::from_millis(33);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
        assert!(
            !matches!(
                motion.get(&trip_id).unwrap().wander_phase,
                WanderPhase::Seated
            ),
            "long on-screen dwell wrongly tripped stale-resume (snapped to Seated mid-dwell)"
        );
    }
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander_phase,
            WanderPhase::AtWaypoint
        ),
        "agent should still be AtWaypoint just before the dwell ends"
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
    assert_eq!(ms.wander_dest_kind, Some(WaypointKind::Pantry));
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
        ms.wander_dest, expected,
        "motion dest must equal the home-desk approach_point (core↔tui mirror)"
    );
}
