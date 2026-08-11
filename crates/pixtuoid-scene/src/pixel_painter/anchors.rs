//! Per-pose sprite anchor + breath bob + walking-position helpers.
//!
//! Pure geometry — no `RgbBuffer`, no rendering.

use std::time::SystemTime;

use crate::layout::{SEAT_RENDER_Y_OFF, WALKING_Y_OFF};
use pixtuoid_core::AgentSlot;

use super::epoch_ms;
use super::seat::SeatView;
use crate::layout::{Point, WaypointKind, DESK_W};
pub(crate) use crate::motion::walking_position;
use crate::pose::{self, Pose};

/// The ONE cross-crate sprite-width authority, re-exported so `pixel_painter`
/// siblings keep importing it via `super::`.
pub(super) use crate::layout::CHARACTER_SPRITE_W;

// All anchor fns center the sprite horizontally on `sprite_w` — the pack's
// character width — so a non-8-wide pack stays centered. The vertical pose
// offsets (8/12/7) are NOT sprite height: both packs are 12px tall, so they
// stay fixed.
/// Where a desk's occupant RENDERS: the walk anchor lifted by the sprite height, so a facing that moves the seat moves both.
pub(super) fn seated_anchor_facing(
    desk: Point,
    sprite_w: u16,
    facing: crate::layout::Facing,
) -> Point {
    let walk = crate::layout::desk_walk_anchor_facing(desk, facing);
    Point {
        x: desk.x + DESK_W.saturating_sub(sprite_w) / 2,
        y: walk.y.saturating_sub(crate::layout::WALKING_Y_OFF),
    }
}

pub(super) fn standing_at_desk_anchor(desk: Point, sprite_w: u16) -> Point {
    Point {
        x: desk.x + DESK_W.saturating_sub(sprite_w) / 2,
        y: desk.y.saturating_sub(12),
    }
}

pub(super) fn walking_anchor(p: Point, sprite_w: u16) -> Point {
    Point {
        x: p.x.saturating_sub(sprite_w / 2),
        y: p.y.saturating_sub(WALKING_Y_OFF),
    }
}

pub(super) fn waypoint_anchor(wp: Point, sprite_w: u16) -> Point {
    Point {
        x: wp.x.saturating_sub(sprite_w / 2),
        y: wp.y.saturating_sub(WALKING_Y_OFF),
    }
}

/// One-pixel vertical bob on a ~4.5 s cycle with a per-agent phase offset, so
/// static (seated / standing) characters look alive instead of frozen.
fn breath_offset_y(agent_id: pixtuoid_core::AgentId, now: SystemTime) -> u16 {
    let elapsed_ms = epoch_ms(now);
    const CYCLE_MS: u64 = 4500;
    let offset_ms = agent_id.raw() % CYCLE_MS;
    let phase = elapsed_ms.wrapping_add(offset_ms) % CYCLE_MS;
    if phase < CYCLE_MS / 2 {
        0
    } else {
        1
    }
}

pub(super) fn with_breath(
    anchor: Point,
    agent_id: pixtuoid_core::AgentId,
    now: SystemTime,
) -> Point {
    Point {
        x: anchor.x,
        y: anchor.y.saturating_sub(breath_offset_y(agent_id, now)),
    }
}

/// Anchor for a back-view sitter on a mirror_vertical'd couch — higher than a
/// front-view seat anchor because `back_couch.sprite` has no transparent
/// head/face area (hair extends across all top rows), so sitting it lower
/// overlaps the couch back row.
pub(super) fn back_couch_anchor(wp: Point, sprite_w: u16) -> Point {
    Point {
        x: wp.x.saturating_sub(sprite_w / 2),
        y: wp.y.saturating_sub(SEAT_RENDER_Y_OFF),
    }
}

/// How far a later arrival steps aside along x so two agents at one
/// stand-beside spot don't render on top of each other. Sized to clear a
/// character sprite (8 px bundled) with a pixel of daylight.
const STEP_ASIDE_DX: i16 = 9;

