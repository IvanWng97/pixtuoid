use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::{
    Reducer, CHILD_END_LEDGER_TTL, HOOK_SESSION_END_TOMBSTONE_TTL,
};
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use crate::{act_end, act_start, sess_end, start};

#[test]
fn hook_session_end_tombstone_blocks_reordered_trailing_event_synthesis() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "exited-invisible");
    let other = AgentId::from_parts("claude-code", "still-alive");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    sess_end(&mut r, &mut scene, id, false, t0, Transport::Hook);
    act_end(
        &mut r,
        &mut scene,
        id,
        None,
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&id),
        "a reordered trailing event must not resurrect a tombstoned session"
    );

    act_end(
        &mut r,
        &mut scene,
        other,
        None,
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&other),
        "the tombstone must be per-id, not a global synthesis gate"
    );
}

#[test]
fn hook_event_after_tombstone_ttl_synthesizes_again() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "revived-later");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    sess_end(&mut r, &mut scene, id, false, t0, Transport::Hook);
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0 + HOOK_SESSION_END_TOMBSTONE_TTL + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&id),
        "past the TTL a hook event is fresh proof of life and must synthesize"
    );
}

#[test]
fn jsonl_child_session_start_within_tombstone_is_gated_too() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-late-file");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    sess_end(&mut r, &mut scene, child, true, t0, Transport::Hook);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-late-file".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "a JSONL child SessionStart racing its own hook Stop must not register"
    );
}

#[test]
fn non_child_session_end_tombstone_alone_gates_a_parented_start() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-nonchild-end");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    sess_end(&mut r, &mut scene, child, false, t0, Transport::Hook);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-nonchild-end".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "an as_child: false end arms ONLY the 5s #242 tombstone — that gate \
         alone must block the parented Start inside the TTL"
    );
}

#[test]
fn child_session_start_past_tombstone_ttl_registers() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-recycled");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    sess_end(&mut r, &mut scene, child, true, t0, Transport::Hook);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-recycled".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + CHILD_END_LEDGER_TTL + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&child),
        "past the ledger TTL a child SessionStart is a fresh registration"
    );
}

#[test]
fn tombstoned_parentless_session_start_still_registers() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    sess_end(&mut r, &mut scene, id, false, t0, Transport::Hook);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: PathBuf::from("/Users/dev/proj"),
            parent_id: None,
        },
        t0 + Duration::from_millis(20),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&id),
        "a parentless SessionStart must register straight through a fresh \
         tombstone (the Reasonix resurrect)"
    );
}

fn apply_hook_payload(
    r: &mut Reducer,
    scene: &mut SceneState,
    payload: serde_json::Value,
    now: SystemTime,
) {
    for ev in pixtuoid_core::source::decoder::decode_hook_payload(payload).expect("decodes") {
        r.apply(scene, ev, now, Transport::Hook);
    }
}

#[test]
fn late_parented_restart_of_an_ended_child_is_gated_by_the_child_ledger() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "01000000-0000-7000-8000-0000000000cc");
    let child = AgentId::from_parts("claude-code", "agent-a0000000000000001");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SessionStart",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "_pixtuoid_source": "claude-code",
            "cwd": "/repo",
        }),
        t0,
    );
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStart",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "agent_id": "a0000000000000001",
            "cwd": "/repo",
            "_pixtuoid_source": "claude-code",
        }),
        t0 + Duration::from_secs(1),
    );
    assert!(scene.agents.contains_key(&child), "child registered");
    let stop = t0 + Duration::from_secs(2);
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "agent_id": "a0000000000000001",
            "_pixtuoid_source": "claude-code",
        }),
        stop,
    );
    r.tick(
        &mut scene,
        stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    let late_start = |r: &mut Reducer, scene: &mut SceneState, at: SystemTime| {
        r.apply(
            scene,
            AgentEvent::SessionStart {
                agent_id: child,
                source: "claude-code".into(),
                session_id: "agent-a0000000000000001".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: Some(parent),
            },
            at,
            Transport::Jsonl,
        );
    };
    late_start(&mut r, &mut scene, stop + Duration::from_secs(30));
    assert!(
        !scene.agents.contains_key(&child),
        "a late parented restart of an ENDED child inside the ledger TTL \
         must not re-register a phantom (#244-w2)"
    );

    late_start(
        &mut r,
        &mut scene,
        stop + CHILD_END_LEDGER_TTL + Duration::from_secs(1),
    );
    assert!(
        scene.agents.contains_key(&child),
        "past CHILD_END_LEDGER_TTL the registration resumes"
    );
}

