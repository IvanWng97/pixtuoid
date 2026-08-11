//! The seat model + seated/standing character painting. [`Seat`] (kind × cell ×
//! facing) is the single source of truth for an occupant of ANY seat — waypoint
//! couch, sofa, meeting chair, island stool, or a home desk: sprite + flip,
//! render anchor, z-key and sit-down glide all derive from it. [`SeatView`] is
//! the LOOK it resolves to, not the authority. `paint_character_at` is the
//! shared recolor-blit.

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

/// The still back view — also the fallback for a pose that has none of its own,
/// so a back-turned sitter keeps their back to the camera whenever the pack has
/// this one.
const SEATED_BACK: &str = "seated_back";

/// A table, not `format!("{base}_back")`, because sprite names are `&'static str`.
const SEATED_BACK_VIEWS: &[(&str, &str)] = &[("seated", SEATED_BACK), ("typing", "typing_back")];

/// Which VIEW of a character a seat shows. ONLY the look: what they sit ON
/// decides the art and the geometry ([`Seat`]), not this.
///
/// The ONE source BOTH the seated render and the sit-down WALK glide derive
/// from. Deriving the glide facing from the travel direction instead is the
/// recurring "sit facing the wrong way then snap" bug: a window-facing (`North`)
/// seat is approached from the north but its foot-cell is pinned SOUTH, so the
/// settle travels south, renders a FRONT walk, and the agent sits facing the
/// camera for ~1s before snapping to `back_couch`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeatView {
    /// Faces the camera (south).
    Front,
    /// Faces away — the window / the back wall.
    Back,
    /// Faces sideways; `flip` mirrors east↔west.
    Side { flip: bool },
}

/// What an agent is sitting ON. The home desk is not a `WaypointKind` — it is
/// one agent's own seat, not a shared destination — so the two meet here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeatKind {
    Waypoint(crate::layout::WaypointKind),
    HomeDesk,
}

/// A seat: WHAT you sit on, WHERE, and which way you LOOK. Everything a sitter
/// needs — sprite, flip, render anchor, z-key, sit-down glide — derives from
/// these three, so a couch, a meeting chair and a home desk are one thing.
///
/// Extend [`view`](Self::view) and [`sprite_for`](Self::sprite_for) to add a
/// seatable furniture — both list their kinds EXPLICITLY, so a new
/// `WaypointKind` is a compile error there. The GEOMETRY pair reads
/// [`seated_furniture`](Self::seated_furniture) instead, which defaults a
/// newcomer to the upright anchor and feet-row key without complaint. The net
/// for THAT is `sit_arc_z_key_is_stable_and_on_the_right_side_of_its_furniture`,
/// a per-kind z-key oracle that stops on a kind it does not name — and it sees
/// a newcomer only if the furniture is `occupies_pos` and ONE 192x158 layout
/// places it, not the whole sweep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Seat {
    kind: SeatKind,
    /// The cell the SPRITE renders on — a waypoint's resolved stand cell, a
    /// desk's walk anchor. NOT the settle foot-cell `settle_seat` matches on,
    /// which for a couch/sofa/chair is `WALKING_Y_OFF - SEAT_RENDER_Y_OFF`
    /// further south; constructing a `Seat` from that shifts anchor and z-key.
    pos: Point,
    /// Which way the SITTER looks, decoupled from the side they approached from.
    facing: crate::layout::Facing,
}

impl Seat {
    pub(super) fn at_waypoint(
        kind: crate::layout::WaypointKind,
        pos: Point,
        facing: crate::layout::Facing,
    ) -> Self {
        Seat {
            kind: SeatKind::Waypoint(kind),
            pos,
            facing,
        }
    }

    /// The seat at a home `desk`. Its cell is the desk's walk anchor — the ONE
    /// value the chair sprite, its occupant and the walk that ends there share.
    pub(super) fn at_desk(desk: Point, facing: crate::layout::Facing) -> Self {
        Seat {
            kind: SeatKind::HomeDesk,
            pos: crate::layout::desk_walk_anchor_facing(desk, facing),
            facing,
        }
    }

