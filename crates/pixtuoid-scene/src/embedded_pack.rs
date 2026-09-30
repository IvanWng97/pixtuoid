//! Sprite packs: the compiled-in default (`include_str!`, so the binary ships
//! standalone), with at most one custom pack merged over it. A custom pack is a
//! directory holding `pack.toml` + each `.sprite` file it references
//! (`sprites/default/` is the canonical example); `PackSource` names where it
//! comes from, and deciding that is the caller's job.

#[cfg(feature = "native")]
use std::path::{Path, PathBuf};

#[cfg(feature = "native")]
use anyhow::{Context, Result};
use pixtuoid_core::sprite::error::PackError;
#[cfg(feature = "native")]
use pixtuoid_core::sprite::format::{DensityMismatch, FrameCountMismatch, load_pack};
use pixtuoid_core::sprite::format::{
    Pack, ValidationReport, load_pack_from_strings, validate_pack_animations,
};

/// Where a sprite pack's custom half comes from. The source decides what a
/// custom pack that fails to load means.
#[cfg(feature = "native")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackSource {
    /// The compiled-in default alone.
    Bundled,
    /// A pack the user named (`--pack-dir`, config `pack-dir`): failing to load
    /// it is an error, since they asked for it.
    Explicit(PathBuf),
    /// A pack found without being named, in the user's config directory:
    /// failing to load it falls back to the default, so a broken file nobody
    /// pointed at never stops the office starting.
    Discovered(PathBuf),
}

/// The sets of pieces a pack should ship whole, each read from the authority its
/// painter picks by: each row is the pantry counters
/// (`pixel_painter::pantry_counter_anim` picks one by room width), a pet kind's
/// poses, or a gateway mascot's poses.
fn art_sets() -> Vec<Vec<&'static str>> {
    let mut sets = vec![crate::pixel_painter::PANTRY_COUNTER_ANIMS.to_vec()];
    sets.extend(
        crate::pet::PetKind::ALL
            .iter()
            .map(|k| vec![k.walk_anim(), k.sit_anim(), k.sleep_anim()]),
    );
    sets.extend(
        pixtuoid_core::source::registry::registered_source_names()
            .filter_map(crate::creatures::gateway_mascot_def)
            .map(|d| vec![d.walk, d.rest]),
    );
    sets
}

/// [`validate_pack_animations`], against this crate's painters' art sets.
pub fn validate_pack(pack: &Pack) -> ValidationReport {
    validate_pack_animations(pack, &art_sets())
}

/// Log a custom pack's animation-validation gaps at load time: a pack missing a
/// required pose LOADS fine and then renders it as NOTHING, so without this the
/// only signal is agents silently vanishing. Warn, don't fail — a
/// partially-authored pack still renders every pose it does carry.
#[cfg(feature = "native")]
fn warn_pack_validation_gaps(pack: &Pack, origin: &str) -> ValidationReport {
    let report = validate_pack(pack);
    // Destructured without `..`: a field added to the report does not compile
    // until it is named here, so a new error category cannot bypass this
    // load-time warning unnoticed.
    let ValidationReport {
        missing_required,
        missing_optional: _,
        insufficient_frames,
        unknown: _,
        mismatched_density,
        orphan_variants,
        mismatched_frame_counts,
        // These still render: `validate-pack` reports them.
        partial_sets: _,
        orphan_derived: _,
        unmarked_heads: _,
        missing_hair_views: _,
        overhanging_hair: _,
        orphan_hairstyles,
    } = &report;
    for name in missing_required {
        tracing::warn!(
            origin,
            animation = ?name,
            "custom sprite pack is missing a REQUIRED character animation — \
             agents will be invisible in that pose (run `pixtuoid validate-pack`)"
        );
    }
    for (name, min, got) in insufficient_frames {
        tracing::warn!(
            origin,
            animation = ?name,
            min,
            got,
            "custom sprite pack animation has too few frames — it will render as nothing"
        );
    }
    // Each finding destructured without `..`, for the reason the report is.
    for DensityMismatch {
        name,
        frame,
        claimed,
        found,
    } in mismatched_density
    {
        tracing::warn!(
            origin,
            animation = ?name,
            frame,
            claimed = ?claimed,
            found = ?found,
            "custom sprite pack density variant is not the size its name claims — \
             renderers skip it for the densest art that fits"
        );
    }
    for name in orphan_variants {
        tracing::warn!(
            origin,
            animation = ?name,
            "custom sprite pack ships a density variant whose base animation it does not — \
             a furniture variant is checked against the default pack's art, not yours; \
             a character variant never draws"
        );
    }
    for FrameCountMismatch {
        name,
        base_frames,
        variant_frames,
    } in mismatched_frame_counts
    {
        tracing::warn!(
            origin,
            animation = ?name,
            base_frames,
            variant_frames,
            "custom sprite pack density variant has a different frame count from its base — \
             renderers skip it for the densest art that fits"
        );
    }
    for style in orphan_hairstyles {
        tracing::warn!(
            origin,
            hairstyle = ?style,
            "custom sprite pack ships a hairstyle at a density it draws no character at — \
             nobody wears it"
        );
    }
    report
}

