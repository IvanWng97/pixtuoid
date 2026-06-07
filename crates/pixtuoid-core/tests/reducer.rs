use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{Activity, AgentEvent, Transport};
use pixtuoid_core::state::reducer::{Reducer, HOOK_WINS_WINDOW};
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;

fn start(reducer: &mut Reducer, scene: &mut SceneState, id: AgentId) {
    reducer.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
}

/// Delegation scaffold shared by the pre-pass ordering pins: parent created
/// via Hook at `t0`, child created via Jsonl at `t0 + 100ms` with the parent
/// link (the same two-transport shape the sibling lifecycle tests hand-roll).
fn delegating_pair(
    r: &mut Reducer,
    scene: &mut SceneState,
    slug: &str,
    t0: SystemTime,
) -> (AgentId, AgentId) {
    let parent = AgentId::from_transcript_path(&format!("/p/{slug}.jsonl"));
    let child = AgentId::from_parts("claude-code", &format!("/p/{slug}/subagents/agent-1.jsonl"));
    r.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    (parent, child)
}

/// Assert the slot renders Active("Delegating") — `ToolDetail::Task`'s
/// display, set by `fsm::enter_delegating`.
#[track_caller]
fn assert_delegating(scene: &SceneState, id: AgentId, msg: &str) {
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Delegating"), "{msg}");
        }
        other => panic!("expected Active(Delegating), got {other:?} — {msg}"),
    }
}

#[test]
fn session_start_creates_idle_slot_at_first_free_desk() {
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).expect("agent inserted");
    assert_eq!(slot.desk_index, 0);
    assert_eq!(
        &*slot.label, "cc·repo",
        "label = source prefix + cwd basename"
    );
    assert_eq!(slot.state, ActivityState::Idle);
}

#[test]
fn activity_start_sets_state_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: Some("Edit: foo.rs".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(
        slot.state,
        ActivityState::Active {
            activity: Activity::Typing,
            ..
        }
    ));
}

#[test]
fn activity_end_arms_debounce_then_tick_flips_to_idle() {
    // After ActivityEnd the slot stays VISUALLY Active for
    // ACTIVE_GRACE_WINDOW (1500ms) — this hides per-tool-call flicker
    // from rapid CC tool chains. `pending_idle_at` is the debounce
    // armed-flag; `reducer.tick` (or another event past the window)
    // realizes the transition.
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );

    // Immediately after ActivityEnd — still Active, debounce armed.
    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(slot.pending_idle_at.is_some());

    // Tick before window expires — still Active.
    r.tick(&mut scene, t0 + Duration::from_millis(900));
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));

    // Tick past the window — flips to Idle.
    r.tick(&mut scene, t0 + Duration::from_millis(2000));
    assert_eq!(scene.agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_none());
}

#[test]
fn activity_start_inside_grace_window_cancels_debounce() {
    // A new tool starting before the debounce window expires must
    // cancel the pending-idle so the slot reads as continuously
    // Active for chained tool work (Read → Glob → Edit etc.).
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_some());
    // Second tool starts 200ms later — well inside the grace window.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t2".into()),
            detail: None,
        },
        t0 + Duration::from_millis(300),
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(
        slot.pending_idle_at.is_none(),
        "ActivityStart inside grace must cancel pending idle"
    );
    // Tick well past the original ActivityEnd's grace — must still be Active.
    r.tick(&mut scene, t0 + Duration::from_millis(2500));
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));
}

#[test]
fn waiting_sets_state_with_reason() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "Bash: rm -rf?".into(),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Waiting { reason } => assert_eq!(&**reason, "Bash: rm -rf?"),
        other => panic!("unexpected state: {other:?}"),
    }
}

#[test]
fn session_end_marks_slot_exiting_then_tick_removes_it_after_grace() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let a = AgentId::from_transcript_path("/p/a.jsonl");
    let b = AgentId::from_transcript_path("/p/b.jsonl");
    start(&mut r, &mut scene, a);
    start(&mut r, &mut scene, b);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: a },
        t0,
        Transport::Hook,
    );

    let slot = scene
        .agents
        .get(&a)
        .expect("slot still present during exit walk-out");
    assert!(
        slot.exiting_at.is_some(),
        "SessionEnd should mark exiting_at"
    );

    r.tick(
        &mut scene,
        t0 + EXIT_GRACE_WINDOW + std::time::Duration::from_millis(100),
    );
    assert!(
        !scene.agents.contains_key(&a),
        "tick should sweep expired exit"
    );
    assert_eq!(scene.next_free_desk(), Some(0));
}

