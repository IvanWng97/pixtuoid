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
//!   image never share a cell.

use std::io::Write;
use std::sync::Arc;
use std::time::SystemTime;

use pixtuoid_core::AgentId;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_scene::cutaway::canvas::CutawayCanvas;
use pixtuoid_scene::floor::{FloorMeta, ObservedFloor};
use pixtuoid_scene::frame_cache::FrameCache;
use pixtuoid_scene::layout::{Bounds, Size};
use pixtuoid_scene::theme::Theme;
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::{Position, Rect};

use crate::graphics::tiles::{Changed, Tile, Tiles};
use crate::graphics::{CellSize, Fit, ImageProtocol, iterm2, kitty, sixel};
use crate::tui::geometry::SceneGeometry;

/// Where the transmits go: the terminal ratatui's backend also writes to.
pub(crate) type Sink = Box<dyn Write + Send>;

/// What an image cell holds before the frame's text is drawn: a cell still
/// equal to it afterwards is image, any other is text.
const SENTINEL: &str = "\u{F8FF}";

/// The cutaway's canvas, its tiles, and what of them the terminal holds.
pub(crate) struct TileCutaway {
    canvas: CutawayCanvas,
    cache: FrameCache,
    cell: CellSize,
    protocol: ImageProtocol,
    tmux: bool,
    /// [`kitty::process_base`].
    base: u32,
    fit: Fit,
    /// The scene `fit` was last fitted to.
    scene: Rect,
    tiles: Tiles,
    out: Sink,
    /// This frame's encoded tiles, written by [`Self::emit`].
    pending: Vec<(Changed, Vec<u8>)>,
    /// When the last transmits were written, for the protocol's cadence.
    sent_at: Option<SystemTime>,
    /// A write failed, perhaps mid-escape: the next one opens with
    /// [`kitty::ST`].
    torn: bool,
}

impl TileCutaway {
    /// [`crate::graphics::Plan::Cutaway`]'s parts, transmitting into `out`.
    pub(crate) fn new(
        pack: Arc<Pack>,
        fit: Fit,
        cell: CellSize,
        protocol: ImageProtocol,
        tmux: bool,
        out: Sink,
    ) -> Self {
        Self {
            canvas: CutawayCanvas::new(pack),
            cache: FrameCache::new(),
            cell,
            protocol,
            tmux,
            base: kitty::process_base(),
            fit,
            scene: Rect::default(),
            tiles: Tiles::new(protocol, cell, fit),
            out,
            pending: Vec::new(),
            sent_at: None,
            torn: false,
        }
    }

    /// The office's logical extent over `scene`'s cells. The scale is the
    /// cell's, so a resize changes only the extent. A resize also clears the
    /// screen ratatui redraws, so every tile is re-sent.
    pub(crate) fn fit_to(&mut self, scene: Rect) -> Size {
        if scene != self.scene {
            self.scene = scene;
            self.fit = self.fit.over(self.cell, scene.as_size());
            self.tiles.forget();
        }
        self.fit.logical()
    }

