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
//! (re-connecting hooks stays the Sources panel's job) and never spawns the
//! TUI. The untrusted wire values (event/tool names) it samples are
//! `sanitize`d before display (R0615-06) — `doctor` is the third consumer of
//! those breadcrumbs and must hold the same line as the headless path + footer.

use pixtuoid_core::source::{drift, registry, REGISTERED_SOURCES};

/// Per-source drift tallied from the log, by `kind`, plus a sanitized sample of
/// the distinctive values (new event/tool names) and the most recent timestamp.
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct LogScanResult {
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
    pub(crate) fn total(&self) -> u64 {
        self.unknown_event + self.missing_field + self.unknown_dispatch + self.shape_drift
    }
}

const SAMPLE_CAP: usize = 5;

// Strip control chars from an untrusted wire value before it reaches stdout
// (R0615-06) — the one canonical `crate::strip_control_chars`.
use crate::strip_control_chars as sanitize;

/// The parsed fields of one `pixtuoid::drift` breadcrumb line, borrowed from it.
struct DriftLine<'a> {
    source: &'a str,
    kind: &'a str,
    /// The fields segment AFTER the `target:` marker — sample values are pulled
    /// from here, so a span field of the same name (rendered BEFORE the target)
    /// can't be picked up (R0615-09).
    fields: &'a str,
}

/// Parse a warn-floor log line as a drift breadcrumb, anchored on the STRUCTURAL
/// tracing-fmt `target:` marker rather than a loose `contains` (R0615-08/-09).
/// `marker` is `"<TARGET>: "` (hoisted by the caller to avoid a per-line alloc).
/// tracing-fmt renders the target verbatim after the level + any span list, so:
/// (1) a line that merely MENTIONS the literal inside a field value isn't matched
/// (the marker carries the `: ` the target position always has, and must be a
/// standalone token, not the suffix of a longer `a::b::pixtuoid::drift` target);
/// (2) fields are parsed only from the segment AFTER it, never an active-span
/// field of the same name. `None` if not a drift line or source/kind is absent.
/// Accepted residual: a non-drift line whose value literally embeds
/// ` <TARGET>: source=… kind=… ` would still match — no in-tree code emits that.
fn parse_drift_line<'a>(line: &'a str, marker: &str) -> Option<DriftLine<'a>> {
    let at = line.find(marker)?;
    if at != 0 && line.as_bytes()[at - 1] != b' ' {
        return None; // suffix of a longer target, not our standalone token
    }
    let fields = &line[at + marker.len()..];
    Some(DriftLine {
        source: field_value(fields, "source")?,
        kind: field_value(fields, "kind")?,
        fields,
    })
}

/// Pull a field value from a tracing-fmt fields segment. Handles the quoted form
/// (`key="…"`, fmt's string-literal rendering) AND the unquoted Display form
/// (`key=val`), INCLUDING a value containing spaces (a hostile wire name): an
/// unquoted value runs to the next ` <ident>=` field boundary or the segment end,
/// not merely the next whitespace (R0615-09). The key must START a field (segment
/// start or space-preceded) so `name` can't match inside `displayName=`.
fn field_value<'a>(seg: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}=");
    let mut from = 0;
    let val_start = loop {
        let abs = from + seg[from..].find(&pat)?;
        if abs == 0 || seg.as_bytes()[abs - 1] == b' ' {
            break abs + pat.len();
        }
        from = abs + pat.len();
    };
    let rest = &seg[val_start..];
    if let Some(after_q) = rest.strip_prefix('"') {
        Some(&after_q[..after_q.find('"').unwrap_or(after_q.len())])
    } else {
        Some(rest[..next_field_boundary(rest).unwrap_or(rest.len())].trim_end())
    }
}

