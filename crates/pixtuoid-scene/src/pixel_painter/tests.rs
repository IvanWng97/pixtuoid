use super::seat::DESK_SEAT_Z_OFF;
use super::*;
use pixtuoid_core::sprite::{Frame, Palette};
use pixtuoid_core::state::GlobalDeskIndex;
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

// The vertical-wall top raise lives in TWO crates — the renderer
// (stitch_vertical_wall, here) and the mask (build_walkable_mask, core).
// The mask raises a top_margin-rooted segment to
// `top_margin - WALL_BAND_TO_TOP_MARGIN`; the renderer raises it to
// `top_wall_h`, which the binary derives from the SAME const. If they ever
// disagree a walkable slot opens at the wall top (the regression
// `vertical_wall_is_impassable_except_through_the_door` guards). Extracting
// the rule into core would drag tui wall constants across the crate
// boundary (invariant #1); this test pins the agreement instead.
#[test]
fn vertical_wall_top_raise_agrees_between_renderer_and_mask() {
    let top_margin = 48u16;
    let tbm = pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN;
    let top_wall_h = top_margin - tbm; // what the binary passes the renderer
    let mask_raise = top_margin.saturating_sub(tbm); // what mask.rs computes
    let (renderer_raise, _) = stitch_vertical_wall(top_margin, 90, top_margin, top_wall_h, &[]);
    assert_eq!(
        renderer_raise, mask_raise,
        "renderer + mask must raise the vertical wall top to the same row"
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
    // West stand (layout marks it Facing::East) mirrors toward the table.
    assert_eq!(
        seat_sprite(WaypointKind::MeetingStand, Facing::East),
        ("standing", true)
    );
    // East stand (Facing::West) is unmirrored.
    assert_eq!(
        seat_sprite(WaypointKind::MeetingStand, Facing::West),
        ("standing", false)
    );
}

fn make_slot(id: pixtuoid_core::AgentId, state: ActivityState) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: id,
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/x").as_path()),
        label: Arc::from("x"),
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
    }
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
    let a = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
    let b = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
    assert_eq!(a.get('B'), b.get('B'));
    assert_eq!(a.get('H'), b.get('H'));
    assert_eq!(a.get('S'), b.get('S'));
}