    /// Paint `observed`, its top-left cell at `origin`, and encode the tiles
    /// it changed once the protocol's cadence allows; until then they stay
    /// owed.
    pub(crate) fn paint(
        &mut self,
        observed: &ObservedFloor,
        theme: &'static Theme,
        floor: FloorMeta,
        now: SystemTime,
        origin: Position,
    ) {
        let frame = self.canvas.frame(
            observed,
            theme,
            self.fit.render_scale(),
            floor,
            now,
            &mut self.cache,
        );
        let changed = self.tiles.changed(frame.buf, &frame.dirty);
        let due = self.sent_at.is_none_or(|at| {
            now.duration_since(at)
                .map_or(true, |since| since >= self.protocol.cadence())
        });
        self.pending.clear();
        if !due {
            return;
        }
        for c in changed {
            let image = self.tiles.image(frame.buf, c.tile);
            let bytes = match self.protocol {
                ImageProtocol::Kitty => kitty::image_id(self.base, c.tile)
                    .map(|id| kitty::transmit(id, &image, self.tmux)),
                ImageProtocol::Sixel => Some(sixel::transmit(&image, origin)),
                ImageProtocol::Iterm2 => iterm2::transmit(&image, origin)
                    .inspect_err(|e| tracing::warn!(error = %e, "iterm2 encode failed"))
                    .ok(),
            };
            self.pending.extend(bytes.map(|b| (c, b)));
        }
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

    /// The tiles under text drawn since [`Self::place`], whose cells it
    /// hands back to ratatui's diff. Kitty's text needs no room made.
    pub(crate) fn cover(&self, buf: &mut Buffer, scene: Rect) -> Vec<u32> {
        if self.protocol == ImageProtocol::Kitty {
            return Vec::new();
        }
        let sentinel = sentinel();
        let mut covered = Vec::new();
        for tile in self.tiles.all() {
            let mut under_text = false;
            for (col, row) in cells(tile) {
                if let Some(cell) = image_cell(buf, scene, col, row)
                    && *cell != sentinel
                {
                    cell.set_diff_option(CellDiffOption::None);
                    under_text = true;
                }
            }
            if under_text {
                covered.push(tile.index);
            }
        }
        covered
    }

    /// Write this frame's tiles but the `covered` ones, after ratatui's
    /// flush: only then are the text cells the tiles must avoid known. A
    /// covered tile is owed again, so it is re-sent once uncovered. A failed
    /// write is logged and leaves its tiles owed.
    pub(crate) fn emit(&mut self, covered: &[u32], now: SystemTime) {
        for &index in covered {
            self.tiles.forget_tile(index);
        }
        let mut send = std::mem::take(&mut self.pending);
        send.retain(|(c, _)| !covered.contains(&c.tile.index));
        if send.is_empty() {
            return;
        }
        match self.protocol {
            ImageProtocol::Kitty => {
                if let Some(last) = send
                    .iter()
                    .filter_map(|(c, _)| kitty::image_id(self.base, c.tile))
                    .max()
                {
                    kitty::on_screen(self.tmux, last);
                }
            }
            ImageProtocol::Sixel | ImageProtocol::Iterm2 => crate::graphics::drawing_in_grid(),
        }
        let mut wrote = if self.torn {
            self.out.write_all(kitty::ST)
        } else {
            Ok(())
        };
        for (_, bytes) in &send {
            if wrote.is_err() {
                break;
            }
            wrote = self.out.write_all(bytes);
        }
        match wrote.and_then(|()| self.out.flush()) {
            Ok(()) => {
                self.torn = false;
                self.sent_at = Some(now);
                let sent: Vec<Changed> = send.into_iter().map(|(c, _)| c).collect();
                self.tiles.sent(&sent);
            }
            Err(e) => {
                self.torn = true;
                tracing::warn!(error = %e, "image transmit failed");
            }
        }
    }

    /// [`CutawayCanvas::hover_at`] on the last frame painted.
    pub(crate) fn hover_at(&self, area: Bounds) -> Option<AgentId> {
        self.canvas.hover_at(area)
    }

    /// The image's cells, its top-left on `scene`'s.
    pub(crate) fn geometry(&self, scene: Rect) -> SceneGeometry {
        SceneGeometry::Cutaway {
            origin: scene.as_position(),
            cell: self.cell,
            scale: self.fit.scale(),
        }
    }

    /// Owe every tile again: the terminal may have dropped them.
    pub(crate) fn forget(&mut self) {
        self.tiles.forget();
    }

    /// Drop the recolored sprites, as a theme change must
    /// ([`TuiRenderer::set_theme`](crate::tui::tui_renderer::TuiRenderer::set_theme)).
    pub(crate) fn reset_cache(&mut self) {
        self.cache = FrameCache::new();
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
