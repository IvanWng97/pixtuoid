use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::{
    Reducer, ACTIVE_GRACE_WINDOW, B1_CASCADE_GRACE, HOOK_WINS_WINDOW,
};
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;
use serde_json::json;

use crate::{delegating_pair, start};

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
            tool_use_id: Some("t-1".into()),
            detail: Some("FROM_JSONL".into()),
        },
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
    // The suppressed subagent End must be DROPPED, not merely state-neutral:
    // an un-suppressed ActivityEnd arms pending_idle (the grace debounce keeps
    // the slot Active, so the state check above can't see the leak). A None
    // pending_idle is what proves the End never reached apply — this is the
    // assertion that kills the `delete ActivityEnd arm in suppress_subagent_leak`
    // mutant the state check alone leaves alive.
    assert!(
        slot.pending_idle_at.is_none(),
        "a suppressed subagent End must not arm the parent's pending-idle"
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
            tool_use_id: Some("t-1".into()),
            detail: Some("hook-side".into()),
        },
        t0,
        Transport::Hook,
    );

    // Same tool_use_id but 600ms later — OUTSIDE HOOK_WINS_WINDOW (500ms), so
    // this JSONL event is NOT deduped and must be applied. Distinct `detail`
    // is the discriminator: a vacuous `Active { .. }` check passes even if the
    // event were wrongly suppressed (the hook already made it Active), so
    // assert the slot reflects the JSONL event's detail specifically.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-1".into()),
            detail: Some("jsonl-side".into()),
        },
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