/// Index of the next ` <ident>=` field boundary in an unquoted value tail (so a
/// spaced value is kept whole instead of truncated at its first space).
fn next_field_boundary(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (0..b.len()).find(|&i| {
        if b[i] != b' ' {
            return false;
        }
        let mut j = i + 1;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
            j += 1;
        }
        j > i + 1 && j < b.len() && b[j] == b'='
    })
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
pub(crate) fn scan_log_for_source(log: &str, source: &str) -> LogScanResult {
    let mut r = LogScanResult::default();
    let marker = format!("{}: ", drift::TARGET);
    for line in log.lines() {
        let Some(p) = parse_drift_line(line, &marker) else {
            continue;
        };
        if p.source != source {
            continue;
        }
        match p.kind {
            "unknown_event" => {
                r.unknown_event += 1;
                push_sample(&mut r.samples, field_value(p.fields, "name"));
            }
            "missing_field" => r.missing_field += 1,
            "unknown_dispatch" => {
                r.unknown_dispatch += 1;
                push_sample(&mut r.samples, field_value(p.fields, "tool"));
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
pub(crate) fn drifted_sources(log: &str) -> Vec<String> {
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
    // No leading `⚠` — the footer painter (`footer.rs` `" ⚠ {warn} "`) owns the
    // glyph, same as the source-death message. Embedding one here double-prints
    // it (`⚠ ⚠ decode drift`), a regression a snapshot caught.
    Some(format!("decode drift: {prefixes} — run `pixtuoid doctor`"))
}

/// Windows-only advisory for the "installed but no sprite" path-split class
/// (CodeWhale / OpenClaw, #census-266-style): when `HOME` is set and differs from
/// `%USERPROFILE%`, a source whose CLI resolves its home differently than pixtuoid
/// did may have its hooks written where the CLI never reads. pixtuoid already
/// mirrors the HOME-first CLIs (`platform::home_first_dir`), so this is a SAFETY
/// NET — it surfaces the one host condition under which any residual resolver
/// mismatch (a closed-source CLI verified only by convention, or a future source)
/// would bite, and points a troubleshooting user straight at it. `None` on
/// non-Windows or when the two homes are equivalent (the common case, where
/// nothing can diverge). Pure (env + platform injected) so it unit-tests on any
/// host.
pub(crate) fn home_split_advisory(
    is_windows: bool,
    home: Option<&str>,
    userprofile: Option<&str>,
) -> Option<String> {
    if !is_windows {
        return None;
    }
    let home = home.map(str::trim).filter(|s| !s.is_empty())?;
    let up = userprofile.map(str::trim).filter(|s| !s.is_empty())?;
    if win_path_eq(home, up) {
        return None;
    }
    // Sanitized: HOME/USERPROFILE are user-controlled env values surfaced in the
    // report (same discipline as the config-warning lines).
    Some(format!(
        "⚠ Windows: HOME ({}) differs from USERPROFILE ({}). pixtuoid resolves \
         CodeWhale/OpenClaw HOME-first to match their CLIs — but if a source's \
         sprite is missing, confirm its hook config landed under the home that \
         CLI actually reads.",
        sanitize(home),
        sanitize(up)
    ))
}

/// Windows path equivalence for the home-split check: case-insensitive (NTFS),
/// `\`/`/` agnostic, trailing-separator agnostic. A `/c/Users/me`-vs-`C:\Users\me`
/// split is a REAL divergence (different roots), so this only collapses cosmetic
/// differences, never a POSIX-form HOME vs a native USERPROFILE.
fn win_path_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

/// Per-source diagnostics rollup — the SHARED source of truth the Connection
/// panel (the board), the boot preflight, and `run` (the CLI report) all read,
/// so the surfaces can't drift apart and no check runs twice. Scope is the
/// CHEAP signals: install-schema soundness + decode drift. Version skew stays
/// report-only (the
/// `<cli> --version` probe, up to 5s each, is too costly for an interactive
/// panel-open across N sources, and is advisory); live activity + transport
/// death stay the panel's per-frame facets.
#[derive(Debug, Default)]
pub struct SourceDiagnostics {
    /// #309 install-schema soundness — `Some` only when hooks are installed in
    /// the target's config; `None` = not checked (no target / not installed).
    pub install: Option<crate::install::verify::SchemaVerifyResult>,
    /// Decode-drift tally from the warn-floor log.
    pub(crate) drift: LogScanResult,
}

impl SourceDiagnostics {
    /// A HARD install problem ⇒ the source is broken (zero sprites despite a
    /// claimed connection). Soft notes + drift do NOT count as broken.
    pub fn is_broken(&self) -> bool {
        self.install.as_ref().is_some_and(|i| !i.is_sound())
    }

    /// The single worst issue as a one-line, glyph-prefixed summary for the
    /// Sources panel detail + the boot warning. `None` = nothing to flag.
    /// Priority: install-broken (hooks can't fire) > decode-drift.
    pub fn summary(&self) -> Option<String> {
        if let Some(i) = &self.install {
            if !i.is_sound() {
                return Some(format!("⚠ install broken: {}", i.issues.join("; ")));
            }
        }
        let n = self.drift.total();
        if n > 0 {
            return Some(format!("⚠ {n} decode drift — run `pixtuoid doctor`"));
        }
        None
    }
}

/// Compute the cheap per-source diagnostics given the warn-floor log text. The
/// install check runs whenever the source's target has managed hooks installed
/// (NOT gated on the connected flag — `run` reports a stale broken install even
/// on a disconnected source; the boot warning gates on connected itself).
pub fn diagnose(source: &str, log: &str) -> SourceDiagnostics {
    let install = crate::install::target::by_source(source)
        .filter(|t| crate::install::has_hooks(t))
        .map(|t| crate::install::verify_target(t, None));
    SourceDiagnostics {
        install,
        drift: scan_log_for_source(log, source),
    }
}

/// One source's diagnosis row (plain data, so `format_doctor_row` is pure/tested).
pub(crate) struct DoctorSourceRow {
    pub prefix: &'static str,
    /// The REGISTERED_SOURCES id (e.g. "claude-code"), NOT a display name — it's
    /// the registry key, distinct from `install::Target.name`/`display_name`.
    pub source_id: &'static str,
    pub connected: bool,
    pub has_target: bool,
    pub hooks_installed: bool,
    /// The installed CLI version (raw probe output), if probeable.
    pub installed_version: Option<String>,
    /// The version this build's decoder was verified against (`"unknown"` = no
    /// anchor), from the source's `SourceDescriptor`.
    pub verified_version: &'static str,
    pub scan: LogScanResult,
    /// Install-schema soundness — `Some` only when hooks are installed;
    /// `None` = not checked (no target / not installed). A non-sound result
    /// flips the verdict glyph to `⚠` and prints the reason on an indented `↳`
    /// continuation line.
    pub schema: Option<crate::install::verify::SchemaVerifyResult>,
}

/// A dotted-run major at or above this looks like a YEAR/date token, not a semver
/// major — used to skip a date prefix in favor of a real version.
const IMPLAUSIBLE_MAJOR: u64 = 1000;

/// Extract a `MAJOR.MINOR[.PATCH]` tuple from a `--version` banner. Tolerant:
/// surrounding text ignored, missing patch = 0, no dotted run = None (a skew
/// check then silently no-ops rather than alarming on garbage). A bare integer
/// (`2026`) is NOT a version (needs at least `MAJOR.MINOR`).
///
/// Banner-order robust: a banner can print a dotted DATE/build token
/// before the semver (`Built 2026.06.04 — v1.2.3`). Selection order:
///   1. a `v`/`V`-prefixed run wins (an explicit version marker);
///   2. else the first run with a plausible (< `IMPLAUSIBLE_MAJOR`) major,
///      skipping a year-like date prefix;
///   3. else the first run — so a genuine CalVer (`2026.06.04`, e.g. cursor)
///      still parses rather than vanishing.
pub(crate) fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let bytes = s.as_bytes();
    // (v_prefixed, (major, minor, patch)) for every dotted-number run.
    let mut runs: Vec<(bool, (u64, u64, u64))> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let run = &s[start..i];
        if !run.contains('.') {
            continue; // a bare integer is too ambiguous to be a version
        }
        let mut parts = run.split('.').filter(|p| !p.is_empty());
        if let Some(major) = parts.next().and_then(|p| p.parse().ok()) {
            let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            let v_prefixed = start > 0 && matches!(bytes[start - 1], b'v' | b'V');
            runs.push((v_prefixed, (major, minor, patch)));
        }
    }
    runs.iter()
        .find(|(vp, _)| *vp)
        .or_else(|| runs.iter().find(|(_, (maj, ..))| *maj < IMPLAUSIBLE_MAJOR))
        .or_else(|| runs.first())
        .map(|(_, v)| *v)
}

/// The version segment for a doctor row. Skew is flagged ONLY when both the
/// installed and the (non-`unknown`) verified version parse — otherwise it just
/// shows the installed version (still useful) with no alarm.
pub(crate) fn version_status(installed: Option<&str>, verified: &str) -> String {
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

/// Whether the row has a HARD install problem (the silent-dead case).
fn row_broken(row: &DoctorSourceRow) -> bool {
    row.schema.as_ref().is_some_and(|s| !s.is_sound())
}

/// The scannable per-row HEALTH verdict glyph (the rollup made visible):
/// `⚠` a problem (install broken OR decode drift), `✓` healthy (installed +
/// sound + no drift), `○` installable but not installed, `–` transcript-only
/// (no install schema to verify).
fn verdict_glyph(row: &DoctorSourceRow) -> char {
    if row_broken(row) || row.scan.total() > 0 {
        '\u{26a0}' // ⚠
    } else if !row.has_target {
        '\u{2013}' // –
    } else if !row.hooks_installed {
        '\u{25cb}' // ○
    } else {
        '\u{2713}' // ✓
    }
}

/// The decode-drift detail (counts + when + samples) for a continuation line.
fn drift_detail(s: &LogScanResult) -> String {
    let mut parts = Vec::new();
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
    format!("{}{when}{samples}", parts.join(", "))
}

/// Render one row: a scannable verdict line (glyph + name + connection + install
/// state + version), plus an indented `↳` continuation line per problem (broken
/// install / soft note / decode drift) so the long detail never wrecks the
/// table's column alignment. Pure — the test seam (like `runtime::summarize`).
pub(crate) fn format_doctor_row(row: &DoctorSourceRow) -> String {
    let conn = if row.connected {
        "connected"
    } else {
        "disconnected"
    };
    let state = if !row.has_target {
        "transcript-only"
    } else if !row.hooks_installed {
        "not installed"
    } else {
        "installed"
    };
    let version = version_status(row.installed_version.as_deref(), row.verified_version);
    let mut out = format!(
        "  {} {}\u{b7}{:<13} {:<12} {:<15} {}",
        verdict_glyph(row),
        row.prefix,
        row.source_id,
        conn,
        state,
        version
    );
    // Reason continuation lines — only emitted when there IS a problem, so a
    // healthy row stays a single clean line. `issues` are already control-char
    // sanitized at the source (`verify::display_safe`).
    if let Some(s) = &row.schema {
        if !s.is_sound() {
            out.push_str(&format!(
                "\n       \u{21b3} install broken: {}",
                s.issues.join("; ")
            ));
        } else if !s.notes.is_empty() {
            out.push_str(&format!("\n       \u{21b3} note: {}", s.notes.join("; ")));
        }
    }
    if row.scan.total() > 0 {
        out.push_str(&format!(
            "\n       \u{21b3} decode drift: {}",
            drift_detail(&row.scan)
        ));
    }
    out
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
    // Kill a `--version` child that hasn't exited within this budget; poll for its
    // exit at this cadence meanwhile.
    const PROBE_VERSION_TIMEOUT_SECS: u64 = 5;
    const PROBE_POLL_INTERVAL_MS: u64 = 20;
    let deadline = Instant::now() + Duration::from_secs(PROBE_VERSION_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(PROBE_POLL_INTERVAL_MS)),
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

/// The desktop activation backend focus-jump would use on THIS host — the
/// per-OS half of the #526 focus diagnostic. Linux detection is the pure
/// [`linux_activation_backend`] over the env markers `focus/linux.rs` keys on.
fn activation_backend() -> String {
    #[cfg(target_os = "macos")]
    {
        "NSRunningApplication (macOS) ✓".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        linux_activation_backend(
            std::env::var_os(crate::focus::SWAY_ENV).is_some(),
            std::env::var_os(crate::focus::HYPRLAND_ENV).is_some(),
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            std::env::var_os("DISPLAY").is_some(),
        )
        .to_string()
    }
    #[cfg(windows)]
    {
        "SetForegroundWindow (Windows) ✓ — the foreground lock may still deny".to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        "none — focus-jump is unsupported on this OS".to_string()
    }
}

/// Pure so every arm is unit-tested on any host: mirrors `focus/linux.rs`'s
/// ONE-channel-per-env order (sway IPC → hyprland IPC → X11 EWMH → nothing) —
/// EXCEPT that a Wayland session without a pid-addressable IPC must NOT be
/// reported as "X11 EWMH ✓": XWayland sets $DISPLAY, but a native-Wayland
/// terminal never appears in XWayland's client list and mutter/kwin block
/// focus-steal anyway, so the ✓ would mislead exactly the users focus fails
/// for. Only the linux `activation_backend` arm calls this in prod — on
/// other hosts the tests are the (deliberate) only caller.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_activation_backend(sway: bool, hyprland: bool, wayland: bool, x11: bool) -> &'static str {
    if sway {
        "sway IPC (swaymsg) ✓"
    } else if hyprland {
        "hyprland IPC (hyprctl) ✓"
    } else if wayland {
        "✗ Wayland compositor without a pid-addressable focus channel (GNOME/KDE \
         forbid focus-steal; xdg-activation unimplemented) — focus will silently \
         no-op for native-Wayland terminals"
    } else if x11 {
        "X11 EWMH ($DISPLAY) ✓"
    } else {
        "✗ none detected — focus will silently no-op"
    }
}

/// The focus-jump block of the report: the activation backend + which
/// pid channel each source family rides, with the transcript-family probe
/// roots checked on disk. Pure over the probed facts; the family buckets come
/// straight from the registry (a new source lands in the right bucket with no
/// edit here).
pub(crate) fn focus_section(
    backend: &str,
    cc_registry: Option<(&std::path::Path, bool)>,
    codex_sessions: (&std::path::Path, bool),
) -> String {
    let prefix_of = |src: &str| {
        registry::descriptor_for(src)
            .map(|d| d.label_prefix)
            .unwrap_or("??")
    };
    use registry::FocusChannel;
    let mut shim_stamp = Vec::new();
    let mut plugin_stamp = Vec::new();
    let mut no_channel = Vec::new();
    for &src in REGISTERED_SOURCES {
        let Some(d) = registry::descriptor_for(src) else {
            continue;
        };
        // Daemons (the lobster) aren't click-focusable agents; the
        // TranscriptProbe sources (CC/Codex) get their own rows below.
        if d.is_daemon() || d.focus_channel() == FocusChannel::TranscriptProbe {
            continue;
        }
        let tag = format!("{}\u{b7}{}", d.label_prefix, src);
        match d.focus_channel() {
            FocusChannel::ShimStamp => shim_stamp.push(tag),
            FocusChannel::PluginStamp => plugin_stamp.push(tag),
            FocusChannel::Unsupported => no_channel.push(tag),
            FocusChannel::TranscriptProbe => unreachable!("skipped above"),
        }
    }
    let mut out = String::from("focus-jump (click a sprite → its terminal comes forward):\n");
    out.push_str(&format!("  backend: {backend}\n"));
    let cc_prefix = prefix_of(pixtuoid_core::source::claude_code::SOURCE_NAME);
    match cc_registry {
        Some((dir, true)) => out.push_str(&format!(
            "  {cc_prefix}\u{b7}claude-code — registry probe: {} ✓\n",
            dir.display()
        )),
        Some((dir, false)) => out.push_str(&format!(
            "  {cc_prefix}\u{b7}claude-code — registry probe: {} ✗ missing (focus no-ops until CC writes it)\n",
            dir.display()
        )),
        None => out.push_str(&format!(
            "  {cc_prefix}\u{b7}claude-code — registry probe disabled (non-standard projects root) — focus no-ops\n"
        )),
    }
    let cx_prefix = prefix_of(pixtuoid_core::source::codex::SOURCE_NAME);
    let (codex_dir, codex_present) = codex_sessions;
    out.push_str(&format!(
        "  {cx_prefix}\u{b7}codex — rollout fd probe: {} {}\n",
        codex_dir.display(),
        if codex_present {
            "✓"
        } else {
            "✗ missing (focus no-ops until codex writes it)"
        }
    ));
    if !shim_stamp.is_empty() {
        out.push_str(&format!(
            "  {} — shim `_pid` (getppid) rides each hook event (unix; absent on Windows)\n",
            shim_stamp.join(", ")
        ));
    }
    if !plugin_stamp.is_empty() {
        out.push_str(&format!(
            "  {} — plugin-stamped `_pid` rides each hook event (all platforms)\n",
            plugin_stamp.join(", ")
        ));
    }
    if !no_channel.is_empty() {
        out.push_str(&format!(
            "  {} — no focus channel (click no-ops)\n",
            no_channel.join(", ")
        ));
    }
    out
}

/// Run the diagnosis: read config + install-state + the log, probe installed CLI
/// versions, print a per-source health table. Read-only. `log_path` is injected
/// by `main` (it owns the log-path resolution, which lives in the bin, not lib).
/// Build the read-only health report. Returns the rendered string (the caller —
/// `main.rs`, an excluded presenter — prints it) so the WHOLE report builder is
/// unit-testable, not just its pure row helpers.
pub fn run(log_path: &std::path::Path) -> anyhow::Result<String> {
    let mut warnings = Vec::new();
    let cfg = crate::config::load(&crate::config::config_path(), &mut warnings);
    // `doctor` is a separate PROCESS from the running TUI, so it derives the
    // connected-set fresh from config via the SAME `resolve_connected` the boot
    // seeder uses (NOT the live in-process `ConnectedSources`, which it can't
    // see). A snapshot diagnostic reading live on-disk state is the correct
    // semantic — it can lag a just-made in-TUI toggle until that toggle persists,
    // which it always does (persist-first; see `connect_source`/`disconnect_source`).
    let connected = crate::config::resolve_connected(&cfg);
    let log = std::fs::read_to_string(log_path).unwrap_or_default();

    let mut out = String::from("pixtuoid doctor — source health\n");
    out.push_str(&format!("log: {}\n", log_path.display()));
    out.push_str(&format!(
        "config: {}\n",
        crate::config::config_path().display()
    ));
    // Terminal capability: the pixel-art office needs a 24-bit-color terminal, and
    // the #1 silent failure is a non-truecolor terminal rendering approximated
    // colors. When $COLORTERM hasn't already declared truecolor, ASK the terminal
    // directly (DECRQSS) — but ONLY when stdout is a real tty (so a piped
    // `pixtuoid doctor > file` neither emits escape codes nor blocks, and the test
    // harness, whose output is captured, never probes) AND the terminal isn't
    // $TERM=dumb (which can't answer DECRQSS — don't emit escapes at it). The same
    // `color_preflight` the launcher acts on drives both the probe skip and the
    // color-status line, so the diagnostic matches what `run` would do. The row is
    // formatted by the PURE, unit-tested `term::terminal_diagnostic_row`; `.ok()`
    // makes an unset var a genuine `None`, not `Some("")`.
    let color_pf = crate::term::color_preflight(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("CLICOLOR_FORCE").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );
    let probe_ok = std::io::IsTerminal::is_terminal(&std::io::stdout())
        && color_pf != crate::term::ColorPreflight::RefuseDumbTerm;
    let truecolor_probe = if probe_ok {
        crate::term::query_truecolor(crate::term::TRUECOLOR_PROBE_TIMEOUT)
    } else {
        None
    };
    out.push_str(&crate::term::terminal_diagnostic_row(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
        truecolor_probe,
    ));
    out.push('\n');
    if let Some(line) = crate::term::color_status_row(color_pf) {
        out.push_str(line);
        out.push('\n');
    }
    // Surface config-load warnings IN the report — a malformed config makes every
    // source read disconnected, and a diagnostic tool must say WHY rather than
    // silently swallow it. Sanitized: a warning can interpolate config content.
    for w in &warnings {
        out.push_str(&format!("⚠ config: {}\n", sanitize(w)));
    }
    out.push('\n');

    let mut any_drift = false;
    let mut broken: Vec<String> = Vec::new(); // prefixes of broken installs (locally fixable)
    for &src in REGISTERED_SOURCES {
        let desc = registry::descriptor_for(src);
        let target = crate::install::target::by_source(src);
        let hooks_installed = target.map(crate::install::has_hooks).unwrap_or(false);
        // ONE shared rollup (install soundness + drift) — the same `diagnose` the
        // Sources panel + boot preflight read, so the report can't drift apart
        // from the live surfaces.
        let diag = diagnose(src, &log);
        let row = DoctorSourceRow {
            prefix: desc.map(|d| d.label_prefix).unwrap_or("??"),
            source_id: src,
            connected: connected.contains(src),
            has_target: target.is_some(),
            hooks_installed,
            installed_version: desc.and_then(|d| d.version_probe).and_then(probe_version),
            verified_version: desc.map(|d| d.verified_version).unwrap_or("unknown"),
            scan: diag.drift,
            schema: diag.install,
        };
        any_drift |= row.scan.total() > 0;
        if row_broken(&row) {
            broken.push(format!("{}\u{b7}{}", row.prefix, row.source_id));
        }
        out.push_str(&format_doctor_row(&row));
        out.push('\n');
    }

    // Rolled-up footer: a broken install is locally fixable (reconnect); decode
    // drift is a report-upstream concern — distinct remediation paths.
    let n = REGISTERED_SOURCES.len();
    if broken.is_empty() {
        out.push_str(&format!("\n{n} sources · ✓ all connected installs sound"));
    } else {
        let verb = if broken.len() == 1 { "needs" } else { "need" };
        out.push_str(&format!(
            "\n{n} sources · ⚠ {} {verb} attention ({}) — reconnect in the Sources panel (press s)",
            broken.len(),
            broken.join(", ")
        ));
    }
    if any_drift {
        out.push_str(
            " · ⚠ decode drift recorded — may predate a CLI's wire format; report: \
             https://github.com/IvanWng97/pixtuoid/issues\n",
        );
    } else {
        out.push_str(" · ✓ no decode drift\n");
    }
    // The #526 focus diagnostic — probe roots come from the SAME default
    // resolution the runtime seeds its watchers with (a --projects-root /
    // --codex-sessions-root override changes the RUNNING app, and doctor
    // diagnoses the default setup — noted so the mismatch can't surprise).
    let cc_projects =
        pixtuoid_core::source::claude_code::ClaudeCodeSource::default_paths().projects_root;
    let cc_registry = pixtuoid_core::source::cc_registry_dir(&cc_projects);
    let codex_sessions = pixtuoid_core::source::codex::CodexSource::default_paths().sessions_root;
    out.push('\n');
    out.push_str(&focus_section(
        &activation_backend(),
        cc_registry.as_deref().map(|d| (d, d.is_dir())),
        (&codex_sessions, codex_sessions.is_dir()),
    ));
    // Windows safety net: a HOME≠USERPROFILE shell is the one host condition under
    // which a CLI's home-resolution could land hooks where pixtuoid didn't write.
    if let Some(adv) = home_split_advisory(
        cfg!(windows),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
    ) {
        out.push_str(&format!("\n{adv}\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[test]
    fn linux_activation_backend_covers_every_channel_in_priority_order() {
        assert!(linux_activation_backend(true, true, true, true).contains("sway"));
        assert!(linux_activation_backend(false, true, true, true).contains("hyprland"));
        // A Wayland session without sway/hyprland must be an HONEST ✗ even
        // when XWayland set $DISPLAY — never "X11 EWMH ✓" (the F2 fix).
        assert!(linux_activation_backend(false, false, true, true).contains("✗ Wayland"));
        assert!(linux_activation_backend(false, false, false, true).contains("EWMH"));
        assert!(linux_activation_backend(false, false, false, false).contains("✗ none"));
    }

    #[test]
    fn focus_section_buckets_sources_from_the_registry() {
        let cc = std::path::Path::new("/home/u/.claude/sessions");
        let cx = std::path::Path::new("/home/u/.codex/sessions");
        let s = focus_section("test-backend ✓", Some((cc, true)), (cx, true));
        assert!(s.contains("backend: test-backend ✓"));
        assert!(s.contains("claude-code — registry probe") && s.contains("✓"));
        assert!(s.contains("codex — rollout fd probe"));
        // Hook-only sources ride the _pid stamp; transcript-only non-probe
        // sources have no channel — both buckets are REGISTRY-derived, so a
        // new source lands correctly with no doctor edit.
        assert!(
            s.contains("opencode") && s.contains("plugin-stamped"),
            "plugin stampers listed separately: {s}"
        );
        assert!(
            s.contains("cursor") && s.contains("shim `_pid`"),
            "shim stampers listed separately: {s}"
        );
        let no_channel_line = s
            .lines()
            .find(|l| l.contains("no focus channel"))
            .expect("a no-channel line");
        assert!(
            no_channel_line.contains("antigravity") && no_channel_line.contains("copilot"),
            "transcript-only sources listed: {no_channel_line}"
        );
        // The daemon (lobster) is not a click-focusable agent.
        assert!(!s.contains("openclaw"), "daemons are excluded: {s}");
    }

    #[test]
    fn focus_section_reports_missing_and_disabled_probe_roots() {
        let cc = std::path::Path::new("/home/u/.claude/sessions");
        let cx = std::path::Path::new("/home/u/.codex/sessions");
        let missing = focus_section("b", Some((cc, false)), (cx, false));
        assert!(missing.contains("✗ missing (focus no-ops until CC writes it)"));
        assert!(missing.contains("✗ missing (focus no-ops until codex writes it)"));
        let disabled = focus_section("b", None, (cx, true));
        assert!(disabled.contains("registry probe disabled (non-standard projects root)"));
    }

    #[test]
    fn run_renders_the_structural_report_headers() {
        // run() reads the real config/log/env, but the structural headers are
        // present regardless — so a whole-body `replace run with Ok(default)`
        // survivor (an empty report) is caught, giving the report builder teeth
        // beyond its pure row helpers. (A missing log path → empty log, fine.)
        let out = run(std::path::Path::new("/nonexistent-pixtuoid-doctor-log")).unwrap();
        assert!(out.contains("pixtuoid doctor — source health"), "{out}");
        assert!(out.contains("config:"), "{out}");
        assert!(out.contains("terminal: TERM="), "{out}");
        assert!(out.contains("sources \u{b7}"), "{out}");
    }

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
    fn home_split_advisory_fires_only_on_windows_with_a_real_home_split() {
        // Windows + HOME≠USERPROFILE → advisory (the bug's host condition).
        let a = home_split_advisory(true, Some(r"C:\msys\home\me"), Some(r"C:\Users\me"));
        assert!(a.is_some());
        let a = a.unwrap();
        assert!(a.contains("HOME") && a.contains("USERPROFILE") && a.contains("sprite"));

        // Non-Windows → never (Unix uses HOME on both sides, no split possible).
        assert!(home_split_advisory(false, Some("/home/a"), Some("/home/b")).is_none());

        // Windows but HOME unset / empty → nothing can diverge.
        assert!(home_split_advisory(true, None, Some(r"C:\Users\me")).is_none());
        assert!(home_split_advisory(true, Some("  "), Some(r"C:\Users\me")).is_none());
        // USERPROFILE unset → no comparison to make.
        assert!(home_split_advisory(true, Some(r"C:\Users\me"), None).is_none());
    }

    #[test]
    fn home_split_advisory_ignores_cosmetic_path_differences() {
        // Same dir, different slash / case / trailing sep → NOT a split.
        assert!(home_split_advisory(true, Some(r"C:\Users\Me"), Some(r"c:/users/me/")).is_none());
        // But a POSIX-form HOME vs a native USERPROFILE IS a real split (Git Bash).
        assert!(home_split_advisory(true, Some("/c/Users/me"), Some(r"C:\Users\me")).is_some());
        // win_path_eq directly.
        assert!(win_path_eq(r"C:\a\b", "c:/a/b/"));
        assert!(!win_path_eq("/c/a/b", r"C:\a\b"));
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

    // R0615-08: a non-drift line that merely MENTIONS the bare target string is
    // NOT counted — the structural `target:` marker (with its `: `) gates it, not
    // a loose `contains(TARGET)` (which the old scanner used). The crafted line
    // carries `source=`/`kind=` so only the missing structural marker saves it.
    #[test]
    fn scan_ignores_a_body_mention_of_the_target_string() {
        let line = "2026-06-15T00:00:00Z  WARN pixtuoid::source::manager: a pixtuoid::drift mention source=copilot kind=unknown_event name=X";
        assert_eq!(scan_log_for_source(line, "copilot").total(), 0);
    }

    // R0615-08: the space-guard rejects a LONGER target that merely SUFFIXES our
    // token (`a::b::pixtuoid::drift`). Distinct from the body-mention path above:
    // here the `pixtuoid::drift: ` marker IS present so `find` succeeds, but it's
    // preceded by `:` (not a space), so the guard returns None. Carries valid
    // source/kind so ONLY the guard prevents the (false) count.
    #[test]
    fn scan_rejects_a_longer_target_suffixing_our_token() {
        let line = "2026-06-15T00:00:00Z  WARN myapp::pixtuoid::drift: source=copilot kind=\"unknown_event\" name=X";
        assert_eq!(scan_log_for_source(line, "copilot").total(), 0);
    }

    // R0615-09: a breadcrumb emitted inside a tracing SPAN that carries its OWN
    // `source=` field — fmt renders span fields BEFORE the target, so parsing
    // after the marker must pick the EVENT's source, never the span's. (No
    // production code wraps a decoder in such a span today; this pins the parser
    // so adding one later can't silently misattribute.)
    #[test]
    fn scan_parses_event_fields_not_a_span_field_of_the_same_name() {
        let line = "2026-06-15T00:00:00Z  WARN decode{source=spanwrong}: pixtuoid::drift: source=copilot kind=\"unknown_event\" name=NewHook";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_event, 1, "event source must win");
        assert!(
            r.samples.contains(&"NewHook".to_string()),
            "{:?}",
            r.samples
        );
        // the span's value must NOT be attributed.
        assert_eq!(scan_log_for_source(line, "spanwrong").total(), 0);
    }

    // R0615-09: a hostile wire name containing a SPACE is preserved whole in the
    // sample, not truncated at the first space (an unquoted Display value runs to
    // the next ` <ident>=` boundary or end-of-line).
    #[test]
    fn scan_preserves_a_spaced_sample_value() {
        let line = "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"unknown_dispatch\" tool=My New Tool";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_dispatch, 1);
        assert!(
            r.samples.contains(&"My New Tool".to_string()),
            "{:?}",
            r.samples
        );
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
            source_id: "codex",
            connected: true,
            has_target: true,
            hooks_installed: true,
            installed_version: Some("2.0.0".into()),
            verified_version: "unknown",
            scan: LogScanResult::default(),
            schema: Some(crate::install::verify::SchemaVerifyResult::default()),
        };
        let c = format_doctor_row(&clean);
        assert!(c.contains("codex") && c.contains("connected") && c.contains("installed"));
        assert!(c.contains("2.0.0"));
        assert!(c.starts_with("  \u{2713}"), "sound row leads with ✓: {c}");
        // A healthy row is a SINGLE line — no `↳` reason continuation.
        assert!(!c.contains('\n'), "a healthy row has no reason line: {c}");
        assert!(
            !c.to_lowercase().contains("broken"),
            "a sound install must not say broken: {c}"
        );

        let drifted = DoctorSourceRow {
            prefix: "cp",
            source_id: "copilot",
            connected: true,
            has_target: false, // transcript-only
            hooks_installed: false,
            installed_version: Some("1.1.0".into()),
            verified_version: "1.0.62",
            scan: LogScanResult {
                missing_field: 3,
                ..Default::default()
            },
            schema: None,
        };
        let d = format_doctor_row(&drifted);
        assert!(
            d.starts_with("  \u{26a0}"),
            "a drifting row leads with ⚠: {d}"
        );
        assert!(d.contains("transcript-only"), "{d}");
        assert!(d.contains("NEWER than verified"), "skew flagged: {d}");
        // Drift detail is on its own `↳` continuation line.
        assert!(
            d.contains("\n       \u{21b3} decode drift: 3 missing-field"),
            "{d}"
        );
    }

    #[test]
    fn format_row_flags_a_broken_install() {
        let broken = DoctorSourceRow {
            prefix: "rx",
            source_id: "reasonix",
            connected: true,
            has_target: true,
            hooks_installed: true,
            installed_version: None,
            verified_version: "unknown",
            scan: LogScanResult::default(),
            schema: Some(crate::install::verify::SchemaVerifyResult {
                issues: vec!["shim binary missing: /old/pixtuoid-hook".into()],
                notes: vec![],
            }),
        };
        let b = format_doctor_row(&broken);
        assert!(
            b.starts_with("  \u{26a0}"),
            "a broken row leads with ⚠: {b}"
        );
        assert!(
            b.contains("\n       \u{21b3} install broken: shim binary missing"),
            "broken reason on its own ↳ line: {b}"
        );
    }

    // --- SourceDiagnostics rollup (the shared panel/boot/report source of truth) ---

    fn diag(
        install: Option<crate::install::verify::SchemaVerifyResult>,
        drift: LogScanResult,
    ) -> SourceDiagnostics {
        SourceDiagnostics { install, drift }
    }

    #[test]
    fn diagnostics_healthy_has_no_summary_and_is_not_broken() {
        let d = diag(
            Some(crate::install::verify::SchemaVerifyResult::default()),
            LogScanResult::default(),
        );
        assert!(!d.is_broken());
        assert_eq!(d.summary(), None);
    }

    #[test]
    fn diagnostics_broken_install_wins_over_drift() {
        let d = diag(
            Some(crate::install::verify::SchemaVerifyResult {
                issues: vec!["shim binary missing: /x".into()],
                notes: vec![],
            }),
            LogScanResult {
                unknown_event: 2,
                ..Default::default()
            },
        );
        assert!(d.is_broken());
        let s = d.summary().unwrap();
        assert!(
            s.contains("install broken") && s.contains("shim binary missing"),
            "{s}"
        );
        assert!(!s.contains("decode drift"), "install-broken must win: {s}");
    }

    #[test]
    fn diagnostics_drift_only_summarizes_when_install_is_sound() {
        let d = diag(
            Some(crate::install::verify::SchemaVerifyResult::default()),
            LogScanResult {
                missing_field: 3,
                ..Default::default()
            },
        );
        assert!(!d.is_broken());
        assert!(d.summary().unwrap().contains("3 decode drift"));
    }

    #[test]
    fn diagnostics_soft_notes_are_not_broken_and_do_not_summarize() {
        let d = diag(
            Some(crate::install::verify::SchemaVerifyResult {
                issues: vec![],
                notes: vec!["pixtuoid-hook not on PATH".into()],
            }),
            LogScanResult::default(),
        );
        assert!(!d.is_broken());
        assert_eq!(d.summary(), None);
    }

    #[test]
    fn diagnostics_no_install_check_is_not_broken() {
        let d = diag(None, LogScanResult::default());
        assert!(!d.is_broken());
        assert_eq!(d.summary(), None);
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

    // #307: a banner that prints a dotted DATE/build token BEFORE the semver must
    // not lock onto the date — the smarter extractor prefers a `v`-prefixed run,
    // else the first plausible (non-year) major, else falls back (CalVer-safe).
    #[test]
    fn parse_version_is_banner_order_robust() {
        // v-prefixed semver wins over a leading date.
        assert_eq!(parse_version("Built 2026.06.04 — v1.2.3"), Some((1, 2, 3)));
        // no `v`: skip the year-like major, take the first plausible run.
        assert_eq!(parse_version("Built 2026.06.04 — 1.2.3"), Some((1, 2, 3)));
        // a genuine CalVer with NO semver still parses (cursor's date style) —
        // fallback rather than vanishing.
        assert_eq!(parse_version("2026.06.04-5fd875e"), Some((2026, 6, 4)));
        // the only anchored CLI today: its raw banner parses to its anchor.
        assert_eq!(parse_version("GitHub Copilot CLI 1.0.62"), Some((1, 0, 62)));
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
        // The footer painter (`footer.rs` `" ⚠ {warn} "`) owns the warning glyph;
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

    // R0615-09 / field_value loop-back (97-98): `name=` inside `displayName=` is a
    // FALSE match (not field-start) and must be skipped so the loop advances to the
    // real `name=` field. Without the boundary guard, `displayName=foo` would be
    // mis-picked → the sample would be "foo", not "Real".
    #[test]
    fn scan_does_not_pick_name_inside_displayname() {
        let line = "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"unknown_event\" displayName=foo name=Real";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_event, 1, "line:\n{line}");
        assert!(r.samples.contains(&"Real".to_string()), "{:?}", r.samples);
        assert!(!r.samples.contains(&"foo".to_string()), "{:?}", r.samples);
    }

    // push_sample's None arm (130): an `unknown_event` breadcrumb with NO `name=`
    // field still counts (parse_drift_line needs only source+kind) but contributes
    // no sample — field_value returns None → push_sample no-ops.
    #[test]
    fn scan_unknown_event_without_a_name_field_counts_but_samples_none() {
        let line =
            "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"unknown_event\"";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_event, 1, "line:\n{line}");
        assert!(r.samples.is_empty(), "{:?}", r.samples);
    }

    // scan_log_for_source `_ => continue` arm (158): a well-formed drift line whose
    // kind is none of the four known kinds is silently ignored — no count, no sample.
    #[test]
    fn scan_ignores_an_unknown_drift_kind() {
        let line =
            "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"bogus_kind\" name=X";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.total(), 0, "line:\n{line}");
        assert!(r.samples.is_empty(), "{:?}", r.samples);
        assert!(
            r.last_ts.is_none(),
            "an ignored kind must not stamp last_ts"
        );
    }

    // parse_version skip arm (355→360 without push): a dotted run whose major
    // overflows u64 is DROPPED (not pushed), so the next valid run is selected; a
    // pure-overflow banner with no fallback run yields None.
    #[test]
    fn parse_version_skips_a_run_with_an_overflowing_major() {
        // 23-digit major overflows u64 → that run is dropped → the v-run wins.
        assert_eq!(
            parse_version("99999999999999999999999.0 v1.2.3"),
            Some((1, 2, 3))
        );
        // No fallback run → the overflowing run is the only one → None.
        assert_eq!(parse_version("99999999999999999999999.0"), None);
    }

    // verdict_glyph en-dash branch (404→405): a CLEAN transcript-only row (no target,
    // no drift, not broken) renders `–`, distinct from the existing transcript-only
    // test row which carries drift (→ `⚠`).
    #[test]
    fn verdict_glyph_dash_for_clean_transcript_only_row() {
        let row = DoctorSourceRow {
            prefix: "cp",
            source_id: "copilot",
            connected: true,
            has_target: false,
            hooks_installed: false,
            installed_version: None,
            verified_version: "unknown",
            scan: LogScanResult::default(),
            schema: None,
        };
        let s = format_doctor_row(&row);
        assert!(
            s.starts_with("  \u{2013}"),
            "clean transcript-only leads with –: {s}"
        );
        assert!(s.contains("transcript-only"), "{s}");
        assert!(!s.contains('\n'), "a clean row is a single line: {s}");
    }

    // verdict_glyph circle branch (406→407) + format state "not installed" (453→454):
    // a row with a target but no installed hooks, clean scan/schema → `○` + "not
    // installed". connected:false also covers the "disconnected" label (448→449).
    #[test]
    fn row_installable_but_not_installed_shows_circle_and_not_installed() {
        let row = DoctorSourceRow {
            prefix: "cc",
            source_id: "claude-code",
            connected: false,
            has_target: true,
            hooks_installed: false,
            installed_version: None,
            verified_version: "unknown",
            scan: LogScanResult::default(),
            schema: None,
        };
        let s = format_doctor_row(&row);
        assert!(
            s.starts_with("  \u{25cb}"),
            "installable-not-installed leads with ○: {s}"
        );
        assert!(s.contains("not installed"), "{s}");
        assert!(
            s.contains("disconnected"),
            "connected:false → disconnected: {s}"
        );
        assert!(!s.contains('\n'), "no problem → single line: {s}");
    }

    // format_doctor_row soft-note continuation (477→478): a SOUND schema (no issues)
    // with non-empty notes emits a `↳ note: …` line but stays a `✓` healthy verdict,
    // never "broken".
    #[test]
    fn format_row_emits_note_continuation_for_sound_schema_with_notes() {
        let row = DoctorSourceRow {
            prefix: "cw",
            source_id: "codewhale",
            connected: true,
            has_target: true,
            hooks_installed: true,
            installed_version: Some("1.0.0".into()),
            verified_version: "unknown",
            scan: LogScanResult::default(),
            schema: Some(crate::install::verify::SchemaVerifyResult {
                issues: vec![],
                notes: vec!["pixtuoid-hook not on PATH".into()],
            }),
        };
        let s = format_doctor_row(&row);
        assert!(s.starts_with("  \u{2713}"), "sound-with-notes still ✓: {s}");
        assert!(
            s.contains("\n       \u{21b3} note: pixtuoid-hook not on PATH"),
            "note on its own ↳ line: {s}"
        );
        assert!(
            !s.to_lowercase().contains("broken"),
            "a note is not broken: {s}"
        );
    }

    // drift_detail kind-branches (417/423/426) + samples join (436) + when=last_ts:
    // a row carrying every drift kind, samples, and a timestamp renders the full
    // detail line. The existing format test only exercises missing-field (419-421).
    #[test]
    fn format_row_drift_detail_covers_all_kinds_samples_and_last_ts() {
        let row = DoctorSourceRow {
            prefix: "cp",
            source_id: "copilot",
            connected: true,
            has_target: false,
            hooks_installed: false,
            installed_version: None,
            verified_version: "unknown",
            scan: LogScanResult {
                unknown_event: 2,
                missing_field: 0,
                unknown_dispatch: 1,
                shape_drift: 1,
                samples: vec!["NewHook".into(), "MyTool".into()],
                last_ts: Some("2026-06-15T00:00:00Z".into()),
            },
            schema: None,
        };
        let s = format_doctor_row(&row);
        assert!(
            s.contains("decode drift: 2 unknown-event, 1 unknown-dispatch, 1 shape-drift"),
            "{s}"
        );
        assert!(s.contains("(last 2026-06-15T00:00:00Z)"), "{s}");
        assert!(s.contains("[NewHook, MyTool]"), "{s}");
    }
}
