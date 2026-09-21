//! Lighting effects — ceiling pools, lamp halos, shadows, corridor runner
//! texture, the neon sign (panel, per-frame look, halo), and wall clock.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::pixel_painter::epoch_ms;
use crate::pixel_painter::palette::{blend_rgb, BLACK, WHITE};
use crate::theme::Theme;

/// An axis-aligned ellipse for the radial floor pools (light + shadow).
#[derive(Clone, Copy)]
pub(in crate::pixel_painter) struct Ellipse {
    pub cx: u16,
    pub cy: u16,
    pub half_w: u16,
    pub half_h: u16,
}

/// Float ellipse geometry for [`paint_radial_falloff`] — all `f32`, so a caller
/// can centre on `(w-1)/2` (half-cell correct) as well as an integer cell.
pub(in crate::pixel_painter) struct RadialFalloff {
    pub min_x: u16,
    pub max_x: u16,
    pub min_y: u16,
    pub max_y: u16,
    pub cx: f32,
    pub cy: f32,
    pub rx_norm: f32,
    pub ry_norm: f32,
}

/// The composite the pools, the lamp halos and the neon glow share: blend `color`
/// over the caller-clipped `xs` × `ys` at each pixel's `t(x, y)`; `None` leaves
/// the pixel alone. A light owns its falloff SHAPE and its clip, never the blend.
fn blend_falloff(
    buf: &mut RgbBuffer,
    xs: std::ops::Range<u16>,
    ys: std::ops::Range<u16>,
    color: Rgb,
    t: impl Fn(u16, u16) -> Option<f32>,
) {
    for y in ys {
        for x in xs.clone() {
            if let Some(t) = t(x, y) {
                let cur = buf.get(x, y);
                buf.put(x, y, blend_rgb(cur, color, t));
            }
        }
    }
}

/// Blend `color` over the region with a quadratic radial falloff from the centre
/// (full `strength`) to the ellipse edge (`r² > 1` skipped), so it reads as a
/// soft round patch rather than a stamped oval.
pub(in crate::pixel_painter) fn paint_radial_falloff(
    buf: &mut RgbBuffer,
    g: RadialFalloff,
    strength: f32,
    color: Rgb,
) {
    blend_falloff(buf, g.min_x..g.max_x, g.min_y..g.max_y, color, |x, y| {
        let nx = (x as f32 - g.cx) / g.rx_norm;
        let ny = (y as f32 - g.cy) / g.ry_norm;
        let r2 = nx * nx + ny * ny;
        (r2 <= 1.0).then_some((1.0 - r2) * strength)
    });
}

/// Blend `color` over an integer-centred ellipse — the ceiling pool + shadow.
fn paint_ellipse_blend(buf: &mut RgbBuffer, e: Ellipse, strength: f32, color: Rgb) {
    if e.half_w == 0 || e.half_h == 0 || strength <= 0.0 {
        return;
    }
    let min_x = e.cx.saturating_sub(e.half_w);
    let max_x = (e.cx + e.half_w).min(buf.width());
    let min_y = e.cy.saturating_sub(e.half_h);
    let max_y = (e.cy + e.half_h).min(buf.height());
    paint_radial_falloff(
        buf,
        RadialFalloff {
            min_x,
            max_x,
            min_y,
            max_y,
            cx: e.cx as f32,
            cy: e.cy as f32,
            rx_norm: e.half_w as f32,
            ry_norm: e.half_h as f32,
        },
        strength,
        color,
    );
}

/// Elliptical "ceiling fluorescent" pool of pale warm light on the floor.
pub(in crate::pixel_painter) fn paint_ceiling_pool(
    buf: &mut RgbBuffer,
    ellipse: Ellipse,
    strength: f32,
    theme: &Theme,
) {
    paint_ellipse_blend(buf, ellipse, strength, theme.lighting.ceiling_pool);
}

