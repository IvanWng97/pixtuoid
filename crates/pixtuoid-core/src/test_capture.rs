//! Test-only tracing capture: one buffer writer + the two capture shapes the
//! in-crate unit-test mods share. `pub(crate)`, so integration tests (`tests/`)
//! can't reach it.

use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl CaptureWriter {
    pub(crate) fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&self) -> CaptureWriter {
        self.clone()
    }
}

fn subscriber(
    writer: CaptureWriter,
    level: tracing::Level,
) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(level)
        .with_ansi(false)
        .without_time()
        .finish()
}

pub(crate) fn capture_logs(f: impl FnOnce()) -> String {
    let buf = CaptureWriter::default();
    tracing::subscriber::with_default(subscriber(buf.clone(), tracing::Level::TRACE), f);
    buf.contents()
}

/// Guard-based capture at the WARN floor for async tests: a closure-scoped
/// `with_default` can't span `.await` points, so the subscriber stays installed
/// while the returned guard lives.
pub(crate) fn capture_warns() -> (CaptureWriter, tracing::subscriber::DefaultGuard) {
    let logs = CaptureWriter::default();
    let guard = tracing::subscriber::set_default(subscriber(logs.clone(), tracing::Level::WARN));
    (logs, guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wire-derived value rides a `?` field: `tracing_subscriber::fmt`'s
    /// `EscapeGuard` covers only the `message` arm, so a `%` Display field writes
    /// ESC/BEL/bidi bytes raw into the log file and the stderr terminal.
    #[test]
    fn debug_fields_escape_control_and_bidi_chars() {
        let hostile = std::path::Path::new("/tmp/\u{1b}]0;x\u{07}\u{202e}fake.jsonl");
        let out = capture_logs(|| {
            tracing::warn!(path = ?hostile, "debug field");
            tracing::warn!(path = %hostile.display(), "display field");
        });
        let (debug_line, display_line) = out.split_once('\n').expect("two lines");
        for raw in ['\u{1b}', '\u{07}', '\u{202e}'] {
            assert!(
                !debug_line.contains(raw),
                "`?` must escape {raw:?}: {debug_line:?}"
            );
            assert!(
                display_line.contains(raw),
                "control: `%` leaks {raw:?}, else the rule is moot: {display_line:?}"
            );
        }
    }
}
