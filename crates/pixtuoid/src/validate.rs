use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use pixtuoid_core::sprite::format::{
    DensityMismatch, FrameCountMismatch, MissingOptional, OrphanDerived, PartialSet, StandIn,
    ValidationReport, load_pack,
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

/// The `WARN:` line for an optional animation the pack leaves out, naming
/// what draws in its place.
fn missing_optional_line(m: &MissingOptional) -> String {
    let stand_in = match m.stand_in {
        StandIn::DefaultPack => "the default pack draws it, in its own style".to_string(),
        StandIn::OwnPiece(piece) => format!("the pack's own \"{piece}\" stands in"),
        StandIn::OwnPose => "another of the pack's poses stands in".to_string(),
    };
    format!(
        "WARN:  missing optional animation \"{}\" ({stand_in})",
        m.name
    )
}

/// The `WARN:` line for an art set the pack ships only part of.
fn partial_set_line(set: &PartialSet) -> String {
    let quoted = |names: &[&str]| {
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "WARN:  ships {} but not {}: the default pack draws the rest, in its own style",
        quoted(&set.shipped),
        quoted(&set.missing)
    )
}

/// The `WARN:` line for a derived piece shipped without its source.
fn orphan_derived_line(o: &OrphanDerived) -> String {
    format!(
        "WARN:  ships \"{}\" without \"{}\": the default pack draws \"{}\", in its own style",
        o.derived, o.source, o.source
    )
}

/// The `ERROR:` line for a density variant with a frame that misses its claim.
/// The name is a key from the pack's own table, so it is stripped as
/// [`unknown_line`]'s is.
fn mismatched_density_line(m: &DensityMismatch) -> String {
    // Destructured without `..`, for the reason `validate_pack` gives.
    let DensityMismatch {
        name,
        frame,
        claimed,
        found,
    } = m;
    format!(
        "ERROR: \"{}\" frame {frame} (from 0) is {}x{}, but its name claims {}x{}",
        strip_control_chars(name),
        found.0,
        found.1,
        claimed.0,
        claimed.1
    )
}

/// The `ERROR:` line for a density variant whose frame count is not its base's.
/// The name is stripped as [`mismatched_density_line`]'s is.
fn frame_count_line(m: &FrameCountMismatch) -> String {
    // Destructured without `..`, for the reason `validate_pack` gives.
    let FrameCountMismatch {
        name,
        base_frames,
        variant_frames,
    } = m;
    format!(
        "ERROR: \"{}\" has {variant_frames} frame(s) but its base has {base_frames}: a \
         density variant redraws every frame of its base",
        strip_control_chars(name)
    )
}

/// The `ERROR:` line for a density variant whose base the pack does not ship.
/// The name is stripped as [`mismatched_density_line`]'s is.
fn orphan_variant_line(name: &str) -> String {
    format!(
        "ERROR: \"{}\" is a density variant of an animation this pack does not ship",
        strip_control_chars(name)
    )
}

pub fn validate_pack(dir: &Path) -> Result<()> {
    let (mut out, mut err) = (cli_stdout(), std::io::stderr());
    let pack = load_pack(dir)?;
    writeln!(out, "{}", ok_line(&pack.name, &pack.version))?;

    let report = pixtuoid_scene::embedded_pack::validate_pack(&pack);

    // Destructured without `..`: a report field added in core does not compile
    // here until this presenter prints it.
    let ValidationReport {
        missing_required,
        missing_optional,
        insufficient_frames,
        unknown,
        mismatched_density,
        orphan_variants,
        mismatched_frame_counts,
        partial_sets,
        orphan_derived,
    } = &report;
    // ERROR diagnostics and the final tally go to stderr so stdout stays the
    // parseable channel even when a caller redirects it.
    for name in missing_required {
        let _ = writeln!(err, "ERROR: missing required animation \"{name}\"");
    }
    // Registry names: the frame floor runs over the registry only.
    for (name, need, got) in insufficient_frames {
        let _ = writeln!(
            err,
            "ERROR: \"{name}\" needs at least {need} frames, has {got}"
        );
    }
    for m in mismatched_density {
        let _ = writeln!(err, "{}", mismatched_density_line(m));
    }
    for m in mismatched_frame_counts {
        let _ = writeln!(err, "{}", frame_count_line(m));
    }
    for name in orphan_variants {
        let _ = writeln!(err, "{}", orphan_variant_line(name));
    }
    for m in missing_optional {
        writeln!(out, "{}", missing_optional_line(m))?;
    }
    for set in partial_sets {
        writeln!(out, "{}", partial_set_line(set))?;
    }
    for o in orphan_derived {
        writeln!(out, "{}", orphan_derived_line(o))?;
    }
    for name in unknown {
        writeln!(out, "{}", unknown_line(name))?;
    }

    let errors = report.error_count();
    let _ = writeln!(
        err,
        "\n{} error(s), {} warning(s)",
        errors,
        report.warning_count()
    );

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
    fn a_missing_optional_line_names_what_draws_in_its_place() {
        let line = |name, stand_in| missing_optional_line(&MissingOptional { name, stand_in });
        assert_eq!(
            line("desk_north", StandIn::OwnPiece("desk")),
            "WARN:  missing optional animation \"desk_north\" (the pack's own \"desk\" stands in)"
        );
        assert!(line("plant", StandIn::DefaultPack).contains("the default pack draws it"));
        assert!(line("walking_coffee", StandIn::OwnPose).contains("another of the pack's poses"));
    }

    #[test]
    fn a_partial_set_line_names_what_ships_and_what_does_not() {
        let line = partial_set_line(&PartialSet {
            shipped: vec!["cat_walk"],
            missing: vec!["cat_sit", "cat_sleep"],
        });
        assert_eq!(
            line,
            "WARN:  ships \"cat_walk\" but not \"cat_sit\", \"cat_sleep\": \
             the default pack draws the rest, in its own style"
        );
    }

    #[test]
    fn a_density_line_names_the_frame_that_misses_the_claim() {
        let line = mismatched_density_line(&DensityMismatch {
            name: "typing@2x\u{1b}[31m".to_string(),
            frame: 1,
            claimed: (2, 2),
            found: (3, 1),
        });
        assert_eq!(
            line,
            "ERROR: \"typing@2x[31m\" frame 1 (from 0) is 3x1, but its name claims 2x2"
        );
    }

    #[test]
    fn a_frame_count_line_names_both_counts() {
        let line = frame_count_line(&FrameCountMismatch {
            name: "seated@2x\u{202e}".to_string(),
            base_frames: 2,
            variant_frames: 1,
        });
        assert_eq!(
            line,
            "ERROR: \"seated@2x\" has 1 frame(s) but its base has 2: a density variant \
             redraws every frame of its base"
        );
    }

    #[test]
    fn an_orphan_variant_line_strips_the_pack_key() {
        assert_eq!(
            orphan_variant_line("desk@4x\u{1b}]0;x\u{7}"),
            "ERROR: \"desk@4x]0;x\" is a density variant of an animation this pack does not ship"
        );
    }

    #[test]
    fn unknown_line_strips_control_chars_from_untrusted_key() {
        let line = unknown_line("anim\u{1b}]0;pwn\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("anim"));
    }
}
