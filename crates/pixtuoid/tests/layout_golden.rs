//! Layout geometry golden snapshots.
//!
//! Snapshots the key fields of `SceneLayout` at fixed buffer sizes so any
//! refactor to `compute` / `compute_with_seed` that accidentally shifts desk
//! positions, room bounds, or waypoint coords will fail immediately.
//!
//! Regenerating after an INTENTIONAL layout change: run the suite to produce
//! `*.snap.new`, eyeball the diff, then `cargo insta accept` (or
//! `INSTA_UPDATE=always cargo test -p pixtuoid --test layout_golden`). The same
//! applies to `walkable_golden.rs`.

use pixtuoid_scene::layout::SceneLayout;

// ── Standard 96×72, seed 0 ──────────────────────────────────────────

#[test]
fn layout_standard_96x72_desks() {
    let l = SceneLayout::compute(96, 72, Some(4)).unwrap();
    insta::assert_debug_snapshot!("desks_96x72", l.home_desks);
}

#[test]
fn layout_standard_96x72_waypoints() {
    let l = SceneLayout::compute(96, 72, Some(4)).unwrap();
    insta::assert_debug_snapshot!("waypoints_96x72", l.waypoints);
}

#[test]
fn layout_standard_96x72_meeting() {
    let l = SceneLayout::compute(96, 72, Some(4)).unwrap();
    insta::assert_debug_snapshot!("meeting_96x72", l.meeting_room_bounds(0));
}

#[test]
fn layout_standard_96x72_room_walls() {
    let l = SceneLayout::compute(96, 72, Some(4)).unwrap();
    insta::assert_debug_snapshot!("room_walls_96x72", l.room_walls);
}

#[test]
fn layout_standard_96x72_zones() {
    let l = SceneLayout::compute(96, 72, Some(4)).unwrap();
    insta::assert_debug_snapshot!("cubicle_band_96x72", l.cubicle_band);
    insta::assert_debug_snapshot!("cubicle_aisle_96x72", l.cubicle_aisle);
    insta::assert_debug_snapshot!("pantry_96x72", l.pantry.map(|p| p.bounds));
}

// ── Larger terminal 192×160, 8 desks, seed 0 ────────────────────────

#[test]
fn layout_standard_192x160_desks() {
    let l = SceneLayout::compute(192, 160, Some(8)).unwrap();
    insta::assert_debug_snapshot!("desks_192x160", l.home_desks);
}

#[test]
fn layout_standard_192x160_waypoints() {
    let l = SceneLayout::compute(192, 160, Some(8)).unwrap();
    insta::assert_debug_snapshot!("waypoints_192x160", l.waypoints);
}

#[test]
fn layout_standard_192x160_meeting() {
    let l = SceneLayout::compute(192, 160, Some(8)).unwrap();
    insta::assert_debug_snapshot!("meeting_192x160", l.meeting_room_bounds(0));
}

#[test]
fn layout_standard_192x160_room_walls() {
    let l = SceneLayout::compute(192, 160, Some(8)).unwrap();
    insta::assert_debug_snapshot!("room_walls_192x160", l.room_walls);
}

// ── Open plan: seed 2 (no meeting room, open pantry) ────────────────
// Seed 2 hashes to floor_variant=1 after the Fibonacci decorrelation.

#[test]
fn layout_open_plan_seed2_desks() {
    let l = SceneLayout::compute_with_seed(160, 120, Some(4), 2).unwrap();
    insta::assert_debug_snapshot!("desks_open_plan_seed2", l.home_desks);
}

#[test]
fn layout_open_plan_seed2_waypoints() {
    let l = SceneLayout::compute_with_seed(160, 120, Some(4), 2).unwrap();
    insta::assert_debug_snapshot!("waypoints_open_plan_seed2", l.waypoints);
}

#[test]
fn layout_open_plan_seed2_meeting() {
    let l = SceneLayout::compute_with_seed(160, 120, Some(4), 2).unwrap();
    insta::assert_debug_snapshot!("meeting_open_plan_seed2", l.meeting_room_bounds(0));
}

#[test]
fn layout_open_plan_seed2_room_walls() {
    let l = SceneLayout::compute_with_seed(160, 120, Some(4), 2).unwrap();
    insta::assert_debug_snapshot!("room_walls_open_plan_seed2", l.room_walls);
}

// ── Dense layout: seed 6 (dual meeting rooms if tall enough) ────────
// Seed 6 hashes to floor_variant=2 after the Fibonacci decorrelation.

#[test]
fn layout_dense_seed6_desks() {
    let l = SceneLayout::compute_with_seed(192, 160, Some(4), 6).unwrap();
    insta::assert_debug_snapshot!("desks_dense_seed6", l.home_desks);
}

#[test]
fn layout_dense_seed6_waypoints() {
    let l = SceneLayout::compute_with_seed(192, 160, Some(4), 6).unwrap();
    insta::assert_debug_snapshot!("waypoints_dense_seed6", l.waypoints);
}

#[test]
fn layout_dense_seed6_meeting() {
    let l = SceneLayout::compute_with_seed(192, 160, Some(4), 6).unwrap();
    insta::assert_debug_snapshot!("meeting_dense_seed6", l.meeting_room_bounds(0));
}

#[test]
fn layout_dense_seed6_room_walls() {
    let l = SceneLayout::compute_with_seed(192, 160, Some(4), 6).unwrap();
    insta::assert_debug_snapshot!("room_walls_dense_seed6", l.room_walls);
}

// ── Floor variant uniqueness ────────────────────────────────────────
// The 5 standard floor seeds (floor_idx × FLOOR_SEED_MULTIPLIER, for
// floor_idx in 0..5) must each map to a distinct layout variant.
// Collisions cause adjacent floors to look identical — see PR #42.

#[test]
fn floor_variant_hash_gives_unique_layouts_per_floor() {
    use pixtuoid_scene::floor::FLOOR_SEED_MULTIPLIER;
    use std::collections::HashSet;

    let signatures: HashSet<_> = (0u64..5)
        .map(|i| {
            let seed = i.wrapping_mul(FLOOR_SEED_MULTIPLIER);
            let l = SceneLayout::compute_with_seed(192, 160, Some(4), seed).unwrap();
            (l.meeting_rooms.clone(), l.pantry, l.home_desks)
        })
        .collect();

    assert_eq!(
        signatures.len(),
        5,
        "floors 0..5 must produce 5 distinct layout variants, got {} unique",
        signatures.len(),
    );
}
