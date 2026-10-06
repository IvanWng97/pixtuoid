//! The frame-pacing bench's way into the real TUI painter: a renderer over
//! ratatui's own crossterm encoder and, for an image protocol, the real tile
//! sink, both writing into a [`Wire`] that counts bytes and the time spent
//! writing them. Mechanism for `examples/pacing.rs`, not a contract.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use pixtuoid_core::sprite::format::Pack;
use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::layout::{Position, Rect, Size};

use crate::graphics::{CellSize, Fit, ImageProtocol};
use crate::tui::tui_renderer::TuiRenderer;

/// How the bench's office reaches the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// The classic, in half-block cells.
    HalfBlock,
    /// The cutaway over the kitty graphics protocol.
    Kitty,
    /// The cutaway over SIXEL.
    Sixel,
    /// The cutaway over iTerm2's inline images.
    Iterm2,
}

impl Protocol {
    /// Every way the bench's office reaches a terminal, in the report's order.
    pub const ALL: [Protocol; 4] = [
        Protocol::HalfBlock,
        Protocol::Kitty,
        Protocol::Sixel,
        Protocol::Iterm2,
    ];

    /// The image protocol the cutaway rides; `None` for the half-blocks.
    fn image(self) -> Option<ImageProtocol> {
        match self {
            Protocol::HalfBlock => None,
            Protocol::Kitty => Some(ImageProtocol::Kitty),
            Protocol::Sixel => Some(ImageProtocol::Sixel),
            Protocol::Iterm2 => Some(ImageProtocol::Iterm2),
        }
    }

    /// Its name in the report: the image protocol's own, or `half-block`.
    pub fn name(self) -> &'static str {
        self.image().map_or("half-block", ImageProtocol::name)
    }
}

/// What a [`Wire`] has carried: bytes, and the time spent writing them.
#[derive(Debug, Default)]
struct Carried {
    bytes: AtomicU64,
    write_ns: AtomicU64,
}

/// A writer that discards what it is given, counting its bytes and timing each
/// write and flush on the monotonic clock. Clones share one count.
#[derive(Debug, Clone, Default)]
pub struct Wire(Arc<Carried>);

impl Wire {
    /// Bytes and write nanoseconds since the last take, zeroing both.
    pub fn take(&self) -> (u64, u64) {
        (
            self.0.bytes.swap(0, Ordering::Relaxed),
            self.0.write_ns.swap(0, Ordering::Relaxed),
        )
    }

    fn timed<T>(&self, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        let ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.0.write_ns.fetch_add(ns, Ordering::Relaxed);
        out
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.timed(|| {
            self.0.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
            std::hint::black_box(buf);
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.timed(|| ());
        Ok(())
    }
}

/// A terminal `size` cells big whose cells are `cell` pixels, encoding through
/// ratatui's crossterm backend into a [`Wire`]: nothing asks a real terminal.
#[derive(Debug)]
pub struct PacedBackend {
    inner: CrosstermBackend<Wire>,
    size: Size,
    cell: CellSize,
    cursor: Position,
}

impl PacedBackend {
    /// Resize the terminal to `cols`×`rows`, as a window drag does.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.size = Size::new(cols, rows);
    }
}

impl Backend for PacedBackend {
    type Error = std::io::Error;
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.inner.draw(content)
    }
    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        Ok(self.cursor)
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        let position = position.into();
        self.cursor = position;
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> std::io::Result<Size> {
        Ok(self.size)
    }
    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::new(
                self.size.width * self.cell.w,
                self.size.height * self.cell.h,
            ),
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

/// A renderer drawing the office over `protocol` into a `cols`×`rows`
/// terminal of `cell_px` cells, with `pets`, and the wire every byte it writes
/// crosses.
pub fn renderer(
    protocol: Protocol,
    cols: u16,
    rows: u16,
    cell_px: (u16, u16),
    pets: Vec<pixtuoid_scene::pet::Pet>,
    pack: Arc<Pack>,
) -> Result<(TuiRenderer<PacedBackend>, Wire)> {
    let wire = Wire::default();
    let cell = CellSize {
        w: cell_px.0,
        h: cell_px.1,
    };
    let backend = PacedBackend {
        inner: CrosstermBackend::new(wire.clone()),
        size: Size::new(cols, rows),
        cell,
        cursor: Position::ORIGIN,
    };
    let mut r = TuiRenderer::new(
        Terminal::new(backend)?,
        &pixtuoid_scene::theme::NORMAL,
        pets,
        Arc::clone(&pack),
    );
    if let Some(image) = protocol.image() {
        let area = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows)).as_size();
        let fit = Fit::new(cell, area, pack.max_density_variant())
            .context("the cell is too small for the cutaway")?;
        r.set_cutaway(crate::tui::cutaway::TileCutaway::new(
            fit,
            cell,
            image,
            false,
            Box::new(wire.clone()),
        ));
    }
    Ok((r, wire))
}

