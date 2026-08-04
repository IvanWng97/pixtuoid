use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, ToolDetail, Transport};
use pixtuoid_core::state::reducer::{
    Reducer, ACTIVE_GRACE_WINDOW, B1_CASCADE_GRACE, HOOK_WINS_WINDOW,
};
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;
use serde_json::json;

use crate::{act_end, act_start, delegating_pair, start, waiting};

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
fn jsonl_duplicate_of_recent_hook_is_dropped() {
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        None,
        t0,
        Transport::Hook,
    );

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        Some("FROM_JSONL"),
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    match &slot.state {
        ActivityState::Active { detail, .. } => {
            assert_ne!(
                detail.as_deref(),
                Some("FROM_JSONL"),
                "jsonl detail must not overwrite"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn hook_activity_during_active_task_is_suppressed() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    start(&mut r, &mut scene, parent);

    let t0 = SystemTime::now();

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0,
        Transport::Hook,
    );

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("subagent-R"),
        Some("Read: /foo"),
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );

    let slot = scene.agents.get(&parent).unwrap();
    match &slot.state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Delegating"));
        }
        other => panic!("expected Active(Delegating), got {other:?}"),
    }

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("subagent-R"),
        t0 + Duration::from_millis(60),
        Transport::Hook,
    );
    let slot = scene.agents.get(&parent).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "parent must remain Active(Task) while task in flight"
    );
    assert!(
        slot.pending_idle_at.is_none(),
        "a suppressed subagent End must not arm the parent's pending-idle"
    );

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
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

#[test]
fn subagent_jsonl_activity_is_unaffected_by_parent_task_suppression() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let subagent = AgentId::from_transcript_path("/p/parent/subagents/agent-x.jsonl");
    start(&mut r, &mut scene, parent);
    start(&mut r, &mut scene, subagent);

    let t0 = SystemTime::now();
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        subagent,
        Some("sub-R"),
        Some("Read: /bar"),
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

#[test]
fn hook_activity_without_active_task_applies_normally() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t"),
        Some("Bash: ls"),
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
fn active_tasks_drained_by_jsonl_end_even_if_hook_end_arrived_first() {
    use pixtuoid_core::source::ToolDetail;

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    act_end(&mut r, &mut scene, id, Some("task-X"), t0, Transport::Hook);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("task-X".into()),
            detail: Some(ToolDetail::Task),
        },
        t0 + Duration::from_millis(700),
        Transport::Jsonl,
    );

    act_end(
        &mut r,
        &mut scene,
        id,
        Some("task-X"),
        t0 + Duration::from_millis(800),
        Transport::Jsonl,
    );

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("other"),
        Some("Bash: ls"),
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
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        Some("hook-side"),
        t0,
        Transport::Hook,
    );

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        Some("jsonl-side"),
        t0 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    match &slot.state {
        ActivityState::Active { detail, .. } => assert_eq!(
            detail.as_deref(),
            Some("jsonl-side"),
            "JSONL event outside the dedup window must be applied"
        ),
        other => panic!("expected Active, got {other:?}"),
    }
}

#[test]
fn hook_wins_dedup_drops_jsonl_duplicate_within_window() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/dedup-hw.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("dedup-1"),
        Some("Edit: hook.rs"),
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("dedup-1"),
        Some("Edit: jsonl.rs"),
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    assert_eq!(
        scene.agents.get(&id).unwrap().tool_call_count,
        1,
        "JSONL duplicate inside hook-wins window must be dropped"
    );
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Edit: hook.rs"));
        }
        other => panic!("expected Active from hook, got {other:?}"),
    }
}

#[test]
fn subagent_is_removed_promptly_when_its_parent_task_completes() {
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
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        child,
        Some("c1"),
        Some("Read: /x"),
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the b1 cascade is grace-deferred (#151) — never immediate"
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(10) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "a completed subagent must leave promptly (within the b1 grace) when its parent's Task drains, not linger to the 30-min idle sweep"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the parent keeps running after a Task completes"
    );
}

#[test]
fn oversized_attach_synthesized_task_start_restores_suppression_and_b1() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/att.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/att/subagents/agent-1.jsonl");
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
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("tu_task"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    assert_delegating(
        &scene,
        parent,
        "the synthesized Jsonl Task start must seed active_tasks — no hook record exists at mid-attach to dedup-eat it",
    );

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("sub-R"),
        Some("Read: /foo"),
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    assert_delegating(
        &scene,
        parent,
        "the misattributed subagent hook must be suppressed, not animated on the parent",
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("sub-R"),
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().pending_idle_at.is_none(),
        "the suppressed subagent End must not arm the parent's pending-idle"
    );

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("tu_task"),
        t0 + Duration::from_secs(10),
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the b1 cascade is grace-deferred (#151) — never immediate"
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(10) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the synthesized Task start must arm b1: the drain cascades the completed subagent out"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the parent keeps running after the Task drains"
    );
}

