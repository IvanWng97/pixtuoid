use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use crate::start;

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
fn capacity_dropped_unknown_cwd_session_consumes_no_ghost_ordinal() {
    // The all-desks-occupied drop returns BEFORE the unknown-cwd ghost-ordinal
    // increment, so a dropped unknown-cwd session must consume NO ordinal — the
    // next ghost is still cc#1. Guards against hoisting the increment above the
    // capacity gate.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use pixtuoid_core::state::MAX_FLOORS;
    let mut caps = [0usize; MAX_FLOORS];
    caps[0] = 1; // exactly one desk in the whole scene
    let mut scene = SceneState::new(caps);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // Fill the single desk with a named session.
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

    // An unknown-cwd session now has no free desk → dropped (not inserted).
    let dropped = AgentId::from_transcript_path("/p/dropped.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: dropped,
            source: "claude-code".into(),
            session_id: "d".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&dropped),
        "no free desk → the session is dropped, not seated"
    );

    // Free the desk, then a NEW unknown-cwd session is the FIRST ghost: cc#1,
    // not cc#2 — the dropped one consumed no ordinal.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: occupant,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    r.tick(&mut scene, t0 + EXIT_GRACE_WINDOW + Duration::from_secs(1));
    assert!(!scene.agents.contains_key(&occupant), "occupant reaped");

    let ghost = AgentId::from_transcript_path("/p/ghost.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: ghost,
            source: "claude-code".into(),
            session_id: "g".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        t0 + EXIT_GRACE_WINDOW + Duration::from_secs(2),
        Transport::Hook,
    );
    assert_eq!(
        &*scene.agents.get(&ghost).unwrap().label,
        "cc#1",
        "a capacity-dropped unknown-cwd session must consume no ghost ordinal"
    );
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
