use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::jsonl::ChildEndUnclaims;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Source;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

use crate::{cc_subagent_line, cc_watcher, fast_watch, write_lines};

// ── Child-end un-claim (#246): the carrier for in-flight multi-turn children ─

/// #246 — the watcher half of the un-claim side-channel: a hook SubagentStop
/// ends a child whose transcript stays `seen`-claimed, so turn-N+1 appends
/// used to decode as unknown-id no-ops with no re-registration carrier on
/// either transport (upstream provides none — codex-rs hook_runtime.rs).
/// Pushing the child id into the shared handle (the ClaudeCodeSource tee's
/// job in production, pinned separately) must make the running watcher
/// release the claim on a scan/notify pass, so a later append re-registers
/// the SAME id with a FRESH SessionStart. (The drain-before-release straggler
/// discipline is pinned deterministically in source/jsonl/tests.rs's
/// `child_end_unclaim_drains_stragglers_then_releases_without_session_end`.)
#[tokio::test]
async fn child_end_unclaim_lets_a_turn_n_plus_1_append_re_register() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let parent_uuid = "0f000000-0000-7000-8000-0000000000f1";
    let sub_dir = projects_root
        .join("-Users-me-proj")
        .join(parent_uuid)
        .join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    let child_stem = "agent-f0000000000000001";
    let transcript = sub_dir.join(format!("{child_stem}.jsonl"));
    let expected = AgentId::from_parts("claude-code", child_stem);

    let unclaims = ChildEndUnclaims::new();
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_poll_interval(Duration::from_millis(100))
        .with_child_end_unclaims(unclaims.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Turn N: the child's transcript registers (claims `seen`).
    write_lines(
        &transcript,
        &[cc_subagent_line(child_stem, "/repo", "tu_n1")],
    )
    .await;
    let mut registered = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if agent_id == expected {
                registered = true;
                break;
            }
        }
    }
    assert!(registered, "turn N must register the child transcript");

    // Turn N ends: the SubagentStop hook is decoded; the tee pushes the id.
    unclaims.push(expected);

    // Turn N+1: appends arrive. The first notify after the push drains the
    // handle (releasing the claim) — appends keep coming during an active
    // turn, so re-registration converges within a couple of writes.
    let mut revived = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut next_tu = 0u32;
    while tokio::time::Instant::now() < deadline && !revived {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .await
            .unwrap();
        next_tu += 1;
        f.write_all(
            format!(
                "{}\n",
                cc_subagent_line(child_stem, "/repo", &format!("tu_n2_{next_tu}"))
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        f.flush().await.unwrap();
        drop(f);
        let wait_until = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < wait_until {
            if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                if agent_id == expected {
                    revived = true;
                    break;
                }
            }
        }
    }
    assert!(
        revived,
        "after the child-end un-claim, a turn-N+1 append must re-register \
         the SAME id with a fresh SessionStart"
    );
    handle.abort();
}

/// THE #246 acceptance test — the full in-flight multi-turn Codex child story
/// across the REAL CodexSource (run()-wiring included) + the reducer:
/// the SubagentStart hook registers the child WITH its parent (the ledger
/// remembers the applied link); the flat rollout is `seen`-claimed by the
/// watcher; the SubagentStop hook (`as_child: true`) ends the slot and — via
/// the tee, simulated here by the push (pinned separately in claude_code.rs)
/// — feeds the un-claim side-channel; the turn-N+1 append then re-emits a
/// parentless SessionStart through the released claim, and the reducer's
/// child ledger RE-LINKS it to the remembered parent: the child re-joins the
/// scope tree mid-flight instead of staying invisible.
#[tokio::test]
async fn in_flight_multi_turn_codex_child_revives_and_relinks_via_unclaim() {
    use pixtuoid_core::source::decoder::decode_hook_payload;
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use pixtuoid_core::{Reducer, SceneState};

    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let child_uuid = "0e000000-0000-7000-8000-0000000000e1";
    let rollout = sessions_root.join(format!("rollout-2026-06-11T10-00-00-{child_uuid}.jsonl"));
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child = AgentId::from_parts("codex", child_uuid);

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let apply_hook = |r: &mut Reducer, scene: &mut SceneState, payload: serde_json::Value| {
        for ev in decode_hook_payload(payload).expect("hook payload decodes") {
            r.apply(scene, ev, SystemTime::now(), Transport::Hook);
        }
    };

    // Thread startup: the parent prompt + the child's SubagentStart hook.
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "parent-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/repo",
        }),
    );
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "SubagentStart",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "cwd": "/repo",
            "_pixtuoid_source": "codex",
        }),
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "first life: the child registers with the parent link"
    );

    // The REAL CodexSource (with the shared handle) claims the rollout.
    let unclaims = ChildEndUnclaims::new();
    let src = CodexSource {
        sessions_root,
        child_end_unclaims: Some(unclaims.clone()),
    };
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher_task = tokio::spawn(async move { Box::new(src).run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": child_uuid, "cwd": "/repo" }
    });
    let work = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t1" }
    });
    write_lines(&rollout, &[meta, work.clone()]).await;
    let mut claimed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !claimed {
        if let Ok(Some((transport, ev))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            claimed = matches!(
                &ev,
                AgentEvent::SessionStart { agent_id, .. } if *agent_id == child
            );
            r.apply(&mut scene, ev, SystemTime::now(), transport);
        }
    }
    assert!(claimed, "the watcher must claim the child's flat rollout");

    // Turn N ends: SubagentStop ends the slot; the tee pushes the id.
    apply_hook(
        &mut r,
        &mut scene,
        serde_json::json!({
            "hook_event_name": "SubagentStop",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "_pixtuoid_source": "codex",
        }),
    );
    unclaims.push(child);
    r.tick(
        &mut scene,
        SystemTime::now() + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(
        !scene.agents.contains_key(&child),
        "the child's first life is GC'd after the SubagentStop"
    );

    // Turn N+1 (parent send_input): rollout appends resume. Through the
    // released claim a parentless SessionStart re-registers the child, and
    // the ledger restores the remembered parent.
    let mut relinked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !relinked {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .await
            .unwrap();
        f.write_all(format!("{work}\n").as_bytes()).await.unwrap();
        f.flush().await.unwrap();
        drop(f);
        let wait_until = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < wait_until {
            if let Ok(Some((transport, ev))) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                r.apply(&mut scene, ev, SystemTime::now(), transport);
                relinked = scene.agents.get(&child).map(|s| s.parent_id) == Some(Some(parent));
                if relinked {
                    break;
                }
            }
        }
    }
    assert!(
        scene.agents.contains_key(&child),
        "turn N+1 must re-register the child through the released claim"
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the revived child must re-link to the remembered parent (the #249 \
         ledger), not register as an orphan"
    );
    watcher_task.abort();
}
