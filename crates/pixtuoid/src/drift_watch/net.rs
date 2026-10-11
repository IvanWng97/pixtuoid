//! Upstream over the network, through the platform's own `curl` and `tar`,
//! which bring no TLS stack or archive crate into the lockfile.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{Fetch, FetchError};

/// A status that means OUR pin is gone, so the watch is dark for that document:
/// probe health, never transient. Every other failure may pass on a retry.
const PERMANENT_HTTP_STATUS: [u16; 3] = [404, 410, 451];

/// The status rides stderr's last line, so the body stays whole on stdout
/// and a curl error message lands above it; [`classify`] reads it there.
const WRITE_OUT: &str = "%{stderr}\n%{http_code}";

#[derive(Debug)]
pub(super) struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
        let out = Command::new("curl")
            .args(["--silent", "--show-error", "--location", "--max-time"])
            .arg(timeout.as_secs().to_string())
            .args(["--user-agent", "pixtuoid-drift-watch"])
            .args(["--write-out", WRITE_OUT])
            .arg(url)
            .output()
            .map_err(|e| FetchError::Transient(format!("could not run curl: {e}")))?;
        classify(
            out.status.success(),
            &String::from_utf8_lossy(&out.stderr),
            out.stdout,
        )
    }

    fn untar(&self, tarball: &[u8], member: &str) -> Result<Option<Vec<u8>>, FetchError> {
        // Listed first: tar fails alike on a missing member and a broken
        // archive, and only the first is our pin going dark.
        let listing = tar(&["-tzf", "-"], tarball)?;
        if !String::from_utf8_lossy(&listing)
            .lines()
            .any(|l| l == member)
        {
            return Ok(None);
        }
        tar(&["-xzOf", "-", member], tarball).map(Some)
    }
}

/// `tar <args>` over `tarball` on stdin: its stdout, or why it failed.
fn tar(args: &[&str], tarball: &[u8]) -> Result<Vec<u8>, FetchError> {
    let mut child = Command::new("tar")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FetchError::Transient(format!("could not run tar: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| FetchError::Transient("tar has no stdin".to_owned()))?;
    // Written from a thread: tar's stdout fills while it still reads, so one
    // thread doing both deadlocks on a tarball larger than the pipes.
    let out = std::thread::scope(|s| {
        s.spawn(move || stdin.write_all(tarball));
        child.wait_with_output()
    })
    .map_err(|e| FetchError::Transient(format!("tar failed: {e}")))?;
    if !out.status.success() {
        return Err(FetchError::Transient(format!(
            "tar failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// What a curl run means: `ran` is its exit, `stderr` ends in the status.
fn classify(ran: bool, stderr: &str, body: Vec<u8>) -> Result<Vec<u8>, FetchError> {
    let status: u16 = stderr
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    if !ran || status == 0 {
        // The last line is the status `--write-out` appended, not curl's say.
        let said: Vec<&str> = stderr.trim_end().lines().collect();
        let said = said.split_last().map_or(&said[..], |(_, rest)| rest);
        return Err(FetchError::Transient(format!(
            "curl failed: {}",
            said.join(" ").trim()
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
            Err(FetchError::Transient(e)) if e.contains("Could not resolve host") && !e.contains("000")
        ));
        assert!(matches!(
            classify(true, "no status line", vec![]),
            Err(FetchError::Transient(_))
        ));
        // A body cut off after a 200 line (`--max-time` mid-transfer) is no document.
        assert!(matches!(
            classify(
                false,
                "curl: (28) Operation timed out\n200",
                b"half".to_vec()
            ),
            Err(FetchError::Transient(_))
        ));
        assert_eq!(classify(true, "\n299", vec![]), Ok(vec![]));
        assert!(matches!(
            classify(true, "\n300", vec![]),
            Err(FetchError::Transient(_))
        ));
    }

    /// What `--write-out` makes curl print is what `classify` reads: curl
    /// expands `%{stderr}` to a switch of stream and `%{http_code}` to the
    /// status (curl.se/docs/manpage.html#-w).
    #[test]
    fn classify_reads_the_status_where_write_out_puts_it() {
        let curl_stderr = |err: &str, code: &str| {
            let (_, after) = WRITE_OUT
                .split_once("%{stderr}")
                .expect("the status goes to stderr");
            format!("{err}{}", after.replace("%{http_code}", code))
        };
        assert_eq!(
            classify(true, &curl_stderr("", "404"), vec![]),
            Err(FetchError::Gone { status: 404 })
        );
        assert!(matches!(
            classify(false, &curl_stderr("curl: (6) Could not resolve host", "000"), vec![]),
            Err(FetchError::Transient(e)) if e.contains("Could not resolve host")
        ));
    }

    /// The legacy half classifies its fetches by its own copy of these values,
    /// and both land in one report: a status one side calls dark and the other
    /// transient would split one issue's verdict.
    #[test]
    fn the_legacy_checker_classifies_fetches_by_the_same_values() {
        let py = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/check_upstream_drift.py"
        ))
        .expect("the legacy checker is readable");
        let value_of = |name: &str| {
            let line = py
                .lines()
                .find(|l| l.starts_with(&format!("{name} = ")))
                .unwrap_or_else(|| panic!("{name} is declared"));
            let mut nums: Vec<u64> = line
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|n| n.parse().ok())
                .collect();
            nums.sort_unstable();
            nums
        };
        let ours: Vec<u64> = PERMANENT_HTTP_STATUS
            .iter()
            .map(|&s| u64::from(s))
            .collect();
        assert_eq!(value_of("PERMANENT_HTTP_STATUS"), ours);
        assert_eq!(
            value_of("FETCH_TIMEOUT_S"),
            [super::super::FETCH_TIMEOUT.as_secs()]
        );
    }
}
