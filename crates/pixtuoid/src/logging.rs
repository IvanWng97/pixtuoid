//! Logging bootstrap: the tracing-subscriber install + the log-file path
//! resolution/rotation.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;

/// Install the global tracing subscriber. TUI mode (and `floating`, a long-running
/// GUI whose launching terminal would just collect noise) ALWAYS logs to the file —
/// the alternate screen owns the terminal, so the log file is the only place a
/// runtime error can surface — at a `warn` floor unless $RUST_LOG, $PIXTUOID_LOG or
/// --log-level raises it. Every other mode goes to stderr.
pub(crate) fn init(tui_active: bool, log_level: &'static str) {
    let rust_log = std::env::var("RUST_LOG").ok();
    let make_filter = || {
        EnvFilter::try_new(filter_directives(rust_log.as_deref(), log_level))
            .unwrap_or_else(|_| EnvFilter::new(log_level))
    };

    let wants_verbose = matches!(log_level, "debug" | "trace");
    // The env var's VALUE is the log file path, so an empty one would "enable" file
    // mode with an unopenable path — `nonempty_env` treats it as unset.
    let explicit_log_file = pixtuoid::install::nonempty_env("PIXTUOID_LOG").is_some();

    if tui_active {
        let rust_log_set = rust_log.as_deref().is_some_and(|v| !v.is_empty());
        let filter = if wants_verbose || explicit_log_file {
            make_filter()
        } else if rust_log_set {
            // Honor RUST_LOG, but floor the parse-failure fallback at warn (not
            // log_level) so the always-on file stays quiet by default.
            EnvFilter::try_new(filter_directives(rust_log.as_deref(), "warn"))
                .unwrap_or_else(|_| EnvFilter::new("warn"))
        } else {
            EnvFilter::new(match log_level {
                lvl @ ("warn" | "error") => lvl,
                _ => "warn",
            })
        };
        let path = log_file_path();
        // Rotation before the open: it no-ops when the file (and so its dir) is
        // absent, which `open_private_append` then creates.
        rotate_if_large(&path);
        match open_private_append(&path) {
            Ok(f) => {
                let writer = Arc::new(Mutex::new(f));
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(move || MutexFileWriter(writer.clone()))
                    .init();
            }
            Err(e) => {
                // The footer's "see log" advice would point at nothing — say so on
                // the pre-altscreen stderr channel rather than degrading silently.
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
/// `RUST_LOG` wins, otherwise the requested `log_level`. An empty `RUST_LOG` is
/// treated as unset — left as-is it parses to zero directives (everything OFF) and
/// silently defeats logging.
fn filter_directives<'a>(rust_log: Option<&'a str>, log_level: &'a str) -> &'a str {
    match rust_log {
        Some(v) if !v.is_empty() => v,
        _ => log_level,
    }
}

pub(crate) fn log_file_path() -> PathBuf {
    // Kept in lockstep with init()'s `explicit_log_file` read, so "file mode
    // enabled" and "which file" cannot disagree on a whitespace value.
    if let Some(p) = pixtuoid::install::nonempty_env("PIXTUOID_LOG") {
        return p;
    }
    if let Some(state) = pixtuoid::install::nonempty_abs_env("XDG_STATE_HOME") {
        return state.join("pixtuoid").join("log");
    }
    if let Some(home) = pixtuoid_core::platform::user_home_opt() {
        return home.join(".cache").join("pixtuoid").join("log");
    }
    // No home dir at all: the log must exist somewhere — it is the only runtime
    // diagnostics channel.
    std::env::temp_dir().join("pixtuoid.log")
}

/// Open a diagnostic sink for append, creating it — and any directory it needs —
/// OWNER-ONLY on Unix, the ONE opener the runtime log and the crash log share:
/// these are the most revealing artifacts pixtuoid creates (warn-floor records
/// carry full transcript paths and `AgentId`s that for some sources ARE the
/// workspace cwd; `--log-level debug` dumps the agent's own shell commands and
/// edited file paths).
///
/// TWO mechanisms, kept separately callable because they cover different cases:
/// [`create_owner_only_append`] binds the mode AT CREATION — race-free, no window
/// in which a co-located user can open the sink — and
/// [`pixtuoid::install::tighten_to_owner_only`] restates it on a sink an older
/// version created 0644, which the create mode cannot bind to. An already-existing
/// DIRECTORY keeps its mode: silently re-moding a user's `~/.cache` is not ours.
pub(crate) fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let f = create_owner_only_append(path)?;
    pixtuoid::install::tighten_to_owner_only(&f);
    Ok(f)
}

/// The CREATE half of [`open_private_append`]. Deliberately does NOT fchmod — a
/// test asserting the mode off THIS fn is asserting what the open established, not
/// what a follow-up chmod repaired.
fn create_owner_only_append(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(parent)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    pixtuoid::install::owner_only_create(&mut opts);
    opts.open(path)
}

/// One-deep rotation at startup (log → log.old) keeps the last two generations
/// without a rotation dependency. Accepted edge: with several instances sharing
/// the default path, one instance's startup rotation renames the file out from
/// under a running sibling (its fd follows; a later rotation strands it on an
/// unlinked inode).
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

fn rotate_if_large(path: &Path) {
    let too_large = std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES);
    if too_large {
        // APPEND ".old" rather than with_extension: a custom $PIXTUOID_LOG like
        // app.log must rotate to app.log.old (not clobber a sibling app.old), and a
        // path already ending in .old must not rename onto itself. OsString
        // concatenation, not format!/display(): display() is lossy on non-UTF-8
        // paths, and a U+FFFD-mangled target would silently break the rotation.
        let mut old = path.as_os_str().to_os_string();
        old.push(".old");
        let _ = std::fs::rename(path, &old);
    }
}

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

/// Serializes the bin crate's env-mutating tests: `crash.rs` and `logging.rs` both
/// drive `XDG_STATE_HOME`/`HOME`, and the bin's unit-test target runs in ONE
/// process under plain `cargo test`.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_fresh_diagnostic_sink_is_created_owner_only() {
        // Drives `create_owner_only_append`, NOT `open_private_append`: the latter's
        // follow-up fchmod would repair a dropped create mode.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("pixtuoid").join("log");
        drop(create_owner_only_append(&path).expect("opens"));
        assert_eq!(mode_of(&path), 0o600, "the sink must not inherit the umask");
        assert_eq!(
            mode_of(&dir.path().join("state").join("pixtuoid")),
            0o700,
            "nor the directory it is created in"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_older_versions_world_readable_sink_is_tightened_on_reopen() {
        // The create mode does not bind on a file that already exists, and these
        // sinks are never unlinked, so an upgrader's 0644 log stays exposed unless
        // `open_private_append` tightens it.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_private_append(&path).expect("reopens"));
        assert_eq!(mode_of(&path), 0o600, "a pre-existing sink is tightened");
    }

    #[test]
    fn empty_rust_log_falls_back_to_requested_level() {
        assert_eq!(filter_directives(Some(""), "debug"), "debug");
        assert_eq!(filter_directives(None, "debug"), "debug");
        assert_eq!(filter_directives(Some("trace"), "warn"), "trace");
        assert_eq!(
            filter_directives(Some("info,pixtuoid=debug"), "warn"),
            "info,pixtuoid=debug"
        );
    }

    #[test]
    fn nonempty_treats_empty_and_whitespace_as_unset() {
        use pixtuoid::install::nonempty;
        assert_eq!(nonempty(None), None);
        assert_eq!(nonempty(Some(String::new())), None);
        assert_eq!(nonempty(Some("   ".into())), None);
        assert_eq!(nonempty(Some("/state".into())), Some("/state".to_string()));
    }

    #[test]
    fn log_file_path_rejects_a_relative_xdg_state_home() {
        // Pins the CALL SITE, not just the primitive: a revert to plain
        // `nonempty_env` here would leak a relative log path.
        let _env = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_log = std::env::var_os("PIXTUOID_LOG");
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        std::env::remove_var("PIXTUOID_LOG");
        // Build the expectation with the SAME joins the impl uses — a hardcoded
        // separator would drift under Windows.
        let home = pixtuoid_core::platform::user_home_opt().expect("a home dir in the test env");
        let cache = home.join(".cache").join("pixtuoid").join("log");
        for rel in ["", "   ", "rel/state", "~/state"] {
            std::env::set_var("XDG_STATE_HOME", rel);
            assert_eq!(
                log_file_path(),
                cache,
                "relative XDG_STATE_HOME {rel:?} must fall back to ~/.cache"
            );
        }
        // A leading slash is not absolute on Windows, so pick per-platform; the impl
        // composes via `format!`, so mirror that.
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

        std::fs::write(&log, b"recent").unwrap();
        rotate_if_large(&log);
        assert!(log.exists(), "under-cap log must not rotate");

        // Sparse via set_len — no real 5MB write.
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
