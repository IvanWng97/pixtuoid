//! Best-effort one-line delivery to the daemon — the ONLY platform-split
//! seam in the shim. Contract on every path (invariant: never block CC):
//! all failures return silently (caller exits 0) and the entire send is
//! bounded by ~WRITE_TIMEOUT on both platforms.

use std::time::Duration;

pub const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

#[cfg(unix)]
pub fn send_line(endpoint: &str, line: &[u8]) {
    use std::io::Write;
    // `UnixStream::connect` has no timeout knob — a missing daemon fails
    // fast (NotFound/ConnectionRefused), but a backlog-saturated listener
    // parks connect() indefinitely, past the 200ms invariant-#5 budget that
    // set_write_timeout only enforces AFTER a successful connect (#167).
    // Bound the WHOLE connect+write phase the way the Windows arm below
    // does: a watchdog thread that hard-exits the process — after stdin is
    // consumed this send is the shim's only job (see main), and
    // exit(0)-on-timeout IS the contract (never block CC, spec §2).
    // Builder::spawn (not thread::spawn) so OS thread exhaustion degrades to
    // dropping the event instead of an abort. The write timeout stays as a
    // second layer: it usually errors out of a stalled write before the
    // watchdog has to shoot the process.
    let watchdog = std::thread::Builder::new().spawn(|| {
        std::thread::sleep(WRITE_TIMEOUT);
        std::process::exit(0);
    });
    if watchdog.is_err() {
        return;
    }
    if let Ok(mut s) = std::os::unix::net::UnixStream::connect(endpoint) {
        let _ = s.set_write_timeout(Some(WRITE_TIMEOUT));
        let _ = s.write_all(line);
    }
}

#[cfg(windows)]
pub fn send_line(endpoint: &str, line: &[u8]) {
    use std::io::Write;
    // Named pipes have no SO_SNDTIMEO equivalent for sync writes, so the
    // 200ms invariant is enforced by a watchdog thread that hard-exits the
    // process: after stdin is consumed this send is the shim's only job,
    // and exit(0)-on-timeout IS the contract (never block CC, spec §2).
    // The daemon sizes its pipe in-buffer >= the shim's 1MiB stdin cap so a
    // write that gets through open() never stalls on quota in practice.
    // Builder::spawn (not thread::spawn) so OS thread exhaustion degrades to
    // dropping the event instead of an abort — and we must NOT enter the
    // retry loop watchdog-less, or the 231 retry becomes unbounded.
    let watchdog = std::thread::Builder::new().spawn(|| {
        std::thread::sleep(WRITE_TIMEOUT);
        std::process::exit(0);
    });
    if watchdog.is_err() {
        return;
    }
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
            // 231 = ERROR_PIPE_BUSY (all server instances mid-handshake):
            // retry until the watchdog fires. Matched on raw_os_error to
            // keep the shipped shim at zero Windows deps.
            Err(e) if e.raw_os_error() == Some(231) => {
                std::thread::sleep(Duration::from_millis(10));
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
