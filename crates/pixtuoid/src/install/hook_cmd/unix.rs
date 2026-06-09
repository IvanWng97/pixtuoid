//! Unix half of the shell-hook-command quoting primitives.

/// POSIX single-quote a string so a shell treats it as one literal token —
/// embedded single quotes become `'\''`. Codex and Reasonix both run the hook
/// `command` under a shell, so an unquoted path containing spaces would split
/// into multiple args and the hook would never be found. Unix-only: on Windows
/// these targets use the windows-half `windows_bare_hook_command` (cmd.exe, not
/// `sh`).
pub(super) fn shell_single_quote(s: &str) -> String {
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
}
