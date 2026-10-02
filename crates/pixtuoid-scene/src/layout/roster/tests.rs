use super::*;
use crate::display::Layer;
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
        .position(|d| matches!(d, Depth::Sorted { .. }))
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
        assert_eq!(f.depth, Depth::sorted(rect.y + rect.height - 1), "{kind:?}");
    }
}

/// The counter the cooler and the bin step off is the one rostered at its
/// waypoint.
#[test]
fn the_pantry_counter_fixture_is_the_rooms_counter_rect() {
    let mut widths = BTreeSet::new();
    for (w, h) in [(55, 72), (89, 72), (120, 96), (192, 158), (320, 180)] {
        for seed in 0..12 {
            let Some(l) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                continue;
            };
            let fixture = l.fixtures().find_map(|f| {
                matches!(
                    f.kind,
                    FixtureKind::Station {
                        station: Station::PantryCounter,
                        ..
                    }
                )
                .then_some(f.visual)
            });
            widths.insert(fixture.map(|f| f.width));
            assert_eq!(
                fixture,
                l.pantry.and_then(|p| p.counter_rect()),
                "{w}x{h} seed {seed}"
            );
        }
    }
    assert_eq!(
        widths,
        BTreeSet::from([
            None,
            Some(crate::layout::COMPACT_COUNTER.w),
            Some(crate::layout::LARGE_COUNTER.w)
        ]),
        "no counter, and each counter size"
    );
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
    for w in [super::super::PANTRY_COUNTER_LARGE_W, 20] {
        let p = l.pantry.as_mut().expect("a pantry");
        p.counter_size.w = w;
        let counter = p.counter_rect().expect("a counter");
        let (lo, hi) = coffee_machine_cols(w);
        assert_eq!(
            l.coffee_machine(),
            Some(Bounds {
                x: counter.x + lo,
                width: hi - lo,
                ..counter
            }),
            "{w}"
        );
    }
    assert_ne!(
        coffee_machine_cols(super::super::PANTRY_COUNTER_LARGE_W),
        coffee_machine_cols(20)
    );
    l.pantry = None;
    assert_eq!(l.coffee_machine(), None);
}

/// A meeting room's notice board hangs on the band, the north wall the viewer
/// sees: inside its room's columns, under the band's last row.
#[test]
fn a_notice_board_hangs_on_the_band_inside_its_room() {
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
        }
    }
    assert!(hung > 0, "no office hangs a notice board");
}

/// Hover orders fixtures on [`Tie`], and a painter on the [`Layer`] it maps
/// to: the two orders must agree, with a figure between the two ties.
#[test]
fn a_tie_maps_to_a_layer_in_the_same_order() {
    const TIES: [Tie; 2] = [Tie::FigureOver, Tie::FixtureOver];
    // A new tie fails to compile here until `TIES` lists it.
    let _ = |t: Tie| match t {
        Tie::FigureOver | Tie::FixtureOver => (),
    };
    assert!(Layer::from(Tie::FigureOver) < Layer::Figure);
    assert!(Layer::Figure < Layer::from(Tie::FixtureOver));
    for a in TIES {
        for b in TIES {
            assert_eq!(a < b, Layer::from(a) < Layer::from(b), "{a:?} vs {b:?}");
        }
    }
}

/// A standing fixture's shadow is centred under its whole art box, on the row
/// under it: a desk's side cabinets included, not just its surface (#906).
#[test]
fn a_standing_fixture_casts_its_shadow_under_its_whole_box() {
    let mut desks = 0;
    for l in offices() {
        for f in l.fixtures() {
            let Some(c) = f.contact() else {
                continue;
            };
            assert_ne!(f.depth, Depth::Backdrop, "{:?} lies flat", f.kind);
            let v = f.visual;
            let ((x0, _), (x1, y1)) = c.bounds();
            // Against the west wall, its west reach clips at column 0.
            if v.x >= crate::ground::CONTACT_REACH {
                assert_eq!(x1 - (v.x + v.width), v.x - x0, "{:?} is centred", f.kind);
            }
            assert!(y1 > v.y + v.height, "{:?} falls south", f.kind);
            desks += usize::from(matches!(f.kind, FixtureKind::Desk(_)));
        }
    }
    assert!(desks > 0, "the sweep saw a desk");
}