#[test]
fn parentless_revival_start_of_an_ended_codex_child_relinks_via_ledger() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    for transport in [Transport::Jsonl, Transport::Hook] {
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let parent = AgentId::from_parts("codex", "parent-sess");
        let child_uuid = "02000000-0000-7000-8000-0000000000cd";
        let child = AgentId::from_parts("codex", child_uuid);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "parent-sess",
                "_pixtuoid_source": "codex",
                "cwd": "/repo",
            }),
            t0,
        );
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "cwd": "/repo",
                "_pixtuoid_source": "codex",
            }),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(
            scene.agents.get(&child).map(|s| s.parent_id),
            Some(Some(parent)),
            "first life: child registered with the parent link ({transport:?})"
        );
        let stop = t0 + Duration::from_secs(2);
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStop",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "_pixtuoid_source": "codex",
            }),
            stop,
        );
        r.tick(
            &mut scene,
            stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
        );
        assert!(
            !scene.agents.contains_key(&child),
            "child GC'd after its first life"
        );

        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: child,
                source: "codex".into(),
                session_id: child_uuid.into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            stop + Duration::from_secs(20),
            transport,
        );
        assert_eq!(
            scene.agents.get(&child).map(|s| s.parent_id),
            Some(Some(parent)),
            "the parentless revival start must re-link to the ledger's \
             remembered parent, not register as an orphan ({transport:?})"
        );
    }
}

#[test]
fn a_multi_turn_child_idle_past_the_end_gate_still_revives_adopted_not_orphaned() {
    use pixtuoid_core::state::reducer::{CHILD_END_RELINK_TTL, EXIT_GRACE_WINDOW};
    use serde_json::json;
    for transport in [Transport::Jsonl, Transport::Hook] {
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let parent = AgentId::from_parts("codex", "parent-sess");
        let child_uuid = "02000000-0000-7000-8000-0000000000ce";
        let child = AgentId::from_parts("codex", child_uuid);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "parent-sess",
                "_pixtuoid_source": "codex",
                "cwd": "/repo",
            }),
            t0,
        );
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "cwd": "/repo",
                "_pixtuoid_source": "codex",
            }),
            t0 + Duration::from_secs(1),
        );
        let stop = t0 + Duration::from_secs(2);
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStop",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "_pixtuoid_source": "codex",
            }),
            stop,
        );
        r.tick(
            &mut scene,
            stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
        );
        assert!(
            !scene.agents.contains_key(&child),
            "child GC'd ({transport:?})"
        );

        let revival = stop + CHILD_END_LEDGER_TTL + Duration::from_secs(1);
        let start = |agent_id| AgentEvent::SessionStart {
            agent_id,
            source: "codex".into(),
            session_id: child_uuid.into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        };
        r.apply(&mut scene, start(child), revival, transport);
        assert_eq!(
            scene.agents.get(&child).map(|s| s.parent_id),
            Some(Some(parent)),
            "a child idle past the END GATE must revive ADOPTED, not orphaned ({transport:?})"
        );

        let mut scene2 = SceneState::uniform(4);
        let mut r2 = Reducer::new();
        apply_hook_payload(
            &mut r2,
            &mut scene2,
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "parent-sess",
                "_pixtuoid_source": "codex",
                "cwd": "/repo",
            }),
            t0,
        );
        apply_hook_payload(
            &mut r2,
            &mut scene2,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "cwd": "/repo",
                "_pixtuoid_source": "codex",
            }),
            t0 + Duration::from_secs(1),
        );
        apply_hook_payload(
            &mut r2,
            &mut scene2,
            json!({
                "hook_event_name": "SubagentStop",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "_pixtuoid_source": "codex",
            }),
            stop,
        );
        r2.tick(
            &mut scene2,
            stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
        );
        r2.apply(
            &mut scene2,
            start(child),
            stop + CHILD_END_RELINK_TTL + Duration::from_secs(1),
            transport,
        );
        assert_eq!(
            scene2.agents.get(&child).map(|s| s.parent_id),
            Some(None),
            "past the relink budget the memory is gone, so the revival is parentless \
             ({transport:?}) — the retention is bounded, not a leak"
        );
    }
}

