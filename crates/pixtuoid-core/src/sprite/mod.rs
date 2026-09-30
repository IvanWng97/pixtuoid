use std::collections::HashMap;
use std::sync::Arc;

use palette::convert::FromColorUnclamped;
use palette::{FromColor, IsWithinBounds, LinSrgb, Mix, Oklab, Srgb};

use crate::grid::Grid;

/// Compositing a `Frame` onto an `RgbBuffer`, skipping transparent pixels.
pub mod blit;
pub mod error;
/// Sprite-pack file format: `pack.toml` + `.sprite` parsing and pack loading.
pub mod format;

/// An opaque 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red channel, 0–255.
    pub r: u8,
    /// Green channel, 0–255.
    pub g: u8,
    /// Blue channel, 0–255.
    pub b: u8,
}

/// The share of the distance left to white or black that one ramp level covers.
/// Small, so a ramp's contrast is chosen per material by how many levels it
/// spans.
const RAMP_LIGHTNESS_STEP: f32 = 0.1;

/// How far one ramp level pulls a color toward the warm or cool hue, in OKLab
/// chroma.
const RAMP_HUE_PULL: f32 = 0.006;

/// The OKLCH hue highlights pull toward: amber, a warm key light.
const RAMP_WARM_HUE_DEG: f32 = 80.0;

/// The OKLCH hue shadows pull toward: blue-violet, a cool ambient fill.
const RAMP_COOL_HUE_DEG: f32 = 280.0;

/// Halvings of an out-of-gamut color's chroma search; far past the point where
/// a further halving moves no 8-bit channel.
const GAMUT_BISECTION_STEPS: u32 = 16;

impl Rgb {
    /// This color `level` steps along a hue-shifted ramp: lighter and warmer
    /// above zero, darker and cooler below, itself at zero.
    ///
    /// Stepped in OKLab, where equal lightness steps look equal whatever the
    /// hue. A level covers a share of the distance left to white or black
    /// rather than a fixed amount, so levels close in on them instead of
    /// clamping: a fixed step would merge a dark base's deeper shadows into one
    /// black.
    ///
    /// The warm and cool shift is a pull in OKLab's a/b plane toward a fixed
    /// hue, not a hue rotation. A rotation "toward yellow" flips direction at
    /// the hue opposite yellow and has nothing to rotate on a grey; the pull is
    /// continuous for every base, and gives a grey warm lights and cool shadows.
    pub fn ramp(self, level: i8) -> Rgb {
        if level == 0 {
            return self;
        }
        let base = self.to_oklab();
        let steps = level.unsigned_abs();
        let keep = (1.0 - RAMP_LIGHTNESS_STEP).powi(i32::from(steps));
        let (l, hue) = if level > 0 {
            (1.0 - (1.0 - base.l) * keep, RAMP_WARM_HUE_DEG)
        } else {
            (base.l * keep, RAMP_COOL_HUE_DEG)
        };
        let (sin, cos) = hue.to_radians().sin_cos();
        let pull = RAMP_HUE_PULL * f32::from(steps);
        Rgb::from_oklab_in_gamut(Oklab::new(l, base.a + pull * cos, base.b + pull * sin))
    }

    /// The color `t` of the way from this one to `other`, `t` clamped to
    /// `0..=1`.
    ///
    /// Interpolated in OKLab, the space [`Rgb::ramp`] steps in, so a mix's
    /// lightness moves evenly with `t` where an sRGB mix of two hues sags
    /// through a darker, muddier middle.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        debug_assert!(!t.is_nan(), "a NaN t survives the clamp");
        Rgb::from_oklab_in_gamut(self.to_oklab().mix(other.to_oklab(), t))
    }

    /// How light this color looks, from black at 0 to white at 1: OKLab's
    /// lightness, the axis [`Rgb::ramp`] steps along and [`Rgb::mix`] runs
    /// evenly on.
    pub fn lightness(self) -> f32 {
        self.to_oklab().l
    }

    fn to_oklab(self) -> Oklab {
        Oklab::from_color(Srgb::new(self.r, self.g, self.b).into_format::<f32>())
    }

    /// The sRGB color at `c`'s lightness and hue with as much of its chroma as
    /// fits. Clipping each channel instead shifts the hue and can undo a ramp
    /// step outright: lit yellow clips back to the yellow itself.
    fn from_oklab_in_gamut(c: Oklab) -> Rgb {
        let at =
            |share: f32| LinSrgb::from_color_unclamped(Oklab::new(c.l, c.a * share, c.b * share));
        let mut share = 1.0;
        if !at(share).is_within_bounds() {
            let (mut fits, mut spills) = (0.0, 1.0);
            for _ in 0..GAMUT_BISECTION_STEPS {
                let mid = (fits + spills) / 2.0;
                if at(mid).is_within_bounds() {
                    fits = mid;
                } else {
                    spills = mid;
                }
            }
            share = fits;
        }
        let out: Srgb<u8> = Srgb::<f32>::from_linear(at(share)).into_format();
        Rgb {
            r: out.red,
            g: out.green,
            b: out.blue,
        }
    }
}

