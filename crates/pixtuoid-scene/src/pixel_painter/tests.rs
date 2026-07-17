use super::anchors::{
    back_couch_anchor, seated_anchor, standing_at_desk_anchor, walking_anchor, waypoint_anchor,
    CHARACTER_SPRITE_W,
};
use super::seat::{seat_sprite, seat_sprite_in_pack, settle_seat_view, SeatView, DESK_SEAT_Z_OFF};
use super::wall::WALL_THICK_H_PX;
use super::*;
// Formerly reached via `super::*` off mod.rs's imports — now that PixelCtx no
// longer names them (it borrows the FloorCtx group), import them directly.
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
    let top_wall_h = top_margin - 4; // 44
    let h_y = 90u16; // a horizontal divider row
    let h_rows = [h_y];

    // Top joint: a segment starting at top_margin rises to the window band.
    let (yt, _) = stitch_vertical_wall(top_margin, 70, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        yt, top_wall_h,
        "top segment should connect up to the window band"
    );

    // Corner joint: a segment ending on the horizontal row extends down by
    // the horizontal's thickness to fill the inside corner.
    let (_, yb) = stitch_vertical_wall(60, h_y, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        yb,
        h_y + (WALL_THICK_H_PX - 1),
        "bottom should fill the corner"
    );

    // Bridge-up joint (the dual-meeting case): a segment starting ~6 px
    // below the cross wall is bridged up to meet it. This branch only fires
    // on variant-2 floors, so it has no end-to-end render guard.
    let (yt2, _) = stitch_vertical_wall(h_y + 6, 120, top_margin, top_wall_h, &h_rows);
    assert_eq!(yt2, h_y, "lower segment should bridge up to the cross wall");

    // No false bridge: a segment well below the tolerance stays put, and a
    // segment with no joints is returned unchanged.
    let (yt3, yb3) = stitch_vertical_wall(h_y + 20, 130, top_margin, top_wall_h, &h_rows);
    assert_eq!(
        (yt3, yb3),
        (h_y + 20, 130),
        "distant segment must not bridge"
    );
    let (yt4, yb4) = stitch_vertical_wall(60, 80, top_margin, top_wall_h, &[]);
    assert_eq!((yt4, yb4), (60, 80), "no joints → unchanged");
}

// The vertical-wall top raise is now ONE shared fn — `stitch_vertical_wall` in
// `layout::rooms::walls` — that BOTH the mask footprint (`wall_segment_rect`) and
// this painter's `enqueue_room_walls_v` call, so glass and blocked ground can't
// disagree by construction (pre-consolidation the raise was DUPLICATED
// renderer-vs-mask, which is what this "agreement" test originally guarded). It
// survives as a smoke test that the shared raise of a `top_margin`-rooted
// segment lands on the band row (`top_margin - WALL_BAND_TO_TOP_MARGIN`) the
// caller passes as `top_wall_h` — a mismatch opens a walkable slot at the wall top.
#[test]
fn vertical_wall_top_raise_lands_on_the_band_row() {
    let top_margin = 48u16;
    let tbm = crate::layout::WALL_BAND_TO_TOP_MARGIN;
    let top_wall_h = top_margin - tbm; // the band row the caller passes
    let band_row = top_margin.saturating_sub(tbm);
    let (stitch_raise, _) = stitch_vertical_wall(top_margin, 90, top_margin, top_wall_h, &[]);
    assert_eq!(
        stitch_raise, band_row,
        "the shared stitch must raise a band-rooted vertical wall top to the band row"
    );
}

#[test]
fn v_door_jambs_sit_flush_on_both_cut_ends() {
    // The glass painters are endpoint-INCLUSIVE, so a doorway's flanking
    // segments end exactly at the Doorway span's start.y/end.y — each jamb
    // must COVER its cut end, or a 1px glass sliver survives between post
    // and opening (the #560 review's empirically-confirmed off-by-one: the
    // top post originally excluded start.y while the bottom one was flush).
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let floor = Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    let mut buf = RgbBuffer::filled(20, 60, floor);
    // Wall segments [10, 24] + [38, 52] flanking the opening (24, 38).
    wall::paint_glass_wall_v(&mut buf, theme, 5, 10, 24);
    wall::paint_glass_wall_v(&mut buf, theme, 5, 38, 52);
    // Per-segment jambs (the y-sorted RoomWallV arm's job): the top segment's
    // SOUTH cut end (y_bot=24) → post starts DOOR_JAMB_PX-1 rows up so it ends
    // on 24; the bottom segment's NORTH cut end (y_top=38) → post starts at 38.
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
    // The jamb_left/jamb_right flags are computed at enqueue (the paint pass
    // has no layout access): gap.start == a segment's x1 ⇒ that segment's
    // RIGHT end gets the jamb; gap.end == a segment's x0 ⇒ LEFT end. Probe a
    // real meeting+pantry floor's drawables for exactly that join.
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
    // Vertical twin of the H test: a doorway splits a N-S run into a top
    // segment (ends at the opening's north edge → jamb on its SOUTH cut end)
    // and a bottom segment (starts at the opening's south edge → jamb on its
    // NORTH cut end). The top segment's south end is the DOOR (a free terminus,
    // no corner extend), so its z-anchor equals y_bot — the south-base anchor
    // that puts a walker behind the north cap. (A segment ending on a crossing
    // H wall anchors at its RAW end, < the corner-extended y_bot, so H stays on
    // top — asserted separately in the mask suite; not universal here.)
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
    // Occlusion: the horizontal wall's frosted glass rises GLASS_CAP_PX
    // north of its footprint, y-sorted at the south base — so a character
    // standing just NORTH of the wall (drawn earlier) is composited over
    // by the translucent glass. Stand in for that character with a vivid
    // warm pixel inside the cap band; the glass must shift it toward the
    // cool tone (red drops, blue rises) rather than leave it untouched.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let y_top = 20u16;
    // Place the stand-in at the REAL northmost row a routed walker's feet
    // can reach: footprint top `y_top` minus (OBSTACLE_PAD_PX=2 + 1) = the
    // first walkable row north of the wall. With GLASS_CAP_PX=6 the cap
    // (rows y_top-6..y_top-1) covers this row, so a walker's feet/lower legs
    // composite behind the glass. (The old test used y_top-2, a row inside
    // the blocked footprint+pad band that no walker ever occupies.)
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
    ); // carpet
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
    // Occlusion twin of the H test, now that the vertical wall is a y-sorted
    // RoomWallV anchored at its SOUTH base (`y_bot`): a character standing just
    // north of the wall's north cap (the visual-only overhang the mask leaves
    // walkable) is drawn earlier, and the frosted glass composites over them.
    // Stand in with a vivid warm pixel in the cap's own column; the glass must
    // cool it (red↓ blue↑) rather than leave it untouched.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (x_left, y_top, y_bot) = (10u16, 20u16, 40u16);
    // The cap overhang is the top WALL_TOP_OVERHANG_PX rows (visual-only floor a
    // walker can stand on). Row y_top is a seam glint (bright specular); probe
    // the next cap row at the soft east edge, the coolest column of the strip.
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
    ); // carpet
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
    // Lounge couch always looks at the window (Facing::North) → back view.
    assert_eq!(
        seat_sprite(WaypointKind::Couch, Facing::North),
        ("back_couch", false),
        "couch's seated facing is North (window) → back_couch, same path as the sofa"
    );
    // North-side sofa seat faces away → back view, no flip.
    assert_eq!(
        seat_sprite(WaypointKind::MeetingSofa, Facing::North),
        ("back_couch", false)
    );
    // South-side sofa seat faces the viewer → front seated, no flip.
    assert_eq!(
        seat_sprite(WaypointKind::MeetingSofa, Facing::South),
        ("seated", false)
    );
    // Head-of-table chairs sit in PROFILE facing the table: the base
    // side_seated sprite faces East (west chair), the east chair mirrors.
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
    // Character anims are never inherited from the embedded default
    // (merge_from is furniture-only), so a pre-side_seated custom pack must
    // show the front pose — a missing animation must never mean an
    // invisible sitter.
    use crate::layout::{Facing, WaypointKind};
    let full = crate::embedded_pack::load_sprite_pack(None).expect("pack");
    assert_eq!(
        seat_sprite_in_pack(&full, WaypointKind::MeetingChair, Facing::West),
        ("side_seated", true),
        "a pack WITH the profile sprite uses it"
    );
    // The charpack fixture predates side_seated — a real pre-F custom pack.
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

