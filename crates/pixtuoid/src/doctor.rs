//! `pixtuoid doctor` — read-only source self-diagnosis.
//!
//! Surfaces the decode-drift breadcrumbs (`source/drift.rs`, structured under the
//! `pixtuoid::drift` tracing target) that otherwise die in the warn-floor log
//! nobody reads — the gap the Task→Agent rename exposed. For each registered
//! source it reports: connected? hooks installed? the installed CLI version
//! (probed via `<cli> --version`) vs the `verified_version` anchor — flagging
//! skew ("NEWER than verified, drift possible") where an anchor exists; and any
//! drift recorded in the log (unknown events / missing fields / unknown dispatch
//! / shape drift), with a sanitized sample of the distinctive new names so the
//! user can report them.
//!
//! Strictly READ-ONLY: log file + config + install-state + best-effort
//! `<cli> --version` subprocess probes (stdin nulled so they can't block; argv
//! from the static registry, never user input). It never writes config
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

/// Source label-prefixes (e.g. `"cc"`) that have ANY decode-drift breadcrumb in
/// the log — for the live footer nudge. Reuses `scan_log_for_source` (tested).
pub fn drifted_sources(log: &str) -> Vec<String> {
    REGISTERED_SOURCES
        .iter()
        .filter(|s| scan_log_for_source(log, s).total() > 0)
        .filter_map(|s| registry::descriptor_for(s).map(|d| d.label_prefix.to_string()))
        .collect()
}

/// Merge the source-death footer warning (HIGHEST priority — the office is
/// partially frozen) with a passive decode-drift nudge. `None` when both clear.
/// The footer (`run_tui`) sets this each frame; the drift list is throttle-scanned.
pub fn footer_warning(source_death: Option<&str>, drifted: &[String]) -> Option<String> {
    if let Some(d) = source_death {
        return Some(d.to_string());
    }
    if drifted.is_empty() {
        return None;
    }
    let prefixes = drifted
        .iter()
        .map(|p| format!("{p}·"))
        .collect::<Vec<_>>()
        .join(" ");
    // No leading `⚠` — the footer painter (`hud.rs` `" ⚠ {warn} "`) owns the
    // glyph, same as the source-death message. Embedding one here double-prints
    // it (`⚠ ⚠ decode drift`), a regression a snapshot caught.
    Some(format!("decode drift: {prefixes} — run `pixtuoid doctor`"))
}

/// One source's diagnosis row (plain data, so `format_doctor_row` is pure/tested).
pub struct DoctorSourceRow {
    pub prefix: &'static str,
    pub name: &'static str,
    pub connected: bool,
    pub has_target: bool,
    pub hooks_installed: bool,
    /// The installed CLI version (raw probe output), if probeable.
    pub installed_version: Option<String>,
    /// The version this build's decoder was verified against (`"unknown"` = no
    /// anchor), from the source's `SourceDescriptor`.
    pub verified_version: &'static str,
    pub scan: LogScanResult,
}

