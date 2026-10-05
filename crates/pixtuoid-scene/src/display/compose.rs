//! Composing a frame's [`DisplayList`], the second reader of `SimFrame`. Of the
//! sim's effects it lists those riding on people and creatures
//! ([`effects`](crate::display::effects)); steam stays with the classic pass.
//! It never advances the sim; a mover here would desync the profiles.

use pixtuoid_core::sprite::format::Pack;

use super::{
    Art, DisplayList, Figure, Flip, Layer, LightPiece, Piece, PieceKind, Screen, Span, StoodProp,
    depth_sort, fingerprint,
};
use crate::atmosphere::Moment;
use crate::display::pen::{ArtPx, ArtRect, Pen};
use crate::display::text::{Align, LABEL_GAP, TextRole, TextRun};
use crate::glass_weather::GlassWeather;
use crate::layout::{
    Bounds, DESK_H, Depth, Fixture, FixtureKind, Point, SceneLayout, Station, Tie,
};
use crate::pack::{
    DESK_CUP_SPRITE, DOOR_SPRITE, MEETING_SOFA_NORTH_SPRITE, NORTH_SOFA_SEAT_ROWS,
    TOKEN_SHEET_SPRITE, TOKEN_TOWER_SPRITE,
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

/// Art pixels between a plate's sides or bottom and its text ([`PLATE_H`] says why not the top).
pub(crate) const PLATE_PAD: u16 = 1;

/// A plate's height on the art grid: padded below only, since the line's
/// accent rows already clear its capitals above.
pub(crate) const PLATE_H: u16 = crate::display::text::LINE_H + PLATE_PAD;

/// A plate around `text` on the art grid, centred on column `centre`, its top
/// at row `top`.
fn plate_at(centre: ArtPx, top: ArtPx, text: &str) -> ArtRect {
    let w = crate::display::text::width(text)
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

/// A badge `run`'s plate on the art grid: centred over its anchor,
/// [`LABEL_GAP`] rows up.
pub(crate) fn badge_plate(run: &TextRun, pen: Pen) -> ArtRect {
    let bottom = pen.art(run.at.y.saturating_sub(LABEL_GAP));
    plate_at(
        pen.art(run.at.x),
        ArtPx(bottom.0.saturating_sub(PLATE_H)),
        &run.text(),
    )
}

/// Where `run`'s line lands on the art grid: the plate a badge or the floor
/// indicator sits on, else the box its glyphs ink. The indicator's plate fills
/// the cell the classic writes it across, not a badge's [`PLATE_H`], which
/// would run into the door below; under the pack's density the cell is
/// shorter than a line, and the plate keeps the line.
pub(crate) fn run_rect(run: &TextRun, pen: Pen) -> ArtRect {
    use crate::display::text::{LINE_H, advance, width};
    let text = run.text();
    let cell = pen.art(crate::layout::CELL_ROWS).0;
    match run.align {
        Align::Over => badge_plate(run, pen),
        Align::Centre => ArtRect {
            h: ArtPx(cell.max(LINE_H)),
            ..plate_at(pen.art(run.at.x), pen.art(run.at.y), &text)
        },
        Align::Left | Align::Right => {
            let x = pen.art(run.at.x).0;
            let x = match run.align {
                Align::Right => x.saturating_sub(advance(&text).0),
                _ => x,
            };
            ArtRect {
                x: ArtPx(x),
                y: ArtPx(pen.art(run.at.y).0 + cell.saturating_sub(LINE_H) / 2),
                w: ArtPx(width(&text).0.max(1)),
                h: ArtPx(LINE_H),
            }
        }
    }
}

/// What a cutaway frame is drawn with and the next one is too: the office
/// itself, where the sky's look, `altitude` and `now` are what move from frame
/// to frame.
#[derive(Debug, Clone, Copy)]
pub struct Office<'a> {
    /// Where everything stands, in LOGICAL units.
    pub layout: &'a SceneLayout,
    /// The art that draws it.
    pub pack: &'a Pack,
    /// Its colours.
    pub theme: &'a Theme,
    /// Buffer pixels per logical unit.
    pub scale: RenderScale,
}

/// Which floor a frame shows, when, and what its wall board says: what moves
/// a frame beyond its office and the sim's world.
#[derive(Debug, Clone, Copy)]
pub struct Showing<'a> {
    /// The floor of the building it shows.
    pub floor: crate::floor::FloorMeta,
    /// The wall-clock instant: the sky, the room's light, the board's flap.
    pub now: std::time::SystemTime,
    /// The wall board, the classic painter's
    /// ([`build_board`](crate::neon_sign::build_board)).
    pub board: &'a crate::neon_sign::BoardModel,
}

