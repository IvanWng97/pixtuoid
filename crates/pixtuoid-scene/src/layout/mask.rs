//! Walkable-mask construction: stamps every obstacle into a `WalkableMask` so
//! A* knows where characters can route.

use super::decor::{FurnitureDef, GroundAlign};
use super::{
    anchored_top_left, furniture_def, Anchor, Furniture, MeetingRoom, PlantItem, PodDecorItem,
    Point, Size, WallDecorItem, WallSegment, Waypoint, WaypointKind, OBSTACLE_PAD_PX,
    PANTRY_FOOTPRINT_DEPTH, WALL_BAND_TO_TOP_MARGIN, WAYPOINT_STAMP_PAD_PX,
};
use pixtuoid_core::walkable::WalkableMask;

/// Stamp a furniture footprint as a collision rect DECLARED relative to its
/// VISUAL box. The visual top-left comes from `anchor` via `anchored_top_left`
/// (the SAME origin the renderer blits from, so blocked ground and painted
/// sprite can't drift); the footprint is offset inside it by the row's
/// [`GroundAlign`] per axis. A footprint-less piece (wall-hung decor) stamps
/// nothing; runtime-footprint pieces build their rect and call `mark_blocked`
/// directly.
fn stamp_ground(mask: &mut WalkableMask, def: &FurnitureDef, anchor: Anchor, pos: Point, pad: u16) {
    if let Some((tl, sz)) = def.ground_rect(anchor, pos) {
        mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, pad);
    }
}

/// The ONE ground-geometry formula: the blocked rect (top-left + size) of a
/// piece's footprint, declared relative to its VISUAL box. Shared by the mask
/// AND the placement sweep's containment/overlap invariants, so the sweep can't
/// grow a second copy of the offset math. The per-call-site `pad` is
/// deliberately NOT part of the rect: pad is routing slack, not the object.
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
/// [`ground_rect`] (the counter's `FurnitureDef` row is `footprint: None`; its
/// real size arrives per-layout as `pantry_counter_size`). A shallow
/// `PANTRY_FOOTPRINT_DEPTH` strip anchored to the sprite base — the walk-behind
/// shape. Shared by the mask stamp AND the placement sweep so it can't fork.
pub(super) fn pantry_ground_rect(pos: Point, counter: Size) -> (Point, Size) {
    let depth = PANTRY_FOOTPRINT_DEPTH.min(counter.h);
    ground_rect(
        Anchor::Center,
        pos,
        Size {
            w: counter.w,
            h: depth,
        },
        counter,
        GroundAlign::Center,
        GroundAlign::End,
    )
}

use super::rooms::walls::wall_segment_rect;

/// WEST-only routing clearance stamped onto a vertical wall's footprint (NOT
/// part of the physical footprint — the placement sweep reads the un-margined
/// rect). The coarse 4×4 router (`pathfind::cell_walkable`) can't see a barrier
/// thinner than a cell: a bare `WALL_THICK_V` at a 2-mod-4 offset splits into
/// two cells at exactly the walkable threshold, so A* threads through. Stamped
/// toward the WEST (room side) so the east, band-facing mask edge stays flush
/// with the visual.
const WALL_ROUTING_MARGIN_X: u16 = 1;

