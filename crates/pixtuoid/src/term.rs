//! Terminal capability detection: the truecolor preflight, and the
//! controlling-terminal query (`query_tty`) it shares with the graphics probe.
//! The truecolor warning is a WARN signal, never a gate on Unix — Windows is
//! the exception, `tui::mod` hard-gates VT there because the WinAPI color
//! fallback renders black-on-black.
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
        return if clicolor_forced(clicolor_force) {
            ColorPreflight::ForceColor
        } else {
            ColorPreflight::RefuseNoColor
        };
    }
    ColorPreflight::Proceed
}

/// The bixense `$CLICOLOR_FORCE` convention: set and `!= 0` forces color "no
/// matter what" — including into a pipe. Shared by [`color_preflight`] (the
/// `$NO_COLOR` override) and doctor's paint decision, so the two can't parse
/// the variable differently.
pub(crate) fn clicolor_forced(v: Option<&str>) -> bool {
    matches!(v.map(str::trim), Some(v) if !v.is_empty() && v != "0")
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

/// An env value for display: sanitized, or `(unset)` for absent/empty.
pub(crate) fn shown_env(v: Option<&str>) -> String {
    match v {
        Some(s) if !s.is_empty() => crate::strip_control_chars(s),
        _ => "(unset)".to_string(),
    }
}

/// What asking the terminal whether it keeps 24-bit color learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truecolor {
    /// It answered: `true` when it echoed the 24-bit color back.
    Answered(bool),
    /// It was asked, and no usable reply came.
    NoAnswer,
    /// Nothing asked it: no terminal took the query, or this platform has none
    /// to send.
    CantAsk,
}

impl Truecolor {
    /// Whether the launcher's stderr warning and doctor's ⚠ row fire. Both read
    /// this, so they cannot disagree; `CantAsk` stays quiet because a platform
    /// with nothing to ask would nag on every launch.
    pub fn warrants_warning(self) -> bool {
        matches!(self, Self::NoAnswer | Self::Answered(false))
    }
}

/// The truecolor verdict, naming HOW it was determined so a "colors look wrong"
/// report is self-diagnosable. `probe` is the `query_truecolor` result, `None`
/// when the caller never ran it (piped / `$TERM=dumb`); only an asked probe may
/// claim the terminal went silent.
pub(crate) fn truecolor_verdict(colorterm: Option<&str>, probe: Option<Truecolor>) -> &'static str {
    if colorterm_is_truecolor(colorterm) {
        "yes (COLORTERM)"
    } else {
        match probe {
            Some(Truecolor::Answered(true)) => "yes (terminal query)",
            Some(Truecolor::Answered(false)) => "no (terminal downsamples)",
            Some(Truecolor::NoAnswer) => "unknown (terminal did not answer)",
            Some(Truecolor::CantAsk) => "unknown (the terminal can't be asked)",
            None => "unknown (probe skipped — no color-capable tty)",
        }
    }
}

/// The probe bytes: set bg to `48;2;1;2;3` — the SEMICOLON 24-bit SGR form
/// crossterm actually emits, so we test what pixtuoid will output, not the
/// stricter colon form some truecolor terminals reject — then `DECRQSS`-query the
/// SGR back (`DCS $ q m ST`) and reset SGR.
#[cfg(unix)]
const DECRQSS_TRUECOLOR_PROBE: &[u8] = b"\x1b[48;2;1;2;3m\x1bP$qm\x1b\\\x1b[0m";

/// Ask the terminal whether it is truecolor by querying it directly.
#[cfg(unix)]
pub fn query_truecolor(timeout: std::time::Duration) -> Truecolor {
    let mut reply = Vec::new();
    let asked = query_tty(
        DECRQSS_TRUECOLOR_PROBE,
        timeout,
        MAX_DECRQSS_RESPONSE_BYTES,
        |chunk| {
            reply.extend_from_slice(chunk);
            response_terminated(&reply)
        },
    );
    match asked {
        None => Truecolor::CantAsk,
        Some(_) => parse_decrqss_truecolor(&reply).map_or(Truecolor::NoAnswer, Truecolor::Answered),
    }
}