/// X-offset applied to a waypoint anchor when multiple agents land at the
/// SAME waypoint in the same cycle. rank 0 = first arrival (no offset); later
/// arrivals step aside.
///
/// An EXCLUSIVE spot never steps aside: sliding an occupant sideways off a
/// discrete slot renders them on thin air — a generic +9 once parked a second
/// chair-sitter ON the meeting table. Gating on `exclusive` — the one authority
/// for "single-occupancy destination" — covers every seat, the stand-beside
/// singles, and anything added later without a second list to keep in sync.
/// Shareable spots (pantry counter / vending / printer / snack shelf) still step
/// aside; queueing is the intent there.
pub(super) fn waypoint_rank_offset_x(kind: WaypointKind, rank: usize) -> i16 {
    if crate::layout::furniture_def(kind.furniture()).exclusive {
        return 0;
    }
    match rank {
        1 => STEP_ASIDE_DX,
        2 => -STEP_ASIDE_DX,
        _ => 0,
    }
}

/// Top-left anchor of an agent's character sprite, derived from pose so labels
/// follow the character rather than staying anchored at the desk. Uses
/// `derive_with_routing` so labels track agents along their A* path instead of
/// jumping to the straight-line midpoint.
pub fn character_anchor(
    agent: &AgentSlot,
    layout: &crate::layout::Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
) -> Option<Point> {
    let desk = layout.home_desk(agent.desk_index.single_floor_local())?;
    let pose = pose::derive_with_routing(agent, now, layout, rctx)?;
    // Labels use the DEFAULT width — a custom pack's true width isn't threaded
    // here and ±1px doesn't matter; blit sites pass the real `frame.width`.
    let w = CHARACTER_SPRITE_W;
    let anchor = match pose {
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => {
            seated_anchor_facing(
                desk,
                w,
                layout.desk_facing(agent.desk_index.single_floor_local()),
            )
        }
        Pose::StandingAtDesk => standing_at_desk_anchor(desk, w),
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            // Anchor off the resolved stand cell so the label tracks where the
            // agent actually stands, not the blocked furniture center.
            let stand = layout.stand_point(wp_obj.kind, wp_obj.pos, desk, wp_obj.facing);
            // Via the ONE authority the sprite blit uses, so label-vs-sprite
            // drift is structurally impossible.
            SeatView::of(kind, wp_obj.facing)
                .waypoint_render_anchor(stand, w)
                .0
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest, w),
        Pose::Walking {
            from, to, t_x1000, ..
        } => walking_anchor(walking_position(from, to, t_x1000), w),
    };
    Some(anchor)
}

/// How long the elevator's open/close transition takes, used as both the opening
/// ramp at the START of an agent's entry/exit window and the closing ramp at the
/// END. 200 ms feels snappy without being abrupt.
const DOOR_TRANSITION_MS: u64 = 200;

/// Compute the elevator door frame (0=closed, 1=half, 2=open) from the agents
/// currently in flight. Stateless: we take the MAX across agents so the door is
/// at least as open as the most-in-progress one needs.
///
/// `door_anim_max_ms` is the per-floor cached maximum entry/exit physics
/// duration; it falls back to `ENTRY_ANIMATION_MS` when zero (before any entry
/// walk is in flight).
pub(super) fn compute_door_frame_idx(
    agents: &[AgentSlot],
    now: SystemTime,
    door_anim_max_ms: u64,
) -> usize {
    fn frame_for_progress(elapsed_ms: u64, total_ms: u64) -> usize {
        if elapsed_ms < DOOR_TRANSITION_MS {
            if elapsed_ms < DOOR_TRANSITION_MS / 2 {
                1
            } else {
                2
            }
        } else if elapsed_ms + DOOR_TRANSITION_MS > total_ms {
            let remaining = total_ms.saturating_sub(elapsed_ms);
            if remaining < DOOR_TRANSITION_MS / 2 {
                0
            } else {
                1
            }
        } else {
            2
        }
    }
    let entry_window_ms = if door_anim_max_ms > 0 {
        door_anim_max_ms
    } else {
        pose::ENTRY_ANIMATION_MS
    };

    let mut max_frame: usize = 0;
    for a in agents {
        if a.exiting_at.is_none() {
            if let Ok(d) = now.duration_since(a.created_at) {
                let ms = d.as_millis() as u64;
                if ms < entry_window_ms {
                    max_frame = max_frame.max(frame_for_progress(ms, entry_window_ms));
                }
            }
        }
        if let Some(exit_at) = a.exiting_at {
            if let Ok(d) = now.duration_since(exit_at) {
                let ms = d.as_millis() as u64;
                // The same window the reducer uses to GC exiting slots, so the
                // door closes right as the agent's slot disappears.
                let exit_window_ms =
                    pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64;
                if ms < exit_window_ms {
                    max_frame = max_frame.max(frame_for_progress(ms, exit_window_ms));
                }
            }
        }
    }
    max_frame
}
