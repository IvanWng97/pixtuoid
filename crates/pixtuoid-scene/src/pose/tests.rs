use super::*;
use crate::motion::{octile_path_len, settle_len};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex, ToolKind};
use pixtuoid_core::walkable::WalkableMask;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Stub router for testing — returns a pre-baked polyline so segment
/// mapping can be exercised without real A* over a layout.
struct StubRouter {
    path: Vec<Point>,
}

impl StubRouter {
    /// Straight-line: `route` returns `[from, to]` regardless of input.
    fn straight() -> Self {
        Self { path: vec![] }
    }
    /// Hardcoded polyline; the binary's `derive_with_routing` then
    /// restores the last point to the original `to` per the
    /// jitter-correction logic.
    fn corners(path: Vec<Point>) -> Self {
        Self { path }
    }
}

impl Router for StubRouter {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &pixtuoid_core::walkable::OccupancyOverlay,
        from: Point,
        to: Point,
    ) -> Vec<Point> {
        if self.path.is_empty() {
            vec![from, to]
        } else {
            self.path.clone()
        }
    }
    fn invalidate(&mut self) {}
}

fn layout() -> Layout {
    Layout::compute(120, 96, Some(4)).expect("fits")
}

/// Router that returns a stable polyline (`first`) for its first few calls
/// then a DIFFERENT one (`rest`) — simulating an overlay-driven A* reroute
/// mid-walk. Counts calls so a test can prove a frozen leg never re-routes.
struct ChangingRouter {
    calls: usize,
    first: Vec<Point>,
    rest: Vec<Point>,
}
impl Router for ChangingRouter {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &pixtuoid_core::walkable::OccupancyOverlay,
        _from: Point,
        _to: Point,
    ) -> Vec<Point> {
        self.calls += 1;
        // Frame 1 makes two calls (entry-profile snapshot + route_walking_pose);
        // both must agree so the frozen path is deterministic. A reroute on a
        // later frame would be call #3+ and gets the different `rest` shape.
        if self.calls <= 2 {
            self.first.clone()
        } else {
            self.rest.clone()
        }
    }
    fn invalidate(&mut self) {}
}