/// A single pixel: `Some(rgb)` or `None` (transparent).
pub type Pixel = Option<Rgb>;

/// How a palette key gets its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Color(Pixel),
    /// `level` steps along `of`'s ramp ([`Rgb::ramp`]).
    Ramp {
        of: char,
        level: i8,
    },
}

/// A palette of single-character keys, each with an index and a color: its
/// own, or a ramp step of another key's.
///
/// A pack holds its frames as these indices ([`Sprite::recolorable`]), so
/// replacing a key's color recolors exactly the pixels drawn with that key and
/// every ramp of it, even where another key has the same color.
#[derive(Debug, Clone, Default)]
pub struct Palette {
    /// In index order.
    entries: Vec<(char, Entry)>,
    index: HashMap<char, usize>,
}

impl Palette {
    /// A palette with no keys.
    pub fn new() -> Self {
        Self::default()
    }

    /// Map `key` to `pixel` (opaque `Some(rgb)` or transparent `None`). A key
    /// that was a ramp becomes this fixed color.
    pub fn insert(&mut self, key: char, pixel: Pixel) {
        self.set(key, Entry::Color(pixel));
    }

    /// Declare `key` as `level` steps along `of`'s ramp ([`Rgb::ramp`]). It has
    /// no color of its own, so replacing `of`'s color moves it too.
    ///
    /// It resolves only while `of` has a color of its own: a ramp of an
    /// undefined key or of another ramp is undefined, so resolution never
    /// chains or cycles.
    pub fn insert_ramp(&mut self, key: char, of: char, level: i8) {
        self.set(key, Entry::Ramp { of, level });
    }

    /// Replace `key`'s entry in place: frames hold its index, so a second slot
    /// for the key would leave them reading the old one.
    fn set(&mut self, key: char, entry: Entry) {
        match self.index.get(&key).and_then(|&i| self.entries.get_mut(i)) {
            Some(slot) => slot.1 = entry,
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push((key, entry));
            }
        }
    }

    /// Look up `key`: `None` if it is undefined, a ramp that does not resolve
    /// included ([`insert_ramp`](Self::insert_ramp)), else `Some(pixel)` (the
    /// pixel itself may be transparent).
    pub fn get(&self, key: char) -> Option<Pixel> {
        self.resolve(self.entry(key)?)
    }

    fn entry(&self, key: char) -> Option<Entry> {
        self.entries.get(self.index_of(key)?).map(|&(_, e)| e)
    }

    fn resolve(&self, entry: Entry) -> Option<Pixel> {
        match entry {
            Entry::Color(pixel) => Some(pixel),
            Entry::Ramp { of, level } => match self.entry(of)? {
                Entry::Color(pixel) => Some(pixel.map(|rgb| rgb.ramp(level))),
                Entry::Ramp { .. } => None,
            },
        }
    }

    fn index_of(&self, key: char) -> Option<usize> {
        self.index.get(&key).copied()
    }

    /// The index a frame stores for `key`, if the key resolves: decided from
    /// the entries alone, so parsing a pixel never derives a ramp's color.
    fn drawable_index(&self, key: char) -> Option<usize> {
        let index = self.index_of(key)?;
        match self.entry(key)? {
            Entry::Color(_) => Some(index),
            Entry::Ramp { of, .. } => matches!(self.entry(of)?, Entry::Color(_)).then_some(index),
        }
    }

    /// Every index's pixel: what a frame's indices resolve through. An index
    /// that does not resolve is transparent, though no parsed frame holds one.
    fn resolved(&self) -> Vec<Pixel> {
        self.entries
            .iter()
            .map(|&(_, e)| self.resolve(e).flatten())
            .collect()
    }
}

