//! Tell cargo to rebuild whenever the embedded SKELETON template changes.
//!
//! `include_str!` in `src/init_pack.rs` bakes the skeleton pack (the
//! `init-pack` template) into the binary at compile time, but cargo doesn't
//! track those paths as source dependencies on its own — editing a `.sprite`
//! file or `pack.toml` would otherwise leave the binary stale until the .rs
//! changes. (The DEFAULT pack lives in `pixtuoid-scene`, which has its own
//! build.rs; the binary only embeds the skeleton.)
//!
//! This script walks `sprites/skeleton/` and emits one `rerun-if-changed`
//! line per file plus one for the directory itself (catches new files).

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

    // Always rerun if THIS script changes.
    println!("cargo:rerun-if-changed=build.rs");
}