// Team Palette tests: build a slot with an explicit cwd + unknown_cwd flag.
#[cfg(test)]
fn make_slot_cwd(id_path: &str, cwd: &str, unknown_cwd: bool) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path(id_path);
    let mut s = make_slot(id, ActivityState::Idle); // reuse the existing builder's defaults
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
    ); // shirt
    p.insert(
        'H',
        Some(Rgb {
            r: 40,
            g: 50,
            b: 60,
        }),
    ); // hair
    p.insert(
        'S',
        Some(Rgb {
            r: 70,
            g: 80,
            b: 90,
        }),
    ); // skin
    p.insert(
        'X',
        Some(Rgb {
            r: 99,
            g: 99,
            b: 99,
        }),
    ); // unrelated key
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
    // X is not a recolor target — must pass through unchanged.
    assert_eq!(
        p.get('X'),
        Some(Some(Rgb {
            r: 99,
            g: 99,
            b: 99
        }))
    );
    // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
    // unlikely to be in any preset, so they should differ.
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
    // Shirt / hair / pants are unaffected by glow.
    assert_eq!(unlit.get('B'), green_glow.get('B'));
    assert_eq!(unlit.get('H'), green_glow.get('H'));
    assert_eq!(unlit.get('P'), green_glow.get('P'));
    // Green glow pushes skin's green channel up.
    let (Some(Some(Rgb { r: _, g: ug, b: _ })), Some(Some(Rgb { r: _, g: gg, b: _ }))) =
        (unlit.get('S'), green_glow.get('S'))
    else {
        panic!("S key missing")
    };
    assert!(
        gg > ug,
        "green glow should push skin green (lit={gg}, unlit={ug})"
    );
    // Blue glow pushes skin's blue channel up.
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
    // Edit and Bash should be different colors.
    assert_ne!(edit_tint, bash_tint, "Edit and Bash should differ");
}

#[test]
fn recolor_frame_substitutes_bhs_pixels() {
    let base = base_palette();
    // Build an agent palette where B/H/S are clearly distinguishable.
    let mut agent_pal = base.clone();
    agent_pal.insert('B', Some(Rgb { r: 200, g: 0, b: 0 })); // red shirt
    agent_pal.insert('H', Some(Rgb { r: 0, g: 200, b: 0 })); // green hair
    agent_pal.insert('S', Some(Rgb { r: 0, g: 0, b: 200 })); // blue skin

    // Frame: 1 pixel per palette key + 1 unrelated pixel + 1 transparent.
    let frame = Frame::from_pixels(
        5,
        1,
        vec![
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30,
            }), // matches base B → should become red
            Some(Rgb {
                r: 40,
                g: 50,
                b: 60,
            }), // matches base H → should become green
            Some(Rgb {
                r: 70,
                g: 80,
                b: 90,
            }), // matches base S → should become blue
            Some(Rgb {
                r: 123,
                g: 45,
                b: 67,
            }), // unrelated     → unchanged
            None, // transparent   → unchanged
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
    // If agent palette equals base, frame must come back identical.
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

/// Helper — build a minimal Drawable for sort-order tests. Uses the
/// MeetingTable variant since it carries no borrowed data.
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
    // Same anchor_y values — TimSort (Rust's stable sort) must
    // preserve insertion order. The y-sort relies on this so that
    // a character at the same anchor_y as the couch behind them
    // still paints first (matches the prior Pass 1 → Pass 1.5
    // layering).
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
    // A south-of-table meeting sofa renders the `back_couch` sprite
    // (Facing::North) — the sitter's body must be occluded BEHIND the
    // sofa back, same as the lounge couch. The back-view sitter's
    // y-sort key is `sofa.y + 2` (back_couch_anchor = stand.y - 7,
    // sprite_h = 9, stand.y = sofa.y); the back sofa must beat that.
    let sofa_y: u16 = 40;
    let sitter_anchor_y = (sofa_y - 7) + 9; // back_couch_anchor + sprite_h
    let back_sofa_anchor_y = sofa_y + 3; // faces_away bump
    let front_sofa_anchor_y = sofa_y + 2; // sitter-on-top default
    assert!(
        back_sofa_anchor_y > sitter_anchor_y,
        "back-view sofa must sort AFTER its sitter (paint on top): \
         sofa={back_sofa_anchor_y}, sitter={sitter_anchor_y}"
    );
    // Front sofa ties the sitter; insertion order (decor first) then
    // keeps the sitter on top — so it must NOT exceed the sitter.
    assert!(
        front_sofa_anchor_y <= sitter_anchor_y,
        "front-view sofa must not sort after its sitter: \
         sofa={front_sofa_anchor_y}, sitter={sitter_anchor_y}"
    );
}

