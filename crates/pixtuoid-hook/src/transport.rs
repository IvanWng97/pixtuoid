//! Best-effort one-line delivery to the daemon — the ONLY platform-split
//! seam in the shim. Contract on every path (invariant: never block CC):
//! all failures return silently (caller exits 0) and the entire send is
//! bounded by ~WRITE_TIMEOUT on both platforms.

use std::time::Duration;

pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

/// Arm the send-timeout watchdog: a thread that hard-exits the process after
/// `WRITE_TIMEOUT`, bounding the whole connect+write phase on BOTH platforms
/// (invariant #5: never block CC — exit(0)-on-timeout IS the contract). Uses
/// `Builder::spawn` (not `thread::spawn`) so OS thread exhaustion degrades to
/// dropping the event instead of an abort. Returns `false` if the thread can't
/// be spawned, so the caller bails before entering its connect/retry path.
fn spawn_timeout_watchdog() -> bool {
    std::thread::Builder::new()
        .spawn(|| {
            std::thread::sleep(WRITE_TIMEOUT);
            std::process::exit(0);
        })
        .is_ok()
}

#[cfg(unix)]
pub(crate) fn send_line(endpoint: &str, line: &[u8]) {
    use std::io::Write;
    // `UnixStream::connect` has no timeout knob — a missing daemon fails
    // fast (NotFound/ConnectionRefused), but a backlog-saturated listener
    // parks connect() indefinitely, past the 200ms invariant-#5 budget that
    // set_write_timeout only enforces AFTER a successful connect (#167).
    // Bound the WHOLE connect+write phase the way the Windows arm below
    // does: a watchdog thread that hard-exits the process — after stdin is
    // consumed this send is the shim's only job (see main), and
    // exit(0)-on-timeout IS the contract (never block CC, spec §2). The write
    // timeout stays as a second layer: it usually errors out of a stalled write
    // before the watchdog has to shoot the process.
    if !spawn_timeout_watchdog() {
        return;
    }
    // Before piping the hook payload anywhere, refuse a HOSTILE per-user tmp
    // rendezvous dir (#485): if the endpoint is our `/tmp/pixtuoid-{uid}/`
    // fallback and that dir isn't a private dir we own, a co-located user has
    // parked it — connecting would leak the payload (cwd, tool names) into
    // their listener. Drop silently (the never-block-CC contract). An ABSENT
    // dir is fine — the connect below just fails fast. One lstat, non-blocking;
    // the XDG / explicit-PIXTUOID_SOCKET branches return None (untouched).
    if let Some(dir) = crate::paths::owned_tmp_socket_dir(endpoint) {
        if owned_dir_is_hostile(&dir) {
            return;
        }
    }
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(endpoint) {
        let _ = s.set_write_timeout(Some(WRITE_TIMEOUT));
        let _ = s.write_all(line);
    }
}

/// True iff `dir` EXISTS but is NOT a private directory we own — a symlink, a
/// non-dir, owned by another uid, or group/other-accessible (`mode & 0o077`).
/// An absent dir returns `false` (not hostile: the daemon simply hasn't bound
/// yet, and the caller's connect will fail fast). `symlink_metadata` (lstat)
/// keeps a planted symlink from being followed to a hostile target. Mirrors the
/// daemon's `ensure_owned_socket_dir` predicate in `hook/unix.rs`.
#[cfg(unix)]
fn owned_dir_is_hostile(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(dir) {
        Ok(md) => {
            // Safety: getuid is always safe on Unix.
            let uid = unsafe { libc::getuid() };
            !md.is_dir() || md.uid() != uid || (md.mode() & 0o077 != 0)
        }
        Err(_) => false,
    }
}

#[cfg(all(unix, test))]
mod tests {
    use super::owned_dir_is_hostile;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn absent_dir_is_not_hostile() {
        // The daemon simply hasn't bound yet — the connect below fails fast and
        // drops; we must NOT treat "not there" as an attack (would lose events).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert!(!owned_dir_is_hostile(&tmp.path().join("nope")));
    }

    #[test]
    fn owned_0700_dir_is_not_hostile() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        assert!(!owned_dir_is_hostile(&dir));
    }

    #[test]
    fn group_or_other_accessible_dir_is_hostile() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("loose");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(owned_dir_is_hostile(&dir));
    }

    #[test]
    fn a_symlink_is_hostile() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let target = tmp.path().join("real");
        std::fs::create_dir(&target).expect("mkdir");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(
            owned_dir_is_hostile(&link),
            "lstat catches the symlink even though its 0700 target is fine"
        );
    }

    #[test]
    fn a_regular_file_at_the_dir_path_is_hostile() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let file = tmp.path().join("f");
        std::fs::write(&file, b"squat").expect("write");
        assert!(owned_dir_is_hostile(&file));
    }
}

#[cfg(windows)]
pub(crate) fn send_line(endpoint: &str, line: &[u8]) {
    use std::io::Write;
    // Named pipes have no SO_SNDTIMEO equivalent for sync writes, so the
    // 200ms invariant is enforced by a watchdog thread that hard-exits the
    // process: after stdin is consumed this send is the shim's only job,
    // and exit(0)-on-timeout IS the contract (never block CC, spec §2).
    // The daemon's 1MiB pipe in-buffer covers the shim's capped stdin
    // (`STDIN_CAP = 1MiB − STAMP_HEADROOM` in main.rs) PLUS the stamps +
    // newline, so a write that gets through open() never stalls on quota. We
    // must NOT enter the retry loop watchdog-less, or the 231 retry becomes
    // unbounded.
    if !spawn_timeout_watchdog() {
        return;
    }
    // 231 = ERROR_PIPE_BUSY (all named-pipe server instances mid-handshake).
    // Matched on the raw numeric code — NOT a windows-crate constant — to keep the
    // shipped shim at ZERO Windows deps (the deliberate reason it's hardcoded, not
    // imported). The backoff is bounded by the 200ms send watchdog either way.
    const ERROR_PIPE_BUSY: i32 = 231;
    const PIPE_BUSY_RETRY_BACKOFF_MS: u64 = 10;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)
        {
            Ok(mut f) => {
                let _ = f.write_all(line);
                return;
            }
            // Retry until the watchdog fires (all server instances mid-handshake).
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                std::thread::sleep(Duration::from_millis(PIPE_BUSY_RETRY_BACKOFF_MS));
            }
            // NotFound etc.: daemon not running — drop the event, same as
            // the Unix connect-failure path.
            Err(_) => return,
        }
    }
}

// No in-process tests here ON PURPOSE: send_line spawns a watchdog that
// exit(0)s the whole process ~200ms later (both platforms), which would kill
// sibling tests under plain `cargo test`'s shared-process runner. All
// send_line coverage lives at the child-process level — tests/shim.rs
// (delivery, missing endpoint, stalled listener) and its Windows twin
// tests/shim_pipe.rs — where exit-is-the-contract is observable, not fatal.
