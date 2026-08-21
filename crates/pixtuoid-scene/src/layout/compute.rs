//! Layout computation helpers for `SceneLayout`.

use super::decor::{DESK_GROUND_H, DESK_GROUND_W};
use super::mask;
use super::*;

/// Counter width that marks the LARGE (detailed kitchen) pantry sprite;
/// consumers test `>= PANTRY_COUNTER_LARGE_W` rather than the bare literal.
pub const PANTRY_COUNTER_LARGE_W: u16 = 32;

/// Horizontal seat offsets for a 3-across sofa, relative to the middle-seat
/// anchor — shared by the lounge couch and the meeting sofas.
const SEAT_DX: [i16; 3] = [-6, 0, 6];

/// A band this wide has room for flanking greenery (the lounge pot's west edge
/// needs 58 by derivation; +2 breathing).
pub(super) const ROOMY_BAND_MIN_W: u16 = 60;

/// Air kept between a scatter plant's sprite box and any obstacle waypoint's
/// visual box — 1px apart in the same column reads as one totem.
pub(super) const PLANT_OBSTACLE_CLEARANCE_PX: u16 = 3;

/// Gap kept between the fish tank's east edge and the elevator door column so
/// the spawn threshold never routes around furniture.
pub(super) const FISH_TANK_ELEVATOR_CLEARANCE: u16 = 2;

fn couch_pos(cubicle_band: &Bounds, top_margin: u16, west_clear_x: u16) -> Point {
    // `west_clear_x` is the divider wall's east edge — the westmost seat's ground must
    // stay east of it (== band start with no wall, so the clamp is a no-op).
    let couch_west_reach =
        (-SEAT_DX[0]) as u16 + furniture_def(Furniture::Couch).footprint.map_or(0, |f| f.w) / 2;
    Point {
        x: (cubicle_band.x + pct(cubicle_band.width, 35)).max(west_clear_x + couch_west_reach),
        y: top_margin + 3,
    }
}

/// The band width that seats ONE desk: the two terms `compute_pod_desks`' x-clamp
/// compares — the first pod's aisle half, plus the desk's blocked GROUND width (side
/// cabinets included, so NOT `DESK_W`).
pub(super) const DESK_BAND_MIN_W: u16 = INTER_POD_AISLE_X / 2 + DESK_GROUND_W;

/// The Y twin. The y-clamp adds a third term, `couch_to_desk_extra`, which is 0
/// everywhere below `COUCH_GAP_GROWTH_BASE_H` — pinned by the `const` assert there.
pub(super) const DESK_BAND_MIN_H: u16 = INTER_POD_AISLE_Y / 2 + DESK_GROUND_H;

/// The 1px column between the left rooms and the cubicle band — `right_x` steps
/// over it and `band_w` subtracts it, so it is one const, not two literals.
const MID_DIVIDER_W: u16 = 1;

/// The cubicle band's WIDTH for a buffer — everything east of the left column and its
/// divider. THE formula `compute_with_seed` and `min_layout_w` both step from.
pub(super) const fn band_w(buf_w: u16, mid_x_pct: u16) -> u16 {
    buf_w.saturating_sub(pct(buf_w, mid_x_pct) + MID_DIVIDER_W)
}

/// The band's HEIGHT — everything below the wall band, less the appliance aisle. THE
/// formula `compute_with_seed` and `min_layout_h` both step from, and it SATURATES: a
/// sub-`top_margin` buffer collapses to 0 rather than wrapping to 58,974 and an office
/// of garbage desks. Only observable below the size gate, so no test can hold it — the
/// gate and the use site together are what keep it unobservable.
pub(super) const fn band_h(buf_h: u16) -> u16 {
    let usable = buf_h.saturating_sub(top_margin(buf_h));
    usable.saturating_sub(cubicle_aisle_h(usable))
}

/// The north wall band's depth — 30% of the buffer, never under `MIN_TOP_MARGIN`.
pub(super) const fn top_margin(buf_h: u16) -> u16 {
    let pct30 = pct(buf_h, 30);
    if pct30 > MIN_TOP_MARGIN {
        pct30
    } else {
        MIN_TOP_MARGIN
    }
}

const fn cubicle_aisle_h(usable_h: u16) -> u16 {
    let tenth = usable_h / 10;
    if tenth > MIN_CUBICLE_AISLE_H {
        tenth
    } else {
        MIN_CUBICLE_AISLE_H
    }
}

/// The appliance aisle south of the pods never shrinks below this.
const MIN_CUBICLE_AISLE_H: u16 = 8;

/// The smallest buffer `compute_with_seed` lays out; below either it returns `None`
/// ("terminal too small"). BOTH axes are SOLVED against the band, not the buffer — the
/// two hand-written floors erred in OPPOSITE directions: W advertised a size that lays
/// out an office with no desk to seat anyone, and H was never re-derived and refused 15
/// buffer px of sizes that render.
pub(super) const MIN_LAYOUT_W: u16 = min_layout_w();
pub(super) const MIN_LAYOUT_H: u16 = min_layout_h();

/// The widest left column any variant takes — the band gets the rest, so this
/// variant is the one that prices the width floor for every seed.
const fn widest_mid_x_pct() -> u16 {
    let mut widest = 0;
    let mut i = 0;
    while i < FloorVariant::ALL.len() {
        let pct = FloorVariant::ALL[i].mid_x_pct();
        if pct > widest {
            widest = pct;
        }
        i += 1;
    }
    widest
}

const fn min_layout_w() -> u16 {
    let mut w = DESK_BAND_MIN_W;
    while band_w(w, widest_mid_x_pct()) < DESK_BAND_MIN_W {
        w += 1;
    }
    w
}

const fn min_layout_h() -> u16 {
    let mut h = DESK_BAND_MIN_H;
    while band_h(h) < DESK_BAND_MIN_H {
        h += 1;
    }
    h
}

/// The smallest buffer that lays out, for a painter that has to TELL the user
/// why it is not drawing an office. Buffer units — each painter owns its own
/// cell aspect, so only it can turn this into rows and columns.
pub fn min_layout_size() -> Size {
    Size {
        w: MIN_LAYOUT_W,
        h: MIN_LAYOUT_H,
    }
}

/// `DESK_BAND_MIN_H` omits the Y clamp's third term because it is 0 at the floor;
/// that holds only while the growth base stays above it.
const _: () = assert!(COUCH_GAP_GROWTH_BASE_H > MIN_LAYOUT_H);

const fn couch_to_desk_extra(buf_h: u16) -> u16 {
    buf_h.saturating_sub(COUCH_GAP_GROWTH_BASE_H) / 20
}

/// Buffer height at which the couch-to-desk gap starts growing. It equalled the
/// old hand-written `MIN_LAYOUT_H` by coincidence, not by derivation — re-tying it
/// to the now-lower floor would widen the gap on most offices tall enough to render
/// today, so it keeps its own name and its own number.
const COUCH_GAP_GROWTH_BASE_H: u16 = 60;

/// A meeting room narrower than this can't host the sofa body with enough
/// walkable margin for the coarse 4×4 router to reach the seats buried in it —
/// find_path returns None and an idle agent sent there TELEPORTS. Below it the
/// room degrades to bare floor.
const MEETING_FURNITURE_MIN_W: u16 = 30;

/// Whether a meeting room's bounds can host its sofa/table trio — wide enough
/// for the sofa body + router margin AND tall enough for the trio.
fn room_fits_furniture(mr: &Bounds) -> bool {
    mr.width >= MEETING_FURNITURE_MIN_W && mr.height >= MeetingRoom::trio_fit_h()
}

