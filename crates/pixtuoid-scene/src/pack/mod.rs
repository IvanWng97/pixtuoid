//! The sprite pack: `sprites/default/`, compiled in (`include_str!`), so the
//! binary ships standalone.

mod density;
mod lookup;

pub(crate) use density::{DenseFrame, densest_frame};
#[cfg(test)]
pub(crate) use lookup::DESK_BEZEL_RAISE;
pub(crate) use lookup::{
    CLOCK_FACE_KEY, CLOCK_SPRITE, COOLER_WATER, CUP_MARK, DESK_BULB_KEY, DESK_CHAIR_SPRITE,
    DESK_CUP_SPRITE, DOOR_SPRITE, FISH_TANK_SPRITE, MEETING_SOFA_NORTH_SPRITE,
    MEETING_TABLE_SPRITE, NORTH_SOFA_SEAT_ROWS, PRINTER_SPRITE, PropMark, SCREEN_GLASS_KEY,
    SCREEN_TEXT_KEY, TOKEN_SHEET_SPRITE, TOKEN_TOWER_SPRITE, TOWER_MARK, VENDING_MACHINE_SPRITE,
    WATER_COOLER_SPRITE, animation_frame_at, appliance_frame_index, appliance_overrides,
    appliance_sprite, bulb_cell, desk_art, desk_art_name, desk_art_top, desk_bulb_offset,
    desk_front, desk_mark, desk_prop_overrides, desk_props_mirrored, drawn_in, fixture_overrides,
    frame_at, looping_frame_index, prop_left,
};
#[cfg(test)]
pub(crate) use lookup::{MONITOR_KEYS, desk_sprite_name};

use pixtuoid_core::sprite::error::PackError;
use pixtuoid_core::sprite::format::{
    Pack, PackContract, ValidationReport, load_pack_from_strings, validate_pack_animations,
};

/// The sets of pieces a pack should ship whole, each read from the authority its
/// painter picks by: each row is the pantry counters
/// (`layout::pantry_counter_anim` picks one by room width), a pet kind's
/// poses, or a gateway mascot's poses.
fn art_sets() -> Vec<Vec<&'static str>> {
    let mut sets = vec![crate::layout::PANTRY_COUNTER_ANIMS.to_vec()];
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

/// Every walk a walker steps by the ground it covers: a person's, each pet's
/// and each gateway mascot's.
fn walks() -> Vec<&'static str> {
    crate::sim::WALKS
        .into_iter()
        .chain(crate::pet::PetKind::ALL.iter().map(|k| k.walk_anim()))
        .chain(
            pixtuoid_core::source::registry::registered_source_names()
                .filter_map(crate::creatures::gateway_mascot_def)
                .map(|d| d.walk),
        )
        .collect()
}

/// The animations a painter loops on the beat, each with the frame its loop
/// starts at: the looping fixtures, the appliances' busy loops
/// ([`appliance_frame_index`]), the typists
/// (`pose::typing_frame`), and every creature pose.
fn looped_animations() -> Vec<(&'static str, usize)> {
    let appliances =
        [VENDING_MACHINE_SPRITE, PRINTER_SPRITE].map(|name| (name, lookup::APPLIANCE_IDLE_FRAMES));
    let pets = crate::pet::PetKind::ALL
        .iter()
        .flat_map(|k| [k.walk_anim(), k.sit_anim(), k.sleep_anim()]);
    let mascots = pixtuoid_core::source::registry::registered_source_names()
        .filter_map(crate::creatures::gateway_mascot_def)
        .flat_map(|def| [def.walk, def.rest]);
    [
        FISH_TANK_SPRITE,
        WATER_COOLER_SPRITE,
        "typing",
        "typing_back",
    ]
    .into_iter()
    .chain(pets)
    .chain(mascots)
    .map(|name| (name, 0))
    .chain(appliances)
    .collect()
}

