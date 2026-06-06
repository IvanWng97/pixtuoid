//! The shim's socket-path resolution, in its own TEST-FREE file on purpose:
//! `pixtuoid-core/tests/socket_path_parity.rs` includes this file via
//! `#[path]` (source inclusion, NOT a cargo dependency — the shim must stay
//! free of pixtuoid-core and vice versa) and asserts it resolves identically
//! to the daemon's `ClaudeCodeSource::default_socket_path` in all three
//! branches. Producer and consumer MUST agree or hook events silently never
//! arrive. If you move or rename this file, that test breaks loudly — fix the
//! `#[path]` there, don't drop the parity pin.

pub fn default_socket_path() -> String {
    if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
        return p;
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{dir}/pixtuoid.sock");
    }
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    format!("/tmp/pixtuoid-{uid}.sock")
}
