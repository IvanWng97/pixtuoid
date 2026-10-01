//! Effects that ride on a character, a pet or a fixture, painted with it in
//! z-order: the looks of the [`crate::effects`] model's, and a screen's own.

use std::time::SystemTime;

use crate::layout::WALKING_Y_OFF;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::epoch_ms;
use super::palette::{WHITE, blend_pixel};
use crate::effects::{Effect, EffectKind, HEART_LIFE_MS, SLEEP_Z_RISE_MS, STEAM_CYCLE_MS};
use crate::layout::{Point, SCREEN_GLASS_COLS};
use crate::theme::Theme;

/// Standby tint on a BACK-TURNED desk's glass — a grid of black rectangles reads as "everyone went home".
pub(super) fn paint_screen_idle(
    buf: &mut RgbBuffer,
    desk_x: u16,
    sprite_top: u16,
    tint: Rgb,
    strength: f32,
) {
    if strength <= 0.0 {
        return;
    }
    for dx in SCREEN_GLASS_COLS {
        for dy in SCREEN_GLASS_ROWS {
            blend_pixel(buf, desk_x + dx, sprite_top + dy, tint, strength);
        }
    }
}

/// Casing rows offset from the sprite TOP; lighting only the lower one leaves a black bar capping the glow.
const SCREEN_CASING_ROWS: std::ops::RangeInclusive<u16> = 0..=1;
const SCREEN_GLASS_ROWS: std::ops::RangeInclusive<u16> = 2..=3;
const SCREEN_CHIN_ROW: u16 = 4;

/// How long the scanline holds each glass column.
const SCANLINE_STEP_MS: u64 = 120;

/// The scanline's color over a screen glowing `tint`.
fn scanline_color(tint: Rgb) -> Rgb {
    tint.mix(WHITE, 0.7)
}

/// `sprite_top` is the monitor's first frame row, NOT `desk.y` — a raised monitor sits rows higher.
pub(super) fn paint_screen_glow(
    buf: &mut RgbBuffer,
    desk_x: u16,
    sprite_top: u16,
    now: SystemTime,
    tint: Rgb,
    theme: &Theme,
) {
    // Untinted, the theme's cool `monitor_frame_lit` read as a metal plate capping the glow.
    const CASING_TINT: f32 = 0.35;
    let frame_lit = theme.effects.monitor_frame_lit.mix(tint, CASING_TINT);
    let glow = tint;
    let glow_bright = tint.mix(WHITE, 0.4);
    let scanline = scanline_color(tint);
    let put = |buf: &mut RgbBuffer, dx: u16, dy: u16, c: Rgb| {
        buf.put_checked(desk_x + dx, sprite_top + dy, c);
    };
    // The casing frames the glass, one column proud of it on each side.
    let casing_cols = SCREEN_GLASS_COLS.start() - 1..=SCREEN_GLASS_COLS.end() + 1;
    for dy in SCREEN_CASING_ROWS {
        for dx in casing_cols.clone() {
            put(buf, dx, dy, frame_lit);
        }
    }
    let (glass_upper, glass_lower) = (*SCREEN_GLASS_ROWS.start(), *SCREEN_GLASS_ROWS.end());
    for dx in SCREEN_GLASS_COLS {
        put(buf, dx, glass_upper, glow_bright);
        put(buf, dx, glass_lower, glow);
    }
    for dx in SCREEN_GLASS_COLS {
        put(buf, dx, SCREEN_CHIN_ROW, frame_lit);
    }
    let elapsed_ms = epoch_ms(now);
    let phase = elapsed_ms / SCANLINE_STEP_MS + u64::from(desk_x);
    let glass_w = SCREEN_GLASS_COLS.end() - SCREEN_GLASS_COLS.start() + 1;
    // The remainder is below `glass_w`, a u16, so the cast cannot truncate.
    let scan_col = SCREEN_GLASS_COLS.start() + (phase % u64::from(glass_w)) as u16;
    for dy in SCREEN_GLASS_ROWS {
        put(buf, scan_col, dy, scanline);
    }
}

/// Paint `e` in its look.
pub(super) fn paint_effect(buf: &mut RgbBuffer, e: &Effect, theme: &Theme) {
    plot_effect(e, theme, &mut |x, y, c, alpha| {
        if alpha >= 1.0 {
            buf.put_checked(x, y, c);
        } else {
            blend_pixel(buf, x, y, c, alpha);
        }
    });
}

