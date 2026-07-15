//! Request-based room walls: each room DECLARES the edges it needs enclosed
//! (horizontal/vertical runs only) plus the doors it wants in them; the
//! resolver merges duplicate requests (two stacked rooms both request their
//! shared boundary — it renders as ONE wall), unions their door requests,
//! trims vertical runs below crossing horizontal wall bodies, and cuts the
//! gaps. Walls are therefore a FUNCTION of the room set — the old
//! `compute_room_walls` derived them from the same scalars the rooms were
//! computed from (parallel geometry, drift-prone; the #556 bookshelf-pierce
//! bug was exactly a "decor can't see the wall" blind spot).
//!
//! Door policy is the ROOMS' (owner call, #557 grill): a meeting room opens
//! a centered door in its east (corridor) wall; the pantry opens the
//! meeting↔pantry door at 60% of the shared wall; two stacked meeting rooms
//! declare NO door on their shared wall — it renders solid (each room has
//! its own corridor door, so connectivity holds; pinned by the sweep's BFS).

use crate::layout::decor::GroundAlign;
use crate::layout::mask::ground_rect;
use crate::layout::{
    pct, Anchor, Bounds, MeetingRoom, Point, Size, WallSegment, WALL_BAND_TO_TOP_MARGIN,
};

// ─── Wall geometry: dimensions + footprint (the collision half of a wall) ───
// A wall is modelled as a linear furniture piece — this module owns BOTH where
// its segments are (`derive_room_walls`, below) AND how thick / where-blocked
// each segment is (`WallDef` + `wall_segment_rect`, here). The render half (glass
// + occlusion) lives one layer up in `pixel_painter` (crate boundary: `layout`
// stays render-agnostic). The mask stamps these rects; the painter paints the
// SAME joints via the shared `stitch_vertical_wall`.

/// Walkable footprint (and render face height) of a horizontal (E-W) interior
/// wall, in px. The renderer derives `WALL_THICK_H_PX` from this so the visible
/// glass face and the blocked ground footprint can never drift apart.
pub const WALL_THICK_H: u16 = 6;
/// Thickness of a vertical (N-S) interior wall, in px — its blocked footprint
/// width AND its drawn width (they are EQUAL: seen edge-on, the width you draw
/// IS the wall's real floor thickness, so a walker collides with what they see).
/// The renderer's `WALL_THICK_V_PX` derives from this — ONE source, exactly like
/// `WALL_THICK_H`. (Was 1px "edge-on line" + a symmetric routing pad; that
/// footprint decoupled from the visual and DRIFTED when #559 widened the visual
/// to 4px — feet-in-wall on the east, phantom blocked floor on the west. The
/// coarse-router clearance the old pad bought is now the X-only
/// `mask::WALL_ROUTING_MARGIN_X` stamped at mask time, not baked into the footprint.)
pub const WALL_THICK_V: u16 = 4;

/// North-end walk-behind overhang for a FREE vertical terminus (a segment whose
/// north end is NOT on a joint — e.g. the run below a door): the top
/// `WALL_TOP_OVERHANG_PX` rows of the glass are visual-only (walkable), so a
/// character parked behind the wall's top cap is occluded by the y-sorted
/// `RoomWallV` drawable — the furniture walk-behind shape (`GroundAlign::End`,
/// invariant #6), now that the vertical wall joins the y-sort. Sized to match the
/// E-W wall's `GLASS_CAP_PX` (`WALL_THICK_H`): a 2px cap only grazed a walker's
/// feet, so a walk-behind past the wall's top read as clipping, not depth — 6px
/// reaches the lower body. A JOINTED top (window band OR a crossing horizontal
/// wall — anything `stitch` raised, so `y_top != seg_top`) is EXEMPT (cap 0, full
/// coverage): it has a wall/the band above it (no free floor behind), and a trim
/// there reopens either the A*-threads-the-wall-top gap or the divider corner
/// hole. A door-terminus segment shorter than the cap keeps `WALL_THICK_V` rows
/// blocked (the `min` guard in `wall_segment_rect`) so the divider never vanishes.
pub(crate) const WALL_TOP_OVERHANG_PX: u16 = WALL_THICK_H;

