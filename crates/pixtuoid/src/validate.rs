use std::path::Path;

use anyhow::{bail, Result};
use pixtuoid_core::sprite::format::{load_pack, validate_pack_animations};

pub fn validate_pack(dir: &Path) -> Result<()> {
    let pack = load_pack(dir)?;
    println!("OK: pack \"{}\" v{} loaded", pack.name, pack.version);

    let report = validate_pack_animations(&pack);

    // ERROR diagnostics and the final tally go to stderr so stdout stays the
    // parseable channel (the OK line, WARN/INFO advisories) even when a caller
    // redirects stdout — errors also drive a non-zero exit via the bail! below.
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
        println!("INFO:  unknown animation \"{name}\" (unused by renderer)");
    }

    let errors = report.missing_required.len() + report.insufficient_frames.len();
    let warnings = report.missing_optional.len();
    eprintln!("\n{} error(s), {} warning(s)", errors, warnings);

    if report.has_errors() {
        bail!("pack validation failed with {errors} error(s)");
    }
    Ok(())
}