/// `e`'s look in layout cells: each one `plot` gets is painted its colour over
/// `alpha` of what lies there, a whole cell at `1.0`.
pub(crate) fn plot_effect(e: &Effect, theme: &Theme, plot: &mut impl FnMut(u16, u16, Rgb, f32)) {
    match e.kind {
        EffectKind::SleepZ => plot_sleep_z(plot, e.at, e.phase, theme),
        EffectKind::WaitingMark => plot_waiting_mark(plot, e.at, theme),
        EffectKind::WalkingDust => plot_walking_dust(plot, e.at, e.phase, theme),
        EffectKind::FlameCrown => plot_flame_crown(plot, e.at, e.phase),
        EffectKind::PetHeart => plot_pet_heart(plot, e.at, e.phase),
        EffectKind::SteamPuff => plot_steam_puff(plot, e.at, e.phase, theme),
        EffectKind::MascotBubble => plot_mascot_bubble(plot, e.at, e.phase),
    }
}

/// [`paint_effect`] each of `effects`, in order.
pub(super) fn paint_effects<'e>(
    buf: &mut RgbBuffer,
    effects: impl IntoIterator<Item = &'e Effect>,
    theme: &Theme,
) {
    for e in effects {
        paint_effect(buf, e, theme);
    }
}

/// Layout rows a sleep z rises over its life.
pub(crate) const SLEEP_Z_MAX_RISE: u16 = 4;

/// How opaque a sleep z `phase_ms` into its rise is, and how far through the
/// rise it is; `None` once it is too faint to see.
pub(crate) fn sleep_z_fade(phase_ms: u64) -> Option<(f32, f32)> {
    // The height-coupled fade (`1.0 - t`) is what keeps the z from reading as a
    // solid mark parked over the sprite: it is only briefly visible near the
    // head, then dissolves.
    const FADE_IN_MS: f32 = 150.0;
    const PEAK_ALPHA: f32 = 0.9;
    let t = phase_ms as f32 / SLEEP_Z_RISE_MS as f32;
    // Ramp-in avoids a hard pop when a fresh z spawns at the head.
    let fade_in = (phase_ms as f32 / FADE_IN_MS).min(1.0);
    let alpha = PEAK_ALPHA * fade_in * (1.0 - t);
    (alpha >= 0.06).then_some((alpha, t))
}

fn plot_sleep_z(
    plot: &mut impl FnMut(u16, u16, Rgb, f32),
    at: Point,
    phase_ms: u64,
    theme: &Theme,
) {
    let z_color = theme.effects.sleep_z;
    let Some((alpha, t)) = sleep_z_fade(phase_ms) else {
        return;
    };
    let rise = (t * SLEEP_Z_MAX_RISE as f32) as u16;
    let z_x = at.x + 5;
    let z_y = at.y.saturating_sub(rise + 3);
    const GLYPH: &[(u16, u16)] = &[(0, 0), (1, 0), (1, 1), (0, 2), (1, 2)];
    for (dx, dy) in GLYPH {
        plot(z_x + dx, z_y + dy, z_color, alpha);
    }
}

/// How long a steam puff holds each row of its rise.
const STEAM_ROW_MS: u64 = 140;
/// How long it holds each side of its wiggle.
const STEAM_WIGGLE_MS: u64 = 200;

fn plot_steam_puff(
    plot: &mut impl FnMut(u16, u16, Rgb, f32),
    spout: Point,
    phase: u64,
    theme: &Theme,
) {
    let rise = (phase / STEAM_ROW_MS) as u16;
    let alpha = 1.0 - phase as f32 / STEAM_CYCLE_MS as f32;
    if alpha < 0.15 {
        return;
    }
    let wiggle = if (phase / STEAM_WIGGLE_MS).is_multiple_of(2) {
        0
    } else {
        1
    };
    let px = spout.x + wiggle;
    let py = spout.y.saturating_sub(rise + 2);
    plot(px, py, theme.effects.coffee_steam, alpha * 0.55);
}

/// The cell under the foot a walker anchored at `walker_anchor` steps on with
/// stride frame `stride`, where its dust rises.
pub(crate) fn walking_dust_foot(walker_anchor: Point, stride: u64) -> Point {
    Point {
        x: walker_anchor.x + if stride == 0 { 6 } else { 1 },
        y: walker_anchor.y + WALKING_Y_OFF,
    }
}

fn plot_walking_dust(
    plot: &mut impl FnMut(u16, u16, Rgb, f32),
    walker_anchor: Point,
    stride: u64,
    theme: &Theme,
) {
    let foot = walking_dust_foot(walker_anchor, stride);
    plot(foot.x, foot.y, theme.effects.walking_dust, 0.45);
}

