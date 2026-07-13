//! Seat orientation + seated/standing character painting. `SeatView` is the
//! single source of truth for how a waypoint occupant faces (sprite + flip +
//! sit-down glide + z-key); `paint_character_at` is the shared recolor-blit.
//! Extracted from mod.rs; see tui/CLAUDE.md ("How is the office rendered").

use super::*;

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving. `glow_tint` should carry the tool-derived monitor color
/// when the character is at a lit screen (SeatedTyping); tints the skin
/// toward that color so the eye reads "the monitor is lighting their face."
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    glow_tint: Option<Rgb>,
    cache: &mut FrameCache,
    now: SystemTime,
) {
    let Some(anim) = pack.animation(anim_name) else {
        return;
    };
    let Some(frame) = frame_at(anim, frame_idx) else {
        return;
    };
    // A cwd backfill re-keys the outfit (Team Palette) mid-lifetime — flag the
    // change so the cache drops the agent's stale recolors before the lookup.
    cache.note_outfit_seed(agent.agent_id, outfit_seed_for(agent));
    // Burn tier (model gate × effort split, see `crate::burn`) recolors the
    // hair and, at Top, crowns the head with flame — judged here in the ONE
    // shared blit so every pose (seated/walking/standing) rides it.
    let burn = crate::burn::slot_burn_tier(agent, now);
    let cached = cache.get_or_make(
        crate::frame_cache::FrameKey {
            agent_id: agent.agent_id,
            anim_name,
            frame_idx,
            flip_x,
            glow_tint,
            burn,
        },
        || {
            let pal = agent_palette(&pack.palette, agent, glow_tint, burn);
            let recolored = recolor_frame(frame, &pal, &pack.palette);
            if flip_x {
                recolored.mirror_horizontal()
            } else {
                recolored
            }
        },
    );
    let sprite_w = cached.width();
    blit_frame(cached, anchor.x, anchor.y, buf);
    if burn == crate::burn::BurnTier::Top {
        super::effects::paint_flame_crown(buf, anchor, sprite_w, now);
    }
}

/// Sprite name + horizontal flip for an agent SEATED at a seat slot, by its
/// SEATED facing (the `facing` field = which way the sitter LOOKS, decoupled
/// from the approach side). A `Facing::North` sitter shows its back (`back_couch`):
/// the lounge couch (always looks at the window/North) and the south-side meeting
/// sofa; other meeting-sofa seats face the viewer across the table (front
/// `seated`), and the head-of-table chairs sit the same way — the painted
/// chair body carries their E/W orientation. Extracted so the facing→sprite
/// mapping is unit-testable.
pub(super) fn seat_sprite(
    kind: crate::layout::WaypointKind,
    facing: crate::layout::Facing,
) -> (&'static str, bool) {
    SeatView::of(kind, facing).seated_sprite()
}

/// The single orientation a seat occupant is shown in — the ONE source BOTH the
/// seated render (`AtWaypoint`, via [`seat_sprite`]) and the sit-down WALK glide
/// onto the seat derive from, so the two can never disagree.
///
/// This is the data-model fix for the recurring "sit facing the wrong way then
/// snap" bug. The two renders used to compute facing independently — the seated
/// sprite from the seat's `facing` field, the glide from the travel direction —
/// and disagreed whenever a seat faces away from its open approach side. A
/// window-facing (`North`) seat (lounge couch AND the south-of-table meeting
/// sofa) is reached from the north, but its foot-cell is pinned SOUTH
/// (`seated_foot_cell` = `pos + (WALKING_Y_OFF − SEAT_RENDER_Y_OFF)`, fixed by
/// pop-free head-alignment); so the settle travels south, the directional walk
/// rule rendered a FRONT walk, and the agent sat facing the camera for ~1s before
/// snapping to `back_couch` at `AtWaypoint`.
///
/// Routing both renders through `SeatView` makes the disagreement structurally
/// impossible: a new seatable furniture picks a view here ONCE (or falls through
/// to the upright default) and the seated sprite, the flip, and the sit-down
/// glide all follow — the bug cannot reappear for a future seat. Sprite names
/// stay in the painter because pixtuoid-core forbids terminal/pack deps; the
/// per-instance `facing` is the core data this projects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SeatView {
    /// Faces the camera (south) — front `seated` / `walking`.
    Front,
    /// Faces away (the window / back wall) — `back_couch` / `walking_back`.
    Back,
    /// Faces sideways; `flip` mirrors east↔west.
    Side { flip: bool },
    /// An upright stander at the plain feet-row z (no `Side`-style table
    /// clearance): the island slots. The bartender pair stands INSIDE the
    /// island body, so its z must stay BELOW the island's south-row key for
    /// the whole glide + settled arc (the legs-behind-the-counter read);
    /// `Side`'s `pos+3` would tie with the island key and pop the sprite in
    /// front of the counter mid-glide.
    Stander { flip: bool },
}

