//! The seat model + seated/standing character painting. [`Seat`] (kind × cell ×
//! facing) is the single source of truth for an occupant of ANY seat — waypoint
//! couch, sofa, meeting chair, island stool, or a home desk: sprite + flip,
//! render anchor, z-key and sit-down glide all derive from it. [`SeatView`] is
//! the LOOK it resolves to, not the authority. `character_frame` is the
//! per-agent recolor both profiles draw through.

use super::*;

use crate::sim::anchors::{back_couch_anchor, waypoint_anchor};
use pixtuoid_core::state::FloorLocalDeskIndex;

/// Which image of a character's art to draw: the part of its
/// [`FrameKey`](crate::frame_cache::FrameKey) the sim's placement decides; the
/// agent, its burn tier and the scale's density key the rest. A request, not
/// the cache's key, which is published and keys the image it made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpritePose {
    pub(crate) anim_name: &'static str,
    pub(crate) frame_idx: usize,
    /// Which way the character FACES: the art mirrored horizontally.
    pub(crate) flip_x: bool,
    /// Blended into the skin by [`agent_overrides`],
    /// so a row of typing agents shows their tools at a glance; `None` unless
    /// typing or thinking at a desk.
    pub(crate) glow_tint: Option<Rgb>,
}

impl SpritePose {
    /// The pose `placement` draws `agent` in under `theme`. The ONE resolution
    /// of the sim's theme-free [`CharacterGlow`], shared by both profiles: were
    /// each to resolve it, the same agent would glow differently in the two for
    /// no reason anyone chose.
    pub(crate) fn of(
        placement: &crate::sim::CharacterPlacement,
        agent: &AgentSlot,
        theme: &crate::theme::Theme,
    ) -> Self {
        Self {
            anim_name: placement.anim_name,
            frame_idx: placement.frame_idx,
            flip_x: placement.flip_x,
            glow_tint: match placement.glow {
                CharacterGlow::None => None,
                CharacterGlow::Thinking => Some(theme.tool_glow.default),
                CharacterGlow::Tool => super::palette::tool_glow_tint(agent, &theme.tool_glow),
            },
        }
    }
}

/// The per-agent RECOLORED sprite for one character, and where the art marks a
/// head, DRESSED, from the cache.
///
/// Split out of [`paint_character_at`] so a second profile gets the identical
/// palette without a second copy of the rule. The ART and the BLIT differ
/// between profiles — the art is [`densest_frame`]'s at
/// `scale`, so the classic pass (at `RenderScale::ONE`) draws the base sprite
/// 1:1 and the cutaway the densest variant its scale lands — and a per-agent
/// palette is exactly the thing that must NOT differ: hair, skin and the
/// cwd-keyed outfit are how a viewer tells two agents apart, so an agent who is
/// auburn in one profile and default-brown in the other is two different
/// people to the eye.
pub(crate) fn character_frame<'c>(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
    now: SystemTime,
) -> Option<CharacterFrame<'c>> {
    let dense = super::densest_frame(pack, pose.anim_name, pose.frame_idx, scale)?;
    let key = character_key_at(&dense, pack, pose, agent, now);
    Some(recolor(dense, &key, pack, cache))
}

/// [`character_frame`] for a [`CharacterKey`] resolved at `scale`.
pub(crate) fn keyed_character_frame<'c>(
    key: &CharacterKey,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
) -> Option<CharacterFrame<'c>> {
    let dense = super::densest_frame(pack, key.frame.anim_name, key.frame.frame_idx, scale)?;
    Some(recolor(dense, key, pack, cache))
}

fn recolor<'c>(
    dense: super::dense::DenseFrame<'_>,
    key: &CharacterKey,
    pack: &Pack,
    cache: &'c mut FrameCache,
) -> CharacterFrame<'c> {
    let flip_x = key.frame.flip_x;
    // A cwd backfill re-keys the outfit (Team Palette) mid-lifetime — flag the
    // change so the cache drops the agent's stale recolors before the lookup.
    cache.note_outfit_seed(key.frame.agent_id, key.outfit);
    let frame = cache.get_or_make(key.frame.clone(), || {
        let bare = dense.recolorable.recolored(&key.palette);
        // Dressed before the facing flip: a style's layers are drawn for the
        // art as authored. The agent id and the art's density pick the style,
        // and the cache's key carries both.
        let recolored = match &key.dress {
            Some(dress) => super::hair::dress(
                &bare,
                dress,
                dress
                    .style
                    .as_deref()
                    .and_then(|name| pack.hairstyle(name, dense.density)),
                &key.palette,
                pack.character_outline(),
            ),
            None => bare,
        };
        if flip_x {
            recolored.mirror_horizontal()
        } else {
            recolored
        }
    });
    let rise = key.dress.as_ref().map_or(0, |d| d.rise);
    CharacterFrame {
        frame,
        blit_at: dense.blit_at,
        rise,
    }
}

