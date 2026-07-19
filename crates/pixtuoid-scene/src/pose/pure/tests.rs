use super::*;
use pixtuoid_core::state::{GlobalDeskIndex, ToolKind};
use std::path::PathBuf;
use std::time::Duration;

fn slot(state: ActivityState, age_ms: u64) -> (AgentSlot, SystemTime) {
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let now = started + Duration::from_millis(age_ms);
    // created_at well before `started` so the entry-animation override
    // doesn't fire in tests that probe regular state→pose mapping.
    let created = started - Duration::from_secs(60);
    let s = AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("abc"),
        cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
        label: "cc".into(),
        state,
        state_started_at: started,
        created_at: created,
        last_event_at: created,
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
    (s, now)
}

fn layout() -> SceneLayout {
    SceneLayout::compute(120, 96, Some(4)).expect("fits")
}

fn typing() -> ActivityState {
    ActivityState::Active {
        tool_use_id: Some("t".into()),
        detail: Some("Edit".into()),
        kind: ToolKind::Edit,
    }
}

/// Phase boundary helper mirroring idle_pose's absolute estimate timeline.
fn phases(agent_id: AgentId) -> (u64, u64, u64, u64) {
    let seated_end = seated_dwell_ms(agent_id);
    let walk_out_end = seated_end + WANDER_WALK_EST_MS;
    let at_wp_end = walk_out_end + WANDER_DWELL_EST_MS;
    (
        seated_end,
        walk_out_end,
        at_wp_end,
        est_wander_cycle_ms(agent_id),
    )
}

/// Find the lowest cycle index where the agent decides to take a trip.
/// Pose tests that probe walking/waypoint phases need a known trip cycle
/// to drive the elapsed offset off of.
fn first_trip_cycle(agent_id: AgentId) -> u64 {
    (0u64..1000)
        .find(|n| takes_trip(agent_id, *n))
        .expect("agent should trip within first 1000 cycles")
}

#[test]
fn active_state_is_seated_typing_with_cycling_frame() {
    let (s, now) = slot(typing(), 0);
    let l = layout();
    assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 0 }));
    let (s, now) = slot(typing(), TYPING_FRAME_MS);
    assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 1 }));
    let (s, now) = slot(typing(), TYPING_FRAME_MS * 2);
    assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 0 }));
}

#[test]
fn waiting_state_is_standing_at_desk() {
    let (s, now) = slot(
        ActivityState::Waiting {
            reason: "perm".into(),
        },
        5_000,
    );
    let l = layout();
    assert_eq!(derive(&s, now, &l), Some(Pose::StandingAtDesk));
}

#[test]
fn idle_phase_0_is_seated_idle() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let (seated_end, _, _, _) = phases(test_slot.agent_id);
    let (s, now) = slot(ActivityState::Idle, seated_end - 1);
    let l = layout();
    assert_eq!(derive(&s, now, &l), Some(Pose::SeatedIdle));
}

#[test]
fn idle_phase_1_is_walking_out() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let (seated_end, walk_out_end, _, _) = phases(test_slot.agent_id);
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let trip_n = first_trip_cycle(test_slot.agent_id);
    let midpoint = trip_n * cycle + seated_end + (walk_out_end - seated_end) / 2;
    let (s, now) = slot(ActivityState::Idle, midpoint);
    let l = layout();
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { t_x1000, frame, .. } => {
            assert!((400..=600).contains(&t_x1000), "t_x1000={t_x1000}");
            assert!(frame < WALKING_FRAMES);
        }
        other => panic!("expected Walking, got {other:?}"),
    }
}

