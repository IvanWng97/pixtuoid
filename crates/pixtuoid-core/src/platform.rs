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
/// supply its own (the installer's config/install call sites call this directly
/// and keep their own per-call fallbacks). This is the one place that knows the
/// precedence; both the `String` and `Option` shapes are derived from
/// [`resolve_user_home_opt`].
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

/// The grok home dir, matching grok-build's own resolution (`xai-grok-config`
/// `paths.rs::grok_home`): `GROK_HOME` UNCONDITIONALLY when set (upstream takes
/// the env var without an exists-check and `create_dir_all`s it — the opposite
/// of codex's existing-dir gate), else `<home>/.grok` on EVERY OS (no XDG, no
/// APPDATA). Upstream resolves home via the re-stabilized
/// `std::env::home_dir()` (USERPROFILE on Windows, `$HOME` never consulted
/// there) — [`user_home`]'s USERPROFILE-first order is the faithful mirror.
/// Used for the sessions root, the installer's `hooks/pixtuoid.json` path, AND
/// the `active_sessions.json` liveness probe, so the three can never disagree.
pub(crate) fn grok_home() -> PathBuf {
    resolve_grok_home(std::env::var("GROK_HOME").ok(), user_home())
}

/// Pure precedence core, separated so it's unit-testable without env mutation.
fn resolve_grok_home(grok_home_env: Option<String>, home: String) -> PathBuf {
    match nonempty(grok_home_env) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(home).join(".grok"),
    }
}

/// An env var value counts as UNSET when it's empty or whitespace-only — a
/// whitespace-only path is never a valid home/config dir. The ONE spelling of
/// that "empty env == unset" rule, shared by every resolver below (the module's
/// "one trim semantics"), so the four call sites can't drift.
pub(crate) fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// Pure precedence core, separated so it's unit-testable without env mutation.
/// (`is_dir` still touches the filesystem.) On a set-but-absent `CODEX_HOME`,
/// upstream codex returns a FATAL error; we deliberately fall back to `~/.codex`
/// instead — benign for a visualizer, since codex itself won't run (and writes
/// no rollouts under that path) when its own home dir is missing.
fn resolve_codex_home(codex_home_env: Option<String>, home: String) -> PathBuf {
    if let Some(p) = nonempty(codex_home_env) {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return pb;
        }
    }
    PathBuf::from(home).join(".codex")
}

/// `HOME`-FIRST home resolution, then `USERPROFILE` on Windows — the precedence
/// used by the agent CLIs that hand-roll their own home dir as `$HOME ?? <OS
/// home>` instead of deferring to a stdlib that ignores `$HOME` on Windows. This
/// is the OPPOSITE of pixtuoid's generic [`user_home`] (`USERPROFILE`-first on
/// Windows, so the JSONL watcher never joins onto Git Bash's POSIX-form `HOME`).
///
/// The divergence is load-bearing: a Windows user who exports `HOME` (Git Bash /
/// MSYS2 / Cygwin) has these CLIs read their config under `%HOME%\…`, so pixtuoid
/// writing hooks to `%USERPROFILE%\…` would land them where the CLI never loads
/// them — installed, but no sprite. The installer routes the config path for the
/// HOME-first CLIs through THIS so the hooks reach the file the CLI reads on every
/// shell. `None` when nothing resolves (the installer maps it to "pass --config").
///
/// Source-verified HOME-first CLIs (the only consumers; both hook/daemon, no
/// watcher counterpart unlike [`codex_home`]):
/// - **CodeWhale** — `config::effective_home_dir` = `$HOME ?? dirs::home_dir()`.
/// - **OpenClaw** — `infra/home-dir.ts::resolveRawOsHomeDir` = `$HOME ??
///   $USERPROFILE ?? os.homedir()` (the installer layers `OPENCLAW_HOME` on top).
///
/// Every OTHER CLI uses its language stdlib (Rust `dirs::home_dir`, Go
/// `os.UserHomeDir`, Node `os.homedir`) — all `USERPROFILE`-first/only on Windows
/// — so they correctly use the generic [`user_home`], NOT this.
pub fn home_first_dir() -> Option<PathBuf> {
    resolve_home_first(
        cfg!(windows),
        std::env::var("HOME").ok(),
        std::env::var("USERPROFILE").ok(),
    )
    .map(PathBuf::from)
}

