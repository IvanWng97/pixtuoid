//! The cutaway over kitty's graphics protocol: a frame's changed tiles are
//! transmitted ahead of ratatui's flush, and the flush shows every tile through
//! Unicode placeholders, which are ordinary text — so a modal or tooltip drawn
//! after them takes their cells as any text would
//! (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>, "Unicode placeholders").

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
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::graphics::tiles::Tiles;
use crate::graphics::{CellSize, Fit, ImageProtocol, kitty};
use crate::tui::geometry::SceneGeometry;

/// Where the transmits go: the terminal ratatui's backend also writes to.
pub(crate) type Sink = Box<dyn Write + Send>;

/// The cutaway's canvas, its tiles, and what of them the terminal holds.
pub(crate) struct KittyCutaway {
    canvas: CutawayCanvas,
    cache: FrameCache,
    cell: CellSize,
    tmux: bool,
    /// [`kitty::process_base`].
    base: u32,
    fit: Fit,
    tiles: Tiles,
    out: Sink,
    /// A write failed, perhaps mid-escape: the next one opens with
    /// [`kitty::ST`].
    torn: bool,
}

impl KittyCutaway {
    /// [`crate::graphics::Plan::Cutaway`]'s parts, transmitting into `out`.
    pub(crate) fn new(pack: Arc<Pack>, fit: Fit, cell: CellSize, tmux: bool, out: Sink) -> Self {
        Self {
            canvas: CutawayCanvas::new(pack),
            cache: FrameCache::new(),
            cell,
            tmux,
            base: kitty::process_base(),
            fit,
            tiles: Tiles::new(ImageProtocol::Kitty, cell, fit),
            out,
            torn: false,
        }
    }

    /// The office's logical extent over `scene`'s cells. The scale is the
    /// cell's, so a resize changes only the extent, and the canvas's new
    /// buffer size makes [`Tiles`] re-send every tile.
    pub(crate) fn fit_to(&mut self, scene: Rect) -> Size {
        self.fit = self.fit.over(self.cell, scene.as_size());
        self.fit.logical()
    }

    /// Paint `observed` and transmit the tiles it changed. A failed write is
    /// logged and leaves them owed to the next frame.
    pub(crate) fn paint(
        &mut self,
        observed: &ObservedFloor,
        theme: &'static Theme,
        floor: FloorMeta,
        now: SystemTime,
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
        if changed.is_empty() {
            return;
        }
        if let Some(last) = changed
            .iter()
            .filter_map(|c| kitty::image_id(self.base, c.tile))
            .max()
        {
            kitty::on_screen(self.tmux, last);
        }
        let mut wrote = if self.torn {
            self.out.write_all(kitty::ST)
        } else {
            Ok(())
        };
        for c in &changed {
            if wrote.is_err() {
                break;
            }
            if let Some(id) = kitty::image_id(self.base, c.tile) {
                let image = self.tiles.image(frame.buf, c.tile);
                wrote = self.out.write_all(&kitty::transmit(id, &image, self.tmux));
            }
        }
        match wrote.and_then(|()| self.out.flush()) {
            Ok(()) => {
                self.torn = false;
                self.tiles.sent(&changed);
            }
            Err(e) => {
                self.torn = true;
                tracing::warn!(error = %e, "kitty transmit failed");
            }
        }
    }

    /// Every tile's placeholders, written into `scene`'s cells of `buf`.
    pub(crate) fn place(&self, buf: &mut Buffer, scene: Rect) {
        let tiles = self
            .tiles
            .all()
            .filter_map(|tile| kitty::placeholders(kitty::image_id(self.base, tile)?, tile));
        for p in tiles.flatten() {
            let at = Position {
                x: scene.x.saturating_add(p.col),
                y: scene.y.saturating_add(p.row),
            };
            if scene.contains(at)
                && let Some(cell) = buf.cell_mut(at)
            {
                cell.reset();
                cell.set_symbol(&p.symbol).set_fg(p.fg);
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
