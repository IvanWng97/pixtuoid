//! Walkable-mask construction. Stamps every obstacle (walls, desks,
//! sofas, plants, decor) into a `WalkableMask` so A* knows where
//! characters can route. The padding constant `OBSTACLE_PAD_PX` adds a
//! clearance band around each obstacle so walkers don't scrape along
//! edges.

use super::{
    anchored_top_left, furniture_def, z_sort_row, Anchor, Furniture, PodDecor, Point, WallDecor,
    Waypoint, WaypointKind, OBSTACLE_PAD_PX, PANTRY_FOOTPRINT_DEPTH, WALL_BAND_TO_TOP_MARGIN,
};
use crate::walkable::WalkableMask;

/// Stamp a `(w, h)` furniture footprint into the mask, positioned by its
/// placement `anchor` (the ONE source for footprint origin — shared with the
/// renderer's sprite origin via `anchored_top_left`, so blocked ground and the
/// painted sprite can't drift). The `pad` clearance band is added on every side.
fn stamp_anchored(mask: &mut WalkableMask, anchor: Anchor, pos: Point, w: u16, h: u16, pad: u16) {
    let tl = anchored_top_left(anchor, pos, w, h);
    mask.mark_blocked(tl.x, tl.y, w, h, pad);
}

/// Stamp ONLY the south (ground-contact) `depth` rows of an ELEVATED furniture
/// whose `sprite_h`-tall sprite overhangs its floor base — the rolling
/// whiteboard's wheels under its panel (invariant #6). The strip's south edge is
/// the sprite's south base (`z_sort_row`, the same row the renderer y-sorts by),
/// so the block hugs the floor: a walker can pass BEHIND the panel and is
/// occluded by it (the overhang via z-sort + the `occludes_behind` back-cap). `w`
/// is the GROUND width, positioned by `anchor` like the full sprite — a plain
/// short footprint stamped `Center`/`TopLeft` would center on the panel instead,
/// lifting the block off the wheels.
fn stamp_south_strip(
    mask: &mut WalkableMask,
    anchor: Anchor,
    pos: Point,
    w: u16,
    sprite_h: u16,
    depth: u16,
    pad: u16,
) {
    let left = anchored_top_left(anchor, pos, w, sprite_h).x;
    let south = z_sort_row(anchor, pos, sprite_h);
    let depth = depth.min(sprite_h);
    // Saturating to match the inline pantry stamp's style: `south + 1` can't
    // realistically overflow (sprite_h is small, pos.y a fraction of buf_h) and
    // `south + 1 >= sprite_h >= depth` so the sub can't underflow — but keep the
    // arithmetic robust so a future large-sprite call site near the bottom edge
    // of a huge buffer can't wrap.
    let top = south.saturating_add(1).saturating_sub(depth);
    mask.mark_blocked(left, top, w, depth, pad);
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

#[allow(clippy::too_many_arguments)]
pub(super) fn build_walkable_mask(
    buf_w: u16,
    buf_h: u16,
    top_margin: u16,
    door: Option<Point>,
    home_desks: &[Point],
    meeting_sofas: &[Point],
    meeting_tables: &[Point],
    pantry_table: Option<Point>,
    pantry_chairs: &[Point],
    waypoints: &[Waypoint],
    plants: &[(super::PlantKind, Point)],
    floor_lamp: Option<Point>,
    lounge_side_table: Option<Point>,
    wall_decor: &[(WallDecor, Point)],
    pod_decor: &[(PodDecor, Point)],
    room_walls: &[(Point, Point)],
    pantry_counter_size: (u16, u16),
) -> WalkableMask {
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
    if let Some(d) = door {
        let cut_x = d.x.saturating_sub(2);
        let cut_h = top_margin.saturating_add(OBSTACLE_PAD_PX);
        mask.mark_walkable(cut_x, 0, 8, cut_h);
    }

    let baseboard_top = buf_h.saturating_sub(3);
    mask.mark_blocked(0, baseboard_top, buf_w, 3, 0);

    // Interior walls. Stardew-style fake-3D perspective:
    //   • horizontal walls (E-W) show their FACE — WALL_THICK_H px tall so the
    //     wall reads as having real mass/height when viewed from the north
    //     room (clearly thicker than the edge-on vertical).
    //   • vertical walls (N-S) are seen EDGE-ON — WALL_THICK_V px thin footprint
    //     (the renderer draws it 3 px wide; visual-wider-than-footprint per the
    //     top-down ground-projection rule).
    // Wall padding is ASYMMETRIC by orientation — driven by the coarse 4×4
    // router grid (`pathfind::cell_walkable`: a cell is walkable when ≥8 of its
    // 16 px are open), NOT by clearance:
    //   • HORIZONTAL (E-W) walls are WALL_THICK_H=6 px tall. 6 contiguous blocked
    //     px already fill a routing cell, so the wall is impassable with pad=0 —
    //     and you stand FLUSH against its south face, so any pad is pure red bloat
    //     (a 6px wall read as 10px). pad=0.
    //   • VERTICAL (N-S) walls are WALL_THICK_V=1 px edge-on (top-down ground
    //     projection, invariant #6). A 1px-wide blocked strip is INVISIBLE to the
    //     coarse grid — every straddling cell keeps ≥12/16 px walkable, so A*
    //     routes STRAIGHT THROUGH the wall. It needs OBSTACLE_PAD_PX (→5px blocked)
    //     to drive the wall's whole cell-column under the threshold. This is the
    //     original design: DOOR_GAP_V=14 is sized for "≥10px effective gap after
    //     [this] padding" (see compute_room_walls). The 1px FOOTPRINT is unchanged
    //     (characters still stand right next to the 3px visual); the pad is a
    //     routing-only clearance band, not a wider wall.
    for (start, end) in room_walls {
        if start.x == end.x {
            let seg_top = start.y.min(end.y);
            let seg_bot = start.y.max(end.y);
            // Mirror the renderer's stitch_vertical_wall: a segment whose top
            // is at top_margin plugs into the north window band — but the band
            // mask now ends WALL_BAND_TO_TOP_MARGIN px higher (the freed carpet
            // apron). Raise the wall's top to meet it, or a walkable slot opens
            // at the wall's top and A* threads between the rooms there (the wall
            // is DRAWN connecting to the band but the mask wouldn't block it).
            // Regression: vertical_wall_is_impassable_except_through_the_door.
            let seg_top = if seg_top == top_margin {
                top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN)
            } else {
                seg_top
            };
            mask.mark_blocked(
                start.x,
                seg_top,
                WALL_THICK_V,
                seg_bot - seg_top + 1,
                OBSTACLE_PAD_PX,
            );
        } else {
            mask.mark_blocked(
                start.x.min(end.x),
                start.y,
                start.x.abs_diff(end.x) + 1,
                WALL_THICK_H,
                0,
            );
        }
    }

    for desk in home_desks {
        // Block ONLY the desk surface — not the 8-px-above seated-character
        // zone. In a top-down 3/4 view a walker passing "behind" a desk row
        // is fine: the seated character paints in Pass 1, the walker also
        // paints in Pass 1 (occasional sprite overlap is acceptable), and
        // the desk paints in Pass 2 on top of both. Routes become much
        // shorter — walkers can cut diagonally between desk rows instead
        // of weaving around each one.
        // Desk footprint comes from the shared FurnitureDef (always Some for
        // the desk); stamped TOP-LEFT at the desk Point (not centred like
        // visited furniture).
        // Stamped TOP-LEFT at the desk Point (not centred like visited
        // furniture); the desk pos IS its NW corner.
        if let Some((w, h)) = super::decor::desk_furniture_def().footprint {
            stamp_anchored(&mut mask, Anchor::TopLeft, *desk, w, h, OBSTACLE_PAD_PX);
        }
    }

    for sofa in meeting_sofas {
        // Sofa BODY footprint from the table (16 ON PURPOSE: 16 + 2·pad = the
        // 20px sprite X footprint, with the pad giving vertical sit clearance —
        // see the furniture_def row). Top-down rule: walk up to its sides.
        if let Some((w, h)) = furniture_def(Furniture::MeetingSofaBody).footprint {
            stamp_anchored(&mut mask, Anchor::Center, *sofa, w, h, OBSTACLE_PAD_PX);
        }
    }

    for t in meeting_tables {
        if let Some((w, h)) = furniture_def(Furniture::MeetingTable).footprint {
            stamp_anchored(&mut mask, Anchor::Center, *t, w, h, OBSTACLE_PAD_PX);
        }
    }

    if let Some(t) = pantry_table {
        if let Some((w, h)) = furniture_def(Furniture::PantryTable).footprint {
            stamp_anchored(&mut mask, Anchor::Center, t, w, h, OBSTACLE_PAD_PX);
        }
    }
    for chair in pantry_chairs {
        // Small stool, stamped CENTERED on its pos like the other centered
        // furniture — was left/top-biased (offset 2), which blocked floor 1px
        // north & west of the 2×2 the painter actually draws.
        if let Some((w, h)) = furniture_def(Furniture::PantryChair).footprint {
            stamp_anchored(&mut mask, Anchor::Center, *chair, w, h, 1);
        }
    }

    for wp in waypoints {
        // Footprint sizes live in `approach::obstacle_footprint` (single source
        // of truth shared with `stand_point`). `None` = meeting slots, which
        // sit/stand on sofa/table furniture already stamped above — no obstacle.
        let Some((w, h)) = super::approach::obstacle_footprint(wp.kind, pantry_counter_size) else {
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
            // behind it is occluded by the back-cap (occludes_behind),
            // couch-style. `stand_point` still uses the FULL (w,h) so the agent
            // parks clear of the whole visual, not inside the upper sprite.
            let depth = PANTRY_FOOTPRINT_DEPTH.min(h);
            let south = wp.pos.y + h / 2;
            mask.mark_blocked(
                wp.pos.x.saturating_sub(w / 2),
                south.saturating_sub(depth),
                w,
                depth,
                1,
            );
            continue;
        }
        stamp_anchored(&mut mask, Anchor::Center, wp.pos, w, h, 1);
    }

    for (kind, p) in plants {
        // GROUND footprint from the table — tighter than the taller visual
        // sprite (top-down rule lets the leaves overhang).
        if let Some((w, h)) = furniture_def(kind.furniture()).footprint {
            stamp_anchored(&mut mask, Anchor::Center, *p, w, h, 1);
        }
    }

    if let Some(lamp) = floor_lamp {
        if let Some((w, h)) = furniture_def(Furniture::FloorLamp).footprint {
            stamp_anchored(&mut mask, Anchor::Center, lamp, w, h, 1);
        }
    }

    if let Some(t) = lounge_side_table {
        // Small footprint, pad=1: sits in the wide open lounge floor with
        // plenty of clearance.
        if let Some((w, h)) = furniture_def(Furniture::LoungeSideTable).footprint {
            stamp_anchored(&mut mask, Anchor::Center, t, w, h, 1);
        }
    }

    // Wall decor is top-left anchored. Only kinds with a ground footprint in
    // the furniture table are obstacles (the whiteboard); the rest are flush
    // against the wall (footprint None) and stamp nothing.
    for (kind, pos) in wall_decor {
        // pad=1 (not OBSTACLE_PAD_PX=2): the only WallDecor with a footprint is
        // the rolling whiteboard — an elevated board whose 10px wheel-base
        // overhangs nothing solid, so a 2px clearance band on every side just
        // inflated the blocked rect back to the 14px board width (hiding the
        // footprint shrink). Matches the pod-decor whiteboard's pad. The wheel
        // strip is SOUTH-anchored to the sprite base (the panel overhangs north).
        if let Some((w, depth)) = furniture_def(kind.furniture()).footprint {
            let sprite_h = furniture_def(kind.furniture()).visual.1;
            stamp_south_strip(&mut mask, Anchor::TopLeft, *pos, w, sprite_h, depth, 1);
        }
    }

    // Pod-aisle decor is centred at `pos`. All variants are obstacles.
    // PhoneBooth + StandingDesk are also waypoints — those entries
    // appear above in `waypoints` and double-block the same area;
    // mark_blocked is idempotent. Use pad=1 (not OBSTACLE_PAD_PX=2)
    // because aisles are tight (14×16) and an extra pixel of pad on
    // each side disconnects the routing grid through the aisle.
    for (kind, pos) in pod_decor {
        // GROUND footprint (not the sprite size) — a tall plant's canopy
        // overhangs its 6×6 pot base and must not block the aisle (invariant #6).
        let Some((w, h)) = furniture_def(kind.furniture()).footprint else {
            continue;
        };
        if matches!(kind, PodDecor::Whiteboard) {
            // Rolling board: the 8-px panel overhangs its 3-px wheel base, so
            // south-anchor the strip to the sprite base (a walker passes behind
            // the panel, occluded by it) instead of centering it on the panel.
            let sprite_h = furniture_def(kind.furniture()).visual.1;
            stamp_south_strip(&mut mask, Anchor::Center, *pos, w, sprite_h, h, 1);
        } else {
            stamp_anchored(&mut mask, Anchor::Center, *pos, w, h, 1);
        }
    }

    mask
}
