use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::{Reducer, B1_CASCADE_GRACE};
use pixtuoid_core::state::{ActivityState, GlobalDeskIndex, SceneState};
use pixtuoid_core::AgentId;

use crate::start;

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
    assert_eq!(slot.desk_index, GlobalDeskIndex(0));
    assert_eq!(
        &*slot.label, "cc·repo",
        "label = source prefix + cwd basename"
    );
    assert_eq!(slot.state, ActivityState::Idle);
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
        AgentEvent::SessionEnd {
            agent_id: a,
            as_child: false,
        },
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
    assert_eq!(scene.next_free_desk(), Some(GlobalDeskIndex(0)));
}

/// With heterogeneous per-floor capacities, the third session should
/// overflow from floor 0 (cap=2) to floor 1's first desk (global index 2).
#[test]
fn session_start_overflows_to_floor1_with_heterogeneous_capacity() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
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
    let desks: Vec<usize> = scene.agents.values().map(|a| a.desk_index.0).collect();
    assert!(desks.contains(&0));
    assert!(desks.contains(&1));
    assert!(
        desks.contains(&2),
        "third agent should get desk 2 (floor 1)"
    );
    assert_eq!(scene.floor_of(GlobalDeskIndex(2)), 1);
}

#[test]
fn session_start_dropped_when_all_desks_occupied() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
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

// Reasonix `/new` fires SessionEnd + SessionStart back-to-back on the SAME
// cwd-keyed AgentId. The SessionStart must resurrect the exiting slot in place
// — otherwise it is swallowed by the exists-branch, the corpse is GC'd at
// 4.5s, and the new session's entire first turn renders nothing.
#[test]
fn session_start_on_exiting_slot_resurrects_in_place() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_some());

    // The rotation's SessionStart lands ms later (same cwd → same id).
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: "/Users/dev/proj".into(),
            parent_id: None,
        },
        t0 + Duration::from_millis(20),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "SessionStart on an exiting slot must cancel the walkout"
    );

    // The new session's first turn works — and survives past the old grace.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: None,
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    r.tick(&mut scene, t0 + EXIT_GRACE_WINDOW + Duration::from_secs(1));
    let slot = scene
        .agents
        .get(&id)
        .expect("slot survives the grace window");
    assert!(matches!(slot.state, ActivityState::Active { .. }));
}

// Resurrect-in-place must FOLD the in-flight Active span into active_ms before
// resetting to Idle — every other Active-exit site does; a direct `state = Idle`
// dropped it, losing the pre-rotation work from the tooltip's "% active" stat.
#[test]
fn resurrect_in_place_folds_the_active_span_into_active_ms() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    // Go Active and hold the span open across the exit (mark_exiting leaves the
    // slot Active; no tick settles it).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&id).unwrap().active_ms,
        0,
        "span not folded yet (mark_exiting doesn't accumulate)"
    );

    // Resurrect 2s later — the open Active span (t0..now) must land in active_ms.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: "/Users/dev/proj".into(),
            parent_id: None,
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(slot.exiting_at.is_none(), "walkout cancelled");
    assert_eq!(
        slot.active_ms, 2000,
        "the 2s Active span must fold into active_ms on resurrect"
    );
}

// The resurrect-in-place arm is gated to ROOT agents on BOTH sides (slot AND
// event parent_id None) so a late duplicate SessionStart can't un-exit a
// b1-cascaded subagent — reducer.rs is the SOLE site that clears exiting_at.
// Pin the negative: a SessionStart on an EXITING subagent leaves it exiting.
#[test]
fn session_start_on_exiting_subagent_does_not_resurrect() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/resurr-parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/resurr-parent/subagents/agent-1.jsonl");
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

    // Parent ends → cascade marks the child exiting.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: parent,
            as_child: false,
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child cascaded to exiting when the parent ended"
    );

    // A duplicate SessionStart for the child (it carries a parent link) must NOT
    // clear exiting_at — only roots resurrect.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "an exiting SUBAGENT must NOT resurrect (the root-only gate)"
    );

    // And it still GCs at the grace window — the reanimation never happened.
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(1) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(
        !scene.agents.contains_key(&child),
        "the cascaded subagent is reaped after its grace window"
    );
}