/// Warm radial halo around the floor lamp — only visible at night.
pub(in crate::pixel_painter) fn paint_floor_lamp_halo(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    strength: f32,
    theme: &Theme,
) {
    /// A room-corner fixture, so much wider than a desk lamp's pool.
    const RADIUS: u16 = 11;
    paint_warm_halo(
        buf,
        cx,
        cy,
        RADIUS,
        strength,
        theme.lighting.floor_lamp_halo,
    );
}

/// Shared so the floor lamp and the desk lamps cannot drift to different falloffs.
pub(in crate::pixel_painter) fn paint_warm_halo(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    radius: u16,
    strength: f32,
    warm: Rgb,
) {
    if strength <= 0.0 {
        return;
    }
    let xs = cx.saturating_sub(radius)..(cx + radius).min(buf.width());
    let ys = cy.saturating_sub(radius)..(cy + radius).min(buf.height());
    let r2max = (radius as f32) * (radius as f32);
    blend_falloff(buf, xs, ys, warm, |x, y| {
        let dx = x as f32 - cx as f32;
        let dy = y as f32 - cy as f32;
        let r2 = dx * dx + dy * dy;
        (r2 <= r2max).then(|| (1.0 - (r2 / r2max).sqrt()) * strength)
    });
}

/// The neon sign's colors for one frame: a bright TUBE, a colored HALO that
/// spills onto the wall and whatever hangs there, and a faintly tinted interior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::pixel_painter) struct NeonLook {
    pub tube: Rgb,
    pub interior: Rgb,
    pub halo: Rgb,
    /// Blend strength AT the tube; [`paint_neon_glow`] falls it off to the radius.
    pub halo_strength: f32,
}

/// How far the halo reaches past the panel (px).
pub(in crate::pixel_painter) const NEON_HALO_RADIUS: u16 = 7;
/// A lit tube is its hue pushed this far toward white — the core of a real neon
/// reads near-white, the COLOR lives in the halo.
const NEON_TUBE_WHITEN: f32 = 0.38;
/// How much of the hue the dark interior picks up at full power.
const NEON_INTERIOR_TINT: f32 = 0.07;
/// Halo strength at the tube, full power, for the brand and the alert hue.
const NEON_HALO_BRAND: f32 = 0.44;
const NEON_HALO_ALERT: f32 = 0.58;
/// The slow brand breath: period and trough. The alert breath is faster and
/// deeper — urgency without a strobe.
const NEON_BREATH_MS: u64 = 6_000;
const NEON_BREATH_FLOOR: f32 = 0.85;
const NEON_ALERT_BREATH_MS: u64 = 3_000;
const NEON_ALERT_BREATH_FLOOR: f32 = 0.62;
/// Daylight washes a neon out: the halo keeps this share at noon, all of it at night.
const NEON_DAYLIGHT_FLOOR: f32 = 0.5;

/// A 0..1 sine breath with trough `floor`. The ms reduce by INTEGER modulo
/// before any float cast — at wall-clock magnitude an f32 of the raw ms cannot
/// tell two frames apart (`neon_breath_advances_at_wall_clock_scale_and_stays_in_band`).
fn neon_breath(elapsed_ms: u64, period_ms: u64, floor: f32) -> f32 {
    let phase = (elapsed_ms % period_ms) as f32 / period_ms as f32;
    floor + (1.0 - floor) * ((std::f32::consts::TAU * phase).sin() * 0.5 + 0.5)
}

