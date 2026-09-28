//! The cutaway profile's paint pass — the second reader of `SimFrame`, and
//! deliberately partial: EFFECTS (weather, steam, the pet) stay with the classic
//! pass, and of the glow it draws only the desks' and a lit sitter's tint. It
//! never advances the sim; a mover here would desync the profiles.

use pixtuoid_core::sprite::blit::blit_frame_scaled;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::RgbBuffer;

use crate::cutaway::order::{depth_sort, Span};
use crate::cutaway::shade::{dither_band, fill, slab, Ramp};
use crate::layout::{Layout, DESK_H};
use crate::pixel_painter::SimFrame;
use crate::render_scale::RenderScale;
use crate::theme::Theme;

/// The front face the cutaway derives under a top-down desk's art (and under the
/// meeting table's slab), as a fraction of `DESK_H` so it tracks the desk.
/// Without one there is no thickness and the office reads as a floor plan.
const DESK_FRONT_NUMER: u16 = 2;
/// Denominator of [`DESK_FRONT_NUMER`].
const DESK_FRONT_DENOM: u16 = 5;

/// How far the key light reaches down the room before the floor falls off.
///
/// The windows are the north wall, so the falloff runs north to south; these
/// bound the dithered transition between the lit and base floor tones.
const FLOOR_LIT_NUMER: u16 = 1;
/// Denominator of [`FLOOR_LIT_NUMER`].
const FLOOR_LIT_DENOM: u16 = 3;

/// Narrowest skyline building, in logical units.
const SKYLINE_MIN_W: u16 = 3;
/// How much wider than [`SKYLINE_MIN_W`] a building may be.
const SKYLINE_W_SPREAD: u16 = 6;
/// Shortest skyline building — below this the city reads as a jagged floor.
const SKYLINE_MIN_H: u16 = 2;

/// Logical rows between a head and its name badge.
const LABEL_GAP_PX: u16 = 2;

/// Thickness of a room's glass wall, in logical units.
const ROOM_WALL_PX: u16 = 1;

/// How many ramp levels the lit glass sits below its glow colour.
const SCREEN_GLASS_LEVEL: i8 = -3;

/// How many ramp levels a lit screen's text sits above its glow colour: bright
/// lines on the dark glass.
const SCREEN_TEXT_LEVEL: i8 = 9;

/// Where a painter should hang one agent's name badge, in BUFFER pixels.
///
/// The engine cannot draw text — the font lives in the binary — so the profile
/// reports anchors and lets the painter render. These are the CUTAWAY's anchors:
/// `overlay::build_overlay` derives its own from the classic projection, so a
/// badge placed with those would float where the classic painter drew the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutawayLabel {
    /// Index into [`SimFrame::agents`].
    pub agent_idx: usize,
    /// Badge anchor: horizontal centre of the sprite, clear above its head and
    /// any raised monitor behind it.
    pub anchor_px: crate::layout::Point,
}

/// Paint `frame`'s office into `buf` as an orthographic cutaway — the classic
/// painter's sibling, not its successor. `layout` is in LOGICAL units and `buf`
/// in buffer pixels; `scale` converts. Returns where each visible agent's badge
/// belongs; see [`CutawayLabel`].
#[allow(clippy::too_many_arguments)]
pub fn render_cutaway(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    now: std::time::SystemTime,
    cache: &mut crate::frame_cache::FrameCache,
    buf: &mut RgbBuffer,
) -> Vec<CutawayLabel> {
    paint_floor(layout, theme, scale, buf);
    paint_wall(layout, theme, scale, buf);
    // Wall decor hangs on the north band, so it is NOT floor-sorted: it paints
    // with the wall, before anything standing on the floor can occlude it.
    for item in &layout.wall_decor {
        paint_wall_decor(item.pos, item.kind.sprite_name(), pack, scale, buf);
    }

    // ONE ordered draw list, so a character and the desk it sits at resolve against
    // each other by depth. That ordering IS the occlusion — there is no second pass.
    let ordered = depth_sort(draw_list(frame, layout, pack, theme, scale));
    let mut labels = Vec::with_capacity(frame.characters.len());
    for kind in &ordered {
        if let Some(l) = paint_piece(kind, frame, pack, theme, scale, now, cache, buf) {
            labels.push(l);
        }
    }
    labels
}

/// Every floor-standing piece of the office, each with its [`Span`]. The push
/// order breaks depth ties, so it is part of the result: a chair pushed after
/// the people keeps it over a sitter who shares its depth.
fn draw_list(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
) -> Vec<(Span, PieceKind)> {
    let mut order: Vec<(Span, PieceKind)> =
        Vec::with_capacity(layout.home_desks.len() + frame.characters.len());
    push_desks(frame, layout, pack, theme, scale, &mut order);
    push_props(layout, pack, &mut order);
    push_appliances(layout, &mut order);
    push_meeting_trios(layout, pack, &mut order);
    let carried = push_characters(frame, layout, pack, scale, &mut order);
    push_chairs(layout, pack, &carried, &mut order);
    wall_segments(layout, &mut order);
    push_pantry_counter(layout, pack, &mut order);
    order
}

/// Paint one piece of the draw list, returning a character's badge anchor.
#[allow(clippy::too_many_arguments)]
fn paint_piece(
    kind: &PieceKind,
    frame: &SimFrame,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    now: std::time::SystemTime,
    cache: &mut crate::frame_cache::FrameCache,
    buf: &mut RgbBuffer,
) -> Option<CutawayLabel> {
    match *kind {
        PieceKind::Desk { at, art, screen } => paint_desk(at, art, screen, pack, theme, scale, buf),
        PieceKind::Chair { at } => paint_chair(at, pack, theme, scale, buf),
        PieceKind::Character {
            idx,
            chair,
            badge_ceiling,
        } => {
            let label = paint_character(
                frame,
                idx,
                badge_ceiling,
                pack,
                theme,
                scale,
                now,
                cache,
                buf,
            );
            // The sitter's own chair, straight after them: one piece, so
            // nothing can sort between a person and the chair they sit in.
            if let Some(at) = chair {
                paint_chair(at, pack, theme, scale, buf);
            }
            return label;
        }
        PieceKind::Prop {
            at,
            sprite,
            mirrored,
        } => paint_prop(at, sprite, mirrored, pack, theme, scale, buf),
        PieceKind::Table { at } => paint_table(at, theme, scale, buf),
        PieceKind::Appliance { at, kind } => paint_appliance(at, kind, theme, scale, buf),
        PieceKind::WallSeg { at, w, h } => paint_wall_seg(at, w, h, theme, scale, buf),
    }
    None
}

/// Each desk in its facing's art, its screen lit by the classic painter's own
/// rule ([`desk_screen_glow`](crate::pixel_painter::desk_screen_glow)) from the
/// sim's observation, so the profiles never disagree about WHICH screens are lit.
fn push_desks(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    order: &mut Vec<(Span, PieceKind)>,
) {
    for (i, d) in layout.home_desks.iter().enumerate() {
        let local = pixtuoid_core::state::FloorLocalDeskIndex(i);
        let facing = layout.desk_facing(local);
        let Some(art) = desk_art(pack, facing) else {
            continue;
        };
        let screen = crate::pixel_painter::desk_screen_glow(
            crate::pixel_painter::desk_occupant(&frame.agents, local),
            facing,
            frame.seated_agents.get(&local).copied().unwrap_or(false),
            theme,
        );
        if let Some(span) = desk_span(pack, art, *d, scale) {
            order.push((
                span,
                PieceKind::Desk {
                    at: *d,
                    art,
                    screen,
                },
            ));
        }
    }
}

