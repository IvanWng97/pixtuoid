use std::io::Write;
use std::path::Path;

use anyhow::{bail, Result};
use pixtuoid_core::sprite::format::{
    load_pack, validate_pack_animations, PartialSet, ValidationReport,
    OPTIONAL_FURNITURE_ANIMATIONS,
};

use crate::{cli_stdout, strip_control_chars};

/// The `OK:` line. **homebrew-core contract**: their `test do` asserts this output
/// matches `OK: pack "skeleton"` after `init-pack`, so the literal prefix + quoting
/// is a public packaging surface — rewording it breaks Homebrew's CI on the next
/// autobump, not ours. Coordinate a core PR.
///
/// `pack.name`/`pack.version` are untrusted TOML string fields: a crafted pack can
/// encode ESC/OSC bytes that would inject a terminal escape when a user runs
/// `validate-pack` on a downloaded pack, so sanitize at the boundary.
fn ok_line(name: &str, version: &str) -> String {
    format!(
        "OK: pack \"{}\" v{} loaded",
        strip_control_chars(name),
        strip_control_chars(version)
    )
}

/// The `INFO:` line for an unknown animation — the name is a raw pack table key,
/// so sanitize it for the same reason as [`ok_line`].
fn unknown_line(name: &str) -> String {
    format!(
        "INFO:  unknown animation \"{}\" (unused by renderer)",
        strip_control_chars(name)
    )
}

/// The `WARN:` line for an optional animation the pack leaves out. A missing
/// piece of furniture is inherited from the default pack (`Pack::merge_from`);
/// a missing character pose is not, since a robot pack must not fall back to
/// human sprites, so one of the pack's own poses stands in.
fn missing_optional_line(name: &str) -> String {
    let consequence = if OPTIONAL_FURNITURE_ANIMATIONS.contains(&name) {
        "the default pack's art draws it"
    } else {
        "another of the pack's poses stands in"
    };
    format!("WARN:  missing optional animation \"{name}\" ({consequence})")
}

/// The `WARN:` line for an art set the pack ships only part of. The names come
/// from the registry, not the pack.
fn partial_set_line(set: &PartialSet) -> String {
    let quoted = |names: &[String]| {
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "WARN:  ships {} but not {}: the default pack's art draws those beside it",
        quoted(&set.shipped),
        quoted(&set.missing)
    )
}

/// The `WARN:` line for a derived piece shipped without its source.
fn orphan_derived_line(derived: &str, source: &str) -> String {
    format!(
        "WARN:  ships \"{derived}\" without \"{source}\": the default pack's \"{source}\" draws beside it"
    )
}

pub fn validate_pack(dir: &Path) -> Result<()> {
    let (mut out, mut err) = (cli_stdout(), std::io::stderr());
    let pack = load_pack(dir)?;
    writeln!(out, "{}", ok_line(&pack.name, &pack.version))?;

    let report = validate_pack_animations(&pack);

    // Destructured without `..`: a report field added in core does not compile
    // here until this presenter prints it.
    let ValidationReport {
        missing_required,
        missing_optional,
        insufficient_frames,
        unknown,
        mismatched_density,
        orphan_variants,
        partial_sets,
        orphan_derived,
    } = &report;
    // ERROR diagnostics and the final tally go to stderr so stdout stays the
    // parseable channel even when a caller redirects it. `missing_*` names come
    // from the registry; every other name can be a density variant found in the
    // pack's own table, so it is pack input and gets the same sanitising as the
    // unknown keys.
    for name in missing_required {
        let _ = writeln!(err, "ERROR: missing required animation \"{name}\"");
    }
    for (name, need, got) in insufficient_frames {
        let _ = writeln!(
            err,
            "ERROR: \"{}\" needs at least {need} frames, has {got}",
            strip_control_chars(name)
        );
    }
    for m in mismatched_density {
        let _ = writeln!(
            err,
            "ERROR: \"{}\" is {}x{}, but its name claims {}x{}",
            strip_control_chars(&m.name),
            m.found.0,
            m.found.1,
            m.claimed.0,
            m.claimed.1
        );
    }
    for name in orphan_variants {
        let _ = writeln!(
            err,
            "ERROR: \"{}\" is a density variant of a piece this pack does not ship",
            strip_control_chars(name)
        );
    }
    for name in missing_optional {
        writeln!(out, "{}", missing_optional_line(name))?;
    }
    for set in partial_sets {
        writeln!(out, "{}", partial_set_line(set))?;
    }
    for (derived, source) in orphan_derived {
        writeln!(out, "{}", orphan_derived_line(derived, source))?;
    }
    for name in unknown {
        writeln!(out, "{}", unknown_line(name))?;
    }

    let errors = report.error_count();
    let warnings = missing_optional.len() + partial_sets.len() + orphan_derived.len();
    let _ = writeln!(err, "\n{} error(s), {} warning(s)", errors, warnings);

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
        // Only the control BYTE is removed — a full `\x1b[31m` SGR would leave the
        // printable `[31m` behind — so the input puts the control char between
        // letters.
        let line = ok_line("ev\u{1b}il", "1.0\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("evil") && line.contains("1.0"));
    }

    #[test]
    fn a_missing_piece_of_furniture_says_the_default_draws_it() {
        assert!(missing_optional_line("plant").contains("the default pack's art draws it"));
        assert!(missing_optional_line("walking_coffee").contains("another of the pack's poses"));
    }

    #[test]
    fn a_partial_set_line_names_what_ships_and_what_does_not() {
        let line = partial_set_line(&PartialSet {
            shipped: vec!["cat_walk".to_string()],
            missing: vec!["cat_sit".to_string(), "cat_sleep".to_string()],
        });
        assert_eq!(
            line,
            "WARN:  ships \"cat_walk\" but not \"cat_sit\", \"cat_sleep\": \
             the default pack's art draws those beside it"
        );
    }

    #[test]
    fn unknown_line_strips_control_chars_from_untrusted_key() {
        let line = unknown_line("anim\u{1b}]0;pwn\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("anim"));
    }
}