#[test]
fn idle_phase_2_is_at_waypoint() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let (_, walk_out_end, at_wp_end, _) = phases(test_slot.agent_id);
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let trip_n = first_trip_cycle(test_slot.agent_id);
    let midpoint = trip_n * cycle + walk_out_end + (at_wp_end - walk_out_end) / 2;
    let (s, now) = slot(ActivityState::Idle, midpoint);
    let l = layout();
    // Trip cycles land at either a named waypoint or an aimless point,
    // depending on per-agent personality.
    match derive(&s, now, &l).expect("pose") {
        Pose::AtWaypoint { wp, .. } => assert!(wp < l.waypoints.len()),
        Pose::AimlessAt { .. } => {}
        other => panic!("expected AtWaypoint or AimlessAt, got {other:?}"),
    }
}

#[test]
fn idle_phase_3_is_walking_back() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let (_, _, at_wp_end, cycle) = phases(test_slot.agent_id);
    let trip_n = first_trip_cycle(test_slot.agent_id);
    let midpoint = trip_n * cycle + at_wp_end + (cycle - at_wp_end) / 2;
    let (s, now) = slot(ActivityState::Idle, midpoint);
    let l = layout();
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { t_x1000, .. } => {
            assert!((400..=600).contains(&t_x1000));
        }
        other => panic!("expected Walking, got {other:?}"),
    }
}

/// Regression: an agent heading to the pantry must walk to a WALKABLE
/// stand cell, not the blocked counter center (which forced A* to detour
/// around/below the counter and the sprite to pop on arrival).
#[test]
fn pantry_walk_destination_is_walkable() {
    let l = layout();
    let pantry_idx = l
        .waypoints
        .iter()
        .position(|w| w.kind == WaypointKind::Pantry)
        .expect("standard floor has a pantry");
    // Find an agent whose first non-aimless trip cycle lands on the pantry.
    let (id, n) = (0..8000u64)
        .find_map(|i| {
            let id = AgentId::from_transcript_path(&format!("/p/pw{i}.jsonl"));
            let n = (0..300u64).find(|n| {
                takes_trip(id, *n)
                    && !is_aimless_cycle(id, *n)
                    && waypoint_index_for_cycle(id, *n, l.waypoints.len()) == pantry_idx
            })?;
            Some((id, n))
        })
        .expect("some agent lands at the pantry");

    let (seated_end, walk_out_end, _, cycle) = phases(id);
    let midpoint = n * cycle + seated_end + (walk_out_end - seated_end) / 2;
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let now = started + Duration::from_millis(midpoint);
    let mut s = slot(ActivityState::Idle, 0).0;
    s.agent_id = id; // desk_index 0 is valid for the 4-agent layout

    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { to, .. } => assert!(
            l.walkable.is_walkable(to.x, to.y),
            "pantry walk dest {to:?} is not walkable (center={:?})",
            l.waypoints[pantry_idx].pos
        ),
        other => panic!("expected Walking at walk-out midpoint, got {other:?}"),
    }
}

fn aid(i: usize) -> AgentId {
    AgentId::from_transcript_path(&format!("/p/dwell{i}.jsonl"))
}

#[test]
fn dwell_ms_is_within_per_kind_range() {
    let cases = [
        (WaypointKind::Couch, 20_000u64, 40_000u64),
        (WaypointKind::MeetingSofa, 20_000, 40_000),
        (WaypointKind::MeetingChair, 20_000, 40_000),
        (WaypointKind::Pantry, 10_000, 18_000),
        (WaypointKind::PhoneBooth, 8_000, 30_000),
        (WaypointKind::StandingDesk, 8_000, 30_000),
        (WaypointKind::VendingMachine, 4_000, 8_000),
        (WaypointKind::Printer, 4_000, 8_000),
    ];
    for (kind, lo, hi) in cases {
        for i in 0..256 {
            let d = dwell_ms(kind, aid(i));
            assert!(
                (lo..hi).contains(&d),
                "{kind:?} dwell {d} out of [{lo},{hi}) for agent {i}"
            );
        }
    }
}

