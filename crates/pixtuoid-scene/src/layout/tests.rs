use super::*;

#[test]
fn kitchen_island_places_on_roomy_pantries_and_refuses_small() {
    // Roomy Senior floor: island + its 4 stands (E/W flank + the two
    // "bartender" slots INSIDE the body — occluded by the counter's y-sort)
    // all inside the pantry, clear of the counter's padded north (the
    // anti-merge routing line).
    let l = SceneLayout::compute_with_seed(240, 160, None, 2).expect("fits");
    let island = l
        .pantry
        .and_then(|p| p.kitchen_island)
        .expect("84-wide Senior pantry hosts the island");
    let pr = l.pantry.map(|p| p.bounds).expect("pantry");
    assert!(island.x >= pr.x && island.x < pr.x + pr.width);
    let stands: Vec<_> = l
        .waypoints
        .iter()
        .filter(|w| matches!(w.kind, WaypointKind::Island))
        .collect();
    assert_eq!(stands.len(), 4, "E + W flanks + 2 bartenders");
    for s in &stands {
        assert!(
            s.pos.x >= pr.x && s.pos.x < pr.x + pr.width && s.pos.y >= pr.y,
            "stand {:?} must stay in the pantry",
            s.pos
        );
    }
    // The bartender pair stands ON the island's center row (feet inside the
    // body, so the counter occludes their legs), at the quarter points —
    // 8px-wide sprites at ±w/4 on a 20px island can't overlap each other.
    let mut bartenders: Vec<Point> = stands
        .iter()
        .filter(|s| s.facing == Facing::South)
        .map(|s| s.pos)
        .collect();
    bartenders.sort_by_key(|p| p.x);
    let quarter = furniture_def(Furniture::KitchenIsland).visual.w / 4;
    assert_eq!(
        bartenders,
        vec![
            Point {
                x: island.x - quarter,
                y: island.y,
            },
            Point {
                x: island.x + quarter,
                y: island.y,
            },
        ],
        "two South-facing bartender slots at the island's quarter points"
    );
    // Small Standard floor: the 26-wide pantry can't host the island +
    // stands + clearances (needs ≥42 = 2·(clr + stand_dx)) — refuse, don't
    // force (and no stray stand waypoints).
    let s = SceneLayout::compute_with_seed(96, 70, None, 0).expect("fits");
    assert_eq!(
        s.pantry.and_then(|p| p.kitchen_island),
        None,
        "26-wide pantry refuses the island"
    );
    assert!(
        !s.waypoints
            .iter()
            .any(|w| matches!(w.kind, WaypointKind::Island)),
        "no island ⇒ no island stands"
    );
}

#[test]
fn meeting_room_donates_surplus_height_to_the_pantry() {
    // Short standard floor (≈ a 50-row terminal, the live report that drove
    // this): under the old unconditional half-split the pantry got ~35 rows
    // < its content height, so the island AND snack shelf y-refused while
    // the meeting trio floated in empty floor. The content-fit split must
    // free exactly the rows the pantry needs — this is the drift guard
    // pinning `pantry_content_h`'s inverse-clamp math to the island block's
    // real clamps (if either side changes alone, the island vanishes here).
    let l = SceneLayout::compute_with_seed(215, 98, None, 0).expect("fits");
    assert!(
        l.pantry.and_then(|p| p.kitchen_island).is_some(),
        "island must place on the short floor after the height transfer"
    );
    assert!(
        l.waypoints
            .iter()
            .any(|w| matches!(w.kind, WaypointKind::SnackShelf)),
        "snack shelf must place too (its y-bound == the island's)"
    );
    let mr = l.meeting_room_bounds(0).expect("meeting room");
    let pr = l.pantry.map(|p| p.bounds).expect("pantry");
    assert!(
        mr.height < pr.height,
        "the surplus flowed south: meeting {} !< pantry {}",
        mr.height,
        pr.height
    );
    // Tall floors sit at the ceiling of the clamp (the old half-split) —
    // their geometry is unchanged by the transfer.
    let tall = SceneLayout::compute_with_seed(240, 160, None, 0).expect("fits");
    let (tmr, tpr) = (
        tall.meeting_room_bounds(0).expect("meeting room"),
        tall.pantry.map(|p| p.bounds).expect("pantry"),
    );
    let usable = tmr.height + tpr.height;
    assert_eq!(
        tmr.height,
        usable / 2,
        "tall floors keep the old half-split exactly"
    );
    // Floor arm: just below the donation floor (donated < trio fit) the old
    // half-split stands and the starved pantry refuses the island — pins the
    // all-or-nothing rule from the other side. (The pin is deliberately
    // asymmetric: if the island's clamps RELAX while pantry_content_h stays,
    // the split merely over-donates a benign row or two — only the
    // starve/rescue boundary is load-bearing.)
    let short = SceneLayout::compute_with_seed(215, 87, None, 0).expect("fits");
    assert_eq!(
        short.pantry.and_then(|p| p.kitchen_island),
        None,
        "below the donation floor the pantry stays starved (no forced cram)"
    );
    let (smr, spr) = (
        short.meeting_room_bounds(0).expect("meeting room"),
        short.pantry.map(|p| p.bounds).expect("pantry"),
    );
    assert_eq!(
        smr.height,
        (smr.height + spr.height) / 2,
        "below the floor the half-split stands"
    );
}

