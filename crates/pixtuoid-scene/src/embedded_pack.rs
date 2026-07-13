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
//! See `crates/pixtuoid-scene/sprites/default/` for the canonical example.
//!
//! ## Sharp edge — palette RGB uniqueness
//!
//! The per-agent recolor (`recolor_frame` in `pixel_painter::palette`)
//! substitutes the H/S/B palette colors by RGB equality. If a custom pack
//! reuses the same RGB for two palette keys, the recolor pass will substitute
//! both, producing visual artifacts. Each palette key MUST map to a unique
//! RGB triple.

use std::path::PathBuf;

use anyhow::Result;
use pixtuoid_core::sprite::format::{
    load_pack, load_pack_from_strings, validate_pack_animations, Pack, ValidationReport,
};

/// Resolve the user's sprite-pack directory if XDG settings point at one.
/// Returns the directory only when `pack.toml` exists inside it — otherwise
/// the caller falls back to the embedded pack.
fn xdg_pack_dir() -> Option<PathBuf> {
    let base = xdg_config_base(
        std::env::var_os("XDG_CONFIG_HOME"),
        pixtuoid_core::platform::user_home_opt().map(PathBuf::from),
    )?;
    let dir = base.join("pixtuoid").join("sprites");
    if dir.join("pack.toml").is_file() {
        Some(dir)
    } else {
        None
    }
}

/// Resolve the XDG config base: the env value when set to a NON-EMPTY path, else
/// `<home>/.config`. Per the XDG basedir spec, an EMPTY `XDG_CONFIG_HOME` counts
/// as unset — without the filter a `Some("")` skips the fallback and yields a
/// CWD-RELATIVE `pixtuoid/sprites` path, silently loading an untrusted pack from
/// the launch directory while ignoring the user's real `~/.config`. Pure (the env
/// value is passed in) so the empty-vs-set precedence is unit-testable without
/// mutating process env.
fn xdg_config_base(xdg: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".config")))
}

/// Log a custom pack's animation-validation gaps at load time. A pack missing
/// a required character pose — or carrying an empty `frames = []` entry —
/// LOADS fine and then renders those poses as NOTHING (`paint_character_at`
/// early-returns on an absent/empty animation), so without this the only
/// signal is agents silently vanishing whenever they sleep / sit on a couch.
/// `pixtuoid validate-pack` reports the same facts, but nothing forces a pack
/// author to run it. Warn, don't fail: a partially-authored pack still
/// renders every pose it does carry. Runs AFTER `merge_from` so furniture
/// inherited from the embedded default isn't misreported as missing.
fn warn_pack_validation_gaps(pack: &Pack, origin: &str) -> ValidationReport {
    let report = validate_pack_animations(pack);
    for name in &report.missing_required {
        tracing::warn!(
            origin,
            animation = %name,
            "custom sprite pack is missing a REQUIRED character animation — \
             agents will be invisible in that pose (run `pixtuoid validate-pack`)"
        );
    }
    for (name, min, got) in &report.insufficient_frames {
        tracing::warn!(
            origin,
            animation = %name,
            min,
            got,
            "custom sprite pack animation has too few frames — it will render as nothing"
        );
    }
    report
}