#[test]
fn dwell_ms_varies_across_agents_and_is_deterministic() {
    let vals: std::collections::HashSet<u64> = (0..64)
        .map(|i| dwell_ms(WaypointKind::Couch, aid(i)))
        .collect();
    assert!(vals.len() >= 16, "expected dwell jitter across agents");
    // Deterministic per agent.
    assert_eq!(
        dwell_ms(WaypointKind::Couch, aid(7)),
        dwell_ms(WaypointKind::Couch, aid(7))
    );
}

#[test]
fn seated_dwell_and_est_cycle_are_consistent() {
    for i in 0..128 {
        let id = aid(i);
        let sd = seated_dwell_ms(id);
        assert!((15_000..30_000).contains(&sd), "seated dwell {sd}");
        assert_eq!(
            est_wander_cycle_ms(id),
            sd + 2 * WANDER_WALK_EST_MS + WANDER_DWELL_EST_MS
        );
    }
}

#[test]
fn idle_pose_holds_at_waypoint_for_the_whole_dwell_window() {
    // Across the full at-waypoint beat the agent stays put (AtWaypoint or
    // AimlessAt), never walking — the "rests too briefly" fix.
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let (_, walk_out_end, at_wp_end, cycle) = phases(test_slot.agent_id);
    let trip_n = first_trip_cycle(test_slot.agent_id);
    let l = layout();
    let window = at_wp_end - walk_out_end;
    assert!(
        window >= WANDER_DWELL_EST_MS,
        "dwell window too short: {window}"
    );
    for k in 0..=10 {
        let t = trip_n * cycle + walk_out_end + (k * window / 10).min(window - 1);
        let (s, now) = slot(ActivityState::Idle, t);
        match derive(&s, now, &l).expect("pose") {
            Pose::AtWaypoint { .. } | Pose::AimlessAt { .. } => {}
            other => panic!("at t={t} (k={k}) expected resting pose, got {other:?}"),
        }
    }
}

#[test]
fn takes_trip_fires_roughly_42_percent_of_cycles() {
    let id = AgentId::from_transcript_path("/p/sample.jsonl");
    let trips = (0u64..1000).filter(|n| takes_trip(id, *n)).count();
    // Per-agent trip chance varies 25..=60%, so across 1000 cycles the
    // count is bounded by those extremes with reasonable tolerance.
    assert!(
        (200..=650).contains(&trips),
        "expected 200..=650 trips out of 1000 (personality-driven), got {trips}"
    );
}

#[test]
fn personality_varies_across_agents() {
    let ps: Vec<Personality> = (0..20)
        .map(|i| personality_for(AgentId::from_transcript_path(&format!("/p/{i}.jsonl"))))
        .collect();
    let trip_chances: std::collections::HashSet<u8> =
        ps.iter().map(|p| p.trip_chance_pct).collect();
    assert!(
        trip_chances.len() >= 5,
        "expected variance in trip_chance_pct"
    );
    for p in &ps {
        assert!((25..=60).contains(&p.trip_chance_pct));
        assert!(p.aimless_pref_pct <= 70);
    }
}

#[test]
fn non_trip_cycle_is_seated_idle_throughout() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let id = test_slot.agent_id;
    let cycle = est_wander_cycle_ms(id);
    // Find a cycle where the agent does NOT trip.
    let stay_n = (0u64..100)
        .find(|n| !takes_trip(id, *n))
        .expect("agent should have a non-trip cycle");
    // Sample 10 points across that cycle; all should be SeatedIdle.
    for k in 0..10 {
        let t = stay_n * cycle + (k * cycle / 10);
        let (s, now) = slot(ActivityState::Idle, t);
        let l = layout();
        assert_eq!(
            derive(&s, now, &l),
            Some(Pose::SeatedIdle),
            "t={t} should be SeatedIdle on non-trip cycle"
        );
    }
}

