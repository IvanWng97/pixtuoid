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

    // A per-file exists-skip would be dead code: with force=false a dest already
    // holding these files trips the non-empty guard above, and force=true
    // overwrites by design.
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