#[test]
fn walk_leg_freezes_path_against_midleg_reroute() {
    // An entry walker snapshots its A* polyline on the first frame. On a
    // later frame the router would return a DIFFERENT shape (overlay
    // churn). The walker must keep following the FROZEN first polyline and
    // make NO router call that frame — else the sprite jumps ("flash") and
    // the per-frame A* cost spikes (the periodic stutter).
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold.expect("door");
    let desk = l.home_desks[0];
    let desk_target = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    // Distinct mid corners so the two shapes are distinguishable.
    let mid_a = Point {
        x: door.x,
        y: (door.y + desk_target.y) / 2,
    };
    let mid_b = Point {
        x: desk_target.x,
        y: door.y,
    };
    assert_ne!(mid_a, mid_b, "test setup: corners must differ");

    let mut router = ChangingRouter {
        calls: 0,
        first: vec![door, mid_a, desk_target],
        rest: vec![door, mid_b, desk_target],
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Frame 1: 200ms into the entry walk — snapshots profile + path.
    let slot1 = entry_slot(now - Duration::from_millis(200));
    let _ = derive_with_routing(
        &slot1,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let calls_after_frame1 = router.calls;

    // Frame 2: 100ms later — router WOULD return the `rest` shape if asked.
    let slot2 = entry_slot(now - Duration::from_millis(200));
    let later = now + Duration::from_millis(100);
    let _ = derive_with_routing(
        &slot2,
        later,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );

    // The freeze means frame 2 re-routes nothing.
    assert_eq!(
        router.calls,
        calls_after_frame1,
        "frozen leg must not re-route on a later frame (got {} extra calls)",
        router.calls - calls_after_frame1
    );

    let frozen = motion
        .get(&slot2.agent_id)
        .and_then(|ms| ms.walk_path.as_ref())
        .expect("walk_path must be snapshotted while walking");
    assert!(
        frozen.path.contains(&mid_a),
        "frozen path must keep the first leg's corner {mid_a:?}, got {:?}",
        frozen.path
    );
    assert!(
        !frozen.path.contains(&mid_b),
        "frozen path must NOT adopt the rerouted corner {mid_b:?} mid-leg, got {:?}",
        frozen.path
    );
}

fn active_slot(state_started_at: SystemTime, created_at: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: AgentId::from_transcript_path("/snap.jsonl"),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: "cc".into(),
        state: ActivityState::Active {
            tool_use_id: Some(Arc::from("t")),
            detail: Some(Arc::from("Edit")),
            kind: ToolKind::Edit,
        },
        state_started_at,
        last_event_at: created_at,
        created_at,
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

fn entry_slot(created_at: SystemTime) -> AgentSlot {
    let mut s = active_slot(created_at, created_at);
    s.state = ActivityState::Idle;
    s
}

#[test]
fn snap_back_walks_from_history_when_state_just_flipped() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Far waypoint position recorded one frame ago: snap-back should fire.
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    match derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    ) {
        Some(Pose::Walking { from, .. }) => {
            assert_eq!(from, prev, "snap-back walk should start from recorded prev");
        }
        other => panic!("expected snap-back Walking pose, got {other:?}"),
    }
}

#[test]
fn seated_waypoint_snap_back_starts_from_the_seat_not_the_approach_cell() {
    // A seated agent (AtWaypoint on a seat) whose slot flips Active must snap
    // back FROM the rendered seat, not the off-side approach cell. The AtWaypoint
    // arm records history; recording `wander.dest` (the approach cell) instead of
    // `wander.seat` (the rendered seat) popped the sprite ~half_extent on the
    // first snap-back frame — the one seated departure the WalkingBack Settle
    // machinery doesn't cover. Unlike the hand-injected snap-back tests, this
    // drives the real AtWaypoint arm so the seat-vs-approach divergence is guarded.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk = l.home_desks[0];
    // Distinct seat vs approach cells, both far from the desk chair (snap arms).
    let seat = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let approach = Point {
        x: desk.x + 56,
        y: desk.y + 30,
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Frame 1: Idle, seated AtWaypoint on a seat → records history at the SEAT.
    let idle = entry_slot(now - Duration::from_secs(60));
    let mut ms = MotionState::new(idle.agent_id);
    ms.wander.phase = crate::motion::WanderPhase::AtWaypoint(walk_profile(
        100,
        WalkIntent::WanderBack,
        idle.agent_id,
    ));
    ms.wander.phase_started_at = now;
    ms.wander.last_advanced_at = now; // pin the phase (advance_wander no-ops at now)
    ms.wander.target = crate::motion::WanderTarget {
        dest: approach,
        kind: crate::motion::WanderKind::Named {
            wp_idx: 0,
            kind: crate::layout::WaypointKind::Couch,
            seat: Some(seat),
        },
    };
    motion.insert(idle.agent_id, ms);
    match derive_with_routing(
        &idle,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    ) {
        Some(Pose::AtWaypoint { .. }) => {}
        other => panic!("expected AtWaypoint pose, got {other:?}"),
    }

    // Frame 2: the slot flips Active (a tool call arrives) → snap-back arms and
    // must start FROM the seat recorded above, not the approach cell.
    let active = active_slot(now, now - Duration::from_secs(60));
    let then = now + Duration::from_millis(50);
    match derive_with_routing(
        &active,
        then,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    ) {
        Some(Pose::Walking { from, .. }) => assert_eq!(
            from, seat,
            "snap-back must start from the rendered seat {seat:?}, not the approach cell {approach:?}"
        ),
        other => panic!("expected snap-back Walking pose, got {other:?}"),
    }
}

#[test]
fn snap_back_origin_is_frozen_across_frames() {
    // A snap-back is a walk FROM the interruption point TO the desk. Its
    // origin is captured once (the `_snap_prev` field of the snap_back tuple)
    // and must stay put for the whole leg — exactly like the EXIT branch,
    // which freezes its origin Point and reuses it every frame.
    //
    // Regression: the origin was re-read from PoseHistory every frame
    // (`from: prev` at the consuming arm). Because route_walking_pose records
    // the advancing walker position into the single-slot history each frame,
    // the next frame read that advanced point back as the "origin" — so the
    // walk's `from` crept toward the desk frame-by-frame (a contraction, not a
    // walk from a fixed start). That made the leg finish faster than its frozen
    // physics profile intends and defeated the walk_path freeze (the per-frame
    // `from` drift means the freeze's `wp.from == from` reuse guard stops
    // matching). Assert the origin is identical on every frame of the leg.
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // state_started_at == now0: the leg arms on frame 0 and stays armed (the
    // re-arm guard keys on state_started_at, which is constant here).
    let slot = active_slot(now0, now0 - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // ~80px manhattan from the desk: far enough that the pre-fix integer-pixel
    // drift surfaces within the 8-frame window (empirically first drifts at
    // frame 4). A snap near SNAP_BACK_MIN_DIST=8 could delay the first integer
    // drift past the window and false-pass on broken code.
    let prev0 = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev0, now0 - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Step several frames well inside the 900 ms window (8 × 33 ms = 231 ms),
    // re-deriving each frame so route_walking_pose advances history just like
    // the real render loop does.
    let mut origins = Vec::new();
    for i in 0..8u64 {
        let t = now0 + Duration::from_millis(i * 33);
        match derive_with_routing(
            &slot,
            t,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            Some(Pose::Walking { from, .. }) => origins.push((i, from)),
            other => panic!("frame {i}: expected Walking pose mid snap-back, got {other:?}"),
        }
    }
    for (i, from) in origins {
        assert_eq!(
            from, prev0,
            "frame {i}: snap-back origin drifted to {from:?}; it must stay frozen at \
             the interruption point {prev0:?} for the whole leg"
        );
    }
}

#[test]
fn snap_back_cornered_leg_freezes_path_no_reroute() {
    // Companion to `walk_leg_freezes_path_against_midleg_reroute` (entry walk),
    // for the snap-back leg. A CORNERED snap-back (>2-point route) must
    // snapshot its A* polyline once and reuse it, making NO router call on
    // later frames — else the per-frame A* cost spikes and an overlay-churn
    // reroute remaps frozen progress onto a new shape (the "flash"). This only
    // holds because the origin is frozen: a per-frame-drifting `from` misses
    // the `wp.from == from` reuse guard and re-routes every frame.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk = l.home_desks[0];
    let snap_target = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    let prev0 = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    // Distinct corners so the frozen vs rerouted shapes are distinguishable.
    let corner_a = Point {
        x: prev0.x,
        y: snap_target.y,
    };
    let corner_b = Point {
        x: snap_target.x,
        y: prev0.y,
    };
    assert_ne!(corner_a, corner_b, "test setup: corners must differ");

    let mut router = ChangingRouter {
        calls: 0,
        first: vec![prev0, corner_a, snap_target],
        rest: vec![prev0, corner_b, snap_target],
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // State flipped 100ms ago — inside the 900ms snap-back window on both frames.
    let slot = active_slot(
        now - Duration::from_millis(100),
        now - Duration::from_secs(60),
    );
    history.record(slot.agent_id, prev0, now - Duration::from_millis(50));

    // Frame 1: arms the snap-back and snapshots the cornered walk_path. The
    // arm routes once (to measure the rendered polyline for the profile) and
    // route_walking_pose routes once for the freeze snapshot — both within
    // ChangingRouter's `first` window, so they agree on the shape.
    let _ = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let calls_after_frame1 = router.calls;
    assert!(
        calls_after_frame1 >= 1,
        "frame 1 must route once to snapshot the cornered leg"
    );

    // Frame 2: 100ms later. The router WOULD return the `rest` shape if asked.
    let later = now + Duration::from_millis(100);
    let _ = derive_with_routing(
        &slot,
        later,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );

    // The frozen origin keeps `wp.from == from` matching, so frame 2 re-routes
    // nothing. Pre-fix the drifting `from` missed the guard → a fresh call here.
    assert_eq!(
        router.calls,
        calls_after_frame1,
        "frozen cornered snap-back must not re-route on a later frame (got {} extra calls)",
        router.calls - calls_after_frame1
    );
    let frozen = motion
        .get(&slot.agent_id)
        .and_then(|ms| ms.walk_path.as_ref())
        .expect("walk_path must be snapshotted while snapping back");
    assert!(
        frozen.path.contains(&corner_a),
        "frozen path must keep the first corner {corner_a:?}, got {:?}",
        frozen.path
    );
    assert!(
        !frozen.path.contains(&corner_b),
        "frozen path must NOT adopt the rerouted corner {corner_b:?} mid-leg, got {:?}",
        frozen.path
    );
}

#[test]
fn snap_back_derive_is_idempotent_within_a_frame() {
    // Regression: the render loop calls derive_with_routing up to 4x per agent
    // per frame (seated map, sprite paint, hit-test, label) at the SAME `now`,
    // sharing one history + motion. It must be idempotent within a frame —
    // every call returns the same pose and its side-effects (history.record,
    // snap_back arm/clear) settle to the same state — else the label and
    // hit-box lead the painted sprite (the "K-call desync"). This holds only
    // because every history-consuming walk freezes its origin: snap-back since
    // #66, exit always, wander via advance_wander's last_advanced_at. A future
    // walk that re-reads the advancing history position per frame would
    // re-break it. Step a full snap-back lifecycle (arm -> walk -> walk_arrived
    // clear), deriving 4x per frame, asserting pose + history are stable.
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now0, now0 - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // A snap-back far enough from the CHAIR to arm (≥ SNAP_BACK_MIN_DIST), stepped
    // through its full lifecycle: arm → physics walk → walk_arrived clear → seated.
    // The chair-distance arm gate means it does NOT re-arm once the walk reaches the
    // chair (dist 0 < MIN), so it settles cleanly. We derive 4× per frame at the SAME
    // `now` and assert the pose + history are stable every frame (the K-call
    // idempotency invariant): this holds because the override arms ONCE per state
    // transition and renders from the FROZEN origin, never re-reading the advancing
    // history position as a per-frame gate.
    let prev0 = Point {
        x: desk.x + 16,
        y: desk.y + 12,
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev0, now0 - Duration::from_millis(50));
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let mut arrived_frame: Option<u64> = None;
    for i in 0..60u64 {
        let t = now0 + Duration::from_millis(i * 33);
        let p0 = derive_with_routing(
            &slot,
            t,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        );
        let h0 = history.recent(slot.agent_id, 300, t);
        if arrived_frame.is_none()
            && matches!(
                p0,
                Some(Pose::SeatedTyping { .. } | Pose::SeatedIdle | Pose::SeatedThinking)
            )
        {
            arrived_frame = Some(i);
        }
        for k in 1..4 {
            let pk = derive_with_routing(
                &slot,
                t,
                &l,
                &mut crate::pose::RouteCtx {
                    router: &mut router,
                    overlay: &overlay,
                    history: &mut history,
                    motion: &mut motion,
                },
            );
            let hk = history.recent(slot.agent_id, 300, t);
            assert_eq!(
                p0, pk,
                "frame {i} call {k}: pose differs within one frame ({p0:?} vs {pk:?}) — K-call desync"
            );
            assert_eq!(
                h0, hk,
                "frame {i} call {k}: history advanced within one frame ({h0:?} vs {hk:?})"
            );
        }
    }
    // The leg must complete (settle to seated) within the run — proving we stepped a
    // full arm → walk → arrive lifecycle (not just a no-op gate), and that the
    // chair-distance gate stopped it re-arming once it reached the seat.
    assert!(
        arrived_frame.is_some(),
        "snap-back should reach the desk (settle to seated) within the run"
    );
}

#[test]
fn wander_derive_is_idempotent_within_a_frame() {
    // Regression (companion to snap_back_derive_is_idempotent_within_a_frame),
    // real A* router: a wander walk's origin is history-based, so it's the
    // likeliest path to re-break per-frame idempotency. Drive a full coffee-run
    // wander, deriving character_anchor 4x per frame, asserting the anchor is
    // stable within each frame (its advance_wander step is guarded by
    // last_advanced_at).
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/idem/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    let old = now0 - Duration::from_secs(120);
    let mut slot = entry_slot(old);
    slot.agent_id = trip_id;
    slot.last_event_at = old;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    for i in 0..200u64 {
        let t = now0 + Duration::from_millis(i * 33);
        let a0 = character_anchor(
            &slot,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        );
        for k in 1..4 {
            let ak = character_anchor(
                &slot,
                &l,
                t,
                &mut crate::pose::RouteCtx {
                    router: &mut router,
                    overlay: &overlay,
                    history: &mut history,
                    motion: &mut motion,
                },
            );
            assert_eq!(
                a0, ak,
                "frame {i} call {k}: wander anchor differs within one frame ({a0:?} vs {ak:?}) — K-call desync"
            );
        }
    }
}

#[test]
fn snap_back_long_distance_renders_past_window_by_physics() {
    // A far snap-back's physics walk exceeds the SNAP_BACK_MS arm window. The OLD
    // design time-compressed it to finish by 900ms; the NEW design renders it to
    // completion by PHYSICS — no time-compression, no render cap — kept brisk by
    // the snap-back's higher accel + cruise. So PAST the 900ms window it KEEPS
    // WALKING (no window-cut teleport), then settles to the seated pose. We step
    // the leg with persistent motion/history and assert: still Walking after the
    // window, and it eventually arrives.
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now0, now0 - Duration::from_secs(60)); // flip at now0
    let desk = l.home_desks[0];
    // Far prev → SnapBack physics duration well over the 900ms arm window.
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now0 - Duration::from_millis(50));
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let (mut walking_after_window, mut arrived) = (false, false);
    for i in 0..90u64 {
        let t = now0 + Duration::from_millis(i * 33);
        match derive_with_routing(
            &slot,
            t,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            Some(Pose::Walking { .. }) if i * 33 > SNAP_BACK_MS => walking_after_window = true,
            Some(Pose::Walking { .. }) => {}
            // Only counts as "arrived" once we've actually been walking.
            Some(Pose::SeatedTyping { .. } | Pose::SeatedIdle | Pose::SeatedThinking)
                if walking_after_window =>
            {
                arrived = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        walking_after_window,
        "far snap-back must keep WALKING past the {SNAP_BACK_MS}ms arm window — not compressed or window-capped"
    );
    assert!(
        arrived,
        "far snap-back must eventually settle to the seated pose by physics"
    );
}

#[test]
fn snap_back_routes_via_the_approach_cell_then_settles_onto_the_chair() {
    // Snap-back is unified with the other desk legs: it routes to a reachable N/E/W
    // approach cell and SETTLES onto the chair, instead of aiming A* at the blocked
    // chair (which find_path snaps to the nearest — south — cell). The frozen
    // polyline therefore ends [..., approach, chair].
    use crate::layout::desk_walk_anchor;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor(desk);
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let mut slot = active_slot(now, now - Duration::from_secs(60));
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.agent_id = AgentId::from_transcript_path("/snapapproach/slot.jsonl");
    // Far from the chair (≥ SNAP_BACK_MIN_DIST) so the snap-back arms.
    let prev = Point {
        x: chair.x + 40,
        y: chair.y + 25,
    };
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let pose = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    )
    .expect("snap-back renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "snap-back must be Walking, got {pose:?}"
    );
    let snap = motion[&slot.agent_id]
        .walk_path
        .as_ref()
        .expect("the cornered snap-back leg is frozen (… approach, chair, len > 2)");
    assert_eq!(
        snap.path.last(),
        Some(&chair),
        "snap-back must settle onto the chair; got {:?}",
        snap.path
    );
    assert_eq!(
        snap.path[snap.path.len() - 2],
        approach,
        "snap-back must arrive via the N/E/W approach cell, not straight at the chair; got {:?}",
        snap.path
    );
}

#[test]
fn snap_back_skipped_when_prev_within_min_distance() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Only 3 px away — below the 8-px snap-back threshold.
    let close = Point {
        x: desk.x + 3,
        y: desk.y,
    };
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, close, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    assert!(
        matches!(p, Some(Pose::SeatedTyping { .. })),
        "close prev should NOT trigger snap-back, got {p:?}"
    );
}

#[test]
fn snap_back_skipped_after_900ms_window() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // state_started_at is 1.5 s ago — past SNAP_BACK_MS=900.
    let slot = active_slot(
        now - Duration::from_millis(1_500),
        now - Duration::from_secs(60),
    );
    let desk = l.home_desks[0];
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    assert!(
        matches!(p, Some(Pose::SeatedTyping { .. })),
        "snap-back window should be expired at 1.5s, got {p:?}"
    );
}

#[test]
fn snap_back_skipped_without_recent_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let mut history = PoseHistory::new(); // empty
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    assert!(
        matches!(p, Some(Pose::SeatedTyping { .. })),
        "no prev history → raw pose, got {p:?}"
    );
}