/// One floating heart for the "pet the cat" interaction.
fn plot_pet_heart(plot: &mut impl FnMut(u16, u16, Rgb, f32), at: Point, phase_ms: u64) {
    let heart_color = Rgb {
        r: 255,
        g: 100,
        b: 100,
    };
    let t = phase_ms as f32 / HEART_LIFE_MS as f32;
    let rise = (t * 6.0) as u16;
    let alpha = 1.0 - t;
    if alpha < 0.05 {
        return;
    }
    let hy = at.y.saturating_sub(4 + rise);
    for dy in 0..2u16 {
        for ddx in 0..2u16 {
            plot(at.x + ddx, hy + dy, heart_color, alpha * 0.8);
        }
    }
}

/// One "working" bubble over a busy gateway mascot, `rise` rows up.
fn plot_mascot_bubble(plot: &mut impl FnMut(u16, u16, Rgb, f32), at: Point, rise: u64) {
    let bubble = Rgb {
        r: 0xd6,
        g: 0xf2,
        b: 0xf8,
    };
    plot(at.x, at.y.saturating_sub(rise as u16), bubble, 1.0);
}

fn plot_waiting_mark(plot: &mut impl FnMut(u16, u16, Rgb, f32), anchor: Point, theme: &Theme) {
    let fg = theme.effects.waiting_bubble;
    const GLYPH: &[&[u8]] = &[b".YYY.", b"...Y.", b"..Y..", b"..Y.."];
    let bx = anchor.x + 1;
    let by = anchor.y.saturating_sub(5) & !1u16;
    for (dy, row) in GLYPH.iter().enumerate() {
        for (dx, byte) in row.iter().enumerate() {
            if *byte != b'Y' {
                continue;
            }
            plot(bx + dx as u16, by + dy as u16, fg, 1.0);
        }
    }
}

/// The flame gradient's deep-ember base, which a burning agent's hair also
/// wears (`palette::EMBER_HAIR` aliases it).
pub(crate) const FLAME_DEEP: Rgb = Rgb {
    r: 0xc2,
    g: 0x28,
    b: 0x12,
};

/// The flame gradient's yellow tip. `pub(crate)` so render tests assert the REAL
/// painted colors instead of re-hardcoding them.
pub(crate) const FLAME_TIP: Rgb = Rgb {
    r: 0xff,
    g: 0xd2,
    b: 0x4a,
};

/// The flame gradient's orange between [`FLAME_DEEP`] and [`FLAME_TIP`].
pub(crate) const FLAME_MID: Rgb = Rgb {
    r: 0xe8,
    g: 0x64,
    b: 0x1f,
};

/// The flame's hottest heart, over [`FLAME_TIP`].
pub(crate) const FLAME_CORE: Rgb = Rgb {
    r: 0xff,
    g: 0xf3,
    b: 0xa0,
};