pub(super) fn compute_with_seed(
    buf_w: u16,
    buf_h: u16,
    max_desks: Option<usize>,
    floor_seed: u64,
) -> Option<SceneLayout> {
    if buf_w < MIN_LAYOUT_W || buf_h < MIN_LAYOUT_H {
        return None;
    }

    let top_margin = top_margin(buf_h);
    let usable_h = buf_h - top_margin;

    let variant = FloorVariant::from_seed(floor_seed);
    let has_meeting = variant.has_meeting();
    let has_dual_meeting = variant == FloorVariant::Dense && usable_h >= MIN_DUAL_MEETING_H;
    let geom = FloorGeometry {
        variant,
        has_dual_meeting,
    };
    let has_pantry = geom.has_pantry();
    let mid_x = pct(buf_w, geom.mid_x_pct());

    // Large counter + a 2-px routing margin each side, else the compact fallback.
    // Width-only, so the size is known before the split prices the pantry against it.
    let pantry_counter_size: Size = if has_pantry && mid_x >= PANTRY_COUNTER_LARGE_W + 4 {
        Size {
            w: PANTRY_COUNTER_LARGE_W,
            h: 10,
        }
    } else {
        super::rooms::pantry::COMPACT_COUNTER
    };

    // CONTENT-FIT, donating the surplus below ALL-OR-NOTHING: a partial donation would
    // cram the trio to its fit gate to buy rows the island still couldn't use.
    let trio_fit_h = MeetingRoom::trio_fit_h();
    let pantry_content_h = PantryRoom::content_fit_h(pantry_counter_size);
    let half_split = usable_h / 2;
    let donated = usable_h.saturating_sub(pantry_content_h);
    let meeting_h = if (trio_fit_h..half_split).contains(&donated) {
        donated
    } else {
        half_split
    };
    let mid_y_split = if has_meeting && !has_dual_meeting {
        top_margin + meeting_h
    } else {
        top_margin + half_split
    };

    let meeting_room = if has_meeting {
        debug_assert!(
            has_pantry || has_dual_meeting,
            "meeting implies pantry-or-dual per the variant table"
        );
        Some(Bounds {
            x: 0,
            y: top_margin,
            width: mid_x,
            height: mid_y_split - top_margin,
        })
    } else {
        None
    };
    let meeting_room_2 = if has_dual_meeting {
        Some(Bounds {
            x: 0,
            y: mid_y_split,
            width: mid_x,
            height: usable_h - usable_h / 2,
        })
    } else {
        None
    };
    let pantry_room = if has_pantry {
        Some(Bounds {
            x: 0,
            y: if has_meeting { mid_y_split } else { top_margin },
            width: mid_x,
            height: if has_meeting {
                usable_h - (mid_y_split - top_margin)
            } else {
                usable_h
            },
        })
    } else {
        None
    };

    let right_x = mid_x + MID_DIVIDER_W;
    let right_w = band_w(buf_w, geom.mid_x_pct());
    // The west bound lounge furniture must clear: the divider wall's east edge, or the
    // band start when no meeting room encloses one.
    let lounge_west_clear = if has_meeting {
        mid_x + super::WALL_THICK_V
    } else {
        right_x
    };
    let cubicle_aisle_h = cubicle_aisle_h(usable_h);
    // NOT `usable_h - cubicle_aisle_h`, though the locals are right here — see
    // `band_h` for what its saturation buys below the gate.
    let cubicle_h = band_h(buf_h);
    let cubicle_band = Bounds {
        x: right_x,
        y: top_margin,
        width: right_w,
        height: cubicle_h,
    };
    let cubicle_aisle = Bounds {
        x: right_x,
        y: top_margin + cubicle_h,
        width: right_w,
        height: cubicle_aisle_h,
    };

    let pod_w = POD_SIDE * DESK_W + (POD_SIDE - 1) * INTRA_POD_GAP_X;
    let pod_h = POD_SIDE * DESK_H + (POD_SIDE - 1) * INTRA_POD_GAP_Y;
    let pod_stride_x = pod_w + INTER_POD_AISLE_X;
    let pod_stride_y = pod_h + INTER_POD_AISLE_Y;
    let couch_to_desk_extra = couch_to_desk_extra(buf_h);
    let pod_cols = ((right_w.saturating_sub(INTER_POD_AISLE_X / 2)) / pod_stride_x).max(1);
    let pod_rows =
        ((cubicle_h.saturating_sub(couch_to_desk_extra) + INTER_POD_AISLE_Y) / pod_stride_y).max(1);
    let pod_grid = PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        stride_x: pod_stride_x,
        stride_y: pod_stride_y,
        couch_to_desk_extra,
    };

    let (home_desks, desk_facings) = compute_pod_desks(max_desks, &cubicle_band, pod_grid);

    let pod_decor = compute_pod_decor(&cubicle_band, pod_grid, floor_seed);

    // Vec index IS the room_id: a room too small for its trio still occupies its slot
    // with `trio: None`, so bounds and furniture can't mis-join. Room 0 is the apron room.
    let mut meeting_rooms: Vec<MeetingRoom> = Vec::new();
    for (room_idx, room) in [meeting_room, meeting_room_2].into_iter().enumerate() {
        let Some(mr) = room else { continue };
        let trio = room_fits_furniture(&mr).then(|| MeetingRoom::place_trio(mr, room_idx != 0));
        meeting_rooms.push(MeetingRoom { bounds: mr, trio });
    }

    // Dense's inter-meeting wall is deliberately solid (#557 door policy).
    let (room_walls, doorways) =
        super::rooms::walls::derive_room_walls(&meeting_rooms, pantry_room);

    // Elevator door — mounted in the back wall's rightmost window position, BOTTOM-aligned
    // with the windows; above the lounge gate so that gate can check couch↔door clearance.
    let top_wall_h = top_margin.saturating_sub(super::WALL_BAND_TO_TOP_MARGIN);
    let window_bottom_y = top_wall_h.saturating_sub(3); // matches paint_floor_and_walls' window_h
    let door = if buf_w >= ELEVATOR_W + 4 && window_bottom_y + 1 >= ELEVATOR_H {
        Some(Point {
            x: buf_w.saturating_sub(ELEVATOR_W + 2),
            // +2 drops the elevator bottom 2 px below the window line, resting it on
            // the floor instead of floating mid-wall.
            y: window_bottom_y + 1 - ELEVATOR_H + 2,
        })
    } else {
        None
    };
    /// How far SOUTH of the floor line the elevator spawn sits, so a character entering
    /// stands on open floor, not on the wall apron the straddling wall decor stamps into.
    const DOOR_THRESHOLD_CLEARANCE_PX: u16 = 4;
    let door_threshold = door.map(|d| Point {
        x: d.x + ELEVATOR_W / 2,
        y: top_margin + DOOR_THRESHOLD_CLEARANCE_PX,
    });

    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(&cubicle_band, top_margin, lounge_west_clear);
    // Below this WEST-side fit the whole vignette degrades away; 30 = the vignette's
    // blocked span + OBSTACLE_PAD_PX each side + walk clearance.
    const LOUNGE_MIN_BAND_W: u16 = 30;
    // EAST-side twin (#566): the east seat's padded ground must stay at-or-west of the
    // threshold column. WAYPOINT_STAMP_PAD_PX is the SEAT stamp's pad, NOT OBSTACLE_PAD_PX.
    let couch_east_ground = couch_x
        + SEAT_DX[SEAT_DX.len() - 1] as u16
        + furniture_def(Furniture::Couch).footprint.map_or(0, |f| f.w) / 2
        + WAYPOINT_STAMP_PAD_PX;
    let couch_clears_door = door_threshold.is_none_or(|dt| couch_east_ground <= dt.x);
    let lounge_fits = cubicle_band.width >= LOUNGE_MIN_BAND_W && couch_clears_door;

    let (mut waypoints, couch_sprite_center) = compute_waypoints(
        &cubicle_band,
        top_margin,
        pantry_room,
        pantry_counter_size,
        &pod_decor,
        &cubicle_aisle,
        &meeting_rooms,
        lounge_fits,
        lounge_west_clear,
    );

    // NOT the pantry (a plant + pad blocks the only bridge to the cubicle area), NOT the
    // cubicle top strip (a 7-px wall-to-couch gap), NOT a meeting interior (seals the door).
    let mut plant_candidates: Vec<PlantItem> = vec![
        PlantItem {
            kind: PlantKind::Flower,
            pos: Point {
                x: cubicle_band.x + 4,
                y: cubicle_aisle.y.saturating_sub(4),
            },
        },
        PlantItem {
            kind: PlantKind::Succulent,
            pos: Point {
                x: cubicle_band.x + cubicle_band.width.saturating_sub(4),
                y: cubicle_aisle.y.saturating_sub(4),
            },
        },
    ]
    .into_iter()
    // West wall only — clear of the east-wall door and the central sofa/table column;
    // the size gate keeps plant + pad from squeezing the strip below routable width.
    .chain(meeting_room.into_iter().flat_map(|mr| {
        if mr.width < 30 || mr.height < 30 {
            Vec::new()
        } else {
            vec![
                PlantItem {
                    kind: PlantKind::Tall,
                    pos: Point {
                        x: mr.x + 5,
                        y: mr.y + 6,
                    },
                },
                PlantItem {
                    kind: PlantKind::Flower,
                    pos: Point {
                        x: mr.x + 5,
                        y: mr.y + mr.height.saturating_sub(7),
                    },
                },
            ]
        }
    }))
    .collect();

    // AFTER `door`: the tank prices its east limit against the elevator column.
    let LoungeVignette {
        floor_lamp,
        side_table: lounge_side_table,
        fish_tank,
    } = place_lounge_vignette(
        couch_x,
        couch_y,
        lounge_west_clear,
        buf_w,
        door,
        lounge_fits,
    );

    // Two Ficus spots — greeting plant west of the elevator, and the lounge's west flank.
    // On a narrower band each seals a top-strip pocket, hence the ROOMY gate.
    if cubicle_band.width >= ROOMY_BAND_MIN_W {
        if let Some(d) = door {
            plant_candidates.push(PlantItem {
                kind: PlantKind::Ficus,
                pos: Point {
                    x: d.x.saturating_sub(5),
                    y: top_margin + 5,
                },
            });
        }
        if lounge_fits {
            // Ground is centred on `pos`, so keep its west edge east of the divider.
            let ficus_half_w = furniture_def(PlantKind::Ficus.furniture())
                .footprint
                .map_or(0, |f| f.w)
                / 2;
            plant_candidates.push(PlantItem {
                kind: PlantKind::Ficus,
                pos: Point {
                    x: couch_x
                        .saturating_sub(17)
                        .max(lounge_west_clear + ficus_half_w),
                    y: couch_y,
                },
            });
        }
    }

    let mut wall_decor = place_wall_decor(
        buf_w,
        top_margin,
        usable_h,
        mid_x,
        meeting_rooms.first(),
        door,
        has_meeting || has_pantry,
        &cubicle_band,
        pod_grid,
    );

    // The island pushes its 4 slots BEFORE the shelf's — the push order the goldens pin.
    let kitchen_island = pantry_room.and_then(|pr| {
        super::rooms::pantry::place_kitchen_island(pr, pantry_counter_size, &mut waypoints)
    });
    if let Some(pr) = pantry_room {
        super::rooms::pantry::place_snack_shelf(pr, pantry_counter_size, &mut waypoints);
    }

    let corridor = Some(Bounds {
        x: 0,
        y: cubicle_aisle.y,
        width: buf_w,
        height: cubicle_aisle.height,
    });

    // Settle only now — AFTER every waypoint exists; filtering at the candidate site
    // checked a subset of the final set.
    let singleton_rects = plant_obstacle_rects(
        fish_tank,
        floor_lamp,
        lounge_side_table,
        kitchen_island,
        &meeting_rooms,
    );
    // Folded, not mapped: the corridor's two pots slide toward each other, so each must
    // also clear the ones already placed.
    let mut plants: Vec<PlantItem> = Vec::new();
    for p in plant_candidates {
        let mut obstacles = singleton_rects.clone();
        obstacles.extend(plants.iter().map(|q| {
            let v = furniture_def(q.kind.furniture()).visual;
            (anchored_top_left(Anchor::Center, q.pos, v.w, v.h), v)
        }));
        if let Some(settled) = settle_plant(
            p,
            &home_desks,
            &waypoints,
            &obstacles,
            &cubicle_band,
            floor_seed,
        ) {
            plants.push(settled);
        }
    }

    let build_mask = |plants: &[PlantItem], wall_decor: &[WallDecorItem]| {
        mask::build_walkable_mask(&mask::MaskObstacles {
            buf_w,
            buf_h,
            top_margin,
            home_desks: &home_desks,
            meeting_rooms: &meeting_rooms,
            kitchen_island,
            waypoints: &waypoints,
            plants,
            floor_lamp,
            lounge_side_table,
            fish_tank,
            wall_decor,
            pod_decor: &pod_decor,
            room_walls: &room_walls,
            pantry_counter_size,
        })
    };
    // The door, where agents enter, so always in the main component.
    let conn_seed = door_threshold
        .or_else(|| home_desks.first().copied())
        .unwrap_or(Point {
            x: buf_w / 2,
            y: buf_h / 2,
        });

    // ROUTER granularity, not just the pixel flood's — a ≤3 px channel is
    // pixel-connected and coarse-IMPASSABLE (scene CLAUDE.md, #566).
    let severed = |mask: &WalkableMask| -> bool {
        if !unreachable_walkable_cells(mask, conn_seed).is_empty() {
            return true;
        }
        let reach = ReachSet::from_mask(mask, conn_seed);
        // South is where the demotion pass below retreats, so a reachable south seat
        // proves no decor arrangement strands a desk.
        home_desks.iter().any(|&d| {
            let chair = desk_walk_anchor_facing(d, crate::layout::Facing::South);
            approach_point(
                Furniture::Desk,
                chair,
                Facing::South,
                pantry_counter_size,
                mask,
                chair,
                &reach,
            ) == chair
        })
    };

    let mut walkable = build_mask(&plants, &wall_decor);
    // Connectivity guard (#566): a decorative plant may NEVER disconnect the office. The
    // flood runs on EVERY compute — the check IS the guard, a net for ANY sealing decor.
    if severed(&walkable) {
        // The pocket cells sit ACROSS the drain from the seal-causing plant, so target
        // by "settled into the aisle", not "borders the pocket".
        plants.retain(|p| !plant_ground_in_bounds(p, &cubicle_aisle));
        walkable = build_mask(&plants, &wall_decor);
        // Next rung — only a wall decor that TOUCHES THE FLOOR can seal a lane, so drop
        // those (by footprint, not by kind) before the drastic clear-all-plants.
        if severed(&walkable) {
            wall_decor.retain(|d| furniture_def(d.kind.furniture()).footprint.is_none());
            walkable = build_mask(&plants, &wall_decor);
        }
        if severed(&walkable) {
            plants.clear();
            walkable = build_mask(&plants, &wall_decor);
        }
        debug_assert!(
            !severed(&walkable),
            "#566 connectivity guard: a pocket (or a coarse-unroutable home desk) survived \
             dropping every scatter plant AND the free-standing whiteboard — a new NON-decor \
             seal cause needs its own fix"
        );
    }

    // ReachSet's seed snap pulls a blocked seed into the adjacent component.
    let reachable = ReachSet::from_mask(&walkable, conn_seed);

    // Couch + lamp + side table are Some exactly iff `lounge_fits`, so the zip is None
    // precisely when the vignette doesn't fit; the aquarium keeps its own east gate.
    let lounge = couch_sprite_center
        .zip(floor_lamp)
        .zip(lounge_side_table)
        .map(|((couch_center, floor_lamp), side_table)| Lounge {
            couch_center,
            floor_lamp,
            side_table,
            fish_tank,
        });

    // A narrow band can wall off a back-turned desk's SOUTH front — demote, don't drop.
    // A NET, not live code — see `SHARP-EDGES.md`.
    let desk_facings: Vec<Facing> = home_desks
        .iter()
        .zip(&desk_facings)
        .map(|(&desk, &facing)| {
            if facing == Facing::North && {
                // `approach_point` returns the probed cell itself as its "no side" sentinel.
                let chair = desk_walk_anchor_facing(desk, facing);
                approach_point(
                    Furniture::Desk,
                    chair,
                    facing,
                    pantry_counter_size,
                    &walkable,
                    chair,
                    &reachable,
                ) == chair
            } {
                Facing::South
            } else {
                facing
            }
        })
        .collect();

    Some(SceneLayout {
        buf_w,
        buf_h,
        cubicle_band,
        cubicle_aisle,
        desk_facings,
        home_desks,
        waypoints,
        plants,
        wall_decor,
        pod_decor,
        lounge,
        door,
        door_threshold,
        meeting_rooms,
        pantry: pantry_room.map(|bounds| PantryRoom {
            bounds,
            counter_size: pantry_counter_size,
            kitchen_island,
        }),
        room_walls,
        doorways,
        top_margin,
        corridor,
        walkable,
        reachable,
    })
}