#[test]
fn multi_segment_path_maps_t_to_segment_via_octile_distance() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // Entry walk in physics mode: 400ms elapsed. Physics (accel ramp) means
    // the agent is early in the path — earlier than linear t=10% would give.
    // The key check is that the segment-mapper correctly places the agent on
    // segment 0 (door→mid) rather than segment 1, regardless of the exact
    // physics-derived t_x1000.
    let slot = entry_slot(now - Duration::from_millis(400));
    let mut history = PoseHistory::new();
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let door = l.door_threshold.expect("door");
    let desk = l.home_desks[0];
    let mid = Point {
        x: (door.x + desk.x) / 2,
        y: (door.y + desk.y) / 2,
    };
    let mut router = StubRouter::corners(vec![door, mid, desk]);
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    match p {
        Some(Pose::Walking {
            from, to, t_x1000, ..
        }) => {
            assert_eq!(from, door, "first segment starts at door, got {from:?}");
            assert_eq!(to, mid, "first segment ends at mid, got {to:?}");
            // Physics progress at 400ms is in [0,500] — we're on the first segment.
            // The wider band covers both physics (accel) and the old linear case.
            assert!(
                (0..=500).contains(&t_x1000),
                "expected first-segment seg_t in [0,500], got t_x1000={t_x1000}"
            );
            assert!(history.recent(slot.agent_id, 1_000, now).is_some());
        }
        other => panic!("expected Walking on segment 0, got {other:?}"),
    }
}

#[test]
fn at_waypoint_pose_records_position_to_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // Construct a synthetic AtWaypoint pose by going through derive
    // with carefully picked timing is hard — instead, exercise the
    // history-record path by feeding derive an AimlessAt pose via
    // a custom orchestration. Easiest: re-call derive_with_routing
    // for a non-walking pose case. Idle agent with state_started_at
    // not in a trip phase → SeatedIdle (non-walking, non-waypoint).
    // After this call, no history is recorded because SeatedIdle
    // isn't in the "record" list. That's correct behaviour — verify
    // by ensuring history is empty after the call.
    let slot = AgentSlot {
        agent_id: AgentId::from_transcript_path("/idle.jsonl"),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: "cc".into(),
        state: ActivityState::Idle,
        state_started_at: now,
        created_at: now - Duration::from_secs(60),
        last_event_at: now - Duration::from_secs(60),
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
    };
    let mut history = PoseHistory::new();
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let _ = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    // SeatedIdle isn't recorded — that's the contract.
    assert!(
        history.recent(slot.agent_id, 1_000, now).is_none(),
        "SeatedIdle should not write history"
    );
}

#[test]
fn delegates_to_derive_for_oob_desk() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let mut slot = active_slot(now, now - Duration::from_secs(60));
    slot.desk_index = GlobalDeskIndex(999);
    let mut history = PoseHistory::new();
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    assert!(derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion
        }
    )
    .is_none());
}

#[test]
fn pose_history_record_and_recent() {
    let id = AgentId::from_transcript_path("/test/a.jsonl");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let pt = Point { x: 42, y: 99 };
    let mut history = PoseHistory::new();
    assert!(history.recent(id, 500, now).is_none());
    history.record(id, pt, now);
    assert_eq!(history.recent(id, 500, now), Some(pt));
}

#[test]
fn pose_history_recent_expires() {
    let id = AgentId::from_transcript_path("/test/b.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let pt = Point { x: 10, y: 20 };
    let mut history = PoseHistory::new();
    history.record(id, pt, t0);
    let t1 = t0 + Duration::from_millis(600);
    assert_eq!(history.recent(id, 500, t1), None);
    assert_eq!(history.recent(id, 700, t1), Some(pt));
}

// ---- Phase 4: snap-back physics tests ---------------------------------

#[test]
fn snap_back_progress_is_physics_eased_not_linear() {
    // Use a SHORT path (desk + 10px) so the walk is in the TRIANGULAR
    // kinematic regime — distance ∝ t², so at 25% of duration the agent
    // covers only 1/16 of the path (t_x1000 ≈ 62), well below linear's 250.
    //
    // Distance choice: prev = desk+(10,5) → snap_target = desk+(6,4)
    //   dx=4, dy=1, octile = 14*1 + 10*(4-1) = 44 units.
    //   L_crit(max speed) ≈ 287 → 44 is firmly triangular for all agents.
    //   T ≈ 2*sqrt(44/6.5e-4) ≈ 520 ms → T/4 ≈ 130 ms < 300 ms history gate.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Qualifying distance from the CHAIR (manhattan 28 ≥ SNAP_BACK_MIN_DIST=8) so
    // the snap-back actually arms.
    let prev = Point {
        x: desk.x + 20,
        y: desk.y + 18,
    };

    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Frame 0: state just flipped — snapshots the physics profile.
    let _pose0 = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let ms = motion
        .get(&slot.agent_id)
        .expect("MotionState created on frame 0");
    let profile = &ms
        .snap_back
        .as_ref()
        .expect("snap_back profile stored")
        .profile;
    let dur_ms = profile.duration_ms;
    assert!(
        dur_ms > 0,
        "profile duration must be > 0 for a non-trivial distance"
    );

    // Frame 1: exactly 25% of profile duration elapsed.
    // Record history at quarter_now - 50ms so it's fresh (age = 50ms < 300ms gate).
    let slot_q = active_slot(now, now - Duration::from_secs(60));
    let quarter_now = now + Duration::from_millis(dur_ms / 4);
    let mut history2 = PoseHistory::new();
    history2.record(
        slot_q.agent_id,
        prev,
        quarter_now - Duration::from_millis(50),
    );
    let p = derive_with_routing(
        &slot_q,
        quarter_now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history2,
            motion: &mut motion,
        },
    );

    match p {
        Some(Pose::Walking { t_x1000, .. }) => {
            // Physics ease-in: the accel ramp at the start keeps progress SUB-LINEAR
            // through the first quarter of the walk — triangular gives s(T/4)=L/16
            // (t≈62); even a trapezoidal snap (the higher snap-back accel can push a
            // moderate distance past L_crit) is still < linear's 250 at T/4 because
            // the ramp dominates the opening. A linear walk would read exactly 250.
            assert!(
                t_x1000 < 250,
                "physics ease-in: expected t_x1000 < 250 at 25% of duration, got {t_x1000}"
            );
        }
        other => panic!("expected Walking pose at 25% of snap-back duration, got {other:?}"),
    }
}