// Resurrect-in-place must evict the previous life's correlation state: a tuid
// left in active_tasks by a session that ended mid-delegation can NEVER drain
// (its End belongs to the dead life), so suppress_subagent_leak would eat
// every hook ActivityStart/End of the resurrected session for its entire
// life. The fresh session's first hook tool must apply.
#[test]
fn resurrect_in_place_clears_stale_active_tasks_so_fresh_session_hooks_apply() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/resurr-tasks.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "res".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    // The old life dispatches a Task… and dies before it drains.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("task-stale".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    // The next session on the same id lands inside the walkout window.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "res".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0 + Duration::from_millis(2_500),
        Transport::Jsonl,
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "resurrected"
    );

    // The fresh life's first hook tool: with the stale tuid still tracked it
    // would be suppressed as a subagent leak and the slot would stay Idle.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-new".into()),
            detail: Some("Read: /x".into()),
        },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(
                detail.as_deref(),
                Some("Read: /x"),
                "the fresh session's hook tool must apply, not render as Delegating"
            );
        }
        other => panic!(
            "the resurrected session's first hook tool must NOT be suppressed by the dead life's active_tasks residue — got {other:?}"
        ),
    }
}

// Resurrect-in-place must also disarm the previous life's pending b1 cascade:
// armed by the old life's Task drain, it would otherwise fire after
// B1_CASCADE_GRACE and cascade-exit the RESURRECTED session's brand-new
// subagent subtree.
#[test]
fn resurrect_in_place_cancels_stale_pending_b1_cascade() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/resurr-b1.jsonl");
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
    // Old life: a Task dispatch drains → the deferred b1 cascade is armed.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            tool_use_id: Some("task-old".into()),
            detail: Some("Task".into()),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-old".into()),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    // The old life ends and the next session resurrects in place, all inside
    // the armed cascade's grace.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: parent,
            as_child: false,
        },
        t0 + Duration::from_millis(2_200),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0 + Duration::from_millis(2_400),
        Transport::Jsonl,
    );
    // The fresh life dispatches a brand-new subagent.
    let child = AgentId::from_parts("claude-code", "/p/resurr-b1/subagents/agent-1.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(2_600),
        Transport::Jsonl,
    );

    // Past the OLD drain's grace: a stale armed cascade would fire here and
    // evict the new subtree.
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(2) + B1_CASCADE_GRACE + Duration::from_millis(10),
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the dead life's armed b1 cascade must not fire into the resurrected session's fresh subagent"
    );
}

// A duplicate SessionStart (Codex/Reasonix re-emit one per UserPromptSubmit)
// is a genuine liveness signal: a prompt landing just under the stale
// threshold must push the boundary out, not lose the race to the sweep while
// the model is still thinking.
#[test]
fn duplicate_session_start_refreshes_liveness() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    // Prompt arrives just before the idle threshold…
    let near = t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(10);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: "/Users/dev/proj".into(),
            parent_id: None,
        },
        near,
        Transport::Hook,
    );
    // …and the slot must still be alive once the ORIGINAL threshold passes.
    r.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_none()),
        "duplicate SessionStart must refresh last_event_at"
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
        AgentEvent::SessionEnd {
            agent_id: parent,
            as_child: false,
        },
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

// ── Hook events are proof of life ───────────────────────────────────────────
// A hook event can only come from a live process. A hook-transport tool /
// permission event whose AgentId has no slot means a LIVE session is invisible
// (its transcript was gated at first sight — mid-attach, idle >1h — so no
// JSONL SessionStart ever ran). The reducer synthesizes the registration the
// missing SessionStart would have performed; identity context the event
// doesn't carry (source/session_id/cwd) stays empty until a later real
// SessionStart back-fills it.

#[test]
fn hook_activity_start_for_unknown_id_synthesizes_slot_and_goes_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: Some("Edit: foo.rs".into()),
        },
        t0,
        Transport::Hook,
    );

    let slot = scene
        .agents
        .get(&id)
        .expect("hook event must synthesize the slot");
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "the synthesizing event itself applies to the fresh slot"
    );
    // The event carries no source/cwd, so the label is the bare ordinal
    // fallback (empty source prefix) — the shape the SessionStart back-fill
    // recognizes and upgrades.
    assert_eq!(&*slot.label, "#1");
    assert!(slot.cwd.as_os_str().is_empty(), "no cwd on the event");
}

#[test]
fn hook_waiting_for_unknown_id_synthesizes_slot_in_waiting_state() {
    // The motivating case: a mid-attached session parked on a permission
    // prompt fires ONLY a Notification hook (no transcript append) — without
    // synthesis it has no revival path at all.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "parked-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0,
        Transport::Hook,
    );

    let slot = scene
        .agents
        .get(&id)
        .expect("hook Waiting must synthesize the slot");
    assert!(
        matches!(slot.state, ActivityState::Waiting { .. }),
        "slot enters Waiting so the permission prompt is visible"
    );
}

