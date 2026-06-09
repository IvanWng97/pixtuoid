//! Layout computation helpers — extracted from mod.rs for file size.
//! All functions here are `pub(super)` so the parent module can call them
//! from `SceneLayout` impl methods.

use super::mask;
use super::*;

/// `n`% of a dimension, computed in u32 to avoid u16 overflow on very large
/// terminals — a bare `buf_h * 30` overflows u16 once `buf_h > 2184` (and the
/// derived sub-region multiplies overflow at the same extreme sizes). Truncating
/// division, matching the original `v * n / 100`.
#[inline]
fn pct(v: u16, n: u16) -> u16 {
    ((v as u32 * n as u32) / 100) as u16
}

/// Counter width that marks the LARGE (detailed kitchen) pantry sprite. The size
/// producer emits this width when the pantry room is wide enough; consumers test
/// `>= PANTRY_COUNTER_LARGE_W` rather than the bare `32` literal.
const PANTRY_COUNTER_LARGE_W: u16 = 32;

/// Y-position percentage of the pantry counter within its room — lower (65%) for
/// the large counter, a touch higher (60%) for the small one. SINGLE SOURCE: the
/// bistro-table clamp (which keeps that cluster clear of the counter) and the
/// counter's own waypoint placement both read it, so they cannot disagree — were
/// they to drift, the clamp would guard a phantom counter position.
fn pantry_counter_y_pct(counter_w: u16) -> u16 {
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        65
    } else {
        60
    }
}

/// Horizontal seat offsets for a 3-across sofa, relative to the middle-seat
/// anchor — shared by the 20px lounge couch and the meeting sofas so the two
/// can't drift.
const SEAT_DX: [i16; 3] = [-6, 0, 6];

/// Lounge-couch sprite origin (the middle-seat anchor). Single-sourced because
/// `compute_with_seed` (floor-lamp placement) and `compute_waypoints` (seat
/// waypoints + `couch_sprite_center`) both derive from it and must agree
/// byte-for-byte — recomputed via this fn rather than threaded as an `Option`
/// (no unwrap on a read-back).
fn couch_pos(cubicle_band: &Bounds, top_margin: u16) -> Point {
    Point {
        x: cubicle_band.x + pct(cubicle_band.width, 35),
        y: top_margin + 3,
    }
}

