//! Ambient wandering creatures — the office pet and the OpenClaw gateway mascot —
//! and WHERE they roam each frame. This is sim/behaviour: `pixel_painter` consumes
//! the positions produced here and paints them (the "scene decides, painter draws"
//! contract). The pet and the gateway mascot DELIBERATELY share their roaming
//! toolkit (the visit-spot geometry + the no-flash `walk_between`) so `pet_position`
//! and `mascot_spots` can't drift; kept together until a second pet/mascot makes a
//! per-entity split pay for itself (moved wholesale out of `pixel_painter/drawable.rs`).

use std::time::SystemTime;

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{DaemonLiveness, DaemonPresence, DaemonState, FloorLocalDeskIndex};
use pixtuoid_core::walkable::OccupancyOverlay;

use crate::layout::{Layout, Point, DESK_W};
use crate::pathfind::{find_path, snap_point_to_walkable};
use crate::pet::PetKind;

/// ms since the Unix epoch — mirrors `pixel_painter::epoch_ms`, kept local so the
/// sim side never imports the render module.
fn epoch_ms(now: SystemTime) -> u64 {
    crate::anim::elapsed_ms(now, SystemTime::UNIX_EPOCH)
}

/// Visit-spot anchors for the wandering creatures (pet + gateway mascot). Each
/// owns ONE furniture's offset so `pet_position` and `mascot_spots` can't drift
/// from each other. Pantry and the lounge couch share the SAME `+(4,6)` offset
/// (both are corner appliances the creature stands beside), so `corner_visit_spot`
/// serves both; the desk and the meeting sofa have their own offsets.
///
/// Only the FURNITURE-GEOMETRY offset is shared here — these are derived from the
/// `DESK_W`/`DESK_H` consts, NOT from any creature's sprite, and both creatures
/// then `walk_between` + `snap_point_to_walkable` to a "near this furniture"
/// target (no pixel-precise stand pose), so a sprite's footprint never enters
/// the math. The creature-specific spot SELECTION stays DELIBERATELY separate and
/// must NOT be folded in: `pet_position` gathers every spot + an `is_idle` bool +
/// the corridor, while `mascot_spots` gathers conditionally on `DaemonState`
/// (Busy → desks, Idle → social) with no corridor. They share where-beside-the-
/// furniture, not which-furniture-when — two different domains.
fn desk_visit_spot(desk: Point) -> Point {
    // Below the desk's own GROUND (walk-behind End: the blocked strip reaches
    // DESK_GROUND_H under the Point, deeper than the DESK_H slot) and on the
    // desk's centerline — the first row past the desk's OBSTACLE_PAD_PX
    // strip. Walkable in the packed grid almost everywhere (the old
    // x+DESK_W+1 corner spot sat inside the desk's OWN padded ground and
    // relied on snap for every desk); the one residue is a bottom-row desk
    // whose spot lands on a corridor appliance's ground — snap still covers
    // that sliver.
    Point {
        x: desk.x + DESK_W / 2,
        y: desk.y + crate::layout::DESK_GROUND_H + crate::layout::OBSTACLE_PAD_PX,
    }
}

/// Pantry / lounge-couch visit anchor (identical `+(4,6)` offset for both).
fn corner_visit_spot(p: Point) -> Point {
    Point {
        x: p.x + 4,
        y: p.y + 6,
    }
}

/// Meeting-sofa visit anchor.
fn sofa_visit_spot(sofa: Point) -> Point {
    Point {
        x: sofa.x + 4,
        y: sofa.y + 4,
    }
}

