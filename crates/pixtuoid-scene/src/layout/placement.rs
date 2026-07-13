//! Anchor conventions: WHERE a furniture/decor box sits relative to its layout
//! `pos`. Shared by the walkable mask (ground footprint rect) and the renderer
//! (sprite blit origin + y-sort row). Centralising the formulas here keeps the
//! three representations of the same fact — blocked rect, sprite top-left,
//! z-sort row — from drifting. That drift WAS the bug class: the side-table z
//! key hand-rolled `table.y + 1`, the pantry centered-stamp, the vertical-wall
//! top raise the mask and renderer each computed by hand.
//!
//! The anchor is a property of the PLACEMENT ROLE, not the furniture — so the
//! call site passes it explicitly rather than reading it off `FurnitureDef`.
//! `Furniture::Whiteboard` proves why: it is `Center` as pod-aisle decor (the
//! mask stamps it `pos - size/2`) but `TopLeft` as a wall-hung board (stamped at
//! `pos`). One geometry row, two placement conventions — a single per-furniture
//! anchor field could not represent both and would have to be overridden at the
//! wall site, recreating the very drift this module removes.

use super::{Point, Size};

/// How a `(w, h)` box is positioned relative to its `pos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// `pos` is the box CENTER. Top-left = `pos - size/2`; the y-sort row is the
    /// box's south (front) edge. Most furniture: plants, vending, sofas, tables,
    /// lamp, phone booth, the pantry counter, pod-aisle whiteboard/TV.
    Center,
    /// `pos` is the box's NW corner. Top-left = `pos`; y-sort row = `pos.y+h-1`.
    /// Wall-hung decor (bookshelf, bulletin board, exit sign, meeting screen,
    /// wall-mounted whiteboard) and the home desk.
    TopLeft,
}

/// Top-left corner of a `(w, h)` box anchored at `pos`. Used for BOTH the mask
/// footprint rect (pass the footprint size) and the sprite blit origin (pass the
/// sprite/visual size) so the blocked ground and the painted sprite can't
/// diverge for a given anchor.
pub fn anchored_top_left(anchor: Anchor, pos: Point, w: u16, h: u16) -> Point {
    match anchor {
        Anchor::Center => Point {
            x: pos.x.saturating_sub(w / 2),
            y: pos.y.saturating_sub(h / 2),
        },
        Anchor::TopLeft => pos,
    }
}

/// The y-sort key for a sprite of height `h` anchored at `pos`: its south
/// (front) base ROW. Derived from [`anchored_top_left`] so it can NEVER drift
/// from where the sprite actually blits (`origin.y + h - 1`). For `Center` this
/// equals the legacy `pos.y + center_pin_south_offset(h)` (i.e. `(h-1)/2`).
/// Half-open AABB intersection of two `(top_left, size)` rects. Lives here —
/// the geometry-primitive module — so the placement sweep's overlap invariant
/// and production collision checks (the whiteboard-vs-desk drop in compute.rs)
/// read ONE formula instead of each hand-rolling the four comparisons (the
/// exact drift class this module exists to remove). Zero-sized rects overlap
/// nothing.
pub(super) fn rects_overlap(a: (Point, Size), b: (Point, Size)) -> bool {
    let (at, asz) = a;
    let (bt, bsz) = b;
    asz.w > 0
        && asz.h > 0
        && bsz.w > 0
        && bsz.h > 0
        && at.x < bt.x + bsz.w
        && bt.x < at.x + asz.w
        && at.y < bt.y + bsz.h
        && bt.y < at.y + asz.h
}

/// Does `probe` overlap `obstacle` grown by `clearance` px on every side? The
/// shared "keep N px of air around this box" test: the scatter-plant singleton
/// scan and `first_blocking_waypoint` (compute.rs) each spelled the same
/// inflate-then-[`rects_overlap`] by hand — a verbatim second copy whose pad
/// term could drift from its twin. Both pass a Center-anchored `obstacle`
/// top-left (via [`anchored_top_left`]) so the grown rect matches the sprite.
pub(super) fn overlaps_within_clearance(
    probe: (Point, Size),
    obstacle: (Point, Size),
    clearance: u16,
) -> bool {
    let (tl, sz) = obstacle;
    let grown_tl = Point {
        x: tl.x.saturating_sub(clearance),
        y: tl.y.saturating_sub(clearance),
    };
    let grown = Size {
        w: sz.w + 2 * clearance,
        h: sz.h + 2 * clearance,
    };
    rects_overlap(probe, (grown_tl, grown))
}

pub fn z_sort_row(anchor: Anchor, pos: Point, h: u16) -> u16 {
    anchored_top_left(anchor, pos, 0, h)
        .y
        .saturating_add(h.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The invariant that drifted: the z-sort row IS the south row of the box
    // the sprite blits into, for every anchor — so a sprite can never sort in
    // front of (or behind) its own base.
    #[test]
    fn z_sort_row_is_the_sprite_south_row_for_every_anchor() {
        let pos = Point { x: 50, y: 40 };
        for &a in &[Anchor::Center, Anchor::TopLeft] {
            for h in 1u16..24 {
                let tl = anchored_top_left(a, pos, 8, h);
                assert_eq!(
                    z_sort_row(a, pos, h),
                    tl.y + h - 1,
                    "{a:?} h={h}: z-sort row must equal the box south row"
                );
            }
        }
    }

    // Center must reproduce the pre-refactor `center_pin_south_offset(h)`
    // exactly, so migrating the renderer's z-keys is byte-identical.
    #[test]
    fn center_matches_legacy_center_pin_offset() {
        let pos = Point { x: 30, y: 25 };
        for h in 1u16..24 {
            assert_eq!(
                z_sort_row(Anchor::Center, pos, h),
                pos.y + (h - 1) / 2,
                "h={h}"
            );
        }
    }

    #[test]
    fn topleft_origin_is_pos() {
        let pos = Point { x: 7, y: 9 };
        assert_eq!(anchored_top_left(Anchor::TopLeft, pos, 14, 11), pos);
        assert_eq!(z_sort_row(Anchor::TopLeft, pos, 11), pos.y + 10);
    }

    // Center origin is pos - size/2 (what every centered mask stamp + sprite
    // blit does today).
    #[test]
    fn center_origin_is_pos_minus_half() {
        let pos = Point { x: 40, y: 30 };
        assert_eq!(
            anchored_top_left(Anchor::Center, pos, 8, 6),
            Point { x: 36, y: 27 }
        );
    }
}
