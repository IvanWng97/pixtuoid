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
/// TWO things are shared: the per-furniture OFFSET (`*_visit_spot`, derived from
/// the `DESK_W`/`DESK_H` consts — NOT any creature's sprite; both then
/// `walk_between` + `snap_point_to_walkable` to a "near this furniture" target, so
/// a footprint never enters the math) AND the social-venue GATHERING
/// (`social_visit_spots`: pantry + sofas + couch, in that order), which both
/// roamers visit. What stays DELIBERATELY per-creature is the state-conditional
/// SELECTION — WHICH set to roam: `pet_position` takes every spot + an `is_idle`
/// bool + the corridor; `mascot_spots` switches on `DaemonState` (Busy → desks,
/// Idle → the social set) with no corridor. They share where-beside-the-furniture
/// AND the social set, not which-furniture-WHEN.
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
    // The social venues (pantry / sofas / couch) — the shared gathering; none is
    // an idle desk, so each rides in with `false`.
    spots.extend(social_visit_spots(layout).into_iter().map(|pt| (pt, false)));
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

    let pick = |n: u64| -> (Point, bool) { spots[golden_index(n, spots.len())] };
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
/// `enqueue_gateway_mascots`), so a 2nd daemon adds exactly one arm here, not two
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

/// Golden-ratio hash of wander-cycle `n` into `[0, len)` — the index both roamers
/// pick a wander spot with (the mascot's `Point` list here, the pet's
/// `(Point, bool)` list in `pet_position`), so the `0x9e37…` multiplier + modulo
/// live once instead of a copy per pick.
fn golden_index(n: u64, len: usize) -> usize {
    (n.wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize) % len
}

fn hash_pick(spots: &[Point], n: u64) -> Point {
    spots[golden_index(n, spots.len())]
}

/// The office's SOCIAL visit-spots — a stand-beside point for the pantry, each
/// meeting sofa, and the lounge couch (in that order). The "where are the social
/// venues" GATHERING both roamers share: `pet_position` appends it to its full
/// spot list, `mascot_spots` uses it as its Idle-state set. This is furniture
/// gathering, NOT the state-conditional SELECTION (which set to roam) the module
/// doc reserves per-creature — the two stay distinct.
fn social_visit_spots(layout: &Layout) -> Vec<Point> {
    let mut spots = Vec::new();
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
    spots
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
        spots.extend(social_visit_spots(layout));
    }
    spots
}

/// The wander seed for ONE daemon instance — folds the source AND the instance id
/// (OpenClaw's resolved gateway port), so N gateways of one source take N different
/// paths, and a gateway restarting on its own port keeps its path (the id is
/// stable). Lives here, beside the motion it seeds: the painter only forwards it.
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
/// lobster — the seed reaches only the steady wander, so the lane the
/// multi-gateway feature is most likely to be seen through (pixtuoid starting
/// while every gateway is already up) was the one lane that collapsed them.
const MASCOT_ENTER_STAGGER_MS: u64 = 900;

/// The seeded walk-in delay for one mascot — its slice of
/// [`MASCOT_ENTER_STAGGER_MS`]. Position stays a pure function of `now` + the
/// presence timestamps + this seed (the stateless invariant — a mascot's motion
/// never depends on which SIBLINGS exist): the leg itself is untouched, it just
/// starts later, so the pop-free join to wander cycle 0 still holds.
///
/// The delay comes off an AVALANCHED hash, not `seed % STAGGER` directly. The
/// realistic multi-gateway deployment is CONSECUTIVE ports, whose folded seeds
/// differ by 1 — a raw modulo reads only the low bits and would hand four adjacent
/// gateways delays 1 ms apart, i.e. no stagger at all (measured: 396/397/398/399
/// for ports 18901-18904, vs 4/506/750/704 once mixed). Distribution, not
/// adversarial separation, is the claim: a rare near-collision between two
/// instances is possible and self-corrects at the wander, but the SYSTEMATIC
/// collapse is gone.
fn mascot_enter_delay(seed: u64) -> u64 {
    pixtuoid_core::id::splitmix64(seed) % MASCOT_ENTER_STAGGER_MS
}

