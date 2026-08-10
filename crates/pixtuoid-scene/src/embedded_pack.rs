//! Sprite pack loader: the user-config path (XDG-style) first, falling back to
//! the embedded default pack (`include_str!`) so the binary ships standalone.
//! A custom pack is a directory at
//! `${XDG_CONFIG_HOME:-~/.config}/pixtuoid/sprites/` holding `pack.toml` + each
//! `.sprite` file it references (`sprites/default/` is the canonical example).
//!
//! Sharp edge: the per-agent recolor (`recolor_frame`) substitutes palette
//! colors by RGB equality, so each palette key MUST map to a UNIQUE RGB triple
//! or the pass substitutes both keys and produces artifacts.

use std::path::PathBuf;

use anyhow::Result;
use pixtuoid_core::sprite::format::{
    load_pack, load_pack_from_strings, validate_pack_animations, Pack, ValidationReport,
};

/// The user's sprite-pack directory, if XDG settings point at one holding a
/// `pack.toml`.
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

/// Resolve the XDG config base: the env value when set to a NON-EMPTY ABSOLUTE
/// path, else `<home>/.config`. Per the XDG basedir spec an EMPTY **or
/// RELATIVE** `XDG_CONFIG_HOME` is invalid and counts as unset; without
/// `is_absolute()` a `Some("rel")` yields a CWD-RELATIVE `pixtuoid/sprites`
/// path, silently loading an untrusted pack from the launch directory. Pure (the
/// env value is passed in) so the precedence is testable without mutating env.
fn xdg_config_base(xdg: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|v| std::path::Path::new(v).is_absolute())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".config")))
}

/// Log a custom pack's animation-validation gaps at load time: a pack missing a
/// required pose LOADS fine and then renders it as NOTHING, so without this the
/// only signal is agents silently vanishing. Warn, don't fail — a
/// partially-authored pack still renders every pose it does carry. Must run
/// AFTER `merge_from`, or furniture inherited from the embedded default is
/// misreported as missing.
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
    // Every error category `has_errors` counts must warn here, or the load path
    // stays quiet about exactly the failure the check exists to catch: a
    // mis-sized variant is picked up BY NAME and drawn at the wrong size, and
    // the author only ever sees it if they happened to run `validate-pack`.
    for m in &report.mismatched_density {
        tracing::warn!(
            origin,
            animation = %m.name,
            claimed = ?m.claimed,
            found = ?m.found,
            "custom sprite pack density variant is not the size its name claims — \
             it will draw at the wrong size wherever a renderer picks that density"
        );
    }
    for name in &report.orphan_variants {
        tracing::warn!(
            origin,
            animation = %name,
            "custom sprite pack ships a density variant whose base piece it does not — \
             its size claim is checked against the default pack's art, not yours"
        );
    }
    report
}

/// Load the character sprite pack: the compiled-in default pack, with an
/// optional `--pack-dir` custom pack merged over it.
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
/// load can't race the env-MUTATING XDG test under plain `cargo test` (one
/// binary, many threads; nextest's per-process isolation masks the race). Every
/// unit test resolving the default pack MUST come through here, never a bare
/// `load_sprite_pack(None)`.
#[cfg(test)]
pub(crate) fn test_default_pack() -> Pack {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    load_sprite_pack(None).expect("default pack loads")
}

fn load_embedded_pack() -> Result<Pack> {
    load_pack_from_strings(
        include_str!("../sprites/default/pack.toml"),
        &embedded_sprite_srcs(),
    )
}

/// Every default sprite as `(filename, source)`. The macro keeps a new sprite to
/// a SINGLE line — not a `let`-binding AND a matching tuple entry that can
/// silently drift. Extracted so [`test_wide_pack`] reuses the EXACT sprite set
/// and only overrides `standing.sprite`.
fn embedded_sprite_srcs() -> Vec<(&'static str, &'static str)> {
    macro_rules! embedded_sprites {
        ($($name:literal),+ $(,)?) => {
            vec![$(($name, include_str!(concat!("../sprites/default/", $name)))),+]
        };
    }
    embedded_sprites![
        "seated.sprite",
        "seated_back.sprite",
        "typing_back_0.sprite",
        "typing_back_1.sprite",
        "side_seated.sprite",
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
        "desk@4x.sprite",
        "desk_north.sprite",
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
    ]
}