/// `frame`'s [`DisplayList`] as `showing` says.
pub(crate) fn compose<'a>(
    frame: &SimFrame,
    office: Office<'a>,
    Showing { floor, now, board }: Showing<'_>,
) -> DisplayList<'a> {
    let timing = floor.motion.timing(now);
    let moment = Moment::resolve(
        crate::sky::Sky::at(timing, floor.weather),
        office.theme,
        floor.altitude,
        timing,
    );
    compose_at(frame, office, &moment, floor, board)
}

/// Compose `frame`'s [`DisplayList`] at `moment`, on `floor`. Every
/// figure, window view and light is resolved here, so painting the list reads
/// neither `frame` nor the moment again.
pub(crate) fn compose_at<'a>(
    frame: &SimFrame,
    office: Office<'a>,
    moment: &Moment,
    floor: crate::floor::FloorMeta,
    board: &crate::neon_sign::BoardModel,
) -> DisplayList<'a> {
    let Office {
        pack, theme, scale, ..
    } = office;
    let ambient = crate::display::light::Ambient::of(&moment.look);
    let mut collected = collect_pieces(frame, office, moment);
    collected.extend(signs(office, floor.floor_idx, board));
    let sorted = depth_sort(
        collected
            .into_iter()
            .map(|(span, kind)| (span, (span, kind)))
            .collect(),
    );
    let pieces: Vec<Piece> = sorted
        .into_iter()
        .map(|(span, kind)| Piece {
            span,
            fingerprint: fingerprint(&kind),
            shadow: ground_shadow(span, &kind, pack),
            kind,
        })
        .collect();
    DisplayList {
        lights: lights(frame, office, moment, floor.floor_idx, ambient),
        ambient,
        carpet: moment.look.carpet(theme),
        flash: crate::display::light::Flash::of(&moment.sky),
        flash_phase: crate::flash::FlashPhase::of(&moment.sky, frame),
        hovers: pieces.iter().filter_map(Piece::hover).collect(),
        pieces,
        pack,
        theme,
        scale,
    }
}

/// Each chitchat bubble over its speaker's badge, among the badges `order`
/// already holds.
fn push_bubbles(frame: &SimFrame, office: Office<'_>, order: &mut Vec<(Span, PieceKind)>) {
    let pen = Pen::for_pack(office.scale, office.pack);
    let bubbles: Vec<_> = frame
        .chitchat_bubbles
        .iter()
        .filter_map(|bubble| {
            let badge_at = order.iter().find_map(|(_, kind)| match kind {
                PieceKind::Text { run } if run.role == TextRole::Badge(bubble.speaker) => {
                    Some(run.at)
                }
                _ => None,
            })?;
            Some(TextRun::bubble(bubble, badge_at, office.theme))
        })
        .map(|run| {
            (
                topmost_span(run_rect(&run, pen), pen),
                PieceKind::Text { run },
            )
        })
        .collect();
    order.extend(bubbles);
}

