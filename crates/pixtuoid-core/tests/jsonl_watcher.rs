use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::{
    cc_derive_label, cc_session_ended, decode_cc_line, ClaudeCodeSource,
};
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::jsonl::JsonlWatcher;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Source;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

fn cc_watcher(root: std::path::PathBuf) -> JsonlWatcher {
    JsonlWatcher::new(
        root,
        "claude-code".to_string(),
        decode_cc_line,
        cc_derive_label,
        cc_session_ended,
    )
}

#[tokio::test]
async fn watcher_emits_session_start_then_activity_for_tool_use() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    let assistant_line = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    f.write_all(format!("{assistant_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let mut got_activity = false;
    let mut start_transport = Transport::Hook;
    let mut activity_transport = Transport::Hook;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((t, AgentEvent::SessionStart { .. }))) => {
                got_start = true;
                start_transport = t;
            }
            Ok(Some((t, AgentEvent::ActivityStart { .. }))) => {
                got_activity = true;
                activity_transport = t;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        if got_start && got_activity {
            break;
        }
    }
    assert!(got_start, "expected SessionStart from JSONL watcher");
    assert!(got_activity, "expected ActivityStart from JSONL watcher");
    assert_eq!(start_transport, Transport::Jsonl);
    assert_eq!(activity_transport, Transport::Jsonl);
    handle.abort();
}

#[tokio::test]
async fn watcher_does_not_consume_partial_trailing_line() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // First write: a complete line + a partial line (no trailing \n).
    let complete = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let partial_head = r#"{"type":"assistant","sessionId":"ses-abc","cwd":"/repo","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_2","name":"Read","input":{"file_path":"/etc/host"#;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{complete}\n{partial_head}").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // We should see the SessionStart + ActivityStart for tu_1, but NOT for tu_2.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut seen_tool_use_ids: Vec<String> = Vec::new();
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart {
            tool_use_id: Some(id),
            ..
        } = ev
        {
            seen_tool_use_ids.push(id);
        }
    }
    assert!(
        seen_tool_use_ids.contains(&"tu_1".to_string()),
        "expected tu_1 from complete line, got {seen_tool_use_ids:?}"
    );
    assert!(
        !seen_tool_use_ids.contains(&"tu_2".to_string()),
        "tu_2 came from a partial line and must not be emitted yet"
    );

    // Now complete tu_2 by appending the rest of the line. tu_2 should appear.
    let partial_tail = "s\"}}]}}\n";
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(partial_tail.as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut got_tu_2 = false;
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart { tool_use_id, .. } = ev {
            if tool_use_id.as_deref() == Some("tu_2") {
                got_tu_2 = true;
            }
        }
    }
    assert!(
        got_tu_2,
        "tu_2 should appear after partial line is completed"
    );

    handle.abort();
}

/// On startup, the watcher must NOT emit SessionStart for every historical
/// .jsonl on disk. With small `max_desks` this would saturate desks with
/// long-dead sessions and starve the user's currently-active session.
/// Files older than the initial-window are seeded with cursor=file_len and
/// left out of the SessionStart stream until they next get written to.
#[tokio::test]
async fn watcher_skips_session_start_for_stale_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-stale");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    // Pre-existing stale transcript (mtime backdated 1 hour).
    let stale = project_dir.join("old.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "old",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_old", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    tokio::fs::write(&stale, format!("{line}\n")).await.unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&stale, backdated).unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the initial scan a moment to run.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    assert!(
        events.is_empty(),
        "stale file must not produce events on startup, got {events:?}"
    );
    handle.abort();
}