/// The four wall-band decorations, each TOP-LEFT-anchored so its bottom row lands on
/// the last wall-band row however tall the band grows. The screen hugs room 0's WEST
/// corner and the bookshelf spreads EAST — LOAD-BEARING, not taste: the carpet apron
/// between the two grounds must drain south AROUND the tucked sofa. Any item whose
/// clamped slot would pierce the divider/exit sign/elevator drops, reopening the lane.
#[allow(clippy::too_many_arguments)]
fn place_wall_decor(
    buf_w: u16,
    top_margin: u16,
    usable_h: u16,
    mid_x: u16,
    meeting_room: Option<&MeetingRoom>,
    door: Option<Point>,
    has_side_rooms: bool,
    cubicle_band: &Bounds,
    pod_grid: PodGrid,
) -> Vec<WallDecorItem> {
    let bookshelf_w = furniture_def(WallDecor::Bookshelf.furniture()).visual.w;
    let screen_w = furniture_def(WallDecor::MeetingScreen.furniture()).visual.w;
    // A room narrower than the screen would hang it ACROSS the east wall — dropping it
    // is the same degradation as the bare meeting room.
    let meeting_screen_x = meeting_room.and_then(|mr| {
        let sx = mr.bounds.x + 1;
        (sx + screen_w < mr.bounds.x + mr.bounds.width).then_some(sx)
    });
    let bookshelf_x = bookshelf_x(buf_w, screen_w, bookshelf_w, meeting_screen_x, meeting_room);
    let exit_sign_x = buf_w.saturating_sub(9);
    let wall_east_limit = exit_sign_x.min(door.map(|d| d.x).unwrap_or(u16::MAX));
    // WEST of the vertical divider too: on narrow trio rooms the drain clamp can push it
    // onto the wall's top segment, where it pierces the glass. Dropping reopens the apron.
    let bookshelf_east_limit = meeting_room
        .map_or(u16::MAX, |mr| mr.bounds.x + mr.bounds.width)
        .min(wall_east_limit);
    let mut wall_decor = Vec::new();
    if bookshelf_x + bookshelf_w < bookshelf_east_limit {
        wall_decor.push(WallDecorItem {
            kind: WallDecor::Bookshelf,
            pos: Point {
                x: bookshelf_x,
                y: top_margin.saturating_sub(12),
            },
        });
    }
    wall_decor.push(WallDecorItem {
        kind: WallDecor::ExitSign,
        pos: Point {
            x: exit_sign_x,
            y: top_margin.saturating_sub(13),
        },
    });
    if has_side_rooms {
        // `usable_h / 3` is a hint, not a slot: unsnapped it drops the board on a desk row
        // or in the intra-pod gap, where the wheel strip plugs the pod's own west lane.
        let wb_def = furniture_def(WallDecor::Whiteboard.furniture());
        let hint = Point {
            x: mid_x + 3,
            y: top_margin + usable_h / 3,
        };
        let snapped = wb_def
            .ground_rect(Anchor::TopLeft, hint)
            .and_then(|(ground, size)| {
                let y = pod_grid.snap_inter_pod_ground_y(cubicle_band, ground.y, size.h)?;
                Some(Point {
                    x: hint.x,
                    y: y.saturating_sub(ground.y - hint.y),
                })
            });
        if let Some(pos) = snapped {
            wall_decor.push(WallDecorItem {
                kind: WallDecor::Whiteboard,
                pos,
            });
        }
    }
    if let (Some(_), Some(sx)) = (meeting_room, meeting_screen_x) {
        wall_decor.push(WallDecorItem {
            kind: WallDecor::MeetingScreen,
            pos: Point {
                x: sx,
                y: top_margin.saturating_sub(12),
            },
        });
    }
    wall_decor
}

