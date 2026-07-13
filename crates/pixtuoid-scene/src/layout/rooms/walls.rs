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

use crate::layout::{pct, Bounds, MeetingRoom, Point, WallSegment, WALL_THICK_H};

/// Doorway width in ABSOLUTE pixels — a percentage shrinks to zero on small
/// terminals, which after the 2-px wall padding leaves no walkable cell for
/// A* and disconnects the room (the documented lesson behind the old
/// `DOOR_GAP_V`/`DOOR_GAP_H` pair; one value, one name). 14 gives ≥10 px
/// effective gap after padding — wide enough that the coarse 4×4 router
/// grid keeps at least one walkable row through the doorway.
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
) -> Vec<WallSegment> {
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

fn resolve(requests: Vec<WallRequest>) -> Vec<WallSegment> {
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
    for req in vs.into_iter().chain(hs) {
        emit(&req, &mut out);
    }
    out
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
fn emit(req: &WallRequest, out: &mut Vec<WallSegment>) {
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
        let walls = derive_room_walls(&rooms, None);
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
        let walls = derive_room_walls(&rooms, pantry);
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
    }

    /// A vertical run starting ON a horizontal wall's line starts below its
    /// stamped body (WALL_THICK_H), and its centered door re-centers on the
    /// TRIMMED run — the dense room-1 east wall's exact legacy geometry.
    #[test]
    fn vertical_run_trims_below_crossing_horizontal_wall() {
        let rooms = [room(0, 20, 40, 30), room(0, 50, 40, 30)];
        let walls = derive_room_walls(&rooms, None);
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
        assert!(derive_room_walls(&[], None).is_empty());
        assert!(derive_room_walls(
            &[],
            Some(Bounds {
                x: 0,
                y: 20,
                width: 40,
                height: 60,
            })
        )
        .is_empty());
    }
}