/// The pack's desk art for a seat facing `facing`: the facing's own when the
/// pack ships it, else what [`Pack::piece_or_source`] draws in its place.
fn desk_art(pack: &Pack, facing: crate::layout::Facing) -> Option<&'static str> {
    pack.piece_or_source(crate::pixel_painter::desk_sprite_name(facing))
}

/// The box a desk drawn with `art` at `desk` occupies at `scale`: the art, the
/// face rows [`desk_face_rows`] derives under it, and the contact row
/// `paint_desk` stamps under those. It sorts on the row just above that contact
/// row. A taller art grows upward from the same bottom row
/// ([`desk_art_top`](crate::pixel_painter::desk_art_top)), so its depth never moves.
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
            y: crate::pixel_painter::desk_art_top(pack, desk.y, h),
        },
        w,
        h,
        desk_face_rows(pack, art, scale),
    );
    Some(span.painting_below(1))
}

/// The rows of front face the cutaway derives under desk `art` at `scale`.
///
/// Only this profile draws a density variant (the classic painter's scale is 1,
/// where `densest_frame` returns the base), so `@Nx` art is authored for this
/// profile with its whole front; a derived face under it would read as a plank
/// on the floor.
fn desk_face_rows(pack: &Pack, art: &str, scale: RenderScale) -> u16 {
    match crate::pixel_painter::densest_frame(pack, art, 0, scale) {
        Some(d) if d.density.get() > 1 => 0,
        _ => desk_front_h(),
    }
}

/// Compares `variant`, a cutaway render of a pack whose desks ship density
/// variants, against `base`, one of `base_pack`, on the two things the cutaway
/// promises: the art
/// lands exactly where the base's does (every pixel outside each desk's foot — its
/// face band and contact row — is identical), and a variant, which draws its own
/// front, gets its contact row right under the art with bare floor below it, while
/// the base gets a derived face first and its contact row one face band lower.
#[cfg(test)]
pub(crate) fn assert_variant_desk_foot(
    variant: &[pixtuoid_core::sprite::Rgb],
    base: &[pixtuoid_core::sprite::Rgb],
    layout: &Layout,
    base_pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
) {
    let mut floor = RgbBuffer::filled(
        scale.to_buffer(layout.buf_w),
        scale.to_buffer(layout.buf_h),
        theme.surface.bg_fallback,
    );
    paint_floor(layout, theme, scale, &mut floor);
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
                crate::pixel_painter::desk_art_top(base_pack, d.y, h) + h,
            ))
        })
        .collect();
    assert!(!feet.is_empty(), "the office must have a desk");
    let in_foot = |x: u16, y: u16| {
        feet.iter()
            .any(|&(x0, x1, below)| (x0..=x1).contains(&x) && (below..=below + face).contains(&y))
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
    let contact = contact_tone(theme);
    // The desk's own edge column: a sitter and their chair stand centred on it.
    for &(x0, _, below) in &feet {
        let x = x0 + 1;
        assert_eq!(
            at(variant, x, below),
            contact,
            "a variant's contact row is right under its art"
        );
        for y in below + 1..=below + face {
            assert_eq!(
                at(variant, x, y),
                at(floor.as_slice(), x, y),
                "the band a variant's foot vacates is bare floor"
            );
        }
        assert_ne!(
            at(base, x, below),
            contact,
            "the base gets a derived face under its art"
        );
        assert_eq!(
            at(base, x, below + face),
            contact,
            "the base's contact row is one face band lower"
        );
    }
}

/// The task chair at each back-turned home desk nobody sits at. The desks in
/// `carried` are skipped: their chairs ride their sitters' pieces
/// (`push_characters`, which returns them).
fn push_chairs(
    layout: &Layout,
    pack: &Pack,
    carried: &[crate::layout::Point],
    order: &mut Vec<(Span, PieceKind)>,
) {
    for (i, d) in layout.home_desks.iter().enumerate() {
        if carried.contains(d) {
            continue;
        }
        let facing = layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i));
        if let Some((span, at)) = chair_span(pack, facing, *d) {
            order.push((span, PieceKind::Chair { at }));
        }
    }
}

/// The chair's box and top-left at a desk facing `facing`, placed and keyed by
/// the classic painter's own rules
/// ([`desk_chair_top_left`](crate::pixel_painter::desk_chair_top_left),
/// [`desk_chair_z_key`](crate::pixel_painter::desk_chair_z_key)); `None` where
/// those stand no chair.
fn chair_span(
    pack: &Pack,
    facing: crate::layout::Facing,
    desk: crate::layout::Point,
) -> Option<(Span, crate::layout::Point)> {
    let at = crate::pixel_painter::desk_chair_top_left(pack, desk, facing)?;
    let (w, h) = art_size(pack, crate::pixel_painter::DESK_CHAIR_SPRITE)?;
    // +1 for the contact shadow `paint_chair` stamps under the box.
    let span = piece_span(crate::layout::Anchor::TopLeft, at, w, h, 1)
        .with_depth(crate::pixel_painter::desk_chair_z_key(desk, facing));
    Some((span, at))
}

/// The layout's plants, floor props, pod decor and lounge couch.
fn push_props(layout: &Layout, pack: &Pack, order: &mut Vec<(Span, PieceKind)>) {
    let push_prop =
        |order: &mut Vec<(Span, PieceKind)>, at: crate::layout::Point, sprite: &'static str| {
            if let Some((w, h)) = art_size(pack, sprite) {
                order.push((
                    // +1 for the contact shadow `paint_prop` stamps under the box.
                    piece_span(crate::layout::Anchor::Center, at, w, h, 1),
                    PieceKind::Prop {
                        at,
                        sprite,
                        mirrored: false,
                    },
                ));
            }
        };
    for pl in &layout.plants {
        push_prop(order, pl.pos, pl.kind.sprite_name());
    }
    for wp in &layout.waypoints {
        if let Some(sprite) = waypoint_sprite(wp.kind) {
            push_prop(order, wp.pos, sprite);
        }
    }
    for d in &layout.pod_decor {
        push_prop(order, d.pos, d.kind.sprite_name());
    }
    // The lounge couch IS a mirrored meeting sofa, as the classic painter draws it.
    if let Some(at) = layout.couch_sprite_center() {
        push_sofa(order, pack, at, true);
    }
}

/// Queue the corridor appliances ([`paint_appliance`]).
fn push_appliances(layout: &Layout, order: &mut Vec<(Span, PieceKind)>) {
    for wp in layout.waypoints.iter().filter(|wp| {
        matches!(
            wp.kind,
            crate::layout::WaypointKind::VendingMachine | crate::layout::WaypointKind::Printer
        )
    }) {
        let def = crate::layout::furniture_def(wp.kind.furniture());
        order.push((
            // +1 for `paint_appliance`'s contact shadow.
            piece_span(
                crate::layout::Anchor::Center,
                wp.pos,
                def.visual.w,
                def.visual.h,
                1,
            ),
            PieceKind::Appliance {
                at: wp.pos,
                kind: wp.kind,
            },
        ));
    }
}

/// Two sofa bodies plus the table between them. Only
/// [`MeetingTrio::sofas`](crate::layout::MeetingTrio::sofas)' south sofa is
/// mirrored: that is what makes the pair FACE each other across the table.
fn push_meeting_trios(layout: &Layout, pack: &Pack, order: &mut Vec<(Span, PieceKind)>) {
    let table = crate::layout::furniture_def(crate::layout::Furniture::MeetingTable).visual;
    for t in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for (i, sofa) in t.sofas.iter().enumerate() {
            push_sofa(order, pack, *sofa, i % 2 != 0);
        }
        order.push((
            // A front face below, and a contact shadow under that.
            piece_span(
                crate::layout::Anchor::Center,
                t.table,
                table.w,
                table.h,
                desk_front_h() + 1,
            ),
            PieceKind::Table { at: t.table },
        ));
    }
}