#[test]
fn island_bartenders_approach_from_behind_never_through_the_front() {
    // The bartender slots sit INSIDE the island body (blocked cells) — the
    // couch-seat pattern: A* routes to a walkable approach cell, the settle
    // glide bridges onto the slot. The approach must be REAL (not the "no
    // valid approach" pos sentinel, which would silently demote every
    // bartender trip to an aimless amble) and must never be SOUTH of the
    // body: a south approach glides visibly THROUGH the counter's front
    // face. Behind (north) and lateral glides stay behind the countertop
    // for the whole settle (the glide z is pinned to the feet row).
    // 240×160 = the roomy tall floor; 215×98 = the donated content-fit
    // floor where the island sits at its single valid y (tightest lane).
    let mut exercised = false;
    for (bw, bh) in [(240u16, 160u16), (215, 98)] {
        for seed in 0..5u64 {
            let l = SceneLayout::compute_with_seed(bw, bh, None, seed).expect("fits");
            let Some(island) = l.pantry.and_then(|p| p.kitchen_island) else {
                continue;
            };
            exercised = true;
            let origin = l.home_desks.first().copied().expect("desks");
            for wp in l
                .waypoints
                .iter()
                .filter(|w| matches!(w.kind, WaypointKind::Island) && w.facing == Facing::South)
            {
                let a = approach_point(
                    wp.kind.furniture(),
                    wp.pos,
                    wp.facing,
                    l.pantry_counter_size(),
                    &l.walkable,
                    origin,
                    &l.reachable,
                );
                assert_ne!(
                    a, wp.pos,
                    "{bw}x{bh} seed {seed}: bartender slot has no approach"
                );
                assert!(
                    a.y <= island.y,
                    "{bw}x{bh} seed {seed}: approach {a:?} is south of the island \
                     center — the glide would cross the counter's front face"
                );
            }
        }
    }
    assert!(
        exercised,
        "no size/seed hosted the island — test lost its teeth"
    );
}

#[test]
fn snack_shelf_hugs_the_west_wall_and_refuses_narrow_rooms() {
    // Roomy floor: one shelf waypoint against the west wall (the east bridge
    // must stay open), its ground inside the room.
    let l = SceneLayout::compute_with_seed(240, 160, None, 2).expect("fits");
    let pr = l.pantry.map(|p| p.bounds).expect("pantry");
    let shelf = l
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, WaypointKind::SnackShelf))
        .expect("roomy pantry hosts the shelf");
    let vis = furniture_def(Furniture::SnackShelf).visual;
    assert_eq!(shelf.pos.x, pr.x + 1 + vis.w / 2, "west-wall hug");
    // Narrow room (36-wide buffer ⇒ 6-7px pantry): refuse.
    let s = SceneLayout::compute_with_seed(36, 100, None, 1).expect("fits");
    assert!(
        !s.waypoints
            .iter()
            .any(|w| matches!(w.kind, WaypointKind::SnackShelf)),
        "a 6px room refuses the 7px shelf"
    );
}

#[test]
fn dense_inter_meeting_wall_is_solid_with_a_corridor_door_each() {
    // #557 door policy (owner call): two stacked meeting rooms do NOT
    // interconnect — their shared wall renders as ONE solid segment — while
    // each room keeps its own centered corridor door in the east wall (the
    // connectivity the sweep's BFS pins). The golden named "dense_seed6" is
    // actually a Senior floor (from_seed(6)), so this REAL dual-floor pin
    // lives here instead of a snapshot.
    let mut saw_dual = false;
    for seed in 0..10u64 {
        let l = SceneLayout::compute_with_seed(192, 160, Some(8), seed).expect("fits");
        if l.meeting_rooms.len() < 2 {
            continue;
        }
        saw_dual = true;
        let split_y = l.meeting_rooms[1].bounds.y;
        let h: Vec<_> = l
            .room_walls
            .iter()
            .filter(|w| w.start.y == split_y && w.end.y == split_y)
            .collect();
        assert_eq!(h.len(), 1, "seed {seed}: ONE solid shared wall, got {h:?}");
        assert_eq!(
            (h[0].start.x, h[0].end.x),
            (0, l.meeting_rooms[1].bounds.width),
            "seed {seed}: no inter-meeting door gap"
        );
        // Each room's east wall still carries a real (non-degenerate) gap.
        for (id, room) in l.meeting_rooms.iter().enumerate() {
            let b = room.bounds;
            let vx = b.x + b.width;
            let mut v: Vec<_> = l
                .room_walls
                .iter()
                .filter(|w| {
                    w.start.x == vx
                        && w.end.x == vx
                        && w.start.y >= b.y
                        && w.end.y <= b.y + b.height
                })
                .collect();
            v.sort_by_key(|w| w.start.y);
            assert_eq!(
                v.len(),
                2,
                "seed {seed} room {id}: east wall split by its door"
            );
            assert!(
                v[0].end.y < v[1].start.y,
                "seed {seed} room {id}: the corridor door gap must be real"
            );
        }
    }
    assert!(saw_dual, "192x160 seeds 0..10 must reach a dual floor");
}

