//! Hook-`command` quoting for CLIs whose hook runner SHELLS the command
//! (`/bin/sh -c` on Unix, `cmd.exe /C` on Windows) — Codex and Reasonix both do.
//!
//! The OS halves live in sibling modules (the `source/hook/` split pattern):
//! `unix` (POSIX single-quoting) and `windows` (cmd-safety + DOS 8.3 FFI). The
//! windows module is compiled on EVERY OS so its injected-`short_path` pure core
//! unit-tests on macOS; only its FFI is `#[cfg(windows)]`. Claude is NOT a caller
//! — it writes the exec form (absolute `.exe` + args), the opposite strategy.

use anyhow::Result;

#[cfg(unix)]
mod unix;
// Compiled on all platforms: the cmd-safety decision core (with the 8.3 resolver
// injected) is pure and unit-tests on macOS; only the Win32 FFI inside is
// `#[cfg(windows)]`.
mod windows;

/// The OS-correct hook `command` string for a shell-running CLI. The single OS
/// fork for that strategy, so a new cmd.exe-shelling CLI pays zero platform cost.
///
/// - **Unix**: env-prefix form `PIXTUOID_SOURCE=<source> '<path>'` (single-quoted
///   for spaces).
/// - **Windows**: BARE exec form `<path> --source <source>` (cmd.exe can't express
///   the env-prefix; the source rides as the shim's `--source` flag, and a
///   space/metacharacter path falls back to its DOS 8.3 short name, rejecting only
///   if 8.3 is disabled).
pub(crate) fn shell_hook_command(path: &str, source: &str) -> Result<String> {
    #[cfg(windows)]
    {
        windows::windows_bare_hook_command(path, source)
    }
    #[cfg(unix)]
    {
        Ok(format!(
            "PIXTUOID_SOURCE={source} {}",
            unix::shell_single_quote(path)
        ))
    }
}