#[test]
fn jsonl_duplicate_of_recent_hook_is_dropped() {
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("t-1".into()),
            detail: Some("FROM_JSONL".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    match &slot.state {
        ActivityState::Active {
            activity, detail, ..
        } => {
            assert_eq!(*activity, Activity::Typing, "hook event must win");
            assert_ne!(
                detail.as_deref(),
                Some("FROM_JSONL"),
                "jsonl detail must not overwrite"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// Bug 2: CC's hook payloads set transcript_path to the PARENT'S transcript
/// even for actions originating in a subagent. Those leak hook events onto
/// the parent's slot, making the parent sprite blink while the actual work
/// is in a subagent. Once the parent has a Task tool in flight, hook
/// ActivityStart/End events for that AgentId should be suppressed — the
/// JSONL stream is authoritative for the subagent (separate AgentId).
#[test]
fn hook_activity_during_active_task_is_suppressed() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    start(&mut r, &mut scene, parent);

    let t0 = SystemTime::now();

    // Parent enters Task tool — hook fires first, carrying the tool_use_id.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0,
        Transport::Hook,
    );

    // Subagent fires a Read hook. CC reports it on parent's transcript_path,
    // so it lands on parent's AgentId — we must drop it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("subagent-R".into()),
            detail: Some("Read: /foo".into()),
        },
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );

    // Parent slot should still reflect Task (now rendered as "Delegating"
    // per ToolDetail::display), not the leaked Read.
    let slot = scene.agents.get(&parent).unwrap();
    match &slot.state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Delegating"));
        }
        other => panic!("expected Active(Delegating), got {other:?}"),
    }

    // Subagent's PostToolUse hook for Read also lands on parent — drop it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("subagent-R".into()),
        },
        t0 + Duration::from_millis(60),
        Transport::Hook,
    );
    let slot = scene.agents.get(&parent).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "parent must remain Active(Task) while task in flight"
    );

    // Task's own PostToolUse: tool_use_id matches the in-flight Task, so the
    // hook IS allowed through. With the Active-grace debounce, the
    // transition to Idle is deferred — `pending_idle_at` arms now,
    // `reducer.tick` past ACTIVE_GRACE_WINDOW (1500ms) realizes it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_millis(200),
        Transport::Hook,
    );
    let slot = scene.agents.get(&parent).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(slot.pending_idle_at.is_some());
    r.tick(&mut scene, t0 + Duration::from_millis(2000));
    assert_eq!(
        scene.agents.get(&parent).unwrap().state,
        ActivityState::Idle
    );
}

/// JSONL is the authoritative attribution for subagent work — its events
/// go to the subagent's own AgentId (different file path) and must NOT be
/// affected by the parent's active Task suppression.
#[test]
fn subagent_jsonl_activity_is_unaffected_by_parent_task_suppression() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let subagent = AgentId::from_transcript_path("/p/parent/subagents/agent-x.jsonl");
    start(&mut r, &mut scene, parent);
    start(&mut r, &mut scene, subagent);

    let t0 = SystemTime::now();
    // Parent enters a Task.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0,
        Transport::Hook,
    );
    // Subagent's JSONL activity targets ITS OWN AgentId — must apply normally.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: subagent,
            activity: Activity::Typing,
            tool_use_id: Some("sub-R".into()),
            detail: Some("Read: /bar".into()),
        },
        t0 + Duration::from_millis(120),
        Transport::Jsonl,
    );
    match &scene.agents.get(&subagent).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Read: /bar"));
        }
        other => panic!("subagent slot should be Active, got {other:?}"),
    }
}

/// Pre-existing behavior: with no active Task, hook events apply normally.
#[test]
fn hook_activity_without_active_task_applies_normally() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: Some("Bash: ls".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Bash: ls"));
        }
        other => panic!("expected Active, got {other:?}"),
    }
}

#[test]
fn session_start_with_cwd_derives_label_from_basename() {
    // No more "cc#1" when the cwd tells us what project this is.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/Users/me/Desktop/pixtuoid"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "cc·pixtuoid");
}

#[test]
fn session_start_without_cwd_falls_back_to_cc_label() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "cc#1");
}

#[test]
fn ghost_label_counter_is_contiguous_after_named_sessions() {
    // A named-cwd session must NOT consume a ghost ordinal: the first
    // unknown-cwd ghost is cc#1 even when named sessions preceded it.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let named = AgentId::from_transcript_path("/p/named.jsonl");
    let ghost = AgentId::from_transcript_path("/p/ghost.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: named,
            source: "claude-code".into(),
            session_id: "named".into(),
            cwd: PathBuf::from("/Users/me/Desktop/pixtuoid"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: ghost,
            source: "claude-code".into(),
            session_id: "ghost".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&named).unwrap().label, "cc·pixtuoid");
    assert_eq!(&*scene.agents.get(&ghost).unwrap().label, "cc#1");
}

#[test]
fn session_start_codex_source_gets_cx_label() {
    // Codex arrives via the shared hook socket (no JSONL Rename), so the cx·
    // prefix must come from the reducer at SessionStart.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("codex", "sess-1");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".into(),
            session_id: "sess-1".into(),
            cwd: PathBuf::from("/Users/me/work/myrepo"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "cx·myrepo");
}

#[test]
fn rename_updates_slot_label() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: id,
            label: "feature-dev:code-explorer".into(),
        },
        SystemTime::now(),
        Transport::Jsonl,
    );
    assert_eq!(
        &*scene.agents.get(&id).unwrap().label,
        "feature-dev:code-explorer"
    );
}

#[test]
fn rename_for_unknown_agent_is_noop() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/missing.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: id,
            label: "x".into(),
        },
        SystemTime::now(),
        Transport::Jsonl,
    );
    assert!(!scene.agents.contains_key(&id));
}

