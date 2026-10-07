//! The cutaway as terminal images: a frame's changed tiles are encoded in the
//! plan's protocol and written after ratatui's flush, once the frame's text
//! cells are known.
//!
//! - kitty shows every tile through Unicode placeholders, which are ordinary
//!   text, so a modal or tooltip drawn after them takes their cells as any
//!   text would (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>,
//!   "Unicode placeholders").
//! - SIXEL and iTerm2 draw pixels at the cursor. Their cells are left out of
//!   ratatui's diff, and a tile under any text cell is withheld: text and
//!   image never share a cell. The withheld tile's other cells show the
//!   frame as half-blocks meanwhile.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use pixtuoid_core::sprite::format::Density;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_scene::cutaway::canvas::Dirty;
use pixtuoid_scene::flash::{FlashHold, FlashPhase, Flashes};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::theme::Theme;
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::{Position, Rect};

use crate::graphics::tiles::{Changed, Tile, Tiles};
use crate::graphics::{CellSize, Fit, ImageProtocol, iterm2, kitty, sixel};
use crate::tui::geometry::SceneGeometry;
use crate::tui::geometry::slide_offsets;
use crate::tui::jank::FrameSend;
use crate::tui::renderer::set_half_block;

/// A floor slide's two floors' frames at progress `t` of a
/// [`FloorTransition`](pixtuoid_scene::floor::FloorTransition).
pub(crate) struct Slide<'a> {
    pub(crate) leaving: &'a RgbBuffer,
    pub(crate) arriving: &'a RgbBuffer,
    /// What of each flashes, the leaving floor's first.
    pub(crate) flashes: Flashes,
    pub(crate) t: f32,
    pub(crate) going_down: bool,
}

/// Where the transmits go: the terminal ratatui's backend also writes to.
pub(crate) type Sink = Box<dyn Write + Send>;

/// What an image cell holds before the frame's text is drawn: a cell still
/// equal to it afterwards is image, any other is text.
const SENTINEL: &str = "\u{F8FF}";

/// What a frame's tiles are cut for: the scene's cells, each `cell` big, and
/// the office fitted over them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fitted {
    pub(crate) scene: Rect,
    pub(crate) cell: CellSize,
    pub(crate) fit: Fit,
}

impl Fitted {
    /// The image's cells, its top-left on the scene's.
    pub(crate) fn geometry(self) -> SceneGeometry {
        SceneGeometry::Cutaway {
            origin: self.scene.as_position(),
            cell: self.cell,
            scale: self.fit.scale(),
        }
    }
}

/// The cutaway's tiles, and what of them the terminal holds.
pub(crate) struct TileCutaway {
    /// The plan's cell.
    planned: CellSize,
    /// The window's cell on the first frame that read one.
    first_window: Option<CellSize>,
    density: Density,
    route: crate::graphics::Route,
    /// [`kitty::process_base`].
    base: u32,
    /// `None` while classic paints.
    fitted: Option<Fitted>,
    tiles: Tiles,
    /// What the tiles are cut from when sent: the last frame, or a slide's
    /// two composed.
    image: RgbBuffer,
    /// The last frame or slide the flash hold kept back: [`Self::image`]
    /// lags it whatever the next frame's damage.
    image_behind: bool,
    /// What the last image showed: a floor's raster diffs against its own
    /// last frame, so any other image before it — a slide, another floor —
    /// leaves the tiles differing anywhere.
    shown: Option<Shown>,
    /// The image's top-left cell.
    origin: Position,
    out: Sink,
    /// This frame's write, once staged; its tiles encoded only if no text
    /// covers them.
    pending: Option<Pending>,
    /// Set once SIXEL or iTerm2 pixels are written, for the unwind
    /// ([`crate::graphics::grid_unwind`]).
    in_grid: &'static AtomicBool,
    /// When the last transmits were written, for the protocol's cadence.
    sent_at: Option<SystemTime>,
    /// The flashes the terminal shows.
    flash: FlashHold<Flashes, Option<Fitted>>,
    /// This frame's written transmits, counted as shown once the frame
    /// lands ([`Self::landed`]).
    landing: Option<Landing>,
    /// What the last frame's transmits did, for its jank report.
    last: FrameSend,
    /// The machine's cores.
    cores: usize,
    /// Whether an audio thread is up to synthesize tracks beside the encode.
    audio: bool,
}

