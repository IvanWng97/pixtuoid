//! Public surface for the pixtuoid binary's internals — exposed because
//! `main.rs`, the examples and the integration tests are separate crates.

// A print macro panics when its reader leaves (`| head`): CLI and headless
// output goes through a `CliOut`, a stderr notice through a `let _ = writeln!`.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]

pub mod aa_text;
pub(crate) mod audio;
pub mod cli;
pub mod config;
pub mod doctor;

#[cfg(test)]
mod drift_surface;
pub mod floating;
pub(crate) mod focus;
pub(crate) mod graphics;
/// The one graphics item that IS public API: `Cmd` and `RunConfig` carry it, and
/// `main.rs` is a separate crate. Everything else in the module — the plan, the
/// probe, the scale rule — is `pub(crate)`, because a `pub` item on a published
/// crate is the one thing a follow-up cannot quietly undo.
pub use graphics::GraphicsMode;
pub mod init_pack;
pub mod install;
#[cfg(feature = "graphics")]
#[doc(hidden)]
pub mod pacing;
pub mod runtime;
pub mod setup;
pub mod sources;
pub mod term;
pub mod tui;
pub mod validate;
pub(crate) mod version;

/// Strip control characters (Cc) and bidi controls from an untrusted string
/// before it reaches a terminal: such a value can carry control bytes that
/// reposition the cursor or inject escapes, or reorder the text shown. One
/// chokepoint, so the policy can't drift across its call sites.
///
/// The non-TUI `tracing` stream cannot be filtered at the SINK: the subscriber
/// emits its own SGR for level coloring, so a sink-side filter could not tell
/// our escapes from an injected one. Its untrusted values are stripped where
/// they ENTER a record instead — here by this fn's callers, and in the core
/// crate by `pixtuoid_core::source::decoder::display_safe`, a per-crate copy of
/// this predicate pinned to it by
/// `the_bidi_table_matches_pixtuoid_cores_display_safe`.
#[doc(hidden)]
pub fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect()
}

/// `s` stripped line by line, the lines rejoined with `sep`, so a multi-line
/// message keeps its shape.
#[doc(hidden)]
pub fn strip_lines(s: &str, sep: &str) -> String {
    s.lines()
        .map(strip_control_chars)
        .collect::<Vec<_>>()
        .join(sep)
}

/// A path as a terminal shows it, stripped. A path from config, env or a
/// hand-editable hook command is stripped where it enters the text that
/// quotes it; per-output-site stripping already missed the `doctor` stdout
/// path once.
#[doc(hidden)]
pub fn display_path(p: &std::path::Path) -> String {
    strip_control_chars(&p.display().to_string())
}

/// A CLI command's stdout. A reader that leaves early (`| head`) is not a
/// failure: a write into a broken pipe succeeds, marks the stream
/// [`closed`](Self::closed) and drops every later write, so the command still
/// completes its side effects and exits with its own verdict, where `println!`
/// would panic. Any other write error still fails.
#[doc(hidden)]
pub struct CliOut<W> {
    inner: W,
    closed: bool,
}

impl<W: std::io::Write> CliOut<W> {
    /// `inner`, with a broken pipe treated as the reader leaving.
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            closed: false,
        }
    }

    /// Whether the reader has gone away.
    pub(crate) fn closed(&self) -> bool {
        self.closed
    }

    fn absorb_broken_pipe<T>(
        &mut self,
        result: std::io::Result<T>,
        on_close: T,
    ) -> std::io::Result<T> {
        match result {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                self.closed = true;
                Ok(on_close)
            }
            other => other,
        }
    }
}

impl<W: std::io::Write> std::io::Write for CliOut<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.closed {
            return Ok(buf.len());
        }
        let result = self.inner.write(buf);
        self.absorb_broken_pipe(result, buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let result = self.inner.flush();
        self.absorb_broken_pipe(result, ())
    }
}

/// Stdout for a CLI command's output; see [`CliOut`].
#[doc(hidden)]
pub fn cli_stdout() -> CliOut<std::io::Stdout> {
    CliOut::new(std::io::stdout())
}