#[test]
fn center_pin_south_offset_lands_on_the_sprite_south_row() {
    // A center-pinned sprite of height h blits at py = center - h/2, so its
    // south (front) ROW is `center + h - 1 - h/2`. The z-key must equal that
    // for BOTH parities — the round-1 fix used `h/2 - 1`, which is one short
    // for ODD h (the 11px whiteboard sorted in front of its own base).
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
    // Regression: the pet south-row z-key derives from the CHOSEN anim's
    // sprite height (not a hardcoded +2). The shorter sleep sprite must sort
    // one row NORTH of the walk/sit sprites — a literal +2 painted a sleeping
    // pet OVER a character whose feet land on pos.y+1. Reads the REAL embedded
    // heights so a pet-sprite resize surfaces HERE, not as a z-order bug.
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
    // The lamp's halo / shadow / z-anchor all use floor_lamp_south_offset();
    // for the 4×10 sprite that's +4 (the base disc). Locks the value so a
    // visual-height edit in the table surfaces HERE, not as a floating halo.
    assert_eq!(floor_lamp_south_offset(), 4);
}

#[test]
fn waypoint_depth_baseline_is_center_pinned_sprite_south() {
    use crate::layout::{furniture_def, WaypointKind};
    // These appliances are center-pinned, so the z-sort key is the sprite's
    // south ROW = pos.y + footprint.h/2 - 1 (NOT +h/2 — that overshoots by
    // one and lets the sprite paint over a character just in front). Lock
    // the corrected offsets (vending 6→2, printer 4→1), DERIVED from the
    // footprint so a shape edit surfaces here, not as a visual layering bug.
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
    // The home desk's walk anchor (desk_furniture_def's geometry, pure
    // algebraic) must land so the WALKING sprite anchor equals the SEATED
    // sprite anchor — zero pop on arrival. This identity is the contract
    // that lets desk_walk_anchor stay a pure fn instead of a side-probe; if
    // seated_anchor or walking_anchor ever change, this fails loudly.
    use crate::layout::desk_walk_anchor;
    for desk in [
        Point { x: 40, y: 30 },
        Point { x: 100, y: 60 },
        Point { x: 7, y: 5 }, // near-origin: saturating_sub edge
    ] {
        // The identity must hold for ANY pack character width — the bundled
        // 8-wide AND the robot 10-wide — because desk_walk_anchor's +4 / -8
        // cancel against the width-centering for every w.
        for w in [CHARACTER_SPRITE_W, 10] {
            assert_eq!(
                walking_anchor(desk_walk_anchor(desk), w),
                seated_anchor(desk, w),
                "walking_anchor(desk_walk_anchor({desk:?}), {w}) must equal seated_anchor",
            );
        }
    }
}

#[test]
fn seated_foot_cell_settles_exactly_on_the_render_anchor() {
    // The UNIFIED zero-pop identity: for every occupies_pos Furniture (the
    // seat kinds AND the home desk), the WALKING sprite anchor at
    // seated_foot_cell(S) must equal the SEATED render anchor at pos — so the
    // post-A* settle ends with zero pop on every arrival side. back_couch
    // render for couch/sofa, waypoint render for stand, seated_anchor for the
    // desk: ONE fn, the correctness lock for the whole convergence.
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
            // The chair occupant SITS (SeatView::Front), so its settle/render
            // pair with the SEAT anchor like the sofas: pairing it with the
            // waypoint anchor instead would leave the seated sprite hovering
            // rows above its chair.
            let s = seated_foot_cell(Furniture::MeetingChair, pos).expect("occupies_pos seat");
            assert_eq!(
                walking_anchor(s, w),
                back_couch_anchor(pos, w),
                "MeetingChair: walking_anchor(S={s:?}) must equal back_couch_anchor(pos={pos:?}) w={w}",
            );
            // The home desk flows through the SAME fn — its S is
            // desk_walk_anchor, its render seated_anchor. Same identity,
            // proving the desk genuinely converged into Furniture.
            let sd = seated_foot_cell(Furniture::Desk, pos).expect("desk is occupies_pos");
            assert_eq!(
                walking_anchor(sd, w),
                seated_anchor(pos, w),
                "Desk: walking_anchor(seated_foot_cell)={:?} must equal seated_anchor",
                walking_anchor(sd, w),
            );
        }
        // Obstacles have no fixed seat — their sprite renders AT the approach
        // cell, so seated_foot_cell is None.
        assert_eq!(seated_foot_cell(Furniture::Pantry, pos), None);
        assert_eq!(seated_foot_cell(Furniture::VendingMachine, pos), None);
    }
}

#[test]
fn settle_view_matches_the_seated_view_for_every_seat() {
    // The unification guarantee: the sit-down settle and the seated render
    // derive from ONE source (`SeatView::of`), so they can never disagree —
    // the "sit facing the wrong way then snap" bug cannot recur, for current
    // OR future seatable furniture (matched generically by having a settle
    // foot-cell, not a hardcoded kind list).
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
        // The sit-down glide onto this seat renders in the seat's view, at the
        // seat's stable z-key.
        assert_eq!(
            settle_seat_view(foot, &l),
            Some((view, view.z_key_for_seat(w.pos))),
            "settle onto {:?}@{:?} must use the seat view {view:?}",
            w.kind,
            w.pos
        );
        // Totality guard (review finding): a seat detected generically by its
        // foot-cell must NOT fall through `SeatView::of`'s upright catch-all —
        // every real seat maps to an explicitly-handled view, so a future seat
        // added to the Furniture table without a `SeatView::of` arm fails HERE
        // rather than silently rendering as an upright stander.
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
        // Single-source invariant: the seated sprite and the sit-down settle
        // agree on orientation (both back-view, or neither) — they cannot
        // diverge because both come from `view`.
        let seated_is_back = view.seated_sprite().0 == "back_couch";
        let (settle_is_back, _) = view.settle_walk();
        assert_eq!(
            seated_is_back, settle_is_back,
            "{:?}: seated render and sit-down settle must share orientation",
            w.kind
        );
        // For seats whose foot-cell is offset from the centre (couch/sofa),
        // the centre is an ordinary travel target — keeps travel facing.
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
    // Bartender slots sit INSIDE the island body: both the settled stander
    // (sim's AtWaypoint arm) and the sit-down glide (settle_seat_view) must
    // z-sort at the plain feet row — BELOW the island's own south-row key —
    // for the entire arc. A `Side`-style `pos+3` key would TIE with the
    // island's key and pop the sprite in front of the counter mid-glide.
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
    // The home desk joins the unified settle: its chair (seated_foot_cell(Desk)
    // = desk_walk_anchor) is a settle target, so the arrival glide onto it goes
    // through SeatView::Front (front-facing, stable z-key) — same path as the
    // sofas, no front-cross.
    use crate::layout::TEST_DEFAULT_DESKS;
    use crate::layout::{desk_walk_anchor, Furniture};
    let l = Layout::compute(192, 158, Some(TEST_DEFAULT_DESKS)).expect("fits");
    let desk = *l.home_desks.first().expect("at least one home desk");
    let chair = desk_walk_anchor(desk);
    assert_eq!(
        settle_seat_view(chair, &l),
        Some((SeatView::Front, desk.y + DESK_SEAT_Z_OFF)),
        "the desk chair {chair:?} must settle as SeatView::Front at the desk z-key"
    );
    // seated_foot_cell(Desk) is exactly desk_walk_anchor — the hook keys off it.
    assert_eq!(
        crate::layout::seated_foot_cell(Furniture::Desk, desk),
        Some(chair)
    );
    // A non-chair cell near the desk is ordinary travel.
    assert_eq!(
        settle_seat_view(desk, &l),
        None,
        "the desk corner is not the chair"
    );
}

