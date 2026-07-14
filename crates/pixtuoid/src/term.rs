//! Terminal capability detection for the truecolor preflight (the pixel-art
//! office renders 24-bit half-block SGR; a terminal that can't parse those shows
//! approximated/garbled colors with no other hint — the #1 baffling-bug class for
//! a truecolor-only TUI). The warning is a WARN signal, never a gate on Unix.
//! (Windows is the exception — `tui::mod` hard-gates VT there because the WinAPI
//! color fallback renders black-on-black.)
//!
//! We do NOT guess truecolor from a `$TERM` name allowlist. Detection ASKS the
//! terminal directly: set an unlikely 24-bit background, then `DECRQSS`-query the
//! SGR back — a truecolor terminal echoes the RGB triple, a 256-color one
//! downsamples it, and one that can't even parse the query stays silent
//! (`query_truecolor`, the termstandard/colors method). `$COLORTERM=truecolor`
//! is honored as an explicit positive (the terminal declaring itself — not a
//! guess) purely to skip the round-trip, and `$PIXTUOID_NO_TRUECOLOR_WARN` is an
//! explicit user override; neither is a heuristic. The query is the authority for
//! everything else (#397).

/// Default round-trip budget for the `DECRQSS` probe: long enough for a laggy SSH
/// link to answer, short enough that a terminal which never answers (no DECRQSS
/// support → genuinely suspect) only costs this once at startup. `select` returns
/// the instant a reply arrives, so a responsive terminal never waits the full
/// budget — only non-responders pay it.
pub const TRUECOLOR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// True iff `$COLORTERM` advertises 24-bit color (`truecolor` or `24bit`) — the
/// terminal explicitly declaring itself (the S-Lang convention), honored as a
/// positive so we can skip the terminal round-trip. Pure (takes the env value).
/// Case-sensitive on purpose: the advertised tokens are lowercase by convention.
fn colorterm_is_truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some(v) if v.contains("truecolor") || v.contains("24bit"))
}

/// True iff `$PIXTUOID_NO_TRUECOLOR_WARN` is set to a truthy token (`1` / `true`
/// / `yes` / `on`, case-insensitive, trimmed) — the explicit user override for a
/// terminal we can't auto-detect (e.g. one that's truecolor but doesn't answer
/// DECRQSS). Empty / `0` / `false` / anything else = not suppressed, so a
/// leftover `PIXTUOID_NO_TRUECOLOR_WARN=` doesn't silently kill the warning.
fn truecolor_warn_suppressed(suppress_env: Option<&str>) -> bool {
    matches!(
        suppress_env.map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("1")
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    )
}

/// Whether we're in the zone where the warning *might* fire and so the terminal
/// query is worth running: a TUI `run` (not headless), attached to a tty, where
/// `$COLORTERM` didn't already declare truecolor and the escape hatch isn't set.
/// Pure so the gate is unit-tested; `main.rs` keeps the `#[cfg(not(windows))]`,
/// the `IsTerminal` probe, and the env reads inline at its (codecov-excluded)
/// call site, then runs `query_truecolor` only when this is true (the "policy in
/// term.rs, IO at the call site" pattern). The final decision is: warn unless the
/// query returns `Some(true)`.
pub fn warn_zone(
    cmd_is_run_tui: bool,
    is_tty: bool,
    colorterm: Option<&str>,
    suppress_env: Option<&str>,
) -> bool {
    cmd_is_run_tui
        && is_tty
        && !colorterm_is_truecolor(colorterm)
        && !truecolor_warn_suppressed(suppress_env)
}

