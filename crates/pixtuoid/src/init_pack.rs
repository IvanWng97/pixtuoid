use std::path::Path;

use anyhow::{bail, Result};

pub fn init_pack(dest: &Path, force: bool) -> Result<()> {
    if dest.exists() && !force {
        if !dest.is_dir() {
            bail!("{} exists and is not a directory", dest.display());
        }
        let has_files = std::fs::read_dir(dest)?.next().is_some();
        if has_files {
            bail!(
                "{} already exists and is non-empty (use --force to overwrite)",
                dest.display()
            );
        }
    }
    std::fs::create_dir_all(dest)?;

    let files: &[(&str, &str)] = &[
        ("pack.toml", include_str!("../sprites/skeleton/pack.toml")),
        (
            "placeholder.sprite",
            include_str!("../sprites/skeleton/placeholder.sprite"),
        ),
    ];

    // No per-file exists-skip here: with force=false a dest already containing
    // pack.toml/placeholder.sprite would have tripped the non-empty guard above,
    // and force=true overwrites by design — so the only reachable behavior is a
    // plain write. (A per-file skip would be dead code; see init_pack tests.)
    for (name, content) in files {
        let path = dest.join(name);
        std::fs::write(&path, content)?;
        println!("wrote: {name}");
    }
    println!("\nSkeleton pack extracted to {}", dest.display());
    println!(
        "Edit the sprites, then validate: pixtuoid validate-pack {}",
        dest.display()
    );
    Ok(())
}