#[test]
fn snap_back_profile_stored_in_motion_state() {
    // Second call for the same snap-back must REUSE the frozen profile
    // (same duration_ms), not re-snapshot a new one.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };

    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Frame 1: creates the profile.
    let _p1 = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let dur1 = motion
        .get(&slot.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|leg| leg.profile.duration_ms)
        .expect("snap_back profile created on frame 1");

    // Frame 2: 100ms later with fresh history but SAME persistent motion map.
    let slot2 = active_slot(now, now - Duration::from_secs(60));
    let t2 = now + Duration::from_millis(100);
    history.record(slot2.agent_id, prev, t2 - Duration::from_millis(50));
    let _p2 = derive_with_routing(
        &slot2,
        t2,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let dur2 = motion
        .get(&slot2.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|leg| leg.profile.duration_ms)
        .expect("snap_back profile still present on frame 2");

    assert_eq!(
        dur1, dur2,
        "snap-back profile must be snapshotted once and reused across frames"
    );
}

#[test]
fn snap_back_rearms_on_new_state_transition() {
    // A SECOND desk-bound transition within the 900ms window (state_started_at
    // advances while snap_back still holds the T0 tuple) must RE-ARM: the
    // stored `snap_back.0` should track the new `state_started_at`, not the
    // stale T0 — otherwise the snap-back clock jumps mid-progress.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk = l.home_desks[0];
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let mut history = PoseHistory::new();

    // T0: first transition fires a snap-back.
    let t0 = now;
    let slot0 = active_slot(t0, now - Duration::from_secs(60));
    let prev0 = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    history.record(slot0.agent_id, prev0, t0 - Duration::from_millis(50));
    let _ = derive_with_routing(
        &slot0,
        t0,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let stored0 = motion
        .get(&slot0.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|leg| leg.started_at)
        .expect("snap_back armed at T0");
    assert_eq!(stored0, t0, "first arm should key on T0 state_started_at");

    // T0+400ms: a NEW transition (state_started_at advanced) within the window.
    let t1_state = t0 + Duration::from_millis(400);
    // Same agent_id (active_slot uses a fixed transcript path) so the motion
    // entry is reused; only state_started_at moved.
    let slot1 = active_slot(t1_state, now - Duration::from_secs(60));
    let now1 = t1_state; // observe at the new transition instant
    let prev1 = Point {
        x: desk.x + 40,
        y: desk.y + 25,
    };
    history.record(slot1.agent_id, prev1, now1 - Duration::from_millis(50));
    let _ = derive_with_routing(
        &slot1,
        now1,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    let stored1 = motion
        .get(&slot1.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|leg| leg.started_at)
        .expect("snap_back still present after new transition");
    assert_eq!(
        stored1, t1_state,
        "snap-back must re-arm to the NEW state_started_at, not the stale T0"
    );
    assert_ne!(
        stored1, t0,
        "re-armed clock must differ from the old T0 clock"
    );
}

// ---- Phase 3: entry/exit physics tests --------------------------------
// These live alongside the existing snap_back_* tests.
// Requires: physics::walk_profile, motion::MotionState (Phase 0-2 outputs).

/// Build an entry slot (Idle, just created). desk_index 0 = nearest desk.
fn entry_slot_near(created_at: SystemTime) -> AgentSlot {
    let mut s = active_slot(created_at, created_at);
    s.state = pixtuoid_core::state::ActivityState::Idle;
    s.desk_index = GlobalDeskIndex(0);
    s
}

/// Build an entry slot for a far desk index.
fn entry_slot_far(created_at: SystemTime, desk_index: usize) -> AgentSlot {
    let mut s = entry_slot_near(created_at);
    s.desk_index = GlobalDeskIndex(desk_index);
    // Give each far slot a distinct agent_id so speed_mult differs.
    s.agent_id = AgentId::from_transcript_path(&format!("/far/{desk_index}.jsonl"));
    s
}

/// Build an exiting slot: state_started_at from long ago, exiting_at = now.
fn exiting_slot(exiting_at: SystemTime, created_at: SystemTime) -> AgentSlot {
    let mut s = active_slot(exiting_at - Duration::from_secs(30), created_at);
    s.exiting_at = Some(exiting_at);
    s.agent_id = AgentId::from_transcript_path("/exit/slot.jsonl");
    s
}

/// Return (near_desk_index, far_desk_index) by actual octile distance
/// from the door to each desk+offset. Panics if layout has < 2 desks
/// or no door_threshold.
fn near_far_desk_indices(l: &Layout) -> (usize, usize) {
    let door = l.door_threshold.expect("layout must have door_threshold");
    let dists: Vec<u32> = l
        .home_desks
        .iter()
        .map(|d| {
            let target = Point {
                x: d.x + 6,
                y: d.y + 4,
            };
            octile_distance(door, target)
        })
        .collect();
    let near_idx = dists
        .iter()
        .enumerate()
        .min_by_key(|&(_, d)| d)
        .map(|(i, _)| i)
        .unwrap();
    let far_idx = dists
        .iter()
        .enumerate()
        .max_by_key(|&(_, d)| d)
        .map(|(i, _)| i)
        .unwrap();
    assert_ne!(
        dists[near_idx], dists[far_idx],
        "need distinct near/far distances for this test"
    );
    assert!(
        dists[far_idx] >= dists[near_idx] * 3 / 2,
        "far dist ({}) must be ≥ 1.5× near dist ({}) for a meaningful test",
        dists[far_idx],
        dists[near_idx]
    );
    (near_idx, far_idx)
}

#[test]
fn entry_duration_scales_with_path_longer_desk_takes_longer() {
    // Compute the actual nearest and farthest desks by octile distance
    // from the door (Correction M — don't assume desk 0 is nearest).
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout(); // 120×96, 4 desks
    let (near_idx, far_idx) = near_far_desk_indices(&l);

    let near = entry_slot_far(now, near_idx);
    let far = entry_slot_far(now, far_idx);

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();

    // Two separate motion maps — each agent's first call snapshots its own profile.
    let mut motion_near: HashMap<AgentId, MotionState> = HashMap::new();
    let mut motion_far: HashMap<AgentId, MotionState> = HashMap::new();
    let mut hist_near = PoseHistory::new();
    let mut hist_far = PoseHistory::new();
    let mut router_n = StubRouter::straight();
    let mut router_f = StubRouter::straight();

    // First call: snapshots the entry profile.
    let _pn = derive_with_routing(
        &near,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_n,
            overlay: &overlay,
            history: &mut hist_near,
            motion: &mut motion_near,
        },
    );
    let _pf = derive_with_routing(
        &far,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_f,
            overlay: &overlay,
            history: &mut hist_far,
            motion: &mut motion_far,
        },
    );

    let dur_near = motion_near[&near.agent_id]
        .entry
        .as_ref()
        .expect("entry profile set for near desk")
        .1
        .duration_ms;
    let dur_far = motion_far[&far.agent_id]
        .entry
        .as_ref()
        .expect("entry profile set for far desk")
        .1
        .duration_ms;

    assert!(
        dur_far >= dur_near,
        "far desk duration {dur_far}ms must be >= near desk {dur_near}ms"
    );
}

#[test]
fn nearer_desk_arrives_before_farther_desk() {
    // Same created_at, same StubRouter (straight-line). Run enough frames
    // so the near desk agent walk_arrived flips; the far desk must still
    // be Walking at that point. Desks are chosen by actual octile distance
    // from the door (Correction M).
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let (near_idx, far_idx) = near_far_desk_indices(&l);

    let near = entry_slot_far(now, near_idx);
    let far = entry_slot_far(now, far_idx);

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion_near = HashMap::new();
    let mut motion_far = HashMap::new();
    let mut hist_near = PoseHistory::new();
    let mut hist_far = PoseHistory::new();
    let mut router_n = StubRouter::straight();
    let mut router_f = StubRouter::straight();

    // Snapshot on first call.
    let _ = derive_with_routing(
        &near,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_n,
            overlay: &overlay,
            history: &mut hist_near,
            motion: &mut motion_near,
        },
    );
    let _ = derive_with_routing(
        &far,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_f,
            overlay: &overlay,
            history: &mut hist_far,
            motion: &mut motion_far,
        },
    );

    // Advance time past the near desk's duration+pause but stay within
    // the far desk's window. Use the near desk's profile to compute exact time.
    let near_profile = motion_near[&near.agent_id].entry.unwrap().1;
    // One ms past the near desk's full trip (duration + pause).
    let done_ms = near_profile.duration_ms + near_profile.pause_ms + 1;
    let t1 = now + Duration::from_millis(done_ms);

    let p_near = derive_with_routing(
        &near,
        t1,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_n,
            overlay: &overlay,
            history: &mut hist_near,
            motion: &mut motion_near,
        },
    );
    let p_far = derive_with_routing(
        &far,
        t1,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router_f,
            overlay: &overlay,
            history: &mut hist_far,
            motion: &mut motion_far,
        },
    );

    assert!(
        !matches!(p_near, Some(Pose::Walking { .. })),
        "near desk must have arrived (no longer Walking), got {p_near:?}"
    );
    assert!(
        matches!(p_far, Some(Pose::Walking { .. })),
        "far desk must still be Walking, got {p_far:?}"
    );
}

#[test]
fn five_same_created_at_agents_have_distinct_entry_durations() {
    // Speed_mult is per-agent-id → 5 distinct IDs must produce 5
    // distinct physics durations even for the same desk index, confirming
    // stagger.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();

    let ids: Vec<AgentId> = (0..5)
        .map(|i| AgentId::from_transcript_path(&format!("/stagger/{i}.jsonl")))
        .collect();

    let mut durations = Vec::new();
    for &id in &ids {
        let mut slot = entry_slot_near(now);
        slot.agent_id = id;
        let mut motion = HashMap::new();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();
        let _ = derive_with_routing(
            &slot,
            now,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut hist,
                motion: &mut motion,
            },
        );
        let dur = motion[&id]
            .entry
            .as_ref()
            .expect("entry profile set")
            .1
            .duration_ms;
        durations.push(dur);
    }

    let unique: std::collections::HashSet<u64> = durations.iter().copied().collect();
    assert!(
        unique.len() >= 4,
        "expected ≥4 distinct durations among 5 agents, got {unique:?}"
    );
}

#[test]
fn exit_profile_snapshotted_once_not_on_subsequent_calls() {
    // Second and third calls to derive_with_routing for an exiting agent
    // must NOT overwrite the profile's started_at — exit is commit-to-route.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion = HashMap::new();
    let mut hist = PoseHistory::new();
    let mut router = StubRouter::straight();

    // First call: snapshot.
    let _ = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut hist,
            motion: &mut motion,
        },
    );
    let started_at_1 = motion[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile set on first call")
        .started_at;

    // Second call 100 ms later: must not re-snapshot.
    let t1 = now + Duration::from_millis(100);
    let _ = derive_with_routing(
        &slot,
        t1,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut hist,
            motion: &mut motion,
        },
    );
    let started_at_2 = motion[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile still present")
        .started_at;

    assert_eq!(
        started_at_1, started_at_2,
        "exit started_at must not change on subsequent calls"
    );
}