/// Conversely, a transcript whose mtime is *within* the initial-window is
/// treated as live: its SessionStart and any historical content replays so
/// in-flight Task / tool state survives a pixtuoid restart.
#[tokio::test]
async fn watcher_emits_session_start_for_recent_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-fresh");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let fresh = project_dir.join("fresh.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "fresh",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_fresh", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    std::fs::write(&fresh, format!("{line}\n")).unwrap();
    // fsync the parent directory so the directory entry is guaranteed visible
    // to read_dir — without this, APFS metadata propagation can race with
    // the watcher's initial seed walk under heavy concurrent I/O.
    std::fs::File::open(&project_dir)
        .unwrap()
        .sync_all()
        .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let fresh_path = fresh.clone();
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the watcher task a chance to complete the initial seed scan, then
    // append a no-op newline to trigger FSEvents as a fallback path in case
    // the initial seed missed the file under heavy I/O contention.
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&fresh_path)
        .await
        .unwrap()
        .sync_all()
        .await
        .unwrap();

    let mut got_start = false;
    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { .. }))) => got_start = true,
            Ok(Some((_, AgentEvent::ActivityStart { .. }))) => got_activity = true,
            _ => {}
        }
        if got_start && got_activity {
            break;
        }
    }
    assert!(got_start, "fresh file should produce SessionStart");
    assert!(got_activity, "fresh file content should be replayed");
    handle.abort();
}

/// First-sight cwd extraction must scan past unparsable prefix lines.
/// `extract_cwd` previously short-circuited via `?` on the first non-JSON
/// (or non-UTF8) line, even if a later line carried the `cwd` field.
#[tokio::test]
async fn first_sight_extracts_cwd_past_non_json_prefix() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-cwd");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cwd.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // First line: garbage. Second line: a system line carrying cwd. Watcher
    // should still derive cwd = /real-repo on the SessionStart for first-sight.
    //
    // The watcher emits SessionStart exactly ONCE per file, with cwd taken from
    // whatever bytes are present at first read. Writing the lines incrementally
    // (or even create-then-write) leaves a window where the 250ms poll observes
    // a partial/empty file, latches cwd="" permanently, and fails this test
    // (flaky under load / coverage instrumentation). Stage the complete content
    // in a sibling `.partial` file — excluded by the watcher's `.jsonl`
    // extension filter — then atomically rename it into place so first sight
    // always reads the full content.
    let sys_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-cwd",
        "cwd": "/real-repo"
    });
    let content = format!("not-json-prefix\n{sys_line}\n");
    let staging = project_dir.join("ses-cwd.jsonl.partial");
    tokio::fs::write(&staging, content.as_bytes())
        .await
        .unwrap();
    tokio::fs::rename(&staging, &transcript).await.unwrap();

    let mut found_cwd = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { cwd, .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            found_cwd = Some(cwd);
            break;
        }
    }
    assert_eq!(
        found_cwd,
        Some(std::path::PathBuf::from("/real-repo")),
        "extract_cwd must scan past non-JSON lines to find cwd"
    );
    handle.abort();
}

/// Stale files become live as soon as CC writes to them — the next notify
/// event must produce a SessionStart, since the file is now active.
#[tokio::test]
async fn stale_file_emits_session_start_when_written_to() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-revive");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let revived = project_dir.join("revive.jsonl");
    tokio::fs::write(&revived, "{}\n").await.unwrap();
    set_file_mtime(
        &revived,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600)),
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // No SessionStart yet (stale + skipped).
    while tokio::time::timeout(Duration::from_millis(20), rx.recv())
        .await
        .is_ok()
    {}

    // Append a real assistant tool_use line — file is now live.
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "revive",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_new", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&revived)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            got_start = true;
            break;
        }
    }
    assert!(got_start, "appending to a stale file should bring it live");
    handle.abort();
}

/// A recent file (within the initial window) that has a session_end marker
/// at its tail must NOT produce a SessionStart on startup — the watcher
/// must detect the ended session and seed the cursor at EOF.
#[tokio::test]
async fn watcher_skips_recent_file_with_session_end_marker() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-ended");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let ended = project_dir.join("ended.jsonl");
    let content = r#"{"type":"system","subtype":"session_start","sessionId":"ended","cwd":"/repo"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"system","subtype":"session_end","sessionId":"ended"}
"#;
    tokio::fs::write(&ended, content).await.unwrap();
    // mtime is "now" — well within the initial window.

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    let has_session_start = events
        .iter()
        .any(|(_, ev)| matches!(ev, AgentEvent::SessionStart { .. }));
    assert!(
        !has_session_start,
        "recent file with session_end marker must not produce SessionStart, got {events:?}"
    );
    handle.abort();
}

fn custom_label(_path: &std::path::Path, _source: &str, _cwd: &std::path::Path) -> String {
    "custom-label-ok".to_string()
}

