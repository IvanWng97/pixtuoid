//! Terminal capability probes for the truecolor preflight (the pixel-art office
//! renders 24-bit half-block SGR; a terminal that can't parse those shows
//! approximated/garbled colors with no other hint — the #1 baffling-bug class for
//! a truecolor-only TUI). Detection is intentionally a WARN signal, never a gate
//! on Unix: many genuinely-truecolor terminals omit `COLORTERM`, so a hard gate
//! would false-negative. (Windows is the exception — `tui::mod` hard-gates VT
//! there because the WinAPI color fallback renders black-on-black.)

/// True iff `$COLORTERM` advertises 24-bit color (`truecolor` or `24bit`) — the
/// S-Lang / terminfo convention also used by bat, alacritty, and wezterm. Pure
/// (takes the env value) so the policy is unit-testable without touching the
/// environment. Case-sensitive on purpose: the advertised tokens are lowercase
/// by convention, and a loose match would treat unrelated values as truecolor.
pub fn colorterm_is_truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some(v) if v.contains("truecolor") || v.contains("24bit"))
}

/// True iff the value of the suppression env var (`$PIXTUOID_NO_TRUECOLOR_WARN`)
/// is set to a truthy token (`1`, `true`, `yes`, `on`, case-insensitive). Pure
/// (takes the env value, `None` = unset) so the policy stays unit-testable
/// without touching the environment. Empty / `0` / `false` / anything else =
/// not suppressed, so a leftover `PIXTUOID_NO_TRUECOLOR_WARN=` doesn't silently
/// kill the warning.
fn truecolor_warn_suppressed(suppress_env: Option<&str>) -> bool {
    matches!(
        suppress_env.map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("1")
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    )
}

/// True iff terminfo advertises 24-bit color for `$TERM` — i.e. the entry
/// carries the `Tc` (S-Lang / tmux) or `RGB` (ncurses 6.0+) capability. Many
/// genuinely-truecolor terminals (tmux that strips `$COLORTERM`, some SSH
/// sessions) signal truecolor THIS way rather than via `$COLORTERM`, so this is
/// the real fix for the false-positive nag (#397). Pure over an INJECTED
/// capability lookup (`caps`: given a `$TERM` entry name, return its terminfo
/// boolean-cap names) so the policy is unit-testable without touching the
/// terminfo database. Lookup failure degrades gracefully: a `caps` that returns
/// `None` (no entry / db unreadable) is treated as "not advertised", leaving
/// behavior unchanged from today.
fn terminfo_advertises_truecolor<F>(term: Option<&str>, caps: F) -> bool
where
    F: FnOnce(&str) -> Option<Vec<String>>,
{
    let Some(term) = term.filter(|t| !t.is_empty()) else {
        return false;
    };
    caps(term).is_some_and(|names| {
        names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("Tc") || n.eq_ignore_ascii_case("RGB"))
    })
}

/// The single truecolor-warning POLICY: return `true` iff the pre-altscreen
/// "your terminal does not advertise truecolor" warning should fire. It does
/// NOT fire when `$COLORTERM` already advertises truecolor, when terminfo
/// exposes `Tc`/`RGB` for `$TERM` (#397's real fix), or when the suppression
/// env var is truthy (the documented escape hatch). Every input is passed in
/// as a value / `Option` (and the terminfo lookup is an injected closure), so
/// the policy is unit-testable without touching the process environment — the
/// same pure-and-injected style as `colorterm_is_truecolor`. main.rs reads
/// `$COLORTERM` / `$TERM` / `$PIXTUOID_NO_TRUECOLOR_WARN` at the call site and
/// passes them in.
pub fn should_warn_no_truecolor<F>(
    colorterm: Option<&str>,
    term: Option<&str>,
    suppress_env: Option<&str>,
    terminfo_caps: F,
) -> bool
where
    F: FnOnce(&str) -> Option<Vec<String>>,
{
    !colorterm_is_truecolor(colorterm)
        && !truecolor_warn_suppressed(suppress_env)
        && !terminfo_advertises_truecolor(term, terminfo_caps)
}