/// A staged write: the tiles it sends and the flashes they show, so only a
/// write that lands can show them.
struct Pending {
    tiles: Vec<Changed>,
    flashes: Flashes,
}

/// Transmits written into the frame, and the flashes they show; the cadence
/// restarts from `at` when there were any.
struct Landing {
    sent: Vec<Changed>,
    flashes: Flashes,
    at: Option<SystemTime>,
}

impl std::fmt::Debug for TileCutaway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileCutaway")
            .field("planned", &self.planned)
            .field("density", &self.density)
            .field("route", &self.route)
            .field("origin", &self.origin)
            .field("pending", &self.pending.as_ref().map(|p| p.tiles.len()))
            .field("landing", &self.landing.as_ref().map(|l| l.sent.len()))
            .finish_non_exhaustive()
    }
}

/// What a [`TileCutaway`]'s image last showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Floor(usize),
    Slide,
}

impl TileCutaway {
    /// [`crate::graphics::Plan::Cutaway`]'s parts, transmitting into `out`.
    pub(crate) fn new(fit: Fit, cell: CellSize, route: crate::graphics::Route, out: Sink) -> Self {
        Self {
            planned: cell,
            first_window: None,
            density: fit.density(),
            route,
            base: kitty::process_base(),
            fitted: None,
            tiles: Tiles::new(route.protocol(), cell, fit),
            image: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            image_behind: false,
            shown: None,
            origin: Position::ORIGIN,
            out,
            pending: None,
            in_grid: &crate::graphics::IN_GRID,
            sent_at: None,
            flash: FlashHold::on(pixtuoid_scene::flash::monotonic()),
            landing: None,
            last: FrameSend::default(),
            cores: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            audio: false,
        }
    }

    /// Fit the office over `scene`'s cells under a window whose cell reads
    /// `window`; `None` while the cell has no [`Fit`], when classic paints.
    /// Every frame fits, a refused one too: any change re-sends every tile,
    /// since a resize clears the screen ratatui redraws, a new cell cuts a new
    /// grid, and classic paints over the image.
    pub(crate) fn fit_to(&mut self, scene: Rect, window: Option<CellSize>) -> Option<Fitted> {
        let cell = self.cell_under(window);
        let Some(fit) = Fit::new(cell, scene.as_size(), self.density) else {
            self.fitted = None;
            // The classic paints this frame: it transmits nothing.
            self.last = FrameSend {
                dirty: crate::tui::jank::Painted::Classic,
                ..FrameSend::default()
            };
            return None;
        };
        let fitted = Fitted { scene, cell, fit };
        if self.fitted != Some(fitted) {
            self.tiles = Tiles::new(self.route.protocol(), cell, fit);
        }
        self.fitted = Some(fitted);
        Some(fitted)
    }

    /// The plan's cell, the terminal's answer where it gave one (as
    /// [`crate::graphics::probe`] ranks them), until a font zoom moves the
    /// window's cell from what it read on the first frame.
    fn cell_under(&mut self, window: Option<CellSize>) -> CellSize {
        if self.first_window.is_none() {
            self.first_window = window;
        }
        match window {
            Some(cell) if Some(cell) != self.first_window => cell,
            _ => self.planned,
        }
    }

    /// The office's logical extent as last fitted; `None` while classic
    /// paints.
    pub(crate) fn office(&self) -> Option<Size> {
        self.fitted.map(|f| f.fit.logical())
    }

