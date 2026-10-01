//! The cutaway profile's paint pass — the second reader of `SimFrame`. Of the
//! sim's effects it draws those riding on people (`cutaway::effects`); steam
//! and the pet stay with the classic pass.
//! It never advances the sim; a mover here would desync the profiles.

use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::blit::blit_frame_scaled;
use pixtuoid_core::sprite::format::Pack;

use crate::atmosphere::Moment;
use crate::cutaway::order::{Span, depth_sort};
use crate::cutaway::pen::{ArtPx, ArtRect, Pen};
use crate::cutaway::shade::{Ramp, fill, slab};
use crate::effects::EffectKind;
use crate::glass_weather::GlassWeather;
use crate::layout::{
    Bounds, DESK_H, Depth, Fixture, FixtureKind, Layer, Layout, Point, Size, Station, Tie,
};
use crate::render_scale::RenderScale;
use crate::sim::SimFrame;
use crate::theme::Theme;

/// The front face the cutaway derives under a top-down piece's base-density art
/// (a desk's, the meeting table's), as a fraction of `DESK_H` so it tracks the
/// desk.
/// Without one there is no thickness and the office reads as a floor plan.
const DESK_FRONT_NUMER: u16 = 2;
/// Denominator of [`DESK_FRONT_NUMER`].
const DESK_FRONT_DENOM: u16 = 5;

/// How far the key light reaches down the room before the ground falls off.
///
/// The windows are the north wall, so the falloff runs north to south; these
/// bound the dithered transition between the lit and base ground tones.
const GROUND_LIT_NUMER: u16 = 1;
/// Denominator of [`GROUND_LIT_NUMER`].
const GROUND_LIT_DENOM: u16 = 3;

/// A carpet tile's side, in logical units: seams on this grid are what turn a
/// flat tone into ground you could walk on.
const GROUND_TILE: u16 = 4;
/// How many ramp stops a seam sits under the ground it crosses.
const SEAM_LEVEL: i8 = -1;
/// The smallest tile, in art pixels, that its seams leave reading as ground;
/// seams on a smaller one turn the ground to plaid.
const MIN_SEAMED_TILE: u16 = 8;

/// A rug's lattice pitch, in art pixels: diamonds this far apart.
const RUG_LATTICE: u16 = 6;
/// How many ramp stops the lattice sits under the rug's field: a pattern woven
/// in, not printed on.
const RUG_MOTIF_LEVEL: i8 = -1;
/// How many ramp stops a rug's fringe sits over its trim: loose threads catch
/// the light the bound edge doesn't.
const RUG_FRINGE_LEVEL: i8 = 2;

/// Logical rows between a head and its name badge.
const LABEL_GAP: u16 = 2;

/// How many ramp levels the lit glass sits below its glow colour.
const SCREEN_GLASS_LEVEL: i8 = -3;

/// How many ramp levels a lit screen's text sits above its glow colour: bright
/// lines on the dark glass.
const SCREEN_TEXT_LEVEL: i8 = 9;

/// How many tones a standby screen's glass steps through as the room darkens:
/// a stepped glow, where a blend would put a colour of its own on every level.
const STANDBY_STOPS: u8 = 2;
/// Ramp levels a standby glass sits under the theme's idle tint for each stop it
/// is short of one past the last, so even the brightest standby stays a step
/// under the tint: the tint itself on the glass reads as a screen switched on.
const STANDBY_LEVEL_PER_STOP: i8 = -2;

/// What a desk's screen shows. A screen is its own light: whatever the room's
/// lights do, they leave its glass alone ([`paint_list`]).
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
    fn of(glow: Option<pixtuoid_core::sprite::Rgb>, scan: u16, idle: f32, theme: &Theme) -> Self {
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

    /// `art` with its screen keys showing this screen.
    fn on(
        self,
        art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    ) -> Option<pixtuoid_core::sprite::Frame> {
        match self {
            Self::Off => None,
            Self::Standby(glass) => {
                Some(art.recolored(&[(crate::embedded_pack::SCREEN_GLASS_KEY, Some(glass))]))
            }
            Self::Lit { glow, scan } => Some(scanline(relight_screen(art, glow), art, glow, scan)),
        }
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

/// Art pixels between a plate's edge and its text.
const PLATE_PAD: u16 = 1;

/// A plate's height on the art grid: at the pack's 4x art, two logical rows,
/// the one terminal cell the classic's text takes.
const PLATE_H: u16 = crate::cutaway::text::LINE_H + 2 * PLATE_PAD;

/// A plate around `text` on the art grid, centred on column `centre`, its top
/// at row `top`.
fn plate_at(centre: ArtPx, top: ArtPx, text: &str) -> ArtRect {
    let w = crate::cutaway::text::width(text)
        .0
        .saturating_add(2 * PLATE_PAD);
    ArtRect {
        x: ArtPx(centre.0.saturating_sub(w / 2)),
        y: top,
        w: ArtPx(w),
        h: ArtPx(PLATE_H),
    }
}

/// The cells of art rect `r`, drawn over everything they meet.
fn topmost_span(r: ArtRect, pen: Pen) -> Span {
    Span {
        x0: pen.logical(r.x),
        x1: pen.logical(ArtPx(r.x.0 + r.w.0 - 1)),
        y0: pen.logical(r.y),
        y1: pen.logical(ArtPx(r.y.0 + r.h.0 - 1)),
        depth: u16::MAX,
        layer: Layer::Over,
    }
}

/// Fill `plate` with `ground`, then paint `runs` one after another inside it.
fn paint_plate(
    pen: Pen,
    buf: &mut RgbBuffer,
    plate: ArtRect,
    ground: pixtuoid_core::sprite::Rgb,
    runs: &[(&str, pixtuoid_core::sprite::Rgb)],
) {
    pen.fill(buf, plate, ground);
    let mut x = plate.x.0 + PLATE_PAD;
    for &(text, ink) in runs {
        crate::cutaway::text::paint(
            pen,
            buf,
            (ArtPx(x), ArtPx(plate.y.0 + PLATE_PAD)),
            text,
            ink,
        );
        x += crate::cutaway::text::advance(text).0;
    }
}

impl Badge {
    fn plate(&self, pen: Pen) -> ArtRect {
        plate_at(
            pen.art(self.at.x),
            ArtPx(pen.art(self.at.y).0.saturating_sub(PLATE_H)),
            &format!("{}{}", crate::overlay::BADGE_MARKER, self.text),
        )
    }
}

/// Paint `badge`'s plate, marker and text.
fn paint_badge(badge: &Badge, theme: &Theme, pen: Pen, buf: &mut RgbBuffer) {
    let ink = crate::overlay::badge_ink(&badge.text, badge.tone, theme);
    let marker = crate::overlay::BADGE_MARKER.to_string();
    let runs = [
        (marker.as_str(), ink.marker),
        (badge.text.as_str(), ink.name),
    ];
    paint_plate(
        pen,
        buf,
        badge.plate(pen),
        crate::overlay::badge_plate(theme),
        &runs,
    );
}

/// The floor indicator over the elevator at `door`, naming floor `floor`: a
/// plate over the cell the classic writes it across
/// ([`floor_indicator_rows`](crate::layout::floor_indicator_rows)).
fn indicator_plate(door: Point, floor: usize, pen: Pen) -> ArtRect {
    plate_at(
        pen.art(door.x + crate::layout::ELEVATOR_W / 2),
        pen.art(crate::layout::floor_indicator_rows(door.y).start),
        &crate::layout::floor_indicator_text(floor),
    )
}

/// Each run of `board` and its top-left on the art grid, as the classic's
/// terminal board lays it: line `i` on the neon interior's `i`th cell row,
/// one character a column, the star flush right.
fn board_runs(
    board: &crate::board::BoardModel,
    pen: Pen,
) -> Vec<((ArtPx, ArtPx), &crate::board::BoardSegment)> {
    use crate::layout::{CELL_ROWS, NEON_PANEL_INNER_W, NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y};
    let pad = pen
        .art(CELL_ROWS)
        .0
        .saturating_sub(crate::cutaway::text::LINE_H)
        / 2;
    let at = |col: u16, line: u16| {
        (
            ArtPx(pen.art(NEON_PANEL_INNER_X).0 + crate::cutaway::text::columns(col).0),
            ArtPx(pen.art(NEON_PANEL_INNER_Y + line * CELL_ROWS).0 + pad),
        )
    };
    let cols = |s: &crate::board::BoardSegment| crate::cutaway::text::cells(&s.text);
    let mut runs = vec![
        (at(0, 0), &board.brand),
        (
            at(NEON_PANEL_INNER_W.saturating_sub(cols(&board.star)), 0),
            &board.star,
        ),
    ];
    for (line, segs) in (1..).zip([&board.mood, &board.context]) {
        let mut col = 0;
        for seg in segs {
            runs.push((at(col, line), seg));
            col += cols(seg);
        }
    }
    runs
}

/// The cells `board`'s text covers.
fn board_span(board: &crate::board::BoardModel, pen: Pen) -> Span {
    let runs = board_runs(board, pen);
    let x1 = runs
        .iter()
        .map(|((x, _), s)| x.0 + crate::cutaway::text::width(&s.text).0)
        .max()
        .unwrap_or(0);
    let y1 = runs
        .iter()
        .map(|((_, y), _)| y.0 + crate::cutaway::text::LINE_H)
        .max()
        .unwrap_or(0);
    let (x0, y0) = runs.first().map_or((ArtPx(0), ArtPx(0)), |&(at, _)| at);
    topmost_span(
        ArtRect {
            x: x0,
            y: y0,
            w: ArtPx(x1.saturating_sub(x0.0).max(1)),
            h: ArtPx(y1.saturating_sub(y0.0).max(1)),
        },
        pen,
    )
}

/// What a cutaway frame is drawn with and the next one is too: the office
/// itself, where the sky's look, `altitude` and `now` are what move from frame
/// to frame.
#[derive(Clone, Copy)]
pub struct Office<'a> {
    /// Where everything stands, in LOGICAL units.
    pub layout: &'a Layout,
    /// The art that draws it.
    pub pack: &'a Pack,
    /// Its colours.
    pub theme: &'a Theme,
    /// Buffer pixels per logical unit.
    pub scale: RenderScale,
}

/// Which floor a frame shows, when, and what its wall board says: what moves
/// a frame beyond its office and the sim's world.
#[derive(Clone, Copy)]
pub struct Showing<'a> {
    /// The floor of the building it shows.
    pub floor: crate::floor::FloorMeta,
    /// The wall-clock instant: the sky, the room's light, the board's flap.
    pub now: std::time::SystemTime,
    /// The wall board, the classic painter's
    /// ([`build_board`](crate::board::build_board)).
    pub board: &'a crate::board::BoardModel,
}

/// Paint `frame`'s `office` into `buf` as an orthographic cutaway — the
/// classic painter's sibling, not its successor — as `showing` says: its
/// windows on the sky from its floor's altitude, its room lit for the hour.
pub fn render_cutaway(
    frame: &SimFrame,
    office: Office<'_>,
    showing: Showing<'_>,
    cache: &mut CutawayCache,
    buf: &mut RgbBuffer,
) {
    let list = frame_list(frame, office, showing);
    paint(office.layout, &list, cache, buf);
}

/// `frame`'s [`DrawList`] as `showing` says.
pub(crate) fn frame_list<'a>(
    frame: &SimFrame,
    office: Office<'a>,
    Showing { floor, now, board }: Showing<'_>,
) -> DrawList<'a> {
    let moment = Moment::resolve(
        crate::sky::Sky::at(now, floor.weather),
        office.theme,
        floor.altitude,
        now,
    );
    build_list(frame, office, &moment, floor.floor_idx, board)
}

/// Paint `list` whole: `layout`'s backdrop, then the list over it.
pub(crate) fn paint(
    layout: &Layout,
    list: &DrawList<'_>,
    cache: &mut CutawayCache,
    buf: &mut RgbBuffer,
) {
    let pen = Pen::for_pack(list.scale, list.pack);
    paint_backdrop(layout, list.theme, list.scale, pen, buf);
    paint_list(list, cache, buf);
}

/// Everything under the list's pieces, none of which moves within a layout,
/// theme, pack and scale.
fn paint_backdrop(
    layout: &Layout,
    theme: &Theme,
    scale: RenderScale,
    pen: Pen,
    buf: &mut RgbBuffer,
) {
    paint_ground(layout, theme, pen, buf);
    for fixture in layout.fixtures() {
        match covering(fixture.kind) {
            Some(Covering::Rug) => paint_rug(fixture.visual, theme, pen, buf),
            Some(Covering::Runner) => paint_runner(fixture.visual, theme, pen, buf),
            None => {}
        }
    }
    paint_wall(layout, theme, scale, pen, buf);
}

/// One frame's pieces — the windows' glass, the decor hung on the wall and
/// everything standing on the ground — built and ordered but not painted.
///
/// ONE ordered list, so a character and the desk it sits at resolve against each
/// other by depth: that order IS the occlusion, and there is no second
/// occlusion pass.
pub(crate) struct DrawList<'a> {
    pieces: Vec<Piece>,
    /// The room's own lights, which paint over every piece at once
    /// ([`paint_list`]).
    lights: Vec<LightPiece>,
    /// How dark the room is: every non-emissive pixel is painted under it, so a
    /// change repaints the whole frame.
    ambient: crate::cutaway::light::Ambient,
    // What it was built with, so painting it cannot use anything else: a
    // figure's key names its density, which only the build's scale picks.
    pack: &'a Pack,
    theme: &'a Theme,
    scale: RenderScale,
}

/// One entry of a [`DrawList`].
pub(crate) struct Piece {
    pub(crate) span: Span,
    pub(crate) kind: PieceKind,
    /// The shadow it casts on the ground ([`ground_shadow`]). Every shadow is
    /// painted before any piece, so it lies under them all; it may reach past
    /// `span`, but never past [`reach`](Self::reach).
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

impl<'a> DrawList<'a> {
    /// The pieces, back to front.
    pub(crate) fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// The room's lights, in no order that matters: a pixel's light is the
    /// brightest's, ties to the lowest rank.
    pub(crate) fn lights(&self) -> &[LightPiece] {
        &self.lights
    }

    pub(crate) fn ambient(&self) -> crate::cutaway::light::Ambient {
        self.ambient
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

/// Build `frame`'s [`DrawList`] at `moment`, on floor `floor_idx`. Every
/// figure, window view and light is resolved here, so painting the list reads
/// neither `frame` nor the moment again.
pub(crate) fn build_list<'a>(
    frame: &SimFrame,
    office: Office<'a>,
    moment: &Moment,
    floor_idx: usize,
    board: &crate::board::BoardModel,
) -> DrawList<'a> {
    let Office {
        pack, theme, scale, ..
    } = office;
    let ambient = crate::cutaway::light::Ambient::of(&moment.look);
    let mut collected = collect_pieces(frame, office, moment);
    collected.extend(signs(office, floor_idx, board));
    let sorted = depth_sort(
        collected
            .into_iter()
            .map(|(span, kind)| (span, (span, kind)))
            .collect(),
    );
    let pieces = sorted
        .into_iter()
        .map(|(span, kind)| Piece {
            span,
            fingerprint: fingerprint(&kind),
            shadow: ground_shadow(span, &kind, pack),
            kind,
        })
        .collect();
    DrawList {
        pieces,
        lights: lights(frame, office, moment, floor_idx, ambient),
        ambient,
        pack,
        theme,
        scale,
    }
}

/// The room's own lights (`crate::lighting`) this frame that the cutaway paints.
fn lights(
    frame: &SimFrame,
    office: Office<'_>,
    moment: &Moment,
    floor_idx: usize,
    ambient: crate::cutaway::light::Ambient,
) -> Vec<LightPiece> {
    let Office {
        layout,
        pack,
        theme,
        scale,
    } = office;
    let lights = crate::lighting::Lights::of(
        layout,
        &moment.look,
        &crate::lighting::LightInputs {
            agents: &frame.agents,
            seated: &frame.seated_agents,
            floor_idx,
            indoor_scale: frame.indoor_scale,
            neon: frame.neon,
            now: moment.now,
        },
    );
    let pen = Pen::for_pack(scale, pack);
    // Each desk's lamp shines from the bulb its art draws, which the cutaway's
    // art stands on the side the desk faces; the model's is the classic's.
    let lamps: Vec<crate::lighting::Emitter> = lights
        .desks
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let facing = layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i));
            let bulb = layout
                .home_desks
                .get(i)
                .zip(desk_art(pack, facing))
                .and_then(|(&at, art)| desk_bulb(at, art, pack, scale));
            match (bulb, d.lamp.light) {
                (Some(centre), crate::lighting::Light::Halo { radius, share, .. }) => {
                    crate::lighting::Emitter {
                        light: crate::lighting::Light::Halo {
                            centre,
                            radius,
                            share,
                        },
                        ..d.lamp
                    }
                }
                _ => d.lamp,
            }
        })
        .collect();
    lights
        .spills
        .iter()
        .chain(&lights.floor_lamp)
        .chain(&lamps)
        .chain(&lights.monitor_halos)
        .chain(std::iter::once(&lights.neon))
        .filter_map(|e| {
            crate::cutaway::light::LightView::of(
                e,
                crate::cutaway::light::tint_of(e.kind, theme, frame.neon),
                ambient,
                pen,
                (layout.buf_w, layout.buf_h),
            )
        })
        .map(|(span, view)| {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            view.hash(&mut h);
            LightPiece {
                span,
                fingerprint: h.finish(),
                view,
            }
        })
        .collect()
}

/// Paint `list` over the backdrop `buf` holds: every piece's shadow
/// ([`ground_shadow`]) first, then the pieces back to front as by day, noting
/// which pixels glow of their own ([`Glow`](crate::cutaway::light::Glow)), and
/// last one pass ([`net_pass`](crate::cutaway::light::net_pass)) takes every
/// other pixel to the hour: darkened with the room and lifted by its lights at
/// once, so no pixel is darkened twice or darkened and relit.
pub(crate) fn paint_list(list: &DrawList<'_>, cache: &mut CutawayCache, buf: &mut RgbBuffer) {
    let pen = Pen::for_pack(list.scale, list.pack);
    paint_ground_shadows(
        list.pieces.iter().filter_map(|p| p.shadow),
        crate::ground::shadow_strength(list.ambient.darkness()),
        pen,
        buf,
    );
    let emission = paint_pieces(list, cache, buf);
    let lights: Vec<&crate::cutaway::light::LightView> =
        list.lights.iter().map(|l| &l.view).collect();
    let whole = ArtRect {
        x: ArtPx(0),
        y: ArtPx(0),
        w: pen.art(list.scale.logical(buf.width()).saturating_add(1)),
        h: pen.art(list.scale.logical(buf.height()).saturating_add(1)),
    };
    crate::cutaway::light::net_pass(
        whole,
        &lights,
        list.ambient,
        &emission,
        pen,
        &mut cache.net_colours,
        buf,
    );
}

/// Paint every piece but the lights, back to front as by day, and return the
/// [`Emission`](crate::cutaway::light::Emission) they leave.
fn paint_pieces(
    list: &DrawList<'_>,
    cache: &mut CutawayCache,
    buf: &mut RgbBuffer,
) -> crate::cutaway::light::Emission {
    use crate::cutaway::light::{Emission, Glow};
    let mut emission = Emission::new(buf.width(), buf.height());
    let mut marks = RgbBuffer::filled(buf.width(), buf.height(), NO_MARK);
    for piece in &list.pieces {
        let (x0, y0) = (
            list.scale.to_buffer(piece.span.x0),
            list.scale.to_buffer(piece.span.y0),
        );
        let x1 = list.scale.to_buffer(piece.span.x1 + 1).min(buf.width());
        let y1 = list.scale.to_buffer(piece.span.y1 + 1).min(buf.height());
        let epoch = buf.begin_writes();
        paint_piece(&piece.kind, list.pack, list.theme, list.scale, cache, buf);
        let glowing = mark(
            &piece.kind,
            (list.pack, list.scale),
            &mut cache.art,
            &mut marks,
        );
        for y in y0..y1 {
            for x in x0..x1 {
                if !buf.written_in(x, y, epoch) {
                    continue;
                }
                let marked = || match marks.get(x, y) {
                    EMISSIVE_MARK => Glow::Emissive,
                    SHADED_MARK => Glow::Shaded,
                    _ => Glow::Lit,
                };
                let glow = match piece.kind {
                    PieceKind::Glass { .. } => Glow::Pane,
                    // A badge keeps the contrast its theme pins at every hour.
                    PieceKind::Neon { .. }
                    | PieceKind::Badge { .. }
                    | PieceKind::Board { .. }
                    | PieceKind::Indicator { .. } => Glow::Emissive,
                    PieceKind::Effect(r) if r.effect.kind == EffectKind::FlameCrown => {
                        Glow::Emissive
                    }
                    PieceKind::Desk { .. }
                    | PieceKind::Prop { .. }
                    | PieceKind::Animated { .. }
                    | PieceKind::Hung { .. }
                    | PieceKind::Door { .. }
                        if glowing =>
                    {
                        marked()
                    }
                    PieceKind::Desk { .. }
                    | PieceKind::Prop { .. }
                    | PieceKind::Animated { .. }
                    | PieceKind::Hung { .. }
                    | PieceKind::Door { .. }
                    | PieceKind::WallSeg { .. }
                    | PieceKind::Chair { .. }
                    | PieceKind::DeskProp(_)
                    | PieceKind::PropBand { .. }
                    | PieceKind::Table { .. }
                    | PieceKind::Character { .. }
                    | PieceKind::Effect(_)
                    | PieceKind::Clock { .. } => Glow::Lit,
                };
                emission.set(x, y, glow);
            }
        }
        if glowing {
            for y in y0..y1 {
                for x in x0..x1 {
                    marks.put(x, y, NO_MARK);
                }
            }
        }
    }
    buf.end_writes();
    emission
}

/// [`mark_glow`]'s marks: colours no art is painted in, only a scratch buffer.
const NO_MARK: pixtuoid_core::sprite::Rgb = pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 };
const EMISSIVE_MARK: pixtuoid_core::sprite::Rgb = pixtuoid_core::sprite::Rgb { r: 1, g: 0, b: 0 };
const SHADED_MARK: pixtuoid_core::sprite::Rgb = pixtuoid_core::sprite::Rgb { r: 2, g: 0, b: 0 };

/// Mark into `marks` where `kind` glows of its own, never outside its span,
/// the most [`paint_pieces`] clears; whether it marked any.
fn mark(
    kind: &PieceKind,
    drawn: (&Pack, RenderScale),
    art_cache: &mut ArtCache,
    marks: &mut RgbBuffer,
) -> bool {
    match *kind {
        PieceKind::Desk { at, art, screen } => mark_glow(at, art, screen, drawn, art_cache, marks),
        PieceKind::Prop { at, art } | PieceKind::Animated { at, art } => {
            mark_bulbs(Placed::Centred(at), art, drawn, art_cache, marks)
        }
        PieceKind::Hung { at, sprite } => mark_bulbs(
            Placed::TopLeft(at),
            Art::still(sprite),
            drawn,
            art_cache,
            marks,
        ),
        PieceKind::Door { at, frame } => mark_bulbs(
            Placed::TopLeft(at),
            Art {
                sprite: DOOR_SPRITE,
                frame,
                flip: Flip::None,
            },
            drawn,
            art_cache,
            marks,
        ),
        PieceKind::Glass { .. }
        | PieceKind::WallSeg { .. }
        | PieceKind::Chair { .. }
        | PieceKind::DeskProp(_)
        | PieceKind::PropBand { .. }
        | PieceKind::Table { .. }
        | PieceKind::Character { .. }
        | PieceKind::Effect(_)
        | PieceKind::Neon { .. }
        | PieceKind::Clock { .. }
        | PieceKind::Badge { .. }
        | PieceKind::Board { .. }
        | PieceKind::Indicator { .. } => false,
    }
}

/// Mark into `marks` where the desk [`paint_desk`] drew at `at` glows of its own:
/// its screen, emissive when on and shaded when off, and its lamp's bulb. Each
/// is where the art draws its key, found by painting that key transparent, so
/// it is the art's own at any density. Returns whether it marked anything.
fn mark_glow(
    at: crate::layout::Point,
    art_name: &'static str,
    screen: Screen,
    (pack, scale): (&Pack, RenderScale),
    art: &mut ArtCache,
    marks: &mut RgbBuffer,
) -> bool {
    use crate::embedded_pack::{DESK_BULB_KEY, SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};
    let (Some(span), Some(desk)) = (
        desk_span(pack, art_name, at, scale),
        crate::embedded_pack::densest_frame(pack, art_name, 0, scale),
    ) else {
        return false;
    };
    let screen_cells = art
        .cells(art_name, 0, &desk, &[SCREEN_GLASS_KEY, SCREEN_TEXT_KEY])
        .to_vec();
    let bulb = art.cells(art_name, 0, &desk, &[DESK_BULB_KEY]);
    let screen_mark = match screen {
        Screen::Off => SHADED_MARK,
        Screen::Standby(_) | Screen::Lit { .. } => EMISSIVE_MARK,
    };
    let (w, h) = (desk.frame.width(), desk.frame.height());
    let pixels: Vec<pixtuoid_core::sprite::Pixel> = bulb
        .iter()
        .zip(&screen_cells)
        .map(|(&bulb, &screen)| {
            if bulb {
                Some(EMISSIVE_MARK)
            } else if screen {
                Some(screen_mark)
            } else {
                None
            }
        })
        .collect();
    let any = pixels.iter().any(Option::is_some);
    let only = pixtuoid_core::sprite::Frame::from_pixels(w, h, pixels);
    let (x, top_y) = (scale.to_buffer(span.x0), scale.to_buffer(span.y0));
    blit_frame_scaled(&only, x, top_y, desk.blit_at, marks);
    any
}

/// Where a piece's art lands: centred on a layout point, or from its top-left.
#[derive(Clone, Copy)]
enum Placed {
    Centred(Point),
    TopLeft(Point),
}

/// Mark emissive where `art` draws a bulb
/// ([`DESK_BULB_KEY`](crate::embedded_pack::DESK_BULB_KEY)); whether it marked
/// any.
fn mark_bulbs(
    placed: Placed,
    art: Art,
    (pack, scale): (&Pack, RenderScale),
    cache: &mut ArtCache,
    marks: &mut RgbBuffer,
) -> bool {
    let Some(dense) = crate::embedded_pack::densest_frame(pack, art.sprite, art.frame, scale)
    else {
        return false;
    };
    let bulbs = cache.cells(
        art.sprite,
        art.frame,
        &dense,
        &[crate::embedded_pack::DESK_BULB_KEY],
    );
    if !bulbs.iter().any(|&b| b) {
        return false;
    }
    let only = pixtuoid_core::sprite::Frame::from_pixels(
        dense.frame.width(),
        dense.frame.height(),
        bulbs.iter().map(|&b| b.then_some(EMISSIVE_MARK)).collect(),
    );
    let (x, y) = placed.top_left(dense.logical, scale);
    blit_frame_scaled(&art.flip.turn(only), x, y, dense.blit_at, marks);
    true
}

impl Placed {
    /// The buffer top-left of `logical`-sized art placed so.
    fn top_left(self, logical: (u16, u16), scale: RenderScale) -> (u16, u16) {
        match self {
            Self::Centred(at) => centred_top_left(at, logical, scale),
            Self::TopLeft(at) => (scale.to_buffer(at.x), scale.to_buffer(at.y)),
        }
    }
}

/// What the cutaway keeps across frames, for one pack.
#[derive(Default)]
pub struct CutawayCache {
    figures: crate::frame_cache::FrameCache,
    art: ArtCache,
    net_colours: crate::cutaway::light::NetMemo,
}

/// Art found by recolouring, kept across frames; keyed by sprite name, so one
/// cache serves one pack.
#[derive(Default)]
pub(crate) struct ArtCache {
    cells: std::collections::HashMap<(&'static str, usize, u16, &'static [char]), Vec<bool>>,
    screens: std::collections::HashMap<(&'static str, u16, Screen), pixtuoid_core::sprite::Frame>,
}

impl ArtCache {
    fn cells(
        &mut self,
        sprite: &'static str,
        frame: usize,
        dense: &crate::embedded_pack::DenseFrame<'_>,
        keys: &'static [char],
    ) -> &[bool] {
        self.cells
            .entry((sprite, frame, dense.density.get(), keys))
            .or_insert_with(|| drawn_in(dense, keys))
    }

    fn desk(
        &mut self,
        art: &'static str,
        desk: &crate::embedded_pack::DenseFrame<'_>,
        screen: Screen,
    ) -> Option<&pixtuoid_core::sprite::Frame> {
        match screen {
            Screen::Off => None,
            Screen::Standby(_) | Screen::Lit { .. } => Some(
                self.screens
                    .entry((art, desk.density.get(), screen))
                    .or_insert_with(|| {
                        screen
                            .on(desk.recolorable)
                            .unwrap_or_else(|| desk.frame.clone())
                    }),
            ),
        }
    }
}

/// Which of `art`'s pixels, row by row, it draws in one of `keys`: those that
/// go transparent when the keys are painted so.
fn drawn_in(art: &crate::embedded_pack::DenseFrame<'_>, keys: &[char]) -> Vec<bool> {
    let without = art
        .recolorable
        .recolored(&keys.iter().map(|&k| (k, None)).collect::<Vec<_>>());
    let (w, h) = (art.frame.width(), art.frame.height());
    let opaque = |f: &pixtuoid_core::sprite::Frame, x, y| f.get(x, y).and_then(|p| *p).is_some();
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| opaque(art.frame, x, y) && !opaque(&without, x, y))
        .collect()
}

/// The layout cell of the desk lamp's bulb the desk `art_name` at `at` draws at
/// `scale`: the middle of its [`DESK_BULB_KEY`](crate::embedded_pack::DESK_BULB_KEY)
/// pixels, or `None` for art that draws no bulb.
fn desk_bulb(
    at: crate::layout::Point,
    art_name: &str,
    pack: &Pack,
    scale: RenderScale,
) -> Option<crate::layout::Point> {
    let span = desk_span(pack, art_name, at, scale)?;
    let desk = crate::embedded_pack::densest_frame(pack, art_name, 0, scale)?;
    let w = usize::from(desk.frame.width());
    let (mut n, mut sx, mut sy) = (0u32, 0u32, 0u32);
    for (i, _) in drawn_in(&desk, &[crate::embedded_pack::DESK_BULB_KEY])
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b)
    {
        n += 1;
        sx += (i % w) as u32;
        sy += (i / w) as u32;
    }
    if n == 0 {
        return None;
    }
    // An art pixel's middle, in cells of the layout, rounded to the cell it lies in.
    let d = f32::from(desk.density.get());
    let cell = |sum: u32| ((sum as f32 / n as f32 + 0.5) / d) as u16;
    Some(crate::layout::Point {
        x: span.x0 + cell(sx),
        y: span.y0 + cell(sy),
    })
}

/// Ramp stops a shadow steps the ground at its centre, per unit of
/// [`shadow_strength`].
///
/// [`shadow_strength`]: crate::ground::shadow_strength
const SHADOW_STOPS_PER_STRENGTH: f32 = 6.0;