#[test]
fn idle_cycle_loops_after_one_cycle() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let (s_early, now_early) = slot(ActivityState::Idle, 1_000);
    let (s_loop, now_loop) = slot(ActivityState::Idle, 1_000 + cycle);
    let l = layout();
    // Same phase within cycle, BUT waypoint differs because cycle_n changed.
    // Only the destination changes — the phase itself is the same kind.
    let e = derive(&s_early, now_early, &l).expect("e");
    let lp = derive(&s_loop, now_loop, &l).expect("loop");
    assert!(
        matches!((e, lp), (Pose::SeatedIdle, Pose::SeatedIdle)),
        "1s into any cycle should be SeatedIdle. got early={e:?} loop={lp:?}"
    );
}

#[test]
fn entry_animation_overrides_normal_pose_for_first_4s() {
    let id = AgentId::from_transcript_path("/p/entry.jsonl");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    // created_at == now0, so since_spawn = 1500ms at probe time
    let s = AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("abc"),
        cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
        label: "cc".into(),
        state: ActivityState::Idle,
        state_started_at: now0,
        created_at: now0,
        last_event_at: now0,
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
    let probe = now0 + Duration::from_millis(1500);
    let l = layout();
    match derive(&s, probe, &l).expect("pose") {
        Pose::Walking { t_x1000, .. } => {
            // 1500/4000 = 0.375 → t_x1000 ~= 375
            assert!((300..=450).contains(&t_x1000), "t_x1000={t_x1000}");
        }
        other => panic!("expected Walking entry, got {other:?}"),
    }
}

#[test]
fn derive_returns_none_when_desk_index_out_of_range() {
    let (mut s, now) = slot(ActivityState::Idle, 0);
    s.desk_index = GlobalDeskIndex(999);
    assert!(derive(&s, now, &layout()).is_none());
}

#[test]
fn exit_override_walks_desk_to_door_within_window() {
    // exiting_at set < ENTRY_ANIMATION_MS ago → Walking from the desk anchor
    // to the door threshold (the inverse of the entry walk).
    let (mut s, now) = slot(ActivityState::Idle, 0);
    s.exiting_at = Some(now - Duration::from_secs(1));
    let l = layout();
    let desk = l.home_desks[s.desk_index.0];
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { from, to, .. } => {
            assert_eq!(
                from,
                desk_walk_anchor(desk),
                "exit walk starts at desk anchor"
            );
            assert_eq!(
                Some(to),
                l.door_threshold,
                "exit walk targets the door threshold"
            );
        }
        other => panic!("expected exit Walking, got {other:?}"),
    }
}

#[test]
fn exit_override_returns_none_past_window() {
    // exiting_at older than ENTRY_ANIMATION_MS → nothing to render (slot GC'd).
    let (mut s, now) = slot(ActivityState::Idle, 0);
    s.exiting_at = Some(now - Duration::from_millis(ENTRY_ANIMATION_MS + 1000));
    assert!(derive(&s, now, &layout()).is_none());
}

#[test]
fn waypoint_index_is_zero_when_no_waypoints() {
    let id = AgentId::from_transcript_path("/p/wp.jsonl");
    assert_eq!(waypoint_index_for_cycle(id, 3, 0), 0);
}

#[test]
fn entry_window_fall_through_uses_state_driven_pose() {
    // door_threshold is Some but since_spawn >= ENTRY_ANIMATION_MS, so the
    // entry override's inner `if` is false and derive falls through to
    // state_driven_pose. A Waiting slot must yield StandingAtDesk (not Walking).
    let (mut s, now) = slot(
        ActivityState::Waiting {
            reason: "perm".into(),
        },
        ENTRY_ANIMATION_MS + 5_000,
    );
    // created_at is 60s before `started`, and probe is well past the entry
    // window, so the entry override does not fire.
    let l = layout();
    assert!(
        l.door_threshold.is_some(),
        "layout must populate door_threshold"
    );
    // Push created_at far enough back to be unambiguously past the window.
    s.created_at = now - Duration::from_millis(ENTRY_ANIMATION_MS + 10_000);
    assert_eq!(derive(&s, now, &l), Some(Pose::StandingAtDesk));
}