/// Map the sim's theme-free `levels` to this frame's colors. `darkness` is the
/// time-of-day term ([`NEON_DAYLIGHT_FLOOR`]).
pub(in crate::pixel_painter) fn neon_look(
    levels: crate::floor::NeonLevels,
    now: SystemTime,
    darkness: f32,
    theme: &Theme,
) -> NeonLook {
    let ms = epoch_ms(now);
    let power = levels.power;
    let hue = blend_rgb(theme.ui.neon_brand, theme.ui.neon_alert, levels.alert);
    let brand = NEON_HALO_BRAND * neon_breath(ms, NEON_BREATH_MS, NEON_BREATH_FLOOR);
    let alert = NEON_HALO_ALERT * neon_breath(ms, NEON_ALERT_BREATH_MS, NEON_ALERT_BREATH_FLOOR);
    let daylight = NEON_DAYLIGHT_FLOOR + (1.0 - NEON_DAYLIGHT_FLOOR) * darkness.clamp(0.0, 1.0);
    // A tube only throws light ABOVE its starved level — one darker than the wall
    // it hangs on has none to give.
    let starved = crate::floor::NeonLevels::EMPTY.power;
    let throw = ((power - starved) / (1.0 - starved)).max(0.0);
    NeonLook {
        tube: blend_rgb(BLACK, blend_rgb(hue, WHITE, NEON_TUBE_WHITEN), power),
        interior: blend_rgb(theme.office.neon_panel_bg, hue, NEON_INTERIOR_TINT * power),
        halo: hue,
        halo_strength: throw * (brand + (alert - brand) * levels.alert) * daylight,
    }
}

/// Neon sign panel — a flat dark interior inside a lit tube, painted in the wall
/// band. The text overlay is each painter's own pass on top.
pub(in crate::pixel_painter) fn paint_neon_panel(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    look: &NeonLook,
) {
    // The SAME const the board-text interior derives from (`NEON_PANEL_INNER_*`),
    // so the cells left dark == the cells the text may fill; they can't drift.
    let b = crate::pixel_painter::NEON_PANEL_BORDER;
    for dy in 0..h {
        for dx in 0..w {
            let on_border = dx < b || dx >= w - b || dy < b || dy >= h - b;
            let color = if on_border { look.tube } else { look.interior };
            buf.put_checked(x + dx, y + dy, color);
        }
    }
}

/// The sign's halo over everything already painted around the `x,y,w,h` panel —
/// a LATE pass, so the light lands ON the shelf and the clock instead of hiding
/// behind them. Falls off with distance to the panel RECT and never enters it
/// (see `the_interior_is_one_flat_color_the_halo_never_enters`).
pub(in crate::pixel_painter) fn paint_neon_glow(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    look: &NeonLook,
) {
    if look.halo_strength <= 0.0 {
        return;
    }
    let r = NEON_HALO_RADIUS;
    let xs = x.saturating_sub(r)..(x + w + r).min(buf.width());
    let ys = y.saturating_sub(r)..(y + h + r).min(buf.height());
    let (left, right) = (x as f32, (x + w - 1) as f32);
    let (top, bottom) = (y as f32, (y + h - 1) as f32);
    blend_falloff(buf, xs, ys, look.halo, |px, py| {
        let dx = (left - px as f32).max(px as f32 - right).max(0.0);
        let dy = (top - py as f32).max(py as f32 - bottom).max(0.0);
        let reach = 1.0 - (dx * dx + dy * dy).sqrt() / r as f32;
        let outside = dx > 0.0 || dy > 0.0;
        (outside && reach > 0.0).then_some(look.halo_strength * reach * reach)
    });
}