#[test]
fn desk_settle_z_key_matches_the_seated_arm() {
    // The desk's settle z-key (desk.y + DESK_SEAT_Z_OFF) must equal the z-key
    // the seated desk arms use (anchor_no_breath.y + 12 with anchor =
    // seated_anchor) so the glide and the settled render sort identically —
    // and both stay below the desk furniture z-key (desk.y + visual.h).
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let seated_arm_z = seated_anchor(desk, w).y + 12;
            assert_eq!(
                desk.y + DESK_SEAT_Z_OFF,
                seated_arm_z,
                "desk settle z-key must equal the SeatedIdle/Typing arm z-key"
            );
            let visual_h = crate::layout::desk_furniture_def().visual.h;
            assert!(
                desk.y + DESK_SEAT_Z_OFF < desk.y + visual_h,
                "desk sitter must sort behind the desk furniture"
            );
        }
    }
}

#[test]
fn sit_arc_z_key_is_stable_and_on_the_right_side_of_its_furniture() {
    // The z-sort flicker fix. The sit-down/stand-up GLIDE and the SEATED state
    // must share ONE z-key (`z_key_for_seat`) so the agent never crosses its
    // furniture's z-key mid-glide (pop in front of the sofa for a frame, then
    // snap behind it). Asserts: (1) the seat z-key equals the historical
    // AtWaypoint formula (seated render unchanged); (2) it lands the agent on
    // the correct side of the furniture for every seat — behind a back-view
    // sofa/couch, on top of (tie with) a front sofa, and in front of the
    // meeting table for a stand.
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

        // (1) Behavior-preserving: equals the historical seated AtWaypoint key.
        let historical = match view {
            // back_couch_anchor.y + sprite_h(9) = (pos.y - 7) + 9. SideSeated
            // shares Front's seat anchor + bottom-row geometry by design.
            SeatView::Front | SeatView::Back | SeatView::SideSeated { .. } => {
                back_couch_anchor(w.pos, CHARACTER_SPRITE_W).y + 9
            }
            // waypoint_anchor.y + sprite_h(12) + 3 = (pos.y - 12) + 12 + 3
            SeatView::Side { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12 + 3,
            // waypoint_anchor.y + sprite_h(12) = pos.y — the AtWaypoint
            // default a plain stander historically used.
            SeatView::Stander { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12,
        };
        assert_eq!(
            z, historical,
            "{:?}@{:?}: seat z-key {z} must equal the historical AtWaypoint key {historical}",
            w.kind, w.pos
        );

        // (2) Correct side of the furniture.
        match w.kind {
            WaypointKind::Couch => {
                // Lounge couch furniture z-key = z_sort_row(Center, center, visual.h).
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
                    // Front sofa: tie at sofa.y+2 (insertion order keeps the
                    // sitter on top).
                    assert!(
                        z <= w.pos.y + 2,
                        "front sofa sitter z {z} must be <= sofa.y+2"
                    );
                }
            }
            WaypointKind::MeetingChair => {
                // The sitter must paint OVER its chair body (enqueued at
                // pos.y+1); the table never overlaps it horizontally, so no
                // table-side constraint applies.
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
    // The same "agent on the correct side of its furniture" guarantee the
    // wander-seat invariant gives, extended to the home desk so EVERY seatable
    // is covered. A seated or standing desk occupant must y-sort BEHIND the
    // desk cubicle (which sorts at `desk.y + visual.h` — pinned by
    // `desk_z_key_is_the_visual_south`). The desk
    // keeps its own render arms (different sprite/work-state by design), but
    // ties its character z-key to its furniture z-key so a footprint or anchor
    // edit can never drift the agent in front of its own desk (no flicker,
    // matching the wander seats — the z-order GUARANTEE is unified even though
    // the render code is intentionally not).
    let visual_h = crate::layout::desk_furniture_def().visual.h;
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let desk_furniture_z = desk.y + visual_h;
            // SeatedIdle / SeatedThinking / SeatedTyping z-key.
            let seated_z = seated_anchor(desk, w).y + 12;
            // StandingAtDesk z-key.
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

#[test]
fn desk_z_key_is_the_visual_south() {
    // The DeskCubicle z-sort baseline is `desk.y + visual.h` — a VISUAL
    // property (it must track the sprite, not the blocked ground, so the
    // walk-behind footprint shrink is z-neutral by construction). Density
    // desk: visual.h = DESK_H+2 = 7. Locks the value so a visual resize
    // surfaces here, not as a layering bug.
    assert_eq!(
        crate::layout::desk_furniture_def().visual.h,
        7,
        "desk z-key offset (DESK_H+2)"
    );
}

#[test]
fn every_pod_occludes_via_overhang() {
    // Occlusion is emergent now (no `occludes_behind` cap): every aisle pod's
    // sprite is TALLER than its shallow south-anchored ground footprint, so a
    // walker parks deep behind it and the overhang's own y-sort hides them.
    // Exhaustive over PodDecor::ALL so a new pod kind is forced through this.
    use crate::layout::{furniture_def, PodDecor, Size};
    assert_eq!(
        PodDecor::ALL.len(),
        5,
        "PodDecor variant added/removed — update ALL (and this count)"
    );
    for &kind in PodDecor::ALL {
        let def = furniture_def(kind.furniture());
        // z-sort precondition: the pod-decor loop anchors at
        // `center_pin_south_offset(visual.1)`, so a 0-height visual would
        // sort the sprite at its own center. Every pod must have visible h.
        assert!(
            def.visual.h > 0,
            "{kind:?}: pod decor needs a non-zero visual height for the z-sort"
        );
        // The overhang IS the occlusion: the sprite must rise above its
        // ground base, else a walker behind it wouldn't be hidden.
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
    // Occlusion for BOTH back-view seat renderers (lounge couch + the
    // north meeting sofa): the furniture must y-sort OVER the back-view
    // sitter so the sofa back occludes the body. The sitter's z-key is
    // `base + 2` (back_couch_anchor stand-7 + sprite_h 9); the back
    // furniture is `base + 3`. Lounge couch (`center.y + 3`) and the north
    // meeting sofa (`sofa.y + 3`) both satisfy it.
    let base: u16 = 40;
    let sitter = (base - 7) + 9; // = base + 2
    let couch_furniture = base + 3; // WaypointCouch drawable
    let back_meeting_sofa = base + 3; // faces_away meeting sofa
    assert!(couch_furniture > sitter, "couch must sort over its sitter");
    assert!(
        back_meeting_sofa > sitter,
        "north meeting sofa must sort over its sitter"
    );
}

#[test]
fn character_anchor_y_exceeds_desk_when_south_of_it() {
    // The bug-fix invariant: a character whose feet (anchor.y + 12)
    // land BELOW the desk's bottom row (desk.y + visual.h) must sort AFTER
    // the desk and therefore paint on top.
    let desk_y: u16 = 20;
    let desk_anchor_y = desk_y
        + crate::layout::furniture_def(crate::layout::Furniture::Desk)
            .visual
            .h;
    let char_feet_anchor = (desk_y + 10) + 12; // walker south of desk
    assert!(
        char_feet_anchor > desk_anchor_y,
        "walker south of desk must sort after it: char={char_feet_anchor}, desk={desk_anchor_y}"
    );
}

#[test]
fn character_anchor_y_below_desk_when_seated_at_it() {
    // Inverse invariant — a SEATED character at this desk has feet
    // that land ABOVE the desk's bottom (because they're tucked
    // under the desktop). They must sort BEFORE the desk so the
    // desk occludes their lower body in top-down view.
    let desk_y: u16 = 20;
    let seated_anchor = seated_anchor(Point { x: 0, y: desk_y }, CHARACTER_SPRITE_W);
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

// --- compute_door_frame_idx -------------------------------------------

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
    let opening = entry_slot(50, now); // frame 1
    let open = entry_slot(2_000, now); // frame 2
    assert_eq!(compute_door_frame_idx(&[opening, open], now, 0), 2);
}

#[test]
fn door_frame_uses_physics_window_when_nonzero() {
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // Slot spawned 3 s ago; with old ENTRY_ANIMATION_MS=4000 it would still
    // be mid-flight. Supply a short physics window (2500 ms) so it reads as
    // near the closing ramp instead.
    let short_window_ms: u64 = 2_500;
    // elapsed=3000, total=2500 → elapsed > total → door should be in closing
    // ramp or closed (remaining = 0 → frame 0).
    let slot = entry_slot(3_000, now);
    let frame = compute_door_frame_idx(&[slot], now, short_window_ms);
    assert_eq!(
        frame, 0,
        "with short physics window elapsed>total should yield closed door, got frame {frame}"
    );

    // Slot spawned 500 ms ago; physics window = 2500 ms → still well in the
    // middle (fully open frame = 2).
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

// --- waypoint_rank_offset_x decollision table -------------------------

#[test]
fn waypoint_rank_offset_x_decollision_table() {
    use super::anchors::waypoint_rank_offset_x;
    use crate::layout::WaypointKind;
    // rank 0 = first arrival, no offset, for every kind.
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Couch, 0), 0);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 0), 0);
    // Couch decollision is ±6 (3 seats on a 20px sofa).
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Couch, 1), 6);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Couch, 2), -6);
    assert_eq!(
        waypoint_rank_offset_x(WaypointKind::Couch, 3),
        0,
        "rank >2 collapses to 0"
    );
    // Generic kinds step aside ±9.
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 1), 9);
    assert_eq!(waypoint_rank_offset_x(WaypointKind::Pantry, 2), -9);
    assert_eq!(
        waypoint_rank_offset_x(WaypointKind::Pantry, 5),
        0,
        "rank >2 collapses to 0"
    );
}