/// Extract a `MAJOR.MINOR[.PATCH]` tuple from a version string (e.g. from
/// `GitHub Copilot CLI 1.0.62.` → (1,0,62)) — the first dotted-number run.
/// Tolerant: trailing/leading text ignored, missing patch = 0, parse failure =
/// None (so a skew check silently no-ops rather than alarming on garbage).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // Take the run of digits and dots starting here.
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let run = &s[start..i];
            let mut parts = run.split('.').filter(|p| !p.is_empty());
            if let Some(major) = parts.next().and_then(|p| p.parse().ok()) {
                let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
                let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
                // Require at least a MAJOR.MINOR to count as a version (a bare
                // integer like a year/count is too ambiguous).
                if run.contains('.') {
                    return Some((major, minor, patch));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// The version segment for a doctor row. Skew is flagged ONLY when both the
/// installed and the (non-`unknown`) verified version parse — otherwise it just
/// shows the installed version (still useful) with no alarm.
pub fn version_status(installed: Option<&str>, verified: &str) -> String {
    // Show the RAW probe string (what the CLI actually reports) — honest, not a
    // lossy reformat (cursor's `2026.06.04-5fd875e` isn't semver). The skew
    // check still parses internally.
    let inst_disp = installed
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    if verified == "unknown" {
        return inst_disp.to_string();
    }
    let cmp = match (installed.and_then(parse_version), parse_version(verified)) {
        (Some(i), Some(v)) if i > v => " — NEWER than verified, drift possible",
        (Some(i), Some(v)) if i < v => " — older than verified",
        (Some(_), Some(_)) => " — matches verified",
        _ => "",
    };
    format!("{inst_disp} (verified {verified}{cmp})")
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
    let version = version_status(row.installed_version.as_deref(), row.verified_version);
    format!(
        "  {}·{:<13} {:<12} hooks: {:<22} {:<34} {}",
        row.prefix, row.name, conn, hooks, version, drift
    )
}

/// First non-empty line of subprocess output, trimmed AND control-char
/// `sanitize`d — `--version` output is untrusted (a PATH-substituted binary
/// could emit ANSI/OSC to manipulate the terminal; R0615-06). Pure → tested.
fn first_sanitized_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(sanitize)
}

/// Probe a source's `<cli> --version` (argv from the static registry — never
/// user input) → the first non-empty output line, sanitized. Best-effort; every
/// failure → None ("unknown"):
/// - missing binary / spawn error,
/// - NONZERO exit (a broken `--version` must not show its error text as the
///   version),
/// - a HANG: stdin is nulled (no block on the inherited TTY) and the child is
///   killed after a deadline (a slow/blocking/PATH-substituted binary can't hang
///   doctor — `output()` has no timeout). Checks stdout then stderr.
fn probe_version(argv: &'static [&'static str]) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let (cmd, args) = argv.split_first()?;
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // `--version` output is tiny, so the piped buffers never fill while we poll
    // (no reader-vs-writer deadlock for this use).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    // A `--version` that exits nonzero is broken — "unknown", never its error text.
    if !output.status.success() {
        return None;
    }
    first_sanitized_line(&output.stdout).or_else(|| first_sanitized_line(&output.stderr))
}

