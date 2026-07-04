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
pub(crate) mod unix;
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
        // Mirror the Windows arm's source guard: `source` is interpolated
        // UNQUOTED into the `/bin/sh -c` env-prefix, so it must be a plain
        // identifier or it could inject a command (`x; rm -rf ~`). Hardcoded
        // today ("codex"/"reasonix"), but this keeps the shared seam
        // injection-proof for any future dynamic source. The path is always
        // single-quoted, so only `source` needs this allowlist.
        if let Some(bad) = source
            .chars()
            .find(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_'))
        {
            anyhow::bail!("internal: hook source {source:?} has a shell-unsafe character {bad:?}");
        }
        Ok(format!(
            "PIXTUOID_SOURCE={source} {}",
            unix::shell_single_quote(path)
        ))
    }
}

/// The OS-correct hook `command` for a CLI that ARGV-EXECs the command (quote-aware
/// word-split, NO shell) instead of running it under `/bin/sh -c`. Hermes is the
/// first such caller: a live capture proved it word-splits (respecting quotes) and
/// execs the first token directly — the env-prefix form (`PIXTUOID_SOURCE=hermes
/// '<path>'`) is treated as a program literally named `PIXTUOID_SOURCE=hermes`
/// ("command not found"), and shell metacharacters (`|`, `>`) arrive as literal
/// argv. So the source rides as the shim's `--source` FLAG, never an env prefix.
///
/// - **Unix**: `'<path>' --source <source>` — the path single-quoted (Hermes honors
///   POSIX quotes, so a spaced path stays one token), the flag as a bare argv token.
/// - **Windows**: BARE `<path> --source <source>` via the shared
///   `windows::windows_bare_hook_command` (DOS 8.3 short-name for a space/metacharacter
///   path, else reject — #195). CAPTURE-GATED: Hermes's Windows arg-splitting is
///   unverified; the bare form (no env prefix, no quotes) is the safest for any
///   splitter and mirrors the Codex/Reasonix Windows form.
pub(crate) fn exec_hook_command(path: &str, source: &str) -> Result<String> {
    // `source` is interpolated as a bare argv token (`--source <source>`); keep it a
    // plain identifier so it can't inject a second token, mirroring the shell form's
    // guard. The Windows arm's `windows_bare_hook_command` applies its own cmd-safety
    // check, so this covers only the Unix arm's bare interpolation.
    if let Some(bad) = source
        .chars()
        .find(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_'))
    {
        anyhow::bail!("internal: hook source {source:?} has an unsafe character {bad:?}");
    }
    #[cfg(windows)]
    {
        windows::windows_bare_hook_command(path, source)
    }
    #[cfg(unix)]
    {
        Ok(format!(
            "{} --source {source}",
            unix::shell_single_quote(path)
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn exec_form_quotes_path_and_appends_source_flag() {
        // Unix argv-exec form: single-quoted path + bare `--source` flag (no env
        // prefix — Hermes execs directly, so the prefix would be a bogus program).
        assert_eq!(
            exec_hook_command("/opt/bin/pixtuoid-hook", "hermes").unwrap(),
            "'/opt/bin/pixtuoid-hook' --source hermes"
        );
        assert_eq!(
            exec_hook_command("/Users/Jane Doe/bin/pixtuoid-hook", "hermes").unwrap(),
            "'/Users/Jane Doe/bin/pixtuoid-hook' --source hermes"
        );
    }

    #[test]
    fn exec_form_rejects_a_shell_unsafe_source_name() {
        for bad in ["x; rm -rf ~", "a b", "a$x", "a&b"] {
            assert!(exec_hook_command("/opt/bin/pixtuoid-hook", bad).is_err());
        }
    }

    #[test]
    fn valid_source_keeps_the_env_prefix_form_byte_for_byte() {
        // Behavior-preserving: a normal source name still yields the exact
        // pre-refactor string (no quoting/validation artifacts).
        assert_eq!(
            shell_hook_command("/opt/bin/pixtuoid-hook", "codex").unwrap(),
            "PIXTUOID_SOURCE=codex '/opt/bin/pixtuoid-hook'"
        );
        assert_eq!(
            shell_hook_command("/opt/bin/pixtuoid-hook", "claude-code").unwrap(),
            "PIXTUOID_SOURCE=claude-code '/opt/bin/pixtuoid-hook'"
        );
    }

    #[test]
    fn rejects_a_shell_unsafe_source_name() {
        for bad in [
            "codex; rm -rf ~",
            "x`id`",
            "a b",
            "a$x",
            "a&b",
            "a|b",
            "a(b)",
        ] {
            assert!(
                shell_hook_command("/opt/bin/pixtuoid-hook", bad).is_err(),
                "source {bad:?} must be rejected"
            );
        }
    }
}