    /// Whether this seat's occupant sits at SEAT height. Read by both geometry
    /// arms so they cannot disagree — a seat-height anchor paired with a
    /// feet-row z-key sorts a sitter through their own furniture. The meeting
    /// chair qualifies because its profile sprite shares the front `seated`
    /// sprite's 8x10 bottom-row geometry.
    fn seated_furniture(self) -> bool {
        use crate::layout::WaypointKind;
        matches!(
            self.kind,
            SeatKind::Waypoint(
                WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingChair
            )
        )
    }

    fn view(self) -> SeatView {
        use crate::layout::{Facing, WaypointKind};
        match self.kind {
            // A seat with only two views: any facing but North reads as front.
            SeatKind::HomeDesk
            | SeatKind::Waypoint(WaypointKind::Couch | WaypointKind::MeetingSofa) => {
                match self.facing {
                    Facing::North => SeatView::Back,
                    _ => SeatView::Front,
                }
            }
            // The base `side_seated` sprite faces East (the west chair's view),
            // so the east chair mirrors. The island's stander flips on the
            // OPPOSITE facing; both directions are pinned, don't align them.
            SeatKind::Waypoint(WaypointKind::MeetingChair) => SeatView::Side {
                flip: matches!(self.facing, Facing::West),
            },
            SeatKind::Waypoint(WaypointKind::Island) => SeatView::Side {
                flip: matches!(self.facing, Facing::East),
            },
            // Not seats — the agent stands AT these.
            SeatKind::Waypoint(
                WaypointKind::Pantry
                | WaypointKind::PhoneBooth
                | WaypointKind::StandingDesk
                | WaypointKind::VendingMachine
                | WaypointKind::Printer
                | WaypointKind::SnackShelf,
            ) => SeatView::Front,
        }
    }

    /// Sprite + horizontal flip, where `base` is the occupant's own FRONT-view
    /// animation. A waypoint occupant always has the one (`seated`); a desk
    /// occupant's is their pose's (`typing`, `seated_sleeping`, …), and the view
    /// only decides which side of it shows. The furniture with art of its own —
    /// the couch's `back_couch`, the chair's profile, an upright stander —
    /// answers with that and ignores `base`; nobody types on a sofa.
    pub(super) fn sprite_for(self, base: &'static str) -> (&'static str, bool) {
        use crate::layout::WaypointKind;
        let view = self.view();
        match self.kind {
            SeatKind::HomeDesk => match view {
                SeatView::Back => (
                    SEATED_BACK_VIEWS
                        .iter()
                        .find(|(front, _)| *front == base)
                        .map_or(SEATED_BACK, |&(_, back)| back),
                    false,
                ),
                _ => (base, false),
            },
            SeatKind::Waypoint(WaypointKind::Couch | WaypointKind::MeetingSofa) => match view {
                SeatView::Back => ("back_couch", false),
                _ => (base, false),
            },
            SeatKind::Waypoint(WaypointKind::MeetingChair) => {
                ("side_seated", matches!(view, SeatView::Side { flip: true }))
            }
            // You leave the counter holding what you came for.
            SeatKind::Waypoint(WaypointKind::Pantry) => ("holding_coffee", false),
            // The island's bartender and the stand-beside appliances.
            SeatKind::Waypoint(
                WaypointKind::Island
                | WaypointKind::PhoneBooth
                | WaypointKind::StandingDesk
                | WaypointKind::VendingMachine
                | WaypointKind::Printer
                | WaypointKind::SnackShelf,
            ) => ("standing", matches!(view, SeatView::Side { flip: true })),
        }
    }