#[test]
fn meeting_rooms_vec_indexes_are_the_waypoint_room_ids() {
    // The join-key pin (#557): a room's index in `meeting_rooms` IS the
    // `room_id` its waypoints carry — bounds and trio live in ONE element,
    // so a bare room keeps its slot and the id can never shift (the old
    // compacted `meeting_furniture` Vec could mis-join if a bare room 0 sat
    // above a fitted room 1; latent then, unrepresentable now). Also pins
    // room-1 furniture containment on the dual-meeting Dense floor.
    let mut saw_dual = false;
    for seed in 0..10u64 {
        let l = SceneLayout::compute_with_seed(192, 160, Some(8), seed).expect("fits");
        assert_eq!(
            l.meeting_room_bounds(l.meeting_rooms.len()),
            None,
            "no room past the Vec"
        );
        // Every meeting waypoint's room_id joins to a room whose bounds
        // CONTAIN it — the definition of the id being the Vec index.
        for wp in l.waypoints.iter().filter(|w| {
            matches!(
                w.kind,
                WaypointKind::MeetingSofa | WaypointKind::MeetingChair
            )
        }) {
            let id = wp.room_id.expect("meeting slots carry a room_id");
            let b = l
                .meeting_room_bounds(id)
                .unwrap_or_else(|| panic!("seed {seed}: room_id {id} has no room"));
            assert!(
                l.meeting_rooms[id].trio.is_some(),
                "seed {seed}: room {id} emits waypoints, so it must host its trio"
            );
            assert!(
                wp.pos.x >= b.x
                    && wp.pos.x < b.x + b.width
                    && wp.pos.y >= b.y
                    && wp.pos.y < b.y + b.height,
                "seed {seed}: waypoint {:?} (room {id}) outside its room {b:?}",
                wp.pos
            );
        }
        if l.meeting_rooms.len() == 2 {
            saw_dual = true;
            let r2 = l.meeting_rooms[1].bounds;
            if let Some(mf) = l.meeting_rooms[1].trio {
                for p in mf.sofas.iter().chain([&mf.table]) {
                    assert!(
                        p.x >= r2.x
                            && p.x < r2.x + r2.width
                            && p.y >= r2.y
                            && p.y < r2.y + r2.height,
                        "seed {seed}: room-1 furniture {p:?} must sit inside its own room {r2:?}"
                    );
                }
            }
        }
    }
    assert!(
        saw_dual,
        "192x160 x seeds 0..10 must reach a dual-meeting Dense floor"
    );
}

#[test]
fn desk_is_walk_behind_the_monitor() {
    // ground_y: End south-anchors the shallow DESK_FOOT_H footprint, so the
    // surface + monitor overhang NORTH and those cells stay walkable — a
    // walker passes behind the monitor, occluded by the desk's own y-sort.
    // The desk's north row (its NW corner) must be walkable; its south
    // front-contact row must be blocked.
    let l = SceneLayout::compute(200, 90, Some(64)).expect("fits");
    let vis = crate::layout::desk_furniture_def().visual;
    let foot_h = crate::layout::DESK_FOOT_H;
    for &d in &l.home_desks {
        let cx = d.x + vis.w / 2;
        // North (monitor overhang) walkable — the walk-behind lane.
        assert!(
            l.walkable.is_walkable(cx, d.y),
            "desk {d:?}: north row (monitor overhang) must be walkable (walk-behind)"
        );
        // South front-contact row blocked (the footprint's south base).
        let south = d.y + vis.h.saturating_sub(1);
        assert!(
            !l.walkable.is_walkable(cx, south.saturating_sub(foot_h / 2)),
            "desk {d:?}: front-contact ground must be blocked"
        );
    }
}

#[test]
fn home_desk_typed_accessor_matches_raw_vec() {
    let l = SceneLayout::compute(160, 200, Some(4)).expect("layout fits");
    assert!(!l.home_desks.is_empty());
    for i in 0..l.home_desks.len() {
        assert_eq!(
            l.home_desk(FloorLocalDeskIndex(i)),
            Some(l.home_desks[i]),
            "typed accessor must agree with the raw vec at index {i}"
        );
    }
    assert_eq!(
        l.home_desk(FloorLocalDeskIndex(l.home_desks.len())),
        None,
        "out-of-range floor-local index returns None"
    );
}

#[test]
fn partial_bottom_row_caps_mid_fill_when_agents_run_out() {
    // Covers the partial-BOTTOM-ROW capacity break in `compute_pod_desks`
    // (compute.rs `'partial_y` loop): a layout whose fill needs the partial
    // bottom row to reach `cap`, fed `num_agents` that runs out PART-WAY through
    // that row. The break must fire after the first partial-row desk, leaving
    // `home_desks.len() == num_agents` (NOT the full row). The earlier full-pod
    // `break 'outer` (small num_agents) caps before this phase, so it never
    // exercises these lines — `num_agents` must be tuned to the grid (here
    // `cap - 1`, one short of filling the 2-desk partial row).
    //
    // Sizes chosen empirically (each has a 2-desk partial bottom row whose first
    // desk appears at num_agents = cap-1 and second at cap, so cap-1 breaks the
    // 'partial_y loop mid-row). Driven through the public compute path (the
    // private PodGrid has no constructor — see the cov verdict).
    for (w, h, cap) in [(88u16, 108u16, 6usize), (88, 120, 8), (88, 175, 10)] {
        // Total capacity at this size is exactly `cap`.
        let full = SceneLayout::compute_with_seed(w, h, Some(TEST_DEFAULT_DESKS), 0).expect("fits");
        assert_eq!(
            full.home_desks.len(),
            cap,
            "{w}x{h}: expected total desk capacity {cap}"
        );
        let max_y = full.home_desks.iter().map(|d| d.y).max().unwrap();
        let band_at = |n: usize| {
            SceneLayout::compute_with_seed(w, h, Some(n), 0)
                .expect("fits")
                .home_desks
                .iter()
                .filter(|d| d.y == max_y)
                .count()
        };
        // Exact truncation: asking for n agents (n ≤ cap) yields exactly n desks,
        // so the cap break fires in whichever phase the count lands in.
        for n in 1..=cap {
            assert_eq!(
                SceneLayout::compute_with_seed(w, h, Some(n), 0)
                    .expect("fits")
                    .home_desks
                    .len(),
                n,
                "{w}x{h}: num_agents {n} must truncate to exactly {n} desks"
            );
        }
        // The partial bottom row fills incrementally: empty at cap-2, one desk at
        // cap-1 (the 'partial_y push ran, then the break fired), full at cap. This
        // is the proof the break executed INSIDE the partial-row loop.
        assert_eq!(
            band_at(cap),
            2,
            "{w}x{h}: partial bottom row seats 2 at cap"
        );
        assert_eq!(
            band_at(cap - 1),
            1,
            "{w}x{h}: cap-1 leaves the partial row half-filled (break mid-row)"
        );
    }
}