/// Probe the system terminfo database for the boolean capability names of a
/// `$TERM` entry, via `infocmp` (ships with ncurses on macOS + Linux). Returns
/// `None` on ANY failure (binary missing, no entry, non-UTF-8, non-zero exit)
/// so the policy degrades to the `$COLORTERM` + suppression-env checks — the
/// graceful-degradation contract `should_warn_no_truecolor`'s `terminfo_caps`
/// closure expects. This is the real-environment seam (`should_warn_no_truecolor`
/// stays pure); main.rs passes it in. The returned names are the entry's
/// boolean caps as `infocmp -1x` prints them (one per line, comma-stripped), so
/// `Tc` / `RGB` are matched verbatim by `terminfo_advertises_truecolor`.
///
/// `-x` is load-bearing: ncurses classifies `Tc` (tmux/S-Lang) and `RGB`
/// (ncurses 6.0+) as USER-DEFINED / extended capabilities, which `infocmp`
/// omits UNLESS `-x` is passed — without it the probe drops the very caps it
/// exists to detect, leaving the false-positive warning intact in exactly the
/// tmux/SSH targets this fixes (#397).
pub fn terminfo_boolean_caps(term: &str) -> Option<Vec<String>> {
    let out = std::process::Command::new("infocmp")
        .arg("-1x")
        .arg(term)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .map(|l| l.trim_end_matches(','))
            // A boolean cap is a bare name (no `=`/`#` value assignment); the
            // header comment line starts with `#`.
            .filter(|l| {
                !l.is_empty() && !l.starts_with('#') && !l.contains('=') && !l.contains('#')
            })
            .map(str::to_string)
            .collect(),
    )
}