/// Regression guard: if a Hook PostToolUse arrives for a Task before its
/// JSONL ActivityStart (startup race where Pre was missed), the matching
/// JSONL ActivityEnd that always follows in the same transcript still drains
/// active_tasks. After the drain, normal hook events are no longer suppressed.
#[test]
fn active_tasks_drained_by_jsonl_end_even_if_hook_end_arrived_first() {
    use pixtuoid_core::source::ToolDetail;

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    // Hook PostToolUse arrives first (active_tasks empty — Pre was missed).
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-X".into()),
        },
        t0,
        Transport::Hook,
    );

    // JSONL ActivityStart for the same Task arrives after the hook dedup
    // window has expired — passes through and populates active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("task-X".into()),
            detail: Some(ToolDetail::Task),
        },
        t0 + Duration::from_millis(700),
        Transport::Jsonl,
    );

    // JSONL ActivityEnd from the same transcript drains active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-X".into()),
        },
        t0 + Duration::from_millis(800),
        Transport::Jsonl,
    );

    // Subsequent hook activity must apply normally — proves active_tasks drained.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("other".into()),
            detail: Some("Bash: ls".into()),
        },
        t0 + Duration::from_millis(900),
        Transport::Hook,
    );

    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(
                detail.as_deref(),
                Some("Bash: ls"),
                "active_tasks must drain so subsequent hook events apply"
            );
        }
        other => panic!("expected Active(Bash: ls), got {other:?}"),
    }
}

#[test]
fn jsonl_event_after_dedup_window_is_applied() {
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(
        slot.state,
        ActivityState::Active {
            activity: Activity::Reading,
            ..
        }
    ));
}

// --- stale-agent sweep ---------------------------------------------------

#[test]
fn stale_idle_agent_is_marked_exiting_after_timeout() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick just before the threshold — should NOT mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "should not mark exiting before timeout"
    );

    // Tick past the threshold — should mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "should mark exiting after timeout"
    );
}

#[test]
fn stale_active_agent_uses_shorter_timeout_than_idle() {
    use pixtuoid_core::state::reducer::{STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT};
    assert!(
        STALE_ACTIVE_TIMEOUT < STALE_IDLE_TIMEOUT,
        "active timeout should be shorter than idle"
    );

    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    reducer.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    // Active timeout is 10 min — should mark exiting after that.
    reducer.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "active agent should be reaped after STALE_ACTIVE_TIMEOUT"
    );
}

#[test]
fn codex_idle_agent_reaps_faster_than_claude_idle() {
    use pixtuoid_core::state::reducer::{STALE_CODEX_IDLE_TIMEOUT, STALE_IDLE_TIMEOUT};
    // Codex exposes no SessionEnd of any kind (no hook, no PID, no durable rollout
    // marker), so a closed Codex session can ONLY be reaped by the stale-sweep —
    // hence a much shorter idle window than CC, which has real SessionEnd signals
    // and keeps the long lunch-break-safe timeout.
    assert!(
        STALE_CODEX_IDLE_TIMEOUT < STALE_IDLE_TIMEOUT,
        "codex idle timeout must be shorter than the generic idle timeout"
    );

    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // One Codex agent and one Claude-Code agent, both idle since t0. The source
    // is carried by the SessionStart event (the AgentId is just the slot key).
    let cx = AgentId::from_transcript_path("/p/codex-sess.jsonl");
    let cc = AgentId::from_transcript_path("/p/cc-sess.jsonl");
    for (id, source) in [(cx, "codex"), (cc, "claude-code")] {
        reducer.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: source.into(),
                session_id: "s".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }

    // Just past the Codex idle window (but far under CC's 30 min): the Codex
    // sprite is reaped; the CC one is spared.
    reducer.tick(
        &mut scene,
        t0 + STALE_CODEX_IDLE_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&cx).unwrap().exiting_at.is_some(),
        "codex idle agent should reap after STALE_CODEX_IDLE_TIMEOUT"
    );
    assert!(
        scene.agents.get(&cc).unwrap().exiting_at.is_none(),
        "claude-code idle agent must NOT reap on the codex-fast window"
    );
}

#[test]
fn fresh_event_resets_stale_timer() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/fresh.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );

    // At 29 min (just before 30 min idle threshold), send a new event.
    let almost = t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(60);
    reducer.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "perm".into(),
        },
        almost,
        Transport::Hook,
    );

    // Now tick at original t0 + 31 min — should NOT reap because
    // last_event_at was reset to `almost` (29 min mark).
    reducer.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "fresh event should have reset the stale timer"
    );
}

#[test]
fn unknown_cwd_agent_reaps_faster() {
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/ghost.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // SessionStart with empty cwd → label falls back to "cc#N".
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::new(),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    let label = scene.agents.get(&id).unwrap().label.clone();
    assert!(
        label.contains('#'),
        "empty cwd should produce source#N label, got {label}"
    );

    // 3 min + 1s → should be reaped (STALE_UNKNOWN_CWD_TIMEOUT = 3 min).
    reducer.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "unknown-cwd agent should reap after STALE_UNKNOWN_CWD_TIMEOUT"
    );
}

#[test]
fn tool_call_count_increments_on_activity_start() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stats.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 0);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t2".into()),
            detail: None,
        },
        t0 + Duration::from_millis(600),
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 2);
}

