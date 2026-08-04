//! Ambient wandering creatures — the office pet and the OpenClaw gateway mascot —
//! and WHERE they roam each frame. This is sim/behaviour: `pixel_painter` consumes
//! the positions produced here and paints them (the "scene decides, painter draws"
//! contract). Pet and mascot share ONE roaming rule: draw a destination from the
//! whole walkable floor (`walkable_target`), walk there with the no-flash
//! `walk_between`, rest. Daemon state reads from the CADENCE (`MASCOT_*_CYCLE_MS`)
//! and the sprite tint, never from the destination.

use std::time::SystemTime;

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{DaemonLiveness, DaemonPresence, DaemonState, FloorLocalDeskIndex};
use pixtuoid_core::walkable::OccupancyOverlay;

use crate::layout::{Layout, Point};
use crate::pathfind::{find_path, snap_point_to_walkable};
use crate::pet::PetKind;

/// ms since the Unix epoch — kept local so the sim side never imports the renderer.
fn epoch_ms(now: SystemTime) -> u64 {
    crate::anim::elapsed_ms(now, SystemTime::UNIX_EPOCH)
}

/// How close a resting spot must be to an idle agent's desk to count as "napping
/// beside them" — sized to the desk's footprint plus a creature's width, so it
/// reads as sharing that workstation rather than merely being on the same floor.
const NAP_NEAR_DESK_PX: i32 = 16;

/// How many draws `walkable_target` tries before falling back to a snap. Walkable
/// floor is roughly half the buffer, so P(all miss) is ~2^-8 per call.
const TARGET_TRIES: u32 = 8;

/// A destination drawn from the WHOLE walkable floor, deterministic per
/// `(seed, n)` — the ONE destination rule both roamers and every daemon state use.
/// A curated spot list is not an option: it is small enough that N creatures share
/// destinations by pigeonhole, and no per-creature offset scheme recovers from that.
///
/// REJECTION sampling, not enumeration: collecting every walkable cell would cost
/// O(w*h) per creature per frame in the render loop. `snap_point_to_walkable` is
/// the exact backstop if every draw lands on furniture.
fn walkable_target(layout: &Layout, seed: u64, n: u64) -> Point {
    let (w, h) = (layout.walkable.width(), layout.walkable.height());
    if w == 0 || h == 0 {
        return Point { x: 0, y: 0 };
    }
    let mut z = seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut last = Point { x: 0, y: 0 };
    for _ in 0..TARGET_TRIES {
        z = pixtuoid_core::id::splitmix64(z);
        // Independent halves of one hash: the high word picks x, the low word y, so
        // a draw is not diagonal-biased.
        last = Point {
            x: ((z >> 32) % u64::from(w)) as u16,
            y: (z % u64::from(h)) as u16,
        };
        if layout.walkable.is_walkable(last.x, last.y) {
            return last;
        }
    }
    snap_point_to_walkable(&layout.walkable, last).unwrap_or(last)
}

/// A centre-anchored sprite's `(w, h)`, or `(0, 0)` when the pack lacks the
/// animation (no extent ⇒ [`clamp_sprite_inside`] leaves the point alone).
fn sprite_extent(pack: &Pack, anim: &str) -> (u16, u16) {
    pack.animation(anim)
        .and_then(|a| a.frames.first())
        .map_or((0, 0), |f| (f.width(), f.height()))
}

/// Keep a centre-anchored creature sprite fully inside the buffer.
///
/// Destinations come from the walkable mask's FULL width ([`walkable_target`]),
/// but `blit_centered` parks a frame at `pos − size/2` and CLIPS at the buffer
/// edge. Clamping the RESOLVED point rather than insetting the draw keeps the
/// wander distribution exactly as it was. A buffer narrower than the sprite has
/// no in-bounds centre at all, so that axis is left untouched.
fn clamp_sprite_inside(p: Point, (fw, fh): (u16, u16), layout: &Layout) -> Point {
    let axis = |v: u16, span: u16, f: u16| {
        let (lo, hi) = (f / 2, span.saturating_sub(f - f / 2));
        if hi < lo {
            v
        } else {
            v.clamp(lo, hi)
        }
    };
    Point {
        x: axis(p.x, layout.walkable.width(), fw),
        y: axis(p.y, layout.walkable.height(), fh),
    }
}

