//! The cutaway profile's paint pass — the second reader of `SimFrame`.
//!
//! Deliberately partial. Floor, wall band, rooms, desks, furniture and the
//! cast are here; EFFECTS are not — weather, glow, steam and the pet all
//! belong to the classic pass and change no part of the question this profile
//! exists to answer: does an orthographic cutaway of THIS office, at THIS
//! density, read better than the classic top-down?
//!
//! It reads `SimFrame` and never advances anything: the sim already ran, and a
//! second renderer that could move the world would make the two profiles
//! disagree about the office.

use pixtuoid_core::sprite::blit::blit_frame_scaled;
use pixtuoid_core::sprite::format::{density_variant_name_into, Pack};
use pixtuoid_core::sprite::RgbBuffer;

use crate::cutaway::order::{depth_sort, Span};
use crate::cutaway::shade::{dither_band, fill, slab, Ramp};
use crate::layout::{Layout, DESK_H};
use crate::pixel_painter::SimFrame;
use crate::render_scale::RenderScale;
use crate::theme::Theme;

/// How much of a desk's height is its FRONT face rather than its top surface.
///
/// The one number that turns a top-down rectangle into a solid: without a front
/// face there is no thickness, and the office reads as a floor plan. Kept a
/// fraction of `DESK_H` so it tracks the desk rather than drifting from it.
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

/// Tint/shade strength for a derived [`Ramp`], in percent.
///
/// One value for every material: the room reads as lit from a single direction
/// because nothing gets its own exposure. Tuned on the ratified visual mock.
const RAMP_TINT_PCT: u8 = 26;
/// Shade counterpart of [`RAMP_TINT_PCT`], deliberately deeper — a surface
/// turning away from the only light loses more than a facing one gains.
const RAMP_SHADE_PCT: u8 = 34;

/// Rows of chair back visible BELOW a seated occupant.
///
/// Deliberately small. A first pass covered the torso from the waist down and
/// swallowed the figure — the shirt vanished behind a dark block. The occupant
/// is already grounded by the desk in front of them, so the chair only has to
/// peek out beneath, not carry the pose.
const CHAIR_BACK_H: u16 = 3;

/// Narrowest skyline building, in logical units.
const SKYLINE_MIN_W: u16 = 3;
/// How much wider than [`SKYLINE_MIN_W`] a building may be.
const SKYLINE_W_SPREAD: u16 = 6;
/// Shortest skyline building — below this the city reads as a jagged floor.
const SKYLINE_MIN_H: u16 = 2;

/// Meeting-table footprint, in logical units. Sized to the trio's sofa gap.
const TABLE_W: u16 = 18;
/// Height of [`TABLE_W`]'s table.
const TABLE_H: u16 = 6;

/// Logical rows between a head and its name badge.
const LABEL_GAP_PX: u16 = 2;

/// Thickness of a room's glass wall, in logical units.
const ROOM_WALL_PX: u16 = 1;

/// How far down the desk sprite the screen's spill lands.
const GLOW_ROW_NUMER: u16 = 5;
/// Denominator of [`GLOW_ROW_NUMER`].
const GLOW_ROW_DENOM: u16 = 8;

/// Where a painter should hang one agent's name badge, in BUFFER pixels.
///
/// The engine cannot draw text — the font lives in the binary — so the profile
/// reports anchors and lets the painter render. Crucially these are the
/// CUTAWAY's anchors: `overlay::build_overlay` derives its own from the classic
/// projection, so a badge placed with those would float where the classic
/// painter would have drawn the body, not where this one did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutawayLabel {
    /// Index into [`SimFrame::agents`].
    pub agent_idx: usize,
    /// Badge anchor: horizontal centre of the sprite, just above its head.
    pub anchor_px: crate::layout::Point,
}