/// Write `query` to the controlling terminal and hand each chunk of its reply
/// to `on_reply` until that returns `true`. `Some(true)` when it did;
/// `Some(false)` when the reply never completed — `timeout` elapsed, more than
/// `cap` bytes arrived, or the read failed; `None` when the terminal could not
/// be opened, put in raw mode, or written to.
///
/// The controlling terminal (`/dev/tty`), so a piped stdout never receives the
/// escapes; in raw mode, so the reply isn't echoed and arrives un-buffered; and
/// its mode restored on every return. The wait is bounded HERE, on this thread
/// — a query answered by a reader it cannot stop would keep reading the
/// terminal after the budget.
#[cfg(unix)]
pub(crate) fn query_tty(
    query: &[u8],
    timeout: std::time::Duration,
    cap: usize,
    on_reply: impl FnMut(&[u8]) -> bool,
) -> Option<bool> {
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

    tty.write_all(query).ok()?;
    tty.flush().ok()?;

    Some(read_reply(&mut tty, fd, timeout, cap, on_reply))
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

/// Per-`read` chunk: a longer reply arrives over several reads, within the one
/// budget.
#[cfg(unix)]
const TTY_READ_CHUNK: usize = 64;
/// Hard cap on the DECRQSS reply before giving up — the bound just stops a
/// chatty/garbage stream from looping.
#[cfg(unix)]
const MAX_DECRQSS_RESPONSE_BYTES: usize = 1024;

/// Read the terminal's reply into `on_reply`, bounded by `timeout` and `cap`:
/// `true` when `on_reply` reported it complete.
#[cfg(unix)]
fn read_reply(
    tty: &mut std::fs::File,
    fd: std::os::fd::RawFd,
    timeout: std::time::Duration,
    cap: usize,
    mut on_reply: impl FnMut(&[u8]) -> bool,
) -> bool {
    use std::io::Read;

    // `FD_SET` on an fd >= FD_SETSIZE writes outside the fd_set's bit array (UB),
    // so the soundness of the unsafe block below rests on this structural guard
    // rather than a prose claim (a negative fd wraps past FD_SETSIZE via the cast
    // and is caught too). Unread, the reply never completes.
    if fd as usize >= libc::FD_SETSIZE {
        return false;
    }
    let start = std::time::Instant::now();
    let mut read_total = 0usize;
    let mut chunk = [0u8; TTY_READ_CHUNK];
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return false;
        }
        let remaining = timeout - elapsed;
        let mut tv = libc::timeval {
            tv_sec: remaining.as_secs() as libc::time_t,
            tv_usec: remaining.subsec_micros() as libc::suseconds_t,
        };
        // `select`, NOT `poll`: macOS `poll()` is broken on tty/pty devices and
        // returns `POLLNVAL` for a valid terminal fd, so no query would ever read
        // a reply. `select` works on ttys on both macOS and Linux.
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
            // within the remaining budget rather than give up on a reply still
            // coming.
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        // SAFETY: `rfds` was populated by `select`; checking our fd's membership.
        if ready == 0 || !unsafe { libc::FD_ISSET(fd, &rfds) } {
            return false;
        }
        match tty.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => {
                if on_reply(&chunk[..n]) {
                    return true;
                }
                read_total += n;
                if read_total > cap {
                    return false;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
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
pub fn query_truecolor(_timeout: std::time::Duration) -> Truecolor {
    Truecolor::CantAsk
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
        assert!(
            color_status_row(ForceColor)
                .unwrap()
                .contains("CLICOLOR_FORCE")
        );
        assert!(
            color_status_row(RefuseNoColor)
                .unwrap()
                .contains("NO_COLOR")
        );
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
    fn cant_ask_never_warns_the_launcher_or_doctor() {
        for (probe, warns) in [
            (Truecolor::Answered(true), false),
            (Truecolor::Answered(false), true),
            (Truecolor::NoAnswer, true),
            (Truecolor::CantAsk, false),
        ] {
            assert_eq!(probe.warrants_warning(), warns, "{probe:?}");
        }
    }

    #[test]
    fn truecolor_verdict_names_how_it_was_determined() {
        assert_eq!(
            truecolor_verdict(Some("truecolor"), None),
            "yes (COLORTERM)"
        );
        assert_eq!(
            truecolor_verdict(None, Some(Truecolor::Answered(true))),
            "yes (terminal query)"
        );
        assert_eq!(
            truecolor_verdict(None, Some(Truecolor::Answered(false))),
            "no (terminal downsamples)"
        );
        assert_eq!(
            truecolor_verdict(None, Some(Truecolor::NoAnswer)),
            "unknown (terminal did not answer)"
        );
        // Neither a skipped probe nor one nothing could send is an unanswered
        // one: neither may claim the terminal went silent.
        assert_eq!(
            truecolor_verdict(None, None),
            "unknown (probe skipped — no color-capable tty)"
        );
        assert_eq!(
            truecolor_verdict(None, Some(Truecolor::CantAsk)),
            "unknown (the terminal can't be asked)"
        );
    }

    /// Off Unix there is no query to send, so nothing is ever asked.
    #[cfg(not(unix))]
    #[test]
    fn off_unix_the_truecolor_query_cannot_ask() {
        assert_eq!(query_truecolor(TRUECOLOR_PROBE_TIMEOUT), Truecolor::CantAsk);
    }

    #[test]
    fn shown_env_falls_back_to_unset_and_sanitizes() {
        assert_eq!(shown_env(None), "(unset)");
        assert_eq!(shown_env(Some("")), "(unset)");
        let sanitized = shown_env(Some("a\x1b[31mb"));
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }
}
