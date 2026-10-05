use super::*;
use crate::physics::walk_profile;
use crate::walk::{octile_path_len, settle_len};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex, ToolKind};
use pixtuoid_core::walkable::WalkableMask;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Stub router: returns a pre-baked polyline instead of running A*.
#[derive(Debug)]
struct StubRouter {
    path: Vec<Point>,
}

impl StubRouter {
    /// Straight-line: `route` returns `[from, to]` regardless of input.
    fn straight() -> Self {
        Self { path: vec![] }
    }
    /// Hardcoded polyline, returned regardless of the requested endpoints.
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

fn layout() -> SceneLayout {
    SceneLayout::compute(120, 96, Some(4)).expect("fits")
}

/// Returns a stable polyline (`first`) for its first few calls then a DIFFERENT
/// one (`rest`) — an overlay-driven A* reroute mid-walk. Counts calls.
#[derive(Debug)]
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
        // Frame 1 makes two calls (entry-profile snapshot + route_walking_pose)
        // that must agree, so only call #3+ can be a later-frame reroute.
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold;
    let desk = l.home_desks[0];
    let desk_target = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    let mid_a = Point {
        x: door.x,
        y: u16::midpoint(door.y, desk_target.y),
    };
    let mid_b = Point {
        x: desk_target.x,
        y: door.y,
    };
    assert_ne!(mid_a, mid_b, "test setup: corners must differ");

    let mut rig = RouteRig::new(ChangingRouter {
        calls: 0,
        first: vec![door, mid_a, desk_target],
        rest: vec![door, mid_b, desk_target],
    });

    let slot1 = entry_slot(now - Duration::from_millis(200));
    let _ = derive_with_routing(&slot1, now, &l, &mut rig.rctx());
    let calls_after_frame1 = rig.router.calls;

    let slot2 = entry_slot(now - Duration::from_millis(200));
    let later = now + Duration::from_millis(100);
    let _ = derive_with_routing(&slot2, later, &l, &mut rig.rctx());

    assert_eq!(
        rig.router.calls,
        calls_after_frame1,
        "frozen leg must not re-route on a later frame (got {} extra calls)",
        rig.router.calls - calls_after_frame1
    );

    let frozen = rig
        .walks
        .get(&slot2.agent_id)
        .and_then(|walk| walk.walk_path.as_ref())
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
    // Far from the desk so the snap-back arms.
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));
    match derive_with_routing(&slot, now, &l, &mut rig.rctx()) {
        Some(Pose::Walking { from, .. }) => {
            assert_eq!(from, prev, "snap-back walk should start from recorded prev");
        }
        other => panic!("expected snap-back Walking pose, got {other:?}"),
    }
}

#[test]
fn seated_waypoint_snap_back_starts_from_the_seat_not_the_approach_cell() {
    // Unlike the hand-injected snap-back tests, this drives the real AtWaypoint
    // arm — the one seated departure the WalkingBack Settle machinery misses.
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
    let mut rig = RouteRig::new(StubRouter::straight());

    let idle = entry_slot(now - Duration::from_secs(60));
    let mut walk = WalkState::new(idle.agent_id);
    walk.wander.phase = crate::walk::WanderPhase::AtWaypoint(walk_profile(
        100,
        WalkIntent::WanderBack,
        idle.agent_id,
    ));
    walk.wander.phase_started_at = now;
    walk.wander.last_advanced_at = now; // pin the phase (advance_wander no-ops at now)
    walk.wander.target = crate::walk::WanderTarget {
        dest: approach,
        kind: crate::walk::WanderKind::Named {
            wp_idx: 0,
            kind: crate::layout::WaypointKind::Couch,
            seat: Some(seat),
        },
    };
    rig.walks.insert(idle.agent_id, walk);
    match derive_with_routing(&idle, now, &l, &mut rig.rctx()) {
        Some(Pose::AtWaypoint { .. }) => {}
        other => panic!("expected AtWaypoint pose, got {other:?}"),
    }

    let active = active_slot(now, now - Duration::from_secs(60));
    let then = now + Duration::from_millis(50);
    match derive_with_routing(&active, then, &l, &mut rig.rctx()) {
        Some(Pose::Walking { from, .. }) => assert_eq!(
            from, seat,
            "snap-back must start from the rendered seat {seat:?}, not the approach cell {approach:?}"
        ),
        other => panic!("expected snap-back Walking pose, got {other:?}"),
    }
}