#[tokio::test]
async fn watcher_custom_label_deriver() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-xyz.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        projects_root.clone(),
        "claude-code".to_string(),
        decode_cc_line,
        custom_label,
        cc_session_ended,
    );
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-xyz",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_custom_rename = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::Rename { label, .. }))) => {
                if label == "custom-label-ok" {
                    got_custom_rename = true;
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(
        got_custom_rename,
        "expected Rename event with custom label from label deriver fn"
    );
    handle.abort();
}

#[tokio::test]
async fn codex_rollout_yields_uuid_keyed_session_start() {
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path);
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/Users/me/dotfiles" }
    });
    f.write_all(format!("{meta}\n").as_bytes()).await.unwrap();
    let task_started = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t" }
    });
    f.write_all(format!("{task_started}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let expected = AgentId::from_parts("codex", uuid);
    let mut saw_session_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_eq!(agent_id, expected, "Codex SessionStart must be UUID-keyed");
                saw_session_start = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(saw_session_start, "expected a SessionStart event");
    handle.abort();
}

#[tokio::test]
async fn default_id_deriver_stays_path_keyed() {
    // Pin the IdDeriver default: a non-Codex watcher must key on the file path
    // (so CC/Antigravity hook↔JSONL coalescing is unchanged).
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let project_dir = root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // The default deriver keys on the file path. The watcher may report the
    // raw TempDir path (rescan via read_dir) or the symlink-resolved path
    // (macOS FSEvents canonicalizes /var → /private/var), so accept either —
    // both are path-keyed. What must NOT match is a UUID/stem key.
    let raw = AgentId::from_parts("claude-code", &transcript.to_string_lossy());
    let canon = std::fs::canonicalize(&transcript)
        .map(|p| AgentId::from_parts("claude-code", &p.to_string_lossy()))
        .unwrap_or(raw);
    let stem_keyed = AgentId::from_parts("claude-code", "abc");
    let mut ok = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_ne!(
                    agent_id, stem_keyed,
                    "default deriver must be path-keyed, not stem-keyed"
                );
                assert!(
                    agent_id == raw || agent_id == canon,
                    "default deriver must key on the file path (raw or canonical)"
                );
                ok = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(ok, "expected a path-keyed SessionStart");
    handle.abort();
}

// CodexSource::run is just `JsonlWatcher::new(...).run(tx)` — drive the real
// Source impl against a TempDir sessions_root so its run()-glue is exercised
// (not only the watcher internals). A rollout file with a task_started line must
// surface an ActivityStart through the source.
#[tokio::test]
async fn codex_source_run_emits_events_from_rollout() {
    let dir = TempDir::new().unwrap();
    let sessions_root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = sessions_root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = CodexSource { sessions_root };
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
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("pixtuoid-test.sock");
    let projects_root = dir.path().join("projects");
    let project_dir = projects_root.join("proj-cc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let src = ClaudeCodeSource {
        socket_path,
        projects_root,
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

// Cursor-safety guard: a transcript truncated below the watcher's stored cursor
// must reset the cursor (not stay stuck) so newly-appended content re-decodes.
#[tokio::test]
async fn watcher_resets_cursor_on_truncation_below_cursor() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-trunc");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("trunc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let tool_line = |id: &str| {
        serde_json::json!({
            "type": "assistant",
            "sessionId": "trunc",
            "cwd": "/repo",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": id, "name": "Bash", "input": { "command": "ls" } }
                ]
            }
        })
        .to_string()
    };

    // Write a long first line so the cursor advances well past a later short one.
    let long = tool_line("tu_long") + &" ".repeat(400);
    tokio::fs::write(&transcript, format!("{long}\n"))
        .await
        .unwrap();

    // Let the watcher advance its cursor to EOF.
    let mut saw_long = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_long") {
                saw_long = true;
                break;
            }
        }
    }
    assert!(saw_long, "expected the first long line to decode");

    // Truncate the file far below the stored cursor, then append a fresh line.
    let fresh = tool_line("tu_fresh");
    tokio::fs::write(&transcript, format!("{fresh}\n"))
        .await
        .unwrap();

    // The cursor (set past the long line) now exceeds file_len → reset to 0 →
    // the fresh line re-decodes on the next scan.
    let mut saw_fresh = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_fresh") {
                saw_fresh = true;
                break;
            }
        }
    }
    assert!(
        saw_fresh,
        "after truncation the cursor must reset so the fresh line decodes"
    );
    handle.abort();
}