/// Pet roaming the whole office. Each 40s cycle picks a destination
/// from all available spots (desks, pantry, meeting sofas, lounge
/// couch, corridor), walks there from the previous spot, then sits or
/// sleeps until the next cycle.
pub(crate) fn pet_position(
    kind: PetKind,
    layout: &Layout,
    pack: &Pack,
    now: SystemTime,
    idle_desk_indices: &[FloorLocalDeskIndex],
    all_idle: bool,
    pet_seed: u64,
) -> Option<(Point, bool, &'static str, usize)> {
    pack.animation(kind.walk_anim())?;
    layout.corridor?;

    let elapsed_ms = epoch_ms(now);

    const CYCLE_MS: u64 = 40_000;
    let cycle_n = (elapsed_ms / CYCLE_MS).wrapping_add(pet_seed);
    let frac = (elapsed_ms % CYCLE_MS) as f32 / CYCLE_MS as f32;

    // Gather all interesting spots the cat can visit.
    let mut spots: Vec<(Point, bool)> = Vec::new();
    for (i, desk) in layout.home_desks.iter().enumerate() {
        spots.push((
            desk_visit_spot(*desk),
            idle_desk_indices.contains(&FloorLocalDeskIndex(i)),
        ));
    }
    if let Some(wp) = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, crate::layout::WaypointKind::Pantry))
    {
        spots.push((corner_visit_spot(wp.pos), false));
    }
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for sofa in trio.sofas {
            spots.push((sofa_visit_spot(sofa), false));
        }
    }
    if let Some(wp) = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, crate::layout::WaypointKind::Couch))
    {
        spots.push((corner_visit_spot(wp.pos), false));
    }
    if let Some(corridor) = layout.corridor {
        spots.push((
            Point {
                x: corridor.x + corridor.width / 2,
                y: corridor.y + corridor.height / 2,
            },
            false,
        ));
    }
    if spots.is_empty() {
        return None;
    }

    let pick = |n: u64| -> (Point, bool) {
        let h = n.wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize;
        spots[h % spots.len()]
    };
    let (dest, is_idle_spot) = pick(cycle_n);
    let (prev, _) = pick(cycle_n.wrapping_sub(1));

    // Pet walk cycle: a 2-frame toggle at this interval.
    const PET_ANIM_FRAME_MS: u64 = 220;
    let frame_idx = (elapsed_ms / PET_ANIM_FRAME_MS) as usize % 2;

    if frac < 0.35 {
        let t = (frac / 0.35).clamp(0.0, 1.0);
        // Facing follows the raw destination intent, independent of where the
        // snapped anchors land.
        let flip = dest.x < prev.x;
        // Same no-flash A*+snap+sample as the gateway mascot (shared helper).
        let pos = walk_between(layout, prev, dest, t);
        return Some((pos, flip, kind.walk_anim(), frame_idx));
    }

    // Rest phase: snap to a walkable cell so the sit/sleep pose isn't on
    // furniture. Same snapped anchor as the leg END ⇒ no pop at the boundary.
    let rest_pos = snap_point_to_walkable(&layout.walkable, dest).unwrap_or(dest);
    let anim = if all_idle || (kind.sleeps_near_idle() && is_idle_spot) {
        kind.sleep_anim()
    } else {
        kind.sit_anim()
    };
    Some((rest_pos, false, anim, 0))
}

/// Sample a polyline at arc-length fraction `t ∈ [0, 1]`, using octile segment
/// length so a diagonal leg doesn't move faster than a cardinal one. `t >= 1`
/// returns `fallback` (the caller's snapped goal) exactly — no float overshoot
/// onto a non-last cell. Precondition: `pts` non-empty (find_path guarantees it).
fn sample_polyline(pts: &[Point], t: f32, fallback: Point) -> Point {
    let Some(&last_pt) = pts.last() else {
        return fallback;
    };
    if pts.len() == 1 || t >= 1.0 {
        return last_pt;
    }
    let mut seg_lens: Vec<f32> = Vec::with_capacity(pts.len() - 1);
    let mut total = 0.0_f32;
    for w in pts.windows(2) {
        let dx = (w[1].x as i32 - w[0].x as i32).unsigned_abs() as f32;
        let dy = (w[1].y as i32 - w[0].y as i32).unsigned_abs() as f32;
        let len = dx.max(dy) + dx.min(dy) * (std::f32::consts::SQRT_2 - 1.0);
        seg_lens.push(len);
        total += len;
    }
    if total < 1e-3 {
        return last_pt;
    }
    let target = (t * total).min(total);
    let mut cumul = 0.0_f32;
    for (i, &slen) in seg_lens.iter().enumerate() {
        let is_last_seg = i == seg_lens.len() - 1;
        if cumul + slen >= target || is_last_seg {
            let local_t = if slen < 1e-3 {
                0.0
            } else {
                ((target - cumul) / slen).clamp(0.0, 1.0)
            };
            let a = pts[i];
            let b = pts[i + 1];
            return Point {
                x: (a.x as f32 + (b.x as f32 - a.x as f32) * local_t) as u16,
                y: (a.y as f32 + (b.y as f32 - a.y as f32) * local_t) as u16,
            };
        }
        cumul += slen;
    }
    last_pt
}

