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
    // The westmost couch seat's ground must stay east of any divider wall to its
    // west; `west_clear_x` is the wall's east edge (== band start when there is
    // no wall, so the clamp is a no-op).
    let couch_west_reach =
        (-SEAT_DX[0]) as u16 + furniture_def(Furniture::Couch).footprint.map_or(0, |f| f.w) / 2;
    Point {
        x: (cubicle_band.x + pct(cubicle_band.width, 35)).max(west_clear_x + couch_west_reach),
        y: top_margin + 3,
    }
}

/// The cubicle band that seats ONE desk, per axis — the terms `compute_pod_desks`'
/// clamp compares on that axis: the first pod's aisle half, plus the desk's blocked
/// GROUND extent (side cabinets / walk-behind overhang included, so NOT
/// `DESK_W`/`DESK_H`). The Y clamp adds a third, `couch_to_desk_extra`, which is 0
/// everywhere below `COUCH_GAP_GROWTH_BASE_H` — the floor is solved well under it.
pub(super) const DESK_BAND_MIN_W: u16 = INTER_POD_AISLE_X / 2 + DESK_GROUND_W;
pub(super) const DESK_BAND_MIN_H: u16 = INTER_POD_AISLE_Y / 2 + DESK_GROUND_H;

/// The 1px column between the left rooms and the cubicle band — `right_x` steps
/// over it and `band_w` subtracts it, so it is one const, not two literals.
const MID_DIVIDER_W: u16 = 1;

/// The cubicle band's extent for a buffer, per axis — everything east of the
/// left column and its divider; everything below the wall band minus the
/// appliance aisle. THE two formulas `compute_with_seed` and the floor
/// derivations both step from.
pub(super) const fn band_w(buf_w: u16, mid_x_pct: u16) -> u16 {
    buf_w.saturating_sub(pct(buf_w, mid_x_pct) + MID_DIVIDER_W)
}

pub(super) const fn band_h(buf_h: u16) -> u16 {
    let usable = buf_h.saturating_sub(top_margin(buf_h));
    usable.saturating_sub(cubicle_aisle_h(usable))
}

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