/// Queue every character, and return the desks whose chairs they carry, so
/// [`push_chairs`] stands none of those again.
fn push_characters(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    scale: RenderScale,
    order: &mut Vec<(Span, PieceKind)>,
) -> Vec<crate::layout::Point> {
    let mut carried = Vec::new();
    for (i, c) in frame.characters.iter().enumerate() {
        // The frame `paint_character` draws: an animation's frames need not
        // share a size.
        let Some((w, h)) =
            crate::pixel_painter::densest_frame(pack, c.anim_name, c.frame_idx, RenderScale::ONE)
                .map(|d| d.logical)
        else {
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
        // +1 for the contact shadow `paint_character` stamps under a figure
        // not seated at a desk (`seat_desk`).
        let shadow = u16::from(c.seat_desk.is_none());
        order.push((
            occupant_span(
                piece_span(
                    crate::layout::Anchor::TopLeft,
                    cutaway_anchor(c),
                    w,
                    h,
                    shadow,
                ),
                c.anchor_y,
                chair.map(|(span, _)| span),
            ),
            PieceKind::Character {
                idx: i,
                chair: chair.map(|(_, at)| at),
                badge_ceiling,
            },
        ));
    }
    carried
}

/// A figure's piece: its drawn bounds, sorted on `depth` — the sim's own z-key,
/// which neither breath nor the sit arc moves, so a person never flips against a
/// neighbour mid-breath. A back-turned sitter and their chair are one piece,
/// bounding the chair's whole box too.
fn occupant_span(body: Span, depth: u16, chair: Option<Span>) -> Span {
    let body = body.with_depth(depth);
    match chair {
        Some(chair) => Span {
            x0: body.x0.min(chair.x0),
            x1: body.x1.max(chair.x1),
            y0: body.y0.min(chair.y0),
            y1: body.y1.max(chair.y1),
            depth: body.depth.max(chair.depth),
        },
        None => body,
    }
}

/// Queue one `meeting_sofa` body, flipped top-to-bottom when `mirrored`.
///
/// NOT `back_couch`: the pack documents that as a character seen from behind, so
/// it would draw a headless torso where the couch belongs.
fn push_sofa(
    order: &mut Vec<(Span, PieceKind)>,
    pack: &Pack,
    at: crate::layout::Point,
    mirrored: bool,
) {
    if let Some((w, h)) = art_size(pack, "meeting_sofa") {
        order.push((
            piece_span(crate::layout::Anchor::Center, at, w, h, 1),
            PieceKind::Prop {
                at,
                sprite: "meeting_sofa",
                mirrored,
            },
        ));
    }
}

/// Rows of a vertical wall run per sorted segment. A segment must be no taller
/// than the SHORTEST thing that can pass in front of it: one spanning both sides
/// of a figure has no correct position. This leaves headroom under the bundled
/// cast's height for a shorter pack, at a piece count the draw list absorbs
/// easily.
const WALL_SEG_H: u16 = 4;

/// Queue every room's walls as sorted pieces, vertical runs split into
/// [`WALL_SEG_H`]-row segments: the long-object case [`crate::cutaway::order`]
/// documents.
fn wall_segments(layout: &Layout, order: &mut Vec<(Span, PieceKind)>) {
    let rooms = layout
        .meeting_rooms
        .iter()
        .map(|r| r.bounds)
        .chain(layout.pantry.iter().map(|p| p.bounds));
    for b in rooms {
        if b.width < ROOM_WALL_PX || b.height < ROOM_WALL_PX {
            continue;
        }
        let mut push = |x: u16, y: u16, w: u16, h: u16| {
            order.push((
                Span::new(x, y, w, h, 0),
                PieceKind::WallSeg {
                    at: crate::layout::Point { x, y },
                    w,
                    h,
                },
            ));
        };
        let south = b.y + b.height - ROOM_WALL_PX;
        push(b.x, b.y, b.width, ROOM_WALL_PX);
        push(b.x, south, b.width, ROOM_WALL_PX);
        for side_x in [b.x, b.x + b.width - ROOM_WALL_PX] {
            let mut y = b.y;
            while y < b.y + b.height {
                let h = WALL_SEG_H.min(b.y + b.height - y);
                push(side_x, y, ROOM_WALL_PX, h);
                y += h;
            }
        }
    }
}

/// One wall segment. A horizontal run gets the top-lit [`slab`] the procedural
/// solids here carry; a vertical one is seen edge-on, so it is a flat fill —
/// an edge tone on a 1-column strip would read as a highlight, not a material.
fn paint_wall_seg(
    at: crate::layout::Point,
    w: u16,
    h: u16,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let glass = Ramp::from_base(theme.office.room_wall_trim_light);
    let (x, y) = (scale.to_buffer(at.x), scale.to_buffer(at.y));
    if w > h {
        slab(
            buf,
            x,
            y,
            scale.to_buffer(w),
            scale.to_buffer(h),
            &glass,
            scale,
        );
    } else {
        fill(
            buf,
            x,
            y,
            scale.to_buffer(w),
            scale.to_buffer(h),
            glass.base,
        );
    }
}

/// What a piece IS, paired with its [`Span`] in the draw list.
///
/// The span is computed WHERE THE PIECE IS BUILT, from the same anchor and box
/// the piece's paint fn draws, plus the rows it stamps under that box — the box
/// depends on the PACK, so a sprite's height is not knowable from its layout
/// point, and geometry derived from anywhere but where the sprite lands can drift
/// from it. `every_piece_paints_only_inside_its_span` pins that every pixel a
/// paint fn writes lies inside its span.
#[derive(Debug)]
enum PieceKind {
    /// One segment of a room's wall run.
    WallSeg {
        /// The logical position, which `paint_wall_seg` scales; walls are pure
        /// geometry with no sprite to look up.
        at: crate::layout::Point,
        w: u16,
        h: u16,
    },
    Desk {
        at: crate::layout::Point,
        /// The facing's art (see [`desk_art`]).
        art: &'static str,
        /// The glow of a lit screen, or `None` for a dark one.
        screen: Option<pixtuoid_core::sprite::Rgb>,
    },
    Chair {
        at: crate::layout::Point,
    },
    Prop {
        at: crate::layout::Point,
        sprite: &'static str,
        /// Flip rows top-to-bottom, as the classic painter's `MeetingSofa`
        /// does.
        mirrored: bool,
    },
    Table {
        at: crate::layout::Point,
    },
    Appliance {
        at: crate::layout::Point,
        kind: crate::layout::WaypointKind,
    },
    Character {
        idx: usize,
        /// A back-turned sitter's chair, painted straight after them.
        chair: Option<crate::layout::Point>,
        /// The logical row their badge must stay above: the top of their desk's
        /// art.
        badge_ceiling: Option<u16>,
    },
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
    crate::pixel_painter::densest_frame(pack, sprite, 0, RenderScale::ONE).map(|d| d.logical)
}

/// Rows of front face derived under a top-down desk: its thickness.
fn desk_front_h() -> u16 {
    (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1)
}

/// The wall rows inset above and below the glass run, as a fraction of the band:
/// the inset leaves the middle of the band as glass. The windows are the
/// cutaway's only light SOURCE on screen, so the band has to read as glass and
/// not as a stripe — that is what makes the north-to-south floor falloff legible
/// as light instead of as a gradient someone chose.
const WINDOW_INSET_NUMER: u16 = 1;
/// Denominator of [`WINDOW_INSET_NUMER`].
const WINDOW_INSET_DENOM: u16 = 4;

/// Paint the north wall band: wall, glass, skyline and sill.
///
/// Its height is the layout's own derivation, not a re-guess — the band ends
/// `WALL_BAND_TO_TOP_MARGIN` above `top_margin`, and the rows between are floor
/// the agents walk on, so a band drawn to `top_margin` would paint over them.
fn paint_wall(layout: &Layout, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    let band_h = layout
        .top_margin
        .saturating_sub(crate::layout::WALL_BAND_TO_TOP_MARGIN);
    if band_h == 0 {
        return;
    }
    let s = scale.get();
    let w = scale.to_buffer(layout.buf_w);
    let wall = Ramp::from_base(theme.surface.wall);
    slab(buf, 0, 0, w, scale.to_buffer(band_h), &wall, scale);

    // One glass run inset inside the band, with a lit sill under it — the sill
    // is what sells the light as coming THROUGH rather than being painted on.
    let inset = (band_h * WINDOW_INSET_NUMER / WINDOW_INSET_DENOM).max(1);
    let glass_h = band_h.saturating_sub(inset * 2);
    if glass_h > 0 {
        let glass = Ramp::from_base(theme.lighting.night_sky_a);
        slab(
            buf,
            0,
            scale.to_buffer(inset),
            w,
            scale.to_buffer(glass_h),
            &glass,
            scale,
        );
        paint_skyline(layout, theme, scale, inset, glass_h, buf);
        fill(
            buf,
            0,
            scale.to_buffer(inset + glass_h),
            w,
            s,
            theme.surface.wall_trim,
        );
    }
    // The wall's own contact line with the floor.
    fill(buf, 0, scale.to_buffer(band_h), w, s, contact_tone(theme));
}

/// A city skyline on the window sill, lit windows scattered through it. Flat
/// glass reads as a painted stripe; a skyline is what makes the band a WINDOW,
/// and the lit windows are what make it night. Deterministic from the layout
/// width so the same office always gets the same city — a per-frame reshuffle
/// would flicker.
fn paint_skyline(
    layout: &Layout,
    theme: &Theme,
    scale: RenderScale,
    glass_top: u16,
    glass_h: u16,
    buf: &mut RgbBuffer,
) {
    let s = scale.get();
    let sill = glass_top + glass_h;
    // A local mix, per this crate's convention: each noise site owns its own
    // finaliser over a disjoint domain.
    let mix = |n: u32| -> u32 {
        let mut v = n.wrapping_mul(0x9E37_79B9);
        v ^= v >> 15;
        v = v.wrapping_mul(0x85EB_CA6B);
        v ^ (v >> 13)
    };

    let mut x = 0u16;
    let mut i = 0u32;
    while x < layout.buf_w {
        let bw = SKYLINE_MIN_W + (mix(i) % u32::from(SKYLINE_W_SPREAD)) as u16;
        let bh = SKYLINE_MIN_H + (mix(i ^ 0x5A5A) % u32::from(glass_h.max(1))) as u16;
        let bh = bh.min(glass_h);
        let dark = mix(i ^ 0x1234) % 3 != 0;
        let tone = if dark {
            theme.office.building_dark
        } else {
            theme.office.building_light
        };
        let top = sill.saturating_sub(bh);
        fill(
            buf,
            scale.to_buffer(x),
            scale.to_buffer(top),
            scale.to_buffer(bw),
            scale.to_buffer(bh),
            tone,
        );
        // Lit windows — the thing that says "night", not just "dark".
        let mut wy = top + 1;
        while wy + 1 < sill {
            let mut wx = x + 1;
            while wx + 1 < x + bw {
                if mix(u32::from(wx) ^ (u32::from(wy) << 8)) % 5 == 0 {
                    fill(
                        buf,
                        scale.to_buffer(wx),
                        scale.to_buffer(wy),
                        s,
                        s,
                        theme.lighting.twilight_a,
                    );
                }
                wx += 2;
            }
            wy += 2;
        }
        x = x.saturating_add(bw + 1);
        i += 1;
    }
}

/// Queue the pantry counter — a fixture the layout already sized, without which
/// the room reads as an empty glass box. A sorted piece rather than a backdrop
/// blit: it stands ON the floor, so an agent at the counter resolves against it
/// like any other solid, and it earns a contact shadow like the other
/// floor-standing pieces.
fn push_pantry_counter(layout: &Layout, pack: &Pack, order: &mut Vec<(Span, PieceKind)>) {
    let Some(pantry) = &layout.pantry else {
        return;
    };
    let sprite = crate::pixel_painter::pantry_counter_anim(pantry.counter_size.w);
    let Some((w, h)) = art_size(pack, sprite) else {
        return;
    };
    // Centred across the room, its top one row under the room's wall;
    // `paint_prop` takes a centre.
    let at = crate::layout::Point {
        x: pantry.bounds.x + pantry.bounds.width.saturating_sub(w) / 2 + w / 2,
        y: pantry.bounds.y + ROOM_WALL_PX + 1 + h / 2,
    };
    order.push((
        piece_span(crate::layout::Anchor::Center, at, w, h, 1),
        PieceKind::Prop {
            at,
            sprite,
            mirrored: false,
        },
    ));
}

fn paint_floor(layout: &Layout, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    let lit = theme.surface.carpet_light;
    let base = theme.surface.carpet_base;
    let dark = theme.surface.carpet_dark;

    let h = scale.to_buffer(layout.buf_h);
    let w = scale.to_buffer(layout.buf_w);
    fill(buf, 0, 0, w, h, base);

    // Anchored at `top_margin`, where the layout says the floor begins, not
    // buffer row 0: the wall band paints over the top of the buffer, so a lit
    // zone anchored there would start behind it.
    let floor_top = scale.to_buffer(layout.top_margin);
    let floor_h = h.saturating_sub(floor_top);

    // The lit share of the floor: its first half solid, dithering to base by its
    // end, then a final fall to dark at the south edge.
    let lit_h = floor_h * FLOOR_LIT_NUMER / FLOOR_LIT_DENOM;
    fill(buf, 0, floor_top, w, lit_h / 2, lit);
    dither_band(
        buf,
        floor_top + lit_h / 2,
        floor_top + lit_h,
        base,
        lit,
        scale,
    );
    dither_band(buf, h.saturating_sub(lit_h / 2), h, dark, base, scale);
}

fn paint_desk(
    at: crate::layout::Point,
    art_name: &str,
    screen: Option<pixtuoid_core::sprite::Rgb>,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let s = scale.get();
    let (Some(span), Some(desk)) = (
        desk_span(pack, art_name, at, scale),
        crate::pixel_painter::densest_frame(pack, art_name, 0, scale),
    ) else {
        return;
    };
    let (x, top_y) = (scale.to_buffer(span.x0), scale.to_buffer(span.y0));
    let relit;
    let art = match screen {
        Some(glow) => {
            relit = relight_screen(desk.recolorable, glow);
            &relit
        }
        None => desk.frame,
    };
    blit_frame_scaled(art, x, top_y, desk.blit_at, buf);

    // The drawn size, from the logical size: variant art blits at `blit_at`, so
    // its own pixel size times the scale would drop the contact row (and a base's
    // face) a whole desk low.
    let (drawn_w, drawn_h) = (
        scale.to_buffer(desk.logical.0),
        scale.to_buffer(desk.logical.1),
    );
    let base_y = top_y + drawn_h;
    let face_h = scale.to_buffer(desk_face_rows(pack, art_name, scale));
    if face_h > 0 {
        let Some(material) = dominant_opaque_row(desk.frame, desk.frame.height().saturating_sub(1))
        else {
            return;
        };
        slab(
            buf,
            x,
            base_y,
            drawn_w,
            face_h,
            &Ramp::from_base(material),
            scale,
        );
    }

    // The contact row, as [`contact_shadow`] draws one for the other solids.
    fill(buf, x, base_y + face_h, drawn_w, s, contact_tone(theme));
}

/// The desk art with its screen lit in `glow`: the glass takes a dark step of
/// the glow and its dim content turns to bright text. Recoloring the pack's own
/// screen KEYS ([`SCREEN_GLASS_KEY`](crate::pixel_painter::SCREEN_GLASS_KEY),
/// [`SCREEN_TEXT_KEY`](crate::pixel_painter::SCREEN_TEXT_KEY)), rather than
/// painting a band over the desk or matching a colour, lights exactly the screen
/// the art drew — at whatever density it was drawn — and no other pixel, even
/// one the same colour as the glass.
fn relight_screen(
    art: pixtuoid_core::sprite::RecolorableFrame<'_>,
    glow: pixtuoid_core::sprite::Rgb,
) -> pixtuoid_core::sprite::Frame {
    art.recolored(&[
        (
            crate::pixel_painter::SCREEN_GLASS_KEY,
            Some(glow.ramp(SCREEN_GLASS_LEVEL)),
        ),
        (
            crate::pixel_painter::SCREEN_TEXT_KEY,
            Some(glow.ramp(SCREEN_TEXT_LEVEL)),
        ),
    ])
}

/// The most common opaque colour in `row` of `frame` — how the cutaway learns a
/// sprite's material without hardcoding it. The front face a top-down sprite
/// never had has to be SOME colour, and the desk's lives in the PACK
/// (palette key `D`), not the theme, where `furniture.wood_top` reads nearly like
/// the carpet; sampling also earns a custom pack's desk a match for free.
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

/// Wall-hung decor: blitted at its own `pos`, with NO ground shadow — two things
/// [`paint_prop`] would get wrong here. `WallDecorItem.pos` is TOP-LEFT, like the
/// classic painter's `Anchor::TopLeft` z-sort rather than the centre-pinned
/// furniture, so centring would hang every board up and west of where it
/// belongs; and touching no floor, a contact shadow would land up the wall.
fn paint_wall_decor(
    pos: crate::layout::Point,
    sprite: &str,
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = crate::pixel_painter::densest_frame(pack, sprite, 0, scale) else {
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
fn cutaway_anchor(c: &crate::pixel_painter::CharacterPlacement) -> crate::layout::Point {
    c.anchor
}

#[allow(clippy::too_many_arguments)]
fn paint_character(
    frame: &SimFrame,
    idx: usize,
    badge_ceiling: Option<u16>,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    now: std::time::SystemTime,
    cache: &mut crate::frame_cache::FrameCache,
    buf: &mut RgbBuffer,
) -> Option<CutawayLabel> {
    let c = frame.characters.get(idx)?;
    let agent = frame.agents.get(c.agent_idx)?;
    let at = cutaway_anchor(c);

    // The classic painter's own recolor + facing-flip path, through the same
    // cache: a raw pack blit clones one placeholder-palette person per agent.
    let glow_tint = crate::pixel_painter::character_glow_tint(c.glow, agent, theme);
    let art = crate::pixel_painter::seat::character_frame(
        c.anim_name,
        c.frame_idx,
        agent,
        pack,
        c.flip_x,
        glow_tint,
        scale,
        cache,
        now,
    )?;
    let (art_w, art_h) = art.logical;

    // A sitter is grounded by their desk (and a back-turned one by their
    // chair), not a shadow.
    if c.seat_desk.is_none() {
        contact_shadow(at, art_w, art_h, theme, scale, buf);
    }
    blit_frame_scaled(
        art.frame,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y),
        art.blit_at,
        buf,
    );
    Some(CutawayLabel {
        agent_idx: c.agent_idx,
        anchor_px: label_anchor(at, art_w, badge_ceiling, scale),
    })
}

/// The badge anchor for a body of `sprite_w` logical columns drawn at `at`:
/// horizontally centred, `LABEL_GAP_PX` logical rows clear of the head — and of
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
    scale: RenderScale,
) -> crate::layout::Point {
    let clear_of = |row: u16| {
        scale
            .to_buffer(row)
            .saturating_sub(LABEL_GAP_PX * scale.get())
    };
    crate::layout::Point {
        x: scale.to_buffer(at.x + sprite_w / 2),
        y: ceiling.map_or(clear_of(at.y), |top| clear_of(at.y).min(clear_of(top))),
    }
}

/// The pack sprite for a waypoint kind, when it has one. `None` covers a seat
/// slot, a fixture drawn elsewhere (Pantry by its room, the Couch as a mirrored
/// meeting sofa), and the corridor appliances (VendingMachine/Printer), which
/// [`paint_appliance`] draws.
fn waypoint_sprite(kind: crate::layout::WaypointKind) -> Option<&'static str> {
    use crate::layout::WaypointKind as K;
    match kind {
        K::PhoneBooth => Some("phone_booth"),
        K::StandingDesk => Some("standing_desk"),
        K::SnackShelf => Some("snack_shelf"),
        K::Couch
        | K::Pantry
        | K::VendingMachine
        | K::Printer
        // A seat slot: a meeting sofa's body is the trio's; this profile draws
        // no meeting-chair or kitchen-island body.
        | K::MeetingSofa
        | K::MeetingChair
        | K::Island => None,
    }
}