/// A palette index as a frame stores it: the indexed-color convention, so a
/// palette addresses at most [`PALETTE_CAPACITY`] keys.
type PaletteIndex = u8;

/// The most keys a palette's frames can address.
const PALETTE_CAPACITY: usize = PaletteIndex::MAX as usize + 1;

/// A frame as palette indices: how a pack holds its art, so a recolor resolves
/// the same indices through a different palette.
#[derive(Debug, Clone)]
struct IndexedFrame(Grid<PaletteIndex>);

impl IndexedFrame {
    /// The frame in `pixels`' colors, one entry per palette index.
    fn resolve(&self, pixels: &[Pixel]) -> Frame {
        let data = self
            .0
            .as_slice()
            .iter()
            .map(|&i| pixels.get(usize::from(i)).copied().flatten())
            .collect();
        Frame::from_pixels(self.0.width(), self.0.height(), data)
    }
}

/// A sprite frame: a `width × height` row-major grid of `Pixel`s.
#[derive(Debug, Clone, Default)]
pub struct Frame(Grid<Pixel>);

impl std::ops::Deref for Frame {
    type Target = Grid<Pixel>;
    fn deref(&self) -> &Grid<Pixel> {
        &self.0
    }
}

impl std::ops::DerefMut for Frame {
    fn deref_mut(&mut self) -> &mut Grid<Pixel> {
        &mut self.0
    }
}

impl Frame {
    /// Build a frame from a row-major pixel `Vec` (length = `width * height`).
    pub fn from_pixels(width: u16, height: u16, pixels: Vec<Pixel>) -> Self {
        Frame(Grid::from_vec(width, height, pixels))
    }

    /// Reverse each row — turns a right-facing sprite into a left-facing one.
    pub fn mirror_horizontal(&self) -> Self {
        let w = self.width as usize;
        let h = self.height as usize;
        let src = self.as_slice();
        let mut pixels = Vec::with_capacity(src.len());
        for y in 0..h {
            let row_start = y * w;
            for x in (0..w).rev() {
                pixels.push(src[row_start + x]);
            }
        }
        Frame::from_pixels(self.width, self.height, pixels)
    }

    /// Flip rows top-to-bottom.
    pub fn mirror_vertical(&self) -> Self {
        let w = self.width as usize;
        let h = self.height as usize;
        let src = self.as_slice();
        let mut pixels = Vec::with_capacity(src.len());
        for y in (0..h).rev() {
            let row_start = y * w;
            for x in 0..w {
                pixels.push(src[row_start + x]);
            }
        }
        Frame::from_pixels(self.width, self.height, pixels)
    }
}

/// A named point on one frame, from a `.sprite` frame's `@mark <name> <x> <y>`:
/// where a painter lays something over the art, in the frame's own pixels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mark {
    name: String,
    x: u16,
    y: u16,
}

impl Mark {
    pub(crate) fn new(name: String, x: u16, y: u16) -> Self {
        Self { name, x, y }
    }

    /// Its name: `head.<view>` is where a hairstyle is laid ([`Sprite::head`]).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its column.
    pub fn x(&self) -> u16 {
        self.x
    }

    /// Its row.
    pub fn y(&self) -> u16 {
        self.y
    }
}

/// Which way a character frame's head faces the viewer: the view a hairstyle
/// draws its layers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum HeadView {
    /// The face toward the viewer.
    Front,
    /// The back of the head toward the viewer.
    Back,
    /// In profile.
    Side,
    /// Seen from above, face down.
    Crown,
}

impl HeadView {
    /// Every view.
    pub const ALL: [HeadView; 4] = [Self::Front, Self::Back, Self::Side, Self::Crown];