/// The placed-piece inventory the mask stamps — a NAMED input so the five
/// interchangeable `Option<Point>` pieces can't be positionally swapped, and so
/// `build_walkable_mask` destructures it with NO `..` (a new field added here
/// must then be handled by the mask, not silently walked through).
pub(super) struct MaskObstacles<'a> {
    pub(super) buf_w: u16,
    pub(super) buf_h: u16,
    pub(super) top_margin: u16,
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

    // To the WALL VISUAL bottom, not the full top_margin: the rows between are
    // carpet apron, and blocking them pushed the walkable boundary south of the
    // visible wall base (invariant #6).
    mask.mark_blocked(
        0,
        0,
        buf_w,
        top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN),
        0,
    );
    const BASEBOARD_H: u16 = 3;

    let baseboard_top = buf_h.saturating_sub(BASEBOARD_H);
    mask.mark_blocked(0, baseboard_top, buf_w, BASEBOARD_H, 0);

    // Both block their FULL visual footprint (invariant #6); only the router
    // clearance is asymmetric — horizontal faces already fill a routing cell, while
    // vertical walls are thinner and take `WALL_ROUTING_MARGIN_X` westward.
    for seg in room_walls {
        let (origin, size) = wall_segment_rect(seg, top_margin, room_walls);
        let mx = if seg.start.x == seg.end.x {
            WALL_ROUTING_MARGIN_X
        } else {
            0
        };
        mask.mark_blocked(
            origin.x.saturating_sub(mx),
            origin.y,
            size.w.saturating_add(mx),
            size.h,
            0,
        );
    }

    for desk in home_desks {
        // WALK-BEHIND: the footprint is a shallow south strip at the sprite base, so
        // the monitor overhangs NORTH and a walker behind it is occluded by the desk's
        // own y-sorted sprite. Stamped TOP-LEFT — the desk pos IS its NW corner.
        let desk_def = super::decor::desk_furniture_def();
        stamp_ground(
            &mut mask,
            &desk_def,
            Anchor::TopLeft,
            *desk,
            OBSTACLE_PAD_PX,
        );
    }

    for trio in meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        // The sofa's pad is what gives vertical sit clearance — see its
        // `furniture_def` row.
        let sofa_def = furniture_def(Furniture::MeetingSofaBody);
        for sofa in trio.sofas {
            stamp_ground(&mut mask, &sofa_def, Anchor::Center, sofa, OBSTACLE_PAD_PX);
        }
        let table_def = furniture_def(Furniture::MeetingTable);
        stamp_ground(
            &mut mask,
            &table_def,
            Anchor::Center,
            trio.table,
            OBSTACLE_PAD_PX,
        );
    }

    if let Some(island) = kitchen_island {
        // South-anchored base only; the countertop rows overhang (walk-behind).
        let def = furniture_def(Furniture::KitchenIsland);
        stamp_ground(&mut mask, &def, Anchor::Center, island, OBSTACLE_PAD_PX);
    }

    for wp in waypoints {
        // `None` = meeting slots, which sit/stand on sofa/table furniture
        // already stamped above — no obstacle of their own.
        let Some(Size { w, h }) = super::approach::obstacle_footprint(wp.kind, pantry_counter_size)
        else {
            continue;
        };
        // WAYPOINT_STAMP_PAD_PX, not OBSTACLE_PAD_PX: waypoint furniture paints after
        // the characters, so a visitor is occluded and needs no clearance band.
        if matches!(wp.kind, WaypointKind::Pantry) {
            // Only the SOUTH base sits on the floor; cabinet tops and backsplash are
            // overhang (invariant #6). `stand_point` uses the FULL visual instead, so
            // the USER parks clear of the whole counter rather than inside it.
            let (tl, sz) = pantry_ground_rect(wp.pos, Size { w, h });
            mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, WAYPOINT_STAMP_PAD_PX);
            continue;
        }
        let def = furniture_def(wp.kind.furniture());
        let (tl, sz) = def.ground_rect_of(Anchor::Center, wp.pos, Size { w, h });
        mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, WAYPOINT_STAMP_PAD_PX);
    }

    for &PlantItem { kind, pos } in plants {
        let def = furniture_def(kind.furniture());
        stamp_ground(&mut mask, &def, Anchor::Center, pos, 1);
    }

    if let Some(lamp) = floor_lamp {
        let def = furniture_def(Furniture::FloorLamp);
        stamp_ground(&mut mask, &def, Anchor::Center, lamp, 1);
    }

    if let Some(t) = lounge_side_table {
        let def = furniture_def(Furniture::LoungeSideTable);
        stamp_ground(&mut mask, &def, Anchor::Center, t, 1);
    }

    if let Some(t) = fish_tank {
        let def = furniture_def(Furniture::FishTank);
        stamp_ground(&mut mask, &def, Anchor::Center, t, 1);
    }

    for &WallDecorItem { kind, pos } in wall_decor {
        // pad=1, not OBSTACLE_PAD_PX: these overhang nothing solid, and a 2px band
        // every side inflated the blocked rect back to the full sprite width.
        let def = furniture_def(kind.furniture());
        stamp_ground(&mut mask, &def, Anchor::TopLeft, pos, 1);
    }

    // PhoneBooth + StandingDesk double-block as waypoints too; `mark_blocked` is
    // idempotent. pad=1 because an extra pixel each side disconnects tight aisles.
    for &PodDecorItem { kind, pos } in pod_decor {
        let def = furniture_def(kind.furniture());
        stamp_ground(&mut mask, &def, Anchor::Center, pos, 1);
    }

    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{z_sort_row, WALL_THICK_V};

    #[test]
    fn vertical_wall_blocks_its_whole_visual_width_in_the_mask() {
        let l = crate::layout::SceneLayout::compute_with_seed(200, 130, Some(8), 0).unwrap();
        let seg = l
            .room_walls
            .iter()
            .find(|w| w.start.x == w.end.x)
            .copied()
            .expect("a vertical wall");
        let (o, s) = wall_segment_rect(&seg, l.top_margin, &l.room_walls);
        let y = o.y + s.h / 2; // deep in the wall body, clear of any north overhang
        for dx in 0..WALL_THICK_V {
            assert!(
                !l.is_walkable(seg.start.x + dx, y),
                "visual column {} must be blocked (no feet-in-wall)",
                seg.start.x + dx
            );
        }
    }

    #[test]
    fn ground_rect_blocks_exactly_its_declared_rect() {
        for anchor in [Anchor::TopLeft, Anchor::Center] {
            for gx in [GroundAlign::Start, GroundAlign::Center, GroundAlign::End] {
                for gy in [GroundAlign::Start, GroundAlign::Center, GroundAlign::End] {
                    let pos = Point { x: 20, y: 20 };
                    let fp = Size { w: 5, h: 3 };
                    let visual = Size { w: 8, h: 12 };
                    let mut mask = WalkableMask::new_open(40, 40);
                    let (tl, sz) = ground_rect(anchor, pos, fp, visual, gx, gy);
                    mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, 0);
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
    fn furniture_def_ground_rect_equals_the_primitive_for_every_row() {
        let pos = Point { x: 20, y: 20 };
        for &kind in Furniture::ALL {
            let def = furniture_def(kind);
            for anchor in [Anchor::TopLeft, Anchor::Center] {
                let expect = def
                    .footprint
                    .map(|fp| ground_rect(anchor, pos, fp, def.visual, def.ground_x, def.ground_y));
                assert_eq!(
                    def.ground_rect(anchor, pos),
                    expect,
                    "{kind:?} ground_rect diverged from the primitive at {anchor:?}"
                );
            }
        }
    }

    #[test]
    fn furniture_def_ground_rect_of_uses_an_explicit_footprint() {
        let def = furniture_def(Furniture::VendingMachine);
        let pos = Point { x: 12, y: 30 };
        let fp = Size { w: 7, h: 2 };
        assert_eq!(
            def.ground_rect_of(Anchor::Center, pos, fp),
            ground_rect(
                Anchor::Center,
                pos,
                fp,
                def.visual,
                def.ground_x,
                def.ground_y
            ),
        );
    }

    #[test]
    fn overhang_footprint_south_anchored_leaves_the_overhang_walkable() {
        // The fixture is a phone booth: a 6-wide sprite 12 tall over a 3-row
        // ground base.
        let mut mask = WalkableMask::new_open(40, 40);
        let pos = Point { x: 20, y: 20 };
        let (tl, sz) = ground_rect(
            Anchor::Center,
            pos,
            Size { w: 6, h: 3 },
            Size { w: 6, h: 12 },
            GroundAlign::Center,
            GroundAlign::End,
        );
        mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, 0);
        let south = z_sort_row(Anchor::Center, pos, 12);
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
        // TopLeft-anchored, and its ground footprint is the WHEEL SPAN at sprite
        // cols 2 and 11 — hence the probe offsets below.
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
        let sprite_h = def.visual.h;
        let base = pos.y + sprite_h - 1;
        assert!(
            !mask.is_walkable(pos.x + 2, base),
            "west wheel column must be blocked"
        );
        assert!(
            !mask.is_walkable(pos.x + 11, base),
            "east wheel column must be blocked"
        );
        assert!(
            mask.is_walkable(pos.x, base),
            "bare floor west of the wheels must stay walkable"
        );
        assert!(
            mask.is_walkable(pos.x + 13, base),
            "floor east of the wheels+pad must stay walkable"
        );
    }

    #[test]
    fn flat_box_footprint_is_centered_not_south_anchored() {
        // The fixture is a flat box: visual == footprint (vending/printer/table).
        let mut mask = WalkableMask::new_open(40, 40);
        let pos = Point { x: 20, y: 20 };
        let (tl, sz) = ground_rect(
            Anchor::Center,
            pos,
            Size { w: 4, h: 6 },
            Size { w: 4, h: 6 },
            GroundAlign::Center,
            GroundAlign::Center,
        );
        mask.mark_blocked(tl.x, tl.y, sz.w, sz.h, 0);
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
        // `GroundAlign::Center` is center-ON-pos `⌊v/2⌋−⌊f/2⌋`, not center-IN-box
        // `⌊(v−f)/2⌋`: they agree ONLY at equal parity. Every TopLeft kind today is
        // same-parity, so this FAILS the day one isn't and the 1px becomes a choice.
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

    #[test]
    fn pantry_south_strip_delegation_is_parity_safe() {
        // `pantry_ground_rect`'s south edge is `pos.y + ⌈counter.h/2⌉` where the
        // forked strip used `⌊counter.h/2⌋`: equal only for an EVEN counter.h. This
        // FAILS the day an odd-height one lands and the 1px becomes a choice.
        let mut counters = vec![crate::layout::rooms::pantry::COMPACT_COUNTER];
        // Sweep widths so the counter height comes from `compute_with_seed`, never a
        // copy of its literal. Duplicates are harmless — the assertion is idempotent.
        for w in (60u16..=260).step_by(20) {
            if let Some(l) = crate::layout::SceneLayout::compute_with_seed(w, 130, None, 0) {
                counters.push(l.pantry_counter_size());
            }
        }
        let pos = Point { x: 100, y: 60 };
        for counter in counters {
            let (tl, sz) = pantry_ground_rect(pos, counter);
            let delegated_south = tl.y + sz.h;
            let forked_south = pos.y + counter.h / 2;
            assert_eq!(
                delegated_south, forked_south,
                "pantry counter h={} is ODD: the south strip diverges 1px from \
                 the pre-refactor base anchor — decide the offset explicitly",
                counter.h
            );
        }
    }
}