/// Paint `frame`'s office into `buf` as an orthographic cutaway.
///
/// `layout` is in LOGICAL units and `buf` in buffer pixels; `scale` converts.
/// The classic painter is untouched — this is its sibling, not its successor.
///
/// Returns where each visible agent's badge belongs; see [`CutawayLabel`].
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

    // ONE ordered draw list, so a character and the desk it sits at resolve
    // against each other by depth instead of by which loop ran first. That
    // ordering IS the occlusion — there is no separate occlusion pass.
    let mut order: Vec<(Span, PieceKind)> =
        Vec::with_capacity(layout.home_desks.len() + frame.characters.len());
    // A desk's screen is lit iff the sim says someone is seated at it — the
    // observation is already in the frame, so the profile never re-derives it.
    let (desk_w, desk_h) = art_size(pack, "desk").unwrap_or((DESK_H, DESK_H));
    order.extend(layout.home_desks.iter().enumerate().map(|(i, d)| {
        (
            // `paint_desk` blits at the classic anchor's 1px monitor-bezel raise
            // and adds a front face below — that is the box it occupies.
            piece_span(
                crate::layout::Anchor::TopLeft,
                crate::layout::Point {
                    x: d.x,
                    y: d.y.saturating_sub(1),
                },
                desk_w,
                desk_h,
                desk_front_h(),
            ),
            PieceKind::Desk {
                at: *d,
                lit: frame
                    .seated_agents
                    .get(&pixtuoid_core::state::FloorLocalDeskIndex(i))
                    .copied()
                    .unwrap_or(false),
            },
        )
    }));
    // Every centre-anchored prop rides one helper, so none of them can grow its
    // own convention again.
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
        push_prop(&mut order, pl.pos, pl.kind.sprite_name());
    }
    // Standalone waypoint furniture. Seats (MeetingSofa/MeetingChair/Island)
    // are deliberately absent: they are slots ON a body that is painted once
    // from the room's trio, so one sprite per seat would triple-paint it.
    for wp in &layout.waypoints {
        if let Some(sprite) = waypoint_sprite(wp.kind) {
            push_prop(&mut order, wp.pos, sprite);
        }
    }
    for d in &layout.pod_decor {
        push_prop(&mut order, d.pos, d.kind.sprite_name());
    }
    // The lounge couch IS a vertical-mirrored meeting sofa, back facing NORTH
    // toward the windows — the classic painter's rule, read from there rather
    // than re-guessed. `back_couch` is NOT couch art: the pack documents it as a
    // character seen from behind, so drawing it here put a headless torso in the
    // corridor where the couch belongs.
    if let Some(at) = layout.couch_sprite_center() {
        push_sofa(&mut order, pack, at, true);
    }
    // Wall decor hangs on the north band, so it is NOT floor-sorted: it paints
    // with the wall, before anything standing on the floor can occlude it.
    for item in &layout.wall_decor {
        paint_wall_decor(item.pos, item.kind.sprite_name(), pack, scale, buf);
    }
    // The corridor appliances classic draws procedurally: they have no sprite
    // to reuse, so the cutaway gives them its own solid geometry.
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
    // The meeting trio: two sofa bodies plus the table between them. `sofas[0]`
    // is the north sofa and `sofas[1]` the south, and only the south one is
    // mirrored — that is what makes the pair FACE each other across the table
    // instead of both facing the same way.
    for t in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for (i, sofa) in t.sofas.iter().enumerate() {
            push_sofa(&mut order, pack, *sofa, i % 2 != 0);
        }
        order.push((
            // A front face below, and (now) a contact shadow under that.
            piece_span(
                crate::layout::Anchor::Center,
                t.table,
                TABLE_W,
                TABLE_H,
                desk_front_h() + 1,
            ),
            PieceKind::Table { at: t.table },
        ));
    }
    for (i, c) in frame.characters.iter().enumerate() {
        let at = cutaway_anchor(c);
        let Some((w, h)) = art_size(pack, c.anim_name) else {
            continue;
        };
        order.push((
            // A seated occupant's chair paints under them, so it is part of the
            // extent they occupy; everyone else carries only their shadow.
            piece_span(
                crate::layout::Anchor::TopLeft,
                at,
                w,
                h,
                if c.seat_desk.is_some() {
                    CHAIR_BACK_H
                } else {
                    1
                },
            ),
            PieceKind::Character { idx: i },
        ));
    }
    // Sorted, not a backdrop: a backdrop wall cannot occlude what is inside
    // the room. Split into segments first — see `wall_segments`.
    wall_segments(layout, &mut order);
    push_pantry_counter(layout, pack, &mut order);

    let ordered = depth_sort(order);

    let mut labels = Vec::with_capacity(frame.characters.len());
    for kind in &ordered {
        match *kind {
            PieceKind::Desk { at, lit } => paint_desk(at.x, at.y, lit, pack, theme, scale, buf),
            PieceKind::Character { idx } => {
                if let Some(l) = paint_character(frame, idx, pack, theme, scale, now, cache, buf) {
                    labels.push(l);
                }
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
    }
    labels
}

/// Queue one `meeting_sofa` body. `mirrored` flips it so its back faces north.
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

/// Rows of a vertical wall run per sorted segment.
///
/// A run spanning a whole room has no meaningful base row — its south edge
/// would sort the ENTIRE wall in front of everything the room contains — so it
/// is split into pieces that each do have one. This is the standard treatment
/// for the long-object case in a painter's-algorithm renderer, and no sort
/// predicate substitutes for it.
///
/// A segment must be no taller than the SHORTEST thing that can pass in front
/// of it: one segment spanning both sides of a figure has no correct position.
/// The bundled cast is 12 rows tall; 4 leaves headroom for a shorter pack, at
/// a piece count the draw list absorbs easily.
const WALL_SEG_H: u16 = 4;

/// Queue every room's walls as sorted pieces, vertical runs split.
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

/// One wall segment. A horizontal run gets the top-lit [`slab`] every solid in
/// this profile carries; a vertical one is seen edge-on, so it is a flat fill —
/// an edge tone on a 1-column strip would read as a highlight, not a material.
fn paint_wall_seg(
    at: crate::layout::Point,
    w: u16,
    h: u16,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let glass = Ramp::from_base(
        theme.office.room_wall_trim_light,
        RAMP_TINT_PCT,
        RAMP_SHADE_PCT,
    );
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
/// The span is computed WHERE THE PIECE IS BUILT, by [`piece_span`] over the
/// same anchor and painted extent the piece's paint fn uses — the extent
/// depends on the PACK, so a sprite's height is not knowable from its layout
/// point, and geometry derived from anything other than where the sprite lands
/// is geometry that can drift from it.
///
/// This replaced three incompatible hand-rolled conventions: a desk sorted on a
/// footprint-derived surface plane four rows above its art, centre-anchored
/// props sorted on their MIDDLE row while painting from their centre, and a
/// seated character kept the CLASSIC projection's key after the cutaway had
/// re-projected where it paints. Any prop whose centre fell between a desk and
/// its occupant painted over the occupant while standing behind them.
enum PieceKind {
    /// One segment of a room's wall run.
    WallSeg {
        /// Buffer-space rect, already converted — walls are pure geometry with
        /// no sprite, so there is nothing for the paint fn to look up.
        at: crate::layout::Point,
        w: u16,
        h: u16,
    },
    Desk {
        at: crate::layout::Point,
        lit: bool,
    },
    Prop {
        at: crate::layout::Point,
        sprite: &'static str,
        /// Flip rows top-to-bottom, turning a sofa's back to the north windows.
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
    },
}

/// A piece's screen footprint: the box its sprite occupies, plus the `below`
/// rows its painter draws underneath (a front face, a contact shadow, a chair
/// back).
///
/// Anchoring goes through [`crate::layout::anchored_top_left`], the same
/// function the walkable mask and the classic painter use, so the box a piece
/// SORTS by cannot drift from the box it BLITS into.
///
/// The desk needs NO special case any more. It used to sort on a surface plane
/// so a seated occupant would paint over it, but that was compensating for the
/// occupant ALSO carrying a wrong key: with both measured at their true base
/// rows the occupant's chair lands several rows south of the desk's front face,
/// so the ratified "head over the surface" reading falls out of the shared
/// convention instead of being bought with a divergence.
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

/// A pack sprite's LOGICAL size — the base animation, never a density variant,
/// since a variant's frame is in buffer units and the sort space is logical.
fn art_size(pack: &Pack, sprite: &str) -> Option<(u16, u16)> {
    pack.animation(sprite)
        .and_then(|a| a.frames.first())
        .map(|f| (f.width(), f.height()))
}

/// Rows of a desk that are its front face — the thickness.
fn desk_front_h() -> u16 {
    (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1)
}

/// The wall rows inset above and below the glass run, as a fraction of the
/// band. A 1/4 inset therefore leaves the middle HALF of the band as glass.
///
/// The windows are the cutaway's only light SOURCE on screen, so the band has to
/// read as glass and not as a stripe — it is what makes the north-to-south floor
/// falloff legible as light instead of as a gradient someone chose.
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
    let wall = Ramp::from_base(theme.surface.wall, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    slab(buf, 0, 0, w, scale.to_buffer(band_h), &wall, scale);

    // One glass run inset inside the band, with a lit sill under it — the sill
    // is what sells the light as coming THROUGH rather than being painted on.
    let inset = (band_h * WINDOW_INSET_NUMER / WINDOW_INSET_DENOM).max(1);
    let glass_h = band_h.saturating_sub(inset * 2);
    if glass_h > 0 {
        let glass = Ramp::from_base(theme.lighting.night_sky_a, RAMP_TINT_PCT, RAMP_SHADE_PCT);
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
    fill(
        buf,
        0,
        scale.to_buffer(band_h),
        w,
        s,
        Ramp::from_base(theme.surface.carpet_dark, 0, RAMP_SHADE_PCT).shade,
    );
}

/// A city skyline standing on the window sill, lit windows scattered through it.
///
/// Flat glass reads as a painted stripe; a skyline is what makes the band a
/// WINDOW, and the lit windows are what make it night. Deterministic from the
/// layout width so the same office always gets the same city — a per-frame
/// reshuffle would flicker.
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
    // A local mix, per this crate's documented convention: each noise site owns
    // its own finaliser over a disjoint domain (see the splitmix sharp edge).
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
/// the room reads as an empty glass box.
///
/// A sorted piece rather than a backdrop blit: it stands ON the floor, so an
/// agent at the counter has to resolve against it like any other solid, and it
/// earns the contact shadow every solid in this profile carries.
fn push_pantry_counter(layout: &Layout, pack: &Pack, order: &mut Vec<(Span, PieceKind)>) {
    let Some(pantry) = &layout.pantry else {
        return;
    };
    let sprite = if pantry.counter_size.w >= crate::layout::PANTRY_COUNTER_LARGE_W {
        "pantry"
    } else {
        "pantry_small"
    };
    let Some((w, h)) = art_size(pack, sprite) else {
        return;
    };
    // The layout places it TOP-LEFT; `paint_prop` centres, so convert once.
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

    // Anchored to where the FLOOR starts, not buffer row 0: the wall band
    // covers everything above `top_margin`, so measuring from the top hides
    // the lit zone behind it.
    let floor_top = scale.to_buffer(layout.top_margin);
    let floor_h = h.saturating_sub(floor_top);

    // North sixth solid lit, dithering to base by the end of the north third,
    // then a final fall to dark at the south edge.
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
    lx: u16,
    ly: u16,
    lit: bool,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let s = scale.get();
    let x = scale.to_buffer(lx);
    // The bezel raise for the BASE desk art. Classic lifts a `desk_north` by its
    // extra height on top of this, and the cutaway always blits `desk` — an art
    // difference, which is the cutaway's own to close, not a seat-side one.
    let top_y = scale.to_buffer(ly.saturating_sub(1));

    let Some((art, blit_at)) = densest_art(pack, "desk", scale) else {
        return;
    };
    blit_frame_scaled(art, x, top_y, blit_at, buf);

    // The front face is the SPRITE'S own material, shaded — sampled from its
    // base row rather than a theme role. Two reasons: the desk's colour lives in
    // the PACK (`"D" = #8b5a2b`), not the theme, so `furniture.wood_top` is a
    // different material entirely and reads nearly identical to the carpet in
    // some themes; and sampling means a custom `--pack-dir` desk gets a matching
    // front face for free instead of a mismatched one.
    let Some(material) = dominant_opaque_row(art, art.height().saturating_sub(1)) else {
        return;
    };
    // An occupied desk spills its screen light onto the surface in front of it.
    // The office is lit by two things — the windows and the monitors — and this
    // is the only place the second one shows.
    //
    // Full desk WIDTH, not the middle half. The occupant is 8 logical columns
    // centred on a 14-column desk, and a half-width band is a strict subset of
    // them on both axes — while `lit` means "someone is seated here", so the
    // occluder is present exactly when the glow is drawn. The band rendered
    // zero visible pixels. The wings either side of the body are where a spill
    // actually reads, and they are also where it belongs physically.
    if lit {
        let glow = Ramp::from_base(
            theme.effects.monitor_frame_lit,
            RAMP_TINT_PCT,
            RAMP_SHADE_PCT,
        );
        fill(
            buf,
            x,
            top_y + art.height() * blit_at.get() * GLOW_ROW_NUMER / GLOW_ROW_DENOM,
            art.width() * blit_at.get(),
            s,
            glow.lit,
        );
    }

    let ramp = Ramp::from_base(material, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    // Sized off what was ACTUALLY drawn, not off the base sprite: variant art blits
    // 1:1 and is already in buffer units, so multiplying again would put the
    // front face a whole desk below the surface it belongs to.
    let drawn_w = art.width() * blit_at.get();
    let drawn_h = art.height() * blit_at.get();
    let base_y = top_y + drawn_h;
    let w = drawn_w;
    slab(
        buf,
        x,
        base_y,
        w,
        scale.to_buffer(desk_front_h()),
        &ramp,
        scale,
    );

    // Contact occlusion hugs the front face; a wide pool reads as a stain.
    fill(
        buf,
        x,
        base_y + scale.to_buffer(desk_front_h()),
        w,
        s,
        Ramp::from_base(theme.surface.carpet_dark, 0, RAMP_SHADE_PCT).shade,
    );
}

/// The most common opaque colour in `row` of `frame`, if any.
///
/// How the cutaway learns a sprite's material without hardcoding it: the front
/// face a top-down sprite never had has to be SOME colour, and the sprite's own
/// base row is the only answer that stays right for a custom pack.
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

/// Wall-hung decor: blitted at its own `pos`, with NO ground shadow.
///
/// Two things separate it from [`paint_prop`], and the cutaway had both wrong
/// by routing it through there. `WallDecorItem.pos` is TOP-LEFT — the classic
/// painter blits it at `pos` directly and z-sorts it `Anchor::TopLeft`, unlike
/// the centre-pinned furniture — so centring it hung every board up and to the
/// west of where it belongs. And a wall-hung item touches no floor, so the
/// contact shadow that grounds a solid was landing partway up the wall.
fn paint_wall_decor(
    pos: crate::layout::Point,
    sprite: &str,
    pack: &Pack,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = pack.animation(sprite).and_then(|a| a.frames.first()) else {
        return;
    };
    blit_frame_scaled(
        art,
        scale.to_buffer(pos.x),
        scale.to_buffer(pos.y),
        scale.factor(),
        buf,
    );
}

/// NO re-projection: the seat side is a per-desk layout fact, so an override here would make the two
/// profiles disagree about which side of its desk half the office sits on.
fn cutaway_anchor(c: &crate::pixel_painter::CharacterPlacement) -> crate::layout::Point {
    c.anchor
}

#[allow(clippy::too_many_arguments)]
fn paint_character(
    frame: &SimFrame,
    idx: usize,
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
    // cache: a raw pack blit clones one placeholder-palette person twelve times.
    let glow_tint = crate::pixel_painter::character_glow_tint(c.glow, agent, theme);
    let (art, _burn) = crate::pixel_painter::seat::character_frame(
        c.anim_name,
        c.frame_idx,
        agent,
        pack,
        c.flip_x,
        glow_tint,
        cache,
        now,
    )?;
    let (art_w, art_h) = (art.width(), art.height());

    // Ground contact BEFORE the body, so the sprite sits on its own shadow
    // rather than the shadow being stamped over their feet.
    //
    // Only for someone STANDING on the floor. A seated occupant's chair back
    // starts at exactly the same row and is three rows deep, so the shadow was
    // drawn and then completely covered — dead pixels, and the chair is already
    // this figure's ground contact.
    if c.seat_desk.is_none() {
        contact_shadow(at, art_w, art_h, theme, scale, buf);
    }
    blit_frame_scaled(
        art,
        scale.to_buffer(at.x),
        scale.to_buffer(at.y),
        scale.factor(),
        buf,
    );
    // The chair paints straight after ITS occupant rather than as its own
    // sorted piece: it belongs to exactly one character, so tying it to them
    // makes the order correct by construction. The trade is that a chair cannot
    // occlude a DIFFERENT agent walking past — acceptable while the profile has
    // no walkers rendered at a desk, and the fix is a Piece::Chair when it does.
    if c.seat_desk.is_some() {
        paint_chair(at, art_w, art_h, theme, scale, buf);
    }
    Some(CutawayLabel {
        agent_idx: c.agent_idx,
        anchor_px: label_anchor(at, art_w, scale),
    })
}

/// The badge anchor for a body of `sprite_w` logical columns drawn at `at`:
/// horizontally centred, `LABEL_GAP_PX` logical rows clear of the head.
///
/// A free fn so the test can drive THE anchor rather than restate its
/// arithmetic — the earlier test recomputed this expression in its own body and
/// then asserted properties of its own copy, which stayed green for any change
/// to the real one.
fn label_anchor(
    at: crate::layout::Point,
    sprite_w: u16,
    scale: RenderScale,
) -> crate::layout::Point {
    crate::layout::Point {
        x: scale.to_buffer(at.x + sprite_w / 2),
        y: scale
            .to_buffer(at.y)
            .saturating_sub(LABEL_GAP_PX * scale.get()),
    }
}

/// The densest art the pack has for `name`, and the factor to blit it at.
///
/// A pack may ship `{name}@{N}x` — the SAME piece drawn on an N-times grid.
/// This picks the densest one that DIVIDES the render scale and blits it at
/// the remaining factor, so 4x art still halves the upscale on an 8x render
/// instead of being discarded for not being an exact match.
///
/// This is the whole mixed-density contract, and its direction is the point:
/// richer art REMOVES the upscale rather than fighting it, so the asset work
/// can land one piece at a time instead of as a flag day. A pack with no
/// variants renders exactly as it did before this existed.
///
/// A variant whose size is not its base's times the density its NAME claims is
/// SKIPPED rather than drawn wrong. `validate_pack_animations` reports it as a
/// hard error, so the check here is the render-time backstop for a pack that
/// was never validated — not the place an author is meant to find out.
fn densest_art<'a>(
    pack: &'a Pack,
    name: &str,
    scale: RenderScale,
) -> Option<(&'a pixtuoid_core::sprite::Frame, std::num::NonZeroU16)> {
    let base = pack.animation(name).and_then(|a| a.frames.first())?;
    let s = scale.get();
    // ONE buffer, reused: the lookup key is `<name>@<N>x` and this loop runs
    // per divisor, per piece, per frame — a fresh `String` each time is an
    // allocation for a HashMap probe that borrows it and drops it.
    let mut key = String::with_capacity(name.len() + 4);
    for density in (2..=s).rev() {
        if !s.is_multiple_of(density) {
            continue;
        }
        key.clear();
        density_variant_name_into(&mut key, name, density);
        let Some(art) = pack.animation(&key).and_then(|a| a.frames.first()) else {
            continue;
        };
        // Saturating for the same reason the validator's twin is: the density
        // is bounded, the BASE is not. A saturated expectation matches nothing,
        // so an absurd pair falls through to the base art rather than wrapping
        // into a size that happens to match.
        if art.width() != base.width().saturating_mul(density)
            || art.height() != base.height().saturating_mul(density)
        {
            continue;
        }
        // `density` divides `s` and both are >= 1, so the quotient is nonzero.
        if let Some(factor) = std::num::NonZeroU16::new(s / density) {
            return Some((art, factor));
        }
    }
    Some((base, scale.factor()))
}

/// The pack sprite for a waypoint kind, when it has one.
///
/// `None` covers three cases, all deliberate: a SEAT slot whose body paints
/// once elsewhere (MeetingSofa/MeetingChair/Island), a fixture already drawn by
/// its room (Pantry), and the corridor appliances the classic painter draws
/// PROCEDURALLY rather than from art (VendingMachine/Printer/Couch) — those
/// need their own cutaway geometry, not a missing-sprite lookup.
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
        | K::MeetingSofa
        | K::MeetingChair
        | K::Island => None,
    }
}

/// The meeting table — a slab, because the classic painter draws it
/// procedurally too and there is no sprite to reuse.
fn paint_table(at: crate::layout::Point, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    let ramp = Ramp::from_base(theme.furniture.wood_top, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    let (w, h) = (TABLE_W, TABLE_H);
    let x = at.x.saturating_sub(w / 2);
    let y = at.y.saturating_sub(h / 2);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        scale.to_buffer(w),
        scale.to_buffer(h),
        &ramp,
        scale,
    );
    // The same front face every solid in this profile gets.
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y + h),
        scale.to_buffer(w),
        scale.to_buffer(desk_front_h()),
        &Ramp::from_base(theme.furniture.wood_trim, RAMP_TINT_PCT, RAMP_SHADE_PCT),
        scale,
    );
    // ...and the ground contact every OTHER solid gets. Without it the table
    // was the one piece in the room with no weight, floating over the carpet
    // while the sofas either side of it sat on theirs.
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