#[test]
fn late_jsonl_dispatch_copy_inside_grace_cancels_premature_cascade() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-late", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        Some("Agent"),
        t1 + Duration::from_millis(50),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the drain must not cascade-exit the subtree immediately — the suppressed second dispatch's JSONL copy may still be in watcher latency"
    );

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        Some("Agent"),
        t1 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_millis(200) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the JSONL copy's Task insert must cancel the pending cascade — the subtree is still working"
    );
    assert_delegating(&scene, parent, "parent stays Delegating on the second Task");

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        t1 + Duration::from_secs(5),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_secs(5) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the last Task's drain must still cascade-exit the completed subtree after the grace"
    );
}

#[test]
fn second_drain_inside_grace_restarts_the_cascade_clock() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-rearm", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        Some("Agent"),
        t1 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        t1 + Duration::from_secs(2),
        Transport::Jsonl,
    );

    r.tick(
        &mut scene,
        t1 + Duration::from_millis(200) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the second drain must restart the grace clock — firing on the FIRST drain's timestamp re-opens the #151A window"
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_secs(2) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the cascade still fires one grace after the last drain"
    );
}

#[test]
fn late_jsonl_replay_of_drained_task_end_does_not_false_resolve_waiting() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-152", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t1 + Duration::from_millis(100),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t1 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t1 + Duration::from_secs(1) + HOOK_WINS_WINDOW + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_secs(1)
            + HOOK_WINS_WINDOW
            + Duration::from_millis(100)
            + ACTIVE_GRACE_WINDOW
            + Duration::from_millis(10),
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a late JSONL replay of the drained Task END must not match the stale gate and false-resolve a still-pending permission Waiting"
    );
}

#[test]
fn task_drain_keeps_parallel_ordinary_tool_gate() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-keep", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        Some("Bash: ls"),
        t1 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t1 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        t1 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_secs(2) + ACTIVE_GRACE_WINDOW + Duration::from_millis(10),
    );
    assert_eq!(
        scene.agents.get(&parent).unwrap().state,
        ActivityState::Idle,
        "the kept gate must let the ordinary tool's END resolve the Waiting"
    );
}

#[test]
fn suppressed_child_event_keeps_parents_own_parallel_tool_gate() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-own-gate", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        Some("Bash: ls"),
        t1 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("sub-R"),
        Some("Read: /foo"),
        t1 + Duration::from_millis(300),
        Transport::Hook,
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a suppressed child event must not hide the parent's own still-pending permission Waiting"
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        t1 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_secs(1) + ACTIVE_GRACE_WINDOW + Duration::from_millis(10),
    );
    assert!(
        !matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "the kept gate must let the own tool's END resolve the Waiting"
    );
}

#[test]
fn own_parallel_tool_end_mid_delegation_returns_parent_to_delegating() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-own-end", t0);
    let t1 = t0 + Duration::from_secs(1);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t1,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        Some("Bash: ls"),
        t1 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("bash-1"),
        t1 + Duration::from_millis(500),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t1 + Duration::from_millis(500) + ACTIVE_GRACE_WINDOW + Duration::from_millis(10),
    );
    assert_delegating(
        &scene,
        parent,
        "parent must stay Delegating while its Task is still in flight — not settle to Idle",
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the own tool's END must not have cascaded the live subtree"
    );
}

#[test]
fn late_batched_jsonl_pair_after_delivered_hook_end_is_fully_dropped() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/batched.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-fast"),
        Some("Read: /x"),
        t0,
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t-fast"),
        t0 + HOOK_WINS_WINDOW / 10,
        Transport::Hook,
    );
    let armed_at = scene.agents.get(&id).unwrap().pending_idle_at;
    assert!(armed_at.is_some(), "hook END arms the idle debounce");
    let count = scene.agents.get(&id).unwrap().tool_call_count;

    // These offsets put the lagged pair PAST the START record's expiry (t0 + W) but
    // still inside the END record's window, so only the END record's both-kinds
    // dominance can drop the stale START.
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-fast"),
        Some("Read: /x"),
        t0 + HOOK_WINS_WINDOW + HOOK_WINS_WINDOW / 20,
        Transport::Jsonl,
    );
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t-fast"),
        t0 + HOOK_WINS_WINDOW + HOOK_WINS_WINDOW / 20,
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(
        slot.pending_idle_at, armed_at,
        "stale JSONL replay must not cancel or re-arm the idle debounce"
    );
    assert_eq!(
        slot.tool_call_count, count,
        "stale JSONL replay must not double-count the tool"
    );
}

#[test]
fn jsonl_task_start_duplicate_does_not_clobber_waiting_parent() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-wait", t0);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 50,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );

    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a dedup-dropped JSONL duplicate of the dispatch must not clobber the parent's pending permission Waiting"
    );
}