    /// The name a head mark (`head.<name>`) and a `[hairstyles]` table call it by.
    pub fn name(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
            Self::Side => "side",
            Self::Crown => "crown",
        }
    }

    /// The view called `name`, if there is one.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }

    /// Its place in [`HeadView::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// A frame's head: which way it faces and the point a hairstyle layer's own
/// head is laid on, read from the frame's one `head.<view>` [`Mark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct HeadMark {
    /// Which way the head faces.
    pub view: HeadView,
    /// The mark's column.
    pub x: u16,
    /// The mark's row.
    pub y: u16,
}

impl HeadMark {
    /// The head `mark` places, where it is a `head.<view>` mark.
    pub fn of(mark: &Mark) -> Option<Self> {
        let view = HeadView::from_name(mark.name().strip_prefix(HEAD_MARK)?)?;
        Some(Self {
            view,
            x: mark.x,
            y: mark.y,
        })
    }
}

/// What a head mark's name starts with, before its view.
pub(crate) const HEAD_MARK: &str = "head.";

/// An animation: its frames in order, the palette indices they were drawn
/// with, each frame's marks, and the per-frame hold time.
#[derive(Debug, Clone)]
pub struct Sprite {
    /// The frames in their own palette's colors.
    frames: Vec<Frame>,
    /// The same frames as indices into `palette`, for a recolor to resolve again.
    indexed: Vec<IndexedFrame>,
    /// Each frame's `@mark`s.
    marks: Vec<Vec<Mark>>,
    /// The palette `indexed` refers to: the sprite's own pack's, which it keeps
    /// when a custom pack inherits it, so a recolor never reads its indices
    /// through another pack's keys.
    palette: Arc<Palette>,
    frame_ms: u32,
}

impl Sprite {
    fn new(marked: Vec<(IndexedFrame, Vec<Mark>)>, palette: Arc<Palette>, frame_ms: u32) -> Self {
        let pixels = palette.resolved();
        let (indexed, marks): (Vec<_>, Vec<_>) = marked.into_iter().unzip();
        let frames = indexed.iter().map(|f| f.resolve(&pixels)).collect();
        Sprite {
            frames,
            indexed,
            marks,
            palette,
            frame_ms,
        }
    }

    /// Frame `idx`'s marks, in the order the file names them.
    pub fn marks(&self, idx: usize) -> &[Mark] {
        self.marks.get(idx).map_or(&[], Vec::as_slice)
    }

    /// Frame `idx`'s head: where a hairstyle dresses it.
    pub fn head(&self, idx: usize) -> Option<HeadMark> {
        self.marks(idx).iter().find_map(HeadMark::of)
    }

    /// The frames, played in order.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// How long each frame holds before advancing, in milliseconds.
    pub fn frame_ms(&self) -> u32 {
        self.frame_ms
    }

    /// Frame `idx` as the palette indices a recolor resolves; `None` past the
    /// last frame.
    pub fn recolorable(&self, idx: usize) -> Option<RecolorableFrame<'_>> {
        Some(RecolorableFrame {
            indexed: self.indexed.get(idx)?,
            palette: &self.palette,
        })
    }
}

/// One frame of a [`Sprite`] as palette indices, from [`Sprite::recolorable`].
#[derive(Debug, Clone, Copy)]
pub struct RecolorableFrame<'a> {
    indexed: &'a IndexedFrame,
    palette: &'a Palette,
}

impl RecolorableFrame<'_> {
    /// The frame with each `(key, pixel)` of `overrides` replacing that key's
    /// color, and so every ramp of it.
    ///
    /// An override replaces a color and never adds one: a key the palette
    /// lacks or holds transparent stays as it is, so a pack that leaves a part
    /// out (a robot with no hair) keeps it out for every agent.
    pub fn recolored(&self, overrides: &[(char, Pixel)]) -> Frame {
        let mut palette = self.palette.clone();
        for &(key, pixel) in overrides {
            if let Some(Some(_)) = palette.get(key) {
                palette.insert(key, pixel);
            }
        }
        self.indexed.resolve(&palette.resolved())
    }
}