// Cursor-safety guard: a > 1 MiB pending tail with no newline must be skipped to
// EOF (not buffered), and a later newline-terminated valid line still decodes.
#[tokio::test]
async fn watcher_skips_oversized_pending_tail() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-big");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("big.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write > 1 MiB of junk with NO newline — file_len - cursor exceeds
    // MAX_PENDING_BYTES, so the watcher seeks the cursor to EOF and skips it.
    let junk = vec![b'x'; (1 << 20) + 1024];
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(&junk).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    // Give the watcher a scan to skip the junk.
    tokio::time::sleep(Duration::from_millis(400)).await;
    while tokio::time::timeout(Duration::from_millis(20), rx.recv())
        .await
        .is_ok()
    {}

    // Append a newline (closing the junk line) plus a valid line. The junk line
    // is past the EOF-seeked cursor, so only the valid line decodes.
    let valid = serde_json::json!({
        "type": "assistant",
        "sessionId": "big",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_after_junk", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("\n{valid}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_after = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_after_junk") {
                got_after = true;
                break;
            }
        }
    }
    assert!(
        got_after,
        "the post-skip valid line must decode after the oversized tail is skipped"
    );
    handle.abort();
}

// The per-line non-UTF8 guard in walk_jsonl: a raw invalid-UTF8 byte line is
// warn-and-skipped, and a following valid JSON line still decodes (the bad line
// is not fatal to the rest of the read).
#[tokio::test]
async fn watcher_skips_non_utf8_line_and_keeps_going() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-nonutf8");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("nonutf8.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let valid = serde_json::json!({
        "type": "assistant",
        "sessionId": "nonutf8",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_valid", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    // Invalid-UTF8 bytes + newline, then a valid JSON line + newline. The bytes
    // can't go through serde_json (JSON is UTF-8) — write them raw.
    let mut bytes: Vec<u8> = vec![0xff, 0xfe, b'\n'];
    bytes.extend_from_slice(format!("{valid}\n").as_bytes());
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(&bytes).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_valid = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::ActivityStart { tool_use_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if tool_use_id.as_deref() == Some("tu_valid") {
                got_valid = true;
                break;
            }
        }
    }
    assert!(
        got_valid,
        "a non-UTF8 line must be skipped, not block the following valid line"
    );
    handle.abort();
}

// Drives detect_parent_id through the REAL watcher recursion: a subagent
// transcript at <root>/proj/parent/subagents/agent-1.jsonl must emit a
// SessionStart whose parent_id derives the parent from the grandparent dir.
#[tokio::test]
async fn watcher_derives_parent_id_for_subagent_path() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let subagent_dir = projects_root.join("proj").join("parent").join("subagents");
    tokio::fs::create_dir_all(&subagent_dir).await.unwrap();
    let transcript = subagent_dir.join("agent-1.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "agent-1",
        "cwd": "/repo",
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Read", "input": { "file_path": "/x" } }
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

    // The watcher reports either the raw or canonicalized root (macOS /var →
    // /private/var), so accept either parent key.
    let parent_path = projects_root.join("proj").join("parent.jsonl");
    let raw = AgentId::from_parts("claude-code", &parent_path.to_string_lossy());
    let canon_root = std::fs::canonicalize(&projects_root).unwrap_or(projects_root.clone());
    let canon = AgentId::from_parts(
        "claude-code",
        &canon_root
            .join("proj")
            .join("parent.jsonl")
            .to_string_lossy(),
    );

    let mut found_parent = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((
            _,
            AgentEvent::SessionStart {
                parent_id: Some(pid),
                ..
            },
        ))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            found_parent = Some(pid);
            break;
        }
    }
    let found = found_parent.expect("expected a SessionStart carrying parent_id");
    assert!(
        found == raw || found == canon,
        "parent_id must derive the parent transcript from the grandparent dir; got {found:?}"
    );
    handle.abort();
}