impl SeatView {
    /// The view a `kind` occupant looks in, from its seat `facing`. The ONE place
    /// a seat's orientation is decided — extend HERE to add a seatable furniture.
    pub(super) fn of(kind: crate::layout::WaypointKind, facing: crate::layout::Facing) -> Self {
        use crate::layout::{Facing, WaypointKind};
        match kind {
            // Couch + sofa: North looks at the window/back wall (back view); the
            // other seats face the viewer across the table.
            WaypointKind::Couch | WaypointKind::MeetingSofa => match facing {
                Facing::North => SeatView::Back,
                _ => SeatView::Front,
            },
            // Head-of-table chairs (decor arc): the occupant SITS. No side-
            // facing seated sprite exists, so both ends use the front `seated`
            // render — the pixel-office convention the north-sofa sitters
            // already follow; the chair body painted under them carries the
            // E/W orientation.
            WaypointKind::MeetingChair => SeatView::Front,
            // Island slots (flanks + the in-body bartender pair) stand at the
            // plain feet-row z — see the `Stander` variant's WHY.
            WaypointKind::Island => SeatView::Stander {
                flip: matches!(facing, Facing::East),
            },
            // Not seat slots — the caller dispatches these directly (they never
            // reach a seated render through SeatView); upright is the safe default.
            // Listed EXPLICITLY (no `_`) so a new WaypointKind is a compile error
            // HERE, forcing a deliberate decision instead of silently rendering as
            // a stander. The totality-guard test still pins the seat-kind set.
            WaypointKind::Pantry
            | WaypointKind::PhoneBooth
            | WaypointKind::StandingDesk
            | WaypointKind::VendingMachine
            | WaypointKind::Printer
            | WaypointKind::SnackShelf => SeatView::Side { flip: false },
        }
    }

    /// Sprite + horizontal flip for the SEATED / standing render (`AtWaypoint`).
    pub(super) fn seated_sprite(self) -> (&'static str, bool) {
        match self {
            SeatView::Front => ("seated", false),
            SeatView::Back => ("back_couch", false),
            SeatView::Side { flip } | SeatView::Stander { flip } => ("standing", flip),
        }
    }

    /// `(going_back, flip)` for the sit-down WALK glide that settles onto the
    /// seat — the SAME orientation as [`seated_sprite`](Self::seated_sprite), so
    /// the sit-down never faces the wrong way (overrides the travel-direction
    /// rule for this terminal segment).
    pub(super) fn settle_walk(self) -> (bool, bool) {
        match self {
            SeatView::Front => (false, false),
            SeatView::Back => (true, false),
            SeatView::Side { flip } | SeatView::Stander { flip } => (false, flip),
        }
    }