    /// Show `floor`'s `frame`, which flashes `flash`, as `fitted`; it may
    /// differ from that floor's last only within `dirty`. Queue the tiles it
    /// changed once the protocol's cadence allows; until then they stay owed.
    /// A frame the flash hold keeps back is not sent, so the terminal, and the
    /// half-blocks of the tiles under text, keep the last.
    pub(crate) fn paint(
        &mut self,
        fitted: Fitted,
        floor: usize,
        frame: &RgbBuffer,
        dirty: Dirty,
        flash: FlashPhase,
        now: SystemTime,
    ) {
        let fresh = self.shown.replace(Shown::Floor(floor)) != Some(Shown::Floor(floor));
        let dirty = if fresh { Dirty::All } else { dirty };
        let flashes = [flash; 2];
        if self.flash.holds(flashes, Some(fitted)) {
            self.tiles.owe(&dirty);
            self.image_behind = true;
            self.pending = None;
            return;
        }
        if dirty != Dirty::Unchanged || self.image_behind {
            self.image.clone_from(frame);
            self.image_behind = false;
        }
        self.stage(&dirty, fresh, flashes, now, fitted.scene.as_position());
    }

    /// Show both floors of `slide`, composed as it places them, and queue the
    /// tiles that changed as [`Self::paint`] does.
    pub(crate) fn paint_slide(
        &mut self,
        fitted: Fitted,
        slide: Slide<'_>,
        theme: &'static Theme,
        now: SystemTime,
    ) {
        let flashes = slide.flashes;
        if self.flash.holds(flashes, Some(fitted)) {
            self.tiles.owe(&Dirty::All);
            self.image_behind = true;
            self.pending = None;
            return;
        }
        let (w, h) = (slide.leaving.width(), slide.leaving.height());
        let offsets = slide_offsets(slide.t, slide.going_down, f32::from(h));
        self.image = RgbBuffer::filled(w, h, theme.surface.bg_fallback);
        self.shown = Some(Shown::Slide);
        for (buf, dy) in [(slide.leaving, offsets.0), (slide.arriving, offsets.1)] {
            for y in 0..h {
                let src = i32::from(y) - dy;
                if let Ok(src) = u16::try_from(src)
                    && src < buf.height()
                {
                    for x in 0..w.min(buf.width()) {
                        self.image.put(x, y, buf.get(x, src));
                    }
                }
            }
        }
        self.stage(&Dirty::All, true, flashes, now, fitted.scene.as_position());
    }

    /// Queue the tiles of [`Self::image`], which shows `flashes`, that differ
    /// from what was sent, among those `dirty` reaches, once the cadence
    /// allows: at once for a new phase, so it shows as long as it lasts.
    /// `fresh` is reported as [`FrameSend::fresh`].
    fn stage(
        &mut self,
        dirty: &Dirty,
        fresh: bool,
        flashes: Flashes,
        now: SystemTime,
        origin: Position,
    ) {
        self.origin = origin;
        let changed =
            tracing::trace_span!("tiles.diff").in_scope(|| self.tiles.changed(&self.image, dirty));
        self.last = FrameSend {
            dirty: dirty.into(),
            fresh,
            changed: changed.len(),
            ..FrameSend::default()
        };
        tracing::trace!(
            dirty = ?self.last.dirty,
            rects = match dirty {
                Dirty::Rects(r) => r.as_slice().len(),
                _ => 0,
            },
            rect_px = match dirty {
                Dirty::Rects(r) => r
                    .as_slice()
                    .iter()
                    .map(|b| u64::from(b.width) * u64::from(b.height))
                    .sum::<u64>(),
                _ => 0,
            },
            changed = changed.len(),
            "tiles.stage"
        );
        let due = self.flash.changes(flashes)
            || self.sent_at.is_none_or(|at| {
                now.duration_since(at)
                    .map_or(true, |since| since >= self.route.protocol().cadence())
            });
        self.pending = Some(Pending {
            tiles: if due { changed } else { Vec::new() },
            flashes,
        });
    }

    /// Send kitty's tiles before ratatui's flush, so the placeholders it
    /// writes find their images there.
    pub(crate) fn before_flush(&mut self, now: SystemTime) {
        if self.route.protocol() == ImageProtocol::Kitty {
            self.send(&[], now);
        }
    }

