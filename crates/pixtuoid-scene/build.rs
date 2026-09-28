//! Embed the default pack: the sprite list is generated from `sprites/default/`
//! itself, so a sprite committed there is embedded by construction, less,
//! without the `density-art` feature, what [`density_art::strip_density_art`]
//! drops from both the manifest and the list.

#[path = "build_support/density_art.rs"]
mod density_art;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set");
    let out_dir = Path::new(&out_dir);
    let asset_dir = Path::new(&manifest_dir).join("sprites/default");

    // The one rerun trigger: cargo rescans a directory for any change, so an
    // added or removed sprite regenerates the list. An edited sprite rebuilds
    // the crate without it, since rustc tracks every `include_str!` input.
    println!("cargo:rerun-if-changed={}", asset_dir.display());

    let pack_toml = std::fs::read_to_string(asset_dir.join("pack.toml")).expect("read pack.toml");
    let (pack_toml, dropped) = if std::env::var_os("CARGO_FEATURE_DENSITY_ART").is_some() {
        (pack_toml, BTreeSet::new())
    } else {
        density_art::strip_density_art(&pack_toml)
    };
    std::fs::write(out_dir.join("embedded_pack.toml"), pack_toml)
        .expect("write embedded_pack.toml");

    let mut sprites: Vec<_> = std::fs::read_dir(&asset_dir)
        .expect("read sprites/default")
        .map(|entry| entry.expect("read a sprites/default entry").path())
        // A dotfile is never a sprite, whatever its extension: macOS writes
        // `._<name>` AppleDouble files beside real ones on non-HFS volumes.
        .filter(|path| {
            path.extension().is_some_and(|e| e == "sprite")
                && !path
                    .file_name()
                    .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
        })
        .collect();
    // Sorted, so the generated list — and the binary — do not depend on the
    // order the filesystem happens to list the directory in.
    sprites.sort();

    let mut list = String::from("&[\n");
    for path in &sprites {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a sprite file name is UTF-8");
        if dropped.contains(name) {
            continue;
        }
        // `{:?}` escapes the path for a Rust string literal, backslashes
        // included.
        writeln!(
            list,
            "    ({name:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("writing to a String");
    }
    list.push_str("]\n");
    std::fs::write(out_dir.join("embedded_sprites.rs"), list).expect("write embedded_sprites.rs");
}