/// The meeting table — a slab, because the classic painter draws it
/// procedurally too and there is no sprite to reuse.
fn paint_table(at: crate::layout::Point, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    let ramp = Ramp::from_base(theme.furniture.wood_top);
    let crate::layout::Size { w, h } =
        crate::layout::furniture_def(crate::layout::Furniture::MeetingTable).visual;
    let crate::layout::Point { x, y } =
        crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, w, h);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        scale.to_buffer(w),
        scale.to_buffer(h),
        &ramp,
        scale,
    );
    // A front face, as the base desk gets one.
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y + h),
        scale.to_buffer(w),
        scale.to_buffer(desk_front_h()),
        &Ramp::from_base(theme.furniture.wood_trim),
        scale,
    );
    // ...and the ground contact the other floor-standing pieces get: without it
    // the table would be the one piece with no weight, floating beside the
    // seated sofas.
    contact_shadow(
        crate::layout::Point {
            x,
            y: y + desk_front_h(),
        },
        w,
        h,
        theme,
        scale,
        buf,
    );
}

/// A corridor appliance as a cutaway solid. Vending machine and printer have no
/// sprite — classic paints them per-pixel — so this gives them a lit body and a
/// contact shadow. Its box is [`furniture_def`](crate::layout::furniture_def)'s
/// `visual`, the box the classic painter draws it at, not a second set of numbers.
fn paint_appliance(
    at: crate::layout::Point,
    kind: crate::layout::WaypointKind,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    use crate::layout::WaypointKind as K;
    let def = crate::layout::furniture_def(kind.furniture());
    let (body, panel) = match kind {
        K::Printer => (theme.appliance.printer_body, theme.appliance.printer_glass),
        _ => (theme.appliance.vending_body, theme.appliance.vending_panel),
    };
    let (w, h) = (def.visual.w, def.visual.h);
    let crate::layout::Point { x, y } =
        crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, w, h);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        scale.to_buffer(w),
        scale.to_buffer(h),
        &Ramp::from_base(body),
        scale,
    );
    // The lit face — a vending display or a printer's glass — is what stops
    // these reading as anonymous blocks in a dark corridor.
    if w > 2 && h > 2 {
        fill(
            buf,
            scale.to_buffer(x + 1),
            scale.to_buffer(y + 1),
            scale.to_buffer(w.saturating_sub(2)),
            scale.to_buffer((h / 2).max(1)),
            panel,
        );
    }
    contact_shadow(crate::layout::Point { x, y }, w, h, theme, scale, buf);
}

