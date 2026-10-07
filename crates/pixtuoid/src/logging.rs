//! Logging bootstrap: installs the tracing subscriber over the sink
//! [`pixtuoid::run_log`] opens.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use pixtuoid::doctor::DriftSeen;
use pixtuoid::run_log::LogLocation;
use std::time::SystemTime;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

/// Install the global tracing subscriber, and return what its drift layer
/// sees. TUI mode (and `floating`, a long-running GUI whose launching terminal
/// would just collect noise) ALWAYS logs to a file — the alternate screen owns
/// the terminal, so the log file is the only place a runtime error can surface
/// — at a `warn` floor unless $RUST_LOG, $PIXTUOID_LOG or --log-level raises
/// it. Every other mode goes to stderr.
pub(crate) fn init(tui_active: bool, log_level: &'static str) -> DriftSeen {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let rust_log = pixtuoid_core::platform::text_env("RUST_LOG");
    let make_filter = || {
        EnvFilter::try_new(filter_directives(rust_log.as_deref(), log_level))
            .unwrap_or_else(|_| EnvFilter::new(log_level))
    };

    let wants_verbose = matches!(log_level, "debug" | "trace");
    let log_at = LogLocation::from_env();
    let explicit_log_file = matches!(log_at, LogLocation::File(_));
    let drift = DriftSeen::default();
    let registry = tracing_subscriber::registry().with(drift.layer());

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
        match log_at.open_sink(SystemTime::now()) {
            Ok(f) => {
                let writer = Arc::new(Mutex::new(f));
                registry
                    .with(
                        fmt_layer()
                            .with_ansi(false)
                            .with_writer(move || MutexFileWriter(writer.clone()))
                            .with_filter(filter),
                    )
                    .init();
            }
            Err((path, e)) => {
                // The footer's "see log" advice would point at nothing — say so on
                // the pre-altscreen stderr channel rather than degrading silently.
                let _ = writeln!(std::io::stderr(), "{}", log_open_failure(&path, &e));
                registry.init();
            }
        }
    } else {
        registry
            .with(
                fmt_layer()
                    .with_writer(std::io::stderr)
                    .with_filter(make_filter()),
            )
            .init();
    }
    drift
}

/// The formatting layer with its internal-error report off: on a failed
/// write that report is an `eprintln!`, which panics when stderr is the pipe
/// that failed, and lands on the TUI's alternate screen when the log file is.
fn fmt_layer<S>() -> tracing_subscriber::fmt::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().log_internal_errors(false)
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

/// The path is [`LogLocation::from_env`]'s, which env decides, so it is stripped
/// before it reaches the terminal.
fn log_open_failure(path: &Path, e: &std::io::Error) -> String {
    format!(
        "⚠ pixtuoid: cannot open log file {} ({e}) — runtime warnings will not be recorded",
        pixtuoid::display_path(path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_open_failure_notice_strips_the_path() {
        let notice = log_open_failure(
            Path::new("/tmp/\u{1b}]0;pwned\u{7}\u{202e}.log"),
            &std::io::ErrorKind::PermissionDenied.into(),
        );
        assert_eq!(pixtuoid::strip_control_chars(&notice), notice);
        assert!(notice.contains("/tmp/]0;pwned.log"), "{notice:?}");
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
}