    /// Send SIXEL's or iTerm2's tiles but the `covered` ones after ratatui's
    /// flush: only then are the text cells they must avoid known. A covered
    /// tile is owed again, so it is re-sent once uncovered.
    pub(crate) fn after_flush(&mut self, covered: &[u32], now: SystemTime) {
        if self.route.protocol() == ImageProtocol::Kitty {
            return;
        }
        for &index in covered {
            self.tiles.forget_tile(index);
        }
        self.send(covered, now);
    }

    /// Encode and write the queued tiles but the `covered` ones, to land with
    /// the frame. A write the sink refuses is logged and leaves its tiles
    /// owed, and its flashes unshown.
    fn send(&mut self, covered: &[u32], now: SystemTime) {
        let Some(Pending {
            tiles: mut send,
            flashes,
        }) = self.pending.take()
        else {
            return;
        };
        send.retain(|c| !covered.contains(&c.tile.index));
        if send.is_empty() {
            self.landing = Some(Landing {
                sent: send,
                flashes,
                at: None,
            });
            return;
        }
        match self.route.protocol() {
            ImageProtocol::Kitty => {
                if let Some(last) = send
                    .iter()
                    .filter_map(|c| kitty::image_id(self.base, c.tile))
                    .max()
                {
                    kitty::on_screen(self.route.tmux(), last);
                }
            }
            ImageProtocol::Sixel | ImageProtocol::Iterm2 => {
                self.in_grid.store(true, Ordering::Relaxed)
            }
        }
        let mut wrote = Ok(());
        let begun = Instant::now();
        let encoded =
            tracing::trace_span!("tiles.encode").in_scope(|| self.encoder().encode_all(&send));
        self.last.encode = begun.elapsed();
        let mut sent = Vec::with_capacity(send.len());
        for (c, bytes) in send.into_iter().zip(encoded) {
            if wrote.is_err() {
                break;
            }
            if let Some(bytes) = bytes {
                wrote = tracing::trace_span!("tile.write").in_scope(|| self.out.write_all(&bytes));
                self.last.bytes += bytes.len() as u64;
                sent.push(c);
            }
        }
        let flushed = wrote.and_then(|()| self.out.flush());
        self.last.sent = sent.len();
        match flushed {
            Ok(()) => {
                self.landing = Some(Landing {
                    sent,
                    flashes,
                    at: Some(now),
                });
            }
            Err(e) => tracing::warn!(error = %e, "image transmit failed"),
        }
    }

    /// The frame reached the terminal: its transmits are shown, and a flash
    /// phase's floor runs from now.
    pub(crate) fn landed(&mut self) {
        if let Some(Landing { sent, flashes, at }) = self.landing.take() {
            if at.is_some() {
                self.sent_at = at;
            }
            self.tiles.sent(&sent);
            self.flash.shown(flashes, self.fitted);
        }
    }

    /// The frame never reached the terminal: nothing it wrote is shown.
    pub(crate) fn lost(&mut self) {
        self.landing = None;
    }