// ── Gateway lobster mascot ──────────────────────────────────────────────
// A presence-gated wandering creature (NOT an agent). Motion *encodes* the
// gateway state: it enters from the elevator on first sight, ambles + rests
// when Idle, shuttles toward the backend desks when Busy (the "routing" read),
// and walks back out to the elevator when the gateway goes Down. Stateless like
// the pet — position is a pure function of `now`, the presence timestamps, and a
// seed — so there is no per-frame state and the A*-on-static-mask legs never
// flash. The per-source sprite is resolved by `gateway_mascot_def`.

const MASCOT_ENTER_MS: u64 = 2200;
const MASCOT_LEAVE_MS: u64 = 2200;
const MASCOT_IDLE_CYCLE_MS: u64 = 9000;
const MASCOT_BUSY_CYCLE_MS: u64 = 4500;
// Degraded (#317) wanders SLOWER than idle — a sluggish, unwell drag.
const MASCOT_DEGRADED_CYCLE_MS: u64 = 14000;
const MASCOT_WALK_FRAC: f32 = 0.45;

/// Per-source gateway mascot facts: its sprite (walk, rest) + the hover-tooltip
/// display name. The ONE place a new gateway registers its creature — `None` for
/// non-gateway / un-mascotted sources (which gates the whole mascot in
/// `enqueue_gateway_mascot`), so a 2nd daemon adds exactly one arm here, not two
/// parallel `match source` tables kept in lockstep.
pub(crate) struct GatewayMascotDef {
    pub walk: &'static str,
    pub rest: &'static str,
    pub display_name: &'static str,
}

pub(crate) fn gateway_mascot_def(source: &str) -> Option<GatewayMascotDef> {
    match source {
        s if s == pixtuoid_core::source::openclaw::SOURCE_NAME => Some(GatewayMascotDef {
            walk: "lobster_walk",
            rest: "lobster_rest",
            display_name: "OpenClaw",
        }),
        _ => None,
    }
}

fn hash_pick(spots: &[Point], n: u64) -> Point {
    let h = n.wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize;
    spots[h % spots.len()]
}

/// A* on the STATIC mask with a throwaway EMPTY overlay (identical inputs every
/// frame of a leg ⇒ identical polyline ⇒ no flash), endpoints pre-snapped to
/// walkable floor, sampled at arc-length `t`. The no-flash walk discipline
/// shared by the pet and the gateway mascot.
fn walk_between(layout: &Layout, from: Point, to: Point, t: f32) -> Point {
    let src = snap_point_to_walkable(&layout.walkable, from).unwrap_or(from);
    let dst = snap_point_to_walkable(&layout.walkable, to).unwrap_or(to);
    let empty = OccupancyOverlay::new();
    if let Some(mut pts) = find_path(&layout.walkable, &empty, layout.corridor, from, to) {
        if let Some(first) = pts.first_mut() {
            *first = src;
        }
        if let Some(last) = pts.last_mut() {
            *last = dst;
        }
        sample_polyline(&pts, t, dst)
    } else {
        Point {
            x: (src.x as f32 + (dst.x as f32 - src.x as f32) * t) as u16,
            y: (src.y as f32 + (dst.y as f32 - src.y as f32) * t) as u16,
        }
    }
}

/// The walkable cell the mascot enters from / leaves to (the elevator
/// threshold), snapped to floor; falls back to the corridor centre.
fn mascot_elevator(layout: &Layout) -> Option<Point> {
    let raw = layout.door_threshold.or(layout.door).or_else(|| {
        layout.corridor.map(|c| Point {
            x: c.x + c.width / 2,
            y: c.y,
        })
    })?;
    snap_point_to_walkable(&layout.walkable, raw)
}

/// The wander "home" beat — the corridor centre, snapped. Also the leg-0 origin
/// so the enter hand-off is pop-free (enter ends here, wander cycle 0 starts here).
fn mascot_home(layout: &Layout) -> Option<Point> {
    let c = layout.corridor?;
    snap_point_to_walkable(
        &layout.walkable,
        Point {
            x: c.x + c.width / 2,
            y: c.y + c.height / 2,
        },
    )
}

