use super::anchors::{
    back_couch_anchor, seated_anchor_facing, standing_at_desk_anchor, walking_anchor,
    waypoint_anchor, CHARACTER_SPRITE_W,
};
use super::seat::{seat_sprite, seat_sprite_in_pack, settle_seat_view, SeatView};
use super::wall::WALL_THICK_H_PX;
use super::*;
use crate::layout::stitch_vertical_wall;
use crate::pose;
use pixtuoid_core::sprite::{Frame, Palette};
use pixtuoid_core::state::{GlobalDeskIndex, ToolKind};
use pixtuoid_core::walkable::OccupancyOverlay;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn stitch_vertical_wall_connects_each_joint() {
    let top_margin = 48u16;
    let top_wall_h = top_margin - 4;
    let h_y = 90u16;
    let h_rows = [h_y];

    let (yt, _) = stitch_vertical_wall(top_margin, 70, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        yt, top_wall_h,
        "top segment should connect up to the window band"
    );

    let (_, yb) = stitch_vertical_wall(60, h_y, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        yb,
        h_y + (WALL_THICK_H_PX - 1),
        "bottom should fill the corner"
    );

    let (yt2, _) = stitch_vertical_wall(h_y + 6, 120, top_margin, top_wall_h, &h_rows);
    assert_eq!(yt2, h_y, "lower segment should bridge up to the cross wall");

    let (yt3, yb3) = stitch_vertical_wall(h_y + 20, 130, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        (yt3, yb3),
        (h_y + 20, 130),
        "distant segment must not bridge"
    );
    let (yt4, yb4) = stitch_vertical_wall(60, 80, top_margin, top_wall_h, &[]);
    assert_eq!((yt4, yb4), (60, 80), "no joints → unchanged");
}

#[test]
fn vertical_wall_top_raise_lands_on_the_band_row() {
    let top_margin = 48u16;
    let tbm = crate::layout::WALL_BAND_TO_TOP_MARGIN;
    let top_wall_h = top_margin - tbm;
    let band_row = top_margin.saturating_sub(tbm);
    let (stitch_raise, _) = stitch_vertical_wall(top_margin, 90, top_margin, top_wall_h, &[]);
    assert_eq!(
        stitch_raise, band_row,
        "the shared stitch must raise a band-rooted vertical wall top to the band row"
    );
}

#[test]
fn v_door_jambs_sit_flush_on_both_cut_ends() {
    // The glass painters are endpoint-INCLUSIVE, so each jamb must COVER its
    // cut end, or a 1px glass sliver survives between post and opening.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(20, 60, floor);
    wall::paint_glass_wall_v(&mut buf, theme, 5, 10, 24);
    wall::paint_glass_wall_v(&mut buf, theme, 5, 38, 52);
    wall::paint_door_jamb_v(&mut buf, theme, 5, 24 - (wall::DOOR_JAMB_PX - 1));
    wall::paint_door_jamb_v(&mut buf, theme, 5, 38);
    let dark = theme.office.room_wall_trim_dark;
    for y in [23, 24, 38, 39] {
        assert_eq!(
            buf.get(5, y),
            dark,
            "row {y} must be jamb (posts cover BOTH inclusive cut ends)"
        );
    }
    for y in 25..38 {
        assert_eq!(buf.get(5, y), floor, "row {y} is the OPENING — untouched");
    }
}

#[test]
fn h_wall_jamb_flags_join_on_the_doorway_cut_ends() {
    use crate::layout::TEST_DEFAULT_DESKS;
    let l = Layout::compute(215, 98, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let dw = l
        .doorways
        .iter()
        .find(|d| d.start.y == d.end.y)
        .expect("the meeting-pantry 60% door");
    let mut drawables = Vec::new();
    enqueue_room_walls_h(&l, &mut drawables);
    let walls: Vec<_> = drawables
        .iter()
        .filter_map(|d| match d.kind {
            DrawableKind::RoomWallH {
                x0,
                x1,
                jamb_left,
                jamb_right,
                ..
            } => Some((x0, x1, jamb_left, jamb_right)),
            _ => None,
        })
        .collect();
    let left = walls
        .iter()
        .find(|(_, x1, ..)| *x1 == dw.start.x)
        .expect("segment left of the door");
    assert!(
        left.3 && !left.2,
        "left segment: jamb on its RIGHT end only"
    );
    let right = walls
        .iter()
        .find(|(x0, ..)| *x0 == dw.end.x)
        .expect("segment right of the door");
    assert!(
        right.2 && !right.3,
        "right segment: jamb on its LEFT end only"
    );
}

#[test]
fn v_wall_jamb_flags_and_south_anchor_on_the_doorway_cut_ends() {
    use crate::layout::TEST_DEFAULT_DESKS;
    let l = Layout::compute(215, 98, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let dw = l
        .doorways
        .iter()
        .find(|d| d.start.x == d.end.x)
        .expect("the meeting room's centered vertical door");
    let mut drawables = Vec::new();
    enqueue_room_walls_v(&l, l.wall_band_h(), &mut drawables);
    let walls: Vec<_> = drawables
        .iter()
        .filter_map(|d| match d.kind {
            DrawableKind::RoomWallV {
                x,
                y_top,
                y_bot,
                jamb_north,
                jamb_south,
            } if x == dw.start.x => Some((d.anchor_y, y_top, y_bot, jamb_north, jamb_south)),
            _ => None,
        })
        .collect();
    let top = walls
        .iter()
        .find(|(_, _, y_bot, ..)| *y_bot == dw.start.y)
        .expect("segment north of the door");
    assert_eq!(
        top.0, top.2,
        "the door-terminus (top) segment y-sorts at its south base"
    );
    assert!(top.4 && !top.3, "top segment: jamb on its SOUTH end only");
    let bottom = walls
        .iter()
        .find(|(_, y_top, ..)| *y_top == dw.end.y)
        .expect("segment south of the door");
    assert!(
        bottom.3 && !bottom.4,
        "bottom segment: jamb on its NORTH end only"
    );
}

#[test]
fn glass_wall_h_back_cap_composites_over_a_character_behind_it() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let y_top = 20u16;
    // `y_top - 3` is the northmost row a routed walker's feet can reach (the
    // footprint top minus OBSTACLE_PAD_PX + 1); closer rows sit inside the
    // blocked band no walker ever occupies.
    let cap_row = y_top - 3;
    let character = Rgb {
        r: 220,
        g: 40,
        b: 40,
    };
    let mut buf = RgbBuffer::filled(
        48,
        48,
        Rgb {
            r: 150,
            g: 110,
            b: 72,
        },
    );
    for x in 4..20 {
        buf.put(x, cap_row, character);
    }
    paint_glass_wall_h(&mut buf, theme, 0, 47, y_top);
    let after = buf.get(8, cap_row);
    assert_ne!(after, character, "glass must composite over the character");
    assert!(
        after.r < character.r && after.b > character.b,
        "frosted glass should cool the occluded pixel (red↓ blue↑): {after:?}"
    );
}

#[test]
fn glass_wall_v_composites_over_a_character_behind_its_north_cap() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (x_left, y_top, y_bot) = (10u16, 20u16, 40u16);
    // Row `y_top` is a seam glint (bright specular), so probe the NEXT cap row
    // at the soft east edge — the coolest column of the strip.
    let probe_col = x_left + crate::layout::WALL_THICK_V - 1;
    let probe_row = y_top + 1;
    let character = Rgb {
        r: 220,
        g: 40,
        b: 40,
    };
    let mut buf = RgbBuffer::filled(
        48,
        48,
        Rgb {
            r: 150,
            g: 110,
            b: 72,
        },
    );
    buf.put(probe_col, probe_row, character);
    paint_glass_wall_v(&mut buf, theme, x_left, y_top, y_bot);
    let after = buf.get(probe_col, probe_row);
    assert_ne!(after, character, "glass must composite over the character");
    assert!(
        after.r < character.r && after.b > character.b,
        "frosted glass should cool the occluded pixel (red↓ blue↑): {after:?}"
    );
}

#[test]
fn seat_sprite_maps_facing_to_sprite_and_flip() {
    use crate::layout::{Facing, WaypointKind};
    assert_eq!(
        seat_sprite(WaypointKind::Couch, Facing::North),
        ("back_couch", false),
        "couch's seated facing is North (window) → back_couch, same path as the sofa"
    );
    assert_eq!(
        seat_sprite(WaypointKind::MeetingSofa, Facing::North),
        ("back_couch", false)
    );
    assert_eq!(
        seat_sprite(WaypointKind::MeetingSofa, Facing::South),
        ("seated", false)
    );
    assert_eq!(
        seat_sprite(WaypointKind::MeetingChair, Facing::East),
        ("side_seated", false)
    );
    assert_eq!(
        seat_sprite(WaypointKind::MeetingChair, Facing::West),
        ("side_seated", true)
    );
}

#[test]
fn seat_sprite_in_pack_degrades_to_front_when_side_seated_is_missing() {
    use crate::layout::{Facing, WaypointKind};
    let full = crate::embedded_pack::load_sprite_pack(None).expect("pack");
    assert_eq!(
        seat_sprite_in_pack(&full, WaypointKind::MeetingChair, Facing::West),
        ("side_seated", true),
        "a pack WITH the profile sprite uses it"
    );
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/charpack");
    let old_pack = crate::embedded_pack::load_sprite_pack(Some(fixture)).expect("fixture pack");
    assert!(
        old_pack.animation("side_seated").is_none(),
        "fixture must lack the profile sprite for this test to bite"
    );
    assert_eq!(
        seat_sprite_in_pack(&old_pack, WaypointKind::MeetingChair, Facing::West),
        ("seated", false),
        "a pack WITHOUT it degrades to the front pose"
    );
}

fn make_slot(id: pixtuoid_core::AgentId, state: ActivityState) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: id,
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/x").as_path()),
        label: "x".into(),
        state,
        state_started_at: now,
        created_at: now,
        last_event_at: now,
        exiting_at: None,
        pending_idle_at: None,

        desk_index: GlobalDeskIndex(0),
        floor_idx: 0,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
        pid: None,
        model: None,
        effort: None,
        tokens_used: 0,
        last_usage: None,
    }
}

#[cfg(test)]
fn make_slot_cwd(id_path: &str, cwd: &str, unknown_cwd: bool) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path(id_path);
    let mut s = make_slot(id, ActivityState::Idle);
    s.cwd = std::sync::Arc::from(std::path::Path::new(cwd));
    s.unknown_cwd = unknown_cwd;
    s
}

fn base_palette() -> Palette {
    let mut p = Palette::new();
    p.insert(
        'B',
        Some(Rgb {
            r: 10,
            g: 20,
            b: 30,
        }),
    );
    p.insert(
        'H',
        Some(Rgb {
            r: 40,
            g: 50,
            b: 60,
        }),
    );
    p.insert(
        'S',
        Some(Rgb {
            r: 70,
            g: 80,
            b: 90,
        }),
    );
    p.insert(
        'X',
        Some(Rgb {
            r: 99,
            g: 99,
            b: 99,
        }),
    );
    p
}

#[test]
fn agent_palette_is_deterministic_per_id() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
    let base = base_palette();
    let a = agent_palette(
        &base,
        &make_slot(id, ActivityState::Idle),
        None,
        crate::burn::BurnTier::Normal,
    );
    let b = agent_palette(
        &base,
        &make_slot(id, ActivityState::Idle),
        None,
        crate::burn::BurnTier::Normal,
    );
    assert_eq!(a.get('B'), b.get('B'));
    assert_eq!(a.get('H'), b.get('H'));
    assert_eq!(a.get('S'), b.get('S'));
}

#[test]
fn agent_palette_overrides_only_bhs_keys() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
    let base = base_palette();
    let p = agent_palette(
        &base,
        &make_slot(id, ActivityState::Idle),
        None,
        crate::burn::BurnTier::Normal,
    );
    assert_eq!(
        p.get('X'),
        Some(Some(Rgb {
            r: 99,
            g: 99,
            b: 99
        }))
    );
    assert_ne!(
        p.get('B'),
        Some(Some(Rgb {
            r: 10,
            g: 20,
            b: 30
        }))
    );
    assert_ne!(
        p.get('H'),
        Some(Some(Rgb {
            r: 40,
            g: 50,
            b: 60
        }))
    );
    assert_ne!(
        p.get('S'),
        Some(Some(Rgb {
            r: 70,
            g: 80,
            b: 90
        }))
    );
}

#[test]
fn agent_palette_glow_tint_shifts_skin_toward_given_color() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
    let base = base_palette();
    let slot = make_slot(id, ActivityState::Idle);
    let unlit = agent_palette(&base, &slot, None, crate::burn::BurnTier::Normal);
    let green_glow = agent_palette(
        &base,
        &slot,
        Some(Rgb {
            r: 140,
            g: 240,
            b: 170,
        }),
        crate::burn::BurnTier::Normal,
    );
    let blue_glow = agent_palette(
        &base,
        &slot,
        Some(Rgb {
            r: 100,
            g: 160,
            b: 255,
        }),
        crate::burn::BurnTier::Normal,
    );
    assert_eq!(unlit.get('B'), green_glow.get('B'));
    assert_eq!(unlit.get('H'), green_glow.get('H'));
    assert_eq!(unlit.get('P'), green_glow.get('P'));
    let (Some(Some(Rgb { r: _, g: ug, b: _ })), Some(Some(Rgb { r: _, g: gg, b: _ }))) =
        (unlit.get('S'), green_glow.get('S'))
    else {
        panic!("S key missing")
    };
    assert!(
        gg > ug,
        "green glow should push skin green (lit={gg}, unlit={ug})"
    );
    let (Some(Some(Rgb { r: _, g: _, b: ub })), Some(Some(Rgb { r: _, g: _, b: bb }))) =
        (unlit.get('S'), blue_glow.get('S'))
    else {
        panic!("S key missing")
    };
    assert!(
        bb > ub,
        "blue glow should push skin blue (lit={bb}, unlit={ub})"
    );
}

