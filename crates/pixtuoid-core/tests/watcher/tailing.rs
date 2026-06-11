use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Transport;

use crate::cc_watcher;

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

    let mut start_id = None;
    let mut activity_id = None;
    let mut start_transport = Transport::Hook;
    let mut activity_transport = Transport::Hook;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((t, AgentEvent::SessionStart { agent_id, .. }))) => {
                start_id = Some(agent_id);
                start_transport = t;
            }
            Ok(Some((t, AgentEvent::ActivityStart { agent_id, .. }))) => {
                activity_id = Some(agent_id);
                activity_transport = t;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        if start_id.is_some() && activity_id.is_some() {
            break;
        }
    }
    let start_id = start_id.expect("expected SessionStart from JSONL watcher");
    let activity_id = activity_id.expect("expected ActivityStart from JSONL watcher");
    // The SessionStart key (id_derive) and the per-line decode key
    // (transcript_path_str) are computed at two different walk_jsonl sites —
    // they must agree or every JSONL event lands on a phantom id (the raw
    // string diverged from the normalized one on the windows runner).
    assert_eq!(
        start_id, activity_id,
        "SessionStart and per-line events must share one AgentId"
    );
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