    /// [`sprite_for`](Self::sprite_for) resolved against a PACK. Character
    /// animations are never inherited from the embedded default (`merge_from` is
    /// furniture-only), so a pre-`side_seated` custom pack degrades to the front
    /// pose — a missing animation must never mean an invisible sitter. Two kinds
    /// of sitter reach this ladder that did not: an UPRIGHT kind used to bypass
    /// the pack check entirely and painted NOTHING when its art was missing, and
    /// a back-turned couch lacking `back_couch` now falls to `seated_back` when
    /// the pack HAS it, rather than to a face at the window.
    pub(super) fn sprite_in_pack(self, base: &'static str, pack: &Pack) -> (&'static str, bool) {
        let (anim, flip) = self.sprite_for(base);
        if pack.animation(anim).is_some() {
            return (anim, flip);
        }
        // A pose whose OWN back view the pack lacks still hides its face if the
        // still one is there.
        if self.view() == SeatView::Back && pack.animation(SEATED_BACK).is_some() {
            return (SEATED_BACK, false);
        }
        (base, false)
    }

    /// `(going_back, flip)` for the sit-down WALK glide that settles onto this
    /// seat — the SAME orientation as [`sprite_for`](Self::sprite_for),
    /// overriding the travel-direction rule for this terminal segment.
    pub(super) fn settle_walk(self) -> (bool, bool) {
        match self.view() {
            SeatView::Front => (false, false),
            SeatView::Back => (true, false),
            SeatView::Side { flip } => (false, flip),
        }
    }

    /// The y-sort key for this seat's occupant — used BOTH for the settled
    /// `AtWaypoint` render AND for the sit-down / stand-up WALK glide. Letting
    /// the glide keep its natural foot z-key instead makes it cross the
    /// furniture's own key on the way down: the agent pops in front of the sofa
    /// mid-glide, then jumps behind it.
    pub(super) fn z_key(self) -> u16 {
        if self.seated_furniture() {
            // Behind a couch/sofa back (which sorts at pos+3) or tied with a
            // front sofa, where insertion order puts the sitter on top.
            return self.pos.y + 2;
        }
        // The plain feet row, which for the bartender sits INSIDE the island
        // body — so the whole arc stays behind the counter.
        self.pos.y
    }

    /// The render ANCHOR-BASE. The ONE authority BOTH the sprite blit
    /// (`sim::resolve_characters`) AND its label twin
    /// (`anchors::character_anchor`) derive the anchor from, so the badge can
    /// never float above the sitter.
    pub(super) fn render_anchor(self, sprite_w: u16) -> Point {
        if self.seated_furniture() {
            back_couch_anchor(self.pos, sprite_w)
        } else {
            // The desk sitter keeps the UPRIGHT anchor: their seat cell IS the
            // walk anchor the arrival settles on, so sprite and walk share it.
            waypoint_anchor(self.pos, sprite_w)
        }
    }
}

/// The [`Seat`] whose settle foot-cell is `cell`, or `None` if `cell` is not
/// one. The caller passes the glide's `to` (settling ONTO a seat) and/or `from`
/// (rising OFF it) — either endpoint on a foot-cell means the agent is on the
/// sit arc and must render in the seat's view and z-key, not the
/// travel-direction / foot-position values.
///
/// Covers the home desk too: `layout.home_desks` are NOT waypoints, but the
/// chair is a settle target once the desk's arrival glides onto it.
pub(super) fn settle_seat(cell: Point, layout: &Layout) -> Option<Seat> {
    use crate::layout::seated_foot_cell;
    layout
        .waypoints
        .iter()
        .find_map(|w| {
            (seated_foot_cell(w.kind.furniture(), w.pos) == Some(cell))
                .then(|| Seat::at_waypoint(w.kind, w.pos, w.facing))
        })
        .or_else(|| {
            layout.home_desks.iter().enumerate().find_map(|(i, &desk)| {
                let seat = Seat::at_desk(desk, layout.desk_facing(FloorLocalDeskIndex(i)));
                (seat.pos == cell).then_some(seat)
            })
        })
}
