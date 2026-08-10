//! `pixtuoid doctor` — read-only source self-diagnosis. Surfaces the
//! decode-drift breadcrumbs (`source/drift.rs`, under the `pixtuoid::drift`
//! tracing target) that otherwise die in the warn-floor log nobody reads.
//!
//! Strictly READ-ONLY: it never writes config (re-connecting hooks stays the
//! Sources panel's job) and never spawns the TUI. The PROBED CLI is not
//! read-only about its own state, though — several bootstrap their state dir on
//! any invocation — so the `<cli> --version` probe is gated by
//! `may_probe_version` on evidence the user already runs that CLI; see the
//! `doctor`-probe sharp edge in `crates/pixtuoid/CLAUDE.md`. Untrusted wire
//! values (event/tool names) are `sanitize`d before display.

use pixtuoid_core::source::{drift, registry};

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct LogScanResult {
    pub unknown_event: u64,
    pub missing_field: u64,
    pub unknown_dispatch: u64,
    pub shape_drift: u64,
    /// Sanitized, deduped, capped distinctive values — safe to print.
    pub samples: Vec<String>,
    /// The leading timestamp token of the latest matching log line.
    pub last_ts: Option<String>,
}

impl LogScanResult {
    pub(crate) fn total(&self) -> u64 {
        self.unknown_event + self.missing_field + self.unknown_dispatch + self.shape_drift
    }
}

const SAMPLE_CAP: usize = 5;

// Strip control chars from an untrusted wire value before it reaches stdout.
use crate::strip_control_chars as sanitize;

struct DriftLine<'a> {
    source: &'a str,
    kind: &'a str,
    /// The fields segment AFTER the `target:` marker, so a span field of the
    /// same name (rendered BEFORE the target) can't be picked up.
    fields: &'a str,
}

/// Parse a warn-floor log line as a drift breadcrumb, anchored on the STRUCTURAL
/// tracing-fmt `target:` marker rather than a loose `contains`, so a line that
/// merely MENTIONS the literal inside a field value isn't matched and a longer
/// `a::b::pixtuoid::drift` target can't suffix-match ours. `marker` is
/// `"<TARGET>: "`, hoisted by the caller to avoid a per-line alloc. Accepted
/// residual: a non-drift line whose value literally embeds
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

/// Pull a field value from a tracing-fmt fields segment, in both the quoted
/// (`key="…"`) and unquoted Display (`key=val`) forms. An unquoted value runs to
/// the next ` <ident>=` field boundary, not merely the next whitespace, so a
/// hostile wire name containing spaces survives whole; and the key must START a
/// field so `name` can't match inside `displayName=`.
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

/// Scan warn-floor log TEXT (not a path, so it's testable against real fmt
/// output) for `pixtuoid::drift` breadcrumbs for ONE source.
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
/// the log — for the live footer nudge.
pub(crate) fn drifted_sources(log: &str) -> Vec<String> {
    registry::registered_source_names()
        .filter(|s| scan_log_for_source(log, s).total() > 0)
        .filter_map(|s| registry::descriptor_for(s).map(|d| d.label_prefix.to_string()))
        .collect()
}

/// Merge the source-death footer warning (HIGHEST priority — the office is
/// partially frozen) with a passive decode-drift nudge.
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
    // No leading `⚠`: the footer painter (`footer.rs` `" ⚠ {warn} "`) owns the
    // glyph, so embedding one here double-prints it (`⚠ ⚠ decode drift`).
    Some(format!("decode drift: {prefixes} — run `pixtuoid doctor`"))
}

/// Windows-only advisory for the "installed but no sprite" path-split class:
/// when `HOME` is set and differs from `%USERPROFILE%`, a source whose CLI
/// resolves its home differently than pixtuoid did may have its hooks written
/// where the CLI never reads. pixtuoid already mirrors the HOME-first CLIs
/// (`platform::home_first_dir`), so this is a SAFETY NET for a residual resolver
/// mismatch. Pure (env + platform injected) so it unit-tests on any host.
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
    // report.
    Some(format!(
        "⚠ Windows: HOME ({}) differs from USERPROFILE ({}). pixtuoid resolves \
         CodeWhale/OpenClaw HOME-first to match their CLIs — but if a source's \
         sprite is missing, confirm its hook config landed under the home that \
         CLI actually reads.",
        sanitize(home),
        sanitize(up)
    ))
}

/// Collapses only COSMETIC differences (case, slash direction, trailing
/// separator). A `/c/Users/me`-vs-`C:\Users\me` split is a REAL divergence
/// (different roots) and must stay unequal.
fn win_path_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

/// The SHARED rollup the Connection panel, the boot preflight, and the CLI
/// report all read, so those surfaces can't drift apart. Scope is the CHEAP
/// signals only — version skew stays report-only, because the `<cli> --version`
/// probe is too costly for an interactive panel-open across N sources.
#[derive(Debug, Default)]
pub struct SourceDiagnostics {
    /// `Some` only when hooks are installed in the target's config; `None` = not
    /// checked (no target / not installed).
    pub install: Option<crate::install::verify::SchemaVerifyResult>,
    pub(crate) drift: LogScanResult,
}