/// The cutaway's office in logical units, and its scale, on a `cols`×`rows`
/// terminal of `cell_px` cells: `None` where the cell fits no cutaway.
pub fn cutaway_office(
    cols: u16,
    rows: u16,
    cell_px: (u16, u16),
    pack: &Pack,
) -> Option<(u16, u16, u16)> {
    let cell = CellSize {
        w: cell_px.0,
        h: cell_px.1,
    };
    let area = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows)).as_size();
    let fit = Fit::new(cell, area, pack.max_density_variant())?;
    Some((fit.logical().w, fit.logical().h, fit.scale().get()))
}

/// The last image the cutaway of `r` showed, `None` while classic paints.
pub fn cutaway_image(r: &TuiRenderer<PacedBackend>) -> Option<pixtuoid_core::sprite::RgbBuffer> {
    r.cutaway()?.shown_image().map(|(img, _)| img.clone())
}

/// Each kitty transport's cost to send the tiles of `cur` that differ from
/// `prev` (every tile when `prev` is `None`), as `r`'s cutaway cuts them.
#[cfg(unix)]
pub fn kitty_transports(
    r: &TuiRenderer<PacedBackend>,
    prev: Option<&pixtuoid_core::sprite::RgbBuffer>,
    reps: usize,
) -> Option<serde_json::Value> {
    use crate::graphics::tiles::{TileImage, Tiles};
    use pixtuoid_scene::cutaway::canvas::Dirty;
    let (cur, fitted) = r.cutaway()?.shown_image()?;
    let mut t = Tiles::new(ImageProtocol::Kitty, fitted.cell, fitted.fit);
    let tiles: Vec<_> = match prev {
        Some(prev) => {
            let first = t.changed(prev, &Dirty::All);
            t.sent(&first);
            t.changed(cur, &Dirty::All)
                .into_iter()
                .map(|c| c.tile)
                .collect()
        }
        None => t
            .changed(cur, &Dirty::All)
            .into_iter()
            .map(|c| c.tile)
            .collect(),
    };
    let median = |mut f: Box<dyn FnMut() -> usize + '_>| {
        let mut runs: Vec<(std::time::Duration, usize)> = (0..reps)
            .map(|_| {
                let at = Instant::now();
                let bytes = f();
                (at.elapsed(), bytes)
            })
            .collect();
        runs.sort();
        let (d, bytes) = runs[runs.len() / 2];
        serde_json::json!({ "ms": d.as_secs_f64() * 1e3, "bytes": bytes })
    };
    let base = 1u32;
    let cut = median(Box::new(|| {
        tiles.iter().map(|&tile| t.image(cur, tile).rgb.len()).sum()
    }));
    let images: Vec<TileImage> = tiles.iter().map(|&tile| t.image(cur, tile)).collect();
    let keys = |img: &TileImage, extra: &str| {
        format!(
            "a=T,U=1,i={},f=24,{extra}s={},v={},c={},r={},",
            base + img.tile.index,
            img.width,
            img.height,
            img.tile.cols,
            img.tile.rows
        )
    };
    let chunked = |keys: &str, payload: &[u8]| {
        let mut out = Vec::with_capacity(payload.len() + 128);
        let chunks = payload.chunks(4096);
        let last = chunks.len().saturating_sub(1);
        for (i, chunk) in chunks.enumerate() {
            out.extend_from_slice(b"\x1b_G");
            if i == 0 {
                out.extend_from_slice(keys.as_bytes());
            }
            out.extend_from_slice(format!("q=2,m={};", u8::from(i < last)).as_bytes());
            out.extend_from_slice(chunk);
            out.extend_from_slice(b"\x1b\\");
        }
        out
    };
    let zlib = median(Box::new(|| {
        images
            .iter()
            .map(|img| crate::graphics::kitty::transmit(base + img.tile.index, img, false).len())
            .sum()
    }));
    let raw = median(Box::new(|| {
        images
            .iter()
            .map(|img| {
                chunked(
                    &keys(img, ""),
                    base64_simd::STANDARD.encode_to_string(&img.rgb).as_bytes(),
                )
                .len()
            })
            .sum()
    }));
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let parallel = median(Box::new(|| {
        let per = images.len().div_ceil(threads).max(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = images
                .chunks(per)
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .map(|img| {
                                crate::graphics::kitty::transmit(base + img.tile.index, img, false)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .map(|b| b.len())
                .sum()
        })
    }));
    // One shm object per image: what t=s costs the sender. The terminal
    // unlinks it after reading; here the sender does, timed apart.
    let pid = std::process::id();
    let shm_put = |n: usize, rgb: &[u8]| -> Option<(String, std::time::Duration)> {
        let name = format!("/pxt{pid}_{n}");
        let c = std::ffi::CString::new(name.clone()).ok()?;
        // SAFETY: plain POSIX shm calls on a name this process made, a length
        // it sized, unmapped and closed before return.
        unsafe {
            let fd = libc::shm_open(
                c.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
                0o600,
            );
            if fd < 0 {
                return None;
            }
            if libc::ftruncate(fd, rgb.len() as libc::off_t) != 0 {
                libc::close(fd);
                return None;
            }
            let p = libc::mmap(
                std::ptr::null_mut(),
                rgb.len(),
                libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if p == libc::MAP_FAILED {
                libc::close(fd);
                return None;
            }
            std::ptr::copy_nonoverlapping(rgb.as_ptr(), p.cast::<u8>(), rgb.len());
            libc::munmap(p, rgb.len());
            libc::close(fd);
        }
        let at = Instant::now();
        // SAFETY: the name is this process's own object.
        unsafe { libc::shm_unlink(c.as_ptr()) };
        Some((name, at.elapsed()))
    };
    let mut unlink = std::time::Duration::ZERO;
    let shm = median(Box::new(|| {
        images
            .iter()
            .enumerate()
            .filter_map(|(n, img)| {
                let (name, u) = shm_put(n, &img.rgb)?;
                unlink += u;
                let extra = format!("t=s,S={},", img.rgb.len());
                Some(
                    chunked(
                        &keys(img, &extra),
                        base64_simd::STANDARD
                            .encode_to_string(name.as_bytes())
                            .as_bytes(),
                    )
                    .len(),
                )
            })
            .sum()
    }));
    let shm_unlink_ms = unlink.as_secs_f64() * 1e3 / reps as f64;
    // Runs of changed tiles along one tile row, each packed into one image.
    let mut runs: Vec<Vec<&TileImage>> = Vec::new();
    for img in &images {
        match runs.last_mut() {
            Some(run)
                if run.last().is_some_and(|l: &&TileImage| {
                    l.tile.row == img.tile.row && l.tile.col + l.tile.cols == img.tile.col
                }) =>
            {
                run.push(img)
            }
            _ => runs.push(vec![img]),
        }
    }
    let packed = median(Box::new(|| {
        runs.iter()
            .enumerate()
            .filter_map(|(n, run)| {
                let h = run[0].height as usize;
                let w: usize = run.iter().map(|i| i.width as usize).sum();
                let mut rgb = Vec::with_capacity(w * h * 3);
                for y in 0..h {
                    for i in run {
                        let row = i.width as usize * 3;
                        rgb.extend_from_slice(&i.rgb[y * row..(y + 1) * row]);
                    }
                }
                let (name, _) = shm_put(n, &rgb)?;
                Some(
                    64 + base64_simd::STANDARD
                        .encode_to_string(name.as_bytes())
                        .len(),
                )
            })
            .sum()
    }));
    // The art before the upscale: what a terminal that scaled each tile
    // itself would be sent.
    let up = u32::from(fitted.fit.upscale());
    let (cw, ch) = (u32::from(fitted.cell.w), u32::from(fitted.cell.h));
    let unscaled = median(Box::new(|| {
        tiles
            .iter()
            .map(|tile| {
                let x0 = u32::from(tile.col) * cw / up;
                let x1 = ((u32::from(tile.col) + u32::from(tile.cols)) * cw)
                    .div_ceil(up)
                    .min(u32::from(cur.width()));
                let y0 = u32::from(tile.row) * ch / up;
                let y1 = ((u32::from(tile.row) + u32::from(tile.rows)) * ch)
                    .div_ceil(up)
                    .min(u32::from(cur.height()));
                let mut rgb = Vec::new();
                for y in y0..y1 {
                    for x in x0..x1 {
                        let p = cur.get(x as u16, y as u16);
                        rgb.extend_from_slice(&[p.r, p.g, p.b]);
                    }
                }
                let z = miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 1);
                base64_simd::STANDARD.encode_to_string(z).len() + 64
            })
            .sum()
    }));
    // The fallback: cut and zlib per tile, split across threads.
    let parallel_cut = median(Box::new(|| {
        let per = tiles.len().div_ceil(threads).max(1);
        let t = &t;
        std::thread::scope(|s| {
            let handles: Vec<_> = tiles
                .chunks(per)
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .map(|&tile| {
                                crate::graphics::kitty::transmit(
                                    base + tile.index,
                                    &t.image(cur, tile),
                                    false,
                                )
                                .len()
                            })
                            .sum::<usize>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .sum()
        })
    }));
    Some(serde_json::json!({
        "h_zlib_direct_parallel_cut_all_in": parallel_cut,
        "tiles": tiles.len(),
        "upscaled_rgb_bytes": images.iter().map(|i| i.rgb.len()).sum::<usize>(),
        "cut": cut,
        "a_zlib_direct": zlib,
        "b_raw_direct": raw,
        "c_raw_shm_per_tile": shm,
        "c_shm_unlink_ms": shm_unlink_ms,
        "d_raw_shm_packed_runs": packed,
        "d_runs": runs.len(),
        "e_zlib_direct_parallel": parallel,
        "e_threads": threads,
        "f_unscaled_zlib_direct": unscaled,
    }))
}

#[cfg(test)]
mod tests {
    use super::{ImageProtocol, Protocol};

    /// The bench measures every protocol the cutaway speaks: a new
    /// `ImageProtocol` fails to compile here until it has a `Protocol`, whose
    /// `ALL` entry, beside the enum, is by hand.
    #[test]
    fn all_lists_every_protocol_the_cutaway_speaks() {
        let of = |image: ImageProtocol| match image {
            ImageProtocol::Kitty => Protocol::Kitty,
            ImageProtocol::Sixel => Protocol::Sixel,
            ImageProtocol::Iterm2 => Protocol::Iterm2,
        };
        for protocol in Protocol::ALL {
            if let Some(image) = protocol.image() {
                assert_eq!(of(image), protocol);
            }
        }
    }
}
