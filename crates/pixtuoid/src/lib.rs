//! Public surface for the pixtuoid binary's internals — exposed so
//! examples and integration tests can import them. The `main.rs` binary is
//! the primary entry point.

pub mod aa_text;
pub mod cli;
pub mod config;
pub mod doctor;
pub mod floating;
pub(crate) mod focus;
pub mod init_pack;
pub mod install;
pub mod runtime;
pub mod setup;
pub mod sources;
pub mod term;
pub mod tui;
pub mod validate;
pub mod version;

/// Strip ASCII/Unicode control characters from an untrusted string before it
/// reaches a terminal sink (the headless `println!` summary, the `doctor`
/// stdout report, the Sources-panel path). Untrusted wire values (agent labels,
/// sampled CLI output, config paths) can carry control bytes that would
/// reposition the cursor or inject escapes; one chokepoint so the policy can't
/// drift across its call sites (R0615-06).
pub(crate) fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect()
}

/// The Unicode Bidi_Control characters (LRE/RLE/PDF/LRO/RLO, LRI/RLI/FSI/PDI,
/// LRM/RLM/ALM). `char::is_control` only covers category Cc; these are category
/// Cf and slip through — yet they REORDER displayed text in a terminal (the
/// "Trojan Source" class, CVE-2021-42574), so an untrusted label can render
/// differently from its underlying bytes. Strip them alongside the C0/C1 controls.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'                    // ALM
            | '\u{200E}'..='\u{200F}' // LRM, RLM
            | '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
            | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
    )
}

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`HOME` / `XDG_CONFIG_HOME` / …). The crate's unit tests share one
/// test binary, so two env-mutating tests can otherwise race under plain
/// `cargo test` (nextest isolates per-process, but the `justfile` falls back to
/// `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_c0_and_c1_controls() {
        assert_eq!(strip_control_chars("a\x1b[31mb\x07c"), "a[31mbc");
        assert_eq!(strip_control_chars("x\u{0085}y"), "xy"); // C1 NEL
    }

    #[test]
    fn strips_trojan_source_bidi_controls() {
        // CVE-2021-42574: bidi overrides/isolates reorder DISPLAYED text, so an
        // untrusted label can render differently from its bytes. They are category
        // Cf (not Cc), so `char::is_control` misses them.
        assert_eq!(strip_control_chars("safe\u{202E}gpj.exe"), "safegpj.exe");
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}',
            '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert_eq!(
                strip_control_chars(&format!("a{c}b")),
                "ab",
                "U+{:04X} not stripped",
                c as u32
            );
        }
    }

    #[test]
    fn keeps_ordinary_text_and_non_bidi_unicode() {
        let s = "hello wörld café 日本語 🦞";
        assert_eq!(strip_control_chars(s), s);
    }
}
