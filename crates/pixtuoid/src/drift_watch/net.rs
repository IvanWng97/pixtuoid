//! Upstream over the network, through the platform's own `curl`, which
//! brings no TLS stack into the lockfile.

use std::process::Command;
use std::time::Duration;

use super::{Fetch, FetchError};

/// A status that means OUR pin is gone, so the watch is dark for that document:
/// probe health, never transient. Every other failure may pass on a retry.
const PERMANENT_HTTP_STATUS: [u16; 3] = [404, 410, 451];

#[derive(Debug)]
pub(super) struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
        let out = Command::new("curl")
            .args(["--silent", "--show-error", "--location", "--max-time"])
            .arg(timeout.as_secs().to_string())
            .args(["--user-agent", "pixtuoid-drift-watch"])
            // The status rides stderr's last line, so the body stays whole on
            // stdout and a curl error message lands above it.
            .args(["--write-out", "%{stderr}\n%{http_code}"])
            .arg(url)
            .output()
            .map_err(|e| FetchError::Transient(format!("could not run curl: {e}")))?;
        classify(
            out.status.success(),
            &String::from_utf8_lossy(&out.stderr),
            out.stdout,
        )
    }
}

/// What a curl run means: `ran` is its exit, `stderr` ends in the status.
fn classify(ran: bool, stderr: &str, body: Vec<u8>) -> Result<Vec<u8>, FetchError> {
    let status: u16 = stderr
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    if !ran || status == 0 {
        return Err(FetchError::Transient(format!(
            "curl failed: {}",
            stderr.trim()
        )));
    }
    match status {
        200..=299 => Ok(body),
        s if PERMANENT_HTTP_STATUS.contains(&s) => Err(FetchError::Gone { status: s }),
        s => Err(FetchError::Transient(format!("HTTP {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A permanent status is our pin gone, never transient: bucketing a 404 as
    /// transient kept the weekly job green while it watched nothing.
    #[test]
    fn a_permanent_status_darkens_the_pin_and_the_rest_may_retry() {
        assert_eq!(classify(true, "\n200", b"ok".to_vec()), Ok(b"ok".to_vec()));
        for s in PERMANENT_HTTP_STATUS {
            assert_eq!(
                classify(true, &format!("\n{s}"), vec![]),
                Err(FetchError::Gone { status: s })
            );
        }
        for s in [403, 429, 500, 502, 503] {
            assert!(matches!(
                classify(true, &format!("\n{s}"), vec![]),
                Err(FetchError::Transient(_))
            ));
        }
        // A connection that never got a response: curl exits non-zero, `000`.
        assert!(matches!(
            classify(false, "curl: (6) Could not resolve host\n000", vec![]),
            Err(FetchError::Transient(e)) if e.contains("Could not resolve host")
        ));
        assert!(matches!(
            classify(true, "no status line", vec![]),
            Err(FetchError::Transient(_))
        ));
    }
}
