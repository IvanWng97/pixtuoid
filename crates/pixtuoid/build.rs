//! Tell cargo to rebuild whenever the embedded SKELETON template changes.
//!
//! `include_str!` in `src/init_pack.rs` bakes the skeleton pack into the binary,
//! but cargo doesn't track those paths as source dependencies on its own —
//! editing a `.sprite` file or `pack.toml` would otherwise leave the binary stale
//! until a `.rs` changes.

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let asset_dir = Path::new(&manifest_dir).join("sprites/skeleton");

    println!("cargo:rerun-if-changed={}", asset_dir.display());

    if let Ok(entries) = std::fs::read_dir(&asset_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_asset = path
                .extension()
                .is_some_and(|e| e == "sprite" || e == "toml");
            if is_asset {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
}