/// The pre-flight decision for the terminal TUI's *color* requirement (distinct
/// from the truecolor *depth* warning above). The pixel-art office is 24-bit
/// color end to end with no legible monochrome fallback, so when the environment
/// disables color we refuse to launch the canvas and explain why — mirroring the
/// Windows VT hard-gate — instead of rendering unreadable block-soup with no hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPreflight {
    /// Color is available — launch normally.
    Proceed,
    /// `$NO_COLOR` is set but `$CLICOLOR_FORCE` overrides it (the BSD precedence).
    /// The caller MUST `crossterm::style::force_color_output(true)`: crossterm
    /// strips color under `$NO_COLOR` and does NOT honor `$CLICOLOR_FORCE` itself
    /// (verified empirically), so without the explicit force the office would
    /// still render colorless despite the user asking for color.
    ForceColor,
    /// `$NO_COLOR` is set (and not force-overridden) → refuse + explain.
    RefuseNoColor,
    /// `$TERM=dumb` → the terminal can't render escapes at all → refuse + explain.
    RefuseDumbTerm,
}

/// Decide the color preflight from the environment (pure; the caller reads env and
/// acts — the "policy in term.rs, IO at the call site" pattern). Precedence,
/// highest first:
///   1. `$TERM=dumb` — a terminal that can't interpret escape sequences at all;
///      nothing we emit renders, and forcing color can't fix it, so refuse first.
///   2. `$NO_COLOR` non-empty — crossterm strips our 24-bit SGR to a bare reset,
///      so the office would be unreadable blocks. Honor the `$CLICOLOR_FORCE`
///      \> `$NO_COLOR` precedence: a non-zero `$CLICOLOR_FORCE` is the user
///      explicitly wanting color, so force it on rather than refuse.
///   3. otherwise proceed.
///
/// `$NO_COLOR` counts as active only when NON-EMPTY — matching crossterm, the
/// thing that actually strips the color (it ignores an empty `$NO_COLOR`). An
/// empty value therefore doesn't break the render and must not block launch.
/// `$CLICOLOR_FORCE` follows the bixense convention (active when set and `!= 0`),
/// the Rust CLI-ecosystem norm — so `$CLICOLOR_FORCE=0` does NOT override.
///
/// `$FORCE_COLOR` (npm) and `$CLICOLOR` are intentionally NOT read: crossterm —
/// the thing that actually strips our color — keys only on `$NO_COLOR`, so they
/// would have no effect on the render. `$CLICOLOR_FORCE` is the lone override
/// because we enact it ourselves (`force_color_output` at the call site).
pub fn color_preflight(
    no_color: Option<&str>,
    clicolor_force: Option<&str>,
    term: Option<&str>,
) -> ColorPreflight {
    if matches!(term, Some(t) if t == "dumb") {
        return ColorPreflight::RefuseDumbTerm;
    }
    let no_color_set = matches!(no_color, Some(v) if !v.is_empty());
    if no_color_set {
        // bixense `!= 0` semantics (not mere presence): `CLICOLOR_FORCE=0` means
        // "do NOT force", so it must not override $NO_COLOR.
        let forced = matches!(clicolor_force.map(str::trim), Some(v) if !v.is_empty() && v != "0");
        return if forced {
            ColorPreflight::ForceColor
        } else {
            ColorPreflight::RefuseNoColor
        };
    }
    ColorPreflight::Proceed
}

/// The `pixtuoid doctor` color-availability line, derived from the SAME
/// `color_preflight` policy the launcher acts on so the diagnostic matches what
/// `run` would do. `None` when color is available (the `terminal:` row already
/// covers depth — no extra line needed); `Some(reason)` when color is disabled or
/// force-overridden, so a "the office is monochrome / won't launch" report is
/// self-diagnosable. Pure (takes the decision) — covered via `doctor::run`.
pub(crate) fn color_status_row(pf: ColorPreflight) -> Option<&'static str> {
    match pf {
        ColorPreflight::Proceed => None,
        ColorPreflight::ForceColor => {
            Some("color: forced on ($CLICOLOR_FORCE overrides $NO_COLOR)")
        }
        ColorPreflight::RefuseNoColor => Some(
            "color: DISABLED — $NO_COLOR is set, so colors are stripped and the \
             office can't render. Unset NO_COLOR, or set CLICOLOR_FORCE=1 to override.",
        ),
        ColorPreflight::RefuseDumbTerm => Some(
            "color: DISABLED — $TERM=dumb; this terminal can't render escape \
             sequences or color.",
        ),
    }
}

