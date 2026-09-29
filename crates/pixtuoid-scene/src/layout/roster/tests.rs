use super::*;

/// Every roster kind, numbered by an exhaustive match: a new kind fails to
/// compile here until it has a number, and then fails the census below until
/// some office places it.
const KINDS: usize = 28;
fn kind_number(kind: FixtureKind) -> usize {
    match kind {
        FixtureKind::Desk(_) => 0,
        FixtureKind::FilingCabinet(_) => 1,
        FixtureKind::DeskChair(_) => 2,
        FixtureKind::Station { .. } => 3,
        FixtureKind::Plant { .. } => 4,
        FixtureKind::Pod { .. } => 5,
        FixtureKind::Wall { .. } => 6,
        FixtureKind::MeetingRug { .. } => 7,
        FixtureKind::MeetingSofa { .. } => 8,
        FixtureKind::MeetingTable { .. } => 9,
        FixtureKind::MeetingChair { .. } => 10,
        FixtureKind::CoatRack { .. } => 11,
        FixtureKind::Doormat { .. } => 12,
        FixtureKind::NoticeBoard { .. } => 13,
        FixtureKind::LoungeRug => 14,
        FixtureKind::LoungeCouch => 15,
        FixtureKind::SideTable => 16,
        FixtureKind::FloorLamp => 17,
        FixtureKind::FishTank => 18,
        FixtureKind::KitchenIsland => 19,
        FixtureKind::PantryMat => 20,
        FixtureKind::IslandMat => 21,
        FixtureKind::WaterCooler => 22,
        FixtureKind::TrashBin => 23,
        FixtureKind::Door => 24,
        FixtureKind::Runner => 25,
        FixtureKind::NeonSign => 26,
        FixtureKind::Clock => 27,
    }
}

#[test]
fn every_fixture_kind_is_placed_on_some_office() {
    let mut seen = [false; KINDS];
    let mut stations = std::collections::HashSet::new();
    for (w, h) in [
        (96u16, 60u16),
        (160, 120),
        (192, 158),
        (240, 160),
        (320, 180),
    ] {
        for seed in 0..12 {
            let Some(l) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                continue;
            };
            for f in l.fixtures() {
                seen[kind_number(f.kind)] = true;
                if let FixtureKind::Station { station, .. } = f.kind {
                    stations.insert(station);
                }
            }
        }
    }
    let missing: Vec<usize> = (0..seen.len()).filter(|&i| !seen[i]).collect();
    assert!(missing.is_empty(), "kinds no office places: {missing:?}");
    assert_eq!(stations.len(), 4, "every station kind: {stations:?}");
}

#[test]
fn backdrop_fixtures_come_first() {
    let l = SceneLayout::compute(192, 158, None).expect("fits");
    let depths: Vec<Depth> = l.fixtures().iter().map(|f| f.depth).collect();
    let first_sorted = depths
        .iter()
        .position(|d| matches!(d, Depth::Sorted(_)))
        .expect("a sorted fixture");
    assert!(depths[..first_sorted].iter().all(|&d| d == Depth::Backdrop));
    assert!(depths[first_sorted..].iter().all(|&d| d != Depth::Backdrop));
}

#[test]
fn only_a_north_facing_desk_stands_a_chair() {
    let desk = Point { x: 40, y: 30 };
    assert!(desk_chair_top_left(desk, Facing::North).is_some());
    for facing in [Facing::South, Facing::East, Facing::West] {
        assert_eq!(desk_chair_top_left(desk, facing), None, "{facing:?}");
    }
}

#[test]
fn the_floor_lamp_base_is_its_sprite_south_row() {
    let l = SceneLayout::compute(192, 158, None).expect("fits");
    let lamp = l.floor_lamp().expect("a lounge");
    let h = furniture_def(Furniture::FloorLamp).visual.h;
    let base = l.floor_lamp_base().expect("a lounge");
    assert_eq!((base.x, base.y), (lamp.x, lamp.y - h / 2 + h - 1));
}

/// The large columns' last one is the falsifier for the large/small split:
/// outside the small machine but inside the large one.
#[test]
fn the_coffee_machine_follows_the_counter_size() {
    let mut l = SceneLayout::compute(160, 200, Some(4)).expect("fits");
    let wp = *l
        .waypoints
        .iter()
        .find(|w| w.kind == WaypointKind::Pantry)
        .expect("a pantry");
    for w in [super::super::PANTRY_COUNTER_LARGE_W, 20] {
        let h = l.pantry_counter_size().h;
        l.pantry.as_mut().expect("a pantry").counter_size = Size { w, h };
        let (lo, hi) = coffee_machine_cols(w);
        let b = l.coffee_machine().expect("a coffee machine");
        assert_eq!(b.x, wp.pos.x - w / 2 + lo, "{w}");
        assert_eq!(b.width, hi - lo, "{w}");
        assert_eq!((b.y, b.height), (wp.pos.y - h / 2, h), "{w}");
    }
    assert_ne!(
        coffee_machine_cols(super::super::PANTRY_COUNTER_LARGE_W),
        coffee_machine_cols(20)
    );
    l.waypoints.retain(|w| w.kind != WaypointKind::Pantry);
    assert_eq!(l.coffee_machine(), None);
}

#[test]
fn a_notice_board_hangs_only_in_a_room_that_fits_it() {
    let room = |width, height| crate::layout::MeetingRoom {
        bounds: Bounds {
            x: 10,
            y: 20,
            width,
            height,
        },
        trio: None,
    };
    assert_eq!(room(15, 40).notice_board_rect(), None);
    assert_eq!(room(30, 20).notice_board_rect(), None);
    assert_eq!(
        room(16, 21).notice_board_rect(),
        Some(Bounds {
            x: 14,
            y: 33,
            width: 8,
            height: 5
        })
    );
}