/// Blit a floor-standing prop from the pack, centred on its layout point.
///
/// The layout already places these and the pack already draws them; the cutaway
/// only adds the ground contact a top-down view never needed.
fn paint_prop(
    at: crate::layout::Point,
    sprite: &str,
    mirrored: bool,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(dense) = crate::pixel_painter::densest_frame(pack, sprite, 0, scale) else {
        return;
    };
    let mirrored_art;
    let art = if mirrored {
        mirrored_art = dense.frame.mirror_vertical();
        &mirrored_art
    } else {
        dense.frame
    };
    // The layout's point is the piece's CENTRE; `blit_frame_scaled` takes a
    // top-left, so undo the centring in logical space before converting.
    let (w, h) = dense.logical;
    let crate::layout::Point { x, y } =
        crate::layout::anchored_top_left(crate::layout::Anchor::Center, at, w, h);
    contact_shadow(crate::layout::Point { x, y }, w, h, theme, scale, buf);
    blit_frame_scaled(
        art,
        scale.to_buffer(x),
        scale.to_buffer(y),
        dense.blit_at,
        buf,
    );
}

/// The tone where a solid meets the floor: the carpet's own shade under the key
/// light, so contact reads as weight rather than as a colour of its own.
fn contact_tone(theme: &Theme) -> pixtuoid_core::sprite::Rgb {
    Ramp::from_base(theme.surface.carpet_dark).shade
}