/// `crown` is the head's top-centre; `frame` which of the two shows.
fn plot_flame_crown(plot: &mut impl FnMut(u16, u16, Rgb, f32), crown: Point, frame: u64) {
    // The asymmetric two-frame flicker is what reads as fire, not a hat, and the
    // tips stay ≤2 px above the hair top so the flame never collides with the
    // name-badge row.
    const TIP: Rgb = FLAME_TIP;
    // Pattern entries are (dx from the head center, dy up from the hair top, color).
    let frame_a: &[(i32, u16, Rgb)] = &[
        (-2, 0, FLAME_MID),
        (-1, 0, FLAME_MID),
        (0, 0, FLAME_DEEP),
        (1, 0, FLAME_MID),
        (-2, 1, FLAME_MID),
        (-1, 1, FLAME_CORE),
        (0, 1, FLAME_MID),
        (1, 1, TIP),
        (-2, 2, TIP),
        (0, 2, TIP),
    ];
    let frame_b: &[(i32, u16, Rgb)] = &[
        (-2, 0, FLAME_MID),
        (-1, 0, FLAME_DEEP),
        (0, 0, FLAME_MID),
        (1, 0, FLAME_MID),
        (-2, 1, TIP),
        (-1, 1, FLAME_MID),
        (0, 1, FLAME_CORE),
        (1, 1, FLAME_MID),
        (-1, 2, TIP),
        (1, 2, TIP),
    ];
    for &(dx, dy, c) in if frame == 1 { frame_b } else { frame_a } {
        let Some(px) = crown.x.checked_add_signed(dx as i16) else {
            continue;
        };
        let Some(py) = crown.y.checked_sub(dy) else {
            continue;
        };
        plot(px, py, c, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The phase is epoch-based, so it outgrows a u16: the scanline must keep
    /// stepping one column per step rather than overflow or jump.
    #[test]
    fn the_scanline_keeps_stepping_past_a_u16_phase() {
        let scan_col = |step: u64| {
            let mut buf = RgbBuffer::filled(32, 8, Rgb { r: 0, g: 0, b: 0 });
            let now = SystemTime::UNIX_EPOCH + Duration::from_millis(step * SCANLINE_STEP_MS + 1);
            let tint = Rgb { r: 0, g: 200, b: 0 };
            paint_screen_glow(&mut buf, 0, 0, now, tint, theme());
            SCREEN_GLASS_COLS
                .clone()
                .find(|&x| buf.get(x, *SCREEN_GLASS_ROWS.start()) == scanline_color(tint))
                .expect("a scanline column")
        };
        let (before, after) = (
            scan_col(u64::from(u16::MAX)),
            scan_col(u64::from(u16::MAX) + 1),
        );
        let glass_w = SCREEN_GLASS_COLS.end() - SCREEN_GLASS_COLS.start() + 1;
        assert_eq!(
            (after - SCREEN_GLASS_COLS.start()),
            (before - SCREEN_GLASS_COLS.start() + 1) % glass_w
        );
    }

    fn theme() -> &'static Theme {
        crate::theme::theme_by_name("normal").expect("normal theme")
    }

    /// The glow is placed by `SCREEN_*`; the art is generated by
    /// scripts/gen-art.py. This pins the pair on the bundled back-turned desk,
    /// the one whose screen a sitter works at: glass under every glass pixel
    /// the glow paints, and an opaque, non-glass casing and chin around it.
    #[test]
    fn the_glow_lands_on_the_desk_arts_monitor() {
        use crate::pack::{SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};
        let pack = crate::pack::test_default_pack();
        let art = pack
            .animation("desk_north")
            .and_then(|a| a.frames().first())
            .expect("the bundled pack ships desk_north");
        let screen: Vec<Rgb> = [SCREEN_GLASS_KEY, SCREEN_TEXT_KEY]
            .iter()
            .map(|&k| {
                pack.palette()
                    .get(k)
                    .flatten()
                    .expect("an opaque screen key")
            })
            .collect();
        let px = |x: u16, y: u16| art.get(x, y).copied().flatten();
        for y in SCREEN_GLASS_ROWS {
            for x in SCREEN_GLASS_COLS {
                assert!(
                    px(x, y).is_some_and(|c| screen.contains(&c)),
                    "({x},{y}) is where the glow paints glass, and the art has no glass there"
                );
            }
        }
        let casing_cols = SCREEN_GLASS_COLS.start() - 1..=SCREEN_GLASS_COLS.end() + 1;
        let frame = SCREEN_CASING_ROWS
            .flat_map(|y| casing_cols.clone().map(move |x| (x, y)))
            .chain(SCREEN_GLASS_COLS.map(|x| (x, SCREEN_CHIN_ROW)));
        for (x, y) in frame {
            assert!(
                px(x, y).is_some_and(|c| !screen.contains(&c)),
                "({x},{y}) is where the glow paints the casing or chin, and the art is not casing there"
            );
        }
    }

    fn render(head: Point, phase_ms: u64) -> RgbBuffer {
        let mut buf = RgbBuffer::filled(64, 64, Rgb { r: 0, g: 0, b: 0 });
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(phase_ms);
        paint_effects(&mut buf, &crate::effects::sleep_z(head, 0, now), theme());
        buf
    }

    fn top_lit(buf: &RgbBuffer, head: Point, bg: Rgb) -> Option<(u16, Rgb)> {
        let zx = head.x + 5;
        (0..head.y).find_map(|y| {
            let p = buf.get(zx, y);
            (p != bg).then_some((y, p))
        })
    }

    #[test]
    fn sleep_z_dims_as_it_rises_then_rests() {
        let head = Point { x: 20, y: 30 };
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let zx = head.x + 5;

        let low = render(head, 200);
        let low_px = low.get(zx, head.y - 3);
        assert!(low_px.lightness() > 0.0, "z near the head is visible");

        let high = render(head, 1600);
        let (top_y, top_px) = top_lit(&high, head, bg).expect("risen z still visible");
        assert!(top_y < head.y - 3, "z rose above its spawn row");
        assert!(
            top_px.lightness() < low_px.lightness(),
            "a higher z must be dimmer than one at the head"
        );

        // 2300 ms lands in the rest gap (phase >= RISE_MS).
        let resting = render(head, 2300);
        for y in 0..resting.height() {
            for x in 0..resting.width() {
                assert_eq!(resting.get(x, y), bg, "no z during the rest gap");
            }
        }
    }
}