#[test]
fn exit_far_completes_before_grace_window_no_vanish() {
    // Regression: a far/slow physics exit walk whose duration exceeds the
    // reducer's EXIT_GRACE_WINDOW (4500ms) must be time-compressed to REACH
    // the door before the slot is GC'd. Before the fix the sprite popped out
    // of existence mid-corridor (~85% along) when the grace window reaped it.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold.expect("door");
    let desk = l.home_desks[0];
    let from = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    // Synthetic long route (≥1600 octile) so the physics exit duration
    // exceeds the exit budget and the compression path is exercised.
    let mid1 = Point {
        x: from.x.saturating_add(80),
        y: from.y,
    };
    let mid2 = Point {
        x: mid1.x,
        y: mid1.y.saturating_add(80),
    };
    let mut router = StubRouter::corners(vec![from, mid1, mid2, door]);
    // Exit started 4300ms ago — just inside the 4500ms grace window.
    let slot = exiting_slot(
        now - Duration::from_millis(4300),
        now - Duration::from_secs(60),
    );
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut hist = PoseHistory::new();
    let mut motion = HashMap::new();
    match derive_with_routing(&slot, now, &l, &mut crate::pose::RouteCtx { router: &mut router, overlay: &overlay, history: &mut hist, motion: &mut motion }) {
            // Reached the door (Walking at the end of the path) or already
            // arrived (None, GC imminent). Either way: NOT stuck mid-corridor.
            Some(Pose::Walking { t_x1000, .. }) => assert!(
                t_x1000 >= 950,
                "far exit must reach the door by the grace window (no mid-corridor vanish), got t_x1000={t_x1000}"
            ),
            None => {}
            other => panic!("expected Walking near the door or None (arrived), got {other:?}"),
        }
    // Sanity: the snapshotted exit profile really exceeded the budget, so the
    // compression branch (not the pass-through) was the one under test.
    let dur = motion[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile snapshotted")
        .profile
        .duration_ms;
    assert!(
        dur > 4200,
        "test setup: exit duration {dur}ms should exceed the ~4200ms exit budget"
    );
}

#[test]
fn exit_uses_commute_speed_faster_than_wander() {
    // Exit profiles must use V_CRUISE_COMMUTE, not V_CRUISE_WANDER.
    // Proxy: compare v_cruise on the exit profile against the constant.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion = HashMap::new();
    let mut hist = PoseHistory::new();
    let mut router = StubRouter::straight();

    let _ = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut hist,
            motion: &mut motion,
        },
    );
    let profile = &motion[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile set")
        .profile;
    // v_cruise stored in WalkProfile is v_base * speed_mult — it must be
    // derived from V_CRUISE_COMMUTE (0.36), NOT V_CRUISE_WANDER (0.25).
    // The minimum possible commute v_cruise = 0.36 * 0.85 ≈ 0.306,
    // while the maximum wander v_cruise = 0.25 * 1.20 ≈ 0.300.
    // There's a gap: anything >= 0.301 is unambiguously commute.
    let min_commute = crate::physics::V_CRUISE_COMMUTE * crate::physics::SPEED_MULT_MIN;
    let max_wander = crate::physics::V_CRUISE_WANDER * crate::physics::SPEED_MULT_MAX;
    assert!(
        min_commute > max_wander,
        "test invariant: commute and wander speed ranges must not overlap"
    );
    assert!(
        profile.v_cruise >= min_commute * 0.99, // small f32 tolerance
        "exit v_cruise {:.4} must be in commute range (>= {min_commute:.4})",
        profile.v_cruise
    );
}

#[test]
fn exit_with_no_door_does_not_vanish() {
    // Regression: on a layout with no door_threshold (very narrow
    // terminal), an exiting agent must NOT return None on its first
    // frame (None is the GC signal — the agent would vanish instantly).
    // It should fall through to the state-driven pose and let the
    // reducer's grace window GC the slot instead.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut l = layout();
    l.door_threshold = None;
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let mut hist = PoseHistory::new();
    let mut router = StubRouter::straight();

    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut hist,
            motion: &mut motion,
        },
    );
    assert!(
        p.is_some(),
        "exiting agent on a no-door layout must not vanish (got None)"
    );
    // No exit profile should have been snapshotted — we never reached
    // the physics exit branch.
    assert!(
        motion
            .get(&slot.agent_id)
            .is_none_or(|ms| ms.exit.is_none()),
        "no exit profile should be snapshotted when there is no door"
    );
}

// ====================================================================
// Coordinate-continuity tests: drive a scenario frame-by-frame at the
// real 33 ms cadence with a REAL A* router, sample `character_anchor`
// (the on-screen sprite pixel) each frame, and assert no single-frame
// jump exceeds a cruise step. A teleport ("flash") is a 30–100 px jump;
// smooth walking is ≤ ~15 px/frame (cruise speed × 33 ms) plus ≤ ~5 px
// sprite-anchor offset at pose-type boundaries. The overlay is CHURNED
// every frame (an obstacle toggles on/off) to reproduce the exact
// condition that used to trigger mid-walk reroutes.
// ====================================================================

/// One frame's max per-axis (Chebyshev) anchor jump allowed. Cruise is
/// ≤ ~15 px/frame; boundaries add ≤ ~5 px. 25 leaves margin while a real
/// teleport on this layout (desk↔waypoint ≈ 30–70 px) blows past it.
const MAX_FRAME_STEP_PX: i32 = 20;

/// Step `slot` (state held constant) for `frames` frames at 33 ms, sampling
/// `character_anchor` each frame against a real `AStarRouter`. Returns
/// `(max_chebyshev_step, walking_frame_count)`. When `churn` is set, an
/// office-interior obstacle toggles every other frame to force A* cache
/// invalidation mid-walk.
fn max_anchor_step(
    slot: &AgentSlot,
    l: &Layout,
    start: SystemTime,
    frames: u64,
    churn: bool,
) -> (i32, usize) {
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let mut overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let ob = l
        .corridor
        .map(|c| Point {
            x: c.x + c.width / 2,
            y: c.y + 2,
        })
        .unwrap_or(Point { x: 40, y: 50 });

    let mut prev: Option<Point> = None;
    let mut max_step = 0i32;
    let mut walking = 0usize;
    for i in 0..frames {
        let now = start + Duration::from_millis(i * 33);
        if churn {
            overlay.clear();
            if i % 2 == 0 {
                overlay.add(ob.x.saturating_sub(5), ob.y.saturating_sub(5), 12, 12);
            }
        }
        if let Some(a) = character_anchor(
            slot,
            l,
            now,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            if let Some(p) = prev {
                let step = (a.x as i32 - p.x as i32)
                    .abs()
                    .max((a.y as i32 - p.y as i32).abs());
                max_step = max_step.max(step);
            }
            prev = Some(a);
            walking += 1;
        }
    }
    (max_step, walking)
}

#[test]
fn entry_walk_coordinates_are_continuous() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // Fresh entry: just created, Idle, walks door→desk.
    let slot = entry_slot(now);
    let (max_step, walking) = max_anchor_step(&slot, &l, now, 150, true);
    assert!(walking > 20, "entry walk should render many frames");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "entry walk teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn desk_approach_cell_is_never_inside_the_blocked_desk() {
    // The desk's ARRIVAL target must be a real walkable cell off an allowed
    // (N/E/W) side — never the chair (blocked, inside the footprint) and never
    // any other blocked cell. Aiming A* at the blocked chair directly is exactly
    // what made it fall back to a straight door→chair line THROUGH the desk body
    // (the "walk through the table" bug). A walkable result is, by construction,
    // outside every blocked cell.
    use crate::layout::desk_walk_anchor;
    let l = layout();
    let mut any_some = false;
    for &desk in &l.home_desks {
        let chair = desk_walk_anchor(desk);
        // The chair is inside the blocked footprint — that is WHY we can't aim
        // A* at it and must settle onto it instead.
        assert!(
            !l.is_walkable(chair.x, chair.y),
            "the desk chair {chair:?} must be blocked (inside the footprint)"
        );
        // None = degenerate layout (every N/E/W side walled off); the entry then
        // falls back to the direct chair target. Acceptable, so not asserted.
        if let Some(cell) = desk_approach_cell(desk, &l) {
            any_some = true;
            assert!(
                l.is_walkable(cell.x, cell.y),
                "approach cell {cell:?} for desk {desk:?} must be walkable \
                 (so it is neither inside the footprint nor the blocked chair)"
            );
            assert_ne!(cell, chair, "approach cell must differ from the chair");
        }
    }
    assert!(
        any_some,
        "at least one desk in an open layout must have a valid approach cell"
    );
}

#[test]
fn desk_entry_routes_around_the_desk_then_settles_onto_the_chair() {
    // The arrival-bug fix: an entering agent walks to a walkable approach cell
    // (off an allowed side) and only THEN settles onto the chair — it must NOT
    // straight-line door→chair through the desk body. With the chair appended as
    // the settle endpoint, the frozen leg polyline is [door, approach, chair].
    use crate::layout::desk_walk_anchor;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold.expect("door");

    // Pick the first desk that has a real (non-degenerate) approach cell.
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor(desk);
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let slot = entry_slot_far(now, desk_index);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let mut history = PoseHistory::new();
    let mut router = StubRouter::straight();

    let pose = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    )
    .expect("entering agent renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "a fresh entry must be Walking, got {pose:?}"
    );

    let snap = motion[&slot.agent_id]
        .walk_path
        .as_ref()
        .expect("the cornered entry+settle leg is frozen (len > 2)");
    assert_eq!(
        snap.path,
        vec![door, approach, chair],
        "the leg must go door→approach→chair, settling onto the chair"
    );
    // The regression guard, stated directly: the agent never targets the blocked
    // chair as its A* goal (that is what straight-lined through the desk).
    assert_ne!(
        snap.path,
        vec![door, chair],
        "entry must not straight-line door→chair through the desk body"
    );
}

