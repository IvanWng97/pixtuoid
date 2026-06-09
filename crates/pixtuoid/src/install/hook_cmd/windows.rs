//! Windows half of the shell-hook-command quoting primitives.
//!
//! The cmd-safety DECISION CORE (`CMD_UNSAFE` / `first_cmd_unsafe_char` /
//! `resolve_windows_command`) is compiled on every OS — with the 8.3 resolver
//! injected — so every branch unit-tests on macOS without the Win32 FFI. Only
//! the FFI itself (`short_path_windows`) and the public entry point that wires
//! it in (`windows_bare_hook_command`) are `#[cfg(windows)]`.

use anyhow::Result;

/// Characters that are special to cmd.exe's command-line parser (so they're
/// unsafe in an UNQUOTED hook path): a space TRUNCATES the command; a
/// separator/redirect/escape/expansion char injects or redirects. (`!` is
/// deliberately excluded — it's special only under delayed expansion, `cmd
/// /V:ON`, which the codex/reasonix hook runners don't enable.)
const CMD_UNSAFE: &[char] = &[' ', '"', '&', '|', '<', '>', '(', ')', '^', '%'];

#[cfg_attr(not(windows), allow(dead_code))]
fn first_cmd_unsafe_char(p: &str) -> Option<char> {
    p.chars().find(|c| CMD_UNSAFE.contains(c))
}

/// The BARE Windows hook `command` for a CLI whose hook runner shells via
/// `cmd.exe /C` — Codex AND Reasonix both do (verified: codex-rs
/// `command_runner.rs`; reasonix `internal/hook/hook.go:414` `shellInvocation`).
/// Form: `<path> --source <name>`. The source rides as the shim's `--source`
/// flag because cmd.exe has no `VAR=value cmd` env-prefix and neither CLI injects
/// a per-hook env. The path is UNQUOTED: a quoted path can't survive `cmd /C`
/// (the host's argv-quoting escapes `"`→`\"`, which cmd then mangles), so cmd
/// PARSES the path — meaning any cmd-special char in it would truncate (`space`,
/// #195) or inject (`& | < > ( ) ^ %` — `C:\Users\a&b\h.exe --source x` splits on
/// `&` and cmd runs the relative tail from the CWD). When the resolved path has
/// such a char we substitute its DOS 8.3 SHORT name (`C:\PROGRA~1\…`, which is
/// space- and metacharacter-free by construction) and only REJECT if the short
/// name is unavailable (8.3 generation disabled on the volume). One place for
/// both targets so the guard can't drift.
#[cfg(windows)]
pub(super) fn windows_bare_hook_command(resolved_path: &str, source: &str) -> Result<String> {
    resolve_windows_command(resolved_path, source, short_path_windows)
}

/// Pure decision core, with the 8.3 resolver injected so every branch is testable
/// without the Win32 FFI (and on any OS).
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_command(
    path: &str,
    source: &str,
    short_path: impl FnOnce(&str) -> Option<String>,
) -> Result<String> {
    // Defense-in-depth: `source` is interpolated into the command alongside the
    // path. It's a hardcoded "codex"/"reasonix" at every call site today, but this
    // is a general guard — screen it for the same cmd-unsafe chars rather than
    // trust the caller, so the command string can never be made injectable here.
    if let Some(bad) = first_cmd_unsafe_char(source) {
        anyhow::bail!(
            "internal: hook source name {source:?} contains a cmd-unsafe character {bad:?}"
        );
    }
    let Some(bad) = first_cmd_unsafe_char(path) else {
        return Ok(format!("{path} --source {source}"));
    };
    // Try the DOS 8.3 short form — space/metacharacter-free by construction.
    // (When 8.3 generation is disabled on the volume, GetShortPathNameW returns
    // the long path unchanged, so we re-check and fall through to the reject.)
    if let Some(s) = short_path(path) {
        if first_cmd_unsafe_char(&s).is_none() {
            return Ok(format!("{s} --source {source}"));
        }
    }
    anyhow::bail!(
        "pixtuoid-hook is at a path containing {bad:?} ({path}) that the cmd.exe /C hook \
         runner can't safely invoke, and no DOS 8.3 short name is available (8.3 \
         generation is disabled on this volume). Install pixtuoid to a path of ordinary \
         characters (e.g. %USERPROFILE%\\.cargo\\bin or the npm global prefix) and re-run \
         `install-hooks`. (Tracking: #195.)"
    );
}

