use super::*;
use std::collections::BTreeSet;

/// Every roster kind's census key, by an exhaustive match: a new kind fails to
/// compile here until it has one.
pub(crate) fn kind_key(kind: FixtureKind) -> &'static str {
    match kind {
        FixtureKind::Desk(_) => "Desk",
        FixtureKind::FilingCabinet(_) => "FilingCabinet",
        FixtureKind::DeskChair(_) => "DeskChair",
        FixtureKind::Station { .. } => "Station",
        FixtureKind::Plant { .. } => "Plant",
        FixtureKind::Pod { .. } => "Pod",
        FixtureKind::Wall { .. } => "Wall",
        FixtureKind::MeetingRug { .. } => "MeetingRug",
        FixtureKind::MeetingSofa { .. } => "MeetingSofa",
        FixtureKind::MeetingTable { .. } => "MeetingTable",
        FixtureKind::MeetingChair { .. } => "MeetingChair",
        FixtureKind::CoatRack { .. } => "CoatRack",
        FixtureKind::Doormat { .. } => "Doormat",
        FixtureKind::NoticeBoard { .. } => "NoticeBoard",
        FixtureKind::LoungeRug => "LoungeRug",
        FixtureKind::LoungeCouch => "LoungeCouch",
        FixtureKind::SideTable => "SideTable",
        FixtureKind::FloorLamp => "FloorLamp",
        FixtureKind::FishTank => "FishTank",
        FixtureKind::KitchenIsland => "KitchenIsland",
        FixtureKind::PantryMat => "PantryMat",
        FixtureKind::IslandMat => "IslandMat",
        FixtureKind::WaterCooler => "WaterCooler",
        FixtureKind::TrashBin => "TrashBin",
        FixtureKind::Door => "Door",
        FixtureKind::Runner => "Runner",
        FixtureKind::NeonSign => "NeonSign",
        FixtureKind::Clock => "Clock",
    }
}

/// Every kind's census key: the census fails on a kind placed but missing here.
pub(crate) fn every_kind_key() -> BTreeSet<&'static str> {
    [
        "Desk",
        "FilingCabinet",
        "DeskChair",
        "Station",
        "Plant",
        "Pod",
        "Wall",
        "MeetingRug",
        "MeetingSofa",
        "MeetingTable",
        "MeetingChair",
        "CoatRack",
        "Doormat",
        "NoticeBoard",
        "LoungeRug",
        "LoungeCouch",
        "SideTable",
        "FloorLamp",
        "FishTank",
        "KitchenIsland",
        "PantryMat",
        "IslandMat",
        "WaterCooler",
        "TrashBin",
        "Door",
        "Runner",
        "NeonSign",
        "Clock",
    ]
    .into_iter()
    .collect()
}

/// The offices the census and the hover sweep lay out.
fn offices() -> impl Iterator<Item = SceneLayout> {
    [
        (96u16, 60u16),
        (160, 120),
        (192, 158),
        (240, 160),
        (320, 180),
    ]
    .into_iter()
    .flat_map(|(w, h)| {
        (0..12).filter_map(move |seed| SceneLayout::compute_with_seed(w, h, None, seed))
    })
}

#[test]
fn every_fixture_kind_is_placed_on_some_office() {
    let mut seen = BTreeSet::new();
    let mut stations = std::collections::HashSet::new();
    for l in offices() {
        for f in l.fixtures() {
            seen.insert(kind_key(f.kind));
            if let FixtureKind::Station { station, .. } = f.kind {
                stations.insert(station);
            }
        }
    }
    let all = every_kind_key();
    assert_eq!(
        seen.difference(&all).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "kinds placed but missing from this census's list: add them to it"
    );
    assert_eq!(
        all.difference(&seen).collect::<Vec<_>>(),
        Vec::<&&str>::new(),
        "kinds no office places"
    );
    assert_eq!(stations.len(), 4, "every station kind: {stations:?}");
}

#[test]
fn backdrop_fixtures_come_first() {
    let l = SceneLayout::compute(192, 158, None).expect("fits");
    let depths: Vec<Depth> = l.fixtures().map(|f| f.depth).collect();
    let first_sorted = depths
        .iter()
        .position(|d| matches!(d, Depth::Sorted(_)))
        .expect("a sorted fixture");
    assert!(depths[..first_sorted].iter().all(|&d| d == Depth::Backdrop));
    assert!(depths[first_sorted..].iter().all(|&d| d != Depth::Backdrop));
}

/// The cooler and the bin stand on the floor, so a walker in front of one
/// paints over it and one behind it is hidden.
#[test]
fn the_pantry_uprights_sort_at_their_south_row() {
    let l = SceneLayout::compute(192, 158, None).expect("fits");
    let p = l.pantry.expect("a pantry");
    let expect = [
        (FixtureKind::WaterCooler, p.water_cooler_rect()),
        (FixtureKind::TrashBin, p.trash_bin_rect()),
    ];
    for (kind, rect) in expect {
        let rect = rect.expect("fits this pantry");
        let f = l.fixtures().find(|f| f.kind == kind).expect("rostered");
        assert_eq!(f.visual, rect, "{kind:?}");
        assert_eq!(f.depth, Depth::Sorted(rect.y + rect.height - 1), "{kind:?}");
    }
}