#[test]
fn compute_returns_none_when_buf_too_small() {
    assert!(SceneLayout::compute(20, 20, Some(4)).is_none());
}

#[test]
fn every_role_enum_variant_maps_to_a_furniture_row() {
    // Each role enum (WallDecor, PlantKind) maps onto exactly one Furniture
    // geometry row via `.furniture()`. The golden seeds never place
    // WallDecor::BulletinBoard or PlantKind::Ficus, so their `.furniture()` arms
    // are otherwise uncovered; this exhaustive sweep (mirrors the
    // `sprite_name()` registry test) maps every variant and confirms each
    // resolves a real Furniture row, doubling as a guard that a new variant
    // can't ship without a mapping.
    for wd in [
        WallDecor::Bookshelf,
        WallDecor::Whiteboard,
        WallDecor::BulletinBoard,
        WallDecor::ExitSign,
        WallDecor::MeetingScreen,
    ] {
        let f = wd.furniture();
        // The mapped row must exist in the unified table (visual non-degenerate).
        assert!(
            furniture_def(f).visual.w > 0 && furniture_def(f).visual.h > 0,
            "{wd:?} → {f:?} must resolve a sized Furniture row"
        );
    }
    for pk in [
        PlantKind::Ficus,
        PlantKind::Tall,
        PlantKind::Flower,
        PlantKind::Succulent,
    ] {
        let f = pk.furniture();
        // Every plant resolves a Furniture row with a (shallow, overhung)
        // ground footprint and a sized canopy visual. Ficus/Tall share the
        // PLANT_FOOTPRINT; Flower/Succulent are de-shared (smaller pot strips).
        let def = furniture_def(f);
        assert!(
            def.footprint.is_some(),
            "{pk:?} → {f:?} must have a ground footprint"
        );
        assert!(
            def.visual.w > 0 && def.visual.h > 0,
            "{pk:?} → {f:?} must have a sized visual"
        );
    }
}

// Regression: the percentage math (`buf_h * 30`, `buf_w * 35`, …) used bare
// u16 multiplies that overflow once a dimension exceeds ~1872–2184. On an
// absurdly large terminal a debug build PANICKED (overflow check) and release
// silently WRAPPED to a garbage layout. pct() now computes in u32.
#[test]
fn compute_does_not_overflow_on_huge_terminal() {
    for &seed in &[0u64, 1, 2, 3, 4] {
        // 4000×4000 px buffer → buf_h*30 = 120_000, well past u16::MAX.
        let l = SceneLayout::compute_with_seed(4000, 4000, Some(TEST_DEFAULT_DESKS), seed);
        assert!(
            l.is_some(),
            "huge terminal (seed {seed}) must lay out, not overflow"
        );
    }
}

#[test]
fn none_fills_desks_past_the_old_cap_on_a_large_buffer() {
    // The office is no longer hard-capped at TEST_DEFAULT_DESKS: a buffer that
    // physically fits more desks is a fuller office (the web hero + big
    // terminals). `None` = fill to the room's true capacity.
    let l = SceneLayout::compute_with_seed(800, 500, None, 0).expect("large buffer lays out");
    assert!(
        l.home_desks.len() > TEST_DEFAULT_DESKS,
        "None must fill past the old {TEST_DEFAULT_DESKS}-desk cap, got {}",
        l.home_desks.len()
    );
}

#[test]
fn compute_returns_none_at_exact_boundary() {
    let min_w = crate::layout::compute::MIN_LAYOUT_W;
    let min_h = crate::layout::compute::MIN_LAYOUT_H;
    assert!(
        SceneLayout::compute(min_w - 1, min_h, Some(1)).is_none(),
        "one pixel below MIN_LAYOUT_W should return None"
    );
    assert!(
        SceneLayout::compute(min_w, min_h - 1, Some(1)).is_none(),
        "one pixel below MIN_LAYOUT_H should return None"
    );
    assert!(
        SceneLayout::compute(min_w, min_h, Some(1)).is_some(),
        "exactly at boundary should return Some"
    );
}

#[test]
fn compute_zones_are_ordered_top_to_bottom_and_nonoverlapping() {
    let l = SceneLayout::compute(120, 80, Some(6)).expect("fits");
    assert!(l.cubicle_band.y < l.cubicle_aisle.y);
    let c_bot = l.cubicle_band.y + l.cubicle_band.height;
    assert!(c_bot <= l.cubicle_aisle.y, "cubicle overlaps cubicle_aisle");
    // Walkway runs to the baseboard now that lounge_band is gone.
    let w_bot = l.cubicle_aisle.y + l.cubicle_aisle.height;
    assert!(w_bot <= l.buf_h);
}

