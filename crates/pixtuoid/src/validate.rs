use std::path::Path;

use anyhow::{bail, Result};
use pixtuoid_core::sprite::format::{load_pack, validate_pack_animations};

use crate::strip_control_chars;

/// The `OK:` line. `pack.name`/`pack.version` are untrusted TOML string fields —
/// a crafted pack can encode ESC/OSC bytes (TOML `\u` escapes) that would inject
/// a terminal escape when a user runs `validate-pack` to inspect a downloaded
/// pack, so sanitize at the boundary (the sibling egresses — headless summary,
/// doctor — all route through `strip_control_chars`).
fn ok_line(name: &str, version: &str) -> String {
    format!(
        "OK: pack \"{}\" v{} loaded",
        strip_control_chars(name),
        strip_control_chars(version)
    )
}

/// The `INFO:` line for an unknown animation. The name is a raw pack table key
/// (untrusted) — sanitize it for the same reason as `ok_line`.
fn unknown_line(name: &str) -> String {
    format!(
        "INFO:  unknown animation \"{}\" (unused by renderer)",
        strip_control_chars(name)
    )
}

pub fn validate_pack(dir: &Path) -> Result<()> {
    let pack = load_pack(dir)?;
    println!("{}", ok_line(&pack.name, &pack.version));

    let report = validate_pack_animations(&pack);

    // ERROR diagnostics and the final tally go to stderr so stdout stays the
    // parseable channel (the OK line, WARN/INFO advisories) even when a caller
    // redirects stdout — errors also drive a non-zero exit via the bail! below.
    // The missing/insufficient names come from the REQUIRED/OPTIONAL registry
    // constants (not pack input), so only the OK line + unknown keys are untrusted.
    for name in &report.missing_required {
        eprintln!("ERROR: missing required animation \"{name}\"");
    }
    for (name, need, got) in &report.insufficient_frames {
        eprintln!("ERROR: \"{name}\" needs at least {need} frames, has {got}");
    }
    for name in &report.missing_optional {
        println!("WARN:  missing optional animation \"{name}\" (will not render)");
    }
    for name in &report.unknown {
        println!("{}", unknown_line(name));
    }

    let errors = report.missing_required.len() + report.insufficient_frames.len();
    let warnings = report.missing_optional.len();
    eprintln!("\n{} error(s), {} warning(s)", errors, warnings);

    if report.has_errors() {
        bail!("pack validation failed with {errors} error(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_line_strips_control_chars_from_untrusted_pack_fields() {
        // A crafted pack name/version carrying ESC/BEL must not reach the terminal
        // as a live escape; the printable text survives. (Only the control BYTE is
        // removed — a full `\x1b[31m` SGR would leave the printable `[31m` behind,
        // which is harmless, so the input puts the control char between letters.)
        let line = ok_line("ev\u{1b}il", "1.0\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("evil") && line.contains("1.0"));
    }

    #[test]
    fn unknown_line_strips_control_chars_from_untrusted_key() {
        let line = unknown_line("anim\u{1b}]0;pwn\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("anim"));
    }
}