// --- tool_glow_tint kind arms ------------------------------------------

/// Render-parity net for the ToolDetail → ToolKind → glow pipeline: for a
/// table of representative production displays, deriving the kind exactly as
/// the reducer does at slot entry (`ToolKind::from_detail`, detail-less →
/// `Other`) must reproduce the tint the old per-frame first-token string
/// parse produced. Each expected value below IS that old parse's answer for
/// the display — change this table only when the glow policy itself changes.
#[test]
fn kind_derivation_reproduces_the_string_parse_tint_for_representative_displays() {
    use pixtuoid_core::ToolDetail;
    let id = pixtuoid_core::AgentId::from_transcript_path("/g.jsonl");
    let glow = &crate::theme::NORMAL.tool_glow;
    // Mirror the reducer's slot entry: detail typed → (display string, kind).
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
        // Delegation is TYPED (displays "Delegating") → glow.agent.
        (Some(ToolDetail::Task), glow.agent),
        (Some(generic("Edit src/main.rs")), glow.edit),
        (Some(generic("Write: src/foo.rs")), glow.edit),
        (Some(generic("MultiEdit lib.rs")), glow.edit),
        (Some(generic("Read: README.md")), glow.read),
        (Some(generic("Bash: cargo test")), glow.bash),
        (Some(generic("Grep: TODO")), glow.grep),
        (Some(generic("Glob **/*.rs")), glow.grep),
        // Unknown tool → glow.default.
        (Some(generic("WebFetch https://x")), glow.default),
        // Detail-less Active (old parse: empty token) → glow.default.
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
    // The one DELIBERATE divergence from the old token parse: a Generic tool
    // whose display merely spells a delegation word is NOT kind Task — it
    // glows default and (the real payoff) never rides the reducer's
    // delegation stale-window carve-out. Impossible from production decoders,
    // which type every dispatch as ToolDetail::Task upstream.
    assert_eq!(
        palette::tool_glow_tint(&active(Some(&generic("Delegating imposter"))), glow),
        Some(glow.default)
    );
}