#[test]
fn active_ms_accumulates_on_state_transitions() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().active_ms, 0);

    // End after 1 second, then tick past grace window to flush to Idle
    let t1 = t0 + Duration::from_secs(1);
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1,
        Transport::Hook,
    );
    // active_ms not yet accumulated (happens on next ActivityStart or expire)
    r.tick(&mut scene, t1 + Duration::from_secs(3));
    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 1000,
        "expected >= 1000ms active, got {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_does_not_double_count_on_duplicate_activity_end() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/dedup.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(2);
    // First ActivityEnd (hook)
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1,
        Transport::Hook,
    );
    // Second ActivityEnd (late JSONL, past dedup window)
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    // Flush to idle
    r.tick(&mut scene, t1 + Duration::from_secs(3));
    let slot = scene.agents.get(&id).unwrap();
    // Should be ~2-3s, not ~4-6s (double-counted)
    assert!(
        slot.active_ms < 5000,
        "active_ms looks double-counted: {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_preserved_when_task_arrives_during_active_tool() {
    use pixtuoid_core::source::ToolDetail;

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/task-active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    // Tool starts
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    // 2 seconds later, Task arrives while still Active (within grace window)
    let t1 = t0 + Duration::from_secs(2);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("task-1".into()),
            detail: Some(ToolDetail::Task),
        },
        t1,
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 2000,
        "expected >= 2000ms active from pre-Task tool span, got {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_preserved_when_waiting_interrupts_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/waiting.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(3);
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t1,
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 3000,
        "expected >= 3000ms active before Waiting, got {}",
        slot.active_ms
    );
}

#[test]
fn session_end_cascades_to_children() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/parent/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    assert!(scene.agents.get(&child).unwrap().exiting_at.is_none());

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: parent },
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent should be exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child should cascade to exiting when parent ends"
    );
}

#[test]
fn session_end_cascades_to_grandchildren() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let grandparent = AgentId::from_transcript_path("/p/gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandparent,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(grandparent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: grandparent,
        },
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "grandchild should cascade to exiting via BFS"
    );
}

#[test]
fn unknown_cwd_agent_uses_faster_stale_timeout() {
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/unknown.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "u".into(),
            cwd: PathBuf::new(),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(slot.unknown_cwd, "empty cwd should set unknown_cwd");

    r.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "unknown_cwd agent should reap after STALE_UNKNOWN_CWD_TIMEOUT"
    );
}

// --- parent-child cascade --------------------------------------------------

#[test]
fn session_end_cascade_marks_all_descendants_exiting() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/cascade-parent.jsonl");
    let child_a = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-a.jsonl");
    let child_b = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-b.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_a,
            source: "claude-code".into(),
            session_id: "ca".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_b,
            source: "claude-code".into(),
            session_id: "cb".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    assert!(scene.agents.get(&child_a).unwrap().exiting_at.is_none());
    assert!(scene.agents.get(&child_b).unwrap().exiting_at.is_none());

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: parent },
        t0 + Duration::from_secs(5),
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent must be marked exiting"
    );
    assert!(
        scene.agents.get(&child_a).unwrap().exiting_at.is_some(),
        "child_a must cascade to exiting when parent ends"
    );
    assert!(
        scene.agents.get(&child_b).unwrap().exiting_at.is_some(),
        "child_b must cascade to exiting when parent ends"
    );
}

// --- hook-wins dedup -------------------------------------------------------

#[test]
fn hook_wins_dedup_drops_jsonl_duplicate_within_window() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/dedup-hw.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    // Hook event first — establishes the tool_use_id in the dedup map.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("dedup-1".into()),
            detail: Some("Edit: hook.rs".into()),
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    // JSONL event with same tool_use_id within 500ms — must be dropped.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("dedup-1".into()),
            detail: Some("Edit: jsonl.rs".into()),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    // tool_call_count should still be 1 — JSONL duplicate was dropped.
    assert_eq!(
        scene.agents.get(&id).unwrap().tool_call_count,
        1,
        "JSONL duplicate inside hook-wins window must be dropped"
    );
    // State should still reflect the hook event.
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Edit: hook.rs"));
        }
        other => panic!("expected Active from hook, got {other:?}"),
    }
}

// --- sweep_stale -----------------------------------------------------------

#[test]
fn sweep_stale_marks_old_agent_exiting_on_tick() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale-sweep.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "sw".into(),
            cwd: PathBuf::from("/old-project"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick well past the idle stale timeout with no intervening events.
    r.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "tick past STALE_IDLE_TIMEOUT should mark agent exiting"
    );
}

#[test]
fn stale_sweep_cascades_to_children() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/stale-cascade.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-cascade/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Heartbeat the child so it is NOT independently stale at the tick below.
    // Only the parent (no events since t0) crosses STALE_IDLE_TIMEOUT, so the
    // child's exit can only come from the cascade.
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: child,
            label: "cc·sub".into(),
        },
        t0 + Duration::from_secs(25 * 60),
        Transport::Jsonl,
    );

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "stale parent should be marked exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child should cascade-exit with a stale-swept parent (it is not independently stale)"
    );
}

// When BOTH parent and child are independently stale, both enter sweep_stale's
// pass-1 `stale` vec. The parent's pass-2 cascade marks the child exiting; the
// child's own pass-2 iteration then hits the `exiting_at.is_some() -> continue`
// write-once guard (reducer.rs) instead of re-stamping / re-logging it. The
// existing cascade tests heartbeat the descendant so it is NEVER in `stale`, so
// they don't exercise this branch — this test drops the heartbeat.
#[test]
fn stale_sweep_already_cascaded_child_is_skipped_in_pass_two() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/double-stale.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/double-stale/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    // No heartbeat for either: both cross STALE_IDLE_TIMEOUT, so both enter the
    // pass-1 `stale` vec. The id is set once, on whichever pass-2 iteration runs
    // first; the other iteration must hit the write-once skip.
    let now = t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1);
    r.tick(&mut scene, now);

    let parent_exit = scene.agents.get(&parent).unwrap().exiting_at;
    let child_exit = scene.agents.get(&child).unwrap().exiting_at;
    assert!(parent_exit.is_some(), "stale parent marked exiting");
    assert!(
        child_exit.is_some(),
        "independently-stale child also marked exiting (write-once, no double-stamp)"
    );
    // Both stamped at the same sweep `now`: the pass-2 skip preserved the first
    // write rather than overwriting it on the second iteration.
    assert_eq!(parent_exit, Some(now));
    assert_eq!(child_exit, Some(now));
}