/// The sizes the north-wall census rendered, the narrowest sweep walls and
/// the committed heroes' buffers (`scripts/media.json`), each at a few seeds.
fn north_wall_census() -> impl Iterator<Item = SceneLayout> {
    [
        (
            super::super::compute::MIN_LAYOUT_W,
            super::super::compute::MIN_LAYOUT_H,
        ),
        (super::super::compute::MIN_LAYOUT_W, 80),
        (96, 60),
        (120, 72),
        (140, 80),
        (160, 96),
        (192, 108),
        (240, 135),
        (320, 180),
        (160, 192),
        (176, 99),
        (208, 176),
        (231, 130),
    ]
    .into_iter()
    .flat_map(|(w, h)| {
        (0..3).map(move |seed| {
            SceneLayout::compute_with_seed(w, h, None, seed).expect("a census size lays out")
        })
    })
}

fn is_exit_sign(k: &FixtureKind) -> bool {
    matches!(
        k,
        FixtureKind::Wall {
            kind: WallDecor::ExitSign,
            ..
        }
    )
}

#[test]
fn the_door_is_centred_in_the_last_window_slot() {
    for l in north_wall_census() {
        let at = format!("{}x{}", l.buf_w, l.buf_h);
        let door = l.door_rect();
        let roster = l.fixtures().find(|f| f.kind == FixtureKind::Door);
        assert_eq!(roster.map(|f| f.visual), Some(door), "{at}");
        let slots: Vec<_> = super::super::window_slots(l.buf_w).collect();
        let Some(&slot) = slots.last() else {
            assert_eq!(door.x, super::super::door_x(l.buf_w), "{at}");
            continue;
        };
        assert!(
            slot.x < door.x && door.x + door.width < slot.span().end,
            "{at}"
        );
        assert_eq!(
            door.x - slot.x,
            slot.span().end - (door.x + door.width),
            "{at}: centred in its slot"
        );
        assert_eq!(l.window_bays().count() + 1, slots.len(), "{at}");
        assert!(
            l.window_bays()
                .all(|b| b.span().end <= door.x || door.x + door.width <= b.x),
            "{at}: a window under the door"
        );
    }
}

#[test]
fn the_exit_sign_hangs_centred_over_the_door_indicator_below_the_window_head() {
    let mut met = 0;
    for l in north_wall_census().chain(offices()) {
        let at = format!("{}x{}", l.buf_w, l.buf_h);
        let Some(sign) = l.fixtures().find(|f| is_exit_sign(&f.kind)) else {
            continue;
        };
        met += 1;
        let (sign, door) = (sign.visual, l.door_rect());
        let (west, east) = (
            sign.x - door.x,
            (door.x + door.width) - (sign.x + sign.width),
        );
        assert!(
            west.abs_diff(east) <= 1,
            "{at}: {sign:?} centred over {door:?}"
        );
        assert!(
            sign.y + sign.height <= super::super::floor_indicator_rows(door.y).start,
            "{at}: {sign:?} above {door:?} and its floor indicator"
        );
        assert!(
            sign.y >= super::super::window_rows(l.wall_band_h()).start,
            "{at}: {sign:?} under the ceiling"
        );
    }
    assert!(met > 0, "the sweep met an exit sign");
}

#[test]
fn a_notice_board_hangs_within_one_pane() {
    let mut met = 0;
    for l in north_wall_census().chain(offices()) {
        for f in l.fixtures() {
            let FixtureKind::NoticeBoard { .. } = f.kind else {
                continue;
            };
            met += 1;
            let b = f.visual;
            assert!(
                l.window_bays()
                    .flat_map(|bay| bay.panes())
                    .any(|p| p.start <= b.x && b.x + b.width <= p.end),
                "{}x{}: {b:?} straddles a frame",
                l.buf_w,
                l.buf_h
            );
        }
    }
    assert!(met > 0, "the sweep met a notice board");
}

/// The sizes whose meeting room hung a board before it snapped to a pane, but
/// 140x80, whose room's north wall has no free pane.
#[test]
fn snapping_to_a_pane_keeps_the_notice_board() {
    for (w, h) in [(160, 96), (192, 108), (200, 120), (240, 144), (320, 180)] {
        let l = SceneLayout::compute_with_seed(w, h, None, 0).expect("lays out");
        assert!(
            l.fixtures()
                .any(|f| matches!(f.kind, FixtureKind::NoticeBoard { .. })),
            "{w}x{h}"
        );
    }
}

#[test]
fn the_clock_hangs_centred_on_a_window_post() {
    let mut met = 0;
    for l in north_wall_census() {
        let at = format!("{}x{}", l.buf_w, l.buf_h);
        let Some(clock) = l.fixtures().find(|f| f.kind == FixtureKind::Clock) else {
            assert!(
                super::super::window_posts(l.buf_w).count() <= 1,
                "{at}: no clock, though a post stands clear of the neon"
            );
            continue;
        };
        met += 1;
        let clock = clock.visual;
        let centre = clock.x + clock.width / 2;
        assert!(
            super::super::window_posts(l.buf_w).any(|p| (p.start + p.end) / 2 == centre),
            "{at}: {clock:?} off every post"
        );
    }
    assert!(met > 0, "the census met a clock");
}