/// A linear wall's geometry policy — the wall analog of a `FurnitureDef` row.
/// A wall is not a point-centred fixed-size piece (its length is per-SEGMENT,
/// set by the room), so it can't be a `Furniture` enum row; but its BLOCKED-AREA
/// LOGIC is identical to furniture's — `footprint ⊆ visual`, the far-side (north)
/// `cap` visual-only (the wall's height projected up-screen = a walk-behind
/// overhang), south-anchored (`GroundAlign::End`), stamped through the SAME
/// `ground_rect`. Each segment builds its own rect from this policy + its own
/// length, so a door gap is just the ABSENCE of a segment — no monolithic-wall
/// special-casing.
#[derive(Clone, Copy)]
pub(crate) struct WallDef {
    /// Blocked thickness — the wall's real floor depth (the short axis).
    pub(crate) thickness: u16,
    /// Visual-only overhang toward the far (north) side: `footprint = visual −
    /// cap`. For the E-W wall it is the height back-cap; for a FREE N-S terminus
    /// it is the walk-behind top cap (a BAND-connected N-S top overrides it to 0).
    pub(crate) cap: u16,
}

/// E-W divider: a `WALL_THICK_H` face + an equal north height back-cap.
pub(crate) const WALL_H: WallDef = WallDef {
    thickness: WALL_THICK_H,
    cap: WALL_THICK_H,
};
/// N-S divider: `WALL_THICK_V` edge-on thickness; a FREE terminus reserves a
/// `WALL_TOP_OVERHANG_PX` north walk-behind cap (band-connected ⇒ cap 0).
pub(crate) const WALL_V: WallDef = WallDef {
    thickness: WALL_THICK_V,
    cap: WALL_TOP_OVERHANG_PX,
};

/// How far BELOW a horizontal wall's row a vertical segment's north end may sit
/// and still bridge UP to it (`derive_room_walls` offsets a lower segment
/// ~`WALL_THICK_H` px to clear the cross wall's body; the slack absorbs the
/// off-by-one of that offset). Named ONCE so the stitch and the placement sweep's
/// bridge re-derivation can't drift apart (two copies of a bridge tolerance is
/// the magic-number-drift class this repo hunts).
pub(crate) const WALL_BRIDGE_SLACK_PX: u16 = 2;

/// The horizontal-wall rows that CROSS a vertical run at column `x` — the
/// `h_rows` stitch INPUT. Shared by the mask footprint (`wall_segment_rect`) and
/// the painter (`enqueue_room_walls_v`) so "shared `stitch_vertical_wall`" also
/// means shared INPUTS: without the x-filter on BOTH sides, a future multi-column
/// layout would bridge/extend the painted glass off a crossing wall in ANOTHER
/// column that the mask footprint ignores — the exact glass-vs-footprint drift
/// this refactor kills, reopened in the painter direction. Today the office is
/// single-column so the filter is a no-op; the sharing keeps it honest.
pub(crate) fn crossing_h_rows(x: u16, room_walls: &[WallSegment]) -> Vec<u16> {
    room_walls
        .iter()
        .filter(|w| {
            w.start.y == w.end.y && (w.start.x.min(w.end.x)..=w.start.x.max(w.end.x)).contains(&x)
        })
        .map(|w| w.start.y)
        .collect()
}

