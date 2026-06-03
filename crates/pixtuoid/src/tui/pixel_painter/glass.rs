//! Frosted-glass room-divider partitions (E-W horizontal + N-S vertical walls).
//! Extracted from mod.rs; the rendering WHY lives in tui/CLAUDE.md
//! ("How do the room dividers render (frosted-glass partitions)?").

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::palette::blend_over;

// Room-divider frosted-glass partitions. The E-W (horizontal) wall shows its
// face — 6 px tall, kept in sync with `mask.rs` WALL_THICK_H — while the N-S
// (vertical) wall is seen edge-on at 3 px (wider than its 1 px footprint). The
// 2:1 ratio sells the top-down fake-3D. Each strip is a cool gradient (bright
// specular edge → tinted body → soft slate edge, all alpha-composited over
// what's behind so the room glows through) with a brighter seam every
// `GLASS_SEAM_STRIDE` px. The horizontal wall paints in the y-sorted drawable
// pass (so it composites over — frostily occluding — a walker standing behind
// it); the vertical paints in the background.
pub(super) const WALL_THICK_V_PX: u16 = 3; // visual; footprint is 1 px (mask.rs)
                                           // Derived from the core mask const so the visible glass face and the blocked
                                           // ground footprint share a single source of truth (can't drift apart).
pub(super) const WALL_THICK_H_PX: u16 = pixtuoid_core::layout::WALL_THICK_H;
const GLASS_SEAM_STRIDE: u16 = 16;
// The horizontal wall's frosted glass rises this many px NORTH of its walkable
// footprint — a "back cap" giving the wall height. Because the strip is
// y-sorted at its south (front) base, a character standing just north of the
// wall has their feet/legs composited behind this translucent cap (occluded
// behind the glass). The cap is over floor (visual only), not the mask.
//
// Derived from WALL_THICK_H_PX (the E-W wall face height) so the cap reaches
// into the legs of a walker at the northmost walkable row (footprint top `W`
// minus OBSTACLE_PAD+1 = `W-3`): the 12px sprite spans `W-15..W-3`, the cap
// covers `W-6..W-1`, so the bottom ~4px (feet + lower legs) read behind the
// pane. At the old value of 3 only the single feet row was grazed. Derived (not
// a bare 6) so retuning the wall face thickness moves the cap with it.
const GLASS_CAP_PX: u16 = WALL_THICK_H_PX;

fn glass_tones(theme: &crate::tui::theme::Theme) -> (Rgb, Rgb, Rgb) {
    let tl = theme.office.room_wall_trim_light;
    (
        Rgb {
            r: tl.r.saturating_add(125),
            g: tl.g.saturating_add(135),
            b: tl.b.saturating_add(124),
        },
        Rgb {
            r: tl.r.saturating_add(70),
            g: tl.g.saturating_add(100),
            b: tl.b.saturating_add(116),
        },
        Rgb {
            r: tl.r.saturating_add(18),
            g: tl.g.saturating_add(52),
            b: tl.b.saturating_add(86),
        },
    )
}

/// Stitch a vertical (N-S) wall segment's `[y_top, y_bot]` to its joints — the
/// terminal-agnostic layout emits raw geometry; the render thicknesses/offsets
/// that open the gaps live here:
///   • Top: a segment starting at `top_margin` abuts the north wall band, which
///     ends 4 px higher at `top_wall_h` — raise it so no floor shows between
///     window and wall. A segment sitting just below a horizontal wall (the
///     dual-meeting layout offsets its lower segment ~6 px to clear the cross
///     wall — see `compute_room_walls`) is bridged up to meet it.
///   • Bottom: where the vertical meets a horizontal wall, extend it down by
///     the horizontal's thickness to fill the inside corner (else its right
///     columns leave an L-notch beside the horizontal run).
pub(super) fn stitch_vertical_wall(
    start_y: u16,
    end_y: u16,
    top_margin: u16,
    top_wall_h: u16,
    h_rows: &[u16],
) -> (u16, u16) {
    let y_top = if start_y == top_margin {
        top_wall_h
    } else if let Some(&hr) = h_rows
        .iter()
        .find(|&&hr| hr < start_y && start_y - hr <= WALL_THICK_H_PX + 2)
    {
        hr
    } else {
        start_y
    };
    let y_bot = if h_rows.contains(&end_y) {
        end_y + (WALL_THICK_H_PX - 1)
    } else {
        end_y
    };
    (y_top, y_bot)
}

/// Paint a horizontal (E-W) frosted-glass wall strip: lit top edge → body →
/// soft bottom edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_h(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x0: u16,
    x1: u16,
    y_top: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    // The strip spans the back cap (rising north of the footprint) + the
    // 6 px face. Row 0 = lit far/top edge (north), last row = soft front base.
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    let rows = GLASS_CAP_PX + WALL_THICK_H_PX;
    for x in x0..=x1.min(bw.saturating_sub(1)) {
        let seam = (x - x0) % GLASS_SEAM_STRIDE == 0;
        for i in 0..rows {
            let y = cap_top + i;
            if y >= bh {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.55)
            } else if i == 0 {
                (hi, 0.82)
            } else if i == rows - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.58)
            };
            let color = blend_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Paint a vertical (N-S) frosted-glass wall strip: lit left edge → body →
/// soft right edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_v(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x_left: u16,
    y_top: u16,
    y_bot: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    for y in y_top..=y_bot.min(bh.saturating_sub(1)) {
        let seam = (y - y_top) % GLASS_SEAM_STRIDE == 0;
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x >= bw {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.6)
            } else if dx == 0 {
                (hi, 0.85)
            } else if dx == WALL_THICK_V_PX - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.6)
            };
            let color = blend_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}
