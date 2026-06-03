//! Public surface for the pixtuoid binary's internals — exposed so
//! examples and integration tests can import them. The `main.rs` binary is
//! the primary entry point.

pub mod cli;
pub mod config;
pub mod init_pack;
pub mod install;
pub mod runtime;
pub mod tui;
pub mod validate;
pub mod version;

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`HOME` / `XDG_CONFIG_HOME` / …). The crate's unit tests share one
/// test binary, so two env-mutating tests can otherwise race under plain
/// `cargo test` (nextest isolates per-process, but the `justfile` falls back to
/// `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