/// Stitch a vertical (N-S) wall segment's raw `[seg_top, seg_bot]` to its joints
/// — the terminal-agnostic layout emits raw geometry; the thicknesses/offsets
/// that plug the render AND the mask gaps live HERE, so the painted glass and the
/// blocked footprint meet the SAME joints (one source, no drift — the pixel
/// painter's `enqueue_room_walls_v` and this module's `wall_segment_rect` both
/// call it, over the SAME `crossing_h_rows` input):
///   • Top: a segment starting at `top_margin` abuts the north window band, which
///     ends `WALL_BAND_TO_TOP_MARGIN` px higher at `top_wall_h` — raise it so no
///     floor shows between window and wall (and A* can't thread the top). A
///     segment sitting just below a horizontal wall (the dual-meeting layout
///     offsets its lower segment ~`WALL_THICK_H` px to clear the cross wall — see
///     `derive_room_walls`) is bridged up to meet it.
///   • Bottom: where the vertical meets a horizontal wall, extend it down by the
///     horizontal's thickness to fill the inside corner (else its east columns
///     leave an L-notch — a walkable bite out of the divider in the mask, a
///     floor sliver in the render).
/// A caller detects a stitched (jointed) top as `y_top != seg_top`: that is
/// exactly when the walk-behind cap must be DROPPED (a jointed top has a wall or
/// the band above it, no free floor for a walker to stand behind).
pub(crate) fn stitch_vertical_wall(
    seg_top: u16,
    seg_bot: u16,
    top_margin: u16,
    top_wall_h: u16,
    h_rows: &[u16],
) -> (u16, u16) {
    let y_top = if seg_top == top_margin {
        top_wall_h
    } else if let Some(&hr) = h_rows
        .iter()
        .find(|&&hr| hr < seg_top && seg_top - hr <= WALL_THICK_H + WALL_BRIDGE_SLACK_PX)
    {
        hr
    } else {
        seg_top
    };
    let y_bot = if h_rows.contains(&seg_bot) {
        seg_bot + (WALL_THICK_H - 1)
    } else {
        seg_bot
    };
    (y_top, y_bot)
}

/// A wall segment's PHYSICAL blocked rect (origin + size), shared by the mask
/// stamp and the placement sweep so the two can't disagree on wall geometry.
/// Each segment is a `WallDef` piece: `footprint = visual − north cap`,
/// south-anchored, through the SAME `ground_rect` furniture rides. The vertical
/// visual box is `stitch_vertical_wall`'s `[y_top, y_bot]` — the SAME joints the
/// glass paints — so the blocked footprint and the drawn wall meet the band /
/// crossing walls identically (no drift, no corner hole).
pub(crate) fn wall_segment_rect(
    seg: &WallSegment,
    top_margin: u16,
    room_walls: &[WallSegment],
) -> (Point, Size) {
    let (start, end) = (seg.start, seg.end);
    if start.x == end.x {
        // VERTICAL (N-S): run along Y, edge-on. `WALL_V` policy — `footprint =
        // visual − north cap`, south-anchored. The cap is reserved ONLY for a
        // FREE north terminus (a run below a door, half-space above it): a top
        // that `stitch` raised to a joint (the window band OR a crossing
        // horizontal wall) has no free floor behind it, so `y_top != seg_top`
        // ⇒ cap 0 (else the overhang leaves a walkable notch BETWEEN the two
        // walls' footprints — a hole straight through the divider).
        let def = WALL_V;
        let seg_top = start.y.min(end.y);
        let seg_bot = start.y.max(end.y);
        let h_rows = crossing_h_rows(start.x, room_walls);
        let top_wall_h = top_margin.saturating_sub(WALL_BAND_TO_TOP_MARGIN);
        let (visual_top, visual_bot) =
            stitch_vertical_wall(seg_top, seg_bot, top_margin, top_wall_h, &h_rows);
        let visual = Size {
            w: def.thickness,
            h: visual_bot - visual_top + 1,
        };
        // Free top ⇒ reserve the walk-behind cap, but never eat the whole
        // segment: keep at least `WALL_THICK_V` rows blocked so a short run below
        // a door stays a divider, not a second opening.
        let cap = if visual_top == seg_top {
            def.cap.min(visual.h.saturating_sub(WALL_THICK_V))
        } else {
            0
        };
        let fp = Size {
            w: def.thickness, // footprint == visual in X (edge-on, no x overhang)
            h: visual.h.saturating_sub(cap),
        };
        ground_rect(
            Anchor::TopLeft,
            Point {
                x: start.x,
                y: visual_top,
            },
            fp,
            visual,
            GroundAlign::Start,
            GroundAlign::End, // south-anchored → north cap overhangs (walk-behind)
        )
    } else {
        // HORIZONTAL (E-W): run along X, face-on. `WALL_H` policy — the visual
        // rises `cap` px NORTH of the blocked face (the glass height back-cap);
        // `footprint = the south `thickness` face`, south-anchored. The returned
        // blocked rect is byte-identical to the pre-WallDef hand-rolled face
        // (the cap only positions the footprint, it is never blocked).
        let def = WALL_H;
        let visual = Size {
            w: start.x.abs_diff(end.x) + 1,
            h: def.thickness + def.cap,
        };
        let fp = Size {
            w: visual.w,
            h: def.thickness,
        };
        ground_rect(
            Anchor::TopLeft,
            Point {
                x: start.x.min(end.x),
                y: start.y.saturating_sub(def.cap),
            },
            fp,
            visual,
            GroundAlign::Start,
            GroundAlign::End,
        )
    }
}