/// Load the compiled-in default pack, with `source`'s custom pack merged over
/// it. Reads nothing but the path `source` names, so a test, a benchmark or a
/// committed snapshot draws the same art on every machine.
#[cfg(feature = "native")]
pub fn load_sprite_pack(source: PackSource) -> Result<Pack> {
    let base = load_bundled_pack()?;
    match source {
        PackSource::Bundled => Ok(base),
        PackSource::Explicit(dir) => load_custom_over(&base, &dir, "explicit")
            .with_context(|| format!("failed to load sprite pack from {dir:?}")),
        PackSource::Discovered(dir) => match load_custom_over(&base, &dir, "discovered") {
            Ok(pack) => Ok(pack),
            Err(e) => {
                let chain = format!("{e:#}");
                tracing::warn!(
                    path = ?dir,
                    error = ?chain,
                    "user sprite pack failed to load; falling back to embedded default"
                );
                Ok(base)
            }
        },
    }
}

/// The custom pack in `dir`, with the furniture it leaves out, and the city if
/// it draws none, inherited from `base`.
#[cfg(feature = "native")]
fn load_custom_over(base: &Pack, dir: &Path, origin: &str) -> Result<Pack> {
    let mut custom = load_pack(dir)?;
    tracing::info!(origin, path = ?dir, "loaded custom sprite pack");
    // Before the merge, so the report is about what the author shipped: after
    // it, a furniture variant whose base the pack leaves out is checked against
    // the default's art and never reported as an orphan.
    warn_pack_validation_gaps(&custom, origin);
    custom.merge_from(base);
    Ok(custom)
}

/// The bundled pack, for unit tests.
#[cfg(test)]
pub(crate) fn test_default_pack() -> Pack {
    load_bundled_pack().expect("default pack loads")
}

/// The default pack's manifest, as `build.rs` embeds it.
const EMBEDDED_PACK_TOML: &str = include_str!(concat!(env!("OUT_DIR"), "/embedded_pack.toml"));

/// The compiled-in default pack alone: all a build without `native` can load.
pub fn load_bundled_pack() -> Result<Pack, PackError> {
    load_pack_from_strings(EMBEDDED_PACK_TOML, &embedded_sprite_srcs())
}

/// Every default sprite as `(filename, source)`: every `.sprite` in
/// `sprites/default/`, listed by `build.rs` (less, without `density-art`, the
/// frames only a density variant draws), so a sprite committed there cannot be
/// left out by omission. `test_pack_with` swaps files within this EXACT set.
fn embedded_sprite_srcs() -> Vec<(&'static str, &'static str)> {
    const SPRITES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/embedded_sprites.rs"));
    SPRITES.to_vec()
}

/// The default pack with a wider `standing` frame (`WIDE_STANDING`), so the
/// pack-resolved `char_w` differs from the bundled `CHARACTER_SPRITE_W`: the
/// only way to drive `sim_step`/`resolve_characters` occupancy and anchors end
/// to end at a non-default width.
#[cfg(test)]
pub(crate) fn test_wide_pack() -> Pack {
    // The bundled standing pose padded to 10 wide with transparent columns
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
    test_pack_with(&[("standing.sprite", WIDE_STANDING)])
}

