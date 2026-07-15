//! The wall's RENDER half — room-divider partitions drawn as frosted glass
//! (E-W horizontal + N-S vertical). This is the painter-side counterpart to the
//! wall's GEOMETRY half in `layout::rooms::walls` (thickness/footprint/joints);
//! the two stay bound by the shared `WALL_THICK_*` consts + `stitch_vertical_wall`
//! (single source, no drift — see `layout::rooms::walls`). This module owns the
//! whole render half: the paint fns (`paint_glass_wall_*`, `paint_door_jamb_*`)
//! AND the `enqueue_room_walls_*` that emit the y-sorted `RoomWall{H,V}` drawables
//! (the `drawable.rs` dispatch arms just delegate back here). The rendering WHY
//! lives in this header + the scene CLAUDE.md room-dividers entry ("How do the
//! room dividers render (frosted-glass partitions)?").

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::drawable::{Drawable, DrawableKind};
use super::palette::blend_over;
use crate::layout::{crossing_h_rows, stitch_vertical_wall, Layout, WallSegment};

// Room-divider frosted-glass partitions. The E-W (horizontal) wall shows its
// face — 6 px tall, kept in sync with `layout::WALL_THICK_H` — while the N-S
// (vertical) wall is seen edge-on at 4 px (its drawn width AND its blocked
// thickness — the edge-on width IS the real floor depth). The 3:2 ratio sells
// the top-down fake-3D. Each strip is a cool gradient (bright specular edge →
// tinted body → soft slate edge, all alpha-composited over what's behind so the
// room glows through) with a brighter seam every `GLASS_SEAM_STRIDE` px. BOTH
// orientations paint in the y-sorted drawable pass (`RoomWallH`/`RoomWallV`), so
// each composites over — frostily occluding — a walker standing behind it (the
// V wall reserves a north walk-behind cap in its MASK footprint,
// `layout::rooms::walls::WALL_TOP_OVERHANG_PX`, at a free terminus). The joint
// stitch that opens the gaps lives one layer down in
// `layout::rooms::walls::stitch_vertical_wall`, shared with the footprint so
// glass and blocked ground meet the joints identically.
//
// Both thicknesses DERIVE from the core mask consts so the visible glass face
// and the blocked ground footprint share one source of truth (can't drift — the
// vertical pair silently diverged at #559 when this was a hardcoded 4).
pub(super) const WALL_THICK_V_PX: u16 = crate::layout::WALL_THICK_V;
pub(super) const WALL_THICK_H_PX: u16 = crate::layout::WALL_THICK_H;
const GLASS_SEAM_STRIDE: u16 = 16;
/// Mullion (partition post) spacing along a glass run — every this-many px a
/// 1px darker post breaks the frosted slab so long walls (the dense solid
/// inter-meeting wall especially) read as panelled partitions instead of one
/// unbroken sheet (#559). Offset from the seam-glint stride so the two
/// rhythms interleave instead of colliding.
const MULLION_STRIDE: u16 = 10;
// The horizontal wall's frosted glass rises this many px NORTH of its walkable
// footprint — a "back cap" giving the wall height. Because the strip is
// y-sorted at its south (front) base, a character standing just north of the
// wall has their feet/legs composited behind this translucent cap (occluded
// behind the glass). The cap is over floor (visual only), not the mask.
//
// Derived from WALL_THICK_H_PX (the E-W wall face height) so the cap reaches
// into the legs of a walker at the northmost walkable row (footprint top `W`
// minus OBSTACLE_PAD+1 = `W-3`): the 12px sprite spans `W-15..W-3`, the cap
// covers `W-6..W-1`, so the bottom ~4px (feet + lower legs) read behind the
// pane. At the old value of 3 only the single feet row was grazed. Derived (not
// a bare 6) so retuning the wall face thickness moves the cap with it.
const GLASS_CAP_PX: u16 = WALL_THICK_H_PX;