/// An opening the resolver CUT into a wall run. The resolver is the one
/// place that knows every door (it holds the `DoorAt` requests), so it hands the
/// openings to the renderer instead of the painter re-inferring them from
/// segment adjacency (#559 — door frames + future doorway dressing draw
/// from this). Axis is implicit: `start.x == end.x` ⇒ a vertical wall's
/// doorway (the span is in y), else horizontal (span in x).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Doorway {
    pub start: Point,
    pub end: Point,
}

/// Doorway width in ABSOLUTE pixels — a percentage shrinks to zero on small
/// terminals, which after the 2-px wall padding leaves no walkable cell for
/// A* and disconnects the room (the documented lesson behind the old
/// `DOOR_GAP_V`/`DOOR_GAP_H` pair; one value, one name). 14 opens a 13-px gap
/// (the segment cuts are endpoint-inclusive, so the opening is `ge-gs-1`), and
/// after the 2-px vertical-wall padding each side that's a 9-px effective gap —
/// still wide enough that the coarse 4×4 router keeps ≥1 walkable row through it.
const DOOR_GAP: u16 = 14;

/// Where along its wall run a door sits.
enum DoorAt {
    /// Midpoint of the (trimmed) run — the meeting room's corridor door.
    Centered,
    /// `pct(run length, p)` from the run's start — the pantry's 60% door.
    Pct(u16),
}

/// One straight enclosure run a room asks for. Axis-aligned only — the
/// office has no diagonal walls (owner-stated simplification).
enum Run {
    /// Vertical wall at `x`, spanning `y0..y1`.
    V { x: u16, y0: u16, y1: u16 },
    /// Horizontal wall at `y`, spanning `x0..x1`.
    H { y: u16, x0: u16, x1: u16 },
}

struct WallRequest {
    run: Run,
    doors: Vec<DoorAt>,
}