#[test]
fn narrow_width_desks_stay_inside_the_band_with_anchors_on_buffer() {
    // 34-66px-wide buffers force one pod column (`pod_cols` floors at 1) even
    // when the 36px pod doesn't fit. Without an x clamp in `push_desk` the
    // pod's 2nd desk column landed past the band's right edge — even entirely
    // off-buffer (x=47 at buf_w=40) — giving agents invisible desks whose walk
    // anchors sit outside the mask. Mirror of the y clamp: those desks are
    // skipped and the floor degrades to fewer desks (capacity auto-computes
    // from home_desks.len(), so the smaller count IS the floor's capacity).
    for &w in &[40u16, 50, 60] {
        for seed in 0..6u64 {
            let Some(l) = SceneLayout::compute_with_seed(w, 70, Some(8), seed) else {
                continue;
            };
            let band_right = l.cubicle_band.x + l.cubicle_band.width;
            for d in &l.home_desks {
                assert!(
                    d.x >= l.cubicle_band.x,
                    "{w}x70 seed {seed}: desk x={} west of band x={}",
                    d.x,
                    l.cubicle_band.x
                );
                assert!(
                    d.x + DESK_W <= band_right,
                    "{w}x70 seed {seed}: desk x={} overflows the band's right edge {band_right}",
                    d.x
                );
                let a = desk_walk_anchor(*d);
                assert!(
                    a.x < l.buf_w && a.y < l.buf_h,
                    "{w}x70 seed {seed}: desk_walk_anchor {a:?} is off-buffer ({}x{})",
                    l.buf_w,
                    l.buf_h
                );
            }
        }
    }
}

#[test]
fn compute_places_all_waypoint_kinds() {
    let l = SceneLayout::compute(120, 96, Some(1)).expect("fits");
    // Couch + Pantry are unconditional; PhoneBooth / StandingDesk
    // may appear depending on the random pod_decor pick — so just
    // require the unconditional pair and let the rest vary.
    assert!(l.waypoints.len() >= 2);
    let kinds: std::collections::HashSet<_> = l.waypoints.iter().map(|w| w.kind).collect();
    assert!(kinds.contains(&WaypointKind::Couch));
    assert!(kinds.contains(&WaypointKind::Pantry));
    for w in &l.waypoints {
        match w.kind {
            WaypointKind::Pantry => {
                let pr = l.pantry.map(|p| p.bounds).expect("pantry");
                assert!(w.pos.y >= pr.y && w.pos.y < pr.y + pr.height);
                assert!(w.pos.x >= pr.x && w.pos.x < pr.x + pr.width);
            }
            WaypointKind::Couch => {
                assert!(w.pos.y >= l.top_margin);
                assert!(w.pos.y < l.cubicle_band.y + DESK_GAP_Y);
            }
            // PhoneBooth + StandingDesk waypoints come from
            // pod_decor slots in the cubicle band. They're
            // valid anywhere inside the cubicle band — the
            // tighter check just confirms they're south of the
            // top wall.
            // The pantry-redesign kinds all live inside the pantry room.
            WaypointKind::Island | WaypointKind::SnackShelf => {
                let pr = l
                    .pantry
                    .map(|p| p.bounds)
                    .expect("pantry room hosts the new kinds");
                assert!(w.pos.y >= pr.y && w.pos.y < pr.y + pr.height);
                assert!(w.pos.x >= pr.x && w.pos.x < pr.x + pr.width);
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {
                assert!(w.pos.y >= l.top_margin);
            }
            WaypointKind::VendingMachine | WaypointKind::Printer => {
                assert!(w.pos.y >= l.top_margin);
            }
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair => {
                // A meeting slot only exists when a meeting room does, and
                // it carries the room id it belongs to.
                assert!(!l.meeting_rooms.is_empty());
                assert!(w.room_id.is_some());
            }
        }
    }
}

#[test]
fn every_home_desk_has_a_reachable_north_approach() {
    // Back-row pod desks face the front row across the thin INTRA_POD_GAP_Y;
    // the first walkable cell scanning north sits at the gap's south EDGE,
    // whose coarse routing cell straddles the desk → ReachSet-rejected. The
    // reachable-aware deeper scan steps past that edge into the gap interior
    // (which always holds a reachable coarse cell), so EVERY desk — front and
    // back row — gets a north approach. Was ~50% (front row only). Pushing the
    // origin far north makes `approach_point` prefer the north side whenever it
    // has a reachable cell, so a north return proves the scan reached it.
    use crate::layout::{approach_point, desk_walk_anchor, Facing, Furniture};
    for (w, h) in [(192u16, 158u16), (160, 120), (240, 160)] {
        let l = SceneLayout::compute(w, h, Some(64)).expect("fits");
        for &desk in &l.home_desks {
            let chair = desk_walk_anchor(desk);
            let north_origin = Point {
                x: chair.x,
                y: chair.y.saturating_sub(40),
            };
            let a = approach_point(
                Furniture::Desk,
                chair,
                Facing::South,
                l.pantry_counter_size(),
                &l.walkable,
                north_origin,
                &l.reachable,
            );
            assert_ne!(a, chair, "desk {desk:?}: no reachable approach (sentinel)");
            assert!(
                a.y < chair.y,
                "desk {desk:?}: approach {a:?} should be NORTH of the chair {chair:?}"
            );
            assert!(
                l.reachable.reaches(a),
                "desk {desk:?}: approach {a:?} must be A*-reachable"
            );
        }
    }
}

#[test]
fn sofas_seat_three_people() {
    // Both venues seat 3: each meeting sofa (3 seats per sofa) and the
    // lounge couch (was 1 seat → 3). Seats are dx ∈ {-6, 0, +6} on the
    // 20px sprite. The lounge keeps room_id = None — its group-chat
    // grouping happens at the chitchat venue-key layer, not via the
    // meeting-only room_id field.
    // 120 wide so the meeting room clears MEETING_FURNITURE_MIN_W (a 96-wide
    // room is too narrow to route to the sofa seats and is intentionally
    // left bare — see the gate in compute.rs). seed 0 → has_meeting.
    let l = SceneLayout::compute(120, 80, Some(4)).expect("fits");

    let couch: Vec<_> = l
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::Couch)
        .collect();
    assert_eq!(couch.len(), 3, "lounge couch should seat 3");
    assert!(
        couch.iter().all(|w| w.room_id.is_none()),
        "couch keeps room_id None (grouping is at the chitchat layer)"
    );
    let mut xs: Vec<u16> = couch.iter().map(|w| w.pos.x).collect();
    xs.sort_unstable();
    assert_eq!(xs[1] - xs[0], 6, "couch seats are 6px apart");
    assert_eq!(xs[2] - xs[1], 6, "couch seats are 6px apart");
    let center = l.couch_sprite_center.expect("couch sprite center recorded");
    assert_eq!(center.x, xs[1], "sprite center sits on the middle seat");

    // 1 meeting room → 2 sofas (per room) → 3 seats each.
    assert!(!l.meeting_rooms.is_empty(), "expected a meeting room");
    let sofa_seats = l
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::MeetingSofa)
        .count();
    let total_sofas: usize = l
        .meeting_rooms
        .iter()
        .filter_map(|r| r.trio.as_ref())
        .map(|t| t.sofas.len())
        .sum();
    assert_eq!(sofa_seats, 3 * total_sofas, "each meeting sofa seats 3");
}

