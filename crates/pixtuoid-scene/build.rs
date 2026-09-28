//! Embed the default pack's sprites, and rebuild whenever one changes.
//!
//! The embedded sprite list is generated from `sprites/default/` itself, so a
//! sprite committed there is embedded by construction; `include_str!` alone does
//! not make cargo track those paths, so each is also declared a rerun trigger.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set");
    let asset_dir = Path::new(&manifest_dir).join("sprites/default");

    println!("cargo:rerun-if-changed={}", asset_dir.display());

    let mut sprites: Vec<_> = std::fs::read_dir(&asset_dir)
        .expect("read sprites/default")
        .map(|entry| entry.expect("read a sprites/default entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e == "sprite" || e == "toml")
        })
        .collect();
    // Sorted, so the generated list — and the binary — do not depend on the
    // order the filesystem happens to list the directory in.
    sprites.sort();

    let mut list = String::from("&[\n");
    for path in &sprites {
        println!("cargo:rerun-if-changed={}", path.display());
        if path.extension().is_some_and(|e| e == "sprite") {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a sprite file name is UTF-8");
            // `{:?}` escapes the path for a Rust string literal, backslashes
            // included.
            writeln!(
                list,
                "    ({name:?}, include_str!({:?})),",
                path.display().to_string()
            )
            .expect("writing to a String");
        }
    }
    list.push_str("]\n");
    std::fs::write(Path::new(&out_dir).join("embedded_sprites.rs"), list)
        .expect("write embedded_sprites.rs");

    println!("cargo:rerun-if-changed=build.rs");
}