#[test]
fn tool_glow_tint_maps_known_tools() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/t.jsonl");
    let edit_slot = make_slot(
        id,
        ActivityState::Active {
            tool_use_id: None,
            detail: Some(Arc::from("Edit src/main.rs")),
            kind: ToolKind::Edit,
        },
    );
    let bash_slot = make_slot(
        id,
        ActivityState::Active {
            tool_use_id: None,
            detail: Some(Arc::from("Bash: ls")),
            kind: ToolKind::Bash,
        },
    );
    let idle_slot = make_slot(id, ActivityState::Idle);
    let glow = &crate::theme::NORMAL.tool_glow;
    let edit_tint = palette::tool_glow_tint(&edit_slot, glow);
    let bash_tint = palette::tool_glow_tint(&bash_slot, glow);
    let idle_tint = palette::tool_glow_tint(&idle_slot, glow);
    assert!(edit_tint.is_some(), "Edit should produce glow");
    assert!(bash_tint.is_some(), "Bash should produce glow");
    assert_eq!(idle_tint, None, "Idle should produce no glow");
    assert_ne!(edit_tint, bash_tint, "Edit and Bash should differ");
}

#[test]
fn recolor_frame_substitutes_bhs_pixels() {
    let base = base_palette();
    let mut agent_pal = base.clone();
    agent_pal.insert('B', Some(Rgb { r: 200, g: 0, b: 0 }));
    agent_pal.insert('H', Some(Rgb { r: 0, g: 200, b: 0 }));
    agent_pal.insert('S', Some(Rgb { r: 0, g: 0, b: 200 }));

    let frame = Frame::from_pixels(
        5,
        1,
        vec![
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30,
            }),
            Some(Rgb {
                r: 40,
                g: 50,
                b: 60,
            }),
            Some(Rgb {
                r: 70,
                g: 80,
                b: 90,
            }),
            Some(Rgb {
                r: 123,
                g: 45,
                b: 67,
            }),
            None,
        ],
    );

    let out = recolor_frame(&frame, &agent_pal, &base);
    assert_eq!(out.width(), 5);
    assert_eq!(out.height(), 1);
    assert_eq!(out.as_slice()[0], Some(Rgb { r: 200, g: 0, b: 0 }));
    assert_eq!(out.as_slice()[1], Some(Rgb { r: 0, g: 200, b: 0 }));
    assert_eq!(out.as_slice()[2], Some(Rgb { r: 0, g: 0, b: 200 }));
    assert_eq!(
        out.as_slice()[3],
        Some(Rgb {
            r: 123,
            g: 45,
            b: 67
        })
    );
    assert_eq!(out.as_slice()[4], None);
}

#[test]
fn recolor_frame_handles_palette_with_no_overrides() {
    let base = base_palette();
    let frame = Frame::from_pixels(
        3,
        1,
        vec![
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30,
            }),
            Some(Rgb {
                r: 40,
                g: 50,
                b: 60,
            }),
            Some(Rgb {
                r: 70,
                g: 80,
                b: 90,
            }),
        ],
    );
    let out = recolor_frame(&frame, &base, &base);
    assert_eq!(out.as_slice(), frame.as_slice());
}

fn drawable(anchor_y: u16) -> Drawable<'static> {
    Drawable {
        anchor_y,
        kind: DrawableKind::MeetingTable {
            pos: Point { x: 0, y: 0 },
        },
    }
}

#[test]
fn drawables_sort_ascending_by_anchor_y() {
    let mut v = [drawable(30), drawable(10), drawable(20)];
    v.sort_by_key(|d| d.anchor_y);
    let ys: Vec<u16> = v.iter().map(|d| d.anchor_y).collect();
    assert_eq!(ys, [10, 20, 30]);
}

#[test]
fn drawables_sort_is_stable_on_ties() {
    let mut v = [
        Drawable {
            anchor_y: 10,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 1, y: 0 },
            },
        },
        Drawable {
            anchor_y: 10,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 2, y: 0 },
            },
        },
        Drawable {
            anchor_y: 10,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 3, y: 0 },
            },
        },
    ];
    v.sort_by_key(|d| d.anchor_y);
    let xs: Vec<u16> = v
        .iter()
        .map(|d| match &d.kind {
            DrawableKind::MeetingTable { pos } => pos.x,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(xs, [1, 2, 3]);
}

#[test]
fn back_view_meeting_sofa_sorts_over_its_sitter() {
    let sofa_y: u16 = 40;
    let sitter_anchor_y = (sofa_y - 7) + 9; // back_couch_anchor + sprite_h
    let back_sofa_anchor_y = sofa_y + 3; // faces_away bump
    let front_sofa_anchor_y = sofa_y + 2; // sitter-on-top default
    assert!(
        back_sofa_anchor_y > sitter_anchor_y,
        "back-view sofa must sort AFTER its sitter (paint on top): \
         sofa={back_sofa_anchor_y}, sitter={sitter_anchor_y}"
    );
    assert!(
        front_sofa_anchor_y <= sitter_anchor_y,
        "front-view sofa must not sort after its sitter: \
         sofa={front_sofa_anchor_y}, sitter={sitter_anchor_y}"
    );
}

#[test]
fn center_pin_south_offset_lands_on_the_sprite_south_row() {
    for h in 1u16..=16 {
        let expected_south = h - 1 - h / 2;
        assert_eq!(
            center_pin_south_offset(h),
            expected_south,
            "h={h}: z-key must land on the sprite south row, not one past it",
        );
    }
}

#[test]
fn pet_z_anchor_tracks_the_selected_anim_sprite_height() {
    let pack = crate::embedded_pack::test_default_pack();
    let pos = Point { x: 40, y: 30 };
    let anim_h = |name: &str| {
        pack.animation(name)
            .and_then(|a| a.frames.first())
            .map(|f| f.height())
            .unwrap_or_else(|| panic!("missing pet anim {name}"))
    };
    for &kind in crate::pet::PetKind::ALL {
        let sleep_h = anim_h(kind.sleep_anim());
        let sleep = z_sort_row(Anchor::Center, pos, sleep_h);
        let walk = z_sort_row(Anchor::Center, pos, anim_h(kind.walk_anim()));
        let sit = z_sort_row(Anchor::Center, pos, anim_h(kind.sit_anim()));
        assert!(
            sleep <= walk && sleep <= sit,
            "{kind:?}: shorter sleep sprite must not sort south of walk/sit \
             (sleep={sleep}, walk={walk}, sit={sit})",
        );
        assert_eq!(
            sleep,
            pos.y + center_pin_south_offset(sleep_h),
            "{kind:?}: sleep pet must land on its sprite's south row",
        );
    }
}

#[test]
fn floor_lamp_south_offset_is_the_base_row() {
    // The lamp's halo / shadow / z-anchor all read this, so a visual-height
    // edit surfaces here rather than as a floating halo.
    assert_eq!(floor_lamp_south_offset(), 4);
}

#[test]
fn waypoint_depth_baseline_is_center_pinned_sprite_south() {
    use crate::layout::{furniture_def, WaypointKind};
    let south_off = |k: WaypointKind| {
        furniture_def(k.furniture())
            .footprint
            .expect("has footprint")
            .h
            / 2
            - 1
    };
    assert_eq!(south_off(WaypointKind::VendingMachine), 2);
    assert_eq!(south_off(WaypointKind::Printer), 1);
}

#[test]
fn desk_walk_anchor_settles_exactly_on_the_seat() {
    for desk in [
        Point { x: 40, y: 30 },
        Point { x: 100, y: 60 },
        Point { x: 7, y: 5 }, // near-origin: saturating_sub edge
    ] {
        for w in [CHARACTER_SPRITE_W, 10] {
            // Only X has teeth — on Y both facings reduce to the same `saturating_sub`.
            // Y drift: `a_back_turned_seat_puts_the_occupant_past_the_desk_body`.
            for facing in [crate::layout::Facing::South, crate::layout::Facing::North] {
                assert_eq!(
                    walking_anchor(crate::layout::desk_walk_anchor_facing(desk, facing), w),
                    seated_anchor_facing(desk, w, facing),
                    "walking_anchor(desk_walk_anchor_facing({desk:?}, {facing:?}), {w}) \
                     must equal seated_anchor_facing",
                );
            }
        }
    }
}

#[test]
fn seated_foot_cell_settles_exactly_on_the_render_anchor() {
    use crate::layout::{seated_foot_cell, Furniture};
    for pos in [
        Point { x: 40, y: 30 },
        Point { x: 100, y: 60 },
        Point { x: 6, y: 8 }, // near-origin: saturating_sub edge
    ] {
        for w in [CHARACTER_SPRITE_W, 10] {
            for f in [Furniture::Couch, Furniture::MeetingSofa] {
                let s = seated_foot_cell(f, pos).expect("occupies_pos seat");
                assert_eq!(
                    walking_anchor(s, w),
                    back_couch_anchor(pos, w),
                    "{f:?}: walking_anchor(S={s:?}) must equal back_couch_anchor(pos={pos:?}) w={w}",
                );
            }
            let s = seated_foot_cell(Furniture::MeetingChair, pos).expect("occupies_pos seat");
            assert_eq!(
                walking_anchor(s, w),
                back_couch_anchor(pos, w),
                "MeetingChair: walking_anchor(S={s:?}) must equal back_couch_anchor(pos={pos:?}) w={w}",
            );
            let sd = seated_foot_cell(Furniture::Desk, pos).expect("desk is occupies_pos");
            assert_eq!(
                walking_anchor(sd, w),
                seated_anchor_facing(pos, w, crate::layout::Facing::South),
                "Desk: walking_anchor(seated_foot_cell)={:?} must equal seated_anchor",
                walking_anchor(sd, w),
            );
        }
        assert_eq!(seated_foot_cell(Furniture::Pantry, pos), None);
        assert_eq!(seated_foot_cell(Furniture::VendingMachine, pos), None);
    }
}

#[test]
fn settle_view_matches_the_seated_view_for_every_seat() {
    use crate::layout::{Facing, WaypointKind, TEST_DEFAULT_DESKS};
    let l = Layout::compute(192, 158, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let seats: Vec<_> = l
        .waypoints
        .iter()
        .filter(|w| crate::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some())
        .collect();
    assert!(
        seats.iter().any(
            |w| matches!(w.kind, WaypointKind::Couch | WaypointKind::MeetingSofa)
                && w.facing == Facing::North
        ),
        "this layout size must have a window-facing (North) seat to exercise the fix"
    );
    for w in &seats {
        let foot = crate::layout::seated_foot_cell(w.kind.furniture(), w.pos)
            .expect("seat occupies_pos → has a settle foot cell");
        let view = SeatView::of(w.kind, w.facing);
        assert_eq!(
            settle_seat_view(foot, &l),
            Some((view, view.z_key_for_seat(w.pos))),
            "settle onto {:?}@{:?} must use the seat view {view:?}",
            w.kind,
            w.pos
        );
        assert!(
            matches!(
                w.kind,
                WaypointKind::Couch
                    | WaypointKind::MeetingSofa
                    | WaypointKind::MeetingChair
                    | WaypointKind::Island
            ),
            "seat kind {:?} has a settle foot-cell but is not explicitly handled \
             in SeatView::of — add an arm there",
            w.kind
        );
        let seated_is_back = view.seated_sprite().0 == "back_couch";
        let (settle_is_back, _) = view.settle_walk();
        assert_eq!(
            seated_is_back, settle_is_back,
            "{:?}: seated render and sit-down settle must share orientation",
            w.kind
        );
        if foot != w.pos {
            assert_eq!(
                settle_seat_view(w.pos, &l),
                None,
                "seat centre {:?} is not a settle foot cell",
                w.pos
            );
        }
    }
}

#[test]
fn island_settle_z_stays_behind_the_countertop() {
    use crate::layout::{Anchor, Furniture, WaypointKind, TEST_DEFAULT_DESKS};
    let mut exercised = false;
    for seed in 0..5u64 {
        let Some(l) = Layout::compute_with_seed(240, 160, Some(TEST_DEFAULT_DESKS), seed) else {
            continue;
        };
        let Some(island) = l.pantry.and_then(|p| p.kitchen_island) else {
            continue;
        };
        exercised = true;
        let island_z = crate::layout::z_sort_row(
            Anchor::Center,
            island,
            crate::layout::furniture_def(Furniture::KitchenIsland)
                .visual
                .h,
        );
        for wp in l
            .waypoints
            .iter()
            .filter(|w| matches!(w.kind, WaypointKind::Island))
        {
            let (_, z) =
                settle_seat_view(wp.pos, &l).expect("island stand foot-cell == pos, so it settles");
            assert_eq!(
                z, wp.pos.y,
                "island stand glide z must be the plain feet row (the settled \
                 AtWaypoint key), not a Side-style +3"
            );
            assert!(
                z < island_z,
                "stand z {z} must sort BEHIND the island's south-row key {island_z}"
            );
        }
    }
    assert!(exercised, "no seed hosted the island — test lost its teeth");
}

#[test]
fn settle_seat_view_recognizes_the_home_desk() {
    use crate::layout::TEST_DEFAULT_DESKS;
    use crate::layout::{desk_walk_anchor_facing, Furniture};
    let l = Layout::compute(192, 158, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let desk = *l.home_desks.first().expect("at least one home desk");
    let chair = desk_walk_anchor_facing(desk, l.desk_facing_at(desk));
    // Pinning `Front` unconditionally would assert the pre-facing world.
    let want_view = if l.desk_facing_at(desk) == crate::layout::Facing::North {
        SeatView::Back
    } else {
        SeatView::Front
    };
    assert_eq!(
        settle_seat_view(chair, &l),
        Some((want_view, chair.y)),
        "the desk chair {chair:?} must settle as {want_view:?} at its own chair row"
    );
    // `seated_foot_cell` is facing-BLIND — it takes a kind and a position, so its
    // desk arm can only answer for the viewer-facing seat.
    assert_eq!(
        crate::layout::seated_foot_cell(Furniture::Desk, desk),
        Some(crate::layout::desk_walk_anchor_facing(
            desk,
            crate::layout::Facing::South
        ))
    );
    assert_eq!(
        settle_seat_view(desk, &l),
        None,
        "the desk corner is not the chair"
    );
}

#[test]
fn desk_settle_z_key_matches_the_seated_arm() {
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let seated_arm_z = seated_anchor_facing(desk, w, crate::layout::Facing::South).y + 12;
            assert_eq!(
                crate::layout::desk_walk_anchor_facing(desk, crate::layout::Facing::South).y,
                seated_arm_z,
                "desk settle z-key must equal the SeatedIdle/Typing arm z-key"
            );
            let visual_h = crate::layout::desk_furniture_def().visual.h;
            assert!(
                crate::layout::desk_walk_anchor_facing(desk, crate::layout::Facing::South).y
                    < desk.y + visual_h,
                "desk sitter must sort behind the desk furniture"
            );
        }
    }
}

#[test]
fn sit_arc_z_key_is_stable_and_on_the_right_side_of_its_furniture() {
    use crate::layout::{
        furniture_def, z_sort_row, Anchor, Facing, Furniture, WaypointKind, TEST_DEFAULT_DESKS,
    };
    let l = Layout::compute(192, 158, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let mut saw_back = false;
    for w in l
        .waypoints
        .iter()
        .filter(|w| crate::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some())
    {
        let view = SeatView::of(w.kind, w.facing);
        let z = view.z_key_for_seat(w.pos);

        let historical = match view {
            // back_couch_anchor.y + sprite_h(9) = (pos.y - 7) + 9. SideSeated
            // shares Front's seat anchor + bottom-row geometry by design.
            SeatView::Front | SeatView::Back | SeatView::SideSeated { .. } => {
                back_couch_anchor(w.pos, CHARACTER_SPRITE_W).y + 9
            }
            // waypoint_anchor.y + sprite_h(12) + 3 = (pos.y - 12) + 12 + 3
            SeatView::Side { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12 + 3,
            // waypoint_anchor.y + sprite_h(12) = pos.y — the AtWaypoint default.
            SeatView::Stander { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12,
        };
        assert_eq!(
            z, historical,
            "{:?}@{:?}: seat z-key {z} must equal the historical AtWaypoint key {historical}",
            w.kind, w.pos
        );

        match w.kind {
            WaypointKind::Couch => {
                let couch_z = z_sort_row(
                    Anchor::Center,
                    w.pos,
                    furniture_def(Furniture::Couch).visual.h,
                );
                assert!(
                    z < couch_z,
                    "couch sitter z {z} must be BEHIND the couch back {couch_z}"
                );
                saw_back = true;
            }
            WaypointKind::MeetingSofa => {
                // Furniture z-key: faces_away (North) → sofa.y+3; else sofa.y+2.
                if w.facing == Facing::North {
                    assert!(z < w.pos.y + 3, "back sofa sitter z {z} must be < sofa.y+3");
                    saw_back = true;
                } else {
                    assert!(
                        z <= w.pos.y + 2,
                        "front sofa sitter z {z} must be <= sofa.y+2"
                    );
                }
            }
            WaypointKind::MeetingChair => {
                assert!(
                    z > w.pos.y + 1,
                    "chair sitter z {z} must clear the chair body at pos.y+1"
                );
            }
            _ => {}
        }
    }
    assert!(
        saw_back,
        "layout must contain a back-view seat to exercise the flicker fix"
    );
}

#[test]
fn desk_occupant_always_sorts_behind_its_desk() {
    let visual_h = crate::layout::desk_furniture_def().visual.h;
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let desk_furniture_z = desk.y + visual_h;
            let seated_z = seated_anchor_facing(desk, w, crate::layout::Facing::South).y + 12;
            let standing_z = standing_at_desk_anchor(desk, w).y + 12;
            assert!(
                seated_z < desk_furniture_z,
                "seated desk occupant z {seated_z} must be BEHIND the desk {desk_furniture_z}"
            );
            assert!(
                standing_z < desk_furniture_z,
                "standing desk occupant z {standing_z} must be BEHIND the desk {desk_furniture_z}"
            );
        }
    }
}

/// The geometry table's desk height must match the ART's, or the z-key sorts on
/// a south row the sprite does not reach. The `- 1`: `desk` blits at `desk.y - 1`
/// (top row is the north-overhanging bezel), so it covers `height - 1` from `desk.y`.
#[test]
fn desk_z_key_is_the_visual_south() {
    let pack = crate::embedded_pack::test_default_pack();
    let art = pack
        .animation("desk")
        .and_then(|a| a.frames.first())
        .expect("the embedded pack ships a desk");
    assert_eq!(
        crate::layout::desk_furniture_def().visual.h,
        art.height() - 1,
        "the desk's visual height must equal the rows its sprite covers from \
         desk.y down (sprite {} rows, blitted one above desk.y)",
        art.height()
    );
}

#[test]
fn every_pod_occludes_via_overhang() {
    use crate::layout::{furniture_def, PodDecor, Size};
    assert_eq!(
        PodDecor::ALL.len(),
        5,
        "PodDecor variant added/removed — update ALL (and this count)"
    );
    for &kind in PodDecor::ALL {
        let def = furniture_def(kind.furniture());
        assert!(
            def.visual.h > 0,
            "{kind:?}: pod decor needs a non-zero visual height for the z-sort"
        );
        let Size { h: fh, .. } = def.footprint.expect("aisle pod has a ground footprint");
        assert!(
            def.visual.h > fh,
            "{kind:?}: aisle pod must overhang its footprint to occlude (visual.h {} > footprint.h {fh})",
            def.visual.h
        );
    }
}

#[test]
fn back_view_seats_sort_over_their_sitter() {
    let base: u16 = 40;
    let sitter = (base - 7) + 9; // = base + 2
    let couch_furniture = base + 3; // lounge couch (MeetingSofa{mirrored:true})
    let back_meeting_sofa = base + 3; // faces_away meeting sofa
    assert!(couch_furniture > sitter, "couch must sort over its sitter");
    assert!(
        back_meeting_sofa > sitter,
        "north meeting sofa must sort over its sitter"
    );
}

#[test]
fn character_anchor_y_exceeds_desk_when_south_of_it() {
    let desk_y: u16 = 20;
    let desk_anchor_y = desk_y
        + crate::layout::furniture_def(crate::layout::Furniture::Desk)
            .visual
            .h;
    let char_feet_anchor = (desk_y + 10) + 12;
    assert!(
        char_feet_anchor > desk_anchor_y,
        "walker south of desk must sort after it: char={char_feet_anchor}, desk={desk_anchor_y}"
    );
}

#[test]
fn character_anchor_y_below_desk_when_seated_at_it() {
    let desk_y: u16 = 20;
    let seated_anchor = seated_anchor_facing(
        Point { x: 0, y: desk_y },
        CHARACTER_SPRITE_W,
        crate::layout::Facing::South,
    );
    let char_feet_anchor = seated_anchor.y + 12;
    let desk_anchor_y = desk_y
        + crate::layout::furniture_def(crate::layout::Furniture::Desk)
            .visual
            .h;
    assert!(
        char_feet_anchor < desk_anchor_y,
        "seated char must sort before desk: char={char_feet_anchor}, desk={desk_anchor_y}"
    );
}

fn entry_slot(created_at_ms_ago: u64, now: SystemTime) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path("/door.jsonl");
    let mut s = make_slot(id, ActivityState::Idle);
    s.created_at = now - std::time::Duration::from_millis(created_at_ms_ago);
    s
}

fn exit_slot(exit_ms_ago: u64, now: SystemTime) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path("/exit.jsonl");
    let mut s = make_slot(id, ActivityState::Idle);
    s.created_at = now - std::time::Duration::from_secs(300);
    s.exiting_at = Some(now - std::time::Duration::from_millis(exit_ms_ago));
    s
}

