//! The cutaway's rasterizer: a frame's display list painted over its backdrop.

use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::blit::blit_frame_scaled;
use pixtuoid_core::sprite::format::Pack;

use crate::cutaway::pen::{ArtPx, ArtRect, Pen};
use crate::cutaway::shade::{Ramp, fill, slab};
#[cfg(test)]
use crate::display::compose::{art_size, desk_art, desk_front_h};
use crate::display::{
    Art, Badge, DOOR_SPRITE, DrawList, Figure, Flip, Ground, Office, PLATE_PAD, PieceKind, Screen,
    Showing, StoodProp, WindowView, board_runs, desk_span, drawn_in, face_rows, frame_list,
    indicator_plate,
};
use crate::effects::EffectKind;
use crate::layout::{Bounds, FixtureKind, Point, SceneLayout, Size};
use crate::render_scale::RenderScale;
use crate::sim::SimFrame;
use crate::theme::Theme;

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

/// How many ramp levels the lit glass sits below its glow colour.
const SCREEN_GLASS_LEVEL: i8 = -3;

/// How many ramp levels a lit screen's text sits above its glow colour: bright
/// lines on the dark glass.
const SCREEN_TEXT_LEVEL: i8 = 9;

impl Screen {
    /// `art` with its screen keys showing this screen.
    pub(crate) fn on(
        self,
        art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    ) -> Option<pixtuoid_core::sprite::Frame> {
        match self {
            Self::Off => None,
            Self::Standby(glass) => {
                Some(art.recolored(&[(crate::pack::SCREEN_GLASS_KEY, Some(glass))]))
            }
            Self::Lit { glow, scan } => Some(scanline(relight_screen(art, glow), art, glow, scan)),
        }
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
        crate::cutaway::text::paint(pen, buf, (ArtPx(x), plate.y), text, ink);
        x += crate::cutaway::text::advance(text).0;
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

/// Paint `list` whole: `layout`'s backdrop, then the list over it.
pub(crate) fn paint(
    layout: &SceneLayout,
    list: &DrawList<'_>,
    cache: &mut CutawayCache,
    buf: &mut RgbBuffer,
) {
    let pen = Pen::for_pack(list.scale(), list.pack());
    paint_backdrop(layout, list.theme(), list.ground(), list.scale(), pen, buf);
    paint_list(list, cache, buf);
}

/// Everything under the list's pieces, none of which moves within a layout,
/// theme, ground, pack and scale.
fn paint_backdrop(
    layout: &SceneLayout,
    theme: &Theme,
    ground: Ground,
    scale: RenderScale,
    pen: Pen,
    buf: &mut RgbBuffer,
) {
    paint_ground(layout, ground, pen, buf);
    for fixture in layout.fixtures() {
        match covering(fixture.kind) {
            Some(Covering::Rug) => paint_rug(fixture.visual, theme, pen, buf),
            Some(Covering::Runner) => paint_runner(fixture.visual, theme, pen, buf),
            None => {}
        }
    }
    paint_wall(layout, theme, scale, pen, buf);
}

/// Paint `list` over the backdrop `buf` holds: every piece's shadow
/// ([`ground_shadow`](crate::display::compose::ground_shadow)) first, then the pieces back to front as by day, noting
/// which pixels glow of their own ([`Glow`](crate::cutaway::light::Glow)), and
/// last one pass ([`net_pass`](crate::cutaway::light::net_pass)) takes every
/// other pixel to the hour: darkened with the room and lifted by its lights at
/// once, so no pixel is darkened twice or darkened and relit.
pub(crate) fn paint_list(list: &DrawList<'_>, cache: &mut CutawayCache, buf: &mut RgbBuffer) {
    let pen = Pen::for_pack(list.scale(), list.pack());
    paint_ground_shadows(
        list.pieces().iter().filter_map(|p| p.shadow),
        crate::ground::shadow_strength(list.ambient().darkness()),
        pen,
        buf,
    );
    let emission = paint_pieces(list, cache, buf);
    let lights: Vec<&crate::cutaway::light::LightView> =
        list.lights().iter().map(|l| &l.view).collect();
    let whole = ArtRect {
        x: ArtPx(0),
        y: ArtPx(0),
        w: pen.art(list.scale().logical(buf.width()).saturating_add(1)),
        h: pen.art(list.scale().logical(buf.height()).saturating_add(1)),
    };
    crate::cutaway::light::net_pass(
        whole,
        &lights,
        (list.ambient(), list.flash()),
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
    for piece in list.pieces() {
        let (x0, y0) = (
            list.scale().to_buffer(piece.span.x0),
            list.scale().to_buffer(piece.span.y0),
        );
        let x1 = list.scale().to_buffer(piece.span.x1 + 1).min(buf.width());
        let y1 = list.scale().to_buffer(piece.span.y1 + 1).min(buf.height());
        let epoch = buf.begin_writes();
        paint_piece(
            &piece.kind,
            list.pack(),
            list.theme(),
            list.scale(),
            cache,
            buf,
        );
        let glowing = mark(
            &piece.kind,
            (list.pack(), list.scale()),
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
                    | PieceKind::Creature { .. }
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
        | PieceKind::Creature { .. }
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
    use crate::pack::{DESK_BULB_KEY, SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};
    let (Some(span), Some(desk)) = (
        desk_span(pack, art_name, at, scale),
        crate::pack::densest_frame(pack, art_name, 0, scale),
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
/// ([`DESK_BULB_KEY`](crate::pack::DESK_BULB_KEY)); whether it marked
/// any.
fn mark_bulbs(
    placed: Placed,
    art: Art,
    (pack, scale): (&Pack, RenderScale),
    cache: &mut ArtCache,
    marks: &mut RgbBuffer,
) -> bool {
    let Some(dense) = crate::pack::densest_frame(pack, art.sprite, art.frame, scale) else {
        return false;
    };
    let bulbs = cache.cells(art.sprite, art.frame, &dense, &[crate::pack::DESK_BULB_KEY]);
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
        dense: &crate::pack::DenseFrame<'_>,
        keys: &'static [char],
    ) -> &[bool] {
        self.cells
            .entry((sprite, frame, dense.density.get(), keys))
            .or_insert_with(|| drawn_in(dense, keys))
    }

    fn desk(
        &mut self,
        art: &'static str,
        desk: &crate::pack::DenseFrame<'_>,
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

/// Ramp stops a shadow steps the ground at its centre, per unit of
/// [`shadow_strength`].
///
/// [`shadow_strength`]: crate::ground::shadow_strength
const SHADOW_STOPS_PER_STRENGTH: f32 = 6.0;

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
        PieceKind::Creature { at, art, degraded } => {
            paint_creature(at, art, degraded, pack, scale, buf);
        }
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
    layout: &SceneLayout,
    base_pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    now: std::time::SystemTime,
) {
    // The room darkens every pixel by the hour's steps; its lights must be off.
    let tones = crate::atmosphere::SkyTones::resolve(&crate::sky::Sky::clock(now), theme);
    let ambient = crate::cutaway::light::Ambient::of(&tones);
    let mut ground = RgbBuffer::filled(
        scale.to_buffer(layout.buf_w),
        scale.to_buffer(layout.buf_h),
        theme.surface.bg_fallback,
    );
    paint_ground(
        layout,
        Ground::of(theme, tones.ground_tint),
        Pen::for_pack(scale, base_pack),
        &mut ground,
    );
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
                crate::pack::desk_art_top(base_pack, d.y, h) + h,
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
                ground_or_its_shadow(at(variant, x, y), at(ground.as_slice(), x, y)),
                "a variant draws its own front: under its art lies floor or its shadow"
            );
        }
        assert!(
            !ground_or_its_shadow(at(base, x, below), at(ground.as_slice(), x, below)),
            "the base gets a derived face under its art"
        );
    }
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

/// Paint the north wall band: the wall, its windows' frames where
/// [`SceneLayout::window_bays`] tiles them, and its trim. The glass is list pieces
/// ([`push_windows`](crate::display::compose::push_windows)).
///
/// Its height is [`SceneLayout::wall_band_h`], not `top_margin`: the rows between
/// are floor the agents walk on, so a band drawn to `top_margin` would paint
/// over them.
fn paint_wall(
    layout: &SceneLayout,
    theme: &Theme,
    scale: RenderScale,
    pen: Pen,
    buf: &mut RgbBuffer,
) {
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
fn paint_ground(layout: &SceneLayout, ground: Ground, pen: Pen, buf: &mut RgbBuffer) {
    let Ground { lit, base, dark } = ground;

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
        crate::pack::densest_frame(pack, art_name, 0, scale),
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
    art: &crate::pack::DenseFrame<'_>,
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
/// screen KEYS ([`SCREEN_GLASS_KEY`](crate::pack::SCREEN_GLASS_KEY),
/// [`SCREEN_TEXT_KEY`](crate::pack::SCREEN_TEXT_KEY)), rather than
/// painting a band over the desk or matching a colour, lights exactly the screen
/// the art drew — at whatever density it was drawn — and no other pixel, even
/// one the same colour as the glass.
fn relight_screen(
    art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    glow: pixtuoid_core::sprite::Rgb,
) -> pixtuoid_core::sprite::Frame {
    art.recolored(&[
        (
            crate::pack::SCREEN_GLASS_KEY,
            Some(glow.ramp(SCREEN_GLASS_LEVEL)),
        ),
        (
            crate::pack::SCREEN_TEXT_KEY,
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
    use crate::pack::{SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};
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
/// `WallDecorItem.pos` is TOP-LEFT, like the classic painter's `Pivot::TopLeft`
/// z-sort rather than the centre-pinned furniture, so centring would hang every
/// board up and west of where it belongs.
fn paint_wall_decor(
    pos: crate::layout::Point,
    sprite: &str,
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = crate::pack::densest_frame(pack, sprite, 0, scale) else {
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

/// The meeting table's art, centred on its layout point, over the front face
/// [`face_rows`] derives under a base-density drawing.
fn paint_table(at: crate::layout::Point, pack: &Pack, scale: RenderScale, buf: &mut RgbBuffer) {
    let Some(table) = crate::pack::densest_frame(pack, crate::pack::MEETING_TABLE_SPRITE, 0, scale)
    else {
        return;
    };
    let (x, y) = centred_top_left(at, table.logical, scale);
    blit_frame_scaled(table.frame, x, y, table.blit_at, buf);
    paint_derived_face(
        &table,
        (x, y),
        face_rows(pack, crate::pack::MEETING_TABLE_SPRITE, scale),
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
    let Some(dense) = crate::pack::densest_frame(pack, art.sprite, art.frame, scale) else {
        return;
    };
    let themed = dense.recolorable.recolored(&theme_overrides(theme));
    let (x, y) = centred_top_left(at, dense.logical, scale);
    blit_frame_scaled(&art.flip.turn(themed), x, y, dense.blit_at, buf);
}

/// A creature's art centred on `at`, in the pack's own colours as the classic
/// draws it, greyed when `degraded`.
fn paint_creature(
    at: Point,
    art: Art,
    degraded: bool,
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(dense) = crate::pack::densest_frame(pack, art.sprite, art.frame, scale) else {
        return;
    };
    let turned = art.flip.turn(dense.frame.clone());
    let shown = if degraded {
        crate::pixel_painter::palette::degraded_frame(&turned)
    } else {
        turned
    };
    let (x, y) = centred_top_left(at, dense.logical, scale);
    blit_frame_scaled(&shown, x, y, dense.blit_at, buf);
}

/// A desk prop in the theme's cup and paper.
fn paint_desk_prop(
    prop: StoodProp,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(f) = crate::pack::densest_frame(pack, prop.sprite, prop.frame, scale) else {
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
    crate::pack::appliance_overrides(&theme.appliance)
        .into_iter()
        .chain(crate::pack::fixture_overrides(theme))
        .collect()
}

fn paint_door(at: Point, frame: usize, pack: &Pack, scale: RenderScale, buf: &mut RgbBuffer) {
    let Some(art) = crate::pack::densest_frame(pack, DOOR_SPRITE, frame, scale) else {
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
    let Some(dial) = crate::pack::densest_frame(pack, CLOCK_SPRITE, 0, scale) else {
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

/// How far the `dial` art's face ([`CLOCK_FACE_KEY`](crate::pack::CLOCK_FACE_KEY))
/// reaches from its centre along its middle row, in art pixels at density
/// `d`: the hands stay inside the rim the art draws.
fn face_radius(dial: &crate::pack::DenseFrame<'_>, d: u16) -> Option<f32> {
    let face = drawn_in(dial, &[crate::pack::CLOCK_FACE_KEY]);
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
        crate::layout::anchored_top_left(crate::layout::Pivot::Center, at, logical.0, logical.1);
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
    let Some(dense) = crate::pack::densest_frame(pack, sprite, 0, scale) else {
        return;
    };
    let (w, h) = dense.logical;
    let tl = crate::layout::anchored_top_left(crate::layout::Pivot::Center, at, w, h);
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
    let Some(art) = crate::pack::densest_frame(pack, crate::pack::DESK_CHAIR_SPRITE, 0, scale)
    else {
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
    use crate::anim::Motion;
    use crate::atmosphere::Moment;
    use crate::display::compose::tests::{
        base_size, empty_frame, every_effect, kind_name, list_at, list_now, lively_office,
        many_layouts, moment_at, queued, quiet_board, showing, sit_down,
    };
    use crate::display::compose::{
        DESK_CUP_SPRITE, MEETING_SOFA_NORTH, NORTH_SOFA_SEAT_ROWS, PLATE_H, TOKEN_SHEET_SPRITE,
        TOKEN_TOWER_SPRITE, build_list, ground_shadow, push_windows,
    };
    use crate::display::{Piece, Span, fingerprint};
    use crate::glass_weather::GlassWeather;
    use crate::pack::test_default_pack;

    /// Relighting recolors the screen KEYS and nothing else — not even a pixel
    /// of another key the same colour as the glass — so the glow is exactly the
    /// screen the art drew, at whatever density.
    #[test]
    fn a_lit_screen_relights_only_the_screen_keys() {
        let glass = crate::pack::SCREEN_GLASS_KEY;
        let text = crate::pack::SCREEN_TEXT_KEY;
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
                let desk = crate::pack::densest_frame(&pack, art, 0, scale).expect("art");
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
                    let f = crate::pack::densest_frame(&pack, prop.sprite, prop.frame, scale)
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
                let art = crate::pack::densest_frame(&pack, sprite, frame, scale)
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

    /// Pins the screen keys ([`SCREEN_GLASS_KEY`](crate::pack::SCREEN_GLASS_KEY),
    /// [`SCREEN_TEXT_KEY`](crate::pack::SCREEN_TEXT_KEY)) to the bundled
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

    /// The seat side is the LAYOUT's to decide, and both profiles read it.
    #[test]
    fn both_profiles_seat_an_occupant_on_the_side_the_layout_chose() {
        use crate::layout::{CHARACTER_SPRITE_W, Facing};
        let desk = crate::layout::Point { x: 40, y: 30 };
        let near = crate::sim::seated_top_left(desk, CHARACTER_SPRITE_W, Facing::North);
        let far = crate::sim::seated_top_left(desk, CHARACTER_SPRITE_W, Facing::South);
        assert_eq!(
            near.y, desk.y,
            "a back-turned occupant's shared top-left lands on desk.y"
        );
        assert!(
            far.y < near.y,
            "a viewer-facing occupant sits BEHIND the desk, a back-turned one in \
             front: far {far:?}, near {near:?}"
        );
        assert_eq!(far.x, near.x, "the seat side never moves the centring");
    }

    /// A plate's runs and the board's segments step on one grid, wide
    /// characters included: the run after `text` starts
    /// [`advance`](crate::cutaway::text::advance)`(text)` on.
    #[test]
    fn plate_runs_and_board_columns_share_one_grid() {
        use crate::board::{BoardSegment, BoardTone};
        use crate::cutaway::text::advance;
        use pixtuoid_core::sprite::Rgb;
        let pen = Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4");
        let (first, second) = ("I日b", "I");
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
        assert_eq!(x1.0 - x0.0, advance(first).0, "the board");
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
        assert_eq!(lb - la, advance(first).0, "the plate");
    }

    /// The layout leaves walkable rows between the wall band and `top_margin`.
    #[test]
    fn the_wall_band_stops_where_the_layout_says_the_ground_begins() {
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let band_h = layout.wall_band_h();
        assert!(band_h > 0, "a laid-out office has a wall band");
        assert!(
            band_h < layout.top_margin,
            "the band must end ABOVE top_margin, leaving walkable rows: \
             band {band_h}, top_margin {}",
            layout.top_margin
        );
    }

    /// The ground is tiled on its own grid, from the wall's foot: a seam sits
    /// under the tile beside it, on the lit ground and the dark alike.
    #[test]
    fn the_ground_is_tiled_from_the_wall_foot() {
        let layout = SceneLayout::compute_with_seed(160, 110, None, 0).expect("lays out");
        assert_ne!(
            layout.wall_band_h() % GROUND_TILE,
            0,
            "a wall foot off the buffer's own grid, or one anchored at row 0 passes too"
        );
        let s = 8;
        let (pen, buf) = ground(&layout, s, 4);
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
    fn a_1x_ground_has_no_seams() {
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let (_, buf) = ground(&layout, 1, 1);
        let y = layout.wall_band_h() + 1;
        assert!(
            (0..buf.width()).all(|x| buf.get(x, y) == crate::theme::NORMAL.surface.carpet_light),
            "row {y} of the lit zone is unbroken"
        );
    }

    /// The ground alone at scale `s`, drawn from art at density `d`, in the
    /// normal theme.
    fn ground(layout: &SceneLayout, s: u16, d: u16) -> (Pen, RgbBuffer) {
        let scale = RenderScale::new(s).expect("nonzero");
        let pen = Pen::new(scale, d).expect("d divides s");
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
        );
        paint_ground(layout, Ground::plain(&crate::theme::NORMAL), pen, &mut buf);
        (pen, buf)
    }

    /// Every rug in the office lies on the ground, the lounge's among them.
    #[test]
    fn every_rug_lies_on_the_ground() {
        let pack = test_default_pack();
        let theme = &crate::theme::NORMAL;
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let f = &theme.furniture;
        let (mut trios, mut lounges) = (0, 0);
        for (w, h) in [(160, 96), (200, 120), (240, 144), (480, 270)] {
            for seed in 0..3 {
                let layout = SceneLayout::compute_with_seed(w, h, None, seed).expect("lays out");
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
                paint_backdrop(
                    &layout,
                    theme,
                    Ground::plain(theme),
                    scale,
                    Pen::for_pack(scale, &pack),
                    &mut buf,
                );
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
    fn a_shadow_short_of_one_stop_still_darkens_the_ground() {
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
    fn a_shadow_steps_the_ground_it_falls_on_darker_toward_its_centre() {
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
            |c, ground: pixtuoid_core::sprite::Rgb| (0..=deepest).any(|k| c == ground.ramp(-k));
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                let ground = if x < buf.width() / 2 { WEST } else { EAST };
                assert!(
                    stepped(buf.get(x, y), ground),
                    "({x}, {y}) is its ground, stepped"
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

    /// Every piece's shadow falls inside its [`Piece::reach`], so a repaint of
    /// the pieces whose reach meets a damaged rect leaves no stale shadow.
    #[test]
    fn a_shadow_falls_inside_its_pieces_reach() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let ground = pixtuoid_core::sprite::Rgb {
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
                Motion::Full.clock(std::time::UNIX_EPOCH),
            ),
            crate::floor::FloorMeta::ground(),
            quiet_board(),
        );
        let mut cast = 0;
        for piece in list.pieces() {
            let Some(shadow) = piece.shadow else { continue };
            cast += 1;
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                ground,
            );
            paint_ground_shadows(
                std::iter::once(shadow),
                crate::ground::shadow_strength(NOON_DARKNESS),
                Pen::for_pack(scale, &pack),
                &mut buf,
            );
            let reach = piece.reach();
            for (i, &c) in buf.as_slice().iter().enumerate() {
                if c != ground {
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

    /// The windows stand where the layout tiles them, as the classic painter's
    /// do, and their glass shows the one city over the classic's sky — its
    /// disc and stars included — art pixel for art pixel, the frame untouched.
    #[test]
    fn the_windows_look_out_on_the_one_city_where_the_layout_tiles_them() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
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
            paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut buf);
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

    #[test]
    fn the_wall_between_two_windows_is_one_frame_post() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            theme.surface.bg_fallback,
        );
        paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut buf);
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

    /// At the art's density the waiting mark and a z float clear of their
    /// figure's badge however far the z has risen, the z only climbs and drifts
    /// away, and the dust lies along the stepping foot's row.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_dense_looks_keep_their_places() {
        looks_keep_their_places(test_default_pack().max_density_variant().get());
    }

    /// [`the_dense_looks_keep_their_places`] on the base art, which draws its own
    /// z and mark beside the head.
    #[test]
    fn the_base_looks_keep_their_places() {
        looks_keep_their_places(1);
    }

    fn looks_keep_their_places(s: u16) {
        use crate::effects::EffectKind as K;
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let scale = RenderScale::new(s).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let mut frame = frames.last().expect("a seated frame").clone();
        for c in &mut frame.characters {
            c.effects = every_effect(c.top_left);
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
            "at scale {s} the mark {mark:?} lands on the badge {badge:?}"
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
                "at scale {} the z at {phase} ms lands on the badge {badge:?}",
                scale.get()
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

    /// A storm's strike lifts the whole frame by whole ramp steps, what glows
    /// of its own too, and its window glass further, the bolt's. That only a
    /// storm strikes is the model's: `a_strike_flashes_at_its_bucket_offset_and_ends_with_the_flash`.
    #[test]
    fn a_strike_lifts_the_room_and_its_glass_most() {
        use crate::sky::{Sky, Weather};
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pack = test_default_pack();
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let frame = empty_frame(&layout);
        let now = crate::localclock::at_hour(23);
        let lift = crate::cutaway::light::FLASH_MAX_STEPS as i8;
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            };
            let drawn = |weather, flash| {
                let sky = Sky::at_with(now, weather).with_flash(flash);
                let list = build_list(
                    &frame,
                    office,
                    &Moment::resolve(sky, theme, 0.0, Motion::Full.clock(now)),
                    crate::floor::FloorMeta::ground(),
                    quiet_board(),
                );
                let mut buf = RgbBuffer::filled(
                    scale.to_buffer(layout.buf_w),
                    scale.to_buffer(layout.buf_h),
                    theme.surface.bg_fallback,
                );
                paint(&layout, &list, &mut CutawayCache::default(), &mut buf);
                let glass: Vec<Span> = list
                    .pieces()
                    .iter()
                    .filter(|p| matches!(p.kind, PieceKind::Glass { .. }))
                    .map(|p| p.span)
                    .collect();
                (buf, glass)
            };
            let (calm, glass) = drawn(Weather::Storm, 0.0);
            let (strike, _) = drawn(Weather::Storm, 1.0);
            let in_glass = |x: u16, y: u16| {
                let (lx, ly) = (x / s, y / s);
                glass
                    .iter()
                    .any(|g| (g.x0..=g.x1).contains(&lx) && (g.y0..=g.y1).contains(&ly))
            };
            let mut bolted = 0;
            for y in 0..calm.height() {
                for x in 0..calm.width() {
                    let (c, f) = (calm.get(x, y), strike.get(x, y));
                    if in_glass(x, y) {
                        bolted += usize::from(f.lightness() > c.ramp(lift).lightness());
                    } else {
                        assert_eq!(f, c.ramp(lift), "at scale {s}, ({x}, {y}) took no strike");
                    }
                }
            }
            assert!(
                bolted > 0,
                "at scale {s} the bolt lit no glass past the room"
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
                c.top_left,
                8,
                Motion::Full.beat(std::time::UNIX_EPOCH),
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

    /// `list` painted as by day, over its backdrop and shadows, with each
    /// pixel's glow: what the net pass starts from.
    pub(crate) fn by_day(
        list: &DrawList<'_>,
        layout: &SceneLayout,
    ) -> (RgbBuffer, crate::cutaway::light::Emission) {
        let mut buf = RgbBuffer::filled(
            list.scale().to_buffer(layout.buf_w),
            list.scale().to_buffer(layout.buf_h),
            list.theme().surface.bg_fallback,
        );
        let pen = Pen::for_pack(list.scale(), list.pack());
        paint_backdrop(
            layout,
            list.theme(),
            list.ground(),
            list.scale(),
            pen,
            &mut buf,
        );
        paint_ground_shadows(
            list.pieces().iter().filter_map(|p| p.shadow),
            crate::ground::shadow_strength(list.ambient().darkness()),
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
        assert!(list.ambient().darkness() > 0.0, "23:00 is dark");
        let (day, emission) = by_day(&list, &layout);
        let mut cache = CutawayCache::default();
        let mut night = RgbBuffer::filled(day.width(), day.height(), theme.surface.bg_fallback);
        paint_backdrop(
            &layout,
            theme,
            list.ground(),
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
                        list.ambient().on(day.get(x, y)),
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
                    f32::from(lift) <= list.ambient().darkness() * 4.0,
                    "({x}, {y}) is lifted {lift} steps past the night"
                );
            }
        }
        assert!(
            unlit > 0 && under_walls > 0,
            "no unlit wall glass was checked"
        );
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

    /// Record each of `list`'s pieces `keep` selects by (scale, span,
    /// fingerprint), asserting one that recurs paints the pixels it painted
    /// before; returns how many recurred.
    pub(crate) fn same_fingerprint_same_pixels(
        painted: &mut std::collections::HashMap<(u16, Span, u64), u64>,
        list: &DrawList<'_>,
        layout: &SceneLayout,
        keep: impl Fn(&Piece) -> bool,
    ) -> usize {
        let mut repeats = 0;
        for piece in list.pieces().iter().filter(|p| keep(p)) {
            let pixels =
                painted_alone(&piece.kind, layout, list.pack(), list.theme(), list.scale());
            let s = list.scale().get();
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

    /// The two fills a piece is painted over to tell what it writes.
    pub(crate) const UNDER: [pixtuoid_core::sprite::Rgb; 2] = [
        pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 },
        pixtuoid_core::sprite::Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
    ];

    /// `kind` painted alone over each of two opposite fills: where the two
    /// buffers agree, the piece wrote the pixel.
    pub(crate) fn painted_over_two_fills(
        kind: &PieceKind,
        layout: &SceneLayout,
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
    pub(crate) fn painted_alone(
        kind: &PieceKind,
        layout: &SceneLayout,
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

    /// The first logical pixel `kind` writes outside `span`, painted alone.
    pub(crate) fn stray_pixel(
        kind: &PieceKind,
        span: Span,
        layout: &SceneLayout,
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
    fn the_walls_contact_row_is_the_ground_a_shade_down() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let layout = SceneLayout::compute_with_seed(240, 144, None, 0).expect("lays out");
        let blank = || {
            RgbBuffer::filled(
                scale.to_buffer(layout.buf_w),
                scale.to_buffer(layout.buf_h),
                UNDER[0],
            )
        };
        let (mut ground, mut laid) = (blank(), blank());
        paint_ground(&layout, Ground::plain(theme), pen, &mut ground);
        paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut laid);
        let row = scale.to_buffer(layout.wall_band_h());
        for x in 0..ground.width() {
            assert_eq!(
                laid.get(x, row),
                ground
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

    pub(crate) fn lowest_painted_row(
        kind: &PieceKind,
        layout: &SceneLayout,
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

    /// The table's span ends on the last row it paints, at the base density
    /// (with the face derived under it) and at the densest (whose art draws its
    /// own front): a span reaching past it would sort the table and cast its
    /// shadow rows below where it stands.
    #[test]
    fn the_tables_span_ends_where_it_paints() {
        let pack = test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
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
        paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut night);
        paint_list(&list, &mut cache, &mut night);
        paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut raw);
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
            let dense =
                crate::pack::densest_frame(&pack, art.sprite, art.frame, scale).expect("the art");
            let lit = drawn_in(&dense, &[crate::pack::DESK_BULB_KEY]);
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
            paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut buf);
            for p in list.pieces().iter().filter(|p| keep(&p.kind)) {
                paint_piece(&p.kind, &pack, theme, scale, cache, &mut buf);
            }
            buf
        };
        let backdrop = painted(&|_| false, &mut cache);
        let glass = painted(&|k| matches!(k, PieceKind::Glass { .. }), &mut cache);
        let all = painted(&|_| true, &mut cache);
        let mut night = blank();
        paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut night);
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
        let dial = crate::pack::densest_frame(&pack, CLOCK_SPRITE, 0, scale).expect("the dial");
        let face = drawn_in(&dial, &[crate::pack::CLOCK_FACE_KEY]);
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
            paint_ground(&layout, Ground::plain(theme), pen, &mut bare);
            paint_backdrop(&layout, theme, Ground::plain(theme), scale, pen, &mut laid);
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

    /// Splitting the back-view sofa loses and moves nothing: its bands paint
    /// the art whole.
    #[test]
    fn a_sofas_bands_paint_its_art_whole() {
        let pack = test_default_pack();
        let sofa = crate::layout::Point { x: 20, y: 10 };
        let (w, h) = base_size(&pack, MEETING_SOFA_NORTH);
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let ground = pixtuoid_core::sprite::Rgb { r: 1, g: 2, b: 3 };
            let blank =
                || RgbBuffer::filled(scale.to_buffer(w + 40), scale.to_buffer(h + 20), ground);
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

    /// [`face_rows`]' rule, through the real paint.
    #[test]
    #[cfg(feature = "density-art")]
    fn only_the_top_down_base_desk_gets_a_derived_front_face() {
        let pack = test_default_pack();
        let ground = pixtuoid_core::sprite::Rgb { r: 1, g: 2, b: 3 };
        let at = crate::layout::Point { x: 1, y: 1 };
        let (bw, bh) = base_size(&pack, "desk");
        let span = desk_span(&pack, "desk", at, RenderScale::ONE).expect("desk is in the pack");
        for (s, face) in [(1, true), (8, false)] {
            let scale = RenderScale::new(s).expect("nonzero");
            let mut buf = RgbBuffer::filled(
                scale.to_buffer(bw + 2 * at.x),
                scale.to_buffer(span.y0 + bh + desk_front_h() + 1),
                ground,
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
                buf.get(x, below) != ground,
                face,
                "scale {s}: the row under the art is {}",
                if face {
                    "the derived face"
                } else {
                    "bare ground"
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
        layout: &SceneLayout,
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
