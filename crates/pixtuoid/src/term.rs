//! Terminal capability probes for the truecolor preflight (the pixel-art office
//! renders 24-bit half-block SGR; a terminal that can't parse those shows
//! approximated/garbled colors with no other hint — the #1 baffling-bug class for
//! a truecolor-only TUI). Detection is intentionally a WARN signal, never a gate
//! on Unix: many genuinely-truecolor terminals omit `COLORTERM`, so a hard gate
//! would false-negative. (Windows is the exception — `tui::mod` hard-gates VT
//! there because the WinAPI color fallback renders black-on-black.)
//!
//! Truecolor is detected from env only — `$COLORTERM` OR a static
//! `$TERM`/`$TERM_PROGRAM` allowlist, the same signals `supports-color` /
//! `anstyle-query` use (neither reads terminfo). The deep case whose ONLY
//! truecolor signal is a terminfo `Tc`/`RGB` override (some tmux/SSH setups) is
//! deliberately NOT auto-detected: that needs an `infocmp` subprocess or a
//! terminfo dependency that is absent on musl/alpine/Nix and stale on stock
//! macOS, far too much for a warn-only nag. It's covered by the
//! `$PIXTUOID_NO_TRUECOLOR_WARN` escape hatch instead (#397).

/// True iff `$COLORTERM` advertises 24-bit color (`truecolor` or `24bit`) — the
/// S-Lang / terminfo convention also used by bat, alacritty, and wezterm. Pure
/// (takes the env value) so the policy is unit-testable without touching the
/// environment. Case-sensitive on purpose: the advertised tokens are lowercase
/// by convention, and a loose match would treat unrelated values as truecolor.
pub fn colorterm_is_truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some(v) if v.contains("truecolor") || v.contains("24bit"))
}

/// Terminals that are genuinely truecolor but don't reliably set `$COLORTERM`
/// (notably over SSH, which forwards `$TERM` via the pty-req but not
/// `$COLORTERM`), identified by `$TERM` / `$TERM_PROGRAM`. This mirrors the
/// static allowlist `supports-color` / `anstyle-query` use — neither reads
/// terminfo. Deliberately does NOT match `*-256color`: that's Apple Terminal.app
/// (256-color until macOS 26 / Tahoe), a genuinely-non-truecolor terminal that
/// MUST still get the warning. Pure (takes the env values) for unit-testing.
fn term_is_truecolor(term: Option<&str>, term_program: Option<&str>) -> bool {
    // `$TERM` names are lowercase by convention → case-sensitive substring match.
    let term_advertises = term.is_some_and(|t| {
        t.ends_with("-direct")
            || t.ends_with("-truecolor")
            || [
                "kitty",
                "ghostty",
                "alacritty",
                "wezterm",
                "foot",
                "contour",
                "rio",
            ]
            .iter()
            .any(|name| t.contains(name))
    });
    // `$TERM_PROGRAM` is a proper name (mixed case: `iTerm.app`, `WezTerm`) →
    // exact, case-insensitive. `Apple_Terminal` is intentionally absent.
    let program_advertises = term_program.is_some_and(|p| {
        [
            "iTerm.app",
            "WezTerm",
            "ghostty",
            "vscode",
            "Hyper",
            "rio",
            "Tabby",
        ]
        .iter()
        .any(|name| p.eq_ignore_ascii_case(name))
    });
    term_advertises || program_advertises
}

/// The single "is this terminal advertising truecolor?" signal, shared by the
/// warn policy and the `doctor` verdict so the two can NEVER disagree: `true`
/// when `$COLORTERM` advertises it OR a known-truecolor `$TERM`/`$TERM_PROGRAM`.
fn truecolor_advertised(
    colorterm: Option<&str>,
    term: Option<&str>,
    term_program: Option<&str>,
) -> bool {
    colorterm_is_truecolor(colorterm) || term_is_truecolor(term, term_program)
}

/// True iff `$PIXTUOID_NO_TRUECOLOR_WARN` is set to a truthy token (`1` / `true`
/// / `yes` / `on`, case-insensitive, trimmed) — the documented escape hatch for
/// a user whose terminal is fine but isn't auto-detected (e.g. a tmux/SSH setup
/// whose only truecolor signal is a terminfo override). Empty / `0` / `false` /
/// anything else = not suppressed, so a leftover `PIXTUOID_NO_TRUECOLOR_WARN=`
/// doesn't silently kill the warning. Pure (takes the env value) for testing.
fn truecolor_warn_suppressed(suppress_env: Option<&str>) -> bool {
    matches!(
        suppress_env.map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("1")
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    )
}

/// Whether to emit the truecolor preflight warning: a TUI `run` (not headless),
/// attached to a tty, whose terminal does NOT advertise truecolor (`$COLORTERM`
/// or the `$TERM`/`$TERM_PROGRAM` allowlist) and hasn't set the
/// `$PIXTUOID_NO_TRUECOLOR_WARN` escape hatch. Pure so the gate LOGIC is
/// unit-tested over its truth table; `main.rs` keeps the `#[cfg(not(windows))]`,
/// the `IsTerminal` probe, and the env reads inline at its (codecov-excluded)
/// call site — the policy lives here, the untestable env/tty/cfg reads stay there
/// (the "policy in term.rs" pattern).
pub fn should_warn_truecolor(
    cmd_is_run_tui: bool,
    is_tty: bool,
    colorterm: Option<&str>,
    term: Option<&str>,
    term_program: Option<&str>,
    suppress_env: Option<&str>,
) -> bool {
    cmd_is_run_tui
        && is_tty
        && !truecolor_advertised(colorterm, term, term_program)
        && !truecolor_warn_suppressed(suppress_env)
}