#[test]
fn door_frame_closed_when_no_agents() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    assert_eq!(compute_door_frame_idx(&[], now, 0), 0);
}

#[test]
fn door_frame_just_spawned_is_half_open() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // 50 ms into the 200 ms opening ramp — first half = frame 1.
    let slot = entry_slot(50, now);
    assert_eq!(compute_door_frame_idx(&[slot], now, 0), 1);
}

#[test]
fn door_frame_after_opening_ramp_is_fully_open() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // 150 ms (still inside opening ramp but past midpoint) → frame 2.
    let s1 = entry_slot(150, now);
    assert_eq!(compute_door_frame_idx(&[s1], now, 0), 2);
    // 2 s into the 4 s window → fully open.
    let s2 = entry_slot(2_000, now);
    assert_eq!(compute_door_frame_idx(&[s2], now, 0), 2);
}

#[test]
fn door_frame_closing_then_closed_at_end_of_entry() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // 150 ms left in the entry window → closing ramp first half → frame 1.
    let mid_close = entry_slot(pose::ENTRY_ANIMATION_MS - 150, now);
    assert_eq!(compute_door_frame_idx(&[mid_close], now, 0), 1);
    // 50 ms left → closing ramp final half → frame 0 (closed).
    let near_end = entry_slot(pose::ENTRY_ANIMATION_MS - 50, now);
    assert_eq!(compute_door_frame_idx(&[near_end], now, 0), 0);
}

#[test]
fn door_frame_expired_entry_contributes_nothing() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // Older than the 4 s entry window → no contribution.
    let old = entry_slot(pose::ENTRY_ANIMATION_MS + 1, now);
    assert_eq!(compute_door_frame_idx(&[old], now, 0), 0);
}

#[test]
fn door_frame_exit_window_uses_4500ms_total() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // 2 s into a 4.5 s exit window → mid-flight → fully open.
    let exiting = exit_slot(2_000, now);
    assert_eq!(compute_door_frame_idx(&[exiting], now, 0), 2);
}

#[test]
fn door_frame_takes_max_across_agents() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let opening = entry_slot(50, now);
    let open = entry_slot(2_000, now);
    assert_eq!(compute_door_frame_idx(&[opening, open], now, 0), 2);
}

#[test]
fn door_frame_uses_physics_window_when_nonzero() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // A short physics window (2500 ms) replaces ENTRY_ANIMATION_MS as the total.
    let short_window_ms: u64 = 2_500;
    // elapsed 3000 > total 2500 → remaining 0 → closed.
    let slot = entry_slot(3_000, now);
    let frame = compute_door_frame_idx(&[slot], now, short_window_ms);
    assert_eq!(
        frame, 0,
        "with short physics window elapsed>total should yield closed door, got frame {frame}"
    );

    // 500 ms into the 2500 ms window → still mid-flight.
    let slot_mid = entry_slot(500, now);
    let frame_mid = compute_door_frame_idx(&[slot_mid], now, short_window_ms);
    assert_eq!(
        frame_mid, 2,
        "500ms into 2500ms window should be fully open, got frame {frame_mid}"
    );
}

#[test]
fn weather_state_covers_all_variants() {
    let mut seen = std::collections::HashSet::new();
    let base = SystemTime::UNIX_EPOCH;
    for cycle in 0..200u64 {
        let now = base + std::time::Duration::from_secs(cycle * 600);
        seen.insert(std::mem::discriminant(&background::weather_state(now)));
    }
    assert!(
        seen.len() >= 8,
        "expected all 8 weather variants in 200 cycles, got {}",
        seen.len()
    );
}

#[test]
fn weather_state_deterministic() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
    let a = background::weather_state(now);
    let b = background::weather_state(now);
    assert_eq!(a, b);
}

#[test]
fn weather_state_changes_across_cycles() {
    let mut states = Vec::new();
    let base = SystemTime::UNIX_EPOCH;
    for cycle in 0..20u64 {
        states.push(background::weather_state(
            base + std::time::Duration::from_secs(cycle * 600),
        ));
    }
    let unique: std::collections::HashSet<_> = states.iter().map(std::mem::discriminant).collect();
    assert!(unique.len() >= 2, "weather should vary across cycles");
}

#[test]
fn waypoint_rank_offset_x_decollision_table() {
    use super::anchors::waypoint_rank_offset_x;
    use crate::layout::WaypointKind;
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Couch, 0), 0);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 0), 0);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 1), 9);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 2), -9);
    assert_eq!(
        waypoint_rank_offset_x(WaypointKind::Pantry, 5),
        0,
        "rank >2 collapses to 0"
    );
}

#[test]
fn no_exclusive_waypoint_kind_ever_steps_aside() {
    use super::anchors::waypoint_rank_offset_x;
    use crate::layout::{furniture_def, WaypointKind};
    let mut exclusive = 0;
    let (mut saw_booth, mut saw_shareable_steps) = (false, false);
    for &kind in WaypointKind::ALL {
        if furniture_def(kind.furniture()).exclusive {
            exclusive += 1;
            if matches!(kind, WaypointKind::PhoneBooth) {
                saw_booth = true;
            }
            for rank in 0..4 {
                assert_eq!(
                    waypoint_rank_offset_x(kind, rank),
                    0,
                    "{kind:?} is exclusive — rank {rank} must not slide it off the spot"
                );
            }
        } else if waypoint_rank_offset_x(kind, 1) != 0 {
            saw_shareable_steps = true;
        }
    }
    assert!(
        exclusive >= 6,
        "expected couch/sofa/chair/island + booth + standing desk, got {exclusive}"
    );
    assert!(saw_booth, "phone booth must be an exclusive spot");
    assert!(
        saw_shareable_steps,
        "a shareable queue spot (pantry/vending/printer/snack) must still step aside"
    );
}

#[test]
fn kind_derivation_reproduces_the_string_parse_tint_for_representative_displays() {
    use pixtuoid_core::ToolDetail;
    let id = pixtuoid_core::AgentId::from_transcript_path("/g.jsonl");
    let glow = &crate::theme::NORMAL.tool_glow;
    let active = |detail: Option<&ToolDetail>| {
        make_slot(
            id,
            ActivityState::Active {
                tool_use_id: None,
                detail: detail.map(|d| Arc::from(d.display())),
                kind: detail.map_or(ToolKind::Other, ToolKind::from_detail),
            },
        )
    };
    let generic = |display: &str| ToolDetail::Generic {
        display: display.into(),
    };
    let table: &[(Option<ToolDetail>, Rgb)] = &[
        (Some(ToolDetail::Task), glow.agent),
        (Some(generic("Edit src/main.rs")), glow.edit),
        (Some(generic("Write: src/foo.rs")), glow.edit),
        (Some(generic("MultiEdit lib.rs")), glow.edit),
        (Some(generic("Read: README.md")), glow.read),
        (Some(generic("Bash: cargo test")), glow.bash),
        (Some(generic("Grep: TODO")), glow.grep),
        (Some(generic("Glob **/*.rs")), glow.grep),
        (Some(generic("WebFetch https://x")), glow.default),
        (None, glow.default),
    ];
    for (detail, expected) in table {
        assert_eq!(
            palette::tool_glow_tint(&active(detail.as_ref()), glow),
            Some(*expected),
            "display {:?} must keep its pre-ToolKind tint",
            detail.as_ref().map(ToolDetail::display),
        );
    }
    // A Generic tool that merely SPELLS a delegation word is NOT kind Task —
    // impossible from production decoders, which type every dispatch as
    // ToolDetail::Task upstream.
    assert_eq!(
        palette::tool_glow_tint(&active(Some(&generic("Delegating imposter"))), glow),
        Some(glow.default)
    );
}