/// The [`kind_key`] pairs whose art overlaps by design, each in key order.
const OVERLAP_BY_DESIGN: &[(&str, &str)] = &[
    // `desk_chair_top_left`'s backrest crosses its desk.
    ("Desk", "DeskChair"),
    // `SceneLayout::island_bar_mat` shows only a sliver past the island.
    ("IslandMat", "KitchenIsland"),
    // A short floor's first desk row hides the foot of the lamp behind it,
    // whose ground already starts at the band's top.
    ("Desk", "FloorLamp"),
    // A rug under what stands on it.
    ("LoungeCouch", "LoungeRug"),
    ("MeetingChair", "MeetingRug"),
    ("MeetingRug", "MeetingSofa"),
    ("MeetingRug", "MeetingTable"),
    // One composed meeting set: in a compact room each piece's art overlaps the
    // edge of the one behind it.
    ("MeetingChair", "MeetingSofa"),
    ("MeetingSofa", "MeetingTable"),
];

/// The [`kind_key`] pairs still overlapping where they should not, each in key
/// order: a fix deletes its entry.
const OVERLAP_DEFECTS: &[(&str, &str)] = &[];

/// Two fixtures' art overlaps only as a listed pair, and each listed pair still
/// occurs.
#[test]
fn no_two_fixtures_overlap_but_by_design() {
    let listed: BTreeSet<(&str, &str)> = OVERLAP_BY_DESIGN
        .iter()
        .chain(OVERLAP_DEFECTS)
        .copied()
        .collect();
    // The corridor's floor: whatever stands in the corridor stands on it.
    let on_runner =
        |f: &Fixture, g: &Fixture| f.kind == FixtureKind::Runner && g.contact().is_some();
    let (mut met, mut stray) = (BTreeSet::new(), Vec::new());
    for l in north_wall_census().chain(offices()) {
        let fixtures: Vec<Fixture> = l.fixtures().collect();
        for (i, a) in fixtures.iter().enumerate() {
            for b in fixtures[i + 1..]
                .iter()
                .filter(|b| a.visual.overlaps(b.visual))
            {
                if on_runner(a, b) || on_runner(b, a) {
                    continue;
                }
                let (x, y) = (kind_key(a.kind), kind_key(b.kind));
                let pair = (x.min(y), x.max(y));
                if listed.contains(&pair) {
                    met.insert(pair);
                } else {
                    stray.push(format!(
                        "{}x{}: {:?} {:?} over {:?} {:?}",
                        l.buf_w, l.buf_h, a.kind, a.visual, b.kind, b.visual
                    ));
                }
            }
        }
    }
    assert_eq!(stray, Vec::<String>::new());
    assert_eq!(
        listed.difference(&met).collect::<Vec<_>>(),
        Vec::<&(&str, &str)>::new(),
        "listed but never met: delete the entry"
    );
}

#[test]
fn no_meeting_furniture_or_plant_blocks_a_doorway() {
    use super::super::rooms::walls::WALL_H;
    let mut met = 0;
    for l in offices().chain(north_wall_census()) {
        for d in &l.doorways {
            met += 1;
            // The cut ends are the jambs' own cells, so the opening lies between.
            let opening = if d.start.y == d.end.y {
                Bounds {
                    x: d.start.x + 1,
                    y: d.start.y - WALL_H.cap,
                    width: d.end.x - d.start.x - 1,
                    height: WALL_H.cap + WALL_H.thickness,
                }
            } else {
                Bounds {
                    x: d.start.x,
                    y: d.start.y + 1,
                    width: super::super::WALL_THICK_V,
                    height: d.end.y - d.start.y - 1,
                }
            };
            for f in l.fixtures().filter(|f| {
                matches!(
                    f.kind,
                    FixtureKind::MeetingRug { .. }
                        | FixtureKind::MeetingSofa { .. }
                        | FixtureKind::MeetingTable { .. }
                        | FixtureKind::MeetingChair { .. }
                        | FixtureKind::Plant { .. }
                )
            }) {
                assert!(
                    !f.visual.overlaps(opening),
                    "{}x{}: {:?} {:?} in the doorway {opening:?}",
                    l.buf_w,
                    l.buf_h,
                    f.kind,
                    f.visual
                );
            }
        }
    }
    assert!(met > 0, "the sweep met a doorway");
}