#[test]
fn stale_sweep_cascades_to_grandchildren() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let grandparent = AgentId::from_transcript_path("/p/stale-gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/stale-gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandparent,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(grandparent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    // Heartbeat the middle + leaf so only the grandparent is independently stale.
    for (id, label) in [(parent, "cc·p"), (child, "cc·c")] {
        r.apply(
            &mut scene,
            AgentEvent::Rename {
                agent_id: id,
                label: label.into(),
            },
            t0 + Duration::from_secs(25 * 60),
            Transport::Jsonl,
        );
    }

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "grandchild should cascade-exit via BFS through the stale grandparent"
    );
}

#[test]
fn stale_sweep_cascade_skips_unrelated_fresh_agents() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/stale-host.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-host/subagents/agent-1.jsonl");
    let unrelated = AgentId::from_transcript_path("/p/other-session.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: unrelated,
            source: "claude-code".into(),
            session_id: "other".into(),
            cwd: PathBuf::from("/other-repo"),
            parent_id: None,
        },
        t0 + Duration::from_millis(150),
        Transport::Hook,
    );
    // Heartbeat the child AND the unrelated agent so neither is independently
    // stale: only the parent crosses the threshold.
    for (id, label) in [(child, "cc·sub"), (unrelated, "cc·other")] {
        r.apply(
            &mut scene,
            AgentEvent::Rename {
                agent_id: id,
                label: label.into(),
            },
            t0 + Duration::from_secs(25 * 60),
            Transport::Jsonl,
        );
    }

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the stale parent's child must cascade-exit"
    );
    assert!(
        scene.agents.get(&unrelated).unwrap().exiting_at.is_none(),
        "a fresh, unrelated agent must NOT be cascaded out"
    );
}

#[test]
fn long_delegation_keeps_parent_and_live_subagent_alive() {
    // A parent delegating a single Task longer than STALE_ACTIVE_TIMEOUT
    // gets no events of its OWN — the subagent's hook events are misattributed
    // to the parent's AgentId and suppressed. Those suppressed events are still
    // proof the subtree is alive, so they must refresh the parent's
    // last_event_at; otherwise sweep_stale reaps the live parent and the
    // cascade drags its still-working subagent out with it.
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/deleg.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/deleg/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    // Parent delegates one long Task → Active{Delegating}. The Task-start arm
    // does NOT bump last_event_at, so the parent's liveness is frozen at t0.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    // The subagent works for ~9 min; each tool call is a hook event CC
    // misattributes to the parent's AgentId, so the reducer suppresses it.
    for (mins, tuid) in [(5u64, "sub-R1"), (9u64, "sub-R2")] {
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: parent,
                activity: Activity::Typing,
                tool_use_id: Some(tuid.into()),
                detail: Some("Read: /x".into()),
            },
            t0 + Duration::from_secs(mins * 60),
            Transport::Hook,
        );
    }

    // Tick just past the parent's Active stale threshold measured from t0, but
    // well within it measured from the last suppressed child event (t0+9min).
    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "a delegating parent must stay alive while its subagent emits events"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the live subagent must NOT be cascaded out by a falsely-stale parent"
    );
}

#[test]
fn stale_sweep_spares_subagent_blocked_under_a_waiting_parent() {
    // A subagent's permission prompt is attributed to the PARENT (hook
    // transcript_path → parent), so the parent goes Waiting (60-min) while the
    // subagent stays Active (its last tool, 10-min) and emits nothing while
    // blocked. The subagent is alive — waiting on a human gate the parent holds
    // — so the stale-sweep must NOT reap it on the aggressive Active timer.
    // Liveness vs readiness: a node under a Waiting ancestor is "not ready",
    // not "dead".
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/perm-parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/perm-parent/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Subagent runs a tool → Active (10-min stale timeout).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: child,
            activity: Activity::Typing,
            tool_use_id: Some("c-tool".into()),
            detail: Some("WebFetch: /x".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    // That tool needs permission → CC's Notification hook lands on the PARENT.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission?".into(),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    // User ignores the prompt for >10 min. No further events.
    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "Waiting parent (60-min threshold) must survive a 10-min wait"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a subagent blocked under a Waiting parent must NOT be reaped on the Active timer"
    );
}

#[test]
fn stale_sweep_spares_grandchild_under_a_waiting_ancestor() {
    // The readiness exemption walks the whole parent_id chain: a stale
    // grandchild whose grandparent is Waiting is still "blocked", not dead.
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let gp = AgentId::from_transcript_path("/p/perm-gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/perm-gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/perm-gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: gp,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(gp),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    // Middle + leaf are Active (10-min); grandparent holds the permission gate.
    for id in [parent, child] {
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: id,
                activity: Activity::Typing,
                tool_use_id: Some("t".into()),
                detail: Some("WebFetch: /x".into()),
            },
            t0 + Duration::from_secs(1),
            Transport::Jsonl,
        );
    }
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: gp,
            reason: "permission?".into(),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60),
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a grandchild under a Waiting ancestor must NOT be reaped on the Active timer"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the middle agent under a Waiting ancestor must NOT be reaped either"
    );
}

