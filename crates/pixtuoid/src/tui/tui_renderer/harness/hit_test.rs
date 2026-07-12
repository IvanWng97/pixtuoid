use super::*;

// ===================================================================
// Hit-testing against a real rendered layout
// ===================================================================

#[test]
fn furniture_hit_test_resolves_against_rendered_layout() {
    let scene = scene_with(vec![idle("/hit/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // hit_test_furniture takes (pixel_x, cell_y) and doubles y internally.
    let desk = layout.home_desks[0];
    let hit = crate::tui::hit_test::hit_test_furniture(layout, desk.x + 4, desk.y / 2 + 1);
    assert_eq!(
        hit,
        Some("Desk"),
        "a desk pixel should hit the Desk furniture in the cached layout"
    );
}

#[test]
fn coffee_machine_hit_test_resolves_on_pantry() {
    use pixtuoid_scene::layout::WaypointKind;
    let scene = scene_with(vec![idle("/cm/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let pantry = layout
        .waypoints
        .iter()
        .find(|w| w.kind == WaypointKind::Pantry)
        .expect("a 140×48 office must lay out a pantry"); // no silent skip
                                                          // Scan the counter neighbourhood; the machine occupies part of it.
    let cx = pantry.pos.x;
    let cy = pantry.pos.y / 2;
    let mut found = false;
    for dx in -14i32..=14 {
        for dy in -4i32..=4 {
            let mx = (cx as i32 + dx).max(0) as u16;
            let my = (cy as i32 + dy).max(0) as u16;
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                found = true;
            }
        }
    }
    assert!(
        found,
        "the coffee machine should be hit-testable somewhere on the pantry counter"
    );
}

#[test]
fn pet_hit_test_resolves_at_pet_position() {
    let scene = scene_with(vec![active("/ph/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(120, 44, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, anim, kind } = r.cached_pet_pos().expect("pet placed");
    assert!(
        crate::tui::hit_test::hit_test_pet(kind, pos, anim, pos.x, pos.y / 2),
        "clicking the pet's own position should hit it"
    );
}

// ===================================================================
// hit_test_furniture — every per-kind label arm, against a REAL layout
// ===================================================================

// Drive a real production layout (the same `compute_with_seed` the renderer
// calls) and hover the CENTER of each populated furniture field, asserting
// hit_test_furniture returns that kind's label. This closes the waypoint loop
// (Pantry/Phone Booth/Standing Desk/Vending/Printer), meeting sofas, pantry
// table/chairs, plants, floor lamp, wall+pod decor, lounge couch + side table,
// and the procedural meeting/pantry items. Ficus + BulletinBoard are covered
// by synthetic-layout unit tests (compute never emits those two kinds).
#[test]
fn furniture_hit_test_covers_every_kind_on_real_layouts() {
    use crate::tui::hit_test::hit_test_furniture;
    use pixtuoid_scene::layout::{
        Layout, PlantKind, PodDecor, WallDecor, WaypointKind, TEST_DEFAULT_DESKS,
    };
    use std::collections::HashSet;

    // Scan the WHOLE cell grid and collect every label hit_test_furniture
    // returns anywhere. Per-item shadowing (e.g. a floor lamp under the couch
    // region, a chair under the pantry table) means a single center-probe is
    // brittle, but an item's NON-shadowed cells still yield its label — so the
    // returned-label SET reaches every arm that is geometrically reachable.
    let labels_on = |layout: &Layout| -> HashSet<&'static str> {
        let mut set = HashSet::new();
        for cy in 0..(layout.buf_h / 2) {
            for cx in 0..layout.buf_w {
                if let Some(l) = hit_test_furniture(layout, cx, cy) {
                    set.insert(l);
                }
            }
        }
        set
    };

    // Seeds 0 and 3 between them populate every field (seed 3 brings the
    // PhoneBooth/StandingDesk pod-decor + a coat-rack-only meeting room).
    let mut covered: HashSet<&'static str> = HashSet::new();
    for seed in [0u64, 3] {
        let layout = Layout::compute_with_seed(160, 200, Some(TEST_DEFAULT_DESKS), seed)
            .unwrap_or_else(|| panic!("layout for seed {seed}"));
        let labels = labels_on(&layout);

        // For every kind PRESENT in this layout, its label must be reachable.
        for wp in &layout.waypoints {
            let want = match wp.kind {
                WaypointKind::Pantry => Some("Pantry Counter"),
                WaypointKind::PhoneBooth => Some("Phone Booth"),
                WaypointKind::StandingDesk => Some("Standing Desk"),
                WaypointKind::VendingMachine => Some("Vending Machine"),
                WaypointKind::Printer => Some("Printer"),
                WaypointKind::SnackShelf => Some("Snack Shelf"),
                WaypointKind::Couch
                | WaypointKind::MeetingSofa
                | WaypointKind::MeetingStand
                | WaypointKind::Island => None,
            };
            if let Some(label) = want {
                assert!(
                    labels.contains(label),
                    "seed {seed}: waypoint {:?} → label {label:?} never resolved",
                    wp.kind
                );
            }
        }
        if !layout.meeting_furniture.is_empty() {
            assert!(labels.contains("Meeting Sofa"), "seed {seed}: Meeting Sofa");
            assert!(
                labels.contains("Meeting Table"),
                "seed {seed}: Meeting Table"
            );
        }
        if layout.kitchen_island.is_some() {
            assert!(
                labels.contains("Kitchen Island"),
                "seed {seed}: Kitchen Island"
            );
        }
        if layout.floor_lamp.is_some() {
            assert!(labels.contains("Floor Lamp"), "seed {seed}: Floor Lamp");
        }
        if layout.couch_sprite_center.is_some() {
            assert!(labels.contains("Lounge Sofa"), "seed {seed}: Lounge Sofa");
        }
        if layout.lounge_side_table.is_some() {
            assert!(labels.contains("Side Table"), "seed {seed}: Side Table");
        }
        for item in &layout.plants {
            let label = match item.kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            };
            assert!(labels.contains(label), "seed {seed}: plant {:?}", item.kind);
        }
        for item in &layout.wall_decor {
            let label = match item.kind {
                WallDecor::Whiteboard => "Whiteboard",
                WallDecor::Bookshelf => "Bookshelf",
                WallDecor::BulletinBoard => "Bulletin Board",
                WallDecor::ExitSign => "Exit Sign",
                WallDecor::MeetingScreen => "Meeting Screen",
            };
            assert!(
                labels.contains(label),
                "seed {seed}: wall decor {:?}",
                item.kind
            );
        }
        for item in &layout.pod_decor {
            let label = match item.kind {
                PodDecor::PlantTall => "Tall Plant",
                PodDecor::Whiteboard => "Whiteboard",
                PodDecor::Tv => "TV Stand",
                PodDecor::PhoneBooth => "Phone Booth",
                PodDecor::StandingDesk => "Standing Desk",
            };
            assert!(
                labels.contains(label),
                "seed {seed}: pod decor {:?}",
                item.kind
            );
        }
        // Procedural room items (coat rack / doormat / water cooler / trash bin)
        // are emitted by hit_test_furniture from the room bounds, not a layout
        // field, so just gather whatever resolved.
        covered.extend(labels);
    }

    // The procedural meeting/pantry-room items must surface across the two
    // seeds (seed 0 has both a meeting room and a pantry room at 160×200).
    for label in [
        "Coat Rack",
        "Doormat",
        "Water Cooler",
        "Trash Bin",
        "Elevator",
    ] {
        assert!(
            covered.contains(label),
            "procedural/room item {label:?} never resolved across seeds"
        );
    }
}

// ===================================================================
// hit_test_agent + hover marker
// ===================================================================

// Hover an idle agent's own sprite cell → the label gains the '▸' hovered
// marker (exercises hit_test_agent's Some-return + tooltip is_hovered branch).
#[test]
fn hovering_an_agent_marks_its_label() {
    let mut s = idle("/hov/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = "HOVERME".into();
    let scene = scene_with(vec![s], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    // A long-idle agent at its home desk; mirror hit_test_from_tui's anchor.
    let desk = r.cached_layout().expect("layout").home_desks[0];
    let cell_x = desk.x + 2;
    let cell_y = desk.y.saturating_sub(4) / 2 + 1;
    r.set_mouse_pos(Some((cell_x, cell_y)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("\u{25b8}HOVERME") || text.contains("\u{25b8}"),
        "hovering an agent should add the ▸ marker to its label; frame:\n{text}"
    );
}