/// A flat RGB buffer used as a blit target.
#[derive(Debug, Clone)]
pub struct RgbBuffer {
    pixels: Grid<Rgb>,
    writes: Option<Writes>,
}

/// A write epoch, only ever minted by [`RgbBuffer::begin_writes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteEpoch(u32);

/// Each pixel's epoch of its last noted write.
#[derive(Debug, Clone)]
struct Writes {
    at: Vec<u32>,
    now: u32,
    tracking: bool,
}

impl Writes {
    fn note(&mut self, i: usize) {
        if self.tracking {
            self.at[i] = self.now;
        }
    }

    fn note_all(&mut self) {
        if self.tracking {
            self.at.fill(self.now);
        }
    }
}

impl std::ops::Deref for RgbBuffer {
    type Target = Grid<Rgb>;
    fn deref(&self) -> &Grid<Rgb> {
        &self.pixels
    }
}

impl RgbBuffer {
    /// A `width × height` buffer with every pixel set to `fill`.
    pub fn filled(width: u16, height: u16, fill: Rgb) -> Self {
        RgbBuffer {
            pixels: Grid::filled(width, height, fill),
            writes: None,
        }
    }

    /// Build from a row-major `Vec<Rgb>` (length = `width * height`).
    pub fn from_pixels(width: u16, height: u16, pixels: Vec<Rgb>) -> Self {
        RgbBuffer {
            pixels: Grid::from_vec(width, height, pixels),
            writes: None,
        }
    }

    #[inline]
    fn raw_index(&self, x: u16, y: u16) -> usize {
        (y as usize) * (self.pixels.width as usize) + (x as usize)
    }

    /// [`raw_index`](Self::raw_index) guarded by a debug-only bounds assert: a
    /// stray `x >= width` would silently read/write the WRONG row rather than
    /// fault. Unchecked in release (the hot path).
    #[inline]
    fn checked_index(&self, x: u16, y: u16) -> usize {
        debug_assert!(
            x < self.pixels.width && y < self.pixels.height,
            "RgbBuffer index out of bounds: ({x},{y}) in {}x{}",
            self.pixels.width,
            self.pixels.height
        );
        self.raw_index(x, y)
    }

    /// Read the `Rgb` at `(x, y)`. Debug-asserts the point is in bounds;
    /// unchecked in release (the hot blit path clips first).
    pub fn get(&self, x: u16, y: u16) -> Rgb {
        self.pixels.as_slice()[self.checked_index(x, y)]
    }

    /// Write `rgb` at `(x, y)`. Debug-asserts the point is in bounds; use
    /// [`put_checked`](Self::put_checked) when `(x, y)` may fall outside.
    pub fn put(&mut self, x: u16, y: u16, rgb: Rgb) {
        let i = self.checked_index(x, y);
        self.write(i, rgb);
    }

    /// Bounds-checked write: a no-op when `(x, y)` falls outside the buffer.
    /// THE clip primitive for per-pixel scatter (glyphs, particles) that can't
    /// pre-clip; the hot blit path clips its loop bounds once and keeps the
    /// unchecked [`put`](Self::put).
    pub fn put_checked(&mut self, x: u16, y: u16, rgb: Rgb) {
        if x < self.pixels.width && y < self.pixels.height {
            let i = self.raw_index(x, y);
            self.write(i, rgb);
        }
    }

    /// Every pixel, row-major, to write in bulk. While
    /// [`begin_writes`](Self::begin_writes) tracks, the whole buffer counts as
    /// written this epoch, since a bulk write may touch any of it.
    pub fn as_mut_slice(&mut self) -> &mut [Rgb] {
        if let Some(w) = &mut self.writes {
            w.note_all();
        }
        self.pixels.as_mut_slice()
    }

    /// Resize to `width × height` with every pixel `fill`, which counts as
    /// writing every pixel.
    pub fn resize_fill(&mut self, width: u16, height: u16, fill: Rgb) {
        let reshaped = (width, height) != (self.pixels.width, self.pixels.height);
        self.pixels.resize_fill(width, height, fill);
        if let Some(w) = &mut self.writes {
            if reshaped {
                w.at.clear();
                w.at.resize(self.pixels.as_slice().len(), 0);
            }
            w.note_all();
        }
    }