/// A fatal error's chain as `main` returning `Err` would print it, stripped line
/// by line so the chain keeps its shape. Mechanism for `main`, not contract.
#[doc(hidden)]
pub fn fatal_error_text(e: &anyhow::Error) -> String {
    format!("Error: {}", strip_lines(&format!("{e:?}"), "\n"))
}

/// Run `cmd` and collect its output, or kill and reap it at `timeout`:
/// [`Command::output`](std::process::Command::output) has no timeout. `None`
/// on a spawn or wait error too.
///
/// The caller pipes the streams it reads (a spawned child inherits by default)
/// and expects little output: nothing drains the pipes until the child exits,
/// so a child that fills one blocks and runs into the timeout.
pub(crate) fn output_within(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::time::{Duration, Instant};
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(_) => return None,
        }
    }
    child.wait_with_output().ok()
}

/// The Unicode Bidi_Control characters. `char::is_control` covers only category
/// Cc; these are Cf and slip through — yet they REORDER displayed text in a
/// terminal (the "Trojan Source" class, CVE-2021-42574).
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'                    // ALM
            | '\u{200E}'..='\u{200F}' // LRM, RLM
            | '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
            | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
    )
}

/// Test-only `tracing` capture for asserting on what reaches the log sink.
#[cfg(test)]
pub(crate) mod test_capture {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    pub(crate) struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl MakeWriter<'_> for Buf {
        type Writer = Buf;
        fn make_writer(&self) -> Buf {
            self.clone()
        }
    }

    /// Capture in the line format the bin's `logging::init` writes to its log
    /// file (no ANSI), so an assertion is validated against the REAL format, not
    /// an assumed one.
    pub(crate) fn capture(f: impl FnOnce()) -> String {
        let buf = Buf::default();
        let sub = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buf.clone())
            .finish();
        tracing::subscriber::with_default(sub, f);
        let bytes = buf.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }
}

#[cfg(test)]
pub(crate) mod test_io {
    /// Fails ONE write, the `fail_at`th (0-based), with `kind`, and accepts
    /// every other: a stream that recovers is what shows a caller stopped
    /// writing rather than kept hitting the error.
    pub(crate) struct FailOnce {
        pub(crate) fail_at: usize,
        pub(crate) kind: std::io::ErrorKind,
        pub(crate) calls: usize,
        pub(crate) written: Vec<u8>,
    }

    impl FailOnce {
        pub(crate) fn new(fail_at: usize, kind: std::io::ErrorKind) -> Self {
            Self {
                fail_at,
                kind,
                calls: 0,
                written: Vec::new(),
            }
        }
    }

    impl std::io::Write for FailOnce {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls - 1 == self.fail_at {
                return Err(self.kind.into());
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_io::FailOnce;

    #[test]
    fn a_broken_pipe_closes_the_stream_and_drops_later_writes() {
        use std::io::Write;
        let mut out = CliOut::new(FailOnce::new(1, std::io::ErrorKind::BrokenPipe));
        writeln!(out, "first").expect("written");
        assert!(!out.closed());
        writeln!(out, "second").expect("a broken pipe is not an error");
        assert!(out.closed());
        writeln!(out, "third").expect("dropped");
        assert_eq!(out.inner.written, b"first\n");
    }

    #[test]
    fn any_other_write_error_still_fails() {
        use std::io::Write;
        let mut out = CliOut::new(FailOnce::new(0, std::io::ErrorKind::StorageFull));
        assert!(writeln!(out, "x").is_err());
        assert!(!out.closed());
    }

    #[test]
    fn display_path_strips_control_chars_from_a_hostile_path() {
        let hostile = std::path::Path::new("/x/\x1b]0;pwned\x07\x1b[31mhook");
        let got = display_path(hostile);
        assert!(!got.chars().any(|c| c.is_control()), "{got:?}");
        assert!(got.contains("hook") && got.contains("/x/"), "{got:?}");
    }

