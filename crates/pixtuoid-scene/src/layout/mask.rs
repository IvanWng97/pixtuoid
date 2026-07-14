//! Walkable-mask construction. Stamps every obstacle (walls, desks,
//! sofas, plants, decor) into a `WalkableMask` so A* knows where
//! characters can route. The padding constant `OBSTACLE_PAD_PX` adds a
//! clearance band around each obstacle so walkers don't scrape along
//! edges.

use super::decor::GroundAlign;
use super::{
    anchored_top_left, furniture_def, Anchor, Furniture, MeetingRoom, PlantItem, PodDecorItem,
    Point, Size, WallDecorItem, WallSegment, Waypoint, WaypointKind, OBSTACLE_PAD_PX,
    PANTRY_FOOTPRINT_DEPTH, WALL_BAND_TO_TOP_MARGIN,
};
use pixtuoid_core::walkable::WalkableMask;

/// Stamp a furniture footprint as a collision rect DECLARED relative to its
/// VISUAL box — the general top-down model. The visual box top-left comes
/// from `anchor` via `anchored_top_left` (the SAME origin the renderer blits
/// from, so blocked ground and painted sprite can't drift); the footprint is
/// offset inside it by the row's [`GroundAlign`] per axis
/// (`ground_x`/`ground_y`), each resolving to a pixel offset from
/// `visual − footprint` (drift-free). One formula covers every legacy shape
/// byte-for-byte:
/// - `ground_y = End`: south strip at the sprite base — plant canopy / booth
///   column / board panel, AND the desk (its shallow `DESK_FOOT_H` front strip;
///   the monitor + surface overhang NORTH — walk-behind, #551) (invariant #6 —
///   a walker parks DEEP behind the overhang and the sprite's own y-sort
///   occludes them, no synthetic cap);
/// - `ground_y = Center`: sofa body, floor lamp;
/// - `ground_y = Start`: currently UNUSED — the desk was here until it went
///   walk-behind; kept as the third align;
/// - `ground_x = Center` (every row today): the wall-decor whiteboard's 10px
///   wheel span sits at sprite cols 2-11, not 0-9.
///
/// This replaced the old `visual_h > footprint_h` per-site INFERENCE, whose
/// three exceptions each bypassed the helper through a dedicated stamp site
/// — in the declared model they are not exceptions, just different aligns.
/// The `pad` clearance band is added on every side.
#[allow(clippy::too_many_arguments)] // mask/visual geometry — each arg distinct
fn stamp_ground(
    mask: &mut WalkableMask,
    anchor: Anchor,
    pos: Point,
    fp: Size,
    visual: Size,
    ground_x: GroundAlign,
    ground_y: GroundAlign,
    pad: u16,
) {
    let (tl, sz) = ground_rect(anchor, pos, fp, visual, ground_x, ground_y);
    mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, pad);
}

/// The ONE ground-geometry formula: the blocked rect (top-left + size) of a
/// piece's footprint, declared relative to its VISUAL box (see `stamp_ground`,
/// which stamps exactly this rect). Shared by the mask AND the placement
/// sweep's containment/overlap invariants — extracted so the sweep can't grow
/// a second copy of the offset math (the drift class this repo hunts). The
/// per-call-site `pad` is deliberately NOT part of the rect: pad is routing
/// slack (2/1/0 depending on the obstacle), not the object.
pub(super) fn ground_rect(
    anchor: Anchor,
    pos: Point,
    fp: Size,
    visual: Size,
    ground_x: GroundAlign,
    ground_y: GroundAlign,
) -> (Point, Size) {
    let vis_tl = anchored_top_left(anchor, pos, visual.w, visual.h);
    let left = vis_tl.x + ground_x.offset(visual.w, fp.w);
    let top = vis_tl.y + ground_y.offset(visual.h, fp.h);
    (Point { x: left, y: top }, fp)
}

/// The pantry counter's blocked-ground rect — the RUNTIME-sized twin of
/// [`ground_rect`] (the counter's `FurnitureDef` row is `footprint: None` /
/// `visual (0,0)`; its real size arrives per-layout as `pantry_counter_size`).
/// A shallow `PANTRY_FOOTPRINT_DEPTH` strip anchored to the sprite base
/// (`pos.y + h/2`) — the walk-behind shape. Shared by the mask stamp AND the
/// placement sweep so the bespoke math can't fork.
pub(super) fn pantry_ground_rect(pos: Point, counter: Size) -> (Point, Size) {
    let depth = PANTRY_FOOTPRINT_DEPTH.min(counter.h);
    let south = pos.y + counter.h / 2;
    (
        Point {
            x: pos.x.saturating_sub(counter.w / 2),
            y: south.saturating_sub(depth),
        },
        Size {
            w: counter.w,
            h: depth,
        },
    )
}

