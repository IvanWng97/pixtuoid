//! Sprite pack loader.
//!
//! Tries the user-config path first (XDG-style) so power users can drop in a
//! custom pack without recompiling. Falls back to the embedded default pack
//! (compile-time `include_str!`) so the binary ships standalone.
//!
//! ## Custom pack layout
//!
//! Drop a directory at `${XDG_CONFIG_HOME:-~/.config}/pixtuoid/sprites/`
//! containing `pack.toml` + each `.sprite` file referenced from the TOML.
//! See `crates/pixtuoid/sprites/default/` for the canonical example.
//!
//! ## Sharp edge — palette RGB uniqueness
//!
//! The renderer's per-agent recolor (`recolor_frame` in `tui::renderer`)
//! substitutes the H/S/B palette colors by RGB equality. If a custom pack
//! reuses the same RGB for two palette keys, the recolor pass will substitute
//! both, producing visual artifacts. Each palette key MUST map to a unique
//! RGB triple.

use std::path::PathBuf;

use anyhow::Result;
use pixtuoid_core::sprite::format::{load_pack, load_pack_from_strings, Pack};

/// Resolve the user's sprite-pack directory if XDG settings point at one.
/// Returns the directory only when `pack.toml` exists inside it — otherwise
/// the caller falls back to the embedded pack.
fn xdg_pack_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let dir = base.join("pixtuoid").join("sprites");
    if dir.join("pack.toml").is_file() {
        Some(dir)
    } else {
        None
    }
}

pub fn load_sprite_pack(pack_dir: Option<PathBuf>) -> Result<Pack> {
    let base = load_embedded_pack()?;

    if let Some(dir) = pack_dir {
        let mut custom = load_pack(&dir).map_err(|e| {
            anyhow::anyhow!("failed to load sprite pack from {}: {e}", dir.display())
        })?;
        tracing::info!(path = %dir.display(), "loaded sprite pack from --pack-dir");
        custom.merge_from(&base);
        return Ok(custom);
    }
    if let Some(dir) = xdg_pack_dir() {
        match load_pack(&dir) {
            Ok(mut p) => {
                tracing::info!(path = %dir.display(), "loaded user sprite pack");
                p.merge_from(&base);
                return Ok(p);
            }
            Err(e) => {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "user sprite pack failed to load; falling back to embedded default"
                );
            }
        }
    }
    Ok(base)
}

