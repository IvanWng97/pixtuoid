//! The cutaway profile's paint pass — the second reader of `SimFrame`.
//!
//! Deliberately partial. It draws the floor, the desks and the cast, which is
//! the smallest set that answers the question the whole profile exists to
//! answer: does an orthographic cutaway of THIS office, at THIS density, read
//! better than the classic top-down? Walls, rooms, plants and effects are not
//! here yet because none of them changes that answer.
//!
//! It reads `SimFrame` and never advances anything: the sim already ran, and a
//! second renderer that could move the world would make the two profiles
//! disagree about the office — the invariant the brief spends its §9 on.

use pixtuoid_core::sprite::blit::blit_frame_scaled;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::RgbBuffer;

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
pub fn render_cutaway(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) -> Vec<CutawayLabel> {
    paint_floor(layout, theme, scale, buf);
    paint_wall(layout, theme, scale, buf);
    paint_rooms(layout, pack, theme, scale, buf);

    // ONE ordered draw list, so a character and the desk it sits at resolve
    // against each other by depth instead of by which loop ran first. That
    // ordering IS the occlusion — there is no separate occlusion pass.
    let mut order: Vec<Piece> =
        Vec::with_capacity(layout.home_desks.len() + frame.characters.len());
    // A desk's screen is lit iff the sim says someone is seated at it — the
    // observation is already in the frame, so the profile never re-derives it.
    order.extend(layout.home_desks.iter().enumerate().map(|(i, d)| {
        Piece::Desk {
            at: *d,
            lit: frame
                .seated_agents
                .get(&pixtuoid_core::state::FloorLocalDeskIndex(i))
                .copied()
                .unwrap_or(false),
        }
    }));
    order.extend(layout.plants.iter().map(|pl| Piece::Plant {
        at: pl.pos,
        sprite: pl.kind.sprite_name(),
    }));
    // Standalone waypoint furniture. Seats (MeetingSofa/MeetingChair/Island)
    // are deliberately absent: they are slots ON a body that is painted once
    // from the room's trio, so one sprite per seat would triple-paint it.
    order.extend(layout.waypoints.iter().filter_map(|wp| {
        waypoint_sprite(wp.kind).map(|sprite| Piece::Plant { at: wp.pos, sprite })
    }));
    // Aisle decor + the lounge couch — both already placed and already drawn by
    // the pack, so both are pure reuse like the plants.
    order.extend(layout.pod_decor.iter().map(|d| Piece::Plant {
        at: d.pos,
        sprite: d.kind.sprite_name(),
    }));
    order.extend(layout.couch_sprite_center().map(|at| Piece::Plant {
        at,
        sprite: "back_couch",
    }));
    // Wall decor hangs on the north band, so it is NOT floor-sorted: it paints
    // with the wall, before anything standing on the floor can occlude it.
    for item in &layout.wall_decor {
        paint_prop(item.pos, item.kind.sprite_name(), pack, theme, scale, buf);
    }
    // The corridor appliances classic draws procedurally: they have no sprite
    // to reuse, so the cutaway gives them its own solid geometry.
    order.extend(
        layout
            .waypoints
            .iter()
            .filter(|wp| {
                matches!(
                    wp.kind,
                    crate::layout::WaypointKind::VendingMachine
                        | crate::layout::WaypointKind::Printer
                )
            })
            .map(|wp| Piece::Appliance {
                at: wp.pos,
                kind: wp.kind,
            }),
    );
    // The meeting trio: two sofa bodies plus the table between them.
    order.extend(
        layout
            .meeting_rooms
            .iter()
            .filter_map(|r| r.trio.as_ref())
            .flat_map(|t| {
                [
                    Piece::Plant {
                        at: t.sofas[0],
                        sprite: "meeting_sofa",
                    },
                    Piece::Plant {
                        at: t.sofas[1],
                        sprite: "meeting_sofa",
                    },
                ]
            }),
    );
    order.extend(
        layout
            .meeting_rooms
            .iter()
            .filter_map(|r| r.trio.as_ref())
            .map(|t| Piece::Table { at: t.table }),
    );
    order.extend(
        frame
            .characters
            .iter()
            .enumerate()
            .map(|(i, c)| Piece::Character {
                idx: i,
                y: c.anchor_y,
            }),
    );
    order.sort_by_key(Piece::depth);

    let mut labels = Vec::with_capacity(frame.characters.len());
    for piece in &order {
        match *piece {
            Piece::Desk { at, lit } => paint_desk(at.x, at.y, lit, pack, theme, scale, buf),
            Piece::Character { idx, .. } => {
                if let Some(l) = paint_character(frame, idx, pack, theme, scale, buf) {
                    labels.push(l);
                }
            }
            Piece::Plant { at, sprite } => paint_prop(at, sprite, pack, theme, scale, buf),
            Piece::Table { at } => paint_table(at, theme, scale, buf),
            Piece::Appliance { at, kind } => paint_appliance(at, kind, theme, scale, buf),
        }
    }
    labels
}