/// The marks every desk's first frame carries: the cup and the token tower
/// stand there in both looks, and the cup's steam rises there — the
/// cutaway's `push_desk_props` reads them, the classic and the steam through
/// [`desk_mark`]. A desk that mirrors its props ([`desk_props_mirrored`])
/// marks each one's bottom-right cell.
const DESK_MARKS: [(&str, &[&str]); 2] = [
    (lookup::DESK_SPRITE, &[CUP_MARK, TOWER_MARK]),
    (lookup::DESK_NORTH_SPRITE, &[CUP_MARK, TOWER_MARK]),
];

/// The key every desk draws its lamp's bulb in: its pool centres there
/// ([`bulb_cell`]).
const DESK_BULBS: [(&str, char); 2] = [
    (lookup::DESK_SPRITE, DESK_BULB_KEY),
    (lookup::DESK_NORTH_SPRITE, DESK_BULB_KEY),
];

/// [`validate_pack_animations`], against this crate's painters' art sets,
/// walks, desk marks, desk bulbs and loops on the Full beat.
pub fn validate_pack(pack: &Pack) -> ValidationReport {
    validate_pack_animations(
        pack,
        &PackContract {
            art_sets: &art_sets(),
            walks: &walks(),
            marks: &DESK_MARKS,
            loops: &looped_animations(),
            beat_ms: crate::anim::FULL_TICK_MS,
            keys: &DESK_BULBS,
        },
    )
}

/// The bundled pack, for unit tests: parsed once per process, since the parse
/// dominates a test that loads it per frame; each caller gets its own copy.
#[cfg(test)]
pub(crate) fn test_default_pack() -> Pack {
    static PACK: std::sync::OnceLock<Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| load_bundled_pack().expect("default pack loads"))
        .clone()
}

/// The default pack's manifest, as `build.rs` embeds it.
const BUNDLED_PACK_TOML: &str = include_str!(concat!(env!("OUT_DIR"), "/bundled_pack.toml"));

/// The compiled-in default pack alone: all a build without `native` can load.
///
/// # Errors
///
/// If the embedded manifest or a bundled sprite source fails to parse or validate.
pub fn load_bundled_pack() -> Result<Pack, PackError> {
    load_pack_from_strings(BUNDLED_PACK_TOML, &bundled_sprite_srcs())
}

/// Every default sprite as `(filename, source)`: every `.sprite` in
/// `sprites/default/`, listed by `build.rs` (less, without `cutaway-assets`, the
/// frames only a density variant draws), so a sprite committed there cannot be
/// left out by omission. `test_pack_with` swaps files within this EXACT set.
fn bundled_sprite_srcs() -> Vec<(&'static str, &'static str)> {
    const SPRITES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/bundled_sprites.rs"));
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
    let mut srcs = bundled_sprite_srcs();
    for &(file, source) in overrides {
        let entry = srcs
            .iter_mut()
            .find(|(name, _)| *name == file)
            .expect("an override names a bundled sprite");
        entry.1 = source;
    }
    load_pack_from_strings(BUNDLED_PACK_TOML, &srcs).expect("the test pack loads")
}

/// The bundled pack with its manifest's `old` text read as `new`, for a test
/// of what a pack declares.
#[cfg(test)]
pub(crate) fn test_pack_declaring(old: &str, new: &str) -> Pack {
    assert!(
        BUNDLED_PACK_TOML.contains(old),
        "the bundled manifest says {old:?}"
    );
    load_pack_from_strings(
        &BUNDLED_PACK_TOML.replacen(old, new, 1),
        &bundled_sprite_srcs(),
    )
    .expect("the test pack loads")
}

#[cfg(test)]
#[path = "../../build_support/density_art.rs"]
mod density_art;

