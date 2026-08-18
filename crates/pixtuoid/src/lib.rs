//! Public surface for the pixtuoid binary's internals — exposed so examples and
//! integration tests can import them.

pub mod aa_text;
pub(crate) mod audio;
pub mod cli;
pub mod config;
pub mod doctor;

#[cfg(test)]
mod drift_surface;
pub mod floating;
pub(crate) mod focus;
pub(crate) mod graphics;
/// The one graphics item that IS public API: `Cmd::Doctor` carries it, and
/// `main.rs` is a separate crate. Everything else in the module — the plan, the
/// probe, the scale rule — is `pub(crate)`, because a `pub` item on a published
/// crate is the one thing a follow-up cannot quietly undo.
pub use graphics::GraphicsMode;
pub mod init_pack;
pub mod install;
pub mod runtime;
pub mod setup;
pub mod sources;
pub mod term;
pub mod tui;
pub mod validate;
pub(crate) mod version;

/// Strip ASCII/Unicode control characters from an untrusted string before it
/// reaches a terminal sink (the headless summary, the `doctor` report, the
/// Sources-panel path, the `connect`/`disconnect` rows, the pre-altscreen config
/// warnings). Untrusted wire values can carry control bytes that would
/// reposition the cursor or inject escapes; one chokepoint so the policy can't
/// drift across its call sites.
///
/// The non-TUI `tracing` stream cannot be filtered at the SINK: the subscriber
/// emits its own SGR for level coloring, so a sink-side filter could not tell
/// our escapes from an injected one. Its untrusted values are stripped where
/// they ENTER a record instead — here by this fn's callers, and in the lib by
/// `pixtuoid_core::source::decoder::display_safe`, a per-crate copy of this
/// predicate pinned to it by `the_bidi_table_matches_pixtuoid_cores_display_safe`.
pub(crate) fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect()
}

/// The Unicode Bidi_Control characters. `char::is_control` covers only category
/// Cc; these are Cf and slip through — yet they REORDER displayed text in a
/// terminal (the "Trojan Source" class, CVE-2021-42574).
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
/// variables. Unit tests share one binary, so two env-mutating tests race under
/// plain `cargo test` (nextest isolates per-process; the `justfile` falls back).
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only `tracing` capture for asserting on what reaches the log sink.
#[cfg(test)]
pub(crate) mod test_capture {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    pub(crate) struct Buf(Arc<Mutex<Vec<u8>>>);
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

    /// Capture through the SAME subscriber shape `main.rs` installs, so an
    /// assertion is validated against the REAL line format, not an assumed one.
    pub(crate) fn capture(f: impl FnOnce()) -> String {
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
}

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

    /// Round-trip one probe name through `pixtuoid-core`'s copy of the predicate,
    /// via `decode_hook_payload`'s unsupported-event `bail!`.
    fn core_display_safe(name: &str) -> String {
        const PREFIX: &str = "unsupported hook_event_name: ";
        let v = serde_json::json!({"hook_event_name": name, "session_id": "s"});
        let e = pixtuoid_core::source::decoder::decode_hook_payload(v)
            .expect_err("an unregistered hook_event_name must bail");
        let msg = e.to_string();
        msg.strip_prefix(PREFIX)
            .unwrap_or_else(|| panic!("core's bail wording moved; re-point this pin: {msg:?}"))
            .to_string()
    }

    #[test]
    fn the_bidi_table_matches_pixtuoid_cores_display_safe() {
        // Two per-crate copies of one security table (core's is `pub(crate)`), so
        // pin them BEHAVIOURALLY: sweep every codepoint either side could
        // plausibly gain or lose — the Cc block and its boundaries, DEL/C1, and
        // the Cf neighbourhoods on both sides of each bidi range.
        let candidates = (0x00..=0x20u32)
            .chain(0x7E..=0xA1)
            .chain(0x061A..=0x061E)
            .chain(0x200B..=0x2010)
            .chain(0x2028..=0x2030)
            .chain(0x2065..=0x206F);
        for cp in candidates {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let probe = format!("PixtuoidParity{c}Probe");
            assert_eq!(
                strip_control_chars(&probe),
                core_display_safe(&probe),
                "U+{cp:04X}: the binary's strip_control_chars and pixtuoid-core's \
                 display_safe disagree — the two Cf/Cc tables have drifted apart",
            );
        }
    }
}
