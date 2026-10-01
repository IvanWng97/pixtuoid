//! The [effects](crate::effects) riding on the cutaway's figures, drawn on the
//! figure's own art grid: a look authored at [`LOOK_DENSITY`] where the figure
//! is drawn there with a head to stand it on, else the classic's look, a
//! layout cell per art cell block. A fade is an ordered dither, never a blend.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::order::Span;
use crate::cutaway::pen::{ArtPx, ArtRect, Pen};
use crate::effects::{Effect, EffectKind};
use crate::pixel_painter::effects::{
    FLAME_CORE, FLAME_DEEP, FLAME_MID, FLAME_TIP, SLEEP_Z_MAX_RISE, plot_effect, sleep_z_fade,
};
use crate::theme::Theme;

/// The density this module's own looks are drawn at.
const LOOK_DENSITY: u16 = 4;

/// A point on the art grid, which a look may hang past the buffer's top or
/// west edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArtAt {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// One effect on the figure it rides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Riding {
    pub(crate) effect: Effect,
    /// The figure's head on the art grid: the top of its hair over the head
    /// mark's column. `None` where its frame has no head to stand a look on.
    pub(crate) head: Option<ArtAt>,
    /// The figure's own grid.
    pub(crate) pen: Pen,
}

impl Riding {
    /// The logical cells it paints, sorted with the figure it rides on at
    /// `depth`; `None` when it paints nothing this frame.
    pub(crate) fn span(&self, theme: &Theme, depth: u16) -> Option<Span> {
        let d = i32::from(self.pen.art(1).0);
        let mut cells: Option<(i32, i32, i32, i32)> = None;
        self.look(theme, &mut |at, size, _, _| {
            let (x1, y1) = (at.x + size - 1, at.y + size - 1);
            cells = Some(cells.map_or((at.x, at.y, x1, y1), |(a, b, c, e)| {
                (a.min(at.x), b.min(at.y), c.max(x1), e.max(y1))
            }));
        });
        let (x0, y0, x1, y1) = cells?;
        let cell = |a: i32| u16::try_from(a.max(0).div_euclid(d)).unwrap_or(u16::MAX);
        let span = Span {
            x0: cell(x0),
            x1: cell(x1),
            y0: cell(y0),
            y1: cell(y1),
            depth,
            layer: crate::layout::Layer::Figure,
        };
        Some(span)
    }

    /// Paint it into `buf`.
    pub(crate) fn paint(&self, theme: &Theme, buf: &mut RgbBuffer) {
        let pen = self.pen;
        self.look(theme, &mut |at, size, c, coverage| {
            let (Ok(x), Ok(y)) = (u16::try_from(at.x), u16::try_from(at.y)) else {
                return;
            };
            if coverage >= 1.0 || crate::dither::takes_next(x, y, coverage) {
                let size = ArtPx(u16::try_from(size).unwrap_or(1));
                let r = ArtRect {
                    x: ArtPx(x),
                    y: ArtPx(y),
                    w: size,
                    h: size,
                };
                pen.fill(buf, r, c);
            }
        });
    }

    /// Its look, as squares of art pixels: each top-left, side, colour and the
    /// share of it covered.
    fn look(&self, theme: &Theme, emit: &mut impl FnMut(ArtAt, i32, Rgb, f32)) {
        let e = self.effect;
        let d = self.pen.art(1).0;
        if let Some(head) = self.head.filter(|_| d == LOOK_DENSITY) {
            dense_look(e, head, theme, emit);
            return;
        }
        let d = i32::from(d);
        plot_effect(&e, theme, &mut |x, y, c, alpha| {
            let at = ArtAt {
                x: i32::from(x) * d,
                y: i32::from(y) * d,
            };
            emit(at, d, c, alpha);
        });
    }
}

/// A sleeping head's z, rising off the side of the head and drifting away.
const SLEEP_Z: &[&str] = &[
    "######", //
    "....##", //
    "...##.", //
    "..##..", //
    ".##...", //
    "######", //
];

/// The waiting mark, beside the head.
const WAITING_MARK: &[&str] = &[
    ".####.", //
    "##..##", //
    "##..##", //
    "...##.", //
    "..##..", //
    "..##..", //
    "......", //
    "..##..", //
    "..##..", //
];

/// Art columns from the head mark to the west edge of what floats beside the
/// head: clear of the hair, and of the badge centred over it.
const BESIDE_DX: i32 = 9;
/// Art rows from the hair's top down to the bottom of what floats beside it.
const BESIDE_DY: i32 = 5;
/// The opacity down to which a z stays whole: a dither through a stroke two
/// pixels wide breaks the glyph, so it dissolves only on its way out.
const SLEEP_Z_SOLID: f32 = 0.3;
/// Art rows a rising z drifts one column away from the head.
const SLEEP_Z_DRIFT: i32 = 4;