/// Every input [`character_frame`] recolors from: the cache's key, the outfit
/// seed the cache drops an agent's entries on, and the palette itself. Within
/// one pack, two equal keys recolor to the same frame.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CharacterKey {
    pub(crate) frame: crate::frame_cache::FrameKey,
    pub(crate) outfit: u64,
    /// The agent's colours, resolved: the recolor reads nothing of the agent
    /// beyond what this and `frame` carry.
    palette: [(char, pixtuoid_core::sprite::Pixel); 4],
    /// How the frame is dressed, resolved once for every reader: the recolor
    /// and the cutaway's figure box.
    pub(crate) dress: Option<super::hair::Dress>,
}

/// [`character_frame`]'s [`CharacterKey`], without the recolor; `None` where it
/// draws nothing.
pub(crate) fn character_key(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    now: SystemTime,
) -> Option<CharacterKey> {
    let dense = super::densest_frame(pack, pose.anim_name, pose.frame_idx, scale)?;
    Some(character_key_at(&dense, pack, pose, agent, now))
}

fn character_key_at(
    dense: &super::dense::DenseFrame<'_>,
    pack: &Pack,
    pose: SpritePose,
    agent: &AgentSlot,
    now: SystemTime,
) -> CharacterKey {
    let SpritePose {
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
    } = pose;
    let burn = crate::burn::slot_burn_tier(agent, now);
    let density = dense.density;
    CharacterKey {
        frame: crate::frame_cache::FrameKey {
            agent_id: agent.agent_id,
            anim_name,
            frame_idx,
            flip_x,
            glow_tint,
            burn,
            density,
        },
        outfit: outfit_seed_for(agent),
        palette: agent_overrides(agent, glow_tint, burn),
        dress: super::hair::dress_for(pack, agent.agent_id, dense.frame, dense.head, density),
    }
}

/// A recolored character frame and how to draw it at the scale it was picked
/// for; `blit_at` as in [`DenseFrame`].
pub(crate) struct CharacterFrame<'c> {
    pub(crate) frame: &'c Frame,
    pub(crate) blit_at: std::num::NonZeroU16,
    /// Art rows the frame reaches above its logical top: its
    /// [`Dress::rise`](super::hair::Dress::rise).
    pub(crate) rise: u16,
}

/// Paint a character at an arbitrary anchor with per-agent recolor, returning
/// the size of the frame it drew.
pub(crate) fn paint_character_at(
    buf: &mut RgbBuffer,
    pose: SpritePose,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
) -> Option<Size> {
    let CharacterFrame {
        frame: cached,
        blit_at: _,
        rise: _,
    } = character_frame(
        pose,
        agent,
        pack,
        crate::render_scale::RenderScale::ONE,
        cache,
        now,
    )?;
    let size = Size {
        w: cached.width(),
        h: cached.height(),
    };
    blit_frame(cached, anchor.x, anchor.y, buf);
    Some(size)
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
/// camera for the whole settle before snapping to `back_couch`.
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
/// a newcomer only if the furniture is `occupies_pos` and the ONE layout it
/// renders places it, not the whole sweep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Seat {
    kind: SeatKind,
    /// The cell the SPRITE renders on — a waypoint's resolved stand cell, a
    /// desk's walk anchor. NOT the settle foot-cell `settle_seat` matches on,
    /// which for a couch/sofa/chair is `WALKING_Y_OFF - SEAT_RENDER_Y_OFF`
    /// further south; constructing a `Seat` from that shifts anchor and z-key.
    pos: Point,
    /// Which way the SITTER looks, decoupled from the side they approached from.
    facing: crate::layout::Facing,
}

/// The depth the sitters of a meeting sofa at `sofa` sort at: their seat's own
/// [`z_key`](Seat::z_key). The cutaway keys the sofa by it, so a front sofa,
/// queued before its sitters, ties them and they sit on it.
pub(crate) fn sofa_sitter_z_key(sofa: Point) -> u16 {
    Seat::at_waypoint(
        crate::layout::WaypointKind::MeetingSofa,
        sofa,
        crate::layout::Facing::South,
    )
    .z_key()
}

impl Seat {
    pub(crate) fn at_waypoint(
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
    pub(crate) fn at_desk(desk: Point, facing: crate::layout::Facing) -> Self {
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
    /// sprite's bottom-row geometry.
    pub(crate) fn seated_furniture(self) -> bool {
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
    pub(crate) fn sprite_for(self, base: &'static str) -> (&'static str, bool) {
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
    /// pose — a missing animation must never mean an invisible sitter. An
    /// UPRIGHT kind goes through it too, so missing art degrades instead of
    /// painting nothing; and a back-turned couch lacking `back_couch` falls to
    /// `seated_back` when the pack HAS it, rather than to a face at the window.
    pub(crate) fn sprite_in_pack(self, base: &'static str, pack: &Pack) -> (&'static str, bool) {
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
    pub(crate) fn settle_walk(self) -> (bool, bool) {
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
    pub(crate) fn z_key(self) -> u16 {
        if self.seated_furniture() {
            return crate::layout::seated_z_key(self.pos);
        }
        // The plain feet row, which for the bartender sits INSIDE the island
        // body — so the whole arc stays behind the counter.
        self.pos.y
    }

    /// The render ANCHOR-BASE, which `sim::resolve_characters` places the sprite
    /// and its badge from.
    pub(crate) fn render_anchor(self, sprite_w: u16) -> Point {
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
pub(crate) fn settle_seat(cell: Point, layout: &Layout) -> Option<Seat> {
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
