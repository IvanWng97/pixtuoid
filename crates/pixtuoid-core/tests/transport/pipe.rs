#![cfg(windows)]
//! Windows twin of socket.rs: the same listener contract over a real
//! named pipe, plus the accept-loop behaviors only a pipe has (instance
//! recreate on connect error, create-next-before-handoff under concurrency).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc;
use tokio::time::sleep;

use pixtuoid_core::source::hook::HookSocketListener;
use pixtuoid_core::source::{AgentEvent, Transport};

/// Each test gets a unique pipe name to avoid cross-test interference.
/// Format: `\\.\pipe\pixtuoid-test-{pid}-{suffix}`
fn pipe_name(suffix: &str) -> String {
    format!(r"\\.\pipe\pixtuoid-test-{}-{}", std::process::id(), suffix)
}

/// Connect to a named pipe, retrying on ERROR_PIPE_BUSY (os error 231).
///
/// Named pipes require the client to retry when the server is between
/// instances (create-next-before-handoff window). Bounded to ~20 tries
/// at 50 ms intervals (~1 s total).
async fn connect_client(name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
    const MAX_TRIES: u32 = 20;
    for attempt in 0..MAX_TRIES {
        match ClientOptions::new().open(name) {
            Ok(c) => return c,
            Err(e) if e.raw_os_error() == Some(231) => {
                // ERROR_PIPE_BUSY — server is swapping instances; retry
                sleep(Duration::from_millis(50)).await;
            }
            Err(e) if attempt == 0 && e.kind() == std::io::ErrorKind::NotFound => {
                // Listener may not be ready yet; brief back-off on first try
                sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("connect_client({name}) failed: {e}"),
        }
    }
    panic!("connect_client({name}): still ERROR_PIPE_BUSY after {MAX_TRIES} tries");
}

// ── Mirrored cases ────────────────────────────────────────────────────────────

#[tokio::test]
async fn listener_parses_line_and_emits_event() {
    let name = pipe_name("parse");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });

    sleep(Duration::from_millis(20)).await;

    let mut c = connect_client(&name).await;
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "ses-1",
        "transcript_path": "/p/a.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    c.write_all(&line).await.unwrap();
    drop(c);

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
    let name = pipe_name("malformed");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    let mut c = connect_client(&name).await;
    c.write_all(b"not json\n").await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionEnd",
        "session_id": "ses-1",
        "transcript_path": "/p/a.jsonl",
        "cwd": "/repo",
        "reason": "exit"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    c.write_all(&line).await.unwrap();
    drop(c);

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
    handle.abort();
}

