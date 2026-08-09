//! Seat orientation + seated/standing character painting. `SeatView` is the
//! single source of truth for how a waypoint occupant faces (sprite + flip +
//! sit-down glide + z-key); `paint_character_at` is the shared recolor-blit.

use super::*;

use super::anchors::{back_couch_anchor, waypoint_anchor};

/// The per-agent RECOLORED sprite for one character, from the cache.
///
/// Split out of [`paint_character_at`] so a second profile gets the identical
/// palette without a second copy of the rule. Only the BLIT differs between
/// profiles — the classic pass writes 1:1, the cutaway writes at its render
/// scale — and a per-agent palette is exactly the thing that must NOT differ:
/// hair, skin and the cwd-keyed outfit are how a viewer tells two agents apart,
/// so an agent who is auburn in one profile and default-brown in the other is
/// two different people to the eye.
///
/// Returns the burn tier alongside, because the caller owns the flame crown
/// (it is painted at the caller's own coordinates).
#[allow(clippy::too_many_arguments)]
pub(crate) fn character_frame<'c>(
    anim_name: &'static str,
    frame_idx: usize,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    glow_tint: Option<Rgb>,
    cache: &'c mut FrameCache,
    now: SystemTime,
) -> Option<(&'c Frame, crate::burn::BurnTier)> {
    let anim = pack.animation(anim_name)?;
    let frame = frame_at(anim, frame_idx)?;
    // A cwd backfill re-keys the outfit (Team Palette) mid-lifetime — flag the
    // change so the cache drops the agent's stale recolors before the lookup.
    cache.note_outfit_seed(agent.agent_id, outfit_seed_for(agent));
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
                // HORIZONTAL: `flip_x` is which way the character FACES.
                recolored.mirror_horizontal()
            } else {
                recolored
            }
        },
    );
    Some((cached, burn))
}

/// Paint a character at an arbitrary anchor with per-agent recolor. `glow_tint`
/// carries the tool-derived monitor color when the character is at a lit screen,
/// tinting the skin so the eye reads "the monitor is lighting their face."
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_character_at(
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
    let Some((cached, burn)) = character_frame(
        anim_name, frame_idx, agent, pack, flip_x, glow_tint, cache, now,
    ) else {
        return;
    };
    let sprite_w = cached.width();
    blit_frame(cached, anchor.x, anchor.y, buf);
    if burn == crate::burn::BurnTier::Top {
        super::effects::paint_flame_crown(buf, anchor, sprite_w, now);
    }
}

/// Sprite name + horizontal flip for an agent SEATED at a seat slot, by its
/// SEATED facing (which way the sitter LOOKS, decoupled from the approach side).
pub(super) fn seat_sprite(
    kind: crate::layout::WaypointKind,
    facing: crate::layout::Facing,
) -> (&'static str, bool) {
    SeatView::of(kind, facing).seated_sprite()
}

/// [`seat_sprite`] resolved against a PACK: character animations are never
/// inherited from the embedded default (`merge_from` is furniture-only), so a
/// pre-`side_seated` custom pack degrades to the front pose — a missing
/// animation must never mean an invisible sitter.
pub(super) fn seat_sprite_in_pack(
    pack: &Pack,
    kind: crate::layout::WaypointKind,
    facing: crate::layout::Facing,
) -> (&'static str, bool) {
    let (anim, flip) = seat_sprite(kind, facing);
    if pack.animation(anim).is_some() {
        (anim, flip)
    } else {
        ("seated", false)
    }
}

/// The single orientation a seat occupant is shown in — the ONE source BOTH the
/// seated render (`AtWaypoint`, via [`seat_sprite`]) and the sit-down WALK glide
/// derive from, so a new seatable furniture picks a view here ONCE.
///
/// Deriving the glide facing from the travel direction instead is the recurring
/// "sit facing the wrong way then snap" bug: a window-facing (`North`) seat is
/// approached from the north but its foot-cell is pinned SOUTH, so the settle
/// travels south, renders a FRONT walk, and the agent sits facing the camera for
/// ~1s before snapping to `back_couch`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SeatView {
    /// Faces the camera (south) — front `seated` / `walking`.
    Front,
    /// Faces away (the window / back wall) — `back_couch` / `walking_back`.
    Back,
    /// Faces sideways; `flip` mirrors east↔west.
    Side { flip: bool },
    /// SITS sideways — the head-of-table chairs. Same seat anchor + z-key as
    /// `Front` (the profile sprite shares the front `seated` sprite's 8x10
    /// bottom-row geometry); only the sprite + mirror differ.
    SideSeated { flip: bool },
    /// An upright stander at the plain feet-row z (no `Side`-style table
    /// clearance): the island slots. The bartender pair stands INSIDE the island
    /// body, so its z must stay BELOW the island's south-row key for the whole
    /// arc (the legs-behind-the-counter read) — `Side`'s `pos+3` would tie with
    /// the island key and pop the sprite in front of the counter mid-glide.
    Stander { flip: bool },
}

impl SeatView {
    /// The view a `kind` occupant looks in, from its seat `facing`. The ONE place
    /// a seat's orientation is decided — extend HERE to add a seatable furniture.
    pub(super) fn of(kind: crate::layout::WaypointKind, facing: crate::layout::Facing) -> Self {
        use crate::layout::{Facing, WaypointKind};
        match kind {
            WaypointKind::Couch | WaypointKind::MeetingSofa => match facing {
                Facing::North => SeatView::Back,
                _ => SeatView::Front,
            },
            // The base `side_seated` sprite faces East (the west chair's view),
            // so the east chair mirrors.
            WaypointKind::MeetingChair => SeatView::SideSeated {
                flip: matches!(facing, Facing::West),
            },
            WaypointKind::Island => SeatView::Stander {
                flip: matches!(facing, Facing::East),
            },
            // Not seat slots — the caller dispatches these directly; upright is
            // the safe default. Listed EXPLICITLY (no `_`) so a new WaypointKind
            // is a compile error HERE, forcing a deliberate decision instead of
            // silently rendering as a stander.
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
            SeatView::SideSeated { flip } => ("side_seated", flip),
            SeatView::Side { flip } | SeatView::Stander { flip } => ("standing", flip),
        }
    }