#[test]
fn snap_back_origin_is_frozen_across_frames() {
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    // state_started_at == now0: the leg arms on frame 0 and stays armed (the
    // re-arm guard keys on state_started_at, which is constant here).
    let slot = active_slot(now0, now0 - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Far from the desk so the pre-fix integer-pixel drift surfaces inside the
    // 8-frame window; a snap near SNAP_BACK_MIN_DIST=8 could delay the first
    // drift past the window and false-pass on broken code.
    let prev0 = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev0, now0 - Duration::from_millis(50));

    // 8 × 33 ms stays well inside the 900 ms window; re-derive each frame so
    // route_walking_pose advances history like the real render loop does.
    let mut origins = Vec::new();
    for i in 0..8u64 {
        let t = now0 + Duration::from_millis(i * 33);
        match derive_with_routing(&slot, t, &l, &mut rig.rctx()) {
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
    let corner_a = Point {
        x: prev0.x,
        y: snap_target.y,
    };
    let corner_b = Point {
        x: snap_target.x,
        y: prev0.y,
    };
    assert_ne!(corner_a, corner_b, "test setup: corners must differ");

    let mut rig = RouteRig::new(ChangingRouter {
        calls: 0,
        first: vec![prev0, corner_a, snap_target],
        rest: vec![prev0, corner_b, snap_target],
    });

    // State flipped 100ms ago — inside the 900ms snap-back window on both frames.
    let slot = active_slot(
        now - Duration::from_millis(100),
        now - Duration::from_secs(60),
    );
    rig.history
        .record(slot.agent_id, prev0, now - Duration::from_millis(50));

    // The arm routes once and route_walking_pose once — both inside
    // ChangingRouter's `first` window, so they agree on the shape.
    let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    let calls_after_frame1 = rig.router.calls;
    assert!(
        calls_after_frame1 >= 1,
        "frame 1 must route once to snapshot the cornered leg"
    );

    let later = now + Duration::from_millis(100);
    let _ = derive_with_routing(&slot, later, &l, &mut rig.rctx());

    assert_eq!(
        rig.router.calls,
        calls_after_frame1,
        "frozen cornered snap-back must not re-route on a later frame (got {} extra calls)",
        rig.router.calls - calls_after_frame1
    );
    let frozen = rig
        .walks
        .get(&slot.agent_id)
        .and_then(|walk| walk.walk_path.as_ref())
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
    // A repeat derive at the SAME `now` must change neither the pose nor the
    // shared history and walks — hence the inner k-loop.
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now0, now0 - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Far enough from the CHAIR to arm (≥ SNAP_BACK_MIN_DIST); the chair-distance
    // gate then stops it re-arming once the walk reaches the chair, so it settles.
    let prev0 = Point {
        x: desk.x + 16,
        y: desk.y + 12,
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev0, now0 - Duration::from_millis(50));

    let mut arrived_frame: Option<u64> = None;
    for i in 0..60u64 {
        let t = now0 + Duration::from_millis(i * 33);
        let p0 = derive_with_routing(&slot, t, &l, &mut rig.rctx());
        let h0 = rig.history.recent(slot.agent_id, 300, t);
        if arrived_frame.is_none()
            && matches!(
                p0,
                Some(Pose::SeatedTyping | Pose::SeatedIdle | Pose::SeatedThinking)
            )
        {
            arrived_frame = Some(i);
        }
        for k in 1..4 {
            let pk = derive_with_routing(&slot, t, &l, &mut rig.rctx());
            let hk = rig.history.recent(slot.agent_id, 300, t);
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
    assert!(
        arrived_frame.is_some(),
        "snap-back should reach the desk (settle to seated) within the run"
    );
}

#[test]
fn wander_derive_is_idempotent_within_a_frame() {
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

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

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);

    for i in 0..200u64 {
        let t = now0 + Duration::from_millis(i * 33);
        let a0 = character_top_left(&slot, &l, t, &mut rig.rctx());
        for k in 1..4 {
            let ak = character_top_left(&slot, &l, t, &mut rig.rctx());
            assert_eq!(
                a0, ak,
                "frame {i} call {k}: wander anchor differs within one frame ({a0:?} vs {ak:?}) — K-call desync"
            );
        }
    }
}

#[test]
fn snap_back_long_distance_renders_past_window_by_physics() {
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now0, now0 - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Far prev → SnapBack physics duration well over the 900ms arm window.
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now0 - Duration::from_millis(50));

    let (mut walking_after_window, mut arrived) = (false, false);
    for i in 0..90u64 {
        let t = now0 + Duration::from_millis(i * 33);
        match derive_with_routing(&slot, t, &l, &mut rig.rctx()) {
            Some(Pose::Walking { .. }) if i * 33 > SNAP_BACK_MS => walking_after_window = true,
            Some(Pose::Walking { .. }) => {}
            Some(Pose::SeatedTyping | Pose::SeatedIdle | Pose::SeatedThinking)
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let mut slot = active_slot(now, now - Duration::from_secs(60));
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.agent_id = AgentId::from_transcript_path("/snapapproach/slot.jsonl");
    // Far from the chair (≥ SNAP_BACK_MIN_DIST) so the snap-back arms.
    let prev = Point {
        x: chair.x + 40,
        y: chair.y + 25,
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));

    let pose =
        derive_with_routing(&slot, now, &l, &mut rig.rctx()).expect("snap-back renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "snap-back must be Walking, got {pose:?}"
    );
    let snap = rig.walks[&slot.agent_id]
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
    // 3 px from the SEAT, not from the desk origin — the threshold is measured
    // against where the agent sits, and the seat carries a per-desk nudge.
    let seat = crate::layout::desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    let close = Point {
        x: seat.x,
        y: seat.y.saturating_sub(3),
    };
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, close, now - Duration::from_millis(50));
    let p = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(
        matches!(p, Some(Pose::SeatedTyping)),
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
    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));
    let p = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(
        matches!(p, Some(Pose::SeatedTyping)),
        "snap-back window should be expired at 1.5s, got {p:?}"
    );
}

#[test]
fn snap_back_skipped_without_recent_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let mut rig = RouteRig::new(StubRouter::straight());
    let p = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(
        matches!(p, Some(Pose::SeatedTyping)),
        "no prev history → raw pose, got {p:?}"
    );
}

#[test]
fn multi_segment_path_maps_t_to_segment_via_octile_distance() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = entry_slot(now - Duration::from_millis(400));
    let door = l.door_threshold;
    let desk = l.home_desks[0];
    let mid = Point {
        x: u16::midpoint(door.x, desk.x),
        y: u16::midpoint(door.y, desk.y),
    };
    let mut rig = RouteRig::new(StubRouter::corners(vec![door, mid, desk]));
    let p = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    match p {
        Some(Pose::Walking {
            from, to, t_x1000, ..
        }) => {
            assert_eq!(from, door, "first segment starts at door, got {from:?}");
            assert_eq!(to, mid, "first segment ends at mid, got {to:?}");
            assert!(
                (0..=500).contains(&t_x1000),
                "expected first-segment seg_t in [0,500], got t_x1000={t_x1000}"
            );
            assert!(rig.history.recent(slot.agent_id, 1_000, now).is_some());
        }
        other => panic!("expected Walking on segment 0, got {other:?}"),
    }
}

#[test]
fn at_waypoint_pose_records_position_to_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
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
    let mut rig = RouteRig::new(StubRouter::straight());
    let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    assert!(
        rig.history.recent(slot.agent_id, 1_000, now).is_none(),
        "SeatedIdle should not write history"
    );
}