/// Walkable footprint (and render face height) of a horizontal (E-W) interior
/// wall, in px. The renderer derives `WALL_THICK_H_PX` from this so the visible
/// glass face and the blocked ground footprint can never drift apart.
pub const WALL_THICK_H: u16 = 6;
/// Walkable footprint of a vertical (N-S) interior wall — seen edge-on, so 1px
/// (the renderer draws it 3px wide, visual-wider-than-footprint per the
/// top-down ground-projection rule). Single source: mask + the placement test
/// read this rather than re-typing `1`.
pub const WALL_THICK_V: u16 = 1;

/// A wall segment's PHYSICAL blocked rect (origin + size), shared by the mask
/// stamp and the placement sweep so the two can't disagree on wall geometry.
/// Vertical segments plugging into the north window band get their top raised
/// by WALL_BAND_TO_TOP_MARGIN — the stitch that keeps A* from threading the
/// wall top (regression: vertical_wall_is_impassable_except_through_the_door);
/// a hand-rolled copy omits it, so both callers derive from HERE.
pub(super) fn wall_segment_rect(seg: &WallSegment, top_margin: u16) -> (Point, Size) {
    let (start, end) = (seg.start, seg.end);
    if start.x == end.x {
        let seg_top = start.y.min(end.y);
        let seg_bot = start.y.max(end.y);
        let seg_top = if seg_top == top_margin {
            top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN)
        } else {
            seg_top
        };
        (
            Point {
                x: start.x,
                y: seg_top,
            },
            Size {
                w: WALL_THICK_V,
                h: seg_bot - seg_top + 1,
            },
        )
    } else {
        (
            Point {
                x: start.x.min(end.x),
                y: start.y,
            },
            Size {
                w: start.x.abs_diff(end.x) + 1,
                h: WALL_THICK_H,
            },
        )
    }
}

/// The placed-piece inventory the mask stamps — a NAMED input so the five
/// interchangeable `Option<Point>` pieces (door/kitchen_island/floor_lamp/
/// lounge_side_table/fish_tank) can't be positionally swapped, and so
/// `build_walkable_mask` destructures it with NO `..` (a new field added here
/// must then be handled by the mask, not silently walked through).
pub(super) struct MaskObstacles<'a> {
    pub(super) buf_w: u16,
    pub(super) buf_h: u16,
    pub(super) top_margin: u16,
    pub(super) door: Option<Point>,
    pub(super) home_desks: &'a [Point],
    pub(super) meeting_rooms: &'a [MeetingRoom],
    pub(super) kitchen_island: Option<Point>,
    pub(super) waypoints: &'a [Waypoint],
    pub(super) plants: &'a [PlantItem],
    pub(super) floor_lamp: Option<Point>,
    pub(super) lounge_side_table: Option<Point>,
    pub(super) fish_tank: Option<Point>,
    pub(super) wall_decor: &'a [WallDecorItem],
    pub(super) pod_decor: &'a [PodDecorItem],
    pub(super) room_walls: &'a [WallSegment],
    pub(super) pantry_counter_size: Size,
}