fn glass_tones(theme: &crate::theme::Theme) -> (Rgb, Rgb, Rgb) {
    let tl = theme.office.room_wall_trim_light;
    (
        Rgb {
            r: tl.r.saturating_add(125),
            g: tl.g.saturating_add(135),
            b: tl.b.saturating_add(124),
        },
        Rgb {
            r: tl.r.saturating_add(70),
            g: tl.g.saturating_add(100),
            b: tl.b.saturating_add(116),
        },
        Rgb {
            r: tl.r.saturating_add(18),
            g: tl.g.saturating_add(52),
            b: tl.b.saturating_add(86),
        },
    )
}

/// Paint a horizontal (E-W) frosted-glass wall strip: lit top edge → body →
/// soft bottom edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_h(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x0: u16,
    x1: u16,
    y_top: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width(), buf.height());
    // The strip spans the back cap (rising north of the footprint) + the
    // 6 px face. Row 0 = lit far/top edge (north), last row = soft front base.
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    let rows = GLASS_CAP_PX + WALL_THICK_H_PX;
    for x in x0..=x1.min(bw.saturating_sub(1)) {
        let seam = (x - x0).is_multiple_of(GLASS_SEAM_STRIDE);
        // Interior posts only: a post AT a run end would double the door
        // frames / corner joints.
        let mullion = x > x0 && x < x1 && (x - x0).is_multiple_of(MULLION_STRIDE);
        for i in 0..rows {
            let y = cap_top + i;
            if y >= bh {
                continue;
            }
            let (g, a) = if mullion {
                (lo, 0.8)
            } else if seam {
                (hi, 0.55)
            } else if i == 0 {
                (hi, 0.82)
            } else if i == rows - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.58)
            };
            let color = blend_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Paint a vertical (N-S) frosted-glass wall strip: lit left edge → body →
/// soft right edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_v(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
    y_bot: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width(), buf.height());
    for y in y_top..=y_bot.min(bh.saturating_sub(1)) {
        let seam = (y - y_top).is_multiple_of(GLASS_SEAM_STRIDE);
        let mullion = y > y_top && y < y_bot && (y - y_top).is_multiple_of(MULLION_STRIDE);
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x >= bw {
                continue;
            }
            let (g, a) = if mullion {
                (lo, 0.8)
            } else if seam {
                (hi, 0.6)
            } else if dx == 0 {
                (hi, 0.85)
            } else if dx == WALL_THICK_V_PX - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.6)
            };
            let color = blend_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Jamb depth in px along the wall's axis — 2 reads as a solid post at
/// half-block scale without eating into the 14px opening.
pub(super) const DOOR_JAMB_PX: u16 = 2;

/// Paint one HORIZONTAL-wall door jamb: `DOOR_JAMB_PX` dark columns starting
/// at `x_left`, spanning the same cap+face strip the glass paints. Called
/// from the `RoomWallH` drawable arm for each segment end that abuts a
/// doorway (flagged at enqueue time — the paint pass has no layout access).
pub(super) fn paint_door_jamb_h(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
) {
    let dark = theme.office.room_wall_trim_dark;
    let (bw, bh) = (buf.width(), buf.height());
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    for x in x_left..(x_left + DOOR_JAMB_PX).min(bw) {
        for i in 0..(GLASS_CAP_PX + WALL_THICK_H_PX) {
            let y = cap_top + i;
            if y < bh {
                buf.put(x, y, dark);
            }
        }
    }
}

/// Paint one VERTICAL-wall door jamb: `DOOR_JAMB_PX` dark rows starting at
/// `y_top`, spanning the wall's `WALL_THICK_V_PX` columns from `x_left`. The
/// per-segment analog of the old `paint_door_frame_v` — called from the
/// `RoomWallV` drawable arm for the cut end that abuts a doorway (flagged at
/// enqueue: the paint pass has no layout access). The caller positions the two
/// possible jambs so the covered rows are byte-identical to the old frame: the
/// north jamb runs from the segment's top row down, the south jamb ends on its
/// bottom row (caller passes `y_bot - (DOOR_JAMB_PX - 1)`).
pub(super) fn paint_door_jamb_v(
    buf: &mut RgbBuffer,
    theme: &crate::theme::Theme,
    x_left: u16,
    y_top: u16,
) {
    let dark = theme.office.room_wall_trim_dark;
    let (bw, bh) = (buf.width(), buf.height());
    for y in y_top..(y_top + DOOR_JAMB_PX).min(bh) {
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x < bw {
                buf.put(x, y, dark);
            }
        }
    }
}

