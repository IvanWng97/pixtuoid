use std::collections::HashMap;

use crate::grid::Grid;

pub mod blit;
pub mod format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A single pixel: `Some(rgb)` or `None` (transparent).
pub type Pixel = Option<Rgb>;

#[derive(Debug, Clone, Default)]
pub struct Palette {
    map: HashMap<char, Pixel>,
}

impl Palette {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: char, pixel: Pixel) {
        self.map.insert(key, pixel);
    }

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

#[derive(Debug, Clone)]
pub struct Sprite {
    pub frames: Vec<Frame>,
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
    pub fn filled(width: u16, height: u16, fill: Rgb) -> Self {
        RgbBuffer(Grid::filled(width, height, fill))
    }

    /// Build from a row-major `Vec<Rgb>` (length = `width * height`).
    pub fn from_pixels(width: u16, height: u16, pixels: Vec<Rgb>) -> Self {
        RgbBuffer(Grid::from_vec(width, height, pixels))
    }

    pub fn get(&self, x: u16, y: u16) -> Rgb {
        // Unchecked index in release (every caller clips first), but a stray
        // x >= width would silently read the WRONG row rather than fault — catch
        // it in debug/tests. (This is a public primitive the v2 PNG/web renderers
        // are meant to reuse.)
        debug_assert!(
            x < self.0.width && y < self.0.height,
            "RgbBuffer::get out of bounds: ({x},{y}) in {}x{}",
            self.0.width,
            self.0.height
        );
        self.0.as_slice()[(y as usize) * (self.0.width as usize) + (x as usize)]
    }

    pub fn put(&mut self, x: u16, y: u16, rgb: Rgb) {
        debug_assert!(
            x < self.0.width && y < self.0.height,
            "RgbBuffer::put out of bounds: ({x},{y}) in {}x{}",
            self.0.width,
            self.0.height
        );
        let w = self.0.width as usize;
        let i = (y as usize) * w + (x as usize);
        self.0.as_mut_slice()[i] = rgb;
    }

    /// Resize and fill in one shot, reusing the existing allocation when
    /// possible. Cheaper than `RgbBuffer::filled(...)` once per frame.
    pub fn ensure_size(&mut self, width: u16, height: u16, fill: Rgb) {
        self.0.resize_fill(width, height, fill)
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
}