/// Pet roaming the whole office: each 40s cycle picks a destination, walks there
/// from the previous one, then sits or sleeps until the next cycle.
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

    let dest = walkable_target(layout, pet_seed, cycle_n);
    let prev = walkable_target(layout, pet_seed, cycle_n.wrapping_sub(1));
    let is_idle_spot = idle_desk_indices.iter().any(|i| {
        layout.home_desks.get(i.0).is_some_and(|d| {
            (i32::from(dest.x) - i32::from(d.x)).abs() <= NAP_NEAR_DESK_PX
                && (i32::from(dest.y) - i32::from(d.y)).abs() <= NAP_NEAR_DESK_PX
        })
    });

    const PET_ANIM_FRAME_MS: u64 = 220;
    let frame_idx = (elapsed_ms / PET_ANIM_FRAME_MS) as usize % 2;

    if frac < 0.35 {
        let t = (frac / 0.35).clamp(0.0, 1.0);
        // Facing follows the raw destination intent, not where the snapped anchors land.
        let flip = dest.x < prev.x;
        let pos = walk_between(layout, prev, dest, t);
        let anim = kind.walk_anim();
        return Some((
            clamp_sprite_inside(pos, sprite_extent(pack, anim), layout),
            flip,
            anim,
            frame_idx,
        ));
    }

    // Snap so the sit/sleep pose isn't on furniture — the same anchor as the leg
    // END, so there is no pop at the boundary.
    let rest_pos = snap_point_to_walkable(&layout.walkable, dest).unwrap_or(dest);
    let anim = if all_idle || (kind.sleeps_near_idle() && is_idle_spot) {
        kind.sleep_anim()
    } else {
        kind.sit_anim()
    };
    Some((
        clamp_sprite_inside(rest_pos, sprite_extent(pack, anim), layout),
        false,
        anim,
        0,
    ))
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

// The gateway mascot's motion *encodes* the gateway state: it enters from the
// elevator on first sight, ambles when Idle, shuttles when Busy, walks back out
// when Down. Stateless like the pet — position is a pure function of `now`, the
// presence timestamps and a seed — so the A*-on-static-mask legs never flash.

const MASCOT_ENTER_MS: u64 = 2200;
const MASCOT_LEAVE_MS: u64 = 2200;
const MASCOT_IDLE_CYCLE_MS: u64 = 9000;
const MASCOT_BUSY_CYCLE_MS: u64 = 4500;
// Degraded wanders SLOWER than idle — a sluggish, unwell drag.
const MASCOT_DEGRADED_CYCLE_MS: u64 = 14000;
const MASCOT_WALK_FRAC: f32 = 0.45;

/// Per-source gateway mascot facts: its sprite (walk, rest) + the hover-tooltip
/// display name. The ONE place a new gateway registers its creature — `None` for
/// non-gateway sources gates the whole mascot in `enqueue_gateway_mascots`.
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

/// A* on the STATIC mask with a throwaway EMPTY overlay (identical inputs every
/// frame of a leg ⇒ identical polyline ⇒ no flash), endpoints pre-snapped to
/// walkable floor, sampled at arc-length `t`.
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

/// The wander seed for ONE daemon instance — folds the source AND the instance id
/// (OpenClaw's resolved gateway port), so N gateways of one source take N different
/// paths and a gateway restarting on its own port keeps its path.
pub(crate) fn mascot_seed(source: &str, instance: &pixtuoid_core::state::DaemonInstanceId) -> u64 {
    source
        .bytes()
        .chain(std::iter::once(b'@'))
        .chain(instance.as_str().bytes())
        .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64))
}

/// How long one mascot may be held at the elevator before its walk-in starts.
/// Gateways that first-sight in the SAME beat would otherwise lerp the identical
/// `elevator → home` line for the whole [`MASCOT_ENTER_MS`] and render as ONE
/// lobster — the seed reaches only the steady wander.
const MASCOT_ENTER_STAGGER_MS: u64 = 900;