#[test]
fn jsonl_event_for_unknown_id_stays_a_no_op() {
    // JSONL lines can be historical replays (the watcher's first-sight gate
    // exists precisely for those) — only a hook proves a live process, so the
    // JSONL unknown-id no-op stays load-bearing.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "replayed-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "perm".into(),
        },
        t0,
        Transport::Jsonl,
    );

    assert!(
        scene.agents.is_empty(),
        "JSONL events for an unknown id must not synthesize a slot"
    );
}

#[test]
fn hook_session_end_for_unknown_id_does_not_create_slot() {
    // An end for an unknown agent proves nothing worth showing — there is
    // nothing to remove and synthesizing a corpse would flash a phantom.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "already-gone");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );

    assert!(scene.agents.is_empty(), "SessionEnd must not synthesize");
}

#[test]
fn jsonl_session_start_after_hook_synthesis_coalesces_into_same_slot() {
    // The revived transcript's SessionStart (same session UUID → same
    // AgentId) must land in the duplicate-SessionStart arm — one sprite, one
    // desk — not mint a second slot.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "revived-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    let desk = scene.agents.get(&id).expect("synthesized").desk_index;

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "revived-sess".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );

    assert_eq!(scene.agents.len(), 1, "no duplicate sprite");
    assert_eq!(
        scene.agents.get(&id).unwrap().desk_index,
        desk,
        "the agent keeps its desk"
    );
}

#[test]
fn hook_synthesized_slot_is_exempt_from_unknown_cwd_reap() {
    // A hook-synthesized slot has an empty cwd, but it is NOT a startup
    // JSONL-seeding ghost (the population the 3-min unknown-cwd reap exists
    // for) — it is process-proven alive. The motivating scenario emits no
    // further event while parked on its permission prompt, so the 3-min reap
    // would kill the slot before any JSONL revive: it must get the normal
    // state-adaptive timeout (Waiting = 60 min) instead.
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "parked-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0,
        Transport::Hook,
    );
    assert!(
        !scene.agents.get(&id).expect("synthesized").unknown_cwd,
        "process-proven slot must not carry the startup-ghost flag"
    );

    r.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(60),
    );
    let slot = scene.agents.get(&id).expect("still present after 4 min");
    assert!(
        slot.exiting_at.is_none(),
        "a parked-on-permission synthesized slot must ride the Waiting timeout, not the 3-min ghost reap"
    );
}

#[test]
fn refused_hook_registration_does_not_poison_dedup_for_the_later_jsonl_copy() {
    // Desk exhaustion can refuse the hook synthesis. The slotless hook
    // ActivityStart must then NOT record into the hook-wins dedup map: a desk
    // can free within HOOK_WINS_WINDOW (an exiting slot's grace elapsing), and
    // the JSONL SessionStart + ActivityStart that then register the session
    // would have their ActivityStart dedup-eaten by the stale record — the
    // freshly visible agent would render Idle through its whole first tool.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use pixtuoid_core::state::MAX_FLOORS;
    let mut caps = [0usize; MAX_FLOORS];
    caps[0] = 1; // exactly one desk in the whole scene
    let mut scene = SceneState::new(caps);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // Fill the single desk, then end the occupant — its slot lingers for the
    // exit walk and keeps the desk occupied until the grace elapses.
    let occupant = AgentId::from_transcript_path("/p/occupant.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: occupant,
            source: "claude-code".into(),
            session_id: "o".into(),
            cwd: PathBuf::from("/Users/me/proj"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: occupant,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );

    // Hook ActivityStart for an unknown session while the desk is still held:
    // synthesis is refused. Sanity: no slot was created.
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let th = t0 + EXIT_GRACE_WINDOW - Duration::from_millis(100);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t9".into()),
            detail: None,
        },
        th,
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&id),
        "desk exhausted — registration must be refused"
    );

    // 300ms later (inside HOOK_WINS_WINDOW) the exit grace has elapsed: the
    // occupant sweeps, the desk frees, and the session registers via JSONL.
    let tj = t0 + EXIT_GRACE_WINDOW + Duration::from_millis(200);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        tj,
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t9".into()),
            detail: None,
        },
        tj,
        Transport::Jsonl,
    );

    let slot = scene
        .agents
        .get(&id)
        .expect("registered once the desk freed");
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "the JSONL ActivityStart must not be dedup-eaten by the refused hook's record"
    );
}

