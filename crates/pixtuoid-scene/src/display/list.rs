//! The display list's types: what one frame shows, piece by piece.

use pixtuoid_core::sprite::format::Pack;

use super::Span;
use crate::atmosphere::Carpet;
use crate::dither::Dithered;
use crate::layout::{Bounds, Point};
use crate::outside::WindowView;
use crate::render_scale::RenderScale;
use crate::theme::Theme;

/// How many tones a standby screen's glass steps through as the room darkens:
/// a stepped glow, where a blend would put a colour of its own on every level.
const STANDBY_STOPS: u8 = 2;
/// Ramp levels a standby glass sits under the theme's idle tint for each stop it
/// is short of one past the last, so even the brightest standby stays a step
/// under the tint: the tint itself on the glass reads as a screen switched on.
const STANDBY_LEVEL_PER_STOP: i8 = -2;

/// What a desk's screen shows. A screen is its own light: whatever the room's
/// lights do, they leave its glass alone ([`paint_list`](crate::cutaway::paint::paint_list)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Screen {
    /// Dark glass.
    Off,
    /// The standby glow of an idle screen at night, its glass in this colour.
    Standby(pixtuoid_core::sprite::Rgb),
    /// Lit by its occupant's tool, in this glow, its scanline on glass column
    /// `scan` ([`crate::sim::scanline_col`]).
    Lit {
        glow: pixtuoid_core::sprite::Rgb,
        scan: u16,
    },
}

impl Screen {
    /// A desk's screen: lit by `glow` with its scanline on `scan`, else
    /// standing by at `idle` ([`crate::lighting::screen_idle`]) in `theme`'s
    /// idle tint, else dark.
    pub(super) fn of(
        glow: Option<pixtuoid_core::sprite::Rgb>,
        scan: u16,
        idle: f32,
        theme: &Theme,
    ) -> Self {
        if let Some(glow) = glow {
            return Self::Lit { glow, scan };
        }
        let share = idle / crate::lighting::SCREEN_IDLE_MAX;
        let stops = (share.clamp(0.0, 1.0) * f32::from(STANDBY_STOPS)).round() as u8;
        if stops == 0 {
            return Self::Off;
        }
        let lacking = (STANDBY_STOPS + 1 - stops) as i8;
        Self::Standby(
            theme
                .effects
                .monitor_idle
                .ramp(lacking * STANDBY_LEVEL_PER_STOP),
        )
    }
}

/// One agent's name badge, painted in the canvas so no terminal text shares a
/// cell with the image: `overlay`'s text and
/// [`BadgeInk`](crate::overlay::BadgeInk) on its
/// [`badge_plate`](crate::overlay::badge_plate). Hung from the CUTAWAY's body:
/// `overlay::build_overlay`'s anchors hang off the classic-drawn sprite, which
/// for a sitter is elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Badge {
    /// Its bottom centre, in logical units: the sprite's centre, clear above
    /// its head and any raised monitor behind it.
    pub(crate) at: Point,
    pub(crate) text: String,
    pub(crate) tone: crate::overlay::LabelTone,
}

/// One frame's pieces — the windows, the decor hung on the wall and
/// everything standing on the ground — built and ordered but not painted.
///
/// ONE ordered list, so a character and the desk it sits at resolve against each
/// other by depth: that order IS the occlusion, and there is no second
/// occlusion pass.
pub(crate) struct DisplayList<'a> {
    pub(super) pieces: Vec<Piece>,
    /// The room's own lights, which paint over every piece at once
    /// ([`paint_list`](crate::cutaway::paint::paint_list)).
    pub(super) lights: Vec<LightPiece>,
    /// How dark the room is: every non-emissive pixel is painted under it, so a
    /// change repaints the whole frame.
    pub(super) ambient: crate::cutaway::light::Ambient,
    /// The carpet the backdrop lays: a change repaints the whole frame too.
    pub(super) carpet: Dithered<Carpet>,
    /// How far lightning lifts the room: a change repaints the whole frame.
    pub(super) flash: crate::cutaway::light::Flash,
    // What it was built with, so painting it cannot use anything else: a
    // figure's key names its density, which only the build's scale picks.
    pub(super) pack: &'a Pack,
    pub(super) theme: &'a Theme,
    pub(super) scale: RenderScale,
}