#[test]
fn active_subagent_keeps_parent_alive_via_jsonl_events() {
    // Liveness flows up the tree via the subagent's OWN JSONL events — not only
    // suppressed hook events (hooks are best-effort and can drop). A subagent
    // actively emitting JSONL keeps its delegating parent from being
    // stale-swept, so the cascade can't evict the live subagent.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/deleg2.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/deleg2/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Parent delegates → Active{Delegating} (10-min threshold); its OWN last
    // event is now frozen at t0+1s.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Subagent works for >10 min, emitting ONLY JSONL events (no hooks reach the
    // parent). Each keeps the parent's lineage alive.
    for mins in [4u64, 8, 12] {
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: child,
                activity: Activity::Typing,
                tool_use_id: Some("c".into()),
                detail: Some("Read: /x".into()),
            },
            t0 + Duration::from_secs(mins * 60),
            Transport::Jsonl,
        );
    }
    // Tick shortly after the last child event — but ~12 min past the parent's
    // OWN last event (the Task start at t0+1s).
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(12 * 60) + Duration::from_secs(30),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "a delegating parent must stay alive while its subagent emits JSONL events"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the live subagent must not be cascaded out by a falsely-stale parent"
    );
}

#[test]
fn subagent_is_removed_promptly_when_its_parent_task_completes() {
    // b1 (Task-drain completion inference): CC writes no "subagent finished"
    // marker, so we infer completion — when the parent's LAST Task drains, the
    // delegated subtree returned, and its subagents must leave promptly (marked
    // exiting) instead of lingering as zombies to the 30-min idle stale-sweep.
    // The parent keeps running.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/orch.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/orch/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Parent delegates a Task → Active{Delegating}, active_tasks[parent]={task-T}.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Subagent does some work.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: child,
            activity: Activity::Typing,
            tool_use_id: Some("c1".into()),
            detail: Some("Read: /x".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    // The Task returns to the parent → drains active_tasks → subagent completed.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "a completed subagent must leave promptly when its parent's Task drains, not linger to the 30-min idle sweep"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the parent keeps running after a Task completes"
    );
}

#[test]
fn jsonl_duplicate_task_end_inside_dedup_window_does_not_fire_cascade() {
    // Ordering pin for `apply()`'s pre-passes (#90): hook-wins dedup MUST run
    // BEFORE task tracking. The hook ActivityStart records the Task's
    // tool_use_id in the dedup map; a JSONL duplicate carrying that id inside
    // HOOK_WINS_WINDOW must be dropped BEFORE it can drain active_tasks and
    // cascade-exit a still-working subtree (b1 above fires on real drains
    // only). Offsets derive from HOOK_WINS_WINDOW so the pin survives any
    // retuning of the window.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-dup", t0);

    // Hook Task start → records "task-T" in the dedup map AND active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // JSONL duplicate END for the same tool_use_id, inside HOOK_WINS_WINDOW.
    // If task tracking ran before dedup, this would drain active_tasks and
    // cascade-exit the child mid-delegation.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a JSONL duplicate of the in-flight Task must be dropped by dedup before it can drain active_tasks and cascade-exit the still-working subtree"
    );
    assert_delegating(
        &scene,
        parent,
        "parent must stay Delegating across the duplicate",
    );
    // Defense-in-depth: if a future change decouples the Task drain from the
    // b1 cascade, the exiting_at assert above goes vacuous — this probe still
    // catches the ordering mutant, because a later misattributed hook event
    // is only suppressed while active_tasks is non-empty.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("subagent-R".into()),
            detail: Some("Read: /foo".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    assert_delegating(
        &scene,
        parent,
        "active_tasks must survive the duplicate — later misattributed hooks stay suppressed",
    );
}

#[test]
fn suppressed_parallel_task_dispatch_jsonl_copy_survives_dedup_and_tracks() {
    // Ordering pin for `apply()`'s pre-passes (#90), leg (1): suppression
    // MUST run before the hook dedup RECORD. A parallel SECOND Task dispatch
    // arrives as a hook ActivityStart while the first Task is in flight, so
    // the leak-suppression drops it — and must drop it BEFORE its
    // tool_use_id is recorded, or the dedup would also kill the JSONL copy
    // (same parent AgentId + tool_use_id — the dispatch lives in the
    // parent's own transcript), leaving the second Task untracked: the
    // first Task's drain would then empty active_tasks and cascade-exit the
    // still-working subtree one Task early. Offsets derive from
    // HOOK_WINS_WINDOW so the pin survives any retuning of the window.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-par", t0);

    // First Task dispatch — applies normally, active_tasks = {task-1}.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-1".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Parallel SECOND Task dispatch via hook while task-1 is in flight —
    // suppressed as a leak (and must NOT record "task-2" in the dedup map).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-2".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 10,
        Transport::Hook,
    );
    // The JSONL copy of the second dispatch, inside HOOK_WINS_WINDOW of the
    // suppressed hook copy (so a wrongly-recorded "task-2" would dedup-drop
    // it). It must survive dedup — it is the only transport left to track
    // task-2.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-2".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 10 + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );
    // First Task's own PostToolUse drains task-1. task-2 must still be in
    // flight, so the subtree must NOT cascade-exit yet.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW * 2 / 5,
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "first Task's drain must not cascade-exit the subtree while the suppressed-then-JSONL-tracked second Task is still in flight"
    );
    assert_delegating(
        &scene,
        parent,
        "parent must stay Delegating on the second Task",
    );

    // Teeth: draining the SECOND Task must fire the cascade — proves the
    // child is wired to the parent and the earlier no-cascade assertion
    // wasn't vacuous.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "last Task's drain must cascade-exit the completed subtree"
    );
}

