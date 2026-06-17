use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use pixtuoid::cli::{Cli, Cmd, SourceArgs};
use pixtuoid::{config, doctor, floating, init_pack, install, runtime, validate};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    install_crash_hook();
    let (log_level, cli_theme, cmd) = Cli::parse().cmd_or_default();
    // The typed LogLevel's as_str is exactly the old free-string levels, so
    // every filter built below is unchanged — the enum only moved typo
    // rejection to the clap seam (a typo used to parse as a bogus EnvFilter
    // TARGET directive that silently filtered everything off, #157 class).
    let log_level: &'static str = log_level.as_str();
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

    // Log routing:
    //   TUI mode: ALWAYS log to the file (#157) — the alternate screen owns
    //     the terminal, so the log file is the only place a runtime error
    //     ("source died", decode failures) can surface. The default floor is
    //     `warn`; $RUST_LOG, $PIXTUOID_LOG, or --log-level raise/shape it.
    //     Crash reporting is handled separately by the panic hook.
    //   Non-TUI (--headless, validate-pack, init-pack): stderr.
    //   `floating`: file-log like the TUI — it's a long-running GUI; tracing spam
    //     into the launching terminal would be noise (config warnings still
    //     eprintln to that terminal via build_run_config before the window opens).
    let tui_active = matches!(&cmd, Cmd::Run { headless, .. } if !*headless)
        || matches!(&cmd, Cmd::Floating { .. });
    let wants_verbose = matches!(log_level, "debug" | "trace");
    // The env var's VALUE is the log file path — an empty value would
    // "enable" file mode with an unopenable path; treat it as unset.
    let explicit_log_file = std::env::var("PIXTUOID_LOG").is_ok_and(|v| !v.is_empty());

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

    match cmd {
        Cmd::Run {
            source,
            max_desks: cli_max_desks,
            headless,
        } => {
            let rc = build_run_config(cli_theme.as_deref(), source, cli_max_desks, headless)?;
            runtime::run(rc)
        }
        Cmd::Floating { source } => {
            // Floating reuses the TUI run prelude (theme/pack/pets/sources/log) but is
            // never headless and has no desk cap — capacity is seeded from the window.
            let rc = build_run_config(cli_theme.as_deref(), source, None, false)?;
            floating::run(rc)
        }
        Cmd::ValidatePack { pack_dir } => validate::validate_pack(&pack_dir),
        Cmd::InitPack { dest, force } => init_pack::init_pack(&dest, force),
        Cmd::Doctor => doctor::run(&log_file_path()),
    }
}

/// Resolve the shared [`runtime::RunConfig`] from CLI args + the on-disk config —
/// the common prelude for `run` (TUI) and `floating` (window). On a non-headless
/// launch it also surfaces config warnings + the broken-install preflight to
/// stderr (visible in the launching terminal / pre-altscreen scrollback, #87/#309).
fn build_run_config(
    cli_theme: Option<&str>,
    source: SourceArgs,
    cli_max_desks: Option<usize>,
    headless: bool,
) -> Result<runtime::RunConfig> {
    let SourceArgs {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
    } = source;
    let cfg_path = config::config_path();
    let mut cfg_warnings = Vec::new();
    let cfg = config::load(&cfg_path, &mut cfg_warnings);
    let theme = config::resolve_theme(&cfg, cli_theme, &mut cfg_warnings)?;
    // The config seam's twin of the clap range(1..) guard: a config max-desks = 0
    // is ignored with a collected warning (eager `.or` argument on purpose — the
    // warning must fire even when the CLI flag overrides).
    let desk_cap = cli_max_desks.or(config::resolve_max_desks(&cfg, &mut cfg_warnings));
    let pack_dir = config::resolve_pack_dir(&cfg, pack_dir);
    let pets = config::resolve_pets(&cfg, &mut cfg_warnings);
    // The connected-source set the office gates sprites on: explicit `[sources]`
    // flags win; an absent flag migrates from the install state (target-bearing
    // source connected iff its hooks are installed; a no-target source connected).
    let connected = config::resolve_connected(&cfg, |src| {
        install::target::by_source(src).map(install::has_hooks)
    });
    if !headless {
        // Config problems must reach the user's eyes, not only the log file (#87):
        // stderr BEFORE any alternate screen / window. Headless already has a
        // stderr tracing subscriber, so re-printing there would duplicate.
        for w in &cfg_warnings {
            eprintln!("⚠ pixtuoid: {w}");
        }
        warn_broken_installs(&connected);
    }
    Ok(runtime::RunConfig {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
        desk_cap,
        headless,
        config_path: cfg_path,
        theme,
        pets,
        connected,
        log_path: Some(log_file_path()),
    })
}

