//! How a floor is drawn: a [`Look`], the per-floor [`Raster`] that keeps each
//! look's state across frames, and [`render`], the one frame entry every
//! painter calls.

use std::collections::HashSet;
use std::sync::Arc;

use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::DaemonState;

use crate::cutaway::canvas::{CanvasFrame, CutawayCanvas, Dirty};
use crate::display::Hovers;
use crate::floor::{FloorInputs, OfficeStores, PerFloor, step_floor};
use crate::footer::FooterFloor;
use crate::layout::{SceneLayout, Size};
use crate::pixel_painter::{ClassicCaches, Drawn, PaintCtx, paint_frame};
use crate::render_scale::RenderScale;
use crate::theme::Theme;

/// How a floor is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The half-block office: one buffer pixel per layout unit.
    Classic,
    /// The cutaway, `scale` buffer pixels per layout unit.
    Cutaway {
        /// Buffer pixels per layout unit.
        scale: RenderScale,
    },
}

/// What only the office knows about the floor a painter draws: its wall
/// board's gateway chip and breadcrumb.
#[derive(Debug, Clone, Copy, Default)]
pub struct Place {
    /// The office's [`office_gateway`](crate::neon_sign::office_gateway).
    pub gateway: Option<DaemonState>,
    /// Where the floor sits, `None` in a one-floor office.
    pub floor: Option<FooterFloor>,
}

/// One floor's frame, beyond its stores.
#[derive(Clone, Copy)]
pub struct RenderInputs<'a> {
    /// The floor it shows.
    pub world: FloorInputs<'a>,
    /// The active color theme.
    pub theme: &'static Theme,
    /// The office's logical extent; a look's scale maps it to buffer pixels.
    pub size: Size,
    /// See [`Place`].
    pub place: Place,
    /// The classic's walkable/approach/route debug layer (the `w` toggle).
    pub debug_walkable: bool,
}

/// One frame from [`render`].
pub struct Rendered<'r> {
    /// The whole frame.
    pub pixels: &'r RgbBuffer,
    /// Where it may differ from the last frame this raster showed.
    pub dirty: Dirty,
    /// The layout the sim stepped on and the frame was drawn on.
    pub layout: Arc<SceneLayout>,
    /// Waypoints with an occupant: the appliance audio cues' feed.
    pub occupied_waypoints: HashSet<usize>,
}

/// The office's raster state, shared by every floor: the cutaway's art.
#[derive(Default)]
pub struct OfficeRaster {
    cutaway: crate::cutaway::paint::CutawayCache,
}

impl OfficeRaster {
    /// Drop the cached art, after a theme change.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// One floor's raster state, for every look it has been drawn in: each look's
/// is built the first time it draws and kept, so a look that switches back and
/// forth never rebuilds it.
pub struct Raster {
    // The pack the cutaway draws; the sim's (`FloorInputs::pack`) must be it.
    pack: Arc<Pack>,
    classic: Option<Classic>,
    cutaway: Option<CutawayCanvas>,
    /// The look of the last frame drawn; the first frame in another repaints whole.
    shown: Option<Look>,
}

struct Classic {
    buf: RgbBuffer,
    caches: ClassicCaches,
    /// What the last frame drew that a pointer finds or a badge hangs from.
    hits: Drawn,
}

/// What the last classic frame drew besides its pixels, for a painter that
/// sets the classic's text and hit-tests it itself.
pub struct ClassicDrawn<'a> {
    /// The frame, for a painter's own wash over it (a modal's dim).
    pub pixels: &'a mut RgbBuffer,
    /// Its text in paint order: each drawn agent's badge, each chitchat
    /// bubble, then the wall board's lines and the floor indicator.
    pub texts: &'a [crate::display::TextRun],
    /// What the frame answers a pointer with.
    pub hovers: &'a Hovers,
}

impl Raster {
    /// A floor drawn with `pack`, in no look yet.
    pub(crate) fn new(pack: Arc<Pack>) -> Self {
        Self {
            pack,
            classic: None,
            cutaway: None,
            shown: None,
        }
    }