pub(super) fn compute_with_seed(
    buf_w: u16,
    buf_h: u16,
    num_agents: usize,
    floor_seed: u64,
) -> Option<SceneLayout> {
    const MIN_W: u16 = DESK_W + DESK_GAP_X * 2;
    let min_h: u16 = 40 + MIN_TOP_MARGIN;
    if buf_w < MIN_W || buf_h < min_h {
        return None;
    }

    let top_margin = pct(buf_h, 30).max(MIN_TOP_MARGIN);
    let usable_h = buf_h - top_margin;

    // Per-floor layout variant: floor_seed encodes floor_idx via
    // Fibonacci hashing (wrapping_mul with golden-ratio constant).
    // The variant hash constant was chosen so that the 5 standard
    // floor seeds (0..5 × FLOOR_SEED_MULTIPLIER) each map to a
    // unique variant in [0..5). There are only 5 hand-authored
    // geometries, so with MAX_FLOORS > 5 the higher floors cycle
    // through the same 5 looks (cosmetic repetition, not a bug).
    let floor_variant = (floor_seed.wrapping_mul(0x4737819096da1dad) % 5) as u8;

    // F1(0): Standard — meeting + pantry, vertical wall between them
    //        and the cubicle area, horizontal wall between meeting/pantry.
    // F2(1): Open plan — pantry only, no vertical wall (open kitchen
    //        corner, counter acts as divider). No meeting room.
    // F3(2): Dense — two meeting rooms (top + bottom), no pantry.
    //        Horizontal wall separates the two rooms. Each gets a door.
    // F4(3): Senior — larger meeting + pantry (like Standard but wider).
    // F5(4): Lounge — pantry only, no vertical wall (open break area).
    let (mut mid_x, has_meeting, mut has_pantry) = match floor_variant {
        0 => (pct(buf_w, 28), true, true),
        1 => (pct(buf_w, 18), false, true),
        2 => (pct(buf_w, 22), true, false),
        3 => (pct(buf_w, 35), true, true),
        _ => (pct(buf_w, 22), false, true),
    };
    // Open-plan floors (1, 4) have no vertical wall — the pantry
    // counter/furniture visually defines the zone boundary.
    let has_vertical_wall = has_meeting;
    // Dense floor (variant 2): two meeting rooms stacked vertically.
    // Only when tall enough for two rooms with furniture + door gaps.
    let has_dual_meeting = floor_variant == 2 && usable_h >= MIN_DUAL_MEETING_H;
    // Variant 2 (Dense) only earns its narrow 22% left column + no-pantry when
    // it actually fits TWO meeting rooms. On a terminal too short for that it
    // degrades fully to the Standard single-meeting+pantry geometry — same 28%
    // column width and pantry. The old degenerate fallback (22% wide, full-
    // height meeting, no pantry) was too narrow to enclose a room and sealed a
    // pocket at 96×70 (surfaced by the dense-variant small-size connectivity
    // sweep). Keeps the dual-meeting wall branch below for the real dense floor.
    if floor_variant == 2 && !has_dual_meeting {
        has_pantry = true;
        mid_x = pct(buf_w, 28);
    }

    let mid_y_split = top_margin + usable_h / 2;

    let meeting_room = if has_meeting {
        // A meeting always shares the left column with either the pantry or a
        // second meeting room (variant table: meeting-bearing variants 0/3 set
        // has_pantry, and variant 2 degrades to has_pantry when not dual) — so
        // the room takes the top HALF unconditionally. The else-arm (full
        // usable_h) was dead; assert the invariant so a future variant-table
        // edit fails loud instead of silently picking a full-height room.
        debug_assert!(
            has_pantry || has_dual_meeting,
            "meeting implies pantry-or-dual per the variant table"
        );
        Some(Bounds {
            x: 0,
            y: top_margin,
            width: mid_x,
            height: usable_h / 2,
        })
    } else {
        None
    };
    // Second meeting room for dense layout (below the first).
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
                usable_h - usable_h / 2
            } else {
                usable_h
            },
        })
    } else {
        None
    };

    let right_x = mid_x + 1;
    let right_w = buf_w.saturating_sub(right_x);
    let walkway_h = (usable_h / 10).max(8);
    let cubicle_h = usable_h.saturating_sub(walkway_h);
    let cubicle_band = Bounds {
        x: right_x,
        y: top_margin,
        width: right_w,
        height: cubicle_h,
    };
    let walkway = Bounds {
        x: right_x,
        y: top_margin + cubicle_h,
        width: right_w,
        height: walkway_h,
    };

    // 2×2 desk pods. Within a pod desks are tight (small intra-gap);
    // between pods we leave a wide aisle for decor + walkers. This
    // breaks the previously-uniform desk grid into team-like
    // clusters and frees up `pod_decor` slots in the aisles.
    let pod_w = POD_SIDE * DESK_W + (POD_SIDE - 1) * INTRA_POD_GAP_X;
    let pod_h = POD_SIDE * DESK_H + (POD_SIDE - 1) * INTRA_POD_GAP_Y;
    let pod_stride_x = pod_w + INTER_POD_AISLE_X;
    let pod_stride_y = pod_h + INTER_POD_AISLE_Y;
    // Extra padding between the viewing couch (top of cubicle area)
    // and the first row of pods. Scales with buf_h so taller
    // terminals get more breathing room.
    let couch_to_desk_extra = buf_h.saturating_sub(60) / 20;
    let pod_cols = ((right_w.saturating_sub(INTER_POD_AISLE_X / 2)) / pod_stride_x).max(1);
    let pod_rows = {
        let raw = ((cubicle_h.saturating_sub(couch_to_desk_extra) + INTER_POD_AISLE_Y)
            / pod_stride_y)
            .max(1);
        let max_pods = MAX_VISIBLE_DESKS as u16 / (POD_SIDE * POD_SIDE);
        let total_pods = (pod_cols * raw).min(max_pods);
        total_pods.div_ceil(pod_cols).min(raw)
    };
    let pod_grid = PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        stride_x: pod_stride_x,
        stride_y: pod_stride_y,
        couch_to_desk_extra,
    };

    let home_desks = compute_pod_desks(num_agents, &cubicle_band, pod_grid);

    let pod_decor = compute_pod_decor(&cubicle_band, pod_grid, floor_seed);

    // One source of truth: the meeting-sofa SPRITE height (was a bare `7`). A
    // hardcoded literal would silently let 1px-too-short rooms pass the gate
    // below if the sprite ever grows → MeetingSofa seat teleport on the coarse
    // grid. furniture_def is a const fn returning Copy.
    let sofa_h = furniture_def(Furniture::MeetingSofaBody).visual.h;
    // A meeting room narrower than this can't host the 16-px-wide sofa body
    // (+ its 2-px pad) with enough walkable margin for the coarse 4×4 router to
    // reach the seats buried in the sofa — find_path returns None and an idle
    // agent sent there TELEPORTS (route() falls back to a straight line). Below
    // it the room degrades to bare floor (no sofa/table/seats), the same
    // graceful degradation the dense floor uses when too short. The threshold
    // is validated by the routability sweep
    // `meeting_and_pantry_waypoints_are_routable_on_the_coarse_grid`.
    const MEETING_FURNITURE_MIN_W: u16 = 30;
    let room_fits_furniture =
        |mr: &Bounds| mr.width >= MEETING_FURNITURE_MIN_W && mr.height >= sofa_h * 2;
    // One source for a meeting room's furniture trio: two facing sofas and the
    // table CENTERED BETWEEN THEM. The table used to sit at the room centre while
    // the sofas sat at 30%/80% of the room height — asymmetric, so the north
    // sofa's front was packed against the table (a sub-coarse-grid seam that cost
    // its seats their front approach) while the south sofa had clearance. Placing
    // the table at the sofa midpoint gives both fronts equal, routable clearance.
    // The sofas keep their 30%/80% bias (backrest clearance from the room's top/
    // bottom walls); only the table follows them. All positions are
    // window-height-driven, so the approach points the agents path to derive from
    // the resulting mask at every size — nothing here is a fixed pixel offset.
    let room_furniture = |mr: &Bounds| -> ([Point; 2], Point) {
        let cx = mr.x + mr.width / 2;
        // Sofas sit SYMMETRICALLY about the room mid-line (20%/80%, was 30%/80%)
        // so each gets equal front clearance to the centred table — the old 30%
        // packed the north sofa's front against the table. Clamps mirror each
        // other: north ≥ sofa_h from the top wall, south ≤ sofa_h from the bottom,
        // so neither backrest clips its wall in a short room.
        let north_y = (mr.y + pct(mr.height, 20)).max(mr.y + sofa_h);
        let south_y = (mr.y + pct(mr.height, 80)).min(mr.y + mr.height.saturating_sub(sofa_h));
        let sofas = [Point { x: cx, y: north_y }, Point { x: cx, y: south_y }];
        let table = Point {
            x: cx,
            y: (north_y + south_y) / 2,
        };
        (sofas, table)
    };
    let mut meeting_sofas: Vec<Point> = Vec::new();
    let mut meeting_table_vec: Vec<Point> = Vec::new();
    // Order is load-bearing: room 0 = `meeting_room`, room 1 = `meeting_room_2`
    // (dense layout). `compute_waypoints` keys seats to a table by this index.
    for room in [meeting_room, meeting_room_2] {
        if let Some(mr) = room.filter(&room_fits_furniture) {
            let (sofas, table) = room_furniture(&mr);
            meeting_sofas.extend(sofas);
            meeting_table_vec.push(table);
        }
    }
    let meeting_tables = meeting_table_vec;

    let room_walls = compute_room_walls(
        RoomPresence {
            has_vertical_wall,
            has_dual_meeting,
            has_meeting,
            has_pantry,
        },
        mid_x,
        mid_y_split,
        top_margin,
        usable_h,
    );

    // Counter footprint depends on pantry width — 32×10 detailed
    // kitchen on default terminals, 20×8 compact fallback for narrow
    // ones. The threshold (36 = 32 sprite + 4 px margins) keeps the
    // walkable strip around the counter wide enough for routing.
    let pantry_counter_size: Size = match pantry_room {
        Some(pr) if pr.width >= 36 => Size {
            w: PANTRY_COUNTER_LARGE_W,
            h: 10,
        },
        _ => Size { w: 20, h: 8 },
    };

    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(&cubicle_band, top_margin);

    let (waypoints, couch_sprite_center) = compute_waypoints(
        &cubicle_band,
        top_margin,
        pantry_room,
        pantry_counter_size,
        &pod_decor,
        &walkway,
        MeetingFurniture {
            sofas: &meeting_sofas,
            tables: &meeting_tables,
        },
    );

    // Plants scatter through the cubicle corridor edges + pantry.
    // No plants in the cubicle TOP strip — that area is too narrow
    // (the gap between top wall and the viewing couch is just 7 px,
    // not enough for a padded plant without blocking the room/door
    // walkability paths). No plants in the meeting room interior
    // either: sofas + table already fill most of the room, and any
    // plant inside its walkable strips disconnects the door gap.
    let plants: Vec<PlantItem> = vec![
        // Corridor edges — far from any door or room exit.
        PlantItem {
            kind: PlantKind::Flower,
            pos: Point {
                x: cubicle_band.x + 4,
                y: walkway.y.saturating_sub(4),
            },
        },
        PlantItem {
            kind: PlantKind::Succulent,
            pos: Point {
                x: cubicle_band.x + cubicle_band.width.saturating_sub(4),
                y: walkway.y.saturating_sub(4),
            },
        },
    ]
    .into_iter()
    // No pantry plants — the room is small (≤ 26 px wide), and the
    // plant + 1-px pad blocks the only horizontal bridge between the
    // pantry interior and the cubicle area's bottom row. Leaving the
    // pantry plant-free keeps the mask fully connected.
    .chain(std::iter::empty::<PlantItem>())
    // Two meeting-room corner plants on the west wall, well clear of
    // the door (which is on the east wall) and the central
    // sofa/table column. Only added when the meeting room is large
    // enough (≥ 30 px wide) that the plant + pad doesn't squeeze the
    // walkable strip below routable width.
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

    // Floor lamp now sits right next to the viewing couch so its halo
    // bathes the seating area at night.
    let floor_lamp = Some(Point {
        x: couch_x + 9,
        y: couch_y + 2,
    });

    // Lounge side table on the OPPOSITE side from the floor lamp
    // (west of the couch). Clamp its x so the footprint's left edge clears the
    // vertical room wall at `right_x` — at the minimum buffer width couch_x-10
    // would otherwise drop the 7-wide footprint onto the wall column.
    let side_half_w = furniture_def(Furniture::LoungeSideTable)
        .footprint
        .map_or(0, |s| s.w / 2);
    let lounge_side_table = Some(Point {
        x: couch_x.saturating_sub(10).max(right_x + side_half_w + 1),
        y: couch_y + 2,
    });

    // Elevator door — 16×14 sprite mounted in the back wall, slotted
    // into the rightmost window position and BOTTOM-aligned with the
    // floor-to-ceiling windows so both sit on the same wall plane.
    // Windows span y=1 to y=top_wall_h-3 inside the wall band; the
    // elevator's bottom row lands at that same y. (`top_wall_h =
    // top_margin - WALL_BAND_TO_TOP_MARGIN`, the one const the renderer's
    // pre-pass and the mask both read so they can't drift.) Requires ≥ 20 px
    // of width to even fit the sprite + margin. ELEVATOR_W / ELEVATOR_H are the
    // shared core consts (read by the renderer too — see layout/mod.rs).
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
    // Spawn point on the floor right outside the elevator's centre:
    // characters walk from here to their desk. Y is 4 px south of
    // the wall edge so the character clears the elevator threshold
    // before pathing.
    let door_threshold = door.map(|d| Point {
        x: d.x + ELEVATOR_W / 2,
        y: top_margin + 4,
    });

    // Wall decor anchored to the BOTTOM of the wall band so the sprites
    // sit "below the windows" no matter how tall the wall band grows.
    // Hardcoded y=6/8 (like the old code) leaves bookshelf + bulletin
    // floating in the sky on tall terminals where the window glass
    // auto-stretches into the wall band.
    //
    // Sprite heights:
    //   bookshelf:      12 px
    //   bulletin_board: 6 px
    //   exit_sign:      ~6 px (already used top_margin - 13 — kept)
    // We position the TOP-LEFT corner of each sprite so its bottom
    // row lands exactly at `top_margin - 1` (last wall band row).
    let mut wall_decor = vec![
        WallDecorItem {
            kind: WallDecor::Bookshelf,
            pos: Point {
                x: pct(buf_w, 18),
                y: top_margin.saturating_sub(12),
            },
        },
        WallDecorItem {
            kind: WallDecor::ExitSign,
            pos: Point {
                x: buf_w.saturating_sub(9),
                y: top_margin.saturating_sub(13),
            },
        },
    ];
    if has_meeting || has_pantry {
        wall_decor.push(WallDecorItem {
            kind: WallDecor::Whiteboard,
            pos: Point {
                x: mid_x + 3,
                y: top_margin + usable_h / 3,
            },
        });
    }
    if let Some(mr) = meeting_room {
        wall_decor.push(WallDecorItem {
            kind: WallDecor::MeetingScreen,
            pos: Point {
                x: mr.x + (mr.width / 2).saturating_sub(7),
                y: top_margin.saturating_sub(12),
            },
        });
    }

    let (pantry_table, pantry_chairs) = if let Some(pr) = pantry_room {
        // Inset the bistro table + stools clear of the room walls. A small
        // pantry puts the raw 25% mark inside the meeting/pantry divider wall
        // (caught by `furniture_does_not_overlap_room_walls`). Clearance = wall
        // face + obstacle pad; the cluster half-extent is 5×4 (table 8×5 plus
        // the ±4 / ±3 stool reach).
        let clr = super::WALL_THICK_H + super::OBSTACLE_PAD_PX;
        let (half_w, half_h) = (5u16, 4u16);
        let min_x = pr.x + clr + half_w;
        let max_x = (pr.x + pr.width).saturating_sub(clr + half_w);
        let min_y = pr.y + clr + half_h;
        // The counter sits lower in the room (60/65%); keep the table cluster's
        // padded south edge above the counter's padded north so the two
        // footprints don't merge into a band that closes the east routing strip
        // in a short pantry (was unreachable at 120×80, outside the old matrix).
        let counter_y = pr.y + pct(pr.height, pantry_counter_y_pct(pantry_counter_size.w));
        let counter_north =
            counter_y.saturating_sub(pantry_counter_size.h / 2 + super::OBSTACLE_PAD_PX);
        let max_y = (pr.y + pr.height)
            .saturating_sub(clr + half_h)
            .min(counter_north.saturating_sub(half_h));
        let tx = (pr.x + pct(pr.width, 25)).clamp(min_x, max_x.max(min_x));
        let ty = (pr.y + pct(pr.height, 25)).clamp(min_y, max_y.max(min_y));
        (
            Some(Point { x: tx, y: ty }),
            vec![
                Point {
                    x: tx.saturating_sub(4),
                    y: ty,
                },
                Point { x: tx + 4, y: ty },
                Point {
                    x: tx,
                    y: ty.saturating_sub(3),
                },
                Point { x: tx, y: ty + 3 },
            ],
        )
    } else {
        (None, vec![])
    };

    let corridor = Some(Bounds {
        x: 0,
        y: walkway.y,
        width: buf_w,
        height: walkway.height,
    });

    let walkable = mask::build_walkable_mask(
        buf_w,
        buf_h,
        top_margin,
        door,
        &home_desks,
        &meeting_sofas,
        &meeting_tables,
        pantry_table,
        &pantry_chairs,
        &waypoints,
        &plants,
        floor_lamp,
        lounge_side_table,
        &wall_decor,
        &pod_decor,
        &room_walls,
        pantry_counter_size,
    );

    // Coarse reachable component, seeded from the door (where agents enter, so
    // always in the main component); fall back to a home desk, then buffer
    // centre. `snap_seed` pulls a blocked seed into the adjacent component.
    let reachable = ReachSet::from_mask(
        &walkable,
        door_threshold
            .or_else(|| home_desks.first().copied())
            .unwrap_or(Point {
                x: buf_w / 2,
                y: buf_h / 2,
            }),
    );

    Some(SceneLayout {
        buf_w,
        buf_h,
        cubicle_band,
        walkway,
        home_desks,
        waypoints,
        plants,
        wall_decor,
        pod_decor,
        floor_lamp,
        lounge_side_table,
        door,
        door_threshold,
        meeting_room,
        pantry_room,
        meeting_sofas,
        meeting_tables,
        room_walls,
        top_margin,
        pantry_table,
        pantry_chairs,
        pantry_counter_size,
        corridor,
        couch_sprite_center,
        walkable,
        reachable,
    })
}