/// The pet and the gateway mascots, each a figure sorted on its feet's row
/// as the classic sorts it, with what rides on it straight after.
fn push_creatures(frame: &SimFrame, office: Office<'_>, order: &mut Vec<(Span, PieceKind)>) {
    let Office {
        pack, theme, scale, ..
    } = office;
    let pet = frame.pet.iter().map(|p| {
        let flip = if p.flip { Flip::Horizontal } else { Flip::None };
        (
            p.pos,
            p.anim_name,
            p.frame_idx,
            flip,
            false,
            &p.effects,
            Some(p.target()),
        )
    });
    let mascots = frame.mascots.iter().map(|m| {
        (
            m.pos,
            m.anim_name,
            m.frame_idx,
            Flip::None,
            m.degraded,
            &m.effects,
            m.target(),
        )
    });
    for (at, sprite, frame_idx, flip, degraded, effects, who) in pet.chain(mascots) {
        let Some(dense) = crate::pack::densest_frame(pack, sprite, frame_idx, RenderScale::ONE)
        else {
            continue;
        };
        let (w, h) = dense.logical;
        let depth = crate::layout::sort_row_at(crate::layout::Pivot::Center, at, h);
        let span = piece_span(crate::layout::Pivot::Center, at, w, h, 0)
            .with_depth(depth)
            .with_layer(Layer::Figure);
        let art = Art {
            sprite,
            frame: frame_idx,
            flip,
        };
        order.push((
            span,
            PieceKind::Creature {
                at,
                art,
                degraded,
                who,
            },
        ));
        let Some(pen) = crate::pack::densest_frame(pack, sprite, frame_idx, scale)
            .and_then(|d| Pen::new(scale, d.density.get()))
        else {
            continue;
        };
        for &effect in effects {
            let riding = crate::display::effects::Riding {
                effect,
                head: None,
                pen,
            };
            if let Some(s) = riding.span(theme, depth) {
                order.push((s, PieceKind::Effect(riding)));
            }
        }
    }
}

/// The room's own lights (`crate::lighting`) this frame that the cutaway paints.
fn lights(
    frame: &SimFrame,
    office: Office<'_>,
    moment: &Moment,
    floor_idx: usize,
    ambient: crate::display::light::Ambient,
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
            beat: moment.timing.beat,
            bulbs: crate::lighting::DeskBulbs::of(pack),
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
                .zip(crate::pack::desk_art_name(pack, facing))
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
            crate::display::light::LightView::of(
                e,
                crate::display::light::tint_of(e.kind, theme, frame.neon),
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

/// The layout cell of the desk lamp's bulb the desk `art_name` at `at` draws at
/// `scale`: the middle of its [`DESK_BULB_KEY`](crate::pack::DESK_BULB_KEY)
/// pixels, or `None` for art that draws no bulb.
fn desk_bulb(
    at: crate::layout::Point,
    art_name: &str,
    pack: &Pack,
    scale: RenderScale,
) -> Option<crate::layout::Point> {
    let span = desk_span(pack, art_name, at, scale)?;
    let (x, y) = crate::pack::bulb_cell(&crate::pack::densest_frame(pack, art_name, 0, scale)?)?;
    Some(crate::layout::Point {
        x: span.x0 + x,
        y: span.y0 + y,
    })
}

/// Where a piece meets the ground, as the shadow it casts there: on the row under
/// its south edge, a standing figure's under its feet. A sitter is grounded by
/// what they sit on, which casts its own: a desk chair a sitter carries, under
/// the chair. Walls, windows, the elevator and what hangs on a wall meet
/// no ground.
pub(crate) fn ground_shadow(
    span: Span,
    kind: &PieceKind,
    pack: &Pack,
) -> Option<crate::ground::Contact> {
    let under = |s: Span| {
        Some(crate::ground::Contact::under(
            s.x0,
            s.x1 - s.x0 + 1,
            s.y1 + 1,
        ))
    };
    match *kind {
        PieceKind::WallSeg { .. }
        | PieceKind::Window { .. }
        | PieceKind::DeskFront { .. }
        | PieceKind::Hung { .. }
        | PieceKind::Door { .. }
        | PieceKind::Neon { .. }
        | PieceKind::Clock { .. }
        | PieceKind::Effect(_)
        | PieceKind::Text { .. } => None,
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
                let (w, h) = art_size(pack, crate::pack::DESK_CHAIR_SPRITE)?;
                under(Span::new(at.x, at.y, w, h, 0))
            }
        }
        // Only the band that reaches the prop's foot meets the ground.
        PieceKind::PropBand { sprite, rows, .. } => art_size(pack, sprite)
            .filter(|&(_, h)| rows.1 == h)
            .and_then(|_| under(span)),
        PieceKind::Desk { .. }
        | PieceKind::DeskProp(_)
        | PieceKind::Chair { .. }
        | PieceKind::Table { .. }
        | PieceKind::Prop { .. }
        | PieceKind::Animated { .. }
        | PieceKind::Creature { .. } => under(span),
    }
}