#[test]
fn tool_glow_for_kind_is_the_shared_kind_to_hue_map() {
    use pixtuoid_core::state::ToolKind;
    let glow = &crate::theme::NORMAL.tool_glow;
    assert_eq!(palette::tool_glow_for_kind(ToolKind::Edit, glow), glow.edit);
    assert_eq!(palette::tool_glow_for_kind(ToolKind::Read, glow), glow.read);
    assert_eq!(palette::tool_glow_for_kind(ToolKind::Bash, glow), glow.bash);
    assert_eq!(
        palette::tool_glow_for_kind(ToolKind::Task, glow),
        glow.agent
    );
    assert_eq!(
        palette::tool_glow_for_kind(ToolKind::Search, glow),
        glow.grep
    );
    assert_eq!(
        palette::tool_glow_for_kind(ToolKind::Other, glow),
        glow.default
    );
    let id = pixtuoid_core::AgentId::from_transcript_path("/g.jsonl");
    let edit = make_slot(
        id,
        ActivityState::Active {
            tool_use_id: None,
            detail: None,
            kind: ToolKind::Edit,
        },
    );
    assert_eq!(palette::tool_glow_tint(&edit, glow), Some(glow.edit));
    assert_eq!(
        palette::tool_glow_tint(&make_slot(id, ActivityState::Idle), glow),
        None
    );
}

#[test]
fn degraded_pixel_desaturates_reddens_and_dims() {
    // Expected value hand-traced through the three blend stages: desaturate,
    // red tint, dim.
    assert_eq!(
        palette::degraded_pixel(Rgb {
            r: 255,
            g: 255,
            b: 255
        }),
        Rgb {
            r: 171,
            g: 130,
            b: 130
        },
    );
    let out = palette::degraded_pixel(Rgb { r: 0, g: 255, b: 0 });
    assert!(
        out.r > out.b,
        "red bias must lift r above b for a pure-green input: {out:?}"
    );
    assert!(
        out.r > 0,
        "the red bias must raise r above the input's 0: {out:?}"
    );
    assert!(
        out.g < 255 && out.r < 255 && out.b < 255,
        "every channel dimmed below its bright max: {out:?}"
    );
}

#[test]
fn degraded_frame_transforms_opaque_pixels_and_preserves_transparency_and_dims() {
    let frame = Frame::from_pixels(
        2,
        1,
        vec![
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255,
            }),
            None,
        ],
    );
    let out = palette::degraded_frame(&frame);
    assert_eq!(out.width(), 2);
    assert_eq!(out.height(), 1);
    assert_eq!(
        out.as_slice()[0],
        Some(palette::degraded_pixel(Rgb {
            r: 255,
            g: 255,
            b: 255
        }))
    );
    assert_eq!(
        out.as_slice()[0],
        Some(Rgb {
            r: 171,
            g: 130,
            b: 130
        })
    );
    assert_eq!(
        out.as_slice()[1],
        None,
        "transparent pixel must stay transparent"
    );
    assert_ne!(out.as_slice()[0], frame.as_slice()[0]);
}

#[test]
fn seat_view_of_obstacle_kinds_is_upright_unflipped() {
    use crate::layout::{Facing, WaypointKind};
    for kind in [
        WaypointKind::Pantry,
        WaypointKind::PhoneBooth,
        WaypointKind::StandingDesk,
        WaypointKind::VendingMachine,
        WaypointKind::Printer,
    ] {
        assert_eq!(
            SeatView::of(kind, Facing::South),
            SeatView::Side { flip: false },
            "{kind:?} must map to the upright default",
        );
    }
}

#[test]
fn top_tier_slot_paints_ember_hair_and_a_flame_crown() {
    use pixtuoid_core::state::EffortObservation;
    use std::time::Duration;
    let pack = crate::embedded_pack::test_default_pack();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let black = Rgb { r: 0, g: 0, b: 0 };
    let anchor = Point { x: 8, y: 8 };
    let mut slot = make_slot(
        pixtuoid_core::AgentId::from_parts("claude-code", "ses_burn"),
        ActivityState::Idle,
    );

    let render = |slot: &pixtuoid_core::AgentSlot| {
        let mut buf = RgbBuffer::filled(32, 32, black);
        paint_character_at(
            &mut buf,
            "seated",
            0,
            anchor,
            slot,
            &pack,
            false,
            None,
            &mut FrameCache::new(),
            now,
        );
        buf
    };
    let has = |buf: &RgbBuffer, c: Rgb| {
        (0..buf.height()).any(|y| (0..buf.width()).any(|x| buf.get(x, y) == c))
    };
    const EMBER: Rgb = super::effects::FLAME_DEEP;
    const TIP: Rgb = super::effects::FLAME_TIP;

    let plain = render(&slot);
    assert!(
        !has(&plain, EMBER) && !has(&plain, TIP),
        "Normal must not burn"
    );

    slot.model = Some("claude-fable-5".into());
    let ember = render(&slot);
    assert!(has(&ember, EMBER), "Premium recolors the hair to ember");
    assert!(!has(&ember, TIP), "Premium must not flame");
    assert_ne!(plain.as_slice(), ember.as_slice());

    slot.effort = Some(EffortObservation::new("ultra".into(), now));
    let burning = render(&slot);
    assert!(has(&burning, TIP), "Top paints flame tips");
    let above = (0..anchor.y).any(|y| (0..32).any(|x| burning.get(x, y) != black));
    assert!(above, "the crown must rise above the sprite's top row");

    slot.effort = Some(EffortObservation::new(
        "ultra".into(),
        now - Duration::from_secs(crate::burn::EFFORT_TTL_SECS + 1),
    ));
    let decayed = render(&slot);
    assert!(!has(&decayed, TIP), "stale effort must decay the flame");
    assert!(has(&decayed, EMBER), "…back to ember hair");
}

#[test]
fn paint_character_at_missing_anim_is_a_noop() {
    let pack = crate::embedded_pack::test_default_pack();
    let mut cache = FrameCache::new();
    let id = pixtuoid_core::AgentId::from_transcript_path("/c.jsonl");
    let slot = make_slot(id, ActivityState::Idle);
    let bg = Rgb { r: 4, g: 5, b: 6 };
    let mut buf = RgbBuffer::filled(40, 40, bg);
    paint_character_at(
        &mut buf,
        "does_not_exist",
        0,
        Point { x: 20, y: 20 },
        &slot,
        &pack,
        false,
        None,
        &mut cache,
        SystemTime::UNIX_EPOCH,
    );
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            assert_eq!(
                buf.get(x, y),
                bg,
                "missing character anim must paint nothing"
            );
        }
    }
}

#[test]
fn glass_wall_h_clamps_below_buffer_bottom() {
    // y_top near the buffer bottom makes the cap+face span exceed the height,
    // firing the per-row `y >= bh continue`.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bh = 16u16;
    let mut buf = RgbBuffer::filled(40, bh, Rgb { r: 0, g: 0, b: 0 });
    paint_glass_wall_h(&mut buf, theme, 0, 39, bh - 1);
    let mut painted = false;
    for y in 0..bh {
        for x in 0..40u16 {
            if buf.get(x, y) != (Rgb { r: 0, g: 0, b: 0 }) {
                painted = true;
            }
        }
    }
    assert!(painted, "in-bounds glass rows should still paint");
}

#[test]
fn glass_wall_v_clamps_past_right_edge() {
    // x_left == bw-1 → x_left+dx for dx>=1 exceeds the width, exercising the
    // `x >= bw continue`. Must not panic.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bw = 12u16;
    let mut buf = RgbBuffer::filled(bw, 40, Rgb { r: 0, g: 0, b: 0 });
    paint_glass_wall_v(&mut buf, theme, bw - 1, 5, 20);
    let mut painted = false;
    for y in 5..21u16 {
        if buf.get(bw - 1, y) != (Rgb { r: 0, g: 0, b: 0 }) {
            painted = true;
        }
    }
    assert!(painted, "the in-bounds glass column should paint");
}

#[test]
fn pet_hearts_skip_dead_and_faded_hearts() {
    use super::effects::paint_pet_hearts;
    let bg = Rgb { r: 0, g: 0, b: 0 };
    let cat_pos = Point { x: 20, y: 20 };
    let painted_count = |elapsed_ms: u64| -> usize {
        let mut buf = RgbBuffer::filled(40, 40, bg);
        paint_pet_hearts(&mut buf, cat_pos, elapsed_ms);
        (0..40u16)
            .flat_map(|y| (0..40u16).map(move |x| (x, y)))
            .filter(|&(x, y)| buf.get(x, y) != bg)
            .count()
    };
    assert_eq!(
        painted_count(2_100),
        0,
        "all hearts past their life → none paint"
    );
    assert!(painted_count(0) > 0, "first heart paints at t=0");
    let faded = painted_count(1_500);
    assert!(
        faded <= painted_count(300),
        "the faded heart drops out (alpha<0.05)"
    );
}

#[test]
fn furniture_room_decor_too_small_bounds_are_noops() {
    use super::furniture::{
        paint_doormat, paint_notice_board, paint_trash_bin, paint_water_cooler,
    };
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 9, g: 9, b: 9 };
    let small = crate::layout::Bounds {
        x: 2,
        y: 2,
        width: 8,
        height: 8,
    };
    let small_meeting = crate::layout::MeetingRoom {
        bounds: small,
        trio: None,
    };
    let small_pantry = crate::layout::PantryRoom {
        bounds: small,
        counter_size: crate::layout::Size { w: 20, h: 8 },
        kitchen_island: None,
    };
    let assert_noop = |f: &dyn Fn(&mut RgbBuffer)| {
        let mut buf = RgbBuffer::filled(60, 60, bg);
        f(&mut buf);
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), bg, "too-small bounds must paint nothing");
            }
        }
    };
    assert_noop(&|b| paint_notice_board(b, small, theme));
    assert_noop(&|b| paint_doormat(b, &small_meeting, theme));
    assert_noop(&|b| {
        paint_water_cooler(b, &small_pantry, std::time::SystemTime::UNIX_EPOCH, theme)
    });
    assert_noop(&|b| paint_trash_bin(b, &small_pantry));
}

#[test]
fn furniture_room_decor_large_bounds_paint() {
    use super::furniture::{
        paint_doormat, paint_notice_board, paint_trash_bin, paint_water_cooler,
    };
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 9, g: 9, b: 9 };
    // A generous room, well above every guard threshold.
    let big = crate::layout::Bounds {
        x: 4,
        y: 4,
        width: 40,
        height: 40,
    };
    let big_meeting = crate::layout::MeetingRoom {
        bounds: big,
        trio: None,
    };
    let big_pantry = crate::layout::PantryRoom {
        bounds: big,
        counter_size: crate::layout::Size { w: 20, h: 8 },
        kitchen_island: None,
    };
    let assert_paints = |f: &dyn Fn(&mut RgbBuffer)| {
        let mut buf = RgbBuffer::filled(120, 80, bg);
        f(&mut buf);
        let painted = (0..80u16)
            .flat_map(|y| (0..120u16).map(move |x| (x, y)))
            .any(|(x, y)| buf.get(x, y) != bg);
        assert!(painted, "large bounds must paint the decor");
    };
    assert_paints(&|b| paint_notice_board(b, big, theme));
    assert_paints(&|b| paint_doormat(b, &big_meeting, theme));
    assert_paints(&|b| {
        paint_water_cooler(b, &big_pantry, std::time::SystemTime::UNIX_EPOCH, theme)
    });
    assert_paints(&|b| paint_trash_bin(b, &big_pantry));
}

#[test]
fn furniture_painters_fill_exactly_their_rect_authority() {
    use super::furniture::{paint_doormat, paint_trash_bin, paint_water_cooler};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 1, g: 2, b: 3 };
    let big = crate::layout::Bounds {
        x: 4,
        y: 4,
        width: 44,
        height: 44,
    };
    let pantry = crate::layout::PantryRoom {
        bounds: big,
        counter_size: crate::layout::Size { w: 20, h: 8 },
        kitchen_island: None,
    };
    let meeting = crate::layout::MeetingRoom {
        bounds: big,
        trio: None,
    };
    let painted_bbox = |f: &dyn Fn(&mut RgbBuffer)| -> Option<crate::layout::Bounds> {
        let mut buf = RgbBuffer::filled(120, 80, bg);
        f(&mut buf);
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (u16::MAX, u16::MAX, 0u16, 0u16);
        let mut any = false;
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                if buf.get(x, y) != bg {
                    any = true;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        any.then(|| crate::layout::Bounds {
            x: min_x,
            y: min_y,
            width: max_x - min_x + 1,
            height: max_y - min_y + 1,
        })
    };
    assert_eq!(
        painted_bbox(&|b| paint_trash_bin(b, &pantry)),
        pantry.trash_bin_rect(),
        "trash bin paints exactly its rect",
    );
    assert_eq!(
        painted_bbox(&|b| paint_doormat(b, &meeting, theme)),
        meeting.doormat_rect(),
        "doormat paints exactly its rect",
    );
    assert_eq!(
        painted_bbox(&|b| {
            paint_water_cooler(b, &pantry, std::time::SystemTime::UNIX_EPOCH, theme)
        }),
        pantry.water_cooler_rect(),
        "water cooler paints exactly its rect (glug bubble stays inside)",
    );
}

#[test]
fn furniture_corner_clip_does_not_panic() {
    use super::furniture::{paint_area_rug, paint_side_table};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    // Centre each piece near the (0,0) corner so part of the sprite has a
    // negative px/py, exercising the `< 0` / out-of-range `continue` clamps.
    let mut buf = RgbBuffer::filled(40, 40, Rgb { r: 0, g: 0, b: 0 });
    paint_area_rug(&mut buf, 1, 1, 10, 8, theme);
    paint_side_table(&mut buf, 1, 1, theme);
    super::furniture::paint_kitchen_island(&mut buf, 1, 1, theme);
    // No panic reaching here is the assertion (negative coords are clipped).
}