#[test]
fn subagent_is_removed_promptly_when_its_parent_task_completes() {
    // b1 (Task-drain completion inference): CC writes no "subagent finished"
    // marker, so we infer completion — when the parent's LAST Task drains, the
    // delegated subtree returned, and its subagents must leave promptly (marked
    // exiting) instead of lingering as zombies to the 30-min idle stale-sweep.
    // The parent keeps running. "Promptly" = within B1_CASCADE_GRACE, NOT
    // immediately (#151): the cascade defers one grace so a suppressed
    // parallel dispatch's JSONL copy can cancel it.
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
    // #222 at the reducer layer: mid-attach to a delegating parent whose
    // > 1 MiB backlog was skipped — the watcher tail-scans and re-emits the
    // in-flight dispatch as a Jsonl-tagged Task ActivityStart. The mid-attach
    // dedup pin (#150): NO hook record for that tuid exists (its PreToolUse
    // predates the listener), so the synthesized start passes the hook-wins
    // dedup and seeds active_tasks — suppression and b1 then work from the
    // Jsonl copy alone, with zero reducer changes.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/att.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/att/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // Both registrations arrive via Jsonl — the watcher's attach replay
    // (emit_first_sight for the parent, the fresh subagent transcript).
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

    // The synthesized Task start (Jsonl, no prior hook record at attach).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("tu_task".into()),
            detail: Some("Agent".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    assert_delegating(
        &scene,
        parent,
        "the synthesized Jsonl Task start must seed active_tasks — no hook record exists at mid-attach to dedup-eat it",
    );

    // The subagent's next tool fires a hook misattributed to the PARENT
    // (CC hook transcript_path is always the parent's). With active_tasks
    // seeded, it must be suppressed — parent stays Delegating.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("sub-R".into()),
            detail: Some("Read: /foo".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    assert_delegating(
        &scene,
        parent,
        "the misattributed subagent hook must be suppressed, not animated on the parent",
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("sub-R".into()),
        },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().pending_idle_at.is_none(),
        "the suppressed subagent End must not arm the parent's pending-idle"
    );

    // The Task's JSONL self-END (the tool_result line in the parent
    // transcript) drains the seeded task and arms the grace-deferred b1
    // cascade — the completed subagent leaves promptly instead of lingering
    // to the 30-min idle sweep.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("tu_task".into()),
        },
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
    // #151A: a parallel SECOND Task dispatch arrives via hook while the
    // first is in flight → suppressed as a leak, tracked ONLY via its JSONL
    // copy. If the FIRST Task's END drains the set while that copy is still
    // in watcher latency, an immediate b1 cascade evicts the second Task's
    // LIVE subtree — unrecoverable: exiting_at has no clearer, and after the
    // 4.5s GC the child's JSONL events no-op forever. The b1 cascade must
    // therefore be grace-deferred, and a Task insert inside the grace must
    // cancel it.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-late", t0);
    let t1 = t0 + Duration::from_secs(1);

    // First Task dispatch — applies normally, active_tasks = {task-1}.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
            detail: Some("Task".into()),
        },
        t1,
        Transport::Hook,
    );
    // Parallel SECOND dispatch via hook — suppressed (leg 1: not recorded).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
            detail: Some("Task".into()),
        },
        t1 + Duration::from_millis(50),
        Transport::Hook,
    );
    // First Task's END drains the set BEFORE the second dispatch's JSONL
    // copy has been delivered (watcher latency) — the #151A race.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
        },
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the drain must not cascade-exit the subtree immediately — the suppressed second dispatch's JSONL copy may still be in watcher latency"
    );

    // The JSONL copy lands inside the grace → tracks task-2 + cancels the
    // pending cascade.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
            detail: Some("Task".into()),
        },
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

    // Teeth: the second Task's drain re-arms, and with nothing arriving the
    // cascade fires after the grace.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
        },
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
    // Re-arm semantics pin (#151): the drain-to-empty `insert` must
    // OVERWRITE a previously armed timestamp, so the grace is always
    // relative to the LATEST drain. Preserve-first semantics (or_insert)
    // would let a drain → insert → drain-again chain fire only
    // (grace − inter-drain gap) after the last drain, re-opening a narrowed
    // #151A window for a third suppressed dispatch. No tick runs between
    // the two drains — a consumed-then-reinserted entry would mask the
    // distinction.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-rearm", t0);
    let t1 = t0 + Duration::from_secs(1);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
            detail: Some("Task".into()),
        },
        t1,
        Transport::Hook,
    );
    // First drain arms the pending cascade.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-1".into()),
        },
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    // A second Task lands and drains again, all inside the first grace.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
            detail: Some("Task".into()),
        },
        t1 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
        },
        t1 + Duration::from_secs(2),
        Transport::Jsonl,
    );

    // At first-drain + grace the clock must have RESTARTED from the second
    // drain — nothing fires yet.
    r.tick(
        &mut scene,
        t1 + Duration::from_millis(200) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the second drain must restart the grace clock — firing on the FIRST drain's timestamp re-opens the #151A window"
    );
    // And it fires one full grace after the second drain.
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
    // #152: the gate recorded while Delegating holds the Task's own tuid.
    // The Task self-END drains via the tracking path (which deliberately
    // skips the main arm — the parent's Waiting must survive the drain), so
    // the gate entry went STALE. A late JSONL replay of that same END
    // (outside HOOK_WINS_WINDOW, so it passes dedup; set already empty, so
    // it falls into the main arm) would match the stale gate via
    // resolves_wait and falsely flip a still-pending permission Waiting to
    // Idle. The drain must clear its own tuid's gate entry.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-152", t0);
    let t1 = t0 + Duration::from_secs(1);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t1,
        Transport::Hook,
    );
    // The parent's own permission prompt fires while Delegating → the gate
    // records the Task's tuid.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission".into(),
        },
        t1 + Duration::from_millis(100),
        Transport::Hook,
    );
    // Task self-END drains via tracking; the parent must stay Waiting
    // (pinned elsewhere) and the gate's tuid is now history.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t1 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Late JSONL replay of the same END, outside the dedup window.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t1 + Duration::from_secs(1) + HOOK_WINS_WINDOW + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Derived past the point a false resolve would become visible: the
    // replay would arm the idle debounce, flipping Waiting → Idle
    // ACTIVE_GRACE_WINDOW later — tick just past that.
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
    // Companion to the stale-gate clear (#152): the clear must be
    // CONDITIONAL on the drained tuid. While delegating, the parent's own
    // ordinary tool (applied via JSONL — suppression is hook-only) can be
    // the gated tool when Waiting fires. The Task drain must NOT wipe that
    // gate, or the ordinary tool's END could never resolve the Waiting.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-keep", t0);
    let t1 = t0 + Duration::from_secs(1);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t1,
        Transport::Hook,
    );
    // Parent's own ordinary tool via JSONL while delegating.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("bash-1".into()),
            detail: Some("Bash: ls".into()),
        },
        t1 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Permission prompt fires mid-bash → gate records bash-1.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission".into(),
        },
        t1 + Duration::from_millis(200),
        Transport::Hook,
    );
    // The Task drains — must not touch bash-1's gate.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t1 + Duration::from_secs(1),
        Transport::Hook,
    );
    // bash-1's END resolves the Waiting through the kept gate.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("bash-1".into()),
        },
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
fn late_batched_jsonl_pair_after_delivered_hook_end_is_fully_dropped() {
    // Asymmetric-matrix pin (#150): a delivered hook END's record suppresses
    // BOTH JSONL kinds for its tuid. When JSONL delivery lags a fast tool
    // (notify coalescing / the 250ms rescan) the transcript's START+END pair
    // can arrive together AFTER both hooks applied — the stale JSONL START
    // must not re-enter Active (it would cancel the armed pending-idle via
    // enter_active and double-count the tool), and the JSONL END must not
    // re-arm anything. Green before AND after the #150 fix: a symmetric
    // kind-matching dedup (START record only drops STARTs, END record only
    // drops ENDs) breaks exactly this — mutation-validated.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/batched.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // Fast tool: both hooks deliver.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-fast".into()),
            detail: Some("Read: /x".into()),
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t-fast".into()),
        },
        t0 + HOOK_WINS_WINDOW / 10,
        Transport::Hook,
    );
    let armed_at = scene.agents.get(&id).unwrap().pending_idle_at;
    assert!(armed_at.is_some(), "hook END arms the idle debounce");
    let count = scene.agents.get(&id).unwrap().tool_call_count;

    // The lagged JSONL pair lands together, PAST the START record's expiry
    // (t0 + W) but inside the END record's window (until t0 + W/10 + W).
    // This is the leg that distinguishes kind-in-the-VALUE from a
    // kind-in-the-key map: a per-kind keyed entry for the START is gc'd by
    // now, so only the END record's both-kinds dominance can drop the stale
    // START. Mutation-validated against BOTH rejected shapes (symmetric
    // value matching AND a (id, tuid, kind) key).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-fast".into()),
            detail: Some("Read: /x".into()),
        },
        t0 + HOOK_WINS_WINDOW + HOOK_WINS_WINDOW / 20,
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t-fast".into()),
        },
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
    // Ordering pin for `apply()`'s pre-passes (#90), leg (2): a JSONL
    // duplicate the dedup drops must be dropped BEFORE it can reach
    // track_active_tasks. The parent's own permission prompt fires right
    // after a Task dispatch → Waiting. The dispatch's JSONL copy (parent's
    // own transcript, same tool_use_id, inside HOOK_WINS_WINDOW of the hook
    // record) is a duplicate — if it reached the tracker first, the Task arm
    // would re-fire enter_delegating and clobber the Waiting back to
    // Active(Delegating), vanishing a genuinely pending prompt.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-wait", t0);

    // Hook Task dispatch — records the Start in the dedup map + active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // The parent's own permission Notification fires mid-dispatch.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission".into(),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 50,
        Transport::Hook,
    );
    // The dispatch's JSONL copy, inside the window — must be dedup-dropped
    // before it can re-enter Delegating over the live Waiting.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
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