/// Where a piece meets the ground, as the shadow it casts there: on the row under
/// its south edge, a standing figure's under its feet. A sitter is grounded by
/// what they sit on, which casts its own: a desk chair a sitter carries, under
/// the chair. Walls, window glass, the elevator and what hangs on a wall meet
/// no ground.
fn ground_shadow(span: Span, kind: &PieceKind, pack: &Pack) -> Option<crate::ground::Contact> {
    let under = |s: Span| {
        Some(crate::ground::Contact::under(
            s.x0,
            s.x1 - s.x0 + 1,
            s.y1 + 1,
        ))
    };
    match *kind {
        PieceKind::WallSeg { .. }
        | PieceKind::Glass { .. }
        | PieceKind::Hung { .. }
        | PieceKind::Door { .. }
        | PieceKind::Neon { .. }
        | PieceKind::Clock { .. }
        | PieceKind::Effect(_)
        | PieceKind::Badge { .. }
        | PieceKind::Board { .. }
        | PieceKind::Indicator { .. } => None,
        PieceKind::Character {
            ref figure,
            body,
            chair,
            ..
        } => {
            if figure.shadow {
                under(body)
            } else {
                let at = chair?;
                let (w, h) = art_size(pack, crate::embedded_pack::DESK_CHAIR_SPRITE)?;
                under(Span::new(at.x, at.y, w, h, 0))
            }
        }
        // Only the band that reaches the prop's foot meets the ground.
        PieceKind::PropBand { sprite, rows, .. } => art_size(pack, sprite)
            .filter(|&(_, h)| rows.1 == h)
            .and_then(|_| under(span)),
        _ => under(span),
    }
}

/// Step the ground darker under `shadows`, toward each one's centre: its falloff
/// at `strength`, rounded to whole ramp stops by
/// [`nearest`](crate::dither::nearest) on the art grid, over their
/// [`Depths`](crate::ground::Depths). A shadow is the ground it falls on,
/// darker, never a colour of its own.
fn paint_ground_shadows(
    shadows: impl Iterator<Item = crate::ground::Contact> + Clone,
    strength: f32,
    pen: Pen,
    buf: &mut RgbBuffer,
) {
    let Some(depths) = crate::ground::Depths::of(shadows, pen.art(1).0) else {
        return;
    };
    let mut stepped: Vec<crate::dither::Stepped> = Vec::new();
    for (ax, ay, depth) in depths.cells() {
        let level = crate::dither::nearest(depth * strength * SHADOW_STOPS_PER_STRENGTH, ax, ay);
        if level == 0 {
            continue;
        }
        while stepped.len() < usize::from(level) {
            stepped.push(crate::dither::Stepped::new(-(stepped.len() as i8 + 1)));
        }
        let r = ArtRect {
            x: ArtPx(ax),
            y: ArtPx(ay),
            w: ArtPx(1),
            h: ArtPx(1),
        };
        let memo = &mut stepped[usize::from(level) - 1];
        pen.recolour(buf, r, |_, _, under| memo.of(under));
    }
}

/// A hash of every input `paint_piece` and [`ground_shadow`] draw `kind` from
/// beyond the layout, theme, pack and scale. Each arm destructures every field,
/// so a field added to a kind fails to compile here until it is hashed or named
/// `_`.
fn fingerprint(kind: &PieceKind) -> u64 {
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
        PieceKind::Glass { ref view } => view.hash(&mut h),
        PieceKind::Hung { at, sprite } => (at, sprite).hash(&mut h),
        PieceKind::Effect(riding) => riding.hash(&mut h),
    }
    h.finish()
}

/// The room's signs: the wall board's text, and the floor indicator naming
/// floor `floor_idx`.
fn signs(
    office: Office<'_>,
    floor_idx: usize,
    board: &crate::board::BoardModel,
) -> [(Span, PieceKind); 2] {
    let pen = Pen::for_pack(office.scale, office.pack);
    let (door, floor) = (office.layout.door, floor_idx + 1);
    [
        (
            board_span(board, pen),
            PieceKind::Board {
                board: board.clone(),
            },
        ),
        (
            topmost_span(indicator_plate(door, floor, pen), pen),
            PieceKind::Indicator { door, floor },
        ),
    ]
}

/// Every piece of the office, each with its [`Span`]. At one depth and layer,
/// push order breaks the tie, so it is part of the result.
fn collect_pieces(frame: &SimFrame, office: Office<'_>, moment: &Moment) -> Vec<(Span, PieceKind)> {
    let layout = office.layout;
    let build = Build {
        frame,
        office,
        moment,
    };
    let mut order: Vec<(Span, PieceKind)> = Vec::new();
    push_windows(office, moment, &GlassWeather::of(moment), &mut order);
    let carried = push_characters(frame, office, moment.now, &mut order);
    for fixture in layout.fixtures() {
        push_fixture(fixture, build, &carried, &mut order);
    }
    wall_segments(layout, &mut order);
    order
}

#[derive(Clone, Copy)]
struct Build<'a, 'f> {
    frame: &'f SimFrame,
    office: Office<'a>,
    moment: &'f Moment,
}

/// How `fixture` ties a figure at its row here: the roster's tie, but for
/// the lounge couch.
fn tie_of(fixture: Fixture) -> Option<Tie> {
    use FixtureKind as K;
    let Depth::Sorted { tie, .. } = fixture.depth else {
        return None;
    };
    Some(match fixture.kind {
        // Seen from behind, facing the window, where the classic draws its front.
        K::LoungeCouch => Tie::FixtureOver,
        K::Desk(_)
        | K::FilingCabinet(_)
        | K::DeskChair(_)
        | K::Station { .. }
        | K::Plant { .. }
        | K::Pod { .. }
        | K::Wall { .. }
        | K::MeetingRug { .. }
        | K::MeetingSofa { .. }
        | K::MeetingTable { .. }
        | K::MeetingChair { .. }
        | K::CoatRack { .. }
        | K::Doormat { .. }
        | K::NoticeBoard { .. }
        | K::LoungeRug
        | K::SideTable
        | K::FloorLamp
        | K::FishTank
        | K::KitchenIsland
        | K::PantryMat
        | K::IslandMat
        | K::WaterCooler
        | K::TrashBin
        | K::Door
        | K::Runner
        | K::NeonSign
        | K::Clock => tie,
    })
}

/// The row the cutaway sorts a fixture on: a backdrop one at the very back.
fn sort_row(depth: Depth) -> u16 {
    match depth {
        Depth::Backdrop => 0,
        Depth::Sorted { row, .. } => row,
    }
}

/// Where to centre art of `b`'s size so it lands on `b`.
fn centre_of(b: Bounds) -> Point {
    Point {
        x: b.x + b.width / 2,
        y: b.y + b.height / 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covering {
    Rug,
    Runner,
}

/// Whether `kind` is a floor covering the backdrop lays ([`paint_backdrop`]):
/// flat on the ground, so it lies under the shadows cast on it, and never moves.
fn covering(kind: FixtureKind) -> Option<Covering> {
    use FixtureKind as K;
    match kind {
        K::MeetingRug { .. } | K::LoungeRug | K::Doormat { .. } | K::PantryMat | K::IslandMat => {
            Some(Covering::Rug)
        }
        K::Runner => Some(Covering::Runner),
        K::Desk(_)
        | K::FilingCabinet(_)
        | K::DeskChair(_)
        | K::Station { .. }
        | K::Plant { .. }
        | K::Pod { .. }
        | K::Wall { .. }
        | K::MeetingSofa { .. }
        | K::MeetingTable { .. }
        | K::MeetingChair { .. }
        | K::CoatRack { .. }
        | K::NoticeBoard { .. }
        | K::LoungeCouch
        | K::SideTable
        | K::FloorLamp
        | K::FishTank
        | K::KitchenIsland
        | K::WaterCooler
        | K::TrashBin
        | K::Door
        | K::NeonSign
        | K::Clock => None,
    }
}

/// The elevator's art.
const DOOR_SPRITE: &str = "door";

/// Queue one of the roster's fixtures, sorted on its depth ([`sort_row`]). A
/// desk chair whose desk is in `carried` rides its sitter's piece instead.
fn push_fixture(
    fixture: Fixture,
    build: Build<'_, '_>,
    carried: &[Point],
    order: &mut Vec<(Span, PieceKind)>,
) {
    use FixtureKind as K;
    let Build {
        frame,
        office,
        moment,
    } = build;
    let Office {
        layout,
        pack,
        theme,
        ..
    } = office;
    let depth = sort_row(fixture.depth);
    let first = order.len();
    let centre = centre_of(fixture.visual);
    let top_left = Point {
        x: fixture.visual.x,
        y: fixture.visual.y,
    };
    match fixture.kind {
        K::Desk(i) => push_desk(i, build, depth, order),
        K::FilingCabinet(_) => push_art(
            order,
            pack,
            centre,
            Art::still("filing_cabinet"),
            depth,
            Motion::Still,
        ),
        K::DeskChair(i) => {
            let Some(&desk) = layout.home_desks.get(i.0) else {
                return;
            };
            if carried.contains(&desk) {
                return;
            }
            if let Some((span, at)) = chair_span(pack, layout.desk_facing(i), desk) {
                order.push((span.with_depth(depth), PieceKind::Chair { at }));
            }
        }
        K::Station { waypoint, station } => {
            let Some(wp) = layout.waypoints.get(waypoint) else {
                return;
            };
            match station {
                Station::PantryCounter => {
                    if let Some(pantry) = &layout.pantry {
                        let sprite = crate::layout::pantry_counter_anim(pantry.counter_size.w);
                        push_art(
                            order,
                            pack,
                            wp.pos,
                            Art::still(sprite),
                            depth,
                            Motion::Still,
                        );
                    }
                }
                Station::SnackShelf => push_art(
                    order,
                    pack,
                    wp.pos,
                    Art::still("snack_shelf"),
                    depth,
                    Motion::Still,
                ),
                Station::VendingMachine | Station::Printer => {
                    let Some(sprite) = crate::embedded_pack::appliance_sprite(wp.kind) else {
                        return;
                    };
                    let Some(anim) = pack.animation(sprite) else {
                        return;
                    };
                    let busy = frame.occupied_waypoints.contains(&waypoint);
                    let art = Art {
                        sprite,
                        frame: crate::embedded_pack::appliance_frame_index(anim, busy, moment.now),
                        flip: Flip::None,
                    };
                    push_art(order, pack, wp.pos, art, depth, Motion::Playing);
                }
            }
        }
        K::Plant { kind, .. } => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Motion::Still,
        ),
        K::Pod { kind, .. } => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Motion::Still,
        ),
        K::Wall { kind, .. } if kind.stands_on_floor() => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Motion::Still,
        ),
        K::Wall { kind, .. } => push_hung(order, pack, top_left, kind.sprite_name(), depth),
        K::NoticeBoard { .. } => push_hung(order, pack, top_left, "notice_board", depth),
        // A back-view sofa splits into bands its sitter sorts between
        // ([`push_sofa`]), so it lays its own layers.
        K::MeetingSofa {
            room,
            seat,
            faces_away,
        } => {
            let sofa = layout
                .meeting_rooms
                .get(room)
                .and_then(|r| r.trio)
                .and_then(|t| t.sofas.get(seat).copied());
            if let (Some(at), Some(tie)) = (sofa, tie_of(fixture)) {
                push_sofa(order, pack, at, faces_away, tie);
            }
            return;
        }
        K::LoungeCouch => {
            if let Some(tie) = tie_of(fixture) {
                push_sofa(order, pack, centre, true, tie);
            }
            return;
        }
        K::MeetingTable { .. } => {
            let table = crate::layout::furniture_def(crate::layout::Furniture::MeetingTable).visual;
            let face = face_rows(
                pack,
                crate::embedded_pack::MEETING_TABLE_SPRITE,
                office.scale,
            );
            order.push((
                piece_span(
                    crate::layout::Anchor::Center,
                    centre,
                    table.w,
                    table.h,
                    face,
                )
                .with_depth(depth),
                PieceKind::Table { at: centre },
            ));
        }
        // The art's back is west, for a sitter facing east.
        K::MeetingChair { facing, .. } => {
            let flip = if facing == crate::layout::Facing::East {
                Flip::None
            } else {
                Flip::Horizontal
            };
            let art = Art {
                sprite: "meeting_chair",
                frame: 0,
                flip,
            };
            push_art(order, pack, centre, art, depth, Motion::Still);
        }
        K::CoatRack { .. } => push_art(
            order,
            pack,
            centre,
            Art::still("coat_rack"),
            depth,
            Motion::Still,
        ),
        K::SideTable => push_art(
            order,
            pack,
            centre,
            Art::still("side_table"),
            depth,
            Motion::Still,
        ),
        K::FloorLamp => push_art(
            order,
            pack,
            centre,
            Art::still("floor_lamp"),
            depth,
            Motion::Still,
        ),
        K::KitchenIsland => push_art(
            order,
            pack,
            centre,
            Art::still("kitchen_island"),
            depth,
            Motion::Still,
        ),
        K::TrashBin => push_art(
            order,
            pack,
            centre,
            Art::still("pantry_bin"),
            depth,
            Motion::Still,
        ),
        K::FishTank => push_looping(order, pack, centre, "fish_tank", moment.now, depth),
        K::WaterCooler => push_looping(order, pack, centre, "water_cooler", moment.now, depth),
        K::Door => {
            let Some((w, h)) = art_size(pack, DOOR_SPRITE) else {
                return;
            };
            order.push((
                piece_span(crate::layout::Anchor::TopLeft, top_left, w, h, 0).with_depth(depth),
                PieceKind::Door {
                    at: top_left,
                    frame: frame.door_frame,
                },
            ));
        }
        K::NeonSign => {
            let look = crate::floor::neon_look(frame.neon, theme);
            let b = fixture.visual;
            order.push((
                Span::new(b.x, b.y, b.width, b.height, 0).with_depth(depth),
                PieceKind::Neon {
                    at: b,
                    tube: look.tube,
                    hue: look.halo,
                    interior: look.interior,
                },
            ));
        }
        K::Clock => {
            let b = fixture.visual;
            order.push((
                Span::new(b.x, b.y, b.width, b.height, 0).with_depth(depth),
                PieceKind::Clock {
                    at: top_left,
                    reading: crate::sky::clock_reading(moment.now),
                },
            ));
        }
        // The backdrop lays them ([`covering`]).
        K::MeetingRug { .. }
        | K::LoungeRug
        | K::Doormat { .. }
        | K::PantryMat
        | K::IslandMat
        | K::Runner => {}
    }
    if let Some(tie) = tie_of(fixture) {
        for (span, _) in &mut order[first..] {
            *span = span.with_layer(Layer::from(tie));
        }
    }
}

/// Whether an art piece's frame moves between builds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Motion {
    Still,
    Playing,
}

fn push_art(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: Point,
    art: Art,
    depth: u16,
    motion: Motion,
) {
    let Some((w, h)) = art_size(pack, art.sprite) else {
        return;
    };
    let kind = match motion {
        Motion::Still => PieceKind::Prop { at, art },
        Motion::Playing => PieceKind::Animated { at, art },
    };
    order.push((
        piece_span(crate::layout::Anchor::Center, at, w, h, 0).with_depth(depth),
        kind,
    ));
}

fn push_looping(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: Point,
    sprite: &'static str,
    now: std::time::SystemTime,
    depth: u16,
) {
    let Some(anim) = pack.animation(sprite) else {
        return;
    };
    let art = Art {
        sprite,
        frame: crate::embedded_pack::looping_frame_index(anim, now),
        flip: Flip::None,
    };
    push_art(order, pack, at, art, depth, Motion::Playing);
}

fn push_hung(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: Point,
    sprite: &'static str,
    depth: u16,
) {
    if let Some((w, h)) = art_size(pack, sprite) {
        order.push((
            piece_span(crate::layout::Anchor::TopLeft, at, w, h, 0).with_depth(depth),
            PieceKind::Hung { at, sprite },
        ));
    }
}

/// Paint one piece of the draw list.
fn paint_piece(
    kind: &PieceKind,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    cache: &mut CutawayCache,
    buf: &mut RgbBuffer,
) {
    match *kind {
        PieceKind::Desk { at, art, screen } => {
            paint_desk(at, art, screen, (pack, scale), &mut cache.art, buf);
        }
        PieceKind::Chair { at } => paint_chair(at, pack, scale, buf),
        PieceKind::DeskProp(prop) => paint_desk_prop(prop, pack, theme, scale, buf),
        PieceKind::Effect(ref riding) => riding.paint(theme, buf),
        PieceKind::Character {
            ref figure, chair, ..
        } => {
            paint_figure(figure, pack, scale, &mut cache.figures, buf);
            // The sitter's own chair, straight after them: one piece, so
            // nothing can sort between a person and the chair they sit in.
            if let Some(at) = chair {
                paint_chair(at, pack, scale, buf);
            }
        }
        PieceKind::Prop { at, art } | PieceKind::Animated { at, art } => {
            paint_art(at, art, pack, theme, scale, buf)
        }
        PieceKind::PropBand { at, sprite, rows } => {
            paint_prop_band(at, sprite, rows, pack, scale, buf);
        }
        PieceKind::Table { at } => paint_table(at, pack, scale, buf),
        PieceKind::Door { at, frame } => paint_door(at, frame, pack, scale, buf),
        PieceKind::Neon {
            at,
            tube,
            hue,
            interior,
        } => paint_neon(at, [tube, hue, interior], Pen::for_pack(scale, pack), buf),
        PieceKind::Clock { at, reading } => paint_clock(at, reading, pack, theme, scale, buf),
        PieceKind::WallSeg {
            piece,
            rows: (y0, y1),
        } => crate::wall::paint_wall(buf, theme, piece, y0..y1, Pen::for_pack(scale, pack)),
        PieceKind::Glass { ref view } => paint_glass(view, Pen::for_pack(scale, pack), buf),
        PieceKind::Hung { at, sprite } => paint_wall_decor(at, sprite, pack, scale, buf),
        PieceKind::Badge { ref badge, .. } => {
            paint_badge(badge, theme, Pen::for_pack(scale, pack), buf);
        }
        PieceKind::Board { ref board } => {
            let pen = Pen::for_pack(scale, pack);
            for (at, seg) in board_runs(board, pen) {
                let ink = crate::board::tone_rgb(seg.tone, theme);
                crate::cutaway::text::paint(pen, buf, at, &seg.text, ink);
            }
        }
        PieceKind::Indicator { door, floor } => {
            let pen = Pen::for_pack(scale, pack);
            let text = crate::layout::floor_indicator_text(floor);
            let plate = indicator_plate(door, floor, pen);
            let runs = [(text.as_str(), theme.ui.neon_brand)];
            paint_plate(pen, buf, plate, theme.ui.tooltip_bg, &runs);
        }
    }
}

/// Desk `i` in its facing's art, its screen lit by the classic painter's own
/// rule ([`desk_screen_glow`](crate::lighting::desk_screen_glow)) from the
/// sim's observation, so the profiles never disagree about WHICH screens are
/// lit; sorted on `depth`.
fn push_desk(
    i: pixtuoid_core::state::FloorLocalDeskIndex,
    build: Build<'_, '_>,
    depth: u16,
    order: &mut Vec<(Span, PieceKind)>,
) {
    let Build {
        frame,
        office,
        moment,
    } = build;
    let Office {
        layout,
        pack,
        theme,
        scale,
    } = office;
    let Some(&d) = layout.home_desks.get(i.0) else {
        return;
    };
    let facing = layout.desk_facing(i);
    let Some(art) = desk_art(pack, facing) else {
        return;
    };
    let props = frame.desk(i);
    let screen = Screen::of(
        crate::lighting::desk_screen_glow(
            crate::sim::desk_occupant(&frame.agents, i),
            facing,
            frame.seated_agents.get(&i).copied().unwrap_or(false),
            theme,
        ),
        props.scanline,
        crate::lighting::screen_idle(facing, moment.look.darkness, frame.indoor_scale),
        theme,
    );
    if let Some(span) = desk_span(pack, art, d, scale) {
        let span = span.with_depth(depth);
        order.push((span, PieceKind::Desk { at: d, art, screen }));
        push_desk_props(&props, (art, span), office, order);
    }
}

/// The desk props' pack animations.
const DESK_CUP_SPRITE: &str = "desk_cup";
const TOKEN_TOWER_SPRITE: &str = "token_tower";
const TOKEN_SHEET_SPRITE: &str = "token_sheet";

/// What stands on the desk whose `art` paints `span`, each at the art's own
/// mark for it, sorted with the desk: the cup where there is one, the token
/// tower at its tier and the sheet falling onto it.
fn push_desk_props(
    props: &crate::sim::DeskProps,
    (art, span): (&'static str, Span),
    office: Office<'_>,
    order: &mut Vec<(Span, PieceKind)>,
) {
    let Office { pack, scale, .. } = office;
    let Some(desk) = crate::embedded_pack::densest_frame(pack, art, 0, scale) else {
        return;
    };
    let k = desk.blit_at.get();
    let (x0, y0) = (scale.to_buffer(span.x0), scale.to_buffer(span.y0));
    // A mark's cell, as the buffer column of its west edge and row past its foot.
    let mark = |name: &str| {
        let m = desk.marks.iter().find(|m| m.name() == name)?;
        Some((x0 + m.x() * k, y0 + (m.y() + 1) * k))
    };
    // Stand frame `frame` of `sprite` on `(x, foot)`; where its top lands.
    let mut stand = |sprite: &'static str, frame: usize, (x, foot): (u16, u16)| {
        let f = crate::embedded_pack::densest_frame(pack, sprite, frame, scale)?;
        let b = f.blit_at.get();
        let (w, h) = (f.frame.width() * b, f.frame.height() * b);
        let y = foot.checked_sub(h)?;
        let s = scale.get();
        let cells = |at: u16, len: u16| (at / s, (at + len - 1) / s - at / s + 1);
        let ((cx, cw), (cy, ch)) = (cells(x, w), cells(y, h));
        let prop = StoodProp {
            sprite,
            frame,
            at: (x, y),
        };
        order.push((
            Span::new(cx, cy, cw, ch, 0)
                .with_depth(span.depth)
                .with_layer(span.layer),
            PieceKind::DeskProp(prop),
        ));
        Some(y)
    };
    if let (Some(_), Some(at)) = (props.cup, mark("cup")) {
        stand(DESK_CUP_SPRITE, 0, at);
    }
    let Some(tier) = usize::from(props.token_tier).checked_sub(1) else {
        return;
    };
    let Some((x, top)) =
        mark("tower").and_then(|at| Some((at.0, stand(TOKEN_TOWER_SPRITE, tier, at)?)))
    else {
        return;
    };
    // The sheet lands as the pile's next sheet: at its full fall it is gone.
    let rest = props
        .sheet_fall
        .and_then(|fallen| crate::token_meter::SHEET_FALL_PX.checked_sub(fallen))
        .filter(|&rest| rest > 0);
    if let Some(rest) = rest {
        let foot = top.checked_sub((rest - 1) * scale.get());
        if let Some(foot) = foot {
            stand(TOKEN_SHEET_SPRITE, 0, (x, foot));
        }
    }
}

/// The pack's desk art for a seat facing `facing`: the facing's own when the
/// pack ships it, else what [`Pack::piece_or_source`] draws in its place.
fn desk_art(pack: &Pack, facing: crate::layout::Facing) -> Option<&'static str> {
    pack.piece_or_source(crate::embedded_pack::desk_sprite_name(facing))
}

/// The box a desk drawn with `art` at `desk` occupies at `scale`: the art and
/// the face rows [`face_rows`] derives under it, sorted on the last of
/// those. A taller art grows upward from the same bottom row
/// ([`desk_art_top`](crate::embedded_pack::desk_art_top)), so its depth never moves.
fn desk_span(
    pack: &Pack,
    art: &str,
    desk: crate::layout::Point,
    scale: RenderScale,
) -> Option<Span> {
    let (w, h) = art_size(pack, art)?;
    let span = piece_span(
        crate::layout::Anchor::TopLeft,
        crate::layout::Point {
            x: desk.x,
            y: crate::embedded_pack::desk_art_top(pack, desk.y, h),
        },
        w,
        h,
        face_rows(pack, art, scale),
    );
    Some(span)
}

/// The rows of front face the cutaway derives under a top-down piece's `art`
/// at `scale`: a desk's, the meeting table's.
///
/// Only this profile draws a density variant (the classic painter's scale is 1,
/// where `densest_frame` returns the base), so `@Nx` art is authored for this
/// profile with its whole front; a derived face under it would read as a plank
/// on the ground.
fn face_rows(pack: &Pack, art: &str, scale: RenderScale) -> u16 {
    match crate::embedded_pack::densest_frame(pack, art, 0, scale) {
        Some(d) if d.density.get() > 1 => 0,
        _ => desk_front_h(),
    }
}

/// The deepest a noon shadow steps the ground.
#[cfg(test)]
fn deepest_shadow_stop() -> i8 {
    let stops = crate::ground::shadow_strength(NOON_DARKNESS) * SHADOW_STOPS_PER_STRENGTH;
    let phases = 0..crate::dither::PERIOD;
    phases
        .clone()
        .flat_map(|y| {
            phases
                .clone()
                .map(move |x| crate::dither::nearest(stops, x, y))
        })
        .max()
        .map_or(0, |deepest| deepest as i8)
}

/// A clear noon's darkness, the hour the shadow tests pin.
#[cfg(test)]
const NOON_DARKNESS: f32 = 0.0;

/// Compares `variant`, a cutaway render of a pack whose desks ship density
/// variants, against `base`, one of `base_pack`, on the two things the cutaway
/// promises: the art
/// lands exactly where the base's does (every pixel outside each desk's foot — its
/// face band and the shadows under it — is identical), and a variant, which draws
/// its own front, has floor under its art where the base gets a derived face.
#[cfg(test)]
pub(crate) fn assert_variant_desk_foot(
    variant: &[pixtuoid_core::sprite::Rgb],
    base: &[pixtuoid_core::sprite::Rgb],
    layout: &Layout,
    base_pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    now: std::time::SystemTime,
) {
    // The room darkens every pixel by the hour's steps; its lights must be off.
    let ambient = crate::cutaway::light::Ambient::of(&crate::atmosphere::SkyTones::resolve(
        &crate::sky::Sky::clock(now),
        theme,
    ));
    let mut floor = RgbBuffer::filled(
        scale.to_buffer(layout.buf_w),
        scale.to_buffer(layout.buf_h),
        theme.surface.bg_fallback,
    );
    paint_ground(layout, theme, Pen::for_pack(scale, base_pack), &mut floor);
    let buf_w = usize::from(scale.to_buffer(layout.buf_w));
    let face = desk_front_h();
    // Each desk's columns and the first row below its art, in logical units.
    let feet: Vec<(u16, u16, u16)> = (0..layout.home_desks.len())
        .filter_map(|i| {
            let d = layout.home_desks[i];
            let art = desk_art(
                base_pack,
                layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i)),
            )?;
            let (w, h) = art_size(base_pack, art)?;
            Some((
                d.x,
                d.x + w - 1,
                crate::embedded_pack::desk_art_top(base_pack, d.y, h) + h,
            ))
        })
        .collect();
    assert!(!feet.is_empty(), "the office must have a desk");
    // A desk's foot: its face band, and where a variant's shadow and the base's
    // fall, one face band apart.
    let in_foot = |x: u16, y: u16| {
        feet.iter().any(|&(x0, x1, below)| {
            let shadow = |base: u16| {
                let ((sx0, sy0), (sx1, sy1)) =
                    crate::ground::Contact::under(x0, x1 - x0 + 1, base).bounds();
                (sx0..sx1).contains(&x) && (sy0..sy1).contains(&y)
            };
            ((x0..=x1).contains(&x) && (below..=below + face).contains(&y))
                || shadow(below)
                || shadow(below + face)
        })
    };
    let differs_outside = (0..variant.len()).find(|&i| {
        let (x, y) = ((i % buf_w) as u16, (i / buf_w) as u16);
        variant[i] != base[i] && !in_foot(scale.logical(x), scale.logical(y))
    });
    assert_eq!(
        differs_outside, None,
        "the desk art moved with its density (first differing buffer pixel)"
    );
    let at = |px: &[pixtuoid_core::sprite::Rgb], x: u16, y: u16| {
        px[usize::from(scale.to_buffer(y)) * buf_w + usize::from(scale.to_buffer(x))]
    };
    let deepest = deepest_shadow_stop();
    let ground_or_its_shadow = |c: pixtuoid_core::sprite::Rgb,
                                floor: pixtuoid_core::sprite::Rgb| {
        (0..=deepest).any(|k| c == ambient.on(floor.ramp(-k)))
    };
    // The desk's own edge column: a sitter and their chair stand centred on it.
    for &(x0, _, below) in &feet {
        let x = x0 + 1;
        for y in below..=below + face {
            assert!(
                ground_or_its_shadow(at(variant, x, y), at(floor.as_slice(), x, y)),
                "a variant draws its own front: under its art lies floor or its shadow"
            );
        }
        assert!(
            !ground_or_its_shadow(at(base, x, below), at(floor.as_slice(), x, below)),
            "the base gets a derived face under its art"
        );
    }
}

/// The chair's box and top-left at a desk facing `facing`, placed and keyed by
/// the layout's rules
/// ([`desk_chair_top_left`](crate::layout::desk_chair_top_left),
/// [`desk_chair_z_key`](crate::layout::desk_chair_z_key)); `None` where
/// those stand no chair or the pack has none.
fn chair_span(
    pack: &Pack,
    facing: crate::layout::Facing,
    desk: crate::layout::Point,
) -> Option<(Span, crate::layout::Point)> {
    let at = crate::layout::desk_chair_top_left(desk, facing)?;
    let (w, h) = art_size(pack, crate::embedded_pack::DESK_CHAIR_SPRITE)?;
    let span = piece_span(crate::layout::Anchor::TopLeft, at, w, h, 0)
        .with_depth(crate::layout::desk_chair_z_key(desk, facing));
    Some((span, at))
}

/// Queue every character, and return the desks whose chairs they carry, so
/// [`push_fixture`] stands none of those again.
fn push_characters(
    frame: &SimFrame,
    office: Office<'_>,
    now: std::time::SystemTime,
    order: &mut Vec<(Span, PieceKind)>,
) -> Vec<crate::layout::Point> {
    let Office {
        layout,
        pack,
        theme,
        scale,
    } = office;
    let pen = Pen::for_pack(scale, pack);
    let namesakes = crate::overlay::Namesakes::of(&frame.agents);
    let mut carried = Vec::new();
    for c in &frame.characters {
        let Some(agent) = frame.agents.get(c.agent_idx) else {
            continue;
        };
        let pose = crate::character::SpritePose::of(c, agent, theme);
        // The frame `paint_figure` draws: an animation's frames need not
        // share a size.
        let Some((w, h)) = crate::embedded_pack::densest_frame(
            pack,
            pose.anim_name,
            pose.frame_idx,
            RenderScale::ONE,
        )
        .map(|d| d.logical) else {
            continue;
        };
        let Some(key) = crate::character::character_key(pose, agent, pack, scale, now) else {
            continue;
        };
        let seat = c.seat_desk.map(|d| (d, layout.desk_facing_at(d)));
        let chair = seat.and_then(|(d, facing)| chair_span(pack, facing, d));
        if let (Some((d, _)), Some(_)) = (seat, chair) {
            carried.push(d);
        }
        let badge_ceiling = seat.and_then(|(d, facing)| {
            desk_span(pack, desk_art(pack, facing)?, d, scale).map(|s| s.y0)
        });
        let at = cutaway_anchor(c);
        let shadow = !c.seated;
        // The drawn box reaches up over the hair its style dresses it in.
        let hair = key
            .dress
            .as_ref()
            .map_or(0, |d| d.rise().div_ceil(key.frame.density.get()));
        let top = crate::layout::Point {
            x: at.x,
            y: at.y.saturating_sub(hair),
        };
        let span = occupant_span(
            piece_span(crate::layout::Anchor::TopLeft, top, w, h + hair, 0),
            c.anchor_y,
            chair.map(|(span, _)| span),
        );
        let riders = riders(c, &key, (w, at), scale);
        // Dust lies on the ground under its walker; the rest ride over them.
        let ride = |order: &mut Vec<(Span, PieceKind)>, beneath: bool| {
            for r in riders.iter().filter(|r| r.effect.kind.beneath() == beneath) {
                if let Some(s) = r.span(theme, span.depth) {
                    order.push((s, PieceKind::Effect(*r)));
                }
            }
        };
        ride(order, true);
        order.push((
            span,
            PieceKind::Character {
                figure: Figure { at, shadow, key },
                chair: chair.map(|(_, at)| at),
                body: Span::new(top.x, top.y, w, h + hair, 0),
            },
        ));
        ride(order, false);
        let badge = Badge {
            at: label_anchor(top, w, badge_ceiling),
            text: namesakes.text(agent),
            tone: crate::overlay::tone_of(agent),
        };
        order.push((
            topmost_span(badge.plate(pen), pen),
            PieceKind::Badge { badge },
        ));
    }
    carried
}