/// Parse a `DECRQSS`-for-SGR reply to our truecolor probe. We set the background
/// to `48;2;1;2;3`; the reply form is `DCS 1 $ r <SGR> ST` for a valid request
/// and `DCS 0 $ r ST` when the terminal can't honor it. Returns
/// `Some(true)` when our exact RGB triple came back (truecolor), `Some(false)`
/// for a valid-but-downsampled reply (not truecolor), and `None` for no valid
/// reply (`0$r`, empty, or a timeout) — which the caller treats as "warn", since
/// a terminal that can't answer DECRQSS is unlikely to be truecolor. Pure
/// (operates on the captured bytes) so every branch is unit-tested without a real
/// terminal. `#[cfg(unix)]` like the rest of the DECRQSS machinery it serves —
/// only the unix `query_truecolor` calls it, so it'd be dead code on Windows.
#[cfg(unix)]
fn parse_decrqss_truecolor(resp: &[u8]) -> Option<bool> {
    let s = String::from_utf8_lossy(resp);
    // A valid SGR reply is `DCS 1 $ r ... m ST`; `0 $ r` = request not honored.
    if !s.contains("1$r") {
        return None;
    }
    // Tolerate `:` / `;` separators and an optional empty colorspace-id field
    // (`48:2::1:2:3`) — both are spec-legal ways to echo our triple back.
    let normalized = s.replace(';', ":");
    Some(normalized.contains("48:2:1:2:3") || normalized.contains("48:2::1:2:3"))
}

/// The `pixtuoid doctor` `terminal:` line — `$TERM` / `$COLORTERM` and the
/// truecolor verdict, naming HOW it was determined so a "colors look wrong"
/// report is self-diagnosable. `probe` is the `query_truecolor` result (or `None`
/// when doctor isn't attached to a tty). Pure (takes its inputs) so the row logic
/// is unit-tested; `doctor::run` returns its report string, so it's covered
/// end-to-end too. Untrusted env values are stripped of control chars.
pub(crate) fn terminal_diagnostic_row(
    term: Option<&str>,
    colorterm: Option<&str>,
    probe: Option<bool>,
) -> String {
    let shown = |v: Option<&str>| match v {
        Some(s) if !s.is_empty() => crate::strip_control_chars(s),
        _ => "(unset)".to_string(),
    };
    let verdict = if colorterm_is_truecolor(colorterm) {
        "yes (COLORTERM)"
    } else {
        match probe {
            Some(true) => "yes (terminal query)",
            Some(false) => "no (terminal downsamples)",
            None => "unknown (terminal did not answer)",
        }
    };
    format!(
        "terminal: TERM={} COLORTERM={} truecolor={}",
        shown(term),
        shown(colorterm),
        verdict,
    )
}

/// The probe bytes: set bg to `48;2;1;2;3` — the SEMICOLON 24-bit SGR form the
/// renderer (crossterm) actually emits, so we test what pixtuoid will output, not
/// the stricter colon form some truecolor terminals reject — then `DECRQSS`-query
/// the SGR back (`DCS $ q m ST`) and reset SGR. Echo is off and we write no
/// printable text, so there's no on-screen effect.
#[cfg(unix)]
const DECRQSS_TRUECOLOR_PROBE: &[u8] = b"\x1b[48;2;1;2;3m\x1bP$qm\x1b\\\x1b[0m";