/// Derive every interior wall from the rooms themselves. `pantry` is the
/// pantry's BOUNDS (the wall pass runs before the island is placed, so the
/// full `PantryRoom` doesn't exist yet — walls only need geometry).
pub(crate) fn derive_room_walls(
    meeting_rooms: &[MeetingRoom],
    pantry: Option<Bounds>,
) -> (Vec<WallSegment>, Vec<Doorway>) {
    let mut requests: Vec<WallRequest> = Vec::new();

    // Each meeting room: an east (corridor) wall with a centered door, and —
    // when another room sits directly below — its half of the shared
    // boundary. The pantry requests the OTHER half plus the 60% door; a
    // lower MEETING room requests its half with NO door (owner rule: two
    // meeting rooms don't interconnect).
    for (i, room) in meeting_rooms.iter().enumerate() {
        let b = room.bounds;
        requests.push(WallRequest {
            run: Run::V {
                x: b.x + b.width,
                y0: b.y,
                y1: b.y + b.height,
            },
            doors: vec![DoorAt::Centered],
        });
        let south = Run::H {
            y: b.y + b.height,
            x0: b.x,
            x1: b.x + b.width,
        };
        let below_meeting = meeting_rooms
            .get(i + 1)
            .is_some_and(|r| stacked(b, r.bounds));
        let below_pantry = pantry.is_some_and(|p| stacked(b, p));
        if below_meeting || below_pantry {
            requests.push(WallRequest {
                run: south,
                doors: vec![],
            });
        }
        if i > 0 && stacked(meeting_rooms[i - 1].bounds, b) {
            requests.push(WallRequest {
                run: Run::H {
                    y: b.y,
                    x0: b.x,
                    x1: b.x + b.width,
                },
                doors: vec![], // meeting↔meeting: solid (no door)
            });
        }
    }
    if let Some(p) = pantry {
        let above_meeting = meeting_rooms.iter().any(|r| stacked(r.bounds, p));
        if above_meeting {
            requests.push(WallRequest {
                run: Run::H {
                    y: p.y,
                    x0: p.x,
                    x1: p.x + p.width,
                },
                doors: vec![DoorAt::Pct(60)],
            });
        }
        // No east wall request AT ALL — "the counter is the boundary" is the
        // pantry's honest shape, not a special case in the wall code.
    }

    resolve(requests)
}

/// `below` sits directly under `above` (same column, touching edges) — the
/// shared-boundary adjacency test.
fn stacked(above: Bounds, below: Bounds) -> bool {
    below.y == above.y + above.height && below.x == above.x && below.width == above.width
}

fn resolve(requests: Vec<WallRequest>) -> (Vec<WallSegment>, Vec<Doorway>) {
    // 1. Merge duplicate/overlapping collinear runs, unioning their doors.
    //    Runs that merely TOUCH end-to-end stay separate: each keeps its own
    //    door (two stacked meeting rooms' east walls touch at the split line
    //    but are two walls with two corridor doors, matching the old
    //    geometry). Only same-span duplicates — the shared boundary
    //    requested from both sides — collapse.
    let mut merged: Vec<WallRequest> = Vec::new();
    'outer: for req in requests {
        for m in &mut merged {
            if same_run(&m.run, &req.run) {
                m.doors.extend(req.doors);
                continue 'outer;
            }
        }
        merged.push(req);
    }

    // 2. Trim: a vertical run STARTING on a horizontal wall's line begins
    //    below that wall's stamped body instead (horizontal walls stamp
    //    WALL_THICK_H rows downward with pad 0; starting inside them would
    //    double-stamp and de-sync the renderer's stitch-up tolerance, which
    //    is defined AS WALL_THICK_H — see `stitch_vertical_wall`).
    let h_runs: Vec<(u16, u16, u16)> = merged
        .iter()
        .filter_map(|r| match r.run {
            Run::H { y, x0, x1 } => Some((y, x0, x1)),
            Run::V { .. } => None,
        })
        .collect();
    for req in &mut merged {
        if let Run::V { x, y0, .. } = &mut req.run {
            // Same line AND the horizontal run actually reaches this
            // column — a coincidental same-y wall in another column must
            // not trim (single-column today, so this is the honest form
            // of "crossing", not a behavior change).
            if h_runs
                .iter()
                .any(|&(y, x0, x1)| y == *y0 && (x0..=x1).contains(x))
            {
                *y0 += WALL_THICK_H;
            }
        }
    }

    // 3. Cut door gaps and emit, vertical runs first (the render/mask order
    //    the old fn produced).
    let (vs, hs): (Vec<_>, Vec<_>) = merged
        .into_iter()
        .partition(|r| matches!(r.run, Run::V { .. }));
    let mut out = Vec::new();
    let mut doorways = Vec::new();
    for req in vs.into_iter().chain(hs) {
        emit(&req, &mut out, &mut doorways);
    }
    (out, doorways)
}