/// 2×2-pod grid geometry shared by [`compute_pod_desks`] + [`compute_pod_decor`].
/// `right_x`/`right_w`/`cubicle_h` are NOT carried — they equal the cubicle
/// band's `.x`/`.width`/`.height` and are derived in-body from the `&Bounds`.
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
    /// cubicle band. The single formula the desk-placement and aisle-decor passes
    /// both step from — golden snapshots pin its byte-exact output.
    fn pod_origin(self, cubicle_band: &Bounds, pod_c: u16, pod_r: u16) -> (u16, u16) {
        let x = cubicle_band.x + INTER_POD_AISLE_X / 2 + pod_c * self.stride_x;
        let y = cubicle_band.y
            + INTER_POD_AISLE_Y / 2
            + self.couch_to_desk_extra
            + pod_r * self.stride_y;
        (x, y)
    }
}

/// Which rooms the floor has — drives [`compute_room_walls`]'s segment set.
#[derive(Clone, Copy)]
pub(super) struct RoomPresence {
    has_vertical_wall: bool,
    has_dual_meeting: bool,
    has_meeting: bool,
    has_pantry: bool,
}

/// A meeting room's furniture in lockstep (2 sofas + 1 table per room).
#[derive(Clone, Copy)]
pub(super) struct MeetingFurniture<'a> {
    sofas: &'a [Point],
    tables: &'a [Point],
}