#[test]
fn tool_glow_for_kind_is_the_shared_kind_to_hue_map() {
    use pixtuoid_core::state::ToolKind;
    let glow = &crate::theme::NORMAL.tool_glow;
    // The pure ToolKind→hue seam the binary's footer reads directly, so a tool
    // segment tints identically to the sprite's monitor glow.
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
    // tool_glow_tint now delegates: Active → Some(mapped hue), off-Active → None.
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

// --- degraded_pixel / degraded_frame (#317 unwell gateway) -------------

#[test]
fn degraded_pixel_desaturates_reddens_and_dims() {
    // Hand-traced through the three blend stages for a pure-white input:
    //   lum=255 → gray={255,255,255}; desat (0.55)={255,255,255};
    //   tinted (0.45 toward {150,40,40})={208,158,158};
    //   dim (0.18 toward black, ×0.82)={171,130,130}.
    // The exact-equality assert is the mutation-killer: a dropped blend
    // stage or a wrong factor changes the bytes.
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
    // Property: a pure-green input has r==b==0 going in; the red bias (toward
    // {150,40,40}) lifts the red channel ABOVE the blue channel, and both end
    // strictly above their input 0 — so red > blue is a falsifiable witness of
    // the red tint (drop the red-bias stage and the desaturate-only result has
    // r == b for a symmetric-in-r/b input). The green channel, though dragged
    // down by desaturate+dim, is also dimmed below its 255 max.
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
    // Mirrors recolor_frame_substitutes_bhs_pixels' shape: a 2×1 frame with
    // one opaque + one transparent pixel.
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
    // Opaque pixel runs through degraded_pixel (the {255,255,255}→{171,130,130}
    // transform proven above).
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
    // Transparency preserved — the falsifiable branch: a mutant dropping the
    // .map None-arm (or recoloring transparent pixels) fails here.
    assert_eq!(
        out.as_slice()[1],
        None,
        "transparent pixel must stay transparent"
    );
    // Identity-mutant guard: the opaque pixel actually changed.
    assert_ne!(out.as_slice()[0], frame.as_slice()[0]);
}

// --- SeatView::of obstacle (upright) arm -------------------------------

#[test]
fn seat_view_of_obstacle_kinds_is_upright_unflipped() {
    use crate::layout::{Facing, WaypointKind};
    // The non-seat obstacle kinds dispatch directly in production and never
    // reach a seated render through SeatView, but the explicit arm maps them to
    // the upright default (Side { flip: false }) for totality.
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

// --- burn tier: ember hair + flame crown through the one shared blit ---

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
    // The painter's own constants — not re-hardcoded copies.
    const EMBER: Rgb = super::effects::FLAME_DEEP;
    const TIP: Rgb = super::effects::FLAME_TIP;

    // Normal (no model): natural hair, no flame colors anywhere.
    let plain = render(&slot);
    assert!(
        !has(&plain, EMBER) && !has(&plain, TIP),
        "Normal must not burn"
    );

    // Premium (top model, no fresh max effort): ember hair, still no flame.
    slot.model = Some("claude-fable-5".into());
    let ember = render(&slot);
    assert!(has(&ember, EMBER), "Premium recolors the hair to ember");
    assert!(!has(&ember, TIP), "Premium must not flame");
    assert_ne!(plain.as_slice(), ember.as_slice());

    // Top (fresh max effort): the flame crown paints ABOVE the sprite anchor.
    slot.effort = Some(EffortObservation::new("ultra".into(), now));
    let burning = render(&slot);
    assert!(has(&burning, TIP), "Top paints flame tips");
    let above = (0..anchor.y).any(|y| (0..32).any(|x| burning.get(x, y) != black));
    assert!(above, "the crown must rise above the sprite's top row");

    // TTL decay: a stale effort falls back to ember (no flame).
    slot.effort = Some(EffortObservation::new(
        "ultra".into(),
        now - Duration::from_secs(crate::burn::EFFORT_TTL_SECS + 1),
    ));
    let decayed = render(&slot);
    assert!(!has(&decayed, TIP), "stale effort must decay the flame");
    assert!(has(&decayed, EMBER), "…back to ember hair");
}

// --- paint_character_at defensive missing-anim early return -----------

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

// --- glass bounds clamps ----------------------------------------------

#[test]
fn glass_wall_h_clamps_below_buffer_bottom() {
    // y_top near the buffer bottom → the cap+face row span exceeds the height,
    // so the per-row `y >= bh continue` fires. Must not panic; in-bounds rows
    // still paint.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bh = 16u16;
    let mut buf = RgbBuffer::filled(40, bh, Rgb { r: 0, g: 0, b: 0 });
    paint_glass_wall_h(&mut buf, theme, 0, 39, bh - 1);
    // The cap rows that ARE in-bounds (above bh) must have painted something.
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
    // The dx==0 column (in-bounds) must have painted.
    let mut painted = false;
    for y in 5..21u16 {
        if buf.get(bw - 1, y) != (Rgb { r: 0, g: 0, b: 0 }) {
            painted = true;
        }
    }
    assert!(painted, "the in-bounds glass column should paint");
}

// --- effects: pet hearts edges ------------------

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
    // Past HEART_LIFE_MS (1550) for the first heart but the later staggered
    // hearts are also dead (i=1 starts at 150 → dead by 1700; ... i=3 at 450 →
    // dead by 2000). At elapsed=2100 all four hearts are past their life → the
    // `local_ms >= HEART_LIFE_MS continue` (152) fires for each → nothing paints.
    assert_eq!(
        painted_count(2_100),
        0,
        "all hearts past their life → none paint"
    );
    // A fresh frame DOES paint (proves the count isn't vacuously 0).
    assert!(painted_count(0) > 0, "first heart paints at t=0");
    // alpha < 0.05 continue (158): for heart i=0, local_ms in [1473,1549] gives
    // alpha just under 0.05 → that heart is skipped while still within its life.
    // Compare the heart count at elapsed=1500 (i=0 faded) vs a fresh stagger
    // where i=0 is bright — fewer hearts at the faded frame proves 158 fired.
    // (i=1..3 may still be alive at 1500, so just assert no panic + bounded.)
    let faded = painted_count(1_500);
    assert!(
        faded <= painted_count(300),
        "the faded heart drops out (alpha<0.05)"
    );
}

// --- furniture decor guards + bodies + corner clip --------------------

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
    assert_noop(&|b| paint_doormat(b, small, theme));
    assert_noop(&|b| paint_water_cooler(b, small, std::time::SystemTime::UNIX_EPOCH, theme));
    assert_noop(&|b| paint_trash_bin(b, small));
}