    fn classic(&mut self) -> &mut Classic {
        self.classic.get_or_insert_with(|| Classic {
            buf: RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 }),
            caches: ClassicCaches::new(),
            hits: Drawn::default(),
        })
    }

    /// The last frame, when the classic drew it.
    pub fn classic_drawn(&mut self) -> Option<ClassicDrawn<'_>> {
        let classic = self
            .classic
            .as_mut()
            .filter(|_| self.shown == Some(Look::Classic))?;
        Some(ClassicDrawn {
            pixels: &mut classic.buf,
            texts: &classic.hits.texts,
            hovers: &classic.hits.hovers,
        })
    }

    /// What the last frame drawn answers a pointer with, in either look;
    /// `None` before the first.
    pub fn hovers(&self) -> Option<&Hovers> {
        match self.shown? {
            Look::Classic => self.classic.as_ref().map(|c| &c.hits.hovers),
            Look::Cutaway { .. } => self.cutaway.as_ref()?.hovers(),
        }
    }

    /// The text the last classic frame set, in paint order; none in another
    /// look.
    pub(crate) fn classic_texts(&self) -> &[crate::display::TextRun] {
        match (self.shown, &self.classic) {
            (Some(Look::Classic), Some(classic)) => &classic.hits.texts,
            _ => &[],
        }
    }

    /// The text the last frame drawn sets, in either look; `None` before the
    /// first.
    pub fn texts(&self) -> Option<&[crate::display::TextRun]> {
        match self.shown? {
            Look::Classic => self.classic.as_ref().map(|c| c.hits.texts.as_slice()),
            Look::Cutaway { .. } => self.cutaway.as_ref()?.texts(),
        }
    }

    /// Drop the classic's recolored sprites of agents no longer in `scene`.
    pub(crate) fn evict_missing(&mut self, scene: &pixtuoid_core::SceneState) {
        if let Some(classic) = &mut self.classic {
            classic.caches.sprites.evict_missing(scene);
        }
    }

    /// Flush the classic's recolored sprites, after a theme change.
    pub fn reset_sprite_cache(&mut self) {
        if let Some(classic) = &mut self.classic {
            classic.caches.sprites = crate::frame_cache::FrameCache::new();
        }
    }

    /// The pixels of the last frame drawn, `None` before the first.
    pub fn pixels(&self) -> Option<&RgbBuffer> {
        match self.shown? {
            Look::Classic => self.classic.as_ref().map(|c| &c.buf),
            Look::Cutaway { .. } => self.cutaway.as_ref().map(CutawayCanvas::buf),
        }
    }
}

/// Step `floor` one frame and draw it in `look` on its raster: the one frame
/// entry every painter calls. `None` when `inputs.size` can't lay out, or when
/// `world.pack` is not the one `floor` was made with.
pub fn render<'r>(
    floor: &'r mut PerFloor,
    office: OfficeStores<'_>,
    look: Look,
    inputs: RenderInputs<'_>,
) -> Option<Rendered<'r>> {
    let PerFloor { ctx, raster } = floor;
    let RenderInputs {
        world,
        theme,
        size,
        place,
        debug_walkable,
    } = inputs;
    if !std::ptr::eq(world.pack, &*raster.pack) {
        tracing::error!("frame refused: the sim steps one pack and the raster draws another");
        return None;
    }
    let Some(stepped) = step_floor(ctx, office.coffee, office.chitchat, world, size) else {
        if look == Look::Classic {
            let classic = raster.classic();
            classic
                .buf
                .resize_fill(size.w, size.h, theme.surface.bg_fallback);
            classic.hits = Drawn::default();
            raster.shown = Some(look);
        }
        return None;
    };
    let switched = raster
        .shown
        .replace(look)
        .is_none_or(|was| std::mem::discriminant(&was) != std::mem::discriminant(&look));
    let board = crate::neon_sign::wall_board(
        world.scene,
        place.gateway,
        place.floor,
        world.floor.motion,
        world.now,
    );
    let (pixels, dirty) = match look {
        Look::Classic => {
            let classic = raster.classic();
            classic
                .buf
                .resize_fill(size.w, size.h, theme.surface.bg_fallback);
            classic.hits = paint_frame(
                &mut PaintCtx::classic(
                    world,
                    &stepped.layout,
                    theme,
                    &mut classic.caches,
                    &mut classic.buf,
                    &ctx.walks,
                    debug_walkable,
                ),
                &stepped.frame,
            );
            classic.hits.texts.extend(board.runs(theme));
            classic.hits.texts.push(crate::display::TextRun::indicator(
                stepped.layout.door,
                world.floor.floor_idx + 1,
                theme,
            ));
            (&classic.buf, Dirty::All)
        }
        Look::Cutaway { scale } => {
            let canvas = raster
                .cutaway
                .get_or_insert_with(|| CutawayCanvas::new(Arc::clone(&raster.pack)));
            let CanvasFrame { buf, dirty } = canvas.frame(
                &stepped,
                theme,
                scale,
                crate::display::Showing {
                    floor: world.floor,
                    now: world.now,
                    board: &board,
                },
                &mut office.raster.cutaway,
            );
            (buf, if switched { Dirty::All } else { dirty })
        }
    };
    Some(Rendered {
        pixels,
        dirty,
        layout: stepped.layout,
        occupied_waypoints: stepped.frame.occupied_waypoints,
    })
}

#[cfg(test)]
mod tests;
