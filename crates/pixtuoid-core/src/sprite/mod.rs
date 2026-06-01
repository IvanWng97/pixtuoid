use std::collections::HashMap;

pub mod animator;
pub mod blit;
pub mod format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

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

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    /// Row-major, length = width * height.
    pub pixels: Vec<Pixel>,
}

impl Frame {
    /// Reverse each row in place — turns a right-facing sprite into a
    /// left-facing one. Cheap (single pass, no reallocation when called
    /// repeatedly on a buffer reuse pattern).
    pub fn mirror_horizontal(&self) -> Self {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in 0..h {
            let row_start = y * w;
            for x in (0..w).rev() {
                pixels.push(self.pixels[row_start + x]);
            }
        }
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Flip rows top-to-bottom. Used to face a couch the opposite way
    /// (e.g. for a meeting room with two sofas facing each other).
    pub fn mirror_vertical(&self) -> Self {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in (0..h).rev() {
            let row_start = y * w;
            for x in 0..w {
                pixels.push(self.pixels[row_start + x]);
            }
        }
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
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
pub struct RgbBuffer {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<Rgb>,
}

impl RgbBuffer {
    pub fn filled(width: u16, height: u16, fill: Rgb) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill; (width as usize) * (height as usize)],
        }
    }

    pub fn get(&self, x: u16, y: u16) -> Rgb {
        self.pixels[(y as usize) * (self.width as usize) + (x as usize)]
    }

    pub fn put(&mut self, x: u16, y: u16, rgb: Rgb) {
        let i = (y as usize) * (self.width as usize) + (x as usize);
        self.pixels[i] = rgb;
    }

    /// Resize and fill in one shot, reusing the existing `pixels` allocation
    /// when possible. Cheaper than `RgbBuffer::filled(...)` once per frame.
    pub fn ensure_size(&mut self, width: u16, height: u16, fill: Rgb) {
        let total = (width as usize) * (height as usize);
        if self.width == width && self.height == height {
            for p in &mut self.pixels {
                *p = fill;
            }
            return;
        }
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(total, fill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_get_and_override() {
        let mut p = Palette::new();
        p.insert('B', Some(Rgb(0, 0, 255)));
        assert_eq!(p.get('B'), Some(Some(Rgb(0, 0, 255))));
        let p2 = p.with_override('B', Some(Rgb(255, 0, 0)));
        assert_eq!(p2.get('B'), Some(Some(Rgb(255, 0, 0))));
        assert_eq!(p.get('B'), Some(Some(Rgb(0, 0, 255))));
    }

    #[test]
    fn mirror_horizontal_reverses_each_row() {
        let f = Frame {
            width: 3,
            height: 2,
            pixels: vec![
                Some(Rgb(1, 0, 0)),
                None,
                Some(Rgb(2, 0, 0)),
                Some(Rgb(3, 0, 0)),
                Some(Rgb(4, 0, 0)),
                None,
            ],
        };
        let m = f.mirror_horizontal();
        assert_eq!(m.width, 3);
        assert_eq!(m.height, 2);
        assert_eq!(
            m.pixels,
            vec![
                Some(Rgb(2, 0, 0)),
                None,
                Some(Rgb(1, 0, 0)),
                None,
                Some(Rgb(4, 0, 0)),
                Some(Rgb(3, 0, 0)),
            ]
        );
    }

    #[test]
    fn rgb_buffer_put_get_roundtrip() {
        let mut b = RgbBuffer::filled(3, 2, Rgb(0, 0, 0));
        b.put(1, 1, Rgb(10, 20, 30));
        assert_eq!(b.get(1, 1), Rgb(10, 20, 30));
        assert_eq!(b.get(0, 0), Rgb(0, 0, 0));
    }
}