/// The bookshelf's wall slot — flush east of the meeting screen, spread further past
/// the sofa's drain edge, or back to flush when that spread would pierce the divider.
fn bookshelf_x(
    buf_w: u16,
    screen_w: u16,
    bookshelf_w: u16,
    meeting_screen_x: Option<u16>,
    meeting_room: Option<&MeetingRoom>,
) -> u16 {
    let x = pct(buf_w, 18);
    match (meeting_screen_x, meeting_room) {
        (Some(sx), Some(mr)) => {
            // The ONE flush slot: screen east edge + a 2-px gap, so the two grounds'
            // pads merge with no strandable apron cell. Every arm below derives from it.
            let flush_east = sx + screen_w + 2;
            // The drain term applies only where room 0 HOSTS its trio: with no sofa,
            // pushing the shelf east hangs it over a desk pod's pad, sealing the gap.
            if let Some(sofa_pad_east) = mr.sofa_east_drain_edge() {
                /// Past the sofa's drain edge by the shelf's OWN 1-px ground pad
                /// (mask.rs stamps wall decor with pad=1, NOT `OBSTACLE_PAD_PX`) + a
                /// ≥2-px walkable channel + slack.
                const BOOKSHELF_DRAIN_GAP: u16 = 5;
                let spread = x.max(flush_east).max(sofa_pad_east + BOOKSHELF_DRAIN_GAP);
                if spread + bookshelf_w < mr.bounds.x + mr.bounds.width {
                    spread
                } else {
                    // Falls back to FLUSH, NOT the pct-18 anchor, which at these widths
                    // opens a strandable gap OVER the sofa pad.
                    flush_east
                }
            } else {
                x.max(flush_east)
            }
        }
        _ => x,
    }
}

/// The lounge vignette singletons, all anchored to the viewing couch and gated
/// as ONE cluster on `lounge_fits`.
struct LoungeVignette {
    floor_lamp: Option<Point>,
    side_table: Option<Point>,
    fish_tank: Option<Point>,
}

/// The lounge vignette around the viewing couch — floor lamp, side table, aquarium.
/// The lamp sits just east so its halo bathes the seating area at night; the side table
/// takes the OPPOSITE (west) flank, clamped clear of the room-divider column. The
/// aquarium carries an EXTRA gate the other two don't: it must stay clear of the
/// elevator `door` column so the spawn threshold never routes around it.
fn place_lounge_vignette(
    couch_x: u16,
    couch_y: u16,
    west_clear_x: u16,
    buf_w: u16,
    door: Option<Point>,
    lounge_fits: bool,
) -> LoungeVignette {
    let floor_lamp = lounge_fits.then_some(Point {
        x: couch_x + 9,
        y: couch_y + 2,
    });
    let side_half_w = furniture_def(Furniture::LoungeSideTable)
        .footprint
        .map_or(0, |s| s.w / 2);
    // The west edge must clear `west_clear_x`, else at the minimum band width
    // `couch_x − 10` drops the table onto the wall.
    let side_table = lounge_fits.then_some(Point {
        x: couch_x.saturating_sub(10).max(west_clear_x + side_half_w),
        y: couch_y + 2,
    });
    let fish_tank = floor_lamp.and_then(|lamp| {
        let def = furniture_def(Furniture::FishTank);
        let half_w = def.visual.w / 2;
        // One clear floor column of breathing room past the lamp shade's east edge; a
        // center-pinned east edge is (w-1)/2 past the anchor.
        const LAMP_TANK_GAP: u16 = 2;
        let lamp_east = lamp.x + (furniture_def(Furniture::FloorLamp).visual.w - 1) / 2;
        let cx = lamp_east + LAMP_TANK_GAP + half_w;
        let east_limit = door.map_or(buf_w.saturating_sub(2), |d| d.x);
        (cx + half_w + FISH_TANK_ELEVATOR_CLEARANCE <= east_limit).then_some(Point {
            x: cx,
            y: couch_y.saturating_sub(4),
        })
    });
    LoungeVignette {
        floor_lamp,
        side_table,
        fish_tank,
    }
}

/// THE non-waypoint obstacle census a scatter plant must clear — one derivation shared
/// by the production settle path and the placement-sweep backstop, so the two can't
/// drift. EVERY singleton is passed EXPLICITLY (omitting one shipped interpenetration
/// bugs) and included IFF its kind [`repels_plants`]; waypoint obstacles are
/// `first_blocking_waypoint`'s job.
pub(super) fn plant_obstacle_rects(
    fish_tank: Option<Point>,
    floor_lamp: Option<Point>,
    lounge_side_table: Option<Point>,
    kitchen_island: Option<Point>,
    meeting_rooms: &[MeetingRoom],
) -> Vec<(Point, Size)> {
    let boxed = |kind: Furniture, pos: Point| -> Option<(Point, Size)> {
        repels_plants(kind).then(|| {
            let v = furniture_def(kind).visual;
            (anchored_top_left(Anchor::Center, pos, v.w, v.h), v)
        })
    };
    [
        (Furniture::FishTank, fish_tank),
        (Furniture::FloorLamp, floor_lamp),
        (Furniture::LoungeSideTable, lounge_side_table),
        (Furniture::KitchenIsland, kitchen_island),
    ]
    .into_iter()
    .filter_map(|(kind, pos)| pos.and_then(|p| boxed(kind, p)))
    .chain(meeting_rooms.iter().flat_map(|room| {
        room.trio.iter().flat_map(|trio| {
            [
                (Furniture::MeetingSofaBody, trio.sofas[0]),
                (Furniture::MeetingSofaBody, trio.sofas[1]),
                (Furniture::MeetingTable, trio.table),
            ]
            .into_iter()
            .filter_map(|(kind, pos)| boxed(kind, pos))
        })
    }))
    .collect()
}

