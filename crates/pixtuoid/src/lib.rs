//! Public surface for the pixtuoid binary's internals — exposed so
//! examples and integration tests can import them. The `main.rs` binary is
//! the primary entry point.

pub mod cli;
pub mod config;
pub mod doctor;
pub mod floating;
pub mod init_pack;
pub mod install;
pub mod runtime;
pub mod tui;
pub mod validate;
pub mod version;

/// Strip ASCII/Unicode control characters from an untrusted string before it
/// reaches a terminal sink (the headless `println!` summary, the `doctor`
/// stdout report, the Sources-panel path). Untrusted wire values (agent labels,
/// sampled CLI output, config paths) can carry control bytes that would
/// reposition the cursor or inject escapes; one chokepoint so the policy can't
/// drift between the three call sites (R0615-06).
pub(crate) fn strip_control_chars(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`HOME` / `XDG_CONFIG_HOME` / …). The crate's unit tests share one
/// test binary, so two env-mutating tests can otherwise race under plain
/// `cargo test` (nextest isolates per-process, but the `justfile` falls back to
/// `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