/// One entry of a [`DisplayList`].
pub(crate) struct Piece {
    pub(crate) span: Span,
    pub(crate) kind: PieceKind,
    /// The shadow it casts on the ground
    /// ([`ground_shadow`](crate::display::compose::ground_shadow)). Every
    /// shadow is painted before any piece, so it lies under them all; it may
    /// reach past `span`, but never past [`reach`](Self::reach).
    pub(crate) shadow: Option<crate::ground::Contact>,
    /// Two pieces with one span and one fingerprint paint the same pixels over
    /// the same pixels under them, so a caller can keep a piece whose pair did
    /// not change without painting it. One that
    /// [`reads_under`](PieceKind::reads_under) changes with what lies under it,
    /// so repainting it starts from a repaint of that. Compare it only within
    /// one layout, theme, pack, scale and ambient: it does not capture a change
    /// to those.
    pub(crate) fingerprint: u64,
}

/// One of the room's lights: what it lifts, over which cells. A change repaints
/// its span from every light that meets it, which alone light a rect as the
/// frame does ([`net_pass`](crate::cutaway::light::net_pass)).
pub(crate) struct LightPiece {
    pub(crate) span: Span,
    pub(crate) view: crate::cutaway::light::LightView,
    pub(crate) fingerprint: u64,
}

impl Piece {
    /// Every logical cell painting it touches: its span, and the ground its
    /// shadow falls on. A repaint of a damaged rect is complete only over the
    /// pieces whose reach meets it.
    pub(crate) fn reach(&self) -> Span {
        let Some(((x0, y0), (x1, y1))) = self.shadow.map(|c| c.bounds()) else {
            return self.span;
        };
        Span {
            x0: self.span.x0.min(x0),
            x1: self.span.x1.max(x1.saturating_sub(1)),
            y0: self.span.y0.min(y0),
            y1: self.span.y1.max(y1.saturating_sub(1)),
            ..self.span
        }
    }
}

impl<'a> DisplayList<'a> {
    /// The pieces, back to front.
    pub(crate) fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// The pieces, for a rasterizer test to add or drop one.
    #[cfg(test)]
    pub(crate) fn pieces_mut(&mut self) -> &mut Vec<Piece> {
        &mut self.pieces
    }

    /// The room's lights, in no order that matters: a pixel's light is the
    /// brightest's, ties to the lowest rank.
    pub(crate) fn lights(&self) -> &[LightPiece] {
        &self.lights
    }

    pub(crate) fn ambient(&self) -> crate::cutaway::light::Ambient {
        self.ambient
    }

    pub(crate) fn carpet(&self) -> Dithered<Carpet> {
        self.carpet
    }

    pub(crate) fn flash(&self) -> crate::cutaway::light::Flash {
        self.flash
    }

    pub(crate) fn pack(&self) -> &'a Pack {
        self.pack
    }

    pub(crate) fn theme(&self) -> &'a Theme {
        self.theme
    }

    pub(crate) fn scale(&self) -> RenderScale {
        self.scale
    }

    /// Each drawn agent's badge, in draw order.
    #[cfg(test)]
    pub(crate) fn badges(&self) -> impl Iterator<Item = &Badge> + '_ {
        self.pieces.iter().filter_map(|p| match &p.kind {
            PieceKind::Badge { badge } => Some(badge),
            _ => None,
        })
    }

    /// Each piece's hover box and the agent it shows, in draw order: a
    /// character's `body`, every other piece's span showing none. A badge is
    /// no hover target, as in the classic: neighbours' plates overlap, so one
    /// would claim the body under another's
    /// (`hovering_a_sitter_under_a_neighbours_badge_names_the_sitter`).
    pub(crate) fn hover_spans(
        &self,
    ) -> impl Iterator<Item = (Span, Option<pixtuoid_core::AgentId>)> + '_ {
        self.pieces.iter().filter_map(|p| match &p.kind {
            PieceKind::Character { figure, body, .. } => {
                Some((*body, Some(figure.key.frame.agent_id)))
            }
            PieceKind::Badge { .. } => None,
            _ => Some((p.span, None)),
        })
    }
}