/// `c`'s effects on the grid of its figure `key`, `w` logical columns wide
/// with its frame's top-left at `at`.
fn riders(
    c: &crate::sim::CharacterPlacement,
    key: &crate::character::CharacterKey,
    (w, at): (u16, crate::layout::Point),
    scale: RenderScale,
) -> Vec<crate::cutaway::effects::Riding> {
    let d = key.frame.density.get();
    let Some(pen) = Pen::new(scale, d) else {
        return Vec::new();
    };
    let head = key.dress.as_ref().map(|dress| {
        let (d, frame_w) = (i32::from(d), i32::from(w) * i32::from(d));
        let x = i32::from(dress.head.x);
        crate::cutaway::effects::ArtPoint {
            x: i32::from(at.x) * d + if key.frame.flip_x { frame_w - 1 - x } else { x },
            y: i32::from(at.y) * d + dress.crest,
        }
    });
    c.effects
        .iter()
        .map(|&effect| crate::cutaway::effects::Riding { effect, head, pen })
        .collect()
}

/// A figure's piece: its drawn bounds, sorted on `depth` — the sim's own z-key,
/// which neither breath nor the sit arc moves, so a person never flips against a
/// neighbour mid-breath. A back-turned sitter and their chair are one piece,
/// bounding the chair's whole box too.
fn occupant_span(body: Span, depth: u16, chair: Option<Span>) -> Span {
    let body = body.with_depth(depth).with_layer(Layer::Figure);
    match chair {
        Some(chair) => Span {
            x0: body.x0.min(chair.x0),
            x1: body.x1.max(chair.x1),
            y0: body.y0.min(chair.y0),
            y1: body.y1.max(chair.y1),
            depth: body.depth.max(chair.depth),
            layer: Layer::Figure,
        },
        None => body,
    }
}

/// The back-view sofa's art: its seat beyond the backrest, the backrest nearest
/// the viewer.
const MEETING_SOFA_NORTH: &str = "meeting_sofa_north";
/// The rows of [`MEETING_SOFA_NORTH`]'s art that lie UNDER its sitter, the seat;
/// the backrest below them draws OVER the sitter's lap. `scripts/gen-art.py`'s
/// `SOFA_SEAT_ROWS` draws to it (`the_north_sofas_backrest_starts_on_its_lit_ridge`).
const NORTH_SOFA_SEAT_ROWS: u16 = 3;

/// Queue one sofa body, sorted with its sitters at `tie`: the front view, or
/// the `back_view`. A pack that draws [`MEETING_SOFA_NORTH`] gets it as two
/// bands, the seat under its sitter and the backrest over their lap
/// ([`NORTH_SOFA_SEAT_ROWS`]); one that draws only its own `meeting_sofa` gets
/// that flipped top-to-bottom, as the classic painter draws it.
///
/// NOT `back_couch`: the pack documents that as a character seen from behind, so
/// it would draw a headless torso where the couch belongs.
fn push_sofa(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: crate::layout::Point,
    back_view: bool,
    tie: Tie,
) {
    let sitters = crate::sim::seat::sofa_sitter_z_key(at);
    if let Some((w, h)) = art_size(pack, MEETING_SOFA_NORTH).filter(|_| back_view) {
        let tl = crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, w, h);
        let split = NORTH_SOFA_SEAT_ROWS.min(h);
        let band = |rows| PieceKind::PropBand {
            at,
            sprite: MEETING_SOFA_NORTH,
            rows,
        };
        // Its own south edge lies rows north of where the sofa stands, where a
        // table in a short room would tie it and paint over it.
        let seat = Span::new(tl.x, tl.y, w, split, 0).with_depth(sitters);
        order.push((seat, band((0, split))));
        order.push((
            Span::new(tl.x, tl.y + split, w, h - split, 0).with_layer(Layer::from(tie)),
            band((split, h)),
        ));
        return;
    }
    if let Some((w, h)) = art_size(pack, "meeting_sofa") {
        let span = piece_span(crate::layout::Anchor::Center, at, w, h, 0)
            .with_depth(sitters)
            .with_layer(Layer::from(tie));
        order.push((
            span,
            PieceKind::Prop {
                at,
                art: Art {
                    sprite: "meeting_sofa",
                    frame: 0,
                    flip: if back_view {
                        Flip::Vertical
                    } else {
                        Flip::None
                    },
                },
            },
        ));
    }
}

/// Queue every room wall's [sort bands](crate::layout::WallPiece::sort_bands) as
/// pieces: the long-object case [`crate::cutaway::order`] documents.
fn wall_segments(layout: &Layout, order: &mut Vec<(Span, PieceKind)>) {
    for &piece in &layout.wall_pieces {
        let (at, size) = piece.visual();
        for (rows, depth) in piece.sort_bands() {
            order.push((
                Span::new(at.x, rows.start, size.w, rows.end - rows.start, 0)
                    .with_depth(depth)
                    .with_layer(Layer::Over),
                PieceKind::WallSeg {
                    piece,
                    rows: (rows.start, rows.end),
                },
            ));
        }
    }
}

impl PieceKind {
    /// Whether it recolours what lies under it rather than painting colours of
    /// its own: a room wall's glass ([`PieceKind::WallSeg`]), not a window's
    /// [`PieceKind::Glass`].
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
            | PieceKind::Glass { .. }
            | PieceKind::Desk { .. }
            | PieceKind::DeskProp(_)
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
    /// One window's glass and what it looks out on.
    Glass {
        view: WindowView,
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
        /// The facing's art (see [`desk_art`]).
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
    /// between ([`push_sofa`]).
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
    /// A prop the model stands on a desk ([`push_desk_props`]).
    DeskProp(StoodProp),
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
    sprite: &'static str,
    frame: usize,
    at: (u16, u16),
}

/// Which art a prop draws: a sprite's frame, turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Art {
    sprite: &'static str,
    frame: usize,
    flip: Flip,
}

impl Art {
    /// `sprite`'s first frame, as drawn.
    fn still(sprite: &'static str) -> Self {
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

impl Flip {
    fn turn(self, frame: pixtuoid_core::sprite::Frame) -> pixtuoid_core::sprite::Frame {
        match self {
            Self::None => frame,
            Self::Vertical => frame.mirror_vertical(),
            Self::Horizontal => frame.mirror_horizontal(),
        }
    }
}

/// Everything [`paint_figure`] draws one figure from, resolved when the list is
/// built.
pub(crate) struct Figure {
    /// The art's top-left, in logical units.
    at: crate::layout::Point,
    /// Grounded by a contact shadow: not sitting on furniture, which grounds a
    /// sitter instead.
    shadow: bool,
    key: crate::character::CharacterKey,
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

/// A piece's bounds, as [`Span::new`] builds them from its sprite's box.
/// Anchoring goes through [`crate::layout::anchored_top_left`], the same function
/// the walkable mask and the classic painter use.
fn piece_span(
    anchor: crate::layout::Anchor,
    pos: crate::layout::Point,
    w: u16,
    h: u16,
    below: u16,
) -> Span {
    let tl = crate::layout::anchored_top_left(anchor, pos, w, h);
    Span::new(tl.x, tl.y, w, h, below)
}

/// Frame 0's LOGICAL size: the size the sort space lays a static piece out in,
/// whichever density it is drawn from. An animated figure sizes from the frame
/// it draws (`push_characters`).
fn art_size(pack: &Pack, sprite: &str) -> Option<(u16, u16)> {
    crate::embedded_pack::densest_frame(pack, sprite, 0, RenderScale::ONE).map(|d| d.logical)
}

/// Rows of front face derived under a top-down desk: its thickness.
fn desk_front_h() -> u16 {
    (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1)
}

/// Paint the north wall band: the wall, its windows' frames where
/// [`Layout::window_bays`] tiles them, and its trim. The glass is list pieces
/// ([`push_windows`]).
///
/// Its height is [`Layout::wall_band_h`], not `top_margin`: the rows between
/// are floor the agents walk on, so a band drawn to `top_margin` would paint
/// over them.
fn paint_wall(layout: &Layout, theme: &Theme, scale: RenderScale, pen: Pen, buf: &mut RgbBuffer) {
    let band_h = layout.wall_band_h();
    if band_h == 0 {
        return;
    }
    let s = scale.get();
    let w = scale.to_buffer(layout.buf_w);
    let wall = Ramp::from_base(theme.surface.wall);
    slab(buf, 0, 0, w, scale.to_buffer(band_h), &wall, scale);
    let rows = crate::layout::window_rows(band_h);
    let window_h = rows.end - rows.start;
    for bay in layout.window_bays() {
        for dy in 0..window_h {
            for dx in 0..bay.w {
                if crate::layout::window_frame(
                    dx,
                    dy,
                    Size {
                        w: bay.w,
                        h: window_h,
                    },
                ) {
                    let cell = ArtRect {
                        x: pen.art(bay.x + dx),
                        y: pen.art(rows.start + dy),
                        w: pen.art(1),
                        h: pen.art(1),
                    };
                    pen.fill(buf, cell, theme.surface.window_frame);
                }
            }
        }
    }
    for post in crate::layout::window_posts(layout.buf_w) {
        let cell = ArtRect {
            x: pen.art(post.start),
            y: pen.art(rows.start),
            w: pen.art(post.end - post.start),
            h: pen.art(window_h),
        };
        pen.fill(buf, cell, theme.surface.window_frame);
    }
    fill(
        buf,
        0,
        scale.to_buffer(crate::layout::wall_trim_row(band_h)),
        w,
        s,
        theme.surface.wall_trim,
    );
    // The wall's contact line: the ground under it, a shade step down.
    let mut contact = crate::dither::Stepped::new(crate::cutaway::shade::RAMP_SHADE_LEVEL);
    pen.recolour(
        buf,
        ArtRect {
            x: ArtPx(0),
            y: pen.art(band_h),
            w: pen.art(layout.buf_w),
            h: pen.art(1),
        },
        |_, _, under| contact.of(under),
    );
}

/// Queue each window's glass as a piece: what it looks out on — the one city
/// ([`CityStrip`]) on the pen's art grid over the classic's sky, disc, stars
/// and blaze ([`SkyView`]), under `weather` — resolved when the list is built, at the very back of the order, so
/// the view changes with the sky, the weather and the city's lights without touching the
/// backdrop.
///
/// [`CityStrip`]: crate::skyline::CityStrip
/// [`SkyView`]: crate::celestial::SkyView
fn push_windows(
    office: Office<'_>,
    moment: &Moment,
    weather: &GlassWeather,
    order: &mut Vec<(Span, PieceKind)>,
) {
    let Office {
        layout,
        pack,
        theme,
        scale,
    } = office;
    let pen = Pen::for_pack(scale, pack);
    let rows = crate::layout::window_rows(layout.wall_band_h());
    let window_h = rows.end - rows.start;
    let glass_h = crate::layout::glass_rows(window_h);
    let Some(density) = pixtuoid_core::sprite::format::Density::new(pen.art(1).0) else {
        return;
    };
    let run = crate::layout::window_run(layout.buf_w);
    let city = crate::skyline::CityStrip::draw(
        pack,
        (run.end - run.start, glass_h),
        moment,
        theme,
        density,
    );
    let d = density.get();
    let sky = crate::celestial::SkyView::of(moment, layout.buf_w, layout.wall_band_h(), theme);
    for bay in layout.window_bays() {
        let size = Size {
            w: bay.w,
            h: window_h,
        };
        let (w, h) = (bay.w * d, window_h * d);
        let (x0, y0) = (pen.art(bay.x).0, pen.art(rows.start).0);
        let pane = sky.pane(bay.x, bay.w, glass_h, d);
        let mut px: Vec<_> = (0..h)
            .flat_map(|ay| (0..w).map(move |ax| (ax, ay)))
            .map(|(ax, ay)| {
                if crate::layout::window_frame(ax / d, ay / d, size) {
                    return None;
                }
                // The strip's art pixel: x from the run's west end, so one city
                // runs on behind every frame and the wall between windows; y from
                // the glass's top, under its top frame row.
                let (cx, cy) = ((bay.x - run.start) * d + ax, ay - d);
                Some(city.at(cx, cy).unwrap_or_else(|| {
                    let open = pane.colour((x0 + ax, y0 + ay), cy);
                    sky.blaze().map_or(open, |b| b.over(open))
                }))
            })
            .collect();
        if let Some((veil, alpha)) = weather.veil {
            for c in px.iter_mut().flatten() {
                *c = crate::composite::blend_rgb(*c, veil, alpha);
            }
        }
        // The glass starts a unit in from the window's top-left frame; a mark
        // that lands on the mullion or transom stays behind it.
        let glass = Size {
            w: bay.w.saturating_sub(2),
            h: glass_h,
        };
        for m in weather.marks(bay.idx, glass, d) {
            let (ax, ay) = (m.x + d, m.y + d);
            if let Some(Some(c)) = px.get_mut(usize::from(ay) * usize::from(w) + usize::from(ax)) {
                *c = m.over(*c, (x0 + ax, y0 + ay));
            }
        }
        order.push((
            Span::new(bay.x, rows.start, bay.w, window_h, 0).with_depth(0),
            PieceKind::Glass {
                view: WindowView {
                    x: x0,
                    y: y0,
                    w,
                    px,
                },
            },
        ));
    }
}

/// What one window shows this frame, resolved when the list is built: the
/// art pixels of its box from its top-left, row by row, `None` on its frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WindowView {
    x: u16,
    y: u16,
    w: u16,
    px: Vec<Option<pixtuoid_core::sprite::Rgb>>,
}

/// Paint a window's [`WindowView`].
fn paint_glass(view: &WindowView, pen: Pen, buf: &mut RgbBuffer) {
    let rows = view.px.chunks(usize::from(view.w).max(1));
    for (row, dy) in rows.zip(0u16..) {
        for (c, dx) in row.iter().zip(0u16..) {
            let Some(c) = *c else { continue };
            let cell = ArtRect {
                x: ArtPx(view.x + dx),
                y: ArtPx(view.y + dy),
                w: ArtPx(1),
                h: ArtPx(1),
            };
            pen.fill(buf, cell, c);
        }
    }
}

/// The carpet, lit near the windows, falling off south and laid in tiles, on
/// the art grid: every edge, dither step and seam lands on an art pixel,
/// whatever the scale.
fn paint_ground(layout: &Layout, theme: &Theme, pen: Pen, buf: &mut RgbBuffer) {
    let lit = theme.surface.carpet_light;
    let base = theme.surface.carpet_base;
    let dark = theme.surface.carpet_dark;

    let h = pen.art(layout.buf_h);
    let w = pen.art(layout.buf_w);
    let band = |y0: u16, rows: u16| ArtRect {
        x: ArtPx(0),
        y: ArtPx(y0),
        w,
        h: ArtPx(rows),
    };
    pen.fill(buf, band(0, h.0), base);

    // Anchored at the wall's foot, where the ground begins, not buffer row 0: the
    // wall band paints over the top of the buffer, so a lit zone anchored there
    // would start behind it.
    let ground_top = pen.art(layout.wall_band_h()).0;
    let ground_h = h.0.saturating_sub(ground_top);

    // The lit share of the ground: its first half solid, dithering to base by its
    // end, then a final fall to dark at the south edge.
    let lit_h = ground_h * GROUND_LIT_NUMER / GROUND_LIT_DENOM;
    pen.fill(buf, band(ground_top, lit_h / 2), lit);
    pen.dither_band(
        buf,
        ArtPx(ground_top + lit_h / 2),
        ArtPx(ground_top + lit_h),
        base,
        lit,
    );
    pen.dither_band(buf, ArtPx(h.0.saturating_sub(lit_h / 2)), h, dark, base);

    let tile = pen.art(GROUND_TILE);
    if tile.0 >= MIN_SEAMED_TILE {
        pen.shade_grid(buf, band(ground_top, ground_h), tile, SEAM_LEVEL);
    }
}

/// A rug on the art grid: the classic's trim, accent line and field, then a
/// woven lattice and fringe at its short ends wherever the density has room.
fn paint_rug(rug: crate::layout::Bounds, theme: &Theme, pen: Pen, buf: &mut RgbBuffer) {
    let f = &theme.furniture;
    let d = pen.art(1).0;
    let (x0, y0, w, h) = (
        pen.art(rug.x).0,
        pen.art(rug.y).0,
        pen.art(rug.width).0,
        pen.art(rug.height).0,
    );
    let rect = |x: u16, y: u16, w: u16, h: u16| ArtRect {
        x: ArtPx(x),
        y: ArtPx(y),
        w: ArtPx(w),
        h: ArtPx(h),
    };
    let inset = |i: u16| {
        rect(
            x0 + i,
            y0 + i,
            w.saturating_sub(2 * i),
            h.saturating_sub(2 * i),
        )
    };
    // Half a logical unit wide, so at 1x this is the classic's rug, cell for cell.
    let trim = (d / 2).max(1);
    pen.fill(buf, inset(0), f.rug_trim);
    pen.fill(buf, inset(trim), f.rug_accent);
    pen.fill(buf, inset(trim + 1), f.rug_field);
    if d == 1 {
        return;
    }

    // Mirrored about the rug's centre column, so it sits square in the field.
    let motif = f.rug_field.ramp(RUG_MOTIF_LEVEL);
    let lattice = inset(trim + 3);
    for y in lattice.y.0..lattice.y.0 + lattice.h.0 {
        for x in lattice.x.0..lattice.x.0 + lattice.w.0 {
            let (dx, dy) = (x - x0, y - y0);
            if (dx + dy).is_multiple_of(RUG_LATTICE)
                || (w - 1 - dx + dy).is_multiple_of(RUG_LATTICE)
            {
                pen.fill(buf, rect(x, y, 1, 1), motif);
            }
        }
    }
    // A tassel every other pixel along each short end, as long as the trim.
    let tassel = f.rug_trim.ramp(RUG_FRINGE_LEVEL);
    if w >= h {
        for y in (y0 + 1..y0 + h - 1).step_by(2) {
            pen.fill(buf, rect(x0.saturating_sub(trim), y, trim, 1), tassel);
            pen.fill(buf, rect(x0 + w, y, trim, 1), tassel);
        }
    } else {
        for x in (x0 + 1..x0 + w - 1).step_by(2) {
            pen.fill(buf, rect(x, y0.saturating_sub(trim), 1, trim), tassel);
            pen.fill(buf, rect(x, y0 + h, 1, trim), tassel);
        }
    }
}

fn paint_desk(
    at: crate::layout::Point,
    art_name: &'static str,
    screen: Screen,
    (pack, scale): (&Pack, RenderScale),
    art: &mut ArtCache,
    buf: &mut RgbBuffer,
) {
    let (Some(span), Some(desk)) = (
        desk_span(pack, art_name, at, scale),
        crate::embedded_pack::densest_frame(pack, art_name, 0, scale),
    ) else {
        return;
    };
    let (x, top_y) = (scale.to_buffer(span.x0), scale.to_buffer(span.y0));
    blit_frame_scaled(
        art.desk(art_name, &desk, screen).unwrap_or(desk.frame),
        x,
        top_y,
        desk.blit_at,
        buf,
    );
    paint_derived_face(
        &desk,
        (x, top_y),
        face_rows(pack, art_name, scale),
        scale,
        buf,
    );
}

/// `rows` of front face under `art` drawn at `top_left`, in the material of its
/// bottom row ([`dominant_opaque_row`]).
fn paint_derived_face(
    art: &crate::embedded_pack::DenseFrame<'_>,
    top_left: (u16, u16),
    rows: u16,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    if rows == 0 {
        return;
    }
    let Some(material) = dominant_opaque_row(art.frame, art.frame.height().saturating_sub(1))
    else {
        return;
    };
    // The drawn size, from the logical size: variant art blits at `blit_at`, so
    // its own pixel size times the scale would drop a base's face a whole piece
    // low.
    let (drawn_w, drawn_h) = (
        scale.to_buffer(art.logical.0),
        scale.to_buffer(art.logical.1),
    );
    slab(
        buf,
        top_left.0,
        top_left.1 + drawn_h,
        drawn_w,
        scale.to_buffer(rows),
        &Ramp::from_base(material),
        scale,
    );
}

/// The desk art with its screen lit in `glow`: the glass takes a dark step of
/// the glow and its dim content turns to bright text. Recoloring the pack's own
/// screen KEYS ([`SCREEN_GLASS_KEY`](crate::embedded_pack::SCREEN_GLASS_KEY),
/// [`SCREEN_TEXT_KEY`](crate::embedded_pack::SCREEN_TEXT_KEY)), rather than
/// painting a band over the desk or matching a colour, lights exactly the screen
/// the art drew — at whatever density it was drawn — and no other pixel, even
/// one the same colour as the glass.
fn relight_screen(
    art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    glow: pixtuoid_core::sprite::Rgb,
) -> pixtuoid_core::sprite::Frame {
    art.recolored(&[
        (
            crate::embedded_pack::SCREEN_GLASS_KEY,
            Some(glow.ramp(SCREEN_GLASS_LEVEL)),
        ),
        (
            crate::embedded_pack::SCREEN_TEXT_KEY,
            Some(glow.ramp(SCREEN_TEXT_LEVEL)),
        ),
    ])
}

/// `lit`, the screen of `art` relit in `glow`, with its scanline on glass
/// column `scan`: the glass's columns split evenly among the classic's
/// [`SCREEN_GLASS_COLS`](crate::layout::SCREEN_GLASS_COLS).
fn scanline(
    lit: pixtuoid_core::sprite::Frame,
    art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    glow: pixtuoid_core::sprite::Rgb,
    scan: u16,
) -> pixtuoid_core::sprite::Frame {
    use crate::embedded_pack::{SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};
    // The glass is wherever the screen keys draw: what clearing them uncovers.
    let bare = art.recolored(&[(SCREEN_GLASS_KEY, None), (SCREEN_TEXT_KEY, None)]);
    let (w, h) = (lit.width(), lit.height());
    let px = |f: &pixtuoid_core::sprite::Frame, x, y| f.get(x, y).copied().flatten();
    let glass = |x, y| px(&lit, x, y).is_some() && px(&bare, x, y).is_none();
    let Some((x0, x1)) = (0..w)
        .filter(|&x| (0..h).any(|y| glass(x, y)))
        .fold(None, |r: Option<(u16, u16)>, x| {
            Some(r.map_or((x, x), |(a, _)| (a, x)))
        })
    else {
        return lit;
    };
    let cols = crate::layout::SCREEN_GLASS_COLS;
    let band = ((x1 - x0 + 1) / (cols.end() - cols.start() + 1)).max(1);
    let line = (x0 + scan * band)..(x0 + (scan + 1) * band);
    let color = crate::effects::look::scanline_color(glow);
    let pixels = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            if line.contains(&x) && glass(x, y) {
                Some(color)
            } else {
                px(&lit, x, y)
            }
        })
        .collect();
    pixtuoid_core::sprite::Frame::from_pixels(w, h, pixels)
}

/// The most common opaque colour in `row` of `frame` — how the cutaway learns a
/// sprite's material without hardcoding it. The front face a top-down sprite
/// never had has to be SOME colour, and the desk's lives in the PACK (the
/// shaded wood, palette key `d`), not the theme, where `furniture.wood_top`
/// reads nearly like the carpet; sampling also earns a custom pack's desk a
/// match for free.
fn dominant_opaque_row(
    frame: &pixtuoid_core::sprite::Frame,
    row: u16,
) -> Option<pixtuoid_core::sprite::Rgb> {
    let w = frame.width();
    let mut best: Option<(pixtuoid_core::sprite::Rgb, usize)> = None;
    for x in 0..w {
        let Some(c) = frame.get(x, row).and_then(|p| *p) else {
            continue;
        };
        let n = (0..w)
            .filter(|&i| frame.get(i, row).and_then(|p| *p) == Some(c))
            .count();
        if best.is_none_or(|(_, bn)| n > bn) {
            best = Some((c, n));
        }
    }
    best.map(|(c, _)| c)
}

/// Wall-hung decor, blitted at its own `pos`: [`paint_art`] would centre it, but
/// `WallDecorItem.pos` is TOP-LEFT, like the classic painter's `Anchor::TopLeft`
/// z-sort rather than the centre-pinned furniture, so centring would hang every
/// board up and west of where it belongs.
fn paint_wall_decor(
    pos: crate::layout::Point,
    sprite: &str,
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = crate::embedded_pack::densest_frame(pack, sprite, 0, scale) else {
        return;
    };
    blit_frame_scaled(
        art.frame,
        scale.to_buffer(pos.x),
        scale.to_buffer(pos.y),
        art.blit_at,
        buf,
    );
}

/// The classic placement's anchor, unchanged: the seat side is a per-desk layout
/// fact, so an override here would make the two profiles disagree about which
/// side of its desk half the office sits on.
fn cutaway_anchor(c: &crate::sim::CharacterPlacement) -> crate::layout::Point {
    c.anchor
}

fn paint_figure(
    figure: &Figure,
    pack: &Pack,
    scale: RenderScale,
    cache: &mut crate::frame_cache::FrameCache,
    buf: &mut RgbBuffer,
) {
    let Figure { at, ref key, .. } = *figure;
    // The classic painter's own recolor + facing-flip path, through the same
    // cache: a raw pack blit clones one placeholder-palette person per agent.
    let Some(art) = crate::character::keyed_character_frame(key, pack, scale, cache) else {
        return;
    };
    // A dressed frame reaches up over its hair, its art's top staying at `at`:
    // whatever rows would start above the buffer are cut, not the whole figure
    // pushed down.
    let blit = i32::from(art.blit_at.get());
    let top = i32::from(scale.to_buffer(at.y)) - i32::from(art.rise) * blit;
    let cut = u16::try_from((-top).max(0).unsigned_abs().div_ceil(blit.unsigned_abs()))
        .unwrap_or(u16::MAX);
    let clipped;
    let frame = if cut == 0 {
        art.frame
    } else {
        let f = art.frame;
        let rows = f.height().saturating_sub(cut);
        let px = (cut..f.height())
            .flat_map(|y| (0..f.width()).map(move |x| (x, y)))
            .map(|(x, y)| f.get(x, y).copied().flatten())
            .collect();
        clipped = pixtuoid_core::sprite::Frame::from_pixels(f.width(), rows, px);
        &clipped
    };
    blit_frame_scaled(
        frame,
        scale.to_buffer(at.x),
        u16::try_from(top + i32::from(cut) * blit).unwrap_or(0),
        art.blit_at,
        buf,
    );
}

/// The badge anchor for a body of `sprite_w` logical columns drawn at `at`:
/// horizontally centred, [`LABEL_GAP`] logical rows clear of the head — and of
/// `ceiling`, a logical row the badge must stay above (a raised monitor behind
/// a back-turned sitter's head).
///
/// A free fn so the test can drive THE anchor rather than restate its
/// arithmetic: a test asserting properties of its own copy stays green for any
/// change to the real one.
fn label_anchor(
    at: crate::layout::Point,
    sprite_w: u16,
    ceiling: Option<u16>,
) -> crate::layout::Point {
    crate::layout::Point {
        x: at.x + sprite_w / 2,
        y: ceiling
            .map_or(at.y, |top| at.y.min(top))
            .saturating_sub(LABEL_GAP),
    }
}

/// The meeting table's art, centred on its layout point, over the front face
/// [`face_rows`] derives under a base-density drawing.
fn paint_table(at: crate::layout::Point, pack: &Pack, scale: RenderScale, buf: &mut RgbBuffer) {
    let Some(table) = crate::embedded_pack::densest_frame(
        pack,
        crate::embedded_pack::MEETING_TABLE_SPRITE,
        0,
        scale,
    ) else {
        return;
    };
    let (x, y) = centred_top_left(at, table.logical, scale);
    blit_frame_scaled(table.frame, x, y, table.blit_at, buf);
    paint_derived_face(
        &table,
        (x, y),
        face_rows(pack, crate::embedded_pack::MEETING_TABLE_SPRITE, scale),
        scale,
        buf,
    );
}

/// Blit `art` centred on `at`, in the theme's colours ([`theme_overrides`]).
fn paint_art(
    at: Point,
    art: Art,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(dense) = crate::embedded_pack::densest_frame(pack, art.sprite, art.frame, scale)
    else {
        return;
    };
    let themed = dense.recolorable.recolored(&theme_overrides(theme));
    let (x, y) = centred_top_left(at, dense.logical, scale);
    blit_frame_scaled(&art.flip.turn(themed), x, y, dense.blit_at, buf);
}

/// A desk prop in the theme's cup and paper.
fn paint_desk_prop(
    prop: StoodProp,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(f) = crate::embedded_pack::densest_frame(pack, prop.sprite, prop.frame, scale) else {
        return;
    };
    let f_themed = f.recolorable.recolored(&desk_prop_overrides(theme));
    blit_frame_scaled(&f_themed, prop.at.0, prop.at.1, f.blit_at, buf);
}

/// The pack keys the desk props draw their cup's body and shadow in.
const CUP_KEY: char = 'V';
const CUP_SHADE_KEY: char = '%';
/// The pack keys the token tower and its sheet draw their paper in.
const PAPER_KEY: char = '¤';
const PAPER_SHADE_KEY: char = '!';

/// The pack keys the desk props take from the theme.
fn desk_prop_overrides(theme: &Theme) -> [(char, pixtuoid_core::sprite::Pixel); 4] {
    let f = &theme.furniture;
    [
        (CUP_KEY, Some(f.coffee_cup)),
        (CUP_SHADE_KEY, Some(f.coffee_cup_shadow)),
        (PAPER_KEY, Some(f.paper)),
        (PAPER_SHADE_KEY, Some(f.paper_shade)),
    ]
}

/// The pack keys art takes from the theme.
fn theme_overrides(theme: &Theme) -> Vec<(char, pixtuoid_core::sprite::Pixel)> {
    crate::embedded_pack::appliance_overrides(&theme.appliance)
        .into_iter()
        .chain(crate::embedded_pack::fixture_overrides(theme))
        .collect()
}

fn paint_door(at: Point, frame: usize, pack: &Pack, scale: RenderScale, buf: &mut RgbBuffer) {
    let Some(art) = crate::embedded_pack::densest_frame(pack, DOOR_SPRITE, frame, scale) else {
        return;
    };
    blit_frame_scaled(
        art.frame,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y),
        art.blit_at,
        buf,
    );
}

