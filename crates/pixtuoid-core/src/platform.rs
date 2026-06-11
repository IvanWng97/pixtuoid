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
//!
//! Every env filter here is TRIM-based — empty or whitespace-only counts as
//! unset, matching the workspace convention (`install::io::nonempty_env` on
//! the binary side, the shim's `PIXTUOID_SOCKET` guard): these values are
//! paths, and a whitespace-only path can never be the absolute path the
//! contracts require. One semantics, so `XDG_CONFIG_HOME="   "` can't be
//! unset for the app config but set for the CLI config-dir resolution.

use std::path::{Path, PathBuf};

/// USERPROFILE-first on Windows, HOME on Unix. See module doc for WHY.
pub(crate) fn user_home() -> String {
    resolve_home(
        cfg!(windows),
        std::env::var("USERPROFILE").ok(),
        std::env::var("HOME").ok(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    )
}

/// `Option` variant of [`user_home`]: the SAME USERPROFILE-vs-HOME rule, but
/// with no host-level fallback — `None` when nothing is set so a caller can
/// supply its own (e.g. the installer's `install::io::user_home`, which keeps
/// its empty-string-is-unset contract by delegating here). This is the one
/// place that knows the precedence; both the `String` and `Option` shapes are
/// derived from [`resolve_user_home_opt`].
pub fn user_home_opt() -> Option<String> {
    resolve_user_home_opt(
        cfg!(windows),
        std::env::var("USERPROFILE").ok(),
        std::env::var("HOME").ok(),
    )
}

/// The Codex home dir, matching codex's own precedence (`codex-rs`
/// `find_codex_home`): `CODEX_HOME` if it's set to an EXISTING directory, else
/// `<user_home>/.codex`. Used for BOTH the rollout sessions root
/// (`source::codex::CodexSource::default_paths`) and the installer's
/// `config.toml` path — so a user who points Codex at a custom home is watched,
/// and gets hooks installed, in the right place on every platform.
pub(crate) fn codex_home() -> PathBuf {
    resolve_codex_home(std::env::var("CODEX_HOME").ok(), user_home())
}

/// Pure precedence core, separated so it's unit-testable without env mutation.
/// (`is_dir` still touches the filesystem.) On a set-but-absent `CODEX_HOME`,
/// upstream codex returns a FATAL error; we deliberately fall back to `~/.codex`
/// instead — benign for a visualizer, since codex itself won't run (and writes
/// no rollouts under that path) when its own home dir is missing.
fn resolve_codex_home(codex_home_env: Option<String>, home: String) -> PathBuf {
    if let Some(p) = codex_home_env.filter(|s| !s.trim().is_empty()) {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return pb;
        }
    }
    PathBuf::from(home).join(".codex")
}

/// Pure mapping of Go's `os.UserConfigDir()` for the platforms we ship, with
/// the OS and env values injected so every arm (incl. macOS) is unit-testable
/// on any host — the runtime `cfg!(target_os)` if-else couldn't test its
/// non-host arms. Pass `std::env::consts::OS` for `os`. Precedence per OS:
/// - macOS → `<home>/Library/Application Support`.
/// - Windows → `%APPDATA%` (Roaming) if set, else `<home>/AppData/Roaming`.
/// - else → `$XDG_CONFIG_HOME` if set, else `<home>/.config`.
///
/// Empty env values count as unset. `home` is the already-resolved user home
/// (see [`user_home`]) used for the relative fallbacks.
pub fn resolve_user_config_dir(
    os: &str,
    appdata: Option<String>,
    xdg: Option<String>,
    home: &Path,
) -> PathBuf {
    let nonempty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    match os {
        "macos" => home.join("Library/Application Support"),
        "windows" => nonempty(appdata)
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming")),
        _ => nonempty(xdg)
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config")),
    }
}

/// Pure resolution core, separated so the Windows branch is unit-testable
/// on any platform (it's string logic, not OS calls). Layers the host-level
/// fallback (`temp_dir` on Windows / `/tmp` on Unix) over the shared
/// `Option` precedence so the USERPROFILE-vs-HOME rule lives in ONE place.
fn resolve_home(
    windows: bool,
    userprofile: Option<String>,
    home: Option<String>,
    temp_dir: String,
) -> String {
    resolve_user_home_opt(windows, userprofile, home).unwrap_or_else(|| {
        if windows {
            temp_dir
        } else {
            "/tmp".into()
        }
    })
}