    /// What encodes this frame's tiles, apart from the sink no thread shares.
    fn encoder(&self) -> Encoder<'_> {
        Encoder {
            tiles: &self.tiles,
            image: &self.image,
            route: self.route,
            base: self.base,
            origin: self.origin,
            threads: encode_cores(self.cores, self.audio),
        }
    }

    /// Whether an audio thread is up: while one is, the encode leaves it a
    /// core.
    pub(crate) fn share_with_audio(&mut self, audio: bool) {
        self.audio = audio;
    }

    /// A new frame, which has transmitted nothing until it paints.
    pub(crate) fn begin_frame(&mut self) {
        self.last = FrameSend::default();
    }

    /// Set `in_grid` where the unwind would read the process's own.
    #[cfg(test)]
    pub(crate) fn arming(mut self, in_grid: &'static AtomicBool) -> Self {
        self.in_grid = in_grid;
        self
    }

    /// Hold flashes on `screen` in place of the process's monotonic clock.
    #[cfg(test)]
    pub(crate) fn hold_on(&mut self, screen: pixtuoid_scene::flash::ScreenClock) {
        self.flash = FlashHold::on(screen);
    }

    /// Split a frame's encode across `cores` whatever the machine has.
    pub(crate) fn split_across(&mut self, cores: usize) {
        self.cores = cores;
    }

    /// The threads a frame's encode may take now.
    #[cfg(test)]
    pub(crate) fn encode_threads(&self) -> usize {
        encode_cores(self.cores, self.audio)
    }

    /// The tile the grid sends in now, its image budget applied.
    #[cfg(test)]
    pub(crate) fn tile_shape(&self) -> crate::graphics::TileShape {
        self.tiles.shape()
    }

    /// Show every tile in `scene`'s cells of `buf`, before the frame's text
    /// is drawn: kitty's placeholders, or for SIXEL and iTerm2 a
    /// [`SENTINEL`] ratatui's diff skips, so the flush never blanks the
    /// pixels.
    pub(crate) fn place(&self, buf: &mut Buffer, scene: Rect) {
        if self.route.protocol() == ImageProtocol::Kitty {
            let tiles = self
                .tiles
                .all()
                .filter_map(|tile| kitty::placeholders(kitty::image_id(self.base, tile)?, tile));
            for p in tiles.flatten() {
                if let Some(cell) = image_cell(buf, scene, p.col, p.row) {
                    cell.reset();
                    cell.set_symbol(&p.symbol).set_fg(p.fg);
                }
            }
            return;
        }
        for tile in self.tiles.all() {
            for (col, row) in cells(tile) {
                if let Some(cell) = image_cell(buf, scene, col, row) {
                    *cell = sentinel();
                }
            }
        }
    }

    /// The tiles under text drawn since [`Self::place`], all of whose cells
    /// it hands back to ratatui's diff: the text, and the rest as
    /// half-blocks. Kitty's text needs no room made.
    pub(crate) fn cover(&self, buf: &mut Buffer, scene: Rect) -> Vec<u32> {
        if self.route.protocol() == ImageProtocol::Kitty {
            return Vec::new();
        }
        let sentinel = sentinel();
        let mut covered = Vec::new();
        let mut halves = None;
        for tile in self.tiles.all() {
            let under_text = cells(tile).any(|(col, row)| {
                image_cell(buf, scene, col, row).is_some_and(|cell| *cell != sentinel)
            });
            if !under_text {
                continue;
            }
            covered.push(tile.index);
            let halves: &RgbBuffer =
                halves.get_or_insert_with(|| self.tiles.half_blocks(&self.image));
            for (col, row) in cells(tile) {
                let Some(cell) = image_cell(buf, scene, col, row) else {
                    continue;
                };
                if *cell == sentinel {
                    let (top, bottom) = (row * 2, row * 2 + 1);
                    if col < halves.width() && bottom < halves.height() {
                        cell.reset();
                        let half = |y| halves.get(col, y);
                        set_half_block(cell, half(top), half(bottom));
                    }
                } else {
                    cell.set_diff_option(CellDiffOption::None);
                }
            }
        }
        covered
    }

    /// What the last frame's transmits did.
    pub(crate) fn last_send(&self) -> FrameSend {
        self.last
    }

    /// Owe every tile again: the terminal may have dropped them.
    pub(crate) fn forget(&mut self) {
        self.tiles.forget();
    }
}

/// A thread's least share of a frame's tiles: below it, its spawn outweighs
/// their encode.
pub(crate) const TILES_PER_THREAD: usize = 32;

/// The cores a frame's encode may take of `cores`: all but one while an
/// `audio` thread is up, which synthesizes a track for seconds at its start
/// and at a swap.
fn encode_cores(cores: usize, audio: bool) -> usize {
    if audio {
        cores.saturating_sub(1).max(1)
    } else {
        cores
    }
}

/// The threads a frame of `tiles` splits across on `cores`: one a
/// [`TILES_PER_THREAD`] share, never more than the cores.
fn threads_for(tiles: usize, cores: usize) -> usize {
    cores.min(tiles / TILES_PER_THREAD).max(1)
}

