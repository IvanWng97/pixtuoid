//! Pins the hook shim's socket path EQUAL to the daemon's, branch by branch.
//!
//! The shim (producer) and `ClaudeCodeSource` (consumer) each compute the
//! default socket path independently — they MUST agree or hook events silently
//! never arrive. Each crate unit-tests its own branches against the same
//! literals, but two parallel literal pins only hold if a reviewer notices the
//! sibling; this compares the two implementations DIRECTLY (#93).
//!
//! The shim source is included via `#[path]`, NOT a cargo dependency, because
//! the hook crate must stay free of pixtuoid-core (workspace invariant #5).

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
// only test in this integration binary, so there is nothing to race.
#[cfg(unix)]
#[test]
fn shim_and_daemon_resolve_identical_socket_paths_in_all_three_branches() {
    let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
    let saved_xdg = std::env::var_os("XDG_RUNTIME_DIR");

    std::env::set_var("PIXTUOID_SOCKET", "/explicit/parity.sock");
    std::env::set_var("XDG_RUNTIME_DIR", "/run/user/7");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/explicit/parity.sock"));

    // Set-but-empty PIXTUOID_SOCKET = unset on BOTH sides.
    std::env::set_var("PIXTUOID_SOCKET", "");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "empty PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));
    std::env::set_var("PIXTUOID_SOCKET", "   ");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "whitespace PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));

    std::env::remove_var("PIXTUOID_SOCKET");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "XDG_RUNTIME_DIR branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));

    // XDG_RUNTIME_DIR is absolute-only per spec, so an empty/relative value must be
    // ignored — never `/pixtuoid.sock` or a cwd-relative path.
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    let tmp_fallback = PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"));
    for invalid in ["", "   ", "relative/run"] {
        std::env::set_var("XDG_RUNTIME_DIR", invalid);
        let (shim, daemon) = both();
        assert_eq!(shim, daemon, "invalid XDG_RUNTIME_DIR {invalid:?} diverged");
        assert_eq!(
            shim, tmp_fallback,
            "invalid XDG_RUNTIME_DIR {invalid:?} must fall to the /tmp subdir"
        );
    }

    // The /tmp fallback is a per-user 0700 SUBDIR, not a flat squattable
    // `pixtuoid-{uid}.sock` (#485).
    std::env::remove_var("XDG_RUNTIME_DIR");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "/tmp-uid fallback branch diverged");
    assert_eq!(
        shim,
        PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"))
    );
    // The shim's pre-connect ownership guard derives its owned-dir from that same
    // fallback endpoint.
    assert_eq!(
        hook_paths::owned_tmp_socket_dir(&shim.to_string_lossy()),
        Some(PathBuf::from(format!("/tmp/pixtuoid-{uid}"))),
    );

    match saved_socket {
        Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
        None => std::env::remove_var("PIXTUOID_SOCKET"),
    }
    match saved_xdg {
        Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }
}

#[cfg(windows)]
#[test]
fn shim_and_daemon_resolve_identical_pipe_names_in_all_branches() {
    let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
    let saved_user = std::env::var_os("USERNAME");

    // PIXTUOID_SOCKET is a pipe name on Windows.
    std::env::set_var("PIXTUOID_SOCKET", r"\\.\pipe\parity-explicit");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\parity-explicit"));

    // Set-but-empty PIXTUOID_SOCKET = unset on BOTH sides.
    std::env::set_var("PIXTUOID_SOCKET", "");
    std::env::set_var("USERNAME", "parity");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "empty PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));
    std::env::set_var("PIXTUOID_SOCKET", "   ");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "whitespace PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));

    std::env::remove_var("PIXTUOID_SOCKET");
    std::env::set_var("USERNAME", "parity");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME default branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));

    // Backslashes are illegal in pipe names, and enterprise boxes set
    // USERNAME=DOMAIN\user — both sides must sanitize identically.
    std::env::set_var("USERNAME", r"CORP\alice");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME sanitize branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-CORP-alice"));

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