#[test]
fn force_weather_sets_known_clears_none_and_errs_on_unknown() {
    // `t`'s natural (un-forced) weather is NOT Storm, so dropping the override
    // shows up in the observed weather, not just in the Ok/Err return. The
    // override is a thread-local Cell — every assert must run on one thread.
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
    force_weather(None).expect("clear is Ok");
    let natural = background::weather_state(t);

    assert!(force_weather(Some("storm")).is_ok(), "known name → Ok");
    assert_eq!(
        background::weather_state(t),
        background::Weather::Storm,
        "force_weather(storm) must drive weather_state to Storm",
    );
    assert_eq!(
        background::weather_state(t + std::time::Duration::from_secs(987_654)),
        background::Weather::Storm,
        "the override must ignore the clock",
    );

    assert!(
        force_weather(Some("STORM")).is_ok(),
        "case-insensitive → Ok"
    );
    assert_eq!(background::weather_state(t), background::Weather::Storm);

    assert!(force_weather(Some("snow")).is_ok());
    assert_eq!(
        background::weather_state(t),
        background::Weather::Snow,
        "a second known name must re-set the override",
    );

    let err = force_weather(Some("not-a-weather")).expect_err("unknown → Err");
    assert_eq!(
        err,
        weather_names(),
        "Err payload must be the canonical weather names",
    );
    assert_eq!(
        background::weather_state(t),
        background::Weather::Snow,
        "an unknown name must NOT touch the override",
    );

    assert!(force_weather(None).is_ok(), "None → Ok");
    assert_eq!(
        background::weather_state(t),
        natural,
        "None must restore the clock-based selection",
    );

    // Reset so the override can't leak into sibling time-based weather tests.
    force_weather(None).expect("reset");
}

#[test]
fn weather_gallery_manifest_matches_the_weather_enum() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/src/weather.json");
    let json = match std::fs::read_to_string(path) {
        Ok(s) => s,
        // crates.io-packaged test runs don't ship the repo's site/ tree.
        Err(_) => {
            eprintln!("skipping: {path} not present (packaged build)");
            return;
        }
    };
    let manifest: Vec<serde_json::Value> =
        serde_json::from_str(&json).expect("weather.json parses");
    let ids: Vec<&str> = manifest
        .iter()
        .map(|w| {
            w["id"]
                .as_str()
                .expect("weather.json entry has a string id")
        })
        .collect();
    assert_eq!(
        ids,
        weather_names(),
        "site/src/weather.json ids must match Weather::ALL names in order — \
         update the manifest + run `just gen-media` when the enum changes"
    );
}

#[test]
fn agent_palette_outfit_is_keyed_by_cwd_not_id() {
    let base = Palette::default();
    let a = make_slot_cwd("/demo/api/aaaa.jsonl", "/demo/api", false);
    let b = make_slot_cwd("/demo/api/bbbb.jsonl", "/demo/api", false);
    let pa = agent_palette(&base, &a, None, crate::burn::BurnTier::Normal);
    let pb = agent_palette(&base, &b, None, crate::burn::BurnTier::Normal);
    assert_eq!(pa.get('B'), pb.get('B'), "same cwd should share shirt");
    assert_eq!(pa.get('P'), pb.get('P'), "same cwd should share pants");
    assert_ne!(
        (pa.get('H'), pa.get('S')),
        (pb.get('H'), pb.get('S')),
        "different agents in the same repo must differ in hair/skin"
    );
}

#[test]
fn agent_palette_unknown_cwd_falls_back_to_id_outfit() {
    let base = Palette::default();
    let unknown = make_slot_cwd("/x/aaaa.jsonl", "/whatever", true);
    let empty = make_slot_cwd("/x/aaaa.jsonl", "", false);
    let p_unknown = agent_palette(&base, &unknown, None, crate::burn::BurnTier::Normal);
    let p_empty = agent_palette(&base, &empty, None, crate::burn::BurnTier::Normal);
    assert_eq!(p_unknown.get('B'), p_empty.get('B'));
    assert_eq!(p_unknown.get('P'), p_empty.get('P'));
    let other = make_slot_cwd("/x/zzzz.jsonl", "", false);
    let p_other = agent_palette(&base, &other, None, crate::burn::BurnTier::Normal);
    assert_ne!(
        p_other.get('B'),
        p_empty.get('B'),
        "cwd-less agents keep distinct per-id outfits"
    );
}

#[test]
fn cwd_backfill_invalidates_cached_outfit_frames() {
    let pack = crate::embedded_pack::test_default_pack();
    let unknown = make_slot_cwd("/p/heal.jsonl", "", true);
    // Pick a cwd whose Team-Palette outfit differs from the id-seeded fallback,
    // or the assertion has no teeth.
    let healed = (0..64)
        .map(|i| make_slot_cwd("/p/heal.jsonl", &format!("/repo/team{i}"), false))
        .find(|h| {
            agent_palette(&pack.palette, h, None, crate::burn::BurnTier::Normal).get('B')
                != agent_palette(&pack.palette, &unknown, None, crate::burn::BurnTier::Normal)
                    .get('B')
        })
        .expect("some cwd lands on a different outfit than the fallback");

    let anchor = Point { x: 2, y: 2 };
    let black = Rgb { r: 0, g: 0, b: 0 };
    let mut cache = FrameCache::new();
    let mut before = RgbBuffer::filled(24, 24, black);
    paint_character_at(
        &mut before,
        "seated",
        0,
        anchor,
        &unknown,
        &pack,
        false,
        None,
        &mut cache,
        SystemTime::UNIX_EPOCH,
    );

    let mut after = RgbBuffer::filled(24, 24, black);
    paint_character_at(
        &mut after,
        "seated",
        0,
        anchor,
        &healed,
        &pack,
        false,
        None,
        &mut cache,
        SystemTime::UNIX_EPOCH,
    );

    let mut fresh = RgbBuffer::filled(24, 24, black);
    paint_character_at(
        &mut fresh,
        "seated",
        0,
        anchor,
        &healed,
        &pack,
        false,
        None,
        &mut FrameCache::new(),
        SystemTime::UNIX_EPOCH,
    );

    assert_ne!(
        before.as_slice(),
        after.as_slice(),
        "the healed cwd must change the painted outfit"
    );
    assert_eq!(
        after.as_slice(),
        fresh.as_slice(),
        "the healed repaint must match a fresh render, not the stale cached outfit"
    );
}

#[test]
fn agent_palette_same_id_different_cwd_changes_outfit() {
    let base = Palette::default();
    let a = make_slot_cwd("/p/aaaa.jsonl", "/demo/api", false);
    let b = make_slot_cwd("/p/aaaa.jsonl", "/demo/infra", false);
    let pa = agent_palette(&base, &a, None, crate::burn::BurnTier::Normal);
    let pb = agent_palette(&base, &b, None, crate::burn::BurnTier::Normal);
    assert_ne!(
        pa.get('B'),
        pb.get('B'),
        "different cwds should pick different outfits"
    );
    assert_eq!(pa.get('H'), pb.get('H'));
    assert_eq!(pa.get('S'), pb.get('S'));
}

struct OwnedSimStores {
    router: crate::pathfind::AStarRouter,
    overlay: OccupancyOverlay,
    history: pose::PoseHistory,
    motion: std::collections::HashMap<pixtuoid_core::AgentId, crate::motion::MotionState>,
    light: LightingState,
    chitchat: std::collections::HashMap<crate::chitchat::VenueKey, crate::chitchat::ActiveChitchat>,
}

impl OwnedSimStores {
    fn new() -> Self {
        Self {
            router: crate::pathfind::AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: pose::PoseHistory::new(),
            motion: std::collections::HashMap::new(),
            light: LightingState::new(),
            chitchat: std::collections::HashMap::new(),
        }
    }

    fn stores(&mut self) -> SimStores<'_> {
        SimStores {
            router: &mut self.router,
            overlay: &mut self.overlay,
            history: &mut self.history,
            motion: &mut self.motion,
            light: &mut self.light,
            chitchat: &mut self.chitchat,
        }
    }
}

fn sim_rig() -> (SceneState, Layout, pixtuoid_core::AgentId, SystemTime, Pack) {
    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(160, 96, None, 0).expect("160x96 lays out");
    let now0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let id = pixtuoid_core::AgentId::from_transcript_path("/p/sim-seam.jsonl");
    let mut slot = make_slot(id, ActivityState::Idle);
    slot.created_at = now0;
    slot.state_started_at = now0;
    slot.last_event_at = now0;
    let mut scene = SceneState::uniform(16);
    scene.agents.insert(id, slot);
    (scene, layout, id, now0, pack)
}

// One AtWaypoint agent ⇒ one blocked rect, so the bbox width IS char_w.
fn reserved_bbox_width(overlay: &OccupancyOverlay, w: u16, h: u16) -> Option<u16> {
    let (mut lo, mut hi) = (None, None);
    for y in 0..h {
        for x in 0..w {
            if overlay.blocks(x, y) {
                lo = Some(lo.map_or(x, |m: u16| m.min(x)));
                hi = Some(hi.map_or(x, |m: u16| m.max(x)));
            }
        }
    }
    Some(hi? - lo? + 1)
}

// The bundled 8-wide pack cannot tell char_w apart from the const, so the
// differential against a wide (10px) fixture pack is what gives this teeth.
#[test]
fn sim_step_reserves_the_pack_resolved_char_width_not_the_bundled_const() {
    use crate::layout::TEST_DEFAULT_DESKS;
    use crate::pose::Pose;
    use std::time::Duration;

    let wide = crate::embedded_pack::test_wide_pack();
    let default = crate::embedded_pack::test_default_pack();
    assert_eq!(
        wide.animation("standing").expect("standing").frames[0].width(),
        10,
        "the wide fixture's standing frame drives char_w"
    );
    assert_eq!(
        default.animation("standing").expect("standing").frames[0].width(),
        CHARACTER_SPRITE_W,
    );

    let layout =
        Layout::compute_with_seed(240, 160, Some(TEST_DEFAULT_DESKS), 0).expect("240x160 lays out");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let (bw, bh) = (layout.walkable.width(), layout.walkable.height());

    // `pose::derive` is pack-INDEPENDENT, so the AtWaypoint instant it finds is
    // the same for BOTH packs.
    let mut found = None;
    'search: for aid in 0..8u32 {
        let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/p/wp-{aid}.jsonl"));
        let mut slot = make_slot(id, ActivityState::Idle);
        slot.created_at = now0;
        slot.state_started_at = now0;
        slot.last_event_at = now0;
        for secs in 1..1800u64 {
            let now = now0 + Duration::from_secs(secs);
            if matches!(
                pose::derive(&slot, now, &layout),
                Some(Pose::AtWaypoint { .. })
            ) {
                found = Some((slot.clone(), now));
                break 'search;
            }
        }
    }
    let (slot, now) = found.expect("an idle agent visits a Named waypoint within the scan window");

    let mut scene = SceneState::uniform(16);
    scene.agents.insert(slot.agent_id, slot);
    let coffee = HashMap::new();

    let reserve = |pack: &Pack| {
        let mut owned = OwnedSimStores::new();
        sim_step(&mut owned.stores(), &scene, &layout, pack, &coffee, 0, now);
        reserved_bbox_width(&owned.overlay, bw, bh)
    };
    assert_eq!(
        reserve(&wide),
        Some(10),
        "wide pack reserves char_w=10 at the AtWaypoint stand cell"
    );
    assert_eq!(
        reserve(&default),
        Some(CHARACTER_SPRITE_W),
        "default pack reserves the bundled char_w=8 — the differential that pins char_w",
    );
}

/// `seat_desk` is the whole of "one simulation, two projections": the cutaway
/// re-anchors a desk-seated body and cannot recover the desk from `anchor`,
/// which is already projected FOR CLASSIC. Its polarity had no test at all —
/// the classic painter never reads the field, so flipping an arm to `None`
/// would break the second profile while every existing assertion stayed green.
///
/// Driven through the real `sim_step` rather than by constructing a placement,
/// so it pins what the sim DECIDES, not what a fixture was handed.
#[test]
fn seat_desk_is_set_exactly_when_the_sim_seats_someone_at_a_desk() {
    use crate::pose::Pose;
    use std::time::Duration;
    let (scene, layout, id, now0, pack) = sim_rig();
    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();

    // Far enough past the entry walk that the agent has arrived and sat down.
    let mut seated_seen = false;
    let mut walking_seen = false;
    for ms in [50u64, 250, 1_000, 4_000, 12_000, 40_000] {
        let f = sim_step(
            &mut stores,
            &scene,
            &layout,
            &pack,
            &coffee,
            0,
            now0 + Duration::from_millis(ms),
        );
        let Some(c) = f.characters.first() else {
            continue;
        };
        match f.poses.get(&id) {
            Some(Some(Pose::Walking { .. })) => {
                walking_seen = true;
                assert_eq!(
                    c.seat_desk, None,
                    "a WALKING agent is not seated at a desk (t={ms}ms)"
                );
            }
            Some(Some(Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. })) => {
                seated_seen = true;
                let desk = c.seat_desk.expect("a seated agent carries its desk");
                assert!(
                    layout.home_desks.contains(&desk),
                    "the carried desk must be one the layout placed (t={ms}ms)"
                );
            }
            _ => {}
        }
    }
    assert!(walking_seen, "the sweep never observed a walking pose");
    assert!(seated_seen, "the sweep never observed a seated pose");
}

