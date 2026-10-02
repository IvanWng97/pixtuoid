//! Each effect's look in layout cells, which every painter draws it from;
//! [`super`] decides only when and where one shows.

use pixtuoid_core::sprite::Rgb;

use super::{Effect, EffectKind, HEART_LIFE_MS, SLEEP_Z_RISE_MS, STEAM_CYCLE_MS};
use crate::composite::WHITE;
use crate::layout::{Point, WALKING_Y_OFF};
use crate::theme::Theme;

/// The scanline's color over a screen glowing `tint`.
pub(crate) fn scanline_color(tint: Rgb) -> Rgb {
    tint.mix(WHITE, 0.7)
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

/// How long a steam puff holds each row of its rise: one Full beat, so it
/// rises evenly.
const STEAM_ROW_MS: u64 = crate::anim::FULL_TICK_MS;
/// How long it holds each side of its wiggle, in whole beats.
const STEAM_WIGGLE_MS: u64 = 2 * crate::anim::FULL_TICK_MS;

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
/// wears (`character::colors::EMBER_HAIR` aliases it).
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
