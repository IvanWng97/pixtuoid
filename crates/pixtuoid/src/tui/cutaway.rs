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
use std::time::SystemTime;

use pixtuoid_core::sprite::format::Density;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_scene::cutaway::canvas::Dirty;
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::theme::Theme;
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::{Position, Rect};

use crate::graphics::tiles::{Changed, Tile, Tiles};
use crate::graphics::{CellSize, Fit, ImageProtocol, iterm2, kitty, sixel};
use crate::tui::geometry::SceneGeometry;
use crate::tui::geometry::slide_offsets;
use crate::tui::renderer::set_half_block;

/// A floor slide's two floors' frames at progress `t` of a
/// [`FloorTransition`](pixtuoid_scene::floor::FloorTransition).
pub(crate) struct Slide<'a> {
    pub(crate) leaving: &'a RgbBuffer,
    pub(crate) arriving: &'a RgbBuffer,
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
    protocol: ImageProtocol,
    tmux: bool,
    /// [`kitty::process_base`].
    base: u32,
    /// `None` while classic paints.
    fitted: Option<Fitted>,
    tiles: Tiles,
    /// What the tiles are cut from when sent: the last frame, or a slide's
    /// two composed.
    image: RgbBuffer,
    /// What the last image showed: a floor's raster diffs against its own
    /// last frame, so any other image before it — a slide, another floor —
    /// leaves the tiles differing anywhere.
    shown: Option<Shown>,
    /// The image's top-left cell.
    origin: Position,
    out: Sink,
    /// This frame's changed tiles, once the cadence allows; encoded only if
    /// no text covers them.
    pending: Vec<Changed>,
    /// Set once SIXEL or iTerm2 pixels are written, for the unwind
    /// ([`crate::graphics::grid_unwind`]).
    in_grid: &'static AtomicBool,
    /// When the last transmits were written, for the protocol's cadence.
    sent_at: Option<SystemTime>,
    /// A write failed, perhaps mid-escape: the next one opens with
    /// [`kitty::ST`].
    torn: bool,
}

/// What a [`TileCutaway`]'s image last showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Floor(usize),
    Slide,
}

impl TileCutaway {
    /// [`crate::graphics::Plan::Cutaway`]'s parts, transmitting into `out`.
    pub(crate) fn new(
        fit: Fit,
        cell: CellSize,
        protocol: ImageProtocol,
        tmux: bool,
        out: Sink,
    ) -> Self {
        Self {
            planned: cell,
            first_window: None,
            density: fit.density(),
            protocol,
            tmux,
            base: kitty::process_base(),
            fitted: None,
            tiles: Tiles::new(protocol, cell, fit),
            image: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            shown: None,
            origin: Position::ORIGIN,
            out,
            pending: Vec::new(),
            in_grid: &crate::graphics::IN_GRID,
            sent_at: None,
            torn: false,
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
            return None;
        };
        let fitted = Fitted { scene, cell, fit };
        if self.fitted != Some(fitted) {
            self.tiles = Tiles::new(self.protocol, cell, fit);
        }
        self.fitted = Some(fitted);
        Some(fitted)
    }

    /// The plan's cell, which the terminal answered and so outranks the
    /// window's (as [`crate::graphics::probe`] ranks them), until a font zoom
    /// moves the window's cell from what it read on the first frame.
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

    /// Show `floor`'s `frame` as `fitted`, which may differ from that floor's
    /// last only within `dirty`, and queue the tiles it changed once the
    /// protocol's cadence allows; until then they stay owed.
    pub(crate) fn paint(
        &mut self,
        fitted: Fitted,
        floor: usize,
        frame: &RgbBuffer,
        dirty: Dirty,
        now: SystemTime,
    ) {
        let dirty = if self.shown.replace(Shown::Floor(floor)) == Some(Shown::Floor(floor)) {
            dirty
        } else {
            Dirty::All
        };
        if dirty != Dirty::Unchanged {
            self.image.clone_from(frame);
        }
        self.stage(&dirty, now, fitted.scene.as_position());
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
        self.stage(&Dirty::All, now, fitted.scene.as_position());
    }