/// A thing to draw, carrying the depth it sorts on.
enum Piece {
    Desk {
        at: crate::layout::Point,
        lit: bool,
    },
    Plant {
        at: crate::layout::Point,
        sprite: &'static str,
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
        y: u16,
    },
}

impl Piece {
    /// The row each piece sorts on — larger paints later, i.e. in front.
    ///
    /// A desk sorts on its TOP SURFACE's south edge, and that is a DELIBERATE
    /// divergence from the classic painter, which sorts it on the desk's visual
    /// base (`desk.y + visual.h`) so the monitor overhangs and hides the seated
    /// occupant's lower body. That reading is right for a pure top-down view and
    /// wrong for a cutaway: the ratified reference draws the occupant's head
    /// OVER the desk surface, because in a cutaway they sit at the near side
    /// facing away. Sorting on the surface plane is what produces that.
    ///
    /// `DESK_H` is the layout FOOTPRINT, not the sprite — hence `desk_top_h`
    /// rather than the constant, so the split can't drift from what is painted.
    fn depth(&self) -> u16 {
        match self {
            Piece::Desk { at, .. } => at.y + desk_top_h(),
            Piece::Character { y, .. } => *y,
            // A plant's ground is its own base row, the layout's `pos` convention.
            Piece::Plant { at, .. } => at.y,
            Piece::Table { at } => at.y,
            Piece::Appliance { at, .. } => at.y,
        }
    }
}

/// Rows of a desk that are its top surface; the rest is the front face.
///
/// The ONE definition both the paint and the depth rule read, so a desk can
/// never sort on a plane it did not draw.
fn desk_top_h() -> u16 {
    DESK_H.saturating_sub(desk_front_h()).max(1)
}

/// Rows of a desk that are its front face — the thickness.
fn desk_front_h() -> u16 {
    (DESK_H * DESK_FRONT_NUMER / DESK_FRONT_DENOM).max(1)
}

/// Rows of the wall band that are glass rather than wall.
///
/// The windows are the cutaway's only light SOURCE on screen, so the band has to
/// read as glass and not as a stripe — it is what makes the north-to-south floor
/// falloff legible as light instead of as a gradient someone chose.
const WINDOW_INSET_NUMER: u16 = 1;
/// Denominator of [`WINDOW_INSET_NUMER`].
const WINDOW_INSET_DENOM: u16 = 4;

