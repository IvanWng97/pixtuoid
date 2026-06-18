#![cfg(unix)]
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    // Open a connection and hold it without sending anything past the 1s
    // CONN_TIMEOUT, then observe the drop DIRECTLY: once the server task
    // times out and drops its end, a read on the client side completes with
    // EOF (Ok(0)). Without this read the test passes even with CONN_TIMEOUT
    // deleted — the per-connection semaphore alone keeps the accept loop
    // serving a second connection.
    let mut slow = UnixStream::connect(&path).await.unwrap();
    sleep(Duration::from_millis(1_200)).await;
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_millis(500), slow.read(&mut buf))
        .await
        .expect("read must complete promptly — CONN_TIMEOUT should have dropped the slow conn")
        .expect("a server-dropped unix conn reads EOF, not an error");
    assert_eq!(n, 0, "server must have closed the slow connection");

    // And the listener is still serving after the drop.
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
// daemon (an unconditional unlink would leave the first instance accepting on
// an anonymous inode forever, with every hook-borne signal vanishing). The
// live owner holds the exclusive lock on the sibling `<sock>.lock`, so the
// second bind's try-lock fails and it returns the typed SocketBusy naming the
// path — which HookRouter::run downcasts to degrade the hook plane to a quiet
// Ok(()) (no SourceDeath; see hook_router_socket_busy_exits_clean_without_death).
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
    assert!(
        err.downcast_ref::<pixtuoid_core::source::hook::SocketBusy>()
            .is_some(),
        "the busy bind must be the typed SocketBusy so the source can degrade: {err:#}"
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("another pixtuoid instance"),
        "error must say what is wrong: {msg}"
    );
    assert!(
        msg.contains(&path.display().to_string()),
        "error must name the contended path: {msg}"
    );

    // A bind attempt against a live owner must be side-effect-free: the
    // try-lock fails before any probe connect or unlink, so the owner's
    // socket keeps serving untouched.
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
// life — the released `<sock>.lock` (the kernel drops the advisory lock with
// the owning process) is what distinguishes stale from live, NOT connect()
// errnos (a backlog-saturated LIVE daemon also yields ECONNREFUSED on macOS).
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

    // The lock sibling is the liveness arbiter — if it regressed to a
    // umask-default mode, another local user could open+flock it and force
    // every future daemon into silent transcript-only degradation.
    let lock_mode = std::fs::metadata(dir.path().join("pixtuoid.sock.lock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(lock_mode, 0o600, "lock file must be owner-only rw (0600)");

    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "pixtuoid.sock".to_string(),
            // The liveness-arbitration lock file is a deliberate, permanent
            // sibling (never unlinked — unlink races re-introduce the TOCTOU
            // it exists to close); only the `.tmp` bind name must be gone.
            "pixtuoid.sock.lock".to_string(),
        ],
        "the temp-name bind must leave nothing but the final socket + its lock"
    );
}

// The lock — not connect() errnos — is the liveness arbiter: with the owner
// LIVE (lock held) but its socket file gone (so any connect probe would see
// NotFound, the old "stale" verdict), a second bind must still yield the
// typed SocketBusy instead of binding over the live owner's path.
#[tokio::test]
async fn bind_respects_lock_arbitration_even_without_a_socket_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");
    let _owner = HookSocketListener::bind(path.clone()).await.unwrap();
    std::fs::remove_file(&path).unwrap();

    let err = HookSocketListener::bind(path.clone())
        .await
        .err()
        .expect("a live lock-holder must make a second bind fail, socket file or not");
    assert!(
        err.downcast_ref::<pixtuoid_core::source::hook::SocketBusy>()
            .is_some(),
        "expected the typed SocketBusy: {err:#}"
    );
}

