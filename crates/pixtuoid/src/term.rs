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

/// The `pixtuoid doctor` `terminal:` line — `$TERM` / `$COLORTERM` and the
/// truecolor verdict. Pure (takes the env values as `Option`s, `None` = unset) so
/// the row logic is unit-testable on its own (and `doctor::run` returns its report
/// string, so it's covered end-to-end too). Untrusted env values are stripped of
/// control chars before display.
pub fn terminal_diagnostic_row(term: Option<&str>, colorterm: Option<&str>) -> String {
    let shown = |v: Option<&str>| match v {
        Some(s) if !s.is_empty() => crate::strip_control_chars(s),
        _ => "(unset)".to_string(),
    };
    format!(
        "terminal: TERM={} COLORTERM={} truecolor={}",
        shown(term),
        shown(colorterm),
        if colorterm_is_truecolor(colorterm) {
            "yes"
        } else {
            "not advertised"
        },
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
        let yes = terminal_diagnostic_row(Some("xterm-256color"), Some("truecolor"));
        assert!(yes.contains("TERM=xterm-256color"));
        assert!(yes.contains("COLORTERM=truecolor"));
        assert!(yes.contains("truecolor=yes"));

        // Unset ($COLORTERM = None) and set-but-empty both read as "(unset)" and a
        // "not advertised" verdict.
        for ct in [None, Some("")] {
            let row = terminal_diagnostic_row(None, ct);
            assert!(row.contains("TERM=(unset)"), "{row}");
            assert!(row.contains("COLORTERM=(unset)"), "{row}");
            assert!(row.contains("truecolor=not advertised"), "{row}");
        }

        // Untrusted env values are control-char-stripped before display.
        let sanitized = terminal_diagnostic_row(Some("a\x1b[31mb"), Some("truecolor"));
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
    }
}
