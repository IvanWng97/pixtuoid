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
/// `>= PANTRY_COUNTER_LARGE_W` rather than the bare `32` literal (`pub` + re-exported
/// from `layout` so the painter's `use_large` selector and the binary's coffee
/// hit-test share this one source instead of re-hardcoding 32).
pub const PANTRY_COUNTER_LARGE_W: u16 = 32;

/// Y-position percentage of the pantry counter within its room — lower (65%) for
/// the large counter, a touch higher (60%) for the small one. SINGLE SOURCE: the
/// island clamp (which keeps the island clear of the counter) and the
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

/// The smallest buffer `compute_with_seed` lays out — below either bound it
/// returns `None` ("terminal too small"). Module-scoped (not fn-local) so the
/// placement sweep's None-arm asserts against THE SAME authority: a `None` at
/// a size at-or-above these bounds is a regression, not a legitimate refusal.
pub(super) const MIN_LAYOUT_W: u16 = DESK_W + DESK_GAP_X * 2;
pub(super) const MIN_LAYOUT_H: u16 = 40 + MIN_TOP_MARGIN;

pub(super) fn compute_with_seed(
    buf_w: u16,
    buf_h: u16,
    max_desks: Option<usize>,
    floor_seed: u64,
) -> Option<SceneLayout> {
    if buf_w < MIN_LAYOUT_W || buf_h < MIN_LAYOUT_H {
        return None;
    }

    let top_margin = pct(buf_h, 30).max(MIN_TOP_MARGIN);
    let usable_h = buf_h - top_margin;

    // Per-floor layout variant: `floor_seed` selects one of the 5 hand-authored
    // geometries via Fibonacci hashing (see `FloorVariant::from_seed`). With
    // MAX_FLOORS > 5 the higher floors cycle through the same looks (cosmetic
    // repetition, not a bug).
    let variant = FloorVariant::from_seed(floor_seed);
    let has_meeting = variant.has_meeting();
    // (The vertical-wall presence == has_meeting; open-plan OpenPlan/Lounge floors
    // skip it — the pantry counter is the boundary. compute_room_walls reads it
    // off `geom`, so no separate local here.)
    // Dense: two meeting rooms stacked vertically, ONLY when tall enough for two
    // rooms with furniture + door gaps. This is the ONE size-dependent bit; every
    // other geometry choice is a const of the variant.
    let has_dual_meeting = variant == FloorVariant::Dense && usable_h >= MIN_DUAL_MEETING_H;
    let geom = FloorGeometry {
        variant,
        has_dual_meeting,
    };
    // Dense only earns its narrow 22% left column + no-pantry when it actually
    // fits TWO meeting rooms; on a terminal too short for that it degrades fully
    // to the Standard single-meeting+pantry geometry (28% column + pantry). The
    // old degenerate fallback (22% wide, full-height meeting, no pantry) was too
    // narrow to enclose a room and sealed a pocket at 96×70 (surfaced by the
    // dense-variant small-size connectivity sweep). `FloorGeometry::{has_pantry,
    // mid_x_pct}` fold in that degrade; the dual-meeting wall branch below handles
    // the real dense floor.
    let has_pantry = geom.has_pantry();
    let mid_x = pct(buf_w, geom.mid_x_pct());

    // Counter footprint depends on pantry width — 32×10 detailed kitchen on
    // default terminals, 20×8 compact fallback for narrow ones. The threshold
    // (36 = 32 sprite + 4 px margins) keeps the walkable strip around the
    // counter wide enough for routing. Width-only (the pantry's width IS
    // mid_x), so it's known before the room split below prices the pantry's
    // content against it.
    let pantry_counter_size: Size = if has_pantry && mid_x >= 36 {
        Size {
            w: PANTRY_COUNTER_LARGE_W,
            h: 10,
        }
    } else {
        Size { w: 20, h: 8 }
    };

    // One source of truth: the meeting-sofa SPRITE height (was a bare `7`). A
    // hardcoded literal would silently let 1px-too-short rooms pass the fit
    // gate if the sprite ever grows → MeetingSofa seat teleport on the coarse
    // grid. furniture_def is a const fn returning Copy.
    let sofa_h = furniture_def(Furniture::MeetingSofaBody).visual.h;
    // Height must price the TABLE between the sofas, not just the two sofa
    // bodies: with both sofa clamps bound (short room) the mirror positions
    // leave `height − 2·sofa_h` between the sofa centres, and the centred
    // table needs its own footprint depth plus the sofa's not to overlap
    // either body (placement-sweep catch: at 96×60 the table ground clipped
    // BOTH sofas by a row). Derived from the same table rows the mask stamps.
    let sofa_fp_h = furniture_def(Furniture::MeetingSofaBody)
        .footprint
        .map_or(0, |s| s.h);
    let table_fp_h = furniture_def(Furniture::MeetingTable)
        .footprint
        .map_or(0, |s| s.h);
    let trio_fit_h = sofa_h * 2 + sofa_fp_h + table_fp_h;

    // Meeting-room height: CONTENT-FIT, donating the surplus to the pantry
    // below. The screen + bookshelf hang on the top WALL BAND (zero floor
    // rows), so the room needs only its sofa/table trio; the old
    // unconditional half-split left the trio floating in empty floor on
    // short terminals while the pantry below starved (island + snack shelf
    // y-refused). The donation is ALL-OR-NOTHING: the room shrinks exactly
    // to `usable_h − pantry_content_h` when that both keeps the trio fit
    // (`trio_fit_h`) AND actually reaches the pantry's content height
    // (`pantry_content_h` — the inverse of the island's y-clamps: the
    // counter line sits at pct(h, pantry_counter_y_pct) and the island needs
    // `island_need` rows of ceiling above it; the snack shelf's bound is
    // identical, its half-height 5 == the island's half_h + pad). Otherwise
    // the old half-split stands — a partial donation would cram the trio to
    // its fit gate to buy rows the island still couldn't use, and floors
    // already tall enough keep their exact pre-change geometry. Dense keeps
    // the raw split: BOTH halves host a trio. Drift guard vs the island
    // block's own clamps: `meeting_room_donates_surplus_height_to_the_pantry`.
    let island_clr = super::WALL_THICK_H + super::OBSTACLE_PAD_PX;
    let island_half_h = furniture_def(Furniture::KitchenIsland).visual.h / 2;
    let island_need = island_clr
        + 2 * (island_half_h + super::OBSTACLE_PAD_PX)
        + 1
        + pantry_counter_size.h / 2
        + super::OBSTACLE_PAD_PX;
    let pantry_content_h = (u32::from(island_need) * 100)
        .div_ceil(u32::from(pantry_counter_y_pct(pantry_counter_size.w)))
        as u16;
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
        // second meeting room (variant table: meeting-bearing variants 0/3 set
        // has_pantry, and variant 2 degrades to has_pantry when not dual) — so
        // the room takes the top of the column up to the split. The else-arm
        // (full usable_h) was dead; assert the invariant so a future
        // variant-table edit fails loud instead of silently picking a
        // full-height room.
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
                usable_h - (mid_y_split - top_margin)
            } else {
                usable_h
            },
        })
    } else {
        None
    };

    let right_x = mid_x + 1;
    let right_w = buf_w.saturating_sub(right_x);
    let cubicle_aisle_h = (usable_h / 10).max(8);
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
    // Fill: the pod grid packs as many rows as physically fit. The desk COUNT
    // cap (if any) is applied at emission in `compute_pod_desks` via `max_desks`
    // — the grid geometry itself is always the room's true capacity, so a bigger
    // canvas is a bigger office (production passes `None`; tests cap the count).
    let pod_rows =
        ((cubicle_h.saturating_sub(couch_to_desk_extra) + INTER_POD_AISLE_Y) / pod_stride_y).max(1);
    let pod_grid = PodGrid {
        cols: pod_cols,
        rows: pod_rows,
        stride_x: pod_stride_x,
        stride_y: pod_stride_y,
        couch_to_desk_extra,
    };

    let home_desks = compute_pod_desks(max_desks, &cubicle_band, pod_grid);

    let pod_decor = compute_pod_decor(&cubicle_band, pod_grid, floor_seed);

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
        |mr: &Bounds| mr.width >= MEETING_FURNITURE_MIN_W && mr.height >= trio_fit_h;
    // One source for a meeting room's furniture trio: two facing sofas and the
    // table CENTERED BETWEEN THEM. The table used to sit at the room centre while
    // the sofas sat at 30%/80% of the room height — asymmetric, so the north
    // sofa's front was packed against the table (a sub-coarse-grid seam that cost
    // its seats their front approach) while the south sofa had clearance. Placing
    // the table at the sofa midpoint gives both fronts equal, routable clearance.
    // The sofas keep their 20%/80% bias (backrest clearance from the room's top/
    // bottom walls); only the table follows them. All positions are
    // window-height-driven, so the approach points the agents path to derive from
    // the resulting mask at every size — nothing here is a fixed pixel offset.
    let room_furniture = |mr: &Bounds, north_floor: u16| -> ([Point; 2], Point) {
        let cx = mr.x + mr.width / 2;
        // Sofas sit SYMMETRICALLY about the room mid-line (20%/80%, was 30%/80%)
        // so each gets equal front clearance to the centred table — the old 30%
        // packed the north sofa's front against the table. The north clamp is
        // per-room (`north_floor`): room 0's top edge is the wall band's
        // walkable carpet apron, so its sofa may TUCK toward the wall. The
        // sofa_h/2 floor binds only if the sprite ever grows (the trio fit
        // gate guarantees pct-20 ≥ 4 > sofa_h/2 today), so pct-20 governs;
        // the 1-row apron strip that remains above the padded body drains
        // laterally through the screen-west/bookshelf-east channel the wall
        // decor placement guarantees — do NOT weaken that channel, or the
        // strip strands (the 150×68 sealed-pocket class). Room 1 (dense)
        // sits under the glass divider, which stamps WALL_THICK_H rows into
        // its top — its floor stays a full sofa_h so the sofa ground clears
        // the wall ground. The south clamp keeps a full sofa_h off the
        // bottom wall on both.
        let north_y = (mr.y + pct(mr.height, 20)).max(mr.y + north_floor);
        let south_y = (mr.y + pct(mr.height, 80)).min(mr.y + mr.height.saturating_sub(sofa_h));
        let sofas = [Point { x: cx, y: north_y }, Point { x: cx, y: south_y }];
        let table = Point {
            x: cx,
            y: (north_y + south_y) / 2,
        };
        (sofas, table)
    };
    let mut meeting_furniture: Vec<MeetingFurniture> = Vec::new();
    // Order is load-bearing: room 0 = `meeting_room`, room 1 = `meeting_room_2`
    // (dense layout). `compute_waypoints` keys seats to a room by this index.
    for (room_idx, room) in [meeting_room, meeting_room_2].into_iter().enumerate() {
        if let Some(mr) = room.filter(&room_fits_furniture) {
            let north_floor = if room_idx == 0 { sofa_h / 2 } else { sofa_h };
            let (sofas, table) = room_furniture(&mr, north_floor);
            meeting_furniture.push(MeetingFurniture { sofas, table });
        }
    }

    let room_walls = compute_room_walls(geom, mid_x, mid_y_split, top_margin, usable_h);

    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(&cubicle_band, top_margin);
    // The whole lounge vignette (couch + floor lamp + side table) is one
    // authored cluster ~23 px wide; on a band narrower than this floor the
    // padded couch blocked the door threshold itself (placement-sweep catch).
    // Below the gate the lounge degrades away entirely — the
    // `couch_sprite_center: None` case the field always documented.
    // 30 = the vignette's blocked span (side-table west edge couch_x−13 →
    // lamp east edge couch_x+10) + OBSTACLE_PAD_PX each side + walk clearance.
    const LOUNGE_MIN_BAND_W: u16 = 30;
    let lounge_fits = cubicle_band.width >= LOUNGE_MIN_BAND_W;

    let (mut waypoints, couch_sprite_center) = compute_waypoints(
        &cubicle_band,
        top_margin,
        pantry_room,
        pantry_counter_size,
        &pod_decor,
        &cubicle_aisle,
        &meeting_furniture,
        lounge_fits,
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
    // bathes the seating area at night. Rides the lounge gate: no couch,
    // no lamp (the vignette lives and dies together).
    let floor_lamp = lounge_fits.then_some(Point {
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
    let lounge_side_table = lounge_fits.then_some(Point {
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
    // The meeting screen hugs room 0's WEST CORNER, not its centre: centred
    // it loomed directly above the sofa group (worst on content-fit short
    // rooms, where the trio tucks against the top wall), reading as a
    // cluttered stack. Corner screen + the bookshelf's east clamp spread the
    // band items to the room's two sides, clearing the air above the sofas.
    // The bookshelf keeps its buffer anchor (18% of width) but ALSO clamps
    // past the sofa pad's east end (+5: its own 2-px ground pad plus a ≥2-px
    // walkable channel) — that channel is LOAD-BEARING, not taste: the
    // wall-band carpet apron between the two decor grounds must drain south
    // AROUND the tucked sofa (whose padded body seals the lane above the
    // backrest), else those apron cells strand (placement-sweep sealed-pocket
    // catch at 150×68). The bookshelf drops entirely when the clamped slot
    // would run into the exit sign / elevator (degenerate widths — same
    // degradation pattern as the bare meeting room), which reopens the
    // channel by absence.
    let bookshelf_w = furniture_def(WallDecor::Bookshelf.furniture()).visual.w;
    let screen_w = furniture_def(WallDecor::MeetingScreen.furniture()).visual.w;
    let meeting_screen_x = meeting_room.map(|mr| mr.x + 1);
    let sofa_fp_w = furniture_def(Furniture::MeetingSofaBody)
        .footprint
        .map_or(0, |s| s.w);
    let bookshelf_x = {
        let x = pct(buf_w, 18);
        match (meeting_screen_x, meeting_room) {
            (Some(sx), Some(mr)) => {
                // The ONE flush slot (screen east edge + a 2-px gap, so the
                // two grounds' pads merge with no strandable apron cell
                // between them) — every arm below derives from it; a second
                // copy of the offset could desync the spread clamp from the
                // fallback and reopen a sub-pad channel.
                let flush_east = sx + screen_w + 2;
                // The drain term applies only when room 0 actually HOSTS its
                // trio: with no sofa there is nothing to route around, and
                // pushing the shelf east anyway hangs it over the cubicle
                // band, where the first desk pod's pad seals the apron gap
                // against it instead (sweep sealed-pocket catch at 48×60 —
                // a bare doll-house room).
                if room_fits_furniture(&mr) {
                    // Mirrors room_furniture's cx + the mask's Center-anchored
                    // sofa ground east edge (fp/2 + OBSTACLE_PAD_PX) — pinned
                    // behaviorally by the sweep's connectivity invariant: if
                    // either side drifts, the drain channel seals and the
                    // sweep reds.
                    let sofa_pad_east =
                        mr.x + mr.width / 2 + sofa_fp_w / 2 + super::OBSTACLE_PAD_PX;
                    // Past the sofa's shadow by the shelf's OWN 1-px ground
                    // pad (mask.rs wall-decor stamp uses pad=1, not
                    // OBSTACLE_PAD_PX) + a ≥2-px walkable channel + slack.
                    const BOOKSHELF_DRAIN_GAP: u16 = 5;
                    let spread = x.max(flush_east).max(sofa_pad_east + BOOKSHELF_DRAIN_GAP);
                    if spread + bookshelf_w < mr.x + mr.width {
                        spread
                    } else {
                        // Narrow trio room: the spread slot would pierce the
                        // divider (owner catch at 150-wide Standard). Fall
                        // back to the FLUSH slot — no strandable apron gap
                        // opens between the pair, and the apron east of them
                        // drains down the room's east strip past the sofa
                        // pad. NOT the pct-18 anchor: at these widths it
                        // opens a gap OVER the sofa pad, the original 150×68
                        // sealed pocket.
                        flush_east
                    }
                } else {
                    x.max(flush_east)
                }
            }
            _ => x,
        }
    };
    // Everything east of the exit sign / elevator face is off-limits. The
    // exit sign's slot is computed ONCE here and reused by its push below —
    // two copies of the `buf_w - 9` offset would silently desync the limit
    // from the sign if the offset ever moves (online-review catch).
    let exit_sign_x = buf_w.saturating_sub(9);
    let wall_east_limit = exit_sign_x.min(door.map(|d| d.x).unwrap_or(u16::MAX));
    // The bookshelf additionally stays WEST of the vertical divider (the
    // meeting room's east wall): on narrow trio rooms the drain clamp can
    // push it onto the wall's top segment (owner catch at 150-wide
    // Standard — the shelf visually pierced the glass). Dropping it there
    // reopens the apron channel, same degradation as the exit-sign limit.
    let bookshelf_east_limit = meeting_room
        .map_or(u16::MAX, |mr| mr.x + mr.width)
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
    if has_meeting || has_pantry {
        let pos = Point {
            x: mid_x + 3,
            y: top_margin + usable_h / 3,
        };
        // The free-standing whiteboard's y (usable_h / 3) is independent of
        // the desk grid — at a handful of narrow-band heights it lands ON a
        // desk row instead of an aisle (sweep catch #2). Its ground is a
        // 10px wheel strip at the sprite base; skip the board when that
        // strip would collide with any desk's ground.
        let wb_def = furniture_def(WallDecor::Whiteboard.furniture());
        let collides_a_desk = wb_def.footprint.is_some_and(|fp| {
            let (wb_tl, wb_sz) = mask::ground_rect(
                Anchor::TopLeft,
                pos,
                fp,
                wb_def.visual,
                wb_def.ground_x,
                wb_def.ground_y,
            );
            let desk = super::decor::desk_furniture_def();
            home_desks.iter().any(|&d| {
                // is_some_and: the desk row's footprint is statically Some,
                // but the house rule bans unwrap/expect in prod — a None
                // simply means no ground to collide with.
                desk.footprint.is_some_and(|fp| {
                    let desk_ground = mask::ground_rect(
                        Anchor::TopLeft,
                        d,
                        fp,
                        desk.visual,
                        desk.ground_x,
                        desk.ground_y,
                    );
                    super::placement::rects_overlap((wb_tl, wb_sz), desk_ground)
                })
            })
        });
        if !collides_a_desk {
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

    // ── Pantry v2: kitchen island (+ stand slots) + snack shelf ──
    // Every piece follows the refuse-don't-force rule with BOTH-axis clamps
    // (the #549/#551/#554 one-axis-clamp class), and keeps clear of the
    // counter's padded north (the anti-merge routing constraint) —
    // placement_sweep's overlap/containment/connectivity/mask-parity
    // invariants are the backstop.
    let kitchen_island = if let Some(pr) = pantry_room {
        let def = furniture_def(Furniture::KitchenIsland);
        let vis = def.visual;
        let (half_w, half_h) = (vis.w / 2, vis.h / 2);
        let clr = super::WALL_THICK_H + super::OBSTACLE_PAD_PX;
        // Stands flank the island 1 walkable cell beyond the body's padded
        // footprint (pad + 1, derived — not a re-hardcoded 3). They must stay
        // in-room too, so the x clamps price the stand extent, not the body.
        let stand_dx = half_w + super::OBSTACLE_PAD_PX + 1;
        let counter_y = pr.y + pct(pr.height, pantry_counter_y_pct(pantry_counter_size.w));
        let counter_north =
            counter_y.saturating_sub(pantry_counter_size.h / 2 + super::OBSTACLE_PAD_PX);
        let min_x = pr.x + clr + stand_dx;
        let max_x = (pr.x + pr.width).saturating_sub(clr + stand_dx);
        // The bartenders' approach lane — the walkable row above the body's
        // padded strip — must be in-room (pad-derived, same rule as stand_dx).
        let min_y = pr.y + clr + half_h + super::OBSTACLE_PAD_PX;
        let max_y = counter_north.saturating_sub(half_h + super::OBSTACLE_PAD_PX + 1);
        if min_x <= max_x && min_y <= max_y {
            let ix = (pr.x + pr.width / 2).clamp(min_x, max_x);
            let iy = (pr.y + pct(pr.height, 40)).clamp(min_y, max_y);
            let island = Point { x: ix, y: iy };
            // Bartender slots sit ON the island's center row at its quarter
            // points: 8px-wide sprites at ±w/4 on the 20px island can't
            // overlap each other, and the blocked pos is fine for an
            // `occupies_pos` slot (the couch-seat pattern — approach_point
            // finds the lane BEHIND the island, the settle glide bridges in,
            // and the island's south-row z-key occludes the standers' legs).
            let bar_dx = (vis.w / 4) as i16;
            for (dx, facing) in [
                (-(stand_dx as i16), Facing::East),
                (stand_dx as i16, Facing::West),
                (-bar_dx, Facing::South),
                (bar_dx, Facing::South),
            ] {
                waypoints.push(Waypoint {
                    pos: Point {
                        x: ix.saturating_add_signed(dx),
                        y: iy,
                    },
                    kind: WaypointKind::Island,
                    facing,
                    room_id: None,
                });
            }
            Some(island)
        } else {
            None
        }
    } else {
        None
    };

    // Snack shelf: hugs the west wall (the buffer edge — the pantry's only
    // wall-free side is the EAST bridge, which must stay open). Waypoint-only
    // (vending-machine class): the mask stamps its table footprint via the
    // generic waypoint loop, the stander approaches from the open east side.
    if let Some(pr) = pantry_room {
        let def = furniture_def(Furniture::SnackShelf);
        let vis = def.visual;
        let (half_w, half_h) = (vis.w / 2, vis.h / 2);
        let clr = super::WALL_THICK_H + super::OBSTACLE_PAD_PX;
        let counter_y = pr.y + pct(pr.height, pantry_counter_y_pct(pantry_counter_size.w));
        let counter_north =
            counter_y.saturating_sub(pantry_counter_size.h / 2 + super::OBSTACLE_PAD_PX);
        let sx = pr.x + 1 + half_w;
        // Width gate: 1px west margin + the 7px shelf + 3px so the east-side
        // stander has an in-room walkable cell — narrower rooms refuse (the
        // sweep's first catch on this block was a 7px shelf in a 6px room).
        let width_fits = pr.width >= vis.w + 4;
        let min_y = pr.y + clr + half_h;
        let max_y = counter_north.saturating_sub(half_h + 1);
        let target = pr.y + pct(pr.height, 30);
        let candidate = (width_fits && min_y <= max_y).then(|| target.clamp(min_y, max_y));
        if let Some(sy) = candidate {
            waypoints.push(Waypoint {
                pos: Point { x: sx, y: sy },
                kind: WaypointKind::SnackShelf,
                facing: Facing::West,
                room_id: None,
            });
        }
    }

    let corridor = Some(Bounds {
        x: 0,
        y: cubicle_aisle.y,
        width: buf_w,
        height: cubicle_aisle.height,
    });

    let walkable = mask::build_walkable_mask(
        buf_w,
        buf_h,
        top_margin,
        door,
        &home_desks,
        &meeting_furniture,
        kitchen_island,
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
    // centre. ReachSet's seed snap pulls a blocked seed into the adjacent component.
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
        cubicle_aisle,
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
        meeting_room_2,
        pantry_room,
        meeting_furniture,
        room_walls,
        top_margin,
        pantry_counter_size,
        kitchen_island,
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
    /// Number of hand-authored geometries; floors past it cycle.
    const COUNT: u64 = 5;
    /// Fibonacci-hash multiplier, chosen so the standard floor seeds each map to
    /// a distinct variant.
    const HASH_MULT: u64 = 0x4737819096da1dad;

    /// Select the variant for a floor seed (Fibonacci hashing).
    fn from_seed(floor_seed: u64) -> Self {
        match floor_seed.wrapping_mul(Self::HASH_MULT) % Self::COUNT {
            0 => FloorVariant::Standard,
            1 => FloorVariant::OpenPlan,
            2 => FloorVariant::Dense,
            3 => FloorVariant::Senior,
            _ => FloorVariant::Lounge,
        }
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
/// `has_dual_meeting` (a Dense floor tall enough for two meeting rooms). Drives
/// [`compute_room_walls`]'s segment set; the `has_pantry` / `mid_x_pct` accessors
/// fold in the Dense-degrade (a too-short Dense floor gains a pantry and widens to
/// the Standard column). Replaced the 4 mutually-constrained bools + debug_asserts.
#[derive(Clone, Copy)]
pub(super) struct FloorGeometry {
    variant: FloorVariant,
    has_dual_meeting: bool,
}

impl FloorGeometry {
    fn has_meeting(self) -> bool {
        self.variant.has_meeting()
    }
    fn has_vertical_wall(self) -> bool {
        self.variant.has_meeting()
    }
    /// Resolved pantry presence AFTER the Dense-degrade: a Dense floor too short
    /// for two rooms gains a pantry (Standard geometry).
    fn has_pantry(self) -> bool {
        if self.variant == FloorVariant::Dense && !self.has_dual_meeting {
            true
        } else {
            self.variant.has_pantry_base()
        }
    }
    /// Resolved mid-column percent AFTER the Dense-degrade (a too-short Dense
    /// widens to the Standard 28% column).
    fn mid_x_pct(self) -> u16 {
        if self.variant == FloorVariant::Dense && !self.has_dual_meeting {
            28
        } else {
            self.variant.mid_x_pct()
        }
    }
}

/// Pod-grid desk placement: full pods, partial columns at right edge,
/// partial row at bottom edge.
pub(super) fn compute_pod_desks(
    max_desks: Option<usize>,
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
    // `None` fills the grid (emission unbounded); `Some(cap)` caps the count —
    // the deterministic knob for tests/snapshots. Bound the allocation hint to
    // the grid's physical desk capacity: `n` may be `usize::MAX` (fill), and
    // `Vec::with_capacity(usize::MAX)` aborts.
    let n = max_desks.unwrap_or(usize::MAX);
    let grid_desk_cap =
        (pod_cols as usize) * (pod_rows as usize) * (POD_SIDE as usize) * (POD_SIDE as usize);
    let mut home_desks = Vec::with_capacity(n.min(grid_desk_cap.max(1)));
    // Clamp: a desk must fit entirely inside the cubicle band.
    // Without this, the last intra-pod row of a bottom pod can
    // extend past cubicle_band into the cubicle_aisle (the pod_rows
    // formula counts strides between origins but not the final
    // pod's tail height).
    // Honest GROUND clamp on Y (the twin of desk_x_max below): the desk is
    // walk-behind (ground_y: End), so its shallow footprint is anchored to the
    // sprite BASE — the blocked ground reaches DESK_GROUND_H (the full visual
    // height) below the desk Point, NOT DESK_H (the slot). Clamping on DESK_H
    // let a bottom-row desk's ground spill up to 2 px south into cubicle_aisle
    // (the walk-behind Start→End move staled the old clamp). Slot-vs-ground on Y.
    let desk_y_max =
        (cubicle_band.y + cubicle_band.height).saturating_sub(super::decor::DESK_GROUND_H);
    // Mirror clamp for x: `pod_cols` floors at 1, so on a 34-66px band the
    // forced pod's 2nd desk column lands past the band's right edge (even
    // entirely off-buffer) — an invisible desk whose walk anchor sits outside
    // the mask. Skip those desks; the floor degrades to fewer desks, the same
    // graceful degradation as the y clamp and the meeting room's
    // MEETING_FURNITURE_MIN_W gate (capacity auto-computes from
    // `home_desks.len()`, so the smaller count IS the floor's real capacity).
    // Honest GROUND clamp: a desk's blocked ground is DESK_GROUND_W wide (the
    // side cabinets, not the DESK_W slot), so the last column must leave room
    // for the full 14-px sprite — DESK_W here let it poke 4 px past the buffer
    // edge (#549 drift). Slot-vs-ground on the X axis.
    let desk_x_max =
        (cubicle_band.x + cubicle_band.width).saturating_sub(super::decor::DESK_GROUND_W);
    let push_desk = |desks: &mut Vec<Point>, x: u16, y: u16| -> bool {
        if desks.len() >= n || y > desk_y_max || x > desk_x_max {
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
    // Mirror of push_desk's x clamp: `pod_cols` floors at 1, so on a 34-41px
    // band the forced pod's horizontal-aisle slot center (pod_origin_x +
    // pod_w/2) lands past the band's right edge — even fully off-buffer — and
    // PhoneBooth/StandingDesk slots there get promoted to wander waypoints,
    // sending idle agents to invisible furniture. Skip a slot whose visual
    // would overflow the band; the floor degrades to fewer decor pieces, the
    // same graceful degradation as desks. The kind cycle still advances so
    // surviving slots keep the kinds they'd have on a wider floor.
    let band_right = cubicle_band.x + cubicle_band.width;
    // Vertical twin of the x clamp: the LAST POD ROW's vertical-aisle slot
    // center (pod_origin_y + pod_h/2) can sit close enough to the band's
    // bottom that a tall centered visual (PhoneBooth, 12px at 200x116 seed 2)
    // crosses into the cubicle_aisle, its south-anchored footprint blocking cubicle_aisle
    // cells. (Horizontal-aisle slots sit a full pod_h shallower and can't
    // reach the edge.) Same centered-blit math the painter uses
    // (pos - h/2 .. pos - h/2 + h).
    let band_bottom = cubicle_band.y + cubicle_band.height;
    let mut push_slot = |pod_decor: &mut Vec<PodDecorItem>, x: u16, y: u16| {
        let kind = PodDecor::ALL[slot_idx % PodDecor::ALL.len()];
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

/// Wall segments with door gaps for meeting/pantry rooms. Branches on the
/// [`FloorGeometry`] (variant + `has_dual_meeting`); every wall condition folds
/// from the variant's consts, so the old debug_asserts guarding the bool tuple
/// (`has_pantry || has_dual_meeting` when `has_meeting`) are now structural.
pub(super) fn compute_room_walls(
    geom: FloorGeometry,
    mid_x: u16,
    mid_y_split: u16,
    top_margin: u16,
    usable_h: u16,
) -> Vec<WallSegment> {
    let has_vertical_wall = geom.has_vertical_wall();
    let has_dual_meeting = geom.has_dual_meeting;
    let has_meeting = geom.has_meeting();
    let has_pantry = geom.has_pantry();
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
        // has_vertical_wall == has_meeting, and a meeting-bearing variant always
        // shares the left column with the pantry or a second meeting room (structural
        // in `FloorVariant`: Standard/Senior carry a pantry, Dense either has dual
        // meetings or degrades to has_pantry) — so the vertical wall always stops at
        // the mid-height split. The full-usable_h else-arm is dead by construction.
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
            // Start below the horizontal wall's mask footprint — the offset IS
            // the wall thickness (horizontal walls stamp with pad 0), so read
            // it from WALL_THICK_H rather than a second copy of the value.
            // This keeps the offset within the renderer's bridge-up tolerance
            // by construction (`WALL_THICK_H_PX + 2` in `stitch_vertical_wall`,
            // where WALL_THICK_H_PX is defined AS this const) — a hardcoded 6
            // would leave this segment rendering as a detached strip if the
            // wall face were ever thinned.
            let v2_top = mid_y_split + super::WALL_THICK_H;
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
#[allow(clippy::too_many_arguments)] // layout inputs — each arg a distinct zone/fact
pub(super) fn compute_waypoints(
    cubicle_band: &Bounds,
    top_margin: u16,
    pantry_room: Option<Bounds>,
    pantry_counter_size: Size,
    pod_decor: &[PodDecorItem],
    cubicle_aisle: &Bounds,
    meeting_furniture: &[MeetingFurniture],
    lounge_fits: bool,
) -> (Vec<Waypoint>, Option<Point>) {
    let right_x = cubicle_band.x;
    let right_w = cubicle_band.width;
    let Point {
        x: couch_x,
        y: couch_y,
    } = couch_pos(cubicle_band, top_margin);
    // Lounge couch: 3 seats across the 20px sofa (dx ∈ {-6, 0, +6}), matching
    // the meeting sofa. room_id stays None — the lounge's group-chat grouping
    // is keyed at the chitchat venue layer (all couch seats share one venue),
    // NOT via the meeting-only room_id field. The sprite paints once, centred
    // on couch_x (the middle seat); see `couch_sprite_center`.
    // Gated on `lounge_fits` (the caller's band-width gate): on a degenerate
    // narrow band the padded 20px couch swallowed the whole floor including
    // the door threshold (placement-sweep catch) — the `couch_sprite_center:
    // None` degradation this fn's signature always documented, now real.
    let mut waypoints: Vec<Waypoint> = if lounge_fits {
        SEAT_DX
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
            .collect()
    } else {
        Vec::new()
    };
    if let Some(pr) = pantry_room {
        // Clamp x so the counter fits within pantry_room. Without this
        // the counter (32px or 20px wide) extends past the east wall
        // into the cubicle band at small buffer widths.
        let half_cw = pantry_counter_size.w / 2;
        let max_cx = pr.x + pr.width.saturating_sub(half_cw + 1);
        // The WEST twin of the east clamp: a room narrower than the counter
        // has no valid center at all — the old un-clamped west side let the
        // 20px counter spill out of a 6-9px room and off the buffer's west
        // edge, silently hidden by saturating_sub (placement-sweep catch;
        // the same one-axis-only clamp class as #549/#551's desk clamps).
        // Refuse rather than force: no counter on a degenerate pantry.
        let min_cx = pr.x + half_cw;
        if min_cx <= max_cx {
            // y is single-sourced with the island clamp; only x is size-shaped
            // (large counter is room-centred, small one sits at 60% width).
            let wy = pr.y + pct(pr.height, pantry_counter_y_pct(pantry_counter_size.w));
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
    if cubicle_aisle.height >= 10 && cubicle_aisle.width > 30 {
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
    if cubicle_aisle.height >= 9 && right_w > 40 {
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

    // Meeting-room slots. Each room's 2 sofas are stored north→south
    // (`MeetingFurniture.sofas[0/1]`); each seats up to 3 agents (dx ∈ {-6, 0, +6}
    // along the 20px sofa) facing the table. Two standing slots flank the table.
    // Every slot in a room shares its `room_id` (the room's index) so the
    // group-chitchat venue keys on the room, not the individual seat.
    for (room_id, room) in meeting_furniture.iter().enumerate() {
        let table = room.table;
        for sofa in room.sofas {
            // North-of-table sofa faces South (front toward the viewer); the
            // south sofa faces North (back toward the viewer) — the pair reads
            // as two people facing each other across the table.
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
        lounge_fits.then_some(Point {
            x: couch_x,
            y: couch_y,
        }),
    )
}
