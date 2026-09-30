use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use pixtuoid_core::sprite::format::{
    DensityMismatch, FrameCountMismatch, HairOverhang, MissingHairView, MissingOptional,
    OrphanDerived, PartialSet, StandIn, UnmarkedHead, ValidationReport, load_pack,
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

fn orphan_derived_line(o: &OrphanDerived) -> String {
    format!(
        "WARN:  ships \"{}\" without \"{}\": the default pack draws \"{}\", in its own style",
        o.derived, o.source, o.source
    )
}

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

fn orphan_variant_line(name: &str) -> String {
    format!(
        "ERROR: \"{}\" is a density variant of an animation this pack does not ship",
        strip_control_chars(name)
    )
}

fn unmarked_head_line(u: &UnmarkedHead) -> String {
    let UnmarkedHead { name, frame } = u;
    format!(
        "WARN:  \"{}\" frame {frame} (from 0) has no head mark: it is drawn without hair",
        strip_control_chars(name)
    )
}

fn missing_hair_view_line(m: &MissingHairView) -> String {
    let MissingHairView { style, view, name } = m;
    format!(
        "WARN:  hairstyle \"{}\" has no \"{}\" layers: a head facing that way (\"{}\") is \
         drawn without hair",
        strip_control_chars(style),
        view.name(),
        strip_control_chars(name)
    )
}

fn overhanging_hair_line(o: &HairOverhang) -> String {
    let HairOverhang {
        style,
        view,
        name,
        frame,
    } = o;
    format!(
        "WARN:  hairstyle \"{}\" \"{}\" hair reaches past the sides of \"{}\" frame {frame} \
         (from 0): the rest is cut off",
        strip_control_chars(style),
        view.name(),
        strip_control_chars(name)
    )
}

fn orphan_hairstyle_line(style: &str) -> String {
    format!(
        "ERROR: hairstyle \"{}\" is at a density this pack draws no character animation at: \
         nobody wears it",
        strip_control_chars(style)
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
        unmarked_heads,
        missing_hair_views,
        overhanging_hair,
        orphan_hairstyles,
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
    for style in orphan_hairstyles {
        let _ = writeln!(err, "{}", orphan_hairstyle_line(style));
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
    for u in unmarked_heads {
        writeln!(out, "{}", unmarked_head_line(u))?;
    }
    for m in missing_hair_views {
        writeln!(out, "{}", missing_hair_view_line(m))?;
    }
    for o in overhanging_hair {
        writeln!(out, "{}", overhanging_hair_line(o))?;
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
    use pixtuoid_core::sprite::HeadView;

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
    fn the_hair_lines_name_the_style_the_view_and_the_frame() {
        assert_eq!(
            unmarked_head_line(&UnmarkedHead {
                name: "standing@4x\u{1b}".to_string(),
                frame: 2,
            }),
            "WARN:  \"standing@4x\" frame 2 (from 0) has no head mark: it is drawn without hair"
        );
        assert_eq!(
            missing_hair_view_line(&MissingHairView {
                style: "mop@4x\u{7}".to_string(),
                view: HeadView::Back,
                name: "walking_back@4x\u{202e}".to_string(),
            }),
            "WARN:  hairstyle \"mop@4x\" has no \"back\" layers: a head facing that way \
             (\"walking_back@4x\") is drawn without hair"
        );
        assert_eq!(
            overhanging_hair_line(&HairOverhang {
                style: "mop@4x".to_string(),
                view: HeadView::Front,
                name: "standing@4x".to_string(),
                frame: 0,
            }),
            "WARN:  hairstyle \"mop@4x\" \"front\" hair reaches past the sides of \
             \"standing@4x\" frame 0 (from 0): the rest is cut off"
        );
        assert_eq!(
            orphan_hairstyle_line("mop@2x\u{1b}"),
            "ERROR: hairstyle \"mop@2x\" is at a density this pack draws no character \
             animation at: nobody wears it"
        );
    }

    #[test]
    fn unknown_line_strips_control_chars_from_untrusted_key() {
        let line = unknown_line("anim\u{1b}]0;pwn\u{7}");
        assert!(!line.contains('\u{1b}') && !line.contains('\u{7}'));
        assert!(line.contains("anim"));
    }
}