#[test]
fn meeting_slots_track_meeting_trios() {
    // Across every floor variant, a meeting slot exists iff a meeting
    // room exists, every slot carries a valid room_id, and a dual-meeting
    // floor produces slots for both rooms.
    let mut saw_room = false;
    let mut saw_no_room = false;
    let mut saw_dual = false;
    for seed in 0..40u64 {
        let l = SceneLayout::compute_with_seed(160, 120, Some(8), seed).expect("fits");
        let sofa_slots: Vec<_> = l
            .waypoints
            .iter()
            .filter(|w| {
                matches!(
                    w.kind,
                    WaypointKind::MeetingSofa | WaypointKind::MeetingChair
                )
            })
            .collect();
        if !l.meeting_rooms.is_empty() {
            saw_room = true;
            assert!(
                sofa_slots
                    .iter()
                    .any(|w| w.kind == WaypointKind::MeetingSofa),
                "seed {seed}: meeting room but no sofa slot"
            );
            assert!(
                sofa_slots
                    .iter()
                    .any(|w| w.kind == WaypointKind::MeetingChair),
                "seed {seed}: meeting room but no chair seat"
            );
            let rooms = l.meeting_rooms.len();
            for w in &sofa_slots {
                let rid = w.room_id.expect("meeting slot has room_id");
                assert!(
                    rid < rooms,
                    "seed {seed}: room_id {rid} out of range {rooms}"
                );
            }
            if rooms == 2 {
                saw_dual = true;
                assert!(
                    sofa_slots.iter().any(|w| w.room_id == Some(1)),
                    "seed {seed}: dual meeting but no room-1 slot"
                );
            }
        } else {
            saw_no_room = true;
            assert!(
                sofa_slots.is_empty(),
                "seed {seed}: no meeting room but {} meeting slots",
                sofa_slots.len()
            );
        }
    }
    assert!(saw_room, "no seed produced a meeting room");
    assert!(saw_no_room, "no seed produced a meeting-less floor");
    assert!(saw_dual, "no seed produced a dual-meeting floor");
}

#[test]
fn meeting_table_is_centered_between_its_two_sofas() {
    // The two sofas face each other across the table, so the table must sit
    // vertically EQUIDISTANT from both — each sofa's front (toward the table)
    // then gets equal, routable approach clearance. Room-CENTER placement
    // packed the north sofa's front against the table (a sub-coarse-grid seam
    // that cost its seats their front approach) while the south sofa had room
    // — an asymmetry users spotted as "the south-facing sofa is missing entry
    // points." Sofa/table positions are window-height-driven, so this relative
    // invariant is swept across sizes × seeds, NOT a fixed pixel offset.
    for (w, h) in [(128u16, 80u16), (160, 120), (192, 160), (240, 160)] {
        for seed in 0..8u64 {
            let Some(l) = SceneLayout::compute_with_seed(w, h, Some(8), seed) else {
                continue;
            };
            for (room_id, room) in l
                .meeting_rooms
                .iter()
                .enumerate()
                .filter_map(|(i, r)| r.trio.as_ref().map(|t| (i, t)))
            {
                let table = room.table;
                let north = room.sofas[0];
                let south = room.sofas[1];
                let gap_n = table.y.abs_diff(north.y);
                let gap_s = south.y.abs_diff(table.y);
                assert!(
                    gap_n.abs_diff(gap_s) <= 1,
                    "{w}x{h} seed {seed} room {room_id}: table not centered \
                     between sofas (north gap {gap_n}px, south gap {gap_s}px)"
                );
            }
        }
    }
}

#[test]
fn meeting_slots_face_the_table() {
    // Sofa seats face the table across the room (north seat faces South,
    // south seat faces North); chair seats face inward toward the table
    // centre (west faces East, east faces West). Sofa facing drives front
    // "seated" vs "back_couch"; chair facing drives the painted chair body's
    // backrest side (the occupant always renders front "seated").
    for seed in 0..40u64 {
        let l = SceneLayout::compute_with_seed(160, 120, Some(8), seed).expect("fits");
        for w in &l.waypoints {
            let Some(room_id) = w.room_id else { continue };
            let table = l.meeting_rooms[room_id]
                .trio
                .expect("slot's room hosts a trio")
                .table;
            match w.kind {
                WaypointKind::MeetingSofa => {
                    let want = if w.pos.y < table.y {
                        Facing::South
                    } else {
                        Facing::North
                    };
                    assert_eq!(
                        w.facing, want,
                        "seed {seed}: sofa {:?} vs table {:?}",
                        w.pos, table
                    );
                }
                WaypointKind::MeetingChair => {
                    let want = if w.pos.x < table.x {
                        Facing::East
                    } else {
                        Facing::West
                    };
                    assert_eq!(
                        w.facing, want,
                        "seed {seed}: stand {:?} vs table {:?}",
                        w.pos, table
                    );
                }
                _ => {}
            }
        }
    }
}