/// A frame's tiles to escapes, each independent of the others.
#[derive(Clone, Copy)]
struct Encoder<'a> {
    tiles: &'a Tiles,
    image: &'a RgbBuffer,
    route: crate::graphics::Route,
    base: u32,
    origin: Position,
    threads: usize,
}

impl Encoder<'_> {
    /// Each of `send`'s escapes, in its order, split across as many threads
    /// as it has [`TILES_PER_THREAD`] shares, up to the cores.
    fn encode_all(self, send: &[Changed]) -> Vec<Option<Vec<u8>>> {
        let threads = threads_for(send.len(), self.threads);
        if threads == 1 {
            return send.iter().map(|&c| self.encode(c)).collect();
        }
        std::thread::scope(|s| {
            let shares: Vec<_> = send
                .chunks(send.len().div_ceil(threads))
                .map(|share| {
                    s.spawn(move || share.iter().map(|&c| self.encode(c)).collect::<Vec<_>>())
                })
                .collect();
            shares
                .into_iter()
                .flat_map(|share| {
                    share
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    }

    /// `c`'s tile in the protocol's escape; `None` where it has none.
    fn encode(self, c: Changed) -> Option<Vec<u8>> {
        let _encode = tracing::trace_span!("tile.encode").entered();
        let image =
            tracing::trace_span!("tile.cut").in_scope(|| self.tiles.image(self.image, c.tile));
        match self.route.protocol() {
            ImageProtocol::Kitty => kitty::image_id(self.base, c.tile)
                .map(|id| kitty::transmit(id, &image, self.route.tmux(), self.route.medium())),
            ImageProtocol::Sixel => Some(sixel::transmit(&image, self.origin)),
            ImageProtocol::Iterm2 => iterm2::transmit(&image, self.origin)
                .inspect_err(|e| tracing::warn!(error = %e, "iterm2 encode failed"))
                .ok(),
        }
    }
}

fn sentinel() -> Cell {
    let mut cell = Cell::new(SENTINEL);
    cell.set_diff_option(CellDiffOption::Skip);
    cell
}

/// `tile`'s cells, from the image's top-left.
fn cells(tile: Tile) -> impl Iterator<Item = (u16, u16)> {
    (tile.row..tile.row + tile.rows)
        .flat_map(move |row| (tile.col..tile.col + tile.cols).map(move |col| (col, row)))
}

/// The cell of `buf` showing image cell `(col, row)`, inside `scene`.
fn image_cell(buf: &mut Buffer, scene: Rect, col: u16, row: u16) -> Option<&mut Cell> {
    let at = Position {
        x: scene.x.saturating_add(col),
        y: scene.y.saturating_add(row),
    };
    scene.contains(at).then(|| buf.cell_mut(at)).flatten()
}

#[cfg(test)]
mod tests {
    use super::{TILES_PER_THREAD, encode_cores, threads_for};

    /// The encode leaves a core only to a live audio thread: a muted user,
    /// who has none, keeps every core, two of them on a 2-core host.
    #[test]
    fn the_encode_leaves_a_core_only_to_live_audio() {
        assert_eq!(encode_cores(2, false), 2);
        assert_eq!(encode_cores(2, true), 1);
        assert_eq!(encode_cores(10, true), 9);
        assert_eq!(encode_cores(1, true), 1);
    }

    /// A frame of two shares splits across two cores, one share short of
    /// that stays on one thread, and one core never splits: a serial
    /// regression fails here, where an instruction count can't see it.
    #[test]
    fn a_frame_of_two_shares_splits_across_the_cores() {
        assert_eq!(threads_for(2 * TILES_PER_THREAD, 8), 2);
        assert_eq!(threads_for(2 * TILES_PER_THREAD - 1, 8), 1);
        assert_eq!(threads_for(100 * TILES_PER_THREAD, 8), 8);
        assert_eq!(threads_for(100 * TILES_PER_THREAD, 1), 1);
    }
}