// Twin of the in-window pin above, OUTSIDE the dedup window: the hook-wins
// record is GC'd at HOOK_WINS_WINDOW, but real JSONL skew runs to ~2.5s (the
// B1_CASCADE_GRACE sizing models the FSEvents coalescing tail), so the
// dispatch's JSONL copy can land with NO dedup record left. The tracker's
// first-insert gate is then the only guard: a duplicate insert
// (`insert() == false`) must not re-fire enter_delegating over the parent's
// genuinely pending permission Waiting.
#[test]
fn jsonl_task_start_replay_outside_dedup_window_does_not_clobber_waiting_parent() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-wait-late", t0);

    // Hook Task dispatch — records the Start in the dedup map + active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // The parent's own permission Notification fires mid-dispatch.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission".into(),
        },
        t0 + Duration::from_secs(1) + HOOK_WINS_WINDOW / 50,
        Transport::Hook,
    );
    // The dispatch's JSONL copy lands at 2× the dedup window — the record is
    // GC'd, so it reaches the tracker. Its insert is a duplicate (the tuid is
    // still in flight), which must NOT re-enter Delegating.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
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

// Third leg of the pin family: the pair-replay AFTER the drain. A fast Task
// (hook Start + hook End both delivered) drains `active_tasks`, and the End's
// dedup record is GC'd at HOOK_WINS_WINDOW — but the transcript's batched
// Start+End pair replays at real skew (~2.5s, the B1_CASCADE_GRACE model;
// up to 60s under the missed-notify poll backstop). With the set drained the
// replayed Start is a FRESH first insert, so the first-insert gate alone
// can't hold: the drained-tuid tombstone must keep enter_delegating from
// re-firing over a Waiting raised in the gap, and the replayed End must not
// settle the still-pending prompt to Idle.
#[test]
fn lagged_jsonl_task_pair_after_drain_does_not_clobber_waiting_parent() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, _child) = delegating_pair(&mut r, &mut scene, "orch-drained", t0);

    // Fast Task: both hooks deliver — the Start tracks, the End DRAINS.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    // The parent's own permission prompt fires in the gap → Waiting.
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: parent,
            reason: "permission".into(),
        },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );

    // The batched JSONL pair lands at real skew — far outside
    // HOOK_WINS_WINDOW, so no dedup record is left and the set is empty.
    let replay_at = t0 + Duration::from_secs(2) + B1_CASCADE_GRACE + Duration::from_millis(100);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
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

    // ...and the replayed End must not arm an idle-resolve on the prompt.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
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
    // #150, ordinary-tool leg: a sub-window tool whose PostToolUse hook
    // drops must still settle via its JSONL END — the only completion signal
    // left. Without this the slot is stuck Active until the agent's next
    // event, or is wrongfully stale-swept after STALE_ACTIVE_TIMEOUT (and a
    // swept CC slot cannot resurrect on the next prompt).
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/fastdrop.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-1".into()),
            detail: Some("Read: /x".into()),
        },
        t0,
        Transport::Hook,
    );
    // PostToolUse hook DROPS. The JSONL END inside HOOK_WINS_WINDOW of the
    // hook START's record must apply and arm the idle debounce.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t-1".into()),
        },
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

    // Tick PAST the b1 grace before asserting (#151): under the
    // record-before-suppress mutant the JSONL copy is dedup-dropped, the
    // drain empties the set and arms the pending cascade — which only an
    // expired grace makes observable. Asserting at the drain instant would
    // pass under the mutant and lose this pin's teeth.
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

    // Teeth: draining the SECOND Task must fire the cascade after the grace
    // — proves the child is wired to the parent and the earlier no-cascade
    // assertion wasn't vacuous.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-2".into()),
        },
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
    // #150: the hook shim is best-effort (200ms write timeout, exit-0) — a
    // Task's PostToolUse hook can drop. JSONL is the documented fallback for
    // dropped hooks, so the JSONL tool_result END for the Task MUST drain
    // active_tasks even when it lands inside HOOK_WINS_WINDOW of the hook
    // ActivityStart's dedup record. Without this, active_tasks[parent] leaks
    // for the rest of the session: every later parent hook event is
    // suppressed as a subagent leak and the b1 completion cascade never
    // fires again.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "orch-drop", t0);

    // Hook Task dispatch — records "task-T" in the dedup map + active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // The Task fails fast; its PostToolUse hook DROPS (never applied). The
    // JSONL END arrives inside HOOK_WINS_WINDOW of the hook START's record —
    // it is the only completion signal the reducer will ever get.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
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
    // The parent must not be stuck Delegating: a later ordinary hook event
    // applies normally instead of being suppressed as a subagent leak.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("b-1".into()),
            detail: Some("Bash: ls".into()),
        },
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