#[test]
fn jsonl_task_start_replay_outside_dedup_window_does_not_clobber_waiting_parent() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-wait-late", t0);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 50,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW * 2,
        Transport::Jsonl,
    );

    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "an out-of-dedup-window JSONL replay of an already-tracked dispatch must not clobber the parent's pending permission Waiting"
    );
}

#[test]
fn lagged_jsonl_task_pair_after_drain_does_not_clobber_waiting_parent() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-drained", t0);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission",
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );

    let replay_at = t0 + Duration::from_secs(2) + B1_CASCADE_GRACE + Duration::from_millis(100);
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        replay_at,
        Transport::Jsonl,
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a replayed Start of an already-drained Task must not re-enter Delegating over a pending Waiting"
    );

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        replay_at,
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        replay_at + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "the replayed pair must leave the still-pending permission Waiting, got {:?}",
        scene.agents.get(&parent).unwrap().state
    );
}

#[test]
fn jsonl_ordinary_tool_end_drains_when_hook_end_drops() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/fastdrop.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        Some("Read: /x"),
        t0,
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t-1"),
        t0 + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&id).unwrap().pending_idle_at.is_some(),
        "the JSONL END is the fallback for the dropped hook END — it must arm the idle debounce, not be eaten by the START's dedup record"
    );
}

#[test]
fn suppressed_parallel_task_dispatch_jsonl_copy_survives_dedup_and_tracks() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-par", t0);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        Some("Agent"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 10,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        Some("Agent"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 10 + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-1"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW * 2 / 5,
        Transport::Hook,
    );

    r.tick(
        &mut scene,
        t0 + Duration::from_secs(1)
            + HOOK_WINS_WINDOW * 2 / 5
            + B1_CASCADE_GRACE
            + Duration::from_millis(10),
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

    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-2"),
        t0 + Duration::from_secs(5),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(5) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "last Task's drain must cascade-exit the completed subtree"
    );
}

#[test]
fn jsonl_task_self_end_drains_when_hook_end_drops() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-drop", t0);

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 5,
        Transport::Jsonl,
    );

    r.tick(
        &mut scene,
        t0 + Duration::from_secs(1)
            + HOOK_WINS_WINDOW / 5
            + B1_CASCADE_GRACE
            + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the JSONL Task self-END must drain active_tasks and fire the b1 cascade (after the #151 grace) — it is the fallback for the dropped hook END"
    );
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("b-1"),
        Some("Bash: ls"),
        t0 + Duration::from_secs(5),
        Transport::Hook,
    );
    match &scene.agents.get(&parent).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(
                detail.as_deref(),
                Some("Bash: ls"),
                "suppression must release once the Task drained via JSONL"
            );
        }
        other => panic!("expected Active(Bash: ls), got {other:?}"),
    }
}

#[test]
fn parent_waiting_on_subagent_permission_resolves_when_the_subagent_resumes() {
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
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission?",
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

    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("sub-bash"),
        Some("Bash: ls"),
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

#[test]
fn parent_waiting_on_subagent_permission_resolves_when_the_subagent_ends_a_tool() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/orch2.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/orch2/subagents/agent-1.jsonl");
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
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission?",
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
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("sub-bash"),
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    assert!(
        matches!(
            scene.agents.get(&parent).unwrap().state,
            ActivityState::Active { .. }
        ),
        "a suppressed child END must ALSO restore Active(Delegating), not leave a stale Waiting"
    );
}