/// The room's signs: the wall board's lines, and the floor indicator naming
/// floor `floor_idx`'s number over the elevator.
fn signs(
    office: Office<'_>,
    floor_idx: usize,
    board: &crate::neon_sign::BoardModel,
) -> Vec<(Span, PieceKind)> {
    let pen = Pen::for_pack(office.scale, office.pack);
    let indicator = TextRun::indicator(office.layout.door, floor_idx + 1, office.theme);
    let mut drawn: Vec<(u16, ArtRect)> = Vec::new();
    board
        .runs(office.theme)
        .into_iter()
        .chain([indicator])
        .filter_map(|run| {
            let rect = run_rect(&run, pen);
            // A run yields to one before it on its line: on the base art's grid
            // the pixel font is too wide for the sign, and the star would write
            // over the brand (`no_run_overprints_another_on_its_line`).
            if drawn.iter().any(|&(y, r)| y == run.at.y && meets(r, rect)) {
                return None;
            }
            drawn.push((run.at.y, rect));
            Some((topmost_span(rect, pen), PieceKind::Text { run }))
        })
        .collect()
}

/// The logical cells `run`'s line takes on `pen`'s grid ([`run_rect`]): where
/// a pointer names a run the cutaway drew.
pub(crate) fn run_box(run: &TextRun, pen: Pen) -> Bounds {
    let r = run_rect(run, pen);
    let d = pen.art(1).0;
    let (x, y) = (pen.logical(r.x), pen.logical(r.y));
    Bounds {
        x,
        y,
        width: (r.x.0 + r.w.0).div_ceil(d) - x,
        height: (r.y.0 + r.h.0).div_ceil(d) - y,
    }
}

/// Whether `a` and `b` share an art pixel.
fn meets(a: ArtRect, b: ArtRect) -> bool {
    a.x.0 < b.x.0 + b.w.0 && b.x.0 < a.x.0 + a.w.0 && a.y.0 < b.y.0 + b.h.0 && b.y.0 < a.y.0 + a.h.0
}

/// Every piece of the office, each with its [`Span`]. At one depth and layer,
/// push order breaks the tie, so it is part of the result.
fn collect_pieces(frame: &SimFrame, office: Office<'_>, moment: &Moment) -> Vec<(Span, PieceKind)> {
    let layout = office.layout;
    let inputs = ComposeInputs {
        frame,
        office,
        moment,
    };
    let mut order: Vec<(Span, PieceKind)> = Vec::new();
    push_windows(office, moment, &GlassWeather::of(moment), &mut order);
    let carried = push_characters(frame, office, moment.timing.now, &mut order);
    push_bubbles(frame, office, &mut order);
    push_creatures(frame, office, &mut order);
    for fixture in layout.fixtures() {
        push_fixture(fixture, inputs, &carried, &mut order);
    }
    wall_segments(layout, &mut order);
    order
}

