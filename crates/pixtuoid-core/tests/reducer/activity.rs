use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;

use crate::start;

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
            tool_use_id: Some("t1".into()),
            detail: Some("Edit: foo.rs".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
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

// A turn-end `Stop` (hook, no tool_use_id — Codex/Reasonix) resolves a stale
// Waiting: an approval prompt BLOCKS those CLIs' turns, so Stop arriving while
// Waiting means the prompt was denied/abandoned and already resolved upstream.
// Without this, a denied Reasonix approval at turn end ghosts "waiting" until
// the 60-min sweep (Reasonix has no second transport to self-heal it).
#[test]
fn turn_end_stop_hook_resolves_stale_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "approval needed: bash rm -rf ./build".into(),
        },
        t0,
        Transport::Hook,
    );
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));

    // Turn ends (denied prompt): Stop → ActivityEnd with no id, Hook transport.
    let end = t0 + Duration::from_millis(800);
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        },
        end,
        Transport::Hook,
    );
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

// Protection (the Hook gate): a JSONL None-id end must NOT resolve a Waiting —
// Codex's JSONL emits None-id ActivityEnds per tool (it opts out of dedup),
// and one can race in just after a fresh PermissionRequest. Only the hook-side
// turn-end signal is trustworthy.
#[test]
fn jsonl_none_id_end_while_waiting_keeps_waiting() {
    use pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("codex", "sess-1");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0,
        Transport::Hook,
    );
    // A late rollout line for the PREVIOUS tool races in after the prompt.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        },
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

// Copilot permission gate, decoder → reducer (cross-layer): a DENIED permission
// must leave the sprite, not strand it in Waiting for the 60-min sweep. The
// decoder emits ActivityStart{tool_use_id:None} on denial; this pins that the
// reducer actually transitions Waiting→Active on it (the bot's PR #292 ask —
// the decoder unit test only proved the event is emitted, not that it clears).
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
    // BYTE-REAL (#294): a matched interactive permission round captured from a
    // live `copilot` 1.0.62 session (requestId 954afe31…, the user pressed Reject).
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