/// The single USERPROFILE-vs-HOME precedence, in its purest form: USERPROFILE
/// then HOME on Windows, HOME only on Unix, with empty strings treated as
/// unset and `None` when nothing resolves. Both [`resolve_home`] (String, with
/// a host fallback) and `install::io::user_home` (caller-supplied fallback)
/// derive from this — pure, so the Windows arm is unit-testable on any host.
pub fn resolve_user_home_opt(
    windows: bool,
    userprofile: Option<String>,
    home: Option<String>,
) -> Option<String> {
    let nonempty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    if windows {
        // USERPROFILE is effectively always set on Windows; a lone HOME here
        // was set deliberately (MSYS users exporting a real Windows path).
        return nonempty(userprofile).or_else(|| nonempty(home));
    }
    nonempty(home)
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
        // empty/whitespace strings are treated as unset
        assert_eq!(resolve_home(true, s(""), s(""), "T".into()), "T");
        assert_eq!(resolve_home(true, s("  "), s("  "), "T".into()), "T");
    }

    #[test]
    fn unix_home_stays_authoritative_and_empty_home_is_unset() {
        assert_eq!(
            resolve_home(false, s(r"C:\ignored"), s("/Users/me"), "T".into()),
            "/Users/me"
        );
        assert_eq!(resolve_home(false, None, None, "T".into()), "/tmp");
        assert_eq!(resolve_home(false, None, s(""), "T".into()), "/tmp");
        assert_eq!(resolve_home(false, None, s("  "), "T".into()), "/tmp");
    }

    #[test]
    fn user_home_opt_is_the_shared_precedence_without_a_host_fallback() {
        // Windows: USERPROFILE wins, then HOME, then None (no temp_dir layered on).
        assert_eq!(
            resolve_user_home_opt(true, s(r"C:\Users\me"), s("/c/Users/me")),
            s(r"C:\Users\me")
        );
        assert_eq!(
            resolve_user_home_opt(true, None, s("/c/Users/me")),
            s("/c/Users/me")
        );
        assert_eq!(resolve_user_home_opt(true, None, None), None);
        // empty/whitespace strings are unset on both axes.
        assert_eq!(resolve_user_home_opt(true, s(""), s("")), None);
        assert_eq!(resolve_user_home_opt(true, s("  "), s("  ")), None);
        // Unix: HOME only, empty = unset, None when absent.
        assert_eq!(
            resolve_user_home_opt(false, s(r"C:\ignored"), s("/Users/me")),
            s("/Users/me")
        );
        assert_eq!(resolve_user_home_opt(false, None, None), None);
        assert_eq!(resolve_user_home_opt(false, None, s("")), None);
    }

    #[test]
    fn user_config_dir_macos_is_application_support() {
        // macOS arm — unit-testable here despite this NOT being a macOS-cfg build.
        assert_eq!(
            resolve_user_config_dir(
                "macos",
                Some(r"C:\ignored".into()),
                Some("/ignored".into()),
                Path::new("/Users/me")
            ),
            PathBuf::from("/Users/me/Library/Application Support")
        );
    }

    #[test]
    fn user_config_dir_windows_prefers_appdata_then_roaming_fallback() {
        assert_eq!(
            resolve_user_config_dir(
                "windows",
                s(r"C:\Users\ada\AppData\Roaming"),
                None,
                Path::new(r"C:\Users\ada")
            ),
            PathBuf::from(r"C:\Users\ada\AppData\Roaming")
        );
        // empty APPDATA → relative fallback under home.
        assert_eq!(
            resolve_user_config_dir("windows", s(""), None, Path::new(r"C:\Users\ada")),
            PathBuf::from(r"C:\Users\ada").join("AppData/Roaming")
        );
        assert_eq!(
            resolve_user_config_dir("windows", None, None, Path::new(r"C:\Users\ada")),
            PathBuf::from(r"C:\Users\ada").join("AppData/Roaming")
        );
    }

    #[test]
    fn user_config_dir_linux_prefers_xdg_then_dot_config() {
        assert_eq!(
            resolve_user_config_dir("linux", None, s("/xdg/cfg"), Path::new("/home/u")),
            PathBuf::from("/xdg/cfg")
        );
        // empty/whitespace XDG → ~/.config.
        assert_eq!(
            resolve_user_config_dir("linux", None, s(""), Path::new("/home/u")),
            PathBuf::from("/home/u/.config")
        );
        assert_eq!(
            resolve_user_config_dir("linux", None, s("   "), Path::new("/home/u")),
            PathBuf::from("/home/u/.config")
        );
        assert_eq!(
            resolve_user_config_dir("linux", None, None, Path::new("/home/u")),
            PathBuf::from("/home/u/.config")
        );
        // any non-macos/windows OS string takes the XDG arm (the `_` catch-all).
        assert_eq!(
            resolve_user_config_dir("freebsd", None, s("/xdg/cfg"), Path::new("/home/u")),
            PathBuf::from("/xdg/cfg")
        );
    }

    #[test]
    fn codex_home_uses_env_when_it_points_at_an_existing_dir() {
        let tmp = std::env::temp_dir().join("pixtuoid-codex-home-exists-test");
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(
            resolve_codex_home(Some(tmp.to_string_lossy().into_owned()), "/home/u".into()),
            tmp
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn codex_home_falls_back_to_dot_codex_when_env_unset_empty_or_missing_dir() {
        let expected = PathBuf::from("/home/u").join(".codex");
        // Unset and empty both fall back.
        assert_eq!(resolve_codex_home(None, "/home/u".into()), expected);
        assert_eq!(
            resolve_codex_home(Some(String::new()), "/home/u".into()),
            expected
        );
        assert_eq!(
            resolve_codex_home(Some("   ".into()), "/home/u".into()),
            expected
        );
        // Set to a non-existent dir → fall back (matches upstream codex's gate).
        let missing = std::env::temp_dir().join("pixtuoid-codex-home-missing-xyz");
        let _ = std::fs::remove_dir_all(&missing);
        assert_eq!(
            resolve_codex_home(
                Some(missing.to_string_lossy().into_owned()),
                "/home/u".into()
            ),
            expected
        );
    }
}