#[test]
fn wander_legs_approach_the_desk_via_an_allowed_side_not_through_the_front() {
    // Regression for the bug the user spotted: the desk-arrival unification
    // fixed the ENTRY leg but the WANDER out/back legs still aimed A* at
    // `desk_walk_anchor` (the blocked chair). `find_path` snaps a blocked goal
    // to the NEAREST walkable coarse cell — which for the south-facing chair is
    // the SOUTH (corridor) side — so the agent walked up THROUGH the desk front
    // on every wander cycle. Every desk-touching leg must instead route via
    // `desk_leg_endpoint` (a reachable N/E/W approach cell) and SETTLE onto the
    // chair, exactly like entry. This pins both legs.
    use crate::layout::desk_walk_anchor;
    use crate::pathfind::AStarRouter;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();

    // A desk with a real (non-degenerate) approach cell.
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor(desk);
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    // A trip-taking agent seated at that desk, long past its entry walk.
    let trip_id = (0u64..3000)
        .map(|i| AgentId::from_transcript_path(&format!("/deskleg/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("a trip agent");
    let old = now - Duration::from_secs(120);
    let mut slot = entry_slot(old);
    slot.agent_id = trip_id;
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.last_event_at = old;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let (mut saw_out, mut saw_back) = (false, false);
    let mut seen_ends: Vec<(Point, Point)> = Vec::new();
    for i in 0..6000u64 {
        let t = now + Duration::from_millis(i * 33);
        let _ = derive_with_routing(
            &slot,
            t,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        );
        let Some(snap) = motion.get(&trip_id).and_then(|m| m.walk_path.as_ref()) else {
            continue;
        };
        if let (Some(&f), Some(&la)) = (snap.path.first(), snap.path.last()) {
            if !seen_ends.contains(&(f, la)) {
                seen_ends.push((f, la));
            }
        }
        // Walk-OUT begins by rising off the chair (prepended), so its 2nd point
        // is the approach cell — never a cell on the blocked south front.
        if snap.path.first() == Some(&chair) {
            saw_out = true;
            assert_eq!(
                snap.path.get(1),
                Some(&approach),
                "walk-out must leave the desk via the N/E/W approach cell, not \
                 straight through the south front; got {:?}",
                snap.path
            );
        }
        // Walk-BACK ends by gliding onto the chair (appended), so its
        // penultimate point is the approach cell.
        if snap.path.last() == Some(&chair) && snap.path.len() >= 2 {
            saw_back = true;
            assert_eq!(
                snap.path[snap.path.len() - 2],
                approach,
                "walk-back must arrive at the desk via the N/E/W approach cell, \
                 not the south front; got {:?}",
                snap.path
            );
        }
    }
    assert!(
        saw_out,
        "expected to observe a walk-out leg; chair={chair:?} approach={approach:?} \
         ends seen={seen_ends:?}"
    );
    assert!(
        saw_back,
        "expected to observe a walk-back leg; chair={chair:?} approach={approach:?} \
         ends seen={seen_ends:?}"
    );
}

#[test]
fn exit_walk_coordinates_are_continuous() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    // Exit GCs (returns None) once arrived; sample until then.
    let (max_step, walking) = max_anchor_step(&slot, &l, now, 200, true);
    assert!(walking > 20, "exit walk should render many frames");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "exit walk teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn exit_from_desk_rises_off_the_chair_via_the_approach_cell() {
    // The exit DEPARTURE is a desk leg too (the case the user caught: the door
    // is NE, not south). An agent leaving its seated chair must rise off it via
    // the N/E/W approach cell — NOT dip south first. Aiming A* from the blocked
    // chair snaps the goal to the nearest (south) cell, sending the agent the
    // wrong way around the desk before doubling back to the door. The frozen
    // exit polyline must therefore START [chair, approach, …], mirroring entry.
    use crate::layout::desk_walk_anchor;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor(desk);
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let mut slot = exiting_slot(now, now - Duration::from_secs(300));
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.agent_id = AgentId::from_transcript_path("/exitdesk/slot.jsonl");

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    // Empty history ⇒ the agent is exiting from the seated state (not mid-wander),
    // so the stored exit origin is the chair and the desk-departure path applies.
    let mut history = PoseHistory::new();
    let mut router = StubRouter::straight();

    let pose = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    )
    .expect("exiting agent renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "a fresh exit must be Walking, got {pose:?}"
    );

    let snap = motion[&slot.agent_id]
        .walk_path
        .as_ref()
        .expect("the cornered exit leg is frozen (chair → approach → door, len > 2)");
    assert_eq!(
        snap.path.first(),
        Some(&chair),
        "exit must START at the chair (Settle::Start glides off it); got {:?}",
        snap.path
    );
    assert_eq!(
        snap.path.get(1),
        Some(&approach),
        "exit must rise off the chair via the N/E/W approach cell, not dip south; \
         got {:?}",
        snap.path
    );
}

#[test]
fn wander_coffee_run_coordinates_continuous_under_churn() {
    // The user-reported case: an idle agent's wander trip (desk→waypoint→
    // desk) must never teleport, even as the occupancy overlay churns every
    // frame. Pre-freeze this flashed — a mid-walk reroute remapped the
    // frozen progress onto a new polyline. ~50 s covers several full cycles
    // (Seated→WalkingOut→AtWaypoint→WalkingBack), exercising every leg and
    // boundary.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/cont/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    // Long-idle, past entry, not in the thinking window (last_event_at ≤
    // created_at ⇒ was_active = false).
    let old = now - Duration::from_secs(120);
    let mut slot = entry_slot(old);
    slot.agent_id = trip_id;
    slot.last_event_at = old;

    let (max_step, walking) = max_anchor_step(&slot, &l, now, 1500, true);
    assert!(walking > 1000, "idle agent should render every frame");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "wander trip teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn wander_interrupted_by_active_does_not_teleport() {
    // A coffee run interrupted by real work: while the agent is mid-walk to
    // a waypoint its state flips Idle→Active. It must snap-back to the desk
    // as a continuous walk, never an instant teleport.
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/intr/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    let old = now - Duration::from_secs(120);
    let mut idle = entry_slot(old);
    idle.agent_id = trip_id;
    idle.last_event_at = old;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let seated = {
        // Reference: the desk seated anchor — used to detect "far from desk".
        use crate::pixel_painter::character_anchor as ca;
        let mut r2 = AStarRouter::new();
        let o2 = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut h2 = PoseHistory::new();
        let mut m2: HashMap<AgentId, MotionState> = HashMap::new();
        ca(
            &idle,
            &l,
            now,
            &mut crate::pose::RouteCtx {
                router: &mut r2,
                overlay: &o2,
                history: &mut h2,
                motion: &mut m2,
            },
        )
        .expect("anchor")
    };

    // Step the idle wander until the agent is clearly mid-walk (anchor far
    // from its desk), capturing the last position.
    let mut last_pos = seated;
    let mut flip_frame = None;
    for i in 0..1500u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_anchor(
            &idle,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            let d = (a.x as i32 - seated.x as i32)
                .abs()
                .max((a.y as i32 - seated.y as i32).abs());
            last_pos = a;
            if d > 30 {
                flip_frame = Some(i);
                break;
            }
        }
    }
    let flip_frame = flip_frame.expect("agent should walk away from its desk within 50 s");

    // Flip to Active at this frame; continue stepping ~1.5 s of snap-back.
    let active = AgentSlot {
        state: ActivityState::Active {
            tool_use_id: Some(Arc::from("t")),
            detail: Some(Arc::from("Edit")),
            kind: ToolKind::Edit,
        },
        state_started_at: now + Duration::from_millis(flip_frame * 33),
        ..idle.clone()
    };
    let mut prev = last_pos;
    let mut max_step = 0i32;
    for i in (flip_frame + 1)..(flip_frame + 46) {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_anchor(
            &active,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            let step = (a.x as i32 - prev.x as i32)
                .abs()
                .max((a.y as i32 - prev.y as i32).abs());
            max_step = max_step.max(step);
            prev = a;
        }
    }
    assert!(
            max_step <= MAX_FRAME_STEP_PX,
            "interrupted wander teleported back to desk: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
        );
}

#[test]
fn floor_offscreen_then_resume_does_not_replay() {
    // Cross-floor: a floor goes off-screen (only the current floor renders,
    // so its motion freezes), then the user switches back ~30 s later. On
    // resume the agent must resync (Seated at desk) and continue smoothly —
    // NOT replay every backlogged wander cycle one transition per frame
    // (the "fast-forward all the movement in a second" bug). Modeled by
    // rendering a warm-up window, SKIPPING a long gap (no calls = frozen
    // motion), then resuming and asserting per-frame continuity.
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/floor/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    let old = now - Duration::from_secs(120);
    let mut slot = entry_slot(old);
    slot.agent_id = trip_id;
    slot.last_event_at = old;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Warm-up: floor visible, agent wanders for ~2 s.
    for i in 0..60u64 {
        let t = now + Duration::from_millis(i * 33);
        let _ = character_anchor(
            &slot,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        );
    }

    // Floor off-screen for ~30 s (frames 60..1000 NOT rendered → frozen),
    // then resume. Assert the resumed stretch is continuous.
    let mut prev: Option<Point> = None;
    let mut max_step = 0i32;
    for i in 1000..1120u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_anchor(
            &slot,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            if let Some(p) = prev {
                let step = (a.x as i32 - p.x as i32)
                    .abs()
                    .max((a.y as i32 - p.y as i32).abs());
                max_step = max_step.max(step);
            }
            prev = Some(a);
        }
    }
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "floor resume replayed/teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn exit_while_wandering_does_not_teleport_to_desk() {
    // A session ending while the agent is out on a wander trip (e.g. at the
    // pantry) must not snap the sprite back to its desk before the exit
    // walk. The exit should begin from the agent's CURRENT position.
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // Real-sized floor (not the tiny 120×96 `layout()`): in the tiny room the
    // meeting sofas are boxed in to only their backrest, so with the no-back-
    // fallback rule a trip agent correctly skips them and never wanders far. A
    // real floor has reachable waypoints, so the agent genuinely walks out —
    // which is what this exit-while-wandering test needs.
    let l = Layout::compute(160, 120, Some(4)).expect("fits");
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/exitw/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    let old = now - Duration::from_secs(120);
    let mut idle = entry_slot(old);
    idle.agent_id = trip_id;
    idle.last_event_at = old;

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let seat = {
        let mut r2 = AStarRouter::new();
        let o2 = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut h2 = PoseHistory::new();
        let mut m2: HashMap<AgentId, MotionState> = HashMap::new();
        character_anchor(
            &idle,
            &l,
            now,
            &mut crate::pose::RouteCtx {
                router: &mut r2,
                overlay: &o2,
                history: &mut h2,
                motion: &mut m2,
            },
        )
        .expect("anchor")
    };

    // Step until the agent is clearly away from its desk.
    let mut last = seat;
    let mut away_frame = None;
    // 3000 frames (~100s) spans several wander cycles, so one near-desk cycle
    // pick can't starve the away-detection.
    for i in 0..3000u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_anchor(
            &idle,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            last = a;
            let d = (a.x as i32 - seat.x as i32)
                .abs()
                .max((a.y as i32 - seat.y as i32).abs());
            // 20, not 30: "clearly away" must hold for the NEAREST legitimate
            // trip destination — the lounge couch sits ~26px chebyshev from
            // desk 0 at this fixture, and which destination a cycle picks
            // re-rolls whenever the waypoint SET changes (the pantry-v2 kinds
            // re-rolled it onto the couch). The assertion under test is
            // exit-from-current-position, not trip length.
            if d > 20 {
                away_frame = Some(i);
                break;
            }
        }
    }
    let away_frame = away_frame.expect("agent should walk away from desk within 100 s");

    // Session ends now: set exiting_at at this frame.
    let exit_at = now + Duration::from_millis(away_frame * 33);
    let exiting = AgentSlot {
        exiting_at: Some(exit_at),
        ..idle.clone()
    };
    // First exit frame, ~1 frame later.
    let t_next = exit_at + Duration::from_millis(33);
    let first_exit = character_anchor(
        &exiting,
        &l,
        t_next,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    )
    .expect("exit pose");
    let jump = (first_exit.x as i32 - last.x as i32)
        .abs()
        .max((first_exit.y as i32 - last.y as i32).abs());
    assert!(
            jump <= MAX_FRAME_STEP_PX,
            "exit-while-wandering teleported {jump}px from the waypoint ({last:?}) to the exit start ({first_exit:?})"
        );

    // And the rest of the exit walk (waypoint → door) must also be smooth.
    let mut prev = first_exit;
    let mut max_step = 0i32;
    for i in 2..200u64 {
        let t = exit_at + Duration::from_millis(i * 33);
        match character_anchor(
            &exiting,
            &l,
            t,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            Some(a) => {
                let step = (a.x as i32 - prev.x as i32)
                    .abs()
                    .max((a.y as i32 - prev.y as i32).abs());
                max_step = max_step.max(step);
                prev = a;
            }
            None => break, // arrived at door & GC'd
        }
    }
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "exit-from-wander walk to door teleported: max frame jump {max_step}px"
    );
}