    /// `(going_back, flip)` for the sit-down WALK glide that settles onto the
    /// seat — the SAME orientation as [`seated_sprite`](Self::seated_sprite),
    /// overriding the travel-direction rule for this terminal segment.
    pub(super) fn settle_walk(self) -> (bool, bool) {
        match self {
            SeatView::Front => (false, false),
            SeatView::Back => (true, false),
            SeatView::SideSeated { flip }
            | SeatView::Side { flip }
            | SeatView::Stander { flip } => (false, flip),
        }
    }

    /// The y-sort key for an agent occupying this seat at waypoint centre
    /// `wp_pos` — used BOTH for the settled `AtWaypoint` render AND for the
    /// sit-down / stand-up WALK glide. Letting the glide keep its natural foot
    /// z-key instead makes it cross the furniture's own key on the way down: the
    /// agent pops in front of the sofa mid-glide, then jumps behind it.
    pub(super) fn z_key_for_seat(self, wp_pos: Point) -> u16 {
        match self {
            // Behind a couch/sofa back (furniture sorts at pos+3) or tied with a
            // front sofa (pos+2, insertion order puts the sitter on top).
            SeatView::Front | SeatView::Back | SeatView::SideSeated { .. } => wp_pos.y + 2,
            // Stand-beside-the-table clearance (+3 over the table's y+2). No seat
            // kind routes Side, so this arm is a defensive default.
            SeatView::Side { .. } => wp_pos.y + 3,
            // Plain feet-row key. The bartender's pos row sits INSIDE the island
            // body, below the island's own south-row key, so the whole arc stays
            // behind it.
            SeatView::Stander { .. } => wp_pos.y,
        }
    }

    /// The render ANCHOR-BASE + sprite base-row height for a WAYPOINT occupant
    /// at resolved stand cell `stand`. The ONE authority BOTH the sprite blit
    /// (`sim::resolve_characters`) AND its label twin
    /// (`anchors::character_anchor`) derive the anchor from, so the badge can
    /// never float above the sitter. The home-DESK sitter is NOT covered here —
    /// it anchors via `seated_anchor_facing(desk, w, layout.desk_facing_at(desk))`.
    pub(super) fn waypoint_render_anchor(self, stand: Point, sprite_w: u16) -> (Point, u16) {
        // UPRIGHT height REUSES the offset `waypoint_anchor` subtracts, so the
        // obstacle z-key `anchor.y + sprite_h` recovers the feet row BY
        // CONSTRUCTION (can't drift). SEATED height is parity-only: seat kinds
        // z-sort via `z_key_for_seat`, so `sprite_h` is dead on that path.
        const UPRIGHT_SPRITE_H: u16 = crate::layout::WALKING_Y_OFF;
        const SEATED_SPRITE_H: u16 = 9;
        match self {
            SeatView::Front | SeatView::Back | SeatView::SideSeated { .. } => {
                (back_couch_anchor(stand, sprite_w), SEATED_SPRITE_H)
            }
            SeatView::Side { .. } | SeatView::Stander { .. } => {
                (waypoint_anchor(stand, sprite_w), UPRIGHT_SPRITE_H)
            }
        }
    }
}

/// The seated [`SeatView`] and stable z-key for the seat whose settle foot-cell
/// is `cell`, or `None` if `cell` is not a seat foot-cell. The caller passes the
/// glide's `to` (settling ONTO a seat) and/or `from` (rising OFF it) — either
/// endpoint on a foot-cell means the agent is on the sit arc and must render in
/// the seat's view and z-key, not the travel-direction / foot-position values.
///
/// Covers the home desk too: `layout.home_desks` are NOT waypoints, but the
/// chair is a settle target once the desk's arrival glides onto it.
pub(super) fn settle_seat_view(cell: Point, layout: &Layout) -> Option<(SeatView, u16)> {
    use crate::layout::seated_foot_cell;
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
            // The desk arm reads the desk's OWN facing rather than
            // `seated_foot_cell`'s viewer-facing default: a back-turned desk
            // seats its occupant a different distance south, and matching on the
            // default cell simply never recognised those chairs — the settle
            // silently lost its seat view. The z-key follows the same anchor, so
            // it cannot disagree with where the sprite lands.
            layout.home_desks.iter().enumerate().find_map(|(i, &desk)| {
                let facing = layout.desk_facing(FloorLocalDeskIndex(i));
                (crate::layout::desk_walk_anchor_facing(desk, facing) == cell).then(|| {
                    let view = if facing == crate::layout::Facing::North {
                        SeatView::Back
                    } else {
                        SeatView::Front
                    };
                    (view, cell.y.saturating_sub(SEAT_Z_LIFT))
                })
            })
        })
}

/// How far ABOVE its chair cell a desk sitter's z-key sits.
///
/// Derived from the chair rather than the desk so it moves with the seat: a
/// viewer-facing chair at `desk.y + 4` keeps the historical `desk.y + 4` key,
/// below the desk furniture's `desk.y + 7`, so that sitter and its sit-down
/// glide sort behind the monitor. A back-turned chair is further south and its
/// key follows, which is what puts that occupant IN FRONT of their desk.
pub(super) const SEAT_Z_LIFT: u16 = 0;