// Regression: the WEST MeetingChair point used to land on the table's padded
// obstacle (blocked x ∈ [t.x-8, t.x+7]; the symmetric -8 hit the inclusive
// left edge), so the router had to snap it off-target. Both chair cells must
// stay walkable across seeds/sizes — DELIBERATELY so, unlike the island's
// in-body slots: the chair has no furniture body to sit inside
// (`Furniture::MeetingChair` keeps `footprint: None`, the painted 7×7 chair
// blocks nothing), and blocking the cell would ripple the mask/approach for
// a piece walkers can visually clip at worst. See the scene CLAUDE.md decor
// entry.
#[test]
fn meeting_chair_cells_stay_walkable() {
    for seed in 0..40u64 {
        for (w, h) in [(160u16, 120u16), (200, 100), (240, 140)] {
            let l = SceneLayout::compute_with_seed(w, h, Some(8), seed).expect("fits");
            for wp in &l.waypoints {
                if wp.kind == WaypointKind::MeetingChair {
                    assert!(
                        l.is_walkable(wp.pos.x, wp.pos.y),
                        "seed {seed} @ {w}x{h}: MeetingChair {:?} is non-walkable",
                        wp.pos
                    );
                }
            }
        }
    }
}

#[test]
fn compute_places_bookshelf_on_wall_and_whiteboard_in_walkway() {
    let l = SceneLayout::compute(120, 96, Some(1)).expect("fits");
    let bookshelf = l.wall_decor.iter().find(|i| i.kind == WallDecor::Bookshelf);
    let whiteboard = l
        .wall_decor
        .iter()
        .find(|i| i.kind == WallDecor::Whiteboard);
    assert!(bookshelf.is_some());
    assert!(whiteboard.is_some());
    assert!(bookshelf.unwrap().pos.y < l.cubicle_band.y);
    assert!(whiteboard.unwrap().pos.y > l.cubicle_band.y);
}

#[test]
fn whiteboard_blocks_only_its_wheel_base_not_the_elevated_panel() {
    // The rolling whiteboard's 8-px board panel overhangs its 3-px wheel base
    // (invariant #6): the mask must block ONLY the south wheel strip so a
    // walker can pass BEHIND the panel (occluded by it), not the full 11-px
    // sprite. Was the full height — a walker couldn't get above the board.
    let l = SceneLayout::compute(120, 96, Some(1)).expect("fits");
    let pos = l
        .wall_decor
        .iter()
        .find(|i| i.kind == WallDecor::Whiteboard)
        .expect("a free-standing whiteboard")
        .pos;
    // Wall board is TopLeft-anchored; the 14×11 sprite's wheels sit at rows
    // 8-10. A panel-surface cell well north of the wheels must be WALKABLE.
    assert!(
        l.is_walkable(pos.x + 5, pos.y + 2),
        "the elevated whiteboard panel must NOT block the floor (invariant #6)"
    );
    // A wheel-base cell (the sprite's south rows) must stay BLOCKED.
    assert!(
        !l.is_walkable(pos.x + 5, pos.y + 9),
        "the whiteboard wheel base must block the floor"
    );
}

