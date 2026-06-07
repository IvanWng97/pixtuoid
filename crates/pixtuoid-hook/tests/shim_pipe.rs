#![cfg(windows)]
//! Windows twins of tests/shim.rs — the invariant-#5 contract against a real
//! named pipe, plus the watchdog's own pin (stalled daemon → exit 0 ~200ms).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::net::windows::named_pipe::{ClientOptions, PipeMode, ServerOptions};

fn shim_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pixtuoid-hook")
}

fn run_shim(pipe_name: &str, stdin_json: &str) -> (std::process::ExitStatus, Duration) {
    let started = Instant::now();
    let mut child = Command::new(shim_bin())
        .env("PIXTUOID_SOCKET", pipe_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn shim");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin_json.as_bytes())
        .expect("write stdin");
    let status = child.wait().expect("wait shim");
    (status, started.elapsed())
}

#[tokio::test]
async fn delivers_one_json_line_to_pipe_listener_and_exits_zero() {
    let name = format!(r"\\.\pipe\pixtuoid-test-{}", std::process::id());
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .pipe_mode(PipeMode::Byte)
        .create(&name)
        .expect("create pipe");

    let name2 = name.clone();
    let shim = std::thread::spawn(move || {
        run_shim(&name2, r#"{"hook_event_name":"Stop","session_id":"s1"}"#)
    });

    server.connect().await.expect("connect");
    let mut got = Vec::new();
    server.read_to_end(&mut got).await.expect("read");
    let line = String::from_utf8(got).expect("utf8");
    assert!(line.ends_with('\n'), "newline-terminated: {line:?}");
    assert!(line.contains(r#""hook_event_name":"Stop""#));
    assert!(line.contains("_shim_ts_ms"), "shim enrichment present");

    let (status, _) = shim.join().expect("join");
    assert!(status.success(), "exit 0");
}

#[test]
fn missing_pipe_exits_zero_without_blocking() {
    let (status, elapsed) = run_shim(r"\\.\pipe\pixtuoid-test-nonexistent", "{}");
    assert!(status.success(), "exit 0 on missing daemon");
    // NotFound is the fast path — generous CI margin, but well under any hang.
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}

#[tokio::test]
async fn stalled_daemon_shim_exits_zero_within_watchdog_bound() {
    // A pipe whose single instance is already taken by another client: the
    // shim's open() gets ERROR_PIPE_BUSY → busy-retry → the 200ms watchdog
    // fires and exits 0. Pins invariant #5's whole-phase bound.
    let name = format!(r"\\.\pipe\pixtuoid-test-stall-{}", std::process::id());
    let _server = ServerOptions::new()
        .first_pipe_instance(true)
        .pipe_mode(PipeMode::Byte)
        .create(&name)
        .expect("create pipe");
    // Occupy the lone instance so the shim's open() stays BUSY.
    let _client = ClientOptions::new().open(&name).expect("occupy instance");

    let (status, elapsed) = run_shim(&name, r#"{"hook_event_name":"Stop","session_id":"s2"}"#);
    assert!(status.success(), "watchdog exits 0");
    // Watchdog bound is 200ms; generous runner-jitter headroom while still
    // proving "never blocks CC".
    assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
}