#[derive(Clone, Copy)]
struct ComposeInputs<'a, 'f> {
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

/// Queue one of the roster's fixtures, sorted on its depth ([`sort_row`]). A
/// desk chair whose desk is in `carried` rides its sitter's piece instead.
fn push_fixture(
    fixture: Fixture,
    inputs: ComposeInputs<'_, '_>,
    carried: &[Point],
    order: &mut Vec<(Span, PieceKind)>,
) {
    use FixtureKind as K;
    let ComposeInputs {
        frame,
        office,
        moment,
    } = inputs;
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
        K::Desk(i) => push_desk(i, inputs, depth, order),
        K::FilingCabinet(_) => push_art(
            order,
            pack,
            centre,
            Art::still("filing_cabinet"),
            depth,
            Playback::Held,
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
                            Playback::Held,
                        );
                    }
                }
                Station::SnackShelf => push_art(
                    order,
                    pack,
                    wp.pos,
                    Art::still("snack_shelf"),
                    depth,
                    Playback::Held,
                ),
                Station::VendingMachine | Station::Printer => {
                    let Some(sprite) = crate::pack::appliance_sprite(wp.kind) else {
                        return;
                    };
                    let Some(anim) = pack.animation(sprite) else {
                        return;
                    };
                    let busy = frame.occupied_waypoints.contains(&waypoint);
                    let art = Art {
                        sprite,
                        frame: crate::pack::appliance_frame_index(anim, busy, moment.timing.beat),
                        flip: Flip::None,
                    };
                    push_art(order, pack, wp.pos, art, depth, Playback::Looping);
                }
            }
        }
        K::Plant { kind, .. } => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Playback::Held,
        ),
        K::Pod { kind, .. } => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Playback::Held,
        ),
        K::Wall { kind, .. } if kind.stands_on_floor() => push_art(
            order,
            pack,
            centre,
            Art::still(kind.sprite_name()),
            depth,
            Playback::Held,
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
            let face = face_rows(pack, crate::pack::MEETING_TABLE_SPRITE, office.scale);
            order.push((
                piece_span(crate::layout::Pivot::Center, centre, table.w, table.h, face)
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
            push_art(order, pack, centre, art, depth, Playback::Held);
        }
        K::CoatRack { .. } => push_art(
            order,
            pack,
            centre,
            Art::still("coat_rack"),
            depth,
            Playback::Held,
        ),
        K::SideTable => push_art(
            order,
            pack,
            centre,
            Art::still("side_table"),
            depth,
            Playback::Held,
        ),
        K::FloorLamp => push_art(
            order,
            pack,
            centre,
            Art::still("floor_lamp"),
            depth,
            Playback::Held,
        ),
        K::KitchenIsland => push_art(
            order,
            pack,
            centre,
            Art::still("kitchen_island"),
            depth,
            Playback::Held,
        ),
        K::TrashBin => push_art(
            order,
            pack,
            centre,
            Art::still("pantry_bin"),
            depth,
            Playback::Held,
        ),
        K::FishTank => push_looping(
            order,
            pack,
            centre,
            crate::pack::FISH_TANK_SPRITE,
            moment.timing.beat,
            depth,
        ),
        K::WaterCooler => push_looping(
            order,
            pack,
            centre,
            crate::pack::WATER_COOLER_SPRITE,
            moment.timing.beat,
            depth,
        ),
        K::Door => {
            let Some((w, h)) = art_size(pack, DOOR_SPRITE) else {
                return;
            };
            order.push((
                piece_span(crate::layout::Pivot::TopLeft, top_left, w, h, 0).with_depth(depth),
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
                    reading: crate::sky::clock_reading(moment.timing.now),
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
enum Playback {
    Held,
    Looping,
}

fn push_art(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: Point,
    art: Art,
    depth: u16,
    playback: Playback,
) {
    let Some((w, h)) = art_size(pack, art.sprite) else {
        return;
    };
    let kind = match playback {
        Playback::Held => PieceKind::Prop { at, art },
        Playback::Looping => PieceKind::Animated { at, art },
    };
    order.push((
        piece_span(crate::layout::Pivot::Center, at, w, h, 0).with_depth(depth),
        kind,
    ));
}

fn push_looping(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: Point,
    sprite: &'static str,
    beat: crate::anim::Beat,
    depth: u16,
) {
    let Some(anim) = pack.animation(sprite) else {
        return;
    };
    let art = Art {
        sprite,
        frame: crate::pack::looping_frame_index(anim, beat),
        flip: Flip::None,
    };
    push_art(order, pack, at, art, depth, Playback::Looping);
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
            piece_span(crate::layout::Pivot::TopLeft, at, w, h, 0).with_depth(depth),
            PieceKind::Hung { at, sprite },
        ));
    }
}

/// Desk `i` in its facing's art, its screen lit by the classic painter's own
/// rule ([`desk_screen_glow`](crate::lighting::desk_screen_glow)) from the
/// sim's observation, so the profiles never disagree about WHICH screens are
/// lit; sorted on `depth`.
fn push_desk(
    i: pixtuoid_core::state::FloorLocalDeskIndex,
    inputs: ComposeInputs<'_, '_>,
    depth: u16,
    order: &mut Vec<(Span, PieceKind)>,
) {
    let ComposeInputs {
        frame,
        office,
        moment,
    } = inputs;
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
    let Some(art) = crate::pack::desk_art_name(pack, facing) else {
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
        if crate::pack::desk_front(pack, art).is_some() {
            order.push((span, PieceKind::DeskFront { at: d, art, screen }));
        }
    }
}

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
    let Some(desk) = crate::pack::densest_frame(pack, art, 0, scale) else {
        return;
    };
    let k = desk.blit_at.get();
    let (x0, y0) = (scale.to_buffer(span.x0), scale.to_buffer(span.y0));
    // A mark's cell, as the buffer column of its west edge and row past its foot.
    let mark = |name: &str| {
        let m = desk.marks.iter().find(|m| m.name() == name)?;
        Some((x0 + m.x() * k, y0 + (m.y() + 1) * k))
    };
    let mirrored = crate::pack::desk_props_mirrored(art);
    // Stand frame `frame` of `sprite` on the mark cell at `(x, foot)`, turned as
    // its desk turns it; where its top lands.
    let mut stand = |sprite: &'static str, frame: usize, (x, foot): (u16, u16)| {
        let f = crate::pack::densest_frame(pack, sprite, frame, scale)?;
        let b = f.blit_at.get();
        let (w, h) = (f.frame.width() * b, f.frame.height() * b);
        let x = crate::pack::prop_left(x, k, w, mirrored)?;
        let y = foot.checked_sub(h)?;
        let s = scale.get();
        let cells = |at: u16, len: u16| (at / s, (at + len - 1) / s - at / s + 1);
        let ((cx, cw), (cy, ch)) = (cells(x, w), cells(y, h));
        let prop = StoodProp {
            sprite,
            frame,
            at: (x, y),
            flip: if mirrored {
                Flip::Horizontal
            } else {
                Flip::None
            },
        };
        order.push((
            Span::new(cx, cy, cw, ch, 0)
                .with_depth(span.depth)
                .with_layer(span.layer),
            PieceKind::DeskProp(prop),
        ));
        Some(y)
    };
    if let (Some(_), Some(at)) = (props.cup, mark(crate::pack::CUP_MARK)) {
        stand(DESK_CUP_SPRITE, 0, at);
    }
    let Some(tier) = usize::from(props.token_tier).checked_sub(1) else {
        return;
    };
    let Some((x, top)) = mark(crate::pack::TOWER_MARK)
        .and_then(|at| Some((at.0, stand(TOKEN_TOWER_SPRITE, tier, at)?)))
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

/// The box a desk drawn with `art` at `desk` occupies at `scale`: the art and
/// the face rows [`face_rows`] derives under it, sorted on the last of
/// those. A taller art grows upward from the same bottom row
/// ([`desk_art_top`](crate::pack::desk_art_top)), so its depth never moves.
pub(crate) fn desk_span(
    pack: &Pack,
    art: &str,
    desk: crate::layout::Point,
    scale: RenderScale,
) -> Option<Span> {
    let (w, h) = art_size(pack, art)?;
    let span = piece_span(
        crate::layout::Pivot::TopLeft,
        crate::layout::Point {
            x: desk.x,
            y: crate::pack::desk_art_top(pack, desk.y, h),
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
pub(crate) fn face_rows(pack: &Pack, art: &str, scale: RenderScale) -> u16 {
    match crate::pack::densest_frame(pack, art, 0, scale) {
        Some(d) if d.density.get() > 1 => 0,
        _ => desk_front_h(),
    }
}

/// The chair's box and top-left at a desk facing `facing`, placed and keyed by
/// the layout's rules
/// ([`desk_chair_top_left`](crate::layout::desk_chair_top_left),
/// [`desk_chair_sort_row`](crate::layout::desk_chair_sort_row)); `None` where
/// those stand no chair or the pack has none.
fn chair_span(
    pack: &Pack,
    facing: crate::layout::Facing,
    desk: crate::layout::Point,
) -> Option<(Span, crate::layout::Point)> {
    let at = crate::layout::desk_chair_top_left(desk, facing)?;
    let (w, h) = art_size(pack, crate::pack::DESK_CHAIR_SPRITE)?;
    let span = piece_span(crate::layout::Pivot::TopLeft, at, w, h, 0)
        .with_depth(crate::layout::desk_chair_sort_row(desk, facing));
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
    let namesakes = crate::badge::Namesakes::of(&frame.agents);
    let mut carried = Vec::new();
    for c in &frame.characters {
        let Some(agent) = frame.agents.get(c.agent_idx) else {
            continue;
        };
        let pose = crate::character::SpritePose::of(c, agent, theme);
        // The frame `paint_figure` draws: an animation's frames need not
        // share a size.
        let Some((w, h)) =
            crate::pack::densest_frame(pack, pose.anim_name, pose.frame_idx, RenderScale::ONE)
                .map(|d| d.logical)
        else {
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
            desk_span(pack, crate::pack::desk_art_name(pack, facing)?, d, scale).map(|s| s.y0)
        });
        let at = cutaway_top_left(c);
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
            piece_span(crate::layout::Pivot::TopLeft, top, w, h + hair, 0),
            c.sort_row,
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
        let anchor = crate::sim::anchors::badge_anchor(
            top,
            crate::layout::Size { w, h: h + hair },
            badge_ceiling,
        );
        let run = crate::display::Badge::new(anchor, agent, &namesakes, theme).run();
        order.push((
            topmost_span(badge_plate(&run, pen), pen),
            PieceKind::Text { run },
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
) -> Vec<crate::display::effects::Riding> {
    let d = key.frame.density.get();
    let Some(pen) = Pen::new(scale, d) else {
        return Vec::new();
    };
    // A dressed frame's head is its mark's column on its crest; the base
    // art's, the classic's crown point: its frame's top, centred.
    let head = match key.dress.as_ref() {
        Some(dress) => {
            let (d, frame_w) = (i32::from(d), i32::from(w) * i32::from(d));
            let x = i32::from(dress.head.x);
            Some(crate::display::effects::ArtPoint {
                x: i32::from(at.x) * d + if key.frame.flip_x { frame_w - 1 - x } else { x },
                y: i32::from(at.y) * d + dress.crest,
            })
        }
        None if d == 1 => Some(crate::display::effects::ArtPoint {
            x: i32::from(at.x + w / 2),
            y: i32::from(at.y),
        }),
        None => None,
    };
    c.effects
        .iter()
        .map(|&effect| crate::display::effects::Riding { effect, head, pen })
        .collect()
}

/// A figure's piece: its drawn bounds, sorted on `depth` — the sim's own sort row,
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

/// Queue one sofa body, sorted with its sitters at `tie`: the front view, or
/// the `back_view`. A pack that draws [`MEETING_SOFA_NORTH_SPRITE`] gets it as two
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
    let sitters = crate::sim::seat::sofa_sitter_sort_row(at);
    if let Some((w, h)) = art_size(pack, MEETING_SOFA_NORTH_SPRITE).filter(|_| back_view) {
        let tl = crate::layout::anchored_top_left(crate::layout::Pivot::Center, at, w, h);
        let split = NORTH_SOFA_SEAT_ROWS.min(h);
        let band = |rows| PieceKind::PropBand {
            at,
            sprite: MEETING_SOFA_NORTH_SPRITE,
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
        let span = piece_span(crate::layout::Pivot::Center, at, w, h, 0)
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
/// pieces: the long-object case [`crate::display::order`] documents.
fn wall_segments(layout: &SceneLayout, order: &mut Vec<(Span, PieceKind)>) {
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

/// A piece's bounds, as [`Span::new`] builds them from its sprite's box.
/// Anchoring goes through [`crate::layout::anchored_top_left`], the same function
/// the walkable mask and the classic painter use.
fn piece_span(
    pivot: crate::layout::Pivot,
    pos: crate::layout::Point,
    w: u16,
    h: u16,
    below: u16,
) -> Span {
    let tl = crate::layout::anchored_top_left(pivot, pos, w, h);
    Span::new(tl.x, tl.y, w, h, below)
}

/// Frame 0's LOGICAL size: the size the sort space lays a static piece out in,
/// whichever density it is drawn from. An animated figure sizes from the frame
/// it draws (`push_characters`).
pub(crate) fn art_size(pack: &Pack, sprite: &str) -> Option<(u16, u16)> {
    crate::pack::densest_frame(pack, sprite, 0, RenderScale::ONE).map(|d| d.logical)
}

/// Rows of front face derived under a top-down desk: its thickness.
pub(crate) fn desk_front_h() -> u16 {
    (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1)
}

/// Queue each window as a piece at the very back of the order: what its glass
/// looks out on under `weather` on the pen's art grid, resolved when the list
/// is built, so the view changes with the sky, the weather and the city's
/// lights without touching the backdrop.
pub(crate) fn push_windows(
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
    let Some(density) =
        pixtuoid_core::sprite::format::Density::new(Pen::for_pack(scale, pack).art(1).0)
    else {
        return;
    };
    let outside = crate::outside::Outside::of(
        moment,
        pack,
        theme,
        (layout.buf_w, layout.wall_band_h()),
        density,
        *weather,
    );
    let rows = crate::layout::window_rows(layout.wall_band_h());
    // The bolt lights the glass and all it shows, over the weather on it.
    let bolt = crate::display::light::bolt_steps(&moment.sky);
    let mut bolt_lift = crate::dither::Stepped::new(bolt as i8);
    for bay in layout.window_bays() {
        let mut view = outside.through(bay);
        if bolt > 0 {
            view.paint(|_, c| bolt_lift.of(c));
        }
        order.push((
            Span::new(bay.x, rows.start, bay.w, rows.end - rows.start, 0).with_depth(0),
            PieceKind::Window {
                view,
                frame: theme.surface.window_frame,
            },
        ));
    }
}

/// The classic placement's top-left, unchanged: the seat side is a per-desk layout
/// fact, so an override here would make the two profiles disagree about which
/// side of its desk half the office sits on.
fn cutaway_top_left(c: &crate::sim::CharacterPlacement) -> crate::layout::Point {
    c.top_left
}

#[cfg(test)]
pub(crate) mod tests;