/// Live wall clock — a 7x7 face whose hands quantize to 8 directions and are
/// drawn as multi-pixel rays so they read clearly at this size.
pub(in crate::pixel_painter) fn paint_clock(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    now: SystemTime,
    theme: &Theme,
) {
    let rim = theme.office.clock_rim;
    let face = theme.office.clock_face;
    let hand_color = theme.office.clock_hand;
    let hand_min = hand_color;

    // 7x7 disc — `R` rim, `F` face, `.` transparent. Center at x+3, y+3.
    let rows: &[&[u8]] = &[
        b"..RRR..", b".RFFFR.", b"RFFFFFR", b"RFFFFFR", b"RFFFFFR", b".RFFFR.", b"..RRR..",
    ];
    for (dy, row) in rows.iter().enumerate() {
        for (dx, ch) in row.iter().enumerate() {
            let c = match ch {
                b'R' => rim,
                b'F' => face,
                _ => continue,
            };
            let px = x + dx as u16;
            let py = y + dy as u16;
            if px < buf.width() && py < buf.height() {
                buf.put(px, py, c);
            }
        }
    }

    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    use chrono::Timelike;
    let hour = local.hour() % 12;
    let minute = local.minute();

    // Fractional positions around the clock (0.0 = 12 o'clock, 0.25 = 3 o'clock).
    let hour_turns = (hour as f32 + minute as f32 / 60.0) / 12.0;
    let min_turns = minute as f32 / 60.0;

    let put = |buf: &mut RgbBuffer, ox: i32, oy: i32, color: Rgb| {
        let px = x as i32 + 3 + ox;
        let py = y as i32 + 3 + oy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width() && (py as u16) < buf.height() {
            buf.put(px as u16, py as u16, color);
        }
    };

    put(buf, 0, 0, hand_color);

    let (hdx, hdy) = octant_offset(hour_turns);
    put(buf, hdx, hdy, hand_color);

    // The 7x7 disc has 3-px face at cardinals but only 1-px face at diagonals, so
    // a length-2 diagonal minute hand would overwrite the rim and gap the border.
    let (mdx, mdy) = octant_offset(min_turns);
    let max_step = if mdx != 0 && mdy != 0 { 1 } else { 2 };
    for step in 1..=max_step {
        put(buf, mdx * step, mdy * step, hand_min);
    }
}

/// Quantize a fractional turn (0.0..1.0, 0.0 = north) to one of 8 octant
/// (dx, dy) unit offsets.
fn octant_offset(turn: f32) -> (i32, i32) {
    // rem_euclid(8) maps every i32 (incl. a NaN turn's 0 cast) into 0..=7, so
    // the table is total — a match would need a dead wildcard arm.
    const OCTANTS: [(i32, i32); 8] = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];
    let oct = ((turn * 8.0).round() as i32).rem_euclid(8);
    OCTANTS[oct as usize]
}

/// Office corridor runner, painted along the cubicle_aisle band so the eye
/// traces a path connecting the door, meeting room, pantry, cubicles and lounge.
/// Just texture over the floor — walls and decor paint on top.
pub(in crate::pixel_painter) fn paint_corridor_runner(
    buf: &mut RgbBuffer,
    rect: crate::layout::Bounds,
    theme: &Theme,
) {
    let runner_base = theme.office.runner_base;
    let runner_stripe = theme.office.runner_stripe;
    let runner_edge = theme.office.runner_edge;
    // Taste pin: a tighter stride read as bathroom tiling rather than a woven
    // runner at half-block scale.
    const RUNNER_LATTICE_STRIDE: i32 = 10;
    let max_x = (rect.x + rect.width).min(buf.width());
    let max_y = (rect.y + rect.height).min(buf.height());
    for y in rect.y..max_y {
        for x in rect.x..max_x {
            let is_edge = y == rect.y || y + 1 == max_y;
            let dy = (y - rect.y) as i32;
            let dx = (x - rect.x) as i32;
            let diamond = ((dx + dy) % RUNNER_LATTICE_STRIDE == 0)
                || ((dx - dy).rem_euclid(RUNNER_LATTICE_STRIDE) == 0);
            let color = if is_edge {
                runner_edge
            } else if diamond {
                runner_stripe
            } else {
                runner_base
            };
            buf.put(x, y, color);
        }
    }
}