#[test]
fn stale_resume_gap_ms_varies_across_agents() {
    // Sanity: a handful of different agent ids should not all map to the
    // same cycle length.
    let ids: Vec<AgentId> = (0..10)
        .map(|i| AgentId::from_transcript_path(&format!("/p/{i}.jsonl")))
        .collect();
    let cycles: std::collections::HashSet<u64> =
        ids.iter().map(|id| stale_resume_gap_ms(*id)).collect();
    assert!(
        cycles.len() >= 3,
        "expected multiple distinct cycle lengths, got {cycles:?}"
    );
    for c in &cycles {
        assert!(
            *c >= STALE_RESUME_GAP_BASE_MS
                && *c < STALE_RESUME_GAP_BASE_MS + STALE_RESUME_GAP_RANGE_MS
        );
    }
}

#[test]
fn waypoint_choice_changes_across_cycles_for_same_agent() {
    let l = layout();
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let (_, walk_out_end, at_wp_end, _) = phases(test_slot.agent_id);
    let mid_at_wp = walk_out_end + (at_wp_end - walk_out_end) / 2;

    // Capture destinations chosen across many cycles. Trip cycles produce
    // either AtWaypoint or AimlessAt — both contribute to destination
    // variety, so we count distinct destination x coords.
    let mut dest_xs = std::collections::HashSet::new();
    for n in 0..50u64 {
        let t = n * cycle + mid_at_wp;
        let (s, now) = slot(ActivityState::Idle, t);
        match derive(&s, now, &l) {
            Some(Pose::AtWaypoint { wp, .. }) => {
                dest_xs.insert(l.waypoints[wp].pos.x);
            }
            Some(Pose::AimlessAt { dest }) => {
                dest_xs.insert(dest.x);
            }
            _ => {}
        }
    }
    assert!(
        dest_xs.len() >= 2,
        "destination should vary across cycles, got {dest_xs:?}"
    );
}

#[test]
fn idle_within_thinking_window_returns_seated_thinking() {
    let (mut s, now) = slot(ActivityState::Idle, 5_000);
    s.last_event_at = now - Duration::from_secs(5);
    let l = layout();
    let p = derive(&s, now, &l).unwrap();
    assert_eq!(p, Pose::SeatedThinking);
}

#[test]
fn idle_past_thinking_window_returns_idle_pose() {
    let (mut s, now) = slot(ActivityState::Idle, 25_000);
    s.last_event_at = now - Duration::from_secs(25);
    let l = layout();
    let p = derive(&s, now, &l).unwrap();
    assert_ne!(p, Pose::SeatedThinking);
}

#[test]
fn freshly_spawned_idle_skips_thinking() {
    let (s, now) = slot(ActivityState::Idle, 5_000);
    assert_eq!(s.last_event_at, s.created_at);
    let l = layout();
    let p = derive(&s, now, &l).unwrap();
    assert_ne!(p, Pose::SeatedThinking);
}

// `in_thinking_window` is the ONE gate both `state_driven_pose` (the pure
// overlay) and `derive_with_routing` (the routed render) consult — pin the
// predicate directly so a change to it is caught here at the interface, not
// left to drift between the two callers.
#[test]
fn in_thinking_window_gates_on_recency_and_prior_activity() {
    let (mut s, now) = slot(ActivityState::Idle, 5_000);

    // Freshly spawned (last_event_at == created_at): never active → false.
    assert_eq!(s.last_event_at, s.created_at);
    assert!(!in_thinking_window(&s, now));

    // Recently active, still inside the window → true.
    s.last_event_at = now - Duration::from_secs(THINKING_WINDOW_SECS - 1);
    assert!(in_thinking_window(&s, now));

    // Active, but the window has lapsed → false.
    s.last_event_at = now - Duration::from_secs(THINKING_WINDOW_SECS + 1);
    assert!(!in_thinking_window(&s, now));
}