/// The `pixtuoid doctor` `terminal:` line — `$TERM` / `$COLORTERM` and the
/// truecolor verdict. Pure (takes the env values as `Option`s, `None` = unset,
/// plus an INJECTED terminfo lookup) so the row logic is unit-testable on its
/// own (and `doctor::run` returns its report string, so it's covered end-to-end
/// too). Untrusted env values are stripped of control chars before display.
///
/// The verdict shares the SAME signals as the startup-warning policy
/// (`should_warn_no_truecolor`): truecolor is advertised when `$COLORTERM` says
/// so OR when terminfo exposes `Tc`/`RGB` for `$TERM` — so `doctor` no longer
/// reports `not advertised` for a TERM-only-truecolor terminal (ghostty,
/// `tmux-direct`) that the startup warning correctly stays silent on (#397).
/// The verdict names which signal matched so a "colors look wrong" report is
/// self-diagnosable. The suppression env var is deliberately NOT consulted here:
/// silencing the nag must not flip the diagnostic to claim a capability the
/// terminal may not have.
pub fn terminal_diagnostic_row<F>(
    term: Option<&str>,
    colorterm: Option<&str>,
    terminfo_caps: F,
) -> String
where
    F: FnOnce(&str) -> Option<Vec<String>>,
{
    let shown = |v: Option<&str>| match v {
        Some(s) if !s.is_empty() => crate::strip_control_chars(s),
        _ => "(unset)".to_string(),
    };
    let verdict = if colorterm_is_truecolor(colorterm) {
        "yes (COLORTERM)"
    } else if terminfo_advertises_truecolor(term, terminfo_caps) {
        "yes (terminfo Tc/RGB)"
    } else {
        "not advertised"
    };
    format!(
        "terminal: TERM={} COLORTERM={} truecolor={}",
        shown(term),
        shown(colorterm),
        verdict,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_tokens_match() {
        assert!(colorterm_is_truecolor(Some("truecolor")));
        assert!(colorterm_is_truecolor(Some("24bit")));
        // A terminal may set a compound value.
        assert!(colorterm_is_truecolor(Some("truecolor:whatever")));
    }

    #[test]
    fn non_truecolor_is_false() {
        assert!(!colorterm_is_truecolor(None));
        assert!(!colorterm_is_truecolor(Some("")));
        assert!(!colorterm_is_truecolor(Some("256color")));
        // Case-sensitive: only the conventional lowercase tokens count.
        assert!(!colorterm_is_truecolor(Some("TrueColor")));
    }

    #[test]
    fn terminal_row_renders_each_state() {
        let yes = terminal_diagnostic_row(Some("xterm-256color"), Some("truecolor"), no_caps);
        assert!(yes.contains("TERM=xterm-256color"));
        assert!(yes.contains("COLORTERM=truecolor"));
        assert!(yes.contains("truecolor=yes (COLORTERM)"));

        // Unset ($COLORTERM = None) and set-but-empty both read as "(unset)" and a
        // "not advertised" verdict (terminfo also not advertising).
        for ct in [None, Some("")] {
            let row = terminal_diagnostic_row(None, ct, no_caps);
            assert!(row.contains("TERM=(unset)"), "{row}");
            assert!(row.contains("COLORTERM=(unset)"), "{row}");
            assert!(row.contains("truecolor=not advertised"), "{row}");
        }

        // $COLORTERM unset but terminfo advertises Tc/RGB -> the verdict names
        // the terminfo signal (the #397 doctor/warning coherence case).
        let tc = |_t: &str| Some(vec!["Tc".to_string()]);
        let terminfo_row = terminal_diagnostic_row(Some("ghostty"), None, tc);
        assert!(
            terminfo_row.contains("truecolor=yes (terminfo Tc/RGB)"),
            "{terminfo_row}"
        );

        // Untrusted env values are control-char-stripped before display.
        let sanitized = terminal_diagnostic_row(Some("a\x1b[31mb"), Some("truecolor"), no_caps);
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }

    // A `terminfo_caps` lookup that never advertises truecolor — the "real db
    // has no Tc/RGB" / graceful-degradation (None) cases use this.
    fn no_caps(_term: &str) -> Option<Vec<String>> {
        Some(vec!["am".to_string(), "bce".to_string(), "ccc".to_string()])
    }
    // The terminfo db is unavailable (binary missing / no entry / unreadable).
    fn caps_unavailable(_term: &str) -> Option<Vec<String>> {
        None
    }

    #[test]
    fn happy_path_colorterm_truecolor_no_warning() {
        // $COLORTERM advertises truecolor -> no warning, unchanged from today,
        // regardless of terminfo / suppression.
        for ct in [Some("truecolor"), Some("24bit"), Some("truecolor:foo")] {
            assert!(!should_warn_no_truecolor(
                ct,
                Some("xterm-256color"),
                None,
                no_caps
            ));
        }
    }

    #[test]
    fn terminfo_tc_or_rgb_suppresses_warning() {
        // $COLORTERM unset but $TERM exposes Tc / RGB -> no warning (#397 fix).
        let tc = |_t: &str| Some(vec!["am".to_string(), "Tc".to_string()]);
        let rgb = |_t: &str| Some(vec!["RGB".to_string()]);
        assert!(!should_warn_no_truecolor(
            None,
            Some("tmux-256color"),
            None,
            tc
        ));
        assert!(!should_warn_no_truecolor(
            None,
            Some("xterm-direct"),
            None,
            rgb
        ));
        // Case-insensitive on the cap name.
        let lc = |_t: &str| Some(vec!["rgb".to_string()]);
        assert!(!should_warn_no_truecolor(None, Some("foo"), None, lc));
    }

    #[test]
    fn suppression_env_var_silences_warning() {
        // Truthy suppression var with no other signal -> no warning.
        for v in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert!(
                !should_warn_no_truecolor(None, Some("xterm-256color"), Some(v), no_caps),
                "{v:?} should suppress"
            );
        }
        // Falsy / empty suppression var -> warning still fires.
        for v in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("x"),
        ] {
            assert!(
                should_warn_no_truecolor(None, Some("xterm-256color"), v, no_caps),
                "{v:?} should NOT suppress"
            );
        }
    }

    #[test]
    fn negative_non_truecolor_term_still_warns() {
        // $COLORTERM unset, $TERM a non-truecolor entry, suppression unset ->
        // warning fires (current behavior preserved).
        assert!(should_warn_no_truecolor(
            None,
            Some("xterm-256color"),
            None,
            no_caps
        ));
    }

    #[test]
    fn terminfo_unavailable_degrades_gracefully() {
        // terminfo db missing/unreadable (caps -> None) falls back to the
        // COLORTERM + suppression checks with no panic.
        assert!(should_warn_no_truecolor(
            None,
            Some("xterm-256color"),
            None,
            caps_unavailable
        ));
        // ...and the COLORTERM / suppression escape hatches still win.
        assert!(!should_warn_no_truecolor(
            Some("truecolor"),
            Some("xterm-256color"),
            None,
            caps_unavailable
        ));
        assert!(!should_warn_no_truecolor(
            None,
            Some("xterm-256color"),
            Some("1"),
            caps_unavailable
        ));
        // An empty/absent $TERM never advertises (and never calls into caps).
        assert!(should_warn_no_truecolor(None, None, None, no_caps));
        assert!(should_warn_no_truecolor(None, Some(""), None, no_caps));
    }

    // Exercises the REAL infocmp seam to pin the `-x` requirement: `Tc`/`RGB`
    // are extended caps that `infocmp` omits without `-x`. Skips when the host
    // has no `infocmp` or no entry carrying an extended truecolor cap (CI
    // runners vary) — a pure presence assertion would be flaky, so we only
    // assert the parse + policy WHEN we can confirm the entry actually
    // advertises one.
    #[test]
    fn real_terminfo_probe_surfaces_extended_truecolor_caps() {
        // ghostty is the most reliable bearer of a bare `Tc`; fall through
        // others a truecolor host might have.
        let advertised = ["ghostty", "xterm-direct", "tmux-direct", "alacritty"]
            .into_iter()
            .find_map(|t| {
                terminfo_boolean_caps(t)
                    .filter(|caps| {
                        caps.iter()
                            .any(|c| c.eq_ignore_ascii_case("Tc") || c.eq_ignore_ascii_case("RGB"))
                    })
                    .map(|_| t)
            });

        let Some(term) = advertised else {
            eprintln!("skipping: no infocmp entry with an extended Tc/RGB cap on this host");
            return;
        };

        // The real probe surfaced the extended cap (proving `-x` is wired), so
        // the policy must NOT warn even with $COLORTERM unset + no suppression.
        assert!(
            !should_warn_no_truecolor(None, Some(term), None, terminfo_boolean_caps),
            "{term} advertises truecolor via terminfo — policy must not warn"
        );
    }
}