#[test]
fn delegates_to_derive_for_oob_desk() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let mut slot = active_slot(now, now - Duration::from_secs(60));
    slot.desk_index = GlobalDeskIndex(999);
    let mut rig = RouteRig::new(StubRouter::straight());
    assert!(derive_with_routing(&slot, now, &l, &mut rig.rctx()).is_none());
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

#[test]
fn snap_back_progress_is_physics_eased_not_linear() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    // Manhattan 28 from the CHAIR (≥ SNAP_BACK_MIN_DIST=8) so the snap-back arms.
    let prev = Point {
        x: desk.x + 20,
        y: desk.y + 18,
    };

    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));

    let _pose0 = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    let walk = rig
        .walks
        .get(&slot.agent_id)
        .expect("WalkState created on frame 0");
    let profile = &walk
        .snap_back
        .as_ref()
        .expect("snap_back profile stored")
        .profile;
    let dur_ms = profile.duration_ms;
    assert!(
        dur_ms > 0,
        "profile duration must be > 0 for a non-trivial distance"
    );

    // Record history 50 ms before `quarter_now` so it stays inside the 300 ms
    // freshness gate.
    let slot_q = active_slot(now, now - Duration::from_secs(60));
    let quarter_now = now + Duration::from_millis(dur_ms / 4);
    rig.history = PoseHistory::new();
    rig.history.record(
        slot_q.agent_id,
        prev,
        quarter_now - Duration::from_millis(50),
    );
    let p = derive_with_routing(&slot_q, quarter_now, &l, &mut rig.rctx());

    match p {
        Some(Pose::Walking { t_x1000, .. }) => {
            assert!(
                t_x1000 < 250,
                "physics ease-in: expected t_x1000 < 250 at 25% of duration, got {t_x1000}"
            );
        }
        other => panic!("expected Walking pose at 25% of snap-back duration, got {other:?}"),
    }
}

#[test]
fn snap_back_profile_stored_in_walk_state() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };

    let mut rig = RouteRig::new(StubRouter::straight());
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));

    let _p1 = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    let dur1 = rig
        .walks
        .get(&slot.agent_id)
        .and_then(|walk| walk.snap_back.as_ref())
        .map(|leg| leg.profile.duration_ms)
        .expect("snap_back profile created on frame 1");

    // Fresh history but the SAME persistent walks map.
    let slot2 = active_slot(now, now - Duration::from_secs(60));
    let t2 = now + Duration::from_millis(100);
    rig.history
        .record(slot2.agent_id, prev, t2 - Duration::from_millis(50));
    let _p2 = derive_with_routing(&slot2, t2, &l, &mut rig.rctx());
    let dur2 = rig
        .walks
        .get(&slot2.agent_id)
        .and_then(|walk| walk.snap_back.as_ref())
        .map(|leg| leg.profile.duration_ms)
        .expect("snap_back profile still present on frame 2");

    assert_eq!(
        dur1, dur2,
        "snap-back profile must be snapshotted once and reused across frames"
    );
}

#[test]
fn snap_back_rearms_on_new_state_transition() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk = l.home_desks[0];
    let mut rig = RouteRig::new(StubRouter::straight());

    let t0 = now;
    let slot0 = active_slot(t0, now - Duration::from_secs(60));
    let prev0 = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    rig.history
        .record(slot0.agent_id, prev0, t0 - Duration::from_millis(50));
    let _ = derive_with_routing(&slot0, t0, &l, &mut rig.rctx());
    let stored0 = rig
        .walks
        .get(&slot0.agent_id)
        .and_then(|walk| walk.snap_back.as_ref())
        .map(|leg| leg.started_at)
        .expect("snap_back armed at T0");
    assert_eq!(stored0, t0, "first arm should key on T0 state_started_at");

    // A NEW transition inside the window. Same agent_id (active_slot uses a fixed
    // transcript path) so its walk state is reused; only state_started_at moved.
    let t1_state = t0 + Duration::from_millis(400);
    let slot1 = active_slot(t1_state, now - Duration::from_secs(60));
    let now1 = t1_state;
    let prev1 = Point {
        x: desk.x + 40,
        y: desk.y + 25,
    };
    rig.history
        .record(slot1.agent_id, prev1, now1 - Duration::from_millis(50));
    let _ = derive_with_routing(&slot1, now1, &l, &mut rig.rctx());
    let stored1 = rig
        .walks
        .get(&slot1.agent_id)
        .and_then(|walk| walk.snap_back.as_ref())
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

fn entry_slot_near(created_at: SystemTime) -> AgentSlot {
    let mut s = active_slot(created_at, created_at);
    s.state = pixtuoid_core::state::ActivityState::Idle;
    s.desk_index = GlobalDeskIndex(0);
    s
}

fn entry_slot_far(created_at: SystemTime, desk_index: usize) -> AgentSlot {
    let mut s = entry_slot_near(created_at);
    s.desk_index = GlobalDeskIndex(desk_index);
    // Give each far slot a distinct agent_id so speed_mult differs.
    s.agent_id = AgentId::from_transcript_path(&format!("/far/{desk_index}.jsonl"));
    s
}

fn exiting_slot(exiting_at: SystemTime, created_at: SystemTime) -> AgentSlot {
    let mut s = active_slot(exiting_at - Duration::from_secs(30), created_at);
    s.exiting_at = Some(exiting_at);
    s.agent_id = AgentId::from_transcript_path("/exit/slot.jsonl");
    s
}

/// `(near, far)` desk indices by octile distance from the door.
fn near_far_desk_indices(l: &SceneLayout) -> (usize, usize) {
    let door = l.door_threshold;
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let (near_idx, far_idx) = near_far_desk_indices(&l);

    let near = entry_slot_far(now, near_idx);
    let far = entry_slot_far(now, far_idx);

    // Separate walks maps — each agent's first call snapshots its own profile.
    let mut rig_near = RouteRig::new(StubRouter::straight());
    let mut rig_far = RouteRig::new(StubRouter::straight());

    let _pn = derive_with_routing(&near, now, &l, &mut rig_near.rctx());
    let _pf = derive_with_routing(&far, now, &l, &mut rig_far.rctx());

    let dur_near = rig_near.walks[&near.agent_id]
        .entry
        .as_ref()
        .expect("entry profile set for near desk")
        .profile
        .duration_ms;
    let dur_far = rig_far.walks[&far.agent_id]
        .entry
        .as_ref()
        .expect("entry profile set for far desk")
        .profile
        .duration_ms;

    assert!(
        dur_far >= dur_near,
        "far desk duration {dur_far}ms must be >= near desk {dur_near}ms"
    );
}

