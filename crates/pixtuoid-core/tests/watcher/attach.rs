// ── Mid-attach scenario suite ────────────────────────────────────────────────
// The acceptance criterion: opening pixtuoid at ANY moment must show all
// active running agent CLIs correctly. These drive JsonlWatcher::run over a
// pre-populated projects-root — the attach moment — and assert the emitted
// live set, exercising the first-sight gate, the liveness-probe bypass, the
// ended check, and the subagent parent link TOGETHER.

use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::mpsc;

use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

use crate::{
    backdate, cc_session_start_line, cc_subagent_line, cc_tool_use_line, cc_watcher,
    vouch_snapshot, write_lines,
};

/// S2 + S6 — THE acceptance test for "attach shows exactly the live set".
/// ONE watcher run over a projects-root holding, side by side:
///   (a) a recent live transcript                 → registers (fresh mtime)
///   (b) a stale transcript the probe vouches for → registers (probe bypass)
///   (c) a stale transcript NOT in the probe      → stays hidden
///   (d) a recent transcript ending in a structural session_end → stays hidden
///   (e) a fresh subagent transcript under (b)    → registers, linked to (b)
/// Exactly {a, b, e} emit SessionStart — and each exactly ONCE across the
/// initial seed, the 250ms rescan, and the poll cycles (S6: emit_first_sight
/// idempotence at the suite level — re-scans must not duplicate registrations).
#[tokio::test]
async fn attach_matrix_registers_exactly_the_live_set() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-mixed");
    let live_a = "aa000000-0000-7000-8000-00000000000a";
    let idle_b = "bb000000-0000-7000-8000-00000000000b";
    let dead_c = "cc000000-0000-7000-8000-00000000000c";
    let ended_d = "dd000000-0000-7000-8000-00000000000d";
    let sub_dir = proj.join(idle_b).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    // (a) recent live: fresh mtime, no end marker.
    write_lines(
        &proj.join(format!("{live_a}.jsonl")),
        &[
            cc_session_start_line(live_a, "/Users/me/proj-a"),
            cc_tool_use_line(
                live_a,
                "/Users/me/proj-a",
                "tu_a",
                "Bash",
                serde_json::json!({"command": "ls"}),
            ),
        ],
    )
    .await;
    // (b) long-idle but probe-live: stale mtime, in the probe's live set.
    let b_path = proj.join(format!("{idle_b}.jsonl"));
    write_lines(
        &b_path,
        &[cc_session_start_line(idle_b, "/Users/me/proj-b")],
    )
    .await;
    backdate(&b_path, 7200);
    // (c) genuinely dead: stale mtime, NOT in the probe.
    let c_path = proj.join(format!("{dead_c}.jsonl"));
    write_lines(
        &c_path,
        &[cc_session_start_line(dead_c, "/Users/me/proj-c")],
    )
    .await;
    backdate(&c_path, 7200);
    // (d) recent but ENDED: fresh mtime, structural session_end at the tail.
    write_lines(
        &proj.join(format!("{ended_d}.jsonl")),
        &[
            cc_session_start_line(ended_d, "/Users/me/proj-d"),
            serde_json::json!({
                "type": "system", "subtype": "session_end", "sessionId": ended_d
            }),
        ],
    )
    .await;
    // (e) fresh subagent under the probe-live parent (b).
    write_lines(
        &sub_dir.join("agent-e1.jsonl"),
        &[cc_subagent_line("agent-e1", "/Users/me/proj-b", "tu_e")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[idle_b])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let id_a = AgentId::from_parts("claude-code", live_a);
    let id_b = AgentId::from_parts("claude-code", idle_b);
    let id_e = AgentId::from_parts("claude-code", "agent-e1");

    let mut starts: std::collections::HashMap<AgentId, usize> = Default::default();
    let mut sub_parent: Option<Option<AgentId>> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            *starts.entry(agent_id).or_default() += 1;
            if agent_id == id_e {
                sub_parent = Some(parent_id);
            }
        }
        if starts.contains_key(&id_a) && starts.contains_key(&id_b) && starts.contains_key(&id_e) {
            break;
        }
    }
    // S6: settle past the 250ms post-startup rescan (plus several poll
    // cycles), then drain — re-scans of the SAME root must not re-emit
    // SessionStart for any fixture.
    tokio::time::sleep(Duration::from_millis(700)).await;
    while let Ok(Some((_, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        if let AgentEvent::SessionStart {
            agent_id,
            parent_id,
            ..
        } = ev
        {
            *starts.entry(agent_id).or_default() += 1;
            if agent_id == id_e {
                sub_parent = Some(parent_id);
            }
        }
    }

    let expected: std::collections::HashMap<AgentId, usize> =
        [(id_a, 1), (id_b, 1), (id_e, 1)].into_iter().collect();
    assert_eq!(
        starts, expected,
        "attach must register EXACTLY the live set, once each — a (recent), b (probe-live), e (fresh subagent); c (stale dead) and d (ended) stay hidden"
    );
    assert_eq!(
        sub_parent.expect("the subagent's SessionStart was seen"),
        Some(id_b),
        "the fresh subagent must attach linked to its probe-live parent"
    );
    handle.abort();
}

/// S3 — the delegating-parent attach: the parent transcript is STALE (it has
/// been silently waiting on its subagent for longer than the window), its tail
/// is a pending Agent dispatch (tool_use with no tool_result), and its UUID is
/// in the probe's live set; the subagent transcript is fresh. One attach scan
/// must register BOTH, link the subagent to the parent, and replay the pending
/// dispatch as a Task ActivityStart (so the reducer's active_tasks suppression
/// picks the in-flight delegation back up).
#[tokio::test]
async fn delegating_parent_attach_registers_parent_and_links_subagent() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-deleg");
    let parent_uuid = "ee000000-0000-7000-8000-00000000000e";
    let sub_dir = proj.join(parent_uuid).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    let parent_path = proj.join(format!("{parent_uuid}.jsonl"));
    write_lines(
        &parent_path,
        &[
            cc_session_start_line(parent_uuid, "/Users/me/deleg"),
            cc_tool_use_line(
                parent_uuid,
                "/Users/me/deleg",
                "tu_task",
                "Agent",
                serde_json::json!({
                    "description": "explore", "subagent_type": "code-explorer", "prompt": "go"
                }),
            ),
        ],
    )
    .await;
    backdate(&parent_path, 7200);
    write_lines(
        &sub_dir.join("agent-x1.jsonl"),
        &[cc_subagent_line("agent-x1", "/Users/me/deleg", "tu_x")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[parent_uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected_parent = AgentId::from_parts("claude-code", parent_uuid);
    let mut parent_started = false;
    let mut sub_parent_id: Option<AgentId> = None;
    let mut dispatch_is_task: Option<bool> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some((
                _,
                AgentEvent::SessionStart {
                    agent_id,
                    parent_id,
                    ..
                },
            ))) => match parent_id {
                Some(pid) => sub_parent_id = Some(pid),
                None => parent_started |= agent_id == expected_parent,
            },
            Ok(Some((
                _,
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id,
                    detail,
                },
            ))) if agent_id == expected_parent && tool_use_id.as_deref() == Some("tu_task") => {
                dispatch_is_task = Some(detail.as_ref().is_some_and(|d| d.is_task()));
            }
            _ => {}
        }
        if parent_started && sub_parent_id.is_some() && dispatch_is_task.is_some() {
            break;
        }
    }
    assert!(
        parent_started,
        "the stale probe-live delegating parent must register at attach"
    );
    assert_eq!(
        sub_parent_id,
        Some(expected_parent),
        "the subagent must attach linked to the parent, not as an orphan"
    );
    assert_eq!(
        dispatch_is_task,
        Some(true),
        "the replayed pending Agent dispatch must decode as a Task ActivityStart"
    );
    handle.abort();
}