/// A tight dark band where a solid meets the floor — one row, not an ellipse: a
/// wide soft pool reads as a stain on a dark carpet, while a band the width of
/// the sprite reads as weight.
fn contact_shadow(
    at: crate::layout::Point,
    sprite_w: u16,
    sprite_h: u16,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let shade = contact_tone(theme);
    let s = scale.get();
    fill(
        buf,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y + sprite_h),
        scale.to_buffer(sprite_w),
        s,
        shade,
    );
}

/// A task chair from the pack's art, with the contact shadow its base casts.
fn paint_chair(
    at: crate::layout::Point,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = crate::pixel_painter::densest_frame(
        pack,
        crate::pixel_painter::DESK_CHAIR_SPRITE,
        0,
        scale,
    ) else {
        return;
    };
    let (w, h) = art.logical;
    contact_shadow(at, w, h, theme, scale, buf);
    blit_frame_scaled(
        art.frame,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y),
        art.blit_at,
        buf,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pack = pack();
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
        let pack = pack();
        let scale = RenderScale::new(pack.max_density_variant()).expect("nonzero");
        let desk = crate::layout::Point { x: 0, y: 20 };
        let art = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
        let desk_box = desk_span(&pack, art, desk, scale).expect("desk");
        let (_, art_h) = art_size(&pack, art).expect("desk");
        // The first row south of the ART, measured from its placement.
        let feet = crate::pixel_painter::desk_art_top(&pack, desk.y, art_h) + art_h;
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
        let order = depth_sort(vec![
            (
                person,
                PieceKind::Character {
                    idx: 0,
                    chair: None,
                    badge_ceiling: None,
                },
            ),
            (
                desk_box,
                PieceKind::Desk {
                    at: desk,
                    art,
                    screen: None,
                },
            ),
        ]);
        assert!(
            matches!(
                order[..],
                [PieceKind::Desk { .. }, PieceKind::Character { .. }]
            ),
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
        let pack = pack();
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
            1,
        );
        assert!(behind_z < desk_z, "desk {desk_z}, walker {behind_z}");
    }

    /// A centre-anchored prop standing in the aisle SOUTH of a desk must paint
    /// in front of that desk and behind its occupant. Keyed on its own middle
    /// row, a tall plant between the two would paint over the occupant while
    /// standing behind them.
    #[test]
    fn an_aisle_prop_sorts_between_the_desk_and_its_occupant() {
        let pack = pack();
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
            1,
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
        let pack = pack();
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
        let pack = pack();
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

        let mut chairs = Vec::new();
        push_chairs(&layout, &pack, &north[..1], &mut chairs);
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

    /// The desk sorts on the row just above the contact row it paints. Pinned to
    /// the paint itself: the ordering tests compare depths by inequality, which a
    /// one-row shift passes.
    #[test]
    fn a_desk_sorts_on_the_row_above_the_contact_row_it_paints() {
        let pack = pack();
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let desk = crate::layout::Point { x: 20, y: 30 };
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let art = desk_art(&pack, facing).expect("desk art");
            for s in [1, pack.max_density_variant()] {
                let scale = RenderScale::new(s).expect("nonzero");
                let span = desk_span(&pack, art, desk, scale).expect("desk");
                let (w, h) = (scale.to_buffer(64), scale.to_buffer(64));
                let mut buf = RgbBuffer::filled(w, h, theme.surface.bg_fallback);
                paint_desk(desk, art, None, &pack, theme, scale, &mut buf);
                let contact = contact_tone(theme);
                let contact_row = (0..h)
                    .rev()
                    .find(|&y| {
                        (0..w).any(|x| {
                            buf.as_slice()[usize::from(y) * usize::from(w) + usize::from(x)]
                                == contact
                        })
                    })
                    .map(|y| scale.logical(y))
                    .expect("the desk paints a contact row");
                assert_eq!(span.depth + 1, contact_row, "{art} at scale {s}");
            }
        }
    }

    /// A standing chair sorts on the classic painter's own chair key.
    #[test]
    fn a_chair_sorts_on_the_classic_chair_key() {
        let pack = pack();
        let desk = crate::layout::Point { x: 20, y: 30 };
        let (span, _) = chair_span(&pack, crate::layout::Facing::North, desk)
            .expect("a back-turned desk stands a chair");
        assert_eq!(
            span.depth,
            crate::pixel_painter::desk_chair_z_key(desk, crate::layout::Facing::North)
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
            push_characters(frame, &layout, &pack, RenderScale::ONE, &mut order);
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
        let chair = Span::new(9, 15, 10, 20, 1).with_depth(40);
        let piece = occupant_span(body, 31, Some(chair));
        assert_eq!(
            (piece.x0, piece.x1, piece.y0, piece.y1, piece.depth),
            (9, 18, 15, 35, 40)
        );
    }

    /// Relighting recolors the screen KEYS and nothing else — not even a pixel
    /// of another key the same colour as the glass — so the glow is exactly the
    /// screen the art drew, at whatever density.
    #[test]
    fn a_lit_screen_relights_only_the_screen_keys() {
        let glass = crate::pixel_painter::SCREEN_GLASS_KEY;
        let text = crate::pixel_painter::SCREEN_TEXT_KEY;
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

    /// Pins the screen keys ([`SCREEN_GLASS_KEY`](crate::pixel_painter::SCREEN_GLASS_KEY),
    /// [`SCREEN_TEXT_KEY`](crate::pixel_painter::SCREEN_TEXT_KEY)) to the bundled
    /// art: the raised desk a back-turned sitter works at must light its glass
    /// and its text at every density the pack draws it, or a key names nothing
    /// and that screen never lights.
    #[test]
    #[cfg(feature = "density-art")]
    fn the_bundled_back_turned_desk_draws_its_screen_in_the_screen_keys() {
        let pack = pack();
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let sentinel = pixtuoid_core::sprite::Rgb {
            r: 255,
            g: 0,
            b: 255,
        };
        let glass_and_text = [SCREEN_GLASS_LEVEL, SCREEN_TEXT_LEVEL];
        let variants = (2..=pack.max_density_variant())
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
        let pack = pack();
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
    fn sit_down(
        facing: crate::layout::Facing,
        seated_ticks: usize,
    ) -> (Layout, Pack, Vec<SimFrame>, crate::layout::Point) {
        sit_down_in(pack(), facing, seated_ticks)
    }

    /// [`sit_down`] with `pack` drawing the office.
    fn sit_down_in(
        pack: Pack,
        facing: crate::layout::Facing,
        seated_ticks: usize,
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
        let id = pixtuoid_core::AgentId::from_transcript_path("/cutaway/sit.jsonl");
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
                .observe(
                    &scene,
                    &pack,
                    LOGICAL.0,
                    LOGICAL.1,
                    meta,
                    now0 + Duration::from_millis(100 * n),
                )
                .expect("lays out");
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
    /// drawn through the real render, it lands above the desk art's top.
    #[test]
    fn a_back_turned_sitters_badge_clears_their_raised_monitor() {
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
        let seated = frames.last().expect("a seated frame");
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let scale = RenderScale::new(4).expect("nonzero");
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            theme.surface.bg_fallback,
        );
        let mut cache = crate::frame_cache::FrameCache::new();
        let labels = render_cutaway(
            seated,
            &layout,
            &pack,
            theme,
            scale,
            std::time::SystemTime::UNIX_EPOCH,
            &mut cache,
            &mut buf,
        );
        let label = labels.first().expect("the sitter has a badge");
        let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
        let top = desk_span(&pack, art, desk, RenderScale::ONE)
            .expect("desk")
            .y0;
        assert!(
            label.anchor_px.y < scale.to_buffer(top),
            "badge at y {} is not above the monitor top at {}",
            label.anchor_px.y,
            scale.to_buffer(top)
        );
    }

    /// Who carries a chair is ONE decision: a sitter skipped for art the pack
    /// lacks carries nothing, so their desk still stands its own chair.
    #[test]
    fn a_sitter_the_pack_cannot_draw_leaves_their_chair_standing() {
        let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
        let seated = frames.last().expect("a seated frame");
        let mut order = Vec::new();
        assert_eq!(
            push_characters(seated, &layout, &pack, RenderScale::ONE, &mut order),
            vec![desk]
        );

        let chair_only = pixtuoid_core::sprite::format::load_pack_from_strings(
            &format!(
                "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
                 [animations.{}]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
                crate::pixel_painter::DESK_CHAIR_SPRITE
            ),
            &[("one.sprite", "@frame 0\nA")],
        )
        .expect("pack builds");
        let mut order = Vec::new();
        let carried = push_characters(seated, &layout, &chair_only, RenderScale::ONE, &mut order);
        assert!(carried.is_empty() && order.is_empty(), "no character art");
        push_chairs(&layout, &chair_only, &carried, &mut order);
        assert!(
            order
                .iter()
                .any(|(_, k)| matches!(k, PieceKind::Chair { at } if Some(*at)
                    == crate::pixel_painter::desk_chair_top_left(&chair_only, desk, crate::layout::Facing::North))),
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
        let order = draw_list(frame, layout, pack, theme, RenderScale::ONE);
        let (person, person_span) = order
            .iter()
            .enumerate()
            .find_map(|(i, (s, k))| matches!(k, PieceKind::Character { .. }).then_some((i, *s)))?;
        if let (_, PieceKind::Character { chair: Some(_), .. }) = &order[person] {
            return Some(true);
        }
        let (at, chair_span) =
            crate::pixel_painter::desk_chair_top_left(pack, desk, crate::layout::Facing::North)
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
        let seat_key = crate::pixel_painter::desk_chair_z_key(desk, Facing::North);
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
            push_characters(frame, &layout, &pack, RenderScale::ONE, &mut order);
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
        use crate::layout::{Facing, CHARACTER_SPRITE_W};
        let desk = crate::layout::Point { x: 40, y: 30 };
        let near =
            crate::pixel_painter::seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::North);
        let far =
            crate::pixel_painter::seated_anchor_facing(desk, CHARACTER_SPRITE_W, Facing::South);
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
    /// `overlay::build_overlay` anchors off the classic projection, which for a
    /// seated agent is not where the cutaway draws them.
    #[test]
    fn a_label_anchor_sits_above_the_head_and_centred_on_the_sprite() {
        let scale = RenderScale::new(3).expect("nonzero");
        let at = crate::layout::Point { x: 10, y: 20 };
        let anchor = label_anchor(at, 8, None, scale);
        assert_eq!(
            anchor.x,
            scale.to_buffer(at.x + 4),
            "centred on the sprite, in logical space then converted"
        );
        assert!(
            anchor.y < scale.to_buffer(at.y),
            "the badge must clear the head, not overlap it"
        );
        assert_eq!(
            scale.to_buffer(at.y) - anchor.y,
            LABEL_GAP_PX * scale.get(),
            "the gap scales with the render, or it closes up at 8x"
        );
    }

    /// A ceiling ABOVE the head lifts the badge clear of it; one below the head
    /// changes nothing.
    #[test]
    fn a_label_anchor_clears_a_ceiling_above_the_head() {
        let scale = RenderScale::new(3).expect("nonzero");
        let at = crate::layout::Point { x: 10, y: 20 };
        let free = label_anchor(at, 8, None, scale);
        let raised = label_anchor(at, 8, Some(at.y - 4), scale);
        assert_eq!(
            raised.y,
            scale.to_buffer(at.y - 4) - LABEL_GAP_PX * scale.get(),
            "the badge clears the monitor top by the same gap it clears a head by"
        );
        assert_eq!(raised.x, free.x);
        assert_eq!(label_anchor(at, 8, Some(at.y + 4), scale), free);
    }

    /// The layout leaves walkable rows between the wall band and `top_margin`.
    #[test]
    fn the_wall_band_stops_where_the_layout_says_the_floor_begins() {
        let layout = Layout::compute_with_seed(160, 96, None, 0).expect("lays out");
        let band_h = layout
            .top_margin
            .saturating_sub(crate::layout::WALL_BAND_TO_TOP_MARGIN);
        assert!(band_h > 0, "a laid-out office has a wall band");
        assert!(
            band_h < layout.top_margin,
            "the band must end ABOVE top_margin, leaving walkable rows: \
             band {band_h}, top_margin {}",
            layout.top_margin
        );
    }

    /// The bundled pack.
    fn pack() -> Pack {
        crate::embedded_pack::load_sprite_pack(crate::embedded_pack::PackSource::Bundled)
            .expect("the embedded pack loads")
    }

    fn near_seat(desk: crate::layout::Point) -> crate::layout::Point {
        crate::pixel_painter::seated_anchor_facing(
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

    /// The whole draw list of a REAL office, checked against every pairwise
    /// "must be behind" fact its own geometry states — what a sort key cannot
    /// give you. Also the long-object guard: an unsplit wall run would sort its
    /// south end in front of the room's contents, and the pantry counter, INSIDE
    /// a room between its north and south walls, is the piece that catches it.
    #[test]
    fn a_real_offices_draw_list_satisfies_every_ordering_constraint() {
        let pack = pack();
        for (w, h) in [(160u16, 96u16), (240, 144), (100, 60)] {
            let layout = Layout::compute_with_seed(w, h, None, 0).expect("lays out");
            let mut order: Vec<(Span, PieceKind)> = Vec::new();
            wall_segments(&layout, &mut order);
            push_pantry_counter(&layout, &pack, &mut order);
            for pl in &layout.plants {
                if let Some((pw, ph)) = art_size(&pack, pl.kind.sprite_name()) {
                    order.push((
                        piece_span(crate::layout::Anchor::Center, pl.pos, pw, ph, 1),
                        PieceKind::Prop {
                            at: pl.pos,
                            sprite: pl.kind.sprite_name(),
                            mirrored: false,
                        },
                    ));
                }
            }
            for (i, d) in layout.home_desks.iter().enumerate() {
                let facing = layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i));
                let art = desk_art(&pack, facing).expect("the bundled pack has the desk art");
                order.push((
                    desk_span(&pack, art, *d, RenderScale::ONE)
                        .expect("the bundled pack has the desk art"),
                    PieceKind::Desk {
                        at: *d,
                        art,
                        screen: None,
                    },
                ));
            }
            push_chairs(&layout, &pack, &[], &mut order);
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
            for s in [1, 3, pack.max_density_variant()] {
                let scale = RenderScale::new(s).expect("nonzero");
                for (span, kind) in draw_list(frame, layout, pack, theme, scale) {
                    if only_people && !matches!(kind, PieceKind::Character { .. }) {
                        continue;
                    }
                    kinds.insert(kind_name(&kind));
                    if let PieceKind::Prop { sprite, .. } = kind {
                        props.insert(sprite);
                    }
                    assert_eq!(
                        stray_pixel(&kind, span, frame, layout, pack, theme, scale),
                        None,
                        "{kind:?} at scale {s} wrote a logical pixel outside {span:?}"
                    );
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
        let pack = pack();
        for (w, h) in [(240u16, 144u16), (100, 60)] {
            let frame = FloorSession::new()
                .observe(
                    &pixtuoid_core::SceneState::uniform(16),
                    &pack,
                    w,
                    h,
                    FloorMeta::ground(),
                    std::time::SystemTime::UNIX_EPOCH,
                )
                .expect("lays out");
            let layout = Layout::compute_with_seed(w, h, None, FloorMeta::ground().floor_seed)
                .expect("lays out");
            check(&pack, &frame, &layout, false);
        }
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            [
                "appliance",
                "chair",
                "character",
                "desk",
                "prop",
                "table",
                "wall"
            ],
            "a piece kind went untested"
        );
        // Every builder that pushes a "prop" must have been reached: the pantry
        // counter at both sizes, a sofa, a plant.
        for sprite in crate::pixel_painter::PANTRY_COUNTER_ANIMS
            .into_iter()
            .chain(["meeting_sofa", "plant"])
        {
            assert!(
                props.contains(sprite),
                "no {sprite} prop was painted: {props:?}"
            );
        }
    }

    fn kind_name(kind: &PieceKind) -> &'static str {
        match kind {
            PieceKind::WallSeg { .. } => "wall",
            PieceKind::Desk { .. } => "desk",
            PieceKind::Chair { .. } => "chair",
            PieceKind::Prop { .. } => "prop",
            PieceKind::Table { .. } => "table",
            PieceKind::Appliance { .. } => "appliance",
            PieceKind::Character { .. } => "character",
        }
    }

    /// The first logical pixel `kind` writes outside `span`, painted alone.
    fn stray_pixel(
        kind: &PieceKind,
        span: Span,
        frame: &SimFrame,
        layout: &Layout,
        pack: &Pack,
        theme: &Theme,
        scale: RenderScale,
    ) -> Option<(u16, u16)> {
        use pixtuoid_core::sprite::Rgb;
        let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
        let [a, b] = [
            Rgb { r: 0, g: 0, b: 0 },
            Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
        ]
        .map(|fill| {
            let mut buf = RgbBuffer::filled(w, h, fill);
            let mut cache = crate::frame_cache::FrameCache::new();
            paint_piece(
                kind,
                frame,
                pack,
                theme,
                scale,
                std::time::SystemTime::UNIX_EPOCH,
                &mut cache,
                &mut buf,
            );
            buf
        });
        a.as_slice()
            .iter()
            .zip(b.as_slice())
            .enumerate()
            .filter(|(_, (pa, pb))| pa == pb)
            .map(|(i, _)| {
                let (x, y) = (i % usize::from(w), i / usize::from(w));
                (scale.logical(x as u16), scale.logical(y as u16))
            })
            .find(|&(x, y)| !((span.x0..=span.x1).contains(&x) && (span.y0..=span.y1).contains(&y)))
    }

    /// Splitting is what makes the office above orderable, so pin it directly:
    /// no wall piece may be tall enough to span a figure.
    #[test]
    fn no_wall_segment_is_taller_than_the_cast() {
        let pack = pack();
        let (_, body_h) = base_size(&pack, "standing");
        let layout = Layout::compute_with_seed(240, 144, None, 0).expect("lays out");
        let mut order: Vec<(Span, PieceKind)> = Vec::new();
        wall_segments(&layout, &mut order);
        assert!(!order.is_empty(), "a laid-out office has rooms");
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
    /// PRODUCT has to agree with the logical size the desk's foot (face or
    /// contact row) is placed by, and getting it wrong is silent — the desk
    /// still renders, with its foot a whole desk below the surface.
    #[test]
    fn the_drawn_size_is_the_same_whichever_density_the_art_came_from() {
        let pack = pack();
        let (bw, bh) = base_size(&pack, "desk");
        for s in 1..=12u16 {
            let scale = RenderScale::new(s).expect("nonzero");
            let d = crate::pixel_painter::densest_frame(&pack, "desk", 0, scale)
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

    /// The flip side of `densest_frame`'s "one piece at a time" (its own tests
    /// pin a variant winning): a piece with no variant that lands at the scale
    /// must be untouched by the lookup, or adding one `@Nx` sprite would be a
    /// flag day for all of them. `plant` ships only 8x art, and 8 does not
    /// divide 4.
    #[test]
    fn a_piece_with_no_variant_at_the_scale_renders_exactly_as_it_did_before() {
        let pack = pack();
        assert!(
            pack.animation("plant@4x").is_none() && pack.animation("plant@2x").is_none(),
            "this test is only meaningful while no `plant` variant divides 4"
        );
        let (bw, _) = base_size(&pack, "plant");
        let scale = RenderScale::new(4).expect("nonzero");
        let d = crate::pixel_painter::densest_frame(&pack, "plant", 0, scale)
            .expect("plant is in the pack");
        assert_eq!((d.frame.width(), d.blit_at.get()), (bw, 4));
    }

    /// [`desk_face_rows`]' rule, through the real paint.
    #[test]
    #[cfg(feature = "density-art")]
    fn only_the_top_down_base_desk_gets_a_derived_front_face() {
        let pack = pack();
        let theme = &crate::theme::NORMAL;
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
            paint_desk(at, "desk", None, &pack, theme, scale, &mut buf);
            let (x, below) = (
                scale.to_buffer(at.x + bw / 2),
                scale.to_buffer(span.y0 + bh),
            );
            assert_eq!(
                buf.get(x, below) != contact_tone(theme),
                face,
                "scale {s}: the row under the art is {}",
                if face {
                    "the derived face"
                } else {
                    "the contact shadow"
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
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let at = crate::layout::Point { x: 3, y: 3 };
        let blank = || RgbBuffer::filled(16, 16, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        let drawn = |buf: &RgbBuffer| buf.get(scale.to_buffer(at.x), scale.to_buffer(at.y));

        let mut buf = blank();
        paint_prop(at, "plant", false, &pack, theme, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "prop");
        let mut buf = blank();
        paint_prop(at, "plant", true, &pack, theme, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "mirrored prop");
        let mut buf = blank();
        paint_wall_decor(at, "whiteboard", &pack, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "wall decor");
        let mut buf = blank();
        paint_chair(at, &pack, theme, scale, &mut buf);
        assert_eq!(drawn(&buf), variant, "chair");
    }
}
