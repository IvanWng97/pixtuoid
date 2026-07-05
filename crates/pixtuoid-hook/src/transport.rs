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
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(endpoint) {
        // For the `/tmp/pixtuoid-{uid}/` fallback we own, verify the connected
        // PEER is us BEFORE writing (#485). This closes the read-side TOCTOU a
        // pre-connect path stat can't: even if a co-located user wins the
        // create-race on an absent fallback dir and owns the listening socket,
        // its peer uid != ours, so we drop instead of leaking the payload (cwd,
        // tool names). The check is on the connected fd — atomic w.r.t. the
        // connection, not a racy pre-stat of the path. Scoped to the fallback:
        // an XDG or explicit PIXTUOID_SOCKET endpoint is the user's own trust
        // decision (it may point at a cross-uid system daemon). One non-blocking
        // getpeereid syscall, inside the watchdog bound.
        if crate::paths::owned_tmp_socket_dir(endpoint).is_some() && !peer_is_us(&s) {
            return;
        }
        let _ = s.set_write_timeout(Some(WRITE_TIMEOUT));
        let _ = s.write_all(line);
    }
}

/// True iff the connected peer's effective uid is OURS. Validated on the live fd
/// (atomic w.r.t. the connection — no TOCTOU), so a co-located user who parked a
/// listening socket at our rendezvous path is rejected before any payload is
/// written. Fails CLOSED when the peer uid can't be read (`None` → `false` →
/// drop): the syscall doesn't fail on a healthy connected `AF_UNIX` stream, so a
/// failure means we cannot prove the peer is us. Complements the daemon's
/// create-side 0700-dir hardening (`hook::unix::ensure_private_dir`).
#[cfg(unix)]
fn peer_is_us(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    // Safety: getuid is always safe on Unix.
    peer_uid(stream.as_raw_fd()) == Some(unsafe { libc::getuid() })
}

/// The connected peer's uid, or `None` if it can't be read. Linux exposes it via
/// `SO_PEERCRED` (`struct ucred`); macOS/BSD via `getpeereid` (`libc` doesn't
/// declare `getpeereid` on Linux, hence the split).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(fd: std::os::unix::io::RawFd) -> Option<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // Safety: `fd` is a live connected socket; the kernel writes `cred`/`len`.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(cred).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(cred.uid)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn peer_uid(fd: std::os::unix::io::RawFd) -> Option<u32> {
    let mut euid: libc::uid_t = 0;
    let mut egid: libc::gid_t = 0;
    // Safety: `fd` is a live connected socket; the kernel writes the out-params.
    let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
    (rc == 0).then_some(euid)
}

#[cfg(all(unix, test))]
mod tests {
    use super::peer_is_us;
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn peer_is_us_for_a_self_connection() {
        // Both ends are this test process, so the peer's uid IS ours — the legit
        // shim→daemon case (same user). Also proves getpeereid links + works on
        // every CI platform the shim targets.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let sock = tmp.path().join("s.sock");
        let listener = UnixListener::bind(&sock).expect("bind");
        let client = UnixStream::connect(&sock).expect("connect");
        let (server, _) = listener.accept().expect("accept");
        assert!(peer_is_us(&client), "our own connection's peer is us");
        assert!(peer_is_us(&server), "the accepted side's peer is us too");
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