/// Pod-grid desk placement: full pods, partial columns at right edge,
/// partial row at bottom edge.
pub(super) fn compute_pod_desks(
    num_agents: usize,
    cubicle_band: &Bounds,
    grid: PodGrid,
) -> Vec<Point> {
    let right_x = cubicle_band.x;
    let right_w = cubicle_band.width;
    let cubicle_h = cubicle_band.height;
    let PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        stride_x: pod_stride_x,
        stride_y: pod_stride_y,
        couch_to_desk_extra,
    } = grid;
    let n = num_agents.min(MAX_VISIBLE_DESKS);
    let mut home_desks = Vec::with_capacity(n);
    // Clamp: a desk must fit entirely inside the cubicle band.
    // Without this, the last intra-pod row of a bottom pod can
    // extend past cubicle_band into the walkway (the pod_rows
    // formula counts strides between origins but not the final
    // pod's tail height).
    let desk_y_max = cubicle_band.y + cubicle_band.height - DESK_H;
    let push_desk = |desks: &mut Vec<Point>, x: u16, y: u16| -> bool {
        if desks.len() >= n || y > desk_y_max {
            return desks.len() >= n;
        }
        desks.push(Point { x, y });
        false
    };

    // Full pods (row-major fill).
    'outer: for pod_r in 0..pod_rows {
        for pod_c in 0..pod_cols {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            for r in 0..POD_SIDE {
                for c in 0..POD_SIDE {
                    let full = push_desk(
                        &mut home_desks,
                        pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                        pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                    );
                    if full {
                        break 'outer;
                    }
                }
            }
        }
    }

    // Partial pod columns at the RIGHT edge — for each leftover
    // strip after `pod_cols` full pods wide enough for a single
    // desk column + half-aisle, append another 1×POD_SIDE partial
    // column. Resolves the "office looks empty on the right" issue
    // at wide buffers where a full 2nd pod doesn't fit but multiple
    // single-desk columns do.
    let main_pod_used_w = INTER_POD_AISLE_X / 2 + pod_cols * pod_stride_x;
    let residual_w = right_w.saturating_sub(main_pod_used_w);
    let partial_col_stride = DESK_W + INTER_POD_AISLE_X / 2;
    let partial_col_count = (residual_w / partial_col_stride).min(4);
    let partial_col_at_right = partial_col_count > 0;
    let partial_col_x = |i: u16| -> u16 {
        right_x + main_pod_used_w + INTER_POD_AISLE_X / 2 + i * partial_col_stride
    };
    if partial_col_at_right {
        'partial_x: for pod_r in 0..pod_rows {
            let (_, pod_origin_y) = grid.pod_origin(cubicle_band, 0, pod_r);
            for r in 0..POD_SIDE {
                for i in 0..partial_col_count {
                    let full = push_desk(
                        &mut home_desks,
                        partial_col_x(i),
                        pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                    );
                    if full {
                        break 'partial_x;
                    }
                }
            }
        }
    }

    // Partial pod ROW at the BOTTOM edge — same idea but vertical.
    // Adds POD_SIDE × pod_cols extra desks (+ the partial column's
    // single desk if it also fits).
    let main_pod_used_h = INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_rows * pod_stride_y;
    let residual_h = cubicle_h.saturating_sub(main_pod_used_h);
    let partial_row_at_bottom = residual_h >= DESK_H + INTER_POD_AISLE_Y / 2;
    if partial_row_at_bottom {
        let partial_y = cubicle_band.y + main_pod_used_h + INTER_POD_AISLE_Y / 2;
        'partial_y: for pod_c in 0..pod_cols {
            let (pod_origin_x, _) = grid.pod_origin(cubicle_band, pod_c, 0);
            for c in 0..POD_SIDE {
                let full = push_desk(
                    &mut home_desks,
                    pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                    partial_y,
                );
                if full {
                    break 'partial_y;
                }
            }
        }
        for i in 0..partial_col_count {
            let full = push_desk(&mut home_desks, partial_col_x(i), partial_y);
            if full {
                break;
            }
        }
    }

    home_desks
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
    // Cycle through ALL with a per-slot counter so every decor type
    // appears at least once before any repeats. Beats the prior
    // golden-ratio hash which (empirically) never picked Tv or
    // PhoneBooth at common buffer sizes — slots were stuck on
    // PlantTall / Whiteboard / StandingDesk.
    let mut slot_idx: usize = (floor_seed % 7) as usize;
    let mut push_slot = |pod_decor: &mut Vec<PodDecorItem>, x: u16, y: u16| {
        let kind = PodDecor::ALL[slot_idx % PodDecor::ALL.len()];
        slot_idx += 1;
        pod_decor.push(PodDecorItem {
            kind,
            pos: Point { x, y },
        });
    };
    // Vertical-aisle slots (between column pod_c and pod_c+1, one
    // per pod row).
    for pod_r in 0..pod_rows {
        for pod_c in 0..pod_cols.saturating_sub(1) {
            let (pod_origin_x, pod_origin_y) = grid.pod_origin(cubicle_band, pod_c, pod_r);
            let aisle_cx = pod_origin_x + pod_w + INTER_POD_AISLE_X / 2;
            let aisle_cy = pod_origin_y + pod_h / 2;
            push_slot(&mut pod_decor, aisle_cx, aisle_cy);
        }
    }
    // Horizontal-aisle slots (between row pod_r and pod_r+1, one
    // per pod column).
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