/// #222 — the oversized delegating-parent attach: like S3, but the parent
/// transcript has > MAX_PENDING_BYTES pending at attach, so the backlog is
/// skipped to EOF instead of replayed. The in-flight Agent dispatch sits in
/// the last 256 KiB (TASK_SCAN_BYTES) with no tool_result — the tail scan
/// must re-emit it as a Task ActivityStart, EXACTLY ONCE across the initial
/// seed + rescan + poll cycles, while the completed Bash backlog stays
/// un-replayed. The parent registers via the bounded head read (#204) and the
/// fresh subagent links to it.
#[tokio::test]
async fn oversized_delegating_parent_attach_replays_pending_dispatch() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let proj = projects_root.join("-Users-me-bigdeleg");
    let parent_uuid = "ab000000-0000-7000-8000-0000000000ab";
    let sub_dir = proj.join(parent_uuid).join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();

    // Parent: head session_start (cwd on line 1), > 1 MiB of completed-tool
    // backlog, then the pending Agent dispatch at the tail.
    let parent_path = proj.join(format!("{parent_uuid}.jsonl"));
    let mut contents = format!(
        "{}\n",
        cc_session_start_line(parent_uuid, "/Users/me/bigdeleg")
    );
    let backlog_line = format!(
        "{}\n",
        cc_tool_use_line(
            parent_uuid,
            "/Users/me/bigdeleg",
            "tu_backlog",
            "Bash",
            serde_json::json!({"command": "ls"}),
        )
    );
    while contents.len() <= (1usize << 20) + 4096 {
        contents.push_str(&backlog_line);
    }
    contents.push_str(&format!(
        "{}\n",
        cc_tool_use_line(
            parent_uuid,
            "/Users/me/bigdeleg",
            "tu_task",
            "Agent",
            serde_json::json!({
                "description": "explore", "subagent_type": "code-explorer", "prompt": "go"
            }),
        )
    ));
    tokio::fs::write(&parent_path, contents.as_bytes())
        .await
        .unwrap();
    write_lines(
        &sub_dir.join("agent-b1.jsonl"),
        &[cc_subagent_line("agent-b1", "/Users/me/bigdeleg", "tu_s")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let expected_parent = AgentId::from_parts("claude-code", parent_uuid);
    let is_parent_start = |e: &AgentEvent| {
        matches!(e, AgentEvent::SessionStart { agent_id, parent_id: None, .. }
            if *agent_id == expected_parent)
    };
    let is_sub_start = |e: &AgentEvent| {
        matches!(
            e,
            AgentEvent::SessionStart {
                parent_id: Some(_),
                ..
            }
        )
    };
    let is_task_start = |e: &AgentEvent| {
        matches!(e, AgentEvent::ActivityStart { agent_id, tool_use_id, detail }
            if *agent_id == expected_parent
                && tool_use_id.as_deref() == Some("tu_task")
                && detail.as_ref().is_some_and(|d| d.is_task()))
    };

    let mut events: Vec<(Transport, AgentEvent)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(pair)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            events.push(pair);
        }
        if events.iter().any(|(_, e)| is_parent_start(e))
            && events.iter().any(|(_, e)| is_sub_start(e))
            && events.iter().any(|(_, e)| is_task_start(e))
        {
            break;
        }
    }
    // Settle past the 250ms rescan + several poll cycles, then drain: the
    // scan runs only on the oversized-skip pass, so re-scans (cursor parked
    // at EOF) must not re-emit the dispatch.
    tokio::time::sleep(Duration::from_millis(700)).await;
    while let Ok(Some(pair)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(pair);
    }

    assert!(
        events.iter().any(|(_, e)| is_parent_start(e)),
        "the oversized delegating parent must register at attach (#204 head read), got {events:?}"
    );
    let sub_parents: Vec<Option<AgentId>> = events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::SessionStart {
                parent_id: Some(pid),
                ..
            } => Some(Some(*pid)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sub_parents,
        vec![Some(expected_parent)],
        "the fresh subagent must attach linked to the parent, exactly once"
    );
    let task_starts: Vec<Transport> = events
        .iter()
        .filter(|(_, e)| is_task_start(e))
        .map(|(t, _)| *t)
        .collect();
    assert_eq!(
        task_starts,
        vec![Transport::Jsonl],
        "the in-flight dispatch must be re-emitted exactly once, Jsonl-tagged (it passes the hook-wins dedup at mid-attach)"
    );
    let backlog_replays = events
        .iter()
        .filter(|(_, e)| {
            matches!(e, AgentEvent::ActivityStart { tool_use_id, .. }
                if tool_use_id.as_deref() == Some("tu_backlog"))
        })
        .count();
    assert_eq!(
        backlog_replays, 0,
        "the completed Bash backlog must not be replayed — this is a Task-seeding scan, not a replay"
    );
    handle.abort();
}

