//! `pixtuoid doctor` — read-only source self-diagnosis.
//!
//! Surfaces the decode-drift breadcrumbs (`source/drift.rs`, structured under the
//! `pixtuoid::drift` tracing target) that otherwise die in the warn-floor log
//! nobody reads — the gap the Task→Agent rename exposed. For each registered
//! source it reports: connected? hooks installed? and any drift recorded in the
//! log (unknown events / missing fields / unknown dispatch / shape drift), with a
//! sanitized sample of the distinctive new names so the user can report them.
//!
//! Strictly READ-ONLY: log file + config + install-state. It never writes config
//! (re-connecting hooks stays the Connection panel's job) and never spawns the
//! TUI. The untrusted wire values (event/tool names) it samples are
//! `sanitize`d before display (R0615-06) — `doctor` is the third consumer of
//! those breadcrumbs and must hold the same line as the headless path + footer.

use pixtuoid_core::source::{drift, registry, REGISTERED_SOURCES};

/// Per-source drift tallied from the log, by `kind`, plus a sanitized sample of
/// the distinctive values (new event/tool names) and the most recent timestamp.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct LogScanResult {
    pub unknown_event: u64,
    pub missing_field: u64,
    pub unknown_dispatch: u64,
    pub shape_drift: u64,
    /// Sanitized, deduped, capped distinctive values (unknown event names / tool
    /// names) — the actionable "what drifted", safe to print.
    pub samples: Vec<String>,
    /// The leading timestamp token of the latest matching log line, if any.
    pub last_ts: Option<String>,
}

impl LogScanResult {
    pub fn total(&self) -> u64 {
        self.unknown_event + self.missing_field + self.unknown_dispatch + self.shape_drift
    }
}

const SAMPLE_CAP: usize = 5;

/// Strip control chars from an untrusted wire value before it reaches stdout
/// (the same discipline as the headless `sanitize_line`; R0615-06).
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Pull `key=value` from a tracing-fmt line: the value runs to the next
/// whitespace (drift breadcrumb fields are space-separated, no spaces in
/// source/kind/name/tool), surrounding quotes stripped. `None` if absent.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}=");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn push_sample(samples: &mut Vec<String>, v: Option<&str>) {
    if let Some(v) = v {
        let s = sanitize(v);
        if !s.is_empty() && samples.len() < SAMPLE_CAP && !samples.contains(&s) {
            samples.push(s);
        }
    }
}

/// Scan warn-floor log text for `pixtuoid::drift` breadcrumbs for ONE source,
/// tallying by `kind`. Pure (takes the log text) so it's testable against real
/// fmt output. Source/kind values are matched, never re-emitted raw; the sampled
/// names ARE sanitized (they're untrusted wire content).
pub fn scan_log_for_source(log: &str, source: &str) -> LogScanResult {
    let mut r = LogScanResult::default();
    for line in log.lines() {
        if !line.contains(drift::TARGET) || field(line, "source") != Some(source) {
            continue;
        }
        let Some(kind) = field(line, "kind") else {
            continue;
        };
        match kind {
            "unknown_event" => {
                r.unknown_event += 1;
                push_sample(&mut r.samples, field(line, "name"));
            }
            "missing_field" => r.missing_field += 1,
            "unknown_dispatch" => {
                r.unknown_dispatch += 1;
                push_sample(&mut r.samples, field(line, "tool"));
            }
            "shape_drift" => r.shape_drift += 1,
            _ => continue,
        }
        if let Some(ts) = line.split_whitespace().next() {
            r.last_ts = Some(ts.to_string());
        }
    }
    r
}

/// One source's diagnosis row (plain data, so `format_doctor_row` is pure/tested).
pub struct DoctorSourceRow {
    pub prefix: &'static str,
    pub name: &'static str,
    pub connected: bool,
    pub has_target: bool,
    pub hooks_installed: bool,
    pub scan: LogScanResult,
}

/// Render one row. Pure — the test seam (like `runtime::summarize`).
pub fn format_doctor_row(row: &DoctorSourceRow) -> String {
    let conn = if row.connected {
        "connected"
    } else {
        "disconnected"
    };
    let hooks = if !row.has_target {
        "n/a (transcript-only)"
    } else if row.hooks_installed {
        "installed"
    } else {
        "NOT installed"
    };
    let drift = if row.scan.total() == 0 {
        "drift: none".to_string()
    } else {
        let mut parts = Vec::new();
        let s = &row.scan;
        if s.unknown_event > 0 {
            parts.push(format!("{} unknown-event", s.unknown_event));
        }
        if s.missing_field > 0 {
            parts.push(format!("{} missing-field", s.missing_field));
        }
        if s.unknown_dispatch > 0 {
            parts.push(format!("{} unknown-dispatch", s.unknown_dispatch));
        }
        if s.shape_drift > 0 {
            parts.push(format!("{} shape-drift", s.shape_drift));
        }
        let when = s
            .last_ts
            .as_deref()
            .map(|t| format!(" (last {t})"))
            .unwrap_or_default();
        let samples = if s.samples.is_empty() {
            String::new()
        } else {
            format!(" [{}]", s.samples.join(", "))
        };
        format!("DRIFT: {}{when}{samples}", parts.join(", "))
    };
    format!(
        "  {}·{:<13} {:<13} hooks: {:<22} {}",
        row.prefix, row.name, conn, hooks, drift
    )
}