// ── G4: duplicate-SessionStart back-fill ────────────────────────────────────
// A slot can exist with missing identity context — hook synthesis registers
// from events that carry only the AgentId; a Codex revive ghost has an empty
// cwd. The FIRST SessionStart carrying the missing context heals the slot;
// established values are never overwritten (first-wins, the duplicate arm's
// existing semantics).

#[test]
fn duplicate_session_start_backfills_hook_synthesized_slot() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "#1");

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: PathBuf::from("/Users/me/repo"),
            parent_id: None,
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/repo"));
    assert!(!slot.unknown_cwd);
    assert_eq!(&*slot.source, "claude-code", "empty source back-filled");
    assert_eq!(
        &*slot.session_id, "gated-sess",
        "empty session_id back-filled"
    );
    assert_eq!(
        &*slot.label, "cc·repo",
        "ordinal fallback upgraded with the back-filled source's prefix"
    );
}

#[test]
fn duplicate_session_start_with_real_cwd_heals_an_unknown_cwd_ghost() {
    // The Codex revive shape: the slot was CREATED by a SessionStart with an
    // empty cwd (unknown_cwd ghost on the 3-min reap), and a later prompt
    // re-emits SessionStart with the real cwd — the ghost heals into a named
    // slot off the aggressive timer.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("codex", "cx-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".into(),
            session_id: "cx-sess".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(slot.unknown_cwd);
    assert_eq!(&*slot.label, "cx#1");

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".into(),
            session_id: "cx-sess".into(),
            cwd: PathBuf::from("/Users/me/myrepo"),
            parent_id: None,
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/myrepo"));
    assert!(!slot.unknown_cwd, "healed ghost leaves the 3-min reap");
    assert_eq!(&*slot.label, "cx·myrepo");
}

#[test]
fn duplicate_session_start_never_overwrites_established_cwd_or_label() {
    // First cwd wins — matching the duplicate arm's existing semantics.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    for cwd in ["/Users/me/repo-a", "/Users/me/repo-b"] {
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "claude-code".into(),
                session_id: "sess".into(),
                cwd: PathBuf::from(cwd),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/repo-a"));
    assert_eq!(&*slot.label, "cc·repo-a");
}

#[test]
fn backfill_does_not_clobber_a_renamed_label() {
    // A Rename-derived label (CC `attributionAgent`) is real information —
    // the back-fill may heal the cwd but must keep the name.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: id,
            label: "code-explorer".into(),
        },
        t0,
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: PathBuf::from("/Users/me/repo"),
            parent_id: None,
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(
        &*slot.cwd,
        std::path::Path::new("/Users/me/repo"),
        "cwd healed"
    );
    assert_eq!(&*slot.label, "code-explorer", "renamed label kept");
}

#[test]
fn two_step_backfill_source_first_then_cwd_still_upgrades_the_label() {
    // A revive SessionStart can itself carry no cwd (the watcher falls back to
    // the head cwd, but a truncated head may have none): the first duplicate
    // back-fills only source/session_id, the second brings the cwd. The
    // ordinal label must still read as a fallback after the source back-fill
    // re-contextualizes its prefix ("#1" under source "claude-code").
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.source, "claude-code", "source back-filled first");
    assert_eq!(&*slot.label, "#1", "no cwd yet — label unchanged");

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: PathBuf::from("/Users/me/repo"),
            parent_id: None,
        },
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/repo"));
    assert_eq!(
        &*slot.label, "cc·repo",
        "fallback still upgrades after the two-step heal"
    );
}

// ── #221: hook Identity — registration with real identity ──────────────────
// Hook decoders attach an `Identity` event (source/session_id/cwd) ahead of
// tool/permission activity events, so the proof-of-life registration for an
// unknown id normally lands with REAL identity instead of a blank `#N` slot
// (which remains the fallback for identity-less events like Stop). The arm
// registers-or-back-fills and NOTHING else: labels, activity state, and
// `last_event_at` stay owned by the activity/SessionStart paths.

#[test]
fn hook_identity_registers_unknown_id_with_real_identity() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: Some(PathBuf::from("/Users/me/repo")),
        },
        t0,
        Transport::Hook,
    );

    let slot = scene
        .agents
        .get(&id)
        .expect("hook Identity must register the slot");
    assert_eq!(
        &*slot.label, "cc·repo",
        "the normal minted label, NOT a blank #N ordinal"
    );
    assert_eq!(&*slot.source, "claude-code");
    assert_eq!(&*slot.session_id, "gated-sess");
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/repo"));
    assert!(
        !slot.unknown_cwd,
        "real-cwd registration — not an unknown-cwd ghost"
    );
    // Same floor/desk behavior as a real-cwd SessionStart registration: a
    // desk is allocated through the shared register_slot.
    assert_eq!(slot.floor_idx, scene.floor_of(slot.desk_index));
    assert!(
        matches!(slot.state, ActivityState::Idle),
        "Identity itself sets no activity state — the paired activity event does"
    );
}

