//! Terminal capability detection for the truecolor preflight. The warning is a
//! WARN signal, never a gate on Unix — Windows is the exception, `tui::mod`
//! hard-gates VT there because the WinAPI color fallback renders black-on-black.
//!
//! We do NOT guess truecolor from a `$TERM` name allowlist. Detection ASKS the
//! terminal directly: set an unlikely 24-bit background, then `DECRQSS`-query the
//! SGR back — a truecolor terminal echoes the RGB triple, a 256-color one
//! downsamples it, and one that can't even parse the query stays silent.
//! `$COLORTERM=truecolor` (the terminal declaring itself) is honored purely to
//! skip the round-trip, and `$PIXTUOID_NO_TRUECOLOR_WARN` is an explicit user
//! override; neither is a heuristic.

/// Default round-trip budget for the `DECRQSS` probe: long enough for a laggy SSH
/// link to answer, short enough that a terminal which never answers only costs
/// this once at startup.
pub const TRUECOLOR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// True iff `$COLORTERM` advertises 24-bit color (`truecolor` or `24bit`).
/// Case-sensitive on purpose: the advertised tokens are lowercase by convention.
fn colorterm_is_truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some(v) if v.contains("truecolor") || v.contains("24bit"))
}

/// True iff `$PIXTUOID_NO_TRUECOLOR_WARN` is set to a truthy token (`1` / `true`
/// / `yes` / `on`, case-insensitive, trimmed). Everything else — including an
/// empty value — is NOT suppressed, so a leftover `PIXTUOID_NO_TRUECOLOR_WARN=`
/// doesn't silently kill the warning.
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
/// The final decision is: warn unless the query returns `Some(true)`.
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
/// disables color we refuse to launch the canvas and explain why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPreflight {
    Proceed,
    /// The caller MUST `crossterm::style::force_color_output(true)`: crossterm
    /// strips color under `$NO_COLOR` and does NOT honor `$CLICOLOR_FORCE` itself,
    /// so without the explicit force the office renders colorless anyway.
    ForceColor,
    RefuseNoColor,
    RefuseDumbTerm,
}

/// Decide the color preflight from the environment. `$TERM=dumb` outranks
/// everything (forcing color can't fix a terminal that renders no escapes), then
/// a NON-EMPTY `$NO_COLOR` refuses unless `$CLICOLOR_FORCE` overrides it.
///
/// The emptiness rule matches crossterm, the thing that actually strips the
/// color; `$CLICOLOR_FORCE` follows the bixense convention (set and `!= 0`), so
/// `$CLICOLOR_FORCE=0` does NOT override. `$FORCE_COLOR` (npm) and `$CLICOLOR`
/// are intentionally NOT read: crossterm keys only on `$NO_COLOR`, so they would
/// have no effect on the render.
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
/// `run` would do. `None` when color is plainly available.
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

/// Parse a `DECRQSS`-for-SGR reply to our truecolor probe (background set to
/// `48;2;1;2;3`). `Some(true)` when our exact RGB triple came back,
/// `Some(false)` for a valid-but-downsampled reply, and `None` for no valid reply
/// (`0$r`, empty, or a timeout) — which the caller treats as "warn".
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
/// when doctor isn't attached to a tty).
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

/// The probe bytes: set bg to `48;2;1;2;3` — the SEMICOLON 24-bit SGR form
/// crossterm actually emits, so we test what pixtuoid will output, not the
/// stricter colon form some truecolor terminals reject — then `DECRQSS`-query the
/// SGR back (`DCS $ q m ST`) and reset SGR.
#[cfg(unix)]
const DECRQSS_TRUECOLOR_PROBE: &[u8] = b"\x1b[48;2;1;2;3m\x1bP$qm\x1b\\\x1b[0m";

/// Ask the terminal whether it is truecolor by querying it directly. Uses the
/// controlling terminal (`/dev/tty`, so a piped `pixtuoid doctor > file` never
/// receives escape codes) in raw mode, so the reply isn't echoed and arrives
/// un-buffered. Returns `None` on any I/O failure or no answer.
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
/// Hard cap on bytes buffered before giving up — the bound just stops a
/// chatty/garbage stream from looping.
#[cfg(unix)]
const MAX_DECRQSS_RESPONSE_BYTES: usize = 1024;

