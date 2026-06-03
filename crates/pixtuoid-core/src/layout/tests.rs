use super::*;

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
    for (w, h, cap) in [(90u16, 110u16, 4usize), (100, 200, 10), (120, 150, 8)] {
        // Total capacity at this size is exactly `cap`.
        let full = SceneLayout::compute_with_seed(w, h, MAX_VISIBLE_DESKS, 0).expect("fits");
        assert_eq!(
            full.home_desks.len(),
            cap,
            "{w}x{h}: expected total desk capacity {cap}"
        );
        let max_y = full.home_desks.iter().map(|d| d.y).max().unwrap();
        let band_at = |n: usize| {
            SceneLayout::compute_with_seed(w, h, n, 0)
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
                SceneLayout::compute_with_seed(w, h, n, 0)
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
    assert!(SceneLayout::compute(20, 20, 4).is_none());
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
        let l = SceneLayout::compute_with_seed(4000, 4000, MAX_VISIBLE_DESKS, seed);
        assert!(
            l.is_some(),
            "huge terminal (seed {seed}) must lay out, not overflow"
        );
    }
}

// Ground-footprint rectangle `(x, y, w, h)` (no clearance pad — the pad is
// routing slack, not the object's solid area). Mirror the stamps in `mask.rs`.
type Rect = (u16, u16, u16, u16);
fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}
fn wall_rect(s: Point, e: Point) -> Rect {
    if s.x == e.x {
        (s.x, s.y.min(e.y), WALL_THICK_V, s.y.abs_diff(e.y) + 1)
    } else {
        (s.x.min(e.x), s.y, s.x.abs_diff(e.x) + 1, WALL_THICK_H)
    }
}