#[test]
fn nearer_desk_arrives_before_farther_desk() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let (near_idx, far_idx) = near_far_desk_indices(&l);

    let near = entry_slot_far(now, near_idx);
    let far = entry_slot_far(now, far_idx);

    let mut rig_near = RouteRig::new(StubRouter::straight());
    let mut rig_far = RouteRig::new(StubRouter::straight());

    let _ = derive_with_routing(&near, now, &l, &mut rig_near.rctx());
    let _ = derive_with_routing(&far, now, &l, &mut rig_far.rctx());

    // One ms past the near desk's full trip, still inside the far desk's window.
    let near_profile = rig_near.walks[&near.agent_id]
        .entry
        .as_ref()
        .unwrap()
        .profile;
    let done_ms = near_profile.duration_ms + near_profile.pause_ms + 1;
    let t1 = now + Duration::from_millis(done_ms);

    let p_near = derive_with_routing(&near, t1, &l, &mut rig_near.rctx());
    let p_far = derive_with_routing(&far, t1, &l, &mut rig_far.rctx());

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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let mut rig = RouteRig::new(StubRouter::straight());

    let ids: Vec<AgentId> = (0..5)
        .map(|i| AgentId::from_transcript_path(&format!("/stagger/{i}.jsonl")))
        .collect();

    let mut durations = Vec::new();
    for &id in &ids {
        let mut slot = entry_slot_near(now);
        slot.agent_id = id;
        let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());
        let dur = rig.walks[&id]
            .entry
            .as_ref()
            .expect("entry profile set")
            .profile
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    let mut rig = RouteRig::new(StubRouter::straight());

    let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    let started_at_1 = rig.walks[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile set on first call")
        .started_at;

    let t1 = now + Duration::from_millis(100);
    let _ = derive_with_routing(&slot, t1, &l, &mut rig.rctx());
    let started_at_2 = rig.walks[&slot.agent_id]
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold;
    let desk = l.home_desks[0];
    let from = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    // A long synthetic route, so the physics exit duration exceeds the exit
    // budget and the compression path is the one under test.
    let mid1 = Point {
        x: from.x.saturating_add(80),
        y: from.y,
    };
    let mid2 = Point {
        x: mid1.x,
        y: mid1.y.saturating_add(80),
    };
    let mut rig = RouteRig::new(StubRouter::corners(vec![from, mid1, mid2, door]));
    // Exit started 4300ms ago — just inside the 4500ms grace window.
    let slot = exiting_slot(
        now - Duration::from_millis(4300),
        now - Duration::from_secs(60),
    );
    match derive_with_routing(&slot, now, &l, &mut rig.rctx()) {
        // Walking at the end of the path, or already arrived (None, GC
        // imminent) — either way NOT stuck mid-corridor.
        Some(Pose::Walking { t_x1000, .. }) => assert!(
            t_x1000 >= 950,
            "far exit must reach the door by the grace window (no mid-corridor vanish), got t_x1000={t_x1000}"
        ),
        None => {}
        other => panic!("expected Walking near the door or None (arrived), got {other:?}"),
    }
    let dur = rig.walks[&slot.agent_id]
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = exiting_slot(now, now - Duration::from_secs(60));
    let mut rig = RouteRig::new(StubRouter::straight());

    let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());
    let profile = &rig.walks[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit profile set")
        .profile;
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

/// One frame's max per-axis (Chebyshev) top-left jump. Cruise is ≤ ~15 px/frame and
/// pose-type boundaries add ≤ ~5 px, while a real teleport on this layout
/// (desk↔waypoint ≈ 30–70 px) blows past it.
const MAX_FRAME_STEP_PX: i32 = 20;

/// Step `slot` for `frames` frames at 33 ms, sampling `character_top_left` against a
/// real `AStarRouter`. Returns `(max_chebyshev_step, walking_frame_count)`. `churn`
/// toggles an interior obstacle every other frame to force A* cache invalidation.
fn max_top_left_step(
    slot: &AgentSlot,
    l: &SceneLayout,
    start: SystemTime,
    frames: u64,
    churn: bool,
) -> (i32, usize) {
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);

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
            rig.overlay.clear();
            if i % 2 == 0 {
                rig.overlay
                    .add(ob.x.saturating_sub(5), ob.y.saturating_sub(5), 12, 12);
            }
        }
        if let Some(a) = character_top_left(slot, l, now, &mut rig.rctx()) {
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
    let slot = entry_slot(now);
    let (max_step, walking) = max_top_left_step(&slot, &l, now, 150, true);
    assert!(walking > 20, "entry walk should render many frames");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "entry walk teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

/// Whether a chair is BLOCKED follows from where the facing puts it relative to
/// the desk's stamped ground — it is not a property every chair has.
#[test]
fn desk_approach_cell_is_never_inside_the_blocked_desk() {
    use crate::layout::{Facing, Furniture, OBSTACLE_PAD_PX, furniture_def};
    let l = layout();
    // The ONE table `mask::stamp_ground` reads, so this can't drift when the sprite is resized.
    let def = furniture_def(Furniture::Desk);
    let fp = def.footprint.expect("the desk carries a static footprint");
    let (gx, gy) = (
        def.ground_x.offset(def.visual.w, fp.w),
        def.ground_y.offset(def.visual.h, fp.h),
    );
    let mut any_some = false;
    let mut facings: Vec<Facing> = Vec::new();
    for &desk in &l.home_desks {
        let facing = l.desk_facing_at(desk);
        if !facings.contains(&facing) {
            facings.push(facing);
        }
        let chair = desk_walk_anchor_facing(desk, facing);
        let (x0, y0) = (desk.x + gx, desk.y + gy);
        let in_pad = (x0.saturating_sub(OBSTACLE_PAD_PX)..x0 + fp.w + OBSTACLE_PAD_PX)
            .contains(&chair.x)
            && (y0.saturating_sub(OBSTACLE_PAD_PX)..y0 + fp.h + OBSTACLE_PAD_PX).contains(&chair.y);
        assert_eq!(
            l.is_walkable(chair.x, chair.y),
            !in_pad,
            "desk {desk:?} facing {facing:?} seats its occupant at {chair:?}, \
             {} the desk's routing pad — so the mask must report it \
             {walkable}",
            if in_pad { "inside" } else { "outside" },
            walkable = if in_pad { "blocked" } else { "walkable" },
        );
        // None = degenerate layout (every allowed side walled off); the entry then
        // falls back to the direct chair target. Acceptable, so not asserted.
        if let Some(cell) = desk_approach_cell(desk, &l) {
            any_some = true;
            assert!(
                l.is_walkable(cell.x, cell.y),
                "approach cell {cell:?} for desk {desk:?} must be walkable \
                 (so it is neither inside the desk's blocked band nor the chair)"
            );
            assert_ne!(cell, chair, "approach cell must differ from the chair");
        }
    }
    assert!(
        any_some,
        "at least one desk in an open layout must have a valid approach cell"
    );
    assert!(
        facings.len() >= 2,
        "a pod seats its two rows on opposite sides, so this layout must exercise \
         more than one facing — got {facings:?}. Under a single facing the pad \
         check above only ever sees one side of the desk."
    );
}

#[test]
fn desk_entry_routes_around_the_desk_then_settles_onto_the_chair() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold;

    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let slot = entry_slot_far(now, desk_index);
    let mut rig = RouteRig::new(StubRouter::straight());

    let pose = derive_with_routing(&slot, now, &l, &mut rig.rctx())
        .expect("entering agent renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "a fresh entry must be Walking, got {pose:?}"
    );

    let snap = rig.walks[&slot.agent_id]
        .walk_path
        .as_ref()
        .expect("the cornered entry+settle leg is frozen (len > 2)");
    assert_eq!(
        snap.path,
        vec![door, approach, chair],
        "the leg must go door→approach→chair, settling onto the chair"
    );
    assert_ne!(
        snap.path,
        vec![door, chair],
        "entry must not straight-line door→chair through the desk body"
    );
}