pub(super) fn build_walkable_mask(obs: &MaskObstacles) -> WalkableMask {
    let &MaskObstacles {
        buf_w,
        buf_h,
        top_margin,
        door,
        home_desks,
        meeting_rooms,
        kitchen_island,
        waypoints,
        plants,
        floor_lamp,
        lounge_side_table,
        fish_tank,
        wall_decor,
        pod_decor,
        room_walls,
        pantry_counter_size,
    } = obs;

    let mut mask = WalkableMask::new_open(buf_w, buf_h);

    // Block the north wall band down to the WALL VISUAL bottom (top_wall_h),
    // not the full top_margin — the rows between are carpet apron under the
    // windows, so blocking them put the walkable boundary ~4px south of the
    // visible wall base. Mask = ground projection (invariant #6).
    mask.mark_blocked(
        0,
        0,
        buf_w,
        top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN),
        0,
    );
    // Door gap punched in the north wall band; south baseboard obstacle strip.
    const DOOR_CUT_W: u16 = 8;
    const BASEBOARD_H: u16 = 3;
    if let Some(d) = door {
        let cut_x = d.x.saturating_sub(2);
        let cut_h = top_margin.saturating_add(OBSTACLE_PAD_PX);
        mask.mark_walkable(cut_x, 0, DOOR_CUT_W, cut_h);
    }

    let baseboard_top = buf_h.saturating_sub(BASEBOARD_H);
    mask.mark_blocked(0, baseboard_top, buf_w, BASEBOARD_H, 0);

    // Interior walls, Stardew-style fake-3D: horizontal (E-W) walls show their
    // FACE (WALL_THICK_H px tall, real mass viewed from the north room); vertical
    // (N-S) walls are seen EDGE-ON (WALL_THICK_V px thin footprint, drawn wider
    // than it blocks per the top-down ground-projection rule, invariant #6).
    //
    // Wall padding is ASYMMETRIC by orientation, driven by the coarse router grid
    // (`pathfind::cell_walkable`), NOT by clearance:
    //   • HORIZONTAL faces are thick enough to fill a routing cell on their own,
    //     so they're impassable at pad=0 — and you stand FLUSH against the south
    //     face, so any pad is pure red bloat.
    //   • VERTICAL walls are a 1px edge-on strip the coarse grid can't see, so A*
    //     would route STRAIGHT THROUGH; OBSTACLE_PAD_PX is a routing-only
    //     clearance band (the FOOTPRINT stays 1px — characters still stand right
    //     next to the 3px visual), sized against DOOR_GAP in rooms/walls.rs.
    // Pinned by pathfind::vertical_wall_is_impassable_except_through_the_door.
    for seg in room_walls {
        let (origin, size) = wall_segment_rect(seg, top_margin);
        // Vertical walls are 1px edge-on — invisible to the coarse grid without
        // OBSTACLE_PAD_PX; horizontal faces already fill their routing cells
        // (pad would only bloat, see the block comment above).
        let pad = if seg.start.x == seg.end.x {
            OBSTACLE_PAD_PX
        } else {
            0
        };
        mask.mark_blocked(origin.x, origin.y, size.w, size.h, pad);
    }

    for desk in home_desks {
        // The desk is a WALK-BEHIND piece (like the plant canopy / TV): its
        // footprint is a shallow `DESK_FOOT_H` south strip anchored to the
        // sprite base by `ground_y: End`, so the monitor + surface overhang
        // NORTH of the blocked ground. A walker (or an agent taking the north
        // approach to sit) passes behind the monitor and is occluded by the
        // desk's own Pass-2 y-sorted sprite — no full-body obstacle to weave
        // around, and routes stay short. Footprint comes from the shared
        // FurnitureDef (always Some for the desk); stamped TOP-LEFT at the
        // desk Point (not centred like visited furniture) — the desk pos IS
        // its NW corner, and `stamp_ground` offsets the shallow strip to the
        // sprite base via `ground_y`. OBSTACLE_PAD still fences the strip.
        let desk_def = super::decor::desk_furniture_def();
        if let Some(fp) = desk_def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::TopLeft,
                *desk,
                fp,
                desk_def.visual,
                desk_def.ground_x,
                desk_def.ground_y,
                OBSTACLE_PAD_PX,
            );
        }
    }

    for trio in meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        // Sofa BODY footprint from the table (16 ON PURPOSE: 16 + 2·pad = the
        // 20px sprite X footprint, with the pad giving vertical sit clearance —
        // see the furniture_def row). Top-down rule: walk up to its sides.
        let sofa_def = furniture_def(Furniture::MeetingSofaBody);
        if let Some(fp) = sofa_def.footprint {
            for sofa in trio.sofas {
                // Declared CENTERED inside the visual (the sofa row's
                // ground_y) — the strip sits on the sofa pos, not its base.
                stamp_ground(
                    &mut mask,
                    Anchor::Center,
                    sofa,
                    fp,
                    sofa_def.visual,
                    sofa_def.ground_x,
                    sofa_def.ground_y,
                    OBSTACLE_PAD_PX,
                );
            }
        }
        let table_def = furniture_def(Furniture::MeetingTable);
        if let Some(fp) = table_def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                trio.table,
                fp,
                table_def.visual,
                table_def.ground_x,
                table_def.ground_y,
                OBSTACLE_PAD_PX,
            );
        }
    }

    if let Some(island) = kitchen_island {
        // Island body: block the south-anchored base only (footprint from
        // the row); the two countertop rows overhang (walk-behind, #6).
        let def = furniture_def(Furniture::KitchenIsland);
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                island,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                OBSTACLE_PAD_PX,
            );
        }
    }

    for wp in waypoints {
        // Footprint sizes live in `approach::obstacle_footprint` (single source
        // of truth shared with `stand_point`). `None` = meeting slots, which
        // sit/stand on sofa/table furniture already stamped above — no obstacle.
        let Some(Size { w, h }) = super::approach::obstacle_footprint(wp.kind, pantry_counter_size)
        else {
            continue;
        };
        // Pad=1 (not OBSTACLE_PAD_PX=2) — waypoint furniture paints in
        // Pass 1.5 (after characters) so a visitor's body is occluded
        // by the sprite. We don't need extra clearance around the
        // sprite footprint; the render order handles overlap correctly.
        if matches!(wp.kind, WaypointKind::Pantry) {
            // The counter sprite (h px tall) is centered on pos, but only its
            // SOUTH base sits on the floor — the receding cabinet tops +
            // backsplash are elevation that overhangs (invariant #6). Block a
            // shallow PANTRY_FOOTPRINT_DEPTH-tall strip anchored to that base
            // (sprite bottom = pos.y + h/2 - 1) instead of the full height, so
            // the non-walkable area hugs the counter foot. A character routed
            // behind it is occluded by the counter's own y-sorted sprite,
            // couch-style. `stand_point` uses the FULL `visual` so the USER parks
            // clear of the whole counter, not inside the upper sprite.
            let (tl, sz) = pantry_ground_rect(wp.pos, Size { w, h });
            mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, 1);
            continue;
        }
        // Anchoring is the table's declared ground alignment: booth/standing-
        // desk strips pin to their sprite base (End); vending/printer/couch
        // are flat.
        let def = furniture_def(wp.kind.furniture());
        stamp_ground(
            &mut mask,
            Anchor::Center,
            wp.pos,
            Size { w, h },
            def.visual,
            def.ground_x,
            def.ground_y,
            1,
        );
    }

    for &PlantItem { kind, pos } in plants {
        // GROUND footprint = a shallow pot strip; the canopy overhangs it, so
        // the table pins it to the sprite base (the leaves then occlude a
        // walker parked north of the pot via their own y-sort; invariant #6).
        let def = furniture_def(kind.furniture());
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                pos,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                1,
            );
        }
    }

    if let Some(lamp) = floor_lamp {
        // The lamp's tall footprint stamps CENTERED despite the sprite
        // overhang — the WHY lives on its ground_y row in decor.rs.
        let def = furniture_def(Furniture::FloorLamp);
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                lamp,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                1,
            );
        }
    }

    if let Some(t) = lounge_side_table {
        // Small footprint, pad=1: sits in the wide open lounge floor with
        // plenty of clearance.
        let def = furniture_def(Furniture::LoungeSideTable);
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                t,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                1,
            );
        }
    }

    if let Some(t) = fish_tank {
        // Only the cabinet base blocks (ground_y End pins the 3-row strip to
        // the sprite base); the glass tank is visual overhang against the
        // wall band. pad=1 like the other lounge pieces.
        let def = furniture_def(Furniture::FishTank);
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::Center,
                t,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                1,
            );
        }
    }

    // Wall decor is top-left anchored. Only kinds with a ground footprint in
    // the furniture table are obstacles (the rolling whiteboard + the floor-
    // standing bookshelf / meeting screen); the truly wall-HUNG kinds (bulletin
    // board, exit sign) are flush against the wall (footprint None) and stamp
    // nothing. Each footprint is the SHALLOW floor base of a sprite that
    // overhangs north (whiteboard panel / bookshelf shelves / TV monitor), so
    // the strip is SOUTH-anchored to the sprite base and the overhang occludes
    // a walker behind it (invariant #6).
    for &WallDecorItem { kind, pos } in wall_decor {
        // pad=1 (not OBSTACLE_PAD_PX=2): these elevated boards/cabinets overhang
        // nothing solid, so a 2px clearance band on every side just inflated the
        // blocked rect back to the full sprite width (hiding the footprint
        // shrink). Matches the pod-decor whiteboard's pad.
        let def = furniture_def(kind.furniture());
        if let Some(fp) = def.footprint {
            stamp_ground(
                &mut mask,
                Anchor::TopLeft,
                pos,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y,
                1,
            );
        }
    }

    // Pod-aisle decor is centred at `pos`. All variants are obstacles.
    // PhoneBooth + StandingDesk are also waypoints — those entries
    // appear above in `waypoints` and double-block the same area;
    // mark_blocked is idempotent. Use pad=1 (not OBSTACLE_PAD_PX=2)
    // because aisles are tight (14×16) and an extra pixel of pad on
    // each side disconnects the routing grid through the aisle.
    for &PodDecorItem { kind, pos } in pod_decor {
        // GROUND footprint (not the sprite size). Every overhanging aisle piece
        // (plant canopy, booth column, TV monitor, whiteboard panel) declares a
        // base-pinned ground_y (End), so the overhang occludes a walker behind
        // it (invariant #6); flat boxes declare Center (offset 0).
        let def = furniture_def(kind.furniture());
        let Some(fp) = def.footprint else {
            continue;
        };
        stamp_ground(
            &mut mask,
            Anchor::Center,
            pos,
            fp,
            def.visual,
            def.ground_x,
            def.ground_y,
            1,
        );
    }

    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::z_sort_row;

    #[test]
    fn ground_rect_matches_what_stamp_ground_blocks() {
        // `ground_rect` is the ONE ground-geometry formula, shared by the mask
        // (via `stamp_ground`) and the placement-invariant sweep — a second
        // copy of the offset math is the drift class this repo hunts. Pin the
        // sharing structurally: for every anchor × align combination, the rect
        // it returns must be EXACTLY the pad-0 cell set `stamp_ground` blocks.
        for anchor in [Anchor::TopLeft, Anchor::Center] {
            for gx in [GroundAlign::Start, GroundAlign::Center, GroundAlign::End] {
                for gy in [GroundAlign::Start, GroundAlign::Center, GroundAlign::End] {
                    let pos = Point { x: 20, y: 20 };
                    let fp = Size { w: 5, h: 3 };
                    let visual = Size { w: 8, h: 12 };
                    let mut mask = WalkableMask::new_open(40, 40);
                    stamp_ground(&mut mask, anchor, pos, fp, visual, gx, gy, 0);
                    let (tl, sz) = ground_rect(anchor, pos, fp, visual, gx, gy);
                    for y in 0..40u16 {
                        for x in 0..40u16 {
                            let in_rect =
                                x >= tl.x && x < tl.x + sz.w && y >= tl.y && y < tl.y + sz.h;
                            assert_eq!(
                                !mask.is_walkable(x, y),
                                in_rect,
                                "({x},{y}) blocked-vs-rect mismatch for {anchor:?}/{gx:?}/{gy:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn overhang_footprint_south_anchored_leaves_the_overhang_walkable() {
        // A 6-wide piece whose sprite is 12 tall but whose ground base is only 3
        // (a phone booth): a base-pinned ground_y=End (visual.h − fp.h = 9) must
        // block ONLY the 3-row south strip at the sprite base, leaving the tall
        // overhang north of it walkable — that's where a walker parks deep so
        // the sprite's y-sort occludes them. This is the core invariant the
        // cap-deletion relies on; pin it directly.
        let mut mask = WalkableMask::new_open(40, 40);
        let pos = Point { x: 20, y: 20 };
        stamp_ground(
            &mut mask,
            Anchor::Center,
            pos,
            Size { w: 6, h: 3 },
            Size { w: 6, h: 12 },
            GroundAlign::Center,
            GroundAlign::End,
            0,
        );
        let south = z_sort_row(Anchor::Center, pos, 12); // sprite base row
        for dy in 0..3 {
            assert!(
                !mask.is_walkable(pos.x, south - dy),
                "base row {} must be blocked",
                south - dy
            );
        }
        assert!(
            mask.is_walkable(pos.x, south - 4),
            "overhang region north of the base must stay walkable (walker parks here)"
        );
        assert!(
            mask.is_walkable(pos.x, pos.y.saturating_sub(5)),
            "sprite-top region must stay walkable"
        );
    }

    #[test]
    fn wall_decor_whiteboard_footprint_centers_under_the_wider_sprite() {
        // The wall-decor whiteboard is TopLeft-anchored: the renderer blits its
        // 14px-wide sprite at `pos`, with the wheels at sprite cols 2 and 11.
        // Its 10px ground footprint is the wheel span — so the south strip must
        // be CENTERED under the visual (left = pos.x + 2), not hug the sprite's
        // west edge: left-aligned, the east wheel column stayed walkable while
        // 2px of bare floor west of the board was blocked.
        use crate::layout::WallDecor;
        let pos = Point { x: 40, y: 30 };
        let wall_decor = vec![WallDecorItem {
            kind: WallDecor::Whiteboard,
            pos,
        }];
        let mask = build_walkable_mask(&MaskObstacles {
            buf_w: 120,
            buf_h: 96,
            top_margin: 20,
            door: None,
            home_desks: &[],
            meeting_rooms: &[],
            kitchen_island: None,
            waypoints: &[],
            plants: &[],
            floor_lamp: None,
            lounge_side_table: None,
            fish_tank: None,
            wall_decor: &wall_decor,
            pod_decor: &[],
            room_walls: &[],
            pantry_counter_size: Size { w: 20, h: 8 },
        });
        let def = furniture_def(Furniture::Whiteboard);
        let sprite_h = def.visual.h; // 11
        let base = pos.y + sprite_h - 1; // TopLeft south row
                                         // Both wheel columns blocked (pad=1 widens beyond them, so probe the
                                         // wheels themselves).
        assert!(
            !mask.is_walkable(pos.x + 2, base),
            "west wheel column must be blocked"
        );
        assert!(
            !mask.is_walkable(pos.x + 11, base),
            "east wheel column must be blocked"
        );
        // Bare floor west of the wheel span (beyond the 1px pad) stays open.
        assert!(
            mask.is_walkable(pos.x, base),
            "bare floor west of the wheels must stay walkable"
        );
        // And the sprite's own east edge column (beyond footprint+pad) too.
        assert!(
            mask.is_walkable(pos.x + 13, base),
            "floor east of the wheels+pad must stay walkable"
        );
    }

    #[test]
    fn flat_box_footprint_is_centered_not_south_anchored() {
        // visual == footprint ⇒ flat box (vending/printer/table), dy 0: a
        // centered stamp, NOT a south strip — the block straddles `pos`, so the
        // row NORTH of center is blocked (a south strip would leave it open).
        let mut mask = WalkableMask::new_open(40, 40);
        let pos = Point { x: 20, y: 20 };
        stamp_ground(
            &mut mask,
            Anchor::Center,
            pos,
            Size { w: 4, h: 6 },
            Size { w: 4, h: 6 },
            GroundAlign::Center,
            GroundAlign::Center,
            0,
        );
        assert!(
            !mask.is_walkable(pos.x, pos.y),
            "centered block: center blocked"
        );
        assert!(
            !mask.is_walkable(pos.x, pos.y - 2),
            "centered block: north-of-center blocked (not a south strip)"
        );
    }

    #[test]
    fn topleft_wall_decor_x_centering_is_parity_safe() {
        // `stamp_ground` centers a TopLeft footprint with `GroundAlign::Center`
        // = center-ON-pos `⌊v/2⌋−⌊f/2⌋`, whereas the OLD `stamp_south_strip`
        // used center-IN-box `⌊(v−f)/2⌋`. The two agree ONLY when v and f have
        // the same parity; they diverge by 1px at opposite parity. Every
        // current TopLeft-stamped wall-decor kind is same-parity, so the two
        // conventions coincide and the mask is byte-identical to before the
        // refactor. This test FAILS the day someone adds a TopLeft wall piece
        // with an even visual width over an odd footprint width (or vice
        // versa) — at which point the 1px offset is a conscious decision, not
        // a silent drift. (Center-ANCHORED pieces are parity-immune: the
        // visual term cancels — see GroundAlign::Center's doc.)
        for kind in [
            Furniture::Whiteboard,
            Furniture::Bookshelf,
            Furniture::MeetingScreen,
        ] {
            let def = furniture_def(kind);
            let Some(fp) = def.footprint else { continue };
            let center_on_pos = def.visual.w / 2 - fp.w / 2;
            let center_in_box = (def.visual.w - fp.w) / 2;
            assert_eq!(
                center_on_pos, center_in_box,
                "{kind:?}: TopLeft x-centering diverges at opposite parity \
                 (visual.w={}, footprint.w={}) — decide the 1px offset explicitly",
                def.visual.w, fp.w
            );
        }
    }
}