/// Wander destinations, state-dependent. Idle roams the social spots (corridor,
/// pantry, sofas, couch); Busy shuttles to the backend desks (the coders it
/// routes to). Snapped lazily inside `walk_between`.
fn mascot_spots(layout: &Layout, state: DaemonState, home: Point) -> Vec<Point> {
    let mut spots = vec![home];
    if state == DaemonState::Busy {
        for desk in &layout.home_desks {
            spots.push(desk_visit_spot(*desk));
        }
    } else {
        if let Some(wp) = layout
            .waypoints
            .iter()
            .find(|w| matches!(w.kind, crate::layout::WaypointKind::Pantry))
        {
            spots.push(corner_visit_spot(wp.pos));
        }
        for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
            for sofa in trio.sofas {
                spots.push(sofa_visit_spot(sofa));
            }
        }
        if let Some(wp) = layout
            .waypoints
            .iter()
            .find(|w| matches!(w.kind, crate::layout::WaypointKind::Couch))
        {
            spots.push(corner_visit_spot(wp.pos));
        }
    }
    spots
}

/// Steady wander position at wander-clock `we_ms`. Returns `(pos, walking)`:
/// walking during the first `MASCOT_WALK_FRAC` of each cycle, resting after.
/// Cycle 0's origin is forced to `home` so it joins the enter walk pop-free.
fn mascot_wander(
    layout: &Layout,
    we_ms: u64,
    seed: u64,
    spots: &[Point],
    home: Point,
    cycle_ms: u64,
) -> (Point, bool) {
    if spots.is_empty() {
        return (home, false);
    }
    let cycle = we_ms / cycle_ms;
    let frac = (we_ms % cycle_ms) as f32 / cycle_ms as f32;
    let dest = hash_pick(spots, seed.wrapping_add(cycle).wrapping_add(1));
    let prev = if cycle == 0 {
        home
    } else {
        hash_pick(spots, seed.wrapping_add(cycle))
    };
    if frac < MASCOT_WALK_FRAC {
        let t = (frac / MASCOT_WALK_FRAC).clamp(0.0, 1.0);
        (walk_between(layout, prev, dest, t), true)
    } else {
        (
            snap_point_to_walkable(&layout.walkable, dest).unwrap_or(dest),
            false,
        )
    }
}

