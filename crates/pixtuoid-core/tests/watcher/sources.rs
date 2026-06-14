use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::copilot::CopilotSource;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Source;
use pixtuoid_core::source::Transport;

use crate::fast_watch;

// CodexSource::run is just `JsonlWatcher::new(...).run(tx)` — drive the real
// Source impl against a TempDir sessions_root so its run()-glue is exercised
// (not only the watcher internals). A rollout file with a task_started line must
// surface an ActivityStart through the source.
#[tokio::test]
async fn codex_source_run_emits_events_from_rollout() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = sessions_root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = CodexSource {
        sessions_root,
        child_end_unclaims: None,
    };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/repo" }
    });
    let task_started = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t" }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{meta}\n{task_started}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_activity = true;
            break;
        }
    }
    assert!(
        got_activity,
        "CodexSource::run should surface ActivityStart"
    );
    handle.abort();
}

// AntigravitySource::run mirrors CodexSource::run — drive the real Source impl
// against a TempDir brain_root.
#[tokio::test]
async fn antigravity_source_run_emits_events_from_transcript() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let brain_root = dir.path().to_path_buf();
    let project_dir = brain_root.join("sess");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("transcript.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = AntigravitySource { brain_root };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let planner = serde_json::json!({
        "step_index": 1,
        "cwd": "/repo",
        "type": "PLANNER_RESPONSE",
        "tool_calls": [ { "name": "list_dir", "args": { "DirectoryPath": "\"/repo/src\"" } } ]
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{planner}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_activity = true;
            break;
        }
    }
    assert!(
        got_activity,
        "AntigravitySource::run should surface ActivityStart"
    );
    handle.abort();
}

// ClaudeCodeSource::run binds the hook socket, spawns the watcher, and enters
// the select! — drive the real Source impl so the bind + spawn + select-entry
// glue is exercised (only the select abort/warn arms stay structurally
// unreachable: both inner tasks loop forever). A CC transcript written under
// the projects_root must surface a SessionStart through the JSONL leg.
#[tokio::test]
async fn claude_code_source_run_binds_socket_and_emits_events() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    // The hook endpoint must be platform-shaped: a filesystem path is an
    // invalid pipe name on Windows and would fail run()'s bind before the
    // JSONL leg (the thing under test) ever starts.
    #[cfg(unix)]
    let socket_path = dir.path().join("pixtuoid-test.sock");
    #[cfg(windows)]
    let socket_path = std::path::PathBuf::from(format!(
        r"\\.\pipe\pixtuoid-test-jsonlw-{}",
        std::process::id()
    ));
    let projects_root = dir.path().join("projects");
    let project_dir = projects_root.join("proj-cc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = ClaudeCodeSource {
        socket_path,
        projects_root,
        child_end_unclaims: None,
    };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-cc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            got_start = true;
            break;
        }
    }
    assert!(
        got_start,
        "ClaudeCodeSource::run should surface SessionStart from the JSONL leg"
    );
    handle.abort();
}

// CopilotSource::run drives a JsonlWatcher over <sessions_root>/<id>/events.jsonl
// with the parent-dir id-deriver. The e2e gap that unit tests can't cover: does
// the real watcher RECURSE into the <sessionId>/ dir, pick up the constant-named
// `events.jsonl`, derive the id from the PARENT DIR (not the "events" stem), and
// surface a copilot SessionStart? Driven against real-shaped bytes.
#[tokio::test]
async fn copilot_source_run_emits_session_start_from_events_jsonl() {
    fast_watch();
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let session_dir = sessions_root.join("65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3");
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    let transcript = session_dir.join("events.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = CopilotSource { sessions_root };
    let handle = tokio::spawn(async move { Box::new(src).run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Real session.start shape + a tool round (single agent → one AgentId).
    let start = serde_json::json!({
        "type": "session.start",
        "data": {"sessionId": "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3", "version": 1,
                 "producer": "copilot-agent", "context": {"cwd": "/repo"}},
        "id": "a", "timestamp": "2026-05-22T05:59:45.488Z", "parentId": serde_json::Value::Null
    });
    let tool = serde_json::json!({
        "type": "tool.execution_start",
        "data": {"toolCallId": "tooluse_1", "toolName": "view", "arguments": {"path": "/repo"}},
        "id": "b", "timestamp": "2026-05-22T06:00:14.298Z", "parentId": "a"
    });
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{start}\n{tool}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut session_id = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                source,
                session_id: sid,
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            assert_eq!(source, "copilot");
            session_id = Some(sid);
            break;
        }
    }
    assert_eq!(
        session_id.as_deref(),
        Some("65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3"),
        "CopilotSource::run should surface a copilot SessionStart from events.jsonl"
    );
    handle.abort();
}