#[test]
fn wander_legs_approach_the_desk_via_an_allowed_side_not_through_the_front() {
    // `find_path` snaps a blocked goal to the NEAREST walkable coarse cell, which
    // for the south-facing chair is the SOUTH (corridor) side — so aiming a leg at
    // `desk_walk_anchor_facing` walks the agent up THROUGH the desk front.
    use crate::pathfind::AStarRouter;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();

    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let trip_id = (0u64..3000)
        .map(|i| AgentId::from_transcript_path(&format!("/deskleg/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("a trip agent");
    let old = now - Duration::from_secs(120);
    let mut slot = entry_slot(old);
    slot.agent_id = trip_id;
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.last_event_at = old;

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);

    let (mut saw_out, mut saw_back) = (false, false);
    let mut seen_ends: Vec<(Point, Point)> = Vec::new();
    for i in 0..6000u64 {
        let t = now + Duration::from_millis(i * 33);
        let _ = derive_with_routing(&slot, t, &l, &mut rig.rctx());
        let Some(snap) = rig
            .walks
            .get(&trip_id)
            .and_then(|walk| walk.walk_path.as_ref())
        else {
            continue;
        };
        if let (Some(&f), Some(&la)) = (snap.path.first(), snap.path.last())
            && !seen_ends.contains(&(f, la))
        {
            seen_ends.push((f, la));
        }
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
    let (max_step, walking) = max_top_left_step(&slot, &l, now, 200, true);
    assert!(walking > 20, "exit walk should render many frames");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "exit walk teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn exit_from_desk_rises_off_the_chair_via_the_approach_cell() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let desk_index = (0..l.home_desks.len())
        .find(|&i| desk_approach_cell(l.home_desks[i], &l).is_some())
        .expect("a desk with a valid approach cell");
    let desk = l.home_desks[desk_index];
    let chair = desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    let approach = desk_approach_cell(desk, &l).expect("approach cell");

    let mut slot = exiting_slot(now, now - Duration::from_secs(300));
    slot.desk_index = GlobalDeskIndex(desk_index);
    slot.agent_id = AgentId::from_transcript_path("/exitdesk/slot.jsonl");

    let mut rig = RouteRig::new(StubRouter::straight());
    // Empty history ⇒ the agent is exiting from the seated state (not mid-wander),
    // so the stored exit origin is the chair and the desk-departure path applies.

    let pose =
        derive_with_routing(&slot, now, &l, &mut rig.rctx()).expect("exiting agent renders a pose");
    assert!(
        matches!(pose, Pose::Walking { .. }),
        "a fresh exit must be Walking, got {pose:?}"
    );

    let snap = rig.walks[&slot.agent_id]
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

    // 1500 frames ≈ 50 s — several full Seated→WalkingOut→AtWaypoint→WalkingBack
    // cycles, so every leg and boundary is exercised.
    let (max_step, walking) = max_top_left_step(&slot, &l, now, 1500, true);
    assert!(walking > 1000, "idle agent should render every frame");
    assert!(
        max_step <= MAX_FRAME_STEP_PX,
        "wander trip teleported: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
    );
}

#[test]
fn wander_interrupted_by_active_does_not_teleport() {
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

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

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);
    // The desk's seated top-left, on throwaway stores — the "far from desk" reference.
    let seated = crate::sim::anchors::character_top_left(
        &idle,
        &l,
        now,
        &mut RouteRig::new(AStarRouter::new()).rctx(),
    )
    .expect("top-left");

    let mut last_pos = seated;
    let mut flip_frame = None;
    for i in 0..1500u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_top_left(&idle, &l, t, &mut rig.rctx()) {
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
        if let Some(a) = character_top_left(&active, &l, t, &mut rig.rctx()) {
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
    // An off-screen floor is simply not rendered, so its walks freeze: the
    // fixture warms up, SKIPS a long gap (no calls at all), then resumes.
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

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

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);

    for i in 0..60u64 {
        let t = now + Duration::from_millis(i * 33);
        let _ = character_top_left(&slot, &l, t, &mut rig.rctx());
    }

    // Frames 60..1000 are NOT rendered — the off-screen gap.
    let mut prev: Option<Point> = None;
    let mut max_step = 0i32;
    for i in 1000..1120u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_top_left(&slot, &l, t, &mut rig.rctx()) {
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
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // A real-sized floor, not the tiny 120×96 `layout()`: in the tiny room the
    // meeting sofas are boxed in to their backrest, so a trip agent skips them and
    // never wanders far — and this test needs the agent to genuinely walk out.
    let l = SceneLayout::compute(160, 120, Some(4)).expect("fits");
    let trip_id = (0u64..1000)
        .map(|i| AgentId::from_transcript_path(&format!("/exitw/{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find a trip agent");
    let old = now - Duration::from_secs(120);
    let mut idle = entry_slot(old);
    idle.agent_id = trip_id;
    idle.last_event_at = old;

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);

    let seat = character_top_left(
        &idle,
        &l,
        now,
        &mut RouteRig::new(AStarRouter::new()).rctx(),
    )
    .expect("top-left");

    let mut last = seat;
    let mut away_frame = None;
    // 3000 frames (~100s) spans several wander cycles, so one near-desk cycle
    // pick can't starve the away-detection.
    for i in 0..3000u64 {
        let t = now + Duration::from_millis(i * 33);
        if let Some(a) = character_top_left(&idle, &l, t, &mut rig.rctx()) {
            last = a;
            let d = (a.x as i32 - seat.x as i32)
                .abs()
                .max((a.y as i32 - seat.y as i32).abs());
            // 20, not 30: "clearly away" must hold for the NEAREST legitimate trip
            // destination, and which one a cycle picks re-rolls whenever the
            // waypoint SET changes. The assertion under test is
            // exit-from-current-position, not trip length.
            if d > 20 {
                away_frame = Some(i);
                break;
            }
        }
    }
    let away_frame = away_frame.expect("agent should walk away from desk within 100 s");

    let exit_at = now + Duration::from_millis(away_frame * 33);
    let exiting = AgentSlot {
        exiting_at: Some(exit_at),
        ..idle.clone()
    };
    let t_next = exit_at + Duration::from_millis(33);
    let first_exit = character_top_left(&exiting, &l, t_next, &mut rig.rctx()).expect("exit pose");
    let jump = (first_exit.x as i32 - last.x as i32)
        .abs()
        .max((first_exit.y as i32 - last.y as i32).abs());
    assert!(
        jump <= MAX_FRAME_STEP_PX,
        "exit-while-wandering teleported {jump}px from the waypoint ({last:?}) to the exit start ({first_exit:?})"
    );

    let mut prev = first_exit;
    let mut max_step = 0i32;
    for i in 2..200u64 {
        let t = exit_at + Duration::from_millis(i * 33);
        match character_top_left(&exiting, &l, t, &mut rig.rctx()) {
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
        let Some(l) = SceneLayout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), seed) else {
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
            let (max_step, _) = max_top_left_step(&slot, &l, now, 600, true);
            assert!(
                max_step <= MAX_FRAME_STEP_PX,
                "geometry {w}x{h} seed={seed} desk={k}: max frame jump {max_step}px (> {MAX_FRAME_STEP_PX})"
            );
        }
    }
}

/// Returns shape `a` until `flipped`, then `b` — switches the A* result mid-leg.
#[derive(Debug)]
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
    // The freeze-specific guard: it fails when the freeze is reverted.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let door = l.door_threshold;
    let desk = l.home_desks[0];
    let desk_t = Point {
        x: desk.x + 6,
        y: desk.y + 4,
    };
    // Shape A is a long DOWN-then-across detour so the walk lasts many frames;
    // shape B is short and very differently routed. Both share the endpoints.
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

    // ~40% of the frozen entry duration — mid-walk, where A and B diverge most.
    let entry_id = entry_slot(now).agent_id;
    let dur = walk_profile(octile_path_len(&a).max(1), WalkIntent::Entry, entry_id).duration_ms;
    let flip_frame = ((dur * 2 / 5) / 33).max(2);

    let mut rig = RouteRig::new(FlipRouter {
        flipped: false,
        a,
        b,
    });

    let mut prev: Option<Point> = None;
    let mut max_step = 0i32;
    for i in 0..(flip_frame + 8) {
        if i == flip_frame {
            rig.router.flipped = true;
        }
        let slot = entry_slot(now - Duration::from_millis(200));
        let t = now + Duration::from_millis(i * 33);
        if let Some(Pose::Walking {
            from, to, t_x1000, ..
        }) = derive_with_routing(&slot, t, &l, &mut rig.rctx())
        {
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
    // NOTE: the real AStarRouter is stable enough that this scenario does not by
    // itself reproduce the freeze regression —
    // `frozen_leg_anchor_continuous_across_router_shape_change` is that guard.
    use crate::pathfind::AStarRouter;
    use crate::sim::anchors::character_top_left;

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

    let mut rig = RouteRig::new(AStarRouter::new());
    rig.router.set_preferred_zone(l.corridor);
    let mut prev: HashMap<AgentId, Point> = HashMap::new();
    let mut max_step = 0i32;

    for i in 0..700u64 {
        let t = now + Duration::from_millis(i * 33);
        // Rebuild the shared overlay as the pixel pass does — this is the churn
        // that re-routes the other walkers.
        rig.overlay.clear();
        for s in &slots {
            if let Some(Pose::AtWaypoint { wp, .. }) = derive(s, t, &l)
                && let Some(w) = l.waypoints.get(wp)
            {
                rig.overlay
                    .add(w.pos.x.saturating_sub(4), w.pos.y.saturating_sub(6), 8, 12);
            }
        }
        for s in &slots {
            if let Some(a) = character_top_left(s, &l, t, &mut rig.rctx()) {
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

fn unit_slot(now: SystemTime) -> AgentSlot {
    active_slot(now, now - Duration::from_secs(60))
}

#[test]
fn route_walking_pose_straight_leg_records_lerp_and_clears_walk_path() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let from = Point { x: 10, y: 20 };
    let to = Point { x: 30, y: 20 };

    let mut rig = RouteRig::new(StubRouter::straight());

    let p = route_walking_pose(
        &slot,
        now,
        &l,
        &mut rig.rctx(),
        Pose::walking(from, to, 500, false),
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
    assert!(
        rig.walks
            .get(&slot.agent_id)
            .is_some_and(|walk| walk.walk_path.is_none()),
        "straight 2-point walk must clear walk_path"
    );
    let recorded = rig
        .history
        .recent(slot.agent_id, 1_000, now)
        .expect("history");
    assert_eq!(
        recorded,
        walking_position(from, to, 500),
        "straight leg records the lerped position"
    );
}

#[test]
fn route_walking_pose_coincident_path_returns_input_pose() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let p = Point { x: 40, y: 40 };

    let mut rig = RouteRig::new(StubRouter::corners(vec![p, p, p]));
    // 3 coincident points: len > 2 (so not the straight branch), total length 0.

    let input = Pose::Walking {
        from: p,
        to: p,
        t_x1000: 500,
        travelled: 2,
        carrying_coffee: false,
    };
    let out = route_walking_pose(&slot, now, &l, &mut rig.rctx(), input, Settle::None);
    assert_eq!(
        out,
        Some(input),
        "a zero-length (coincident) polyline returns the input pose unchanged"
    );
}

#[test]
fn route_walking_pose_records_at_waypoint_and_aimless_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    assert!(!l.waypoints.is_empty(), "layout must have waypoints");
    let slot = unit_slot(now);

    let mut rig = RouteRig::new(StubRouter::straight());

    let wp0 = l.waypoints[0];
    let out = route_walking_pose(
        &slot,
        now,
        &l,
        &mut rig.rctx(),
        Pose::AtWaypoint {
            wp: 0,
            kind: wp0.kind,
        },
        Settle::None,
    );
    assert!(matches!(out, Some(Pose::AtWaypoint { wp: 0, .. })));
    assert_eq!(
        rig.history.recent(slot.agent_id, 1_000, now),
        Some(wp0.pos),
        "AtWaypoint must record the waypoint pos to history"
    );

    let dest = Point { x: 55, y: 60 };
    let later = now + Duration::from_millis(10);
    let out2 = route_walking_pose(
        &slot,
        later,
        &l,
        &mut rig.rctx(),
        Pose::AimlessAt { dest },
        Settle::None,
    );
    assert!(matches!(out2, Some(Pose::AimlessAt { .. })));
    assert_eq!(
        rig.history.recent(slot.agent_id, 1_000, later),
        Some(dest),
        "AimlessAt must record its dest to history"
    );
}

#[test]
fn snap_back_profile_length_measures_the_routed_polyline() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let (snap_target, chair_settle) = crate::pose::desk_leg_endpoint(desk, &l);
    let prev = Point {
        x: desk.x + 50,
        y: desk.y + 30,
    };
    let corner = Point {
        x: prev.x,
        y: prev.y + 40,
    };
    let detour = vec![prev, corner, snap_target];
    let mut rig = RouteRig::new(StubRouter::corners(detour.clone()));
    rig.history
        .record(slot.agent_id, prev, now - Duration::from_millis(50));

    let _ = derive_with_routing(&slot, now, &l, &mut rig.rctx());

    let leg = rig
        .walks
        .get(&slot.agent_id)
        .and_then(|walk| walk.snap_back.as_ref())
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

/// A cornered walk's `travelled` is how far along the WHOLE polyline it is,
/// not along the segment it is on: the frame keeps stepping across a corner.
#[test]
fn route_walking_pose_measures_travelled_along_the_whole_route() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let a = Point { x: 10, y: 10 };
    let b = Point { x: 30, y: 10 };
    let c = Point { x: 30, y: 30 };
    let total = octile_path_len(&[a, b, c]);
    let mut rig = RouteRig::new(StubRouter::corners(vec![a, b, c]));
    for t in [0, 250, 500, 750, 1000] {
        let out = route_walking_pose(
            &slot,
            now,
            &l,
            &mut rig.rctx(),
            Pose::walking(a, c, t, false),
            Settle::None,
        );
        let Some(Pose::Walking { travelled, .. }) = out else {
            panic!("a walk at {t}, got {out:?}");
        };
        assert_eq!(travelled, pure::distance_at(t, total), "at {t}");
    }
}

#[test]
fn route_walking_pose_t_overshoot_snaps_to_final_segment() {
    // The "past the last segment" fall-through fires only when `travelled` exceeds
    // the summed leg lengths, which in-tree callers never do — hence the
    // out-of-range t_x1000=2000 below.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = unit_slot(now);
    let a = Point { x: 10, y: 10 };
    let b = Point { x: 30, y: 10 };
    let c = Point { x: 30, y: 30 };

    let mut rig = RouteRig::new(StubRouter::corners(vec![a, b, c]));

    let out = route_walking_pose(
        &slot,
        now,
        &l,
        &mut rig.rctx(),
        Pose::walking(a, c, 2000, false),
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
        rig.history.recent(slot.agent_id, 1_000, now),
        Some(c),
        "snap-to-final records the final polyline point to history"
    );
}

#[test]
fn a_resurrect_after_the_walkout_arrived_re_enters_through_the_door() {
    let l = layout();
    let door = l.door_threshold;
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // Born long before the spawn window, so only a re-arm can produce an entry.
    let created = t0 - Duration::from_secs(3600);
    let mut slot = entry_slot(created);
    slot.exiting_at = Some(t0);

    let mut rig = RouteRig::new(StubRouter::straight());

    // Let the leg arrive: the sprite is off-floor (`None`) and `history` has
    // nothing recent — the state the snap-back cannot recover from.
    derive_with_routing(&slot, t0, &l, &mut rig.rctx());
    assert!(
        rig.walks[&slot.agent_id].exit.is_some(),
        "test setup: the walkout must have snapshotted a leg"
    );
    let arrived = t0 + pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    assert!(
        derive_with_routing(&slot, arrived, &l, &mut rig.rctx()).is_none(),
        "test setup: the walkout must have reached the door"
    );

    // exiting_at cleared, state_started_at re-stamped, created_at untouched —
    // exactly what `fsm::resurrect_in_place` leaves behind.
    slot.exiting_at = None;
    slot.state_started_at = arrived;

    let p = derive_with_routing(&slot, arrived, &l, &mut rig.rctx());
    match p {
        Some(Pose::Walking { from, .. }) => assert_eq!(
            from, door,
            "a resurrect past its walkout must re-enter from the door"
        ),
        other => panic!("expected a fresh entry walk, got {other:?}"),
    }
    assert!(
        rig.walks[&slot.agent_id].exit.is_none(),
        "the spent exit leg must be cleared, or the NEXT exit replays an \
         already-arrived profile and the sprite vanishes instead of walking out"
    );
}

#[test]
fn a_resurrect_mid_walkout_re_enters_from_the_live_position() {
    let l = layout();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let created = t0 - Duration::from_secs(3600);
    let mut slot = entry_slot(created);
    slot.exiting_at = Some(t0);

    let mut rig = RouteRig::new(StubRouter::straight());

    derive_with_routing(&slot, t0, &l, &mut rig.rctx());
    let mid = t0 + Duration::from_millis(200);
    assert!(
        matches!(
            derive_with_routing(&slot, mid, &l, &mut rig.rctx()),
            Some(Pose::Walking { .. })
        ),
        "test setup: the walkout must still be in flight"
    );
    let live = rig
        .history
        .recent(slot.agent_id, HISTORY_RECENT_MS, mid)
        .expect("the walkout renders every frame, so history holds a position");

    slot.exiting_at = None;
    slot.state_started_at = mid;
    let pose = derive_with_routing(&slot, mid, &l, &mut rig.rctx());

    let leg = rig.walks[&slot.agent_id]
        .entry
        .as_ref()
        .expect("an in-flight resurrect must re-arm entry");
    assert_eq!(
        leg.from, live,
        "it re-enters from the sprite's real position, not the door"
    );
    assert_ne!(
        leg.from, l.door_threshold,
        "starting at the door would jump the walker backwards"
    );
    assert!(
        matches!(pose, Some(Pose::Walking { .. })),
        "and it RENDERS that walk — the entry branch runs before the wander \
         dispatch, which would otherwise return SeatedThinking and teleport it"
    );
}

/// The exit render and the resurrect check must read "has the walkout finished?"
/// through the SAME time-compressed clock. Every other resurrect test routes
/// straight-line on the small layout, where the compression branch is dead — so
/// only this fixture can catch a raw-elapsed read at the resurrect site.
#[test]
fn the_resurrect_check_reads_the_same_compressed_clock_as_the_exit_render() {
    let l = layout();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut slot = entry_slot(t0 - Duration::from_secs(3600));
    slot.exiting_at = Some(t0);

    // A long cornered route: physics duration lands well past the compression
    // budget, so raw and compressed elapsed genuinely disagree.
    let far: Vec<Point> = (0..24)
        .map(|i| Point {
            x: if i % 2 == 0 { 8 } else { 110 },
            y: 20 + i * 3,
        })
        .collect();
    let mut rig = RouteRig::new(StubRouter::corners(far));

    derive_with_routing(&slot, t0, &l, &mut rig.rctx());
    let profile = rig.walks[&slot.agent_id]
        .exit
        .as_ref()
        .expect("exit leg snapshotted")
        .profile;
    let budget = (pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64)
        .saturating_sub(EXIT_BUDGET_MARGIN_MS);
    assert!(
        profile.duration_ms + profile.pause_ms > budget,
        "fixture must exceed the compression budget, else this pins nothing \
         (duration {} + pause {} vs budget {budget})",
        profile.duration_ms,
        profile.pause_ms
    );

    // The first instant the COMPRESSED clock reports arrival. Compression scales
    // elapsed by duration/budget, so arrival lands past the budget edge — find it
    // rather than assume it.
    let arrived_at = (1..20_000u64)
        .find(|e| walk_arrived(&profile, exit_elapsed_ms(&profile, *e)))
        .expect("compressed clock must arrive within the search range");
    assert!(
        !walk_arrived(&profile, arrived_at),
        "raw elapsed must still read in-flight at {arrived_at}ms — otherwise the \
         two clocks agree and this fixture cannot tell them apart"
    );
    let at = t0 + Duration::from_millis(arrived_at);

    // Render one frame just BEFORE the compressed arrival so `PoseHistory` holds
    // a fresh corridor position. Without it both clocks fall back to the door
    // (the arrived exit branch returns None without recording) and the fixture
    // cannot tell them apart — the trap this test exists to avoid.
    let just_before = t0 + Duration::from_millis(arrived_at.saturating_sub(100));
    derive_with_routing(&slot, just_before, &l, &mut rig.rctx());
    assert!(
        rig.history
            .recent(slot.agent_id, HISTORY_RECENT_MS, at)
            .is_some(),
        "setup: history must be fresh at the resurrect instant"
    );
    slot.exiting_at = None;
    slot.state_started_at = at;
    derive_with_routing(&slot, at, &l, &mut rig.rctx());
    let leg = rig.walks[&slot.agent_id]
        .entry
        .as_ref()
        .expect("a resurrect must re-arm entry");
    assert_eq!(
        leg.started_at, at,
        "the re-armed leg runs on a fresh clock, not the un-restamped birth"
    );
    assert_eq!(
        leg.from, l.door_threshold,
        "past the COMPRESSED arrival the sprite is gone — the door is the only \
         honest origin, and raw elapsed would resume it mid-corridor"
    );
}