/// A hash of every input `paint_piece` and
/// [`ground_shadow`](crate::display::compose::ground_shadow) draw `kind`
/// from beyond the layout, theme, pack and scale. Each arm destructures every field,
/// so a field added to a kind fails to compile here until it is hashed or named
/// `_`.
pub(crate) fn fingerprint(kind: &PieceKind) -> u64 {
    use std::hash::{Hash, Hasher};
    // `DefaultHasher::new` starts from the same keys on every call, so one frame
    // built twice hashes alike (`one_frame_builds_one_list`); a `RandomState`
    // would not.
    let mut h = std::hash::DefaultHasher::new();
    std::mem::discriminant(kind).hash(&mut h);
    match *kind {
        PieceKind::WallSeg { piece, rows } => (piece, rows).hash(&mut h),
        PieceKind::Desk { at, art, screen } => (at, art, screen).hash(&mut h),
        PieceKind::Chair { at } => at.hash(&mut h),
        PieceKind::DeskProp(prop) => prop.hash(&mut h),
        PieceKind::Creature { at, art, degraded } => (at, art, degraded).hash(&mut h),
        PieceKind::Prop { at, art } | PieceKind::Animated { at, art } => (at, art).hash(&mut h),
        PieceKind::PropBand { at, sprite, rows } => (at, sprite, rows).hash(&mut h),
        PieceKind::Table { at } => at.hash(&mut h),
        PieceKind::Door { at, frame } => (at, frame).hash(&mut h),
        PieceKind::Neon {
            at,
            tube,
            hue,
            interior,
        } => (at, tube, hue, interior).hash(&mut h),
        PieceKind::Clock { at, reading } => (at, reading).hash(&mut h),
        // The body follows from `at` and `key`.
        PieceKind::Character {
            figure:
                Figure {
                    at,
                    shadow,
                    ref key,
                },
            chair,
            body: _,
        } => (at, shadow, key, chair).hash(&mut h),
        PieceKind::Badge { ref badge } => badge.hash(&mut h),
        PieceKind::Board { ref board } => board.hash(&mut h),
        PieceKind::Indicator { door, floor } => (door, floor).hash(&mut h),
        PieceKind::Window { ref view, frame } => (view, frame).hash(&mut h),
        PieceKind::Hung { at, sprite } => (at, sprite).hash(&mut h),
        PieceKind::Effect(riding) => riding.hash(&mut h),
    }
    h.finish()
}

impl PieceKind {
    /// Whether it recolours what lies under it rather than painting colours of
    /// its own: a room wall's glass ([`PieceKind::WallSeg`]), not a window
    /// ([`PieceKind::Window`]).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the incremental canvas repaints under it")
    )]
    pub(crate) fn reads_under(&self) -> bool {
        matches!(self, PieceKind::WallSeg { .. })
    }

    /// Whether two builds of one office always give it one fingerprint: what a
    /// cache of the room at rest may hold. The rest change with the sky, the
    /// hour's lights or the people.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the incremental canvas caches the room at rest")
    )]
    pub(crate) fn is_static(&self) -> bool {
        match self {
            PieceKind::WallSeg { .. }
            | PieceKind::Hung { .. }
            | PieceKind::Chair { .. }
            | PieceKind::Prop { .. }
            | PieceKind::PropBand { .. }
            | PieceKind::Table { .. } => true,
            // Frames that play, the elevator opening, the sign's levels, the time.
            PieceKind::Animated { .. }
            | PieceKind::Door { .. }
            | PieceKind::Neon { .. }
            | PieceKind::Clock { .. }
            | PieceKind::Window { .. }
            | PieceKind::Desk { .. }
            | PieceKind::DeskProp(_)
            | PieceKind::Creature { .. }
            | PieceKind::Character { .. }
            | PieceKind::Effect(_)
            | PieceKind::Badge { .. }
            | PieceKind::Board { .. }
            | PieceKind::Indicator { .. } => false,
        }
    }
}