/// Settle a scatter-plant candidate: keep its authored spot when clear, else slide 1px
/// at a time toward the band's horizontal centre (bounded) until the desk-ground and
/// clearance rules pass. SLIDING, not deleting — yield-by-deletion stripped the greenery.
fn settle_plant(
    p: PlantItem,
    home_desks: &[Point],
    waypoints: &[Waypoint],
    singletons: &[(Point, Size)],
    band: &Bounds,
    floor_seed: u64,
) -> Option<PlantItem> {
    // 12 = two appliance widths: clears any single corner appliance without wandering
    // out of the authored corner region.
    const MAX_PLANT_NUDGE_PX: u16 = 12;
    /// How far into the nudge budget the seeded first try may reach — the budget and its
    /// inward direction are the ladder's own, so no new clearance rule and no new container.
    const PLANT_SCATTER_PX: u16 = 4;
    // The sharp edge's claim — a scattered pot only stands where a DISPLACED one
    // already could — holds only while the scatter fits inside the dodge budget.
    const _: () = assert!(PLANT_SCATTER_PX <= MAX_PLANT_NUDGE_PX);
    let dir: i16 = if p.pos.x < band.x + band.width / 2 {
        1
    } else {
        -1
    };
    let clear = |cand: Point| plant_spot_clear(p.kind, cand, home_desks, waypoints, singletons);
    let slide = |step: u16| Point {
        x: p.pos.x.saturating_add_signed(dir * step as i16),
        y: p.pos.y,
    };
    let seeded = pixtuoid_core::id::splitmix64(floor_seed ^ super::decor::point_seed(p.pos))
        % (u64::from(PLANT_SCATTER_PX) + 1);
    let first = slide(seeded as u16);
    if clear(first) {
        return Some(PlantItem {
            kind: p.kind,
            pos: first,
        });
    }
    // Beside the blocking obstacle on ITS row: the plant's own row is desk-saturated on
    // packed floors, so the corridor floor beside it is the one desk-free spot.
    let pv = furniture_def(p.kind.furniture()).visual;
    if let Some(w) = first_blocking_waypoint(p.kind, p.pos, waypoints) {
        let wdef = furniture_def(w.kind.furniture());
        let m = PLANT_OBSTACLE_CLEARANCE_PX;
        // Derive the spot from the inflated box's EDGES: width-sum arithmetic
        // truncates w/2 on odd widths and landed 1px inside the box.
        let infl_left = w.pos.x.saturating_sub(wdef.visual.w / 2 + m);
        let infl_right = infl_left + wdef.visual.w + 2 * m;
        let cand_x = if dir < 0 {
            infl_left.saturating_sub(pv.w - pv.w / 2)
        } else {
            infl_right + pv.w / 2
        };
        let cand = Point {
            x: cand_x,
            y: w.pos.y,
        };
        if clear(cand) {
            return Some(PlantItem {
                kind: p.kind,
                pos: cand,
            });
        }
    }
    (0..=MAX_PLANT_NUDGE_PX).find_map(|step| {
        let cand = slide(step);
        clear(cand).then_some(PlantItem {
            kind: p.kind,
            pos: cand,
        })
    })
}

/// The first obstacle waypoint whose clearance box the plant's authored spot
/// violates — the thing `settle_plant` steps around.
fn first_blocking_waypoint(
    kind: PlantKind,
    pos: Point,
    waypoints: &[Waypoint],
) -> Option<&Waypoint> {
    let pv = furniture_def(kind.furniture()).visual;
    let plant_tl = anchored_top_left(Anchor::Center, pos, pv.w, pv.h);
    waypoints.iter().find(|w| {
        let wdef = furniture_def(w.kind.furniture());
        if wdef.footprint.is_none() {
            return false;
        }
        let wp_tl = anchored_top_left(Anchor::Center, w.pos, wdef.visual.w, wdef.visual.h);
        super::placement::overlaps_within_clearance(
            (plant_tl, pv),
            (wp_tl, wdef.visual),
            PLANT_OBSTACLE_CLEARANCE_PX,
        )
    })
}

/// Both placement rules for one plant spot: ground never overlaps a desk
/// ground, and the sprite box keeps PLANT_OBSTACLE_CLEARANCE_PX of air from
/// every obstacle waypoint's box.
fn plant_spot_clear(
    kind: PlantKind,
    pos: Point,
    home_desks: &[Point],
    waypoints: &[Waypoint],
    singletons: &[(Point, Size)],
) -> bool {
    let def = furniture_def(kind.furniture());
    if def
        .ground_rect(Anchor::Center, pos)
        .is_some_and(|r| overlaps_a_desk_ground(r, home_desks))
    {
        return false;
    }
    // Fixed singletons get the same inflated-clearance rule as waypoints.
    let pv = def.visual;
    let plant_tl = anchored_top_left(Anchor::Center, pos, pv.w, pv.h);
    if singletons.iter().any(|&(tl, sz)| {
        super::placement::overlaps_within_clearance(
            (plant_tl, pv),
            (tl, sz),
            PLANT_OBSTACLE_CLEARANCE_PX,
        )
    }) {
        return false;
    }
    first_blocking_waypoint(kind, pos, waypoints).is_none()
}

/// Does `r` (a blocked ground rect) overlap ANY home desk's ground? THE one
/// desk-collision scan — the whiteboard-yield and the scatter-plant-yield both
/// read it, so a future pad/anchor tweak can't land on one copy.
fn overlaps_a_desk_ground(r: (Point, Size), home_desks: &[Point]) -> bool {
    let desk = super::decor::desk_furniture_def();
    home_desks.iter().any(|&d| {
        desk.ground_rect(Anchor::TopLeft, d)
            .is_some_and(|desk_ground| super::placement::rects_overlap(r, desk_ground))
    })
}

/// Walkable cells NOT reachable from `seed` by 4-connected flood (a sealed
/// pocket). Empty when the office is one region OR when `seed` itself is blocked
/// — a bad seed can't judge connectivity, so the guard stays a no-op rather than
/// falsely condemning every cell.
pub(super) fn unreachable_walkable_cells(mask: &WalkableMask, seed: Point) -> Vec<Point> {
    let (w, h) = (mask.width(), mask.height());
    if !mask.is_walkable(seed.x, seed.y) {
        return Vec::new();
    }
    let idx = |x: u16, y: u16| y as usize * w as usize + x as usize;
    let mut seen = vec![false; w as usize * h as usize];
    let mut stack = vec![seed];
    seen[idx(seed.x, seed.y)] = true;
    while let Some(p) = stack.pop() {
        for (dx, dy) in [(0i32, -1i32), (0, 1), (-1, 0), (1, 0)] {
            let (nx, ny) = (p.x as i32 + dx, p.y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let (nx, ny) = (nx as u16, ny as u16);
            if !seen[idx(nx, ny)] && mask.is_walkable(nx, ny) {
                seen[idx(nx, ny)] = true;
                stack.push(Point { x: nx, y: ny });
            }
        }
    }
    (0..h)
        .flat_map(|y| (0..w).map(move |x| Point { x, y }))
        .filter(|p| mask.is_walkable(p.x, p.y) && !seen[idx(p.x, p.y)])
        .collect()
}

/// Does scatter plant `p`'s ground rect fall inside `b` (the cubicle aisle)? A
/// plant `settle_plant` relocated onto an obstacle's aisle row lands here and can
/// plug the drain — THE seal-causer selector for the #566 connectivity guard.
fn plant_ground_in_bounds(p: &PlantItem, b: &Bounds) -> bool {
    let def = furniture_def(p.kind.furniture());
    let Some(ground) = def.ground_rect(Anchor::Center, p.pos) else {
        return false;
    };
    super::placement::rects_overlap(
        ground,
        (
            Point { x: b.x, y: b.y },
            Size {
                w: b.width,
                h: b.height,
            },
        ),
    )
}

/// 2×2-pod grid geometry shared by [`compute_pod_desks`] + [`compute_pod_decor`].
#[derive(Clone, Copy)]
pub(super) struct PodGrid {
    cols: u16,
    rows: u16,
    stride_x: u16,
    stride_y: u16,
    couch_to_desk_extra: u16,
}

impl PodGrid {
    /// NW origin (top-left of the first desk) of pod `(pod_c, pod_r)` within the
    /// cubicle band — the single formula the desk-placement and aisle-decor
    /// passes both step from.
    fn pod_origin(self, cubicle_band: &Bounds, pod_c: u16, pod_r: u16) -> (u16, u16) {
        let x = cubicle_band.x + INTER_POD_AISLE_X / 2 + pod_c * self.stride_x;
        let y = cubicle_band.y
            + INTER_POD_AISLE_Y / 2
            + self.couch_to_desk_extra
            + pod_r * self.stride_y;
        (x, y)
    }

    /// The full-width y-bands BETWEEN consecutive pod rows, north-to-south — the only
    /// floor a free-standing piece may stand on. NOT "every pod-free strip": the north
    /// margin and south remainder hold the lounge and the door's approach.
    fn inter_pod_y_bands(self, cubicle_band: &Bounds) -> Vec<(u16, u16)> {
        // NOT `stride_y - INTER_POD_AISLE_Y`: that prices the SLOT, and a desk's blocked
        // ground runs to `DESK_GROUND_H` below its corner, so the overhang rows look free.
        let pod_h = (POD_SIDE - 1) * (DESK_H + INTRA_POD_GAP_Y) + DESK_GROUND_H;
        (0..self.rows.saturating_sub(1))
            .map(|pod_r| {
                let (_, top) = self.pod_origin(cubicle_band, 0, pod_r);
                (top + pod_h, top + self.stride_y)
            })
            .filter(|&(start, end)| end > start)
            .collect()
    }

    /// Top row for a ground strip `h` px tall, centred in the inter-pod aisle nearest
    /// `desired` — `None` when no aisle can hold it. Centred, not flush: flush against a
    /// pod the strip piles all its clearance on one side and can seal the lane.
    fn snap_inter_pod_ground_y(self, cubicle_band: &Bounds, desired: u16, h: u16) -> Option<u16> {
        let distance_to = |(start, end): (u16, u16)| (start + (end - start) / 2).abs_diff(desired);
        let (start, end) = self
            .inter_pod_y_bands(cubicle_band)
            .into_iter()
            .filter(|&(start, end)| end - start >= h)
            .min_by_key(|&band| distance_to(band))?;
        Some(start + (end - start - h) / 2)
    }
}

/// The five hand-authored floor geometries. `floor_seed` selects one via
/// Fibonacci hashing; floors past the fifth cycle through the same looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FloorVariant {
    /// Meeting + pantry, vertical wall between them and the cubicle area,
    /// horizontal wall between meeting/pantry.
    Standard,
    /// Pantry only, no vertical wall (open kitchen corner, the counter is the
    /// divider). No meeting room.
    OpenPlan,
    /// Two meeting rooms (top + bottom), no pantry — a horizontal wall separates
    /// them, each gets a door. Degrades to Standard when too short for two rooms.
    Dense,
    /// Larger meeting + pantry (like Standard but a wider left column).
    Senior,
    /// Pantry only, no vertical wall (open break area).
    Lounge,
}