/// The default pack with each `(file, source)` in `overrides` swapped in.
///
/// An override orphans any density variant that redraws its file: a denser
/// scale skips that variant
/// ([`variant_redraws`](pixtuoid_core::sprite::format::variant_redraws)) and
/// draws the swapped base.
#[cfg(test)]
pub(crate) fn test_pack_with(overrides: &[(&str, &'static str)]) -> Pack {
    let mut srcs = embedded_sprite_srcs();
    for &(file, source) in overrides {
        let entry = srcs
            .iter_mut()
            .find(|(name, _)| *name == file)
            .expect("an override names a bundled sprite");
        entry.1 = source;
    }
    load_pack_from_strings(EMBEDDED_PACK_TOML, &srcs).expect("the test pack loads")
}

#[cfg(test)]
#[path = "../build_support/density_art.rs"]
mod density_art;

#[cfg(test)]
#[path = "../build_support/comments.rs"]
mod comments;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "native")]
    use std::fs;
    #[cfg(feature = "native")]
    use std::path::Path;

    #[test]
    fn every_art_set_member_is_registered_furniture_in_one_set_only() {
        let sets = art_sets();
        assert!(!sets.is_empty());
        let mut seen = std::collections::HashSet::new();
        for set in &sets {
            assert!(set.len() >= 2, "a one-piece set can't be partial: {set:?}");
            for &name in set {
                assert!(
                    pixtuoid_core::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS.contains(&name),
                    "{name}"
                );
                assert!(seen.insert(name), "{name} is in two sets");
            }
        }
    }

    /// Copy this crate's char-only fixture into `dst`: no furniture, so the merge
    /// assertion bites; in-crate, so `cargo test` passes from an extracted .crate.
    #[cfg(feature = "native")]
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
    #[cfg(feature = "native")]
    fn load_sprite_pack_from_custom_dir_merges_with_embedded() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pack_dir = tmp.path().join("custom");
        copy_skeleton_pack(&pack_dir);

        let pack = load_sprite_pack(PackSource::Explicit(pack_dir)).expect("custom pack loads");
        assert!(
            pack.animation("seated").is_some(),
            "custom pack must carry the seated character pose"
        );
        assert!(
            pack.animation("desk").is_some(),
            "furniture merged from the embedded default"
        );
    }

    #[cfg(feature = "native")]
    #[derive(Clone)]
    struct WarnCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    #[cfg(feature = "native")]
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

    /// The bundled pack is the one no user validates: a mis-sized `@Nx` variant
    /// in it silently falls back to the upscaled base.
    #[test]
    fn the_embedded_pack_passes_its_own_validation() {
        let pack = test_default_pack();
        let report = validate_pack(&pack);
        assert!(!report.has_errors(), "{report:?}");
        // `StandIn::DefaultPack` promises the default draws what a custom pack
        // leaves out.
        assert_eq!(report.warning_count(), 0, "{report:?}");
    }

    /// `build.rs` embeds every sprite in `sprites/default/`, so one no animation,
    /// hairstyle or building registers — a generated file whose `pack.toml`
    /// entry was never written — ships as dead bytes and draws nothing. A sprite
    /// the pack still loads without is exactly that.
    #[test]
    fn every_embedded_sprite_is_a_frame_the_pack_loads() {
        let unregistered = undrawn(EMBEDDED_PACK_TOML, &embedded_sprite_srcs());
        assert!(
            unregistered.is_empty(),
            "sprites pack.toml never registers: {unregistered:?}"
        );
    }

    /// The sprites `toml` loads without: the ones nothing it registers draws.
    fn undrawn<'a>(toml: &str, srcs: &[(&'a str, &'a str)]) -> Vec<&'a str> {
        // Every leave-one-out load below errors when the whole set does, which
        // would report nothing undrawn.
        load_pack_from_strings(toml, srcs).expect("the whole set loads");
        srcs.iter()
            .map(|&(name, _)| name)
            .filter(|&name| {
                let without: Vec<_> = srcs.iter().copied().filter(|&(n, _)| n != name).collect();
                load_pack_from_strings(toml, &without).is_ok()
            })
            .collect()
    }

    /// The web hero's pack. Nothing runs this suite without `density-art`
    /// (`just hack` only checks), so this is the one place it is loaded.
    #[test]
    fn the_pack_without_density_art_loads_whole() {
        let (toml, dropped) =
            density_art::embedded_without_density_art(include_str!("../sprites/default/pack.toml"));
        assert!(!dropped.is_empty(), "the bundled pack ships density art");
        let srcs: Vec<_> = embedded_sprite_srcs()
            .into_iter()
            .filter(|(name, _)| !dropped.contains(*name))
            .collect();
        let pack = load_pack_from_strings(&toml, &srcs).expect("loads without density art");
        assert_eq!(pack.max_density_variant(), 1);
        assert!(
            pack.buildings().next().is_some(),
            "the city keeps its buildings"
        );
        let d = |n| std::num::NonZeroU16::new(n).expect("nonzero");
        assert!(
            pack.buildings().all(|b| b.variant(d(4)).is_none()),
            "at their base alone"
        );
        let report = validate_pack(&pack);
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(report.warning_count(), 0, "{report:?}");
        let undrawn = undrawn(&toml, &srcs);
        assert!(undrawn.is_empty(), "undrawn sprites: {undrawn:?}");
    }

    /// What a default run's render scale rounds to (`RenderScale::fit`): a
    /// change to the bundled art's densest variant should be a decision.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_bundled_pack_is_drawn_at_most_at_4x() {
        assert_eq!(test_default_pack().max_density_variant(), 4);
    }

    /// Each bundled building ships a variant at the bundled art's density,
    /// beside the base the classic painter draws, for a city drawn on that
    /// art's grid.
    #[test]
    #[cfg(feature = "density-art")]
    fn every_bundled_building_is_drawn_at_1x_and_4x() {
        let pack = test_default_pack();
        assert!(
            pack.buildings().next().is_some(),
            "the bundled pack draws a city"
        );
        let d = |n| std::num::NonZeroU16::new(n).expect("nonzero");
        for b in pack.buildings() {
            assert!(b.variant(d(4)).is_some(), "{}", b.name());
        }
    }

    /// A transparent last row lifts a piece off the floor both painters ground
    /// it on.
    #[test]
    fn every_bundled_furniture_frame_draws_its_bottom_row() {
        let pack = test_default_pack();
        // Pets and mascots stand on their feet, wherever their frame ends.
        let figures: std::collections::HashSet<&str> = crate::pet::PetKind::ALL
            .iter()
            .flat_map(|k| [k.walk_anim(), k.sit_anim(), k.sleep_anim()])
            .chain(
                pixtuoid_core::source::registry::registered_source_names()
                    .filter_map(crate::creatures::gateway_mascot_def)
                    .flat_map(|m| [m.walk, m.rest]),
            )
            .collect();
        let floating: Vec<String> = pack
            .animation_names()
            .into_iter()
            .filter(|name| {
                let base = name.split('@').next().unwrap_or(name);
                pixtuoid_core::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS.contains(&base)
                    && !figures.contains(base)
            })
            .filter(|name| {
                pack.animation(name).is_some_and(|s| {
                    s.frames().iter().any(|f| {
                        let w = usize::from(f.width());
                        f.as_slice()[f.as_slice().len() - w..]
                            .iter()
                            .all(Option::is_none)
                    })
                })
            })
            .collect();
        assert!(floating.is_empty(), "{floating:?}");
    }

    #[test]
    fn embedded_default_pack_animations_are_all_in_the_registry() {
        // An animation the EMBEDDED pack ships but the registry doesn't know is
        // falsely reported "unused by renderer" by validate-pack.
        let pack = test_default_pack();
        let report = validate_pack(&pack);
        assert!(
            report.unknown.is_empty(),
            "embedded animation missing from the registry: {:?}",
            report.unknown
        );
    }

    #[test]
    #[cfg(feature = "native")]
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
            load_sprite_pack(PackSource::Explicit(pack_dir))
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
    #[cfg(feature = "native")]
    fn load_sprite_pack_from_missing_custom_dir_errors() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert!(
            load_sprite_pack(PackSource::Explicit(missing)).is_err(),
            "a nonexistent --pack-dir must surface a load error"
        );
    }

    #[test]
    #[cfg(feature = "native")]
    fn a_discovered_pack_loads_over_the_default_and_a_broken_one_falls_back() {
        let seated = |p: &Pack| p.animation("seated").expect("seated").frames()[0].clone();
        let embedded = seated(&test_default_pack());

        let good = tempfile::TempDir::new().expect("tempdir");
        copy_skeleton_pack(good.path());
        let pack =
            load_sprite_pack(PackSource::Discovered(good.path().into())).expect("user pack loads");
        assert_ne!(
            seated(&pack).as_slice(),
            embedded.as_slice(),
            "the user's own art"
        );
        assert!(
            pack.animation("desk").is_some(),
            "furniture merged from the default"
        );

        let bad = tempfile::TempDir::new().expect("tempdir");
        fs::write(bad.path().join("pack.toml"), b"this is not valid toml {{{")
            .expect("write malformed pack.toml");
        let fallback = load_sprite_pack(PackSource::Discovered(bad.path().into()))
            .expect("a broken discovered pack never errors");
        assert_eq!(seated(&fallback).as_slice(), embedded.as_slice());
        assert!(
            load_sprite_pack(PackSource::Explicit(bad.path().into())).is_err(),
            "the same pack, named, is an error"
        );
    }

    /// A furniture variant whose base the pack leaves out is only an orphan
    /// before the merge fills the base in from the default. Sized as a true 4x
    /// of the default's desk, so a check after the merge finds nothing to warn
    /// about.
    #[test]
    #[cfg(feature = "native")]
    fn a_custom_variant_without_its_base_warns_at_load() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        copy_skeleton_pack(tmp.path());
        let load_warns = || {
            let warns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            tracing::subscriber::with_default(WarnCounter(warns.clone()), || {
                load_sprite_pack(PackSource::Explicit(tmp.path().into()))
            })
            .expect("pack loads");
            warns.load(std::sync::atomic::Ordering::SeqCst)
        };
        assert_eq!(load_warns(), 0, "the fixture itself is clean");

        let desk = test_default_pack()
            .animation("desk")
            .expect("desk")
            .frames()[0]
            .clone();
        let row = vec!["W"; usize::from(desk.width()) * 4].join(" ");
        let rows = vec![row; usize::from(desk.height()) * 4].join("\n");
        fs::write(
            tmp.path().join("desk4x.sprite"),
            format!("@frame 0\n{rows}\n"),
        )
        .expect("write desk4x.sprite");
        let toml_path = tmp.path().join("pack.toml");
        let mut toml = fs::read_to_string(&toml_path).expect("read pack.toml");
        toml.push_str("\n[animations.\"desk@4x\"]\nframes=[\"desk4x.sprite\"]\nframe_ms=100\n");
        fs::write(&toml_path, toml).expect("write pack.toml");
        assert_eq!(load_warns(), 1, "the orphan desk@4x warns");
    }

    #[test]
    fn character_sprite_size_matches_the_embedded_pack() {
        let pack = test_default_pack();
        let frame = pack
            .animation("standing")
            .and_then(|a| a.frames().first())
            .expect("embedded pack carries a standing pose");
        let (w, h) = (frame.width(), frame.height());
        assert_eq!(
            w,
            crate::layout::CHARACTER_SPRITE_W,
            "embedded 'standing' sprite is {w}px wide but CHARACTER_SPRITE_W is {} — \
             update the const so hit-test/decor/label geometry tracks the pack",
            crate::layout::CHARACTER_SPRITE_W
        );
        assert_eq!(
            h,
            crate::layout::CHARACTER_SPRITE_H,
            "embedded 'standing' sprite is {h}px tall but CHARACTER_SPRITE_H is {} — \
             update the const so the hit-test box and the painter's fallback track the pack",
            crate::layout::CHARACTER_SPRITE_H
        );
    }

    /// A facing does not change how big a desk IS: `desk_north` is taller only above `desk.y`.
    /// Checked on the edge COLUMNS the monitor never covers — the middle legitimately differs.
    #[test]
    fn both_desk_variants_are_the_same_desk_below_the_monitor() {
        let pack = test_default_pack();
        let frame = |n: &str| {
            pack.animation(n)
                .and_then(|a| a.frames().first())
                .unwrap_or_else(|| panic!("the embedded pack ships {n}"))
        };
        let (base, north) = (frame("desk"), frame("desk_north"));
        assert_eq!(base.width(), north.width(), "a facing never changes width");
        // Both blit so their BOTTOM rows coincide, so the taller one's extra rows are all above.
        let lift = north
            .height()
            .checked_sub(base.height())
            .expect("the raised variant is the taller one");
        let raise = crate::pixel_painter::DESK_BEZEL_RAISE;
        let edges = [0, 1, base.width() - 2, base.width() - 1];
        for x in edges {
            for dy in 0..(base.height() - raise) {
                let b = base.get(x, raise + dy);
                let n = north.get(x, raise + lift + dy);
                assert_eq!(
                    b, n,
                    "column {x} differs at desk.y+{dy}: the two variants must be \
                     the same desk below the monitor"
                );
            }
        }

        // The column loop above starts at `desk.y`, so wood a variant grows ABOVE that row is
        // invisible to it: count the rows holding surface — the wood or its lit back edge — in
        // any column.
        let surface = ['D', 'O'].map(|key| {
            pack.palette()
                .get(key)
                .flatten()
                .unwrap_or_else(|| panic!("{key:?} is an opaque surface key"))
        });
        let surface_rows = |f: &pixtuoid_core::sprite::Frame| {
            (0..f.height())
                .filter(|&y| {
                    (0..f.width()).any(|x| {
                        f.get(x, y)
                            .copied()
                            .flatten()
                            .is_some_and(|c| surface.contains(&c))
                    })
                })
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

    /// The generated desk art follows the layout's row split — the bezel raise,
    /// then surface, front lip and legs — which scripts/gen-art.py copies; a
    /// drift there moves the wood the depth sort and the glow assume. Read by
    /// STRUCTURE, so an art restyle cannot fail it: legs show open floor
    /// between them, the lip spans the width, the surface is opaque at the west
    /// edge, and nothing but the monitor rises above it.
    #[test]
    fn a_desks_rows_follow_the_layout() {
        use crate::layout::{DESK_FRONT_ROWS, DESK_LEG_ROWS, DESK_SURFACE_ROWS};
        let pack = test_default_pack();
        let raise = crate::pixel_painter::DESK_BEZEL_RAISE;
        let base_h = raise + DESK_SURFACE_ROWS + DESK_FRONT_ROWS + DESK_LEG_ROWS;
        for name in ["desk", "desk_north"] {
            let f = pack
                .animation(name)
                .and_then(|a| a.frames().first())
                .unwrap_or_else(|| panic!("the embedded pack ships {name}"));
            let (w, h) = (f.width(), f.height());
            let opaque = |x: u16, y: u16| f.get(x, y).copied().flatten().is_some();
            let lift = h
                .checked_sub(base_h)
                .expect("no desk is shorter than the layout's split");
            let legs0 = h - DESK_LEG_ROWS;
            let lip0 = legs0 - DESK_FRONT_ROWS;
            let top = lip0 - DESK_SURFACE_ROWS;
            assert_eq!(
                top,
                raise + lift,
                "{name}: the surface starts below the monitor's raise"
            );
            for y in legs0..h {
                assert!(
                    opaque(0, y) && !opaque(w / 2, y),
                    "{name} row {y}: legs, open between"
                );
            }
            for y in lip0..legs0 {
                assert!(
                    (0..w).all(|x| opaque(x, y)),
                    "{name} row {y}: the lip spans the desk"
                );
            }
            for y in top..lip0 {
                assert!(opaque(0, y), "{name} row {y}: surface at the west edge");
            }
            for y in 0..top {
                assert!(
                    !opaque(0, y),
                    "{name} row {y}: only the monitor rises above the wood"
                );
            }
        }
    }

    // The desk art's width is scripts/gen-art.py's `DESK_ART_W`, a copy of
    // `desk_furniture_def().visual.w`: a `DESK_W` edit moves `visual.w` but not
    // the generated art, silently desyncing render vs mask/occlusion/collision.
    #[test]
    fn desk_sprite_width_tracks_the_footprint_overhang() {
        let pack = test_default_pack();
        let w = pack
            .animation("desk")
            .and_then(|a| a.frames().first())
            .expect("embedded pack carries a desk sprite")
            .width();
        assert_eq!(
            w,
            crate::layout::desk_furniture_def().visual.w,
            "embedded 'desk' sprite is {w}px wide but visual.w is {} — \
             a DESK_W edit moved visual.w but not scripts/gen-art.py's DESK_ART_W; render/mask/z-sort will drift",
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
                    .and_then(|a| a.frames().first())
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