#[test]
fn fish_tank_sits_east_of_the_lounge_lamp_clear_of_the_elevator() {
    // Decor arc: the aquarium joins the lounge vignette (owner-picked spot —
    // east of the couch's floor lamp, against the north wall band). It rides
    // the SAME lounge gate as lamp/side table, plus an east-clearance gate so
    // it never crowds the elevator door threshold.
    let l = SceneLayout::compute(192, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let lamp = l.floor_lamp.expect("lounge fits at this size");
    let tank = l.fish_tank.expect("tank fits at this size");
    let half_w = furniture_def(Furniture::FishTank).visual.w / 2;
    let lamp_east = lamp.x + (furniture_def(Furniture::FloorLamp).visual.w - 1) / 2;
    assert_eq!(
        tank.x - half_w,
        lamp_east + 2,
        "tank west edge sits exactly the pinned gap past the lamp's east edge"
    );
    let door_west = l.door.expect("elevator fits at this size").x;
    assert!(
        tank.x + half_w + super::compute::FISH_TANK_ELEVATOR_CLEARANCE <= door_west,
        "tank + clearance stays west of the elevator door column"
    );
    // The vignette lives and dies together: never a tank without the couch.
    for (w, h) in [(96u16, 70u16), (120, 80), (150, 68), (215, 98), (240, 160)] {
        let Some(l) = SceneLayout::compute(w, h, Some(TEST_DEFAULT_DESKS)) else {
            continue;
        };
        if l.fish_tank.is_some() {
            assert!(
                l.couch_sprite_center.is_some(),
                "{w}x{h}: tank requires the lounge"
            );
        }
    }
}

#[test]
fn meeting_table_ends_are_chair_seats_not_stands() {
    // Decor arc (owner-ratified): the two table-end spots are SEATS now —
    // same positions/facings as the old stands (E/W flanking the table),
    // renamed MeetingChair with a seated render + a painted chair body.
    let l = SceneLayout::compute(192, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let trio = l.meeting_rooms[0].trio.as_ref().expect("trio");
    let chairs: Vec<_> = l
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::MeetingChair)
        .collect();
    assert_eq!(chairs.len(), 2, "one chair per table end");
    let (west, east) = (
        chairs
            .iter()
            .find(|w| w.pos.x < trio.table.x)
            .expect("west chair"),
        chairs
            .iter()
            .find(|w| w.pos.x > trio.table.x)
            .expect("east chair"),
    );
    assert_eq!(west.facing, Facing::East, "west chair faces the table");
    assert_eq!(east.facing, Facing::West, "east chair faces the table");
    assert_eq!(west.pos.y, trio.table.y, "chairs sit on the table's row");
    // Owner catch (PR #561): the stands-era offsets were -9/+8, so the east
    // chair body sat 1px closer to the table wood and swallowed the rug
    // border its west twin showed. Chairs must MIRROR around the table.
    assert_eq!(
        trio.table.x - west.pos.x,
        east.pos.x - trio.table.x,
        "chair offsets mirror around the table center"
    );
}

#[test]
fn coat_rack_yields_to_the_east_chair_in_narrow_fitted_rooms() {
    // Arc-final audit catch (PR #561): on fitted rooms ≲40 wide the rack's
    // coats (west reach cx−2) overprinted the east chair body + its sitter.
    // The rack yields in that geometry; bare rooms and roomy floors keep it.
    let narrow = SceneLayout::compute(120, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let r = &narrow.meeting_rooms[0];
    assert!(r.trio.is_some(), "120x160 hosts a fitted room");
    assert_eq!(
        r.coat_rack_pos(),
        None,
        "narrow fitted room drops the rack instead of interpenetrating the chair"
    );
    let roomy = SceneLayout::compute(192, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    assert!(
        roomy.meeting_rooms[0].coat_rack_pos().is_some(),
        "roomy fitted room keeps the rack"
    );
}

#[test]
fn pod_grid_fills_every_desk_row_that_fits() {
    // #552: main_pod_used_h counted a phantom trailing aisle below the last
    // pod row, starving residual_h so a bottom row that physically fits never
    // fired. Invariant, from outputs only: a whole extra desk row (one full
    // row gap below the lowest row) must NOT fit inside the band.
    for w in [140u16, 160, 192, 215, 240] {
        for h in (100..=240u16).step_by(10) {
            let Some(l) = SceneLayout::compute(w, h, Some(64)) else {
                continue;
            };
            if l.home_desks.is_empty() {
                continue;
            }
            let lowest = l.home_desks.iter().map(|d| d.y).max().unwrap();
            let next_row_y = lowest + DESK_H + INTER_POD_AISLE_Y;
            let desk_y_max = (l.cubicle_band.y + l.cubicle_band.height)
                .saturating_sub(super::decor::DESK_GROUND_H);
            assert!(
                next_row_y > desk_y_max,
                "{w}x{h}: a whole desk row fits below the grid (next_row_y {next_row_y} <= desk_y_max {desk_y_max})"
            );
        }
    }
}

#[test]
fn desk_columns_stay_on_one_lattice_and_never_shift_with_width() {
    // #553 (ratified: snap-to-stride): every desk column sits on the pod
    // lattice — offset from the grid origin is k*stride + {0 | intra column
    // step} — and sweeping the width only ADDS columns (band-relative offsets
    // never move), so resize can't make inter-desk spacing jump.
    use std::collections::BTreeSet;
    let stride_x = POD_SIDE * DESK_W + (POD_SIDE - 1) * INTRA_POD_GAP_X + INTER_POD_AISLE_X;
    let mut prev: BTreeSet<u16> = BTreeSet::new();
    for w in (140..=260u16).step_by(4) {
        let Some(l) = SceneLayout::compute(w, 160, Some(64)) else {
            continue;
        };
        let origin = l.cubicle_band.x + INTER_POD_AISLE_X / 2;
        let offs: BTreeSet<u16> = l.home_desks.iter().map(|d| d.x - origin).collect();
        for &off in &offs {
            let in_pod = off % stride_x;
            assert!(
                in_pod == 0 || in_pod == DESK_W + INTRA_POD_GAP_X,
                "{w}x160: column offset {off} is off the pod lattice (in_pod {in_pod})"
            );
        }
        assert!(
            prev.is_subset(&offs),
            "{w}x160: existing columns moved or vanished on widen: {prev:?} -> {offs:?}"
        );
        prev = offs;
    }
}

#[test]
fn desk_rows_stay_on_one_lattice() {
    // The Y twin of the #553 column invariant (owner catch: the partial
    // bottom row sat 14px below the last full row vs the 23px inter-pod
    // rhythm). Every desk row top must be on the row lattice: offset from
    // the grid origin is k*stride_y + {0 | intra row step}.
    let stride_y = POD_SIDE * DESK_H + (POD_SIDE - 1) * INTRA_POD_GAP_Y + INTER_POD_AISLE_Y;
    for w in [140u16, 160, 192, 240] {
        for h in (100..=240u16).step_by(20) {
            let Some(l) = SceneLayout::compute(w, h, Some(64)) else {
                continue;
            };
            let Some(first) = l.home_desks.first() else {
                continue;
            };
            let origin_y = first.y;
            for d in &l.home_desks {
                let off = d.y - origin_y;
                let in_pod = off % stride_y;
                assert!(
                    in_pod == 0 || in_pod == DESK_H + INTRA_POD_GAP_Y,
                    "{w}x{h}: desk row top offset {off} is off the row lattice (in_pod {in_pod})"
                );
            }
        }
    }
}
