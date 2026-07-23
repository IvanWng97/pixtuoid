use std::collections::HashMap;

use crate::grid::Grid;

/// Compositing a `Frame` onto an `RgbBuffer` (`blit_frame`), skipping
/// transparent pixels.
pub mod blit;
/// Sprite-pack file format: `pack.toml` + `.sprite` parsing, the recolor-key
/// palette guard, and pack loading.
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

/// A single pixel: `Some(rgb)` or `None` (transparent).
pub type Pixel = Option<Rgb>;

/// A map from single-character frame codes to pixels (opaque `Rgb` or
/// transparent).
#[derive(Debug, Clone, Default)]
pub struct Palette {
    map: HashMap<char, Pixel>,
}

impl Palette {
    /// A palette with no keys.
    pub fn new() -> Self {
        Self::default()
    }

    /// Map `key` to `pixel` (opaque `Some(rgb)` or transparent `None`).
    pub fn insert(&mut self, key: char, pixel: Pixel) {
        self.map.insert(key, pixel);
    }

    /// Look up `key`: `None` if the key is undefined, else `Some(pixel)` (the
    /// pixel itself may be transparent).
    pub fn get(&self, key: char) -> Option<Pixel> {
        self.map.get(&key).copied()
    }

    /// Iterate `(key, pixel)` pairs. Lets callers assert palette invariants —
    /// notably that every key maps to a DISTINCT RGB, since `recolor_frame`
    /// substitutes by RGB equality and two keys sharing a color would be
    /// indistinguishable.
    pub fn iter(&self) -> impl Iterator<Item = (char, Pixel)> + '_ {
        self.map.iter().map(|(&k, &p)| (k, p))
    }

    /// Replace one palette key's color — used for per-agent recoloring.
    pub fn with_override(&self, key: char, pixel: Pixel) -> Self {
        let mut out = self.clone();
        out.map.insert(key, pixel);
        out
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

    /// Reverse each row in place — turns a right-facing sprite into a
    /// left-facing one. Cheap (single pass, no reallocation when called
    /// repeatedly on a buffer reuse pattern).
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

    /// Flip rows top-to-bottom. Used to face a couch the opposite way
    /// (e.g. for a meeting room with two sofas facing each other).
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

/// An animation: an ordered list of frames plus the per-frame hold time.
#[derive(Debug, Clone)]
pub struct Sprite {
    /// The frames, played in order.
    pub frames: Vec<Frame>,
    /// How long each frame holds before advancing, in milliseconds.
    pub frame_ms: u32,
}

/// A flat RGB buffer used as a blit target. Alpha is ignored — transparent
/// pixels leave the underlying buffer unchanged.
#[derive(Debug, Clone)]
pub struct RgbBuffer(Grid<Rgb>);

impl std::ops::Deref for RgbBuffer {
    type Target = Grid<Rgb>;
    fn deref(&self) -> &Grid<Rgb> {
        &self.0
    }
}

impl std::ops::DerefMut for RgbBuffer {
    fn deref_mut(&mut self) -> &mut Grid<Rgb> {
        &mut self.0
    }
}

impl RgbBuffer {
    /// A `width × height` buffer with every pixel set to `fill`.
    pub fn filled(width: u16, height: u16, fill: Rgb) -> Self {
        RgbBuffer(Grid::filled(width, height, fill))
    }

    /// Build from a row-major `Vec<Rgb>` (length = `width * height`).
    pub fn from_pixels(width: u16, height: u16, pixels: Vec<Rgb>) -> Self {
        RgbBuffer(Grid::from_vec(width, height, pixels))
    }

    /// The row-major flat index — the ONE `y*w + x` formula `get`/`put`/
    /// `put_checked` share (a second copy is the drift bug the sprite guide warns
    /// about). No bounds check: callers that can't guarantee `(x, y)` in range
    /// use [`checked_index`](Self::checked_index) or [`put_checked`](Self::put_checked).
    #[inline]
    fn raw_index(&self, x: u16, y: u16) -> usize {
        (y as usize) * (self.0.width as usize) + (x as usize)
    }

    /// [`raw_index`](Self::raw_index) guarded by a debug-only bounds assert — a
    /// stray `x >= width` would silently read/write the WRONG row rather than
    /// fault, so catch it in debug/tests. Unchecked in release (the hot path).
    #[inline]
    fn checked_index(&self, x: u16, y: u16) -> usize {
        debug_assert!(
            x < self.0.width && y < self.0.height,
            "RgbBuffer index out of bounds: ({x},{y}) in {}x{}",
            self.0.width,
            self.0.height
        );
        self.raw_index(x, y)
    }

    /// Read the `Rgb` at `(x, y)`. Debug-asserts the point is in bounds;
    /// unchecked in release (the hot blit path clips first).
    pub fn get(&self, x: u16, y: u16) -> Rgb {
        // Unchecked index in release (every caller clips first); this is a
        // public primitive the v2 PNG/web renderers are meant to reuse.
        self.0.as_slice()[self.checked_index(x, y)]
    }

    /// Write `rgb` at `(x, y)`. Debug-asserts the point is in bounds; unchecked
    /// in release. Use [`put_checked`](Self::put_checked) when `(x, y)` may fall
    /// outside the buffer.
    pub fn put(&mut self, x: u16, y: u16, rgb: Rgb) {
        let i = self.checked_index(x, y);
        self.0.as_mut_slice()[i] = rgb;
    }

    /// Bounds-checked write: a no-op when `(x, y)` falls outside the buffer.
    /// THE clip primitive for procedural painters, which would otherwise each
    /// guard the unchecked [`put`](Self::put) with `if x < width && y < height`
    /// (the idiom this collapses at ~20 sites). Deliberately distinct from
    /// `put` — the hot blit path clips its loop bounds once and keeps the
    /// unchecked write; per-pixel scatter (glyphs, particles, coffee cups) that
    /// can't pre-clip uses this. The v2 PNG/web renderers reuse it too.
    pub fn put_checked(&mut self, x: u16, y: u16, rgb: Rgb) {
        if x < self.0.width && y < self.0.height {
            let i = self.raw_index(x, y);
            self.0.as_mut_slice()[i] = rgb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_get_and_override() {
        let mut p = Palette::new();
        p.insert('B', Some(Rgb { r: 0, g: 0, b: 255 }));
        assert_eq!(p.get('B'), Some(Some(Rgb { r: 0, g: 0, b: 255 })));
        let p2 = p.with_override('B', Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(p2.get('B'), Some(Some(Rgb { r: 255, g: 0, b: 0 })));
        assert_eq!(p.get('B'), Some(Some(Rgb { r: 0, g: 0, b: 255 })));
    }

    // The unchecked get/put index would silently read/write the WRONG row on a
    // stray out-of-range x (x >= width with small y maps into an earlier row);
    // the debug_assert turns that into a loud fault under test.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "out of bounds")]
    fn rgbbuffer_get_out_of_bounds_panics_in_debug() {
        let b = RgbBuffer::filled(4, 4, Rgb { r: 0, g: 0, b: 0 });
        let _ = b.get(4, 0); // x == width
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
        // In bounds: writes, exactly like `put`.
        b.put_checked(2, 1, fg);
        assert_eq!(b.get(2, 1), fg);
        // Out of bounds on either axis (and both): silent no-op, no panic — the
        // clip contract the ~20 guard closures relied on. `get` would debug-panic
        // here, so probe the buffer's unchanged state instead of reading OOB.
        for (x, y) in [(3, 0), (0, 2), (3, 2), (99, 99)] {
            b.put_checked(x, y, fg);
        }
        // The only written cell is (2,1); every other cell is still `bg`.
        for y in 0..2 {
            for x in 0..3 {
                let want = if (x, y) == (2, 1) { fg } else { bg };
                assert_eq!(b.get(x, y), want, "cell ({x},{y})");
            }
        }
    }
}
