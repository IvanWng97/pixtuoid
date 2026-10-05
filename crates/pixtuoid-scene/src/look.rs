//! How a floor is drawn: a [`Look`], the per-floor [`Raster`] that keeps each
//! look's state across frames, and [`render`], the one frame entry every
//! painter calls.

use std::collections::HashSet;
use std::sync::Arc;

use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::DaemonState;

use crate::chitchat::ChitchatBubble;
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
    /// The office's [`office_gateway`](crate::board::office_gateway).
    pub gateway: Option<DaemonState>,
    /// Where the floor sits, `None` in a one-floor office.
    pub floor: Option<FooterFloor>,
}

/// One floor's frame, beyond its stores.
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug)]
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

/// The office's raster state, shared by every floor and both looks: the
/// cutaway's art and the clouds' masses.
#[derive(Debug, Default)]
pub struct OfficeRaster {
    pub(crate) cutaway: crate::cutaway::paint::CutawayCache,
    pub(crate) clouds: crate::clouds::CloudCache,
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
#[derive(Debug)]
pub struct Raster {
    // The pack the cutaway draws; the sim's (`FloorInputs::pack`) must be it.
    pack: Arc<Pack>,
    classic: Option<Classic>,
    cutaway: Option<CutawayCanvas>,
    /// The look of the last frame drawn; the first frame in another repaints whole.
    shown: Option<Look>,
}

#[derive(Debug)]
struct Classic {
    buf: RgbBuffer,
    caches: ClassicCaches,
    /// What the last frame drew that a pointer finds or a badge hangs from.
    hits: Drawn,
    /// The last frame's wall board lines and floor indicator.
    signs: Vec<crate::display::TextRun>,
    /// The last frame's speech bubbles, which only the classic sets as text.
    bubbles: Vec<ChitchatBubble>,
}

/// What the last classic frame drew besides its pixels, for a painter that
/// sets the classic's text and hit-tests it itself.
#[derive(Debug)]
pub struct ClassicDrawn<'a> {
    /// The frame, for a painter's own wash over it (a modal's dim).
    pub pixels: &'a mut RgbBuffer,
    /// Each drawn agent's badge, in paint order.
    pub badges: &'a [crate::display::Badge],
    /// The wall board's lines and the floor indicator.
    pub signs: &'a [crate::display::TextRun],
    /// What the frame answers a pointer with.
    pub hovers: &'a Hovers,
    /// Active speech bubbles.
    pub bubbles: &'a [ChitchatBubble],
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
            signs: Vec::new(),
            bubbles: Vec::new(),
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
            badges: &classic.hits.badges,
            signs: &classic.signs,
            hovers: &classic.hits.hovers,
            bubbles: &classic.bubbles,
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

    /// The badges the last classic frame set, in paint order; none in
    /// another look.
    pub(crate) fn classic_badges(&self) -> &[crate::display::Badge] {
        self.shown_classic()
            .map_or(&[], |classic| &classic.hits.badges)
    }

    /// The signs the last classic frame set; none in another look.
    pub(crate) fn classic_signs(&self) -> &[crate::display::TextRun] {
        self.shown_classic().map_or(&[], |classic| &classic.signs)
    }

    /// The classic, when it drew the last frame.
    fn shown_classic(&self) -> Option<&Classic> {
        self.classic
            .as_ref()
            .filter(|_| self.shown == Some(Look::Classic))
    }

    /// The cells of the star the last frame drew, where a pointer opens the
    /// repo, in either look; `None` before the first or when it drew none.
    pub fn star(&self) -> Option<crate::layout::Bounds> {
        match self.shown? {
            Look::Classic => self
                .classic
                .as_ref()?
                .signs
                .iter()
                .find(|run| run.role == crate::display::TextRole::Star)
                .map(crate::display::TextRun::hit_box),
            Look::Cutaway { .. } => self.cutaway.as_ref()?.star(),
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
    let Some(mut stepped) = step_floor(ctx, office.coffee, office.chitchat, world, size) else {
        if look == Look::Classic {
            let classic = raster.classic();
            classic
                .buf
                .resize_fill(size.w, size.h, theme.surface.bg_fallback);
            classic.hits = Drawn::default();
            classic.signs.clear();
            classic.bubbles.clear();
            raster.shown = Some(look);
        }
        return None;
    };
    let switched = raster
        .shown
        .replace(look)
        .is_none_or(|was| std::mem::discriminant(&was) != std::mem::discriminant(&look));
    let board = crate::board::wall_board(
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
                    (&mut classic.caches, &mut office.raster.clouds),
                    &mut classic.buf,
                    &ctx.walks,
                    debug_walkable,
                ),
                &stepped.frame,
            );
            classic.signs.clear();
            classic.signs.extend(board.runs(theme));
            classic.signs.push(crate::display::TextRun::indicator(
                stepped.layout.door,
                world.floor.floor_idx + 1,
                theme,
            ));
            classic.bubbles = std::mem::take(&mut stepped.frame.chitchat_bubbles);
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
                (&mut office.raster.cutaway, &mut office.raster.clouds),
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
