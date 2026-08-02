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

/// How far down the desk sprite the screen's spill lands.
const GLOW_ROW_NUMER: u16 = 5;
/// Denominator of [`GLOW_ROW_NUMER`].
const GLOW_ROW_DENOM: u16 = 8;

/// Paint `frame`'s office into `buf` as an orthographic cutaway.
///
/// `layout` is in LOGICAL units and `buf` in buffer pixels; `scale` converts.
/// The classic painter is untouched — this is its sibling, not its successor.
pub fn render_cutaway(
    frame: &SimFrame,
    layout: &Layout,
    pack: &Pack,
    theme: &Theme,
    scale: RenderScale,
    buf: &mut RgbBuffer,
) {
    paint_floor(layout, theme, scale, buf);
    paint_wall(layout, theme, scale, buf);

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

    for piece in &order {
        match *piece {
            Piece::Desk { at, lit } => paint_desk(at.x, at.y, lit, pack, theme, scale, buf),
            Piece::Character { idx, .. } => paint_character(frame, idx, pack, theme, scale, buf),
        }
    }
}

/// A thing to draw, carrying the depth it sorts on.
enum Piece {
    Desk { at: crate::layout::Point, lit: bool },
    Character { idx: usize, y: u16 },
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
) {
    let Some(c) = frame.characters.get(idx) else {
        return;
    };
    let Some(anim) = pack.animation(c.anim_name) else {
        return;
    };
    let Some(f) = anim.frames.get(c.frame_idx) else {
        return;
    };
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