/// Pure precedence core (`HOME`-first, then `USERPROFILE` on Windows), separated
/// so the Windows arm is unit-testable on any host. Empty/whitespace counts as
/// unset. Unix with no `HOME` → `None`: we deliberately don't reach for
/// `dirs::home_dir`'s getpwuid fallback (HOME is effectively always set on Unix,
/// and the installer already errors helpfully when no home resolves).
fn resolve_home_first(
    windows: bool,
    home: Option<String>,
    userprofile: Option<String>,
) -> Option<String> {
    nonempty(home).or_else(|| if windows { nonempty(userprofile) } else { None })
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
/// a host fallback) and [`user_home_opt`] (the `Option` shape the installer
/// calls) derive from this — pure, so the Windows arm is unit-testable on any host.
pub fn resolve_user_home_opt(
    windows: bool,
    userprofile: Option<String>,
    home: Option<String>,
) -> Option<String> {
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
    fn nonempty_treats_empty_and_whitespace_as_unset() {
        // The ONE trim authority CLAUDE_CONFIG_DIR / COPILOT_HOME / codex_home
        // all route through — a whitespace-only value must resolve to None, not a
        // relative "  /…" path.
        assert_eq!(nonempty(None), None);
        assert_eq!(nonempty(s("")), None);
        assert_eq!(nonempty(s("   ")), None);
        assert_eq!(nonempty(s("\t \n")), None);
        assert_eq!(nonempty(s(" /home/u ")).as_deref(), Some(" /home/u "));
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
    fn grok_home_takes_env_unconditionally_even_when_missing() {
        // The deliberate difference from codex: grok honors GROK_HOME with NO
        // exists-check (upstream create_dir_all's it), so a not-yet-created
        // GROK_HOME must still win — the watcher/installer/probe will all
        // point where grok is ABOUT to write.
        let missing = std::env::temp_dir().join("pixtuoid-grok-home-missing-xyz");
        let _ = std::fs::remove_dir_all(&missing);
        assert_eq!(
            resolve_grok_home(
                Some(missing.to_string_lossy().into_owned()),
                "/home/u".into()
            ),
            missing
        );
    }

    #[test]
    fn grok_home_falls_back_to_dot_grok_when_env_unset_or_empty() {
        let expected = PathBuf::from("/home/u").join(".grok");
        assert_eq!(resolve_grok_home(None, "/home/u".into()), expected);
        assert_eq!(
            resolve_grok_home(Some(String::new()), "/home/u".into()),
            expected
        );
        assert_eq!(
            resolve_grok_home(Some("   ".into()), "/home/u".into()),
            expected
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
    fn home_first_is_home_then_userprofile_on_windows() {
        // HOME wins on Windows when set — the WHOLE point: the HOME-first CLIs
        // (CodeWhale, OpenClaw) resolve home as $HOME first, so the installer must
        // mirror that, not pixtuoid's generic USERPROFILE-first.
        assert_eq!(
            resolve_home_first(true, s(r"C:\Users\me"), s(r"C:\Users\other")),
            s(r"C:\Users\me")
        );
        // Windows, HOME unset → USERPROFILE (== dirs::home_dir()/os.homedir()),
        // which is why the native cmd/PowerShell case (HOME unset) was correct.
        assert_eq!(
            resolve_home_first(true, None, s(r"C:\Users\me")),
            s(r"C:\Users\me")
        );
        // empty/whitespace HOME is unset → falls to USERPROFILE on Windows.
        assert_eq!(
            resolve_home_first(true, s("  "), s(r"C:\Users\me")),
            s(r"C:\Users\me")
        );
        // neither set → None (installer maps to the "pass --config" error).
        assert_eq!(resolve_home_first(true, None, None), None);
    }

    #[test]
    fn home_first_is_home_only_on_unix() {
        // Unix: HOME only; USERPROFILE is ignored, and no HOME → None.
        assert_eq!(
            resolve_home_first(false, s("/Users/me"), s(r"C:\ignored")),
            s("/Users/me")
        );
        assert_eq!(resolve_home_first(false, None, s(r"C:\ignored")), None);
        assert_eq!(resolve_home_first(false, s(""), None), None);
    }

    // The per-source Windows home-basis CONTRACT (the audit, codified). A source
    // whose CLI resolves home $HOME-first MUST route through `home_first_dir`; one
    // whose CLI uses a stdlib home (USERPROFILE-first/only on Windows) uses the
    // generic `user_home`. Getting this wrong = "installed but no sprite" on a
    // HOME-exporting Windows shell (Git Bash/MSYS2). Upstream-verified bases:
    //   HOME-first  : codewhale (config::effective_home_dir),
    //                 openclaw  (infra/home-dir.ts::resolveRawOsHomeDir)
    //   USERPROFILE : codex (dirs::home_dir), reasonix (Go os.UserHomeDir),
    //                 claude/cursor/copilot (Node os.homedir), opencode (XDG +
    //                 os.homedir), antigravity (deliberate, see its default_paths)
    #[test]
    fn home_first_and_generic_user_home_diverge_on_windows_with_home_set() {
        // The regression guard for the Windows "installed but no sprite" class:
        // with HOME set on Windows the GENERIC resolver picks USERPROFILE while the
        // HOME-first resolver (codewhale/openclaw installers) picks HOME. If these
        // ever stopped diverging, a "simplification" back to the generic home would
        // silently re-break CodeWhale + OpenClaw on Windows.
        let home = s("/c/Users/me");
        let userprofile = s(r"C:\Users\me");
        assert_eq!(
            resolve_user_home_opt(true, userprofile.clone(), home.clone()),
            userprofile,
            "generic resolver is USERPROFILE-first on Windows"
        );
        assert_eq!(
            resolve_home_first(true, home.clone(), userprofile),
            home,
            "HOME-first resolver picks HOME — the two MUST diverge here"
        );
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