#[test]
fn furniture_room_decor_large_bounds_paint() {
    use super::furniture::{
        paint_doormat, paint_notice_board, paint_trash_bin, paint_water_cooler,
    };
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 9, g: 9, b: 9 };
    // A generous room: width 40, height 40, well above every guard threshold.
    let big = crate::layout::Bounds {
        x: 4,
        y: 4,
        width: 40,
        height: 40,
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
    assert_paints(&|b| paint_doormat(b, big, theme));
    assert_paints(&|b| paint_water_cooler(b, big, std::time::SystemTime::UNIX_EPOCH, theme));
    assert_paints(&|b| paint_trash_bin(b, big));
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
    // The public `--weather` override entry point. Three arms:
    //   Some(known, case-insensitive) → Ok + override SET (observable through
    //     the single weather_state chokepoint, which all weather derivation
    //     funnels through);
    //   None → Ok + override CLEARED (time-based selection restored);
    //   Some(unknown) → Err(weather_names()) WITHOUT touching the override.
    // The override is a thread-local Cell, so all asserts run on one thread
    // and we reset to None at the very end so the override can't leak into the
    // time-based weather_state_* sibling tests sharing this thread.
    //
    // Sentinel time whose natural (un-forced) weather is NOT Storm — so a
    // mutant that drops the set_weather_override call (leaving the time-based
    // value) is caught by the observed-weather assert, not just the Ok/Err
    // return value.
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
    force_weather(None).expect("clear is Ok");
    let natural = background::weather_state(t);

    // Known name → Ok, and weather_state now FORCES that exact variant.
    assert!(force_weather(Some("storm")).is_ok(), "known name → Ok");
    assert_eq!(
        background::weather_state(t),
        background::Weather::Storm,
        "force_weather(storm) must drive weather_state to Storm",
    );
    // Forcing pins it regardless of time (the override bypasses time selection).
    assert_eq!(
        background::weather_state(t + std::time::Duration::from_secs(987_654)),
        background::Weather::Storm,
        "the override must ignore the clock",
    );

    // Case-insensitive Ok arm → same forced variant.
    assert!(
        force_weather(Some("STORM")).is_ok(),
        "case-insensitive → Ok"
    );
    assert_eq!(background::weather_state(t), background::Weather::Storm);

    // A different known name re-targets the override (proves set, not stuck).
    assert!(force_weather(Some("snow")).is_ok());
    assert_eq!(
        background::weather_state(t),
        background::Weather::Snow,
        "a second known name must re-set the override",
    );

    // Unknown name → Err carrying the canonical names, and the override is
    // UNTOUCHED (weather_state still reads the previously-forced Snow).
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

    // None → Ok and the override is CLEARED (natural time-based value back).
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
    // site/src/weather.json drives the site's weather gallery AND the gen-media
    // render loop; the `Weather` enum drives what actually renders. Site CI never
    // runs the binary, so nothing else ties the two together — this test is the
    // bridge: manifest ids must equal the canonical names, in order. (A new or
    // renamed variant fails here until the manifest + `just gen-media` art
    // are updated with it.)
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
    // Same cwd, DIFFERENT agent ids.
    let a = make_slot_cwd("/demo/api/aaaa.jsonl", "/demo/api", false);
    let b = make_slot_cwd("/demo/api/bbbb.jsonl", "/demo/api", false);
    let pa = agent_palette(&base, &a, None, crate::burn::BurnTier::Normal);
    let pb = agent_palette(&base, &b, None, crate::burn::BurnTier::Normal);
    // Same cwd => same outfit (shirt 'B' + pants 'P').
    assert_eq!(pa.get('B'), pb.get('B'), "same cwd should share shirt");
    assert_eq!(pa.get('P'), pb.get('P'), "same cwd should share pants");
    // Different agent_id => hair/skin still differ (individuals stay distinct).
    assert_ne!(
        (pa.get('H'), pa.get('S')),
        (pb.get('H'), pb.get('S')),
        "different agents in the same repo must differ in hair/skin"
    );
}

#[test]
fn agent_palette_unknown_cwd_falls_back_to_id_outfit() {
    let base = Palette::default();
    // unknown_cwd and empty-cwd both fall back to the agent_id-seeded outfit.
    let unknown = make_slot_cwd("/x/aaaa.jsonl", "/whatever", true);
    let empty = make_slot_cwd("/x/aaaa.jsonl", "", false);
    let p_unknown = agent_palette(&base, &unknown, None, crate::burn::BurnTier::Normal);
    let p_empty = agent_palette(&base, &empty, None, crate::burn::BurnTier::Normal);
    // Same agent_id under both fallback triggers => identical outfit.
    assert_eq!(p_unknown.get('B'), p_empty.get('B'));
    assert_eq!(p_unknown.get('P'), p_empty.get('P'));
    // Fallback preserves per-agent variety: two cwd-less agents with different
    // ids must NOT collapse to one "unknown" outfit.
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
    // A slot first seen without a cwd caches frames in the agent_id-seeded
    // fallback outfit; core's backfill_identity then heals (cwd, unknown_cwd)
    // on the next identity-bearing event. Already-cached poses must repaint
    // in the healed Team-Palette outfit — pinned by comparing the healed
    // repaint (same cache) against a fresh-cache render.
    let pack = crate::embedded_pack::test_default_pack();
    let unknown = make_slot_cwd("/p/heal.jsonl", "", true);
    // Pick a cwd whose Team-Palette outfit differs from the id-seeded
    // fallback outfit, so the assertion has teeth.
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

    // Heal the cwd, repaint the SAME pose through the SAME cache.
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

    // Ground truth: the same repaint through a FRESH cache.
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
    // Same id stem, different cwds chosen to land on different pool indices.
    let a = make_slot_cwd("/p/aaaa.jsonl", "/demo/api", false);
    let b = make_slot_cwd("/p/aaaa.jsonl", "/demo/infra", false);
    let pa = agent_palette(&base, &a, None, crate::burn::BurnTier::Normal);
    let pb = agent_palette(&base, &b, None, crate::burn::BurnTier::Normal);
    assert_ne!(
        pa.get('B'),
        pb.get('B'),
        "different cwds should pick different outfits"
    );
    // Hair/skin (same id) stay identical regardless of cwd.
    assert_eq!(pa.get('H'), pb.get('H'));
    assert_eq!(pa.get('S'), pb.get('S'));
}

// --- the sim/paint split (the two-phase seam behind render_to_rgb_buffer) --

