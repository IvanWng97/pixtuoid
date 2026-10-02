//! The [effects](crate::effects) riding on the cutaway's figures, drawn on the
//! figure's own art grid: a look authored at [`LOOK_DENSITY`] where the figure
//! is drawn there with a head to stand it on, the base art's z and waiting
//! mark beside its head, else its [shared look](crate::effects::look), a
//! layout cell per art cell block. A fade is an ordered dither, never a blend,
//! and a look stays whole down to [`LOOK_SOLID`].

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::order::Span;
use crate::cutaway::pen::{ArtPx, ArtRect, Pen};
use crate::effects::look::{
    FLAME_CORE, FLAME_DEEP, FLAME_MID, FLAME_TIP, SLEEP_Z_MAX_RISE, plot_effect, sleep_z_fade,
    walking_dust_foot,
};
use crate::effects::{Effect, EffectKind};
use crate::theme::Theme;

/// The density this module's own looks are drawn at.
const LOOK_DENSITY: u16 = 4;

/// A point on the art grid, which a look may hang past the buffer's top or
/// west edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArtPoint {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// One effect on the figure it rides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Riding {
    pub(crate) effect: Effect,
    /// The figure's head on the art grid: the top of its hair over the head
    /// mark's column. `None` where its frame has no head to stand a look on.
    pub(crate) head: Option<ArtPoint>,
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
            let (Ok(x), Ok(y), Ok(side)) = (
                u16::try_from(at.x),
                u16::try_from(at.y),
                u16::try_from(size),
            ) else {
                return;
            };
            // Dithered on the look's own grid, so a look drawn in blocks
            // matches itself at every density.
            if coverage >= 1.0 || crate::dither::takes_next(x / side, y / side, coverage) {
                let size = ArtPx(side);
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
    fn look(&self, theme: &Theme, emit: &mut impl FnMut(ArtPoint, i32, Rgb, f32)) {
        let e = self.effect;
        let d = self.pen.art(1).0;
        match self.head {
            Some(head) if d == LOOK_DENSITY => return dense_look(e, head, theme, emit),
            Some(head) if d == 1 && base_look(e, head, theme, emit) => return,
            _ => {}
        }
        let d = i32::from(d);
        plot_effect(&e, theme, &mut |x, y, c, alpha| {
            let at = ArtPoint {
                x: i32::from(x) * d,
                y: i32::from(y) * d,
            };
            emit(at, d, c, (alpha / LOOK_SOLID).min(1.0));
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
/// The opacity down to which a look stays whole: a dither through a glyph a
/// stroke or two wide breaks it, so it dissolves only on its way out.
const LOOK_SOLID: f32 = 0.3;
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

/// The base art's sleep z: the classic's glyph.
const SLEEP_Z_1X: &[&str] = &[
    "##", //
    ".#", //
    "##", //
];
/// The base art's waiting mark: the classic's glyph.
const WAITING_MARK_1X: &[&str] = &[
    "###", //
    "..#", //
    ".#.", //
    ".#.", //
];
/// Columns from the base art's head to the west edge of what floats beside it:
/// past the head, and the badge centred over it.
const BESIDE_DX_1X: i32 = 5;
/// Rows from the head's top down to the bottom of what floats beside it.
const BESIDE_DY_1X: i32 = 1;

/// `e`'s look on the base art, from its rider's `head`, where the base art
/// has a look of its own: whether it drew one.
fn base_look(
    e: Effect,
    head: ArtPoint,
    theme: &Theme,
    emit: &mut impl FnMut(ArtPoint, i32, Rgb, f32),
) -> bool {
    match e.kind {
        EffectKind::SleepZ => {
            if let Some((alpha, t)) = sleep_z_fade(e.phase) {
                let rise = (t * f32::from(SLEEP_Z_MAX_RISE)) as i32;
                let top = head.y + BESIDE_DY_1X - glyph_h(SLEEP_Z_1X) - rise;
                let coverage = (alpha / LOOK_SOLID).min(1.0);
                stamp(SLEEP_Z_1X, head.x + BESIDE_DX_1X, top, emit, |_| {
                    Some((theme.effects.sleep_z, coverage))
                });
            }
            true
        }
        EffectKind::WaitingMark => {
            let top = head.y + BESIDE_DY_1X - glyph_h(WAITING_MARK_1X);
            stamp(WAITING_MARK_1X, head.x + BESIDE_DX_1X, top, emit, |_| {
                Some((theme.effects.waiting_bubble, 1.0))
            });
            true
        }
        _ => false,
    }
}

/// `e`'s look at [`LOOK_DENSITY`], from its rider's `head`.
fn dense_look(
    e: Effect,
    head: ArtPoint,
    theme: &Theme,
    emit: &mut impl FnMut(ArtPoint, i32, Rgb, f32),
) {
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
            let coverage = (alpha / LOOK_SOLID).min(1.0);
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
            let foot = walking_dust_foot(e.at, e.phase);
            let x = i32::from(foot.x) * d + d / 2 - glyph_w(DUST) / 2;
            let y = i32::from(foot.y) * d - 1;
            stamp(DUST, x, y, emit, |_| {
                Some((theme.effects.walking_dust, 1.0))
            });
        }
        // A creature's riders have no head, so `look` plots them as the classic
        // does; steam isn't drawn here.
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
    emit: &mut impl FnMut(ArtPoint, i32, Rgb, f32),
    ink: impl Fn(char) -> Option<(Rgb, f32)>,
) {
    for (dy, row) in glyph.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch == '.' {
                continue;
            }
            if let Some((c, coverage)) = ink(ch) {
                let at = ArtPoint {
                    x: x + dx as i32,
                    y: y + dy as i32,
                };
                emit(at, 1, c, coverage);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A look stays whole while it shows at least [`LOOK_SOLID`] and only then
    /// dithers out: a fresh heart paints all its cells, a fading one fewer, a
    /// spent one none.
    #[test]
    fn a_fading_look_stays_whole_then_dissolves() {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let pen = Pen::new(crate::render_scale::RenderScale::ONE, 1).expect("1 divides 1");
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let drawn = |phase| {
            let heart = Riding {
                effect: Effect {
                    kind: EffectKind::PetHeart,
                    at: crate::layout::Point { x: 10, y: 20 },
                    phase,
                },
                head: None,
                pen,
            };
            let mut buf = RgbBuffer::filled(32, 32, bg);
            heart.paint(theme, &mut buf);
            buf.as_slice().iter().filter(|&&c| c != bg).count()
        };
        let life = crate::effects::HEART_LIFE_MS;
        let fresh = drawn(0);
        assert_eq!(fresh, 4, "a fresh heart is whole");
        let fading = drawn(life * 4 / 5);
        assert!(
            0 < fading && fading < fresh,
            "a fading heart dissolves: {fading}"
        );
        assert_eq!(drawn(life - 1), 0, "a spent heart is gone");
    }

    /// A look is centred and stacked by its first row's width and its row
    /// count, so a ragged row would sit it off its mark.
    #[test]
    fn every_look_is_a_rectangle() {
        for glyph in [SLEEP_Z, WAITING_MARK, DUST, FLAME_CROWN[0], FLAME_CROWN[1]] {
            assert!(
                glyph.iter().all(|row| row.len() == glyph[0].len()),
                "{glyph:?}"
            );
        }
        for frame in FLAME_CROWN {
            assert!(
                frame
                    .iter()
                    .flat_map(|row| row.chars())
                    .all(|c| ".DMTC".contains(c)),
                "{frame:?} inks a colour the crown has no name for"
            );
        }
    }
}