/// The DOS 8.3 short path for an EXISTING path via `GetShortPathNameW` (two-call
/// length-then-fill pattern). Returns `None` on any failure — a missing path, or
/// a volume with 8.3 generation disabled (then the API yields the long path,
/// which the caller re-checks).
#[cfg(windows)]
fn short_path_windows(long: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let wide: Vec<u16> = std::ffi::OsStr::new(long)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string. Passing a null
    // out-buffer with length 0 is the documented "return required size (incl. NUL)"
    // probe; it writes nothing and returns 0 on failure.
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` has `needed` u16 slots; the API writes at most `needed-1` chars
    // plus a NUL and returns the count written (excl. NUL), or 0 on failure.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), needed) };
    if written == 0 || written >= needed {
        return None;
    }
    String::from_utf16(&buf[..written as usize]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 8.3 decision logic, with the short-path resolver injected so every
    // branch runs on any OS (no FFI). The real GetShortPathNameW is smoke-tested
    // separately below on windows-test.
    #[test]
    fn windows_command_is_bare_for_a_clean_path() {
        let c = resolve_windows_command(r"C:\tools\pixtuoid-hook.exe", "codex", |_| {
            panic!("short_path must NOT be called for a clean path")
        });
        assert_eq!(c.unwrap(), r"C:\tools\pixtuoid-hook.exe --source codex");
    }

    #[test]
    fn windows_command_uses_8dot3_short_form_when_path_has_a_space() {
        let c =
            resolve_windows_command(r"C:\Program Files\x\pixtuoid-hook.exe", "reasonix", |_| {
                Some(r"C:\PROGRA~1\x\PIXTUO~1.EXE".to_string())
            });
        assert_eq!(c.unwrap(), r"C:\PROGRA~1\x\PIXTUO~1.EXE --source reasonix");
    }

    #[test]
    fn windows_command_rejects_when_8dot3_is_unavailable() {
        // 8.3 disabled → resolver returns the long (still-unsafe) path → reject.
        let long = resolve_windows_command(r"C:\Program Files\x\h.exe", "codex", |p| {
            Some(p.to_string())
        });
        assert!(long.is_err());
        // resolver fails outright (missing path) → reject.
        let none = resolve_windows_command(r"C:\a&b\h.exe", "codex", |_| None);
        let err = none.unwrap_err().to_string();
        assert!(
            err.contains("cmd.exe") && err.contains("ordinary characters"),
            "reject message must stay actionable: {err}"
        );
    }

    #[test]
    fn windows_command_rejects_a_cmd_unsafe_source() {
        // `source` is interpolated too — a metacharacter-bearing source is rejected
        // even with a perfectly clean path (defense-in-depth; never injectable here).
        let c = resolve_windows_command(r"C:\tools\hook.exe", "co&dex", |_| {
            panic!("must reject on source before touching the short-path resolver")
        });
        assert!(c.unwrap_err().to_string().contains("source name"));
    }

    // Smoke-test the real FFI: for an EXISTING dir it returns Some(non-empty),
    // whether or not 8.3 is enabled (disabled → the long path, still Some). Pins
    // that the two-call length-then-fill pattern doesn't panic / mis-size.
    #[cfg(windows)]
    #[test]
    fn short_path_windows_resolves_an_existing_dir() {
        let tmp = std::env::temp_dir();
        let got = short_path_windows(&tmp.to_string_lossy());
        assert!(
            got.is_some_and(|s| !s.is_empty()),
            "GetShortPathNameW must resolve an existing dir to a non-empty path"
        );
    }
}
