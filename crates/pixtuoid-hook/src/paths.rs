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
    #[cfg(unix)]
    {
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            return format!("{dir}/pixtuoid.sock");
        }
        // Safety: getuid is always safe on Unix.
        let uid = unsafe { libc::getuid() };
        format!("/tmp/pixtuoid-{uid}.sock")
    }
    #[cfg(windows)]
    {
        // The pipe NAME is namespacing only — the security boundary is the
        // server-side DACL (spec §2). USERNAME is std-only, present in any
        // login session, and computed identically by shim and daemon
        // (parity-pinned in pixtuoid-core/tests/socket_path_parity.rs).
        // Backslashes are sanitized: pipe names can't contain them, and
        // enterprise boxes do set USERNAME=DOMAIN\user.
        let user = std::env::var("USERNAME")
            .unwrap_or_else(|_| "default".into())
            .replace('\\', "-");
        format!(r"\\.\pipe\pixtuoid-{user}")
    }
}