/// Run the diagnosis: read config + install-state + the log, probe installed CLI
/// versions, print a per-source health table. Read-only. `log_path` is injected
/// by `main` (it owns the log-path resolution, which lives in the bin, not lib).
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
        let desc = registry::descriptor_for(src);
        let target = crate::install::target::by_source(src);
        let row = DoctorSourceRow {
            prefix: desc.map(|d| d.label_prefix).unwrap_or("??"),
            name: src,
            connected: connected.contains(src),
            has_target: target.is_some(),
            hooks_installed: target.map(crate::install::has_hooks).unwrap_or(false),
            installed_version: desc.and_then(|d| d.version_probe).and_then(probe_version),
            verified_version: desc.map(|d| d.verified_version).unwrap_or("unknown"),
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
            installed_version: Some("2.0.0".into()),
            verified_version: "unknown",
            scan: LogScanResult::default(),
        };
        let c = format_doctor_row(&clean);
        assert!(c.contains("codex") && c.contains("connected") && c.contains("installed"));
        assert!(c.contains("2.0.0"));
        assert!(c.contains("drift: none"));

        let drifted = DoctorSourceRow {
            prefix: "cp",
            name: "copilot",
            connected: true,
            has_target: false, // transcript-only
            hooks_installed: false,
            installed_version: Some("1.1.0".into()),
            verified_version: "1.0.62",
            scan: LogScanResult {
                missing_field: 3,
                ..Default::default()
            },
        };
        let d = format_doctor_row(&drifted);
        assert!(d.contains("3 missing-field"));
        assert!(d.contains("n/a (transcript-only)"));
        assert!(d.contains("NEWER than verified"), "skew flagged: {d}");
    }

    #[test]
    fn parse_version_extracts_the_dotted_run() {
        assert_eq!(parse_version("1.0.62"), Some((1, 0, 62)));
        assert_eq!(
            parse_version("GitHub Copilot CLI 1.0.62."),
            Some((1, 0, 62))
        );
        assert_eq!(parse_version("v2.1"), Some((2, 1, 0))); // missing patch = 0
        assert_eq!(parse_version("codex 0.41.0 (abc)"), Some((0, 41, 0)));
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("2026"), None); // a bare integer is not a version
    }

    #[test]
    fn version_status_flags_skew_only_with_a_known_anchor() {
        // unknown anchor → just the installed version (raw), no skew text.
        let u = version_status(Some("3.4.5"), "unknown");
        assert_eq!(u, "3.4.5");
        assert!(!u.contains("verified"));
        // newer / older / matches.
        assert!(version_status(Some("1.1.0"), "1.0.62").contains("NEWER than verified"));
        assert!(version_status(Some("1.0.0"), "1.0.62").contains("older than verified"));
        assert!(version_status(Some("1.0.62"), "1.0.62").contains("matches verified"));
        // un-probeable installed → shows unknown, no false skew.
        let n = version_status(None, "1.0.62");
        assert!(n.contains("unknown") && !n.contains("NEWER"));
    }

    #[test]
    fn drifted_sources_and_footer_warning() {
        let log = capture(|| {
            drift::unknown_event("claude-code", "NewHook");
            drift::missing_field("codex", "function_call", "name");
        });
        let mut d = drifted_sources(&log);
        d.sort();
        assert_eq!(d, vec!["cc".to_string(), "cx".to_string()]);
        // source-death wins (the office is partially frozen).
        assert_eq!(
            footer_warning(Some("source 'x' died"), &d).as_deref(),
            Some("source 'x' died")
        );
        // drift nudge when no death.
        let w = footer_warning(None, &d).unwrap();
        assert!(
            w.contains("cc·") && w.contains("cx·") && w.contains("doctor"),
            "{w}"
        );
        // The footer painter (`hud.rs` `" ⚠ {warn} "`) owns the warning glyph;
        // neither the drift NOR the death message may embed its own or it
        // double-prints (`⚠ ⚠ …`).
        assert!(!w.contains('⚠'), "drift msg must not embed ⚠: {w}");
        // Death tier: route the REAL `source_warning_message` output through the
        // merge (not a literal) — if that producer ever embeds a glyph, this
        // catches the same double-print at the death tier too.
        let death = crate::tui::widgets::source_warning_message(&[
            pixtuoid_core::source::manager::SourceDeath::new("claude-code", "x"),
        ])
        .unwrap();
        let dw = footer_warning(Some(&death), &d).unwrap();
        assert!(!dw.contains('⚠'), "death msg must not embed ⚠: {dw}");
        // both clear → nothing.
        assert_eq!(footer_warning(None, &[]), None);
    }

    #[test]
    fn probe_output_is_sanitized_and_first_nonempty() {
        // Leading blank lines skipped; the version line returned with control
        // chars (ANSI/OSC/BEL) stripped — a PATH-substituted binary can't drive
        // the terminal through `--version`.
        let raw = b"\n\n\x1b]0;pwned\x07cli \x1b[31m1.2.3\x1b[0m\nnext line";
        let got = first_sanitized_line(raw).unwrap();
        assert_eq!(got, "]0;pwnedcli [31m1.2.3[0m"); // ESC + BEL stripped, text kept
        assert!(
            !got.chars().any(|c| c.is_control()),
            "no control chars: {got:?}"
        );
        assert_eq!(first_sanitized_line(b""), None);
        assert_eq!(first_sanitized_line(b"   \n  \n"), None);
    }
}