#[test]
fn parentless_session_start_enriching_a_parentless_child_slot_adopts_ledger_parent() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child_uuid = "05000000-0000-7000-8000-0000000000d0";
    let child = AgentId::from_parts("codex", child_uuid);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |agent_id, sid: &str, parent_id| AgentEvent::SessionStart {
        agent_id,
        source: "codex".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id,
    };

    r.apply(
        &mut scene,
        session_start(parent, "parent-sess", None),
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start(child, child_uuid, Some(parent)),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    sess_end(
        &mut r,
        &mut scene,
        child,
        true,
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    let gone = t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1);
    r.tick(&mut scene, gone);
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    act_start(
        &mut r,
        &mut scene,
        child,
        Some("t-straggler"),
        None,
        gone + Duration::from_secs(10),
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(None),
        "precondition: the straggler re-registered the child parentless"
    );

    r.apply(
        &mut scene,
        session_start(child, child_uuid, None),
        gone + Duration::from_secs(11),
        Transport::Jsonl,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the enrichment path must adopt the ledger's remembered parent for a \
         parentless child slot"
    );
}

#[test]
fn tombstoned_codex_child_flat_first_sight_relinks_within_ledger_ttl() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child_uuid = "03000000-0000-7000-8000-0000000000ce";
    let child = AgentId::from_parts("codex", child_uuid);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "parent-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/repo",
        }),
        t0,
    );
    let subagent_stop = json!({
        "hook_event_name": "SubagentStop",
        "session_id": "parent-sess",
        "agent_id": child_uuid,
        "_pixtuoid_source": "codex",
    });
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStart",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "cwd": "/repo",
            "_pixtuoid_source": "codex",
        }),
        t0 + Duration::from_secs(1),
    );
    let stop = t0 + Duration::from_secs(2);
    apply_hook_payload(&mut r, &mut scene, subagent_stop.clone(), stop);
    r.tick(
        &mut scene,
        stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    let straggler = stop + EXIT_GRACE_WINDOW + Duration::from_secs(2);
    apply_hook_payload(&mut r, &mut scene, subagent_stop, straggler);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "codex".into(),
            session_id: child_uuid.into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        straggler + Duration::from_millis(500),
        Transport::Jsonl,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the tombstoned child's parentless flat first-sight must register \
         parent-LINKED via the ledger (#244-w1), not as an orphan phantom"
    );
}

#[test]
fn adopted_ledger_parent_still_runs_the_cycle_filter() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let p = AgentId::from_parts("codex", "p-root");
    let x = AgentId::from_parts("codex", "x-child");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |agent_id, sid: &str, parent_id| AgentEvent::SessionStart {
        agent_id,
        source: "codex".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id,
    };

    r.apply(
        &mut scene,
        session_start(p, "p-root", None),
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start(x, "x-child", Some(p)),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    sess_end(
        &mut r,
        &mut scene,
        x,
        true,
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&x), "X GC'd");

    r.apply(
        &mut scene,
        session_start(p, "p-root", Some(x)),
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&p).map(|s| s.parent_id),
        Some(Some(x)),
        "precondition: P now dangles on the dead X"
    );

    r.apply(
        &mut scene,
        session_start(x, "x-child", None),
        t0 + Duration::from_secs(11),
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&x).expect("X re-registers");
    assert_eq!(
        slot.parent_id, None,
        "an adopted ledger parent that would close a cycle must degrade to \
         parentless (the #240 filter runs on adopted links too)"
    );
}

#[test]
fn reasonix_resurrect_is_unaffected_by_a_ledger_entry_for_another_id() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let codex_parent = AgentId::from_parts("codex", "parent-sess");
    let codex_child = AgentId::from_parts("codex", "04000000-0000-7000-8000-0000000000cf");
    let rx = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, codex_parent);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: codex_child,
            source: "codex".into(),
            session_id: "04000000-0000-7000-8000-0000000000cf".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(codex_parent),
        },
        t0,
        Transport::Hook,
    );
    sess_end(
        &mut r,
        &mut scene,
        codex_child,
        true,
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    sess_end(
        &mut r,
        &mut scene,
        rx,
        false,
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: rx,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: PathBuf::from("/Users/dev/proj"),
            parent_id: None,
        },
        t0 + Duration::from_secs(2) + Duration::from_millis(20),
        Transport::Hook,
    );
    let slot = scene
        .agents
        .get(&rx)
        .expect("the Reasonix resurrect registers");
    assert_eq!(
        slot.parent_id, None,
        "a ledger entry for a DIFFERENT id must never re-parent a Reasonix \
         session (its ids never enter the ledger by construction)"
    );
}
