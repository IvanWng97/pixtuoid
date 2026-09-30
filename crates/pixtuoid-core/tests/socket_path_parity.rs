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
        hook_paths::default_socket_path(),
        ClaudeCodeSource::default_socket_path(),
    )
}

#[cfg(unix)]
#[test]
fn shim_and_daemon_resolve_identical_socket_paths_in_all_three_branches() {
    let mut env = pixtuoid_core::test_env::EnvGuard::lock();

    env.set("PIXTUOID_SOCKET", "/explicit/parity.sock");
    env.set("XDG_RUNTIME_DIR", "/run/user/7");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/explicit/parity.sock"));

    // Set-but-empty PIXTUOID_SOCKET = unset on BOTH sides.
    env.set("PIXTUOID_SOCKET", "");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "empty PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));
    env.set("PIXTUOID_SOCKET", "   ");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "whitespace PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));

    env.remove("PIXTUOID_SOCKET");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "XDG_RUNTIME_DIR branch diverged");
    assert_eq!(shim, PathBuf::from("/run/user/7/pixtuoid.sock"));

    // XDG_RUNTIME_DIR is absolute-only per spec, so an empty/relative value must be
    // ignored — never `/pixtuoid.sock` or a cwd-relative path.
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    let tmp_fallback = PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"));
    for invalid in ["", "   ", "relative/run"] {
        env.set("XDG_RUNTIME_DIR", invalid);
        let (shim, daemon) = both();
        assert_eq!(shim, daemon, "invalid XDG_RUNTIME_DIR {invalid:?} diverged");
        assert_eq!(
            shim, tmp_fallback,
            "invalid XDG_RUNTIME_DIR {invalid:?} must fall to the /tmp subdir"
        );
    }

    // The /tmp fallback is a per-user 0700 SUBDIR, not a flat squattable
    // `pixtuoid-{uid}.sock` (#485).
    env.remove("XDG_RUNTIME_DIR");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "/tmp-uid fallback branch diverged");
    assert_eq!(
        shim,
        PathBuf::from(format!("/tmp/pixtuoid-{uid}/pixtuoid.sock"))
    );
    // The shim's pre-connect ownership guard derives its owned-dir from that same
    // fallback endpoint.
    assert_eq!(
        hook_paths::owned_tmp_socket_dir(&shim),
        Some(PathBuf::from(format!("/tmp/pixtuoid-{uid}"))),
    );
}

#[cfg(windows)]
#[test]
fn shim_and_daemon_resolve_identical_pipe_names_in_all_branches() {
    let mut env = pixtuoid_core::test_env::EnvGuard::lock();

    // PIXTUOID_SOCKET is a pipe name on Windows.
    env.set("PIXTUOID_SOCKET", r"\\.\pipe\parity-explicit");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\parity-explicit"));

    // Set-but-empty PIXTUOID_SOCKET = unset on BOTH sides.
    env.set("PIXTUOID_SOCKET", "");
    env.set("USERNAME", "parity");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "empty PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));
    env.set("PIXTUOID_SOCKET", "   ");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "whitespace PIXTUOID_SOCKET branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));

    env.remove("PIXTUOID_SOCKET");
    env.set("USERNAME", "parity");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME default branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-parity"));

    // Backslashes are illegal in pipe names, and enterprise boxes set
    // USERNAME=DOMAIN\user — both sides must sanitize identically.
    env.set("USERNAME", r"CORP\alice");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME sanitize branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-CORP-alice"));

    env.remove("USERNAME");
    let (shim, daemon) = both();
    assert_eq!(shim, daemon, "USERNAME-absent fallback branch diverged");
    assert_eq!(shim, PathBuf::from(r"\\.\pipe\pixtuoid-default"));
}
