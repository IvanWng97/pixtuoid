//! Unix half of the shell-hook-command quoting primitives.

/// POSIX single-quote a string so a shell treats it as one literal token.
/// Targets run the hook `command` under a shell, so an unquoted path containing
/// spaces would split into multiple args and the hook would never be found.
/// Unix-only: on Windows those targets use `windows_bare_hook_command` instead.
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
        // '\'' is close, escaped literal, reopen — the string stays ONE token.
        assert_eq!(shell_single_quote("a'b"), r#"'a'\''b'"#);
    }

    // The WRITE side here and the READ side (`verify::posix_unquote`, a separate
    // all-platform module that legitimately cannot be merged with this one) must
    // round-trip byte-for-byte, or the on-disk shim path can't be recovered → a
    // false "shim binary missing" in `doctor` / the Sources panel.
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