/// Ask the terminal whether it is truecolor by querying it directly (no `$TERM`
/// allowlist). Opens the controlling terminal (`/dev/tty`, so a piped
/// `pixtuoid doctor > file` never receives escape codes), switches it to raw so
/// the reply isn't echoed and arrives un-buffered, writes the probe, reads the
/// reply with a `select` timeout, restores the terminal, and parses. Returns `None`
/// on any I/O failure or no answer — degrading to "warn", unchanged from a
/// terminal we can't confirm. The IO seam (codecov-excluded); the parser and the
/// policy around it are the unit-tested pure pieces.
#[cfg(unix)]
pub fn query_truecolor(timeout: std::time::Duration) -> Option<bool> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();

    // SAFETY: `tcgetattr` only fills the zeroed repr(C) `termios` for a valid fd;
    // all-zero is a valid starting value (overwritten on success).
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    // Restore the saved settings on EVERY exit path (incl. a panic unwinding).
    let _restore = TermiosRestore { fd, saved };

    let mut raw = saved;
    // SAFETY: `cfmakeraw` only mutates the termios struct in place.
    unsafe { libc::cfmakeraw(&mut raw) };
    // SAFETY: applying a well-formed termios to the open tty fd.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }

    tty.write_all(DECRQSS_TRUECOLOR_PROBE).ok()?;
    tty.flush().ok()?;

    parse_decrqss_truecolor(&read_until_terminator(&mut tty, fd, timeout))
}

/// RAII restore of the terminal's saved `termios` — fires on return, `?`, and
/// panic unwinding so a probe can never leave the terminal in raw mode.
#[cfg(unix)]
struct TermiosRestore {
    fd: std::os::fd::RawFd,
    saved: libc::termios,
}

