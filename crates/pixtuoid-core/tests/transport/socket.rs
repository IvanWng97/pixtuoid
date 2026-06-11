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

// A second pixtuoid instance must NOT silently steal the socket from a live
// daemon (the old unconditional unlink left the first instance accepting on
// an anonymous inode forever, with every hook-borne signal vanishing). The
// bind must probe the existing socket and bail loudly, naming the path — the
// error propagates out of Source::run into the #157 SourceDeath channel.
#[tokio::test]
async fn bind_bails_when_a_live_listener_holds_the_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    let err = HookSocketListener::bind(path.clone())
        .await
        .err()
        .expect("a second bind on a LIVE socket must fail loudly, not steal it");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("another pixtuoid instance"),
        "error must say what is wrong: {msg}"
    );
    assert!(
        msg.contains(&path.display().to_string()),
        "error must name the contended path: {msg}"
    );

    // The liveness probe's connect must not have harmed the first listener.
    let mut s = UnixStream::connect(&path).await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-probe",
        "transcript_path": "/p/probe.jsonl",
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

// A crashed daemon's residue must still be reclaimed: neither std nor tokio
// unlink the socket file on listener drop, so the file alone is not proof of
// life — the probe's ConnectionRefused is what distinguishes stale from live.
#[tokio::test]
async fn bind_reclaims_a_stale_socket_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    drop(HookSocketListener::bind(path.clone()).await.unwrap());
    assert!(
        path.exists(),
        "premise: the socket file survives the listener drop (a crash leaves exactly this residue)"
    );

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(16);
    let listener = HookSocketListener::bind(path.clone())
        .await
        .expect("a stale socket file must be reclaimed");
    let handle = tokio::spawn(async move { listener.run(tx).await });
    sleep(Duration::from_millis(20)).await;

    let mut s = UnixStream::connect(&path).await.unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "after-reclaim",
        "transcript_path": "/p/reclaim.jsonl",
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

// The socket must be owner-only 0600 the moment it is reachable at the public
// path (temp-name bind + chmod + atomic rename — no process-global umask
// mutation), and the temp-bind must leave no residue next to it.
#[tokio::test]
async fn bound_socket_is_owner_only_with_no_temp_residue() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let _listener = HookSocketListener::bind(path.clone()).await.unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "hook socket must be owner-only rw (0600)");

    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["pixtuoid.sock".to_string()],
        "the temp-name bind must leave nothing but the final socket"
    );
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

// The sun_path-overflow fallback (final path fits, the `.<pid>.tmp` twin
// doesn't): bind must still succeed via the direct-bind+chmod path and the
// socket must still end up owner-only — pins both the >100 threshold (a
// future edit silently breaking 88-100-byte custom paths fails here) and the
// 0600 mode on the fallback.
#[tokio::test]
async fn long_path_fallback_binds_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    // Pad the FINAL path to exactly 97 bytes: ≤100 (no fallback needed for
    // the final name, and well under sun_path 104), while the temp twin
    // `.{pid}.tmp` adds ≥6 bytes → >100 → must take the fallback branch.
    let base = dir.path().to_string_lossy().len();
    let pad = 97usize
        .checked_sub(base + 1 + ".sock".len())
        .expect("tempdir path too long to stage a 97-byte socket path");
    let name = format!("{}{}", "x".repeat(pad), ".sock");
    let path = dir.path().join(name);
    assert_eq!(path.as_os_str().len(), 97, "fixture: final path length");

    let listener = HookSocketListener::bind(path.clone()).await.unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "fallback-bound socket must be owner-only");
    // And it actually accepts: the shim-visible contract is unchanged.
    drop(listener);
}

// The SocketBusy degradation contract (#232 review): a SECOND instance whose
// hook bind loses to a live daemon must still run its JSONL watcher —
// transcript-only, not dead. A fresh transcript written before spawn must
// produce a SessionStart from the degraded source.
#[tokio::test]
async fn claude_source_degrades_to_transcript_only_when_socket_busy() {
    use pixtuoid_core::source::claude_code::ClaudeCodeSource;
    use pixtuoid_core::source::Source;

    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("pixtuoid.sock");
    // The "first instance": occupy the socket and keep it alive.
    let _owner = HookSocketListener::bind(sock.clone()).await.unwrap();

    let projects = dir.path().join("projects");
    std::fs::create_dir_all(projects.join("proj")).unwrap();
    std::fs::write(
        projects.join("proj/11111111-2222-3333-4444-555555555555.jsonl"),
        "{\"type\":\"user\",\"cwd\":\"/repo\"}\n",
    )
    .unwrap();

    let src = ClaudeCodeSource {
        socket_path: sock,
        projects_root: projects,
    };
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let task = tokio::spawn(async move { Box::new(src).run(tx).await });

    // The initial seed walk must register the fresh transcript even though
    // the hook bind lost — transcript-only, not a dead source.
    let ev = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (transport, ev) = rx.recv().await.expect("source must stay alive");
            if matches!(ev, AgentEvent::SessionStart { .. }) {
                return (transport, ev);
            }
        }
    })
    .await
    .expect("degraded source must still emit the transcript's SessionStart");
    assert_eq!(ev.0, Transport::Jsonl);
    task.abort();
}
