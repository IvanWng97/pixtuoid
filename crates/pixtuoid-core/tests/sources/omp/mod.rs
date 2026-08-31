//! omp's bridge-extension rounds with NO transcript to coalesce against —
//! recorded from real `omp` runs with the extension installed (#951). They
//! live here rather than under `sources/fixtures/omp/` because the
//! conformance harness's one-transcript rule cannot hold them: an EMPTY
//! session never persists a file (that gap is the point), a resume's file
//! predates the run, and a task run is two sprites.

use std::time::{Duration, SystemTime};

use pixtuoid_core::harness::Drive;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::{Reducer, EXIT_GRACE_WINDOW};
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

fn lines(scenario: &str) -> Vec<String> {
    super::captures::fixture_lines(
        &super::captures::sources_root()
            .join(format!("omp/fixtures/{scenario}/hook-payloads.jsonl")),
    )
}

fn events(scenario: &str) -> Vec<AgentEvent> {
    let d = Drive::hooks().lines(lines(scenario));
    d.assert_clean(scenario);
    d.events
}

/// The issue's empty-session criterion: a session that shuts down before any
/// assistant message persists NOTHING (no transcript, no `session_exit`), so
/// the bridge's `session_shutdown` is the only end signal — and it must
/// remove the slot on the exit grace, not the 30-minute stale sweep.
#[test]
fn an_empty_session_round_registers_then_leaves_on_the_exit_grace() {
    let evs = events("empty-session-recorded");
    let id = evs[0].agent_id();
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for (i, ev) in evs.into_iter().enumerate() {
        r.apply(
            &mut scene,
            ev,
            t0 + Duration::from_secs(i as u64),
            Transport::Hook,
        );
    }
    assert!(
        scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_some()),
        "the bridge shutdown must start the walkout"
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(
        !scene.agents.contains_key(&id),
        "an empty session leaves on the exit grace, not the stale sweep"
    );
}

/// `omp --continue` in a fresh process fires `session_start` on the SAME
/// stem-keyed id the earlier life used — the ended session walks back in
/// through ordinary parentless re-registration (registry row doc).
#[test]
fn a_resumed_session_re_registers_after_its_earlier_end() {
    let evs = events("resume-recorded");
    let id = evs[0].agent_id();
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    // First life: the recorded round is start → shutdown; let the slot leave.
    for (i, ev) in events("resume-recorded").into_iter().enumerate() {
        r.apply(
            &mut scene,
            ev,
            t0 + Duration::from_secs(i as u64),
            Transport::Hook,
        );
    }
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&id), "first life left");

    // Second life: the SAME recorded start must register again.
    let t1 = t0 + Duration::from_secs(600);
    r.apply(&mut scene, evs[0].clone(), t1, Transport::Hook);
    assert!(
        scene.agents.contains_key(&id),
        "a resume's session_start re-registers the ended id"
    );
}

/// A task run under the bridge: the parent AND the in-process child each fire
/// their own lifecycle, the child keyed by the NESTED file so both transports
/// mint one id, parent-linked by path, both stamped with the ONE omp pid.
#[test]
fn a_task_round_is_two_linked_lifecycles_under_one_pid() {
    let evs = events("task-recorded");
    let starts: Vec<&AgentEvent> = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionStart { .. }))
        .collect();
    let [AgentEvent::SessionStart {
        agent_id: parent,
        session_id: parent_key,
        parent_id: None,
        ..
    }, AgentEvent::SessionStart {
        agent_id: child,
        session_id: child_key,
        parent_id: Some(linked),
        ..
    }] = starts.as_slice()
    else {
        panic!("expected a parentless start then a linked child start: {starts:?}");
    };
    assert_eq!(linked, parent, "the child links to the parent by path");
    assert_eq!(
        *child_key,
        format!("{parent_key}/SayHi"),
        "the child keys on the nested stem chain, same as the transcript side"
    );
    assert_eq!(*child, AgentId::from_parts("omp", child_key));

    let ends: Vec<bool> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionEnd { as_child, .. } => Some(*as_child),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        vec![false, true],
        "the root ends as a root, the nested child as a child"
    );

    // One in-process pid on the raw wire — the whole fan-out is one omp.
    let pids: std::collections::BTreeSet<i64> = lines("task-recorded")
        .iter()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).expect("valid json")["_pid"]
                .as_i64()
                .expect("the extension stamps _pid")
        })
        .collect();
    assert_eq!(pids.len(), 1, "extensions run in-process: one pid");
}