#[test]
fn hook_identity_without_cwd_registers_reap_exempt_blank_cwd() {
    // Mirrors the blank synthesis path's exemption: a cwd-less Identity
    // (e.g. Codex PermissionRequest) registers an ordinal-labeled slot that
    // is process-proven alive — NOT a startup-seeding ghost, so it must ride
    // the normal state-adaptive timeouts, not the 3-min unknown-cwd reap.
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("codex", "cx-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "codex".into(),
            session_id: "cx-sess".into(),
            cwd: None,
        },
        t0,
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).expect("registered");
    assert_eq!(&*slot.label, "cx#1", "no cwd → ordinal label");
    assert_eq!(&*slot.source, "codex", "source still lands");
    assert_eq!(&*slot.session_id, "cx-sess", "session_id still lands");
    assert!(!slot.unknown_cwd, "process-proven — reap-exempt");

    r.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_none()),
        "must outlive the 3-min unknown-cwd reap"
    );
}

#[test]
fn hook_identity_backfills_blank_synthesized_slot() {
    // An identity-less hook event (e.g. a reordered Stop) synthesized a blank
    // slot first; the next Identity heals source/session_id/cwd — but leaves
    // the label alone (label upgrades stay on the SessionStart path) and does
    // not touch activity state.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "#1", "blank slot");

    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: Some(PathBuf::from("/Users/dev/proj")),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.source, "reasonix", "empty source healed");
    assert_eq!(&*slot.session_id, "/Users/dev/proj", "session_id healed");
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/dev/proj"));
    assert!(!slot.unknown_cwd);
    assert_eq!(
        &*slot.label, "#1",
        "Identity carries no label authority — upgrade stays on SessionStart"
    );
}

#[test]
fn hook_identity_does_not_clobber_existing_identity() {
    // First-wins, exactly like the duplicate-SessionStart back-fill: a slot
    // with established identity is untouched by a divergent Identity.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "sess".into(),
            cwd: PathBuf::from("/Users/me/repo-a"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "codex".into(),
            session_id: "other-sess".into(),
            cwd: Some(PathBuf::from("/Users/me/repo-b")),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(&*slot.source, "claude-code");
    assert_eq!(&*slot.session_id, "sess");
    assert_eq!(&*slot.cwd, std::path::Path::new("/Users/me/repo-a"));
    assert_eq!(&*slot.label, "cc·repo-a");
}

#[test]
fn hook_identity_respects_session_end_tombstone() {
    // A SessionEnd for an unknown id tombstones it; a reordered trailing
    // Identity from the same dying session must not re-register it — the same
    // guard the blank synthesis path runs.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "exited-invisible");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "exited-invisible".into(),
            cwd: Some(PathBuf::from("/Users/me/repo")),
        },
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&id),
        "a tombstoned id must not be re-registered by a reordered Identity"
    );
}

#[test]
fn jsonl_identity_is_a_no_op() {
    // Boundary (1) made structural: JSONL events must never synthesize — a
    // transcript line can be a historical replay. Nothing in-tree emits
    // Identity on Jsonl; the guard IS the boundary.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "replayed-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "replayed-sess".into(),
            cwd: Some(PathBuf::from("/Users/me/repo")),
        },
        t0,
        Transport::Jsonl,
    );
    assert!(
        scene.agents.is_empty(),
        "a JSONL Identity must not register a slot"
    );
}

#[test]
fn hook_identity_desk_refusal_is_quiet() {
    // Desk exhaustion refuses the registration — no slot, no panic, and (with
    // no tool_use_id on Identity) nothing that could poison the hook-wins
    // dedup map (boundary 3 untouched).
    use pixtuoid_core::state::MAX_FLOORS;
    let caps = [0usize; MAX_FLOORS]; // zero desks anywhere
    let mut scene = SceneState::new(caps);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "gated-sess");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::Identity {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "gated-sess".into(),
            cwd: Some(PathBuf::from("/Users/me/repo")),
        },
        t0,
        Transport::Hook,
    );
    assert!(
        scene.agents.is_empty(),
        "no desks — the registration must be quietly refused"
    );
}