pub fn load_sprite_pack(pack_dir: Option<PathBuf>) -> Result<Pack> {
    let base = load_embedded_pack()?;

    if let Some(dir) = pack_dir {
        let mut custom = load_pack(&dir).map_err(|e| {
            anyhow::anyhow!("failed to load sprite pack from {}: {e}", dir.display())
        })?;
        tracing::info!(path = %dir.display(), "loaded sprite pack from --pack-dir");
        custom.merge_from(&base);
        warn_pack_validation_gaps(&custom, "--pack-dir");
        return Ok(custom);
    }
    if let Some(dir) = xdg_pack_dir() {
        match load_pack(&dir) {
            Ok(mut p) => {
                tracing::info!(path = %dir.display(), "loaded user sprite pack");
                p.merge_from(&base);
                warn_pack_validation_gaps(&p, "xdg");
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

/// Test-only default-pack loader: takes the crate's `TEST_ENV_LOCK` around the
/// `XDG_CONFIG_HOME` read inside [`load_sprite_pack`], so an env-READING pack
/// load can't race the env-MUTATING test
/// (`load_sprite_pack_resolves_then_falls_back_via_xdg`) under plain
/// `cargo test` — one test binary, many threads (nextest's per-process
/// isolation masks the race). Every unit test resolving the default pack must
/// come through here, never a bare `load_sprite_pack(None)`.
#[cfg(test)]
pub(crate) fn test_default_pack() -> Pack {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    load_sprite_pack(None).expect("default pack loads")
}

fn load_embedded_pack() -> Result<Pack> {
    // Embed each default sprite ONCE by filename. The macro expands every entry
    // to `("<name>", include_str!(concat!("../sprites/default/", "<name>")))`, so
    // a new sprite is a SINGLE line here — not a `let`-binding AND a matching
    // tuple entry that can silently drift out of sync. Byte-identical to the
    // hand-listed form (`concat!` folds to the same path literal at compile
    // time); `build.rs` still emits the per-file `rerun-if-changed`.
    macro_rules! embedded_sprites {
        ($($name:literal),+ $(,)?) => {
            &[$(($name, include_str!(concat!("../sprites/default/", $name)))),+]
        };
    }

    load_pack_from_strings(
        include_str!("../sprites/default/pack.toml"),
        embedded_sprites![
            "seated.sprite",
            "typing_0.sprite",
            "typing_1.sprite",
            "standing.sprite",
            "walking_0.sprite",
            "walking_1.sprite",
            "walking_back_0.sprite",
            "walking_back_1.sprite",
            "walking_coffee_0.sprite",
            "walking_coffee_1.sprite",
            "desk.sprite",
            "plant.sprite",
            "plant_tall.sprite",
            "plant_flower.sprite",
            "plant_succulent.sprite",
            "floor_lamp.sprite",
            "door.sprite",
            "door_half.sprite",
            "door_open.sprite",
            "bulletin_board.sprite",
            "exit_sign.sprite",
            "filing_cabinet.sprite",
            "cat_walk_0.sprite",
            "cat_walk_1.sprite",
            "cat_sit.sprite",
            "cat_sleep.sprite",
            "dog_walk_0.sprite",
            "dog_walk_1.sprite",
            "dog_sit.sprite",
            "dog_sleep.sprite",
            "lobster_walk_0.sprite",
            "lobster_walk_1.sprite",
            "lobster_rest.sprite",
            "meeting_sofa.sprite",
            "meeting_screen.sprite",
            "back_couch.sprite",
            "seated_sleeping.sprite",
            "seated_sleeping_alt.sprite",
            "holding_coffee.sprite",
            "pantry.sprite",
            "pantry_small.sprite",
            "whiteboard.sprite",
            "bookshelf.sprite",
            "snack_shelf.sprite",
            "tv_stand.sprite",
            "phone_booth.sprite",
            "standing_desk.sprite",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn xdg_config_base_treats_empty_as_unset() {
        // XDG basedir spec: an EMPTY XDG_CONFIG_HOME counts as unset. Without the
        // filter, `Some("")` skips the fallback and the pack dir resolves relative
        // to CWD (loading an untrusted pack from the launch directory).
        assert_eq!(
            xdg_config_base(
                Some(std::ffi::OsString::from("")),
                Some(PathBuf::from("/home/u"))
            ),
            Some(PathBuf::from("/home/u/.config")),
        );
    }

    #[test]
    fn xdg_config_base_prefers_a_set_value_over_home() {
        assert_eq!(
            xdg_config_base(
                Some(std::ffi::OsString::from("/xdg")),
                Some(PathBuf::from("/home/u")),
            ),
            Some(PathBuf::from("/xdg")),
        );
    }

    #[test]
    fn xdg_config_base_falls_back_to_home_when_absent() {
        assert_eq!(
            xdg_config_base(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config")),
        );
    }

    #[test]
    fn xdg_config_base_is_none_without_xdg_or_home() {
        assert_eq!(
            xdg_config_base(Some(std::ffi::OsString::from("")), None),
            None
        );
        assert_eq!(xdg_config_base(None, None), None);
    }

    /// Copy this crate's char-only pack fixture (a valid, loadable character
    /// pack with NO furniture — so the merge-from-embedded-default assertion
    /// isn't tautological) into `dst`. The fixture lives INSIDE pixtuoid-scene
    /// (`tests/fixtures/charpack/`) and ships in the published tarball, so the
    /// test stays self-contained — `cargo test` passes from an extracted .crate
    /// (it must NOT reach into the sibling `pixtuoid` binary crate's skeleton).
    fn copy_skeleton_pack(dst: &Path) {
        fs::create_dir_all(dst).expect("mkdir pack dir");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/charpack");
        for entry in fs::read_dir(&src).expect("read skeleton dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().expect("file name");
                fs::copy(&path, dst.join(name)).expect("copy pack file");
            }
        }
    }

    #[test]
    fn load_sprite_pack_from_custom_dir_merges_with_embedded() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pack_dir = tmp.path().join("custom");
        copy_skeleton_pack(&pack_dir);

        let pack = load_sprite_pack(Some(pack_dir)).expect("custom pack loads");
        // The custom pack supplies character poses; furniture is merged from the
        // embedded default, so both must be present.
        assert!(
            pack.animation("seated").is_some(),
            "custom pack must carry the seated character pose"
        );
        assert!(
            pack.animation("desk").is_some(),
            "furniture merged from the embedded default"
        );
    }

    /// Counts WARN-level tracing events emitted inside `with_default`.
    #[derive(Clone)]
    struct WarnCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.level() == &tracing::Level::WARN
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, _: &tracing::Event<'_>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn custom_pack_missing_required_pose_loads_with_a_load_time_warning() {
        // A --pack-dir pack missing a required character pose must (a) still
        // LOAD — warn, not fail — and (b) be LOUD about the gap at load time:
        // the pose renders as nothing (paint_character_at early-returns), so
        // without the warning the only signal is agents silently vanishing.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pack_dir = tmp.path().join("gappy");
        copy_skeleton_pack(&pack_dir);
        // Strip the back_couch animation (the fixture's last section).
        let toml_path = pack_dir.join("pack.toml");
        let toml = fs::read_to_string(&toml_path).expect("read pack.toml");
        let stripped = toml
            .split("[animations.back_couch]")
            .next()
            .expect("split never yields zero pieces")
            .to_string();
        assert_ne!(stripped, toml, "fixture must carry back_couch to strip");
        fs::write(&toml_path, stripped).expect("write stripped pack.toml");

        let warns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pack = tracing::subscriber::with_default(WarnCounter(warns.clone()), || {
            load_sprite_pack(Some(pack_dir))
        })
        .expect("a pack missing a required pose must still LOAD (warn, not fail)");
        assert!(
            pack.animation("back_couch").is_none(),
            "the stripped pose is really absent (never inherited: character \
             animations don't merge from the embedded default)"
        );
        assert!(
            warns.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "load_sprite_pack must warn about the missing required pose at load time"
        );
        // The gap report names exactly the stripped pose.
        assert_eq!(
            warn_pack_validation_gaps(&pack, "test").missing_required,
            vec!["back_couch".to_string()]
        );
    }

    #[test]
    fn load_sprite_pack_from_missing_custom_dir_errors() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert!(
            load_sprite_pack(Some(missing)).is_err(),
            "a nonexistent --pack-dir must surface a load error"
        );
    }

    // The XDG path mutates a process-global env var. The TEST_ENV_LOCK
    // serializes this mutator against the crate's env-READING pack loads —
    // every `test_default_pack()` caller (floor / pixel_painter / the
    // embedded-pack tests below resolve the default pack through the same
    // XDG_CONFIG_HOME read) — so a reader can't observe the temp dirs set
    // here under plain `cargo test` (nextest's per-process isolation masks
    // the race). This test calls `load_sprite_pack` DIRECTLY, not the locked
    // helper: it already holds the (non-reentrant) lock.
    #[test]
    fn load_sprite_pack_resolves_then_falls_back_via_xdg() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("XDG_CONFIG_HOME");

        // (a) Valid XDG pack at $XDG/pixtuoid/sprites/ → loaded.
        let good = tempfile::TempDir::new().expect("tempdir");
        let good_sprites = good.path().join("pixtuoid").join("sprites");
        copy_skeleton_pack(&good_sprites);
        std::env::set_var("XDG_CONFIG_HOME", good.path());
        let pack = load_sprite_pack(None).expect("xdg pack loads");
        assert!(
            pack.animation("seated").is_some(),
            "the valid XDG pack must be loaded (xdg Ok arm)"
        );

        // (b) Malformed pack.toml at the XDG path → warn + fall back to embedded.
        let bad = tempfile::TempDir::new().expect("tempdir");
        let bad_sprites = bad.path().join("pixtuoid").join("sprites");
        fs::create_dir_all(&bad_sprites).expect("mkdir bad sprites");
        fs::write(bad_sprites.join("pack.toml"), b"this is not valid toml {{{")
            .expect("write malformed pack.toml");
        std::env::set_var("XDG_CONFIG_HOME", bad.path());
        // The malformed pack triggers the Err arm → falls back to embedded (Ok),
        // which still carries the embedded character poses.
        let fallback = load_sprite_pack(None).expect("malformed pack falls back, never errors");
        assert!(
            fallback.animation("seated").is_some(),
            "fallback to the embedded default after a malformed user pack"
        );

        // Restore env for the rest of the suite.
        match saved {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    // The recolor invariant applies to EVERY palette key, not just the 4
    // recolor targets: recolor_frame matches by RGB equality, so two keys
    // sharing a color are indistinguishable (a recolor — or any future
    // per-key logic — swaps both). Transparent (None) keys are exempt. Caught
    // the e/q = #1a1a1a dup that the B/H/S/P-only check below missed.
    #[test]
    fn embedded_pack_all_palette_keys_are_distinct_rgbs() {
        let pack = test_default_pack();
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
        let pack = test_default_pack();
        // The single source of truth — same set recolor_frame + the load guard use.
        let keys = pixtuoid_core::sprite::format::RECOLOR_KEYS;
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

    // `layout::CHARACTER_SPRITE_W` is the width every out-of-pixel_painter site
    // (hit-test pin box, decor walk-offset, floating label centering) hard-codes
    // its geometry on, as the width-unknown fallback for the pack's real
    // `frame.width`. If the embedded pack's character sprite ever grows/shrinks,
    // the const must move with it — else the pin box drifts off the painted
    // sprite. `sim.rs` resolves the SAME "standing" reference pose per frame.
    #[test]
    fn character_sprite_w_matches_the_embedded_pack() {
        let pack = test_default_pack();
        let frame = pack
            .animation("standing")
            .and_then(|a| a.frames.first())
            .expect("embedded pack carries a standing pose");
        let (w, h) = (frame.width(), frame.height());
        assert_eq!(
            w,
            crate::layout::CHARACTER_SPRITE_W,
            "embedded 'standing' sprite is {w}px wide but CHARACTER_SPRITE_W is {} — \
             update the const so hit-test/decor/label geometry tracks the pack",
            crate::layout::CHARACTER_SPRITE_W
        );
        // The px sprite is `H_CELLS` half-block rows tall (2 px per cell); pin
        // the cell const too so the hit-test box height can't drift from the pack.
        assert_eq!(
            h,
            crate::layout::CHARACTER_SPRITE_H_CELLS * 2,
            "embedded 'standing' sprite is {h}px tall but CHARACTER_SPRITE_H_CELLS \
             ({}) implies {}px — update the const so the hit-test box tracks the pack",
            crate::layout::CHARACTER_SPRITE_H_CELLS,
            crate::layout::CHARACTER_SPRITE_H_CELLS * 2
        );
    }

    // The desk sprite's row width is a THIRD copy of `DESK_W + 4` (baked into the
    // `.sprite` asset rows), alongside the FurnitureDef `visual.w` the renderer
    // blits from and the mask/z-key/anchor read. `DESK_W`'s doc invites future
    // laptop-density edits; such an edit moves `visual.w` but NOT the asset rows,
    // silently desyncing render vs mask/occlusion/collision. Pin the asset width
    // to `visual.w` so that drift fails loud (mirrors
    // `character_sprite_w_matches_the_embedded_pack` above).
    #[test]
    fn desk_sprite_width_tracks_the_footprint_overhang() {
        let pack = test_default_pack();
        let w = pack
            .animation("desk")
            .and_then(|a| a.frames.first())
            .expect("embedded pack carries a desk sprite")
            .width();
        assert_eq!(
            w,
            crate::layout::desk_furniture_def().visual.w,
            "embedded 'desk' sprite is {w}px wide but visual.w (DESK_W+4) is {} — \
             a DESK_W edit moved visual.w but not the .sprite rows; render/mask/z-sort will drift",
            crate::layout::desk_furniture_def().visual.w
        );
    }
}