/// Where two fixtures overlap, the one painted over the other answers: a
/// back-turned desk's chair sorts past the desk and covers its front edge.
#[test]
fn fixture_at_names_the_topmost() {
    let l = SceneLayout::compute(160, 200, Some(16)).expect("fits");
    let chair = l
        .fixtures()
        .find(|f| matches!(f.kind, FixtureKind::DeskChair(_)))
        .expect("a back-turned desk");
    let FixtureKind::DeskChair(i) = chair.kind else {
        unreachable!()
    };
    let desk = l
        .fixtures()
        .find(|f| f.kind == FixtureKind::Desk(i))
        .expect("its desk");
    assert!(chair.depth > desk.depth, "the chair paints over the desk");
    let cell = Bounds {
        x: chair.visual.x + chair.visual.width / 2,
        y: chair.visual.y,
        width: 1,
        height: 1,
    };
    assert!(
        cell.y < desk.visual.y + desk.visual.height,
        "inside the desk too"
    );
    assert_eq!(l.fixture_at(cell), Some(chair.kind));
}

/// Hover reaches every fixture that shows anywhere: all but those a
/// later-painted fixture's box covers whole.
#[test]
fn fixture_at_reaches_every_fixture_not_painted_over() {
    let within = |inner: Bounds, outer: Bounds| {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.width <= outer.x + outer.width
            && inner.y + inner.height <= outer.y + outer.height
    };
    for l in [(160u16, 200u16, 0u64), (160, 200, 3), (240, 160, 5)]
        .into_iter()
        .filter_map(|(w, h, seed)| SceneLayout::compute_with_seed(w, h, Some(16), seed))
    {
        let mut reached = std::collections::HashSet::new();
        for y in (0..l.buf_h).step_by(2) {
            for x in 0..l.buf_w {
                let cell = Bounds {
                    x,
                    y,
                    width: 1,
                    height: 2,
                };
                reached.extend(l.fixture_at(cell));
            }
        }
        let fixtures: Vec<Fixture> = l.fixtures().collect();
        for (i, f) in fixtures.iter().enumerate() {
            let painted_over = fixtures
                .iter()
                .enumerate()
                .any(|(j, g)| (g.depth, j) > (f.depth, i) && within(f.visual, g.visual));
            assert!(
                painted_over || reached.contains(&f.kind),
                "{}x{}: {:?} is never reached",
                l.buf_w,
                l.buf_h,
                f.kind
            );
        }
    }
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

/// A meeting room's notice board hangs on the band, the north wall the viewer
/// sees: inside its room's columns, under the band's last row, and clear of
/// the band's decor, the door and the plants.
#[test]
fn a_notice_board_hangs_on_the_band_clear_of_its_neighbours() {
    let mut hung = 0;
    for l in offices() {
        for room in 0..l.meeting_rooms.len() {
            let Some(board) = l.notice_board_rect(room) else {
                continue;
            };
            hung += 1;
            let r = l.meeting_rooms[room].bounds;
            assert!(
                board.x > r.x && board.x + board.width < r.x + r.width,
                "{board:?} in {r:?}"
            );
            assert!(
                board.y + board.height <= l.wall_band_h(),
                "{board:?} leaves the band"
            );
            let apart = |v: Bounds| {
                v.x + v.width <= board.x
                    || board.x + board.width <= v.x
                    || v.y + v.height <= board.y
                    || board.y + board.height <= v.y
            };
            for f in l.fixtures() {
                if f.kind != (FixtureKind::NoticeBoard { room }) && f.depth != Depth::Backdrop {
                    assert!(
                        apart(f.visual),
                        "{board:?} hangs over {:?} at {:?}",
                        f.kind,
                        f.visual
                    );
                }
            }
        }
    }
    assert!(hung > 0, "no office hangs a notice board");
}

/// The pieces that stand together keep apart: the lounge's couch, lamp, side
/// table and aquarium, and the kitchen island clear of every mat.
#[test]
fn the_lounge_and_the_island_keep_clear_of_their_neighbours() {
    let apart = |a: Bounds, b: Bounds| {
        a.x + a.width <= b.x
            || b.x + b.width <= a.x
            || a.y + a.height <= b.y
            || b.y + b.height <= a.y
    };
    let mut checked = 0;
    for l in offices().chain(
        [(200u16, 120u16), (240, 144), (320, 180)]
            .into_iter()
            .flat_map(|(w, h)| {
                (0..4).filter_map(move |s| SceneLayout::compute_with_seed(w, h, None, s))
            }),
    ) {
        let lounge: Vec<Fixture> = l
            .fixtures()
            .filter(|f| {
                matches!(
                    f.kind,
                    FixtureKind::LoungeCouch
                        | FixtureKind::FloorLamp
                        | FixtureKind::SideTable
                        | FixtureKind::FishTank
                )
            })
            .collect();
        for (i, a) in lounge.iter().enumerate() {
            for b in &lounge[i + 1..] {
                assert!(
                    apart(a.visual, b.visual),
                    "{}x{}: {:?} over {:?}",
                    l.buf_w,
                    l.buf_h,
                    a,
                    b
                );
                checked += 1;
            }
        }
        let island = l.fixtures().find(|f| f.kind == FixtureKind::KitchenIsland);
        for mat in l
            .fixtures()
            .filter(|f| matches!(f.kind, FixtureKind::Doormat { .. } | FixtureKind::PantryMat))
        {
            if let Some(island) = island {
                assert!(
                    apart(island.visual, mat.visual),
                    "{}x{}: island over {:?}",
                    l.buf_w,
                    l.buf_h,
                    mat
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no office sets a lounge or an island");
}