#[test]
fn wander_continuous_across_layouts_and_agents() {
    // "All routing scenarios": verify no teleport across a spread of office
    // GEOMETRIES (different decoration seeds + terminal sizes ⇒ different
    // desk grids, corridor shapes, and waypoint kinds/positions) and across
    // MULTIPLE desks per layout (so different home positions and wander
    // destinations, incl. couch/pantry/etc., are exercised). Overlay churns
    // every frame. A teleport on any geometry/agent fails the sweep.
    use crate::layout::TEST_DEFAULT_DESKS;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let geometries: [(u16, u16, u64); 5] = [
        (120, 96, 0),
        (120, 96, 7),
        (160, 100, 3),
        (96, 80, 11),
        (200, 120, 5),
    ];

    for (w, h, seed) in geometries {
        let Some(l) = Layout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), seed) else {
            continue;
        };
        if l.home_desks.is_empty() || l.waypoints.is_empty() {
            continue;
        }
        let n = l.home_desks.len().min(4);
        for k in 0..n {
            let id = AgentId::from_transcript_path(&format!("/geo/{w}x{h}-{seed}/{k}.jsonl"));
            let old = now - Duration::from_secs(120);
            let mut slot = entry_slot(old);
            slot.agent_id = id;
            slot.desk_index = GlobalDeskIndex(k);
            slot.last_event_at = old;
            // ~20 s ⇒ 2–3 full wander cycles per agent.
            let (max_step, _) = max_anchor_step(&slot, &l, now, 600, true);
            assert!(
                    max_step <= MAX_FRAME_STEP_PX,
                    "geometry {w}x{h} seed={seed} desk={k}: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
                );
        }
    }
}

/// Router that returns shape `a` until `flipped`, then shape `b` — lets a
/// test switch the A* result MID-LEG at a chosen (high-t) frame.
struct FlipRouter {
    flipped: bool,
    a: Vec<Point>,
    b: Vec<Point>,
}
impl Router for FlipRouter {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &pixtuoid_core::walkable::OccupancyOverlay,
        _from: Point,
        _to: Point,
    ) -> Vec<Point> {
        if self.flipped {
            self.b.clone()
        } else {
            self.a.clone()
        }
    }
    fn invalidate(&mut self) {}
}

#[test]
fn frozen_leg_anchor_continuous_across_router_shape_change() {
    // OUTPUT-level guard for the path-freeze (bug #1): drive an entry walk,
    // then mid-leg (high t) switch the router to a very differently-shaped
    // polyline. With the freeze, the leg keeps following its snapshotted
    // shape and the sampled sprite position stays continuous. Reverting the
    // freeze makes the walk adopt the new shape at high t → a large jump.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold.expect("door");
    let desk = l.home_desks[0];
    let desk_t = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    // Shape A is a long DOWN-then-across detour (so the entry walk lasts
    // many frames); shape B is short and very differently routed. Both share
    // endpoints (door → desk).
    let a = vec![
        door,
        Point {
            x: door.x,
            y: door.y + 40,
        },
        Point {
            x: desk_t.x,
            y: door.y + 40,
        },
        desk_t,
    ];
    let b = vec![
        door,
        Point {
            x: desk_t.x,
            y: door.y,
        },
        desk_t,
    ];

    // Flip the router at ~40% of the (frozen) entry duration — guaranteed
    // mid-walk, where A and B diverge maximally.
    let entry_id = entry_slot(now).agent_id;
    let dur = walk_profile(octile_path_len(&a).max(1), WalkIntent::Entry, entry_id).duration_ms;
    let flip_frame = ((dur * 2 / 5) / 33).max(2);

    let mut router = FlipRouter {
        flipped: false,
        a,
        b,
    };
    let overlay = OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let mut prev: Option<Point> = None;
    let mut max_step = 0i32;
    for i in 0..(flip_frame + 8) {
        if i == flip_frame {
            router.flipped = true; // switch shape mid-walk (high t)
        }
        let slot = entry_slot(now - Duration::from_millis(200));
        let t = now + Duration::from_millis(i * 33);
        if let Some(Pose::Walking {
            from, to, t_x1000, ..
        }) = derive_with_routing(
            &slot,
            t,
            &l,
            &mut crate::pose::RouteCtx {
                router: &mut router,
                overlay: &overlay,
                history: &mut history,
                motion: &mut motion,
            },
        ) {
            let pos = walking_position(from, to, t_x1000);
            if let Some(p) = prev {
                let step = (pos.x as i32 - p.x as i32)
                    .abs()
                    .max((pos.y as i32 - p.y as i32).abs());
                max_step = max_step.max(step);
            }
            prev = Some(pos);
        }
    }
    assert!(
            max_step <= 20,
            "frozen leg must keep the anchor continuous despite a mid-leg router shape change (max jump {max_step}px)"
        );
}

#[test]
fn multiple_agents_share_overlay_without_teleport() {
    // Realistic multi-agent continuity guard: 3 long-idle agents on ONE
    // shared router/overlay/history/motion, with the overlay rebuilt from
    // their actual AtWaypoint positions each frame (mirroring
    // render_to_rgb_buffer's churn). Guards the wander/Seated/bootstrap
    // fixes in a multi-agent setting. NOTE: the real AStarRouter is stable
    // enough that this scenario does not by itself reproduce the freeze
    // regression — `frozen_leg_anchor_continuous_across_router_shape_change`
    // is the freeze-specific guard (it fails when the freeze is reverted).
    use crate::pathfind::AStarRouter;
    use crate::pixel_painter::character_anchor;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let n = l.home_desks.len().min(3);
    let old = now - Duration::from_secs(120);
    let slots: Vec<AgentSlot> = (0..n)
        .map(|k| {
            let mut s = entry_slot(old);
            s.agent_id = AgentId::from_transcript_path(&format!("/multi/{k}.jsonl"));
            s.desk_index = GlobalDeskIndex(k);
            s.last_event_at = old;
            s
        })
        .collect();

    let mut router = AStarRouter::new();
    router.set_preferred_zone(l.corridor);
    let mut overlay = OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
    let mut prev: HashMap<AgentId, Point> = HashMap::new();
    let mut max_step = 0i32;

    for i in 0..700u64 {
        let t = now + Duration::from_millis(i * 33);
        // Rebuild the shared overlay from AtWaypoint agents (as the pixel
        // pass does) — this is the churn that re-routes other walkers.
        overlay.clear();
        for s in &slots {
            if let Some(Pose::AtWaypoint { wp, .. }) = derive(s, t, &l) {
                if let Some(w) = l.waypoints.get(wp) {
                    overlay.add(w.pos.x.saturating_sub(4), w.pos.y.saturating_sub(6), 8, 12);
                }
            }
        }
        for s in &slots {
            if let Some(a) = character_anchor(
                s,
                &l,
                t,
                &mut crate::pose::RouteCtx {
                    router: &mut router,
                    overlay: &overlay,
                    history: &mut history,
                    motion: &mut motion,
                },
            ) {
                if let Some(p) = prev.get(&s.agent_id) {
                    let step = (a.x as i32 - p.x as i32)
                        .abs()
                        .max((a.y as i32 - p.y as i32).abs());
                    max_step = max_step.max(step);
                }
                prev.insert(s.agent_id, a);
            }
        }
    }
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "agents sharing a churning overlay must not teleport (max frame jump {max_step}px)"
    );
}