/// A corridor appliance as a cutaway solid.
///
/// Vending machine and printer have no sprite — classic paints them per-pixel —
/// so this gives them the same body + front-face + lit-panel treatment every
/// other solid here gets. Its footprint comes from the SHARED furniture table,
/// not a second set of numbers, so the cutaway box matches the ground the mask
/// actually blocks.
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
    let x = at.x.saturating_sub(w / 2);
    let y = at.y.saturating_sub(h / 2);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        scale.to_buffer(w),
        scale.to_buffer(h),
        &Ramp::from_base(body, RAMP_TINT_PCT, RAMP_SHADE_PCT),
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
/// only adds the ground contact a top-down view never needed. Reusing the art
/// rather than re-inventing it is the whole shape of this profile's asset work.
fn paint_prop(
    at: crate::layout::Point,
    sprite: &str,
    mirrored: bool,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(base) = pack.animation(sprite).and_then(|a| a.frames.first()) else {
        return;
    };
    let art = if mirrored {
        base.mirror_vertical()
    } else {
        base.clone()
    };
    let art = &art;
    // The layout's point is the piece's CENTRE; `blit_frame_scaled` takes a
    // top-left, so undo the centring in logical space before converting.
    let x = at.x.saturating_sub(art.width() / 2);
    let y = at.y.saturating_sub(art.height() / 2);
    contact_shadow(
        crate::layout::Point { x, y },
        art.width(),
        art.height(),
        theme,
        scale,
        buf,
    );
    blit_frame_scaled(
        art,
        scale.to_buffer(x),
        scale.to_buffer(y),
        scale.factor(),
        buf,
    );
}

