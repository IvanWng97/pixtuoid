//! Test-only stand-in process for the unit tests that watch, probe or mark a
//! live pid. `pub(crate)`, so integration tests (`tests/`) can't reach it.

/// A child that outlives any test's assertions; the test kills and reaps it.
/// `ping` on Windows, which ships no `sleep`, whose `timeout` refuses a
/// redirected stdin and whose `waitfor` is not on every SKU.
pub(crate) fn sleeper() -> std::process::Child {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "61", "127.0.0.1"]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = std::process::Command::new("sleep");
        c.arg("60");
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn a sleeper")
}