#[test]
fn task_drain_while_parent_waiting_keeps_waiting() {
    use pixtuoid_core::source::ToolDetail;
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
            tool_use_id: Some("task-T".into()),
            detail: Some(ToolDetail::Task),
        },
        t0,
        Transport::Hook,
    );
    waiting(
        &mut r,
        &mut scene,
        id,
        "permission",
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    assert!(matches!(
        scene.agents[&id].state,
        ActivityState::Waiting { .. }
    ));

    act_end(
        &mut r,
        &mut scene,
        id,
        Some("task-T"),
        t0 + Duration::from_millis(1000),
        Transport::Hook,
    );
    assert!(
        scene.agents[&id].pending_idle_at.is_none(),
        "Task drain must not arm idle-resolve on a Waiting parent"
    );

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
fn real_codewhale_subagent_payload_nests_the_child_under_its_workspace_parent() {
    fn feed(r: &mut Reducer, scene: &mut SceneState, v: serde_json::Value, t: SystemTime) {
        for ev in decode_hook_payload(v).expect("real CodeWhale payload must decode") {
            r.apply(scene, ev, t, Transport::Hook);
        }
    }

    const WS: &str = "/Users/navepnow/dotfiles";
    let parent = AgentId::from_parts("codewhale", WS);
    let child = AgentId::from_parts("codewhale", "agent_ad945f4c");
    assert_ne!(
        parent, child,
        "child keys on agent_id, parent on cwd — structurally distinct sprites"
    );

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    feed(
        &mut r,
        &mut scene,
        json!({ "event": "session_start", "cwd": WS, "_pixtuoid_source": "codewhale" }),
        t0,
    );
    assert!(
        scene.agents.contains_key(&parent),
        "parent registers on its cwd-keyed sprite"
    );

    feed(
        &mut r,
        &mut scene,
        json!({
            "event": "subagent_spawn",
            "agent_id": "agent_ad945f4c",
            "session_id": "sess_1a2b3c4d",
            "workspace": WS,
            "mode": "Yolo",
            "model": "deepseek-v4-pro",
            "total_tokens": 4096,
            "prompt_preview": "Search the web for how people organize their dotfiles.",
            "prompt_truncated": true,
            "_pixtuoid_source": "codewhale"
        }),
        t0 + Duration::from_secs(1),
    );
    let slot = scene
        .agents
        .get(&child)
        .expect("the subagent registers as its OWN sprite, distinct from the workspace parent");
    assert_eq!(
        slot.parent_id,
        Some(parent),
        "the workspace-keyed parent link must resolve to the parent's own cwd-keyed AgentId — the byte-match holds for the real captured workspace string"
    );

    feed(
        &mut r,
        &mut scene,
        json!({
            "event": "subagent_complete",
            "agent_id": "agent_ad945f4c",
            "session_id": "sess_1a2b3c4d",
            "workspace": WS,
            "status": "completed",
            "result_preview": "I cannot proceed with this task because the required tools are not available.",
            "result_truncated": true,
            "_pixtuoid_source": "codewhale"
        }),
        t0 + Duration::from_secs(11),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "subagent_complete must mark the child exiting (a child end), not leave it live"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "ending a subagent must never exit its parent"
    );
}

#[test]
fn desk_exhausted_task_dispatch_leaves_no_ghost_slot() {
    let mut scene = SceneState::new([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut r = Reducer::new();
    let seated = AgentId::from_transcript_path("/proj/seated.jsonl");
    start(&mut r, &mut scene, seated);
    assert_eq!(scene.agents.len(), 1, "the single desk is occupied");

    let t0 = SystemTime::now();
    let orphan = AgentId::from_transcript_path("/proj/orphan.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: orphan,
            tool_use_id: Some("t1".into()),
            detail: Some(ToolDetail::Task),
        },
        t0,
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&orphan),
        "the desk-starved dispatch mints no ghost slot"
    );
    assert_eq!(
        scene.agents.len(),
        1,
        "the orphan never registered a session"
    );

    r.tick(&mut scene, t0 + Duration::from_secs(2));
    assert!(
        !scene.agents.contains_key(&orphan),
        "tick introduced no ghost slot for the orphan"
    );
    assert_eq!(scene.agents.len(), 1, "the seated agent is untouched");
}

/// The `handled_by_task_tracking` guard's REMOVAL was invisible to all 36 test
/// binaries: every existing test drains the LAST task, where the drain path and
/// the general arm happen to agree. With parallel Tasks in flight they diverge —
/// without the guard the general arm re-runs `enter_delegating`, resetting
/// `state_started_at` on a parent that never left Delegating.
///
/// Asserting the resulting STATE alone would reproduce the gap (it is Delegating
/// either way); the clock is what pins the guard.
#[test]
fn ending_one_of_two_parallel_tasks_does_not_restart_the_parents_delegating_clock() {
    let mut r = Reducer::new();
    let mut scene = SceneState::uniform(4);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let parent = AgentId::from_transcript_path("/p/parallel.jsonl");
    start(&mut r, &mut scene, parent);

    // TWO Task dispatches in flight — the case no existing test covers.
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-a"),
        Some("Agent"),
        t0,
        Transport::Hook,
    );
    // The SECOND dispatch must ride JSONL: on the hook transport a parallel
    // Task is suppressed as a subagent leak, so a hook copy would never reach
    // `active_tasks` and the parallel state under test would not exist.
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-b"),
        Some("Agent"),
        t0,
        Transport::Jsonl,
    );
    let armed_at = scene.agents[&parent].state_started_at;

    // Drain ONE. The other is still live, so the parent stays Delegating.
    act_end(
        &mut r,
        &mut scene,
        parent,
        Some("task-a"),
        t0 + Duration::from_secs(30),
        Transport::Hook,
    );

    let slot = &scene.agents[&parent];
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "one of two parallel Tasks ending must leave the parent Delegating"
    );
    assert_eq!(
        slot.state_started_at, armed_at,
        "the drain already applied this event; the general arm must be SKIPPED, \
         or it re-enters Delegating and restarts the clock mid-delegation"
    );
}