/// Wall segments with door gaps for meeting/pantry rooms.
pub(super) fn compute_room_walls(
    rooms: RoomPresence,
    mid_x: u16,
    mid_y_split: u16,
    top_margin: u16,
    usable_h: u16,
) -> Vec<WallSegment> {
    let RoomPresence {
        has_vertical_wall,
        has_dual_meeting,
        has_meeting,
        has_pantry,
    } = rooms;
    // Doorway widths are ABSOLUTE pixels — using percentages here makes
    // the gap shrink to zero on smaller terminals, which after the
    // 2-px wall obstacle padding leaves no walkable cell for A* and
    // disconnects the meeting room from the rest of the office.
    //
    // Minimums: 14 px gives ≥10 px effective gap after padding, which
    // is wide enough that the coarsened 4×4 router grid has at least
    // one row of walkable cells through the doorway.
    const DOOR_GAP_V: u16 = 14;
    const DOOR_GAP_H: u16 = 14;
    let mut room_walls = Vec::new();
    // Vertical wall: only when we have enclosed rooms (meeting or
    // meeting+pantry). Open-plan/lounge pantry-only floors skip it
    // — the counter is the visual boundary.
    if has_vertical_wall {
        // has_vertical_wall == has_meeting (compute_with_seed), and a meeting
        // always shares the left column with the pantry or a second meeting room
        // per the variant table — so the vertical wall always stops at the
        // mid-height split. The else-arm (full usable_h) was dead; assert the
        // invariant (mirrors the meeting_room height assert) so a future
        // variant-table edit fails loud instead of silently picking a full wall.
        debug_assert!(
            has_pantry || has_dual_meeting,
            "meeting implies pantry-or-dual per the variant table"
        );
        let v_x = mid_x;
        let v_top = top_margin;
        let v_bot = mid_y_split;
        let v_door_center = top_margin + (v_bot - v_top) / 2;
        let v_door_top = v_door_center.saturating_sub(DOOR_GAP_V / 2);
        let v_door_bot = (v_door_center + DOOR_GAP_V / 2).min(v_bot);
        room_walls.push(WallSegment {
            start: Point { x: v_x, y: v_top },
            end: Point {
                x: v_x,
                y: v_door_top,
            },
        });
        room_walls.push(WallSegment {
            start: Point {
                x: v_x,
                y: v_door_bot,
            },
            end: Point { x: v_x, y: v_bot },
        });
        // Second meeting room or pantry below: extend wall with
        // its own door gap.
        if has_dual_meeting {
            // Second meeting room: extend wall below horizontal.
            // Start below the horizontal wall (its thickness + pad). This
            // offset MUST stay within the renderer's bridge-up tolerance
            // (`WALL_THICK_H_PX + 2` in `stitch_vertical_wall`) or this lower
            // segment renders as a detached strip below the cross wall.
            let v2_top = mid_y_split + 6;
            let v2_bot = top_margin + usable_h;
            let v2_center = v2_top + (v2_bot - v2_top) / 2;
            let v2_door_top = v2_center.saturating_sub(DOOR_GAP_V / 2);
            let v2_door_bot = (v2_center + DOOR_GAP_V / 2).min(v2_bot);
            room_walls.push(WallSegment {
                start: Point { x: v_x, y: v2_top },
                end: Point {
                    x: v_x,
                    y: v2_door_top,
                },
            });
            room_walls.push(WallSegment {
                start: Point {
                    x: v_x,
                    y: v2_door_bot,
                },
                end: Point { x: v_x, y: v2_bot },
            });
        }
    }
    // Horizontal wall: separates meeting from pantry, or two meetings.
    let h_y = mid_y_split;
    let h_door_center = pct(mid_x, 60);
    let h_door_left = h_door_center.saturating_sub(DOOR_GAP_H / 2);
    let h_door_right = (h_door_center + DOOR_GAP_H / 2).min(mid_x);
    if (has_meeting && has_pantry) || has_dual_meeting {
        room_walls.push(WallSegment {
            start: Point { x: 0, y: h_y },
            end: Point {
                x: h_door_left,
                y: h_y,
            },
        });
        room_walls.push(WallSegment {
            start: Point {
                x: h_door_right,
                y: h_y,
            },
            end: Point { x: mid_x, y: h_y },
        });
    }
    room_walls
}