fn first_trip_cycle_to_kind(
    agent_id: AgentId,
    layout: &SceneLayout,
    target_kind: WaypointKind,
) -> Option<u64> {
    (0u64..2000).find(|n| {
        takes_trip(agent_id, *n) && !is_aimless_cycle(agent_id, *n) && {
            let idx = waypoint_index_for_cycle(agent_id, *n, layout.waypoints.len());
            layout.waypoints[idx].kind == target_kind
        }
    })
}

#[test]
fn walk_back_from_pantry_carries_coffee() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let l = layout();
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let (_, _, at_wp_end, _) = phases(test_slot.agent_id);
    let trip_n = first_trip_cycle_to_kind(test_slot.agent_id, &l, WaypointKind::Pantry)
        .expect("agent should visit Pantry within 2000 cycles");
    // Place time in the walk-back phase (phase 3).
    let midpoint = trip_n * cycle + at_wp_end + (cycle - at_wp_end) / 2;
    let (s, now) = slot(ActivityState::Idle, midpoint);
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking {
            carrying_coffee, ..
        } => {
            assert!(carrying_coffee, "walk-back from Pantry must carry coffee");
        }
        other => panic!("expected Walking, got {other:?}"),
    }
}

#[test]
fn walk_back_from_non_pantry_no_coffee() {
    let (test_slot, _) = slot(ActivityState::Idle, 0);
    let l = layout();
    let cycle = est_wander_cycle_ms(test_slot.agent_id);
    let (_, _, at_wp_end, _) = phases(test_slot.agent_id);
    // Find a trip cycle to a non-Pantry waypoint.
    let trip_n = (0u64..2000)
        .find(|n| {
            takes_trip(test_slot.agent_id, *n) && !is_aimless_cycle(test_slot.agent_id, *n) && {
                let idx = waypoint_index_for_cycle(test_slot.agent_id, *n, l.waypoints.len());
                l.waypoints[idx].kind != WaypointKind::Pantry
            }
        })
        .expect("agent should visit a non-Pantry waypoint within 2000 cycles");
    let midpoint = trip_n * cycle + at_wp_end + (cycle - at_wp_end) / 2;
    let (s, now) = slot(ActivityState::Idle, midpoint);
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking {
            carrying_coffee, ..
        } => {
            assert!(
                !carrying_coffee,
                "walk-back from non-Pantry must NOT carry coffee"
            );
        }
        other => panic!("expected Walking, got {other:?}"),
    }
}

#[test]
fn entry_walk_does_not_carry_coffee() {
    let id = AgentId::from_transcript_path("/p/entry-coffee.jsonl");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let s = AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("abc"),
        cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
        label: "cc".into(),
        state: ActivityState::Idle,
        state_started_at: now0,
        created_at: now0,
        last_event_at: now0,
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
    let probe = now0 + Duration::from_millis(1500);
    let l = layout();
    match derive(&s, probe, &l).expect("pose") {
        Pose::Walking {
            carrying_coffee, ..
        } => {
            assert!(!carrying_coffee, "entry walk must not carry coffee");
        }
        other => panic!("expected Walking (entry), got {other:?}"),
    }
}