#[test]
fn parent_waiting_on_subagent_permission_resolves_when_the_subagent_resumes() {
    // During delegation a subagent's permission Notification is misattributed to
    // the parent → the parent goes Waiting. When the subagent resumes work (a
    // suppressed child hook event arrives while the parent is still delegating),
    // the gate has resolved — the parent must return to Active(Delegating), not
    // sit on a stale "permission?" Waiting until the 60-min stale-sweep.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/orch.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/orch/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Parent delegates → Active{Delegating}, active_tasks[parent]={task-T}.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Subagent's permission prompt → Notification misattributed to the parent.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission?".into(),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "parent goes Waiting on the subagent's permission"
    );

    // User grants; the subagent resumes a tool → a misattributed child hook,
    // suppressed because the parent is in-Task.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("sub-bash".into()),
            detail: Some("Bash: ls".into()),
        },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );

    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Active { .. }
        ),
        "parent resumes Active(Delegating) once the subagent works again — no stale Waiting"
    );
    assert!(scene.agents.get(&child).unwrap().exiting_at.is_none());
}

/// With heterogeneous per-floor capacities, the third session should
/// overflow from floor 0 (cap=2) to floor 1's first desk (global index 2).
#[test]
fn session_start_overflows_to_floor1_with_heterogeneous_capacity() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 4, 0, 0, 0]);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    for i in 0..3 {
        let id = AgentId::from_transcript_path(&format!("/proj/{i}.jsonl"));
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "cc".into(),
                session_id: format!("s{i}"),
                cwd: std::path::PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Jsonl,
        );
    }
    assert_eq!(scene.agents.len(), 3);
    let desks: Vec<usize> = scene.agents.values().map(|a| a.desk_index).collect();
    assert!(desks.contains(&0));
    assert!(desks.contains(&1));
    assert!(
        desks.contains(&2),
        "third agent should get desk 2 (floor 1)"
    );
    assert_eq!(scene.floor_of(2), 1);
}

#[test]
fn session_start_dropped_when_all_desks_occupied() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 0, 0, 0, 0]);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    for i in 0..2 {
        let id = AgentId::from_transcript_path(&format!("/proj/{i}.jsonl"));
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "cc".into(),
                session_id: format!("s{i}"),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }
    assert_eq!(scene.agents.len(), 2);
    assert!(scene.next_free_desk().is_none());

    let overflow_id = AgentId::from_transcript_path("/proj/overflow.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: overflow_id,
            source: "cc".into(),
            session_id: "s-overflow".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.len(),
        2,
        "third SessionStart must be silently dropped when desks are full"
    );
    assert!(
        !scene.agents.contains_key(&overflow_id),
        "overflow agent should not exist"
    );
}

// A CC permission Notification fires while a tool (t1) is mid-flight:
//   PreToolUse(t1)[Active] -> Notification[Waiting] -> PostToolUse(t1).
// PostToolUse(t1) means t1 ran (permission granted) and finished, so the
// Waiting is RESOLVED. Captured live (probe): the gated tool's ActivityEnd
// carries the same tool_use_id that was Active when Waiting began. Resolving on
// it clears the question-mark when the tool finishes instead of holding it
// until the agent's *next* tool (~6 s later). Debounced through pending_idle
// like a normal Active->Idle so a fast next tool doesn't flicker.
#[test]
fn gated_tool_end_while_waiting_resolves_to_idle_after_grace() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );

    // The gated tool's own PostToolUse arrives — arms the idle debounce, still
    // visually Waiting for the grace window (no instant flip).
    let end = t0 + Duration::from_millis(1000);
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        end,
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Waiting { .. }),
        "still Waiting during grace, got {:?}",
        slot.state
    );
    assert!(
        slot.pending_idle_at.is_some(),
        "gated tool end must arm the resolve debounce"
    );

    // After the grace window, the resolved Waiting settles to Idle.
    r.tick(
        &mut scene,
        end + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(scene.agents.get(&id).unwrap().state, ActivityState::Idle),
        "resolved permission must settle to Idle, got {:?}",
        scene.agents.get(&id).unwrap().state
    );
}

// Protection (preserved): a PARALLEL tool (t2) ending while a DIFFERENT tool's
// permission (t1) is still pending must NOT clear the Waiting — the id doesn't
// match the gated tool, so the prompt stays up.
#[test]
fn parallel_tool_end_while_waiting_keeps_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );

    // A different tool ends — must be ignored (its permission isn't this one).
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t2".into()),
        },
        t0 + Duration::from_millis(1000),
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Waiting { .. }),
        "parallel tool end must keep Waiting, got {:?}",
        slot.state
    );
    assert!(
        slot.pending_idle_at.is_none(),
        "parallel tool end must not arm the resolve debounce"
    );

    // ...and it does NOT resolve even after the grace window passes.
    r.tick(
        &mut scene,
        t0 + Duration::from_millis(1000) + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "still Waiting — permission t1 never resolved"
    );
}

