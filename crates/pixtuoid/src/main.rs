// The lib's rules, for the same reasons (see its crate root).
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

use std::io::Write;

/// `ExitCode`, not `Result`: std would print the error chain raw, and pack and
/// config text reach it; [`pixtuoid::fatal_error_text`] strips it.
fn main() -> std::process::ExitCode {
    match pixtuoid::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Not `eprintln!`, which panics when the write fails (a broken
            // pipe): std's own error path ignores the failure, and a clean
            // failure must not turn into a crash.
            let _ = writeln!(std::io::stderr(), "{}", pixtuoid::fatal_error_text(&e));
            std::process::ExitCode::FAILURE
        }
    }
}