/// Read the terminal's reply, bounded by `timeout`, until the `DCS` string
/// terminator (`ESC \`) or `BEL` arrives (or the budget elapses / the buffer
/// caps).
#[cfg(unix)]
fn read_until_terminator(
    tty: &mut std::fs::File,
    fd: std::os::fd::RawFd,
    timeout: std::time::Duration,
) -> Vec<u8> {
    use std::io::Read;

    // `FD_SET` on an fd >= FD_SETSIZE writes outside the fd_set's bit array (UB),
    // so the soundness of the unsafe block below rests on this structural guard
    // rather than a prose claim (a negative fd wraps past FD_SETSIZE via the cast
    // and is caught too). An empty reply reads as "no confirmation".
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
            break;
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
/// `BEL`); either means the reply is complete.
#[cfg(unix)]
fn response_terminated(buf: &[u8]) -> bool {
    buf.windows(2).any(|w| w == [0x1b, b'\\']) || buf.contains(&0x07)
}

/// Non-Unix stub: Windows hard-gates VT separately in `tui::mod`, so there is no
/// preflight query there.
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
        assert!(warn_zone(true, true, None, None));
        assert!(warn_zone(true, true, Some("256color"), None));
        assert!(!warn_zone(false, true, None, None));
        assert!(!warn_zone(true, false, None, None));
        assert!(!warn_zone(true, true, Some("truecolor"), None));
        assert!(!warn_zone(true, true, None, Some("1")));
    }

    #[test]
    fn color_preflight_precedence_and_thresholds() {
        use ColorPreflight::*;
        assert_eq!(color_preflight(None, None, Some("xterm-256color")), Proceed);
        assert_eq!(color_preflight(Some(""), None, None), Proceed);
        assert_eq!(color_preflight(Some("1"), None, None), RefuseNoColor);
        assert_eq!(color_preflight(Some("anything"), None, None), RefuseNoColor);
        assert_eq!(color_preflight(Some("1"), Some("1"), None), ForceColor);
        assert_eq!(color_preflight(Some("1"), Some("yes"), None), ForceColor);
        assert_eq!(color_preflight(Some("1"), Some(""), None), RefuseNoColor);
        assert_eq!(color_preflight(Some("1"), Some("0"), None), RefuseNoColor);
        assert_eq!(color_preflight(Some("1"), Some(" 0 "), None), RefuseNoColor);
        assert_eq!(color_preflight(None, Some("1"), None), Proceed);
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
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48:2:1:2:3m\x1b\\"),
            Some(true)
        );
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r0;48;2;1;2;3m\x1b\\"),
            Some(true)
        );
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48:2::1:2:3m\x1b\\"),
            Some(true)
        );
        assert_eq!(
            parse_decrqss_truecolor(b"\x1bP1$r48;5;16m\x1b\\"),
            Some(false)
        );
        assert_eq!(parse_decrqss_truecolor(b"\x1bP1$r0m\x1b\\"), Some(false));
        assert_eq!(parse_decrqss_truecolor(b"\x1bP0$r\x1b\\"), None);
        assert_eq!(parse_decrqss_truecolor(b""), None);
    }

    // `response_terminated` is `#[cfg(unix)]`, so its test must be gated too —
    // else `check-windows` fails to compile.
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

        assert!(terminal_diagnostic_row(Some("xterm"), None, Some(true))
            .contains("truecolor=yes (terminal query)"));
        assert!(terminal_diagnostic_row(Some("xterm"), None, Some(false))
            .contains("truecolor=no (terminal downsamples)"));

        let unknown = terminal_diagnostic_row(None, None, None);
        assert!(unknown.contains("TERM=(unset)"), "{unknown}");
        assert!(unknown.contains("COLORTERM=(unset)"), "{unknown}");
        assert!(
            unknown.contains("truecolor=unknown (terminal did not answer)"),
            "{unknown}"
        );

        let sanitized = terminal_diagnostic_row(Some("a\x1b[31mb"), Some("truecolor"), None);
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }
}
