//! Unix half of the shell-hook-command quoting primitives.

/// POSIX single-quote a string so a shell treats it as one literal token —
/// embedded single quotes become `'\''`. Codex and Reasonix both run the hook
/// `command` under a shell, so an unquoted path containing spaces would split
/// into multiple args and the hook would never be found (Claude's explicit
/// `--hook-path` arm reuses it for the same reason). Unix-only: on Windows
/// these targets use the windows-half `windows_bare_hook_command` (cmd.exe, not
/// `sh`).
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_path() {
        assert_eq!(
            shell_single_quote("/opt/bin/pixtuoid-hook"),
            "'/opt/bin/pixtuoid-hook'"
        );
    }

    #[test]
    fn quotes_path_with_spaces() {
        assert_eq!(
            shell_single_quote("/Users/Jane Doe/bin/pixtuoid-hook"),
            "'/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    #[test]
    fn escapes_embedded_single_quote() {
        // A single quote becomes '\'' — close, escaped literal, reopen — so the
        // whole string stays one shell token.
        assert_eq!(shell_single_quote("a'b"), r#"'a'\''b'"#);
    }

    // The WRITE side (`shell_single_quote`, here — cfg(unix)) and the READ side
    // (`verify::posix_unquote`, a different, all-platform module) must round-trip
    // byte-for-byte, or the on-disk shim path can't be recovered → a false "shim
    // binary missing" in `doctor` / the Sources panel. They legitimately can't be
    // merged (the read side is not cfg-forked — see verify.rs's module doc), so
    // pin the write/read PAIR directly instead of trusting they stay in sync.
    #[test]
    fn quoting_round_trips_through_the_verify_reader() {
        use crate::install::verify::posix_unquote;
        for p in [
            "/opt/bin/pixtuoid-hook",
            "/Users/Jane Doe/bin/pixtuoid-hook",
            "/opt/it's mine/pixtuoid-hook",
            "/a'b'c/pixtuoid-hook",
            "/weird; path,&(x)/pixtuoid-hook",
        ] {
            assert_eq!(
                posix_unquote(&shell_single_quote(p)),
                p,
                "shell_single_quote → posix_unquote drifted for {p:?}"
            );
        }
    }
}
