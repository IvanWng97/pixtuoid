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
    // ERROR_PIPE_BUSY = all named-pipe server instances mid-handshake. Now the
    // windows-sys constant (#495 pulled the crate in for the peer check below),
    // no longer a hand-hardcoded 231. The backoff is bounded by the 200ms send
    // watchdog either way.
    const ERROR_PIPE_BUSY: i32 = windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32;
    const PIPE_BUSY_RETRY_BACKOFF_MS: u64 = 10;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)
        {
            Ok(mut f) => {
                // #495: before writing, verify the pipe SERVER runs as US — a
                // co-located user who squatted our predictable default pipe (so
                // our daemon's create failed → SocketBusy degrade) would else
                // receive the payload (cwd, tool names). Scoped to our default
                // rendezvous; an explicit PIXTUOID_SOCKET pipe is the user's
                // trust call (parity with the Unix owned_tmp_socket_dir scope).
                // Fail-closed drop, never a panic (invariant #5).
                if endpoint == crate::paths::default_windows_pipe_name() && !peer::server_is_us(&f)
                {
                    return;
                }
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

/// The Windows counterpart of the Unix `peer_is_us` check (#495): verify the
/// named-pipe SERVER (the process that created the pipe) runs as OUR user before
/// the shim writes the hook payload. Windows named pipes are a machine-global,
/// unprivileged namespace, so a co-located user can pre-create our predictable
/// `\\.\pipe\pixtuoid-{USERNAME}` (making our daemon's create fail → the hook
/// plane silently degrades) and receive the payload. Comparing the connected
/// server's token user SID to ours closes that. EVERYTHING here fails CLOSED
/// (any FFI failure ⇒ `false` ⇒ drop) and never panics — invariant #5.
///
/// KNOWN SHARP EDGE — pid→token, not fd-atomic (don't "fix" it): unlike Unix
/// `getpeereid` (which reads the peer off the connected fd atomically), this
/// resolves the server via `GetNamedPipeServerProcessId` → `OpenProcess`, a
/// pid→handle pair with an inherent PID-reuse TOCTOU. It is fail-closed AND
/// effectively unexploitable: for the payload to leak, the squatter's server
/// process must stay ALIVE to receive the write, so its pid can't have been
/// recycled; a recycled pid means the squatter exited, its pipe instance died
/// with it, and the write goes nowhere. There is no atomic Win32 alternative
/// that ALSO survives elevation — `GetSecurityInfo(OWNER)` reads the pipe object
/// owner in one call but would false-negative an admin daemon (owner = the
/// Administrators group, not the user). The `default_windows_pipe_name` re-read
/// in `send_line`'s scope check is a deliberate one-shot cost (the shim runs once
/// per hook and exits).
#[cfg(windows)]
mod peer {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// The `TOKEN_USER` blob for `process`'s token, `u64`-backed so the embedded
    /// `PSID` pointer is properly aligned (a `Vec<u8>` would only guarantee
    /// align-1). The returned buffer OWNS the SID — keep it alive across the
    /// `EqualSid` call. `None` on any failure.
    ///
    /// SAFETY: `process` is a valid process handle for the call's duration.
    unsafe fn token_user_blob(process: HANDLE) -> Option<Vec<u64>> {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        // Size probe (returns 0 + sets `len`), then the real read; close the
        // token on every path.
        let mut len: u32 = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let blob = if len == 0 {
            None
        } else {
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
                None
            } else {
                Some(buf)
            }
        };
        CloseHandle(token);
        blob
    }

    /// The `PSID` embedded in a `TOKEN_USER` blob. Valid only while `blob` lives.
    ///
    /// SAFETY: `blob` is a `TOKEN_USER` written by `GetTokenInformation`, u64-aligned.
    unsafe fn sid_of(blob: &[u64]) -> PSID {
        (*(blob.as_ptr().cast::<TOKEN_USER>())).User.Sid
    }

    /// True iff the pipe server behind `file` runs as our user. Fail-closed.
    pub(super) fn server_is_us(file: &std::fs::File) -> bool {
        let handle = file.as_raw_handle() as HANDLE;
        // SAFETY: `handle` is a live connected pipe; `server` (from OpenProcess)
        // is closed exactly once; the two SID buffers outlive the EqualSid call.
        unsafe {
            let mut server_pid: u32 = 0;
            if GetNamedPipeServerProcessId(handle, &mut server_pid) == 0 {
                return false;
            }
            // Our own user SID (GetCurrentProcess is a pseudo-handle — never closed).
            let Some(ours) = token_user_blob(GetCurrentProcess()) else {
                return false;
            };
            let server = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server_pid);
            if server.is_null() {
                return false;
            }
            let theirs = token_user_blob(server);
            CloseHandle(server);
            let Some(theirs) = theirs else {
                return false;
            };
            EqualSid(sid_of(&ours), sid_of(&theirs)) != 0
        }
    }
}

// No in-process tests for `send_line` ON PURPOSE: it spawns a watchdog that
// exit(0)s the whole process ~200ms later (both platforms), which would kill
// sibling tests under plain `cargo test`'s shared-process runner. All
// send_line coverage lives at the child-process level — tests/shim.rs
// (delivery, missing endpoint, stalled listener) and its Windows twin
// tests/shim_pipe.rs — where exit-is-the-contract is observable, not fatal.
// `peer::server_is_us` spawns NO watchdog, so it IS unit-testable below.

// The peer-cred check runs only on the `windows-test` CI job (no reachable path
// on Unix). Both ends of a self-hosted pipe are THIS process → same user → true.
#[cfg(all(windows, test))]
mod win_peer_tests {
    #[tokio::test]
    async fn server_is_us_for_a_self_hosted_pipe() {
        // A unique name per run so parallel test processes don't collide.
        let name = format!(r"\\.\pipe\pixtuoid-peer-selftest-{}", std::process::id());
        let _server = tokio::net::windows::named_pipe::ServerOptions::new()
            .create(&name)
            .expect("create self-hosted pipe server");
        // A blocking std client — the same handle type `send_line` writes over.
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("open client end");
        assert!(
            super::peer::server_is_us(&client),
            "our own process's pipe server must verify as us"
        );
    }
}