#[cfg(unix)]
impl Drop for TermiosRestore {
    fn drop(&mut self) {
        // SAFETY: re-applying the termios we captured from this same fd.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

/// Per-`read` chunk / initial buffer size — a DECRQSS SGR reply is a few dozen
/// bytes, so one small chunk usually drains it in a single syscall.
#[cfg(unix)]
const DECRQSS_READ_CHUNK: usize = 64;
/// Hard cap on bytes buffered before giving up — a well-behaved terminal replies
/// in well under this; the bound just stops a chatty/garbage stream from looping.
#[cfg(unix)]
const MAX_DECRQSS_RESPONSE_BYTES: usize = 1024;

/// Read the terminal's reply, bounded by `timeout`, until the `DCS` string
/// terminator (`ESC \`) or `BEL` arrives (or the budget elapses / the buffer
/// caps). `poll` returns the instant bytes are ready, so a prompt terminal never
/// waits the full budget.
#[cfg(unix)]
fn read_until_terminator(
    tty: &mut std::fs::File,
    fd: std::os::fd::RawFd,
    timeout: std::time::Duration,
) -> Vec<u8> {
    use std::io::Read;

    // `FD_SET` on an fd >= FD_SETSIZE writes outside the fd_set's bit array (UB).
    // The tty is opened early so this can't fire in practice, but the soundness
    // of the unsafe block below must rest on a structural guard, not a prose
    // claim (a negative fd wraps past FD_SETSIZE via the cast and is caught
    // too). An empty reply reads as "no confirmation" — same as a timeout.
    if fd as usize >= libc::FD_SETSIZE {
        return Vec::new();
    }
    let start = std::time::Instant::now();
    let mut buf = Vec::with_capacity(DECRQSS_READ_CHUNK);
    let mut chunk = [0u8; DECRQSS_READ_CHUNK];
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            break;
        }
        let remaining = timeout - elapsed;
        let mut tv = libc::timeval {
            tv_sec: remaining.as_secs() as libc::time_t,
            tv_usec: remaining.subsec_micros() as libc::suseconds_t,
        };
        // `select`, NOT `poll`: macOS `poll()` is broken on tty/pty devices and
        // returns `POLLNVAL` for a valid terminal fd, which would make every
        // non-`$COLORTERM` terminal read nothing and falsely warn. `select` works
        // on ttys on both macOS and Linux.
        // SAFETY: a zeroed `fd_set` with our single valid fd registered; the fd
        // is < FD_SETSIZE by the structural guard at the top of this fn.
        let mut rfds: libc::fd_set = unsafe { std::mem::zeroed() };
        unsafe { libc::FD_SET(fd, &mut rfds) };
        // SAFETY: one read fd, null write/error sets, a valid timeval.
        let ready = unsafe {
            libc::select(
                fd + 1,
                &mut rfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };
        if ready < 0 {
            // A signal (e.g. SIGWINCH at startup) interrupted the wait — retry
            // within the remaining budget rather than give up to a false warn.
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        // SAFETY: `rfds` was populated by `select`; checking our fd's membership.
        if ready == 0 || !unsafe { libc::FD_ISSET(fd, &rfds) } {
            break; // timeout, or not a read-ready event
        }
        match tty.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if response_terminated(&buf) || buf.len() > MAX_DECRQSS_RESPONSE_BYTES {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    buf
}

/// A `DCS` reply ends with the string terminator `ESC \` (some terminals use
/// `BEL`). Seeing either means the reply is complete — stop reading.
#[cfg(unix)]
fn response_terminated(buf: &[u8]) -> bool {
    buf.windows(2).any(|w| w == [0x1b, b'\\']) || buf.contains(&0x07)
}

/// Non-Unix stub: Windows hard-gates VT separately in `tui::mod`, so there is no
/// preflight query there. Keeps `doctor` / the call site cross-platform.
#[cfg(not(unix))]
pub fn query_truecolor(_timeout: std::time::Duration) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorterm_truecolor_tokens() {
        assert!(colorterm_is_truecolor(Some("truecolor")));
        assert!(colorterm_is_truecolor(Some("24bit")));
        assert!(colorterm_is_truecolor(Some("truecolor:whatever")));
        assert!(!colorterm_is_truecolor(None));
        assert!(!colorterm_is_truecolor(Some("")));
        assert!(!colorterm_is_truecolor(Some("256color")));
        // Case-sensitive: only the conventional lowercase tokens count.
        assert!(!colorterm_is_truecolor(Some("TrueColor")));
    }

    #[test]
    fn suppress_env_truthy_tokens_only() {
        for v in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert!(truecolor_warn_suppressed(Some(v)), "{v:?} should suppress");
        }
        for v in [
            None,
            Some(""),
            Some(" "),
            Some("0"),
            Some("false"),
            Some("no"),
        ] {
            assert!(!truecolor_warn_suppressed(v), "{v:?} must NOT suppress");
        }
    }

    #[test]
    fn warn_zone_truth_table() {
        // In the zone (→ query the terminal) only for a TUI run, on a tty, with no
        // COLORTERM truecolor declaration and no escape hatch.
        assert!(warn_zone(true, true, None, None));
        assert!(warn_zone(true, true, Some("256color"), None));
        // Out of the zone (skip the query, never warn): not a run, not a tty,
        // COLORTERM already truecolor, or the hatch set.
        assert!(!warn_zone(false, true, None, None));
        assert!(!warn_zone(true, false, None, None));
        assert!(!warn_zone(true, true, Some("truecolor"), None));
        assert!(!warn_zone(true, true, None, Some("1")));
    }

    #[test]
    fn color_preflight_precedence_and_thresholds() {
        use ColorPreflight::*;
        // Healthy: no NO_COLOR, no dumb → launch.
        assert_eq!(color_preflight(None, None, Some("xterm-256color")), Proceed);
        // Empty NO_COLOR is ignored (crossterm doesn't strip on it) → still launch.
        assert_eq!(color_preflight(Some(""), None, None), Proceed);
        // NO_COLOR set (any non-empty value, value-agnostic) → refuse.
        assert_eq!(color_preflight(Some("1"), None, None), RefuseNoColor);
        assert_eq!(color_preflight(Some("anything"), None, None), RefuseNoColor);
        // CLICOLOR_FORCE overrides NO_COLOR (BSD precedence) → force, don't refuse.
        assert_eq!(color_preflight(Some("1"), Some("1"), None), ForceColor);
        // A non-zero value other than "1" still forces (active when set && != 0).
        assert_eq!(color_preflight(Some("1"), Some("yes"), None), ForceColor);
        // ...but an EMPTY CLICOLOR_FORCE is not a force → still refuse.
        assert_eq!(color_preflight(Some("1"), Some(""), None), RefuseNoColor);
        // ...and CLICOLOR_FORCE=0 means "do not force" (bixense != 0) → still refuse.
        assert_eq!(color_preflight(Some("1"), Some("0"), None), RefuseNoColor);
        assert_eq!(color_preflight(Some("1"), Some(" 0 "), None), RefuseNoColor);
        // CLICOLOR_FORCE with no NO_COLOR is a no-op here (nothing to override).
        assert_eq!(color_preflight(None, Some("1"), None), Proceed);
        // TERM=dumb outranks everything — even a force can't make dumb render.
        assert_eq!(color_preflight(None, None, Some("dumb")), RefuseDumbTerm);
        assert_eq!(
            color_preflight(Some("1"), Some("1"), Some("dumb")),
            RefuseDumbTerm
        );
    }

    #[test]
    fn color_status_row_only_speaks_when_color_is_not_plainly_available() {
        use ColorPreflight::*;
        assert_eq!(color_status_row(Proceed), None);
        assert!(color_status_row(ForceColor)
            .unwrap()
            .contains("CLICOLOR_FORCE"));
        assert!(color_status_row(RefuseNoColor)
            .unwrap()
            .contains("NO_COLOR"));
        assert!(color_status_row(RefuseDumbTerm).unwrap().contains("dumb"));
    }

    #[cfg(unix)]
    #[test]
    fn parse_decrqss_distinguishes_truecolor_from_downsample() {
        // A truecolor terminal echoes our exact RGB triple (colon form).
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48:2:1:2:3m\x1b\\"),
            Some(true)
        );
        // Semicolon form is normalized to the same triple.
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r0;48;2;1;2;3m\x1b\\"),
            Some(true)
        );
        // Empty colorspace-id field is spec-legal and still truecolor.
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48:2::1:2:3m\x1b\\"),
            Some(true)
        );
        // A valid reply that downsampled to a 256-color index is NOT truecolor.
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48;5;16m\x1b\\"),
            Some(false)
        );
        // A bare attribute reply (no color set) — answered, but not our triple.
        assert_eq!(parse_decrqss_truecolor(b"\x1bP1$r0m\x1b\\"), Some(false));
        // `0$r` = request not honored → ambiguous; empty/timeout → ambiguous.
        assert_eq!(parse_decrqss_truecolor(b"\x1bP0$r\x1b\\"), None);
        assert_eq!(parse_decrqss_truecolor(b""), None);
    }

    // `response_terminated` is `#[cfg(unix)]` (it serves the unix-only read loop),
    // so its test must be gated too — else `check-windows` fails to compile.
    #[cfg(unix)]
    #[test]
    fn response_terminated_on_st_or_bel() {
        assert!(response_terminated(b"\x1bP1$r0m\x1b\\"));
        assert!(response_terminated(b"\x1bP1$r0m\x07"));
        assert!(!response_terminated(b"\x1bP1$r0m"));
    }

    #[test]
    fn terminal_row_names_how_truecolor_was_determined() {
        let by_colorterm = terminal_diagnostic_row(Some("xterm-256color"), Some("truecolor"), None);
        assert!(by_colorterm.contains("TERM=xterm-256color"));
        assert!(by_colorterm.contains("COLORTERM=truecolor"));
        assert!(by_colorterm.contains("truecolor=yes (COLORTERM)"));

        // COLORTERM silent → the verdict reports the terminal-query outcome.
        assert!(terminal_diagnostic_row(Some("xterm"), None, Some(true))
            .contains("truecolor=yes (terminal query)"));
        assert!(terminal_diagnostic_row(Some("xterm"), None, Some(false))
            .contains("truecolor=no (terminal downsamples)"));

        // No tty / no answer → unknown, and unset values read as "(unset)".
        let unknown = terminal_diagnostic_row(None, None, None);
        assert!(unknown.contains("TERM=(unset)"), "{unknown}");
        assert!(unknown.contains("COLORTERM=(unset)"), "{unknown}");
        assert!(
            unknown.contains("truecolor=unknown (terminal did not answer)"),
            "{unknown}"
        );

        // Untrusted env values are control-char-stripped before display.
        let sanitized = terminal_diagnostic_row(Some("a\x1b[31mb"), Some("truecolor"), None);
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }
}