#[tokio::test]
async fn listener_survives_non_utf8_read_error() {
    let name = pipe_name("nonutf8");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // First connection: invalid UTF-8 → BufReader::lines() Err arm fires.
    let mut bad = connect_client(&name).await;
    bad.write_all(&[0xFF, 0xFE, b'\n']).await.unwrap();
    drop(bad);

    // Second connection: a valid payload must still be delivered.
    let mut c = connect_client(&name).await;
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-bad-read",
        "transcript_path": "/p/c.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    c.write_all(&line).await.unwrap();
    drop(c);

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
    let name = pipe_name("concurrent");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(64);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // 5 concurrent clients — also pins create-next-before-handoff: a handoff
    // gap would cause some clients to get NotFound, failing here.
    let mut handles = Vec::new();
    for i in 0..5usize {
        let n = name.clone();
        handles.push(tokio::spawn(async move {
            let mut c = connect_client(&n).await;
            let payload = serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": format!("ses-{i}"),
                "transcript_path": format!("/p/{i}.jsonl"),
                "cwd": "/repo"
            });
            let mut line = serde_json::to_vec(&payload).unwrap();
            line.push(b'\n');
            c.write_all(&line).await.unwrap();
            drop(c);
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

#[tokio::test]
async fn listener_drops_slow_connection_via_timeout() {
    let name = pipe_name("slowconn");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // Open a connection and hold it without sending anything past the 1s
    // CONN_TIMEOUT, then observe the drop DIRECTLY: once the server task
    // times out and drops its end, a read on the client side completes.
    // Without this read the test passes even with CONN_TIMEOUT deleted — the
    // per-connection semaphore alone keeps the accept loop serving a second
    // connection (the pipe even pre-creates the next instance).
    let mut slow = connect_client(&name).await;
    sleep(Duration::from_millis(1_200)).await;
    let mut buf = [0u8; 1];
    let res = tokio::time::timeout(Duration::from_millis(500), slow.read(&mut buf))
        .await
        .expect("read must complete promptly — CONN_TIMEOUT should have dropped the slow conn");
    // A dropped server end reads as EOF (Ok(0)) or a broken-pipe error
    // depending on how the close lands; both prove the drop — a LIVE server
    // end would park the read past the timeout above.
    match res {
        Ok(0) | Err(_) => {}
        Ok(n) => panic!("unexpected {n} bytes from a dropped connection"),
    }

    // And the listener is still serving after the drop.
    let mut c = connect_client(&name).await;
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-timeout",
        "transcript_path": "/p/b.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    c.write_all(&line).await.unwrap();
    drop(c);

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
    let name = pipe_name("path");
    let path = std::path::PathBuf::from(&name);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    assert_eq!(listener.path(), path.as_path());
}

// ── New cases ─────────────────────────────────────────────────────────────────

/// Open and immediately drop a client 5× in a loop (zero bytes written), then
/// connect a real client and assert the decoded event arrives. Pins the
/// connect-error/instance-recreate path: after each broken-pipe the server must
/// recreate its instance and stay alive for the next connect.
#[tokio::test]
async fn clients_reconnect_after_open_close_churn() {
    let name = pipe_name("churn");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(&name).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    // 5 open-and-drop cycles with zero bytes — the server sees a connect +
    // immediate EOF/broken-pipe on each, triggering its recreate path.
    for _ in 0..5 {
        let _c = connect_client(&name).await;
        // drop immediately — the server gets a broken read
        sleep(Duration::from_millis(10)).await;
    }

    // After all the churn the listener must still be live.
    let mut c = connect_client(&name).await;
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-churn",
        "transcript_path": "/p/churn.jsonl",
        "cwd": "/repo"
    });
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    c.write_all(&line).await.unwrap();
    drop(c);

    let (transport, ev) = tokio::time::timeout(Duration::from_millis(1_000), rx.recv())
        .await
        .expect("timed out waiting for event after churn")
        .unwrap();
    assert_eq!(transport, Transport::Hook);
    assert!(matches!(ev, AgentEvent::SessionStart { .. }));
    handle.abort();
}

/// Binding two listeners on the same pipe name must fail: the first instance
/// held `first_pipe_instance(true)`, so the second attempt gets ACCESS_DENIED
/// — mapped to the typed SocketBusy (Unix parity) so ClaudeCodeSource::run
/// can degrade to transcript-only instead of killing the whole CC source.
#[tokio::test]
async fn second_listener_on_same_name_fails_with_typed_socket_busy() {
    let name = pipe_name("squat");
    let _first = HookSocketListener::bind(&name).await.unwrap();
    let err = HookSocketListener::bind(&name)
        .await
        .err()
        .expect("bind on an already-owned pipe must fail, not silently queue");
    assert!(
        err.downcast_ref::<pixtuoid_core::source::hook::SocketBusy>()
            .is_some(),
        "the busy bind must be the typed SocketBusy so the source can degrade: {err:#}"
    );
    assert!(
        format!("{err:#}").contains(&name),
        "error must name the contended pipe: {err:#}"
    );
}

// HookRouter SocketBusy degradation, Windows twin of socket.rs's
// hook_router_socket_busy_exits_clean_without_death: a 2nd instance whose pipe
// create loses ACCESS_DENIED (mapped to the typed SocketBusy) takes ONLY the
// hook plane down — `run` returns Ok(()) with NO SourceDeath. Pinned via
// spawn_with_health so a regression to `Err(SocketBusy)` would spuriously fire
// the #157 footer death every time a 2nd instance launches.
#[tokio::test]
async fn hook_router_socket_busy_exits_clean_without_death() {
    use pixtuoid_core::source::hook::HookRouter;
    use pixtuoid_core::source::manager::{SourceDeath, SourceManager};

    let name = pipe_name("router-busy");
    // The "first instance": occupy the pipe name and keep it alive.
    let _owner = HookSocketListener::bind(&name).await.unwrap();

    let (tx, _rx) = mpsc::channel::<(Transport, AgentEvent)>(8);
    let (deaths_tx, deaths_rx) = tokio::sync::watch::channel(Vec::<SourceDeath>::new());
    let handles = SourceManager::new()
        .with_source(Box::new(HookRouter::new(std::path::PathBuf::from(&name))))
        .spawn_with_health(tx, deaths_tx);
    for h in handles {
        tokio::time::timeout(Duration::from_secs(10), h)
            .await
            .expect("router must exit promptly on SocketBusy")
            .unwrap();
    }
    assert!(
        deaths_rx.borrow().is_empty(),
        "SocketBusy must degrade quietly (Ok) — no SourceDeath, the hook plane just goes dark"
    );
}
