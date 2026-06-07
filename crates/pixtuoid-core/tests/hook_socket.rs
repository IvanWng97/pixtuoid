#![cfg(unix)]
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::sleep;

use pixtuoid_core::source::hook::HookSocketListener;
use pixtuoid_core::source::{AgentEvent, Transport};

#[tokio::test]
async fn listener_parses_line_and_emits_event() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });

    sleep(Duration::from_millis(20)).await;

    let mut s = UnixStream::connect(&path).await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "ses-1",
        "transcript_path": "/p/a.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    s.shutdown().await.unwrap();

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionStart { .. }));

    handle.abort();
}

#[tokio::test]
async fn listener_skips_malformed_line_and_keeps_going() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    let mut s = UnixStream::connect(&path).await.unwrap();
    s.write_all(b"not json\n").await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionEnd",
        "session_id": "ses-1",
        "transcript_path": "/p/a.jsonl",
        "cwd": "/repo",
        "reason": "exit"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    s.shutdown().await.unwrap();

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
    handle.abort();
}

#[tokio::test]
async fn listener_drops_slow_connection_via_timeout() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // Open a connection but hold it without sending anything. The 1s
    // CONN_TIMEOUT should drop it. Then send a second valid connection
    // to prove the listener is still alive.
    let _slow = UnixStream::connect(&path).await.unwrap();
    sleep(Duration::from_millis(1_200)).await;

    let mut s = UnixStream::connect(&path).await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-timeout",
        "transcript_path": "/p/b.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    s.shutdown().await.unwrap();

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionStart { .. }));
    handle.abort();
}

#[tokio::test]
async fn listener_path_accessor_returns_bound_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    assert_eq!(listener.path(), path.as_path());
}

// The read-error arm in handle_conn: tokio's Lines::next_line() returns an
// io::Error (InvalidData) on invalid UTF-8, which the listener warns-and-returns
// for that connection WITHOUT killing the accept loop. A second valid connection
// must still produce its event. (The existing malformed-line test sends valid
// UTF-8 that's just bad JSON, hitting the serde warn instead.)
#[tokio::test]
async fn listener_survives_non_utf8_read_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // First connection: invalid UTF-8 bytes → next_line() Err arm fires.
    let mut bad = UnixStream::connect(&path).await.unwrap();
    bad.write_all(&[0xFF, 0xFE, b'\n']).await.unwrap();
    bad.shutdown().await.unwrap();

    // Second connection: a valid payload must still be delivered, proving the
    // accept loop survived the read error.
    let mut s = UnixStream::connect(&path).await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-bad-read",
        "transcript_path": "/p/c.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    s.shutdown().await.unwrap();

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionStart { .. }));
    handle.abort();
}

#[tokio::test]
async fn listener_handles_concurrent_connections() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    let mut handles = Vec::new();
    for i in 0..5 {
        let p = path.clone();
        handles.push(tokio::spawn(async move {
            let mut s = UnixStream::connect(&p).await.unwrap();
            let payload = serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": format!("ses-{i}"),
                "transcript_path": format!("/p/{i}.jsonl"),
                "cwd": "/repo"
            });
            let mut line = serde_json::to_vec(&payload).unwrap();
            line.push(b'\n');
            s.write_all(&line).await.unwrap();
            s.shutdown().await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let mut count = 0;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        count += 1;
        if count == 5 {
            break;
        }
    }
    assert_eq!(
        count, 5,
        "all 5 concurrent connections should produce events"
    );
    handle.abort();
}
