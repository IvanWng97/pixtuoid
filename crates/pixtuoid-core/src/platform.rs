//! Cross-platform home-dir resolution.
//!
//! On native Windows, `HOME` is normally unset — today's `env::var("HOME")`
//! sites silently fall back to `/tmp`, so the watcher would watch
//! `/tmp/.claude/projects` and never see a session. When Git Bash *does*
//! export a `HOME`, it's a POSIX-form path (`/c/Users/me`) that native Rust
//! code must not join onto — so `USERPROFILE` must win on Windows. On Unix,
//! `HOME` stays authoritative and behavior matches the old per-site
//! `env::var("HOME")` reads (one deliberate improvement: an empty `HOME` is
//! treated as unset).

/// USERPROFILE-first on Windows, HOME on Unix. See module doc for WHY.
pub(crate) fn user_home() -> String {
    resolve_home(
        cfg!(windows),
        std::env::var("USERPROFILE").ok(),
        std::env::var("HOME").ok(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    )
}

/// Pure resolution core, separated so the Windows branch is unit-testable
/// on any platform (it's string logic, not OS calls).
fn resolve_home(
    windows: bool,
    userprofile: Option<String>,
    home: Option<String>,
    temp_dir: String,
) -> String {
    let nonempty = |v: Option<String>| v.filter(|s| !s.is_empty());
    if windows {
        if let Some(p) = nonempty(userprofile) {
            return p;
        }
        // USERPROFILE is effectively always set on Windows; a lone HOME here
        // was set deliberately (MSYS users exporting a real Windows path).
        return nonempty(home).unwrap_or(temp_dir);
    }
    nonempty(home).unwrap_or_else(|| "/tmp".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn windows_prefers_userprofile_over_home() {
        // Git Bash exports HOME=/c/Users/me — must lose to USERPROFILE.
        let got = resolve_home(true, s(r"C:\Users\me"), s("/c/Users/me"), "T".into());
        assert_eq!(got, r"C:\Users\me");
    }

    #[test]
    fn windows_falls_back_to_home_then_tempdir() {
        assert_eq!(
            resolve_home(true, None, s("/c/Users/me"), "T".into()),
            "/c/Users/me"
        );
        assert_eq!(resolve_home(true, None, None, "T".into()), "T");
        // empty strings are treated as unset
        assert_eq!(resolve_home(true, s(""), s(""), "T".into()), "T");
    }

    #[test]
    fn unix_home_stays_authoritative_and_empty_home_is_unset() {
        assert_eq!(
            resolve_home(false, s(r"C:\ignored"), s("/Users/me"), "T".into()),
            "/Users/me"
        );
        assert_eq!(resolve_home(false, None, None, "T".into()), "/tmp");
        assert_eq!(resolve_home(false, None, s(""), "T".into()), "/tmp");
    }
}