/// Boot preflight (#309): warn (stderr) when a CONNECTED source's hooks are
/// installed but structurally BROKEN — it would render zero sprites with no other
/// hint, so the fully-passive user who never opens the Sources panel still learns.
/// Routed through the SHARED `doctor::diagnose` rollup (empty log = skip the drift
/// scan; warns on broken installs only) so this surface can't drift from the panel
/// and the CLI report. Iterates TARGETS, not REGISTERED_SOURCES: only an
/// install-bearing source can be install-BROKEN.
fn warn_broken_installs(connected: &std::collections::HashSet<String>) {
    for &t in install::target::TARGETS {
        if !connected.contains(t.core_source) {
            continue;
        }
        let diag = doctor::diagnose(t.core_source, "");
        if diag.is_broken() {
            let issues = diag
                .install
                .as_ref()
                .map(|v| v.issues.join("; "))
                .unwrap_or_default();
            eprintln!(
                "⚠ pixtuoid: {} hooks are installed but BROKEN: {issues} — \
                 reconnect in the Sources panel (press s)",
                t.core_source
            );
        }
    }
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Same ordering contract as tui::teardown_terminal: mouse-capture
        // restore must precede disable_raw_mode (see the WHY there).
        let _ = crossterm::execute!(
            std::io::stderr(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();

        let version = env!("CARGO_PKG_VERSION");
        let crash_path = crash_log_path();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let panic_msg = extract_panic_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();

        let bt = std::backtrace::Backtrace::force_capture();
        let bt_str = bt.to_string();

        let mut report = String::new();
        report.push_str(&format!("pixtuoid v{version} crashed at {timestamp}\n"));
        report.push_str(&format!("{panic_msg}\n  at {location}\n\n"));
        report.push_str(&bt_str);
        report.push('\n');

        if let Some(parent) = crash_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            use std::io::Write;
            let _ = f.write_all(report.as_bytes());
        }

        let issue_url = build_issue_url(version, &panic_msg, &location, &bt_str, &crash_path);

        eprintln!("\n\x1b[1;31mpixtuoid v{version} crashed — sorry about that.\x1b[0m\n");
        eprintln!("  \x1b[2m{panic_msg}\x1b[0m");
        eprintln!("  \x1b[2mat {location}\x1b[0m\n");
        eprintln!("  \x1b[1mHelp fix it\x1b[0m — open this link to file a pre-filled bug report");
        eprintln!("  (panic + backtrace already included, no typing needed):\n");
        eprintln!("  \x1b[4m{issue_url}\x1b[0m\n");
        eprintln!(
            "  Full backtrace saved to \x1b[2m{}\x1b[0m",
            crash_path.display()
        );
        eprintln!("  \x1b[2m(attach if the reviewer asks — the link above only carries a truncated trace)\x1b[0m\n");
    }));
}

#[allow(deprecated)]
fn extract_panic_message(info: &std::panic::PanicInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic".to_string()
}