/// S4 — the #203 identity property pinned for the ATTACH path: a worktree
/// cwd-split puts the parent transcript and the subagent transcript under
/// DIFFERENT project dirs. At attach time the parent is stale-but-probe-live
/// and the subagent is fresh; both must register, and the `<parent-uuid>`
/// join must hold across the project dirs.
#[tokio::test]
async fn cwd_split_attach_links_subagent_to_probe_live_stale_parent() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let parent_uuid = "ff000000-0000-7000-8000-00000000000f";

    let proj_a = projects_root.join("-Users-me-wt-main");
    tokio::fs::create_dir_all(&proj_a).await.unwrap();
    let parent_path = proj_a.join(format!("{parent_uuid}.jsonl"));
    write_lines(
        &parent_path,
        &[cc_session_start_line(parent_uuid, "/Users/me/wt-main")],
    )
    .await;
    backdate(&parent_path, 7200);

    let sub_dir = projects_root
        .join("-Users-me-wt-feature")
        .join(parent_uuid)
        .join("subagents");
    tokio::fs::create_dir_all(&sub_dir).await.unwrap();
    write_lines(
        &sub_dir.join("agent-w1.jsonl"),
        &[cc_subagent_line("agent-w1", "/Users/me/wt-feature", "tu_w")],
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone())
        .with_initial_window(Duration::from_secs(60))
        .with_liveness_probe(std::sync::Arc::new(move || vouch_snapshot(&[parent_uuid])));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut parent_agent_id: Option<AgentId> = None;
    let mut sub_parent_id: Option<AgentId> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            match parent_id {
                Some(pid) => sub_parent_id = Some(pid),
                None => parent_agent_id = Some(agent_id),
            }
        }
        if parent_agent_id.is_some() && sub_parent_id.is_some() {
            break;
        }
    }
    let parent_agent_id =
        parent_agent_id.expect("the stale probe-live parent must register at attach");
    let sub_parent_id = sub_parent_id.expect("the fresh subagent must register at attach");
    assert_eq!(
        parent_agent_id,
        AgentId::from_parts("claude-code", parent_uuid),
        "the parent registers on its session UUID"
    );
    assert_eq!(
        sub_parent_id, parent_agent_id,
        "the UUID join must link the subagent to the parent across project dirs (a worktree cwd-split) on the attach path"
    );
    handle.abort();
}
