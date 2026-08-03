use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;

use crate::{act_end, act_start, start, waiting};

#[test]
fn activity_start_sets_state_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        Some("Edit: foo.rs"),
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
}

#[test]
fn activity_end_arms_debounce_then_tick_flips_to_idle() {
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(slot.pending_idle_at.is_some());

    r.tick(&mut scene, t0 + Duration::from_millis(900));
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));

    r.tick(&mut scene, t0 + Duration::from_millis(2000));
    assert_eq!(scene.agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_none());
}

#[test]
fn activity_start_inside_grace_window_cancels_debounce() {
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_some());
    // A second tool starts well inside the grace window.
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t2"),
        None,
        t0 + Duration::from_millis(300),
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(
        slot.pending_idle_at.is_none(),
        "ActivityStart inside grace must cancel pending idle"
    );
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

    waiting(
        &mut r,
        &mut scene,
        id,
        "Bash: rm -rf?",
        SystemTime::now(),
        Transport::Hook,
    );

    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Waiting { reason } => assert_eq!(&**reason, "Bash: rm -rf?"),
        other => panic!("unexpected state: {other:?}"),
    }
}

#[test]
fn tool_call_count_increments_on_activity_start() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stats.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 0);

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t2"),
        None,
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

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().active_ms, 0);

    let t1 = t0 + Duration::from_secs(1);
    act_end(&mut r, &mut scene, id, Some("t1"), t1, Transport::Hook);
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

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(2);
    act_end(&mut r, &mut scene, id, Some("t1"), t1, Transport::Hook);
    // The 2nd end is deliberately PAST the hook-wins window, so it is NOT deduped.
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        t1 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    r.tick(&mut scene, t1 + Duration::from_secs(3));
    let slot = scene.agents.get(&id).unwrap();
    // 2600ms = the ONE span t0 → t1+600, EXTENDED by the late end; a second
    // folded span would read ~4600.
    assert_eq!(
        slot.active_ms, 2600,
        "active_ms should be the single t0→t1+600 span (2600ms), not double-counted"
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

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );

    // Task arrives while still Active (within the grace window).
    let t1 = t0 + Duration::from_secs(2);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
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

    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(3);
    waiting(&mut r, &mut scene, id, "permission", t1, Transport::Hook);

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 3000,
        "expected >= 3000ms active before Waiting, got {}",
        slot.active_ms
    );
}

// CC wire fact (captured live): when a permission Notification fires mid-tool
// (PreToolUse(t1) → Notification → PostToolUse(t1)), the gated tool's
// ActivityEnd carries the same tool_use_id that was Active when Waiting began —
// so PostToolUse(t1) means the permission was granted and the Waiting resolved.
#[test]
fn gated_tool_end_while_waiting_resolves_to_idle_after_grace() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
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

    // The gated tool's own PostToolUse arrives.
    let end = t0 + Duration::from_millis(1000);
    act_end(&mut r, &mut scene, id, Some("t1"), end, Transport::Hook);
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

#[test]
fn parallel_tool_end_while_waiting_keeps_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        None,
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

    // A different tool ends.
    act_end(
        &mut r,
        &mut scene,
        id,
        Some("t2"),
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

// An approval prompt BLOCKS a Codex/Reasonix turn, so a turn-end `Stop` (hook,
// no tool_use_id) arriving while Waiting means the prompt was already resolved
// upstream — and Reasonix has no second transport to self-heal a stale one.
#[test]
fn turn_end_stop_hook_resolves_stale_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    waiting(
        &mut r,
        &mut scene,
        id,
        "approval needed: bash rm -rf ./build",
        t0,
        Transport::Hook,
    );
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));

    let end = t0 + Duration::from_millis(800);
    act_end(&mut r, &mut scene, id, None, end, Transport::Hook);
    r.tick(
        &mut scene,
        end + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(scene.agents.get(&id).unwrap().state, ActivityState::Idle),
        "turn-end Stop must resolve the stale Waiting to Idle, got {:?}",
        scene.agents.get(&id).unwrap().state
    );
}

// Codex's JSONL emits None-id ActivityEnds per tool (it opts out of dedup), and
// one can race in just after a fresh PermissionRequest — so only the HOOK-side
// turn-end signal may resolve a Waiting.
#[test]
fn jsonl_none_id_end_while_waiting_keeps_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("codex", "sess-1");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    waiting(&mut r, &mut scene, id, "permission", t0, Transport::Hook);
    act_end(
        &mut r,
        &mut scene,
        id,
        None,
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_millis(200) + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a racing JSONL None-id end must keep the permission prompt up"
    );
}

#[test]
fn codex_permission_then_jsonl_output_resumes_to_active() {
    // The hook Waiting and the JSONL resume coalesce on the session UUID.
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

    waiting(
        &mut reducer,
        &mut scene,
        id,
        "permission",
        now,
        Transport::Hook,
    );
    assert!(
        matches!(scene.agents[&id].state, ActivityState::Waiting { .. }),
        "should be Waiting on permission"
    );

    act_start(
        &mut reducer,
        &mut scene,
        id,
        None,
        Some("exec_command"),
        now,
        Transport::Jsonl,
    );
    assert!(
        matches!(scene.agents[&id].state, ActivityState::Active { .. }),
        "resume must return to Active"
    );
}

