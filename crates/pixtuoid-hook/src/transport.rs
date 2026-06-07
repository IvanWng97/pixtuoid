//! Best-effort one-line delivery to the daemon — the ONLY platform-split
//! seam in the shim. Contract on every path (invariant: never block CC):
//! all failures return silently (caller exits 0) and the entire send is
//! bounded by ~WRITE_TIMEOUT on both platforms.

use std::time::Duration;

pub const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

#[cfg(unix)]
pub fn send_line(endpoint: &str, line: &[u8]) {
    use std::io::Write;
    // Today's behavior, moved verbatim: connect (no explicit timeout — a
    // missing daemon fails fast with NotFound), bound the WRITE.
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn unix_send_line_delivers_one_line() {
        let dir = std::env::temp_dir().join(format!("pixtuoid-shim-t-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("t.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

        send_line(sock.to_str().unwrap(), b"{\"x\":1}\n");

        let (mut conn, _) = listener.accept().unwrap();
        let mut got = String::new();
        conn.read_to_string(&mut got).unwrap();
        assert_eq!(got, "{\"x\":1}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unix_send_line_missing_endpoint_returns_silently() {
        // Must not panic, error, or hang — the caller's exit-0 contract.
        send_line("/nonexistent/pixtuoid-none.sock", b"{}\n");
    }
}