/// How far a mascot may stand from the exact visit spot — its per-instance
/// standing offset. N mascots roam ONE small visit-spot set, so by the pigeonhole
/// principle two of N pick the SAME spot in the same cycle (with four gateways it
/// happens constantly), and without an offset they rest pixel-IDENTICAL: the user
/// runs four gateways and sees three lobsters. The office already answers exactly
/// this for multiple occupants of a SHAREABLE queue spot
/// (`pixel_painter::anchors::waypoint_rank_offset_x`'s ±`STEP_ASIDE_DX`); this is the
/// mascot lane's version. It cannot reuse that one: rank-keyed offsets require
/// knowing which siblings exist, and it takes a `WaypointKind` a mascot spot has no
/// equivalent of.
/// (No claim/probe machinery: these are shared social venues — two creatures at
/// the pantry is honest — and claims would couple one mascot's motion to which
/// siblings exist, breaking the stateless invariant.)
const MASCOT_SPOT_OFFSET_PX: i32 = 4;

/// The candidate standing offsets around a visit spot, tried in a seeded ROTATION.
/// A ring, not a random `(dx, dy)` draw: a raw nudge lands on the furniture as
/// often as not, and `snap_point_to_walkable` then pulls every instance back onto
/// the SAME free cell — the collision the offset exists to break. Walking the ring
/// from a seeded start takes the first offset that is genuinely walkable AND not
/// the shared spot itself, so two instances only collide when they also share a
/// ring position.
const MASCOT_SPOT_RING: [(i32, i32); 8] = [
    (MASCOT_SPOT_OFFSET_PX, 0),
    (-MASCOT_SPOT_OFFSET_PX, 0),
    (0, MASCOT_SPOT_OFFSET_PX),
    (0, -MASCOT_SPOT_OFFSET_PX),
    (MASCOT_SPOT_OFFSET_PX, MASCOT_SPOT_OFFSET_PX),
    (-MASCOT_SPOT_OFFSET_PX, MASCOT_SPOT_OFFSET_PX),
    (MASCOT_SPOT_OFFSET_PX, -MASCOT_SPOT_OFFSET_PX),
    (-MASCOT_SPOT_OFFSET_PX, -MASCOT_SPOT_OFFSET_PX),
];