#[test]
fn sim_step_advances_motion_without_painting() {
    use crate::pose::Pose;
    use std::time::Duration;
    let (scene, layout, id, now0, pack) = sim_rig();
    let coffee = HashMap::new();

    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();

    let walk_t = |f: &SimFrame| match f.poses.get(&id) {
        Some(Some(Pose::Walking { t_x1000, .. })) => *t_x1000,
        other => panic!("expected an entry walk pose, got {other:?}"),
    };
    let f1 = sim_step(
        &mut stores,
        &scene,
        &layout,
        &pack,
        &coffee,
        0,
        now0 + Duration::from_millis(50),
    );
    let f2 = sim_step(
        &mut stores,
        &scene,
        &layout,
        &pack,
        &coffee,
        0,
        now0 + Duration::from_millis(250),
    );
    assert!(
        walk_t(&f2) > walk_t(&f1),
        "entry walk must progress between ticks: {} -> {}",
        walk_t(&f1),
        walk_t(&f2)
    );
    assert!(
        f2.characters
            .iter()
            .any(|c| c.anim_name.starts_with("walking")),
        "the tick's placements carry the walking sprite"
    );
    let _ = stores;
    assert!(
        owned.motion.get(&id).is_some_and(|m| m.entry.is_some()),
        "sim_step snapshotted the entry walk profile into the motion store"
    );
}

#[test]
fn paint_frame_is_pure_and_byte_identical() {
    use std::time::Duration;
    let (scene, layout, id, now0, pack) = sim_rig();
    let _ = id;
    let coffee = HashMap::new();

    let mut owned = OwnedSimStores::new();
    let now = now0 + Duration::from_millis(120);
    let frame = sim_step(&mut owned.stores(), &scene, &layout, &pack, &coffee, 0, now);

    let light_before = owned.light.level();
    let motion_before = format!("{:?}", owned.motion);
    let history_before = format!("{:?}", owned.history);
    let chitchat_before = owned.chitchat.len();

    let theme = crate::theme::theme_by_name("normal").expect("normal theme");
    let black = Rgb { r: 0, g: 0, b: 0 };
    let mut cache = FrameCache::new();
    let mut buf1 = RgbBuffer::filled(layout.buf_w, layout.buf_h, black);
    let mut buf2 = RgbBuffer::filled(layout.buf_w, layout.buf_h, black);
    for buf in [&mut buf1, &mut buf2] {
        paint_frame(
            &mut PaintCtx {
                scene: &scene,
                layout: &layout,
                pack: &pack,
                now,
                buf,
                cache: &mut cache,
                theme,
                floor: crate::floor::FloorMeta::ground(),
                active_pet: None,
                floor_pet: None,
                coffee: &coffee,
                motion: &owned.motion,
                door_anim_max_ms: 0,
                debug_walkable: false,
            },
            &frame,
        );
    }

    assert_eq!(
        buf1.as_slice(),
        buf2.as_slice(),
        "painting the same SimFrame twice must be byte-identical"
    );
    assert!(
        buf1.as_slice().iter().any(|p| *p != black),
        "the paint pass actually painted the office"
    );
    assert_eq!(
        owned.light.level(),
        light_before,
        "paint must not tick lighting"
    );
    assert_eq!(
        format!("{:?}", owned.motion),
        motion_before,
        "paint must not move motion state"
    );
    assert_eq!(
        format!("{:?}", owned.history),
        history_before,
        "paint must not record pose history"
    );
    assert_eq!(
        owned.chitchat.len(),
        chitchat_before,
        "paint must not start/expire chitchat"
    );
}

#[test]
fn corridor_runner_weaves_sparse_diamonds_without_inner_edge_rows() {
    // Taste pin: stride-10 lattice, border rows only — the old stride-6 +
    // inner-edge treatment read as bathroom tiling, not a woven runner.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(60, 24, floor);
    let rect = crate::layout::Bounds {
        x: 0,
        y: 4,
        width: 60,
        height: 12,
    };
    paint_corridor_runner(&mut buf, rect, theme);
    let base = theme.office.runner_base;
    let stripe = theme.office.runner_stripe;
    let edge = theme.office.runner_edge;
    assert_eq!(buf.get(0, 4), edge, "border row stays");
    assert_eq!(
        buf.get(2, 5),
        base,
        "inner-edge row (dx=2,dy=1) must be base"
    );
    assert_eq!(
        buf.get(2, 8),
        base,
        "old stride-6 lattice point must be base"
    );
    assert_eq!(buf.get(7, 7), stripe, "(dx+dy)=10 lands on the new lattice");
}

#[test]
fn pantry_doorway_gets_a_centered_entry_mat() {
    // Taste pin: an entry mat centered under the pantry's north doorway, one
    // clear row off the wall face.
    use crate::layout::{TEST_DEFAULT_DESKS, WALL_THICK_H};
    let l = Layout::compute(192, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let p = l.pantry.expect("pantry");
    let dw = l
        .doorways
        .iter()
        .find(|d| d.start.y == d.end.y && d.start.y == p.bounds.y)
        .expect("the pantry north door");
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(192, 160, floor);
    furniture::paint_pantry_entry_mat(&mut buf, &l, theme);
    let cx = (dw.start.x + dw.end.x) / 2;
    let mat_cy = dw.start.y + WALL_THICK_H + 3;
    assert_ne!(buf.get(cx, mat_cy), floor, "mat center row painted");
    assert_ne!(buf.get(cx - 7, mat_cy), floor, "mat spans west of center");
    assert_ne!(buf.get(cx + 7, mat_cy), floor, "mat spans east of center");
    assert_eq!(buf.get(cx - 9, mat_cy), floor, "floor beyond the west edge");
    assert_eq!(buf.get(cx + 9, mat_cy), floor, "floor beyond the east edge");
    assert_eq!(
        buf.get(cx, dw.start.y + WALL_THICK_H),
        floor,
        "one clear row between wall face and mat"
    );
}

#[test]
fn kitchen_island_sits_on_a_bar_mat() {
    // Taste pin: a thin bordered mat under the island whose south sliver peeks
    // out in front of the bar.
    use crate::layout::TEST_DEFAULT_DESKS;
    let l = Layout::compute(192, 160, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let isl = l
        .pantry
        .and_then(|p| p.kitchen_island)
        .expect("island at this size");
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(192, 160, floor);
    furniture::paint_island_bar_mat(&mut buf, &l, theme);
    assert_ne!(
        buf.get(isl.x, isl.y + 4),
        floor,
        "mat painted under the island front"
    );
    assert_eq!(
        buf.get(isl.x + 14, isl.y + 4),
        floor,
        "floor beyond the east edge"
    );
    assert_eq!(
        buf.get(isl.x - 14, isl.y + 4),
        floor,
        "floor beyond the west edge"
    );
    let before = buf.get(isl.x, isl.y);
    furniture::paint_kitchen_island(&mut buf, isl.x, isl.y, theme);
    assert_ne!(
        buf.get(isl.x, isl.y),
        before,
        "island body must cover the mat center"
    );
}

#[test]
fn pantry_mats_stay_inside_the_pantry_bounds() {
    use crate::layout::TEST_DEFAULT_DESKS;
    // 120x160 is the narrow-pantry case where the entry mat box reaches the
    // water-cooler column (the paint-order catch).
    for (w, h) in [(192u16, 160u16), (240, 160), (160, 120), (120, 160)] {
        let Some(l) = Layout::compute(w, h, Some(TEST_DEFAULT_DESKS)) else {
            continue;
        };
        let Some(p) = l.pantry else { continue };
        let floor = Rgb {
            r: 150,
            g: 110,
            b: 72,
        };
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let mut buf = RgbBuffer::filled(w, h, floor);
        furniture::paint_pantry_entry_mat(&mut buf, &l, theme);
        furniture::paint_island_bar_mat(&mut buf, &l, theme);
        let b = p.bounds;
        for y in 0..h {
            for x in 0..w {
                let inside = x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height;
                if !inside {
                    assert_eq!(
                        buf.get(x, y),
                        floor,
                        "{w}x{h}: mat pixel escaped the pantry at ({x},{y})"
                    );
                }
            }
        }
    }
}

#[test]
fn fish_tank_paints_water_fish_and_cabinet_from_the_furniture_row() {
    use crate::layout::{furniture_def, Furniture};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(60, 40, floor);
    let pos = Point { x: 30, y: 20 };
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_234_567);
    furniture::paint_fish_tank(&mut buf, pos, now, theme);
    let def = furniture_def(Furniture::FishTank);
    let (x0, y0) = (pos.x - def.visual.w / 2, pos.y - def.visual.h / 2);
    let fc = &theme.furniture;
    assert_eq!(
        buf.get(x0, y0),
        theme.office.room_wall_trim_dark,
        "lid row is the shared dark frame"
    );
    assert_eq!(
        buf.get(x0 + 7, y0 + 2),
        fc.tank_water,
        "water body fills the glass"
    );
    assert_eq!(
        buf.get(x0 + 7, y0 + 1),
        fc.tank_water_line,
        "lit surface row under the lid"
    );
    let lane =
        |dy: u16, color: Rgb| (1..def.visual.w - 1).any(|dx| buf.get(x0 + dx, y0 + dy) == color);
    assert!(lane(3, fc.tank_fish), "a fish patrols the upper lane");
    assert!(
        lane(5, fc.tank_fish_alt),
        "the alt fish patrols the lower lane"
    );
    assert!(
        (2..8).any(|dy| buf.get(x0 + 2, y0 + dy) == fc.tank_plant),
        "plant sprig rises from the gravel"
    );
    assert_eq!(
        buf.get(x0 + 3, y0 + 9),
        fc.wood_top,
        "cabinet row reuses the wood family"
    );
}

#[test]
fn meeting_chairs_paint_with_backrests_toward_the_table_ends() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(40, 20, floor);
    let pos = Point { x: 20, y: 10 };
    furniture::paint_meeting_chair(&mut buf, pos, true, theme);
    let fc = &theme.furniture;
    assert_eq!(
        buf.get(pos.x - 3, pos.y),
        fc.chair_trim,
        "west backrest bar"
    );
    assert_eq!(
        buf.get(pos.x, pos.y),
        furniture::MEETING_FABRIC,
        "cushion wears the sofa fabric"
    );
    let mut buf2 = RgbBuffer::filled(40, 20, floor);
    furniture::paint_meeting_chair(&mut buf2, pos, false, theme);
    assert_eq!(
        buf2.get(pos.x + 3, pos.y),
        fc.chair_trim,
        "east backrest bar"
    );
    assert_eq!(
        buf2.get(pos.x - 3, pos.y),
        floor,
        "no bar on the table side"
    );
}

#[test]
fn meeting_chair_fabric_matches_the_sofa_sprite_palette() {
    // The sofa is an un-themed sprite, so the painter can't read Theme for it —
    // these consts are deliberate copies of the pack palette's couch fabric.
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let c = pack.palette.get('C').flatten().expect("couch fabric key");
    let g = pack
        .palette
        .get('G')
        .flatten()
        .expect("cushion highlight key");
    assert_eq!(furniture::MEETING_FABRIC, c, "chair fabric == sofa 'C'");
    assert_eq!(
        furniture::MEETING_FABRIC_LIT,
        g,
        "chair highlight == sofa 'G'"
    );
}

#[test]
fn chair_sitter_bottom_row_lands_on_its_z_key_overlapping_the_chair_body() {
    use crate::layout::{Facing, Point, WaypointKind, SEAT_RENDER_Y_OFF};
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let view = SeatView::of(WaypointKind::MeetingChair, Facing::West);
    let (anim, _) = view.seated_sprite();
    let seated_h = pack.animation(anim).expect("chair sprite").frames[0].height();
    let pos = Point { x: 40, y: 30 };
    let anchor_y = pos.y - SEAT_RENDER_Y_OFF;
    let bottom = anchor_y + seated_h - 1;
    assert_eq!(
        bottom,
        view.z_key_for_seat(pos),
        "the chair sprite's bottom row must land on its seat z-key row"
    );
    let chair = crate::layout::furniture_def(crate::layout::Furniture::MeetingChair).visual;
    let chair_top = pos.y - chair.h / 2;
    assert!(
        bottom > chair_top,
        "sitter bottom ({bottom}) must overlap the chair body (top {chair_top})"
    );
}

#[test]
fn busy_printer_ejects_a_page_and_idle_printer_stays_still() {
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("pack");
    let mut cache = FrameCache::new();
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pos = Point { x: 30, y: 20 };
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(600); // mid-eject
    let bg = Rgb { r: 1, g: 2, b: 3 };
    let mut render = |busy: bool| {
        let mut buf = RgbBuffer::filled(60, 40, bg);
        let d = Drawable {
            anchor_y: pos.y + 2,
            kind: DrawableKind::Printer { pos, busy },
        };
        paint_drawable(
            &d,
            &mut super::drawable::DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme,
            },
        );
        buf
    };
    let busy = render(true);
    let idle = render(false);
    let paper = theme.appliance.printer_paper;
    let below = (1..=3u16).any(|dx| busy.get(pos.x - 2 + dx, pos.y + 2) == paper);
    assert!(below, "busy printer shows paper emerging below the tray");
    assert!(
        (1..=3u16).all(|dx| idle.get(pos.x - 2 + dx, pos.y + 2) == bg),
        "idle printer paints nothing below the tray"
    );
}

#[test]
fn busy_vending_machine_drops_a_can_and_idle_stays_stocked() {
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("pack");
    let mut cache = FrameCache::new();
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pos = Point { x: 30, y: 20 };
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_200); // mid-drop
    let bg = Rgb { r: 1, g: 2, b: 3 };
    let mut render = |busy: bool| {
        let mut buf = RgbBuffer::filled(60, 40, bg);
        let d = Drawable {
            anchor_y: pos.y + 3,
            kind: DrawableKind::VendingMachine { pos, busy },
        };
        paint_drawable(
            &d,
            &mut super::drawable::DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme,
            },
        );
        buf
    };
    let busy = render(true);
    let idle = render(false);
    let (sdx, sdy) = super::drawable::VENDING_PICKUP_SLOT;
    let slot = (pos.x.saturating_sub(2) + sdx, pos.y.saturating_sub(3) + sdy);
    assert!(
        theme
            .appliance
            .vending_drinks
            .contains(&busy.get(slot.0, slot.1)),
        "busy vending drops a can into the slot"
    );
    assert_eq!(
        idle.get(slot.0, slot.1),
        theme.appliance.vending_trim,
        "idle vending keeps the plain slot"
    );
}