// The post-lock belt-and-braces probe (unix.rs lines 86-97): a LOCK-LESS live
// owner — an older pixtuoid mid-upgrade, or a squatter — holds the socket open
// but never acquired the `<sock>.lock` sibling. A fresh bind THEN acquires that
// free lock (so the lock arbiter alone would say "stale, reclaim"), but the
// socket file still exists, so it connect-probes before reclaiming: a SUCCESSFUL
// connect (`Ok(_stream) => true`) proves a live owner and bind defers via
// SocketBusy rather than stealing. This is the ONLY busy path that reaches the
// connect probe — every other busy test trips the lock try-lock first.
#[tokio::test]
async fn bind_defers_to_a_lockless_live_listener_via_the_connect_probe() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pixtuoid.sock");

    // A RAW tokio listener creates the socket file and NOTHING else — crucially
    // no `.lock` sibling — emulating a lock-protocol-unaware live owner. It need
    // never call accept(): the OS backlogs (or accepts) the probe connect.
    let _raw = tokio::net::UnixListener::bind(&path).unwrap();
    assert!(
        !dir.path().join("pixtuoid.sock.lock").exists(),
        "premise: a raw listener creates no lock sibling, so bind will acquire the lock freely"
    );

    let err = HookSocketListener::bind(path.clone())
        .await
        .err()
        .expect("a lock-less but LIVE listener must be deferred to, not reclaimed");
    assert!(
        err.downcast_ref::<pixtuoid_core::source::hook::SocketBusy>()
            .is_some(),
        "the connect-probe-detected live owner must defer via the typed SocketBusy: {err:#}"
    );

    // The probe must defer, never unlink: the raw owner's socket file survives.
    // (A regression flipping `Ok(_stream) => true` to `false` would make bind
    // reclaim — remove_file then bind — yielding Ok and a different inode.)
    assert!(
        path.exists(),
        "the deferring probe must NOT unlink the live owner's socket file"
    );
    let mut s = UnixStream::connect(&path).await.unwrap();
    s.shutdown().await.unwrap();
    drop(_raw);
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

// HookRouter is the socket's honest owner (it lifts off ClaudeCodeSource). The
// SocketBusy degradation under the split: a SECOND instance whose hook bind
// loses to a live daemon takes ONLY the hook plane down — `run` returns Ok(())
// with NO SourceDeath (the transcript sources run as independent tasks). This
// is the negative complement of `fatal_source_exit_is_published_on_the_health_
// channel` (manager.rs): the GRACEFUL path must push nothing. Pinned via
// `spawn_with_health` so a regression to `Err(SocketBusy)` would spuriously
// fire the #157 footer death every time a 2nd instance launches.
#[tokio::test]
async fn hook_router_socket_busy_exits_clean_without_death() {
    use pixtuoid_core::source::hook::HookRouter;
    use pixtuoid_core::source::manager::{SourceDeath, SourceManager};

    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("pixtuoid.sock");
    // The "first instance": occupy the socket and keep it alive.
    let _owner = HookSocketListener::bind(sock.clone()).await.unwrap();

    let (tx, _rx) = mpsc::channel::<(Transport, AgentEvent)>(8);
    let (deaths_tx, deaths_rx) = tokio::sync::watch::channel(Vec::<SourceDeath>::new());
    let handles = SourceManager::new()
        .with_source(Box::new(HookRouter::new(sock)))
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

// The #246 tee now rides HookRouter (its honest owner): a SubagentStop on the
// shared socket must reach the downstream channel unchanged (Hook-tagged
// `as_child` SessionEnd) AND land its child id in the
// shared un-claim handle. Codex-stamped — the motivating #246 case, proving the
// router feeds EVERY source's child ends into the ONE handle.
#[tokio::test]
async fn hook_router_tee_captures_child_ends_from_the_shared_socket() {
    use pixtuoid_core::source::hook::HookRouter;
    use pixtuoid_core::source::jsonl::ChildEndUnclaims;
    use pixtuoid_core::source::Source;
    use pixtuoid_core::AgentId;

    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("pixtuoid.sock");

    let unclaims = ChildEndUnclaims::new();
    let router = HookRouter::new(sock.clone()).with_child_end_unclaims(Some(unclaims.clone()));
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let task = tokio::spawn(async move { Box::new(router).run(tx).await });
    sleep(Duration::from_millis(50)).await;

    let child_uuid = "0d000000-0000-7000-8000-0000000000d1";
    let expected = AgentId::from_parts("codex", child_uuid);
    let payload = serde_json::json!({
        "hook_event_name": "SubagentStop",
        "session_id": "parent-sess",
        "agent_id": child_uuid,
        "_pixtuoid_source": "codex",
    });
    let mut s = UnixStream::connect(&sock).await.unwrap();
    let mut line = serde_json::to_vec(&payload).unwrap();
    line.push(b'\n');
    s.write_all(&line).await.unwrap();
    s.shutdown().await.unwrap();

    let (transport, ev) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (transport, ev) = rx.recv().await.expect("router must stay alive");
            if matches!(ev, AgentEvent::SessionEnd { .. }) {
                return (transport, ev);
            }
        }
    })
    .await
    .expect("the SubagentStop must reach the downstream channel through the tee");
    assert_eq!(
        transport,
        Transport::Hook,
        "the Transport tag flows through"
    );
    assert_eq!(
        ev,
        AgentEvent::SessionEnd {
            agent_id: expected,
            as_child: true
        },
        "event parity: the decoded end is forwarded unchanged"
    );
    assert_eq!(
        unclaims.take_matching(|id| *id == expected),
        vec![expected],
        "the child id must land in the shared un-claim handle"
    );
    task.abort();
}