impl SourceDiagnostics {
    /// A HARD install problem ⇒ the source is broken (zero sprites despite a
    /// claimed connection). Soft notes and drift do NOT count as broken.
    pub fn is_broken(&self) -> bool {
        self.install.as_ref().is_some_and(|i| !i.is_sound())
    }

    /// The single worst issue as a one-line, glyph-prefixed summary. Priority:
    /// install-broken (hooks can't fire) > decode-drift.
    pub(crate) fn summary(&self) -> Option<String> {
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

/// The install check runs whenever the source's target has managed hooks
/// installed, NOT gated on the connected flag — the report must surface a stale
/// broken install even on a disconnected source. `config` injects a config root
/// (`None` in prod) so an install-broken verdict is exercisable through the SAME
/// root both the `has_hooks` gate and `verify_target` read.
pub fn diagnose(source: &str, log: &str, config: Option<std::path::PathBuf>) -> SourceDiagnostics {
    let install = crate::install::target::by_source(source)
        .filter(|t| crate::install::has_hooks(t, config.clone()))
        .map(|t| crate::install::verify_target(t, config.clone()));
    SourceDiagnostics {
        install,
        drift: scan_log_for_source(log, source),
    }
}

pub(crate) struct DoctorSourceRow {
    pub prefix: &'static str,
    /// The registry source id (e.g. "claude-code"), distinct from
    /// `install::Target.name`/`display_name`.
    pub source_id: &'static str,
    pub connected: bool,
    pub has_target: bool,
    pub hooks_installed: bool,
    /// Raw probe output, if probeable.
    pub installed_version: Option<String>,
    /// The version this build's decoder was verified against; `"unknown"` = no
    /// anchor.
    pub verified_version: &'static str,
    /// The SAME [`SourceDiagnostics`] the Sources panel + boot preflight read,
    /// embedded whole so `is_broken`/`drift` stay the ONE authority.
    pub diag: SourceDiagnostics,
}

/// A dotted-run major at or above this looks like a YEAR/date token, not a semver
/// major — used to skip a date prefix in favor of a real version.
const IMPLAUSIBLE_MAJOR: u64 = 1000;

/// Extract a `MAJOR.MINOR[.PATCH]` tuple from a `--version` banner. Tolerant:
/// no dotted run = None, so a skew check silently no-ops rather than alarming on
/// garbage. Banner-order robust — a banner can print a dotted DATE/build token
/// BEFORE the semver (`Built 2026.06.04 — v1.2.3`) — while still parsing a
/// genuine CalVer (`2026.06.04`, e.g. cursor) rather than letting it vanish.
pub(crate) fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let bytes = s.as_bytes();
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
            continue;
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

/// Skew is flagged ONLY when both the installed and the (non-`unknown`) verified
/// version parse — otherwise the installed version shows with no alarm.
pub(crate) fn version_status(installed: Option<&str>, verified: &str) -> String {
    // Display the RAW probe string, not a lossy reformat (cursor's
    // `2026.06.04-5fd875e` isn't semver); the skew check still parses internally.
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

fn verdict_glyph(row: &DoctorSourceRow) -> char {
    if row.diag.is_broken() || row.diag.drift.total() > 0 {
        '\u{26a0}' // ⚠
    } else if !row.has_target {
        '\u{2013}' // –
    } else if !row.hooks_installed {
        '\u{25cb}' // ○
    } else {
        '\u{2713}' // ✓
    }
}

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

/// Render one row: a scannable verdict line, plus an indented `↳` continuation
/// line per problem so the long detail never wrecks the table's column alignment.
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
    // `issues` are already control-char sanitized at the source
    // (`verify::display_safe`).
    if let Some(s) = &row.diag.install {
        if !s.is_sound() {
            out.push_str(&format!(
                "\n       \u{21b3} install broken: {}",
                s.issues.join("; ")
            ));
        } else if !s.notes.is_empty() {
            out.push_str(&format!("\n       \u{21b3} note: {}", s.notes.join("; ")));
        }
    }
    if row.diag.drift.total() > 0 {
        out.push_str(&format!(
            "\n       \u{21b3} decode drift: {}",
            drift_detail(&row.diag.drift)
        ));
    }
    out
}

/// First non-empty line of subprocess output, trimmed AND control-char
/// `sanitize`d — `--version` output is untrusted (a PATH-substituted binary
/// could emit ANSI/OSC to manipulate the terminal).
fn first_sanitized_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(sanitize)
}

/// Probe a source's `<cli> --version` (argv from the static registry — never
/// user input) → the first non-empty output line, sanitized. Best-effort; a
/// spawn error, a NONZERO exit (whose error text must never show as a version),
/// or a hang all yield None. stdin is nulled so the child can't block on the
/// inherited TTY, and it is killed after a deadline because `output()` has no
/// timeout.
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
    if !output.status.success() {
        return None;
    }
    first_sanitized_line(&output.stdout).or_else(|| first_sanitized_line(&output.stderr))
}

