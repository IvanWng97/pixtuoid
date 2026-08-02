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

    // ONE ordered draw list, so a character and the desk it sits at resolve
    // against each other by depth instead of by which loop ran first. That
    // ordering IS the occlusion — there is no separate occlusion pass.
    let mut order: Vec<Piece> =
        Vec::with_capacity(layout.home_desks.len() + frame.characters.len());
    order.extend(layout.home_desks.iter().map(|d| Piece::Desk { at: *d }));
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
            Piece::Desk { at } => paint_desk(at.x, at.y, pack, theme, scale, buf),
            Piece::Character { idx, .. } => paint_character(frame, idx, pack, scale, buf),
        }
    }
}

/// A thing to draw, carrying the depth it sorts on.
enum Piece {
    Desk { at: crate::layout::Point },
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
            Piece::Desk { at } => at.y + desk_top_h(),
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

fn paint_character(
    frame: &SimFrame,
    idx: usize,
    pack: &Pack,
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
    blit_frame_scaled(
        &art,
        scale.to_buffer(c.anchor.x),
        scale.to_buffer(c.anchor.y),
        scale.factor(),
        buf,
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
    fn a_character_north_of_the_desk_sorts_behind_it() {
        // The other half: someone walking along the far side is occluded BY the
        // desk, which is what gives the office depth rather than a flat plan.
        let desk = Piece::Desk {
            at: crate::layout::Point { x: 0, y: 10 },
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