/// Resolve the mascot's frame this tick: `(pos, anim_name, frame_idx)`, or
/// `None` when it should not be drawn (gateway gone after the walk-out).
pub(crate) fn mascot_position(
    layout: &Layout,
    presence: &DaemonPresence,
    walk_anim: &'static str,
    rest_anim: &'static str,
    now: SystemTime,
    seed: u64,
) -> Option<(Point, &'static str, usize)> {
    let elevator = mascot_elevator(layout)?;
    let home = mascot_home(layout)?;
    // Mascot (lobster) walk cycle: a 2-frame toggle at this interval.
    const MASCOT_ANIM_FRAME_MS: u64 = 200;
    let frame = ((epoch_ms(now) / MASCOT_ANIM_FRAME_MS) % 2) as usize;

    if presence.liveness == DaemonLiveness::Down {
        // Walk-out: from where the lobster was at the instant of Down, to the elevator.
        let down_age = now.duration_since(presence.last_seen).ok()?.as_millis() as u64;
        if down_age >= MASCOT_LEAVE_MS {
            return None; // gone
        }
        // The walk-out `from` is reconstructed with the IDLE spot set even if the
        // gateway was Busy at the instant of death. This is deliberate, NOT a bug:
        // the mascot is STATELESS (position is a pure function of `now` + the
        // presence timestamps — no retained per-frame state, see the module note),
        // and `DaemonState` carries no prev-state, so on a `Down` presence Idle is
        // the ONLY reconstructable wander. A direct Busy→Down (gateway killed
        // mid-run) can therefore jump one frame before the 2.2s elevator leg
        // re-lerps it — an accepted cosmetic edge on a rare path, not worth
        // threading retained state through and breaking the stateless invariant.
        let spots = mascot_spots(layout, DaemonState::Idle, home);
        let down_we = presence
            .last_seen
            .duration_since(presence.entered_at)
            .ok()
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            .saturating_sub(MASCOT_ENTER_MS);
        let (from, _) = mascot_wander(layout, down_we, seed, &spots, home, MASCOT_IDLE_CYCLE_MS);
        let t = down_age as f32 / MASCOT_LEAVE_MS as f32;
        return Some((walk_between(layout, from, elevator, t), walk_anim, frame));
    }

    let age = now.duration_since(presence.entered_at).ok()?.as_millis() as u64;
    if age < MASCOT_ENTER_MS {
        // Walk-in from the elevator to the home beat.
        let t = age as f32 / MASCOT_ENTER_MS as f32;
        return Some((walk_between(layout, elevator, home, t), walk_anim, frame));
    }

    // Steady wander, styled by state.
    let cycle_ms = match presence.display_state() {
        DaemonState::Busy => MASCOT_BUSY_CYCLE_MS,
        DaemonState::Degraded => MASCOT_DEGRADED_CYCLE_MS,
        _ => MASCOT_IDLE_CYCLE_MS,
    };
    let spots = mascot_spots(layout, presence.display_state(), home);
    let (pos, walking) = mascot_wander(layout, age - MASCOT_ENTER_MS, seed, &spots, home, cycle_ms);
    if walking {
        Some((pos, walk_anim, frame))
    } else {
        Some((pos, rest_anim, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: u16, y: u16) -> Point {
        Point { x, y }
    }

    #[test]
    fn sample_polyline_empty_returns_fallback() {
        assert_eq!(sample_polyline(&[], 0.5, p(9, 9)), p(9, 9));
    }

    #[test]
    fn sample_polyline_single_point_returns_it() {
        assert_eq!(sample_polyline(&[p(3, 4)], 0.5, p(9, 9)), p(3, 4));
    }

    #[test]
    fn sample_polyline_t_at_or_past_one_returns_last() {
        let pts = [p(0, 0), p(10, 0)];
        assert_eq!(sample_polyline(&pts, 1.0, p(9, 9)), p(10, 0));
        assert_eq!(sample_polyline(&pts, 2.5, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_t_zero_returns_first() {
        assert_eq!(sample_polyline(&[p(0, 0), p(10, 0)], 0.0, p(9, 9)), p(0, 0));
    }

    #[test]
    fn sample_polyline_midpoint_on_straight_segment() {
        assert_eq!(sample_polyline(&[p(0, 0), p(10, 0)], 0.5, p(9, 9)), p(5, 0));
    }

    #[test]
    fn sample_polyline_arc_length_hits_corner_of_l() {
        // L: (0,0)->(10,0) len 10, ->(10,10) len 10; total 20. t=0.5 → arc 10 →
        // exactly the corner.
        let pts = [p(0, 0), p(10, 0), p(10, 10)];
        assert_eq!(sample_polyline(&pts, 0.5, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_octile_weights_diagonal() {
        // Cardinal leg len 10, diagonal leg octile len ≈14.14; total ≈24.14.
        // Sampling at arc-distance 10/total lands exactly on the corner — proves
        // the diagonal is weighted by octile length, not raw point count.
        let pts = [p(0, 0), p(10, 0), p(20, 10)];
        let total = 10.0 + 10.0 * std::f32::consts::SQRT_2;
        assert_eq!(sample_polyline(&pts, 10.0 / total, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_zero_length_leading_segment_no_div_by_zero() {
        // Duplicate first point (zero-length segment) must not panic.
        let pts = [p(5, 5), p(5, 5), p(15, 5)];
        assert_eq!(sample_polyline(&pts, 0.5, p(0, 0)), p(10, 5));
    }

    #[test]
    fn sample_polyline_target_on_zero_length_segment_uses_local_t_zero() {
        // The CHOSEN segment (not merely a leading one) has zero length: target=0
        // selects i=0 whose seg is the duplicate (0,0)->(0,0), slen<1e-3, so the
        // `local_t = 0.0` branch fires and returns the segment start.
        let pts = [p(0, 0), p(0, 0), p(10, 0)];
        assert_eq!(sample_polyline(&pts, 0.0, p(9, 9)), p(0, 0));
    }

    fn test_pack() -> Pack {
        crate::embedded_pack::test_default_pack()
    }

    #[test]
    fn pet_rest_picks_sleep_anim_when_all_idle() {
        // frac >= 0.35 (rest phase) AND all_idle => the sleep anim is selected
        // regardless of whether the rest spot is an idle desk.
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        let pack = test_pack();
        // elapsed % 40_000 == 20_000 → frac = 0.5 (rest phase).
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(20_000);
        let (_, _, anim, frame) =
            pet_position(PetKind::Cat, &layout, &pack, now, &[], true, 0).expect("a pet position");
        assert_eq!(anim, PetKind::Cat.sleep_anim(), "all_idle → sleep anim");
        assert_eq!(frame, 0, "rest pose uses frame 0");
    }

    #[test]
    fn pet_no_route_falls_back_to_straight_lerp() {
        // Build a Layout whose walkable mask is split into two disconnected
        // pockets by a solid vertical wall. With one spot in each pocket, the
        // pet's walk leg routes between them, find_path returns None, and the
        // straight-lerp fallback (the cited 297-300) is taken.
        use crate::layout::{Bounds, ReachSet};
        use pixtuoid_core::walkable::WalkableMask;
        let (w, h) = (200u16, 120u16);
        let mut mask = WalkableMask::new_open(w, h);
        // Solid wall band x∈[80,120) for the full height → left (x<80) and right
        // (x>=120) pockets are unreachable from each other on the coarse grid.
        mask.mark_blocked(80, 0, 40, h, 0);
        let reachable = ReachSet::from_mask(&mask, Point { x: 20, y: 20 });
        let mut layout = crate::layout::Layout::compute(w, h, Some(4)).expect("layout fits");
        // Override geometry: exactly two spots, one per pocket — the desk's
        // visit spot on the LEFT, the corridor centre on the RIGHT.
        layout.home_desks = vec![Point { x: 20, y: 30 }];
        layout.waypoints.clear();
        layout.meeting_rooms.clear();
        layout.corridor = Some(Bounds {
            x: 150,
            y: 40,
            width: 20,
            height: 20,
        });
        layout.walkable = mask;
        layout.reachable = reachable;
        let pack = test_pack();

        // The two spots pet_position gathers, in its order: the home desk
        // (left pocket) then the corridor centre (right pocket).
        let spots = [
            desk_visit_spot(Point { x: 20, y: 30 }),
            Point { x: 160, y: 50 },
        ];
        // Walk phase: elapsed 5s → frac 0.125 (<0.35); cycle_n == pet_seed
        // (elapsed/40000 == 0). Replicate pet_position's pick so we KNOW the leg
        // crosses the wall (prev ≠ dest), guaranteeing find_path → None — the
        // fallback branch is then the ONLY way a position is produced (a broken
        // fallback would panic here, not pass silently).
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(5_000);
        let seed = 0u64;
        let pick = |n: u64| spots[(n.wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize) % spots.len()];
        let dest = pick(seed);
        let prev = pick(seed.wrapping_sub(1));
        assert_ne!(prev, dest, "seed must make the leg cross the wall");

        // Precondition: the two snapped anchors are genuinely unroutable.
        let src_anchor = snap_point_to_walkable(&layout.walkable, prev).expect("prev snaps");
        let dst_anchor = snap_point_to_walkable(&layout.walkable, dest).expect("dest snaps");
        assert!(
            find_path(
                &layout.walkable,
                &OccupancyOverlay::new(),
                layout.corridor,
                prev,
                dest
            )
            .is_none(),
            "the two pockets must be disconnected so the straight-lerp fallback is the only path"
        );

        // The fallback is the EXACT straight lerp between the snapped anchors at
        // t = frac/0.35 — pin the math so a regression in 297-300 fails the test.
        let t = (0.125_f32 / 0.35).clamp(0.0, 1.0);
        let lerp = |a: u16, b: u16| (a as f32 + (b as f32 - a as f32) * t) as u16;
        let expected = Point {
            x: lerp(src_anchor.x, dst_anchor.x),
            y: lerp(src_anchor.y, dst_anchor.y),
        };

        let (pos, _, anim, _) =
            pet_position(PetKind::Cat, &layout, &pack, now, &[], false, seed).expect("walk pos");
        assert_eq!(anim, PetKind::Cat.walk_anim(), "walk phase");
        assert_eq!(
            pos, expected,
            "no-route leg must be the straight lerp between snapped anchors"
        );
    }

    #[test]
    fn gateway_mascot_def_maps_openclaw_and_rejects_others() {
        // The openclaw source resolves to its lobster sprite + tooltip name; every
        // other source name hits the `_ => None` arm (no mascot).
        let def = gateway_mascot_def(pixtuoid_core::source::openclaw::SOURCE_NAME)
            .expect("openclaw must have a mascot def");
        assert_eq!(def.walk, "lobster_walk");
        assert_eq!(def.rest, "lobster_rest");
        assert_eq!(def.display_name, "OpenClaw");
        assert!(
            gateway_mascot_def("codex").is_none(),
            "codex is not a gateway → no mascot"
        );
        assert!(
            gateway_mascot_def("some-other").is_none(),
            "unknown source → no mascot"
        );
    }

    #[test]
    fn mascot_elevator_falls_back_to_corridor_top_when_no_door() {
        // With BOTH door fields absent, mascot_elevator takes the corridor-top
        // centre fallback (430-434): (corridor.x + width/2, corridor.y), then snaps
        // to walkable. A normal layout always has a door_threshold, so this is the
        // only path that exercises the `or_else` branch.
        let mut layout = crate::layout::Layout::compute(160, 120, Some(4)).expect("layout fits");
        layout.door = None;
        layout.door_threshold = None;
        let corridor = layout.corridor.expect("compute gives a corridor");
        let raw = Point {
            x: corridor.x + corridor.width / 2,
            y: corridor.y,
        };
        let expected = snap_point_to_walkable(&layout.walkable, raw)
            .expect("corridor-top centre must snap to a walkable cell");
        assert_eq!(
            mascot_elevator(&layout),
            Some(expected),
            "no door → snapped corridor-top centre, not None and not a door cell"
        );
    }

    #[test]
    fn mascot_wander_empty_spots_returns_home_and_cycle0_starts_from_home() {
        // (a) The empty-spots guard (496) returns the home beat, not walking.
        // (b) Cycle 0 forces prev=home (502) so leg 0 joins the enter walk pop-free:
        //     the walking position equals walk_between(home → hash_pick(spots, seed+1)).
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        let home = mascot_home(&layout).expect("home beat");

        // (a) empty guard.
        assert_eq!(
            mascot_wander(&layout, 9_000, 7, &[], home, MASCOT_IDLE_CYCLE_MS),
            (home, false),
            "no spots → rest at home"
        );

        // (b) cycle 0 origin == home. Pick a we_ms inside the walking fraction of
        // cycle 0 (frac < MASCOT_WALK_FRAC) so the walk_between is exercised.
        let spots = mascot_spots(&layout, DaemonState::Idle, home);
        assert!(
            spots.len() >= 2,
            "idle spots must include home + social spots"
        );
        let cycle_ms = MASCOT_IDLE_CYCLE_MS;
        let we_ms = (cycle_ms as f32 * 0.2) as u64; // frac 0.2 < 0.45 → walking
        let seed = 3u64;
        let frac = (we_ms % cycle_ms) as f32 / cycle_ms as f32;
        let t = (frac / MASCOT_WALK_FRAC).clamp(0.0, 1.0);
        // cycle == 0 → dest = hash_pick(spots, seed+0+1); prev forced to home.
        let dest = hash_pick(&spots, seed.wrapping_add(0).wrapping_add(1));
        let expected = walk_between(&layout, home, dest, t);
        let (pos, walking) = mascot_wander(&layout, we_ms, seed, &spots, home, cycle_ms);
        assert!(walking, "frac < walk_frac → walking");
        assert_eq!(
            pos, expected,
            "cycle 0 leg must originate from home, not from a hash-picked prev spot"
        );
    }

    fn idle_presence(now: SystemTime, age_ms: u64) -> DaemonPresence {
        DaemonPresence {
            // Up with an empty run set ⇒ Idle (the derived projection).
            liveness: DaemonLiveness::UP,
            active_sessions: 0,
            last_seen: now,
            entered_at: now - std::time::Duration::from_millis(age_ms),
            in_flight_run_keys: std::collections::HashSet::new(),
            current_pid: Some(1),
        }
    }

    #[test]
    fn mascot_position_walks_in_from_elevator_during_enter_window() {
        // age < MASCOT_ENTER_MS → the walk-in arm (559-563) lerps elevator→home at
        // t = age/2200. age=0 lands exactly at the elevator; age≈half lands midway
        // (distinct from both endpoints).
        let layout = crate::layout::Layout::compute(160, 120, Some(4)).expect("layout fits");
        let elevator = mascot_elevator(&layout).expect("elevator");
        let home = mascot_home(&layout).expect("home");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(20_000);
        let seed = 0u64;

        // age = 0 → at the elevator, walk anim.
        let p0 = idle_presence(now, 0);
        let (pos0, anim0, _) =
            mascot_position(&layout, &p0, "lobster_walk", "lobster_rest", now, seed)
                .expect("walk-in position");
        assert_eq!(anim0, "lobster_walk", "enter window → walk anim");
        assert_eq!(
            pos0,
            walk_between(&layout, elevator, home, 0.0),
            "age 0 → exactly at the elevator end"
        );

        // age = 1100 (half the 2200 window) → midway along elevator→home.
        let age = 1_100u64;
        let p_mid = idle_presence(now, age);
        let (pos_mid, anim_mid, _) =
            mascot_position(&layout, &p_mid, "lobster_walk", "lobster_rest", now, seed)
                .expect("walk-in mid position");
        assert_eq!(anim_mid, "lobster_walk");
        let t = age as f32 / MASCOT_ENTER_MS as f32;
        assert_eq!(
            pos_mid,
            walk_between(&layout, elevator, home, t),
            "mid enter → the elevator→home interpolation"
        );
        // Sanity: midway is genuinely off both endpoints (so the lerp is live, not a
        // degenerate where elevator==home).
        assert_ne!(
            elevator, home,
            "elevator and home must differ for a real walk-in"
        );
    }

    #[test]
    fn mascot_position_degraded_uses_slower_wander_cycle() {
        // The Degraded arm (569) selects MASCOT_DEGRADED_CYCLE_MS (14000), slower
        // than Idle's 9000. Pick a `now` where the two cycles land the mascot in
        // DIFFERENT wander phases so the rendered anim/pos differs. A mutant mapping
        // Degraded → 9000 would make the two results identical.
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        // Fixed entry anchor; we vary `now` so `age = now - entered_at` actually
        // grows (an entered_at pinned at `now - k` would make age constant).
        let entered_at = SystemTime::UNIX_EPOCH;
        let seed = 0u64;

        // Both presences identical except degraded-ness (Idle vs Degraded — the
        // only two this test exercises); both well past the enter window. Empty
        // run set, so `degraded: false` ⇒ Idle and `true` ⇒ Degraded.
        let mk = |degraded: bool, now: SystemTime| DaemonPresence {
            liveness: DaemonLiveness::Up { degraded },
            active_sessions: 0,
            last_seen: now,
            entered_at,
            in_flight_run_keys: std::collections::HashSet::new(),
            current_pid: Some(1),
        };

        // Search for an `age` (we_ms = age - ENTER) where Idle's 9000-cycle and
        // Degraded's 14000-cycle frac fall in DIFFERENT bands (one walking, one
        // resting) → the two anims must differ.
        let mut found = None;
        for age in (MASCOT_ENTER_MS..(MASCOT_ENTER_MS + 14_000)).step_by(100) {
            let we = age - MASCOT_ENTER_MS;
            let frac_idle = (we % MASCOT_IDLE_CYCLE_MS) as f32 / MASCOT_IDLE_CYCLE_MS as f32;
            let frac_deg = (we % MASCOT_DEGRADED_CYCLE_MS) as f32 / MASCOT_DEGRADED_CYCLE_MS as f32;
            let idle_walking = frac_idle < MASCOT_WALK_FRAC;
            let deg_walking = frac_deg < MASCOT_WALK_FRAC;
            if idle_walking != deg_walking {
                found = Some(entered_at + std::time::Duration::from_millis(age));
                break;
            }
        }
        let now = found.expect("a tick where idle vs degraded phases diverge must exist");

        let idle = mk(false, now);
        let degraded = mk(true, now);
        let (_, idle_anim, _) =
            mascot_position(&layout, &idle, "lobster_walk", "lobster_rest", now, seed)
                .expect("idle pos");
        let (_, deg_anim, _) = mascot_position(
            &layout,
            &degraded,
            "lobster_walk",
            "lobster_rest",
            now,
            seed,
        )
        .expect("degraded pos");
        assert_ne!(
            idle_anim, deg_anim,
            "degraded's slower cycle must put the mascot in a different phase than idle at this tick"
        );
    }
}