/// Shared rig for the seam tests: one fresh agent (mid entry-walk), a real
/// layout, and every sim store — no pixel buffer anywhere near the sim half.
/// The six sim stores, owned — each sim test needs the full mutable set and
/// hand-rolling them per test triple-spelled the bundle (E-review nit).
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

// The x-span of the overlay's blocked cells (one AtWaypoint agent ⇒ one rect,
// so the bbox width IS char_w). None when nothing is reserved.
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

// A wide (10px) `standing` pack makes char_w=10 ≠ the bundled CHARACTER_SPRITE_W=8,
// so the AtWaypoint occupancy reservation must span 10, not the const (#606's fix;
// #609 — the 8-wide bundled pack can't tell them apart, so a revert to the const
// survives every existing sim_step test).
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

    // `pose::derive` is pack-INDEPENDENT (pose only), so the AtWaypoint `now` it finds
    // is stationary for BOTH packs. Scan idle agents × a window (some cycles roll Aimless).
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

#[test]
fn sim_step_advances_motion_without_painting() {
    // The whole point of the split: the world advances with NO pixel buffer
    // in sight — sim_step's signature has no RgbBuffer, and this test never
    // constructs one. A fresh agent is mid entry-walk; two ticks apart the
    // walk must have progressed and the motion store must hold its leg.
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
    // The observable outcomes are on the frame, not behind a render: the
    // placement resolved to a walking sprite, and the motion store holds the
    // snapshotted entry leg.
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
    // The immutability proof for the paint half: painting the SAME SimFrame
    // twice yields byte-identical buffers and moves NO sim state. The type
    // system already bars paint from the stores (PaintCtx carries no `&mut`
    // sim store — router/overlay absent entirely, motion an immutable view);
    // this pins the observable halves: light level, motion, history,
    // chitchat all unchanged, pixels reproducible.
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
    // Taste pin from the interior-decor mock round (owner picked SOFT over
    // keep/narrow): stride-10 lattice, border rows only. The old stride-6 +
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
    // Inner-edge rows are plain weave now — no second stripe line.
    assert_eq!(
        buf.get(2, 5),
        base,
        "inner-edge row (dx=2,dy=1) must be base"
    );
    // The old stride-6 lattice point (dx=2,dy=4: 6%6==0) dissolves to weave...
    assert_eq!(
        buf.get(2, 8),
        base,
        "old stride-6 lattice point must be base"
    );
    // ...and the sparse stride-10 lattice appears where the old one had none.
    assert_eq!(buf.get(7, 7), stripe, "(dx+dy)=10 lands on the new lattice");
}

#[test]
fn pantry_doorway_gets_a_centered_entry_mat() {
    // Decor-arc taste pin (owner picked B1): an entry mat centered under the
    // pantry's north doorway, echoing the meeting-room doormat convention.
    // One clear row separates it from the wall face (derived from the SAME
    // WALL_THICK_H the impl offsets by, so they can't drift apart).
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
    // Decor-arc taste pin (owner picked B2 alongside B1): a thin bordered mat
    // under the island whose south sliver peeks out in front of the bar.
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
    // Under-furniture order: the island BODY paints over the mat's center
    // (the mat lives in the background pass; only its south sliver shows).
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
    // Both soft-goods mats derive from in-pantry anchors; neither may bleed
    // past the room, whatever the floor size.
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
    // The aquarium's geometry derives from its FurnitureDef row (14x11,
    // center-anchored like the mask stamp); fish patrol their lanes on the
    // anim clock, so SOME cell in each lane must carry a fish color at any
    // instant. Frame reuses room_wall_trim_dark, cabinet reuses wood_*.
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
    // The chair body's backrest bar rides the OUTER side — reinforcing the
    // profile sitter's orientation, carrying it alone when the chair is
    // empty; cushion from the chair_* theme family shared with the desk
    // chairs.
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
    // back_west: the backrest column sits west of the cushion.
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
    // The chair consts are deliberate copies of the pack palette's couch
    // fabric ("C"/"G") — the sofa is an un-themed sprite, so the painter
    // can't read Theme for it. This pin makes a sofa retint fail HERE
    // instead of silently stranding the chairs in the old fabric.
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
    // The three-way identity the floating-sitter bug broke silently: the
    // seat render anchor (pos.y − SEAT_RENDER_Y_OFF) + the REAL seated
    // sprite's height − 1 must land exactly on SeatView::Front's z-key row
    // (pos.y + 2) — which sits INSIDE the 7-row chair body, so the sitter
    // visibly occupies the cushion. The z-key tests alone pass even when the
    // sprite hovers rows above the chair, so this pins the full identity.
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
    // B-4 (owner-ratified): appliance feedback — a page slides out of the
    // tray only while an agent stands at the printer. Probe one mid-eject
    // phase for paper below the body, and the same instant with busy=false
    // for stillness.
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
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme);
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
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme);
        buf
    };
    let busy = render(true);
    let idle = render(false);
    // The pickup slot shows a drink color mid-drop; idle shows the machine's
    // trim there.
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
    // Ambient like the coffee steam: a lit-water bubble climbs the bottle on
    // a fixed cycle. The bubble reuses tank_water_line (THE lit-water color)
    // — NOT the mascot harness's #d6f2f8 sentinel.
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let bg = Rgb { r: 1, g: 2, b: 3 };
    let pr = crate::layout::Bounds {
        x: 4,
        y: 4,
        width: 30,
        height: 40,
    };
    let render = |ms: u64| {
        let mut buf = RgbBuffer::filled(60, 60, bg);
        furniture::paint_water_cooler(
            &mut buf,
            pr,
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
    // Pins the two halves of the busy wiring the pixel tests can't reach
    // (both survived as mutants in review): (a) sim_step publishes wp_rank's
    // keys as occupied_waypoints; (b) the appliance enqueuer turns
    // membership into the drawable's busy flag.
    use std::time::Duration;
    let (scene, layout, _id, now0, pack) = sim_rig();
    let coffee = HashMap::new();
    let mut owned = OwnedSimStores::new();
    let mut stores = owned.stores();
    // (a) walk the idle agent through wander cycles until it settles at SOME
    // waypoint; the frame must report that occupancy.
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
    // (b) synthetic membership must surface as busy on the drawable — on a
    // floor tall enough to host the corridor appliances.
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
fn precipitation_level_maps_audible_rain_and_honors_the_override() {
    // The audio model's weather feed: storm=1.0, rain=in-between, everything
    // else (incl. snow — precipitation you can't HEAR) = 0.0. Runs through
    // the same force_weather override as rendering; thread-local, so reset
    // to None at the end (the sibling-test leak rule above).
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