#[test]
fn water_cooler_glugs_a_rising_bubble() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 1, g: 2, b: 3 };
    let pr = crate::layout::Bounds {
        x: 4,
        y: 4,
        width: 30,
        height: 40,
    };
    let pantry = crate::layout::PantryRoom {
        bounds: pr,
        counter_size: crate::layout::Size { w: 20, h: 8 },
        kitchen_island: None,
    };
    let render = |ms: u64| {
        let mut buf = RgbBuffer::filled(60, 60, bg);
        furniture::paint_water_cooler(
            &mut buf,
            &pantry,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms),
            theme,
        );
        buf
    };
    let bubble = theme.furniture.tank_water_line;
    let (wx, wy) = (pr.x + pr.width - 6, pr.y + 8);
    let a = render(100); // phase 0: bubble low
    let b = render(500); // phase 1: bubble high
    assert_eq!(
        a.get(wx + 1, wy + 1),
        bubble,
        "bubble starts low in the bottle"
    );
    assert_eq!(b.get(wx + 1, wy), bubble, "bubble rises a row");
    let c = render(1_500); // rest of the cycle: no bubble
    assert_ne!(c.get(wx + 1, wy), bubble);
    assert_ne!(c.get(wx + 1, wy + 1), bubble);
}

#[test]
fn sim_reports_occupied_waypoints_and_enqueue_marks_them_busy() {
    use std::time::Duration;
    let (scene, layout, _id, now0, pack) = sim_rig();
    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();
    let mut pinned = false;
    for step in 0..240u64 {
        let now = now0 + Duration::from_secs(5 * step);
        let f = sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now);
        let at_wp: Vec<usize> = f
            .poses
            .values()
            .filter_map(|p| match p {
                Some(crate::pose::Pose::AtWaypoint { wp, .. }) => Some(*wp),
                _ => None,
            })
            .collect();
        if !at_wp.is_empty() {
            for wp in at_wp {
                assert!(
                    f.occupied_waypoints.contains(&wp),
                    "AtWaypoint({wp}) must appear in occupied_waypoints"
                );
            }
            pinned = true;
            break;
        }
    }
    assert!(
        pinned,
        "the idle agent never reached a waypoint in 20 min of sim"
    );
    let layout = Layout::compute(192, 160, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("fits");
    let printer_idx = layout
        .waypoints
        .iter()
        .position(|w| w.kind == crate::layout::WaypointKind::Printer)
        .expect("printer at 160x96");
    let occupied: std::collections::HashSet<usize> = [printer_idx].into();
    let mut drawables = Vec::new();
    enqueue_lounge_pantry_appliances(&layout, &occupied, &mut drawables);
    let busy_flag = drawables
        .iter()
        .find_map(|d| match d.kind {
            DrawableKind::Printer { busy, .. } => Some(busy),
            _ => None,
        })
        .expect("printer drawable enqueued");
    assert!(
        busy_flag,
        "occupied printer waypoint must enqueue busy=true"
    );
}

#[test]
fn no_two_agents_ever_occupy_the_same_exclusive_waypoint() {
    use crate::layout::{furniture_def, TEST_DEFAULT_DESKS};
    use crate::pose::Pose;
    use std::time::Duration;

    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(192, 160, Some(TEST_DEFAULT_DESKS), 0).expect("fits");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut scene = SceneState::uniform(64);
    for i in 0..TEST_DEFAULT_DESKS {
        let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/p/seat{i}.jsonl"));
        let mut slot = make_slot(id, ActivityState::Idle);
        // Stagger the idle starts so wander cycles desync instead of moving as
        // one lockstep wave.
        let started = now0 - Duration::from_secs(5 + (i as u64 * 11) % 80);
        slot.created_at = started;
        slot.state_started_at = started;
        slot.last_event_at = started;
        slot.desk_index = GlobalDeskIndex(i);
        scene.agents.insert(id, slot);
    }

    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();
    let mut seat_visits = 0usize;
    for step in 0..3_600u64 {
        let now = now0 + Duration::from_millis(250 * step);
        let frame = sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now);
        let mut occupants: HashMap<usize, usize> = HashMap::new();
        for pose in frame.poses.values().flatten() {
            let Pose::AtWaypoint { wp, kind } = pose else {
                continue;
            };
            if !furniture_def(kind.furniture()).exclusive {
                continue;
            }
            seat_visits += 1;
            let n = occupants.entry(*wp).or_insert(0);
            *n += 1;
            assert_eq!(
                *n, 1,
                "{kind:?} waypoint {wp} double-booked at step {step} — an exclusive spot is single-occupancy"
            );
        }
    }
    assert!(seat_visits > 100, "agents barely sat down ({seat_visits})");
}

#[test]
fn an_active_agent_releases_the_seat_it_snapped_back_from() {
    use crate::layout::{furniture_def, TEST_DEFAULT_DESKS};
    use crate::motion::WanderKind;
    use crate::pose::Pose;
    use std::time::Duration;

    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(192, 160, Some(TEST_DEFAULT_DESKS), 0).expect("fits");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let id = pixtuoid_core::AgentId::from_transcript_path("/p/claim-release.jsonl");
    let mut slot = make_slot(id, ActivityState::Idle);
    slot.created_at = now0;
    slot.state_started_at = now0;
    slot.last_event_at = now0;
    let mut scene = SceneState::uniform(16);
    scene.agents.insert(id, slot);

    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();

    let mut sat_at = None;
    let mut now = now0;
    for _ in 0..2_000 {
        now += Duration::from_millis(250);
        let frame = sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now);
        if let Some(Pose::AtWaypoint { wp, kind }) = frame.poses.get(&id).copied().flatten() {
            if furniture_def(kind.furniture()).occupies_pos {
                sat_at = Some(wp);
                break;
            }
        }
    }
    let sat_at = sat_at.expect("agent never reached a seat");
    assert!(
        matches!(
            owned.motion[&id].wander.target.kind,
            WanderKind::Named { wp_idx, .. } if wp_idx == sat_at
        ),
        "the seated agent should hold its seat's claim"
    );

    scene.agents.get_mut(&id).expect("slot").state = ActivityState::Active {
        tool_use_id: None,
        detail: None,
        kind: ToolKind::Other,
    };
    scene.agents.get_mut(&id).expect("slot").state_started_at = now;
    let mut stores = owned.stores();
    now += Duration::from_millis(250);
    sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now);

    assert!(
        matches!(owned.motion[&id].wander.target.kind, WanderKind::Aimless),
        "an agent that left the wander machine must release its seat claim"
    );
}

#[test]
fn precipitation_level_maps_audible_rain_and_honors_the_override() {
    // force_weather's override is thread-local — reset at the end so it can't
    // leak into the sibling time-based weather tests.
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);

    force_weather(Some("storm")).expect("storm is known");
    assert_eq!(precipitation_level(t), 1.0, "storm is full precipitation");

    force_weather(Some("rain")).expect("rain is known");
    let rain = precipitation_level(t);
    assert!(
        rain > 0.0 && rain < 1.0,
        "rain sits strictly between clear and storm, got {rain}"
    );

    for quiet in ["clear", "snow", "fog", "overcast", "windy", "smog"] {
        force_weather(Some(quiet)).expect("known name");
        assert_eq!(
            precipitation_level(t),
            0.0,
            "{quiet} must be silent precipitation"
        );
    }

    force_weather(None).expect("restore");
}

#[test]
fn one_meeting_sofa_still_seats_three_agents_at_once() {
    use crate::layout::{furniture_def, WaypointKind, TEST_DEFAULT_DESKS};
    use crate::pose::Pose;
    use std::time::Duration;

    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(192, 160, Some(TEST_DEFAULT_DESKS), 0).expect("fits");
    let sofa: Vec<usize> = {
        let mut out: Vec<usize> = vec![];
        for (i, w) in layout.waypoints.iter().enumerate() {
            if w.kind != WaypointKind::MeetingSofa {
                continue;
            }
            match out.first() {
                None => out.push(i),
                Some(&f) => {
                    if w.pos.y == layout.waypoints[f].pos.y
                        && w.room_id == layout.waypoints[f].room_id
                    {
                        out.push(i);
                    }
                }
            }
            if out.len() == 3 {
                break;
            }
        }
        out
    };
    assert_eq!(sofa.len(), 3, "expected a 3-seat sofa, got {sofa:?}");
    assert!(
        sofa.iter()
            .all(|&i| furniture_def(layout.waypoints[i].kind.furniture()).exclusive),
        "each sofa seat must be an exclusive waypoint"
    );

    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut scene = SceneState::uniform(64);
    for i in 0..TEST_DEFAULT_DESKS {
        let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/p/sofa{i}.jsonl"));
        let mut slot = make_slot(id, ActivityState::Idle);
        let started = now0 - Duration::from_secs(5 + (i as u64 * 11) % 80);
        slot.created_at = started;
        slot.state_started_at = started;
        slot.last_event_at = started;
        slot.desk_index = GlobalDeskIndex(i);
        scene.agents.insert(id, slot);
    }

    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();
    let mut max_on_sofa = 0usize;
    // A BUDGET, not part of the assertion: three-on-a-sofa is reached by random
    // wander, whose route rides live desk positions.
    for step in 0..60_000u64 {
        let now = now0 + Duration::from_millis(250 * step);
        let frame = sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now);
        let n = frame
            .poses
            .values()
            .flatten()
            .filter(|p| matches!(p, Pose::AtWaypoint { wp, .. } if sofa.contains(wp)))
            .count();
        max_on_sofa = max_on_sofa.max(n);
        if max_on_sofa >= 3 {
            break;
        }
    }
    assert_eq!(
        max_on_sofa, 3,
        "one sofa must seat 3 agents at once; peaked at {max_on_sofa}"
    );
}

#[test]
fn character_anchor_meeting_chair_label_tracks_the_seat_sprite_not_5px_high() {
    use crate::layout::{stand_point, WaypointKind, TEST_DEFAULT_DESKS};
    use crate::pose::{Pose, RouteCtx};
    use std::time::Duration;

    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(192, 160, Some(TEST_DEFAULT_DESKS), 0).expect("fits");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut scene = SceneState::uniform(64);
    for i in 0..TEST_DEFAULT_DESKS {
        let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/p/mc{i}.jsonl"));
        let mut slot = make_slot(id, ActivityState::Idle);
        let started = now0 - Duration::from_secs(5 + (i as u64 * 11) % 80);
        slot.created_at = started;
        slot.state_started_at = started;
        slot.last_event_at = started;
        slot.desk_index = GlobalDeskIndex(i);
        scene.agents.insert(id, slot);
    }
    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut checked = false;
    for step in 0..4_000u64 {
        let now = now0 + Duration::from_millis(250 * step);
        let frame = {
            let mut stores = owned.stores();
            sim_step(&mut stores, &scene, &layout, &pack, &coffee, 0, now)
        };
        let mc = frame.poses.iter().find_map(|(id, p)| match p {
            Some(Pose::AtWaypoint {
                wp,
                kind: WaypointKind::MeetingChair,
            }) => Some((*id, *wp)),
            _ => None,
        });
        let Some((id, wp)) = mc else { continue };
        let agent = scene.agents.get(&id).unwrap();
        let desk = layout
            .home_desk(agent.desk_index.single_floor_local())
            .unwrap();
        let w = &layout.waypoints[wp];
        let stand = stand_point(
            w.kind,
            w.pos,
            layout.pantry_counter_size(),
            &layout.walkable,
            desk,
            w.facing,
            &layout.reachable,
        );
        // Idempotent re-derive at the same `now` (sim_step already stamped
        // last_advanced_at, so no wander transition fires here).
        let label = {
            let mut rctx = RouteCtx {
                router: &mut owned.router,
                overlay: &owned.overlay,
                history: &mut owned.history,
                motion: &mut owned.motion,
            };
            character_anchor(agent, &layout, now, &mut rctx).expect("chair sitter is visible")
        };
        let seat = back_couch_anchor(stand, CHARACTER_SPRITE_W);
        let walk = waypoint_anchor(stand, CHARACTER_SPRITE_W);
        assert!(
            (label.y as i32 - seat.y as i32).abs() <= 1,
            "meeting-chair label y {} must track the seat sprite anchor {} (±breath), not float above",
            label.y,
            seat.y
        );
        assert!(
            (label.y as i32 - walk.y as i32).abs() >= 4,
            "meeting-chair label must NOT sit on the 5px-high waypoint_anchor {} (the bug)",
            walk.y
        );
        checked = true;
        break;
    }
    assert!(checked, "no meeting-chair sitter appeared in 1000s of sim");
}

#[test]
fn waypoint_render_anchor_matches_the_pre_lift_kind_partition() {
    use crate::layout::{Facing, Point, WaypointKind};
    let stand = Point { x: 80, y: 60 };
    let w = CHARACTER_SPRITE_W;
    for &kind in WaypointKind::ALL {
        // Independent oracle: the exact pre-lift partition, keyed on kind alone.
        let expected = match kind {
            WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingChair => {
                (back_couch_anchor(stand, w), 9u16)
            }
            _ => (waypoint_anchor(stand, w), 12u16),
        };
        for facing in [Facing::North, Facing::South, Facing::East, Facing::West] {
            assert_eq!(
                SeatView::of(kind, facing).waypoint_render_anchor(stand, w),
                expected,
                "{kind:?}/{facing:?}: render anchor drifted from the pre-lift partition"
            );
        }
    }
}

#[test]
fn waypoint_render_anchor_upright_height_recovers_the_feet_row() {
    use crate::layout::{Facing, Point, WaypointKind};
    let stand = Point { x: 80, y: 60 };
    let w = CHARACTER_SPRITE_W;
    for &kind in WaypointKind::ALL {
        if matches!(
            kind,
            WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingChair
        ) {
            continue;
        }
        let (anchor, sprite_h) = SeatView::of(kind, Facing::South).waypoint_render_anchor(stand, w);
        assert_eq!(
            anchor.y + sprite_h,
            stand.y,
            "{kind:?}: upright anchor.y + sprite_h must land on the feet row (stand.y)"
        );
    }
}

#[test]
fn desk_shadow_tracks_the_desk_zsort_row_not_a_hardcoded_offset() {
    let desk = Point { x: 40, y: 30 };
    let e = desk_shadow_ellipse(desk);
    assert_eq!(e.cy, desk.y + crate::layout::desk_furniture_def().visual.h);
    assert_eq!(e.cx, desk.x + DESK_W / 2);
}