#[test]
fn copilot_denied_permission_clears_waiting_through_the_reducer() {
    use pixtuoid_core::source::copilot::decode_copilot_line;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let path = "/p/session-state/sess/events.jsonl";
    let id = AgentId::from_parts("copilot", "sess");
    let feed = |r: &mut Reducer, scene: &mut SceneState, line: &str| {
        for ev in decode_copilot_line(path, "copilot", serde_json::from_str(line).unwrap()).unwrap()
        {
            r.apply(scene, ev, SystemTime::now(), Transport::Jsonl);
        }
    };
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"session.start","data":{"sessionId":"sess","context":{"cwd":"/repo"}},"id":"a","parentId":null}"#,
    );
    // BYTE-REAL: a matched permission round captured from a live copilot session
    // (the user pressed Reject) — do not hand-edit these payloads.
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"permission.requested","data":{"requestId":"954afe31-559a-4afc-9eb6-13e30cf48aea","permissionRequest":{"kind":"shell","toolCallId":"call_nf1RvU9GxssNg2g7WtPgHqQ4","fullCommandText":"cat /etc/hostname","intention":"Show system hostname","commands":[{"identifier":"cat","readOnly":true}],"possiblePaths":["/etc/hostname"],"possibleUrls":[],"hasWriteFileRedirection":false,"canOfferSessionApproval":true},"promptRequest":{"kind":"path","accessKind":"shell","paths":["/etc/hostname"],"toolCallId":"call_nf1RvU9GxssNg2g7WtPgHqQ4"}},"id":"5240af45-3ad2-4bf7-bc37-83c329c9c2ea","timestamp":"2026-06-14T21:38:40.507Z","parentId":"cb3c0a03-3f84-451c-bac6-843f0632ba9f"}"#,
    );
    assert!(
        matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "permission.requested should set Waiting"
    );
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"permission.completed","data":{"requestId":"954afe31-559a-4afc-9eb6-13e30cf48aea","toolCallId":"call_nf1RvU9GxssNg2g7WtPgHqQ4","result":{"kind":"denied-interactively-by-user"}},"id":"60dae716-c76c-45e2-84e1-c3248ce3790c","timestamp":"2026-06-14T21:38:43.086Z","parentId":"5240af45-3ad2-4bf7-bc37-83c329c9c2ea"}"#,
    );
    assert!(
        !matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "a DENIED permission must clear Waiting (else the sprite hangs 60 min)"
    );
}

// Line shapes follow a captured omp ask round, sanitized.
#[test]
fn omp_ask_round_waits_then_answer_clears_through_the_reducer() {
    use pixtuoid_core::source::omp::decode_omp_line;
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let path =
        "/h/.omp/agent/sessions/-p/2026-07-05T20-37-08-710Z_01000000-0000-7000-8000-000000000001.jsonl";
    let id = AgentId::from_parts(
        "omp",
        "2026-07-05T20-37-08-710Z_01000000-0000-7000-8000-000000000001",
    );
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let feed = |r: &mut Reducer, scene: &mut SceneState, line: &str, at: SystemTime| {
        for ev in decode_omp_line(path, "omp", serde_json::from_str(line).unwrap()).unwrap() {
            r.apply(scene, ev, at, Transport::Jsonl);
        }
    };
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"session","version":3,"id":"01000000-0000-7000-8000-000000000001","timestamp":"2026-07-05T20:37:08.710Z","cwd":"/repo"}"#,
        t0,
    );
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"message","id":"f689ad6b","parentId":"71a1e3cb","timestamp":"2026-07-05T20:39:46.078Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"toolu_01ASKASKASKASKASKASKASKA","name":"ask","arguments":{"i":"Resolving packages/ui collision","questions":[{"id":"ui_collision","question":"packages/ui already exists. What should happen?","options":[{"label":"Replace"},{"label":"Merge"}],"recommended":1}]}}],"stopReason":"toolUse","timestamp":1783283963575}}"#,
        t0 + Duration::from_millis(100),
    );
    assert!(
        matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "the ask toolCall should park the slot Waiting"
    );
    // The user answers: the ask's toolResult (same toolCallId) arrives.
    feed(
        &mut r,
        &mut scene,
        r#"{"type":"message","id":"4a4176db","parentId":"98404700","timestamp":"2026-07-05T20:40:39.589Z","message":{"role":"toolResult","toolCallId":"toolu_01ASKASKASKASKASKASKASKA","toolName":"ask","content":[{"type":"text","text":"User selected: Merge"}],"isError":false,"timestamp":1783284039588}}"#,
        t0 + Duration::from_millis(500),
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_millis(500) + ACTIVE_GRACE_WINDOW + Duration::from_millis(100),
    );
    assert!(
        !matches!(
            scene.agents.get(&id).unwrap().state,
            ActivityState::Waiting { .. }
        ),
        "the answered ask must clear Waiting (else the sprite hangs 60 min)"
    );
}