/// Soft elliptical contact shadow under furniture / characters — grounds
/// floating sprites so they read as standing on the floor, not hovering.
pub(in crate::pixel_painter) fn paint_shadow(
    buf: &mut RgbBuffer,
    ellipse: Ellipse,
    strength: f32,
    theme: &Theme,
) {
    paint_ellipse_blend(buf, ellipse, strength, theme.office.shadow);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::floor::NeonLevels;
    use crate::pixel_painter::{NEON_PANEL_BORDER, NEON_PANEL_H, NEON_PANEL_W};
    use std::time::Duration;

    /// A WALL-CLOCK-scale epoch — the magnitude [`neon_breath`]'s integer modulo
    /// exists for.
    const WALL_CLOCK_MS: u64 = 1_767_000_000_000;

    fn at_ms(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn look(levels: NeonLevels, ms: u64, darkness: f32) -> NeonLook {
        neon_look(levels, at_ms(ms), darkness, &crate::theme::NORMAL)
    }

    const WALL: Rgb = Rgb {
        r: 90,
        g: 90,
        b: 90,
    };
    /// Room for the panel plus its whole halo on every side.
    const PANEL_AT: u16 = 12;

    fn lit_wall(levels: NeonLevels) -> (RgbBuffer, NeonLook) {
        let side = PANEL_AT * 2 + NEON_PANEL_W;
        let mut buf = RgbBuffer::filled(side, side, WALL);
        let look = look(levels, WALL_CLOCK_MS, 1.0);
        paint_neon_panel(
            &mut buf,
            PANEL_AT,
            PANEL_AT,
            NEON_PANEL_W,
            NEON_PANEL_H,
            &look,
        );
        paint_neon_glow(
            &mut buf,
            PANEL_AT,
            PANEL_AT,
            NEON_PANEL_W,
            NEON_PANEL_H,
            &look,
        );
        (buf, look)
    }

    #[test]
    fn neon_breath_advances_at_wall_clock_scale_and_stays_in_band() {
        let a = neon_breath(WALL_CLOCK_MS, NEON_BREATH_MS, NEON_BREATH_FLOOR);
        let b = neon_breath(WALL_CLOCK_MS + 33, NEON_BREATH_MS, NEON_BREATH_FLOOR);
        assert_ne!(a, b, "the breath must move across a 33ms frame");
        for ms in (0..NEON_BREATH_MS).step_by(97) {
            let v = neon_breath(WALL_CLOCK_MS + ms, NEON_BREATH_MS, NEON_BREATH_FLOOR);
            assert!((NEON_BREATH_FLOOR..=1.0).contains(&v), "{v} at +{ms}ms");
        }
    }

    #[test]
    fn neon_hue_is_the_brand_until_someone_waits() {
        let theme = &crate::theme::NORMAL;
        assert_eq!(
            look(NeonLevels::BUSY, WALL_CLOCK_MS, 1.0).halo,
            theme.ui.neon_brand
        );
        assert_eq!(
            look(NeonLevels::CALM, WALL_CLOCK_MS, 1.0).halo,
            theme.ui.neon_brand
        );
        assert_eq!(
            look(NeonLevels::ALERT, WALL_CLOCK_MS, 1.0).halo,
            theme.ui.neon_alert
        );
    }

    #[test]
    fn neon_halo_drops_to_its_daylight_floor_and_a_calm_sign_glows_less_than_a_busy_one() {
        let night = look(NeonLevels::BUSY, WALL_CLOCK_MS, 1.0).halo_strength;
        let day = look(NeonLevels::BUSY, WALL_CLOCK_MS, 0.0).halo_strength;
        assert!(
            (day - night * NEON_DAYLIGHT_FLOOR).abs() < 1e-6,
            "{day} vs {night}"
        );
        let calm = look(NeonLevels::CALM, WALL_CLOCK_MS, 1.0).halo_strength;
        assert!(calm > 0.0 && calm < night, "{calm} vs {night}");
    }

    #[test]
    fn a_starved_tube_throws_no_halo_and_a_flash_does() {
        assert_eq!(
            look(NeonLevels::EMPTY, WALL_CLOCK_MS, 1.0).halo_strength,
            0.0
        );
        assert!(look(NeonLevels::FLASH, WALL_CLOCK_MS, 1.0).halo_strength > 0.0);
        let (buf, _) = lit_wall(NeonLevels::EMPTY);
        assert_eq!(buf.get(PANEL_AT - 1, PANEL_AT + NEON_PANEL_H / 2), WALL);
    }

    /// A terminal text cell shows only its BOTTOM pixel (ratatui keeps the old bg
    /// under a glyph), while an uncovered cell shows both — so the interior must
    /// be one flat color and the halo must stay out of it, or the rows band.
    #[test]
    fn the_interior_is_one_flat_color_the_halo_never_enters() {
        let (buf, look) = lit_wall(NeonLevels::ALERT);
        let b = NEON_PANEL_BORDER;
        for dy in 0..NEON_PANEL_H {
            for dx in 0..NEON_PANEL_W {
                let edge = dx < b || dx >= NEON_PANEL_W - b || dy < b || dy >= NEON_PANEL_H - b;
                let want = if edge { look.tube } else { look.interior };
                assert_eq!(buf.get(PANEL_AT + dx, PANEL_AT + dy), want, "({dx},{dy})");
            }
        }
    }

    #[test]
    fn the_halo_lights_the_wall_beside_the_tube_and_stops_at_its_radius() {
        let (buf, _) = lit_wall(NeonLevels::ALERT);
        let mid_y = PANEL_AT + NEON_PANEL_H / 2;
        let beside = buf.get(PANEL_AT - 1, mid_y);
        let further = buf.get(PANEL_AT - 3, mid_y);
        assert_ne!(beside, WALL, "the wall next to the tube is lit");
        assert_ne!(further, WALL);
        assert!(
            beside.r > further.r,
            "and the light falls off with distance: {beside:?} vs {further:?}"
        );
        assert_eq!(
            buf.get(PANEL_AT - NEON_HALO_RADIUS, mid_y),
            WALL,
            "nothing AT the radius"
        );
        assert_ne!(buf.get(PANEL_AT - NEON_HALO_RADIUS + 1, mid_y), WALL);
    }

    #[test]
    fn an_unpowered_halo_leaves_the_wall_alone() {
        let mut buf = RgbBuffer::filled(60, 40, WALL);
        let mut dark = look(NeonLevels::EMPTY, 0, 1.0);
        dark.halo_strength = 0.0;
        paint_neon_glow(
            &mut buf,
            PANEL_AT,
            PANEL_AT,
            NEON_PANEL_W,
            NEON_PANEL_H,
            &dark,
        );
        assert!((0..40).all(|y| (0..60).all(|x| buf.get(x, y) == WALL)));
    }

    /// The two lights that pre-date [`blend_falloff`], re-derived the way they were
    /// written before it: the shared loop must not move one of their pixels.
    #[test]
    fn the_shared_falloff_loop_matches_the_loops_it_replaced() {
        let fill = Rgb {
            r: 40,
            g: 70,
            b: 110,
        };
        let tint = Rgb {
            r: 250,
            g: 200,
            b: 90,
        };
        let (w, h) = (40u16, 30u16);

        let g = || RadialFalloff {
            min_x: 3,
            max_x: 37,
            min_y: 2,
            max_y: 28,
            cx: 19.5,
            cy: 14.5,
            rx_norm: 16.5,
            ry_norm: 12.5,
        };
        let mut got = RgbBuffer::filled(w, h, fill);
        paint_radial_falloff(&mut got, g(), 0.63, tint);
        let mut want = RgbBuffer::filled(w, h, fill);
        let e = g();
        for y in e.min_y..e.max_y {
            for x in e.min_x..e.max_x {
                let nx = (x as f32 - e.cx) / e.rx_norm;
                let ny = (y as f32 - e.cy) / e.ry_norm;
                let r2 = nx * nx + ny * ny;
                if r2 > 1.0 {
                    continue;
                }
                let cur = want.get(x, y);
                want.put(x, y, blend_rgb(cur, tint, (1.0 - r2) * 0.63));
            }
        }
        assert!(got.as_slice() == want.as_slice(), "radial falloff moved");

        // Off-centre and clipped by two edges, like a lamp in a corner.
        let (cx, cy, radius) = (36u16, 4u16, 11u16);
        let mut got = RgbBuffer::filled(w, h, fill);
        paint_warm_halo(&mut got, cx, cy, radius, 0.47, tint);
        let mut want = RgbBuffer::filled(w, h, fill);
        let r2max = (radius as f32) * (radius as f32);
        for y in cy.saturating_sub(radius)..(cy + radius).min(h) {
            for x in cx.saturating_sub(radius)..(cx + radius).min(w) {
                let dx = x as f32 - cx as f32;
                let dy = y as f32 - cy as f32;
                let r2 = dx * dx + dy * dy;
                if r2 > r2max {
                    continue;
                }
                let cur = want.get(x, y);
                want.put(
                    x,
                    y,
                    blend_rgb(cur, tint, (1.0 - (r2 / r2max).sqrt()) * 0.47),
                );
            }
        }
        assert!(got.as_slice() == want.as_slice(), "warm halo moved");
    }

    #[test]
    fn ellipse_blend_degenerate_is_a_noop() {
        let theme = &crate::theme::NORMAL;
        let fill = Rgb {
            r: 30,
            g: 30,
            b: 30,
        };
        let mut buf = RgbBuffer::filled(20, 20, fill);
        paint_ellipse_blend(
            &mut buf,
            Ellipse {
                cx: 10,
                cy: 10,
                half_w: 0,
                half_h: 5,
            },
            0.8,
            theme.lighting.ceiling_pool,
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), fill, "half_w==0 must paint nothing");
            }
        }
        let mut buf = RgbBuffer::filled(20, 20, fill);
        paint_ellipse_blend(
            &mut buf,
            Ellipse {
                cx: 10,
                cy: 10,
                half_w: 5,
                half_h: 5,
            },
            0.0,
            theme.lighting.ceiling_pool,
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), fill, "strength<=0 must paint nothing");
            }
        }
    }

    // Negative control for `ellipse_blend_degenerate_is_a_noop`: without this a
    // painter that never painted at all would pass it vacuously.
    #[test]
    fn ellipse_blend_paints_when_valid() {
        let theme = &crate::theme::NORMAL;
        let fill = Rgb {
            r: 30,
            g: 30,
            b: 30,
        };
        let mut buf = RgbBuffer::filled(20, 20, fill);
        paint_ellipse_blend(
            &mut buf,
            Ellipse {
                cx: 10,
                cy: 10,
                half_w: 5,
                half_h: 5,
            },
            0.9,
            theme.lighting.ceiling_pool,
        );
        assert_ne!(buf.get(10, 10), fill, "the ellipse centre must be tinted");
    }

    // Off-edge must clip, not panic: the panel through `put_checked`, the glow
    // through its clamped ranges.
    #[test]
    fn neon_panel_off_edge_does_not_panic() {
        let theme = &crate::theme::NORMAL;
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5);
        let mut buf = RgbBuffer::filled(10, 10, Rgb { r: 0, g: 0, b: 0 });
        // x=8, w=6 → px reaches 13 (>= width 10); y=8, h=5 → py reaches 12.
        let look = neon_look(crate::floor::NeonLevels::ALERT, now, 1.0, theme);
        paint_neon_panel(&mut buf, 8, 8, 6, 5, &look);
        paint_neon_glow(&mut buf, 8, 8, 6, 5, &look);
        assert_ne!(
            buf.get(8, 8),
            Rgb { r: 0, g: 0, b: 0 },
            "in-bounds frame paints"
        );
    }
}