/// The default pack with a 10px-wide `standing` frame so the pack-resolved
/// `char_w` differs from the bundled 8-wide `CHARACTER_SPRITE_W` — the only way
/// to drive `sim_step`/`resolve_characters` occupancy + anchors end-to-end at a
/// non-default width. Reuses the FULL default sprite set so `resolve_characters`
/// still finds every pose; only `standing.sprite` is swapped.
#[cfg(test)]
pub(crate) fn test_wide_pack() -> Pack {
    // No TEST_ENV_LOCK: unlike test_default_pack this builds via the pure
    // load_pack_from_strings and never reads XDG_CONFIG_HOME.
    // The bundled 8x12 standing pose padded to 10 wide with transparent columns
    // (same palette keys).
    const WIDE_STANDING: &str = "\
@frame 0
. . n H H H H n . .
. n H H H H H H n .
. H H S S S S H H .
. H S e S S e S H .
. . S S S m S S . .
. . n S S S S n . .
. . B B B B B B . .
. B B B B B B B B .
. S B B B B B B S .
. . P P P P P P . .
. . P P P P P P . .
. . P . . . . P . .
";
    let mut srcs = embedded_sprite_srcs();
    for entry in &mut srcs {
        if entry.0 == "standing.sprite" {
            entry.1 = WIDE_STANDING;
        }
    }
    load_pack_from_strings(include_str!("../sprites/default/pack.toml"), &srcs)
        .expect("wide test pack loads")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn xdg_config_base_treats_empty_or_relative_as_unset() {
        for invalid in ["", "   ", "rel/config", "~/config"] {
            assert_eq!(
                xdg_config_base(
                    Some(std::ffi::OsString::from(invalid)),
                    Some(PathBuf::from("/home/u"))
                ),
                Some(PathBuf::from("/home/u/.config")),
                "invalid XDG_CONFIG_HOME {invalid:?} must fall to ~/.config"
            );
        }
    }

    #[test]
    fn xdg_config_base_prefers_a_set_value_over_home() {
        // A leading-slash path is NOT absolute on Windows (no drive prefix).
        let abs = if cfg!(windows) { "C:/xdg" } else { "/xdg" };
        assert_eq!(
            xdg_config_base(
                Some(std::ffi::OsString::from(abs)),
                Some(PathBuf::from("/home/u")),
            ),
            Some(PathBuf::from(abs)),
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

    /// Copy this crate's char-only pack fixture into `dst`. It carries NO
    /// furniture, so the merge-from-embedded-default assertion isn't
    /// tautological, and it lives INSIDE pixtuoid-scene so `cargo test` passes
    /// from an extracted .crate — it must NOT reach into the sibling `pixtuoid`
    /// binary crate's skeleton.
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
        assert!(
            pack.animation("seated").is_some(),
            "custom pack must carry the seated character pose"
        );
        assert!(
            pack.animation("desk").is_some(),
            "furniture merged from the embedded default"
        );
    }

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
    fn embedded_default_pack_animations_are_all_in_the_registry() {
        // An animation the EMBEDDED pack ships but the registry doesn't know is
        // falsely reported "unused by renderer" by validate-pack.
        let pack = load_sprite_pack(None).expect("embedded pack");
        let report = pixtuoid_core::sprite::format::validate_pack_animations(&pack);
        assert!(
            report.unknown.is_empty(),
            "embedded animation missing from the registry: {:?}",
            report.unknown
        );
    }

    #[test]
    fn custom_pack_missing_required_pose_loads_with_a_load_time_warning() {
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

    // Mutates a process-global env var, so it takes TEST_ENV_LOCK to serialize
    // against every env-READING `test_default_pack()` caller. It calls
    // `load_sprite_pack` DIRECTLY, not the locked helper: it already holds the
    // (non-reentrant) lock.
    #[test]
    fn load_sprite_pack_resolves_then_falls_back_via_xdg() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("XDG_CONFIG_HOME");

        let good = tempfile::TempDir::new().expect("tempdir");
        let good_sprites = good.path().join("pixtuoid").join("sprites");
        copy_skeleton_pack(&good_sprites);
        std::env::set_var("XDG_CONFIG_HOME", good.path());
        let pack = load_sprite_pack(None).expect("xdg pack loads");
        assert!(
            pack.animation("seated").is_some(),
            "the valid XDG pack must be loaded (xdg Ok arm)"
        );

        // A malformed pack.toml at the XDG path takes the Err arm.
        let bad = tempfile::TempDir::new().expect("tempdir");
        let bad_sprites = bad.path().join("pixtuoid").join("sprites");
        fs::create_dir_all(&bad_sprites).expect("mkdir bad sprites");
        fs::write(bad_sprites.join("pack.toml"), b"this is not valid toml {{{")
            .expect("write malformed pack.toml");
        std::env::set_var("XDG_CONFIG_HOME", bad.path());
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

    // Wider than the recolor-key check below: EVERY palette key must be a
    // distinct RGB, because recolor_frame matches by equality. Transparent
    // (None) keys are exempt.
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

    #[test]
    fn embedded_pack_recolor_keys_are_distinct_rgbs() {
        let pack = test_default_pack();
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
        // The px sprite is `CHARACTER_SPRITE_H_CELLS` half-block rows tall, 2 px
        // per cell.
        assert_eq!(
            h,
            crate::layout::CHARACTER_SPRITE_H_CELLS * 2,
            "embedded 'standing' sprite is {h}px tall but CHARACTER_SPRITE_H_CELLS \
             ({}) implies {}px — update the const so the hit-test box tracks the pack",
            crate::layout::CHARACTER_SPRITE_H_CELLS,
            crate::layout::CHARACTER_SPRITE_H_CELLS * 2
        );
    }

    /// A facing does not change how big a desk IS: `desk_north` is taller only above `desk.y`.
    /// Checked on the edge COLUMNS the monitor never covers — the middle legitimately differs.
    #[test]
    fn both_desk_variants_are_the_same_desk_below_the_monitor() {
        let pack = test_default_pack();
        let frame = |n: &str| {
            pack.animation(n)
                .and_then(|a| a.frames.first())
                .unwrap_or_else(|| panic!("the embedded pack ships {n}"))
        };
        let (base, north) = (frame("desk"), frame("desk_north"));
        assert_eq!(base.width(), north.width(), "a facing never changes width");
        // Both blit so their BOTTOM rows coincide, so the taller one's extra rows are all above.
        let lift = north
            .height()
            .checked_sub(base.height())
            .expect("the raised variant is the taller one");
        // `desk.y` is one row into the base sprite (its top row is the bezel).
        const BASE_DESK_Y_ROW: u16 = 1;
        let edges = [0, 1, base.width() - 2, base.width() - 1];
        for x in edges {
            for dy in 0..(base.height() - BASE_DESK_Y_ROW) {
                let b = base.get(x, BASE_DESK_Y_ROW + dy);
                let n = north.get(x, BASE_DESK_Y_ROW + lift + dy);
                assert_eq!(
                    b, n,
                    "column {x} differs at desk.y+{dy}: the two variants must be \
                     the same desk below the monitor"
                );
            }
        }

        // The column loop above starts at `desk.y`, so wood a variant grows ABOVE that row is
        // invisible to it; counting OPAQUE rows would miss it too (a screen row is opaque either way).
        let wood = base
            .get(0, BASE_DESK_Y_ROW)
            .expect("the desk's west edge at desk.y is surface");
        let surface_rows = |f: &pixtuoid_core::sprite::Frame| {
            (0..f.height())
                .filter(|&y| (0..f.width()).any(|x| f.get(x, y) == Some(wood)))
                .count() as u16
        };
        for (name, art) in [("desk", base), ("desk_north", north)] {
            assert_eq!(
                surface_rows(art),
                crate::layout::DESK_SURFACE_ROWS,
                "{name} draws {} rows of surface; DESK_SURFACE_ROWS declares {}",
                surface_rows(art),
                crate::layout::DESK_SURFACE_ROWS
            );
        }
    }

    // The desk sprite's row width is a THIRD copy of `DESK_W + 4`, baked into the
    // `.sprite` asset rows: a `DESK_W` edit moves `visual.w` but NOT the asset,
    // silently desyncing render vs mask/occlusion/collision.
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

    #[test]
    fn pet_hitboxes_track_the_embedded_pack() {
        use crate::pet::PetKind;
        let pack = test_default_pack();
        for &kind in PetKind::ALL {
            for anim in [kind.walk_anim(), kind.sit_anim(), kind.sleep_anim()] {
                let frame = pack
                    .animation(anim)
                    .and_then(|a| a.frames.first())
                    .unwrap_or_else(|| panic!("embedded pack carries a '{anim}' sprite"));
                let hb = kind.hitbox(anim);
                assert_eq!(
                    (hb.w, hb.h),
                    (frame.width(), frame.height()),
                    "{anim} hitbox {}x{} != sprite {}x{} — a pet-sprite resize drifted the click target",
                    hb.w,
                    hb.h,
                    frame.width(),
                    frame.height()
                );
            }
        }
    }
}