/// The visit spot as THIS instance stands at it — the first walkable
/// [`MASCOT_SPOT_RING`] offset from a seeded start, else `p` itself when the spot
/// is boxed in. CONSTANT per instance, not per cycle, so a leg's `prev` and `dest`
/// shift together and the walk stays pop-free.
fn mascot_spot_for(layout: &Layout, p: Point, seed: u64) -> Point {
    // A distinct salt from the walk-in stagger's draw, so the two offsets one seed
    // yields are independent.
    let start = (pixtuoid_core::id::splitmix64(seed ^ 0x5A5C_0715) % MASCOT_SPOT_RING.len() as u64)
        as usize;
    for k in 0..MASCOT_SPOT_RING.len() {
        let (dx, dy) = MASCOT_SPOT_RING[(start + k) % MASCOT_SPOT_RING.len()];
        let cand = Point {
            x: p.x.saturating_add_signed(dx as i16),
            y: p.y.saturating_add_signed(dy as i16),
        };
        if layout.walkable.is_walkable(cand.x, cand.y) {
            return cand;
        }
    }
    p
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
        return (mascot_spot_for(layout, home, seed), false);
    }
    let cycle = we_ms / cycle_ms;
    let frac = (we_ms % cycle_ms) as f32 / cycle_ms as f32;
    let dest = mascot_spot_for(
        layout,
        hash_pick(spots, seed.wrapping_add(cycle).wrapping_add(1)),
        seed,
    );
    let prev = if cycle == 0 {
        home
    } else {
        mascot_spot_for(layout, hash_pick(spots, seed.wrapping_add(cycle)), seed)
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
    // Every clock below is measured from the END of this instance's stagger, so the
    // walk-out's reconstructed origin stays on the same wander phase as the walk-in.
    let enter_delay = mascot_enter_delay(seed);
    // The door is an anchor like any other: with N gateways first-sighted in one
    // beat — the very case the stagger exists for — all N would otherwise share this
    // ONE cell for up to 900ms before peeling off, which is the elevator half of the
    // overlap residual the scene guide quantifies. Same seeded ring the wander spots
    // use, applied at ALL THREE elevator sites below (hold, walk-in origin, walk-out
    // target) so the legs still join pop-free — offsetting only the hold would put a
    // jump at t=0.
    let door = mascot_spot_for(layout, elevator, seed);

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
            .saturating_sub(MASCOT_ENTER_MS + enter_delay);
        let (from, _) = mascot_wander(layout, down_we, seed, &spots, home, MASCOT_IDLE_CYCLE_MS);
        let t = down_age as f32 / MASCOT_LEAVE_MS as f32;
        return Some((walk_between(layout, from, door, t), walk_anim, frame));
    }

    let age = now.duration_since(presence.entered_at).ok()?.as_millis() as u64;
    if age < enter_delay {
        // Still holding at the door — this instance's stagger. REST, not walk: the
        // position is fixed, so an advancing walk cycle paddles in place (visible for
        // three of four consecutive-port gateways, whose delays are 4/506/750/704ms).
        return Some((door, rest_anim, 0));
    }
    let entered = age - enter_delay;
    if entered < MASCOT_ENTER_MS {
        // Walk-in from the door to the home beat.
        let t = entered as f32 / MASCOT_ENTER_MS as f32;
        return Some((walk_between(layout, door, home, t), walk_anim, frame));
    }

    // Steady wander, styled by state.
    let cycle_ms = match presence.display_state() {
        DaemonState::Busy => MASCOT_BUSY_CYCLE_MS,
        DaemonState::Degraded => MASCOT_DEGRADED_CYCLE_MS,
        _ => MASCOT_IDLE_CYCLE_MS,
    };
    let spots = mascot_spots(layout, presence.display_state(), home);
    let (pos, walking) = mascot_wander(
        layout,
        entered - MASCOT_ENTER_MS,
        seed,
        &spots,
        home,
        cycle_ms,
    );
    if walking {
        Some((pos, walk_anim, frame))
    } else {
        Some((pos, rest_anim, 0))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_registered_daemon_source_has_a_mascot_def() {
        // `gateway_mascot_def` is the ONE per-source daemon table with neither a
        // compile error nor a lockstep test behind it: a second `SourceKind::Daemon`
        // row would decode, key, sweep and roll up correctly and render NO mascot at
        // all — the daemon's only visible output gone, with every test and `doctor`
        // green. This is the twin of the registry lockstep guards the badge hues and
        // the wire matrix already carry ("registration is not coverage").
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
    fn golden_index_is_deterministic_and_in_range() {
        for len in [1usize, 3, 7, 50] {
            for n in [0u64, 1, 2, 999, u64::MAX] {
                let i = golden_index(n, len);
                assert!(i < len, "index {i} out of range for len {len}");
                assert_eq!(i, golden_index(n, len), "deterministic per (n, len)");
            }
        }
    }

    #[test]
    fn social_visit_spots_gathers_exactly_pantry_sofas_couch() {
        use crate::layout::{SceneLayout, WaypointKind};
        let l = SceneLayout::compute_with_seed(240, 170, None, 3).expect("fits");
        let has_pantry = l
            .waypoints
            .iter()
            .any(|w| matches!(w.kind, WaypointKind::Pantry)) as usize;
        let has_couch = l
            .waypoints
            .iter()
            .any(|w| matches!(w.kind, WaypointKind::Couch)) as usize;
        let n_sofas: usize = l
            .meeting_rooms
            .iter()
            .filter_map(|r| r.trio.as_ref())
            .map(|t| t.sofas.len())
            .sum();
        let spots = social_visit_spots(&l);
        // Exactly one spot per pantry(≤1) + each sofa + couch(≤1): no desks, no
        // corridor, no more, no less.
        assert_eq!(spots.len(), has_pantry + n_sofas + has_couch);
        assert!(
            has_pantry + n_sofas + has_couch > 0,
            "a 240x170 office has venues"
        );
        // ORDER is load-bearing (pet/mascot index this list via golden_index, so a
        // same-count reorder silently changes which venue is visited at a cycle),
        // and so is the per-venue offset fn. Pin pantry-corner FIRST, couch-corner
        // LAST (sofa spots between), each via its correct offset fn — a reorder or a
        // wrong offset fn breaks one of these even when the count still matches.
        if has_pantry == 1 {
            let pantry = l
                .waypoints
                .iter()
                .find(|w| matches!(w.kind, WaypointKind::Pantry))
                .unwrap();
            assert_eq!(
                spots[0],
                corner_visit_spot(pantry.pos),
                "pantry corner leads"
            );
        }
        if has_couch == 1 {
            let couch = l
                .waypoints
                .iter()
                .find(|w| matches!(w.kind, WaypointKind::Couch))
                .unwrap();
            assert_eq!(
                *spots.last().unwrap(),
                corner_visit_spot(couch.pos),
                "couch corner trails"
            );
        }
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
        // Ride the SAME golden_index production picks with, so this oracle can't
        // drift from `pet_position`'s pick (which it replicates).
        let pick = |n: u64| spots[golden_index(n, spots.len())];
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
        // (a) The empty-spots guard rests at the home beat — at THIS instance's own
        //     standing offset, so N mascots don't stack on one cell in a layout with
        //     no visit spots either.
        // (b) Cycle 0 forces prev=home (502) so leg 0 joins the enter walk pop-free:
        //     the walking position equals walk_between(home → hash_pick(spots, seed+1)).
        let layout = crate::layout::Layout::compute(160, 200, Some(4)).expect("layout fits");
        let home = mascot_home(&layout).expect("home beat");

        // (a) empty guard.
        assert_eq!(
            mascot_wander(&layout, 9_000, 7, &[], home, MASCOT_IDLE_CYCLE_MS),
            (mascot_spot_for(&layout, home, 7), false),
            "no spots → rest at home, at this instance's standing offset"
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
        // cycle == 0 → dest = the seed+1 spot at THIS instance's standing offset;
        // prev forced to home. Derived through the same helper the impl uses, so the
        // assertion is about the ORIGIN, not a second copy of the dest math.
        let dest = mascot_spot_for(
            &layout,
            hash_pick(&spots, seed.wrapping_add(0).wrapping_add(1)),
            seed,
        );
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
            in_flight_runs: Default::default(),
            current_pid: Some(1),
        }
    }

    #[test]
    fn consecutive_gateway_ports_get_spread_walk_in_delays() {
        // The realistic multi-gateway deployment is N CONSECUTIVE ports, and their
        // folded seeds differ by 1 — so a raw `seed % STAGGER` reads only the low
        // bits and hands every gateway a delay 1 ms from its neighbour's: the
        // stagger would exist in the code and not on screen. Pinned on the REAL
        // seeds (this is why `mascot_seed` lives here, not in the painter).
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
        // The seed itself is instance-DISTINCT for the same set (the wander's own
        // differentiation), so the two mechanisms can't silently share a weakness.
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

    #[test]
    fn two_instances_entering_together_are_never_superimposed_on_the_way_in() {
        // The walk-in was the one lane the seed did NOT reach: two gateways with the
        // same `entered_at` lerped the IDENTICAL elevator→home line, so for the whole
        // 2.2s window they rendered as ONE lobster — and the reachable case is the
        // common one (pixtuoid starting while both gateways are already up).
        let layout = crate::layout::Layout::compute(160, 120, Some(4)).expect("layout fits");
        let entered = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(20_000);
        // Two seeds whose stagger slices differ — the property the fix rests on.
        let (a, b) = (0u64, 450u64);
        assert_ne!(
            mascot_enter_delay(a),
            mascot_enter_delay(b),
            "the fixture must exercise two DIFFERENT stagger slices"
        );

        let pos_at = |seed: u64, age_ms: u64| {
            let now = entered + std::time::Duration::from_millis(age_ms);
            let p = idle_presence(now, age_ms);
            mascot_position(&layout, &p, "lobster_walk", "lobster_rest", now, seed)
                .expect("inside the enter window")
                .0
        };
        // The window where the claim holds: from when the LATER instance leaves the
        // door to before the EARLIER one joins its wander. The only remaining
        // legitimate co-location is crossing at the shared `home` beat as one
        // arrives while the other departs — ordinary traffic, not the collapse.
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

        // The STAGGER window itself, which this test used to concede ("held together
        // at the elevator door"): each instance now waits on its OWN seeded ring
        // offset from the door, so they are separated from the very first frame — the
        // elevator was the one anchor the standing-offset ring did not cover, and with
        // N gateways first-sighted in one beat it was where they visibly stacked.
        for age in (0..=da.min(db)).step_by(1) {
            assert_ne!(
                pos_at(a, age),
                pos_at(b, age),
                "two instances held at the door must not share its cell (age {age}ms)"
            );
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
        // The walk-in starts at THIS instance's door — the seeded ring offset from
        // the shared elevator cell, so N gateways don't stack on it — and the hold
        // before it uses the same point, so the leg joins pop-free.
        let door = mascot_spot_for(&layout, elevator, seed);
        assert_eq!(
            pos0,
            walk_between(&layout, door, home, 0.0),
            "age 0 → exactly at this instance's door"
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
            walk_between(&layout, door, home, t),
            "mid enter → the door→home interpolation"
        );
        // Sanity: midway is genuinely off both endpoints (so the lerp is live, not a
        // degenerate where door==home).
        assert_ne!(
            door, home,
            "the door and home must differ for a real walk-in"
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
            in_flight_runs: Default::default(),
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

    /// The ring's whole job is to give N mascots N DISTINCT places to stand at one
    /// shared visit spot. A duplicate entry silently shrinks the candidate set, so
    /// two instances collide more often — the "runs four gateways, sees three
    /// lobsters" bug the ring was written to prevent, back at lower probability.
    /// Mutation testing found nothing pinned it: deleting any of the three diagonal
    /// minus signs turns an entry into a copy of its neighbour, and neither scene's
    /// creature tests nor the binary's mascot harness went red.
    ///
    /// The ring must actually be USED, not merely declared correctly. Mutation
    /// testing found the const test below cannot see this: turning the INDEX
    /// reduction `(start + k) % LEN` into `/ LEN` leaves the const pristine while
    /// making the index only ever 0 or 1 — on an open floor every instance then
    /// returns ring[0], so all N mascots stand on the identical cell and the whole
    /// per-instance offset is dead.
    ///
    /// Threshold is 4 of 8 — the FLOOR of what the adjudicated mixing mutants
    /// produce, NOT a margin above them. Measured over ports 18901-18916: the real
    /// mixing spreads over 7 ring positions, the collapse yields exactly 1, and the
    /// two mutants excluded in `.cargo/mutants.toml` yield 4 (`^ -> |`) and 6
    /// (`^ -> &`). So this catches the collapse ONLY and never becomes a golden on
    /// the hash arithmetic — and do NOT tighten `>= 4`, because the OR mutant sits
    /// exactly ON it: tightening would start failing for a degradation this repo has
    /// deliberately left unpinned (see that config entry for why).
    #[test]
    fn consecutive_gateway_ports_spread_over_the_ring_instead_of_one_offset() {
        use pixtuoid_core::state::DaemonInstanceId;
        use pixtuoid_core::walkable::WalkableMask;
        let (w, h) = (200u16, 120u16);
        let mut layout = crate::layout::Layout::compute(w, h, Some(4)).expect("layout fits");
        // Fully open floor: no candidate is ever rejected, so the returned point is
        // exactly the ring entry the seed selected — this isolates the SELECTION
        // from the walkability filter.
        layout.walkable = WalkableMask::new_open(w, h);
        let spot = p(100, 60);

        // Consecutive ports are the realistic multi-gateway deployment (and what
        // `just openclaw-multi-e2e` runs), so they are the case that must not clump.
        let offsets: std::collections::BTreeSet<(i32, i32)> = (0..16u32)
            .map(|i| {
                let id = DaemonInstanceId::new((18901 + i).to_string()).expect("non-empty");
                let got = mascot_spot_for(&layout, spot, mascot_seed("openclaw", &id));
                (
                    i32::from(got.x) - i32::from(spot.x),
                    i32::from(got.y) - i32::from(spot.y),
                )
            })
            .collect();

        assert!(
            offsets.len() >= 4,
            "16 consecutive gateways must spread over the ring, not clump onto a few \
             cells — got {} distinct offsets: {offsets:?}",
            offsets.len()
        );
        assert!(
            !offsets.contains(&(0, 0)),
            "no instance may stand ON the shared spot — that is the collision the \
             offset exists to break: {offsets:?}"
        );
    }

    /// The walk-out must begin where the lobster actually WAS. `mascot_position`
    /// states this in a comment — "every clock below is measured from the END of this
    /// instance's stagger, so the walk-out's reconstructed origin stays on the same
    /// wander phase as the walk-in" — and the Down path implements it by subtracting
    /// `MASCOT_ENTER_MS + enter_delay`, exactly what the live path subtracts. Nothing
    /// tested it: mutation testing flipped that `+` to `-`, shifting the
    /// reconstruction by TWICE the stagger (up to 1500ms against a 9000ms idle cycle,
    /// so ~17% of a lap — a visible jump, not a rounding wobble) with the suite green.
    ///
    /// Asserted at the instant of death (`now == last_seen`, so the exit lerp is at
    /// t=0 and yields its own origin), which is what makes the two paths directly
    /// comparable without pinning any ms arithmetic.
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

            let (was, _, _) = mascot_position(&layout, &alive, "w", "r", died_at, seed)
                .expect("a live gateway renders a mascot");
            let (leaving_from, _, _) = mascot_position(&layout, &down, "w", "r", died_at, seed)
                .expect("a just-died gateway is still walking out");
            // NOT byte-equality: the exit lerp routes its origin through
            // `walk_between`'s A*+snap, which can shift it a pixel or two off the raw
            // wander point. Measured — real code deviates 0-2px across these four
            // ports, the `+ -> -` mutant 24px (port 18903, whose 750ms stagger is the
            // largest) — so 4 sits clear of both.
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

            // The stagger itself, which every assertion above sits PAST (age 30s).
            // During it the mascot holds AT the elevator; the frame after, it has
            // started walking. Turning the `age < enter_delay` guard into `==` makes
            // `age - enter_delay` underflow on the very first frame of a mascot's
            // life — a panic reachable by simply having a gateway appear.
            let delay = mascot_enter_delay(seed);
            assert!(delay > 0, "port {port} must exercise a real stagger");
            let elevator = mascot_elevator(&layout).expect("layout has an elevator");
            for early_ms in [0, delay / 2, delay - 1] {
                let at = entered_at + std::time::Duration::from_millis(early_ms);
                let held = DaemonPresence {
                    last_seen: at,
                    ..alive.clone()
                };
                let (pos, anim, frame) = mascot_position(&layout, &held, "w", "r", at, seed)
                    .expect("a staggered mascot still renders");
                // HELD means still: the position is fixed for the whole slice, so an
                // advancing walk cycle would paddle in place (visible for three of
                // four consecutive-port gateways, whose delays are 4/506/750/704ms).
                assert_eq!(
                    (anim, frame),
                    ("r", 0),
                    "gateway {port} held at the door must REST, not walk in place"
                );
                assert_eq!(
                    pos,
                    mascot_spot_for(&layout, elevator, seed),
                    "gateway {port} at age {early_ms}ms (< {delay}ms stagger) must hold \
                     at ITS OWN door offset, not the shared elevator cell"
                );
            }
        }
    }

    /// The BLOCKED half of the ring walk, which the open-floor test above cannot
    /// reach: it always succeeds at `k == 0`, so nothing there advances the cursor.
    /// Mutation testing exposed that gap — turning `(start + k)` into `(start - k)`
    /// survived, and it is a latent PANIC, not a cosmetic drift: `start` and `k` are
    /// `usize`, so the first seed with `start < k` underflows the moment a candidate
    /// is rejected. Only a mascot standing near furniture reaches that, which is
    /// precisely the case no test covered.
    #[test]
    fn a_boxed_in_spot_falls_back_to_itself_and_a_crowded_one_still_finds_a_free_cell() {
        use pixtuoid_core::state::DaemonInstanceId;
        use pixtuoid_core::walkable::WalkableMask;
        let (w, h) = (200u16, 120u16);
        let base = crate::layout::Layout::compute(w, h, Some(4)).expect("layout fits");
        let spot = p(100, 60);
        let seeds: Vec<u64> = (0..16u32)
            .map(|i| {
                let id = DaemonInstanceId::new((18901 + i).to_string()).expect("non-empty");
                mascot_seed("openclaw", &id)
            })
            .collect();

        // FULLY boxed in: every ring candidate blocked ⇒ the documented fallback is
        // the shared spot itself. Walks all 8 candidates for every seed, so an
        // underflowing cursor cannot hide behind an early success.
        let mut boxed = WalkableMask::new_open(w, h);
        for (dx, dy) in MASCOT_SPOT_RING {
            let x = (i32::from(spot.x) + dx) as u16;
            let y = (i32::from(spot.y) + dy) as u16;
            boxed.mark_blocked(x, y, 1, 1, 0);
        }
        let mut layout = base.clone();
        layout.walkable = boxed;
        for &seed in &seeds {
            assert_eq!(
                mascot_spot_for(&layout, spot, seed),
                spot,
                "a boxed-in spot has no free offset — the fallback is the spot itself"
            );
        }

        // CROWDED: exactly one candidate left open. Every seed must converge on it,
        // which means the cursor advanced past up to seven rejections.
        let free = MASCOT_SPOT_RING[3];
        let mut crowded = WalkableMask::new_open(w, h);
        for (dx, dy) in MASCOT_SPOT_RING {
            if (dx, dy) == free {
                continue;
            }
            let x = (i32::from(spot.x) + dx) as u16;
            let y = (i32::from(spot.y) + dy) as u16;
            crowded.mark_blocked(x, y, 1, 1, 0);
        }
        let mut layout = base;
        layout.walkable = crowded;
        let want = Point {
            x: (i32::from(spot.x) + free.0) as u16,
            y: (i32::from(spot.y) + free.1) as u16,
        };
        for &seed in &seeds {
            assert_eq!(
                mascot_spot_for(&layout, spot, seed),
                want,
                "the ONE walkable offset must be found from any seeded start"
            );
        }
    }

    /// Characterizes the set COMPLETELY (rather than spot-checking entries) so one
    /// assertion covers sign flips, duplicates and a changed magnitude alike.
    #[test]
    fn the_mascot_spot_ring_is_the_eight_distinct_neighbours_of_its_spot() {
        let o = MASCOT_SPOT_OFFSET_PX;
        let got: std::collections::BTreeSet<(i32, i32)> =
            MASCOT_SPOT_RING.iter().copied().collect();
        assert_eq!(
            got.len(),
            MASCOT_SPOT_RING.len(),
            "every ring offset must be DISTINCT — a duplicate re-collides two instances: {MASCOT_SPOT_RING:?}"
        );
        let want: std::collections::BTreeSet<(i32, i32)> = [-o, 0, o]
            .into_iter()
            .flat_map(|dx| [-o, 0, o].map(move |dy| (dx, dy)))
            .filter(|&p| p != (0, 0))
            .collect();
        assert_eq!(
            got, want,
            "the ring must be exactly the 8 one-step neighbours; (0,0) is EXCLUDED because \
             it is the shared spot itself, which is what two instances must not both take"
        );
    }
}