    /// The y-sort key for an agent occupying this seat at waypoint centre
    /// `wp_pos` — used BOTH for the settled `AtWaypoint` render AND for the
    /// sit-down / stand-up WALK glide. Using one key for the whole sit arc is the
    /// z-sort half of the single-source fix: the settle is a `Walking` pose whose
    /// natural z-key is the foot position (`pos.y`), which glides from the
    /// approach point down to `seated_foot_cell = pos+5` and so CROSSES the
    /// furniture's own z-key (`pos+3` for a couch/back sofa) for a frame or two
    /// before snapping to the seated key (`pos+2`) — the agent pops in front of
    /// the sofa mid-glide, then jumps behind it. Pinning the glide to this stable
    /// key keeps the agent on the correct side of its furniture for the entire
    /// arc. Values match the historical `AtWaypoint` formulas exactly (back/front:
    /// `back_couch_anchor.y + 9 = pos+2`; side/stand: `waypoint_anchor.y + 12 + 3
    /// = pos+3`, the +3 clearing the meeting-table z-key), so the seated render is
    /// unchanged — only the glide is pulled into agreement with it.
    pub(super) fn z_key_for_seat(self, wp_pos: Point) -> u16 {
        match self {
            // Behind a couch/sofa back (furniture sorts at pos+3) or tied with a
            // front sofa (pos+2, insertion order puts the sitter on top).
            SeatView::Front | SeatView::Back => wp_pos.y + 2,
            // Stand-beside-the-table clearance (+3 over the table's y+2).
            // No seat kind routes Side today (the head-of-table chairs went
            // Front) and the non-seat obstacle kinds never settle onto a
            // seat, so this arm is a defensive default, not a live seat key.
            SeatView::Side { .. } => wp_pos.y + 3,
            // Plain feet-row key — the AtWaypoint default for a stander. The
            // bartender's pos row sits INSIDE the island body, below the
            // island's own south-row key, so the whole arc stays behind it.
            SeatView::Stander { .. } => wp_pos.y,
        }
    }
}

/// The seated [`SeatView`] (for the glide facing) and the seat's stable z-key
/// for the seat whose settle foot-cell is `cell`, or `None` if `cell` is not a
/// seat foot-cell. The caller passes the glide's `to` (sit-down: settling ONTO
/// the seat) and/or `from` (stand-up: rising OFF it) — either endpoint landing
/// on a foot-cell means the agent is on the sit arc and must render in the
/// seat's view and at the seat's stable z-key, not the travel-direction /
/// foot-position values.
///
/// Covers ALL seatables:
/// - Wander seats — any `layout.waypoints` entry whose furniture has a
///   `seated_foot_cell`; the view comes from [`SeatView::of`], the z-key from
///   [`SeatView::z_key_for_seat`].
/// - The home desk — `layout.home_desks` are NOT waypoints, but the chair
///   (`seated_foot_cell(Desk)` = `desk_walk_anchor`) is a settle target too once
///   the desk's arrival glides onto it (see `pose::desk_approach_cell`). The desk
///   sitter faces the camera (front) and renders at the desk's seated z-key
///   (`seated_anchor.y + 12 = desk.y + 4`, below the desk furniture's `desk.y + 7`),
///   so the glide stays behind the desk — no front-cross.
pub(super) fn settle_seat_view(cell: Point, layout: &Layout) -> Option<(SeatView, u16)> {
    use crate::layout::{seated_foot_cell, Furniture};
    layout
        .waypoints
        .iter()
        .find_map(|w| {
            (seated_foot_cell(w.kind.furniture(), w.pos) == Some(cell)).then(|| {
                let view = SeatView::of(w.kind, w.facing);
                (view, view.z_key_for_seat(w.pos))
            })
        })
        .or_else(|| {
            layout.home_desks.iter().find_map(|&desk| {
                (seated_foot_cell(Furniture::Desk, desk) == Some(cell))
                    // == the seated arms' `anchor_no_breath.y + 12` (= desk.y+4);
                    // pinned by `desk_settle_z_key_matches_the_seated_arm`.
                    .then_some((SeatView::Front, desk.y + DESK_SEAT_Z_OFF))
            })
        })
}

/// The home-desk sitter's z-key offset south of `desk`: `seated_anchor.y(=desk.y
/// − 8) + sprite_h(12) = desk.y + 4`. Below the desk furniture key (`desk.y + 7`)
/// so the sitter and its sit-down glide always sort behind the desk monitor.
pub(super) const DESK_SEAT_Z_OFF: u16 = 4;