/// The neon sign over `at`. At base density its border is the tube, as the
/// classic draws it; denser, the tube is a one-pixel lit core between two
/// pixels of its hue.
fn paint_neon(
    at: Bounds,
    [tube, hue, interior]: [pixtuoid_core::sprite::Rgb; 3],
    pen: Pen,
    buf: &mut RgbBuffer,
) {
    let (x, y, w, h) = (
        pen.art(at.x).0,
        pen.art(at.y).0,
        pen.art(at.width).0,
        pen.art(at.height).0,
    );
    let rect = |x: u16, y: u16, w: u16, h: u16| ArtRect {
        x: ArtPx(x),
        y: ArtPx(y),
        w: ArtPx(w),
        h: ArtPx(h),
    };
    pen.fill(buf, rect(x, y, w, h), interior);
    let border = pen.art(crate::layout::NEON_PANEL_BORDER).0;
    let ring = |buf: &mut RgbBuffer, i: u16, c| {
        let (x, y, w, h) = (x + i, y + i, w - 2 * i, h - 2 * i);
        for edge in [
            rect(x, y, w, 1),
            rect(x, y + h - 1, w, 1),
            rect(x, y, 1, h),
            rect(x + w - 1, y, 1, h),
        ] {
            pen.fill(buf, edge, c);
        }
    };
    if border == 1 {
        ring(buf, 0, tube);
        return;
    }
    let core = border / 2;
    ring(buf, core - 1, hue);
    ring(buf, core, tube);
    if core + 1 < border {
        ring(buf, core + 1, hue);
    }
}

/// The wall clock's dial from its top-left `at`, its hands at `reading` on the
/// art grid: the classic's octant hands at base density, a line from the
/// centre pin denser.
fn paint_clock(
    at: Point,
    reading: crate::sky::ClockReading,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(dial) = crate::embedded_pack::densest_frame(pack, CLOCK_SPRITE, 0, scale) else {
        return;
    };
    let themed = dial.recolorable.recolored(&theme_overrides(theme));
    let (bx, by) = (scale.to_buffer(at.x), scale.to_buffer(at.y));
    blit_frame_scaled(&themed, bx, by, dial.blit_at, buf);
    let pen = Pen::for_pack(scale, pack);
    let (ax, ay) = (pen.art(at.x).0, pen.art(at.y).0);
    let hand = theme.office.clock_hand;
    let dot = |buf: &mut RgbBuffer, x: i32, y: i32| {
        if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
            pen.fill(
                buf,
                ArtRect {
                    x: ArtPx(x),
                    y: ArtPx(y),
                    w: ArtPx(1),
                    h: ArtPx(1),
                },
                hand,
            );
        }
    };
    let (hour, minute) = reading.turns();
    let d = pen.art(1).0;
    let side = pen.art(crate::layout::CLOCK.w).0;
    if d == 1 {
        // The classic's [`CLOCK`](crate::layout::CLOCK) face: the centre pin, the
        // hour a step out, the minute two, but one on a diagonal, where two
        // would cut the rim.
        let (cx, cy) = (i32::from(ax + side / 2), i32::from(ay + side / 2));
        dot(buf, cx, cy);
        let (hx, hy) = crate::sky::octant_offset(hour);
        dot(buf, cx + hx, cy + hy);
        let (mx, my) = crate::sky::octant_offset(minute);
        let reach = if mx != 0 && my != 0 { 1 } else { 2 };
        for step in 1..=reach {
            dot(buf, cx + mx * step, cy + my * step);
        }
        return;
    }
    let centre = (
        f32::from(ax) + f32::from(side) / 2.0,
        f32::from(ay) + f32::from(side) / 2.0,
    );
    let Some(face) = face_radius(&dial, d) else {
        return;
    };
    for (turn, share) in [
        (hour, CLOCK_HOUR_HAND_SHARE),
        (minute, CLOCK_MINUTE_HAND_SHARE),
    ] {
        let (sin, cos) = (turn * std::f32::consts::TAU).sin_cos();
        for step in 1..=(face * share) as i32 {
            let t = step as f32;
            dot(
                buf,
                (centre.0 + t * sin).floor() as i32,
                (centre.1 - t * cos).floor() as i32,
            );
        }
    }
}

/// The dial's art.
const CLOCK_SPRITE: &str = "wall_clock";

/// How far the `dial` art's face ([`CLOCK_FACE_KEY`](crate::embedded_pack::CLOCK_FACE_KEY))
/// reaches from its centre along its middle row, in art pixels at density
/// `d`: the hands stay inside the rim the art draws.
fn face_radius(dial: &crate::embedded_pack::DenseFrame<'_>, d: u16) -> Option<f32> {
    let face = drawn_in(dial, &[crate::embedded_pack::CLOCK_FACE_KEY]);
    let (w, h) = (
        usize::from(dial.frame.width()),
        usize::from(dial.frame.height()),
    );
    let row = &face[h / 2 * w..(h / 2 + 1) * w];
    let first = row.iter().position(|&f| f)?;
    let per_px = f32::from(d) / f32::from(dial.density.get());
    Some((w as f32 / 2.0 - first as f32) * per_px)
}
/// How far across the face each hand reaches.
const CLOCK_HOUR_HAND_SHARE: f32 = 0.5;
const CLOCK_MINUTE_HAND_SHARE: f32 = 0.85;

/// The classic's corridor runner on the art grid, its lines one art pixel wide.
fn paint_runner(b: Bounds, theme: &Theme, pen: Pen, buf: &mut RgbBuffer) {
    let o = &theme.office;
    let (x0, y0, w, h) = (
        pen.art(b.x).0,
        pen.art(b.y).0,
        pen.art(b.width).0,
        pen.art(b.height).0,
    );
    let pitch = i32::from(pen.art(1).0) * crate::layout::roster::RUNNER_LATTICE_STRIDE;
    let px = |buf: &mut RgbBuffer, x: u16, y: u16, c| {
        pen.fill(
            buf,
            ArtRect {
                x: ArtPx(x),
                y: ArtPx(y),
                w: ArtPx(1),
                h: ArtPx(1),
            },
            c,
        );
    };
    pen.fill(
        buf,
        ArtRect {
            x: ArtPx(x0),
            y: ArtPx(y0),
            w: ArtPx(w),
            h: ArtPx(h),
        },
        o.runner_base,
    );
    for dy in 1..h.saturating_sub(1) {
        for dx in 0..w {
            let (i, j) = (i32::from(dx), i32::from(dy));
            if (i + j) % pitch == 0 || (i - j).rem_euclid(pitch) == 0 {
                px(buf, x0 + dx, y0 + dy, o.runner_stripe);
            }
        }
    }
    for dx in 0..w {
        px(buf, x0 + dx, y0, o.runner_edge);
        px(buf, x0 + dx, y0 + h - 1, o.runner_edge);
    }
}

/// The buffer top-left of a `logical`-sized piece centred on `at`, for
/// `blit_frame_scaled`, which takes a top-left: the centring is undone in
/// logical space before converting, so the piece lands on the layout's grid.
fn centred_top_left(
    at: crate::layout::Point,
    logical: (u16, u16),
    scale: RenderScale,
) -> (u16, u16) {
    let crate::layout::Point { x, y } =
        crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, logical.0, logical.1);
    (scale.to_buffer(x), scale.to_buffer(y))
}

/// Blit rows `rows` of a centred prop ([`PieceKind::PropBand`]).
fn paint_prop_band(
    at: crate::layout::Point,
    sprite: &str,
    rows: (u16, u16),
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(dense) = crate::embedded_pack::densest_frame(pack, sprite, 0, scale) else {
        return;
    };
    let (w, h) = dense.logical;
    let tl = crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, w, h);
    let (r0, r1) = (rows.0.min(h), rows.1.min(h));
    let d = dense.density.get();
    let fw = usize::from(dense.frame.width());
    let px = dense.frame.as_slice();
    let band = pixtuoid_core::sprite::Frame::from_pixels(
        dense.frame.width(),
        (r1 - r0).saturating_mul(d),
        px[usize::from(r0) * usize::from(d) * fw..usize::from(r1) * usize::from(d) * fw].to_vec(),
    );
    blit_frame_scaled(
        &band,
        scale.to_buffer(tl.x),
        scale.to_buffer(tl.y + r0),
        dense.blit_at,
        buf,
    );
}

