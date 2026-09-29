//! The frosted-glass room partition's look, pixel-free: what the glass does to
//! each cell of a [`WallPiece`](crate::layout::WallPiece)'s box. Both painters
//! walk their own grid over that box and ask it, so the glass is one design
//! drawn at two densities.
//!
//! Glass recolours what already lies behind it, one cell at a time: a
//! translucency, not a gradient, so a cell that was one colour stays one.

use pixtuoid_core::sprite::Rgb;

use crate::layout::WallPiece;
use crate::theme::Theme;

/// Seam-glint spacing along a run, in logical units.
const SEAM_STRIDE: u16 = 16;
/// Mullion (partition post) spacing along a run, in logical units: a darker post
/// this often reads as panelled partitions instead of one unbroken sheet.
/// Offset from [`SEAM_STRIDE`] so the two rhythms interleave.
const MULLION_STRIDE: u16 = 10;

/// What the theme's trim is lifted by, per channel, for the glass's highlight,
/// body and shadow tones.
const HI_LIFT: [u8; 3] = [125, 135, 124];
/// See [`HI_LIFT`].
const MID_LIFT: [u8; 3] = [70, 100, 116];
/// See [`HI_LIFT`].
const LO_LIFT: [u8; 3] = [18, 52, 86];

/// How much of a tone a mullion lays over what is behind it.
const MULLION_OPACITY: f32 = 0.8;
/// How much of the shadow tone the far edge lays over what is behind it.
const FAR_EDGE_OPACITY: f32 = 0.72;

/// How a partition is seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// An E-W run, showing its face.
    Face,
    /// A N-S run, seen edge-on through more glass, so its tones sit denser.
    EdgeOn,
}

/// The opacities that depend on the [`View`].
struct Opacity {
    seam: f32,
    near_edge: f32,
    pane: f32,
}

impl View {
    fn opacity(self) -> Opacity {
        match self {
            View::Face => Opacity {
                seam: 0.55,
                near_edge: 0.82,
                pane: 0.58,
            },
            View::EdgeOn => Opacity {
                seam: 0.6,
                near_edge: 0.85,
                pane: 0.6,
            },
        }
    }
}

/// One partition's glass on one painter's grid.
pub(crate) struct Glass {
    hi: Rgb,
    mid: Rgb,
    lo: Rgb,
    view: View,
    run: u16,
    depth: u16,
    per_unit: u16,
}

impl Glass {
    /// `piece`'s glass on a grid of `per_unit` cells to a logical unit.
    pub(crate) fn of(theme: &Theme, piece: WallPiece, per_unit: u16) -> Self {
        let per_unit = per_unit.max(1);
        let (_, size) = piece.visual();
        let (view, run, depth) = match piece {
            WallPiece::Horizontal { .. } => (View::Face, size.w, size.h),
            WallPiece::Vertical { .. } => (View::EdgeOn, size.h, size.w),
        };
        let trim = theme.office.room_wall_trim_light;
        let lift = |[r, g, b]: [u8; 3]| Rgb {
            r: trim.r.saturating_add(r),
            g: trim.g.saturating_add(g),
            b: trim.b.saturating_add(b),
        };
        Self {
            hi: lift(HI_LIFT),
            mid: lift(MID_LIFT),
            lo: lift(LO_LIFT),
            view,
            run: run.saturating_mul(per_unit),
            depth: depth.saturating_mul(per_unit),
            per_unit,
        }
    }

    /// `under` seen through the cell `dx` across and `dy` down the wall's
    /// [`visual`](WallPiece::visual) box.
    pub(crate) fn over(&self, under: Rgb, dx: u16, dy: u16) -> Rgb {
        let (along, across) = match self.view {
            View::Face => (dx, dy),
            View::EdgeOn => (dy, dx),
        };
        let (tone, opacity) = self.tint(along, across);
        crate::pixel_painter::blend_rgb(under, tone, opacity)
    }

    fn tint(&self, along: u16, across: u16) -> (Rgb, f32) {
        let o = self.view.opacity();
        // Interior posts only: one AT a run's end would double its door frame
        // or corner joint.
        let mullion = along > 0
            && along + 1 < self.run
            && along.is_multiple_of(MULLION_STRIDE * self.per_unit);
        if mullion {
            (self.lo, MULLION_OPACITY)
        } else if along.is_multiple_of(SEAM_STRIDE * self.per_unit) {
            (self.hi, o.seam)
        } else if across == 0 {
            (self.hi, o.near_edge)
        } else if across + 1 == self.depth {
            (self.lo, FAR_EDGE_OPACITY)
        } else {
            (self.mid, o.pane)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEHIND: Rgb = Rgb {
        r: 220,
        g: 40,
        b: 40,
    };

    /// A 40-unit E-W wall.
    const RUN: WallPiece = WallPiece::Horizontal {
        x0: 0,
        x1: 39,
        y_face: 20,
        jamb_west: false,
        jamb_east: false,
    };

    #[test]
    fn glass_cools_what_is_behind_it() {
        let seen = Glass::of(&crate::theme::NORMAL, RUN, 1).over(BEHIND, 3, 5);
        assert!(
            seen.r < BEHIND.r && seen.b > BEHIND.b,
            "frosted glass cools the pixel behind it (red down, blue up): {seen:?}"
        );
    }

    #[test]
    fn a_denser_grid_keeps_the_rhythm_in_logical_units() {
        let (one, four) = (
            Glass::of(&crate::theme::NORMAL, RUN, 1),
            Glass::of(&crate::theme::NORMAL, RUN, 4),
        );
        for x in 0..40 {
            assert_eq!(
                one.over(BEHIND, x, 5),
                four.over(BEHIND, x * 4, 20),
                "cell {x} lands on the same part of the pane at 4x"
            );
        }
        assert_ne!(
            four.over(BEHIND, MULLION_STRIDE * 4, 20),
            four.over(BEHIND, MULLION_STRIDE * 4 + 1, 20),
            "a mullion is one cell wide at any density"
        );
    }
}