    fn write(&mut self, i: usize, rgb: Rgb) {
        self.pixels.as_mut_slice()[i] = rgb;
        if let Some(w) = &mut self.writes {
            w.note(i);
        }
    }

    /// Start a write epoch and return it, for [`written_in`](Self::written_in):
    /// unlike a diff, it sees a pixel written in the colour already there.
    pub fn begin_writes(&mut self) -> WriteEpoch {
        let n = self.pixels.as_slice().len();
        let w = self.writes.get_or_insert_with(|| Writes {
            at: vec![0; n],
            now: 0,
            tracking: false,
        });
        w.tracking = true;
        if w.at.len() != n {
            w.at = vec![0; n];
        }
        w.now = w.now.wrapping_add(1);
        if w.now == 0 {
            w.at.fill(0);
            w.now = 1;
        }
        WriteEpoch(w.now)
    }

    /// Stop noting writes until the next [`begin_writes`](Self::begin_writes);
    /// epochs already noted still answer [`written_in`](Self::written_in).
    pub fn end_writes(&mut self) {
        if let Some(w) = &mut self.writes {
            w.tracking = false;
        }
    }

    /// Whether `(x, y)` was written in `epoch` ([`begin_writes`](Self::begin_writes)).
    pub fn written_in(&self, x: u16, y: u16, epoch: WriteEpoch) -> bool {
        x < self.pixels.width
            && y < self.pixels.height
            && self
                .writes
                .as_ref()
                .is_some_and(|w| w.at.get(self.raw_index(x, y)) == Some(&epoch.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn a_bulk_write_counts_as_writing_every_pixel() {
        let mut buf = RgbBuffer::filled(3, 2, rgb(0, 0, 0));
        let epoch = buf.begin_writes();
        assert!(!buf.written_in(1, 1, epoch));
        buf.as_mut_slice()[0] = rgb(9, 9, 9);
        assert!((0..2).all(|y| (0..3).all(|x| buf.written_in(x, y, epoch))));
        let epoch = buf.begin_writes();
        buf.resize_fill(4, 4, rgb(1, 1, 1));
        assert!(buf.written_in(3, 3, epoch), "a resize writes every pixel");
    }

    #[test]
    fn a_write_after_end_writes_leaves_the_last_epoch_standing() {
        let mut buf = RgbBuffer::filled(2, 1, rgb(0, 0, 0));
        let epoch = buf.begin_writes();
        buf.put(0, 0, rgb(9, 9, 9));
        buf.end_writes();
        buf.put(1, 0, rgb(9, 9, 9));
        assert!(buf.written_in(0, 0, epoch));
        assert!(
            !buf.written_in(1, 0, epoch),
            "a write after end_writes is not noted"
        );
    }

    #[test]
    fn a_reshape_forgets_epochs_noted_in_the_old_layout() {
        let mut buf = RgbBuffer::filled(4, 2, rgb(0, 0, 0));
        let epoch = buf.begin_writes();
        buf.put(0, 1, rgb(9, 9, 9));
        buf.end_writes();
        buf.resize_fill(2, 4, rgb(0, 0, 0));
        assert!(!(0..4).any(|y| (0..2).any(|x| buf.written_in(x, y, epoch))));
    }

    #[test]
    fn an_epoch_past_the_last_never_reads_an_unwritten_pixel_as_written() {
        let mut buf = RgbBuffer::filled(2, 1, rgb(0, 0, 0));
        buf.begin_writes();
        buf.put(0, 0, rgb(9, 9, 9));
        if let Some(w) = &mut buf.writes {
            w.now = u32::MAX;
        }
        let epoch = buf.begin_writes();
        assert!(!buf.written_in(0, 0, epoch));
        assert!(!buf.written_in(1, 0, epoch));
    }

    /// Mid grey, dark hair, skin, and red, blue and yellow, which a lit step
    /// pushes out of gamut; black and white are left out, having no darker or
    /// lighter.
    const RAMP_BASES: [Rgb; 6] = [
        rgb(128, 128, 128),
        rgb(42, 26, 14),
        rgb(232, 180, 138),
        rgb(255, 0, 0),
        rgb(0, 0, 255),
        rgb(255, 255, 0),
    ];

    /// Also catches a clipped out-of-gamut step ([`Rgb::from_oklab_in_gamut`]).
    #[test]
    fn every_ramp_level_a_pack_may_declare_is_lighter_than_the_one_below() {
        let max = format::MAX_RAMP_LEVEL;
        for base in RAMP_BASES {
            assert_eq!(base.ramp(0), base);
            let lightness: Vec<f32> = (-max..=max).map(|n| base.ramp(n).lightness()).collect();
            assert!(
                lightness.windows(2).all(|w| w[0] < w[1]),
                "{base:?}: {lightness:?}"
            );
        }
    }

    #[test]
    fn a_write_is_noted_in_its_epoch_even_in_the_colour_already_there() {
        let grey = rgb(128, 128, 128);
        let mut buf = RgbBuffer::filled(4, 2, grey);
        let first = buf.begin_writes();
        buf.put(1, 0, grey);
        buf.put_checked(9, 9, grey);
        assert!(buf.written_in(1, 0, first));
        assert!(!buf.written_in(2, 0, first));
        assert!(!buf.written_in(9, 9, first));
        let second = buf.begin_writes();
        assert!(
            !buf.written_in(1, 0, second),
            "an epoch sees only its own writes"
        );
        buf.put(3, 1, grey);
        assert!(buf.written_in(3, 1, second));
    }

    #[test]
    fn a_mix_runs_from_one_color_to_the_other_evenly_in_lightness() {
        let (navy, amber) = (rgb(18, 26, 52), rgb(252, 215, 110));
        assert_eq!(navy.mix(amber, 0.0), navy);
        assert_eq!(navy.mix(amber, 1.0), amber);
        assert_eq!(navy.mix(amber, -1.0), navy, "t clamps below");
        assert_eq!(navy.mix(amber, 2.0), amber, "t clamps above");
        let l = |t: f32| navy.mix(amber, t).lightness();
        let (l0, l1) = (l(0.0), l(1.0));
        for t in [0.25, 0.5, 0.75] {
            let even = l0 + (l1 - l0) * t;
            assert!((l(t) - even).abs() < 0.01, "t={t}: {} vs {even}", l(t));
        }
    }

    #[test]
    fn lightness_runs_from_black_to_white_as_the_eye_sees_it() {
        assert!(rgb(0, 0, 0).lightness().abs() < 1e-4);
        assert!((rgb(255, 255, 255).lightness() - 1.0).abs() < 1e-4);
        assert!(
            rgb(0, 0, 255).lightness() < rgb(0, 160, 0).lightness(),
            "perceived, not a channel sum: pure blue is darker than a dimmer green"
        );
    }

    #[test]
    fn a_ramp_warms_its_lights_and_cools_its_shadows_even_on_a_grey() {
        let grey = rgb(128, 128, 128);
        let (lit, shaded) = (grey.ramp(1), grey.ramp(-1));
        assert!(lit.r > lit.b, "lit grey should lean warm: {lit:?}");
        assert!(
            shaded.b > shaded.r,
            "shaded grey should lean cool: {shaded:?}"
        );
    }

    /// Blue to yellow leaves the sRGB gamut just past blue: the mix gives up
    /// chroma and keeps the hue it interpolated, where clipping each channel
    /// would turn it.
    #[test]
    fn a_mix_that_leaves_the_gamut_keeps_its_hue() {
        let (blue, yellow) = (rgb(0, 0, 255), rgb(255, 255, 0));
        let (from, to) = (blue.to_oklab(), yellow.to_oklab());
        let hue = |c: Oklab| c.b.atan2(c.a).to_degrees();
        for t in [0.05, 0.1] {
            let lerped = from.mix(to, t);
            assert!(
                !LinSrgb::from_color_unclamped(lerped).is_within_bounds(),
                "t={t} must leave the gamut, or this checks nothing"
            );
            let got = hue(blue.mix(yellow, t).to_oklab());
            assert!(
                (got - hue(lerped)).abs() < 0.3,
                "t={t}: hue {got} vs {}",
                hue(lerped)
            );
        }
    }

    #[test]
    fn palette_ramp_derives_from_its_base_and_follows_an_override() {
        let hair = rgb(40, 20, 10);
        let mut p = Palette::new();
        p.insert('H', Some(hair));
        p.insert_ramp('h', 'H', -1);
        assert_eq!(p.get('h'), Some(Some(hair.ramp(-1))));

        let blond = rgb(200, 160, 80);
        p.insert('H', Some(blond));
        assert_eq!(p.get('h'), Some(Some(blond.ramp(-1))));

        let picked = rgb(90, 40, 60);
        p.insert('h', Some(picked));
        assert_eq!(p.get('h'), Some(Some(picked)), "a hand-picked shade wins");
    }

    #[test]
    fn a_ramp_of_a_ramp_or_of_a_missing_key_is_undefined() {
        let mut p = Palette::new();
        p.insert('H', Some(rgb(40, 20, 10)));
        p.insert_ramp('h', 'H', -1);
        p.insert_ramp('x', 'h', -1);
        p.insert_ramp('y', 'Q', 1);
        assert_eq!(p.get('x'), None, "a ramp of a ramp");
        assert_eq!(p.get('y'), None, "a ramp of a missing key");
        assert_eq!(p.drawable_index('x'), None);
        assert_eq!(p.drawable_index('y'), None);
        assert_eq!(p.drawable_index('h'), Some(1));
    }

    #[test]
    fn palette_insert_replaces_a_keys_color_in_place() {
        let mut p = Palette::new();
        p.insert('B', Some(rgb(0, 0, 255)));
        p.insert('X', Some(rgb(9, 9, 9)));
        p.insert('B', Some(rgb(255, 0, 0)));
        assert_eq!(p.get('B'), Some(Some(rgb(255, 0, 0))));
        assert_eq!(p.index_of('B'), Some(0));
        assert_eq!(p.resolved().len(), 2);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "out of bounds")]
    fn rgbbuffer_get_out_of_bounds_panics_in_debug() {
        let b = RgbBuffer::filled(4, 4, Rgb { r: 0, g: 0, b: 0 });
        let _ = b.get(4, 0);
    }

    #[test]
    fn mirror_horizontal_reverses_each_row() {
        let f = Frame::from_pixels(
            3,
            2,
            vec![
                Some(Rgb { r: 1, g: 0, b: 0 }),
                None,
                Some(Rgb { r: 2, g: 0, b: 0 }),
                Some(Rgb { r: 3, g: 0, b: 0 }),
                Some(Rgb { r: 4, g: 0, b: 0 }),
                None,
            ],
        );
        let m = f.mirror_horizontal();
        assert_eq!(m.width, 3);
        assert_eq!(m.height, 2);
        assert_eq!(
            m.as_slice(),
            vec![
                Some(Rgb { r: 2, g: 0, b: 0 }),
                None,
                Some(Rgb { r: 1, g: 0, b: 0 }),
                None,
                Some(Rgb { r: 4, g: 0, b: 0 }),
                Some(Rgb { r: 3, g: 0, b: 0 }),
            ]
        );
    }

    #[test]
    fn rgb_buffer_put_get_roundtrip() {
        let mut b = RgbBuffer::filled(3, 2, Rgb { r: 0, g: 0, b: 0 });
        b.put(
            1,
            1,
            Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
        );
        assert_eq!(
            b.get(1, 1),
            Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
        assert_eq!(b.get(0, 0), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn put_checked_writes_in_bounds_and_noops_out_of_bounds() {
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let fg = Rgb { r: 9, g: 8, b: 7 };
        let mut b = RgbBuffer::filled(3, 2, bg);
        b.put_checked(2, 1, fg);
        assert_eq!(b.get(2, 1), fg);
        for (x, y) in [(3, 0), (0, 2), (3, 2), (99, 99)] {
            b.put_checked(x, y, fg);
        }
        for y in 0..2 {
            for x in 0..3 {
                let want = if (x, y) == (2, 1) { fg } else { bg };
                assert_eq!(b.get(x, y), want, "cell ({x},{y})");
            }
        }
    }
}