#[test]
fn agent_palette_overrides_only_bhs_keys() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
    let base = base_palette();
    let p = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
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
    let unlit = agent_palette(&base, &slot, None);
    let green_glow = agent_palette(
        &base,
        &slot,
        Some(Rgb {
            r: 140,
            g: 240,
            b: 170,
        }),
    );
    let blue_glow = agent_palette(
        &base,
        &slot,
        Some(Rgb {
            r: 100,
            g: 160,
            b: 255,
        }),
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
        },
    );
    let bash_slot = make_slot(
        id,
        ActivityState::Active {
            tool_use_id: None,
            detail: Some(Arc::from("Bash: ls")),
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
    assert_eq!(out.width, 5);
    assert_eq!(out.height, 1);
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
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let pos = Point { x: 40, y: 30 };
    let anim_h = |name: &str| {
        pack.animation(name)
            .and_then(|a| a.frames.first())
            .map(|f| f.height)
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
    use pixtuoid_core::layout::{seated_foot_cell, Furniture};
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
            let s = seated_foot_cell(Furniture::MeetingStand, pos).expect("occupies_pos seat");
            assert_eq!(
                walking_anchor(s, w),
                waypoint_anchor(pos, w),
                "MeetingStand: walking_anchor(S={s:?}) must equal waypoint_anchor(pos={pos:?}) w={w}",
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
    use crate::layout::{Facing, WaypointKind, MAX_VISIBLE_DESKS};
    let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
    let seats: Vec<_> = l
        .waypoints
        .iter()
        .filter(|w| pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some())
        .collect();
    assert!(
        seats.iter().any(
            |w| matches!(w.kind, WaypointKind::Couch | WaypointKind::MeetingSofa)
                && w.facing == Facing::North
        ),
        "this layout size must have a window-facing (North) seat to exercise the fix"
    );
    for w in &seats {
        let foot = pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos)
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
            matches!(w.kind, WaypointKind::Couch | WaypointKind::MeetingSofa)
                || matches!(w.kind, WaypointKind::MeetingStand),
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
fn settle_seat_view_recognizes_the_home_desk() {
    // The home desk joins the unified settle: its chair (seated_foot_cell(Desk)
    // = desk_walk_anchor) is a settle target, so the arrival glide onto it goes
    // through SeatView::Front (front-facing, stable z-key) — same path as the
    // sofas, no front-cross.
    use crate::layout::MAX_VISIBLE_DESKS;
    use pixtuoid_core::layout::{desk_walk_anchor, Furniture};
    let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
    let desk = *l.home_desks.first().expect("at least one home desk");
    let chair = desk_walk_anchor(desk);
    assert_eq!(
        settle_seat_view(chair, &l),
        Some((SeatView::Front, desk.y + DESK_SEAT_Z_OFF)),
        "the desk chair {chair:?} must settle as SeatView::Front at the desk z-key"
    );
    // seated_foot_cell(Desk) is exactly desk_walk_anchor — the hook keys off it.
    assert_eq!(
        pixtuoid_core::layout::seated_foot_cell(Furniture::Desk, desk),
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
    // and both stay below the desk furniture z-key (desk.y + 8).
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let seated_arm_z = seated_anchor(desk, w).y + 12;
            assert_eq!(
                desk.y + DESK_SEAT_Z_OFF,
                seated_arm_z,
                "desk settle z-key must equal the SeatedIdle/Typing arm z-key"
            );
            let fp_h = crate::layout::desk_furniture_def()
                .footprint
                .expect("desk footprint")
                .h;
            assert!(
                desk.y + DESK_SEAT_Z_OFF < desk.y + fp_h + DESK_FRONT_OVERHANG,
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
        furniture_def, z_sort_row, Anchor, Facing, Furniture, WaypointKind, MAX_VISIBLE_DESKS,
    };
    let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
    let mut saw_back = false;
    for w in l
        .waypoints
        .iter()
        .filter(|w| pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some())
    {
        let view = SeatView::of(w.kind, w.facing);
        let z = view.z_key_for_seat(w.pos);

        // (1) Behavior-preserving: equals the historical seated AtWaypoint key.
        let historical = match view {
            // back_couch_anchor.y + sprite_h(9) = (pos.y - 7) + 9
            SeatView::Front | SeatView::Back => back_couch_anchor(w.pos, CHARACTER_SPRITE_W).y + 9,
            // waypoint_anchor.y + sprite_h(12) + 3 = (pos.y - 12) + 12 + 3
            SeatView::Side { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12 + 3,
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
            WaypointKind::MeetingStand => {
                // Stand clears the meeting table row it stands beside.
                assert!(
                    z > w.pos.y + 2,
                    "stand z {z} must clear the table at pos.y+2"
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
    // desk cubicle (which sorts at `desk.y + footprint.h + DESK_FRONT_OVERHANG`
    // — pinned by `desk_z_key_is_footprint_front_plus_overhang`). The desk
    // keeps its own render arms (different sprite/work-state by design), but
    // ties its character z-key to its furniture z-key so a footprint or anchor
    // edit can never drift the agent in front of its own desk (no flicker,
    // matching the wander seats — the z-order GUARANTEE is unified even though
    // the render code is intentionally not).
    let fp_h = crate::layout::desk_furniture_def()
        .footprint
        .expect("desk has a footprint")
        .h;
    for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
        for w in [CHARACTER_SPRITE_W, 10] {
            let desk_furniture_z = desk.y + fp_h + DESK_FRONT_OVERHANG;
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
fn desk_z_key_is_footprint_front_plus_overhang() {
    // The DeskCubicle z-sort baseline is `desk.y + footprint.h +
    // DESK_FRONT_OVERHANG` — footprint-front-derived (consistent with the
    // waypoint/wall baselines), not a bare sprite-bottom literal. Equals
    // the historical `desk.y + 8` (6 + 2). Locks the relationship so a
    // footprint or overhang edit surfaces here, not as a layering bug.
    let fp_h = crate::layout::desk_furniture_def()
        .footprint
        .expect("desk has a footprint")
        .h;
    assert_eq!(fp_h + DESK_FRONT_OVERHANG, 8, "desk z-key offset (was +8)");
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
    // land BELOW the desk's bottom row (desk.y + 8) must sort AFTER
    // the desk and therefore paint on top.
    let desk_y: u16 = 20;
    let desk_anchor_y = desk_y + 8;
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
    let desk_anchor_y = desk_y + 8;
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

#[test]
fn sunset_strength_varies_across_day() {
    let mut strengths = Vec::new();
    let base = SystemTime::UNIX_EPOCH;
    for hour in 0..24u64 {
        strengths.push(background::sunset_strength(
            base + std::time::Duration::from_secs(hour * 3600),
        ));
    }
    let has_zero = strengths.iter().any(|s| *s < 0.05);
    let has_nonzero = strengths.iter().any(|s| *s > 0.1);
    assert!(has_zero, "sunset should be ~0 at some hours");
    assert!(has_nonzero, "sunset should be >0 at dawn/dusk hours");
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

// --- tool_glow_tint token arms ----------------------------------------

#[test]
fn tool_glow_tint_maps_delegation_search_and_unknown_tokens() {
    let id = pixtuoid_core::AgentId::from_transcript_path("/g.jsonl");
    let glow = &crate::theme::NORMAL.tool_glow;
    let active = |detail: &str| {
        make_slot(
            id,
            ActivityState::Active {
                tool_use_id: None,
                detail: Some(Arc::from(detail)),
            },
        )
    };
    // Agent / Task → glow.agent.
    assert_eq!(
        palette::tool_glow_tint(&active("Agent code-reviewer"), glow),
        Some(glow.agent)
    );
    assert_eq!(
        palette::tool_glow_tint(&active("Task: do X"), glow),
        Some(glow.agent)
    );
    // Grep / Glob → glow.grep.
    assert_eq!(
        palette::tool_glow_tint(&active("Grep pattern"), glow),
        Some(glow.grep)
    );
    assert_eq!(
        palette::tool_glow_tint(&active("Glob **/*.rs"), glow),
        Some(glow.grep)
    );
    // Unknown token → glow.default.
    assert_eq!(
        palette::tool_glow_tint(&active("WebFetch https://x"), glow),
        Some(glow.default)
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
    assert_eq!(out.width, 2);
    assert_eq!(out.height, 1);
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

// --- paint_character_at defensive missing-anim early return -----------

#[test]
fn paint_character_at_missing_anim_is_a_noop() {
    let pack = crate::embedded_pack::load_sprite_pack(None).expect("embedded pack");
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
    );
    for y in 0..buf.height {
        for x in 0..buf.width {
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
        for y in 0..buf.height {
            for x in 0..buf.width {
                assert_eq!(buf.get(x, y), bg, "too-small bounds must paint nothing");
            }
        }
    };
    assert_noop(&|b| paint_notice_board(b, small, theme));
    assert_noop(&|b| paint_doormat(b, small, theme));
    assert_noop(&|b| paint_water_cooler(b, small, theme));
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
    assert_paints(&|b| paint_water_cooler(b, big, theme));
    assert_paints(&|b| paint_trash_bin(b, big));
}

#[test]
fn furniture_corner_clip_does_not_panic() {
    use super::furniture::{paint_area_rug, paint_pantry_table, paint_side_table};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    // Centre each piece near the (0,0) corner so part of the sprite has a
    // negative px/py, exercising the `< 0` / out-of-range `continue` clamps.
    let mut buf = RgbBuffer::filled(40, 40, Rgb { r: 0, g: 0, b: 0 });
    paint_area_rug(&mut buf, 1, 1, 10, 8, theme);
    paint_side_table(&mut buf, 1, 1, theme);
    paint_pantry_table(&mut buf, 1, 1, theme);
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