/// A task chair from the pack's art.
fn paint_chair(at: crate::layout::Point, pack: &Pack, scale: RenderScale, buf: &mut RgbBuffer) {
    let Some(art) = crate::embedded_pack::densest_frame(
        pack,
        crate::embedded_pack::DESK_CHAIR_SPRITE,
        0,
        scale,
    ) else {
        return;
    };
    blit_frame_scaled(
        art.frame,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y),
        art.blit_at,
        buf,
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::embedded_pack::test_default_pack;

    /// The wall board of an empty office, which no clock moves: for frames
    /// whose board a test does not read.
    pub(crate) fn quiet_board() -> &'static crate::board::BoardModel {
        static BOARD: std::sync::LazyLock<crate::board::BoardModel> =
            std::sync::LazyLock::new(|| {
                crate::board::build_board(
                    crate::board::StateCounts::default(),
                    0,
                    None,
                    None,
                    std::time::UNIX_EPOCH,
                )
            });
        &BOARD
    }

    /// `floor` at `now`, under the [`quiet_board`].
    pub(crate) fn showing(
        floor: crate::floor::FloorMeta,
        now: std::time::SystemTime,
    ) -> Showing<'static> {
        Showing {
            floor,
            now,
            board: quiet_board(),
        }
    }

    /// A piece's base row — the ordering key — through the SAME `piece_span`
    /// the draw list builds with. Width does not affect the base row, so the
    /// call sites stay focused on depth.
    fn sort_row(
        anchor: crate::layout::Anchor,
        pos: crate::layout::Point,
        h: u16,
        below: u16,
    ) -> u16 {
        piece_span(anchor, pos, 1, h, below).depth
    }

    /// The desk sorts on its face's south edge and a back-turned sitter on their
    /// seat's z-key; that key lands south of the face, so the "head
    /// over the surface" reading needs no special case.
    #[test]
    fn a_seated_occupant_sorts_in_front_of_the_desk_it_sits_at() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 0, y: 10 };
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let desk_z = desk_span(&pack, art, desk, RenderScale::ONE)
            .expect("desk")
            .depth;
        let seated_z = seated_back_span(&pack, desk).depth;
        assert!(
            seated_z > desk_z,
            "a seated occupant must paint over its desk (desk {desk_z}, seated {seated_z})"
        );
    }

    /// The depth sort reserves no face under a density variant, which draws its
    /// own front, so someone on the first row south of the art stands in FRONT
    /// of the desk, not behind a face that is never drawn.
    #[test]
    #[cfg(feature = "density-art")]
    fn someone_just_south_of_a_variant_desk_sorts_in_front_of_it() {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let desk = crate::layout::Point { x: 0, y: 20 };
        let art = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
        let desk_box = desk_span(&pack, art, desk, scale).expect("desk");
        let (_, art_h) = art_size(&pack, art).expect("desk");
        // The first row south of the ART, measured from its placement.
        let feet = crate::embedded_pack::desk_art_top(&pack, desk.y, art_h) + art_h;
        let (w, h) = base_size(&pack, "standing");
        let person = piece_span(
            crate::layout::Anchor::TopLeft,
            crate::layout::Point {
                x: desk.x,
                y: feet + 1 - h,
            },
            w,
            h,
            0,
        );
        assert_eq!(
            depth_sort(vec![(person, "person"), (desk_box, "desk")]),
            ["desk", "person"],
            "the person just south of the art must paint over the desk"
        );
    }

    /// A back-turned sitter's depth box, built the way `push_characters` builds
    /// it, at the z-key the sim seats an occupant at (their seat's walk anchor).
    fn seated_back_span(pack: &Pack, desk: crate::layout::Point) -> Span {
        use crate::layout::Facing;
        let (w, h) = base_size(pack, "seated_back");
        let chair = chair_span(pack, Facing::North, desk).map(|(s, _)| s);
        occupant_span(
            piece_span(crate::layout::Anchor::TopLeft, near_seat(desk), w, h, 0),
            crate::layout::desk_walk_anchor_facing(desk, Facing::North).y,
            chair,
        )
    }

    /// The other half: someone on the FAR side is occluded BY the desk, which is
    /// what gives the office depth rather than a flat plan.
    #[test]
    fn a_character_north_of_the_desk_sorts_behind_it() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 0, y: 20 };
        let (_, body_h) = base_size(&pack, "standing");

        let plain = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
        let desk_z = desk_span(&pack, plain, desk, RenderScale::ONE)
            .expect("desk")
            .depth;
        // Standing at the desk's north approach, feet on the row just north of
        // its anchor.
        let behind_z = sort_row(
            crate::layout::Anchor::TopLeft,
            crate::layout::Point {
                x: desk.x,
                y: desk.y - body_h,
            },
            body_h,
            0,
        );
        assert!(behind_z < desk_z, "desk {desk_z}, walker {behind_z}");
    }

    /// A centre-anchored prop standing in the aisle SOUTH of a desk must paint
    /// in front of that desk and behind its occupant. Keyed on its own middle
    /// row, a tall plant between the two would paint over the occupant while
    /// standing behind them.
    #[test]
    fn an_aisle_prop_sorts_between_the_desk_and_its_occupant() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 0, y: 10 };
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let desk_box = desk_span(&pack, art, desk, RenderScale::ONE).expect("desk");
        let seated = seated_back_span(&pack, desk);
        let (plant_w, plant_h) = base_size(&pack, "plant");
        // A plant whose BASE sits just south of the desk's front face.
        let plant_base = desk_box.depth + 1;
        let plant_centre = crate::layout::Point {
            x: plant_w / 2,
            y: plant_base + plant_h / 2 - plant_h + 1,
        };
        let plant = piece_span(
            crate::layout::Anchor::Center,
            plant_centre,
            plant_w,
            plant_h,
            0,
        );
        assert!(
            desk_box.depth < plant.depth && plant.depth < seated.depth,
            "the fixture must separate all three by depth, or a tie-break decides: \
             desk {desk_box:?}, plant {plant:?}, seated {seated:?}"
        );
        // Pushed in REVERSE, so no tie-break by push order can produce the answer.
        let drawn = crate::cutaway::order::depth_sort(vec![
            (seated, "seated"),
            (plant, "plant"),
            (desk_box, "desk"),
        ]);
        assert_eq!(drawn, vec!["desk", "plant", "seated"]);
    }

    /// A back-turned desk's own art is taller only ABOVE the desk: it sorts on
    /// the same base row as the plain desk, so swapping the art moves no depth.
    #[test]
    fn a_back_turned_desk_grows_upward_and_keeps_its_base_row() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 20, y: 30 };
        let north = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let south = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
        assert_ne!(north, south, "the bundled pack ships the raised art");
        let plain = desk_span(&pack, south, desk, RenderScale::ONE).expect("desk");
        let raised = desk_span(&pack, north, desk, RenderScale::ONE).expect("desk_north");
        assert_eq!(raised.depth, plain.depth);
        let ((_, plain_h), (_, north_h)) = (base_size(&pack, south), base_size(&pack, north));
        assert!(
            north_h > plain_h,
            "the back-turned desk's monitor stands above the base desk"
        );
        assert_eq!(plain.y0 - raised.y0, north_h - plain_h);
    }

    /// A pack that draws only its own front sofa gets it flipped for the back
    /// view, never the default pack's back-view art in another style.
    #[test]
    fn a_pack_without_the_back_view_sofa_draws_its_own_front_one_flipped() {
        let mut own = pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.meeting_sofa]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
            &[("one.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        own.merge_from(&test_default_pack());
        let mut order = Vec::new();
        push_sofa(
            &mut order,
            &own,
            crate::layout::Point { x: 10, y: 10 },
            true,
            Tie::FixtureOver,
        );
        assert!(
            matches!(
                order.as_slice(),
                [(
                    _,
                    PieceKind::Prop {
                        art: Art {
                            sprite: "meeting_sofa",
                            flip: Flip::Vertical,
                            ..
                        },
                        ..
                    }
                )]
            ),
            "{order:?}"
        );
    }

    /// A pack without the facing's own art draws the piece it derives from, the
    /// classic painter's rule, rather than no desk.
    #[test]
    fn a_pack_without_the_back_turned_art_draws_the_plain_desk() {
        let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
            &[("one.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        assert_eq!(desk_art(&pack, crate::layout::Facing::North), Some("desk"));
    }

    /// A back-turned sitter and their chair are ONE piece, so nothing can sort
    /// between them: the occupied desk pushes no chair of its own, and the
    /// sitter's box covers the chair's at either phase of the breathing bob.
    #[test]
    fn a_back_turned_sitter_carries_their_own_chair() {
        let pack = test_default_pack();
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let north: Vec<crate::layout::Point> = layout
            .home_desks
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(*i))
                    == crate::layout::Facing::North
            })
            .map(|(_, d)| *d)
            .collect();
        assert!(!north.is_empty(), "the office has back-turned desks");

        let chairs = queued(&layout, &pack, RenderScale::ONE, &north[..1], |k| {
            matches!(k, FixtureKind::DeskChair(_))
        });
        assert_eq!(
            chairs.len(),
            north.len() - 1,
            "the occupied desk's chair rides its sitter"
        );

        let desk = north[0];
        let (chair, _) =
            chair_span(&pack, crate::layout::Facing::North, desk).expect("a north chair");
        for anim in ["typing_back", "seated_back"] {
            let (w, h) = base_size(&pack, anim);
            for bob in [0, 1] {
                let seat = near_seat(desk);
                let body = piece_span(
                    crate::layout::Anchor::TopLeft,
                    crate::layout::Point {
                        x: seat.x,
                        y: seat.y + bob,
                    },
                    w,
                    h,
                    0,
                );
                let depth =
                    crate::layout::desk_walk_anchor_facing(desk, crate::layout::Facing::North).y;
                let piece = occupant_span(body, depth, Some(chair));
                assert!(
                    piece.depth >= chair.depth
                        && piece.depth >= depth
                        && piece.x0 <= chair.x0.min(body.x0)
                        && piece.x1 >= chair.x1.max(body.x1)
                        && piece.y0 <= chair.y0.min(body.y0)
                        && piece.y1 >= chair.y1.max(body.y1),
                    "{anim}, bob {bob}: {piece:?} must cover {body:?} and {chair:?}"
                );
            }
        }
        assert!(
            chair_span(&pack, crate::layout::Facing::South, desk).is_none(),
            "a viewer-facing occupant sits in front of their own chair"
        );

        // A chair wider than its sitter is still inside their piece: a passer-by
        // overlapping only the chair's columns sorts against the sitter too.
        let piece = occupant_span(
            Span::new(10, 10, 4, 6, 0),
            16,
            Some(Span::new(8, 13, 8, 4, 0)),
        );
        assert_eq!((piece.x0, piece.x1), (8, 15));
    }

    /// A desk's shadow centres on the row just under the one it sorts on: the
    /// row it meets the ground on. Pinned exactly, as the ordering tests compare
    /// depths by inequality, which a one-row shift passes.
    #[test]
    fn a_desks_shadow_centres_on_the_row_under_where_it_sorts() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 20, y: 30 };
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let art = desk_art(&pack, facing).expect("desk art");
            for s in [1, pack.max_density_variant().get()] {
                let scale = RenderScale::new(s).expect("nonzero");
                let span = desk_span(&pack, art, desk, scale).expect("desk");
                let kind = PieceKind::Desk {
                    at: desk,
                    art,
                    screen: Screen::Off,
                };
                let shadow = ground_shadow(span, &kind, &pack).expect("a desk casts a shadow");
                let ((_, top), (_, past)) = shadow.bounds();
                assert_eq!((top + past) / 2, span.depth + 1, "{art} at scale {s}");
            }
        }
    }

    /// A standing chair sorts on the classic painter's own chair key.
    #[test]
    fn a_chair_sorts_on_the_classic_chair_key() {
        let pack = test_default_pack();
        let desk = crate::layout::Point { x: 20, y: 30 };
        let (span, _) = chair_span(&pack, crate::layout::Facing::North, desk)
            .expect("a back-turned desk stands a chair");
        assert_eq!(
            span.depth,
            crate::layout::desk_chair_z_key(desk, crate::layout::Facing::North)
        );
    }

    /// A walker sorts on the sim's z-key for them, every step.
    #[test]
    fn a_walker_sorts_on_the_sims_z_key() {
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 0);
        let mut walked = 0;
        for frame in &frames {
            let Some(c) = frame.characters.first().filter(|c| c.seat_desk.is_none()) else {
                continue;
            };
            let mut order = Vec::new();
            push_characters(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme: &crate::theme::NORMAL,
                    scale: RenderScale::ONE,
                },
                std::time::UNIX_EPOCH,
                &mut order,
            );
            let (span, _) = order.first().expect("the walker is drawn");
            assert_eq!(span.depth, c.anchor_y);
            walked += 1;
        }
        assert!(walked > 1, "the fixture never walked, so this pins nothing");
    }

    /// A chair that rises above its sitter's head still lies inside their
    /// piece, which sorts on the later of the two depths.
    #[test]
    fn an_occupant_span_bounds_a_chair_taller_than_its_sitter() {
        let body = Span::new(10, 20, 8, 12, 0);
        let chair = Span::new(9, 15, 10, 20, 0).with_depth(40);
        let piece = occupant_span(body, 31, Some(chair));
        assert_eq!(
            (piece.x0, piece.x1, piece.y0, piece.y1, piece.depth),
            (9, 18, 15, 34, 40)
        );
    }

    /// Relighting recolors the screen KEYS and nothing else — not even a pixel
    /// of another key the same colour as the glass — so the glow is exactly the
    /// screen the art drew, at whatever density.
    #[test]
    fn a_lit_screen_relights_only_the_screen_keys() {
        let glass = crate::embedded_pack::SCREEN_GLASS_KEY;
        let text = crate::embedded_pack::SCREEN_TEXT_KEY;
        let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
            &format!(
                "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\
                 \"{glass}\"=\"#1c2a36\"\n\"{text}\"=\"#34424e\"\n\
                 \"D\"=\"#8b5a2b\"\n\"x\"=\"#1c2a36\"\n\
                 \".\"=\"transparent\"\n\
                 [animations.desk]\nframes=[\"desk.sprite\"]\nframe_ms=100\n"
            ),
            &[("desk.sprite", &format!("@frame 0\n{glass} D x . {text}"))],
        )
        .expect("pack builds");
        let anim = pack.animation("desk").expect("desk");
        let glow = pixtuoid_core::sprite::Rgb {
            r: 40,
            g: 180,
            b: 220,
        };
        let lit = relight_screen(anim.recolorable(0).expect("frame 0"), glow);
        let original = anim.frames()[0].as_slice();
        assert_eq!(
            lit.as_slice(),
            &[
                Some(glow.ramp(SCREEN_GLASS_LEVEL)),
                original[1],
                original[2],
                None,
                Some(glow.ramp(SCREEN_TEXT_LEVEL)),
            ]
        );
        assert_eq!(
            original[2], original[0],
            "the fixture's `x` shares the glass colour"
        );
    }

    /// A lit screen's scanline lights its glass on the column the model names
    /// and nowhere else, at every density the desk art is drawn at: the glass's
    /// columns split evenly among the classic's glass columns.
    #[test]
    fn a_lit_screens_scanline_is_on_the_models_column() {
        let pack = test_default_pack();
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let glow = pixtuoid_core::sprite::Rgb {
            r: 40,
            g: 180,
            b: 220,
        };
        let line = crate::effects::look::scanline_color(glow);
        let cols = crate::layout::SCREEN_GLASS_COLS;
        let n = cols.end() - cols.start() + 1;
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let desk = crate::embedded_pack::densest_frame(&pack, art, 0, scale).expect("the art");
            let w = usize::from(desk.frame.width());
            let glass: Vec<u16> = drawn_in(
                &desk,
                &[
                    crate::embedded_pack::SCREEN_GLASS_KEY,
                    crate::embedded_pack::SCREEN_TEXT_KEY,
                ],
            )
            .iter()
            .enumerate()
            .filter(|&(_, &g)| g)
            .map(|(i, _)| (i % w) as u16)
            .collect();
            let x0 = *glass.iter().min().expect("the art draws glass");
            let band = (glass.iter().max().expect("glass") - x0 + 1) / n;
            for scan in 0..n {
                let lit = Screen::Lit { glow, scan }
                    .on(desk.recolorable)
                    .expect("a lit screen");
                let lined: std::collections::BTreeSet<u16> = (0..lit.height())
                    .flat_map(|y| (0..lit.width()).map(move |x| (x, y)))
                    .filter(|&(x, y)| lit.get(x, y).copied().flatten() == Some(line))
                    .map(|(x, _)| x)
                    .collect();
                assert_eq!(
                    lined,
                    (x0 + scan * band..x0 + (scan + 1) * band).collect(),
                    "at scale {s}, scan {scan} lit other columns"
                );
            }
        }
    }

    /// Each prop stands with its foot on the cell its desk art marks for it, at
    /// every density, the cup at 1x on the classic's own cell, and a sheet one
    /// sheet's fall short of landing hangs that many rows over the tower.
    #[test]
    fn the_desk_props_stand_on_their_marks() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let mut frame = frames.last().expect("a seated frame").clone();
        for d in &mut frame.desks {
            d.cup = Some(crate::sim::Cup::Steaming);
            d.token_tier = crate::token_meter::MAX_TIER;
            d.sheet_fall = Some(1);
        }
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            };
            let list = list_at(&frame, office, 12);
            let pieces = list.pieces();
            let mut stood = 0;
            for (i, p) in pieces.iter().enumerate() {
                let PieceKind::Desk { at, art, .. } = p.kind else {
                    continue;
                };
                let desk = crate::embedded_pack::densest_frame(&pack, art, 0, scale).expect("art");
                let k = desk.blit_at.get();
                let mark = |name: &str| {
                    let m = desk
                        .marks
                        .iter()
                        .find(|m| m.name() == name)
                        .expect("a mark");
                    (
                        scale.to_buffer(p.span.x0) + m.x() * k,
                        scale.to_buffer(p.span.y0) + m.y() * k,
                    )
                };
                // This desk's props follow it in the list: cup, tower, sheet.
                let props: Vec<StoodProp> = pieces[i + 1..]
                    .iter()
                    .map_while(|q| match q.kind {
                        PieceKind::DeskProp(prop) => Some(prop),
                        _ => None,
                    })
                    .collect();
                let [cup, tower, sheet] = props[..] else {
                    panic!("desk at {at:?} stood {props:?}");
                };
                let foot = |prop: StoodProp| {
                    let f =
                        crate::embedded_pack::densest_frame(&pack, prop.sprite, prop.frame, scale)
                            .expect("prop art");
                    let b = f.blit_at.get();
                    (prop.at.0, prop.at.1 + (f.frame.height() - 1) * b, b)
                };
                for (prop, name) in [(cup, "cup"), (tower, "tower")] {
                    let (x, y, b) = foot(prop);
                    let (mx, my) = mark(name);
                    assert_eq!(
                        (x, y),
                        (mx, my + k - b),
                        "at scale {s}, the {name} is off its mark"
                    );
                }
                if s == 1 {
                    assert_eq!(
                        (cup.at.0, cup.at.1),
                        (crate::sim::desk_cup_at(at).x, crate::sim::desk_cup_at(at).y),
                        "the 1x cup is off the classic's cell"
                    );
                }
                let rest = crate::token_meter::SHEET_FALL_PX - 1;
                assert_eq!(
                    (sheet.at.0, sheet.at.1),
                    (tower.at.0, tower.at.1 - rest * s),
                    "at scale {s}, the sheet hangs off its fall"
                );
                stood += 1;
            }
            assert!(stood > 0, "no desk stood its props");
        }
    }

    /// Every key the desk props take a theme colour in is one their art draws,
    /// at every density: a key renamed in the pack would stop the theme
    /// reaching the prop.
    #[test]
    fn the_desk_props_draw_the_keys_the_theme_recolours() {
        let pack = test_default_pack();
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            for (sprite, frame, keys) in [
                (DESK_CUP_SPRITE, 0, &[CUP_KEY, CUP_SHADE_KEY][..]),
                (TOKEN_TOWER_SPRITE, 0, &[PAPER_KEY, PAPER_SHADE_KEY]),
                (TOKEN_SHEET_SPRITE, 0, &[PAPER_KEY]),
            ] {
                let art = crate::embedded_pack::densest_frame(&pack, sprite, frame, scale)
                    .expect("the bundled pack draws the prop");
                for &key in keys {
                    assert!(
                        drawn_in(&art, &[key]).contains(&true),
                        "{sprite} at scale {s} draws no {key:?}"
                    );
                }
            }
        }
    }

    /// Pins the screen keys ([`SCREEN_GLASS_KEY`](crate::embedded_pack::SCREEN_GLASS_KEY),
    /// [`SCREEN_TEXT_KEY`](crate::embedded_pack::SCREEN_TEXT_KEY)) to the bundled
    /// art: the raised desk a back-turned sitter works at must light its glass
    /// and its text at every density the pack draws it, or a key names nothing
    /// and that screen never lights.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_bundled_back_turned_desk_draws_its_screen_in_the_screen_keys() {
        let pack = test_default_pack();
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let sentinel = pixtuoid_core::sprite::Rgb {
            r: 255,
            g: 0,
            b: 255,
        };
        let glass_and_text = [SCREEN_GLASS_LEVEL, SCREEN_TEXT_LEVEL];
        let variants = (2..=pack.max_density_variant().get())
            .filter_map(pixtuoid_core::sprite::format::Density::new)
            .map(|d| pixtuoid_core::sprite::format::density_variant_name(art, d))
            .filter(|n| pack.animation(n).is_some());
        let mut drawn = 0;
        for name in std::iter::once(art.to_string()).chain(variants) {
            let anim = pack.animation(&name).expect("the bundled pack ships it");
            let lit = relight_screen(anim.recolorable(0).expect("frame 0"), sentinel);
            for level in glass_and_text {
                assert!(
                    lit.as_slice().contains(&Some(sentinel.ramp(level))),
                    "{name} has no pixel lit at level {level}"
                );
            }
            drawn += 1;
        }
        assert!(
            drawn > 1,
            "the bundled pack ships a density variant of {art}"
        );
    }

    /// A base desk's derived face is its bottom row's material, and the bundled
    /// legs tie their shadow against their dark inner side: the tie breaks
    /// west-first, onto the shadow wood. An art edit that flips it recolours
    /// every base-art face in the cutaway and nothing else notices.
    #[test]
    fn a_bundled_base_desks_face_is_its_shadow_wood() {
        let pack = test_default_pack();
        let shadow = pack
            .palette()
            .get('d')
            .flatten()
            .expect("`d` is the desk's opaque shadow wood");
        for name in ["desk", "desk_north"] {
            let f = pack
                .animation(name)
                .and_then(|a| a.frames().first())
                .unwrap_or_else(|| panic!("the bundled pack ships {name}"));
            assert_eq!(
                dominant_opaque_row(f, f.height() - 1),
                Some(shadow),
                "{name}'s derived face"
            );
        }
    }

    /// The bundled office with one editing agent homed at its first desk facing
    /// `facing`, observed through the real sim every tick of their walk there:
    /// the frames up to the first where they sit, then `seated_ticks` more, and
    /// that desk.
    pub(crate) fn sit_down(
        facing: crate::layout::Facing,
        seated_ticks: usize,
    ) -> (Layout, Pack, Vec<SimFrame>, crate::layout::Point) {
        sit_down_in(test_default_pack(), facing, seated_ticks)
    }

    /// [`sit_down`] with `pack` drawing the office.
    fn sit_down_in(
        pack: Pack,
        facing: crate::layout::Facing,
        seated_ticks: usize,
    ) -> (Layout, Pack, Vec<SimFrame>, crate::layout::Point) {
        let id = pixtuoid_core::AgentId::from_transcript_path("/cutaway/sit.jsonl");
        sit_down_as(pack, facing, seated_ticks, id)
    }

    /// [`sit_down_in`] with `id` doing the walking.
    fn sit_down_as(
        pack: Pack,
        facing: crate::layout::Facing,
        seated_ticks: usize,
        id: pixtuoid_core::AgentId,
    ) -> (Layout, Pack, Vec<SimFrame>, crate::layout::Point) {
        use crate::floor::{FloorMeta, FloorSession};
        use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex, ToolKind};
        use std::time::{Duration, SystemTime};
        const LOGICAL: (u16, u16) = (160, 96);
        let meta = FloorMeta::ground();
        let layout = Layout::compute_with_seed(LOGICAL.0, LOGICAL.1, None, meta.floor_seed)
            .expect("lays out");
        let home = (0..layout.home_desks.len())
            .find(|&i| layout.desk_facing(FloorLocalDeskIndex(i)) == facing)
            .expect("the office has a desk facing that way");
        let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut scene = pixtuoid_core::SceneState::uniform(16);
        scene.agents.insert(
            id,
            pixtuoid_core::AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from("s"),
                cwd: std::sync::Arc::from(std::path::Path::new("/w/x")),
                label: "x".into(),
                state: ActivityState::Active {
                    tool_use_id: None,
                    detail: None,
                    kind: ToolKind::Edit,
                },
                state_started_at: now0,
                created_at: now0,
                last_event_at: now0,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: pixtuoid_core::GlobalDeskIndex(home),
                floor_idx: 0,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
        let mut session = FloorSession::new();
        let mut frames = Vec::new();
        let mut seated_at = None;
        for n in 1..=1200u64 {
            let frame = session
                .step(
                    crate::floor::FloorInputs {
                        scene: &scene,
                        pack: &pack,
                        now: now0 + Duration::from_millis(100 * n),
                        floor: meta,
                        pets: crate::floor::PetInputs::default(),
                    },
                    crate::layout::Size {
                        w: LOGICAL.0,
                        h: LOGICAL.1,
                    },
                )
                .expect("lays out")
                .frame;
            if seated_at.is_none()
                && frame
                    .seated_agents
                    .get(&FloorLocalDeskIndex(home))
                    .copied()
                    .unwrap_or(false)
            {
                seated_at = Some(frames.len());
            }
            frames.push(frame);
            if seated_at.is_some_and(|at| frames.len() > at + seated_ticks) {
                let desk = layout.home_desks[home];
                return (layout, pack, frames, desk);
            }
        }
        panic!("the agent never sat at their desk");
    }

    /// A back-turned sitter's badge clears the raised monitor behind their head:
    /// its whole plate lands above the desk art's top.
    #[test]
    fn a_back_turned_sitters_badge_clears_their_raised_monitor() {
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
        let seated = frames.last().expect("a seated frame");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme: crate::theme::theme_by_name("normal").expect("theme"),
            scale: RenderScale::new(4).expect("nonzero"),
        };
        let list = frame_list(
            seated,
            office,
            showing(
                crate::floor::FloorMeta::ground(),
                std::time::SystemTime::UNIX_EPOCH,
            ),
        );
        let plate = list
            .pieces()
            .iter()
            .find(|p| matches!(p.kind, PieceKind::Badge { .. }))
            .expect("the sitter has a badge")
            .span;
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let top = desk_span(&pack, art, desk, RenderScale::ONE)
            .expect("desk")
            .y0;
        assert!(plate.y1 < top, "{plate:?} reaches the monitor top at {top}");
    }

    /// Who carries a chair is ONE decision: a sitter skipped for art the pack
    /// lacks carries nothing, so their desk still stands its own chair.
    #[test]
    fn a_sitter_the_pack_cannot_draw_leaves_their_chair_standing() {
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
        let seated = frames.last().expect("a seated frame");
        let mut order = Vec::new();
        assert_eq!(
            push_characters(
                seated,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme: &crate::theme::NORMAL,
                    scale: RenderScale::ONE
                },
                std::time::UNIX_EPOCH,
                &mut order
            ),
            vec![desk]
        );

        let chair_only = pixtuoid_core::sprite::format::load_pack_from_strings(
            &format!(
                "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
                 [animations.{}]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
                crate::embedded_pack::DESK_CHAIR_SPRITE
            ),
            &[("one.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        let mut order = Vec::new();
        let carried = push_characters(
            seated,
            Office {
                layout: &layout,
                pack: &chair_only,
                theme: &crate::theme::NORMAL,
                scale: RenderScale::ONE,
            },
            std::time::UNIX_EPOCH,
            &mut order,
        );
        assert!(carried.is_empty() && order.is_empty(), "no character art");
        order.extend(queued(
            &layout,
            &chair_only,
            RenderScale::ONE,
            &carried,
            |k| matches!(k, FixtureKind::DeskChair(_)),
        ));
        assert!(
            order
                .iter()
                .any(|(_, k)| matches!(k, PieceKind::Chair { at } if Some(*at)
                    == crate::layout::desk_chair_top_left(desk, crate::layout::Facing::North))),
            "the undrawn sitter's desk lost its chair"
        );
    }

    /// Whether desk `desk`'s chair draws over `frame`'s one person — riding their
    /// piece once they sit, or as its own piece before — or `None` where the two
    /// do not overlap, so their order shows nothing.
    fn chair_over_person(
        frame: &SimFrame,
        layout: &Layout,
        pack: &Pack,
        desk: crate::layout::Point,
    ) -> Option<bool> {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let order = collect_pieces(
            frame,
            Office {
                layout,
                pack,
                theme,
                scale: RenderScale::ONE,
            },
            &Moment::resolve(
                crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                theme,
                0.0,
                std::time::UNIX_EPOCH,
            ),
        );
        let (person, person_span) = order
            .iter()
            .enumerate()
            .find_map(|(i, (s, k))| matches!(k, PieceKind::Character { .. }).then_some((i, *s)))?;
        if let (_, PieceKind::Character { chair: Some(_), .. }) = &order[person] {
            return Some(true);
        }
        let (at, chair_span) =
            crate::layout::desk_chair_top_left(desk, crate::layout::Facing::North)
                .zip(chair_span(pack, crate::layout::Facing::North, desk).map(|(s, _)| s))?;
        let chair = order
            .iter()
            .position(|(_, k)| matches!(k, PieceKind::Chair { at: a } if *a == at))?;
        let overlap = person_span.x0 <= chair_span.x1
            && chair_span.x0 <= person_span.x1
            && person_span.y0 <= chair_span.y1
            && chair_span.y0 <= person_span.y1;
        if !overlap {
            return None;
        }
        let drawn = crate::cutaway::order::depth_sort(
            order
                .iter()
                .enumerate()
                .map(|(i, (s, _))| (*s, i))
                .collect(),
        );
        let pos = |i: usize| drawn.iter().position(|&j| j == i);
        Some(pos(chair)? > pos(person)?)
    }

    /// The chair draws over its occupant through the settle arc — every frame
    /// the sim keys them at their seat — as in the classic painter: keyed on its
    /// own box, it would sort behind them until they sat and jump in front the
    /// frame they did.
    #[test]
    fn a_chair_keeps_its_order_to_its_sitter_through_the_settle() {
        use crate::layout::Facing;
        let (layout, pack, frames, desk) = sit_down(Facing::North, 0);
        let seat_key = crate::layout::desk_chair_z_key(desk, Facing::North);
        let orders: Vec<(usize, bool)> = frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.characters.first().is_some_and(|c| c.anchor_y == seat_key))
            .filter_map(|(n, f)| chair_over_person(f, &layout, &pack, desk).map(|o| (n, o)))
            .collect();
        assert!(
            orders.len() > 1,
            "the sitter was never keyed at their seat before sitting"
        );
        assert!(
            orders.iter().all(|&(_, over)| over),
            "the chair flipped under its sitter at frames {:?}",
            orders
                .iter()
                .filter(|(_, o)| !o)
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
        );
    }

    /// A person sorts on the sim's own key, so a viewer-facing sitter's depth
    /// holds while their breath moves their drawn box.
    #[test]
    fn a_sitters_depth_holds_through_their_breath() {
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::South, 60);
        let (mut depths, mut tops) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for frame in frames.iter().filter(|f| {
            f.characters
                .first()
                .is_some_and(|c| c.seat_desk == Some(desk))
        }) {
            let mut order = Vec::new();
            push_characters(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme: &crate::theme::NORMAL,
                    scale: RenderScale::ONE,
                },
                std::time::UNIX_EPOCH,
                &mut order,
            );
            let (span, _) = order.first().expect("the sitter is drawn");
            depths.insert(span.depth);
            tops.insert(span.y0);
        }
        assert!(
            tops.len() > 1,
            "the sitter never breathed, so this pins nothing: {tops:?}"
        );
        assert_eq!(
            depths.len(),
            1,
            "their depth moved with their breath: {depths:?}"
        );
    }

    /// The seat side is the LAYOUT's to decide, and both profiles read it.
    #[test]
    fn both_profiles_seat_an_occupant_on_the_side_the_layout_chose() {
        use crate::layout::{CHARACTER_SPRITE_W, Facing};
        let desk = crate::layout::Point { x: 40, y: 30 };
        let near = crate::sim::seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::North);
        let far = crate::sim::seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::South);
        assert_eq!(
            near.y, desk.y,
            "a back-turned occupant's shared anchor lands on desk.y"
        );
        assert!(
            far.y < near.y,
            "a viewer-facing occupant sits BEHIND the desk, a back-turned one in \
             front: far {far:?}, near {near:?}"
        );
        assert_eq!(far.x, near.x, "the seat side never moves the centring");
    }

    /// The badge follows the CUTAWAY's body, not the classic one:
    /// `overlay::build_overlay` hangs off the classic-drawn sprite, which for a
    /// seated agent is not where the cutaway draws them.
    #[test]
    fn a_label_anchor_sits_above_the_head_and_centred_on_the_sprite() {
        let at = crate::layout::Point { x: 10, y: 20 };
        let anchor = label_anchor(at, 8, None);
        assert_eq!(anchor.x, at.x + 4, "centred on the sprite");
        assert_eq!(at.y - anchor.y, LABEL_GAP, "clear of the head");
    }

    /// A plate's runs and the board's segments step on one grid: the run after
    /// `n` characters starts [`columns`](crate::cutaway::text::columns)`(n)` on.
    #[test]
    fn plate_runs_and_board_columns_share_one_grid() {
        use crate::board::{BoardSegment, BoardTone};
        use crate::cutaway::text::columns;
        use pixtuoid_core::sprite::Rgb;
        let pen = Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4");
        let (first, second) = ("Iab", "I");
        let mut board = quiet_board().clone();
        board.mood = [first, second]
            .map(|text| BoardSegment {
                text: text.into(),
                tone: BoardTone::Idle,
            })
            .to_vec();
        // Brand, star, then the mood line's segments.
        let runs = board_runs(&board, pen);
        let ((x0, _), _) = runs[2];
        let ((x1, _), _) = runs[3];
        assert_eq!(x1.0 - x0.0, columns(3).0, "the board");
        let (a, b) = (Rgb { r: 255, g: 0, b: 0 }, Rgb { r: 0, g: 255, b: 0 });
        let mut buf = RgbBuffer::filled(64, 16, Rgb { r: 0, g: 0, b: 0 });
        let plate = ArtRect {
            x: ArtPx(0),
            y: ArtPx(0),
            w: ArtPx(40),
            h: ArtPx(PLATE_H),
        };
        paint_plate(
            pen,
            &mut buf,
            plate,
            Rgb { r: 1, g: 1, b: 1 },
            &[(first, a), (second, b)],
        );
        // An `I`'s top bar spans its whole cell, so its first ink is its run's start.
        let left =
            |ink| (0..buf.width()).find(|&x| (0..buf.height()).any(|y| buf.get(x, y) == ink));
        let (la, lb) = (
            left(a).expect("the first run"),
            left(b).expect("the second run"),
        );
        assert_eq!(lb - la, columns(3).0, "the plate");
    }

    /// At the pack's 4x art the board writes inside the neon sign's dark
    /// interior, as the classic's terminal board does, however full its lines.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_board_writes_inside_the_signs_interior() {
        use crate::layout::{
            NEON_PANEL_INNER_H, NEON_PANEL_INNER_W, NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y,
        };
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let counts = crate::board::StateCounts {
            waiting: 12,
            active: 34,
            idle: 56,
            exiting: 0,
            total: 102,
        };
        let gateway = Some(pixtuoid_core::state::DaemonState::Degraded);
        for ms in (0..16_000).step_by(100) {
            let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
            let board = crate::board::build_board(counts, 99 * 3_600, Some((12, 12)), gateway, now);
            let span = board_span(&board, Pen::for_pack(scale, &pack));
            assert!(
                span.x0 >= NEON_PANEL_INNER_X
                    && span.x1 < NEON_PANEL_INNER_X + NEON_PANEL_INNER_W
                    && span.y0 >= NEON_PANEL_INNER_Y
                    && span.y1 < NEON_PANEL_INNER_Y + NEON_PANEL_INNER_H,
                "{span:?} at +{ms}ms"
            );
        }
    }

    /// A ceiling ABOVE the head lifts the badge clear of it; one below the head
    /// changes nothing.
    #[test]
    fn a_label_anchor_clears_a_ceiling_above_the_head() {
        let at = crate::layout::Point { x: 10, y: 20 };
        let free = label_anchor(at, 8, None);
        let raised = label_anchor(at, 8, Some(at.y - 4));
        assert_eq!(
            raised.y,
            at.y - 4 - LABEL_GAP,
            "the badge clears the monitor top by the same gap it clears a head by"
        );
        assert_eq!(raised.x, free.x);
        assert_eq!(label_anchor(at, 8, Some(at.y + 4)), free);
    }

    /// The layout leaves walkable rows between the wall band and `top_margin`.
    #[test]
    fn the_wall_band_stops_where_the_layout_says_the_floor_begins() {
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let band_h = layout.wall_band_h();
        assert!(band_h > 0, "a laid-out office has a wall band");
        assert!(
            band_h < layout.top_margin,
            "the band must end ABOVE top_margin, leaving walkable rows: \
             band {band_h}, top_margin {}",
            layout.top_margin
        );
    }

    /// The floor is tiled on its own grid, from the wall's foot: a seam sits
    /// under the tile beside it, on the lit floor and the dark alike.
    #[test]
    fn the_floor_is_tiled_from_the_wall_foot() {
        let layout = Layout::compute_with_seed(160, 110, None, 0).expect("lays out");
        assert_ne!(
            layout.wall_band_h() % GROUND_TILE,
            0,
            "a wall foot off the buffer's own grid, or one anchored at row 0 passes too"
        );
        let s = 8;
        let (pen, buf) = floor(&layout, s, 4);
        let luma = |x: u16, y: u16| buf.get(x, y).lightness();
        let k = s / 4;
        let tile = pen.art(GROUND_TILE).0 * k;
        let ground_top = layout.wall_band_h() * s;
        let seam = tile * 3;
        let mid = seam + tile / 2;
        assert!(
            luma(mid, ground_top) < luma(mid, ground_top + k),
            "the first seam runs along the wall's foot, one art pixel deep"
        );
        for y in [ground_top + tile + tile / 2, buf.height() - tile / 2] {
            assert!(
                luma(seam, y) < luma(seam + tile / 2, y),
                "row {y}: the seam at column {seam} must sit under its tile"
            );
        }
    }

    /// At 1x a seam every tile would be a quarter of the ground, so there are
    /// none: the lit zone is one flat tone.
    #[test]
    fn a_1x_floor_has_no_seams() {
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let (_, buf) = floor(&layout, 1, 1);
        let y = layout.wall_band_h() + 1;
        assert!(
            (0..buf.width()).all(|x| buf.get(x, y) == crate::theme::NORMAL.surface.carpet_light),
            "row {y} of the lit zone is unbroken"
        );
    }

    /// The floor alone at scale `s`, drawn from art at density `d`, in the
    /// normal theme.
    fn floor(layout: &Layout, s: u16, d: u16) -> (Pen, RgbBuffer) {
        let scale = RenderScale::new(s).expect("nonzero");
        let pen = Pen::new(scale, d).expect("d divides s");
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
        );
        paint_ground(layout, &crate::theme::NORMAL, pen, &mut buf);
        (pen, buf)
    }

    /// Every rug in the office lies on the ground, the lounge's among them.
    #[test]
    fn every_rug_lies_on_the_floor() {
        let pack = test_default_pack();
        let theme = &crate::theme::NORMAL;
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let f = &theme.furniture;
        let (mut trios, mut lounges) = (0, 0);
        for (w, h) in [(160, 96), (200, 120), (240, 144), (480, 270)] {
            for seed in 0..3 {
                let layout = Layout::compute_with_seed(w, h, None, seed).expect("lays out");
                trios += layout
                    .meeting_rooms
                    .iter()
                    .filter(|r| r.trio.is_some())
                    .count();
                lounges += usize::from(layout.lounge.is_some());
                let mut buf = RgbBuffer::filled(
                    scale.to_buffer(layout.buf_w),
                    scale.to_buffer(layout.buf_h),
                    pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
                );
                paint_backdrop(&layout, theme, scale, Pen::for_pack(scale, &pack), &mut buf);
                let rugs = layout.fixtures().filter_map(|f| {
                    matches!(
                        f.kind,
                        FixtureKind::MeetingRug { .. } | FixtureKind::LoungeRug
                    )
                    .then_some(f.visual)
                });
                for rug in rugs {
                    let c = buf.get(
                        scale.to_buffer(rug.x + rug.width / 2),
                        scale.to_buffer(rug.y + rug.height / 2),
                    );
                    assert!(
                        c == f.rug_field || c == f.rug_field.ramp(RUG_MOTIF_LEVEL),
                        "{w}x{h} seed {seed}: the rug at {rug:?} is woven, not floor: {c:?}"
                    );
                }
            }
        }
        assert!(
            trios > 0 && lounges > 0,
            "the sweep lays meeting rooms and a lounge"
        );
    }

    /// At 1x the cutaway's rug is the classic painter's, cell for cell, the
    /// lounge rug's short side included.
    #[test]
    fn a_1x_rug_is_the_classic_rug() {
        let theme = &crate::theme::NORMAL;
        let pen = Pen::new(RenderScale::new(1).expect("nonzero"), 1).expect("1 divides 1");
        for (width, height) in [(18, 24), (22, 7), (3, 3)] {
            let rug = crate::layout::Bounds {
                x: 2,
                y: 2,
                width,
                height,
            };
            let blank =
                || RgbBuffer::filled(32, 32, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
            let (mut cutaway, mut classic) = (blank(), blank());
            paint_rug(rug, theme, pen, &mut cutaway);
            crate::pixel_painter::paint_area_rug(&mut classic, rug, theme);
            assert!(
                cutaway.as_slice() == classic.as_slice(),
                "the {width}x{height} rug"
            );
        }
    }

    /// A 12x12 floor, its west half one tone and its east half another, under
    /// `shadows` at noon, drawn at scale `s` from art at density `d`.
    fn shadowed(s: u16, d: u16, shadows: &[crate::ground::Contact]) -> RgbBuffer {
        let scale = RenderScale::new(s).expect("nonzero");
        let pen = Pen::new(scale, d).expect("d divides s");
        let mut buf = RgbBuffer::filled(scale.to_buffer(12), scale.to_buffer(12), WEST);
        for y in 0..buf.height() {
            for x in buf.width() / 2..buf.width() {
                buf.put(x, y, EAST);
            }
        }
        paint_ground_shadows(
            shadows.iter().copied(),
            crate::ground::shadow_strength(NOON_DARKNESS),
            pen,
            &mut buf,
        );
        buf
    }

    const WEST: pixtuoid_core::sprite::Rgb = pixtuoid_core::sprite::Rgb {
        r: 150,
        g: 110,
        b: 72,
    };
    const EAST: pixtuoid_core::sprite::Rgb = pixtuoid_core::sprite::Rgb {
        r: 70,
        g: 90,
        b: 140,
    };
    /// A shadow centred on the seam at `(6, 6)`.
    fn seam_shadow() -> crate::ground::Contact {
        crate::ground::Contact::under(2, 8, 6)
    }

    /// A shadow short of one whole stop still darkens the ground: the falloff
    /// rounds to its nearest stop, where flooring would drop it.
    #[test]
    fn a_shadow_short_of_one_stop_still_darkens_the_floor() {
        let pen = Pen::new(RenderScale::ONE, 1).expect("d divides s");
        let mut buf = RgbBuffer::filled(12, 12, WEST);
        paint_ground_shadows(
            std::iter::once(seam_shadow()),
            0.6 / SHADOW_STOPS_PER_STRENGTH,
            pen,
            &mut buf,
        );
        assert!(buf.as_slice().iter().any(|&p| p != WEST));
    }

    /// A shadow is the ground it falls on, darker toward its centre: whole ramp
    /// stops of that floor's own colour, never a colour of its own.
    #[test]
    fn a_shadow_steps_the_floor_it_falls_on_darker_toward_its_centre() {
        let buf = shadowed(8, 4, &[seam_shadow()]);
        let deepest = deepest_shadow_stop();
        let (cx, cy) = (6 * 8, 6 * 8);
        assert_eq!(buf.get(cx - 2, cy), WEST.ramp(-deepest), "west of the seam");
        assert_eq!(
            buf.get(cx + 2, cy),
            EAST.ramp(-deepest),
            "east of it, its own tone"
        );
        assert_eq!(buf.get(0, 0), WEST, "nothing past the rim");
        let stepped =
            |c, floor: pixtuoid_core::sprite::Rgb| (0..=deepest).any(|k| c == floor.ramp(-k));
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                let floor = if x < buf.width() / 2 { WEST } else { EAST };
                assert!(
                    stepped(buf.get(x, y), floor),
                    "({x}, {y}) is its floor, stepped"
                );
            }
        }
    }

    /// Two shadows over one spot are as deep as the deeper, not their sum, in
    /// either order.
    #[test]
    fn overlapping_shadows_take_the_deeper_not_the_sum() {
        let (narrow, wide) = (
            crate::ground::Contact::under(4, 3, 6),
            crate::ground::Contact::under(1, 10, 7),
        );
        let (a, b) = (shadowed(8, 4, &[narrow]), shadowed(8, 4, &[wide]));
        let (ab, ba) = (
            shadowed(8, 4, &[narrow, wide]),
            shadowed(8, 4, &[wide, narrow]),
        );
        assert!(ab.as_slice() == ba.as_slice(), "order-free");
        let lum = pixtuoid_core::sprite::Rgb::lightness;
        let mut mixed = false;
        for ((&both, &a), &b) in ab.as_slice().iter().zip(a.as_slice()).zip(b.as_slice()) {
            let deeper = if lum(a) <= lum(b) { a } else { b };
            assert_eq!(both, deeper, "the deeper of the two, not their sum");
            mixed |= a != b && both == a;
        }
        assert!(mixed, "each shadow wins somewhere");
    }

    /// Every step of a shadow covers whole art pixels.
    #[test]
    fn a_shadow_lands_on_the_art_grid() {
        let buf = shadowed(8, 4, &[seam_shadow()]);
        let k = 8 / 4;
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), buf.get(x - x % k, y - y % k), "({x}, {y})");
            }
        }
    }

    /// A solid casts its shadow under its south edge; a prop's upper band does
    /// not.
    #[test]
    fn what_meets_the_floor_casts_a_shadow() {
        let pack = test_default_pack();
        let span = Span::new(10, 10, 8, 12, 0);
        let chair = PieceKind::Chair {
            at: crate::layout::Point { x: 10, y: 10 },
        };
        assert_eq!(
            ground_shadow(span, &chair, &pack),
            Some(crate::ground::Contact::under(10, 8, span.y1 + 1)),
            "centred under it, on the ground row under its south edge"
        );
        let (_, h) = art_size(&pack, MEETING_SOFA_NORTH).expect("sofa art");
        let band = |rows| PieceKind::PropBand {
            at: crate::layout::Point { x: 10, y: 10 },
            sprite: MEETING_SOFA_NORTH,
            rows,
        };
        assert!(
            ground_shadow(span, &band((0, NORTH_SOFA_SEAT_ROWS)), &pack).is_none(),
            "the seat band"
        );
        assert!(
            ground_shadow(span, &band((NORTH_SOFA_SEAT_ROWS, h)), &pack).is_some(),
            "the foot band"
        );
    }

    /// A figure standing casts its own shadow; sitting, the chair they carry
    /// casts one under itself, so a chair's shadow stays put as they sit down.
    #[test]
    fn a_sitters_chair_casts_the_shadow_they_do_not() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let (mut standing, mut sitting) = (false, false);
        for frame in &frames {
            for (span, kind) in collect_pieces(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale: RenderScale::ONE,
                },
                &Moment::resolve(
                    crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                    theme,
                    0.0,
                    std::time::UNIX_EPOCH,
                ),
            ) {
                let PieceKind::Character {
                    ref figure,
                    body,
                    chair,
                    ..
                } = kind
                else {
                    continue;
                };
                let cast = ground_shadow(span, &kind, &pack).expect("every figure grounds");
                if figure.shadow {
                    standing = true;
                    assert_eq!(
                        cast,
                        crate::ground::Contact::under(body.x0, body.x1 - body.x0 + 1, body.y1 + 1)
                    );
                } else {
                    sitting = true;
                    let at = chair.expect("a desk sitter carries their chair");
                    let (w, h) = art_size(&pack, crate::embedded_pack::DESK_CHAIR_SPRITE)
                        .expect("chair art");
                    let empty = piece_span(crate::layout::Anchor::TopLeft, at, w, h, 0);
                    assert_eq!(
                        Some(cast),
                        ground_shadow(empty, &PieceKind::Chair { at }, &pack),
                        "the chair casts the shadow it cast empty"
                    );
                }
            }
        }
        assert!(standing && sitting, "the walk in and the sit");
    }

    /// A pack with no back-view art flips its front view, over the sitters.
    #[test]
    fn a_flipped_sofa_sorts_in_front_of_its_sitters() {
        let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
             \"F\"=\"#202020\"\n[animations.meeting_sofa]\nframes=[\"s.sprite\"]\nframe_ms=100\n",
            &[("s.sprite", "@frame 0\nF F F\nF F F\n")],
        )
        .expect("a pack of one sofa loads");
        let at = crate::layout::Point { x: 40, y: 30 };
        let mut order = Vec::new();
        push_sofa(&mut order, &pack, at, true, Tie::FixtureOver);
        let [
            (
                span,
                PieceKind::Prop {
                    art:
                        Art {
                            flip: Flip::Vertical,
                            ..
                        },
                    ..
                },
            ),
        ] = order.as_slice()
        else {
            panic!("a flipped sofa is one mirrored prop: {order:?}");
        };
        let sitter = Span::new(at.x, at.y, 1, 1, 0)
            .with_depth(crate::sim::seat::sofa_sitter_z_key(at))
            .with_layer(Layer::Figure);
        assert_eq!(
            crate::cutaway::order::depth_sort(vec![(*span, "sofa"), (sitter, "sitter")]),
            ["sitter", "sofa"]
        );
    }

    /// Every piece's shadow falls inside its [`Piece::reach`], so a repaint of
    /// the pieces whose reach meets a damaged rect leaves no stale shadow.
    #[test]
    fn a_shadow_falls_inside_its_pieces_reach() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let floor = pixtuoid_core::sprite::Rgb {
            r: 150,
            g: 110,
            b: 72,
        };
        let list = build_list(
            frames.last().expect("a frame"),
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            },
            &Moment::resolve(
                crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                theme,
                0.0,
                std::time::UNIX_EPOCH,
            ),
            0,
            quiet_board(),
        );
        let mut cast = 0;
        for piece in list.pieces() {
            let Some(shadow) = piece.shadow else { continue };
            cast += 1;
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                floor,
            );
            paint_ground_shadows(
                std::iter::once(shadow),
                crate::ground::shadow_strength(NOON_DARKNESS),
                Pen::for_pack(scale, &pack),
                &mut buf,
            );
            let reach = piece.reach();
            for (i, &c) in buf.as_slice().iter().enumerate() {
                if c != floor {
                    let w = usize::from(buf.width());
                    let (x, y) = (scale.logical((i % w) as u16), scale.logical((i / w) as u16));
                    assert!(
                        (reach.x0..=reach.x1).contains(&x) && (reach.y0..=reach.y1).contains(&y),
                        "{:?}'s shadow reaches ({x}, {y}) past {reach:?}",
                        kind_name(&piece.kind)
                    );
                }
            }
        }
        assert!(cast > 0, "the office casts shadows");
    }

    /// A frame of an office nobody is in, its room lights full and its sign
    /// calm.
    pub(crate) fn empty_frame(layout: &Layout) -> SimFrame {
        SimFrame {
            agents: Vec::new(),
            poses: std::collections::HashMap::new(),
            seated_agents: std::collections::HashMap::new(),
            characters: Vec::new(),
            indoor_scale: 1.0,
            neon: crate::floor::NeonLevels::CALM,
            chitchat_bubbles: Vec::new(),
            new_coffee_carriers: Vec::new(),
            occupied_waypoints: Default::default(),
            pet: None,
            mascots: Vec::new(),
            desks: vec![Default::default(); layout.home_desks.len()],
            door_frame: 0,
        }
    }

    /// The pieces the fixtures `keep` picks queue ([`push_fixture`]) in an
    /// empty office.
    fn queued(
        layout: &Layout,
        pack: &Pack,
        scale: RenderScale,
        carried: &[crate::layout::Point],
        keep: impl Fn(FixtureKind) -> bool,
    ) -> Vec<(Span, PieceKind)> {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let frame = empty_frame(layout);
        let now = std::time::UNIX_EPOCH;
        let moment = Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now);
        let build = Build {
            frame: &frame,
            office: Office {
                layout,
                pack,
                theme,
                scale,
            },
            moment: &moment,
        };
        let mut order = Vec::new();
        for f in layout.fixtures().filter(|f| keep(f.kind)) {
            push_fixture(f, build, carried, &mut order);
        }
        order
    }

    fn near_seat(desk: crate::layout::Point) -> crate::layout::Point {
        crate::sim::seated_anchor_facing(
            desk,
            crate::layout::CHARACTER_SPRITE_W,
            crate::layout::Facing::North,
        )
    }

    fn base_size(pack: &Pack, name: &str) -> (u16, u16) {
        let f = pack
            .animation(name)
            .and_then(|a| a.frames().first())
            .expect("the bundled pack has this piece");
        (f.width(), f.height())
    }

    /// The windows stand where the layout tiles them, as the classic painter's
    /// do, and their glass shows the one city over the classic's sky — its
    /// disc and stars included — art pixel for art pixel, the frame untouched.
    #[test]
    fn the_windows_look_out_on_the_one_city_where_the_layout_tiles_them() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let d = pen.art(1).0;
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let rows = crate::layout::window_rows(layout.wall_band_h());
        let window_h = rows.end - rows.start;
        let glass_h = crate::layout::glass_rows(window_h);
        let run = crate::layout::window_run(layout.buf_w);
        let k = scale.get() / d;
        // Noon, the sun at dusk, a full moon up, a new moon down.
        for (day, hour) in [(1, 12), (2, 18), (2, 22), (17, 0)] {
            let now = crate::localclock::on_day(day, hour);
            // Clear, so no weather lies over the city and the sky.
            let moment = moment_at(crate::sky::Weather::Clear, now);
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                theme.surface.bg_fallback,
            );
            paint_backdrop(&layout, theme, scale, pen, &mut buf);
            let mut order = Vec::new();
            push_windows(office, &moment, &GlassWeather::of(&moment), &mut order);
            let mut again = Vec::new();
            push_windows(office, &moment, &GlassWeather::of(&moment), &mut again);
            let prints = |o: &[(Span, PieceKind)]| -> Vec<u64> {
                o.iter().map(|(_, kind)| fingerprint(kind)).collect()
            };
            assert_eq!(prints(&order), prints(&again), "one moment, one view");
            assert_eq!(
                order.len(),
                layout.window_bays().count(),
                "a glass piece a window"
            );
            for (span, kind) in &order {
                let PieceKind::Glass { view } = kind else {
                    panic!("a window is glass: {kind:?}");
                };
                assert_eq!(span.depth, 0, "glass sorts at the very back");
                paint_glass(view, pen, &mut buf);
            }
            let city = crate::skyline::CityStrip::draw(
                &pack,
                (run.end - run.start, glass_h),
                &moment,
                theme,
                pixtuoid_core::sprite::format::Density::new(d).expect("nonzero"),
            );
            let sky =
                crate::celestial::SkyView::of(&moment, layout.buf_w, layout.wall_band_h(), theme);
            let at = |ax: u16, ay: u16| buf.get(ax * k, ay * k);
            let (mut glass, mut buildings) = (0, 0);
            for bay in layout.window_bays() {
                let pane = sky.pane(bay.x, bay.w, glass_h, d);
                let size = Size {
                    w: bay.w,
                    h: window_h,
                };
                for ay in pen.art(rows.start).0..pen.art(rows.end).0 {
                    for ax in pen.art(bay.x).0..pen.art(bay.span().end).0 {
                        let (dx, dy) = (ax / d - bay.x, ay / d - rows.start);
                        let here = format!("{day}/{hour}h ({ax}, {ay})");
                        if crate::layout::window_frame(dx, dy, size) {
                            assert_eq!(at(ax, ay), theme.surface.window_frame, "frame {here}");
                            continue;
                        }
                        glass += 1;
                        let cy = ay - pen.art(rows.start + 1).0;
                        match city.at(ax - pen.art(run.start).0, cy) {
                            Some(c) => {
                                buildings += 1;
                                assert_eq!(at(ax, ay), c, "the city {here}");
                            }
                            None => {
                                let open = pane.colour((ax, ay), cy);
                                let open = sky.blaze().map_or(open, |b| b.over(open));
                                assert_eq!(at(ax, ay), open, "sky {here}");
                            }
                        }
                    }
                }
            }
            assert!(glass > 0 && buildings > 0, "windows, and a city in them");
        }
    }

    /// The glass `push_windows` queues for `layout` at `moment` under
    /// `weather`, at the densest scale.
    fn glass_views(layout: &Layout, moment: &Moment, weather: &GlassWeather) -> Vec<WindowView> {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let mut order = Vec::new();
        push_windows(
            Office {
                layout,
                pack: &pack,
                theme: &crate::theme::NORMAL,
                scale,
            },
            moment,
            weather,
            &mut order,
        );
        order
            .into_iter()
            .map(|(_, kind)| match kind {
                PieceKind::Glass { view } => view,
                other => panic!("a window is glass: {other:?}"),
            })
            .collect()
    }

    fn moment_at(w: crate::sky::Weather, now: std::time::SystemTime) -> Moment {
        Moment::resolve(
            crate::sky::Sky::at_with(now, w),
            &crate::theme::NORMAL,
            0.0,
            now,
        )
    }

    /// Against the same sky under a clear model, a weather changes only glass
    /// pixels — never a frame cell, which stays the backdrop's — and every one
    /// that veils or falls changes some.
    #[test]
    fn the_weather_shows_on_the_glass_and_only_there() {
        use crate::sky::Weather;
        for (w, h) in [(160, 96), (240, 135)] {
            let layout = Layout::compute_with_seed(w, h, None, 0).expect("lays out");
            for hour in [12, 0] {
                let now = crate::localclock::at_hour(hour);
                let clear = GlassWeather::of(&moment_at(Weather::Clear, now));
                for weather in Weather::ALL {
                    let moment = moment_at(weather, now);
                    let shown = glass_views(&layout, &moment, &GlassWeather::of(&moment));
                    let crisp = glass_views(&layout, &moment, &clear);
                    let mut changed = 0;
                    for (a, b) in shown.iter().zip(&crisp) {
                        for (pa, pb) in a.px.iter().zip(&b.px) {
                            assert_eq!(pa.is_some(), pb.is_some(), "{weather:?} on a frame");
                            changed += usize::from(pa != pb);
                        }
                    }
                    let shows = weather != Weather::Clear;
                    assert_eq!(changed > 0, shows, "{weather:?} at {w}x{h} {hour}h");
                }
            }
        }
    }

    /// The canvas repaints a window only when its fingerprint moves: over one
    /// sky, the glass weather's tick moves it where something falls and
    /// nowhere else, and one key always gives one fingerprint.
    #[test]
    fn a_window_s_fingerprint_moves_with_the_weather_tick_where_it_falls() {
        use crate::sky::Weather;
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let at = |w, ms| {
            moment_at(
                w,
                crate::localclock::at_hour(12) + std::time::Duration::from_millis(ms),
            )
        };
        for w in Weather::ALL {
            let moment = at(w, 5_000);
            let prints = |ms| -> Vec<u64> {
                glass_views(&layout, &moment, &GlassWeather::of(&at(w, ms)))
                    .into_iter()
                    .map(|view| fingerprint(&PieceKind::Glass { view }))
                    .collect()
            };
            assert_eq!(prints(5_000), prints(5_000), "{w:?}: one key");
            let falls = matches!(
                w,
                Weather::Rain | Weather::Storm | Weather::Snow | Weather::Windy
            );
            assert_eq!(prints(5_000) != prints(5_600), falls, "{w:?}");
        }
    }

    #[test]
    fn the_wall_between_two_windows_is_one_frame_post() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            theme.surface.bg_fallback,
        );
        paint_backdrop(&layout, theme, scale, pen, &mut buf);
        let k = scale.get() / pen.art(1).0;
        let rows = crate::layout::window_rows(layout.wall_band_h());
        let mut posts = 0;
        for post in crate::layout::window_posts(layout.buf_w) {
            posts += 1;
            for x in post.clone() {
                for y in rows.clone() {
                    let (ax, ay) = (pen.art(x).0, pen.art(y).0);
                    assert_eq!(
                        buf.get(ax * k, ay * k),
                        theme.surface.window_frame,
                        "post {post:?} at ({x}, {y})"
                    );
                }
            }
        }
        assert!(posts > 0, "this wall has posts");
    }

    /// A REAL office's draw list, checked against every pairwise "must be
    /// behind" fact its own geometry states — what a sort key cannot give you.
    #[test]
    fn a_real_offices_draw_list_satisfies_every_ordering_constraint() {
        let pack = test_default_pack();
        for (w, h) in [(160u16, 96u16), (240, 144), (100, 60)] {
            let layout = Layout::compute_with_seed(w, h, None, 0).expect("lays out");
            let mut order = queued(&layout, &pack, RenderScale::ONE, &[], |_| true);
            wall_segments(&layout, &mut order);
            assert!(order.len() > 10, "{w}x{h} produced a trivial list");

            let spans: Vec<Span> = order.iter().map(|(s, _)| *s).collect();
            let tagged: Vec<(Span, usize)> = spans.iter().copied().zip(0..).collect();
            let produced = crate::cutaway::order::depth_sort(tagged);
            assert_eq!(produced.len(), spans.len(), "{w}x{h} dropped a piece");
            assert_eq!(
                crate::cutaway::order::check_order(&spans, &produced),
                None,
                "{w}x{h}: the draw list violates a constraint its geometry states"
            );
        }
    }

    /// Pins [`Span`]'s bounds contract for every piece kind and prop builder. A
    /// pixel counts as WRITTEN where two paints over different fills agree, so
    /// no colour is assumed to be one the paint never uses.
    #[test]
    fn every_piece_paints_only_inside_its_span() {
        use crate::floor::{FloorMeta, FloorSession};
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (mut kinds, mut props) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        let mut check = |pack: &Pack, frame: &SimFrame, layout: &Layout, only_people: bool| {
            for s in [1, 3, pack.max_density_variant().get()] {
                let scale = RenderScale::new(s).expect("nonzero");
                let office = Office {
                    layout,
                    pack,
                    theme,
                    scale,
                };
                let moment = Moment::resolve(
                    crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                    theme,
                    0.0,
                    std::time::UNIX_EPOCH,
                );
                for (span, kind) in collect_pieces(frame, office, &moment)
                    .into_iter()
                    .chain(signs(office, 0, quiet_board()))
                {
                    if only_people
                        && !matches!(
                            kind,
                            PieceKind::Character { .. }
                                | PieceKind::Effect(_)
                                | PieceKind::Badge { .. }
                        )
                    {
                        continue;
                    }
                    kinds.insert(kind_name(&kind));
                    if let PieceKind::Prop { art, .. } | PieceKind::Animated { art, .. } = kind {
                        props.insert(art.sprite);
                    }
                    assert_eq!(
                        stray_pixel(&kind, span, layout, pack, theme, scale),
                        None,
                        "{kind:?} at scale {s} wrote a logical pixel outside {span:?}"
                    );
                    // At the densities the cutaway draws at.
                    if s % pack.max_density_variant().get() == 0
                        && ground_shadow(span, &kind, pack).is_some()
                    {
                        assert_eq!(
                            lowest_painted_row(&kind, layout, pack, theme, scale),
                            Some(span.y1),
                            "{kind:?} at scale {s} is grounded on a row it doesn't reach: {span:?}"
                        );
                    }
                }
            }
        };
        // Every step of a walk to each desk facing and the sit, for the mover...
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let (layout, pack, frames, _) = sit_down(facing, 2);
            for frame in &frames {
                check(&pack, frame, &layout, true);
            }
            // ...the office around them once, a lit screen and a carried chair
            // included...
            check(
                &pack,
                frames.last().expect("a seated frame"),
                &layout,
                false,
            );
        }
        // ...the mover in every style the pack draws, so a box that forgot the
        // rows a style's hair rises by shows...
        let styles: std::collections::BTreeSet<_> = test_default_pack()
            .hairstyles()
            .map(|s| s.name().to_owned())
            .collect();
        let mut worn = std::collections::BTreeSet::new();
        for i in 0..1000 {
            let pack = test_default_pack();
            let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
            let dense = crate::embedded_pack::densest_frame(&pack, "walking", 0, scale)
                .expect("the walk's art");
            let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/style/{i}.jsonl"));
            let style =
                crate::character::dress_for(&pack, id, dense.frame, dense.head, dense.density)
                    .and_then(|d| d.style);
            let Some(style) = style.filter(|s| !worn.contains(s)) else {
                continue;
            };
            worn.insert(style);
            let (layout, pack, frames, _) = sit_down_as(pack, crate::layout::Facing::South, 0, id);
            for frame in &frames {
                check(&pack, frame, &layout, true);
            }
            if worn == styles {
                break;
            }
        }
        assert_eq!(worn, styles, "the walks wore every style");
        // ...every effect a figure carries, on each step of a walk and the sit...
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        for mut frame in frames {
            for c in &mut frame.characters {
                c.effects = every_effect(c.anchor);
            }
            check(&pack, &frame, &layout, true);
        }
        // ...every desk prop, each tower tier with a sheet mid-fall, both ways
        // a desk faces...
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let (layout, pack, frames, _) = sit_down(facing, 2);
            let mut frame = frames.last().expect("a seated frame").clone();
            for (i, d) in frame.desks.iter_mut().enumerate() {
                d.cup = Some(crate::sim::Cup::Steaming);
                d.token_tier = (i % usize::from(crate::token_meter::MAX_TIER + 1)) as u8;
                d.sheet_fall = Some(1);
            }
            check(&pack, &frame, &layout, false);
        }
        // ...a walk whose frames differ in size, so a span sized from the wrong
        // frame shows...
        const LONG_STRIDE: &str = "\
@frame 0
. n H H H H n .
n H H H H H H n
H H S S S S H H
H S e S S e S H
. S S S m S S .
. n S S S S n .
. B B B B B B .
B B B B B B B B
S B B B B B B S
. P P P P P P .
. P P P P P P .
. P . . . . P P
. P . . . . . P
";
        let uneven = crate::embedded_pack::test_pack_with(&[("walking_1.sprite", LONG_STRIDE)]);
        let (layout, uneven, frames, _) = sit_down_in(uneven, crate::layout::Facing::South, 0);
        for frame in &frames {
            check(&uneven, frame, &layout, true);
        }
        // ...and offices whose sizes gate in the pieces 160x96 lacks, empty.
        let pack = test_default_pack();
        for (w, h) in [(240u16, 144u16), (100, 60)] {
            let stepped = FloorSession::new()
                .step(
                    crate::floor::FloorInputs {
                        scene: &pixtuoid_core::SceneState::uniform(16),
                        pack: &pack,
                        now: std::time::SystemTime::UNIX_EPOCH,
                        floor: FloorMeta::ground(),
                        pets: crate::floor::PetInputs::default(),
                    },
                    crate::layout::Size { w, h },
                )
                .expect("lays out");
            check(&pack, &stepped.frame, &stepped.layout, false);
        }
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            [
                "animated",
                "badge",
                "board",
                "chair",
                "character",
                "clock",
                "desk",
                "desk prop",
                "door",
                "effect",
                "glass",
                "hung decor",
                "indicator",
                "neon",
                "prop",
                "prop band",
                "table",
                "wall"
            ],
            "a piece kind went untested"
        );
        // Every fixture drawn from art must have been reached, the pantry
        // counter at both its sizes.
        for sprite in crate::layout::PANTRY_COUNTER_ANIMS.into_iter().chain([
            "meeting_sofa",
            "plant",
            "filing_cabinet",
            "meeting_chair",
            "coat_rack",
            "side_table",
            "floor_lamp",
            "kitchen_island",
            "pantry_bin",
            "fish_tank",
            "water_cooler",
            "vending_machine",
            "printer",
        ]) {
            assert!(
                props.contains(sprite),
                "no {sprite} prop was painted: {props:?}"
            );
        }
    }

    /// At the art's density the waiting mark and a z float clear of their
    /// figure's badge however far the z has risen, the z only climbs and drifts
    /// away, and the dust lies along the stepping foot's row.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_dense_looks_keep_their_places() {
        use crate::effects::EffectKind as K;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let mut frame = frames.last().expect("a seated frame").clone();
        for c in &mut frame.characters {
            c.effects = every_effect(c.anchor);
        }
        let list = list_at(&frame, office, 12);
        let badge = list
            .pieces()
            .iter()
            .find(|p| matches!(p.kind, PieceKind::Badge { .. }))
            .expect("the badge")
            .span;
        let rider = |kind| {
            list.pieces()
                .iter()
                .find_map(|p| match p.kind {
                    PieceKind::Effect(r) if r.effect.kind == kind => Some((r, p.span)),
                    _ => None,
                })
                .expect("the rider")
        };
        let clear =
            |s: Span| s.x1 < badge.x0 || badge.x1 < s.x0 || s.y1 < badge.y0 || badge.y1 < s.y0;
        let (_, mark) = rider(K::WaitingMark);
        assert!(
            clear(mark),
            "the mark {mark:?} lands on the badge {badge:?}"
        );
        let (z, _) = rider(K::SleepZ);
        let mut last: Option<Span> = None;
        for phase in (0..crate::effects::SLEEP_Z_RISE_MS).step_by(100) {
            let r = crate::cutaway::effects::Riding {
                effect: crate::effects::Effect { phase, ..z.effect },
                ..z
            };
            let Some(s) = r.span(theme, 0) else {
                continue;
            };
            assert!(
                clear(s),
                "the z at {phase} ms {s:?} lands on the badge {badge:?}"
            );
            if let Some(l) = last {
                assert!(
                    s.y0 <= l.y0 && s.x0 >= l.x0,
                    "the z sank or drifted back at {phase} ms"
                );
            }
            last = Some(s);
        }
        let (dust, span) = rider(K::WalkingDust);
        let foot = crate::effects::look::walking_dust_foot(dust.effect.at, dust.effect.phase);
        assert!(
            (span.x0..=span.x1).contains(&foot.x) && (span.y0..=span.y1).contains(&foot.y),
            "the dust {span:?} is off the foot {foot:?}"
        );
    }

    /// A figure's dust paints straight before them and their other riders
    /// straight after, so nothing sorts between a person and what rides on
    /// them.
    #[test]
    fn riders_paint_beside_their_figure() {
        use crate::effects::EffectKind as K;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        for mut frame in frames {
            for c in &mut frame.characters {
                c.effects = every_effect(c.anchor);
            }
            let list = list_at(&frame, office, 12);
            let kinds: Vec<Option<K>> = list
                .pieces()
                .iter()
                .map(|p| match p.kind {
                    PieceKind::Effect(r) => Some(r.effect.kind),
                    _ => None,
                })
                .collect();
            let figure = list
                .pieces()
                .iter()
                .position(|p| matches!(p.kind, PieceKind::Character { .. }))
                .expect("the figure");
            assert_eq!(
                kinds[figure - 1..=figure + 3],
                [
                    Some(K::WalkingDust),
                    None,
                    Some(K::FlameCrown),
                    Some(K::SleepZ),
                    Some(K::WaitingMark)
                ]
            );
        }
    }

    /// A burning figure's crown stands on the top of their hair, at the
    /// density the art is drawn at and at the base art's.
    #[test]
    fn the_flame_crown_stands_on_the_hair() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let mut frame = frames.last().expect("a seated frame").clone();
        for c in &mut frame.characters {
            c.effects = vec![crate::effects::flame_crown(
                c.anchor,
                8,
                std::time::UNIX_EPOCH,
            )];
        }
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            };
            let list = list_at(&frame, office, 12);
            // The rows a piece paints alone, top and bottom.
            let rows = |want: fn(&PieceKind) -> bool| {
                let piece = list
                    .pieces()
                    .iter()
                    .find(|p| want(&p.kind))
                    .expect("the piece");
                let [a, b] = painted_over_two_fills(&piece.kind, &layout, &pack, theme, scale);
                let painted: Vec<u16> = (0..a.height())
                    .filter(|&y| {
                        (0..a.width()).any(|x| a.get(x, y) != UNDER[0] || b.get(x, y) != UNDER[1])
                    })
                    .collect();
                (painted.first().copied(), painted.last().copied())
            };
            let (hair, _) = rows(|k| matches!(k, PieceKind::Character { .. }));
            let (_, base) = rows(|k| matches!(k, PieceKind::Effect(_)));
            // Within the hair's top layout row.
            assert!(
                base.zip(hair)
                    .is_some_and(|(base, hair)| (hair..hair + s).contains(&base)),
                "at scale {s} the crown's base row {base:?} is off the hair's top {hair:?}"
            );
        }
    }

    /// What the incremental canvas relies on: two pieces sharing a span and a
    /// fingerprint paint the same pixels, so one may stand in for the other.
    /// Walked over every tick of a walk to a desk and a sit, where the figure's
    /// and the desk's fingerprints both change (asserted below). Every piece is
    /// compared at scale 1; at the densest scale only figures, the kind that
    /// changes tick to tick (the glass is walked in
    /// `a_window_s_fingerprint_moves_with_the_moment_it_shows`).
    #[test]
    fn one_span_and_fingerprint_always_paint_the_same_pixels() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let now = std::time::SystemTime::UNIX_EPOCH;
        // Keyed by scale too: a fingerprint holds within one scale (`Piece`).
        let mut painted: std::collections::HashMap<(u16, Span, u64), u64> =
            std::collections::HashMap::new();
        let (mut repeats, mut figure_changes, mut desk_changes) = (0, 0, 0);
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let (layout, pack, frames, desk) = sit_down(facing, 2);
            for s in [1, pack.max_density_variant().get()] {
                let scale = RenderScale::new(s).expect("nonzero");
                let mut last: Option<(u64, u64)> = None;
                for frame in &frames {
                    let list = build_list(
                        frame,
                        Office {
                            layout: &layout,
                            pack: &pack,
                            theme,
                            scale,
                        },
                        &Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now),
                        0,
                        quiet_board(),
                    );
                    repeats += same_fingerprint_same_pixels(&mut painted, &list, &layout, |p| {
                        s == 1 || matches!(p.kind, PieceKind::Character { .. })
                    });
                    let fp_of = |want: fn(&PieceKind, crate::layout::Point) -> bool| {
                        list.pieces()
                            .iter()
                            .find(|p| want(&p.kind, desk))
                            .map(|p| p.fingerprint)
                    };
                    let figure = fp_of(|k, _| matches!(k, PieceKind::Character { .. }));
                    let desk_fp = fp_of(|k, d| matches!(k, PieceKind::Desk { at, .. } if *at == d));
                    if let (Some((f0, d0)), Some(f1), Some(d1)) = (last, figure, desk_fp) {
                        figure_changes += usize::from(f0 != f1);
                        desk_changes += usize::from(d0 != d1);
                    }
                    last = figure.zip(desk_fp);
                }
            }
        }
        assert!(repeats > 0, "no piece recurred, so nothing was compared");
        assert!(figure_changes > 0, "the walker never changed fingerprint");
        assert!(
            desk_changes > 0,
            "the home desk's screen never changed fingerprint"
        );
    }

    /// The glass half of the property: a window keeps its span while the sky
    /// and the city's lights move under it, so only the fingerprint can tell
    /// two moments twelve hours apart, at the densest scale where the view is art pixels.
    #[test]
    fn a_window_s_fingerprint_moves_with_the_moment_it_shows() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let mut painted = std::collections::HashMap::new();
        let glass = |now: std::time::SystemTime, painted: &mut _| {
            let list = build_list(
                &frames[0],
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale,
                },
                &Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now),
                0,
                quiet_board(),
            );
            let is_glass = |p: &Piece| matches!(p.kind, PieceKind::Glass { .. });
            same_fingerprint_same_pixels(painted, &list, &layout, is_glass);
            list.pieces()
                .iter()
                .filter(|p| is_glass(p))
                .map(|p| (p.span, p.fingerprint))
                .collect::<Vec<_>>()
        };
        let noon = std::time::UNIX_EPOCH + std::time::Duration::from_secs(12 * 3600);
        let (a, b) = (
            glass(std::time::UNIX_EPOCH, &mut painted),
            glass(noon, &mut painted),
        );
        assert!(!a.is_empty(), "the office has windows");
        assert_eq!(a.len(), b.len(), "the same windows at every hour");
        for ((span_a, fp_a), (span_b, fp_b)) in a.iter().zip(&b) {
            assert_eq!(span_a, span_b, "a window never moves");
            assert_ne!(
                fp_a, fp_b,
                "a window at {span_a:?} looks the same twelve hours apart"
            );
        }
        assert_eq!(glass(noon, &mut painted), b, "one moment, one fingerprint");
    }

    /// `frame`'s list at local `hour`, under a clear sky.
    fn list_at<'a>(frame: &SimFrame, office: Office<'a>, hour: u32) -> DrawList<'a> {
        let now = crate::localclock::at_hour(hour);
        let sky = crate::sky::Sky::at_with(now, crate::sky::Weather::Clear);
        build_list(
            frame,
            office,
            &Moment::resolve(sky, office.theme, 0.0, now),
            0,
            quiet_board(),
        )
    }

    /// What the rest cache relies on: between noon and midnight a static
    /// piece keeps its span and fingerprint, while the room's tone and its
    /// lights move.
    #[test]
    fn noon_and_midnight_differ_only_in_dynamic_pieces() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        for s in [1, pack.max_density_variant().get()] {
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::new(s).expect("nonzero"),
            };
            let (noon, night) = (list_at(frame, office, 12), list_at(frame, office, 23));
            let at_rest = |list: &DrawList| -> Vec<(Span, u64)> {
                list.pieces()
                    .iter()
                    .filter(|p| p.kind.is_static())
                    .map(|p| (p.span, p.fingerprint))
                    .collect()
            };
            assert_eq!(at_rest(&noon), at_rest(&night), "a static piece moved");
            assert_ne!(noon.ambient, night.ambient, "the room keeps its noon tone");
            let lit = |list: &DrawList| -> Vec<u64> {
                list.lights().iter().map(|l| l.fingerprint).collect()
            };
            assert_ne!(lit(&noon), lit(&night), "the lights keep their noon levels");
        }
    }

    /// `list`'s lights over a flat `under`, relit over `rect` alone by the
    /// frame's own pass: every light that meets it, over an all-lit room.
    fn lights_over(list: &DrawList<'_>, layout: &Layout, rect: Span) -> RgbBuffer {
        let (w, h) = (
            list.scale.to_buffer(layout.buf_w),
            list.scale.to_buffer(layout.buf_h),
        );
        let mut buf = RgbBuffer::filled(w, h, list.theme.surface.bg_fallback);
        let pen = Pen::for_pack(list.scale, list.pack);
        let lights: Vec<&crate::cutaway::light::LightView> =
            list.lights().iter().map(|l| &l.view).collect();
        crate::cutaway::light::net_pass(
            ArtRect {
                x: pen.art(rect.x0),
                y: pen.art(rect.y0),
                w: pen.art(rect.x1 - rect.x0 + 1),
                h: pen.art(rect.y1 - rect.y0 + 1),
            },
            &lights,
            list.ambient,
            &crate::cutaway::light::Emission::new(w, h),
            pen,
            &mut crate::cutaway::light::NetMemo::default(),
            &mut buf,
        );
        buf
    }

    /// The light half of the fingerprint property, through the frame's own
    /// pass: a light's span relit from the same lights over the same room paints
    /// alike, so a canvas repainting a light's span on a change to any light
    /// that meets it is complete. Over a walk at three hours, at scale 1 and
    /// the densest, where some light changes its bands in place.
    #[test]
    fn one_set_of_lights_one_set_of_pixels() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let mut painted: std::collections::HashMap<(u16, Span, Vec<u64>), u64> =
            std::collections::HashMap::new();
        let mut seen: std::collections::HashMap<(u16, Span), std::collections::BTreeSet<u64>> =
            std::collections::HashMap::new();
        let mut repeats = 0;
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            for hour in [18, 20, 23] {
                for frame in frames.iter().step_by(4) {
                    let list = list_at(
                        frame,
                        Office {
                            layout: &layout,
                            pack: &pack,
                            theme,
                            scale,
                        },
                        hour,
                    );
                    for light in list.lights() {
                        seen.entry((s, light.span))
                            .or_default()
                            .insert(light.fingerprint);
                        let meeting: Vec<u64> = list
                            .lights()
                            .iter()
                            .filter(|l| {
                                l.span.x0 <= light.span.x1
                                    && light.span.x0 <= l.span.x1
                                    && l.span.y0 <= light.span.y1
                                    && light.span.y0 <= l.span.y1
                            })
                            .map(|l| l.fingerprint)
                            .collect();
                        let pixels = {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::hash::DefaultHasher::new();
                            lights_over(&list, &layout, light.span)
                                .as_slice()
                                .hash(&mut h);
                            (list.ambient, h.finish())
                        };
                        match painted.entry((s, light.span, meeting)) {
                            std::collections::hash_map::Entry::Occupied(e) => {
                                assert_eq!(
                                    *e.get(),
                                    pixels.1,
                                    "one set of lights at {:?} painted two ways",
                                    light.span
                                );
                                repeats += 1;
                            }
                            std::collections::hash_map::Entry::Vacant(v) => {
                                v.insert(pixels.1);
                            }
                        }
                    }
                }
            }
        }
        assert!(repeats > 0, "no light recurred, so nothing was compared");
        assert!(
            seen.values().any(|fps| fps.len() > 1),
            "no light changed its bands in place"
        );
    }

    /// A light alone, through the frame's own pass over the whole room under a
    /// sky that darkens nothing, changes no pixel outside its span, at every
    /// scale.
    #[test]
    fn a_light_paints_only_inside_its_span() {
        use crate::cutaway::light::{Ambient, Emission, NetMemo, net_pass};
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let (mut lights, mut lit) = (0, 0);
        for s in [1, 3, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            };
            let list = list_at(frame, office, 23);
            let pen = Pen::for_pack(scale, &pack);
            let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
            let whole = ArtRect {
                x: ArtPx(0),
                y: ArtPx(0),
                w: pen.art(layout.buf_w),
                h: pen.art(layout.buf_h),
            };
            for light in list.lights() {
                lights += 1;
                let mut buf = RgbBuffer::filled(w, h, theme.surface.bg_fallback);
                net_pass(
                    whole,
                    &[&light.view],
                    Ambient::default(),
                    &Emission::new(w, h),
                    pen,
                    &mut NetMemo::default(),
                    &mut buf,
                );
                lit += buf
                    .as_slice()
                    .iter()
                    .filter(|&&c| c != theme.surface.bg_fallback)
                    .count();
                let stray = buf.as_slice().iter().enumerate().find(|&(i, &c)| {
                    let (x, y) = (
                        scale.logical((i % usize::from(w)) as u16),
                        scale.logical((i / usize::from(w)) as u16),
                    );
                    c != theme.surface.bg_fallback
                        && !((light.span.x0..=light.span.x1).contains(&x)
                            && (light.span.y0..=light.span.y1).contains(&y))
                });
                assert_eq!(
                    stray.map(|(i, _)| i),
                    None,
                    "a light at scale {s} wrote outside {:?}",
                    light.span
                );
            }
        }
        assert!(lights > 0, "the night office has no lights");
        assert!(lit > 0, "no light lifted a pixel, so this pins nothing");
    }

    /// [`mark`] writes nothing outside the piece's span, at every scale: a lit
    /// and a dark screen, a desk lamp's bulb, the wall decor's, a floor lamp's
    /// and an open elevator's.
    #[test]
    fn a_piece_marks_its_glow_only_inside_its_span() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (seated, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let lively = lively_office();
        let open = SimFrame {
            door_frame: 2,
            ..empty_frame(&lively)
        };
        let mut marked = std::collections::BTreeSet::new();
        for (layout, frame) in [
            (&seated, frames.last().expect("a seated frame")),
            (&seated, &frames[0]),
            (&lively, &open),
        ] {
            for s in [1, 3, pack.max_density_variant().get()] {
                let scale = RenderScale::new(s).expect("nonzero");
                let office = Office {
                    layout,
                    pack: &pack,
                    theme,
                    scale,
                };
                let list = list_at(frame, office, 23);
                let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
                let mut art = ArtCache::default();
                for p in list.pieces() {
                    let mut marks = RgbBuffer::filled(w, h, NO_MARK);
                    if mark(&p.kind, (&pack, scale), &mut art, &mut marks) {
                        marked.insert(kind_name(&p.kind));
                    }
                    let stray = marks.as_slice().iter().enumerate().find(|&(i, &c)| {
                        let (x, y) = (
                            scale.logical((i % usize::from(w)) as u16),
                            scale.logical((i / usize::from(w)) as u16),
                        );
                        c != NO_MARK
                            && !((p.span.x0..=p.span.x1).contains(&x)
                                && (p.span.y0..=p.span.y1).contains(&y))
                    });
                    assert_eq!(
                        stray.map(|(i, _)| i),
                        None,
                        "{} at scale {s} marked outside {:?}",
                        kind_name(&p.kind),
                        p.span
                    );
                }
            }
        }
        // Only the density art draws bulbs.
        let want: &[&str] = if cfg!(feature = "density-art") {
            &["desk", "door", "hung decor", "prop"]
        } else {
            &["desk"]
        };
        assert_eq!(
            marked.into_iter().collect::<Vec<_>>(),
            want,
            "a kind that marks went unchecked"
        );
    }

    /// The cutaway's sign glows only where the sign is drawn, and its lamps
    /// light the night room.
    #[test]
    fn the_cutaway_glows_only_around_its_sign() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
        };
        let (mut lamps, mut glows) = (0, 0);
        for hour in [12, 18, 23] {
            let list = list_at(frame, office, hour);
            let signs = list
                .pieces()
                .iter()
                .filter(|p| matches!(p.kind, PieceKind::Neon { .. }))
                .count();
            for light in list.lights() {
                lamps += usize::from(light.view.is(crate::lighting::EmitterKind::DeskLamp));
                let glow = light.view.is(crate::lighting::EmitterKind::NeonGlow);
                assert!(!glow || signs == 1, "{hour}:00 glows with no sign");
                glows += usize::from(glow);
            }
        }
        assert!(glows > 0, "the sign never glows");
        assert!(
            lamps > 0,
            "nothing lights the night room, so this pins nothing"
        );
    }

    #[test]
    fn the_neon_glow_takes_its_tubes_hue() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
        };
        for neon in [
            crate::floor::NeonLevels::CALM,
            crate::floor::NeonLevels::ALERT,
            crate::floor::NeonLevels {
                alert: 0.5,
                power: 1.0,
            },
        ] {
            let frame = SimFrame {
                neon,
                ..empty_frame(&layout)
            };
            let list = list_at(&frame, office, 23);
            let hue = list.pieces().iter().find_map(|p| match p.kind {
                PieceKind::Neon { hue, .. } => Some(hue),
                _ => None,
            });
            let glow = list
                .lights()
                .iter()
                .find(|l| l.view.is(crate::lighting::EmitterKind::NeonGlow))
                .map(|l| l.view.tint());
            assert_eq!(glow, Some(hue), "{neon:?}");
        }
    }

    /// Every desk's lamp pools where its art hangs the bulb, whichever way the
    /// desk faces and at every density its art is drawn at: the cutaway's art
    /// stands it on the side the desk faces.
    #[test]
    #[cfg(feature = "density-art")]
    fn a_desk_lamp_pools_under_its_painted_bulb() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = Layout::compute_with_seed(240, 144, None, 0).expect("lays out");
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let pen = Pen::for_pack(scale, &pack);
            let frame = empty_frame(&layout);
            let list = list_at(
                &frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale,
                },
                23,
            );
            // Each painted bulb's middle, in art pixels.
            let bulbs: Vec<(f32, f32)> = layout
                .home_desks
                .iter()
                .enumerate()
                .filter_map(|(i, &at)| {
                    let art = desk_art(
                        &pack,
                        layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i)),
                    )?;
                    let span = desk_span(&pack, art, at, scale)?;
                    let desk = crate::embedded_pack::densest_frame(&pack, art, 0, scale)?;
                    let cells = drawn_in(&desk, &[crate::embedded_pack::DESK_BULB_KEY]);
                    let w = usize::from(desk.frame.width());
                    let hits: Vec<(f32, f32)> = cells
                        .iter()
                        .enumerate()
                        .filter(|&(_, &b)| b)
                        .map(|(i, _)| ((i % w) as f32, (i / w) as f32))
                        .collect();
                    // Art pixels of this art, on the pen's grid.
                    let k = f32::from(pen.art(1).0) / f32::from(desk.density.get());
                    (!hits.is_empty()).then(|| {
                        let n = hits.len() as f32;
                        (
                            f32::from(pen.art(span.x0).0)
                                + hits.iter().map(|h| h.0).sum::<f32>() / n * k,
                            f32::from(pen.art(span.y0).0)
                                + hits.iter().map(|h| h.1).sum::<f32>() / n * k,
                        )
                    })
                })
                .collect();
            assert!(!bulbs.is_empty(), "the pack's desks draw no bulb");
            let lamps: Vec<(f32, f32)> = list
                .lights()
                .iter()
                .filter(|l| l.view.is(crate::lighting::EmitterKind::DeskLamp))
                .map(|l| l.view.peak())
                .collect();
            assert_eq!(lamps.len(), bulbs.len(), "one pool a bulb");
            let near = f32::from(pen.art(1).0);
            for bulb in &bulbs {
                assert!(
                    lamps
                        .iter()
                        .any(|p| (p.0 - bulb.0).abs() <= near && (p.1 - bulb.1).abs() <= near),
                    "no lamp pools within {near} art px of the bulb at {bulb:?}: {lamps:?}"
                );
            }
        }
    }

    /// `list` painted as by day, over its backdrop and shadows, with each
    /// pixel's glow: what the net pass starts from.
    fn by_day(
        list: &DrawList<'_>,
        layout: &Layout,
    ) -> (RgbBuffer, crate::cutaway::light::Emission) {
        let mut buf = RgbBuffer::filled(
            list.scale.to_buffer(layout.buf_w),
            list.scale.to_buffer(layout.buf_h),
            list.theme.surface.bg_fallback,
        );
        let pen = Pen::for_pack(list.scale, list.pack);
        paint_backdrop(layout, list.theme, list.scale, pen, &mut buf);
        paint_ground_shadows(
            list.pieces.iter().filter_map(|p| p.shadow),
            crate::ground::shadow_strength(list.ambient.darkness()),
            pen,
            &mut buf,
        );
        let mut cache = CutawayCache::default();
        let emission = paint_pieces(list, &mut cache, &mut buf);
        (buf, emission)
    }

    /// At night, a pixel no light reaches is its daylight colour stepped down
    /// by the room once: a wall's glass over the ground included, which recolours
    /// the ground it lies on and so must not darken it a second time. A pixel a
    /// light reaches is lifted back toward daylight, never past.
    #[test]
    fn the_night_room_is_its_daylight_stepped_down_once() {
        use crate::cutaway::light::Glow;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let list = list_at(frame, office, 23);
        assert!(list.ambient.darkness() > 0.0, "23:00 is dark");
        let (day, emission) = by_day(&list, &layout);
        let mut cache = CutawayCache::default();
        let mut night = RgbBuffer::filled(day.width(), day.height(), theme.surface.bg_fallback);
        paint_backdrop(
            &layout,
            theme,
            scale,
            Pen::for_pack(scale, &pack),
            &mut night,
        );
        paint_list(&list, &mut cache, &mut night);
        let lights: Vec<&crate::cutaway::light::LightView> =
            list.lights().iter().map(|l| &l.view).collect();
        let k = scale.get() / Pen::for_pack(scale, &pack).art(1).0;
        let walls: Vec<Span> = list
            .pieces()
            .iter()
            .filter(|p| matches!(p.kind, PieceKind::WallSeg { .. }))
            .map(|p| p.span)
            .collect();
        let (mut unlit, mut under_walls) = (0, 0);
        for y in 0..day.height() {
            for x in 0..day.width() {
                if emission.get(x, y) != Glow::Lit {
                    continue;
                }
                let lift = lights
                    .iter()
                    .map(|l| l.lift_at(x / k, y / k))
                    .max()
                    .unwrap_or(0);
                if lift == 0 {
                    assert_eq!(
                        night.get(x, y),
                        list.ambient.on(day.get(x, y)),
                        "({x}, {y}) is not its daylight a room's steps down"
                    );
                    unlit += 1;
                    let (lx, ly) = (scale.logical(x), scale.logical(y));
                    under_walls += usize::from(
                        walls
                            .iter()
                            .any(|w| (w.x0..=w.x1).contains(&lx) && (w.y0..=w.y1).contains(&ly)),
                    );
                }
                assert!(
                    f32::from(lift) <= list.ambient.darkness() * 4.0,
                    "({x}, {y}) is lifted {lift} steps past the night"
                );
            }
        }
        assert!(
            unlit > 0 && under_walls > 0,
            "no unlit wall glass was checked"
        );
    }

    /// A sitter in front of a glowing screen takes the room's light over it: the
    /// last piece to paint a pixel says how it glows.
    #[test]
    fn a_sitter_over_a_screen_takes_the_rooms_light() {
        use crate::cutaway::light::Glow;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let mut list = list_at(frame, office, 23);
        let (with, glow) = by_day(&list, &layout);
        list.pieces
            .retain(|p| !matches!(p.kind, PieceKind::Character { .. }));
        let (without, glow_without) = by_day(&list, &layout);
        let mut over = 0;
        for y in 0..with.height() {
            for x in 0..with.width() {
                if with.get(x, y) == without.get(x, y) {
                    continue;
                }
                assert_eq!(glow.get(x, y), Glow::Lit, "({x}, {y}) of a sitter glows");
                over += usize::from(glow_without.get(x, y) != Glow::Lit);
            }
        }
        assert!(
            over > 0,
            "no sitter covers a screen, so nothing was compared"
        );
    }

    #[test]
    fn a_piece_takes_the_pixels_it_paints_in_the_colour_already_there() {
        use crate::cutaway::light::Glow;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let mut list = list_at(frame, office, 12);
        let (painted, _) = by_day(&list, &layout);
        let pen = Pen::for_pack(scale, &pack);
        let (w, h) = (pen.art(4).0, pen.art(2).0);
        let (x, y) = (pen.art(desk.x).0, pen.art(desk.y).0);
        let (bx, by) = (pen.buffer(ArtPx(x)), pen.buffer(ArtPx(y)));
        let px = (0..h)
            .flat_map(|dy| (0..w).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| Some(painted.get(pen.buffer(ArtPx(x + dx)), pen.buffer(ArtPx(y + dy)))))
            .collect();
        list.pieces.push(Piece {
            span: Span::new(desk.x, desk.y, 4, 2, 0),
            kind: PieceKind::Glass {
                view: WindowView { x, y, w, px },
            },
            shadow: None,
            fingerprint: 0,
        });
        let (repainted, glow) = by_day(&list, &layout);
        assert_eq!(
            repainted.get(bx, by),
            painted.get(bx, by),
            "the pane repaints the desk's colour"
        );
        assert_eq!(glow.get(bx, by), Glow::Pane);
    }

    /// The hour reaches the room: over the whole frame, the night room is
    /// darker than the noon one.
    #[test]
    fn the_night_room_is_darker_than_the_noon_room() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let frame = frames.last().expect("a seated frame");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let luma = |hour: u32| -> u64 {
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                theme.surface.bg_fallback,
            );
            let mut cache = CutawayCache::default();
            render_cutaway(
                frame,
                office,
                showing(
                    crate::floor::FloorMeta::ground(),
                    crate::localclock::at_hour(hour),
                ),
                &mut cache,
                &mut buf,
            );
            buf.as_slice()
                .iter()
                .map(|c| u64::from(c.r) + u64::from(c.g) + u64::from(c.b))
                .sum()
        };
        assert!(luma(23) < luma(12), "midnight lights the room like noon");
    }

    /// A static piece keeps its fingerprint whatever the hour and whoever is at
    /// the appliances; a busy appliance plays, so it is not one.
    #[test]
    fn a_static_piece_holds_through_the_hours_and_the_queue() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = Layout::compute_with_seed(240, 144, None, 0).expect("lays out");
        let idle = empty_frame(&layout);
        let mut busy = idle.clone();
        busy.occupied_waypoints = (0..layout.waypoints.len()).collect();
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
        };
        let lists: Vec<Vec<(Span, u64, bool, bool)>> =
            [(&idle, 12), (&busy, 12), (&idle, 23), (&busy, 23)]
                .into_iter()
                .map(|(frame, hour)| {
                    list_at(frame, office, hour)
                        .pieces()
                        .iter()
                        .map(|p| {
                            (
                                p.span,
                                p.fingerprint,
                                p.kind.is_static(),
                                matches!(p.kind, PieceKind::Animated { art, .. }
                                    if ["vending_machine", "printer"].contains(&art.sprite)),
                            )
                        })
                        .collect()
                })
                .collect();
        let at_rest = |l: &Vec<(Span, u64, bool, bool)>| -> Vec<(Span, u64)> {
            l.iter().filter(|p| p.2).map(|p| (p.0, p.1)).collect()
        };
        for l in &lists[1..] {
            assert_eq!(at_rest(&lists[0]), at_rest(l), "a static piece moved");
        }
        let appliances = |l: &Vec<(Span, u64, bool, bool)>| -> Vec<u64> {
            l.iter().filter(|p| p.3).map(|p| p.1).collect()
        };
        assert!(
            !appliances(&lists[0]).is_empty(),
            "the office has no appliance"
        );
        assert_ne!(
            appliances(&lists[0]),
            appliances(&lists[1]),
            "a busy appliance holds its rest frame, so this pins nothing"
        );
    }

    /// Record each of `list`'s pieces `keep` selects by (scale, span,
    /// fingerprint), asserting one that recurs paints the pixels it painted
    /// before; returns how many recurred.
    fn same_fingerprint_same_pixels(
        painted: &mut std::collections::HashMap<(u16, Span, u64), u64>,
        list: &DrawList<'_>,
        layout: &Layout,
        keep: impl Fn(&Piece) -> bool,
    ) -> usize {
        let mut repeats = 0;
        for piece in list.pieces().iter().filter(|p| keep(p)) {
            let pixels = painted_alone(&piece.kind, layout, list.pack, list.theme, list.scale);
            let s = list.scale.get();
            match painted.entry((s, piece.span, piece.fingerprint)) {
                std::collections::hash_map::Entry::Occupied(seen) => {
                    assert_eq!(
                        *seen.get(),
                        pixels,
                        "{:?} at scale {s} shares {:?}'s fingerprint but paints differently",
                        piece.kind,
                        piece.span
                    );
                    repeats += 1;
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(pixels);
                }
            }
        }
        repeats
    }

    /// The figure half of the property, where it is hardest: a sitter keeps
    /// their span while each thing their figure is painted from changes, so
    /// only the fingerprint can tell the frames apart. Every variant is painted
    /// at scale 1 and the densest, and any two sharing a span and fingerprint
    /// must paint alike.
    #[test]
    fn a_sitters_fingerprint_moves_with_everything_their_figure_paints_from() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let now = std::time::SystemTime::UNIX_EPOCH;
        let mut painted = std::collections::HashMap::new();
        let (mut figures, mut built) = (std::collections::HashSet::new(), 0);
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let (layout, pack, frames, _) = sit_down(facing, 2);
            let seated = frames
                .iter()
                .rev()
                .find(|f| f.characters.first().is_some_and(|c| c.seat_desk.is_some()))
                .expect("the fixture sits down");
            let other = pixtuoid_core::AgentId::from_transcript_path("/cutaway/other.jsonl");
            let variants: [&FigureEdit; 7] = [
                &|_, _| {},
                &|c, a| {
                    c.glow = crate::sim::CharacterGlow::Tool;
                    a.state = pixtuoid_core::state::ActivityState::Active {
                        tool_use_id: None,
                        detail: None,
                        kind: pixtuoid_core::state::ToolKind::Read,
                    };
                },
                &|_, a| a.model = Some(std::sync::Arc::from("claude-fable-5")),
                &|_, a| a.cwd = std::sync::Arc::from(std::path::Path::new("/w/other")),
                &|c, _| c.flip_x = !c.flip_x,
                &|c, _| {
                    c.anim_name = "seated_sleeping";
                    c.frame_idx = 0;
                },
                &move |_, a| a.agent_id = other,
            ];
            // A set: without the density art the densest scale is 1.
            let scales = std::collections::BTreeSet::from([1, pack.max_density_variant().get()]);
            for s in scales {
                let scale = RenderScale::new(s).expect("nonzero");
                for edit in variants {
                    built += 1;
                    let frame = varied(seated, edit);
                    let list = build_list(
                        &frame,
                        Office {
                            layout: &layout,
                            pack: &pack,
                            theme,
                            scale,
                        },
                        &Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now),
                        0,
                        quiet_board(),
                    );
                    same_fingerprint_same_pixels(&mut painted, &list, &layout, |p| {
                        matches!(p.kind, PieceKind::Character { .. })
                    });
                    for p in list.pieces() {
                        if matches!(p.kind, PieceKind::Character { .. }) {
                            figures.insert((s, p.span, p.fingerprint));
                        }
                    }
                }
            }
        }
        // One figure per facing, scale and variant, no two alike: had a variant
        // left the fingerprint unmoved, it would have been compared above.
        assert_eq!(
            figures.len(),
            built,
            "a variant did not move the fingerprint"
        );
    }

    /// An edit to one figure and its agent.
    type FigureEdit = dyn Fn(&mut crate::sim::CharacterPlacement, &mut pixtuoid_core::AgentSlot);

    /// `frame` with `edit` applied to its first figure and that figure's agent.
    fn varied(frame: &SimFrame, edit: &FigureEdit) -> SimFrame {
        let mut varied = frame.clone();
        if let Some(c) = varied.characters.first_mut() {
            edit(c, &mut varied.agents[c.agent_idx]);
        }
        varied
    }

    /// The two fills a piece is painted over to tell what it writes.
    const UNDER: [pixtuoid_core::sprite::Rgb; 2] = [
        pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
        pixtuoid_core::sprite::Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    ];

    /// `kind` painted alone over each of two opposite fills: where the two
    /// buffers agree, the piece wrote the pixel.
    fn painted_over_two_fills(
        kind: &PieceKind,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        scale: RenderScale,
    ) -> [RgbBuffer; 2] {
        let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
        UNDER.map(|fill| {
            let mut buf = RgbBuffer::filled(w, h, fill);
            paint_piece(
                kind,
                pack,
                theme,
                scale,
                &mut CutawayCache::default(),
                &mut buf,
            );
            buf
        })
    }

    /// A hash of the pixels `kind` writes, painted alone.
    fn painted_alone(
        kind: &PieceKind,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        scale: RenderScale,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        for buf in painted_over_two_fills(kind, layout, pack, theme, scale) {
            buf.as_slice().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Building one frame twice gives the same list, so a caller diffing two
    /// frames sees only what moved.
    #[test]
    fn one_frame_builds_one_list() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 0);
        let frame = frames.last().expect("a seated frame");
        let now = std::time::SystemTime::UNIX_EPOCH;
        let summary = |list: &DrawList| {
            list.pieces()
                .iter()
                .map(|p| (p.span, p.fingerprint))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            summary(&build_list(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale: RenderScale::ONE
                },
                &Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now),
                0,
                quiet_board()
            )),
            summary(&build_list(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale: RenderScale::ONE
                },
                &Moment::resolve(crate::sky::Sky::clock(now), theme, 0.0, now),
                0,
                quiet_board()
            )),
        );
    }

    /// Each drawn agent has one hover box, inside the span of a piece that
    /// paints, in draw order, and every badge belongs to one of them.
    #[test]
    fn each_drawn_agent_hovers_inside_its_piece() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        for frame in &frames {
            let list = build_list(
                frame,
                Office {
                    layout: &layout,
                    pack: &pack,
                    theme,
                    scale: RenderScale::ONE,
                },
                &Moment::resolve(
                    crate::sky::Sky::clock(std::time::SystemTime::UNIX_EPOCH),
                    theme,
                    0.0,
                    std::time::SystemTime::UNIX_EPOCH,
                ),
                0,
                quiet_board(),
            );
            let pieces: Vec<&Piece> = list
                .pieces()
                .iter()
                .filter(|p| matches!(p.kind, PieceKind::Character { .. }))
                .collect();
            let hovers: Vec<(pixtuoid_core::AgentId, Span)> = list
                .hover_spans()
                .filter_map(|(body, agent)| Some((agent?, body)))
                .collect();
            assert_eq!(hovers.len(), pieces.len());
            assert_eq!(hovers.len(), frame.characters.len());
            for ((_, body), piece) in hovers.iter().zip(&pieces) {
                let span = piece.span;
                assert!(
                    span.x0 <= body.x0
                        && body.x1 <= span.x1
                        && span.y0 <= body.y0
                        && body.y1 <= span.y1,
                    "hover box {body:?} leaves its piece's span {span:?}"
                );
                let [a, b] =
                    painted_over_two_fills(&piece.kind, &layout, &pack, theme, RenderScale::ONE);
                assert!(
                    a.as_slice()
                        .iter()
                        .zip(b.as_slice())
                        .any(|(pa, pb)| pa == pb),
                    "hover box {body:?} belongs to a piece that paints nothing"
                );
            }
            // Each drawn agent's badge, known by its text.
            let namesakes = crate::overlay::Namesakes::of(&frame.agents);
            let mut badged: Vec<_> = list.badges().map(|b| b.text.clone()).collect();
            let mut drawn: Vec<_> = hovers
                .iter()
                .filter_map(|&(id, _)| frame.agents.iter().find(|a| a.agent_id == id))
                .map(|a| namesakes.text(a))
                .collect();
            badged.sort();
            drawn.sort();
            assert_eq!(badged, drawn);
        }
    }

    fn kind_name(kind: &PieceKind) -> &'static str {
        match kind {
            PieceKind::WallSeg { .. } => "wall",
            PieceKind::Desk { .. } => "desk",
            PieceKind::Chair { .. } => "chair",
            PieceKind::Prop { .. } => "prop",
            PieceKind::PropBand { .. } => "prop band",
            PieceKind::Table { .. } => "table",
            PieceKind::Animated { .. } => "animated",
            PieceKind::Door { .. } => "door",
            PieceKind::Neon { .. } => "neon",
            PieceKind::Clock { .. } => "clock",
            PieceKind::Character { .. } => "character",
            PieceKind::Glass { .. } => "glass",
            PieceKind::Hung { .. } => "hung decor",
            PieceKind::Effect(_) => "effect",
            PieceKind::Badge { .. } => "badge",
            PieceKind::DeskProp(_) => "desk prop",
            PieceKind::Board { .. } => "board",
            PieceKind::Indicator { .. } => "indicator",
        }
    }

    /// One of each effect a figure carries, riding on its `anchor`, each at a
    /// step it shows at.
    fn every_effect(anchor: crate::layout::Point) -> Vec<crate::effects::Effect> {
        let at = |ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
        vec![
            crate::effects::walking_dust(anchor, 0),
            crate::effects::flame_crown(anchor, 8, at(0)),
            crate::effects::sleep_z(anchor, 0, at(500)).expect("a z rising at 500 ms"),
            crate::effects::waiting_mark(anchor),
        ]
    }

    /// The first logical pixel `kind` writes outside `span`, painted alone.
    fn stray_pixel(
        kind: &PieceKind,
        span: Span,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        scale: RenderScale,
    ) -> Option<(u16, u16)> {
        let w = scale.to_buffer(layout.buf_w);
        let [a, b] = painted_over_two_fills(kind, layout, pack, theme, scale);
        // A pixel that is neither the same over both fills nor left alone is a
        // recolouring of what lay under it.
        let recoloured = a
            .as_slice()
            .iter()
            .zip(b.as_slice())
            .any(|(pa, pb)| pa != pb && [*pa, *pb] != UNDER);
        assert!(
            !recoloured || kind.reads_under(),
            "{kind:?} recolours what lies under it, so it must say it reads under"
        );
        a.as_slice()
            .iter()
            .zip(b.as_slice())
            .enumerate()
            .filter(|(_, (pa, pb))| {
                if kind.reads_under() {
                    [**pa, **pb] != UNDER
                } else {
                    pa == pb
                }
            })
            .map(|(i, _)| {
                let (x, y) = (i % usize::from(w), i / usize::from(w));
                (scale.logical(x as u16), scale.logical(y as u16))
            })
            .find(|&(x, y)| !((span.x0..=span.x1).contains(&x) && (span.y0..=span.y1).contains(&y)))
    }

    #[test]
    fn the_walls_contact_row_is_the_floor_a_shade_down() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let layout = Layout::compute_with_seed(240, 144, None, 0).expect("lays out");
        let blank = || {
            RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                UNDER[0],
            )
        };
        let (mut floor, mut laid) = (blank(), blank());
        paint_ground(&layout, theme, pen, &mut floor);
        paint_backdrop(&layout, theme, scale, pen, &mut laid);
        let row = scale.to_buffer(layout.wall_band_h());
        for x in 0..floor.width() {
            assert_eq!(
                laid.get(x, row),
                floor
                    .get(x, row)
                    .ramp(crate::cutaway::shade::RAMP_SHADE_LEVEL),
                "column {x}"
            );
        }
    }

    #[test]
    fn a_rug_is_mirrored_about_its_centre_column() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let rug = crate::layout::Bounds {
            x: 4,
            y: 3,
            width: 18,
            height: 11,
        };
        let (w, h) = (
            scale.to_buffer(rug.x * 2 + rug.width),
            scale.to_buffer(rug.y * 2 + rug.height),
        );
        let mut buf = RgbBuffer::filled(w, h, UNDER[0]);
        paint_rug(rug, theme, pen, &mut buf);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(buf.get(x, y), buf.get(w - 1 - x, y), "({x}, {y})");
            }
        }
    }

    fn lowest_painted_row(
        kind: &PieceKind,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        scale: RenderScale,
    ) -> Option<u16> {
        let w = usize::from(scale.to_buffer(layout.buf_w));
        let [a, b] = painted_over_two_fills(kind, layout, pack, theme, scale);
        a.as_slice()
            .iter()
            .zip(b.as_slice())
            .enumerate()
            .filter(|(_, (pa, pb))| {
                if kind.reads_under() {
                    [**pa, **pb] != UNDER
                } else {
                    pa == pb
                }
            })
            .map(|(i, _)| scale.logical((i / w) as u16))
            .max()
    }

    /// Splitting is what makes the office above orderable, so pin it directly:
    /// no band of a N-S wall may be tall enough to span a figure. An E-W wall
    /// is whole on one row, which a figure is wholly north or south of.
    #[test]
    fn no_wall_segment_is_taller_than_the_cast() {
        let pack = test_default_pack();
        let (_, body_h) = base_size(&pack, "standing");
        let layout = Layout::compute_with_seed(240, 144, None, 0).expect("lays out");
        let mut order: Vec<(Span, PieceKind)> = Vec::new();
        wall_segments(&layout, &mut order);
        order.retain(|(_, k)| {
            matches!(
                k,
                PieceKind::WallSeg {
                    piece: crate::layout::WallPiece::Vertical { .. },
                    ..
                }
            )
        });
        assert!(!order.is_empty(), "a laid-out office has N-S walls");
        for (span, _) in &order {
            let h = span.y1 - span.y0 + 1;
            assert!(
                h <= body_h,
                "a {h}-row wall segment can span the {body_h}-row cast, so one \
                 segment would be both in front of and behind the same figure"
            );
        }
    }

    /// THE property the whole mixed-density contract rests on: a density variant
    /// changes how a piece is DRAWN, never how big it is. `densest_frame`'s
    /// variant and base arms return different (frame, factor) pairs whose
    /// PRODUCT has to agree with the logical size the desk's foot (its face) is
    /// placed by, and getting it wrong is silent — the desk
    /// still renders, with its foot a whole desk below the surface.
    #[test]
    fn the_drawn_size_is_the_same_whichever_density_the_art_came_from() {
        let pack = test_default_pack();
        let (bw, bh) = base_size(&pack, "desk");
        for s in 1..=12u16 {
            let scale = RenderScale::new(s).expect("nonzero");
            let d = crate::embedded_pack::densest_frame(&pack, "desk", 0, scale)
                .expect("desk is in the pack");
            let drawn = (
                d.frame.width() * d.blit_at.get(),
                d.frame.height() * d.blit_at.get(),
            );
            assert_eq!(
                drawn,
                (scale.to_buffer(bw), scale.to_buffer(bh)),
                "scale {s} drew a different size than the base art implies"
            );
            assert_eq!(
                drawn,
                (scale.to_buffer(d.logical.0), scale.to_buffer(d.logical.1)),
                "scale {s} drew a different size than the foot is placed by"
            );
        }
    }

    /// A back-view sofa sits its sitter between its two bands: the seat under
    /// them, the backrest, nearest the viewer, over their lap.
    #[test]
    fn a_back_view_sofa_seats_its_sitter_between_its_seat_and_its_backrest() {
        let pack = test_default_pack();
        let sofa = crate::layout::Point { x: 40, y: 30 };
        let mut order = Vec::new();
        push_sofa(&mut order, &pack, sofa, true, Tie::FixtureOver);
        let [
            (seat, PieceKind::PropBand { rows: under, .. }),
            (back, PieceKind::PropBand { rows: over, .. }),
        ] = order.as_slice()
        else {
            panic!("a back-view sofa is two bands: {order:?}");
        };
        let sitter = crate::sim::seat::sofa_sitter_z_key(sofa);
        assert!(
            seat.depth == sitter && sitter < back.depth,
            "seat {} = sitter {sitter} < backrest {}",
            seat.depth,
            back.depth
        );
        let (_, h) = base_size(&pack, MEETING_SOFA_NORTH);
        assert_eq!((under.0, under.1, over.0, over.1), (0, over.0, under.1, h));
    }

    /// A meeting room at its tightest still stands the table behind the
    /// back-view sofa's seat: sorted on its own south edge, the seat tied the
    /// table there, and the table, pushed after it, painted over its cushions.
    #[test]
    fn the_table_sorts_behind_the_back_view_sofas_seat_in_the_tightest_room() {
        let pack = test_default_pack();
        let mut checked = 0;
        for (w, h) in [(110, 66), (130, 90)] {
            for seed in 0..4 {
                let Some(layout) = Layout::compute_with_seed(w, h, None, seed) else {
                    continue;
                };
                let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
                    matches!(
                        k,
                        FixtureKind::MeetingSofa { .. } | FixtureKind::MeetingTable { .. }
                    )
                });
                let seats = order.iter().filter_map(|(s, k)| match k {
                    PieceKind::PropBand { rows: (0, _), .. } => Some(s.depth),
                    _ => None,
                });
                let tables = order.iter().filter_map(|(s, k)| match k {
                    PieceKind::Table { .. } => Some(s.depth),
                    _ => None,
                });
                for (seat, table) in seats.zip(tables) {
                    assert!(table < seat, "{w}x{h}/{seed}: table {table}, seat {seat}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "the sizes lay out meeting trios");
    }

    /// The table's span ends on the last row it paints, at the base density
    /// (with the face derived under it) and at the densest (whose art draws its
    /// own front): a span reaching past it would sort the table and cast its
    /// shadow rows below where it stands.
    #[test]
    fn the_tables_span_ends_where_it_paints() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let order = queued(&layout, &pack, scale, &[], |k| {
                matches!(k, FixtureKind::MeetingTable { .. })
            });
            let (span, kind) = order
                .iter()
                .find(|(_, k)| matches!(k, PieceKind::Table { .. }))
                .expect("a meeting trio");
            let [a, b] = painted_over_two_fills(kind, &layout, &pack, theme, scale);
            let bottom = (0..a.height())
                .rev()
                .find(|&y| {
                    (0..a.width()).any(|x| a.get(x, y) != UNDER[0] || b.get(x, y) != UNDER[1])
                })
                .expect("the table paints");
            assert_eq!(
                span.y1,
                bottom / s,
                "scale {s}: the span's south row is not the table's painted bottom"
            );
        }
    }

    /// An office with every moving fixture: an aquarium and a cooler that
    /// loop, an elevator, the sign and the clock.
    pub(crate) fn lively_office() -> Layout {
        many_layouts()
            .find(|l| {
                let kinds: Vec<FixtureKind> = l.fixtures().map(|f| f.kind).collect();
                [
                    FixtureKind::FishTank,
                    FixtureKind::WaterCooler,
                    FixtureKind::Door,
                    FixtureKind::Clock,
                ]
                .iter()
                .all(|k| kinds.contains(k))
            })
            .expect("an office has a lounge aquarium, a pantry cooler, an elevator and a clock")
    }

    /// `frame`'s list at `now`, under a clear sky.
    fn list_now<'a>(
        frame: &SimFrame,
        office: Office<'a>,
        now: std::time::SystemTime,
    ) -> DrawList<'a> {
        let sky = crate::sky::Sky::at_with(now, crate::sky::Weather::Clear);
        build_list(
            frame,
            office,
            &Moment::resolve(sky, office.theme, 0.0, now),
            0,
            quiet_board(),
        )
    }

    /// What the canvas relies on for what moves: a moving piece that keeps its
    /// span and fingerprint paints what it painted, across the clock, the
    /// elevator and the sign; and each shows more than one fingerprint over
    /// them, so a kind that stopped hashing what moves it fails here.
    #[test]
    fn a_moving_pieces_fingerprint_moves_with_what_it_shows() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = lively_office();
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
        };
        let moving = |p: &Piece| {
            matches!(
                p.kind,
                PieceKind::Animated { .. }
                    | PieceKind::Door { .. }
                    | PieceKind::Neon { .. }
                    | PieceKind::Clock { .. }
            )
        };
        let mut painted = std::collections::HashMap::new();
        let mut seen: std::collections::HashMap<&str, std::collections::HashSet<u64>> =
            std::collections::HashMap::new();
        let base = crate::localclock::at_hour_min(12, 0);
        let moments = [
            base,
            base + std::time::Duration::from_millis(450),
            base + std::time::Duration::from_millis(900),
            crate::localclock::at_hour_min(12, 15),
            crate::localclock::at_hour_min(3, 40),
        ];
        let levels = [
            crate::floor::NeonLevels::CALM,
            crate::floor::NeonLevels::ALERT,
            crate::floor::NeonLevels::EMPTY,
        ];
        for now in moments {
            for door_frame in 0..3 {
                for neon in levels {
                    let frame = SimFrame {
                        door_frame,
                        neon,
                        ..empty_frame(&layout)
                    };
                    let list = list_now(&frame, office, now);
                    same_fingerprint_same_pixels(&mut painted, &list, &layout, moving);
                    for p in list.pieces().iter().filter(|p| moving(p)) {
                        assert!(!p.kind.is_static(), "{} holds still", kind_name(&p.kind));
                        seen.entry(kind_name(&p.kind))
                            .or_default()
                            .insert(p.fingerprint);
                    }
                }
            }
        }
        for kind in ["animated", "door", "neon", "clock"] {
            let n = seen.get(kind).map_or(0, |s| s.len());
            assert!(
                n > 1,
                "the {kind} showed one fingerprint across what moves it"
            );
        }
    }

    /// A light of its own keeps its colour through the night: the sign's tube,
    /// a floor lamp's bulb, and the ceiling of an open elevator's car, while
    /// the room around them darkens.
    #[test]
    #[cfg(feature = "density-art")]
    fn what_glows_of_its_own_keeps_its_colour_at_night() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = lively_office();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let frame = SimFrame {
            door_frame: 2,
            neon: crate::floor::NeonLevels::ALERT,
            ..empty_frame(&layout)
        };
        let list = list_at(&frame, office, 23);
        let pen = Pen::for_pack(scale, &pack);
        let blank = || {
            RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
            )
        };
        let (mut night, mut raw) = (blank(), blank());
        let mut cache = CutawayCache::default();
        paint_backdrop(&layout, theme, scale, pen, &mut night);
        paint_list(&list, &mut cache, &mut night);
        paint_backdrop(&layout, theme, scale, pen, &mut raw);
        for p in list.pieces() {
            paint_piece(&p.kind, &pack, theme, scale, &mut cache, &mut raw);
        }
        assert_ne!(
            night.as_slice(),
            raw.as_slice(),
            "the night room kept its daylight"
        );
        // Every buffer pixel of each bulb-key art pixel `art` draws placed so.
        let bulbs = |placed: Placed, art: Art| -> Vec<(u16, u16)> {
            let dense = crate::embedded_pack::densest_frame(&pack, art.sprite, art.frame, scale)
                .expect("the art");
            let lit = drawn_in(&dense, &[crate::embedded_pack::DESK_BULB_KEY]);
            let (x0, y0) = placed.top_left(dense.logical, scale);
            let w = usize::from(dense.frame.width());
            let k = dense.blit_at.get();
            lit.iter()
                .enumerate()
                .filter(|&(_, &b)| b)
                .map(|(i, _)| (x0 + (i % w) as u16 * k, y0 + (i / w) as u16 * k))
                .collect()
        };
        let (mut signs, mut lamps, mut cars) = (0, 0, 0);
        for p in list.pieces() {
            let cells = match p.kind {
                PieceKind::Neon { at, .. } => {
                    signs += 1;
                    let (x0, y0) = (scale.to_buffer(at.x), scale.to_buffer(at.y));
                    let (x1, y1) = (
                        scale.to_buffer(at.x + at.width),
                        scale.to_buffer(at.y + at.height),
                    );
                    (y0..y1)
                        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                        .collect()
                }
                PieceKind::Prop { at, art } if art.sprite == "floor_lamp" => {
                    let cells = bulbs(Placed::Centred(at), art);
                    lamps += cells.len();
                    cells
                }
                PieceKind::Door { at, frame } => {
                    let art = Art {
                        sprite: DOOR_SPRITE,
                        frame,
                        flip: Flip::None,
                    };
                    let cells = bulbs(Placed::TopLeft(at), art);
                    cars += cells.len();
                    cells
                }
                _ => Vec::new(),
            };
            for (x, y) in cells {
                assert_eq!(
                    night.get(x, y),
                    raw.get(x, y),
                    "{} dimmed at ({x}, {y})",
                    kind_name(&p.kind)
                );
            }
        }
        assert!(
            signs == 1 && lamps > 0 && cars > 0,
            "sign {signs}, lamp {lamps}, car {cars}"
        );
    }

    /// At night a window's sky keeps its own light, never darkened with the
    /// room, while the sign's glow still lifts the glass beside it.
    #[test]
    fn a_window_pane_keeps_its_sky_and_takes_the_signs_glow() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = lively_office();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let frame = SimFrame {
            neon: crate::floor::NeonLevels::ALERT,
            ..empty_frame(&layout)
        };
        let list = list_at(&frame, office, 23);
        let pen = Pen::for_pack(scale, &pack);
        let blank = || {
            RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
            )
        };
        let mut cache = CutawayCache::default();
        let painted = |keep: &dyn Fn(&PieceKind) -> bool, cache: &mut _| {
            let mut buf = blank();
            paint_backdrop(&layout, theme, scale, pen, &mut buf);
            for p in list.pieces().iter().filter(|p| keep(&p.kind)) {
                paint_piece(&p.kind, &pack, theme, scale, cache, &mut buf);
            }
            buf
        };
        let backdrop = painted(&|_| false, &mut cache);
        let glass = painted(&|k| matches!(k, PieceKind::Glass { .. }), &mut cache);
        let all = painted(&|_| true, &mut cache);
        let mut night = blank();
        paint_backdrop(&layout, theme, scale, pen, &mut night);
        paint_list(&list, &mut cache, &mut night);
        let luma = pixtuoid_core::sprite::Rgb::lightness;
        let (mut kept, mut lifted) = (0, 0);
        for y in 0..night.height() {
            for x in 0..night.width() {
                let pane =
                    glass.get(x, y) != backdrop.get(x, y) && all.get(x, y) == glass.get(x, y);
                if !pane {
                    continue;
                }
                let (lit, sky) = (night.get(x, y), all.get(x, y));
                assert!(
                    luma(lit) >= luma(sky),
                    "the night darkened a pane at ({x}, {y})"
                );
                kept += usize::from(lit == sky);
                lifted += usize::from(luma(lit) > luma(sky));
            }
        }
        assert!(kept > 0 && lifted > 0, "panes kept {kept}, lifted {lifted}");
    }

    /// The clock's hands stay inside the face its dial art draws, at every
    /// reading of the day.
    #[test]
    fn the_clocks_hands_stay_on_its_face() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let at = Point { x: 2, y: 2 };
        let dial =
            crate::embedded_pack::densest_frame(&pack, CLOCK_SPRITE, 0, scale).expect("the dial");
        let face = drawn_in(&dial, &[crate::embedded_pack::CLOCK_FACE_KEY]);
        let (w, k) = (usize::from(dial.frame.width()), dial.blit_at.get());
        let on_face = |x: u16, y: u16| {
            let (ax, ay) = (
                usize::from((x - scale.to_buffer(at.x)) / k),
                usize::from((y - scale.to_buffer(at.y)) / k),
            );
            face.get(ay * w + ax).copied().unwrap_or(false)
        };
        let blank = || {
            RgbBuffer::filled(
                scale.to_buffer(12),
                scale.to_buffer(12),
                pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
            )
        };
        let mut bare = blank();
        blit_frame_scaled(
            &dial.recolorable.recolored(&theme_overrides(theme)),
            scale.to_buffer(at.x),
            scale.to_buffer(at.y),
            dial.blit_at,
            &mut bare,
        );
        let mut hands = 0;
        for minutes in (0..12 * 60).step_by(7) {
            let reading = crate::sky::ClockReading {
                hour: minutes / 60,
                minute: minutes % 60,
            };
            let mut buf = blank();
            paint_clock(at, reading, &pack, theme, scale, &mut buf);
            for y in 0..buf.height() {
                for x in 0..buf.width() {
                    if buf.get(x, y) != bare.get(x, y) {
                        hands += 1;
                        assert!(
                            on_face(x, y),
                            "{reading:?}: a hand leaves the face at ({x}, {y})"
                        );
                    }
                }
            }
        }
        assert!(hands > 0, "no hand was drawn");
    }

    /// A desk sorts on the roster's row, the classic painter's: a figure one row
    /// south of it draws over it, one row north draws under it.
    #[test]
    fn a_walker_just_south_of_a_desk_front_draws_over_it() {
        let pack = test_default_pack();
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let desk = layout
            .fixtures()
            .find(|f| matches!(f.kind, FixtureKind::Desk(_)))
            .expect("a desk");
        let Depth::Sorted { row, .. } = desk.depth else {
            panic!("a desk sorts: {desk:?}");
        };
        let [(span, PieceKind::Desk { .. })] =
            queued(&layout, &pack, RenderScale::ONE, &[], |k| k == desk.kind)[..]
        else {
            panic!("one desk piece");
        };
        assert_eq!(span.depth, row, "the desk sorts on the roster's row");
        let walker = |depth| Span::new(span.x0, span.y0, 4, 8, 0).with_depth(depth);
        for (depth, over) in [(row + 1, true), (row - 1, false)] {
            let drawn = depth_sort(vec![(span, "desk"), (walker(depth), "walker")]);
            assert_eq!(
                drawn[1] == "walker",
                over,
                "a walker sorted on {depth}, the desk on {row}"
            );
        }
    }

    /// A figure at a fixture piece's row paints over it, but for a desk chair
    /// and a sofa seen from behind, which hide their sitters; a back-view
    /// sofa's seat, which its sitter sits on, stays under.
    #[test]
    fn a_fixture_ties_a_figure_as_the_roster_says() {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let mut over = std::collections::BTreeSet::new();
        for layout in many_layouts() {
            for fixture in layout.fixtures() {
                if fixture.depth == Depth::Backdrop {
                    continue;
                }
                use FixtureKind as K;
                let hides_its_sitter = match fixture.kind {
                    K::DeskChair(_) | K::LoungeCouch => true,
                    K::MeetingSofa { faces_away, .. } => faces_away,
                    K::Desk(_)
                    | K::FilingCabinet(_)
                    | K::Station { .. }
                    | K::Plant { .. }
                    | K::Pod { .. }
                    | K::Wall { .. }
                    | K::MeetingRug { .. }
                    | K::MeetingTable { .. }
                    | K::MeetingChair { .. }
                    | K::CoatRack { .. }
                    | K::Doormat { .. }
                    | K::NoticeBoard { .. }
                    | K::LoungeRug
                    | K::SideTable
                    | K::FloorLamp
                    | K::FishTank
                    | K::KitchenIsland
                    | K::PantryMat
                    | K::IslandMat
                    | K::WaterCooler
                    | K::TrashBin
                    | K::Door
                    | K::Runner
                    | K::NeonSign
                    | K::Clock => false,
                };
                for (span, kind) in queued(&layout, &pack, scale, &[], |k| k == fixture.kind) {
                    let seat = matches!(kind, PieceKind::PropBand { rows: (0, _), .. });
                    let body = Span::new(span.x0, span.y0, 1, 1, 0);
                    let figure = occupant_span(body, span.depth, None);
                    // The figure queued first, so push order alone would draw it under.
                    let drawn = crate::cutaway::order::depth_sort(vec![
                        (figure, "figure"),
                        (span, "fixture"),
                    ]);
                    let want = if hides_its_sitter && !seat {
                        over.insert(crate::layout::roster::tests::kind_key(fixture.kind));
                        ["figure", "fixture"]
                    } else {
                        ["fixture", "figure"]
                    };
                    assert_eq!(drawn, want, "{:?}: {kind:?}", fixture.kind);
                }
            }
        }
        assert_eq!(
            over.len(),
            3,
            "the chair and both sofas hide a sitter: {over:?}"
        );
    }

    /// A glass wall band composites over whoever stands behind it at its row.
    #[test]
    fn a_wall_band_draws_over_a_figure_at_its_row() {
        let mut walls = 0;
        for layout in many_layouts() {
            let mut order = Vec::new();
            wall_segments(&layout, &mut order);
            for (span, _) in order {
                let figure = occupant_span(Span::new(span.x0, span.y0, 1, 1, 0), span.depth, None);
                let drawn =
                    crate::cutaway::order::depth_sort(vec![(span, "wall"), (figure, "figure")]);
                assert_eq!(drawn, ["figure", "wall"], "{span:?}");
                walls += 1;
            }
        }
        assert!(walls > 0, "no office had a wall");
    }

    /// The lounge couch faces the window, so the cutaway draws its back.
    #[test]
    fn the_lounge_couch_is_drawn_from_behind() {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let layout = many_layouts()
            .find(|l| l.lounge.is_some())
            .expect("an office with a lounge");
        let pieces = queued(&layout, &pack, scale, &[], |k| {
            k == FixtureKind::LoungeCouch
        });
        assert!(
            !pieces.is_empty()
                && pieces.iter().all(|(_, k)| matches!(
                    k,
                    PieceKind::PropBand { sprite, .. } if *sprite == MEETING_SOFA_NORTH
                )),
            "{pieces:?}"
        );
    }

    /// Every fixture the roster yields is drawn: queued as a piece of the list,
    /// or laid by the backdrop as a covering, over the ground. Every kind the
    /// roster has is met on some office.
    #[test]
    fn the_cutaway_draws_every_fixture_the_roster_yields() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let mut met = std::collections::BTreeSet::new();
        for layout in many_layouts() {
            let blank = || {
                RgbBuffer::filled(
                    scale.to_buffer(layout.buf_w),
                    scale.to_buffer(layout.buf_h),
                    pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
                )
            };
            let (mut bare, mut laid) = (blank(), blank());
            paint_ground(&layout, theme, pen, &mut bare);
            paint_backdrop(&layout, theme, scale, pen, &mut laid);
            for fixture in layout.fixtures() {
                met.insert(crate::layout::roster::tests::kind_key(fixture.kind));
                if covering(fixture.kind).is_some() {
                    let b = fixture.visual;
                    let (x0, y0) = (scale.to_buffer(b.x), scale.to_buffer(b.y));
                    let (x1, y1) = (
                        scale.to_buffer(b.x + b.width),
                        scale.to_buffer(b.y + b.height),
                    );
                    let covered = (y0..y1.min(bare.height()))
                        .flat_map(|y| (x0..x1.min(bare.width())).map(move |x| (x, y)))
                        .any(|(x, y)| bare.get(x, y) != laid.get(x, y));
                    assert!(covered, "{:?} lays nothing at {b:?}", fixture.kind);
                } else {
                    let pieces = queued(&layout, &pack, scale, &[], |k| k == fixture.kind);
                    assert!(!pieces.is_empty(), "{:?} queues no piece", fixture.kind);
                }
            }
        }
        assert_eq!(
            met,
            crate::layout::roster::tests::every_kind_key(),
            "a fixture kind was never met"
        );
    }

    /// The cutaway grounds exactly the fixtures the roster says stand
    /// ([`Fixture::contact`](crate::layout::Fixture::contact)): the two
    /// painters cast from one answer.
    #[test]
    fn the_cutaway_grounds_what_the_roster_says_stands() {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        for layout in many_layouts() {
            for fixture in layout.fixtures() {
                let grounded = covering(fixture.kind).is_none()
                    && queued(&layout, &pack, scale, &[], |k| k == fixture.kind)
                        .iter()
                        .any(|(span, kind)| ground_shadow(*span, kind, &pack).is_some());
                assert_eq!(
                    grounded,
                    fixture.contact().is_some(),
                    "{:?} on {}x{}",
                    fixture.kind,
                    layout.buf_w,
                    layout.buf_h
                );
            }
        }
    }

    /// Layouts across the sizes and seeds that place every kind of piece this
    /// module draws: pods with booths and desks, meeting rooms, a pantry.
    fn many_layouts() -> impl Iterator<Item = Layout> {
        // The small sizes are an 80x24-class terminal's, where a door can run
        // flush with its wall's end.
        [
            (64, 50),
            (80, 46),
            (80, 48),
            (160, 96),
            (200, 120),
            (240, 144),
            (320, 180),
        ]
        .into_iter()
        .flat_map(|(w, h)| {
            (0..4).filter_map(move |seed| Layout::compute_with_seed(w, h, None, seed))
        })
    }

    /// Pins [`push_sofa`]'s front view on its sitters' key, so they paint over
    /// it.
    #[test]
    fn a_front_view_sofa_ties_its_sitters() {
        let pack = test_default_pack();
        let sofa = crate::layout::Point { x: 40, y: 30 };
        let mut order = Vec::new();
        push_sofa(&mut order, &pack, sofa, false, Tie::FigureOver);
        let [
            (
                span,
                PieceKind::Prop {
                    art: Art {
                        flip: Flip::None, ..
                    },
                    ..
                },
            ),
        ] = order.as_slice()
        else {
            panic!("a front sofa is one prop: {order:?}");
        };
        assert_eq!(span.depth, crate::sim::seat::sofa_sitter_z_key(sofa));
    }

    /// No wall this module stands closes a doorway, and every doorway is framed
    /// by two jambs.
    #[test]
    fn walls_leave_every_doorway_open_and_frame_it() {
        let mut checked = 0;
        for layout in many_layouts() {
            let mut order = Vec::new();
            wall_segments(&layout, &mut order);
            for d in &layout.doorways {
                let vertical = d.start.x == d.end.x;
                let (lo, hi) = if vertical {
                    (d.start.y.min(d.end.y), d.start.y.max(d.end.y))
                } else {
                    (d.start.x.min(d.end.x), d.start.x.max(d.end.x))
                };
                for v in lo + 1..hi {
                    let (x, y) = if vertical {
                        (d.start.x, v)
                    } else {
                        (v, d.start.y)
                    };
                    // Its own wall's pieces only: a crossing wall's glass rising
                    // in front of the opening is occlusion, not a wall in it.
                    let blocked = order.iter().any(|(s, k)| {
                        let own = match k {
                            PieceKind::WallSeg {
                                piece: crate::layout::WallPiece::Vertical { x: px, .. },
                                ..
                            } => vertical && *px == d.start.x,
                            PieceKind::WallSeg {
                                piece: crate::layout::WallPiece::Horizontal { y_face, .. },
                                ..
                            } => !vertical && *y_face == d.start.y,
                            _ => false,
                        };
                        own && (s.x0..=s.x1).contains(&x) && (s.y0..=s.y1).contains(&y)
                    });
                    assert!(
                        !blocked,
                        "a wall stands in the doorway at ({x}, {y}): {d:?}"
                    );
                }
                checked += 1;
            }
            let jambs: std::collections::HashSet<_> = order
                .iter()
                .filter_map(|(_, k)| match k {
                    PieceKind::WallSeg { piece, .. } => Some(*piece),
                    _ => None,
                })
                .flat_map(|piece| {
                    let (at, size) = piece.visual();
                    piece.jambs().inspect(move |&(p, s)| {
                        assert!(
                            p.x >= at.x
                                && p.y >= at.y
                                && p.x + s.w <= at.x + size.w
                                && p.y + s.h <= at.y + size.h,
                            "a jamb {p:?}+{s:?} reaches past its wall {piece:?}"
                        );
                    })
                })
                .collect();
            assert_eq!(
                jambs.len(),
                2 * layout.doorways.len(),
                "two jambs a doorway"
            );
        }
        assert!(checked > 0, "the layouts cut doorways");
    }

    /// The pantry counter stands where the layout stands it: on the Pantry
    /// waypoint the mask blocks and the visitor faces.
    #[test]
    fn the_pantry_counter_stands_on_its_waypoint() {
        let pack = test_default_pack();
        let mut checked = 0;
        for layout in many_layouts() {
            let Some(wp) = layout
                .waypoints
                .iter()
                .find(|wp| wp.kind == crate::layout::WaypointKind::Pantry)
            else {
                continue;
            };
            let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
                matches!(
                    k,
                    FixtureKind::Station {
                        station: Station::PantryCounter,
                        ..
                    }
                )
            });
            let [(_, PieceKind::Prop { at, .. })] = order.as_slice() else {
                panic!("one counter: {order:?}");
            };
            assert_eq!(*at, wp.pos);
            checked += 1;
        }
        assert!(checked > 0, "the layouts have pantries");
    }

    /// No piece is queued twice, and every pod decor piece once: a phone booth
    /// or a standing desk is pod decor, and its waypoint is only where a
    /// visitor stands.
    #[test]
    fn no_piece_is_queued_twice() {
        let pack = test_default_pack();
        let mut booths = 0;
        for layout in many_layouts() {
            let order = queued(&layout, &pack, RenderScale::ONE, &[], |_| true);
            let mut seen = std::collections::HashSet::new();
            for (span, kind) in &order {
                assert!(
                    seen.insert((span.x0, span.y0, span.x1, span.y1, fingerprint(kind))),
                    "{} queued twice at {span:?}",
                    kind_name(kind)
                );
            }
            for d in &layout.pod_decor {
                let queued = order
                    .iter()
                    .filter(|(_, k)| {
                        matches!(k, PieceKind::Prop { at, art }
                            if *at == d.pos && art.sprite == d.kind.sprite_name())
                    })
                    .count();
                assert_eq!(queued, 1, "{:?} at {:?}", d.kind, d.pos);
                booths += usize::from(d.kind == crate::layout::PodDecor::PhoneBooth);
            }
        }
        assert!(booths > 0, "the layouts place a phone booth");
    }

    /// Wall decor that stands on the ground sorts among the ground's pieces, so a
    /// figure north of a whiteboard between pods goes behind it; decor that
    /// only hangs on the band is left to the backdrop.
    #[test]
    fn floor_standing_wall_decor_sorts_with_the_floor() {
        let pack = test_default_pack();
        let mut standing = 0;
        for layout in many_layouts() {
            let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
                matches!(k, FixtureKind::Wall { .. })
            });
            for item in &layout.wall_decor {
                let queued = order.iter().any(|(_, k)| {
                    matches!(k, PieceKind::Prop { art, .. } if art.sprite == item.kind.sprite_name())
                });
                assert_eq!(queued, item.kind.stands_on_floor(), "{:?}", item.kind);
                standing += usize::from(queued);
            }
        }
        assert!(standing > 0, "the layouts place floor-standing decor");
    }

    /// Splitting the back-view sofa loses and moves nothing: its bands paint
    /// the art whole.
    #[test]
    fn a_sofas_bands_paint_its_art_whole() {
        let pack = test_default_pack();
        let sofa = crate::layout::Point { x: 20, y: 10 };
        let (w, h) = base_size(&pack, MEETING_SOFA_NORTH);
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let floor = pixtuoid_core::sprite::Rgb { r: 1, g: 2, b: 3 };
            let blank =
                || RgbBuffer::filled(scale.to_buffer(w + 40), scale.to_buffer(h + 20), floor);
            let (mut whole, mut split) = (blank(), blank());
            paint_art(
                sofa,
                Art::still(MEETING_SOFA_NORTH),
                &pack,
                &crate::theme::NORMAL,
                scale,
                &mut whole,
            );
            let top = NORTH_SOFA_SEAT_ROWS;
            for rows in [(0, top), (top, h)] {
                paint_prop_band(sofa, MEETING_SOFA_NORTH, rows, &pack, scale, &mut split);
            }
            assert!(
                whole.as_slice() == split.as_slice(),
                "scale {s}: the bands differ from the art"
            );
        }
    }

    /// The split row is the art's at every density: the back-view sofa's
    /// backrest starts on its lit ridge, just under the seam where the seat
    /// meets it, so the seam paints under the sitter and the ridge over them.
    #[test]
    fn the_north_sofas_backrest_starts_on_its_lit_ridge() {
        let pack = test_default_pack();
        let densities = std::iter::once(pixtuoid_core::sprite::format::Density::ONE)
            .chain(pack.density_variants());
        for d in densities {
            let name = if d == pixtuoid_core::sprite::format::Density::ONE {
                MEETING_SOFA_NORTH.to_owned()
            } else {
                pixtuoid_core::sprite::format::density_variant_name(MEETING_SOFA_NORTH, d)
            };
            let Some(f) = pack.animation(&name).and_then(|a| a.frames().first()) else {
                continue;
            };
            let (x, split) = (f.width() / 2, NORTH_SOFA_SEAT_ROWS * d.get());
            let luma = |y: u16| {
                let c = f
                    .get(x, y)
                    .copied()
                    .flatten()
                    .expect("the sofa is opaque at its centre");
                c.lightness()
            };
            assert!(
                luma(split) > luma(split - 1),
                "{name}: row {split} must be the ridge, lit over the seam above it"
            );
        }
    }

    /// [`face_rows`]' rule, through the real paint.
    #[test]
    #[cfg(feature = "density-art")]
    fn only_the_top_down_base_desk_gets_a_derived_front_face() {
        let pack = test_default_pack();
        let floor = pixtuoid_core::sprite::Rgb { r: 1, g: 2, b: 3 };
        let at = crate::layout::Point { x: 1, y: 1 };
        let (bw, bh) = base_size(&pack, "desk");
        let span = desk_span(&pack, "desk", at, RenderScale::ONE).expect("desk is in the pack");
        for (s, face) in [(1, true), (8, false)] {
            let scale = RenderScale::new(s).expect("nonzero");
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(bw + 2 * at.x),
                scale.to_buffer(span.y0 + bh + desk_front_h() + 1),
                floor,
            );
            paint_desk(
                at,
                "desk",
                Screen::Off,
                (&pack, scale),
                &mut ArtCache::default(),
                &mut buf,
            );
            let (x, below) = (
                scale.to_buffer(at.x + bw / 2),
                scale.to_buffer(span.y0 + bh),
            );
            assert_eq!(
                buf.get(x, below) != floor,
                face,
                "scale {s}: the row under the art is {}",
                if face {
                    "the derived face"
                } else {
                    "bare floor"
                }
            );
        }
    }

    /// Every static piece's painter draws the densest variant its scale lands,
    /// not the base block-scaled: a prop (the sofa among them), wall decor and
    /// the task chair.
    #[test]
    fn static_pieces_draw_their_density_variant() {
        let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\"B\"=\"#a0b0c0\"\n\
             [animations.plant]\nframes=[\"a.sprite\"]\nframe_ms=100\n\
             [animations.\"plant@2x\"]\nframes=[\"b.sprite\"]\nframe_ms=100\n\
             [animations.whiteboard]\nframes=[\"a.sprite\"]\nframe_ms=100\n\
             [animations.\"whiteboard@2x\"]\nframes=[\"b.sprite\"]\nframe_ms=100\n\
             [animations.desk_chair]\nframes=[\"a.sprite\"]\nframe_ms=100\n\
             [animations.\"desk_chair@2x\"]\nframes=[\"b.sprite\"]\nframe_ms=100\n",
            &[
                ("a.sprite", "@frame 0\nA"),
                ("b.sprite", "@frame 0\nB B\nB B"),
            ],
        )
        .expect("pack builds");
        let variant = pixtuoid_core::sprite::Rgb {
            r: 0xa0,
            g: 0xb0,
            b: 0xc0,
        };
        let scale = RenderScale::new(2).expect("nonzero");
        let at = crate::layout::Point { x: 3, y: 3 };
        let blank = || RgbBuffer::filled(16, 16, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        let drawn = |buf: &RgbBuffer| buf.get(scale.to_buffer(at.x), scale.to_buffer(at.y));

        let mut buf = blank();
        let theme = &crate::theme::NORMAL;
        paint_art(at, Art::still("plant"), &pack, theme, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "prop");
        let mut buf = blank();
        let flipped = Art {
            flip: Flip::Vertical,
            ..Art::still("plant")
        };
        paint_art(at, flipped, &pack, theme, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "mirrored prop");
        let mut buf = blank();
        paint_wall_decor(at, "whiteboard", &pack, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "wall decor");
        let mut buf = blank();
        paint_chair(at, &pack, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "chair");
    }

    /// `frame` painted through the real cutaway at render scale `s`.
    fn render_at(
        frame: &SimFrame,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        s: u16,
    ) -> RgbBuffer {
        let scale = RenderScale::new(s).expect("nonzero");
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            theme.surface.bg_fallback,
        );
        let mut cache = CutawayCache::default();
        render_cutaway(
            frame,
            Office {
                layout,
                pack,
                theme,
                scale,
            },
            showing(
                crate::floor::FloorMeta::ground(),
                std::time::SystemTime::UNIX_EPOCH,
            ),
            &mut cache,
            &mut buf,
        );
        buf
    }

    /// The cutaway paints on ONE grid, the art pixel: at a render scale `k`
    /// times the art's density, the frame is the density render upscaled `k`
    /// times, which also makes every aligned `k`x`k` block one colour. A painter
    /// sizing a thin feature in buffer pixels, or a band edge that rounds at
    /// buffer resolution, breaks it.
    ///
    /// Every theme over a walk and a sit; the office, there to gate in a pantry
    /// and a meeting room, under one theme, since no painter branches on it.
    #[test]
    fn the_cutaway_paints_whole_art_pixels() {
        use crate::floor::{FloorMeta, FloorSession};
        let pack = test_default_pack();
        let d = pack.max_density_variant().get();
        let (walk_layout, _, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let walking = &frames[frames.len() / 2];
        let seated = frames.last().expect("a seated frame");
        let office = FloorSession::new()
            .step(
                crate::floor::FloorInputs {
                    scene: &pixtuoid_core::SceneState::uniform(16),
                    pack: &pack,
                    now: std::time::SystemTime::UNIX_EPOCH,
                    floor: FloorMeta::ground(),
                    pets: crate::floor::PetInputs::default(),
                },
                crate::layout::Size { w: 240, h: 144 },
            )
            .expect("lays out");
        assert!(
            office.layout.pantry.is_some() && !office.layout.meeting_rooms.is_empty(),
            "the office must gate in the rooms it is here to cover"
        );
        let cases = [
            ("walking", &walk_layout, walking),
            ("seated", &walk_layout, seated),
            ("office", &office.layout, &office.frame),
        ];
        for theme in crate::theme::ALL_THEMES {
            for (name, layout, frame) in cases {
                if name == "office" && theme.name != crate::theme::ALL_THEMES[0].name {
                    continue;
                }
                let at_d = render_at(frame, layout, &pack, theme, d);
                for k in [2u16, 3] {
                    let at_s = render_at(frame, layout, &pack, theme, d * k);
                    let off_upscale = (0..at_s.height())
                        .flat_map(|y| (0..at_s.width()).map(move |x| (x, y)))
                        .find(|&(x, y)| at_s.get(x, y) != at_d.get(x / k, y / k));
                    if let Some(at) = off_upscale {
                        // The first split art pixel, if any, points at the
                        // painter: one sizing a feature in buffer pixels.
                        let split = (0..at_s.height())
                            .step_by(usize::from(k))
                            .flat_map(|y| {
                                (0..at_s.width())
                                    .step_by(usize::from(k))
                                    .map(move |x| (x, y))
                            })
                            .find(|&(x, y)| {
                                let c = at_s.get(x, y);
                                (0..k).any(|dy| (0..k).any(|dx| at_s.get(x + dx, y + dy) != c))
                            });
                        panic!(
                            "{} {name} at k={k}: the frame is not the density render upscaled \
                             (first at {at:?}; first split {k}x{k} art pixel: {split:?})",
                            theme.name
                        );
                    }
                }
            }
        }
    }
}