/// Whether `doctor` may spawn this source's `<cli> --version` probe: only with
/// evidence the user already runs that CLI — CONNECTED, or its install target
/// probed PRESENT. `cli_detected` is `None` for a transcript-only source.
///
/// `--version` is not side-effect-free on the other side: several agent CLIs
/// bootstrap their own state dir on ANY invocation, and those dirs are exactly
/// what `presence_probe` / `detect_installed` key on, so an unconditional probe
/// MANUFACTURED the presence it was diagnosing and the natural `doctor` →
/// `setup --yes` order then installed hooks into CLIs `setup --yes` alone had
/// just declined to touch. Gating on presence removes the observer effect by
/// construction: a CLI that has already written its own state cannot be
/// perturbed into existence by one more `--version`.
fn may_probe_version(connected: bool, cli_detected: Option<bool>) -> bool {
    connected || cli_detected.unwrap_or(false)
}

fn activation_backend() -> String {
    #[cfg(target_os = "macos")]
    {
        "NSRunningApplication (macOS) ✓".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        linux_activation_backend(
            marker_set(std::env::var(crate::focus::SWAY_ENV).ok()),
            marker_set(std::env::var(crate::focus::HYPRLAND_ENV).ok()),
            marker_set(std::env::var("WAYLAND_DISPLAY").ok()),
            marker_set(std::env::var("DISPLAY").ok()),
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

/// Is a compositor/display env marker actually SET? EMPTY (or whitespace-only)
/// counts as UNSET, NOT bare presence: a leftover `WAYLAND_DISPLAY=`/`SWAYSOCK=`
/// (systemd user units and non-forwarded ssh sessions leave them routinely)
/// would otherwise print a confidently wrong verdict at a user whose X11 EWMH
/// channel works fine. `focus::linux::detect_channel` keys the live channel on
/// the SAME rule.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn marker_set(value: Option<String>) -> bool {
    crate::install::io::nonempty(value).is_some()
}

/// Mirrors `focus/linux.rs`'s ONE-channel-per-env order (sway IPC → hyprland IPC
/// → X11 EWMH → nothing) — EXCEPT that a Wayland session without a
/// pid-addressable IPC must NOT be reported as "X11 EWMH ✓": XWayland sets
/// $DISPLAY, but a native-Wayland terminal never appears in XWayland's client
/// list and mutter/kwin block focus-steal anyway, so the ✓ would mislead exactly
/// the users focus fails for.
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

/// The family buckets come straight from the registry, so a new source lands in
/// the right bucket with no edit here.
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
    for src in registry::registered_source_names() {
        let Some(d) = registry::descriptor_for(src) else {
            continue;
        };
        // Daemons aren't click-focusable agents; the TranscriptProbe sources get
        // their own rows below.
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
            "  {} — shim-resolved `_pid` rides each hook event (an ancestor walk past the interposed shell — a `$SHELL -c` wrapper, a `cmd.exe /C` — on macOS/Linux/Windows; a raw parent elsewhere)\n",
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

/// Read the warn-floor log for a drift scan, separating "there is no log yet"
/// from "the log could not be read" — the latter gets a warning line.
///
/// A missing log is the ordinary no-TUI-run-yet state and genuinely means "no
/// drift recorded". Every other error class means the drift counts are UNKNOWN,
/// and folding that into the same silent empty string made `doctor` positively
/// assert `✓ no decode drift` off an input it never read.
///
/// The warning is `sanitize`d where it is MINTED, not per presenter: the path it
/// interpolates comes from `PIXTUOID_LOG`/`XDG_STATE_HOME`, and the two readers
/// print it to different terminals — sanitizing per reader is exactly how the
/// escape reached one of them.
pub fn read_log(path: &std::path::Path) -> (String, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(s) => (s, None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => (
            String::new(),
            Some(sanitize(&format!(
                "log unreadable: {} ({e}) — the decode-drift counts below are not meaningful",
                path.display()
            ))),
        ),
    }
}

/// Returns the rendered report rather than printing it, so the WHOLE report
/// builder is unit-testable. `log_path` is injected by `main`, which owns the
/// log-path resolution; `graphics` is the `--graphics` flag.
pub fn run(log_path: &std::path::Path, graphics: crate::GraphicsMode) -> anyhow::Result<String> {
    let mut warnings = Vec::new();
    let cfg = crate::config::load(&crate::config::config_path(), &mut warnings);
    // `doctor` is a separate PROCESS from the running TUI, so it derives the
    // connected-set fresh from config rather than the live in-process
    // `ConnectedSources` it can't see. That can lag a just-made in-TUI toggle
    // until it persists, which it always does (persist-first).
    let connected = crate::config::resolve_connected(&cfg);
    let (log, log_warning) = read_log(log_path);

    let mut out = String::from("pixtuoid doctor — source health\n");
    out.push_str(&format!("log: {}\n", log_path.display()));
    out.push_str(&format!(
        "config: {}\n",
        crate::config::config_path().display()
    ));
    // ASK the terminal for truecolor (DECRQSS) — but ONLY when stdout is a real
    // tty, so a piped `pixtuoid doctor > file` neither emits escape codes nor
    // blocks (and the capture-output test harness never probes), and only when
    // $TERM isn't dumb, which can't answer. The same `color_preflight` the
    // launcher acts on drives both the probe skip and the color-status line, so
    // the diagnostic matches what `run` would do.
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
    // Whether this terminal could paint the cutaway profile, gated on the SAME
    // `probe_ok` as the truecolor row: the capability query writes escape
    // sequences and reads the replies, so a piped `doctor > file` must neither
    // emit them nor wait for an answer that cannot come. `--graphics off` skips
    // it too — the flag says not to consider protocols, so asking anyway would
    // spend up to 2s establishing a fact the answer cannot use.
    let ask = probe_ok && graphics != crate::GraphicsMode::Off;
    out.push_str(&crate::graphics::graphics_diagnostic_row(
        graphics,
        ask.then(crate::graphics::detect).flatten(),
        // The pack a default `run` resolves — embedded, with the user's
        // `$XDG_CONFIG_HOME/pixtuoid/sprites` merged over it if they have one.
        // NOT just the bundled art: a user pack that ships density variants
        // raises this, and reporting the bundled number would understate what
        // their own `run` could draw. `--pack-dir` is the one case this misses,
        // and doctor was not pointed at it.
        pixtuoid_scene::embedded_pack::load_sprite_pack(None)
            .map_or(1, |p| p.max_density_variant()),
    ));
    out.push('\n');
    // A malformed config makes every source read disconnected, and a diagnostic
    // tool must say WHY rather than silently swallow it. Sanitized: a warning can
    // interpolate config content.
    for w in &warnings {
        out.push_str(&format!("⚠ config: {}\n", sanitize(w)));
    }
    if let Some(w) = &log_warning {
        out.push_str(&format!("⚠ {}\n", sanitize(w)));
    }
    out.push('\n');

    let mut any_drift = false;
    let mut broken: Vec<String> = Vec::new();
    for src in registry::registered_source_names() {
        let desc = registry::descriptor_for(src);
        let target = crate::install::target::by_source(src);
        let hooks_installed = target
            .map(|t| crate::install::has_hooks(t, None))
            .unwrap_or(false);
        let diag = diagnose(src, &log, None);
        let is_connected = connected.contains(src);
        let cli_detected = target.map(crate::install::target::is_present);
        let row = DoctorSourceRow {
            prefix: desc.map(|d| d.label_prefix).unwrap_or("??"),
            source_id: src,
            connected: is_connected,
            has_target: target.is_some(),
            hooks_installed,
            installed_version: may_probe_version(is_connected, cli_detected)
                .then(|| desc.and_then(|d| d.version_probe).and_then(probe_version))
                .flatten(),
            verified_version: desc.map(|d| d.verified_version).unwrap_or("unknown"),
            diag,
        };
        any_drift |= row.diag.drift.total() > 0;
        if row.diag.is_broken() {
            broken.push(format!("{}\u{b7}{}", row.prefix, row.source_id));
        }
        out.push_str(&format_doctor_row(&row));
        out.push('\n');
    }

    out.push_str(&health_summary(
        registry::registered_source_names().count(),
        &broken,
        any_drift,
    ));

    // A wrong root has no symptom but an empty office, so state it outright
    // (#880). Via `resolved_source_root` — the SAME call the driver makes, since
    // a second copy could show a healthy path while the watcher polled another.
    let roots: Vec<String> = registry::registered_source_names()
        .filter_map(|src| {
            let root = pixtuoid_core::source::resolved_source_root(src)?;
            let env = registry::descriptor_for(src)
                .and_then(|d| d.home_env)
                // Trim-based, matching every resolver's `nonempty` policy: a `"  "`
                // override is ignored by the resolver, so it must not render as
                // `via $VAR` or raise the ⚠ (the #172 class).
                .map(|v| {
                    let set = std::env::var(v).is_ok_and(|s| !s.trim().is_empty());
                    (v, set)
                });
            Some(root_row(src, &root, root.is_dir(), env))
        })
        .collect();
    if !roots.is_empty() {
        out.push_str("\nresolved roots\n");
        for r in roots {
            out.push_str(&r);
            out.push('\n');
        }
    }
    // Probe roots come from the DEFAULT resolution: a --projects-root /
    // --codex-sessions-root override changes the RUNNING app, but doctor
    // deliberately diagnoses the default setup.
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
    // Read as BYTES, lossy ONLY here: the advisory renders these for a human,
    // it never opens them, so this is the one boundary where losing an
    // ill-formed byte costs nothing.
    let (home, up) = (
        pixtuoid_core::platform::path_env("HOME"),
        pixtuoid_core::platform::path_env("USERPROFILE"),
    );
    if let Some(adv) = home_split_advisory(
        cfg!(windows),
        home.as_ref().map(|p| p.to_string_lossy()).as_deref(),
        up.as_ref().map(|p| p.to_string_lossy()).as_deref(),
    ) {
        out.push_str(&format!("\n{adv}\n"));
    }
    Ok(out)
}

/// One `roots:` line per source with a single on-disk root. A FACT row, not an
/// alarm — a missing root is normal for a CLI the user never runs, so the only
/// ⚠ is missing WHILE that source's override is set (#880).
pub(crate) fn root_row(
    source: &str,
    root: &std::path::Path,
    exists: bool,
    home_env: Option<(&str, bool)>,
) -> String {
    let via = match home_env {
        Some((var, true)) => format!(" via ${var}"),
        _ => String::new(),
    };
    let mark = if exists { "ok" } else { "missing" };
    let mut line = format!(
        "  {:<13} {} ({mark}{via})",
        source,
        sanitize(&root.display().to_string())
    );
    if !exists {
        if let Some((var, true)) = home_env {
            line.push_str(&format!(
                "\n    ⚠ ${var} is set but that root does not exist — if this source's \
                 sprite never appears, that env var is the first thing to check"
            ));
        }
    }
    line
}

/// The rolled-up doctor footer: the broken-install rollup (locally fixable via
/// reconnect) + the decode-drift note (report-upstream).
fn health_summary(n: usize, broken: &[String], any_drift: bool) -> String {
    let mut out = String::new();
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_capture::capture;

    #[test]
    fn health_summary_one_broken_uses_the_singular_verb() {
        let s = health_summary(9, &["cc·claude-code".to_string()], false);
        assert!(
            s.contains("⚠ 1 needs attention (cc·claude-code)"),
            "singular verb + the broken id: {s:?}"
        );
        assert!(s.contains("✓ no decode drift"), "clean-drift branch: {s:?}");
    }

    #[test]
    fn health_summary_many_broken_pluralizes_and_comma_joins() {
        let s = health_summary(
            9,
            &["cc·claude-code".to_string(), "cx·codex".to_string()],
            false,
        );
        assert!(
            s.contains("⚠ 2 need attention (cc·claude-code, cx·codex)"),
            "plural verb + comma-joined list: {s:?}"
        );
    }

    #[test]
    fn health_summary_clean_installs_with_and_without_drift() {
        // Exact byte layout, deliberately: the substring tests above can't catch a
        // whitespace regression in the leading `\n` or the ` · ` joiners.
        let clean = health_summary(9, &[], false);
        assert_eq!(
            clean,
            "\n9 sources · ✓ all connected installs sound · ✓ no decode drift\n"
        );
        let drifted = health_summary(9, &[], true);
        assert!(
            drifted.contains("⚠ decode drift recorded"),
            "drift flag surfaces the report note: {drifted:?}"
        );
    }

    #[test]
    fn linux_activation_backend_covers_every_channel_in_priority_order() {
        assert!(linux_activation_backend(true, true, true, true).contains("sway"));
        assert!(linux_activation_backend(false, true, true, true).contains("hyprland"));
        assert!(linux_activation_backend(false, false, true, true).contains("✗ Wayland"));
        assert!(linux_activation_backend(false, false, false, true).contains("EWMH"));
        assert!(linux_activation_backend(false, false, false, false).contains("✗ none"));
    }

    #[test]
    fn an_exported_but_blank_compositor_marker_is_not_a_running_compositor() {
        assert!(!marker_set(None));
        assert!(!marker_set(Some(String::new())), "SWAYSOCK= is a leftover");
        assert!(!marker_set(Some("  \t ".to_string())));
        assert!(marker_set(Some("/run/user/1000/sway-ipc.sock".to_string())));
        assert_eq!(
            linux_activation_backend(
                marker_set(Some(String::new())),
                marker_set(None),
                marker_set(Some(String::new())),
                marker_set(Some(":0".to_string())),
            ),
            "X11 EWMH ($DISPLAY) ✓"
        );
    }

    #[test]
    fn focus_section_buckets_sources_from_the_registry() {
        let cc = std::path::Path::new("/home/u/.claude/sessions");
        let cx = std::path::Path::new("/home/u/.codex/sessions");
        let s = focus_section("test-backend ✓", Some((cc, true)), (cx, true));
        assert!(s.contains("backend: test-backend ✓"));
        assert!(s.contains("claude-code — registry probe") && s.contains("✓"));
        assert!(s.contains("codex — rollout fd probe"));
        assert!(
            s.contains("opencode") && s.contains("plugin-stamped"),
            "plugin stampers listed separately: {s}"
        );
        assert!(
            s.contains("cursor") && s.contains("shim-resolved `_pid`"),
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
        let out = run(
            std::path::Path::new("/nonexistent-pixtuoid-doctor-log"),
            crate::GraphicsMode::Auto,
        )
        .unwrap();
        assert!(out.contains("pixtuoid doctor — source health"), "{out}");
        assert!(out.contains("config:"), "{out}");
        assert!(out.contains("terminal: TERM="), "{out}");
        assert!(out.contains("sources \u{b7}"), "{out}");
    }

    #[test]
    fn an_unreadable_log_is_reported_but_a_missing_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_log(&dir.path().join("nope")), (String::new(), None));

        let f = dir.path().join("log");
        std::fs::write(&f, "hello").unwrap();
        assert_eq!(read_log(&f), ("hello".to_string(), None));

        // A directory reads as IsADirectory / PermissionDenied — never NotFound,
        // on any platform and any uid (a chmod-000 file would not bind as root).
        let (text, warning) = read_log(dir.path());
        assert!(text.is_empty());
        let warning = warning.expect("an unreadable log must be reported");
        assert!(
            warning.contains("log unreadable") && warning.contains("not meaningful"),
            "got: {warning}"
        );

        let out = run(dir.path(), crate::GraphicsMode::Auto).unwrap();
        assert!(out.contains("⚠ log unreadable"), "{out}");
    }

    #[test]
    fn the_unreadable_log_warning_is_stripped_where_it_is_minted() {
        // Asserted on `read_log`'s OWN return value, not on either presenter —
        // sanitizing per presenter is how one of them shipped raw.
        let dir = tempfile::tempdir().unwrap();
        // Windows forbids codepoints 1-31 in a filename (Microsoft's "Naming Files,
        // Paths, and Namespaces"), so the Cc half of the vector cannot exist in a
        // path there; U+202E is unreserved and still reaches the terminal.
        let hostile = if cfg!(windows) {
            dir.path().join("l\u{202e}og")
        } else {
            dir.path().join("l\u{1b}]0;PWNED\u{7}\u{202e}og")
        };
        std::fs::create_dir(&hostile).unwrap(); // a directory never reads as NotFound
        let (_, warning) = read_log(&hostile);
        let warning = warning.expect("an unreadable log must be reported");
        assert!(
            !warning.contains(['\u{1b}', '\u{7}', '\u{202e}']),
            "the minted warning carries a live OSC / Trojan-Source override: {warning:?}"
        );
        assert!(warning.contains("log unreadable"), "got: {warning}");
    }

    #[test]
    fn version_probe_is_gated_on_evidence_the_user_runs_that_cli() {
        assert!(may_probe_version(true, Some(false)));
        assert!(may_probe_version(true, None));
        assert!(may_probe_version(false, Some(true)));
        assert!(!may_probe_version(false, Some(false)));
        assert!(!may_probe_version(false, None));
    }

    #[cfg(unix)]
    #[test]
    fn run_never_spawns_a_version_probe_for_a_cli_it_has_no_evidence_of() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (home, bin) = (dir.path().join("home"), dir.path().join("bin"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        // Spawning the stand-in CLI AT ALL leaves this trace.
        let marker = dir.path().join("spawned");
        let fake = bin.join("opencode");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\n: > '{}'\necho 1.0.0\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let saved: Vec<(&str, Option<std::ffi::OsString>)> =
            ["HOME", "XDG_CONFIG_HOME", "PATH", "OPENCODE_CONFIG_DIR"]
                .iter()
                .map(|k| (*k, std::env::var_os(k)))
                .collect();
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::remove_var("OPENCODE_CONFIG_DIR");
        std::env::set_var("PATH", &bin);
        let out = run(
            std::path::Path::new("/nonexistent-pixtuoid-doctor-log"),
            crate::GraphicsMode::Auto,
        );
        let spawned = marker.exists();
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }

        out.expect("the report still builds");
        assert!(
            !spawned,
            "doctor spawned `opencode --version` in a pristine HOME where opencode \
             is undetected — the probe must be gated on evidence the user runs it"
        );
    }

    #[test]
    fn root_row_states_the_fact_and_only_warns_on_missing_with_an_override() {
        let p = std::path::Path::new("/home/u/.copilot/session-state");

        let ok = root_row("copilot", p, true, Some(("COPILOT_HOME", true)));
        assert!(ok.contains("(ok via $COPILOT_HOME)"), "{ok}");
        assert!(!ok.contains('⚠'), "an existing root never warns: {ok}");

        // A missing root with NO override is the ordinary "never ran this CLI"
        // case — a fact, not an alarm.
        let quiet = root_row("copilot", p, false, Some(("COPILOT_HOME", false)));
        assert!(quiet.contains("(missing)"), "{quiet}");
        assert!(!quiet.contains('⚠'), "no override set -> no alarm: {quiet}");
        assert!(
            !quiet.contains("via $"),
            "an unset var is not a 'via': {quiet}"
        );

        // Missing WHILE overridden is the #880 shape, and the only one worth a ⚠.
        let loud = root_row("copilot", p, false, Some(("COPILOT_HOME", true)));
        assert!(
            loud.contains('⚠') && loud.contains("COPILOT_HOME"),
            "{loud}"
        );

        // A source with no override column at all never grows a `via`.
        let plain = root_row("antigravity", p, false, None);
        assert!(!plain.contains('⚠') && !plain.contains("via $"), "{plain}");
    }

    #[test]
    fn home_split_advisory_fires_only_on_windows_with_a_real_home_split() {
        let a = home_split_advisory(true, Some(r"C:\msys\home\me"), Some(r"C:\Users\me"));
        assert!(a.is_some());
        let a = a.unwrap();
        assert!(a.contains("HOME") && a.contains("USERPROFILE") && a.contains("sprite"));

        assert!(home_split_advisory(false, Some("/home/a"), Some("/home/b")).is_none());

        assert!(home_split_advisory(true, None, Some(r"C:\Users\me")).is_none());
        assert!(home_split_advisory(true, Some("  "), Some(r"C:\Users\me")).is_none());
        assert!(home_split_advisory(true, Some(r"C:\Users\me"), None).is_none());
    }

    #[test]
    fn home_split_advisory_ignores_cosmetic_path_differences() {
        assert!(home_split_advisory(true, Some(r"C:\Users\Me"), Some(r"c:/users/me/")).is_none());
        // A POSIX-form HOME vs a native USERPROFILE IS a real split (Git Bash).
        assert!(home_split_advisory(true, Some("/c/Users/me"), Some(r"C:\Users\me")).is_some());
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
            drift::unknown_event("codex", "OtherHook");
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
        let rc = scan_log_for_source(&log, "codex");
        assert_eq!(rc.unknown_event, 1);
        assert_eq!(rc.missing_field, 0);
    }

    #[test]
    fn scan_of_empty_log_is_clean() {
        assert_eq!(scan_log_for_source("", "copilot"), LogScanResult::default());
    }

    // The crafted line carries valid `source=`/`kind=`, so ONLY the missing
    // structural `target:` marker keeps it out of the tally.
    #[test]
    fn scan_ignores_a_body_mention_of_the_target_string() {
        let line = "2026-06-15T00:00:00Z  WARN pixtuoid::source::manager: a pixtuoid::drift mention source=copilot kind=unknown_event name=X";
        assert_eq!(scan_log_for_source(line, "copilot").total(), 0);
    }

    // Distinct from the body-mention case above: the marker IS present, so `find`
    // succeeds and only the space-guard prevents the false count.
    #[test]
    fn scan_rejects_a_longer_target_suffixing_our_token() {
        let line = "2026-06-15T00:00:00Z  WARN myapp::pixtuoid::drift: source=copilot kind=\"unknown_event\" name=X";
        assert_eq!(scan_log_for_source(line, "copilot").total(), 0);
    }

    // No production code wraps a decoder in a `source=`-carrying span today; this
    // pins the parser so adding one later can't silently misattribute.
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
        assert_eq!(scan_log_for_source(line, "spanwrong").total(), 0);
    }

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
                drift::unknown_event("cursor", "Dup");
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
            diag: SourceDiagnostics {
                install: Some(crate::install::verify::SchemaVerifyResult::default()),
                drift: LogScanResult::default(),
            },
        };
        let c = format_doctor_row(&clean);
        assert!(c.contains("codex") && c.contains("connected") && c.contains("installed"));
        assert!(c.contains("2.0.0"));
        assert!(c.starts_with("  \u{2713}"), "sound row leads with ✓: {c}");
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
            diag: SourceDiagnostics {
                install: None,
                drift: LogScanResult {
                    missing_field: 3,
                    ..Default::default()
                },
            },
        };
        let d = format_doctor_row(&drifted);
        assert!(
            d.starts_with("  \u{26a0}"),
            "a drifting row leads with ⚠: {d}"
        );
        assert!(d.contains("transcript-only"), "{d}");
        assert!(d.contains("NEWER than verified"), "skew flagged: {d}");
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
            diag: SourceDiagnostics {
                install: Some(crate::install::verify::SchemaVerifyResult {
                    issues: vec!["shim binary missing: /old/pixtuoid-hook".into()],
                    notes: vec![],
                }),
                drift: LogScanResult::default(),
            },
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
    fn diagnose_surfaces_an_install_broken_verdict_through_the_injected_config_root() {
        // BOTH the has_hooks gate and verify_target must read the SAME injected
        // root, or the broken verdict is unreachable hermetically.
        use crate::install::{install_target, target::CLAUDE};
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        std::fs::write(&cfg, "{}\n").unwrap();
        install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(std::path::PathBuf::from("/nonexistent/pixtuoid-hook")),
        )
        .unwrap();

        let d = diagnose(CLAUDE.core_source, "", Some(cfg));
        assert!(
            d.is_broken(),
            "a sentinel'd install with a missing shim must read broken through the injected root"
        );
        let s = d.summary().unwrap();
        assert!(
            s.contains("install broken") && s.contains("shim binary missing"),
            "unexpected summary: {s:?}"
        );
    }

    #[test]
    fn parse_version_extracts_the_dotted_run() {
        assert_eq!(parse_version("1.0.62"), Some((1, 0, 62)));
        assert_eq!(
            parse_version("GitHub Copilot CLI 1.0.62."),
            Some((1, 0, 62))
        );
        assert_eq!(parse_version("v2.1"), Some((2, 1, 0)));
        assert_eq!(parse_version("codex 0.41.0 (abc)"), Some((0, 41, 0)));
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("2026"), None);
    }

    #[test]
    fn parse_version_is_banner_order_robust() {
        assert_eq!(parse_version("Built 2026.06.04 — v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("Built 2026.06.04 — 1.2.3"), Some((1, 2, 3)));
        // A genuine CalVer with NO semver must fall back, not vanish.
        assert_eq!(parse_version("2026.06.04-5fd875e"), Some((2026, 6, 4)));
        assert_eq!(parse_version("GitHub Copilot CLI 1.0.62"), Some((1, 0, 62)));
    }

    #[test]
    fn version_status_flags_skew_only_with_a_known_anchor() {
        let u = version_status(Some("3.4.5"), "unknown");
        assert_eq!(u, "3.4.5");
        assert!(!u.contains("verified"));
        assert!(version_status(Some("1.1.0"), "1.0.62").contains("NEWER than verified"));
        assert!(version_status(Some("1.0.0"), "1.0.62").contains("older than verified"));
        assert!(version_status(Some("1.0.62"), "1.0.62").contains("matches verified"));
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
        assert_eq!(
            footer_warning(Some("source 'x' died"), &d).as_deref(),
            Some("source 'x' died")
        );
        let w = footer_warning(None, &d).unwrap();
        assert!(
            w.contains("cc·") && w.contains("cx·") && w.contains("doctor"),
            "{w}"
        );
        // The footer painter (`footer.rs` `" ⚠ {warn} "`) owns the warning glyph;
        // an embedded one double-prints (`⚠ ⚠ …`).
        assert!(!w.contains('⚠'), "drift msg must not embed ⚠: {w}");
        // The REAL `source_warning_message` output, not a literal, so the death
        // tier is checked against its actual producer.
        let death = crate::tui::widgets::source_warning_message(&[
            pixtuoid_core::source::manager::SourceDeath::new("claude-code", "x"),
        ])
        .unwrap();
        let dw = footer_warning(Some(&death), &d).unwrap();
        assert!(!dw.contains('⚠'), "death msg must not embed ⚠: {dw}");
        assert_eq!(footer_warning(None, &[]), None);
    }

    #[test]
    fn probe_output_is_sanitized_and_first_nonempty() {
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

    #[test]
    fn scan_does_not_pick_name_inside_displayname() {
        let line = "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"unknown_event\" displayName=foo name=Real";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_event, 1, "line:\n{line}");
        assert!(r.samples.contains(&"Real".to_string()), "{:?}", r.samples);
        assert!(!r.samples.contains(&"foo".to_string()), "{:?}", r.samples);
    }

    #[test]
    fn scan_unknown_event_without_a_name_field_counts_but_samples_none() {
        let line =
            "2026-06-15T00:00:00Z  WARN pixtuoid::drift: source=copilot kind=\"unknown_event\"";
        let r = scan_log_for_source(line, "copilot");
        assert_eq!(r.unknown_event, 1, "line:\n{line}");
        assert!(r.samples.is_empty(), "{:?}", r.samples);
    }

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

    #[test]
    fn parse_version_skips_a_run_with_an_overflowing_major() {
        // The 23-digit major overflows u64, so that run is dropped.
        assert_eq!(
            parse_version("99999999999999999999999.0 v1.2.3"),
            Some((1, 2, 3))
        );
        assert_eq!(parse_version("99999999999999999999999.0"), None);
    }

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
            diag: SourceDiagnostics {
                install: None,
                drift: LogScanResult::default(),
            },
        };
        let s = format_doctor_row(&row);
        assert!(
            s.starts_with("  \u{2013}"),
            "clean transcript-only leads with –: {s}"
        );
        assert!(s.contains("transcript-only"), "{s}");
        assert!(!s.contains('\n'), "a clean row is a single line: {s}");
    }

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
            diag: SourceDiagnostics {
                install: None,
                drift: LogScanResult::default(),
            },
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
            diag: SourceDiagnostics {
                install: Some(crate::install::verify::SchemaVerifyResult {
                    issues: vec![],
                    notes: vec!["pixtuoid-hook not on PATH".into()],
                }),
                drift: LogScanResult::default(),
            },
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
            diag: SourceDiagnostics {
                install: None,
                drift: LogScanResult {
                    unknown_event: 2,
                    missing_field: 0,
                    unknown_dispatch: 1,
                    shape_drift: 1,
                    samples: vec!["NewHook".into(), "MyTool".into()],
                    last_ts: Some("2026-06-15T00:00:00Z".into()),
                },
            },
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
