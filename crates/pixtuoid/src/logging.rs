//! Logging bootstrap (#157): the tracing-subscriber install + the log-file path
//! resolution/rotation. Binary-crate module (lifted out of `main.rs`); `main()`
//! computes `tui_active` from the parsed command and calls [`init`] once.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

/// Install the global tracing subscriber. Log routing:
///   TUI mode: ALWAYS log to the file (#157) — the alternate screen owns
///     the terminal, so the log file is the only place a runtime error
///     ("source died", decode failures) can surface. The default floor is
///     `warn`; $RUST_LOG, $PIXTUOID_LOG, or --log-level raise/shape it.
///     Crash reporting is handled separately by the panic hook.
///   Non-TUI (--headless, validate-pack, init-pack): stderr.
///   `floating`: file-log like the TUI — it's a long-running GUI; tracing spam
///     into the launching terminal would be noise (config warnings still
///     eprintln to that terminal via build_run_config before the window opens).
pub(crate) fn init(tui_active: bool, log_level: &'static str) {
    // RUST_LOG wins only when set to a NON-EMPTY value; an empty RUST_LOG
    // parses as Ok(zero directives) = everything OFF, which would silently
    // defeat logging on the verbose / $PIXTUOID_LOG / --headless paths that
    // route through make_filter (the #157 silent-diagnostics class). The
    // empty=unset normalization is pinned by `filter_directives` + its test.
    let rust_log = std::env::var("RUST_LOG").ok();
    let make_filter = || {
        EnvFilter::try_new(filter_directives(rust_log.as_deref(), log_level))
            .unwrap_or_else(|_| EnvFilter::new(log_level))
    };

    let wants_verbose = matches!(log_level, "debug" | "trace");
    // The env var's VALUE is the log file path — an empty (or whitespace-only)
    // value would "enable" file mode with an unopenable path; treat it as unset
    // via the ONE empty-as-unset env filter (io::nonempty), the same trim
    // semantics XDG_STATE_HOME / XDG_CONFIG_HOME / PIXTUOID_HOOK already use.
    let explicit_log_file = pixtuoid::install::nonempty_env("PIXTUOID_LOG").is_some();

    if tui_active {
        // Explicit verbosity keeps today's semantics (the full --log-level /
        // RUST_LOG filter); the always-on default floors at warn so the file
        // captures errors without accumulating info-level noise. RUST_LOG
        // set-but-EMPTY parses as Ok(zero directives) = everything OFF —
        // treat it as unset, or it silently defeats the always-on floor
        // (the exact silent-failure class #157 exists to kill).
        let rust_log_set = rust_log.as_deref().is_some_and(|v| !v.is_empty());
        let filter = if wants_verbose || explicit_log_file {
            make_filter()
        } else if rust_log_set {
            // Honor RUST_LOG, but floor the parse-failure fallback at warn (not
            // log_level) so the always-on file stays quiet by default. Routed
            // through filter_directives so an empty RUST_LOG can't silence this
            // path either if the rust_log_set guard above ever changes.
            EnvFilter::try_new(filter_directives(rust_log.as_deref(), "warn"))
                .unwrap_or_else(|_| EnvFilter::new("warn"))
        } else {
            EnvFilter::new(match log_level {
                lvl @ ("warn" | "error") => lvl,
                _ => "warn",
            })
        };
        let path = log_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        rotate_if_large(&path);
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                let writer = Arc::new(Mutex::new(f));
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(move || MutexFileWriter(writer.clone()))
                    .init();
            }
            Err(e) => {
                // The footer's "see log" advice would point at nothing —
                // say so on the pre-altscreen stderr channel rather than
                // degrading silently (the #157 failure class).
                eprintln!(
                    "⚠ pixtuoid: cannot open log file {} ({e}) — runtime warnings will not be recorded",
                    path.display()
                );
            }
        }
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(make_filter())
            .with_writer(std::io::stderr)
            .init();
    }
}

/// The tracing directive string to build the `EnvFilter` from: a NON-EMPTY
/// `RUST_LOG` wins, otherwise the requested `log_level`. An empty `RUST_LOG`
/// is treated as unset — left as-is it parses to zero directives (everything
/// OFF) and silently defeats logging (#157). Pure (env read by the caller) so
/// the normalization is unit-testable without mutating process env.
fn filter_directives<'a>(rust_log: Option<&'a str>, log_level: &'a str) -> &'a str {
    match rust_log {
        Some(v) if !v.is_empty() => v,
        _ => log_level,
    }
}

pub(crate) fn log_file_path() -> PathBuf {
    // Empty/whitespace-only value = unset (the value is the PATH, not an on/off
    // toggle; an empty path would silently fail to open and log nothing) — the
    // shared io::nonempty semantics, kept in lockstep with the
    // `explicit_log_file` read in init() so "file mode enabled" and "which
    // file" can't disagree on a whitespace value.
    if let Some(p) = pixtuoid::install::nonempty_env("PIXTUOID_LOG") {
        return PathBuf::from(p);
    }
    if let Some(state) = pixtuoid::install::nonempty_abs_env("XDG_STATE_HOME") {
        return PathBuf::from(format!("{state}/pixtuoid/log"));
    }
    if let Some(home) = pixtuoid_core::platform::user_home_opt() {
        return PathBuf::from(home)
            .join(".cache")
            .join("pixtuoid")
            .join("log");
    }
    // No home dir at all: mirror crash_log_path's temp fallback — the log
    // must exist somewhere, it is the only runtime diagnostics channel (#157).
    std::env::temp_dir().join("pixtuoid.log")
}