/// A tight dark band where a figure meets the floor.
///
/// One row, not an ellipse: a wide soft pool reads as a stain on a dark carpet,
/// while a band the width of the sprite reads as weight.
fn contact_shadow(
    at: crate::layout::Point,
    sprite_w: u16,
    sprite_h: u16,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let shade = Ramp::from_base(theme.surface.carpet_dark, 0, RAMP_SHADE_PCT).shade;
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

/// A chair back covering the occupant's lower torso.
///
/// Without it a seated figure floats: the cutaway shows their whole body, where
/// the classic painter hid the lower half behind the desk's overhang.
fn paint_chair(
    at: crate::layout::Point,
    sprite_w: u16,
    sprite_h: u16,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let ramp = Ramp::from_base(theme.furniture.chair_trim, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    // Just below the body, one pixel proud on each side — a seat back peeking
    // out, not a panel over the occupant.
    slab(
        buf,
        scale.to_buffer(at.x.saturating_sub(1)),
        scale.to_buffer(at.y + sprite_h),
        scale.to_buffer(sprite_w + 2),
        scale.to_buffer(CHAIR_BACK_H),
        &ramp,
        scale,
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
        piece_span(anchor, pos, 1, h, below).y1
    }

    /// Every piece's sort row measured the same way: the south base row of what
    /// it actually blits. The occupant's chair lands south of the desk's front
    /// face, so the ratified "head over the surface" reading falls out of the
    /// shared convention — the desk needs no divergence of its own.
    #[test]
    fn a_seated_occupant_sorts_in_front_of_the_desk_it_sits_at() {
        let pack = pack();
        let desk = crate::layout::Point { x: 0, y: 10 };
        let (_, desk_h) = base_size(&pack, "desk");
        let (_, body_h) = base_size(&pack, "seated");

        let desk_z = sort_row(
            crate::layout::Anchor::TopLeft,
            crate::layout::Point {
                x: desk.x,
                y: desk.y - 1,
            },
            desk_h,
            desk_front_h(),
        );
        let seated_z = sort_row(
            crate::layout::Anchor::TopLeft,
            near_seat(desk),
            body_h,
            CHAIR_BACK_H,
        );
        assert!(
            seated_z > desk_z,
            "a seated occupant must paint over its desk (desk {desk_z}, seated {seated_z})"
        );
    }

    /// The other half: someone on the FAR side is occluded BY the desk, which is
    /// what gives the office depth rather than a flat plan.
    #[test]
    fn a_character_north_of_the_desk_sorts_behind_it() {
        let pack = pack();
        let desk = crate::layout::Point { x: 0, y: 20 };
        let (_, desk_h) = base_size(&pack, "desk");
        let (_, body_h) = base_size(&pack, "standing");

        let desk_z = sort_row(
            crate::layout::Anchor::TopLeft,
            crate::layout::Point {
                x: desk.x,
                y: desk.y - 1,
            },
            desk_h,
            desk_front_h(),
        );
        // Standing at the desk's north approach, feet on its top row.
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
    /// in front of that desk and behind its occupant. Under the old key it
    /// sorted on its own middle row, so a tall plant between the two painted
    /// over the occupant while standing behind them.
    #[test]
    fn an_aisle_prop_sorts_between_the_desk_and_its_occupant() {
        let pack = pack();
        let desk = crate::layout::Point { x: 0, y: 10 };
        let (_, desk_h) = base_size(&pack, "desk");
        let (_, body_h) = base_size(&pack, "seated");
        let (_, plant_h) = base_size(&pack, "plant");

        let desk_z = sort_row(
            crate::layout::Anchor::TopLeft,
            crate::layout::Point {
                x: desk.x,
                y: desk.y - 1,
            },
            desk_h,
            desk_front_h(),
        );
        let seated_z = sort_row(
            crate::layout::Anchor::TopLeft,
            near_seat(desk),
            body_h,
            CHAIR_BACK_H,
        );
        // A plant whose BASE sits between the desk's front face and the chair.
        let plant_base = desk_z + 1;
        let plant_centre = crate::layout::Point {
            x: 30,
            y: plant_base + plant_h / 2 - plant_h + 1,
        };
        let plant_z = sort_row(crate::layout::Anchor::Center, plant_centre, plant_h, 1);
        assert!(
            desk_z < plant_z && plant_z < seated_z,
            "desk {desk_z} < plant {plant_z} < seated {seated_z}"
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
            "the deleted override hardcoded desk.y; the shared anchor must still \
             land there for a back-turned desk, or deleting it moved someone"
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
    /// seated agent is eight rows higher.
    #[test]
    fn a_label_anchor_sits_above_the_head_and_centred_on_the_sprite() {
        let scale = RenderScale::new(3).expect("nonzero");
        let at = crate::layout::Point { x: 10, y: 20 };
        let anchor = label_anchor(at, 8, scale);
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

    /// The band is derived from the layout's OWN `top_margin` minus its own
    /// constant, never re-guessed — the rows between are floor the agents
    /// walk on, so a band drawn to `top_margin` would paint over walkers.
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

    /// The bundled pack, which ships `desk` (14x8) and `desk@4x` (56x32).
    fn pack() -> Pack {
        crate::embedded_pack::load_sprite_pack(None).expect("the embedded pack loads")
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
            .and_then(|a| a.frames.first())
            .expect("the bundled pack has this piece");
        (f.width(), f.height())
    }

    /// The whole draw list of a REAL office, checked against every pairwise
    /// "must be behind" fact its own geometry states.
    ///
    /// This is what a sort key cannot give you. It is also the guard on the
    /// long-object class: a wall run left unsplit would sort its south end in
    /// front of the room's contents, and the pantry counter — which stands
    /// INSIDE a room, between its north and south walls — is exactly the piece
    /// that catches it.
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
            let (dw, dh) = base_size(&pack, "desk");
            for d in &layout.home_desks {
                order.push((
                    piece_span(
                        crate::layout::Anchor::TopLeft,
                        crate::layout::Point {
                            x: d.x,
                            y: d.y.saturating_sub(1),
                        },
                        dw,
                        dh,
                        desk_front_h(),
                    ),
                    PieceKind::Desk { at: *d, lit: false },
                ));
            }
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

    /// THE property the whole mixed-density contract rests on: a density
    /// variant must change how a piece is DRAWN, never how big it is. The two
    /// branches return different (frame, factor) pairs whose PRODUCT has to
    /// agree, and getting that wrong is silent — the desk still renders, just
    /// with its front face a whole desk below the surface (which is exactly
    /// what shipped before this test existed).
    #[test]
    fn the_drawn_size_is_the_same_whichever_density_the_art_came_from() {
        let pack = pack();
        let (bw, bh) = base_size(&pack, "desk");
        for s in 1..=12u16 {
            let scale = RenderScale::new(s).expect("nonzero");
            let (art, blit_at) = densest_art(&pack, "desk", scale).expect("desk is in the pack");
            assert_eq!(
                (art.width() * blit_at.get(), art.height() * blit_at.get()),
                (scale.to_buffer(bw), scale.to_buffer(bh)),
                "scale {s} drew a different size than the base art implies"
            );
        }
    }

    /// `desk@4x` must WIN at 4x and blit 1:1 (the upscale is what richer art
    /// exists to remove), must HALVE the upscale at 8x rather than being
    /// discarded for not matching exactly, and must LOSE at 3x — 4 does not
    /// divide 3, so blitting it would draw a desk a third too wide.
    #[test]
    fn a_density_variant_is_taken_only_at_the_density_it_was_authored_for() {
        let pack = pack();
        let (bw, bh) = base_size(&pack, "desk");
        let (hi_w, hi_h) = base_size(&pack, "desk@4x");
        assert_eq!((hi_w, hi_h), (bw * 4, bh * 4), "desk@4x is the 4x variant");

        let (art, blit_at) =
            densest_art(&pack, "desk", RenderScale::new(4).expect("nonzero")).expect("desk exists");
        assert_eq!(
            (art.width(), blit_at.get()),
            (hi_w, 1),
            "at its own density the variant art blits 1:1"
        );

        let (art, blit_at) =
            densest_art(&pack, "desk", RenderScale::new(8).expect("nonzero")).expect("desk exists");
        assert_eq!(
            (art.width(), blit_at.get()),
            (hi_w, 2),
            "at 8x the 4x art still halves the upscale"
        );

        let (art, blit_at) =
            densest_art(&pack, "desk", RenderScale::new(3).expect("nonzero")).expect("desk exists");
        assert_eq!(
            (art.width(), blit_at.get()),
            (bw, 3),
            "at 3x the 4x art does not divide and the base block-scales"
        );
    }

    /// The other half of "the asset work lands one piece at a time": every
    /// piece the pack has NOT redrawn must be untouched by the lookup, or
    /// adding one `@Nx` sprite would be a flag day for all of them.
    #[test]
    fn a_piece_with_no_density_variant_renders_exactly_as_it_did_before() {
        let pack = pack();
        assert!(
            pack.animation("plant@4x").is_none(),
            "this test is only meaningful while `plant` has no 4x variant"
        );
        let (bw, _) = base_size(&pack, "plant");
        let scale = RenderScale::new(4).expect("nonzero");
        let (art, blit_at) = densest_art(&pack, "plant", scale).expect("plant is in the pack");
        assert_eq!((art.width(), blit_at.get()), (bw, 4));
    }

    #[test]
    fn a_desk_is_thicker_than_its_top_surface_alone() {
        // Without a front face the office is a floor plan; pin that the split
        // leaves BOTH parts non-empty however DESK_H moves.
        let front_h = (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1);
        let top_h = DESK_H.saturating_sub(front_h).max(1);
        assert!(front_h >= 1 && top_h >= 1, "top {top_h}, front {front_h}");
        assert!(
            front_h < DESK_H,
            "the front face is a fraction, not the desk"
        );
    }
}