/// Run the diagnosis: read config + install-state + the log, print a per-source
/// health table. Read-only. `log_path` is injected by `main` (it owns the
/// log-path resolution, which lives in the bin crate, not the lib).
pub fn run(log_path: &std::path::Path) -> anyhow::Result<()> {
    let mut warnings = Vec::new();
    let cfg = crate::config::load(&crate::config::config_path(), &mut warnings);
    let connected = crate::config::resolve_connected(&cfg, |src| {
        crate::install::target::by_source(src).map(crate::install::has_hooks)
    });
    let log = std::fs::read_to_string(log_path).unwrap_or_default();

    let mut out = String::from("pixtuoid doctor — source health\n");
    out.push_str(&format!("log: {}\n", log_path.display()));
    // Surface config-load warnings IN the report — a malformed config makes every
    // source read disconnected, and a diagnostic tool must say WHY rather than
    // silently swallow it. Sanitized: a warning can interpolate config content.
    for w in &warnings {
        out.push_str(&format!("⚠ config: {}\n", sanitize(w)));
    }
    out.push('\n');

    let mut any_drift = false;
    for &src in REGISTERED_SOURCES {
        let prefix = registry::descriptor_for(src)
            .map(|d| d.label_prefix)
            .unwrap_or("??");
        let target = crate::install::target::by_source(src);
        let row = DoctorSourceRow {
            prefix,
            name: src,
            connected: connected.contains(src),
            has_target: target.is_some(),
            hooks_installed: target.map(crate::install::has_hooks).unwrap_or(false),
            scan: scan_log_for_source(&log, src),
        };
        any_drift |= row.scan.total() > 0;
        out.push_str(&format_doctor_row(&row));
        out.push('\n');
    }

    if any_drift {
        out.push_str(
            "\n⚠ decode drift recorded — your pixtuoid may predate a CLI's current wire format.\n   \
             Please report it: https://github.com/IvanWng97/pixtuoid/issues\n",
        );
    } else {
        out.push_str("\n✓ no decode drift recorded in the log.\n");
    }
    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
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

    // Capture through the SAME subscriber shape main.rs's file log uses
    // (fmt + ansi off + default timestamp), so the scanner is validated against
    // the REAL line format, not an assumed one.
    fn capture(f: impl FnOnce()) -> String {
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

    #[test]
    fn scan_counts_real_breadcrumb_lines_per_source() {
        let log = capture(|| {
            drift::unknown_event("copilot", "NewHookV2");
            drift::missing_field("copilot", "tool.execution_start", "toolName");
            drift::unknown_dispatch("copilot", "AgentV3");
            drift::shape_drift("copilot", "registry missing pid");
            drift::unknown_event("codex", "OtherHook"); // different source
        });
        let r = scan_log_for_source(&log, "copilot");
        assert_eq!(r.unknown_event, 1, "log:\n{log}");
        assert_eq!(r.missing_field, 1);
        assert_eq!(r.unknown_dispatch, 1);
        assert_eq!(r.shape_drift, 1);
        assert_eq!(r.total(), 4);
        assert!(
            r.samples.contains(&"NewHookV2".to_string()),
            "samples={:?}",
            r.samples
        );
        assert!(r.samples.contains(&"AgentV3".to_string()));
        assert!(r.last_ts.is_some());
        // The codex line must not bleed into copilot's tally.
        let rc = scan_log_for_source(&log, "codex");
        assert_eq!(rc.unknown_event, 1);
        assert_eq!(rc.missing_field, 0);
    }

    #[test]
    fn scan_of_empty_log_is_clean() {
        assert_eq!(scan_log_for_source("", "copilot"), LogScanResult::default());
    }

    #[test]
    fn samples_are_sanitized_deduped_and_capped() {
        let log = capture(|| {
            for _ in 0..3 {
                drift::unknown_event("cursor", "Dup"); // dedup → one sample
            }
            for i in 0..10 {
                drift::unknown_event("cursor", Box::leak(format!("E{i}").into_boxed_str()));
            }
        });
        let r = scan_log_for_source(&log, "cursor");
        assert!(r.unknown_event >= 11);
        assert!(r.samples.len() <= SAMPLE_CAP, "capped: {:?}", r.samples);
        assert_eq!(
            r.samples.iter().filter(|s| *s == "Dup").count(),
            1,
            "deduped"
        );
        // Control chars never survive into a sample.
        assert!(!r.samples.iter().any(|s| s.chars().any(|c| c.is_control())));
    }

    #[test]
    fn format_row_clean_vs_drift_and_transcript_only() {
        let clean = DoctorSourceRow {
            prefix: "cx",
            name: "codex",
            connected: true,
            has_target: true,
            hooks_installed: true,
            scan: LogScanResult::default(),
        };
        let c = format_doctor_row(&clean);
        assert!(c.contains("codex") && c.contains("connected") && c.contains("installed"));
        assert!(c.contains("drift: none"));

        let drifted = DoctorSourceRow {
            prefix: "cp",
            name: "copilot",
            connected: true,
            has_target: false, // transcript-only
            hooks_installed: false,
            scan: LogScanResult {
                missing_field: 3,
                ..Default::default()
            },
        };
        let d = format_doctor_row(&drifted);
        assert!(d.contains("3 missing-field"));
        assert!(d.contains("n/a (transcript-only)"));
    }
}