#[cfg(test)]
#[path = "../../build_support/comments.rs"]
mod comments;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_scale::RenderScale;
    use pixtuoid_core::sprite::format::Density;

    /// Every loop the beat plays from the bundled pack holds each frame whole
    /// Full beats, so the beat neither skips nor stretches one, and every
    /// variant keeps its base's timing, the one that plays.
    #[test]
    fn every_bundled_loop_holds_its_frames_whole_beats() {
        loops_hold_whole_beats("default", &test_default_pack());
    }

    /// `pack`'s loops hold whole Full beats, and its variants their base's
    /// timing.
    fn loops_hold_whole_beats(pack_name: &str, pack: &Pack) {
        let report = validate_pack(pack);
        assert_eq!(report.off_beat_loops, [], "{pack_name}");
        assert_eq!(report.unread_variant_timing, [], "{pack_name}");
    }

    /// The beat check reaches every loop a painter plays: a typist off the
    /// beat is reported.
    #[test]
    fn a_typist_off_the_beat_is_reported() {
        let pack = test_pack_declaring(
            "[animations.typing]\nframes   = [\"typing_0.sprite\", \"typing_1.sprite\"]\nframe_ms = 125\n",
            "[animations.typing]\nframes   = [\"typing_0.sprite\", \"typing_1.sprite\"]\nframe_ms = 400\n",
        );
        let off: Vec<_> = validate_pack(&pack)
            .off_beat_loops
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(off, ["typing"]);
    }

    #[test]
    fn every_art_set_member_is_registered_inherited_art_in_one_set_only() {
        let sets = art_sets();
        assert!(!sets.is_empty());
        let mut seen = std::collections::HashSet::new();
        for set in &sets {
            assert!(set.len() >= 2, "a one-piece set can't be partial: {set:?}");
            for &name in set {
                assert!(
                    pixtuoid_core::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS
                        .iter()
                        .chain(pixtuoid_core::sprite::format::OPTIONAL_CREATURE_ANIMATIONS)
                        .any(|&n| n == name),
                    "{name}"
                );
                assert!(seen.insert(name), "{name} is in two sets");
            }
        }
    }

    /// Copy this crate's char-only fixture into `dst`: no furniture, so the merge
    /// assertion bites; in-crate, so `cargo test` passes from an extracted .crate.
    #[test]
    fn the_desk_props_draw_the_keys_the_theme_recolours() {
        let pack = test_default_pack();
        for s in [1, pack.max_density_variant().get()] {
            let scale = crate::render_scale::RenderScale::new(s).expect("nonzero");
            for (sprite, frame, keys) in [
                (
                    DESK_CUP_SPRITE,
                    0,
                    &[lookup::CUP_KEY, lookup::CUP_SHADE_KEY][..],
                ),
                (
                    TOKEN_TOWER_SPRITE,
                    0,
                    &[lookup::PAPER_KEY, lookup::PAPER_SHADE_KEY],
                ),
                (TOKEN_SHEET_SPRITE, 0, &[lookup::PAPER_KEY]),
            ] {
                let art = densest_frame(&pack, sprite, frame, scale)
                    .expect("the bundled pack draws the prop");
                for &key in keys {
                    assert!(
                        drawn_in(&art, &[key]).contains(&true),
                        "{sprite} at scale {s} draws no {key:?}"
                    );
                }
            }
        }
    }

    /// A mis-sized `@Nx` variant in the bundled pack silently falls back to
    /// the upscaled base, so this is where a break in its art shows.
    #[test]
    fn the_bundled_pack_passes_its_own_validation() {
        let pack = test_default_pack();
        let report = validate_pack(&pack);
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(report.warning_count(), 0, "{report:?}");
    }

    /// A pack that ships no back-turned desk draws its back-turned desks from
    /// `desk`: it stands their props at `desk`'s marks and lights `desk`'s lamp.
    #[test]
    fn a_pack_without_desk_north_stands_its_props_on_desk() {
        use crate::layout::{Facing, Point};
        // the base table alone, whichever density variants a build ships
        let north = "[animations.desk_north]\nframes   = [\"desk_north.sprite\"]\nframe_ms = 600\n";
        let pack = test_pack_declaring(north, "");
        let desk = Point { x: 20, y: 30 };
        for mark in [CUP_MARK, TOWER_MARK] {
            let at = |facing| desk_mark(&pack, desk, facing, mark);
            assert!(
                at(Facing::North).is_some(),
                "the back-turned {mark} vanished"
            );
            assert_eq!(at(Facing::North), at(Facing::South), "{mark}");
        }
        let bulb = |facing| desk_bulb_offset(&pack, facing);
        assert!(
            bulb(Facing::North).is_some(),
            "the back-turned lamp went dark"
        );
        assert_eq!(bulb(Facing::North), bulb(Facing::South));
    }

    /// A desk's front is its own art, cut down: on the desk's canvas, every
    /// pixel it draws the desk's, so a desk with no props in front of its
    /// front paints as it always did.
    #[test]
    fn a_desk_front_is_its_desk_cut_down() {
        let pack = test_default_pack();
        let front = desk_front(&pack, lookup::DESK_SPRITE).expect("the bundled desk has a front");
        for scale in [
            RenderScale::ONE,
            RenderScale::from(pack.max_density_variant()),
        ] {
            let desk = densest_frame(&pack, lookup::DESK_SPRITE, 0, scale).expect("the desk");
            let over = densest_frame(&pack, front, 0, scale).expect("the front");
            assert_eq!(
                (over.frame.width(), over.frame.height(), over.blit_at),
                (desk.frame.width(), desk.frame.height(), desk.blit_at),
                "at {scale:?}, the front keeps its desk's canvas"
            );
            let mut drawn = 0;
            for y in 0..desk.frame.height() {
                for x in 0..desk.frame.width() {
                    if let Some(px) = over.frame.get(x, y).copied().flatten() {
                        drawn += 1;
                        assert_eq!(
                            desk.frame.get(x, y).copied().flatten(),
                            Some(px),
                            "at {scale:?}, the front differs from its desk at ({x}, {y})"
                        );
                    }
                }
            }
            assert!(drawn > 0, "at {scale:?}, the front draws something");
        }
        assert_eq!(
            desk_front(&pack, lookup::DESK_NORTH_SPRITE),
            None,
            "nothing stands before a back-turned sitter's props"
        );
    }

    /// A back-turned desk's mirrored tower teeters off its west wing: every
    /// such desk the layout places leaves it the room, so none loses its tower.
    #[test]
    fn every_back_turned_desk_has_room_for_its_teeter() {
        use crate::layout::{Facing, SceneLayout};
        let pack = test_default_pack();
        let tower = densest_frame(&pack, TOKEN_TOWER_SPRITE, 0, RenderScale::ONE)
            .expect("the tower")
            .frame
            .width();
        let mut desks = 0;
        for (w, h) in [(60, 40), (100, 60), (160, 96), (240, 144), (400, 240)] {
            for seed in 0..4 {
                let Some(layout) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                    continue;
                };
                for &desk in &layout.home_desks {
                    if layout.desk_facing_at(desk) != Facing::North {
                        continue;
                    }
                    desks += 1;
                    let mark = desk_mark(&pack, desk, Facing::North, TOWER_MARK).expect("a mark");
                    assert!(mark.mirrored, "the back-turned desk mirrors its props");
                    assert!(
                        mark.left(tower).is_some(),
                        "{w}x{h} seed {seed}: the desk at {desk:?} has no room for its tower"
                    );
                }
            }
        }
        assert!(desks > 0, "the sample must place back-turned desks");
    }

    /// The pack authors' guide names the desk's contract by the keys and marks
    /// the painters read: a rename here fails until the guide follows.
    #[test]
    fn the_guide_names_the_desk_contract() {
        // Read at runtime: the extracted crate may ship without its guide.
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/AGENTS.md"));
        let Ok(guide) = std::fs::read_to_string(path) else {
            return;
        };
        for needle in [
            format!("@mark {CUP_MARK} <x> <y>"),
            format!("@mark {TOWER_MARK} <x> <y>"),
            format!("palette key `{DESK_BULB_KEY}`"),
        ] {
            assert!(guide.contains(&needle), "the sprite format lost {needle:?}");
        }
    }

    /// A creature's walk steps by the ground like a person's, so a pack's
    /// without a stride slides its feet too.
    #[test]
    fn a_creature_walk_without_a_stride_is_flagged() {
        for (walk, stride) in [("cat_walk", 2), ("lobster_walk", 6)] {
            // its own table, header through stride: a build without
            // `cutaway-assets` drops the density variant's after it
            let line = format!("stride   = {stride}\n");
            let from = BUNDLED_PACK_TOML
                .find(&format!("[animations.{walk}]\n"))
                .expect("the manifest declares the walk");
            let to = from
                + BUNDLED_PACK_TOML[from..]
                    .find(&line)
                    .expect("with its stride")
                + line.len();
            let table = &BUNDLED_PACK_TOML[from..to];
            let pack = test_pack_declaring(table, &table.replacen(&line, "", 1));
            assert_eq!(validate_pack(&pack).walks_without_stride, [walk]);
        }
    }

    /// `build.rs` embeds every sprite in `sprites/default/`, so one no animation,
    /// hairstyle or building registers — a generated file whose `pack.toml`
    /// entry was never written — ships as dead bytes and draws nothing. A sprite
    /// the pack still loads without is exactly that.
    #[test]
    fn every_bundled_sprite_is_a_frame_the_pack_loads() {
        let unregistered = undrawn(BUNDLED_PACK_TOML, &bundled_sprite_srcs());
        assert!(
            unregistered.is_empty(),
            "sprites pack.toml never registers: {unregistered:?}"
        );
    }

    /// The sprites `toml` loads without: the ones nothing it registers draws.
    fn undrawn<'a>(toml: &str, srcs: &[(&'a str, &'a str)]) -> Vec<&'a str> {
        let read: std::collections::HashSet<String> =
            pixtuoid_core::sprite::format::frames_read_by(toml, srcs)
                .expect("the whole set loads")
                .into_iter()
                .collect();
        srcs.iter()
            .map(|&(name, _)| name)
            .filter(|&name| !read.contains(name))
            .collect()
    }

    /// The web hero's pack. Nothing runs this suite without `cutaway-assets`
    /// (`just hack` only checks), so this is the one place it is loaded.
    #[test]
    fn the_pack_without_density_art_loads_whole() {
        let (toml, dropped) = density_art::bundled_without_density_art(include_str!(
            "../../sprites/default/pack.toml"
        ));
        assert!(!dropped.is_empty(), "the bundled pack ships density art");
        let srcs: Vec<_> = bundled_sprite_srcs()
            .into_iter()
            .filter(|(name, _)| !dropped.contains(*name))
            .collect();
        let pack = load_pack_from_strings(&toml, &srcs).expect("loads without density art");
        assert_eq!(pack.max_density_variant(), Density::ONE);
        assert!(
            pack.buildings().next().is_some(),
            "the city keeps its buildings"
        );
        let d = |n| Density::new(n).expect("nonzero");
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
    #[cfg(feature = "cutaway-assets")]
    fn the_bundled_pack_is_drawn_at_most_at_4x() {
        assert_eq!(test_default_pack().max_density_variant().get(), 4);
    }

    /// Each bundled building ships a variant at the bundled art's density,
    /// beside the base the classic painter draws, for a city drawn on that
    /// art's grid.
    #[test]
    #[cfg(feature = "cutaway-assets")]
    fn every_bundled_building_is_drawn_at_1x_and_4x() {
        let pack = test_default_pack();
        assert!(
            pack.buildings().next().is_some(),
            "the bundled pack draws a city"
        );
        let d = |n| Density::new(n).expect("nonzero");
        for b in pack.buildings() {
            assert!(b.variant(d(4)).is_some(), "{}", b.name());
        }
    }

    /// Core's creature list is exactly the animations the pets and the gateway
    /// mascots draw, so neither can gain a creature the other misses.
    #[test]
    fn the_creature_animations_are_the_pets_and_mascots() {
        let drawn: std::collections::BTreeSet<&str> = crate::pet::PetKind::ALL
            .iter()
            .flat_map(|k| [k.walk_anim(), k.sit_anim(), k.sleep_anim()])
            .chain(
                pixtuoid_core::source::registry::registered_source_names()
                    .filter_map(crate::creatures::gateway_mascot_def)
                    .flat_map(|m| [m.walk, m.rest]),
            )
            .collect();
        let listed: std::collections::BTreeSet<&str> =
            pixtuoid_core::sprite::format::OPTIONAL_CREATURE_ANIMATIONS
                .iter()
                .copied()
                .collect();
        assert_eq!(drawn, listed);
    }

    /// A transparent last row lifts a piece off the floor both painters ground
    /// it on.
    #[test]
    fn every_bundled_furniture_frame_draws_its_bottom_row() {
        let pack = test_default_pack();
        let floating: Vec<String> = pack
            .animation_names()
            .into_iter()
            .filter(|name| {
                let base = name.split('@').next().unwrap_or(name);
                pixtuoid_core::sprite::format::OPTIONAL_FURNITURE_ANIMATIONS.contains(&base)
                    && !pixtuoid_core::sprite::format::OVERLAY_PIECES
                        .iter()
                        .any(|&(overlay, _)| overlay == base)
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
    fn bundled_default_pack_animations_are_all_in_the_registry() {
        // An animation the BUNDLED pack ships but the registry doesn't know is
        // falsely reported "unused by renderer" by validation.
        let pack = test_default_pack();
        let report = validate_pack(&pack);
        assert!(
            report.unknown.is_empty(),
            "bundled animation missing from the registry: {:?}",
            report.unknown
        );
    }

    #[test]
    fn character_sprite_size_matches_the_bundled_pack() {
        let pack = test_default_pack();
        let frame = pack
            .animation("standing")
            .and_then(|a| a.frames().first())
            .expect("bundled pack carries a standing pose");
        let (w, h) = (frame.width(), frame.height());
        assert_eq!(
            w,
            crate::layout::CHARACTER_SPRITE_W,
            "bundled 'standing' sprite is {w}px wide but CHARACTER_SPRITE_W is {} — \
             update the const so hit-test/decor/label geometry tracks the pack",
            crate::layout::CHARACTER_SPRITE_W
        );
        assert_eq!(
            h,
            crate::layout::CHARACTER_SPRITE_H,
            "bundled 'standing' sprite is {h}px tall but CHARACTER_SPRITE_H is {} — \
             update the const so the hit-test box and the painter's fallback track the pack",
            crate::layout::CHARACTER_SPRITE_H
        );
    }

    /// A facing does not change how big a desk IS: `desk_north` is taller only above `desk.y`,
    /// and below it the same desk mirrored, as its arrangement mirrors the lamp. Checked on the
    /// edge COLUMNS the monitor never covers — the middle legitimately differs.
    #[test]
    fn both_desk_variants_are_the_same_desk_below_the_monitor() {
        let pack = test_default_pack();
        let frame = |n: &str| {
            pack.animation(n)
                .and_then(|a| a.frames().first())
                .unwrap_or_else(|| panic!("the bundled pack ships {n}"))
        };
        let (base, north) = (frame("desk"), frame("desk_north"));
        assert_eq!(base.width(), north.width(), "a facing never changes width");
        // Both blit so their BOTTOM rows coincide, so the taller one's extra rows are all above.
        let lift = north
            .height()
            .checked_sub(base.height())
            .expect("the raised variant is the taller one");
        let raise = crate::pack::DESK_BEZEL_RAISE;
        let edges = [0, 1, base.width() - 2, base.width() - 1];
        for x in edges {
            for dy in 0..(base.height() - raise) {
                let b = base.get(x, raise + dy);
                let n = north.get(base.width() - 1 - x, raise + lift + dy);
                assert_eq!(
                    b, n,
                    "column {x} differs at desk.y+{dy}: the two variants must be \
                     the same desk, mirrored, below the monitor"
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
        let raise = crate::pack::DESK_BEZEL_RAISE;
        let base_h = raise + DESK_SURFACE_ROWS + DESK_FRONT_ROWS + DESK_LEG_ROWS;
        for name in ["desk", "desk_north"] {
            let f = pack
                .animation(name)
                .and_then(|a| a.frames().first())
                .unwrap_or_else(|| panic!("the bundled pack ships {name}"));
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
            .expect("bundled pack carries a desk sprite")
            .width();
        assert_eq!(
            w,
            crate::layout::desk_furniture_def().visual.w,
            "bundled 'desk' sprite is {w}px wide but visual.w is {} — \
             a DESK_W edit moved visual.w but not scripts/gen-art.py's DESK_ART_W; render/mask/z-sort will drift",
            crate::layout::desk_furniture_def().visual.w
        );
    }
}