/// Waypoints: couch, pantry, pod-decor-promoted (PhoneBooth/StandingDesk),
/// corridor appliances (VendingMachine/Printer).
pub(super) fn compute_waypoints(
    cubicle_band: &Bounds,
    top_margin: u16,
    pantry_room: Option<Bounds>,
    pantry_counter_size: Size,
    pod_decor: &[PodDecorItem],
    walkway: &Bounds,
    meeting: MeetingFurniture<'_>,
) -> (Vec<Waypoint>, Option<Point>) {
    let right_x = cubicle_band.x;
    let right_w = cubicle_band.width;
    let MeetingFurniture {
        sofas: meeting_sofas,
        tables: meeting_tables,
    } = meeting;
    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(cubicle_band, top_margin);
    // Lounge couch: 3 seats across the 20px sofa (dx ∈ {-6, 0, +6}), matching
    // the meeting sofa. room_id stays None — the lounge's group-chat grouping
    // is keyed at the chitchat venue layer (all couch seats share one venue),
    // NOT via the meeting-only room_id field. The sprite paints once, centred
    // on couch_x (the middle seat); see `couch_sprite_center`.
    let mut waypoints: Vec<Waypoint> = SEAT_DX
        .into_iter()
        .map(|dx| Waypoint {
            pos: Point {
                x: couch_x.saturating_add_signed(dx),
                y: couch_y,
            },
            kind: WaypointKind::Couch,
            // SEATED facing: the sitter looks NORTH at the window (→ back_couch
            // sprite). The APPROACH side is decoupled (Furniture::Couch uses
            // ApproachSides::ALL — the agent walks up from the south/lounge,
            // whose front is the window WALL); see decor.rs Couch row.
            facing: Facing::North,
            room_id: None,
        })
        .collect();
    if let Some(pr) = pantry_room {
        // Clamp x so the counter fits within pantry_room. Without this
        // the counter (32px or 20px wide) extends past the east wall
        // into the cubicle band at small buffer widths.
        let half_cw = pantry_counter_size.w / 2;
        let max_cx = pr.x + pr.width.saturating_sub(half_cw + 1);
        // y is single-sourced with the bistro-table clamp; only x is size-shaped
        // (large counter is room-centred, small one sits at 60% width).
        let wy = pr.y + pct(pr.height, pantry_counter_y_pct(pantry_counter_size.w));
        let wx = if pantry_counter_size.w >= PANTRY_COUNTER_LARGE_W {
            (pr.x + pr.width / 2).min(max_cx)
        } else {
            (pr.x + pct(pr.width, 60)).min(max_cx)
        };
        waypoints.push(Waypoint {
            pos: Point { x: wx, y: wy },
            kind: WaypointKind::Pantry,
            facing: Facing::South,
            room_id: None,
        });
    }
    // Interactive pod-aisle decor -> also waypoints. PhoneBooth and
    // StandingDesk are workstation-like destinations agents can
    // wander to during Idle cycles. Plant/Whiteboard/TV are pure
    // decor (already obstacles via pod_decor).
    for &PodDecorItem { kind, pos } in pod_decor {
        // Exhaustive (no `_`): a NEW PodDecor must make a deliberate
        // wander-destination decision here — `None` = pure decor (aisle
        // obstacle only), `Some(kind)` = also a walkable destination. A `_`
        // would silently leave a new interactive kind unreachable.
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

    // Corridor appliances — stored as centre points (same convention
    // as Pantry/Couch). Painter derives top-left via sub(w/2, h/2).
    // Sizes: vending 4×6, printer 5×4.
    if walkway.height >= 10 && walkway.width > 30 {
        waypoints.push(Waypoint {
            pos: Point {
                x: right_x + 5,
                y: walkway.y + 3,
            },
            kind: WaypointKind::VendingMachine,
            facing: Facing::South,
            room_id: None,
        });
    }
    if walkway.height >= 9 && right_w > 40 {
        waypoints.push(Waypoint {
            pos: Point {
                x: right_x + right_w.saturating_sub(10),
                y: walkway.y + 2,
            },
            kind: WaypointKind::Printer,
            facing: Facing::South,
            room_id: None,
        });
    }

    // Meeting-room slots. Sofas are stored north→south per room (2 per
    // room, see `meeting_sofas` assembly); each seats up to 3 agents
    // (dx ∈ {-6, 0, +6} along the 20px sofa) facing the table. Two standing
    // slots flank each table. Every slot in a room shares its `room_id` so the
    // group-chitchat venue keys on the room, not the individual seat.
    for (i, sofa) in meeting_sofas.iter().enumerate() {
        let room_id = i / 2;
        // Lockstep invariant: 2 sofas + 1 table per room (see meeting_sofas /
        // meeting_tables assembly), so room_id < meeting_tables.len() always.
        // The map_or fallback below is therefore dead; assert it so a future
        // break (e.g. a 3rd sofa, conditional table) surfaces loudly instead of
        // silently flipping a sofa's facing.
        debug_assert!(
            room_id < meeting_tables.len(),
            "meeting sofa/table lockstep broken: sofa {i} -> room {room_id} but {} tables",
            meeting_tables.len()
        );
        let table_y = meeting_tables.get(room_id).map_or(sofa.y, |t| t.y);
        // North-of-table sofa faces South (front toward the viewer); the
        // south sofa faces North (back toward the viewer) — the pair reads
        // as two people facing each other across the table.
        let facing = if sofa.y < table_y {
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
    for (room_id, table) in meeting_tables.iter().enumerate() {
        // West stand faces East (toward the table centre); east stand faces West.
        // The table obstacle (mask.rs) is `mark_blocked(t.x-5, w=11, pad=2)` →
        // blocks x ∈ [t.x-7, t.x+7] (symmetric, 7 px each side). West stand at
        // t.x-9 clears by 2 px; east stand at t.x+8 clears by 1 px. (The -9 keeps
        // margin for any future footprint bump — leave it even though -8 would
        // also clear today.)
        for (dx, facing) in [(-9i16, Facing::East), (8, Facing::West)] {
            waypoints.push(Waypoint {
                pos: Point {
                    x: table.x.saturating_add_signed(dx),
                    y: table.y,
                },
                kind: WaypointKind::MeetingStand,
                facing,
                room_id: Some(room_id),
            });
        }
    }

    // Load-bearing invariant for chitchat venue grouping: a waypoint carries a
    // `room_id` IFF it is a meeting slot. A non-meeting waypoint with a stray
    // `room_id` would mis-group into a meeting venue; a meeting slot without one
    // would never group. Enforced here at the single construction site.
    debug_assert!(
        waypoints.iter().all(|w| {
            matches!(
                w.kind,
                WaypointKind::MeetingSofa | WaypointKind::MeetingStand
            ) == w.room_id.is_some()
        }),
        "room_id must be Some exactly for meeting-slot waypoints"
    );

    (
        waypoints,
        Some(Point {
            x: couch_x,
            y: couch_y,
        }),
    )
}
