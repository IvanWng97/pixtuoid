//! Per-pose sprite anchor + breath bob + walking-position helpers.
//!
//! Pure geometry — no `RgbBuffer`, no rendering. The orchestrator calls
//! these to compute the top-left pixel where each character sprite
//! should land based on its pose (seated at desk, standing, walking,
//! at a waypoint, etc.).

use std::time::SystemTime;

use crate::layout::{SEAT_RENDER_Y_OFF, WALKING_Y_OFF};
use pixtuoid_core::AgentSlot;

use super::epoch_ms;
use crate::layout::{Point, WaypointKind, DESK_W};
// Walk-leg lerp lives on the sim side (`motion` — pose history records with
// it); re-imported here for the painter's blit/anchor call-sites.
pub(crate) use crate::motion::walking_position;
use crate::pose::{self, Pose};

/// Re-export the ONE cross-crate sprite-width authority (`layout::CHARACTER_SPRITE_W`)
/// so pixel_painter siblings keep importing it via `super::`. Used to anchor
/// LABELS (`character_anchor`), where a custom pack's true width isn't threaded
/// and ±1px doesn't matter; the sprite BLIT sites pass the real `frame.width`.
pub(super) use crate::layout::CHARACTER_SPRITE_W;

// All anchor fns center the sprite horizontally on `sprite_w` — the pack's
// character width (8 for the bundled pack, 10 for the robot pack). The vertical
// pose offsets (8/12/7) are NOT sprite height: both packs are 12px tall, so they
// stay fixed. Passing the real width keeps a non-8-wide pack centered.
pub(super) fn seated_anchor(desk: Point, sprite_w: u16) -> Point {
    Point {
        x: desk.x + DESK_W.saturating_sub(sprite_w) / 2,
        y: desk.y.saturating_sub(8),
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

/// One-pixel vertical bob on a ~4.5 s cycle with a per-agent phase offset,
/// so static (seated / standing) characters look alive instead of frozen.
/// Walking + waypoint-trip poses already animate, so we skip those.
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

/// Anchor for a back-view sitter on a mirror_vertical'd couch. Couch back
/// is now at the BOTTOM of the sprite, so the character's body sits
/// ENTIRELY ABOVE the couch back (head 7 px above couch center, body
/// ending right at the couch back row). Different from a normal front-view seat anchor
/// because back_couch.sprite has no transparent head/face area — its hair
/// extends across all top rows, so positioning it lower would put the
/// character's "head" overlapping the couch back row.
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
/// An EXCLUSIVE spot never steps aside. `exclusive` — the one authority for
/// "single-occupancy destination" — is the gate, so this holds for every seat
/// (couch / meeting sofa / meeting chair / island stand) AND the stand-beside
/// singles (phone booth, standing desk) AND anything added later, without a
/// second list to keep in sync. Sliding an occupant sideways off a discrete slot
/// renders them on thin air — and the old fossil arm made that concrete: its
/// generic +9 happened to equal `MEETING_CHAIR_TABLE_DX` (a chair's distance
/// from the table centre — a coincidence of two unrelated 9s, NOT one value;
/// `STEP_ASIDE_DX` is now shareable-only and never touches a chair), so a second
/// chair-sitter was parked ON the meeting table. The offsets predate per-seat
/// waypoints (one `Couch` waypoint used to spread 3 sitters over the sofa —
/// hence the old ±6 == `SEAT_DX`); exclusive spots are now single-occupancy at
/// SELECTION (`pose::SpotClaims`), so `rank` is always 0 here for them and the
/// guard is belt-and-braces: an exact overlap reads as one sprite, the honest
/// render of a bug, not a plausible wrong one. Shareable spots (pantry counter /
/// vending / printer / snack shelf) still step aside — queueing is the intent.
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

/// Current rendered position of an agent's character — derived from pose
/// so labels can follow the character rather than staying anchored at the
/// desk. Returns the top-left anchor of the character sprite. Uses
/// `derive_with_routing` so labels track agents along their A* path
/// instead of jumping the straight-line midpoint.
pub fn character_anchor(
    agent: &AgentSlot,
    layout: &crate::layout::Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
) -> Option<Point> {
    let desk = layout.home_desk(agent.desk_index.single_floor_local())?;
    let pose = pose::derive_with_routing(agent, now, layout, rctx)?;
    // Labels anchor off the DEFAULT character width — a custom pack's true
    // width isn't threaded here and ±1px doesn't matter for a text label.
    let w = CHARACTER_SPRITE_W;
    let anchor = match pose {
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => {
            seated_anchor(desk, w)
        }
        Pose::StandingAtDesk => standing_at_desk_anchor(desk, w),
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            // Anchor off the resolved stand cell (same `desk` origin as the
            // walk destination), so the label tracks where the agent actually
            // stands instead of the blocked furniture center.
            let stand = crate::layout::stand_point(
                wp_obj.kind,
                wp_obj.pos,
                layout.pantry_counter_size(),
                &layout.walkable,
                desk,
                wp_obj.facing,
                &layout.reachable,
            );
            match kind {
                WaypointKind::Couch | WaypointKind::MeetingSofa => back_couch_anchor(stand, w),
                _ => waypoint_anchor(stand, w),
            }
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest, w),
        Pose::Walking {
            from, to, t_x1000, ..
        } => walking_anchor(walking_position(from, to, t_x1000), w),
    };
    Some(anchor)
}

/// How long the elevator's open/close transition takes. Used as both
/// the opening ramp at the START of an agent's entry/exit window and
/// the closing ramp at the END. 200 ms feels snappy without being
/// abrupt — the half-open frame is visible for ~100 ms (DOOR_TRANSITION_MS/2) each way.
const DOOR_TRANSITION_MS: u64 = 200;

/// Compute the elevator door frame (0=closed, 1=half, 2=open) from
/// the agents currently in flight. Stateless: each agent contributes
/// a per-frame value based on how far through their entry/exit window
/// they are; we take the MAX across all agents so the door is at
/// least as open as the most-in-progress agent needs.
///
/// `door_anim_max_ms` is the per-floor cached maximum entry/exit physics
/// duration (written each frame from `fctx.motion`). Falls back to
/// `ENTRY_ANIMATION_MS` when zero (e.g. before any entry walk is in flight).
pub(super) fn compute_door_frame_idx(
    agents: &[AgentSlot],
    now: SystemTime,
    door_anim_max_ms: u64,
) -> usize {
    fn frame_for_progress(elapsed_ms: u64, total_ms: u64) -> usize {
        // 0..200ms: opening (0 → 1 → 2)
        if elapsed_ms < DOOR_TRANSITION_MS {
            if elapsed_ms < DOOR_TRANSITION_MS / 2 {
                1
            } else {
                2
            }
        } else if elapsed_ms + DOOR_TRANSITION_MS > total_ms {
            // last 200ms: closing (2 → 1 → 0)
            let remaining = total_ms.saturating_sub(elapsed_ms);
            if remaining < DOOR_TRANSITION_MS / 2 {
                0
            } else {
                1
            }
        } else {
            // middle: fully open
            2
        }
    }
    // Use the physics-derived window when available; fall back to the
    // fixed constant so the door cosmetic still works before any entry
    // is in flight (door_anim_max_ms == 0 at frame 0).
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
                // Use the same window the reducer uses to GC exiting
                // slots so the door closes right as the agent's slot
                // disappears.
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
