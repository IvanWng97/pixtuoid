//! The shim's socket-path resolution, in its own TEST-FREE file on purpose:
//! `pixtuoid-core/tests/socket_path_parity.rs` includes it via `#[path]` (source
//! inclusion, NOT a cargo dependency — the shim must stay free of pixtuoid-core
//! and vice versa) and pins it to the daemon's
//! `ClaudeCodeSource::default_socket_path`. Producer and consumer MUST agree or
//! hook events silently never arrive; if you move this file, fix that `#[path]`
//! rather than dropping the parity pin.

pub(crate) fn default_socket_path() -> String {
    if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
        // Set-but-empty/whitespace = unset (the RUST_LOG policy): honored
        // verbatim, "" makes the daemon's bind fail fatally and the shim
        // silently drop every event.
        if !p.trim().is_empty() {
            return p;
        }
    }
    #[cfg(unix)]
    {
        // XDG spec: absolute-only. Empty → `/pixtuoid.sock` (fatal bind); relative →
        // shim/daemon cwd mis-rendezvous. Non-absolute is invalid → treated as unset.
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            if !dir.trim().is_empty() && std::path::Path::new(&dir).is_absolute() {
                return format!("{dir}/pixtuoid.sock");
            }
        }
        // No XDG_RUNTIME_DIR (macOS, bare Linux): a per-user subdir the daemon
        // creates 0700-owned-by-us, NOT a flat predictable /tmp name. A
        // co-located user could squat/lock a flat `pixtuoid-{uid}.sock` and
        // silently disable the hook plane (#485); a 0700 subdir they cannot
        // write into makes the daemon's bind fail loudly instead.
        // Safety: getuid is always safe on Unix.
        let uid = unsafe { libc::getuid() };
        format!("/tmp/pixtuoid-{uid}/pixtuoid.sock")
    }
    #[cfg(windows)]
    {
        default_windows_pipe_name()
    }
}

/// The default hook pipe name `\\.\pipe\pixtuoid-{USERNAME}`, WITHOUT the
/// `PIXTUOID_SOCKET` override short-circuit. The security boundary is the
/// server-side DACL; the NAME is namespacing only. Backslashes are sanitized:
/// pipe names can't contain them, and enterprise boxes set USERNAME=DOMAIN\user.
/// The shim compares the resolved endpoint against this to SCOPE its #495
/// peer-cred check to our own predictable rendezvous — an explicit
/// `PIXTUOID_SOCKET` pipe stays the user's trust decision.
#[cfg(windows)]
pub(crate) fn default_windows_pipe_name() -> String {
    let user = std::env::var("USERNAME")
        .unwrap_or_else(|_| "default".into())
        .replace('\\', "-");
    format!(r"\\.\pipe\pixtuoid-{user}")
}

/// The per-user tmp dir we OWN (`/tmp/pixtuoid-{uid}`) when `endpoint` is the
/// no-XDG `/tmp` FALLBACK — else `None`. PURE (no I/O), so it stays parity-safe.
/// SCOPES the shim's connected-peer-uid check (#485) to that fallback: the XDG /
/// explicit-override branches are systemd's / the user's trust decision, not
/// ours to police (an override may legitimately point at a cross-uid daemon).
#[cfg(unix)]
pub(crate) fn owned_tmp_socket_dir(endpoint: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    let owned = PathBuf::from(format!("/tmp/pixtuoid-{uid}"));
    (Path::new(endpoint).parent() == Some(owned.as_path())).then_some(owned)
}
