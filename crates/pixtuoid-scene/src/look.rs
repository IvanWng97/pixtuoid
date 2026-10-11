//! How a floor is drawn: a [`Look`], the per-floor [`Raster`] that keeps each
//! look's state across frames, and [`render`], the one frame entry every
//! painter calls.

use std::collections::HashSet;
use std::sync::Arc;

use crate::pack::OfficeArt;
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::state::DaemonState;

/// Where a [`Rendered`] frame may differ from the last: the frame entry's
/// word for it, which the model reads without naming a rasterizer.
#[doc(hidden)]
pub use crate::cutaway::canvas::Dirty;
use crate::cutaway::canvas::{CanvasFrame, CutawayCanvas};
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
        /// Who sets its world text.
        text: WorldText,
    },
}

/// Who sets a cutaway's world text: the badges, bubbles, wall board and floor
/// indicator. The classic's is always the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldText {
    /// The image, in the scene's pixel font.
    Baked,
    /// The host, in its own font over the image ([`Raster::host_text`]).
    Host,
}

/// What only the office knows about the floor a painter draws: its wall
/// board's gateway chip and breadcrumb.
#[derive(Debug, Clone, Copy, Default)]
pub struct Place {
    /// The office's [`office_gateway`](crate::tally::office_gateway).
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
pub struct Rendered {
    /// Where it may differ from the last frame this raster showed.
    pub(crate) dirty: Dirty,
    /// The layout the sim stepped on and the frame was drawn on.
    pub layout: Arc<SceneLayout>,
    /// Waypoints with an occupant: the appliance audio cues' feed.
    pub(crate) occupied_waypoints: HashSet<usize>,
    /// What of the frame flashes, for a painter's hold.
    pub flash: crate::flash::FlashPhase,
}

/// The frame entry's `tracing` span names, for a profiler's subscriber.
#[doc(hidden)]
pub mod spans {
    /// Stepping the floor's model.
    pub const COMPOSE: &str = "frame.compose";
    /// Drawing the stepped floor in its look.
    pub const RASTERIZE: &str = "frame.rasterize";
}

/// What the last frame of a [`Raster`] was drawn under, for a painter's
/// jank report.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameNote {
    /// Why the cutaway painted it whole, when it did; `None` for the classic,
    /// which paints every frame whole.
    pub repaint: Option<crate::cutaway::canvas::Repaint>,
    /// The weather it shows: the slot's, the next slot's, and the next one's
    /// share of the clouds.
    pub weather: (crate::sky::Weather, crate::sky::Weather, f32),
    /// Whether a strike lit it.
    pub strike: bool,
}

/// The office's raster state, shared by every floor and both looks: the
/// cutaway's art, and the outside's clouds and last views.
#[derive(Debug, Default)]
pub struct OfficeRaster {
    pub(crate) cutaway: crate::cutaway::paint::CutawayCache,
    pub(crate) outside: crate::outside::OutsideCache,
    /// The pack and the theme every cache holds art of, by identity: the
    /// `Arc` keeps the pack's address its own while it is held.
    serving: Option<(Arc<OfficeArt>, &'static Theme)>,
}

impl OfficeRaster {
    /// Have the next frame draw every cloud mass the coming second needs,
    /// not a few a frame: the boot frame, which no frame before it drew
    /// ahead for.
    #[doc(hidden)]
    pub fn warm(&mut self) {
        self.outside.clouds.warm();
    }

    /// Drop the art of agents no longer in `scene`.
    pub(crate) fn evict_missing(&mut self, scene: &pixtuoid_core::SceneState) {
        self.cutaway.evict_missing(scene);
    }

    /// Draw `pack` in `theme` from now on: every cache holds one pack's art
    /// in one theme's colours, so a frame of another empties them first.
    fn serve(&mut self, pack: &Arc<OfficeArt>, theme: &'static Theme) {
        match &self.serving {
            Some((p, t)) if Arc::ptr_eq(p, pack) && std::ptr::eq(*t, theme) => {}
            Some(_) => {
                *self = Self {
                    serving: Some((Arc::clone(pack), theme)),
                    ..Self::default()
                }
            }
            // A warm before the first frame is for this one.
            None => self.serving = Some((Arc::clone(pack), theme)),
        }
    }
}

/// One floor's raster state, for every look it has been drawn in: each look's
/// is built the first time it draws and kept, so a look that switches back and
/// forth never rebuilds it.
#[derive(Debug)]
pub struct Raster {
    // The pack the cutaway draws; the sim's (`FloorInputs::pack`) must be it.
    pack: Arc<OfficeArt>,
    classic: Option<Classic>,
    cutaway: Option<CutawayCanvas>,
    /// The look of the last frame drawn; the first frame in another repaints whole.
    shown: Option<Look>,
    note: Option<FrameNote>,
}

#[derive(Debug)]
struct Classic {
    buf: RgbBuffer,
    caches: ClassicCaches,
    /// What the last frame drew that a pointer finds or a badge hangs from.
    hits: Drawn,
    /// The last frame's wall board lines and floor indicator.
    signs: Vec<crate::display::TextRun>,
}

impl Classic {
    fn host_text(&self) -> crate::display::HostText<'_> {
        crate::display::HostText {
            badges: &self.hits.badges,
            bubbles: &self.hits.bubbles,
            signs: &self.signs,
        }
    }
}

/// What the last classic frame drew besides its pixels, for a painter that
/// sets the classic's text and hit-tests it itself.
#[derive(Debug)]
pub struct ClassicDrawn<'a> {
    /// The frame, for a painter's own wash over it (a modal's dim).
    pub pixels: &'a mut RgbBuffer,
    /// The text the painter sets over it.
    pub text: crate::display::HostText<'a>,
    /// What the frame answers a pointer with.
    pub hovers: &'a Hovers,
}

