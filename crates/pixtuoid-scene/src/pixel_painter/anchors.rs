//! Per-pose sprite anchor + breath bob + walking-position helpers.
//!
//! Pure geometry — no `RgbBuffer`, no rendering.

use std::time::SystemTime;

use crate::layout::{Anchor, Size, SEAT_RENDER_Y_OFF, WALKING_Y_OFF};
use pixtuoid_core::AgentSlot;

use super::epoch_ms;
use super::seat::Seat;
use crate::layout::{Point, WaypointKind};
pub(crate) use crate::motion::walking_position;
use crate::pose::{self, Pose};

/// The ONE cross-crate sprite-width authority, re-exported so `pixel_painter`
/// siblings keep importing it via `super::`.
pub(super) use crate::layout::CHARACTER_SPRITE_W;

// All anchor fns center the sprite horizontally on `sprite_w` — the pack's
// character width — so a non-8-wide pack stays centered. The vertical pose
// offsets (8/12/7) are NOT sprite height: both packs are 12px tall, so they
// stay fixed.
/// Where a desk's occupant RENDERS — the desk's seat cell put through the same
/// `Seat` model every other seat uses, so the chair, its occupant and the walk
/// that ends there cannot drift apart. Re-exported from `pixel_painter` so the
/// binary's hit-test can't drift from the fn that places the sprite.
pub fn seated_anchor_facing(desk: Point, sprite_w: u16, facing: crate::layout::Facing) -> Point {
    Seat::at_desk(desk, facing).render_anchor(sprite_w)
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

/// Nudge a sprite so the whole frame lands inside the canvas, answering in the
/// SAME anchor space `pos` came in.
///
/// Parameterized on [`Anchor`] rather than split in two, so no call site can
/// reach for the other convention's bounds: centre-anchored creatures and
/// top-left-anchored characters need different arithmetic and read it off one
/// authority.
///
/// It lives at PAINT because invariant #6 runs one way: sprite size never moves
/// a sim position.
pub(crate) fn keep_sprite_on_canvas(anchor: Anchor, pos: Point, size: Size, buf: Size) -> Point {
    match anchor {
        // `min` before `max`: on a buffer narrower than the sprite the lower
        // bound wins (sprite flush left/top) instead of `clamp`'s
        // inverted-range panic.
        Anchor::Center => Point {
            x: pos
                .x
                .min(buf.w.saturating_sub(size.w.div_ceil(2)))
                .max(size.w / 2),
            y: pos
                .y
                .min(buf.h.saturating_sub(size.h.div_ceil(2)))
                .max(size.h / 2),
        },
        // No lower bound needed — `u16` already floors a top-left `pos` at 0.
        Anchor::TopLeft => Point {
            x: pos.x.min(buf.w.saturating_sub(size.w)),
            y: pos.y.min(buf.h.saturating_sub(size.h)),
        },
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
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            // Anchor off the resolved stand cell so the label tracks where the
            // agent actually stands, not the blocked furniture center.
            let stand = layout.stand_point(wp_obj.kind, wp_obj.pos, desk, wp_obj.facing);
            // Via the ONE authority the sprite blit uses, so label-vs-sprite
            // drift is structurally impossible.
            Seat::at_waypoint(kind, stand, wp_obj.facing).render_anchor(w)
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest, w),
        Pose::Walking {
            from, to, t_x1000, ..
        } => walking_anchor(walking_position(from, to, t_x1000), w),
    };
    // The label's twin of the sprite's own guard in `sim::resolve_characters`,
    // on the DEFAULT frame size like the anchor above — a badge that stayed put
    // while its sprite was nudged would be worse than the ±1px.
    Some(keep_sprite_on_canvas(
        Anchor::TopLeft,
        anchor,
        Size {
            w,
            h: crate::layout::CHARACTER_SPRITE_H_CELLS * 2,
        },
        Size {
            w: layout.buf_w,
            h: layout.buf_h,
        },
    ))
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