fn paint_wall(layout: &Layout, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    // The layout's own derivation, not a re-guess: the wall band ends
    // `WALL_BAND_TO_TOP_MARGIN` above `top_margin`, and the rows between are
    // floor the agents walk on.
    let band_h = layout
        .top_margin
        .saturating_sub(crate::layout::WALL_BAND_TO_TOP_MARGIN);
    if band_h == 0 {
        return;
    }
    let s = scale.get();
    let w = scale.to_buffer(layout.buf_w);
    let wall = Ramp::from_base(theme.surface.wall, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    slab(buf, 0, 0, w, band_h * s, &wall);

    // One glass run inset inside the band, with a lit sill under it — the sill
    // is what sells the light as coming THROUGH rather than being painted on.
    let inset = (band_h * WINDOW_INSET_NUMER / WINDOW_INSET_DENOM).max(1);
    let glass_h = band_h.saturating_sub(inset * 2);
    if glass_h > 0 {
        let glass = Ramp::from_base(theme.lighting.night_sky_a, RAMP_TINT_PCT, RAMP_SHADE_PCT);
        slab(buf, 0, inset * s, w, glass_h * s, &glass);
        paint_skyline(layout, theme, scale, inset, glass_h, buf);
        fill(buf, 0, (inset + glass_h) * s, w, s, theme.surface.wall_trim);
    }
    // The wall's own contact line with the floor.
    fill(
        buf,
        0,
        band_h * s,
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
        fill(buf, scale.to_buffer(x), top * s, bw * s, bh * s, tone);
        // Lit windows — the thing that says "night", not just "dark".
        let mut wy = top + 1;
        while wy + 1 < sill {
            let mut wx = x + 1;
            while wx + 1 < x + bw {
                if mix(u32::from(wx) ^ (u32::from(wy) << 8)) % 5 == 0 {
                    fill(
                        buf,
                        scale.to_buffer(wx),
                        wy * s,
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

/// The enclosed rooms — meeting rooms and the pantry — as glass boxes.
///
/// Their bounds already exist on the layout, and without them the whole west
/// half of the office reads as empty floor: the classic painter fills it with
/// walls, furniture and rugs this profile has not drawn yet, so the rooms are
/// the cheapest thing that restores the office's SHAPE.
///
/// Glass rather than solid: a cutaway shows what is inside a room, which is the
/// whole reason the concept is a cutaway and not a floor plan.
fn paint_rooms(
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let s = scale.get();
    let glass = Ramp::from_base(
        theme.office.room_wall_trim_light,
        RAMP_TINT_PCT,
        RAMP_SHADE_PCT,
    );
    let bounds: Vec<_> = layout
        .meeting_rooms
        .iter()
        .map(|r| r.bounds)
        .chain(layout.pantry.iter().map(|p| p.bounds))
        .collect();
    // The pantry's counter is a fixture the layout already sized — paint it so
    // the room reads as a pantry rather than an empty glass box.
    if let Some(pantry) = &layout.pantry {
        let sprite = if pantry.counter_size.w >= crate::layout::PANTRY_COUNTER_LARGE_W {
            "pantry"
        } else {
            "pantry_small"
        };
        if let Some(art) = pack.animation(sprite).and_then(|a| a.frames.first()) {
            let x = pantry.bounds.x + (pantry.bounds.width.saturating_sub(art.width())) / 2;
            let y = pantry.bounds.y + ROOM_WALL_PX + 1;
            blit_frame_scaled(
                art,
                scale.to_buffer(x),
                scale.to_buffer(y),
                scale.factor(),
                buf,
            );
        }
    }
    for b in bounds {
        let (x, y) = (scale.to_buffer(b.x), scale.to_buffer(b.y));
        let (w, h) = (b.width * s, b.height * s);
        // Walls only — the interior stays floor, so the room reads as a room
        // you can see into rather than a filled block.
        slab(buf, x, y, w, ROOM_WALL_PX * s, &glass);
        slab(
            buf,
            x,
            y + h - ROOM_WALL_PX * s,
            w,
            ROOM_WALL_PX * s,
            &glass,
        );
        fill(buf, x, y, ROOM_WALL_PX * s, h, glass.base);
        fill(
            buf,
            x + w - ROOM_WALL_PX * s,
            y,
            ROOM_WALL_PX * s,
            h,
            glass.base,
        );
    }
}

fn paint_floor(layout: &Layout, theme: &Theme, scale: RenderScale, buf: &mut RgbBuffer) {
    let lit = theme.surface.carpet_light;
    let base = theme.surface.carpet_base;
    let dark = theme.surface.carpet_dark;

    let h = scale.to_buffer(layout.buf_h);
    let w = scale.to_buffer(layout.buf_w);
    fill(buf, 0, 0, w, h, base);

    // North third lit, then dither down to base, then a final fall to dark at
    // the south edge — the falloff that says where the light comes from.
    let lit_h = h * FLOOR_LIT_NUMER / FLOOR_LIT_DENOM;
    fill(buf, 0, 0, w, lit_h / 2, lit);
    dither_band(buf, lit_h / 2, lit_h, base, lit);
    dither_band(buf, h.saturating_sub(lit_h / 2), h, dark, base);
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
    // The SAME anchor the classic painter uses, including its 1px raise for the
    // monitor bezel — read from there rather than re-derived, so the two
    // profiles cannot place the same desk differently.
    let top_y = scale.to_buffer(ly.saturating_sub(1));

    let art = pack.animation("desk").and_then(|a| a.frames.first());
    let Some(art) = art else { return };
    blit_frame_scaled(art, x, top_y, scale.factor(), buf);

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
    if lit {
        let glow = Ramp::from_base(
            theme.effects.monitor_frame_lit,
            RAMP_TINT_PCT,
            RAMP_SHADE_PCT,
        );
        fill(
            buf,
            x + art.width() * s / 4,
            top_y + art.height() * s * GLOW_ROW_NUMER / GLOW_ROW_DENOM,
            art.width() * s / 2,
            s,
            glow.lit,
        );
    }

    let ramp = Ramp::from_base(material, RAMP_TINT_PCT, RAMP_SHADE_PCT);
    let base_y = top_y + art.height() * s;
    let w = art.width() * s;
    slab(buf, x, base_y, w, desk_front_h() * s, &ramp);

    // Contact occlusion hugs the front face; a wide pool reads as a stain.
    fill(
        buf,
        x,
        base_y + desk_front_h() * s,
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

/// Where the cutaway seats an occupant, given the desk the sim says they sit at.
///
/// The classic painter RAISES a seated sprite above its desk so the monitor
/// overhangs and hides the lower body — right for a pure top-down view. A
/// cutaway seats them at the NEAR side instead, head over the surface, which is
/// what the ratified reference shows. Anchoring at the desk's own row does that:
/// the head lands on the surface band and the torso falls in front of it.
fn cutaway_seat_anchor(
    desk: crate::layout::Point,
    classic: crate::layout::Point,
) -> crate::layout::Point {
    crate::layout::Point {
        // x keeps the sim's centring (it already accounts for sprite width, and
        // re-deriving it here would be a second copy that could drift).
        x: classic.x,
        y: desk.y,
    }
}

fn paint_character(
    frame: &SimFrame,
    idx: usize,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) -> Option<CutawayLabel> {
    let c = frame.characters.get(idx)?;
    let anim = pack.animation(c.anim_name)?;
    let f = anim.frames.get(c.frame_idx)?;
    let art = if c.flip_x {
        f.mirror_vertical()
    } else {
        f.clone()
    };
    // Re-project a desk-seated pose; everyone else keeps the sim's anchor.
    let at = match c.seat_desk {
        Some(desk) => cutaway_seat_anchor(desk, c.anchor),
        None => c.anchor,
    };
    // Ground contact BEFORE the body, so the sprite sits on its own shadow
    // rather than the shadow being stamped over their feet.
    contact_shadow(at, art.width(), art.height(), theme, scale, buf);
    blit_frame_scaled(
        &art,
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
        paint_chair(at, art.width(), art.height(), theme, scale, buf);
    }
    Some(CutawayLabel {
        agent_idx: c.agent_idx,
        anchor_px: crate::layout::Point {
            x: scale.to_buffer(at.x + art.width() / 2),
            y: scale
                .to_buffer(at.y)
                .saturating_sub(LABEL_GAP_PX * scale.get()),
        },
    })
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
    let s = scale.get();
    let (w, h) = (TABLE_W, TABLE_H);
    let x = at.x.saturating_sub(w / 2);
    let y = at.y.saturating_sub(h / 2);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        w * s,
        h * s,
        &ramp,
    );
    // The same front face every solid in this profile gets.
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y + h),
        w * s,
        desk_front_h() * s,
        &Ramp::from_base(theme.furniture.wood_trim, RAMP_TINT_PCT, RAMP_SHADE_PCT),
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
    let s = scale.get();
    let (w, h) = (def.visual.w, def.visual.h);
    let x = at.x.saturating_sub(w / 2);
    let y = at.y.saturating_sub(h / 2);
    slab(
        buf,
        scale.to_buffer(x),
        scale.to_buffer(y),
        w * s,
        h * s,
        &Ramp::from_base(body, RAMP_TINT_PCT, RAMP_SHADE_PCT),
    );
    // The lit face — a vending display or a printer's glass — is what stops
    // these reading as anonymous blocks in a dark corridor.
    if w > 2 && h > 2 {
        fill(
            buf,
            scale.to_buffer(x + 1),
            scale.to_buffer(y + 1),
            (w - 2) * s,
            (h / 2).max(1) * s,
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
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    let Some(art) = pack.animation(sprite).and_then(|a| a.frames.first()) else {
        return;
    };
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
/// Two rows, not an ellipse: a wide soft pool reads as a stain on a dark carpet
/// (the visual mock proved that twice), while a band the width of the sprite
/// reads as weight.
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
        sprite_w * s,
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
    let s = scale.get();
    // Just below the body, one pixel proud on each side — a seat back peeking
    // out, not a panel over the occupant.
    slab(
        buf,
        scale.to_buffer(at.x.saturating_sub(1)),
        scale.to_buffer(at.y + sprite_h),
        (sprite_w + 2) * s,
        CHAIR_BACK_H * s,
        &ramp,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seated_occupant_sorts_in_front_of_the_desk_surface() {
        // THE cutaway occlusion decision, and the one place it diverges from
        // classic. Classic sorts a desk on its visual base so the monitor hides
        // the occupant; the reference draws the head OVER the surface, so the
        // cutaway sorts on the surface plane instead. The classic sim seats an
        // occupant at desk.y + 4 (its documented seated z-key), so that value is
        // the real input this rule has to beat.
        let desk = Piece::Desk {
            at: crate::layout::Point { x: 0, y: 10 },
            lit: false,
        };
        let seated = Piece::Character { idx: 0, y: 10 + 4 };
        assert_eq!(desk.depth(), 10 + desk_top_h());
        assert!(
            seated.depth() > desk.depth(),
            "a seated occupant must paint over the surface (desk {}, seated {})",
            desk.depth(),
            seated.depth()
        );
    }

    #[test]
    fn the_cutaway_seats_an_occupant_lower_than_the_classic_painter_does() {
        // The visible difference between the two profiles for the same agent.
        // Classic raises the sprite above the desk (monitor overhangs, lower
        // body hidden); the cutaway drops it onto the surface so the head reads
        // over the desk, which is what the reference shows.
        let desk = crate::layout::Point { x: 40, y: 30 };
        let classic = crate::layout::Point { x: 41, y: 30 - 8 };
        let cut = cutaway_seat_anchor(desk, classic);
        assert_eq!(cut.x, classic.x, "x centring comes from the sim, unchanged");
        assert!(
            cut.y > classic.y,
            "the cutaway must seat LOWER (classic {}, cutaway {})",
            classic.y,
            cut.y
        );
        assert_eq!(cut.y, desk.y, "the head lands on the desk's own row");
    }

    #[test]
    fn a_label_anchor_sits_above_the_head_and_centred_on_the_sprite() {
        // The badge has to follow the CUTAWAY's body, not the classic one:
        // `overlay::build_overlay` anchors off the classic projection, which
        // for a seated agent is eight rows higher. Pin the two properties a
        // painter depends on — centred, and clear of the head.
        let scale = RenderScale::new(3).expect("nonzero");
        let at = crate::layout::Point { x: 10, y: 20 };
        let (w, gap) = (8u16, LABEL_GAP_PX);
        let anchor = crate::layout::Point {
            x: scale.to_buffer(at.x + w / 2),
            y: scale.to_buffer(at.y).saturating_sub(gap * scale.get()),
        };
        assert_eq!(anchor.x, scale.to_buffer(at.x) + scale.to_buffer(w / 2));
        assert!(
            anchor.y < scale.to_buffer(at.y),
            "the badge must clear the head, not overlap it"
        );
    }

    #[test]
    fn the_wall_band_stops_where_the_layout_says_the_floor_begins() {
        // The band is derived from the layout's OWN `top_margin` minus its own
        // constant, never re-guessed — the rows between are floor the agents
        // walk on, so a band drawn to `top_margin` would paint over walkers.
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

    #[test]
    fn a_character_north_of_the_desk_sorts_behind_it() {
        // The other half: someone walking along the far side is occluded BY the
        // desk, which is what gives the office depth rather than a flat plan.
        let desk = Piece::Desk {
            at: crate::layout::Point { x: 0, y: 10 },
            lit: false,
        };
        let behind = Piece::Character { idx: 0, y: 9 };
        assert!(behind.depth() < desk.depth());
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