// Regression (adversarial review): a parent Waiting on a permission while a
// Task is in flight must NOT be false-cleared to Idle when that Task drains.
// The Task-drain debounce arms `pending_idle_at`; without a state guard it
// would trip the resolved-Waiting expiry even though the permission is still
// pending (e.g. a parallel Task + a permission-gated Bash in the same turn).
#[test]
fn task_drain_while_parent_waiting_keeps_waiting() {
    use pixtuoid_core::source::ToolDetail;
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    // Parent delegates a Task → Active{Delegating, task-T}.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some(ToolDetail::Task),
        },
        t0,
        Transport::Hook,
    );
    // A permission prompt fires while delegating → Waiting.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    assert!(matches!(
        scene.agents[&id].state,
        ActivityState::Waiting { .. }
    ));

    // The Task's own PostToolUse drains active_tasks — must NOT arm an idle
    // resolve on the Waiting parent (permission still pending).
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_millis(1000),
        Transport::Hook,
    );
    assert!(
        scene.agents[&id].pending_idle_at.is_none(),
        "Task drain must not arm idle-resolve on a Waiting parent"
    );

    // ...and it stays Waiting past the grace window.
    r.tick(
        &mut scene,
        t0 + Duration::from_millis(1000) + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(scene.agents[&id].state, ActivityState::Waiting { .. }),
        "parent's permission must stay Waiting through a Task drain, got {:?}",
        scene.agents[&id].state
    );
}

#[test]
fn codex_permission_then_jsonl_output_resumes_to_active() {
    // Regression: a cx· agent stuck Waiting on a permission prompt must return
    // to Active once the transcript's function_call_output (an ActivityStart)
    // arrives. Hook and JSONL coalesce on the session UUID.
    use pixtuoid_core::source::ToolDetail;
    let mut reducer = Reducer::new();
    let mut scene = SceneState::uniform(4);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let id = AgentId::from_parts("codex", uuid);

    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".into(),
            session_id: uuid.into(),
            cwd: PathBuf::from("/Users/me/dotfiles"),
            parent_id: None,
        },
        now,
        Transport::Hook,
    );

    reducer.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        now,
        Transport::Hook,
    );
    assert!(
        matches!(scene.agents[&id].state, ActivityState::Waiting { .. }),
        "should be Waiting on permission"
    );

    reducer.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: None,
            detail: Some(ToolDetail::from("exec_command")),
        },
        now,
        Transport::Jsonl,
    );
    assert!(
        matches!(scene.agents[&id].state, ActivityState::Active { .. }),
        "resume must return to Active"
    );
}

// --- Codex subagent: parent_id enrichment (JSONL-first race) ---------------
//
// A Codex subagent owns a separate rollout file, so the JSONL watcher renders
// it as a sprite — but keyed flat, with parent_id=None (orphan). The
// SubagentStart hook is the only carrier of the parent link. Because the two
// transports race, the link must apply whichever order they arrive in: a later
// SessionStart{parent_id=Some} must ENRICH an existing orphan, not no-op.

fn codex_session_start(
    r: &mut Reducer,
    scene: &mut SceneState,
    id: AgentId,
    parent: Option<AgentId>,
    transport: Transport,
) {
    r.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".into(),
            session_id: "sid".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: parent,
        },
        SystemTime::now(),
        transport,
    );
}

#[test]
fn session_start_enriches_parent_id_on_existing_orphan() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child = AgentId::from_parts("codex", "child-agent");

    // JSONL creates the orphan subagent first.
    codex_session_start(&mut r, &mut scene, child, None, Transport::Jsonl);
    assert!(
        scene.agents.get(&child).unwrap().parent_id.is_none(),
        "JSONL-created subagent starts orphaned"
    );

    // SubagentStart hook arrives with the parent link → must enrich, not no-op.
    codex_session_start(&mut r, &mut scene, child, Some(parent), Transport::Hook);
    assert_eq!(
        scene.agents.get(&child).unwrap().parent_id,
        Some(parent),
        "existing orphan must be enriched with the parent link"
    );
}

#[test]
fn session_start_does_not_reparent_when_parent_already_set() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let child = AgentId::from_parts("codex", "child");
    let p1 = AgentId::from_parts("codex", "p1");
    let p2 = AgentId::from_parts("codex", "p2");

    codex_session_start(&mut r, &mut scene, child, Some(p1), Transport::Hook);
    codex_session_start(&mut r, &mut scene, child, Some(p2), Transport::Hook);
    assert_eq!(
        scene.agents.get(&child).unwrap().parent_id,
        Some(p1),
        "an established parent link is never overwritten"
    );
}

#[test]
fn codex_subagent_cascades_with_parent_on_session_end() {
    // The payoff: once enriched, a Codex subagent rides the existing scope
    // cascade — ending the parent takes the subagent with it.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child = AgentId::from_parts("codex", "child-agent");
    let now = SystemTime::now();

    codex_session_start(&mut r, &mut scene, parent, None, Transport::Hook);
    codex_session_start(&mut r, &mut scene, child, Some(parent), Transport::Hook);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: parent },
        now,
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent should be exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "subagent should cascade out with its parent"
    );
}