/// The append-only log was opt-in before #157; now that it is always on in
/// TUI mode it needs a growth bound. One-deep rotation at startup (log →
/// log.old) keeps the last two generations without a rotation dependency.
/// Known accepted edge: with several pixtuoid instances sharing the default
/// path, one instance's startup rotation renames the file out from under a
/// running sibling (its fd follows; a later rotation strands it on an
/// unlinked inode) — startup-only one-deep rotation is the deliberate
/// no-dependency trade-off.
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

fn rotate_if_large(path: &Path) {
    let too_large = std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES);
    if too_large {
        // APPEND ".old" rather than with_extension: a custom $PIXTUOID_LOG
        // like app.log must rotate to app.log.old (not clobber a sibling
        // app.old), and a path already ending in .old must not rename onto
        // itself (a no-op that would never rotate). OsString concatenation,
        // not format!/display(): display() is lossy on non-UTF-8 paths, and
        // a U+FFFD-mangled target would silently break the rotation.
        let mut old = path.as_os_str().to_os_string();
        old.push(".old");
        let _ = std::fs::rename(path, &old);
    }
}

/// Adapter that gives `tracing-subscriber` a `Write`-able file behind a Mutex.
struct MutexFileWriter(Arc<Mutex<std::fs::File>>);

impl std::io::Write for MutexFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .flush()
    }
}

/// Serializes the bin crate's env-mutating tests. `crash.rs` and `logging.rs`
/// both drive `XDG_STATE_HOME`/`HOME` to pin their XDG path fallbacks, and the
/// bin's unit-test target runs in ONE process under plain `cargo test`, so a
/// single shared lock keeps the two suites from racing on that global env.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rust_log_falls_back_to_requested_level() {
        // The bug: an empty-but-set RUST_LOG must be treated as unset, not as
        // "everything off". (#157 — the make_filter path lacked this guard.)
        assert_eq!(filter_directives(Some(""), "debug"), "debug");
        assert_eq!(filter_directives(None, "debug"), "debug");
        // A non-empty RUST_LOG still wins, simple level or full directive.
        assert_eq!(filter_directives(Some("trace"), "warn"), "trace");
        assert_eq!(
            filter_directives(Some("info,pixtuoid=debug"), "warn"),
            "info,pixtuoid=debug"
        );
    }

    #[test]
    fn nonempty_treats_empty_and_whitespace_as_unset() {
        // `io::nonempty` directly backs the `PIXTUOID_LOG` read (which may be a
        // relative path); XDG_STATE_HOME instead routes through `nonempty_abs_env`
        // (see `log_file_path_rejects_a_relative_xdg_state_home`).
        use pixtuoid::install::nonempty;
        assert_eq!(nonempty(None), None);
        assert_eq!(nonempty(Some(String::new())), None);
        assert_eq!(nonempty(Some("   ".into())), None);
        assert_eq!(nonempty(Some("/state".into())), Some("/state".to_string()));
    }

    #[test]
    fn log_file_path_rejects_a_relative_xdg_state_home() {
        // Pins the CALL SITE (not just the primitive): a relative/empty
        // XDG_STATE_HOME → ~/.cache fallback, so a revert to plain `nonempty_env`
        // here would leak a relative log path and red this test.
        let _env = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_log = std::env::var_os("PIXTUOID_LOG");
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        std::env::remove_var("PIXTUOID_LOG");
        // Expected fallback: the SAME user_home_opt + joins the impl uses (no
        // hardcoded separator — Windows-safe per the path-string sharp edge).
        let home = pixtuoid_core::platform::user_home_opt().expect("a home dir in the test env");
        let cache = PathBuf::from(home)
            .join(".cache")
            .join("pixtuoid")
            .join("log");
        for rel in ["", "   ", "rel/state", "~/state"] {
            std::env::set_var("XDG_STATE_HOME", rel);
            assert_eq!(
                log_file_path(),
                cache,
                "relative XDG_STATE_HOME {rel:?} must fall back to ~/.cache"
            );
        }
        // Absolute wins (platform-specific — leading-slash isn't absolute on
        // Windows). The impl composes it via `format!`, so mirror that.
        let abs = if cfg!(windows) { "C:/state" } else { "/state" };
        std::env::set_var("XDG_STATE_HOME", abs);
        assert_eq!(
            log_file_path(),
            PathBuf::from(format!("{abs}/pixtuoid/log"))
        );
        match saved_log {
            Some(v) => std::env::set_var("PIXTUOID_LOG", v),
            None => std::env::remove_var("PIXTUOID_LOG"),
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }

    #[test]
    fn rotate_if_large_rotates_once_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");

        // Small file: untouched.
        std::fs::write(&log, b"recent").unwrap();
        rotate_if_large(&log);
        assert!(log.exists(), "under-cap log must not rotate");

        // Over the cap (sparse via set_len — no real 5MB write).
        let f = std::fs::OpenOptions::new().write(true).open(&log).unwrap();
        f.set_len(LOG_ROTATE_BYTES + 1).unwrap();
        drop(f);
        rotate_if_large(&log);
        assert!(!log.exists(), "over-cap log rotates away");
        assert!(
            dir.path().join("log.old").exists(),
            "one prior generation is kept"
        );
    }

    #[test]
    fn rotate_if_large_appends_old_to_dotted_custom_paths() {
        // A custom $PIXTUOID_LOG like app.log must rotate to app.log.old —
        // replacing the extension would clobber an unrelated app.old, and a
        // *.old path would rename onto itself and never rotate.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("app.log");
        let f = std::fs::File::create(&log).unwrap();
        f.set_len(LOG_ROTATE_BYTES + 1).unwrap();
        drop(f);
        rotate_if_large(&log);
        assert!(!log.exists());
        assert!(
            dir.path().join("app.log.old").exists(),
            ".old is appended, not substituted"
        );
    }
}