// --- CodeWhale subagent nesting, anchored on a REAL captured transcript -----

#[test]
fn real_codewhale_subagent_payload_nests_the_child_under_its_workspace_parent() {
    // Anchored on an ACTUAL CodeWhale subagent run (#276): the user's
    // ~/dotfiles/.deepseek/state/subagents.v1.json recorded child id
    // `agent_ad945f4c` (an `explore` subagent) with workspace
    // `/Users/navepnow/dotfiles`. The hook payload shape here is verbatim from
    // upstream `crates/tui/src/tui/ui.rs::execute_subagent_observer_hook`
    // (event / agent_id / session_id / workspace / mode / model / total_tokens
    // + a `<text>_preview` + `<text>_truncated` pair) — the decoder reads only
    // event/agent_id/workspace, so the extra real-world fields must pass
    // through inertly (this is the regression the minimal earlier tests miss).
    //
    // The byte-match this pins (the R0613-08 worry): the PARENT registers via
    // an env-mode `session_start` keyed on `cwd`; the CHILD's parent link is
    // `workspace`-keyed. Both strings originate in the same CodeWhale
    // `App.workspace`, so when they're equal the link MUST resolve to the
    // parent's own AgentId and the child MUST be a distinct (un-coalesced)
    // sprite nested under it. Decode runs through the public
    // `decode_hook_payload` (the socket listener's own entry), routed by the
    // shim-stamped `_pixtuoid_source`.
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

    // 1. Parent session_start — the shim's env-mode envelope (cwd resolved from
    //    DEEPSEEK_WORKSPACE, source stamped). Keys the parent on its cwd.
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

    // 2. Child subagent_spawn — full upstream payload, REAL values, forwarded
    //    RAW from CodeWhale stdin (plain hook entry, no --event).
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

    // 3. subagent_complete ends the child AS A CHILD (mark_exiting + scope
    //    cascade), NOT a top-level session end, and must not touch the parent.
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