/// The `pixtuoid doctor` `terminal:` line — `$TERM` / `$COLORTERM` /
/// `$TERM_PROGRAM` and the truecolor verdict, naming WHICH signal advertised it
/// so a "colors look wrong" report is self-diagnosable. Pure (takes the env
/// values as `Option`s, `None` = unset) so the row logic is unit-testable on its
/// own (and `doctor::run` returns its report string, so it's covered end-to-end
/// too). Shares `truecolor_advertised`'s inputs with the warn policy, so the
/// diagnostic and the startup warning can never disagree. Untrusted env values
/// are stripped of control chars before display.
pub fn terminal_diagnostic_row(
    term: Option<&str>,
    colorterm: Option<&str>,
    term_program: Option<&str>,
) -> String {
    let shown = |v: Option<&str>| match v {
        Some(s) if !s.is_empty() => crate::strip_control_chars(s),
        _ => "(unset)".to_string(),
    };
    let verdict = if colorterm_is_truecolor(colorterm) {
        "yes (COLORTERM)"
    } else if term_is_truecolor(term, term_program) {
        "yes (TERM/TERM_PROGRAM)"
    } else {
        "not advertised"
    };
    format!(
        "terminal: TERM={} COLORTERM={} TERM_PROGRAM={} truecolor={}",
        shown(term),
        shown(colorterm),
        shown(term_program),
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
    fn term_allowlist_matches_modern_terminals_but_not_256color() {
        // Direct-color entries + the modern-terminal $TERM family advertise.
        for t in [
            "xterm-direct",
            "tmux-truecolor",
            "xterm-kitty",
            "xterm-ghostty",
            "alacritty",
            "wezterm",
            "foot-extra",
            "contour",
            "rio",
        ] {
            assert!(term_is_truecolor(Some(t), None), "{t} should advertise");
        }
        // $TERM_PROGRAM (proper-name, case-insensitive); Apple_Terminal must NOT.
        for p in [
            "iTerm.app",
            "WezTerm",
            "ghostty",
            "vscode",
            "Hyper",
            "tabby",
        ] {
            assert!(
                term_is_truecolor(None, Some(p)),
                "{p} program should advertise"
            );
        }
        // The load-bearing exclusion: Apple Terminal.app is genuinely 256-color
        // (until macOS 26) and MUST still warn — neither its $TERM nor its
        // $TERM_PROGRAM may match the allowlist.
        assert!(!term_is_truecolor(
            Some("xterm-256color"),
            Some("Apple_Terminal")
        ));
        // tmux/screen default $TERM carries no truecolor signal by name (the deep
        // terminfo-Tc case is the documented residual, covered by the hatch).
        assert!(!term_is_truecolor(Some("tmux-256color"), None));
        assert!(!term_is_truecolor(Some("screen-256color"), None));
        assert!(!term_is_truecolor(None, None));
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
    fn should_warn_truecolor_truth_table() {
        // Warn ONLY for a TUI run, on a tty, with no truecolor signal + no hatch.
        assert!(should_warn_truecolor(true, true, None, None, None, None));
        assert!(should_warn_truecolor(
            true,
            true,
            Some("256color"),
            Some("xterm-256color"),
            Some("Apple_Terminal"),
            None
        ));
        // Suppressed by ANY of: not a TUI run, not a tty, $COLORTERM truecolor, a
        // known-truecolor $TERM / $TERM_PROGRAM, or the escape hatch.
        assert!(!should_warn_truecolor(false, true, None, None, None, None));
        assert!(!should_warn_truecolor(true, false, None, None, None, None));
        assert!(!should_warn_truecolor(
            true,
            true,
            Some("truecolor"),
            None,
            None,
            None
        ));
        assert!(!should_warn_truecolor(
            true,
            true,
            None,
            Some("xterm-kitty"),
            None,
            None
        ));
        assert!(!should_warn_truecolor(
            true,
            true,
            None,
            None,
            Some("iTerm.app"),
            None
        ));
        assert!(!should_warn_truecolor(
            true,
            true,
            None,
            None,
            None,
            Some("1")
        ));
        // The SSH false-positive #397 targets: $COLORTERM stripped but $TERM
        // survives the pty-req → no warning.
        assert!(!should_warn_truecolor(
            true,
            true,
            None,
            Some("xterm-ghostty"),
            None,
            None
        ));
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
        let yes = terminal_diagnostic_row(Some("xterm-256color"), Some("truecolor"), None);
        assert!(yes.contains("TERM=xterm-256color"));
        assert!(yes.contains("COLORTERM=truecolor"));
        assert!(yes.contains("truecolor=yes (COLORTERM)"));

        // $COLORTERM unset but a known-truecolor $TERM → the verdict names the
        // TERM signal (the doctor/warning coherence case).
        let by_term = terminal_diagnostic_row(Some("xterm-kitty"), None, None);
        assert!(by_term.contains("TERM=xterm-kitty"), "{by_term}");
        assert!(
            by_term.contains("truecolor=yes (TERM/TERM_PROGRAM)"),
            "{by_term}"
        );

        // Unset ($COLORTERM = None) and set-but-empty both read as "(unset)" and a
        // "not advertised" verdict (no $TERM signal either).
        for ct in [None, Some("")] {
            let row = terminal_diagnostic_row(None, ct, None);
            assert!(row.contains("TERM=(unset)"), "{row}");
            assert!(row.contains("COLORTERM=(unset)"), "{row}");
            assert!(row.contains("TERM_PROGRAM=(unset)"), "{row}");
            assert!(row.contains("truecolor=not advertised"), "{row}");
        }

        // Untrusted env values are control-char-stripped before display.
        let sanitized =
            terminal_diagnostic_row(Some("a\x1b[31mb"), Some("truecolor"), Some("x\x1by"));
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }
}