    #[test]
    fn a_fatal_error_keeps_its_chain_and_loses_its_controls() {
        let e = anyhow::anyhow!("bad key \u{1b}[31m\u{202e}")
            .context("failed to load sprite pack from \"/p\"");
        let text = fatal_error_text(&e);
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{202e}'),
            "{text:?}"
        );
        assert!(
            text.starts_with("Error: failed to load sprite pack"),
            "{text:?}"
        );
        assert!(text.contains("\nCaused by:\n"), "{text:?}");
    }

    #[test]
    fn strips_c0_and_c1_controls() {
        assert_eq!(strip_control_chars("a\x1b[31mb\x07c"), "a[31mbc");
        assert_eq!(strip_control_chars("x\u{0085}y"), "xy"); // C1 NEL
    }

    #[test]
    fn strips_trojan_source_bidi_controls() {
        assert_eq!(strip_control_chars("safe\u{202E}gpj.exe"), "safegpj.exe");
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}',
            '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert_eq!(
                strip_control_chars(&format!("a{c}b")),
                "ab",
                "U+{:04X} not stripped",
                c as u32
            );
        }
    }

    #[test]
    fn keeps_ordinary_text_and_non_bidi_unicode() {
        let s = "hello wörld café 日本語 🦞";
        assert_eq!(strip_control_chars(s), s);
    }

    /// Round-trip one probe name through `pixtuoid-core`'s copy of the predicate,
    /// via `decode_hook_payload`'s unsupported-event `bail!`.
    fn core_display_safe(name: &str) -> String {
        const PREFIX: &str = "unsupported hook_event_name: ";
        let v = serde_json::json!({"hook_event_name": name, "session_id": "s"});
        let e = pixtuoid_core::source::decoder::decode_hook_payload(v)
            .expect_err("an unregistered hook_event_name must bail");
        let msg = e.to_string();
        msg.strip_prefix(PREFIX)
            .unwrap_or_else(|| panic!("core's bail wording moved; re-point this pin: {msg:?}"))
            .to_string()
    }

    #[test]
    fn the_bidi_table_matches_pixtuoid_cores_display_safe() {
        // Two per-crate copies of one security table (core's is `pub(crate)`), so
        // pin them BEHAVIOURALLY: sweep every codepoint either side could
        // plausibly gain or lose — the Cc block and its boundaries, DEL/C1, and
        // the Cf neighbourhoods on both sides of each bidi range.
        let candidates = (0x00..=0x20u32)
            .chain(0x7E..=0xA1)
            .chain(0x061A..=0x061E)
            .chain(0x200B..=0x2010)
            .chain(0x2028..=0x2030)
            .chain(0x2065..=0x206F);
        for cp in candidates {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let probe = format!("PixtuoidParity{c}Probe");
            assert_eq!(
                strip_control_chars(&probe),
                core_display_safe(&probe),
                "U+{cp:04X}: the binary's strip_control_chars and pixtuoid-core's \
                 display_safe disagree — the two Cf/Cc tables have drifted apart",
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_outlives_its_timeout_is_killed_and_reaped() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = tempfile::TempDir::new().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo $$ > \"$0\"; exec sleep 30")
            .arg(&pid_file)
            .stdout(Stdio::piped());
        let started = Instant::now();
        assert!(output_within(&mut cmd, Duration::from_millis(200)).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .expect("the child wrote its pid")
            .trim()
            .parse()
            .expect("a pid");
        // ESRCH only once the child is reaped: a zombie still answers kill 0.
        // SAFETY: signal 0 sends nothing; it only checks the pid exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the child {pid} was left running or unreaped");
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_exits_in_time_yields_its_output_and_status() {
        use std::process::{Command, Stdio};
        use std::time::Duration;
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi; exit 3"]).stdout(Stdio::piped());
        let out = output_within(&mut cmd, Duration::from_secs(5)).expect("exits in time");
        assert_eq!(out.stdout, b"hi\n");
        assert_eq!(out.status.code(), Some(3));
    }
}
