#![cfg(unix)]
//! Integration tests for the hook shim BINARY's I/O contract (invariant #5:
//! "always exit 0, never block CC"). The unit tests in main.rs only cover the
//! pure `enrich_payload`; these spawn the real binary and exercise the
//! connect / write / timeout / exit-0 / malformed-input paths end to end.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pixtuoid-hook");

/// A short, unique socket path under /tmp (stays well under the ~104-byte
/// `sun_path` limit, unlike a deep tempdir). Removed if a stale one exists.
fn sock_path(tag: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(format!(
        "/tmp/pixtuoid-hook-it-{}-{tag}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// Spawn the shim with the given socket path + optional source env, pipe
/// `stdin` to it, and return its exit status (after closing stdin → EOF).
fn run_shim(
    socket: &std::path::Path,
    source: Option<&str>,
    stdin: &[u8],
) -> std::process::ExitStatus {
    let mut cmd = Command::new(BIN);
    cmd.env("PIXTUOID_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match source {
        Some(s) => {
            cmd.env("PIXTUOID_SOURCE", s);
        }
        None => {
            cmd.env_remove("PIXTUOID_SOURCE");
        }
    }
    let mut child = cmd.spawn().expect("spawn shim");
    child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(stdin)
        .expect("write stdin");
    // stdin dropped here → EOF, so the shim's read_to_string returns.
    child.wait().expect("wait shim")
}

#[test]
fn delivers_one_json_line_to_listener_and_exits_zero() {
    let path = sock_path("deliver");
    let listener = UnixListener::bind(&path).expect("bind listener");
    listener.set_nonblocking(true).unwrap();

    let status = run_shim(
        &path,
        Some("codex"),
        br#"{"hook_event_name":"Stop","session_id":"abc"}"#,
    );
    assert!(status.success(), "shim must exit 0; got {status:?}");

    // The connection is already queued (shim connected+wrote+exited); poll-accept
    // with a deadline so a regression can't hang the test forever.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "shim never delivered to the socket"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    // accept() inherited the listener's non-blocking mode — restore blocking.
    // The shim has already exited (we waited above), so its socket end is closed:
    // read_to_string drains the buffered line then sees EOF and returns (no hang).
    stream.set_nonblocking(false).unwrap();
    let mut got = String::new();
    stream
        .read_to_string(&mut got)
        .expect("read delivered line");

    let line = got.lines().next().expect("at least one line");
    let v: serde_json::Value = serde_json::from_str(line).expect("delivered line is valid JSON");
    assert_eq!(v["hook_event_name"], "Stop");
    assert_eq!(v["session_id"], "abc", "original payload preserved");
    assert_eq!(v["_pixtuoid_source"], "codex", "shim stamps the CLI source");
    assert!(v.get("_shim_ts_ms").is_some(), "shim stamps a timestamp");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_socket_exits_zero_without_blocking() {
    // No listener bound at this path: connect fails, the event is dropped, exit 0.
    let path = sock_path("nosock");
    let start = Instant::now();
    let status = run_shim(&path, None, br#"{"hook_event_name":"Stop"}"#);
    assert!(
        status.success(),
        "must exit 0 even with no listener; got {status:?}"
    );
    // A missing socket makes `connect()` return ConnectionRefused in microseconds
    // — it never reaches the 200ms WRITE_TIMEOUT (that guards the write AFTER a
    // successful connect). So this bound isn't testing the 200ms invariant; it
    // guards against a regression that added a blocking retry/backoff on connect
    // failure — don't delete it. The bound measures a CHILD PROCESS's whole
    // spawn+exit wall-clock, so it is load-sensitive: under the fully-parallel
    // suite it flaked at 1s from exec/dyld scheduling jitter alone (#161). Two
    // defenses: .config/nextest.toml gives this test the machine to itself
    // (threads-required), and 3s keeps headroom for slow runners while still
    // catching any real hang or retry loop.
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "shim must not block when the socket is absent"
    );
}

#[test]
fn malformed_stdin_exits_zero() {
    // Unparseable stdin must be dropped silently (never block / fail CC).
    let path = sock_path("garbage");
    let status = run_shim(&path, None, b"this is not json at all {{{");
    assert!(
        status.success(),
        "malformed stdin must still exit 0; got {status:?}"
    );
}