impl FloorVariant {
    /// THE roster: the floor derivations sweep it, `from_seed` indexes it, `COUNT` is
    /// its length. A variant missing here is unreachable — clippy's `dead_code` reds on
    /// the never-constructed arm, NOT `the_sweep_reaches_every_floor_variant`, which
    /// catches the other direction. What NOTHING catches: `has_meeting` /
    /// `has_pantry_base` are `matches!` lists, so a variant left out silently gets neither.
    pub(super) const ALL: [Self; 5] = [
        FloorVariant::Standard,
        FloorVariant::OpenPlan,
        FloorVariant::Dense,
        FloorVariant::Senior,
        FloorVariant::Lounge,
    ];
    const COUNT: u64 = Self::ALL.len() as u64;
    /// Fibonacci-hash multiplier, chosen so the standard floor seeds each map to
    /// a distinct variant.
    const HASH_MULT: u64 = 0x4737819096da1dad;

    /// Select the variant for a floor seed (Fibonacci hashing).
    fn from_seed(floor_seed: u64) -> Self {
        Self::ALL[(floor_seed.wrapping_mul(Self::HASH_MULT) % Self::COUNT) as usize]
    }

    /// Whether this variant encloses a meeting room (== the vertical-wall presence).
    const fn has_meeting(self) -> bool {
        matches!(
            self,
            FloorVariant::Standard | FloorVariant::Dense | FloorVariant::Senior
        )
    }

    /// Pantry presence BEFORE the Dense-degrade fixup (Dense has none until it
    /// degrades on a short terminal).
    const fn has_pantry_base(self) -> bool {
        matches!(
            self,
            FloorVariant::Standard
                | FloorVariant::OpenPlan
                | FloorVariant::Senior
                | FloorVariant::Lounge
        )
    }

    /// Left-column split as a percent of buffer width, BEFORE the Dense-degrade.
    const fn mid_x_pct(self) -> u16 {
        match self {
            FloorVariant::Standard => 28,
            FloorVariant::OpenPlan => 18,
            FloorVariant::Dense => 22,
            FloorVariant::Senior => 35,
            FloorVariant::Lounge => 22,
        }
    }
}

/// The resolved floor geometry: the `variant` plus the ONE size-dependent bit,
/// `has_dual_meeting` (a Dense floor tall enough for two meeting rooms). The
/// `has_pantry` / `mid_x_pct` accessors fold in the Dense-degrade (a too-short
/// Dense floor gains a pantry and widens to the Standard column).
#[derive(Clone, Copy)]
pub(super) struct FloorGeometry {
    variant: FloorVariant,
    has_dual_meeting: bool,
}

impl FloorGeometry {
    /// Resolved pantry presence AFTER the Dense-degrade: a Dense floor too short
    /// for two rooms gains a pantry (Standard geometry).
    fn has_pantry(self) -> bool {
        if self.variant == FloorVariant::Dense && !self.has_dual_meeting {
            true
        } else {
            self.variant.has_pantry_base()
        }
    }
    /// Resolved mid-column percent AFTER the Dense-degrade — reads the Standard
    /// row rather than repeating its percent, so retuning that row can't leave a
    /// degraded Dense floor on the old column.
    fn mid_x_pct(self) -> u16 {
        if self.variant == FloorVariant::Dense && !self.has_dual_meeting {
            FloorVariant::Standard.mid_x_pct()
        } else {
            self.variant.mid_x_pct()
        }
    }
}

/// Which way a desk on pod row `r` seats its occupant — a pod's two rows face EACH
/// OTHER across the inner gap. A partial bottom row is the next pod's row 0.
fn pod_row_facing(r: u16) -> Facing {
    if r == 0 {
        Facing::South
    } else {
        Facing::North
    }
}

/// Pod-grid desk placement: full pods, partial columns at right edge,
/// partial row at bottom edge.
pub(super) fn compute_pod_desks(
    max_desks: Option<usize>,
    cubicle_band: &Bounds,
    grid: PodGrid,
) -> (Vec<Point>, Vec<Facing>) {
    let PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        ..
    } = grid;
    // The hint is bounded by physical capacity: `Vec::with_capacity(usize::MAX)` aborts.
    let n = max_desks.unwrap_or(usize::MAX);
    let grid_desk_cap =
        (pod_cols as usize) * (pod_rows as usize) * (POD_SIDE as usize) * (POD_SIDE as usize);
    let mut home_desks = Vec::with_capacity(n.min(grid_desk_cap.max(1)));
    let mut facings = Vec::with_capacity(n.min(grid_desk_cap.max(1)));
    // Ground, not slot, on BOTH axes: a walk-behind desk's blocked ground runs
    // DESK_GROUND_H/W past its Point, and the slot dims let it spill past the band.
    let desk_y_max =
        (cubicle_band.y + cubicle_band.height).saturating_sub(super::decor::DESK_GROUND_H);
    // `pod_cols` floors at 1, so a narrow band's forced pod puts its 2nd desk column
    // past the edge — an invisible desk whose walk anchor sits outside the mask.
    let desk_x_max =
        (cubicle_band.x + cubicle_band.width).saturating_sub(super::decor::DESK_GROUND_W);
    let push_desk = |desks: &mut Vec<Point>,
                     facings: &mut Vec<Facing>,
                     x: u16,
                     y: u16,
                     facing: Facing|
     -> bool {
        if desks.len() >= n || y > desk_y_max || x > desk_x_max {
            return desks.len() >= n;
        }
        desks.push(Point { x, y });
        facings.push(facing);
        false
    };

    'outer: for pod_r in 0..pod_rows {
        for pod_c in 0..pod_cols {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            for r in 0..POD_SIDE {
                for c in 0..POD_SIDE {
                    let full = push_desk(
                        &mut home_desks,
                        &mut facings,
                        pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                        pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                        pod_row_facing(r),
                    );
                    if full {
                        break 'outer;
                    }
                }
            }
        }
    }

    // Partial right-edge columns CONTINUE the pod lattice, so spacing never jumps.
    let partial_col_x = |i: u16| -> u16 {
        let (x, _) = grid.pod_origin(cubicle_band, pod_cols + i / POD_SIDE, 0);
        x + (i % POD_SIDE) * (DESK_W + INTRA_POD_GAP_X)
    };
    // POD_SIDE: a further column would need a residual wider than the stride already consumed.
    let partial_col_count = (0..POD_SIDE)
        .take_while(|&i| partial_col_x(i) <= desk_x_max)
        .count() as u16;
    let partial_col_at_right = partial_col_count > 0;
    if partial_col_at_right {
        'partial_x: for pod_r in 0..pod_rows {
            let (_, pod_origin_y) = grid.pod_origin(cubicle_band, 0, pod_r);
            for r in 0..POD_SIDE {
                for i in 0..partial_col_count {
                    let full = push_desk(
                        &mut home_desks,
                        &mut facings,
                        partial_col_x(i),
                        pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                        pod_row_facing(r),
                    );
                    if full {
                        break 'partial_x;
                    }
                }
            }
        }
    }

    // The Y twin: the row IS the first row of the (pod_rows)-th pod, so the rhythm holds.
    let (_, partial_y) = grid.pod_origin(cubicle_band, 0, pod_rows);
    let partial_row_at_bottom = partial_y <= desk_y_max;
    if partial_row_at_bottom {
        'partial_y: for pod_c in 0..pod_cols {
            let (pod_origin_x, _) = grid.pod_origin(cubicle_band, pod_c, 0);
            for c in 0..POD_SIDE {
                let full = push_desk(
                    &mut home_desks,
                    &mut facings,
                    pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                    partial_y,
                    pod_row_facing(0),
                );
                if full {
                    break 'partial_y;
                }
            }
        }
        for i in 0..partial_col_count {
            let full = push_desk(
                &mut home_desks,
                &mut facings,
                partial_col_x(i),
                partial_y,
                pod_row_facing(0),
            );
            if full {
                break;
            }
        }
    }

    (home_desks, facings)
}

