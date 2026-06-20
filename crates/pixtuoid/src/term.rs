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

/// The live `$COLORTERM`-based verdict for the running process.
pub fn truecolor_supported() -> bool {
    colorterm_is_truecolor(std::env::var("COLORTERM").ok().as_deref())
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
}