/// Horizontal (E-W) room dividers join the y-sort, anchored at their south
/// (front) edge so a character standing behind (north of) the wall is
/// composited over by the frosted glass rather than painting on top of it.
/// The vertical (edge-on) dividers join via [`enqueue_room_walls_v`].
/// Emitted LAST so a character tied with a wall row still paints behind it.
pub(super) fn enqueue_room_walls_h<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.y == end.y {
            let (x0, x1) = (start.x.min(end.x), start.x.max(end.x));
            // A cut end abutting a doorway gets a jamb — flagged HERE because
            // the paint pass has no layout access. gap.start == this
            // segment's x1 (the run was cut there), gap.end == a segment x0.
            let jamb_right = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.start.x == x1);
            let jamb_left = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.end.x == x0);
            drawables.push(Drawable {
                anchor_y: start.y + (WALL_THICK_H_PX - 1),
                kind: DrawableKind::RoomWallH {
                    x0,
                    x1,
                    y_top: start.y,
                    jamb_left,
                    jamb_right,
                },
            });
        }
    }
}

/// Vertical (N-S, edge-on) room dividers join the y-sort, anchored at their
/// raw SOUTH end — so a character standing north of the wall's north cap (the
/// visual-only overhang the mask leaves walkable) is composited behind the
/// frosted glass, matching the horizontal wall's walk-behind. Each segment
/// carries its own stitched `[y_top, y_bot]` for PAINT (the layout emits raw
/// geometry; the render offsets that plug the joints live in
/// `stitch_vertical_wall`) — but its z-key is the raw end, not the stitched
/// `y_bot`, so a corner where `y_bot` is extended into a crossing H wall still
/// paints H-over-V (see the enqueue for why). Plus its door-jamb flags (a cut
/// end abutting a doorway — flagged HERE, the paint pass has no layout access).
/// Emitted LAST, like the H walls, so a character tied with a wall row still
/// paints behind it.
pub(super) fn enqueue_room_walls_v<'a>(
    layout: &'a Layout,
    top_wall_h: u16,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.x != end.x {
            continue; // horizontal walls handled by enqueue_room_walls_h
        }
        // SAME x-filtered crossing rows the mask footprint uses (`wall_segment_rect`
        // rides the identical `crossing_h_rows`), so the painted glass and the
        // blocked ground bridge off the same H walls — no glass-vs-footprint drift
        // even under a future multi-column layout.
        let h_rows = crossing_h_rows(start.x, &layout.room_walls);
        let (y_top, y_bot) =
            stitch_vertical_wall(start.y, end.y, layout.top_margin, top_wall_h, &h_rows);
        // Jamb flags on the RAW cut ends (a door cut is never a stitch joint, so
        // the stitched y_top/y_bot the paint arm uses equal these): gap.start.y
        // == this segment's south end ⇒ south jamb; gap.end.y == its north end
        // ⇒ north jamb.
        let jamb_south = layout
            .doorways
            .iter()
            .any(|d| d.start.x == start.x && d.end.x == start.x && d.start.y == end.y);
        let jamb_north = layout
            .doorways
            .iter()
            .any(|d| d.start.x == start.x && d.end.x == start.x && d.end.y == start.y);
        drawables.push(Drawable {
            // z-key = the RAW south end, NOT the stitched `y_bot`. Where this
            // segment meets a crossing horizontal wall, the stitch extends
            // `y_bot` DOWN by the H wall's thickness to fill the inside-corner
            // L-notch — a paint hack, not real southward geometry. Anchoring at
            // that extended row would make the vertical glass paint OVER the
            // crossing H wall (and the pantry counter), breaking the clean corner
            // the background pass used to give (the H wall paints over the V).
            // The raw end (`< y_bot` only at a corner) keeps H painting last
            // there while the V still paints its extended notch-fill first.
            anchor_y: end.y,
            kind: DrawableKind::RoomWallV {
                x: start.x,
                y_top,
                y_bot,
                jamb_north,
                jamb_south,
            },
        });
    }
}