/// The kind for aisle slot `slot_idx`. Each pass deals a FRESH permutation of
/// `PodDecor::ALL` — a fixed cyclic order lets one slot's kind tell you the next —
/// rotated past a collision with the previous pass's last kind, because two identical
/// pieces in neighbouring aisles is the failure a user sees first. Path-dependent, so
/// passes are re-walked, not cached; pinned by `no_two_adjacent_aisle_slots_share_a_kind`.
pub(super) fn decor_for_slot(floor_seed: u64, slot_idx: usize) -> PodDecor {
    // Below two kinds the adjacency rule is unhonourable and `rotate_left(1)` a no-op.
    const _: () = assert!(PodDecor::ALL.len() >= 2);
    // `a_wide_floors_decor_order_is_not_one_fixed_cycle` needs more distinct arrangements
    // than the roster has kinds, and can only observe `MAX_FLOORS` of them.
    const _: () = assert!(PodDecor::ALL.len() < pixtuoid_core::state::MAX_FLOORS);
    let n = PodDecor::ALL.len();
    let shuffled = |pass: u64| {
        let mut bag = PodDecor::ALL.to_vec();
        let mut z = floor_seed ^ pass;
        for i in (1..n).rev() {
            z = pixtuoid_core::id::splitmix64(z);
            bag.swap(i, (z % (i as u64 + 1)) as usize);
        }
        bag
    };
    let mut bag = shuffled(0);
    for pass in 1..=(slot_idx / n) as u64 {
        let prev_tail = bag[n - 1];
        bag = shuffled(pass);
        if bag[0] == prev_tail {
            bag.rotate_left(1);
        }
    }
    bag[slot_idx % n]
}

/// Decor items placed in aisles between 2x2 desk pods.
pub(super) fn compute_pod_decor(
    cubicle_band: &Bounds,
    grid: PodGrid,
    floor_seed: u64,
) -> Vec<PodDecorItem> {
    let PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        stride_x: pod_stride_x,
        stride_y: pod_stride_y,
        ..
    } = grid;
    let pod_w = pod_stride_x - INTER_POD_AISLE_X;
    let pod_h = pod_stride_y - INTER_POD_AISLE_Y;
    let mut pod_decor: Vec<PodDecorItem> = Vec::new();
    let mut slot_idx: usize = 0;
    // Mirror of push_desk's x clamp: a slot centre past the band edge would promote a
    // PhoneBooth/StandingDesk to a waypoint, sending idle agents to invisible furniture.
    let band_right = cubicle_band.x + cubicle_band.width;
    // Vertical twin: the LAST POD ROW's slot centre can sit close enough to the bottom
    // that a tall centred visual crosses into cubicle_aisle and blocks its cells.
    let band_bottom = cubicle_band.y + cubicle_band.height;
    let mut push_slot = |pod_decor: &mut Vec<PodDecorItem>, x: u16, y: u16| {
        let kind = decor_for_slot(floor_seed, slot_idx);
        // The cycle advances even when the slot drops, so survivors keep the kinds
        // they'd have on a wider floor.
        slot_idx += 1;
        let vis = furniture_def(kind.furniture()).visual;
        // Same centred-blit math the painter uses (pos − h/2 .. pos − h/2 + h).
        if x.saturating_sub(vis.w / 2) + vis.w > band_right
            || y.saturating_sub(vis.h / 2) + vis.h > band_bottom
        {
            return;
        }
        pod_decor.push(PodDecorItem {
            kind,
            pos: Point { x, y },
        });
    };
    for pod_r in 0..pod_rows {
        for pod_c in 0..pod_cols.saturating_sub(1) {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            let aisle_cx = pod_origin_x + pod_w + INTER_POD_AISLE_X / 2;
            let aisle_cy = pod_origin_y + pod_h / 2;
            push_slot(&mut pod_decor, aisle_cx, aisle_cy);
        }
    }
    for pod_r in 0..pod_rows.saturating_sub(1) {
        for pod_c in 0..pod_cols {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            let aisle_cx = pod_origin_x + pod_w / 2;
            let aisle_cy = pod_origin_y + pod_h + INTER_POD_AISLE_Y / 2;
            push_slot(&mut pod_decor, aisle_cx, aisle_cy);
        }
    }
    pod_decor
}