impl Raster {
    /// A floor drawn with `pack`, in no look yet.
    pub(crate) fn new(pack: Arc<OfficeArt>) -> Self {
        Self {
            pack,
            classic: None,
            cutaway: None,
            shown: None,
            note: None,
        }
    }

    fn classic(&mut self) -> &mut Classic {
        self.classic.get_or_insert_with(|| Classic {
            buf: RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 }),
            caches: ClassicCaches::new(),
            hits: Drawn::default(),
            signs: Vec::new(),
        })
    }

    /// The last frame, when the classic drew it.
    pub fn classic_drawn(&mut self) -> Option<ClassicDrawn<'_>> {
        let classic = self
            .classic
            .as_mut()
            .filter(|_| self.shown == Some(Look::Classic))?;
        let Classic {
            buf, hits, signs, ..
        } = classic;
        Some(ClassicDrawn {
            pixels: buf,
            text: crate::display::HostText {
                badges: &hits.badges,
                bubbles: &hits.bubbles,
                signs,
            },
            hovers: &hits.hovers,
        })
    }

    /// The world text the last frame left to its host, in either look;
    /// `None` before the first and for a cutaway that baked its own.
    pub fn host_text(&self) -> Option<crate::display::HostText<'_>> {
        match self.shown? {
            Look::Classic => self.classic.as_ref().map(Classic::host_text),
            Look::Cutaway { .. } => self.cutaway.as_ref()?.host_text(),
        }
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

    /// The classic, when it drew the last frame.
    fn shown_classic(&self) -> Option<&Classic> {
        self.classic
            .as_ref()
            .filter(|_| self.shown == Some(Look::Classic))
    }

    /// The cells of the star the last frame baked, where a pointer opens the
    /// repo; `None` before the first, when it drew none, or where its host
    /// sets it ([`Self::host_text`]), which then places it.
    pub(crate) fn star(&self) -> Option<crate::layout::Bounds> {
        match self.shown? {
            Look::Classic => None,
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

    /// Paint the next cutaway frame whole: the frame-pacing bench's worst case.
    #[doc(hidden)]
    pub fn forget_shown(&mut self) {
        if let Some(cutaway) = &mut self.cutaway {
            cutaway.forget();
        }
    }

    /// What the last frame drawn was drawn under, `None` before the first.
    #[doc(hidden)]
    pub fn note(&self) -> Option<FrameNote> {
        self.note
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
pub fn render(
    floor: &mut PerFloor,
    office: OfficeStores<'_>,
    look: Look,
    inputs: RenderInputs<'_>,
) -> Option<Rendered> {
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
    office.raster.serve(&raster.pack, theme);
    let stepped = tracing::trace_span!(spans::COMPOSE)
        .in_scope(|| step_floor(ctx, office.coffee, office.chitchat, world, size));
    let Some(stepped) = stepped else {
        if look == Look::Classic {
            let classic = raster.classic();
            classic
                .buf
                .resize_fill(size.w, size.h, theme.surface.bg_fallback);
            classic.hits = Drawn::default();
            classic.signs.clear();
            raster.shown = Some(look);
        }
        return None;
    };
    let switched = raster
        .shown
        .replace(look)
        .is_none_or(|was| std::mem::discriminant(&was) != std::mem::discriminant(&look));
    let _rasterize = tracing::trace_span!(spans::RASTERIZE).entered();
    let board = crate::neon_sign::wall_board(
        world.scene,
        place.gateway,
        place.floor,
        world.floor.motion,
        world.now,
    );
    let sky = crate::sky::Sky::at(world.floor.motion.timing(world.now), world.floor.weather);
    raster.note = Some(FrameNote {
        repaint: None,
        weather: {
            let mix = sky.weather();
            let [from, to] = mix.ends();
            (from, to, mix.incoming(crate::sky::Element::Cloud))
        },
        strike: sky.strike().is_some(),
    });
    let (dirty, flash) = match look {
        Look::Classic => {
            let classic = raster.classic();
            classic
                .buf
                .resize_fill(size.w, size.h, theme.surface.bg_fallback);
            let mut paint = PaintCtx::classic(
                world,
                &stepped.layout,
                theme,
                (&mut classic.caches, &mut office.raster.outside),
                &mut classic.buf,
                &ctx.walks,
                debug_walkable,
            );
            let flash = paint.flash(&stepped.frame);
            classic.hits = paint_frame(&mut paint, &stepped.frame);
            classic.signs =
                crate::display::TextRun::signs(&board, stepped.layout.door, world.floor, theme);
            (Dirty::All, flash)
        }
        Look::Cutaway { scale, text } => {
            let canvas = raster
                .cutaway
                .get_or_insert_with(|| CutawayCanvas::new(Arc::clone(&raster.pack)));
            let CanvasFrame {
                dirty,
                flash,
                repaint,
            } = canvas.frame(
                &stepped,
                theme,
                scale,
                crate::display::Showing {
                    floor: world.floor,
                    now: world.now,
                    board: &board,
                },
                text,
                (&mut office.raster.cutaway, &mut office.raster.outside),
            );
            if let Some(note) = &mut raster.note {
                note.repaint = repaint.or(switched.then(|| crate::cutaway::canvas::Repaint {
                    first: true,
                    ..Default::default()
                }));
            }
            (if switched { Dirty::All } else { dirty }, flash)
        }
    };
    Some(Rendered {
        dirty,
        layout: stepped.layout,
        occupied_waypoints: stepped.frame.occupied_waypoints,
        flash,
    })
}

#[cfg(test)]
mod tests;