/// The flame crown's two frames, centred over the head mark, their bottom row
/// on the hair's top: `D`eep, `M`id, `T`ip, `C`ore.
const FLAME_CROWN: [&[&str]; 2] = [
    &[
        "......M........", //
        "......DM....M..", //
        "..M..DMD...DM..", //
        "..DM.DTMD..DMD.", //
        ".DMD.MTTMD.MTD.", //
        ".DTMDMTCTMDTTMD", //
        "DMTTMTCCCTMTCTD", //
        "DTCCTCCCCCTCCMD", //
        "DMTTCCTCCTCTTMD", //
    ],
    &[
        "........M......", //
        "...M...MD......", //
        "...MD..DMD..M..", //
        "..DMD.DMTD..MD.", //
        ".DMTD.MTTMD.DMD", //
        "DMTTMDTCTMDMTD.", //
        "DTCTMTCCCTMTTMD", //
        "DMCCTCCCCCTCCTD", //
        "DMTTCTCCTCCTTMD", //
    ],
];

/// The walking dust's speckle, kicked up round a foot's sole.
const DUST: &[&str] = &[
    "..#.#..", //
    ".#.#.#.", //
    "#.#.#.#", //
];

/// `e`'s look at [`LOOK_DENSITY`], from its rider's `head`.
fn dense_look(e: Effect, head: ArtAt, theme: &Theme, emit: &mut impl FnMut(ArtAt, i32, Rgb, f32)) {
    let d = i32::from(LOOK_DENSITY);
    let centred = |glyph: &[&str]| head.x - glyph_w(glyph) / 2;
    match e.kind {
        EffectKind::SleepZ => {
            let Some((alpha, t)) = sleep_z_fade(e.phase) else {
                return;
            };
            let rise = (t * f32::from(SLEEP_Z_MAX_RISE) * d as f32) as i32;
            let x = head.x + BESIDE_DX + rise / SLEEP_Z_DRIFT;
            let top = head.y + BESIDE_DY - glyph_h(SLEEP_Z) - rise;
            let coverage = (alpha / SLEEP_Z_SOLID).min(1.0);
            stamp(SLEEP_Z, x, top, emit, |_| {
                Some((theme.effects.sleep_z, coverage))
            });
        }
        EffectKind::WaitingMark => {
            let top = head.y + BESIDE_DY - glyph_h(WAITING_MARK);
            stamp(WAITING_MARK, head.x + BESIDE_DX, top, emit, |_| {
                Some((theme.effects.waiting_bubble, 1.0))
            });
        }
        EffectKind::FlameCrown => {
            let glyph = FLAME_CROWN[usize::from(e.phase == 1)];
            let top = head.y - glyph_h(glyph) + 1;
            stamp(glyph, centred(glyph), top, emit, |ch| {
                let c = match ch {
                    'D' => FLAME_DEEP,
                    'M' => FLAME_MID,
                    'T' => FLAME_TIP,
                    _ => FLAME_CORE,
                };
                Some((c, 1.0))
            });
        }
        EffectKind::WalkingDust => {
            // The classic's foot columns, on the layout grid the walker's
            // anchor shares with it.
            let foot = if e.phase == 0 { 6 } else { 1 };
            let x = (i32::from(e.at.x) + foot) * d + d / 2 - glyph_w(DUST) / 2;
            let y = (i32::from(e.at.y) + i32::from(crate::layout::WALKING_Y_OFF)) * d - 1;
            stamp(DUST, x, y, emit, |_| {
                Some((theme.effects.walking_dust, 1.0))
            });
        }
        // Riders of pets, cups and mascots, which the cutaway does not draw.
        EffectKind::PetHeart | EffectKind::SteamPuff | EffectKind::MascotBubble => {}
    }
}

fn glyph_w(glyph: &[&str]) -> i32 {
    glyph.first().map_or(0, |row| row.len() as i32)
}

fn glyph_h(glyph: &[&str]) -> i32 {
    glyph.len() as i32
}

/// Emit each of `glyph`'s inked cells from `(x, y)`, coloured by `ink`.
fn stamp(
    glyph: &[&str],
    x: i32,
    y: i32,
    emit: &mut impl FnMut(ArtAt, i32, Rgb, f32),
    ink: impl Fn(char) -> Option<(Rgb, f32)>,
) {
    for (dy, row) in glyph.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch == '.' {
                continue;
            }
            if let Some((c, coverage)) = ink(ch) {
                let at = ArtAt {
                    x: x + dx as i32,
                    y: y + dy as i32,
                };
                emit(at, 1, c, coverage);
            }
        }
    }
}