/// Waypoints: couch, pantry, pod-decor-promoted (PhoneBooth/StandingDesk), corridor
/// appliances (VendingMachine/Printer). Each argument is a distinct zone or fact.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_waypoints(
    cubicle_band: &Bounds,
    top_margin: u16,
    pantry_room: Option<Bounds>,
    pantry_counter_size: Size,
    pod_decor: &[PodDecorItem],
    cubicle_aisle: &Bounds,
    meeting_rooms: &[MeetingRoom],
    lounge_fits: bool,
    west_clear_x: u16,
) -> (Vec<Waypoint>, Option<Point>) {
    let right_x = cubicle_band.x;
    let right_w = cubicle_band.width;
    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(cubicle_band, top_margin, west_clear_x);
    // room_id stays None: lounge grouping is keyed at the chitchat venue layer, not here.
    let mut waypoints: Vec<Waypoint> = if lounge_fits {
        SEAT_DX
            .into_iter()
            .map(|dx| Waypoint {
                pos: Point {
                    x: couch_x.saturating_add_signed(dx),
                    y: couch_y,
                },
                kind: WaypointKind::Couch,
                // SEATED facing: the sitter looks NORTH at the window. The APPROACH side
                // is decoupled (Furniture::Couch uses ApproachSides::ALL, decor.rs).
                facing: Facing::North,
                room_id: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    if let Some(pr) = pantry_room {
        let half_cw = pantry_counter_size.w / 2;
        let max_cx = pr.x + pr.width.saturating_sub(half_cw + 1);
        // A room narrower than the counter has no valid centre — refuse rather than force.
        let min_cx = pr.x + half_cw;
        if min_cx <= max_cx {
            // y is single-sourced with the island clamp; only x is size-shaped.
            let wy = PantryRoom::counter_center_y(pr, pantry_counter_size);
            let wx = if pantry_counter_size.w >= PANTRY_COUNTER_LARGE_W {
                (pr.x + pr.width / 2).clamp(min_cx, max_cx)
            } else {
                (pr.x + pct(pr.width, 60)).clamp(min_cx, max_cx)
            };
            waypoints.push(Waypoint {
                pos: Point { x: wx, y: wy },
                kind: WaypointKind::Pantry,
                facing: Facing::South,
                room_id: None,
            });
        }
    }
    for &PodDecorItem { kind, pos } in pod_decor {
        // Exhaustive (no `_`): a NEW PodDecor must make a deliberate wander decision
        // here — `None` = pure decor, `Some(kind)` = also a walkable destination.
        let wp_kind = match kind {
            PodDecor::PhoneBooth => Some(WaypointKind::PhoneBooth),
            PodDecor::StandingDesk => Some(WaypointKind::StandingDesk),
            PodDecor::PlantTall | PodDecor::Whiteboard | PodDecor::Tv => None,
        };
        if let Some(wp_kind) = wp_kind {
            waypoints.push(Waypoint {
                pos,
                kind: wp_kind,
                facing: Facing::South,
                room_id: None,
            });
        }
    }

    const VENDING_MIN_AISLE_H: u16 = 10;
    const VENDING_MIN_AISLE_W: u16 = 30;
    const PRINTER_MIN_AISLE_H: u16 = 9;
    const PRINTER_MIN_AISLE_W: u16 = 40;
    if cubicle_aisle.height >= VENDING_MIN_AISLE_H && cubicle_aisle.width > VENDING_MIN_AISLE_W {
        waypoints.push(Waypoint {
            pos: Point {
                x: right_x + 5,
                y: cubicle_aisle.y + 3,
            },
            kind: WaypointKind::VendingMachine,
            facing: Facing::South,
            room_id: None,
        });
    }
    if cubicle_aisle.height >= PRINTER_MIN_AISLE_H && cubicle_aisle.width > PRINTER_MIN_AISLE_W {
        waypoints.push(Waypoint {
            pos: Point {
                x: right_x + right_w.saturating_sub(10),
                y: cubicle_aisle.y + 2,
            },
            kind: WaypointKind::Printer,
            facing: Facing::South,
            room_id: None,
        });
    }

    // `room_id` is the room's TRUE index — a trio-less room keeps its slot, so it never shifts.
    for (room_id, room) in meeting_rooms.iter().enumerate() {
        let Some(trio) = room.trio else { continue };
        let table = trio.table;
        for sofa in trio.sofas {
            let facing = if sofa.y < table.y {
                Facing::South
            } else {
                Facing::North
            };
            for dx in SEAT_DX {
                waypoints.push(Waypoint {
                    pos: Point {
                        x: sofa.x.saturating_add_signed(dx),
                        y: sofa.y,
                    },
                    kind: WaypointKind::MeetingSofa,
                    facing,
                    room_id: Some(room_id),
                });
            }
        }
        // The offsets must MIRROR: the table blocks x ∈ [t.x−7, t.x+7], and an asymmetric
        // pair puts one chair closer to the wood, swallowing the rug border its twin shows.
        let chair_dx = super::rooms::meeting::MEETING_CHAIR_TABLE_DX as i16;
        for (dx, facing) in [(-chair_dx, Facing::East), (chair_dx, Facing::West)] {
            waypoints.push(Waypoint {
                pos: Point {
                    x: table.x.saturating_add_signed(dx),
                    y: table.y,
                },
                kind: WaypointKind::MeetingChair,
                facing,
                room_id: Some(room_id),
            });
        }
    }

    debug_assert!(
        waypoints.iter().all(|w| {
            matches!(
                w.kind,
                WaypointKind::MeetingSofa | WaypointKind::MeetingChair
            ) == w.room_id.is_some()
        }),
        "room_id must be Some exactly for meeting-slot waypoints"
    );

    (
        waypoints,
        lounge_fits.then_some(Point {
            x: couch_x,
            y: couch_y,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{FloorGeometry, FloorVariant};

    #[test]
    fn a_degraded_dense_floor_reads_the_standard_column_percent() {
        let degraded = FloorGeometry {
            variant: FloorVariant::Dense,
            has_dual_meeting: false,
        };
        assert_eq!(
            degraded.mid_x_pct(),
            FloorVariant::Standard.mid_x_pct(),
            "a too-short Dense floor degrades to the Standard geometry"
        );
        let dual = FloorGeometry {
            variant: FloorVariant::Dense,
            has_dual_meeting: true,
        };
        assert_eq!(
            dual.mid_x_pct(),
            FloorVariant::Dense.mid_x_pct(),
            "a Dense floor that KEEPS both meeting rooms keeps its own column"
        );
    }

    /// `DESK_BAND_MIN_H` omits the Y clamp's third term on the grounds that it is 0
    /// at the floor. Pin that premise: move `COUCH_GAP_GROWTH_BASE_H` under the floor
    /// and the height derivation is silently short by the gap it stops accounting for.
    #[test]
    fn the_couch_gap_is_zero_everywhere_the_height_floor_is_solved() {
        let mut h = super::MIN_LAYOUT_H;
        while h < super::COUCH_GAP_GROWTH_BASE_H {
            assert_eq!(
                super::couch_to_desk_extra(h),
                0,
                "buf_h {h}: the Y clamp's third term is non-zero where DESK_BAND_MIN_H \
                 assumes it is 0"
            );
            h += 1;
        }
    }

    /// Neither floor carries a SAFETY MARGIN — one-directional on purpose. It catches a
    /// floor set too HIGH, which on the width axis nothing else can: `pct` floors, so
    /// `band_w(37,35) == band_w(38,35)` and `layout::tests`' `narrowest_band ==` assert
    /// is blind to +1. Too LOW is `every_floor_variant_seats_a_desk…`'s job. Tautological
    /// against today's `while` loops — that IS the point: it fires when a number replaces it.
    #[test]
    fn neither_floor_carries_a_safety_margin() {
        assert!(
            super::band_w(super::MIN_LAYOUT_W - 1, super::widest_mid_x_pct())
                < super::DESK_BAND_MIN_W,
            "the width floor is not tight: {} px still clears the band",
            super::MIN_LAYOUT_W - 1
        );
        assert!(
            super::band_h(super::MIN_LAYOUT_H - 1) < super::DESK_BAND_MIN_H,
            "the height floor is not tight: {} px still clears the band",
            super::MIN_LAYOUT_H - 1
        );
    }

    /// `DESK_BAND_MIN_W` must be where `compute_pod_desks` ITSELF stops fitting a desk —
    /// the width floor is derived from it, so a formula that merely looks right
    /// would advertise a minimum the placer disagrees with.
    #[test]
    fn min_band_w_is_the_placers_own_first_desk_boundary() {
        let pod_h =
            super::POD_SIDE * super::DESK_H + (super::POD_SIDE - 1) * super::INTRA_POD_GAP_Y;
        let grid = super::PodGrid {
            // The forced single pod of a narrow band (`pod_cols`/`pod_rows` floor at 1).
            cols: 1,
            rows: 1,
            stride_x: super::POD_SIDE * super::DESK_W
                + (super::POD_SIDE - 1) * super::INTRA_POD_GAP_X
                + super::INTER_POD_AISLE_X,
            stride_y: pod_h + super::INTER_POD_AISLE_Y,
            couch_to_desk_extra: 0,
        };
        for (width, seats) in [
            (super::DESK_BAND_MIN_W, true),
            (super::DESK_BAND_MIN_W - 1, false),
        ] {
            let band = super::Bounds {
                x: 5,
                y: 5,
                width,
                height: 200,
            };
            let (desks, _) = super::compute_pod_desks(None, &band, grid);
            assert_eq!(
                !desks.is_empty(),
                seats,
                "a {width}px band: expected seats={seats}, placed {} desk(s)",
                desks.len()
            );
        }
        // The Y twin, same clamp on the other axis — the height floor derives from it.
        for (height, seats) in [
            (super::DESK_BAND_MIN_H, true),
            (super::DESK_BAND_MIN_H - 1, false),
        ] {
            let band = super::Bounds {
                x: 5,
                y: 5,
                width: 200,
                height,
            };
            let (desks, _) = super::compute_pod_desks(None, &band, grid);
            assert_eq!(
                !desks.is_empty(),
                seats,
                "a {height}px-tall band: expected seats={seats}, placed {} desk(s)",
                desks.len()
            );
        }
    }

    /// Cross-checked against `compute_pod_desks`' own positions, not the band formula a
    /// test could only copy.
    #[test]
    fn no_inter_pod_band_row_is_ground_a_desk_blocks() {
        let band = super::Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 400,
        };
        let pod_h =
            super::POD_SIDE * super::DESK_H + (super::POD_SIDE - 1) * super::INTRA_POD_GAP_Y;
        let grid = super::PodGrid {
            cols: 2,
            rows: 3,
            stride_x: super::POD_SIDE * super::DESK_W
                + (super::POD_SIDE - 1) * super::INTRA_POD_GAP_X
                + super::INTER_POD_AISLE_X,
            stride_y: pod_h + super::INTER_POD_AISLE_Y,
            couch_to_desk_extra: 0,
        };
        let (desks, _) = super::compute_pod_desks(None, &band, grid);
        assert!(!desks.is_empty(), "the fixture grid must place desks");
        let bands = grid.inter_pod_y_bands(&band);
        assert!(!bands.is_empty(), "a 3-row grid has bands between its rows");
        for &(start, end) in &bands {
            for d in &desks {
                let (g0, g1) = (d.y, d.y + super::decor::DESK_GROUND_H);
                assert!(
                    start >= g1 || end <= g0,
                    "band {start}..{end} overlaps desk {d:?}'s ground rows {g0}..{g1}"
                );
            }
        }
    }
}