// ====================================================================
// No-door exit-fallback arm: on a layout with NO door_threshold, an
// exiting agent whose state-driven pose is a Walking (it was mid-wander
// walk-out when the session ended) routes that walk via route_walking_pose
// with Settle::None instead of vanishing. Covers mod.rs:194-205.
// ====================================================================
#[test]
fn no_door_exiting_walking_pose_routes_via_settle_none() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut l = layout();
    l.door_threshold = None;

    // An agent whose cycle-0 is a trip, so idle_pose yields a Walking pose in
    // the walk-out band. takes_trip(id, 0) drives the directed walk.
    let trip_id = (0u64..2000)
        .map(|i| AgentId::from_transcript_path(&format!("/nodoor/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");

    // Place state_started_at so elapsed lands in the walk-out band:
    // [seated_dwell, seated_dwell + WANDER_WALK_EST_MS). Idle, was_active=false
    // (last_event_at == created_at) so derive_state_only routes through idle_pose,
    // not SeatedThinking.
    let seated = seated_dwell_ms(trip_id);
    let into_walk = WANDER_WALK_EST_MS / 2;
    let created = now - Duration::from_secs(300);
    let mut slot = entry_slot(created);
    slot.agent_id = trip_id;
    slot.last_event_at = created;
    slot.state_started_at = now - Duration::from_millis(seated + into_walk);
    slot.exiting_at = Some(now);

    // Sanity: derive_state_only must indeed produce a Walking pose here.
    assert!(
        matches!(
            derive_state_only(&slot, now, &l),
            Some(Pose::Walking { .. })
        ),
        "test setup: the exiting agent must be mid walk-out for the no-door arm"
    );

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let p = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );
    assert!(
        matches!(p, Some(Pose::Walking { .. })),
        "no-door exiting Walking pose must route to a Walking pose (not vanish), got {p:?}"
    );
    // No exit profile was snapshotted — we took the no-door fallback arm, not the
    // physics exit branch.
    assert!(
        motion
            .get(&slot.agent_id)
            .is_none_or(|ms| ms.exit.is_none()),
        "the no-door arm must not snapshot a physics exit profile"
    );
}

// ====================================================================
// route_walking_pose DIRECT unit tests (it's module-private; the sibling
// `mod tests` has `use super::*`). These hit branches the orchestration
// tests can't reach precisely.
// ====================================================================

fn unit_slot(now: SystemTime) -> AgentSlot {
    active_slot(now, now - Duration::from_secs(60))
}

#[test]
fn route_walking_pose_straight_leg_records_lerp_and_clears_walk_path() {
    // Settle::None straight 2-point leg at t_x1000=500: the returned Walking
    // keeps the original (from,to), walk_path is set to None (straight legs are
    // never frozen), and the lerped midpoint is recorded to history.
    // Covers mod.rs:796-797 + 805-812.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let from = Point { x: 10, y: 20 };
    let to = Point { x: 30, y: 20 };

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let p = route_walking_pose(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
        Pose::Walking {
            from,
            to,
            t_x1000: 500,
            frame: 0,
            carrying_coffee: false,
        },
        Settle::None,
    );

    match p {
        Some(Pose::Walking {
            from: f,
            to: t,
            t_x1000,
            ..
        }) => {
            assert_eq!((f, t), (from, to), "straight leg keeps original endpoints");
            assert_eq!(t_x1000, 500, "straight leg passes t_x1000 through");
        }
        other => panic!("expected straight Walking, got {other:?}"),
    }
    // A 2-point leg is never frozen.
    assert!(
        motion
            .get(&slot.agent_id)
            .is_some_and(|ms| ms.walk_path.is_none()),
        "straight 2-point walk must clear walk_path"
    );
    // History records the lerped midpoint of from→to at t=500.
    let recorded = history.recent(slot.agent_id, 1_000, now).expect("history");
    assert_eq!(
        recorded,
        walking_position(from, to, 500),
        "straight leg records the lerped position"
    );
}

#[test]
fn route_walking_pose_coincident_path_returns_input_pose() {
    // A coincident corners path [p,p,p] has total octile length 0; the
    // `total == 0` guard returns the input pose unchanged. Covers mod.rs:823.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let p = Point { x: 40, y: 40 };

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    // 3 coincident points → len > 2 (so it isn't the straight branch) but total
    // segment length is 0.
    let mut router = StubRouter::corners(vec![p, p, p]);
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let input = Pose::Walking {
        from: p,
        to: p,
        t_x1000: 500,
        frame: 2,
        carrying_coffee: false,
    };
    let out = route_walking_pose(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
        input,
        Settle::None,
    );
    assert_eq!(
        out,
        Some(input),
        "a zero-length (coincident) polyline returns the input pose unchanged"
    );
}

#[test]
fn route_walking_pose_records_at_waypoint_and_aimless_history() {
    // The non-Walking arm records the AtWaypoint waypoint pos (mod.rs:731) and
    // the AimlessAt dest (mod.rs:732) into history (mod.rs:736) so a subsequent
    // snap-back has a valid "previous position".
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    assert!(!l.waypoints.is_empty(), "layout must have waypoints");
    let slot = unit_slot(now);

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // AtWaypoint → records layout.waypoints[0].pos.
    let wp0 = l.waypoints[0];
    let out = route_walking_pose(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
        Pose::AtWaypoint {
            wp: 0,
            kind: wp0.kind,
        },
        Settle::None,
    );
    assert!(matches!(out, Some(Pose::AtWaypoint { wp: 0, .. })));
    assert_eq!(
        history.recent(slot.agent_id, 1_000, now),
        Some(wp0.pos),
        "AtWaypoint must record the waypoint pos to history"
    );

    // AimlessAt → records its dest.
    let dest = Point { x: 55, y: 60 };
    let later = now + Duration::from_millis(10);
    let out2 = route_walking_pose(
        &slot,
        later,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
        Pose::AimlessAt { dest },
        Settle::None,
    );
    assert!(matches!(out2, Some(Pose::AimlessAt { .. })));
    assert_eq!(
        history.recent(slot.agent_id, 1_000, later),
        Some(dest),
        "AimlessAt must record its dest to history"
    );
}

#[test]
fn snap_back_profile_length_measures_the_routed_polyline() {
    // The snap-back leg RENDERS through route_walking_pose's A* polyline, so
    // the WalkProfile armed for it must measure that same polyline (mirroring
    // the entry/exit arms). Arming the straight-line octile distance made a
    // detouring (cornered) snap-back traverse a longer path inside the
    // straight-line duration — visibly faster than V_CRUISE_SNAPBACK.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let (snap_target, chair_settle) = crate::pose::desk_leg_endpoint(desk, &l);
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    // A detouring polyline well longer than the straight line.
    let corner = Point {
        x: prev.x,
        y: prev.y + 40,
    };
    let detour = vec![prev, corner, snap_target];
    let mut router = StubRouter::corners(detour.clone());
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let _ = derive_with_routing(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
    );

    let leg = motion
        .get(&slot.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .expect("snap-back must be armed");
    let settle = settle_len(snap_target, chair_settle);
    let routed = octile_path_len(&detour) + settle;
    let straight = octile_path_len(&[prev, snap_target]) + settle;
    assert!(
        routed > straight,
        "test setup: the detour must be longer than the straight line"
    );
    assert_eq!(
        leg.profile.path_len_octile,
        routed.max(1),
        "armed profile must cover the routed polyline (+ chair settle), not the straight line"
    );
}

#[test]
fn route_walking_pose_t_overshoot_snaps_to_final_segment() {
    // The "past the last segment — snap to final" fall-through (mod.rs:849-857)
    // fires only when `traveled` exceeds the summed leg lengths, which in-tree
    // callers never do (they pass t_x1000<=1000). Driving the private fn with an
    // out-of-range t_x1000=2000 makes traveled = 2*total > total, so the segment
    // loop never returns and the final-segment arm runs: it returns the LAST
    // segment (second_to_last → last) at t_x1000=1000 and records path[last].
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let a = Point { x: 10, y: 10 };
    let b = Point { x: 30, y: 10 };
    let c = Point { x: 30, y: 30 };

    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut history = PoseHistory::new();
    let mut router = StubRouter::corners(vec![a, b, c]);
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let out = route_walking_pose(
        &slot,
        now,
        &l,
        &mut crate::pose::RouteCtx {
            router: &mut router,
            overlay: &overlay,
            history: &mut history,
            motion: &mut motion,
        },
        Pose::Walking {
            from: a,
            to: c,
            t_x1000: 2000,
            frame: 0,
            carrying_coffee: false,
        },
        Settle::None,
    );

    match out {
        Some(Pose::Walking {
            from, to, t_x1000, ..
        }) => {
            assert_eq!(from, b, "snap-to-final uses path[last-1] as `from`");
            assert_eq!(to, c, "snap-to-final uses path[last] as `to`");
            assert_eq!(
                t_x1000, 1000,
                "snap-to-final pins t_x1000 to the segment end"
            );
        }
        other => panic!("expected final-segment Walking, got {other:?}"),
    }
    assert_eq!(
        history.recent(slot.agent_id, 1_000, now),
        Some(c),
        "snap-to-final records the final polyline point to history"
    );
}

#[test]
fn settle_from_pair_both_none_is_settle_none() {
    // settle_from_pair's (None, None) => Settle::None arm (mod.rs:699) is
    // unreachable from the wander orchestration (which always supplies at least
    // one seat). The pure fn is private but visible via `use super::*`, so call
    // it directly and pin all four arms — a mutant collapsing any arm to the
    // wrong variant fails here.
    let p = Point { x: 1, y: 1 };
    let q = Point { x: 2, y: 2 };

    assert!(
        matches!(settle_from_pair(None, None), Settle::None),
        "(None, None) collapses to Settle::None"
    );
    assert!(
        matches!(settle_from_pair(Some(p), None), Settle::Start(s) if s == p),
        "(Some, None) collapses to Settle::Start(start)"
    );
    assert!(
        matches!(settle_from_pair(None, Some(q)), Settle::End(e) if e == q),
        "(None, Some) collapses to Settle::End(end)"
    );
    assert!(
        matches!(
            settle_from_pair(Some(p), Some(q)),
            Settle::Both { start, end } if start == p && end == q
        ),
        "(Some, Some) collapses to Settle::Both start,end"
    );
}