fn same_run(a: &Run, b: &Run) -> bool {
    match (a, b) {
        (
            Run::V { x, y0, y1 },
            Run::V {
                x: x2,
                y0: y02,
                y1: y12,
            },
        ) => x == x2 && y0 == y02 && y1 == y12,
        (
            Run::H { y, x0, x1 },
            Run::H {
                y: y2,
                x0: x02,
                x1: x12,
            },
        ) => y == y2 && x0 == x02 && x1 == x12,
        _ => false,
    }
}

/// Cut the run's door gaps and push the remaining wall pieces. Degenerate
/// (zero-length) pieces are pushed too — the mask stamp of an empty segment
/// is a no-op and the old fn emitted them unconditionally (kept for exact
/// behavior equality).
fn emit(req: &WallRequest, out: &mut Vec<WallSegment>, doorways: &mut Vec<Doorway>) {
    let (start, end) = match req.run {
        Run::V { x: _, y0, y1 } => (y0, y1),
        Run::H { y: _, x0, x1 } => (x0, x1),
    };
    let len = end.saturating_sub(start);
    // Today a run carries at most ONE door (meeting east / pantry north);
    // a doorless run emits whole. Fail LOUD if a future policy unions a
    // second door onto a shared run — silently dropping a requested
    // opening would read as a sealed room.
    debug_assert!(
        req.doors.len() <= 1,
        "multi-door runs are not implemented; a request was dropped"
    );
    let gap = req.doors.first().map(|at| {
        let center = match at {
            DoorAt::Centered => start + len / 2,
            DoorAt::Pct(p) => start + pct(len, *p),
        };
        (
            center.saturating_sub(DOOR_GAP / 2),
            (center + DOOR_GAP / 2).min(end),
        )
    });
    if let Some((gs, ge)) = gap {
        doorways.push(match req.run {
            Run::V { x, .. } => Doorway {
                start: Point { x, y: gs },
                end: Point { x, y: ge },
            },
            Run::H { y, .. } => Doorway {
                start: Point { x: gs, y },
                end: Point { x: ge, y },
            },
        });
    }
    let spans: Vec<(u16, u16)> = match gap {
        Some((gs, ge)) => vec![(start, gs), (ge, end)],
        None => vec![(start, end)],
    };
    for (s, e) in spans {
        out.push(match req.run {
            Run::V { x, .. } => WallSegment {
                start: Point { x, y: s },
                end: Point { x, y: e },
            },
            Run::H { y, .. } => WallSegment {
                start: Point { x: s, y },
                end: Point { x: e, y },
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::MeetingTrio;

    #[test]
    fn vertical_wall_free_terminus_reserves_a_north_walk_behind_cap() {
        // FREE terminus (north end NOT on the band — e.g. the run below a door):
        // full 4px width (no phantom floor west, no feet-in-wall east), but the
        // north `WALL_TOP_OVERHANG_PX` rows are a visual-only walk-behind cap —
        // south-anchored footprint, composited over by the y-sorted `RoomWallV`.
        let top_margin = 20;
        let seg = WallSegment {
            start: Point { x: 56, y: 60 },
            end: Point { x: 56, y: 100 },
        };
        let (o, s) = wall_segment_rect(&seg, top_margin, &[]);
        assert_eq!(o.x, 56, "west edge sits at start.x (no west bleed)");
        assert_eq!(s.w, WALL_THICK_V, "footprint width == the drawn width");
        assert_eq!(
            o.y,
            60 + WALL_TOP_OVERHANG_PX,
            "north cap trimmed (south-anchored)"
        );
        assert_eq!(
            s.h,
            (100 - 60 + 1) - WALL_TOP_OVERHANG_PX,
            "height == visual − cap"
        );
    }

    #[test]
    fn vertical_wall_on_the_window_band_is_full_height_and_plugged() {
        // A north end AT top_margin plugs into the window band: raised UP by
        // WALL_BAND_TO_TOP_MARGIN so no routable floor shows between the band and
        // the glass (else A* threads the wall top). Full height, like every
        // vertical segment.
        let top_margin = 20;
        let seg = WallSegment {
            start: Point {
                x: 56,
                y: top_margin,
            },
            end: Point { x: 56, y: 80 },
        };
        let (o, s) = wall_segment_rect(&seg, top_margin, &[]);
        assert_eq!(o.x, 56);
        assert_eq!(s.w, WALL_THICK_V);
        assert_eq!(
            o.y,
            top_margin - WALL_BAND_TO_TOP_MARGIN,
            "plugged up to the band"
        );
        assert_eq!(
            s.h,
            80 - (top_margin - WALL_BAND_TO_TOP_MARGIN) + 1,
            "full height, no north trim"
        );
    }

    #[test]
    fn vertical_wall_below_a_crossing_wall_drops_its_north_cap() {
        // Regression (walkable-map hole): a vertical segment whose north end
        // butts a crossing horizontal wall must NOT reserve the walk-behind cap —
        // it starts WALL_THICK_H below the H-wall's line (the `resolve` trim), so
        // a cap would leave WALL_TOP_OVERHANG_PX walkable rows BETWEEN the two
        // walls' footprints: a hole straight through the divider. `stitch` bridges
        // the blocked top UP to the H-wall row (fully overlapping it, capless), so
        // the divider is solid across the join.
        let hwall = WallSegment {
            start: Point { x: 40, y: 50 },
            end: Point { x: 56, y: 50 },
        };
        // Trimmed lower segment: starts WALL_THICK_H below the H wall's row.
        let vseg = WallSegment {
            start: Point {
                x: 56,
                y: 50 + WALL_THICK_H,
            },
            end: Point { x: 56, y: 100 },
        };
        let (capless, _) = wall_segment_rect(&vseg, 20, &[hwall, vseg]);
        assert_eq!(
            capless.y, 50,
            "north end abuts the H wall ⇒ no cap, blocked top BRIDGED onto the H wall row"
        );
        // Same segment with NO crossing wall keeps its free-terminus cap.
        let (capped, _) = wall_segment_rect(&vseg, 20, &[vseg]);
        assert_eq!(
            capped.y,
            50 + WALL_THICK_H + WALL_TOP_OVERHANG_PX,
            "a genuinely free north terminus still reserves the walk-behind cap"
        );
    }

    #[test]
    fn horizontal_wall_rect_is_full_face_unchanged() {
        // Routed through the same ground_rect for uniformity — geometry must be
        // byte-identical to the pre-refactor hand-rolled rect.
        let seg = WallSegment {
            start: Point { x: 20, y: 50 },
            end: Point { x: 60, y: 50 },
        };
        let (o, s) = wall_segment_rect(&seg, 20, &[]);
        assert_eq!((o.x, o.y), (20, 50));
        assert_eq!((s.w, s.h), (60 - 20 + 1, WALL_THICK_H));
    }

    fn room(x: u16, y: u16, w: u16, h: u16) -> MeetingRoom {
        MeetingRoom {
            bounds: Bounds {
                x,
                y,
                width: w,
                height: h,
            },
            trio: None::<MeetingTrio>,
        }
    }

    /// The owner-named constraint: two stacked meeting rooms' shared
    /// boundary is requested from BOTH sides but resolves to ONE wall — and
    /// per the door policy it is SOLID (no gap).
    #[test]
    fn dense_shared_wall_resolves_once_and_solid() {
        let rooms = [room(0, 20, 40, 30), room(0, 50, 40, 30)];
        let (walls, _) = derive_room_walls(&rooms, None);
        let h: Vec<_> = walls.iter().filter(|w| w.start.y == w.end.y).collect();
        assert_eq!(h.len(), 1, "one horizontal wall, not two: {h:?}");
        assert_eq!(
            (h[0].start.x, h[0].end.x),
            (0, 40),
            "solid across the full span — no inter-meeting door"
        );
    }

    /// Meeting + pantry: the shared wall keeps the pantry's 60% door, and
    /// every ENCLOSED room keeps at least one door (the meeting room's
    /// centered east door) — the connectivity floor of the door policy.
    #[test]
    fn pantry_door_survives_and_every_enclosed_room_has_a_door() {
        let rooms = [room(0, 20, 40, 30)];
        let pantry = Some(Bounds {
            x: 0,
            y: 50,
            width: 40,
            height: 30,
        });
        let (walls, doorways) = derive_room_walls(&rooms, pantry);
        let h: Vec<_> = walls.iter().filter(|w| w.start.y == w.end.y).collect();
        assert_eq!(h.len(), 2, "the 60% door splits the shared wall: {h:?}");
        let gap = (h[0].end.x, h[1].start.x);
        let door_center = pct(40, 60);
        assert_eq!(
            gap,
            (door_center - DOOR_GAP / 2, door_center + DOOR_GAP / 2)
        );
        let v: Vec<_> = walls.iter().filter(|w| w.start.x == w.end.x).collect();
        assert_eq!(v.len(), 2, "east wall split by the centered door");
        assert!(
            v[0].end.y < v[1].start.y,
            "a real gap exists — the meeting room is never sealed"
        );
        // The resolver HANDS both openings to the renderer (#559): one per
        // cut, spans exactly matching the segment gaps above.
        assert_eq!(doorways.len(), 2, "one Doorway per cut opening");
        let v_door = doorways
            .iter()
            .find(|d| d.start.x == d.end.x)
            .expect("east door");
        assert_eq!((v_door.start.y, v_door.end.y), (v[0].end.y, v[1].start.y));
        let h_door = doorways
            .iter()
            .find(|d| d.start.y == d.end.y)
            .expect("60% door");
        assert_eq!((h_door.start.x, h_door.end.x), gap);
    }

    /// A vertical run starting ON a horizontal wall's line starts below its
    /// stamped body (WALL_THICK_H), and its centered door re-centers on the
    /// TRIMMED run — the dense room-1 east wall's exact legacy geometry.
    #[test]
    fn vertical_run_trims_below_crossing_horizontal_wall() {
        let rooms = [room(0, 20, 40, 30), room(0, 50, 40, 30)];
        let (walls, _) = derive_room_walls(&rooms, None);
        let v: Vec<_> = walls.iter().filter(|w| w.start.x == w.end.x).collect();
        // room 0's pair spans [20, 50]; room 1's pair starts BELOW the wall.
        assert_eq!(v[0].start.y, 20);
        assert_eq!(v[1].end.y, 50);
        let trimmed_top = 50 + WALL_THICK_H;
        assert_eq!(v[2].start.y, trimmed_top, "trimmed below the shared wall");
        assert_eq!(v[3].end.y, 80);
        let c = trimmed_top + (80 - trimmed_top) / 2;
        assert_eq!(
            (v[2].end.y, v[3].start.y),
            (c - DOOR_GAP / 2, c + DOOR_GAP / 2),
            "door centers on the trimmed run (legacy v2_center)"
        );
    }

    /// No rooms, or a pantry with nothing above it (open-plan) → no walls.
    #[test]
    fn open_plan_requests_nothing() {
        assert!(derive_room_walls(&[], None).0.is_empty());
        let (w, d) = derive_room_walls(
            &[],
            Some(Bounds {
                x: 0,
                y: 20,
                width: 40,
                height: 60,
            }),
        );
        assert!(w.is_empty() && d.is_empty());
    }
}