/// The smallest buffer `compute_with_seed` lays out — below either bound it
/// returns `None` ("terminal too small").
///
/// BOTH axes are SOLVED against the band, not the buffer. The two hand-written
/// floors erred in OPPOSITE directions: W priced one desk against the whole buffer
/// and advertised a size that lays out an office with no desk to seat anyone, while
/// H was simply never re-derived and refused 15 buffer px of sizes that render.
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

    // Large counter + a 2-px routing margin each side, else the compact
    // fallback. Width-only, so the size is known before the room split below
    // prices the pantry's content against it.
    let pantry_counter_size: Size = if has_pantry && mid_x >= PANTRY_COUNTER_LARGE_W + 4 {
        Size {
            w: PANTRY_COUNTER_LARGE_W,
            h: 10,
        }
    } else {
        super::rooms::pantry::COMPACT_COUNTER
    };

    // Meeting-room height: CONTENT-FIT, donating the surplus to the pantry
    // below. The donation is ALL-OR-NOTHING — the room shrinks exactly to
    // `usable_h − pantry_content_h` when that both keeps the trio fit AND
    // reaches the pantry's content height, else the half-split stands; a partial
    // donation would cram the trio to its fit gate to buy rows the island still
    // couldn't use. Dense keeps the raw split: BOTH halves host a trio.
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
        // A meeting always shares the left column with either the pantry or a
        // second meeting room, so it takes the top of the column up to the split.
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
    // East edge of the meeting-room divider wall — the west bound lounge
    // furniture must clear. No meeting room ⇒ no wall ⇒ the clamp collapses to
    // the band start.
    let lounge_west_clear = if has_meeting {
        mid_x + super::WALL_THICK_V
    } else {
        right_x
    };
    let cubicle_aisle_h = cubicle_aisle_h(usable_h);
    let cubicle_h = usable_h.saturating_sub(cubicle_aisle_h);
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
    let couch_to_desk_extra = buf_h.saturating_sub(COUCH_GAP_GROWTH_BASE_H) / 20;
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

    // Vec index IS the room_id: a room too small for its trio still occupies its
    // slot with `trio: None`, so bounds and furniture can't mis-join. `dense` =
    // room 1 (under the glass divider); room 0 is the wall-apron room.
    let mut meeting_rooms: Vec<MeetingRoom> = Vec::new();
    for (room_idx, room) in [meeting_room, meeting_room_2].into_iter().enumerate() {
        let Some(mr) = room else { continue };
        let trio = room_fits_furniture(&mr).then(|| MeetingRoom::place_trio(mr, room_idx != 0));
        meeting_rooms.push(MeetingRoom { bounds: mr, trio });
    }

    // Walls are a FUNCTION of the rooms: each requests its enclosure edges +
    // doors, the resolver merges shared boundaries and cuts gaps. Dense's
    // inter-meeting wall is deliberately solid (#557 door policy).
    let (room_walls, doorways) =
        super::rooms::walls::derive_room_walls(&meeting_rooms, pantry_room);

    // Elevator door — sprite mounted in the back wall, slotted into the
    // rightmost window position and BOTTOM-aligned with the floor-to-ceiling
    // windows so both sit on the same wall plane. Computed HERE (above the
    // lounge gate) so the gate can check couch↔door clearance.
    let top_wall_h = top_margin.saturating_sub(super::WALL_BAND_TO_TOP_MARGIN);
    let window_bottom_y = top_wall_h.saturating_sub(3); // matches paint_floor_and_walls' window_h
    let door = if buf_w >= ELEVATOR_W + 4 && window_bottom_y + 1 >= ELEVATOR_H {
        Some(Point {
            x: buf_w.saturating_sub(ELEVATOR_W + 2),
            // +2 nudge: drops the elevator bottom 2 px below the
            // window line so it visually rests against the floor
            // instead of floating mid-wall.
            y: window_bottom_y + 1 - ELEVATOR_H + 2,
        })
    } else {
        None
    };
    /// How far SOUTH of the floor line the elevator spawn sits, so a character
    /// entering stands on open floor rather than on the wall apron — the strip
    /// the straddling wall decor stamps its ground into.
    const DOOR_THRESHOLD_CLEARANCE_PX: u16 = 4;
    let door_threshold = door.map(|d| Point {
        x: d.x + ELEVATOR_W / 2,
        y: top_margin + DOOR_THRESHOLD_CLEARANCE_PX,
    });

    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(&cubicle_band, top_margin, lounge_west_clear);
    // Below this WEST-side fit the whole lounge vignette (couch + floor lamp +
    // side table) degrades away. 30 = the vignette's blocked span +
    // OBSTACLE_PAD_PX each side + walk clearance.
    const LOUNGE_MIN_BAND_W: u16 = 30;
    // EAST-side twin of the width gate (#566): the east couch seat's padded
    // ground must stay at-or-west of door_threshold.x, else the couch seals the
    // spawn threshold's own column. WAYPOINT_STAMP_PAD_PX is the pad the mask's
    // SEAT stamp uses, NOT the OBSTACLE_PAD_PX routing pad.
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

    // Plants scatter at the cubicle corridor edges + the meeting-room corners
    // (plus the two gated Ficus below). NOT the pantry (a plant + pad blocks the
    // only bridge to the cubicle area), NOT the cubicle top strip (a 7-px
    // wall-to-couch gap), NOT a meeting interior (disconnects the door gap).
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
    // Meeting-room corner plants on the west wall, clear of the east-wall door
    // and the central sofa/table column. Gated on room size so the plant + pad
    // can't squeeze the walkable strip below routable width.
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

    // Lounge vignette — computed AFTER `door` because the tank prices its east
    // limit against the elevator column.
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

    // Two Ficus spots: a greeting plant west of the elevator door, and the
    // lounge's west flank. Gated on a ROOMY band — on a narrower one the lounge
    // pot lands against the rooms column and the elevator one pinches the door
    // approach, each sealing a top-strip pocket.
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
            // Ground is centred on `pos`, so keep its west edge east of the
            // divider wall.
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

    // The island pushes its 4 Island slots BEFORE the snack shelf's slot — the
    // waypoint push order the goldens pin.
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

    // Scatter plants settle only now — AFTER every waypoint exists; filtering at
    // the candidate site checked a subset of the final set. Each candidate yields
    // to desk grounds and keeps PLANT_OBSTACLE_CLEARANCE_PX from every obstacle
    // box, SLIDING inward along the aisle before giving up — yield-by-deletion
    // stripped the office's greenery. `plant_obstacle_rects` derives the fixed
    // NON-waypoint singletons; omitting one shipped interpenetration bugs.
    let singleton_rects = plant_obstacle_rects(
        fish_tank,
        floor_lamp,
        lounge_side_table,
        kitchen_island,
        &meeting_rooms,
    );
    // Folded, not mapped: the corridor's two pots slide toward each other, so
    // each must clear against the ones already placed.
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
    // Seed for BOTH the connectivity guard and the ReachSet: the door, where
    // agents enter, so always in the main component.
    let conn_seed = door_threshold
        .or_else(|| home_desks.first().copied())
        .unwrap_or(Point {
            x: buf_w / 2,
            y: buf_h / 2,
        });

    // Connectivity at ROUTER granularity, not just the pixel flood's — a ≤3 px
    // channel is pixel-connected and coarse-IMPASSABLE (scene CLAUDE.md, #566).
    let severed = |mask: &WalkableMask| -> bool {
        if !unreachable_walkable_cells(mask, conn_seed).is_empty() {
            return true;
        }
        let reach = ReachSet::from_mask(mask, conn_seed);
        // Judged at `Facing::South` for EVERY desk: South is where the demotion pass below
        // retreats, so a reachable south seat proves no decor arrangement strands a desk.
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
    // Connectivity guard (#566): a scatter plant can settle onto the aisle floor
    // and plug the SOLE inter-pod drain, sealing the appliance strip off from the
    // door. A decorative plant may NEVER disconnect the office. The flood runs on
    // EVERY compute (not gated to narrow bands): the check IS the guard, a
    // generic net for ANY future sealing decor, and compute is resize-gated, not
    // per-frame.
    if severed(&walkable) {
        // The pocket cells sit ACROSS the drain from the seal-causing plant (not
        // 4-adjacent to it), so target by "settled into the aisle", not "borders
        // the pocket".
        plants.retain(|p| !plant_ground_in_bounds(p, &cubicle_aisle));
        walkable = build_mask(&plants, &wall_decor);
        // Next rung — only a wall decor that TOUCHES THE FLOOR can seal a lane, so drop
        // those (by footprint, not by kind) before the drastic clear-all-plants.
        if severed(&walkable) {
            wall_decor.retain(|d| furniture_def(d.kind.furniture()).footprint.is_none());
            walkable = build_mask(&plants, &wall_decor);
        }
        // Last resort: drop every remaining scatter plant.
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

    // The lounge vignette as ONE unit: couch + lamp + side table are Some exactly
    // iff `lounge_fits`, so the zip is None precisely when the vignette doesn't
    // fit; the aquarium rides along as its own Option (extra east-clearance gate).
    let lounge = couch_sprite_center
        .zip(floor_lamp)
        .zip(lounge_side_table)
        .map(|((couch_center, floor_lamp), side_table)| Lounge {
            couch_center,
            floor_lamp,
            side_table,
            fish_tank,
        });

    // A narrow band can wall off a back-turned desk's SOUTH front, leaving it with no
    // reachable approach; demote rather than drop. Safe after the mask: a desk's blocked
    // ground is its body whichever way its occupant sits. A NET, not live code — see
    // `SHARP-EDGES.md`.
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

/// Place the four wall-band decorations (bookshelf, exit sign, whiteboard,
/// meeting screen), each TOP-LEFT-anchored so its bottom row lands on the last
/// wall-band row no matter how tall the band grows.
///
/// The meeting screen hugs room 0's WEST corner; the bookshelf then spreads to
/// the room's EAST side. That spread is LOAD-BEARING, not taste: the wall-band
/// carpet apron between the two decor grounds must drain south AROUND the tucked
/// sofa (whose padded body seals the lane above the backrest), else those apron
/// cells strand. Any wall item whose clamped slot would pierce the divider /
/// exit sign / elevator drops entirely, reopening the channel by absence.
#[allow(clippy::too_many_arguments)] // layout inputs — each arg a distinct zone/fact
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
    // A room narrower than the screen would hang it ACROSS the east wall — drop
    // it entirely, the same degradation as the bare meeting room.
    let meeting_screen_x = meeting_room.and_then(|mr| {
        let sx = mr.bounds.x + 1;
        (sx + screen_w < mr.bounds.x + mr.bounds.width).then_some(sx)
    });
    let bookshelf_x = {
        let x = pct(buf_w, 18);
        match (meeting_screen_x, meeting_room) {
            (Some(sx), Some(mr)) => {
                // The ONE flush slot: screen east edge + a 2-px gap, so the two
                // grounds' pads merge with no strandable apron cell between
                // them. Every arm below derives from it.
                let flush_east = sx + screen_w + 2;
                // The drain term applies only when room 0 actually HOSTS its
                // trio: with no sofa, pushing the shelf east hangs it over the
                // cubicle band, where a desk pod's pad seals the apron gap.
                if let Some(sofa_pad_east) = mr.sofa_east_drain_edge() {
                    // Past the sofa's drain edge by the shelf's OWN 1-px ground
                    // pad (mask.rs stamps wall decor with pad=1, NOT
                    // OBSTACLE_PAD_PX) + a ≥2-px walkable channel + slack.
                    const BOOKSHELF_DRAIN_GAP: u16 = 5;
                    let spread = x.max(flush_east).max(sofa_pad_east + BOOKSHELF_DRAIN_GAP);
                    if spread + bookshelf_w < mr.bounds.x + mr.bounds.width {
                        spread
                    } else {
                        // Narrow trio room: the spread slot would pierce the
                        // divider. Fall back to the FLUSH slot — NOT the pct-18
                        // anchor, which at these widths opens a strandable gap
                        // OVER the sofa pad.
                        flush_east
                    }
                } else {
                    x.max(flush_east)
                }
            }
            _ => x,
        }
    };
    // Everything east of the exit sign / elevator face is off-limits.
    let exit_sign_x = buf_w.saturating_sub(9);
    let wall_east_limit = exit_sign_x.min(door.map(|d| d.x).unwrap_or(u16::MAX));
    // The bookshelf additionally stays WEST of the vertical divider: on narrow
    // trio rooms the drain clamp can push it onto the wall's top segment, where
    // it visually pierces the glass. Dropping it there reopens the apron channel.
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

/// The lounge vignette singletons, all anchored to the viewing couch and gated
/// as ONE cluster on `lounge_fits`.
struct LoungeVignette {
    floor_lamp: Option<Point>,
    side_table: Option<Point>,
    fish_tank: Option<Point>,
}

/// Place the lounge vignette — floor lamp, side table, aquarium — around the
/// viewing couch; the three live and die together on `lounge_fits`. The lamp
/// sits just east of the couch so its halo bathes the seating area at night; the
/// side table takes the OPPOSITE (west) flank, clamped clear of the room-divider
/// column. The aquarium carries an EXTRA gate the other two don't: it must stay
/// clear of the elevator `door` column so the spawn threshold never routes
/// around it.
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
        // The tank's west edge sits LAMP_TANK_GAP columns past the lamp shade's
        // east edge — one clear floor column of vignette breathing room. A
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

/// THE non-waypoint obstacle census a scatter plant must clear — the single
/// derivation shared by the production settle path and the placement-sweep
/// backstop, so the two can't drift. Takes EVERY non-waypoint singleton
/// EXPLICITLY and includes each IFF its kind [`repels_plants`]; waypoint
/// obstacles are handled by `first_blocking_waypoint`.
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

/// Settle a scatter-plant candidate: keep its authored spot when clear, else
/// slide 1px at a time toward the cubicle band's horizontal centre (bounded)
/// until both the desk-ground and obstacle-clearance rules pass; a candidate
/// that never clears yields entirely.
fn settle_plant(
    p: PlantItem,
    home_desks: &[Point],
    waypoints: &[Waypoint],
    singletons: &[(Point, Size)],
    band: &Bounds,
    floor_seed: u64,
) -> Option<PlantItem> {
    // 12: two appliance widths — enough to clear any single corner appliance
    // without wandering out of the authored corner region.
    const MAX_PLANT_NUDGE_PX: u16 = 12;
    /// How far into the nudge budget the seeded first try may reach. The budget
    /// and its inward direction are the ladder's own, so a scattered pot can
    /// only stand where a DISPLACED one already could — no new clearance rule,
    /// and no way out of the container the ladder already respects.
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
    // Beside the blocking obstacle, toward the band centre, on ITS row: it owns
    // the plant's authored corner at most sizes and the plant's own row is
    // desk-saturated on packed floors, so the corridor floor beside it is the
    // one desk-free spot.
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
    /// THE roster: the floor derivations sweep it, `from_seed` indexes it, and
    /// `COUNT` is its length. A variant missing from here is unreachable — clippy's
    /// `dead_code` under `-D warnings` reds on the never-constructed enum arm, NOT
    /// `the_sweep_reaches_every_floor_variant`, which counts observed shapes against
    /// `ALL.len()` and so stays green when both stay at 5. One added HERE that the
    /// sweep seeds never reach is the direction that test does catch.
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
    // `None` fills the grid; `Some(cap)` caps the count. Bound the allocation
    // hint to the grid's physical capacity: `n` may be `usize::MAX`, and
    // `Vec::with_capacity(usize::MAX)` aborts.
    let n = max_desks.unwrap_or(usize::MAX);
    let grid_desk_cap =
        (pod_cols as usize) * (pod_rows as usize) * (POD_SIDE as usize) * (POD_SIDE as usize);
    let mut home_desks = Vec::with_capacity(n.min(grid_desk_cap.max(1)));
    let mut facings = Vec::with_capacity(n.min(grid_desk_cap.max(1)));
    // Honest GROUND clamp on Y (the twin of desk_x_max below): the desk is
    // walk-behind (ground_y: End), so its blocked ground reaches DESK_GROUND_H
    // below the desk Point, NOT DESK_H (the slot) — clamping on DESK_H let a
    // bottom-row desk's ground spill south into cubicle_aisle.
    let desk_y_max =
        (cubicle_band.y + cubicle_band.height).saturating_sub(super::decor::DESK_GROUND_H);
    // Mirror clamp for x: `pod_cols` floors at 1, so on a narrow band the forced
    // pod's 2nd desk column lands past the band's right edge — an invisible desk
    // whose walk anchor sits outside the mask. Skip those; the floor degrades to
    // fewer desks. Ground, not slot: the blocked ground is DESK_GROUND_W wide
    // (the side cabinets), so the last column must leave room for the full
    // sprite, and DESK_W here let it poke past the buffer edge.
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

    // Partial pod columns at the RIGHT edge: each leftover strip wide enough for
    // a single desk column + half-aisle gets another 1×POD_SIDE column. They
    // CONTINUE the pod lattice — column i is the (i % POD_SIDE)-th column of the
    // (pod_cols + i/POD_SIDE)-th pod — so spacing never jumps as width changes.
    let partial_col_x = |i: u16| -> u16 {
        let (x, _) = grid.pod_origin(cubicle_band, pod_cols + i / POD_SIDE, 0);
        x + (i % POD_SIDE) * (DESK_W + INTRA_POD_GAP_X)
    };
    // POD_SIDE: a further column is arithmetically unreachable — it would need a
    // residual wider than the pod stride `pod_cols` already consumed.
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

    // Partial pod ROW at the BOTTOM edge — the Y twin of the partial columns:
    // the row IS the first row of the (pod_rows)-th pod, so the inter-pod rhythm
    // holds.
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

/// The kind for aisle slot `slot_idx`. Each pass over the roster deals a FRESH
/// permutation of `PodDecor::ALL`, rotated past a collision with the previous
/// pass's last kind, so a floor sees every kind before any repeats — the
/// property a bare rotation was chosen for — WITHOUT its fixed cyclic order,
/// where one slot's kind tells you the next one's. Per pass, not once: a floor
/// holds several passes' worth of slots, so one permutation cycled would repeat
/// itself verbatim.
///
/// Two identical pieces in neighbouring aisles is the failure a user sees
/// first, and the one property the rotation gave for free — hence the collision
/// rotate. It is path-dependent, so every pass is re-walked from 0 on each call
/// rather than cached, keeping this a pure function of the seed; pinned by
/// `no_two_adjacent_aisle_slots_share_a_kind`.
pub(super) fn decor_for_slot(floor_seed: u64, slot_idx: usize) -> PodDecor {
    // Two kinds is the floor for alternating; below it the adjacency rule
    // cannot be honoured and `rotate_left(1)` is a no-op. The ceiling is the
    // floor COUNT: `a_wide_floors_decor_order_is_not_one_fixed_cycle` needs more
    // distinct arrangements than the roster has kinds, and it can only observe
    // `MAX_FLOORS` of them.
    const _: () = assert!(PodDecor::ALL.len() >= 2);
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
    // Mirror of push_desk's x clamp: `pod_cols` floors at 1, so on a narrow band
    // the forced pod's aisle-slot centre lands past the band's right edge, and a
    // PhoneBooth/StandingDesk there gets promoted to a wander waypoint, sending
    // idle agents to invisible furniture. The kind cycle still advances so
    // surviving slots keep the kinds they'd have on a wider floor.
    let band_right = cubicle_band.x + cubicle_band.width;
    // Vertical twin of the x clamp: the LAST POD ROW's aisle-slot centre can sit
    // close enough to the band's bottom that a tall centred visual crosses into
    // the cubicle_aisle and blocks its cells. Same centred-blit math the painter
    // uses (pos - h/2 .. pos - h/2 + h).
    let band_bottom = cubicle_band.y + cubicle_band.height;
    let mut push_slot = |pod_decor: &mut Vec<PodDecorItem>, x: u16, y: u16| {
        let kind = decor_for_slot(floor_seed, slot_idx);
        slot_idx += 1;
        let vis = furniture_def(kind.furniture()).visual;
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
    // Vertical-aisle slots (between adjacent pod columns, one per pod row).
    for pod_r in 0..pod_rows {
        for pod_c in 0..pod_cols.saturating_sub(1) {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            let aisle_cx = pod_origin_x + pod_w + INTER_POD_AISLE_X / 2;
            let aisle_cy = pod_origin_y + pod_h / 2;
            push_slot(&mut pod_decor, aisle_cx, aisle_cy);
        }
    }
    // Horizontal-aisle slots (between adjacent pod rows, one per pod column).
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

/// Waypoints: couch, pantry, pod-decor-promoted (PhoneBooth/StandingDesk),
/// corridor appliances (VendingMachine/Printer).
#[allow(clippy::too_many_arguments)] // layout inputs — each arg a distinct zone/fact
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
    // Lounge couch: 3 seats across the sofa, matching the meeting sofa. room_id
    // stays None — the lounge's group-chat grouping is keyed at the chitchat
    // venue layer, NOT via the meeting-only room_id field. Gated on
    // `lounge_fits`: on a degenerate narrow band the padded couch swallows the
    // whole floor, door threshold included.
    let mut waypoints: Vec<Waypoint> = if lounge_fits {
        SEAT_DX
            .into_iter()
            .map(|dx| Waypoint {
                pos: Point {
                    x: couch_x.saturating_add_signed(dx),
                    y: couch_y,
                },
                kind: WaypointKind::Couch,
                // SEATED facing: the sitter looks NORTH at the window. The
                // APPROACH side is decoupled (Furniture::Couch uses
                // ApproachSides::ALL); see decor.rs Couch row.
                facing: Facing::North,
                room_id: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    if let Some(pr) = pantry_room {
        // Clamp x so the counter fits within pantry_room instead of extending
        // past the east wall into the cubicle band.
        let half_cw = pantry_counter_size.w / 2;
        let max_cx = pr.x + pr.width.saturating_sub(half_cw + 1);
        // The WEST twin of the east clamp: a room narrower than the counter has
        // no valid centre at all, and an un-clamped west side spills the counter
        // off the buffer, silently hidden by saturating_sub. Refuse rather than
        // force — no counter on a degenerate pantry.
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
        // Exhaustive (no `_`): a NEW PodDecor must make a deliberate
        // wander-destination decision here — `None` = pure decor (aisle obstacle
        // only), `Some(kind)` = also a walkable destination.
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

    // Corridor appliances — stored as centre points (same convention as
    // Pantry/Couch); the painter derives top-left via sub(w/2, h/2).
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

    // Meeting-room slots. Every slot in a room shares its `room_id` (the room's
    // TRUE index in `meeting_rooms` — a bare trio-less room keeps its slot, so
    // the id can never shift) so the group-chitchat venue keys on the room.
    for (room_id, room) in meeting_rooms.iter().enumerate() {
        let Some(trio) = room.trio else { continue };
        let table = trio.table;
        for sofa in trio.sofas {
            // The pair must read as two people facing each other across the
            // table.
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
        // The offsets must MIRROR: the table obstacle blocks x ∈ [t.x−7, t.x+7],
        // and an asymmetric pair puts one chair body closer to the table wood,
        // swallowing the rug border its twin shows.
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

    // Load-bearing for chitchat venue grouping: a stray `room_id` mis-groups a
    // non-meeting waypoint into a meeting venue, and a missing one never groups.
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