    /// Queue the tiles of [`Self::image`] that differ from what was sent,
    /// among those `dirty` reaches, once the cadence allows.
    fn stage(&mut self, dirty: &Dirty, now: SystemTime, origin: Position) {
        self.origin = origin;
        let changed = self.tiles.changed(&self.image, dirty);
        let due = self.sent_at.is_none_or(|at| {
            now.duration_since(at)
                .map_or(true, |since| since >= self.protocol.cadence())
        });
        self.pending = if due { changed } else { Vec::new() };
    }

    /// Send kitty's tiles before ratatui's flush, so the placeholders it
    /// writes find their images there.
    pub(crate) fn before_flush(&mut self, now: SystemTime) {
        if self.protocol == ImageProtocol::Kitty {
            self.send(&[], now);
        }
    }

    /// Send SIXEL's or iTerm2's tiles but the `covered` ones after ratatui's
    /// flush: only then are the text cells they must avoid known. A covered
    /// tile is owed again, so it is re-sent once uncovered.
    pub(crate) fn after_flush(&mut self, covered: &[u32], now: SystemTime) {
        if self.protocol == ImageProtocol::Kitty {
            return;
        }
        for &index in covered {
            self.tiles.forget_tile(index);
        }
        self.send(covered, now);
    }

    /// Encode and write the queued tiles but the `covered` ones. A failed
    /// write is logged and leaves its tiles owed.
    fn send(&mut self, covered: &[u32], now: SystemTime) {
        let mut send = std::mem::take(&mut self.pending);
        send.retain(|c| !covered.contains(&c.tile.index));
        if send.is_empty() {
            return;
        }
        match self.protocol {
            ImageProtocol::Kitty => {
                if let Some(last) = send
                    .iter()
                    .filter_map(|c| kitty::image_id(self.base, c.tile))
                    .max()
                {
                    kitty::on_screen(self.tmux, last);
                }
            }
            ImageProtocol::Sixel | ImageProtocol::Iterm2 => {
                self.in_grid.store(true, Ordering::Relaxed)
            }
        }
        let mut wrote = if self.torn {
            self.out.write_all(kitty::ST)
        } else {
            Ok(())
        };
        let mut sent = Vec::with_capacity(send.len());
        for c in send {
            if wrote.is_err() {
                break;
            }
            if let Some(bytes) = self.encode(c) {
                wrote = self.out.write_all(&bytes);
                sent.push(c);
            }
        }
        match wrote.and_then(|()| self.out.flush()) {
            Ok(()) => {
                self.torn = false;
                self.sent_at = Some(now);
                self.tiles.sent(&sent);
            }
            Err(e) => {
                self.torn = true;
                tracing::warn!(error = %e, "image transmit failed");
            }
        }
    }

    /// `c`'s tile in the protocol's escape; `None` where it has none.
    fn encode(&self, c: Changed) -> Option<Vec<u8>> {
        let image = self.tiles.image(&self.image, c.tile);
        match self.protocol {
            ImageProtocol::Kitty => {
                kitty::image_id(self.base, c.tile).map(|id| kitty::transmit(id, &image, self.tmux))
            }
            ImageProtocol::Sixel => Some(sixel::transmit(&image, self.origin)),
            ImageProtocol::Iterm2 => iterm2::transmit(&image, self.origin)
                .inspect_err(|e| tracing::warn!(error = %e, "iterm2 encode failed"))
                .ok(),
        }
    }

    /// Set `in_grid` where the unwind would read the process's own.
    #[cfg(test)]
    pub(crate) fn arming(mut self, in_grid: &'static AtomicBool) -> Self {
        self.in_grid = in_grid;
        self
    }

    /// Show every tile in `scene`'s cells of `buf`, before the frame's text
    /// is drawn: kitty's placeholders, or for SIXEL and iTerm2 a
    /// [`SENTINEL`] ratatui's diff skips, so the flush never blanks the
    /// pixels.
    pub(crate) fn place(&self, buf: &mut Buffer, scene: Rect) {
        if self.protocol == ImageProtocol::Kitty {
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
        if self.protocol == ImageProtocol::Kitty {
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

    /// Owe every tile again: the terminal may have dropped them.
    pub(crate) fn forget(&mut self) {
        self.tiles.forget();
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