/// What a [`Piece`] IS; its [`Span`] rides beside it.
///
/// The span is computed WHERE THE PIECE IS BUILT, from the same anchor and box
/// the piece's paint fn draws, plus the rows it stamps under that box — the box
/// depends on the PACK, so a sprite's height is not knowable from its layout
/// point, and geometry derived from anywhere but where the sprite lands can drift
/// from it. `every_piece_paints_only_inside_its_span` pins that every pixel a
/// paint fn writes lies inside its span.
#[derive(Debug)]
pub(crate) enum PieceKind {
    /// One window: its glass, then its joinery in `frame`.
    Window {
        view: WindowView,
        frame: pixtuoid_core::sprite::Rgb,
    },
    /// Decor hung on the north band, blitted at its top-left `at`.
    Hung {
        at: crate::layout::Point,
        sprite: &'static str,
    },
    /// One segment of a room's wall run.
    WallSeg {
        piece: crate::layout::WallPiece,
        /// The logical rows of `piece` this segment paints, top inclusive and
        /// bottom exclusive.
        rows: (u16, u16),
    },
    Desk {
        at: crate::layout::Point,
        /// The facing's art (see [`desk_art`](crate::display::compose::desk_art)).
        art: &'static str,
        screen: Screen,
    },
    Chair {
        at: crate::layout::Point,
    },
    /// Art centred on `at` that holds still.
    Prop {
        at: crate::layout::Point,
        art: Art,
    },
    /// Art centred on `at`, on the frame it shows now.
    Animated {
        at: crate::layout::Point,
        art: Art,
    },
    /// Rows `rows` of a centred prop, top inclusive, bottom exclusive, in
    /// logical rows from the art's top: one band of a piece its sitter sits
    /// between (`push_sofa`).
    PropBand {
        at: crate::layout::Point,
        sprite: &'static str,
        rows: (u16, u16),
    },
    Table {
        at: crate::layout::Point,
    },
    /// The elevator from its top-left `at`, as open as `frame` shows.
    Door {
        at: crate::layout::Point,
        frame: usize,
    },
    /// The neon sign over `at`, in this frame's colours.
    Neon {
        at: Bounds,
        tube: pixtuoid_core::sprite::Rgb,
        hue: pixtuoid_core::sprite::Rgb,
        interior: pixtuoid_core::sprite::Rgb,
    },
    /// The wall clock's dial from its top-left `at`, its hands at `reading`.
    Clock {
        at: crate::layout::Point,
        reading: crate::sky::ClockReading,
    },
    /// A prop the model stands on a desk (`push_desk_props`).
    DeskProp(StoodProp),
    /// The pet or a gateway mascot, its art centred on `at`; a degraded
    /// gateway's greyed (`push_creatures`).
    Creature {
        at: crate::layout::Point,
        art: Art,
        degraded: bool,
    },
    Character {
        figure: Figure,
        /// A back-turned sitter's chair, painted straight after them.
        chair: Option<crate::layout::Point>,
        /// The box their art lands in: the span without their chair. A standing
        /// figure's shadow falls under it.
        body: Span,
    },
    /// An effect riding on the figure pushed beside it.
    Effect(crate::cutaway::effects::Riding),
    Badge {
        badge: Badge,
    },
    /// The wall board's text, over the neon sign's interior.
    Board {
        board: crate::board::BoardModel,
    },
    /// The floor indicator over the elevator at `door`.
    Indicator {
        door: Point,
        floor: usize,
    },
}

/// One desk prop: frame `frame` of `sprite`, its art's top-left at buffer pixel
/// `at`, where the desk art's mark for it stands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StoodProp {
    pub(crate) sprite: &'static str,
    pub(crate) frame: usize,
    pub(crate) at: (u16, u16),
}

/// Which art a prop draws: a sprite's frame, turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Art {
    pub(crate) sprite: &'static str,
    pub(crate) frame: usize,
    pub(crate) flip: Flip,
}

impl Art {
    /// `sprite`'s first frame, as drawn.
    pub(crate) fn still(sprite: &'static str) -> Self {
        Self {
            sprite,
            frame: 0,
            flip: Flip::None,
        }
    }
}

/// How a prop's art is turned: the back-view sofa top to bottom, the far
/// meeting chair side to side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Flip {
    None,
    Vertical,
    Horizontal,
}

/// Everything `paint_figure` draws one figure from, resolved when the list is
/// built.
pub(crate) struct Figure {
    /// The art's top-left, in logical units.
    pub(crate) at: crate::layout::Point,
    /// Grounded by a contact shadow: not sitting on furniture, which grounds a
    /// sitter instead.
    pub(super) shadow: bool,
    pub(crate) key: crate::character::CharacterKey,
}

impl std::fmt::Debug for Figure {
    // `CharacterKey` is not `Debug`; its frame's name and index say which art.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Figure")
            .field("at", &self.at)
            .field("shadow", &self.shadow)
            .field("agent", &self.key.frame.agent_id)
            .field("anim_name", &self.key.frame.anim_name)
            .field("frame_idx", &self.key.frame.frame_idx)
            .finish()
    }
}