#[test]
fn freestanding_decor_does_not_overlap_room_walls() {
    // Placement-overlap guard for FREE-STANDING decor only — items meant to
    // sit in open floor (pantry bistro table, lounge side table). Burying
    // one inside a wall (the reported pantry-table bug) is a placement
    // error. NOT checked: wall-ADJACENT furniture (meeting sofa/table sit
    // against the glass partitions by design) — that overlap is intended +
    // physically correct, and the occlusion / z-sort system draws it right.
    // Includes the minimum-width sizes (34 = MIN_W, 48) where the lounge
    // side table sits closest to the vertical room wall.
    for &(w, h) in &[
        (96u16, 72u16),
        (120, 80),
        (160, 120),
        (192, 160),
        (34, 60),
        (48, 60),
    ] {
        for seed in 0..6u64 {
            let Some(l) = SceneLayout::compute_with_seed(w, h, 8, seed) else {
                continue;
            };
            let mut items: Vec<(&str, Rect)> = Vec::new();
            if let Some(t) = l.pantry_table {
                let Size { w, h } = furniture_def(Furniture::PantryTable).footprint.unwrap();
                items.push((
                    "pantry_table",
                    (t.x.saturating_sub(w / 2), t.y.saturating_sub(h / 2), w, h),
                ));
            }
            if let Some(t) = l.lounge_side_table {
                let Size { w, h } = furniture_def(Furniture::LoungeSideTable).footprint.unwrap();
                items.push((
                    "lounge_side_table",
                    (t.x.saturating_sub(w / 2), t.y.saturating_sub(h / 2), w, h),
                ));
            }
            for (item, rect) in &items {
                for &WallSegment { start: s, end: e } in &l.room_walls {
                    let wr = wall_rect(s, e);
                    assert!(
                        !rects_overlap(*rect, wr),
                        "{w}x{h} seed {seed}: {item} {rect:?} overlaps wall {wr:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn compute_returns_none_at_exact_boundary() {
    let min_w = DESK_W + DESK_GAP_X * 2; // 34
    let min_h: u16 = 40 + MIN_TOP_MARGIN; // 60
    assert!(
        SceneLayout::compute(min_w - 1, min_h, 1).is_none(),
        "one pixel below MIN_W should return None"
    );
    assert!(
        SceneLayout::compute(min_w, min_h - 1, 1).is_none(),
        "one pixel below min_h should return None"
    );
    assert!(
        SceneLayout::compute(min_w, min_h, 1).is_some(),
        "exactly at boundary should return Some"
    );
}

#[test]
fn compute_zones_are_ordered_top_to_bottom_and_nonoverlapping() {
    let l = SceneLayout::compute(120, 80, 6).expect("fits");
    assert!(l.cubicle_band.y < l.walkway.y);
    let c_bot = l.cubicle_band.y + l.cubicle_band.height;
    assert!(c_bot <= l.walkway.y, "cubicle overlaps walkway");
    // Walkway runs to the baseboard now that lounge_band is gone.
    let w_bot = l.walkway.y + l.walkway.height;
    assert!(w_bot <= l.buf_h);
}

#[test]
fn compute_places_one_home_desk_per_agent() {
    let l = SceneLayout::compute(160, 80, 5).expect("fits");
    assert!(l.home_desks.len() <= 5 && !l.home_desks.is_empty());
    for d in &l.home_desks {
        assert!(d.y >= l.cubicle_band.y);
        assert!(d.y + DESK_H <= l.cubicle_band.y + l.cubicle_band.height);
        assert!(d.x >= l.cubicle_band.x);
    }
}

#[test]
fn compute_places_all_waypoint_kinds() {
    let l = SceneLayout::compute(120, 96, 1).expect("fits");
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
                let pr = l.pantry_room.expect("pantry");
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
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {
                assert!(w.pos.y >= l.top_margin);
            }
            WaypointKind::VendingMachine | WaypointKind::Printer => {
                assert!(w.pos.y >= l.top_margin);
            }
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {
                // A meeting slot only exists when a meeting room does, and
                // it carries the room id it belongs to.
                assert!(l.meeting_room.is_some());
                assert!(w.room_id.is_some());
            }
        }
    }
}

#[test]
fn every_waypoint_kind_is_placed_in_some_layout() {
    // Placement conformance: a `WaypointKind` defined in `decor.rs` but never
    // pushed by `compute_waypoints` compiles green and passes every existing
    // test while being SILENTLY INVISIBLE in the office (the most forgettable
    // failure mode when adding furniture — there is no compile guard that a
    // declared kind actually gets a placement site). This sweep is that guard.
    //
    // The sizes MUST span small→large: `VendingMachine`/`Printer` are
    // corridor-height-gated (`walkway.height >= …` in `compute_waypoints`), so
    // they only appear at large terminals; a single 192×80 seed would falsely
    // "fail" for them.
    use std::collections::HashSet;
    let mut seen: HashSet<WaypointKind> = HashSet::new();
    for seed in 0..40u64 {
        for (w, h) in [
            (160u16, 100u16),
            (192, 80),
            (240, 120),
            (300, 140),
            (400, 200),
            (500, 250),
        ] {
            if let Some(l) = SceneLayout::compute_with_seed(w, h, 24, seed) {
                seen.extend(l.waypoints.iter().map(|wp| wp.kind));
            }
        }
    }
    // Kinds deliberately NOT a wander destination (none today). A new
    // WaypointKind that is intentionally never placed goes here WITH a reason.
    const ALLOWLIST: &[WaypointKind] = &[];
    let missing: Vec<_> = WaypointKind::ALL
        .iter()
        .copied()
        .filter(|k| !seen.contains(k) && !ALLOWLIST.contains(k))
        .collect();
    assert!(
        missing.is_empty(),
        "WaypointKind(s) declared in ::ALL but never pushed by compute_waypoints \
         in any swept layout: {missing:?}. Add a placement site in \
         compute_waypoints, or add to ALLOWLIST with a reason."
    );
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
        let l = SceneLayout::compute(w, h, 64).expect("fits");
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
                l.pantry_counter_size,
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
    let l = SceneLayout::compute(120, 80, 4).expect("fits");

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

    // 1 meeting room → 2 sofas (meeting_sofas center-points) → 3 seats each.
    assert!(!l.meeting_sofas.is_empty(), "expected a meeting room");
    let sofa_seats = l
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::MeetingSofa)
        .count();
    assert_eq!(
        sofa_seats,
        3 * l.meeting_sofas.len(),
        "each meeting sofa seats 3"
    );
}

#[test]
fn meeting_slots_track_meeting_rooms() {
    // Across every floor variant, a meeting slot exists iff a meeting
    // room exists, every slot carries a valid room_id, and a dual-meeting
    // floor produces slots for both rooms.
    let mut saw_room = false;
    let mut saw_no_room = false;
    let mut saw_dual = false;
    for seed in 0..40u64 {
        let l = SceneLayout::compute_with_seed(160, 120, 8, seed).expect("fits");
        let sofa_slots: Vec<_> = l
            .waypoints
            .iter()
            .filter(|w| {
                matches!(
                    w.kind,
                    WaypointKind::MeetingSofa | WaypointKind::MeetingStand
                )
            })
            .collect();
        if l.meeting_room.is_some() {
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
                    .any(|w| w.kind == WaypointKind::MeetingStand),
                "seed {seed}: meeting room but no standing slot"
            );
            let rooms = l.meeting_tables.len();
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
            let Some(l) = SceneLayout::compute_with_seed(w, h, 8, seed) else {
                continue;
            };
            for (room_id, table) in l.meeting_tables.iter().enumerate() {
                let north = l.meeting_sofas[2 * room_id];
                let south = l.meeting_sofas[2 * room_id + 1];
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
    // south seat faces North); standing slots face inward toward the table
    // centre (west faces East, east faces West). This is what makes the
    // render pick front "seated" vs "back_couch" and the correct flip.
    for seed in 0..40u64 {
        let l = SceneLayout::compute_with_seed(160, 120, 8, seed).expect("fits");
        for w in &l.waypoints {
            let Some(room_id) = w.room_id else { continue };
            let table = l.meeting_tables[room_id];
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
                WaypointKind::MeetingStand => {
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

// Regression: the WEST MeetingStand point used to land on the table's padded
// obstacle (blocked x ∈ [t.x-8, t.x+7]; the symmetric -8 hit the inclusive
// left edge), so the router had to snap it off-target. Both stands must be on
// walkable cells across seeds/sizes.
#[test]
fn meeting_stand_points_are_walkable() {
    for seed in 0..40u64 {
        for (w, h) in [(160u16, 120u16), (200, 100), (240, 140)] {
            let l = SceneLayout::compute_with_seed(w, h, 8, seed).expect("fits");
            for wp in &l.waypoints {
                if wp.kind == WaypointKind::MeetingStand {
                    assert!(
                        l.is_walkable(wp.pos.x, wp.pos.y),
                        "seed {seed} @ {w}x{h}: MeetingStand {:?} is non-walkable",
                        wp.pos
                    );
                }
            }
        }
    }
}

#[test]
fn compute_places_bookshelf_on_wall_and_whiteboard_in_walkway() {
    let l = SceneLayout::compute(120, 96, 1).expect("fits");
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
    let l = SceneLayout::compute(120, 96, 1).expect("fits");
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
fn compute_places_plants_in_lounge_and_walkway() {
    let l = SceneLayout::compute(120, 96, 1).expect("fits");
    assert!(!l.plants.is_empty());
    for p in &l.plants {
        assert!(p.pos.x < l.buf_w);
        assert!(p.pos.y < l.buf_h);
    }
}

#[test]
fn compute_truncates_home_desks_when_more_agents_than_fit() {
    let l = SceneLayout::compute(50, 80, 20).expect("fits");
    assert!(l.home_desks.len() < 20);
}

/// Pixel-level BFS from `door_threshold` must reach every walkable
/// pixel across a range of buffer sizes. If this regresses, any
/// agent stranded in an unreachable pocket will see A* return its
/// straight-line fallback and visibly teleport across walls ("闪现").
///
/// The probed sizes span small (typical 80-col terminal) through
/// large (4K-cell terminal). Each pair is also probed with a high
/// agent count to exercise the overflow seat placement.
#[test]
fn walkable_mask_is_fully_connected_across_buffer_sizes() {
    use std::collections::VecDeque;

    // Range covers the realistic terminal sizes — a small 80×35-cell
    // terminal up to a 4K-cell rig. Below 96×70 the meeting room
    // sofas + table + walls degenerate (sofa padding covers the
    // entire interior) which would be a layout-design problem
    // rather than a pathfinding regression.
    let sizes = [
        (96u16, 70u16, 7usize),
        (128, 80, 10),
        (160, 100, 12),
        (240, 130, 16),
        (320, 180, 16),
    ];
    for (buf_w, buf_h, num_agents) in sizes {
        let l = SceneLayout::compute(buf_w, buf_h, num_agents)
            .unwrap_or_else(|| panic!("layout fits at {buf_w}x{buf_h}"));
        let w = l.buf_w as usize;
        let h = l.buf_h as usize;
        let start = l
            .door_threshold
            .unwrap_or_else(|| panic!("door_threshold missing at {buf_w}x{buf_h}"));
        assert!(
            l.is_walkable(start.x, start.y),
            "door_threshold {start:?} not walkable at {buf_w}x{buf_h}"
        );

        // BFS from the threshold.
        let mut visited = vec![false; w * h];
        visited[(start.y as usize) * w + (start.x as usize)] = true;
        let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
        queue.push_back((start.x as usize, start.y as usize));
        let mut reachable = 1usize;
        while let Some((x, y)) = queue.pop_front() {
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let (nx, ny) = (nx as usize, ny as usize);
                if nx >= w || ny >= h || visited[ny * w + nx] {
                    continue;
                }
                if !l.is_walkable(nx as u16, ny as u16) {
                    continue;
                }
                visited[ny * w + nx] = true;
                reachable += 1;
                queue.push_back((nx, ny));
            }
        }

        // Total walkable pixels.
        let mut walkable_total = 0usize;
        for y in 0..h {
            for x in 0..w {
                if l.is_walkable(x as u16, y as u16) {
                    walkable_total += 1;
                }
            }
        }
        assert_eq!(
            reachable,
            walkable_total,
            "{buf_w}x{buf_h} ({num_agents} agents): {} disconnected pixels — \
             some open area is isolated from the door",
            walkable_total - reachable
        );
    }
}

#[test]
fn walkable_mask_connected_across_floor_seeds() {
    use std::collections::VecDeque;

    // Sweep the two SMALLEST sizes across all floor seeds too: the dense
    // floor variant (seed 2) stacks two meeting rooms and is the riskiest
    // for connectivity at narrow widths — the size-only test runs seed 0.
    for (buf_w, buf_h, num_agents) in [(160u16, 100u16, 12usize), (96, 70, 7), (128, 80, 10)] {
        for seed in 0..5u64 {
            let l = SceneLayout::compute_with_seed(buf_w, buf_h, num_agents, seed)
                .expect("layout fits");
            let w = l.buf_w as usize;
            let h = l.buf_h as usize;
            let start = l.door_threshold.expect("door_threshold");
            assert!(l.is_walkable(start.x, start.y));

            let mut visited = vec![false; w * h];
            visited[(start.y as usize) * w + (start.x as usize)] = true;
            let mut queue = VecDeque::new();
            queue.push_back((start.x, start.y));
            let mut reachable = 1usize;
            while let Some((cx, cy)) = queue.pop_front() {
                for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as u16, ny as u16);
                    let idx = (ny as usize) * w + (nx as usize);
                    if !visited[idx] && l.is_walkable(nx, ny) {
                        visited[idx] = true;
                        reachable += 1;
                        queue.push_back((nx, ny));
                    }
                }
            }
            let walkable_total = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| l.is_walkable(x as u16, y as u16))
                .count();
            assert_eq!(
                reachable,
                walkable_total,
                "seed={seed}: {buf_w}x{buf_h}: {} disconnected pixels",
                walkable_total - reachable
            );
        }
    }
}
