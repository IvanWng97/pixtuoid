//! Pins the hook shim's socket path EQUAL to the daemon's, branch by branch.
//!
//! The shim (producer) and `ClaudeCodeSource` (consumer) each compute the
//! default socket path independently — they MUST agree or hook events
//! silently never arrive. Each crate already unit-tests its own three
//! branches against the same literals, but two parallel literal pins only
//! hold if a reviewer notices the sibling when one side changes; this test
//! compares the two implementations DIRECTLY, so a one-sided change fails
//! regardless of which crate's unit test got updated (#93).
//!
//! The shim source is included via `#[path]` — compile-time source inclusion,
//! NOT a cargo dependency — because the hook crate must stay free of
//! pixtuoid-core (workspace invariant #5: nothing may slow or bloat the shim).

#[path = "../../pixtuoid-hook/src/paths.rs"]
mod hook_paths;

use std::path::PathBuf;

use pixtuoid_core::source::claude_code::ClaudeCodeSource;

fn both() -> (PathBuf, PathBuf) {
    (
        PathBuf::from(hook_paths::default_socket_path()),
        ClaudeCodeSource::default_socket_path(),
    )
}

// All three branches in ONE test: env vars are process-global, and this is the
// only test in this integration binary, so there is nothing to race (same
// pattern as the per-crate branch tests).
#[cfg(unix)]
#[test]
fn shim_and_daemon_resolve_identical_socket_paths_in_all_three_branches() {
    let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
    let saved_xdg = std::env::var_os("XDG_RUNTIME_DIR");

    // Branch 1: PIXTUOID_SOCKET wins on both sides.
    std::env::set_var("PIXTUOID_SOCKET", "/explicit/parity.sock");
    std::env::set_var("XDG_RUNTIME_DIR", "/run/user/7");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/explicit/parity.sock"));

    // Branch 2: XDG_RUNTIME_DIR drives the path on both sides.
    std::env::remove_var("PIXTUOID_SOCKET");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "XDG_RUNTIME_DIR branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));

    // Branch 3: uid-suffixed /tmp fallback on both sides.
    std::env::remove_var("XDG_RUNTIME_DIR");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "/tmp-uid fallback branch diverged");

    match saved_socket {
        Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
        None => std::env::remove_var("PIXTUOID_SOCKET"),
    }
    match saved_xdg {
        Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }
}

// Only EXERCISED on a Windows runner (PR 3's CI job) — until then the
// ubuntu cross-check job keeps it compiling. The windows CI job is part
// of invariant #3 from that point on.
#[cfg(windows)]
#[test]
fn shim_and_daemon_resolve_identical_pipe_names_in_all_branches() {
    let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
    let saved_user = std::env::var_os("USERNAME");

    // Branch 1: PIXTUOID_SOCKET (a pipe name on Windows) wins on both sides.
    std::env::set_var("PIXTUOID_SOCKET", r"\\.\pipe\parity-explicit");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\parity-explicit"));

    // Branch 2: USERNAME-keyed default pipe name on both sides.
    std::env::remove_var("PIXTUOID_SOCKET");
    std::env::set_var("USERNAME", "parity");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME default branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));

    // Branch 3: DOMAIN\user-form USERNAME is sanitized identically on both
    // sides (backslashes are illegal in pipe names; enterprise boxes set
    // USERNAME=DOMAIN\user).
    std::env::set_var("USERNAME", r"CORP\alice");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME sanitize branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-CORP-alice"));

    // Branch 4: USERNAME absent → the shared "default" fallback on both sides.
    std::env::remove_var("USERNAME");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME-absent fallback branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-default"));

    match saved_socket {
        Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
        None => std::env::remove_var("PIXTUOID_SOCKET"),
    }
    match saved_user {
        Some(v) => std::env::set_var("USERNAME", v),
        None => std::env::remove_var("USERNAME"),
    }
}