#[test]
fn ceiling_pool_regions_yields_desks_then_pantry_then_corridor_in_order() {
    let l =
        Layout::compute(192, 160, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("192x160 fits");
    let pools: Vec<_> = ceiling_pool_regions(&l).collect();
    assert_eq!(
        pools.len(),
        l.home_desks.len() + l.pantry.is_some() as usize + l.corridor.is_some() as usize
    );
    let mut desk_rows = std::collections::HashSet::new();
    for (i, ((pool, keep), desk)) in pools.iter().zip(&l.home_desks).enumerate() {
        // Derived from the ONE authority rather than restating its arithmetic.
        let want = crate::layout::desk_ceiling_pool_center(
            *desk,
            l.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i)),
        );
        assert_eq!((pool.cx, pool.cy), (want.x, want.y));
        assert_eq!((pool.half_w, pool.half_h), (10, 5));
        assert!(*keep < 1.0, "a desk tube dims after dark");
        desk_rows.insert(pool.cy as i32 - desk.y as i32);
    }
    // Negative control: with one lift, a facing-blind impl passes the loop above.
    assert!(
        desk_rows.len() >= 2,
        "this layout seats both sides, so its desk lights must sit at two \
         different offsets — got {desk_rows:?}"
    );
    if let Some(pr) = l.pantry.map(|p| p.bounds) {
        let (p, keep) = pools[l.home_desks.len()];
        assert_eq!((p.cx, p.cy), (pr.x + pr.width / 2, pr.y + pr.height / 2));
        assert_eq!((p.half_w, p.half_h), (12, 6));
        assert_eq!(keep, 1.0, "shared spaces keep their overhead light");
    }
    if let Some(c) = l.corridor {
        let (p, keep) = *pools.last().unwrap();
        assert_eq!((p.cx, p.cy), (c.x + c.width / 2, c.y + c.height / 2));
        assert_eq!((p.half_w, p.half_h), (14, 5));
        assert_eq!(keep, 1.0, "shared spaces keep their overhead light");
    }
}

#[test]
fn floor_shadow_ellipses_fit_each_family_in_paint_order() {
    use crate::layout::WaypointKind;
    let l =
        Layout::compute(192, 160, Some(crate::layout::TEST_DEFAULT_DESKS)).expect("192x160 fits");
    // Ellipse is Copy, not PartialEq — compare by field tuple.
    let e = |el: &Ellipse| (el.cx, el.cy, el.half_w, el.half_h);

    let mut expected: Vec<(u16, u16, u16, u16)> = Vec::new();
    for &desk in &l.home_desks {
        expected.push(e(&desk_shadow_ellipse(desk)));
    }
    for wp in l.waypoints.iter().filter(|w| {
        !matches!(
            w.kind,
            WaypointKind::Couch | WaypointKind::Printer | WaypointKind::Island
        )
    }) {
        let vis_w = crate::layout::furniture_def(wp.kind.furniture()).visual.w;
        let half_w = if vis_w > 0 { (vis_w / 2 + 1).min(7) } else { 7 };
        expected.push((wp.pos.x, wp.pos.y + 2, half_w, 2));
    }
    if let Some(island) = l.pantry.and_then(|p| p.kitchen_island) {
        let vis = crate::layout::furniture_def(crate::layout::Furniture::KitchenIsland).visual;
        expected.push((
            island.x,
            island.y + center_pin_south_offset(vis.h),
            vis.w / 2 + 1,
            2,
        ));
    }
    for wp in l
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::Printer)
    {
        expected.push((wp.pos.x, wp.pos.y + 1, 5, 1));
    }
    if let Some(c) = l.couch_sprite_center() {
        expected.push((c.x, c.y + 2, 7, 2));
    }
    for &PlantItem { kind, pos } in &l.plants {
        expected.push((
            pos.x,
            pos.y
                + center_pin_south_offset(crate::layout::furniture_def(kind.furniture()).visual.h),
            3,
            1,
        ));
    }
    if let Some(lamp) = l.floor_lamp() {
        expected.push((lamp.x, lamp.y + floor_lamp_south_offset(), 2, 1));
    }

    let got: Vec<_> = floor_shadow_ellipses(&l).map(|el| e(&el)).collect();
    assert_eq!(
        got, expected,
        "one fitted shadow per family member, emitted in paint order"
    );
}

#[test]
fn character_render_names_resolve_in_the_animation_registry() {
    use pixtuoid_core::sprite::format::{
        OPTIONAL_CHARACTER_ANIMATIONS, REQUIRED_CHARACTER_ANIMATIONS,
    };
    for n in [
        "seated",
        "typing",
        "standing",
        "walking",
        "walking_back",
        "walking_coffee",
        "holding_coffee",
        "seated_sleeping",
        "seated_sleeping_alt",
        "back_couch",
        "side_seated",
    ] {
        assert!(
            REQUIRED_CHARACTER_ANIMATIONS.contains(&n)
                || OPTIONAL_CHARACTER_ANIMATIONS.contains(&n),
            "character render name {n:?} is not a registered \
             REQUIRED_/OPTIONAL_CHARACTER_ANIMATIONS key"
        );
    }
}

// Sweeps gateway ports × wander phases: the escape is destination-hash-driven,
// so no single port/instant demonstrates it.
#[test]
fn a_roaming_creature_is_never_sliced_by_the_canvas_edge() {
    use pixtuoid_core::source::daemon::{apply_presence, DaemonInstanceKey, DaemonPresenceUpdate};
    use pixtuoid_core::state::DaemonInstanceId;
    use std::time::Duration;

    let pack = crate::embedded_pack::test_default_pack();
    let layout = Layout::compute_with_seed(192, 128, None, 0).expect("layout");
    let theme = crate::theme::theme_by_name("normal").expect("normal theme");
    let boot = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let coffee = HashMap::new();
    let motion = HashMap::new();
    let src = pixtuoid_core::source::openclaw::SOURCE_NAME;
    let pet = crate::pet::Pet::defaulted(crate::pet::PetKind::Cat);

    let mut escapes: Vec<String> = Vec::new();
    for port in 18900..18924u32 {
        // The pet's roam is keyed on the FLOOR seed, the mascot's on its instance
        // id — vary both, or the pet half of the sweep rides one trajectory.
        let floor = crate::floor::FloorMeta {
            floor_seed: u64::from(port),
            ..crate::floor::FloorMeta::ground()
        };
        let mut scene = SceneState::uniform(16);
        let key = DaemonInstanceKey::new(src, DaemonInstanceId::new(port.to_string()).expect("id"));
        apply_presence(
            &mut scene,
            &key,
            DaemonPresenceUpdate::GatewayUp { pid: Some(7) },
            boot,
        );
        // Past the enter stagger + walk-in, then across several wander cycles so
        // both the walking legs and the resting cells get sampled.
        for step in 0..24u64 {
            let now = boot + Duration::from_millis(6_000 + step * 1_700);
            let mut buf = RgbBuffer::filled(layout.buf_w, layout.buf_h, Rgb { r: 0, g: 0, b: 0 });
            let mut cache = FrameCache::new();
            let ctx = PaintCtx {
                scene: &scene,
                layout: &layout,
                pack: &pack,
                now,
                buf: &mut buf,
                cache: &mut cache,
                theme,
                floor,
                active_pet: None,
                floor_pet: Some(&pet),
                coffee: &coffee,
                motion: &motion,
                door_anim_max_ms: 0,
                debug_walkable: false,
            };
            let mut drawables = Vec::new();
            let pet_frame = enqueue_pet(&ctx, &[], &mut drawables);
            for m in enqueue_gateway_mascots(&ctx, &mut drawables) {
                let (w, h) = (m.w, m.h);
                if m.pos.x < w / 2
                    || m.pos.x + w.div_ceil(2) > layout.buf_w
                    || m.pos.y < h / 2
                    || m.pos.y + h.div_ceil(2) > layout.buf_h
                {
                    escapes.push(format!(
                        "mascot port {port} step {step} at {:?} ({w}x{h}) escapes {}x{}",
                        m.pos, layout.buf_w, layout.buf_h
                    ));
                }
            }
            if let Some(p) = pet_frame {
                let (w, h) = pack
                    .animation(p.anim)
                    .and_then(|a| a.frames.first())
                    .map_or((0, 0), |f| (f.width(), f.height()));
                if p.pos.x < w / 2
                    || p.pos.x + w.div_ceil(2) > layout.buf_w
                    || p.pos.y < h / 2
                    || p.pos.y + h.div_ceil(2) > layout.buf_h
                {
                    escapes.push(format!(
                        "pet step {step} at {:?} ({w}x{h}) escapes {}x{}",
                        p.pos, layout.buf_w, layout.buf_h
                    ));
                }
            }
        }
    }
    assert!(
        escapes.is_empty(),
        "every roamer must render whole inside the canvas: {escapes:#?}"
    );
}

#[test]
fn a_back_turned_seat_puts_the_occupant_past_the_desk_body() {
    use crate::layout::{Facing, Furniture};
    let desk_h = crate::layout::furniture_def(Furniture::Desk).visual.h;
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        let far = seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::South);
        let near = seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::North);
        assert!(
            far.y < desk.y,
            "a viewer-facing occupant sits ABOVE the desk row: {far:?} vs {desk:?}"
        );
        assert!(
            near.y >= desk.y,
            "a back-turned occupant must reach the desk's own row, not hover above \
             it: {near:?} vs {desk:?}"
        );
        assert!(
            near.y < desk.y + desk_h,
            "…but still overlap the desk body rather than float below it: \
             {near:?} vs desk {desk:?} + visual h {desk_h}"
        );
    }
}

#[test]
fn a_desk_lamp_is_lit_whichever_way_the_desk_seats_its_occupant() {
    use crate::layout::Facing;
    // A lamp is a FIXTURE on the desk's west wing, visible from either side; the
    // standby SCREEN is the one that gates on facing.
    for darkness in [0.0_f32, 0.5, 1.0] {
        let north = super::desk_light(Facing::North, darkness);
        let south = super::desk_light(Facing::South, darkness);
        assert_eq!(
            north.lamp, south.lamp,
            "the lamp may not depend on facing (darkness {darkness})"
        );
        assert!(
            south.screen_idle == 0.0 && north.screen_idle >= south.screen_idle,
            "only a back-turned desk shows its screen (darkness {darkness})"
        );
    }
    assert!(
        super::desk_light(Facing::South, 1.0).lamp > 0.0,
        "a viewer-facing desk must still light its lamp after dark"
    );
}

#[test]
fn a_lamp_casting_no_pool_is_not_drawn_lit() {
    // Whatever the fixture reads as, it must track the light it casts.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let desk = Point { x: 20, y: 14 };
    let bg = Rgb { r: 9, g: 9, b: 9 };
    let render = |strength: f32| {
        let mut buf = RgbBuffer::filled(60, 40, bg);
        super::drawable::paint_desk_lamp(&mut buf, desk, strength, theme);
        buf
    };
    let (dim, bright) = (render(0.05), render(1.0));
    let lum = |c: Rgb| 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
    assert!(
        lum(dim.get(desk.x, desk.y)) < lum(bright.get(desk.x, desk.y)),
        "a barely-lit lamp must not paint the same shade as a fully-lit one: \
         {:?} vs {:?}",
        dim.get(desk.x, desk.y),
        bright.get(desk.x, desk.y)
    );
}

/// Asserted on the DRAWABLE list, not on pixels: a chair's rect is lit by the
/// desk's ceiling pool, which this branch made facing-dependent, so contrasting
/// a north desk's rect against a south one measures the pool as much as the
/// chair — it passes with no chair drawn.
#[test]
fn every_north_facing_desk_enqueues_a_chair_and_no_south_one_does() {
    for seed in 0..8u64 {
        let layout =
            Layout::compute_with_seed(240, 160, Some(crate::layout::TEST_DEFAULT_DESKS), seed)
                .expect("240x160 lays out");
        let mut drawables = Vec::new();
        super::enqueue_desk_chairs(&layout, &mut drawables);
        // Keyed on the FULL position: desks in one pod column share an x, so an
        // x-only key silently folds a wrongly-chaired south desk onto its
        // north neighbour and the assertion cannot see it.
        let seated: std::collections::BTreeSet<(u16, u16)> = drawables
            .iter()
            .map(|d| match d.kind {
                super::DrawableKind::DeskChair { pos, .. } => (pos.x, pos.y),
                _ => panic!("enqueue_desk_chairs pushed a non-chair drawable"),
            })
            .collect();
        let want: std::collections::BTreeSet<(u16, u16)> = layout
            .home_desks
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(*i))
                    == crate::layout::Facing::North
            })
            .map(|(_, &d)| {
                (
                    super::anchors::seated_anchor_facing(
                        d,
                        super::drawable::DESK_CHAIR_BACK_W,
                        crate::layout::Facing::North,
                    )
                    .x,
                    d.y + 6,
                )
            })
            .collect();
        assert!(!want.is_empty(), "seed {seed}: fixture has no north desk");
        assert_eq!(
            seated, want,
            "seed {seed}: one chair per north desk, at the seat"
        );
    }
}

/// The painter half of the chair, on a BLANK buffer: no floor, no ceiling pool,
/// no hour — so the only thing that can move these pixels is the chair itself.
#[test]
fn paint_chair_back_writes_its_mask_and_nothing_outside_it() {
    const BG: Rgb = Rgb { r: 1, g: 2, b: 3 };
    let mut buf = RgbBuffer::filled(64, 32, BG);
    let at = Point { x: 20, y: 10 };
    super::drawable::paint_chair_back(&mut buf, at, crate::theme::theme_by_name("normal").unwrap());
    let painted: Vec<(u16, u16)> = (0..buf.height())
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf.get(x, y) != BG)
        .collect();
    assert!(!painted.is_empty(), "the chair must paint something");
    let (x0, x1) = (
        painted.iter().map(|p| p.0).min().unwrap(),
        painted.iter().map(|p| p.0).max().unwrap(),
    );
    let (y0, y1) = (
        painted.iter().map(|p| p.1).min().unwrap(),
        painted.iter().map(|p| p.1).max().unwrap(),
    );
    assert_eq!(
        (x0, y0),
        (at.x, at.y),
        "the mask must start at the requested top-left"
    );
    assert!(
        x1 < at.x + super::drawable::DESK_CHAIR_BACK_W && y1 < at.y + 8,
        "the chair painted outside its own box: {:?}..{:?}",
        (x0, y0),
        (x1, y1)
    );
}