fn build_issue_url(
    version: &str,
    panic_msg: &str,
    location: &str,
    backtrace: &str,
    crash_path: &std::path::Path,
) -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let title_msg = if panic_msg.len() > 80 {
        let cut = truncate_to_char_boundary(panic_msg, 80);
        format!("{}…", &panic_msg[..cut])
    } else {
        panic_msg.to_string()
    };
    let title = format!("Crash: {title_msg}");

    // Truncate backtrace to keep URL under GitHub's 8191-byte limit.
    const MAX_BT: usize = 1500;
    let bt_body = if backtrace.len() > MAX_BT {
        let cut = truncate_to_char_boundary(backtrace, MAX_BT);
        format!(
            "{}\n\n... truncated — see {} for full trace",
            &backtrace[..cut],
            crash_path.display()
        )
    } else {
        backtrace.to_string()
    };

    let body = format!(
        "## Environment\n\
         - **Version:** {version}\n\
         - **OS:** {os}/{arch}\n\n\
         ## Panic\n\
         ```\n{panic_msg}\n  at {location}\n```\n\n\
         ## Backtrace\n\
         ```\n{bt_body}\n```\n"
    );

    format!(
        "https://github.com/IvanWng97/pixtuoid/issues/new?labels=crash-report&title={}&body={}",
        percent_encode(&title),
        percent_encode(&body),
    )
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

fn crash_log_path() -> PathBuf {
    // Empty XDG_STATE_HOME = unset (see io::nonempty_env) — left unfiltered,
    // "" yields the root-absolute `/pixtuoid/...` (unwritable for non-root).
    if let Some(state) = pixtuoid::install::io::nonempty_env("XDG_STATE_HOME") {
        return PathBuf::from(format!("{state}/pixtuoid/crash.log"));
    }
    if let Some(home) = pixtuoid_core::platform::user_home_opt() {
        return PathBuf::from(home)
            .join(".cache")
            .join("pixtuoid")
            .join("crash.log");
    }
    std::env::temp_dir().join("pixtuoid-crash.log")
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

fn log_file_path() -> PathBuf {
    // Empty value = unset (the value is the PATH, not an on/off toggle; an
    // empty path would silently fail to open and log nothing).
    if let Ok(p) = std::env::var("PIXTUOID_LOG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(state) = pixtuoid::install::io::nonempty_env("XDG_STATE_HOME") {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

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
        // The shared io::nonempty filter backs XDG_STATE_HOME here: an
        // unfiltered empty value would route the crash log / runtime log to
        // the root-absolute `/pixtuoid/...`.
        use pixtuoid::install::io::nonempty;
        assert_eq!(nonempty(None), None);
        assert_eq!(nonempty(Some(String::new())), None);
        assert_eq!(nonempty(Some("   ".into())), None);
        assert_eq!(nonempty(Some("/state".into())), Some("/state".to_string()));
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

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), 5);
        assert_eq!(
            &"hello world"[..truncate_to_char_boundary("hello world", 5)],
            "hello"
        );
    }

    #[test]
    fn truncate_multibyte_boundary() {
        // "café" is 5 bytes: c(1) a(1) f(1) é(2)
        let s = "café";
        assert_eq!(s.len(), 5);
        // Cutting at byte 4 lands inside the é (2-byte char starting at 3)
        let cut = truncate_to_char_boundary(s, 4);
        assert_eq!(cut, 3);
        assert_eq!(&s[..cut], "caf");
    }

    #[test]
    fn truncate_beyond_length() {
        assert_eq!(truncate_to_char_boundary("short", 100), 5);
    }

    #[test]
    fn percent_encode_ascii() {
        assert_eq!(percent_encode("hello"), "hello");
        assert_eq!(percent_encode("a b"), "a%20b");
    }

    #[test]
    fn percent_encode_special_chars() {
        assert_eq!(percent_encode("#&="), "%23%26%3D");
        assert_eq!(percent_encode("a\nb"), "a%0Ab");
    }

    #[test]
    fn build_issue_url_starts_with_github() {
        let url = build_issue_url(
            "0.4.0",
            "test panic",
            "file.rs:1:1",
            "bt",
            Path::new("/tmp/x"),
        );
        assert!(url.starts_with("https://github.com/IvanWng97/pixtuoid/issues/new?"));
        assert!(url.contains("labels=crash-report"));
        assert!(url.contains("title="));
        assert!(url.contains("body="));
    }

    #[test]
    fn build_issue_url_truncates_long_backtrace() {
        let long_bt = "x".repeat(2000);
        let url = build_issue_url("0.4.0", "msg", "loc", &long_bt, Path::new("/tmp/x"));
        // URL should stay under GitHub's 8191 byte limit
        assert!(url.len() < 8191);
    }
}
