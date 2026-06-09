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

/// Spawn the shim with the given socket path, optional source env, and extra
/// argv, pipe `stdin` to it, and return its exit status (after closing stdin → EOF).
fn run_shim_inner(
    socket: &std::path::Path,
    source: Option<&str>,
    args: &[&str],
    stdin: &[u8],
) -> std::process::ExitStatus {
    let mut cmd = Command::new(BIN);
    cmd.env("PIXTUOID_SOCKET", socket)
        .args(args)
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

/// Spawn the shim with a source via the `PIXTUOID_SOURCE` env (the Unix install form).
fn run_shim(
    socket: &std::path::Path,
    source: Option<&str>,
    stdin: &[u8],
) -> std::process::ExitStatus {
    run_shim_inner(socket, source, &[], stdin)
}

/// Spawn the shim with extra argv and NO `PIXTUOID_SOURCE` env, so a `--source`
/// flag is the only source signal (the Windows install form).
fn run_shim_args(
    socket: &std::path::Path,
    args: &[&str],
    stdin: &[u8],
) -> std::process::ExitStatus {
    run_shim_inner(socket, None, args, stdin)
}

/// Poll-accept one connection (the shim has already connected+written+exited),
/// drain it to EOF, and parse the first delivered line as JSON. Shared by the
/// env-source and argv-source delivery tests.
fn recv_delivered_json(listener: &UnixListener) -> serde_json::Value {
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
    // accept() inherited the listener's non-blocking mode — restore blocking. The
    // shim has already exited, so its end is closed: read drains then sees EOF.
    stream.set_nonblocking(false).unwrap();
    let mut got = String::new();
    stream
        .read_to_string(&mut got)
        .expect("read delivered line");
    let line = got.lines().next().expect("at least one line");
    serde_json::from_str(line).expect("delivered line is valid JSON")
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

    let v = recv_delivered_json(&listener);
    assert_eq!(v["hook_event_name"], "Stop");
    assert_eq!(v["session_id"], "abc", "original payload preserved");
    assert_eq!(v["_pixtuoid_source"], "codex", "shim stamps the CLI source");
    assert!(v.get("_shim_ts_ms").is_some(), "shim stamps a timestamp");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn argv_source_flag_stamps_source_without_env() {
    // The Windows install form: `pixtuoid-hook --source codex` with NO
    // PIXTUOID_SOURCE env. The flag alone must drive the `_pixtuoid_source` stamp.
    let path = sock_path("argvsrc");
    let listener = UnixListener::bind(&path).expect("bind listener");
    listener.set_nonblocking(true).unwrap();

    let status = run_shim_args(
        &path,
        &["--source", "codex"],
        br#"{"hook_event_name":"Stop","session_id":"abc"}"#,
    );
    assert!(status.success(), "shim must exit 0; got {status:?}");

    let v = recv_delivered_json(&listener);
    assert_eq!(
        v["_pixtuoid_source"], "codex",
        "the --source flag must stamp the CLI source with no env set"
    );
    assert_eq!(v["session_id"], "abc", "original payload preserved");

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
    // A missing socket makes `connect()` return ConnectionRefused in
    // microseconds, so this guards against a regression that added a blocking
    // retry/backoff on connect failure — don't delete it. Bound decomposition
    // (same as the stalled-listener test below and shim_pipe.rs's twins):
    // worst-case child RUNTIME is the ~200ms #167 watchdog; the rest is
    // spawn/exec jitter margin — the bound measures a CHILD PROCESS's whole
    // spawn+exit wall-clock, which is load-sensitive (#161: flaked at 1s
    // under the fully-parallel suite, passed isolated). The jitter term is
    // attacked directly by .config/nextest.toml's threads-required override
    // (this test runs with the machine to itself).
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "shim must not block when the socket is absent"
    );
}

#[test]
fn stalled_listener_shim_exits_zero_within_watchdog_bound() {
    // A listener whose accept loop is wedged while its backlog saturates —
    // the one Unix path where `connect()` itself can park forever (#167).
    // Kernel-dependent: Linux BLOCKS the shim's connect (the watchdog must
    // shoot the process at ~200ms — the load-bearing arm, exercised on CI),
    // macOS fails fast with ECONNREFUSED. Both must exit 0 within the bound.
    // Mirrors shim_pipe.rs's stalled_daemon_shim_exits_zero_within_watchdog_bound.
    let path = sock_path("stall");
    let listener = UnixListener::bind(&path).expect("bind listener");

    // Saturate the accept backlog (std binds with backlog 128) without ever
    // accepting. Each filler holds its connection (or its blocked connect)
    // open by parking; the threads die with the test process.
    let fillers: Vec<_> = (0..160)
        .map(|_| {
            let p = path.clone();
            std::thread::spawn(move || {
                let _conn = std::os::unix::net::UnixStream::connect(&p);
                std::thread::park();
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));

    let start = Instant::now();
    let status = run_shim(&path, None, br#"{"hook_event_name":"Stop"}"#);
    assert!(
        status.success(),
        "stalled listener must still exit 0; got {status:?}"
    );
    // Watchdog bound is 200ms; the rest is spawn-jitter headroom (the
    // nextest threads-required override runs this test alone).
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "watchdog must bound the connect phase; took {:?}",
        start.elapsed()
    );

    drop(listener);
    drop(fillers);
    let _ = std::fs::remove_file(&path);
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