/// `derive_state_only` must return the state-driven pose even when the
/// slot is inside the entry-animation window (now - created_at < 4 s).
/// This proves it does NOT emit the door→desk entry Walking pose that
/// `derive` would return — preventing double-walk when the routed motion physics
/// layer is already driving its own entry walk.
#[test]
fn derive_state_only_skips_entry_override() {
    let id = AgentId::from_transcript_path("/p/entry-so.jsonl");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    // Slot with created_at == now0; probe at 1500 ms → inside entry window.
    let s = AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("abc"),
        cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
        label: "cc".into(),
        // Active state so we can assert a non-Walking result.
        state: ActivityState::Active {
            tool_use_id: Some("t".into()),
            detail: Some("Edit".into()),
            kind: ToolKind::Edit,
        },
        state_started_at: now0,
        created_at: now0,
        last_event_at: now0,
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
    let probe = now0 + Duration::from_millis(1500);
    let l = layout();

    // `derive` would return a door→desk Walking pose here.
    match derive(&s, probe, &l).expect("derive pose") {
        Pose::Walking { .. } => {} // expected — entry override fires in derive
        other => panic!("derive should return Walking in entry window, got {other:?}"),
    }

    // `derive_state_only` must return the state-driven pose (SeatedTyping),
    // NOT the entry Walking.
    match derive_state_only(&s, probe, &l).expect("derive_state_only pose") {
        Pose::SeatedTyping { .. } => {}
        other => panic!(
            "derive_state_only should return SeatedTyping for Active slot in entry window, got {other:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Thinking-gate release continuity (#22)
// ---------------------------------------------------------------------------

/// Search the id space for an agent matching `pred` (personality/dwell are
/// id-hashed, so tests that need a specific combination scan for one).
fn agent_matching(pred: impl Fn(AgentId) -> bool) -> AgentId {
    (0..100_000u64)
        .map(|i| AgentId::from_transcript_path(&format!("/think/{i}.jsonl")))
        .find(|id| pred(*id))
        .expect("id space should contain a matching agent")
}

/// Build an Idle slot whose SeatedThinking gate releases `hold_ms` into the
/// Idle period: `last_event_at = state_started_at - (THINKING_WINDOW - hold)`,
/// i.e. the reducer settled to Idle (window - hold) after the last event.
fn thinking_slot(id: AgentId, hold_ms: u64) -> AgentSlot {
    let (mut s, _) = slot(ActivityState::Idle, 0);
    s.agent_id = id;
    s.last_event_at =
        s.state_started_at - Duration::from_millis(THINKING_WINDOW_SECS * 1000 - hold_ms);
    s
}

#[test]
fn thinking_gate_release_is_continuous_with_seated_thinking() {
    // The gate holds SeatedThinking while `since_last_event < 20s`, but the
    // wander clock runs from `state_started_at` throughout — so when the
    // release lands PAST the cycle's seated phase, `derive()` jumped straight
    // from SeatedThinking (at the desk) to mid-Walking / AtWaypoint (a
    // desk→corridor pop in every stateless consumer). The release cycle must
    // instead stay seated: SeatedThinking → SeatedIdle is continuous.
    let l = layout();
    // Settle lag 2s → gate releases 18s into the Idle period.
    let hold_ms = THINKING_WINDOW_SECS * 1000 - 2_000;
    let id = agent_matching(|id| takes_trip(id, 0) && seated_dwell_ms(id) + 500 < hold_ms);
    let s = thinking_slot(id, hold_ms);

    // Just before release: the gate itself holds the thinking pose.
    let before = s.state_started_at + Duration::from_millis(hold_ms - 100);
    assert_eq!(derive(&s, before, &l), Some(Pose::SeatedThinking));

    // Just after release: continuous with the desk, NOT mid-wander.
    let after = s.state_started_at + Duration::from_millis(hold_ms + 200);
    assert_eq!(
        derive(&s, after, &l),
        Some(Pose::SeatedIdle),
        "gate release must land on a desk-seated pose, not mid-wander"
    );
}

#[test]
fn walk_out_intact_when_gate_releases_during_the_seated_phase() {
    // When the gate releases while the cycle is still in its seated phase
    // (hold <= seated_dwell), nothing was masked — the cycle-0 walk-out must
    // run normally from its beginning (pins that the continuity guard does
    // not over-suppress).
    let l = layout();
    let hold_ms = THINKING_WINDOW_SECS * 1000 - 2_000;
    let id = agent_matching(|id| takes_trip(id, 0) && seated_dwell_ms(id) > hold_ms + 1_000);
    let s = thinking_slot(id, hold_ms);
    let desk = l.home_desks[0];

    let mid_walk_out = seated_dwell_ms(id) + WANDER_WALK_EST_MS / 2;
    let now = s.state_started_at + Duration::from_millis(mid_walk_out);
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { from, t_x1000, .. } => {
            assert_eq!(from, desk, "cycle-0 walk-out starts from the desk");
            assert!((400..=600).contains(&t_x1000), "t_x1000={t_x1000}");
        }
        other => panic!("expected the cycle-0 walk-out, got {other:?}"),
    }
}

#[test]
fn cycle_after_a_suppressed_release_walks_out_from_its_beginning() {
    // The continuity guard is confined to the RELEASE cycle: the next trip
    // cycle's walk-out plays in full, starting at phase 0.
    let l = layout();
    let hold_ms = THINKING_WINDOW_SECS * 1000 - 2_000;
    let id = agent_matching(|id| {
        takes_trip(id, 0) && takes_trip(id, 1) && seated_dwell_ms(id) + 500 < hold_ms
    });
    let s = thinking_slot(id, hold_ms);
    let desk = l.home_desks[0];

    let cycle = est_wander_cycle_ms(id);
    let mid_walk_out = cycle + seated_dwell_ms(id) + WANDER_WALK_EST_MS / 2;
    let now = s.state_started_at + Duration::from_millis(mid_walk_out);
    match derive(&s, now, &l).expect("pose") {
        Pose::Walking { from, t_x1000, .. } => {
            assert_eq!(from, desk, "cycle-1 walk-out starts from the desk");
            assert!((400..=600).contains(&t_x1000), "t_x1000={t_x1000}");
        }
        other => panic!("expected the cycle-1 walk-out, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// pick_aimless_dest fallback walkability (#24)
// ---------------------------------------------------------------------------

#[test]
fn aimless_fallback_scans_the_midline_for_a_walkable_cell() {
    // The corridor midline hosts furniture footprints (vending / printer /
    // water cooler / trash), so the post-probe fallback point can be blocked.
    // Force the fallback (only ONE walkable pixel in the whole mask — the 32
    // zone probes virtually never hit it) and require a walkable result.
    let mut l = layout();
    let c = l.corridor.unwrap_or(l.cubicle_aisle);
    let mid_y = c.y + c.height / 2;
    let open = Point {
        x: c.x + c.width - 2,
        y: mid_y,
    };
    let mut mask = pixtuoid_core::walkable::WalkableMask::new_open(l.buf_w, l.buf_h);
    mask.mark_blocked(0, 0, l.buf_w, l.buf_h, 0);
    mask.mark_walkable(open.x, open.y, 1, 1);
    l.walkable = mask;

    let desk = l.home_desks[0];
    for seed in 0..16u64 {
        let p = pick_aimless_dest(&l, seed, desk);
        assert!(
            l.is_walkable(p.x, p.y),
            "seed {seed}: fallback returned a blocked cell {p:?}"
        );
        assert_eq!(
            p,
            pick_aimless_dest(&l, seed, desk),
            "must stay deterministic in (layout, seed)"
        );
    }
}

#[test]
fn aimless_fallback_on_a_fully_blocked_mask_returns_the_desk_anchor() {
    // Degenerate layout (nothing walkable at all): the fallback must hand out
    // the agent's own desk anchor — a destination every consumer already
    // handles (A* snap / render anchor) — rather than a cell inside furniture.
    let mut l = layout();
    let mut mask = pixtuoid_core::walkable::WalkableMask::new_open(l.buf_w, l.buf_h);
    mask.mark_blocked(0, 0, l.buf_w, l.buf_h, 0);
    l.walkable = mask;

    let desk = l.home_desks[0];
    for seed in 0..8u64 {
        assert_eq!(
            pick_aimless_dest(&l, seed, desk),
            crate::layout::desk_walk_anchor(desk),
            "seed {seed}: fully blocked corridor must fall back to the desk anchor"
        );
    }
}