/// The seeded walk-in delay for one mascot — its slice of
/// [`MASCOT_ENTER_STAGGER_MS`]. Position stays a pure function of `now` + the
/// presence timestamps + this seed (a mascot's motion never depends on which
/// SIBLINGS exist): the leg is untouched, it just starts later, so the pop-free
/// join to wander cycle 0 still holds.
///
/// The delay comes off an AVALANCHED hash, not `seed % STAGGER` directly: the
/// realistic multi-gateway deployment is CONSECUTIVE ports, whose folded seeds
/// differ by 1, so a raw modulo reads only the low bits and hands adjacent
/// gateways delays a millisecond apart — no stagger at all.
fn mascot_enter_delay(seed: u64) -> u64 {
    pixtuoid_core::id::splitmix64(seed) % MASCOT_ENTER_STAGGER_MS
}

/// Steady wander position at wander-clock `we_ms`. Returns `(pos, walking)`:
/// walking during the first `MASCOT_WALK_FRAC` of each cycle, resting after.
/// Cycle 0's origin is forced to `home` so it joins the enter walk pop-free.
fn mascot_wander(
    layout: &Layout,
    we_ms: u64,
    seed: u64,
    home: Point,
    cycle_ms: u64,
) -> (Point, bool) {
    let cycle = we_ms / cycle_ms;
    let frac = (we_ms % cycle_ms) as f32 / cycle_ms as f32;
    let dest = walkable_target(layout, seed, cycle.wrapping_add(1));
    let prev = if cycle == 0 {
        home
    } else {
        walkable_target(layout, seed, cycle)
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
    pack: &Pack,
    presence: &DaemonPresence,
    walk_anim: &'static str,
    rest_anim: &'static str,
    now: SystemTime,
    seed: u64,
) -> Option<(Point, &'static str, usize)> {
    let elevator = mascot_elevator(layout)?;
    let home = mascot_home(layout)?;
    let anchor = |pos: Point, anim: &'static str, frame_idx: usize| {
        (
            clamp_sprite_inside(pos, sprite_extent(pack, anim), layout),
            anim,
            frame_idx,
        )
    };
    const MASCOT_ANIM_FRAME_MS: u64 = 200;
    let frame = ((epoch_ms(now) / MASCOT_ANIM_FRAME_MS) % 2) as usize;
    // Every clock below is measured from the END of this instance's stagger, so the
    // walk-out's reconstructed origin stays on the same wander phase as the walk-in.
    let enter_delay = mascot_enter_delay(seed);

    if presence.liveness == DaemonLiveness::Down {
        // Walk-out: from where the lobster was at the instant of Down, to the elevator.
        let down_age = now.duration_since(presence.last_seen).ok()?.as_millis() as u64;
        if down_age >= MASCOT_LEAVE_MS {
            return None;
        }
        // Reconstructed at the IDLE CADENCE even if the gateway was Busy at the
        // instant of death: the mascot is STATELESS and `DaemonState` carries no
        // prev-state, so Idle is the only reconstructable clock. Every state draws
        // from the same whole-floor rule, so only the CYCLE LENGTH differs.
        let down_we = presence
            .last_seen
            .duration_since(presence.entered_at)
            .ok()
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            .saturating_sub(MASCOT_ENTER_MS + enter_delay);
        let (from, _) = mascot_wander(layout, down_we, seed, home, MASCOT_IDLE_CYCLE_MS);
        let t = down_age as f32 / MASCOT_LEAVE_MS as f32;
        return Some(anchor(
            walk_between(layout, from, elevator, t),
            walk_anim,
            frame,
        ));
    }

    let age = now.duration_since(presence.entered_at).ok()?.as_millis() as u64;
    if age < enter_delay {
        // A creature that has not walked in is not in the room: NOT DRAWN during its
        // own stagger, rather than held visibly at the elevator on top of its peers.
        return None;
    }
    let entered = age - enter_delay;
    if entered < MASCOT_ENTER_MS {
        let t = entered as f32 / MASCOT_ENTER_MS as f32;
        return Some(anchor(
            walk_between(layout, elevator, home, t),
            walk_anim,
            frame,
        ));
    }

    let cycle_ms = match presence.display_state() {
        DaemonState::Busy => MASCOT_BUSY_CYCLE_MS,
        DaemonState::Degraded => MASCOT_DEGRADED_CYCLE_MS,
        _ => MASCOT_IDLE_CYCLE_MS,
    };
    let (pos, walking) = mascot_wander(layout, entered - MASCOT_ENTER_MS, seed, home, cycle_ms);
    if walking {
        Some(anchor(pos, walk_anim, frame))
    } else {
        Some(anchor(pos, rest_anim, 0))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_registered_daemon_source_has_a_mascot_def() {
        // `gateway_mascot_def` is the ONE per-source daemon table with neither a
        // compile error nor a lockstep test behind it: a new daemon row would decode,
        // key, sweep and roll up correctly and render NO mascot at all.
        use pixtuoid_core::source::registry::REGISTRY;
        for d in REGISTRY.iter().filter(|d| d.is_daemon()) {
            assert!(
                super::gateway_mascot_def(d.name).is_some(),
                "daemon source {:?} has no GatewayMascotDef — it would render no mascot",
                d.name
            );
        }
    }

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
        let pts = [p(0, 0), p(10, 0), p(10, 10)];
        assert_eq!(sample_polyline(&pts, 0.5, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_octile_weights_diagonal() {
        let pts = [p(0, 0), p(10, 0), p(20, 10)];
        let total = 10.0 + 10.0 * std::f32::consts::SQRT_2;
        assert_eq!(sample_polyline(&pts, 10.0 / total, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_zero_length_leading_segment_no_div_by_zero() {
        let pts = [p(5, 5), p(5, 5), p(15, 5)];
        assert_eq!(sample_polyline(&pts, 0.5, p(0, 0)), p(10, 5));
    }

    #[test]
    fn sample_polyline_target_on_zero_length_segment_uses_local_t_zero() {
        // The CHOSEN segment, not merely a leading one, has zero length.
        let pts = [p(0, 0), p(0, 0), p(10, 0)];
        assert_eq!(sample_polyline(&pts, 0.0, p(9, 9)), p(0, 0));
    }

    fn test_pack() -> Pack {
        crate::embedded_pack::test_default_pack()
    }

    #[test]
    fn pet_rest_picks_sleep_anim_when_all_idle() {
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
        use crate::layout::{Bounds, ReachSet};
        use pixtuoid_core::walkable::WalkableMask;
        let (w, h) = (200u16, 120u16);
        let mut mask = WalkableMask::new_open(w, h);
        // Solid wall band x∈[80,120) for the full height → the left (x<80) and right
        // (x>=120) pockets are unreachable from each other on the coarse grid.
        mask.mark_blocked(80, 0, 40, h, 0);
        let reachable = ReachSet::from_mask(&mask, Point { x: 20, y: 20 });
        let mut layout = crate::layout::Layout::compute(w, h, Some(4)).expect("layout fits");
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

        // Walk phase: elapsed 5s → frac 0.125 (< 0.35), and cycle_n == pet_seed, so
        // the seed IS the pick index the production code uses. A cross-wall leg can't
        // be staged by construction — SEARCH for a seed whose two draws land in
        // opposite pockets, so find_path → None is guaranteed.
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(5_000);
        let (seed, prev, dest) = (0u64..4_000)
            .find_map(|sd| {
                let dest = walkable_target(&layout, sd, sd);
                let prev = walkable_target(&layout, sd, sd.wrapping_sub(1));
                ((dest.x < 80) != (prev.x < 80)).then_some((sd, prev, dest))
            })
            .expect("some seed must straddle the wall");

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
        // A normal layout always has a door_threshold, so clearing BOTH door fields
        // is the only way to reach the `or_else` branch.
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
    fn mascot_wander_cycle0_starts_from_home() {
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        let home = mascot_home(&layout).expect("home beat");
        let cycle_ms = MASCOT_IDLE_CYCLE_MS;
        let we_ms = (cycle_ms as f32 * 0.2) as u64; // frac 0.2 < 0.45 → walking
        let seed = 3u64;
        let frac = (we_ms % cycle_ms) as f32 / cycle_ms as f32;
        let t = (frac / MASCOT_WALK_FRAC).clamp(0.0, 1.0);
        // Derived through the impl's own picker, so the assertion is about the ORIGIN
        // rather than a second copy of the destination math.
        let dest = walkable_target(&layout, seed, 1);
        let expected = walk_between(&layout, home, dest, t);
        let (pos, walking) = mascot_wander(&layout, we_ms, seed, home, cycle_ms);
        assert!(walking, "frac < walk_frac → walking");
        assert_eq!(
            pos, expected,
            "cycle 0 leg must originate from home, not from a picked prev cell"
        );
    }

    // Unlike desks/waypoints (placed by the layout with margins), nothing insets a
    // creature by its own sprite extent — so the assertion is on the sprite RECT.
    #[test]
    fn a_wandering_mascot_sprite_never_spills_past_the_buffer_edge() {
        let (w, h) = (192u16, 160u16);
        let layout = crate::layout::Layout::compute(w, h, Some(12)).expect("layout fits");
        let pack = test_pack();
        let src = pixtuoid_core::source::openclaw::SOURCE_NAME;
        let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for port in ["18901", "18902", "18903", "18904", "100", "805"] {
            let inst = pixtuoid_core::state::DaemonInstanceId::new(port).expect("non-empty");
            let seed = mascot_seed(src, &inst);
            // Well past the walk-in so every sample is a steady wander beat.
            for step_ms in (0..MASCOT_IDLE_CYCLE_MS * 4).step_by(120) {
                let now = t0 + std::time::Duration::from_millis(MASCOT_ENTER_MS * 2 + step_ms);
                let presence = idle_presence(now, MASCOT_ENTER_MS * 2 + step_ms);
                let Some((pos, anim, _)) = mascot_position(
                    &layout,
                    &pack,
                    &presence,
                    "lobster_walk",
                    "lobster_rest",
                    now,
                    seed,
                ) else {
                    continue;
                };
                let frame = pack
                    .animation(anim)
                    .and_then(|a| a.frames.first())
                    .expect("the bundled pack has both lobster anims");
                let (fw, fh) = (frame.width(), frame.height());
                let (x0, y0) = (pos.x.saturating_sub(fw / 2), pos.y.saturating_sub(fh / 2));
                assert!(
                    x0 + fw <= w && y0 + fh <= h,
                    "port {port} at +{step_ms}ms: {fw}x{fh} sprite at {pos:?} spills the \
                     {w}x{h} buffer (rect {x0},{y0}..{},{})",
                    x0 + fw,
                    y0 + fh
                );
            }
        }
    }

    fn idle_presence(now: SystemTime, age_ms: u64) -> DaemonPresence {
        DaemonPresence {
            // Up with an empty run set ⇒ Idle (the derived projection).
            liveness: DaemonLiveness::UP,
            active_sessions: 0,
            last_seen: now,
            entered_at: now - std::time::Duration::from_millis(age_ms),
            in_flight_runs: Default::default(),
            current_pid: Some(1),
        }
    }

    #[test]
    fn consecutive_gateway_ports_get_spread_walk_in_delays() {
        // CONSECUTIVE ports are the realistic deployment and their folded seeds
        // differ by 1 — the case a raw `seed % STAGGER` gets wrong. Pinned on the
        // REAL seeds, so the fixture can't drift off the production key.
        let src = pixtuoid_core::source::openclaw::SOURCE_NAME;
        let delays: Vec<u64> = ["18901", "18902", "18903", "18904", "18905", "18906"]
            .iter()
            .map(|p| {
                let inst = pixtuoid_core::state::DaemonInstanceId::new(*p).expect("non-empty");
                mascot_enter_delay(mascot_seed(src, &inst))
            })
            .collect();
        let spread = delays.iter().max().unwrap() - delays.iter().min().unwrap();
        assert!(
            spread > MASCOT_ENTER_STAGGER_MS / 3,
            "adjacent ports must spread across the stagger window, got {delays:?}"
        );
        let distinct: std::collections::BTreeSet<_> = delays.iter().collect();
        assert_eq!(
            distinct.len(),
            delays.len(),
            "no two adjacent ports may share a walk-in slice: {delays:?}"
        );
        let seeds: std::collections::BTreeSet<u64> = ["18901", "18902", "18903", "18904"]
            .iter()
            .map(|p| {
                mascot_seed(
                    src,
                    &pixtuoid_core::state::DaemonInstanceId::new(*p).expect("non-empty"),
                )
            })
            .collect();
        assert_eq!(seeds.len(), 4, "each instance must seed differently");
    }

    /// The bound asserts the PROPERTY, not a percentage: it sits well above the rate
    /// the whole-floor rule produces and well below what a small curated destination
    /// set produced, so it catches a return to a curated list without becoming a
    /// golden on the hash arithmetic. Crowding is measured as BOX overlap, the
    /// pessimistic metric — a 14x12 box intersection is not a visual merge, and no
    /// destination rule drives it to zero because routed legs share the aisles.
    #[test]
    fn four_gateways_rarely_crowd_now_that_the_whole_floor_is_in_play() {
        use pixtuoid_core::state::DaemonInstanceId;
        const SPRITE_W: i32 = 14;
        const SPRITE_H: i32 = 12;
        const FRAMES: u64 = 1080;
        const CROWDED_MAX_PCT: u64 = 60;

        // `max_desks: None` fills the buffer as every real painter does; a capped test
        // layout understates the office and flatters the measurement.
        let layout = crate::layout::Layout::compute(140, 120, None).expect("layout fits");
        let entered = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let seeds: Vec<u64> = (0..4u32)
            .map(|i| {
                let id = DaemonInstanceId::new((18901 + i).to_string()).expect("non-empty");
                mascot_seed("openclaw", &id)
            })
            .collect();

        let mut crowded = 0u64;
        let mut drawn_frames = 0u64;
        for f in 0..FRAMES {
            let now = entered + std::time::Duration::from_millis(f * 1000 / 12);
            let p = idle_presence(now, f * 1000 / 12);
            let pts: Vec<Point> = seeds
                .iter()
                .filter_map(|&sd| {
                    mascot_position(
                        &layout,
                        &test_pack(),
                        &p,
                        "lobster_walk",
                        "lobster_rest",
                        now,
                        sd,
                    )
                    .map(|(pos, _, _)| pos)
                })
                .collect();
            if pts.len() < 2 {
                continue;
            }
            drawn_frames += 1;
            if (0..pts.len()).any(|i| {
                ((i + 1)..pts.len()).any(|j| {
                    (i32::from(pts[i].x) - i32::from(pts[j].x)).abs() < SPRITE_W
                        && (i32::from(pts[i].y) - i32::from(pts[j].y)).abs() < SPRITE_H
                })
            }) {
                crowded += 1;
            }
        }
        assert!(
            drawn_frames > FRAMES / 2,
            "the sample must actually draw them"
        );
        let pct = 100 * crowded / drawn_frames;
        assert!(
            pct <= CROWDED_MAX_PCT,
            "four gateways crowded in {pct}% of {drawn_frames} frames (bound {CROWDED_MAX_PCT}%) \
             — a small destination set is back"
        );
    }

    #[test]
    fn two_instances_entering_together_are_never_superimposed_on_the_way_in() {
        // Two gateways sharing an `entered_at` is the common case: pixtuoid starting
        // while both are already up.
        let layout = crate::layout::Layout::compute(160, 120, Some(4)).expect("layout fits");
        let entered = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(20_000);
        let (a, b) = (0u64, 450u64);
        assert_ne!(
            mascot_enter_delay(a),
            mascot_enter_delay(b),
            "the fixture must exercise two DIFFERENT stagger slices"
        );

        let pos_at = |seed: u64, age_ms: u64| {
            let now = entered + std::time::Duration::from_millis(age_ms);
            let p = idle_presence(now, age_ms);
            mascot_position(
                &layout,
                &test_pack(),
                &p,
                "lobster_walk",
                "lobster_rest",
                now,
                seed,
            )
            .expect("inside the enter window")
            .0
        };
        // The window where the claim holds: from when the LATER instance leaves the
        // door to before the EARLIER one joins its wander. Outside it, crossing at
        // the shared `home` beat is ordinary traffic, not the collapse.
        let (da, db) = (mascot_enter_delay(a), mascot_enter_delay(b));
        let (lo, hi) = (da.max(db) + 1, da.min(db) + MASCOT_ENTER_MS);
        assert!(
            hi > lo + 1_000,
            "the fixture must leave a wide shared walk-in window, got {lo}..{hi}"
        );
        for age in (lo..hi).step_by(50) {
            assert_ne!(
                pos_at(a, age),
                pos_at(b, age),
                "two instances must never occupy one cell mid-walk-in (age {age}ms)"
            );
        }

        let drawn = |seed: u64, age_ms: u64| {
            let now = entered + std::time::Duration::from_millis(age_ms);
            let p = idle_presence(now, age_ms);
            mascot_position(
                &layout,
                &test_pack(),
                &p,
                "lobster_walk",
                "lobster_rest",
                now,
                seed,
            )
            .is_some()
        };
        let (first, second) = (da.min(db), da.max(db));
        for age in 0..first {
            assert!(
                !drawn(a, age) && !drawn(b, age),
                "before either stagger elapses, neither instance is in the room (age {age}ms)"
            );
        }
        for age in (first + 1)..second {
            assert!(
                drawn(a, age) != drawn(b, age),
                "between the two arrivals exactly one is drawn (age {age}ms)"
            );
        }
    }

    #[test]
    fn mascot_position_walks_in_from_elevator_during_enter_window() {
        let layout = crate::layout::Layout::compute(160, 120, Some(4)).expect("layout fits");
        let elevator = mascot_elevator(&layout).expect("elevator");
        let home = mascot_home(&layout).expect("home");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(20_000);
        let seed = 0u64;

        let p0 = idle_presence(now, 0);
        let (pos0, anim0, _) = mascot_position(
            &layout,
            &test_pack(),
            &p0,
            "lobster_walk",
            "lobster_rest",
            now,
            seed,
        )
        .expect("walk-in position");
        assert_eq!(anim0, "lobster_walk", "enter window → walk anim");
        assert_eq!(
            pos0,
            walk_between(&layout, elevator, home, 0.0),
            "age 0 → exactly at the elevator"
        );

        let age = 1_100u64;
        let p_mid = idle_presence(now, age);
        let (pos_mid, anim_mid, _) = mascot_position(
            &layout,
            &test_pack(),
            &p_mid,
            "lobster_walk",
            "lobster_rest",
            now,
            seed,
        )
        .expect("walk-in mid position");
        assert_eq!(anim_mid, "lobster_walk");
        let t = age as f32 / MASCOT_ENTER_MS as f32;
        assert_eq!(
            pos_mid,
            walk_between(&layout, elevator, home, t),
            "mid enter → the elevator→home interpolation"
        );
        assert_ne!(
            elevator, home,
            "the elevator and home must differ for a real walk-in"
        );
    }

    #[test]
    fn mascot_position_degraded_uses_slower_wander_cycle() {
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        // Fixed entry anchor; we vary `now` so `age = now - entered_at` actually
        // grows (an entered_at pinned at `now - k` would make age constant).
        let entered_at = SystemTime::UNIX_EPOCH;
        let seed = 0u64;

        // Empty run set, so `degraded: false` ⇒ Idle and `true` ⇒ Degraded.
        let mk = |degraded: bool, now: SystemTime| DaemonPresence {
            liveness: DaemonLiveness::Up { degraded },
            active_sessions: 0,
            last_seen: now,
            entered_at,
            in_flight_runs: Default::default(),
            current_pid: Some(1),
        };

        // An `age` where the idle and degraded cycles fall in DIFFERENT bands (one
        // walking, one resting), so the two anims must differ.
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
        let (_, idle_anim, _) = mascot_position(
            &layout,
            &test_pack(),
            &idle,
            "lobster_walk",
            "lobster_rest",
            now,
            seed,
        )
        .expect("idle pos");
        let (_, deg_anim, _) = mascot_position(
            &layout,
            &test_pack(),
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

    /// Every other Down test in this module hand-builds `DaemonPresence { last_seen:
    /// died_at, .. }` — it ASSUMES a correctly anchored clock, so none of them can
    /// observe a mis-anchored one. This one drives the REAL `apply_presence`.
    #[test]
    fn an_idle_gateway_that_is_killed_still_walks_out_instead_of_vanishing() {
        use pixtuoid_core::source::daemon::{
            apply_presence, DaemonInstanceKey, DaemonPresenceUpdate,
        };
        use pixtuoid_core::state::{DaemonInstanceId, SceneState};

        let layout = crate::layout::Layout::compute(200, 120, Some(4)).expect("layout fits");
        let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Idle far longer than the 2.2s walk-out window (and past the stagger +
        // walk-in, so the mascot is out in the wander when it dies).
        let killed_at = boot + std::time::Duration::from_millis(30_000);

        let src = "openclaw";
        let id = DaemonInstanceId::new("18901").expect("non-empty");
        let key = DaemonInstanceKey::new(src, id.clone());
        let mut scene = SceneState::default();
        apply_presence(
            &mut scene,
            &key,
            DaemonPresenceUpdate::GatewayUp { pid: Some(7) },
            boot,
        );
        // No traffic in between — an idle gateway sends nothing, so the boot receipt
        // is the only proof-of-life the clock has before this SIGKILL.
        apply_presence(
            &mut scene,
            &key,
            DaemonPresenceUpdate::PidExited { pid: 7 },
            killed_at,
        );

        let presence = scene
            .daemon(src, &id)
            .expect("the killed gateway is still Down");
        assert_eq!(presence.liveness, DaemonLiveness::Down);
        let seed = mascot_seed(src, &id);
        assert!(
            mascot_position(&layout, &test_pack(), presence, "w", "r", killed_at, seed).is_some(),
            "a gateway killed after idling must play its elevator walk-out; it \
             vanished instantly instead, which is what the exit-watch rung exists \
             to avoid"
        );
    }

    #[test]
    fn the_walk_out_starts_from_where_the_mascot_was_when_it_died() {
        use pixtuoid_core::state::DaemonInstanceId;
        let layout = crate::layout::Layout::compute(200, 120, Some(4)).expect("layout fits");
        let entered_at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Well past the stagger + the 2.2s walk-in, so both paths are in the wander.
        let died_at = entered_at + std::time::Duration::from_millis(30_000);

        for port in ["18901", "18902", "18903", "18904"] {
            let id = DaemonInstanceId::new(port).expect("non-empty");
            let seed = mascot_seed("openclaw", &id);
            let alive = DaemonPresence {
                liveness: DaemonLiveness::Up { degraded: false },
                active_sessions: 0,
                last_seen: died_at,
                entered_at,
                in_flight_runs: Default::default(),
                current_pid: Some(1),
            };
            let down = DaemonPresence {
                liveness: DaemonLiveness::Down,
                ..alive.clone()
            };

            let (was, _, _) =
                mascot_position(&layout, &test_pack(), &alive, "w", "r", died_at, seed)
                    .expect("a live gateway renders a mascot");
            let (leaving_from, _, _) =
                mascot_position(&layout, &test_pack(), &down, "w", "r", died_at, seed)
                    .expect("a just-died gateway is still walking out");
            // NOT byte-equality: the exit lerp routes its origin through
            // `walk_between`'s A*+snap, which shifts it a pixel or two off the raw
            // wander point.
            const MAX_SNAP_DRIFT_PX: i32 = 4;
            let drift = (i32::from(leaving_from.x) - i32::from(was.x))
                .abs()
                .max((i32::from(leaving_from.y) - i32::from(was.y)).abs());
            assert!(
                drift <= MAX_SNAP_DRIFT_PX,
                "gateway {port}: the walk-out must start at the lobster's last live \
                 position, or it teleports before heading for the elevator — was \
                 {was:?}, leaving from {leaving_from:?} ({drift}px)"
            );

            let delay = mascot_enter_delay(seed);
            assert!(delay > 0, "port {port} must exercise a real stagger");
            for early_ms in [0, delay / 2, delay - 1] {
                let at = entered_at + std::time::Duration::from_millis(early_ms);
                let held = DaemonPresence {
                    last_seen: at,
                    ..alive.clone()
                };
                assert!(
                    mascot_position(&layout, &test_pack(), &held, "w", "r", at, seed).is_none(),
                    "gateway {port} at age {early_ms}ms (< {delay}ms stagger) must not be \
                     drawn yet"
                );
            }
            let arrived = entered_at + std::time::Duration::from_millis(delay);
            let at_arrival = DaemonPresence {
                last_seen: arrived,
                ..alive.clone()
            };
            assert!(
                mascot_position(&layout, &test_pack(), &at_arrival, "w", "r", arrived, seed)
                    .is_some(),
                "gateway {port} must appear once its {delay}ms stagger elapses"
            );
        }
    }
}
