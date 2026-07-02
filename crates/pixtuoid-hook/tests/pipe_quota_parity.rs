//! Pins the shim's 1 MiB stdin-cap arithmetic AGAINST the daemon's Windows
//! named-pipe in-buffer quota, cross-crate.
//!
//! The shim (producer) caps stdin at `STDIN_CAP = (1 << 20) - STAMP_HEADROOM`
//! (src/main.rs) so a stamped wire line always fits the daemon's
//! `IN_BUFFER_SIZE = 1 << 20` (pixtuoid-core src/source/hook/windows.rs) and
//! the shim's sync write can't stall on quota (stall → 200ms watchdog → the
//! event is silently dropped). Each crate pins its own copy against the same
//! literal (the shim's `stdin_cap_plus_headroom_equals_the_pipe_quota` unit
//! test; core's const IS the literal), but two parallel literal pins only hold
//! if a reviewer notices the sibling when one side changes — the same drift
//! class the socket path had before core's `socket_path_parity` (#93).
//!
//! Unlike the socket-path twin, the daemon's source can't be `#[path]`-included
//! here: windows.rs needs tokio/windows-sys, and the shim must stay
//! dependency-free (nothing may slow or bloat it). So this pins the three
//! DEFINITION LINES textually instead — a value can't change without editing
//! its definition line, so any one-sided change fails here until the sibling
//! (and this pin) are updated in lockstep. Deliberately fail-loud on a
//! rename/move too: a drift guard that silently stops guarding is worse than
//! one that asks to be re-pointed.
//!
//! Workspace-only (reads a sibling crate's source): excluded from the
//! published tarball via this crate's Cargo.toml `exclude`.

const SHIM_MAIN: &str = include_str!("../src/main.rs");
const DAEMON_WINDOWS: &str = include_str!("../../pixtuoid-core/src/source/hook/windows.rs");

#[test]
fn shim_stdin_cap_and_daemon_pipe_quota_stay_in_lockstep() {
    assert!(
        SHIM_MAIN.contains("const STAMP_HEADROOM: u64 = 256;"),
        "shim STAMP_HEADROOM definition changed/moved — re-check the daemon's \
         IN_BUFFER_SIZE (pixtuoid-core source/hook/windows.rs) still covers \
         STDIN_CAP + STAMP_HEADROOM, then update this pin"
    );
    assert!(
        SHIM_MAIN.contains("const STDIN_CAP: u64 = (1 << 20) - STAMP_HEADROOM;"),
        "shim STDIN_CAP definition changed/moved — re-check it still equals \
         the daemon's IN_BUFFER_SIZE minus STAMP_HEADROOM, then update this pin"
    );
    assert!(
        DAEMON_WINDOWS.contains("const IN_BUFFER_SIZE: u32 = 1 << 20;"),
        "daemon IN_BUFFER_SIZE definition changed/moved — re-check the shim's \
         STDIN_CAP + STAMP_HEADROOM still equals it, then update this pin"
    );
}