fn load_embedded_pack() -> Result<Pack> {
    let pack_toml = include_str!("../../sprites/default/pack.toml");
    let seated = include_str!("../../sprites/default/seated.sprite");
    let typing_0 = include_str!("../../sprites/default/typing_0.sprite");
    let typing_1 = include_str!("../../sprites/default/typing_1.sprite");
    let standing = include_str!("../../sprites/default/standing.sprite");
    let walking_0 = include_str!("../../sprites/default/walking_0.sprite");
    let walking_1 = include_str!("../../sprites/default/walking_1.sprite");
    let walking_back_0 = include_str!("../../sprites/default/walking_back_0.sprite");
    let walking_back_1 = include_str!("../../sprites/default/walking_back_1.sprite");
    let walking_coffee_0 = include_str!("../../sprites/default/walking_coffee_0.sprite");
    let walking_coffee_1 = include_str!("../../sprites/default/walking_coffee_1.sprite");
    let desk = include_str!("../../sprites/default/desk.sprite");
    let plant = include_str!("../../sprites/default/plant.sprite");
    let plant_tall = include_str!("../../sprites/default/plant_tall.sprite");
    let plant_fl = include_str!("../../sprites/default/plant_flower.sprite");
    let plant_suc = include_str!("../../sprites/default/plant_succulent.sprite");
    let floor_lamp = include_str!("../../sprites/default/floor_lamp.sprite");
    let trash_bin = include_str!("../../sprites/default/trash_bin.sprite");
    let door = include_str!("../../sprites/default/door.sprite");
    let door_half = include_str!("../../sprites/default/door_half.sprite");
    let door_open = include_str!("../../sprites/default/door_open.sprite");
    let bulletin = include_str!("../../sprites/default/bulletin_board.sprite");
    let exit_sign = include_str!("../../sprites/default/exit_sign.sprite");
    let filing = include_str!("../../sprites/default/filing_cabinet.sprite");
    let cat_0 = include_str!("../../sprites/default/cat_walk_0.sprite");
    let cat_1 = include_str!("../../sprites/default/cat_walk_1.sprite");
    let cat_sit = include_str!("../../sprites/default/cat_sit.sprite");
    let cat_sleep = include_str!("../../sprites/default/cat_sleep.sprite");
    let dog_0 = include_str!("../../sprites/default/dog_walk_0.sprite");
    let dog_1 = include_str!("../../sprites/default/dog_walk_1.sprite");
    let dog_sit = include_str!("../../sprites/default/dog_sit.sprite");
    let dog_sleep = include_str!("../../sprites/default/dog_sleep.sprite");
    let meeting_sofa = include_str!("../../sprites/default/meeting_sofa.sprite");
    let meeting_screen = include_str!("../../sprites/default/meeting_screen.sprite");
    let back_couch = include_str!("../../sprites/default/back_couch.sprite");
    let sleeping_seat = include_str!("../../sprites/default/seated_sleeping.sprite");
    let sleeping_alt = include_str!("../../sprites/default/seated_sleeping_alt.sprite");
    let holding = include_str!("../../sprites/default/holding_coffee.sprite");
    let pantry = include_str!("../../sprites/default/pantry.sprite");
    let pantry_small = include_str!("../../sprites/default/pantry_small.sprite");
    let whiteboard = include_str!("../../sprites/default/whiteboard.sprite");
    let bookshelf = include_str!("../../sprites/default/bookshelf.sprite");
    let tv_stand = include_str!("../../sprites/default/tv_stand.sprite");
    let phone_booth = include_str!("../../sprites/default/phone_booth.sprite");
    let standing_desk = include_str!("../../sprites/default/standing_desk.sprite");

    load_pack_from_strings(
        pack_toml,
        &[
            ("seated.sprite", seated),
            ("typing_0.sprite", typing_0),
            ("typing_1.sprite", typing_1),
            ("standing.sprite", standing),
            ("walking_0.sprite", walking_0),
            ("walking_1.sprite", walking_1),
            ("walking_back_0.sprite", walking_back_0),
            ("walking_back_1.sprite", walking_back_1),
            ("walking_coffee_0.sprite", walking_coffee_0),
            ("walking_coffee_1.sprite", walking_coffee_1),
            ("desk.sprite", desk),
            ("plant.sprite", plant),
            ("plant_tall.sprite", plant_tall),
            ("plant_flower.sprite", plant_fl),
            ("plant_succulent.sprite", plant_suc),
            ("floor_lamp.sprite", floor_lamp),
            ("trash_bin.sprite", trash_bin),
            ("door.sprite", door),
            ("door_half.sprite", door_half),
            ("door_open.sprite", door_open),
            ("bulletin_board.sprite", bulletin),
            ("exit_sign.sprite", exit_sign),
            ("filing_cabinet.sprite", filing),
            ("cat_walk_0.sprite", cat_0),
            ("cat_walk_1.sprite", cat_1),
            ("cat_sit.sprite", cat_sit),
            ("cat_sleep.sprite", cat_sleep),
            ("dog_walk_0.sprite", dog_0),
            ("dog_walk_1.sprite", dog_1),
            ("dog_sit.sprite", dog_sit),
            ("dog_sleep.sprite", dog_sleep),
            ("meeting_sofa.sprite", meeting_sofa),
            ("meeting_screen.sprite", meeting_screen),
            ("back_couch.sprite", back_couch),
            ("seated_sleeping.sprite", sleeping_seat),
            ("seated_sleeping_alt.sprite", sleeping_alt),
            ("holding_coffee.sprite", holding),
            ("pantry.sprite", pantry),
            ("pantry_small.sprite", pantry_small),
            ("whiteboard.sprite", whiteboard),
            ("bookshelf.sprite", bookshelf),
            ("tv_stand.sprite", tv_stand),
            ("phone_booth.sprite", phone_booth),
            ("standing_desk.sprite", standing_desk),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // The recolor invariant applies to EVERY palette key, not just the 4
    // recolor targets: recolor_frame matches by RGB equality, so two keys
    // sharing a color are indistinguishable (a recolor — or any future
    // per-key logic — swaps both). Transparent (None) keys are exempt. Caught
    // the e/q = #1a1a1a dup that the B/H/S/P-only check below missed.
    #[test]
    fn embedded_pack_all_palette_keys_are_distinct_rgbs() {
        let pack = load_sprite_pack(None).expect("embedded pack loads");
        let entries: Vec<(char, pixtuoid_core::sprite::Rgb)> = pack
            .palette
            .iter()
            .filter_map(|(k, p)| p.map(|rgb| (k, rgb)))
            .collect();
        for i in 0..entries.len() {
            for j in (i + 1)..entries.len() {
                assert_ne!(
                    entries[i].1, entries[j].1,
                    "palette keys {:?} and {:?} share an RGB — recolor_frame can't distinguish them",
                    entries[i].0, entries[j].0
                );
            }
        }
    }

    // recolor_frame (pixel_painter/palette.rs) substitutes agent colors by RGB
    // equality against the base pack's B/H/S/P entries. If any two share an RGB,
    // the recolor pass swaps both and produces artifacts. No validate-pack check
    // enforces it, so this guards the documented uniqueness invariant for the
    // shipped embedded pack.
    #[test]
    fn embedded_pack_recolor_keys_are_distinct_rgbs() {
        let pack = load_sprite_pack(None).expect("embedded pack loads");
        let keys = ['B', 'H', 'S', 'P'];
        let rgbs: Vec<_> = keys
            .iter()
            .map(|&k| {
                pack.palette
                    .get(k)
                    .flatten()
                    .unwrap_or_else(|| panic!("embedded pack missing recolor key {k:?}"))
            })
            .collect();
        for i in 0..rgbs.len() {
            for j in (i + 1)..rgbs.len() {
                assert_ne!(
                    rgbs[i], rgbs[j],
                    "recolor keys {:?} and {:?} share an RGB — recolor_frame would swap both",
                    keys[i], keys[j]
                );
            }
        }
    }
}
