//! `TuiRenderer` — the half-block terminal painter; its inherent `render` is
//! the production flush entry point.

use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
#[cfg(test)]
use pixtuoid_core::AgentId;
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::SceneState;

use ratatui::Terminal;
use ratatui::backend::Backend;

use ratatui::layout::Rect;

use crate::tui::renderer::PetState;
use pixtuoid_scene::display::Hovers;
use pixtuoid_scene::floor::{
    FloorInputs, FloorMeta, FloorTransition, OfficeSession, PetInputs, project_floor_scene,
};
use pixtuoid_scene::layout::{SceneLayout, Size};
use pixtuoid_scene::look::{Look, Place, RenderInputs};
use pixtuoid_scene::pathfind::Router;

#[derive(Debug, Default)]
struct PopupState {
    open: bool,
    /// When the last visible↔hidden edge happened — the animation clock.
    started_at: Option<SystemTime>,
    /// Scale captured at that edge so an interrupted animation continues from its
    /// current position instead of snapping back to the start/end.
    scale_at_edge: f32,
    /// Scale computed during the most recent `render()`; the mouse handler reads
    /// this instead of recomputing with a fresh `SystemTime`, so click geometry
    /// stays in sync with what was painted.
    last_scale: f32,
}

#[derive(Debug)]
pub struct TuiRenderer<B: Backend<Error: Send + Sync + 'static>> {
    pub terminal: Terminal<B>,
    /// The pack every floor's raster draws with, which each frame's must be.
    pack: Arc<Pack>,
    /// The frames' times and janks.
    jank: crate::jank::Jank,
    /// What holds each frame's output and presents it whole; `None` writes
    /// straight to the backend.
    frame_out: Option<crate::tui::FrameOut>,
    /// A frame was dropped unwritten, though ratatui's diff counts its cells
    /// as shown: the next repaints everything.
    redraw_owed: bool,
    /// The terminal refused the last frame: a run of refusals warns once.
    refusing: bool,
    /// Every floor, the office-wide state they share (a cup of coffee
    /// survives a floor switch), which floor shows and the slide to another,
    /// and the floor a pointer grips: the floating window's own.
    session: OfficeSession,
    /// [`Self::office_extent`] after the last frame.
    last_extent: Option<(u16, u16)>,
    mouse_pos: Option<(u16, u16)>,
    cached_layout: Option<Arc<SceneLayout>>,
    last_hovers: Hovers,
    last_star: Option<pixtuoid_scene::layout::Bounds>,
    /// The left button's gesture over the office.
    pointer: pixtuoid_scene::interact::Pointer,
    last_geometry: Option<crate::tui::geometry::SceneGeometry>,
    /// Live walkable/approach/route debug layer toggle (`w`); not persisted.
    debug_walkable: bool,
    chrome: Chrome,
    /// The flashes the half-blocks show; the cutaway holds its own.
    flash: pixtuoid_scene::flash::FlashHold<pixtuoid_scene::flash::Flashes, ratatui::layout::Size>,
    /// The cutaway, painted as terminal images in place of the half-blocks.
    #[cfg(feature = "graphics")]
    cutaway: Option<crate::tui::cutaway::TileCutaway>,
}

/// Everything a frame shows besides the floors: kept apart from `session` so a
/// frame borrows it while the session renders ([`Chrome::office_world`]).
#[derive(Debug)]
struct Chrome {
    theme: &'static pixtuoid_scene::theme::Theme,
    theme_picker: Option<usize>,
    active_pet: Option<PetState>,
    pets: Vec<pixtuoid_scene::pet::Pet>,
    popup: PopupState,
    help_open: bool,
    /// Footer warning when a source has died; `None` while healthy.
    source_warning: Option<String>,
    dashboard: crate::panels::dashboard::DashboardFrame,
    connection: crate::panels::connection::ConnectionFrame,
    onboarding: crate::panels::welcome::OnboardingFrame,
    /// Ambient-audio gateway; inert unless installed.
    audio: crate::audio::AudioHandle,
    /// Transient +/- volume readout (percent); `None` past [`crate::audio::VOLUME_FLASH_MS`].
    volume_flash: Option<u8>,
    weather: pixtuoid_scene::sky::WeatherPolicy,
    motion: pixtuoid_scene::anim::Motion,
}

impl PopupState {
    fn scale(&self, now: SystemTime) -> f32 {
        use pixtuoid_scene::anim::{Easing, eased_progress};
        const VERSION_POPUP_GROW_MS: u32 = 200;
        const VERSION_POPUP_SHRINK_MS: u32 = 120;
        match (self.open, self.started_at) {
            (true, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_GROW_MS, Easing::EaseOutCubic, now);
                self.scale_at_edge + (1.0 - self.scale_at_edge) * progress
            }
            (false, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_SHRINK_MS, Easing::EaseInQuad, now);
                self.scale_at_edge * (1.0 - progress)
            }
            (true, None) => 1.0,
            (false, None) => 0.0,
        }
    }
}

impl Chrome {
    /// Floor `floor` of `session`'s office in `scene`, whose projection is
    /// `floor_scene`: its inputs as [`OfficeSession::floor_world`] gives
    /// every painter's floors.
    fn world<'a>(
        &'a self,
        session: &OfficeSession,
        floor_scene: &'a SceneState,
        pack: &'a Pack,
        now: SystemTime,
        floor: usize,
    ) -> FloorInputs<'a> {
        session.floor_world(
            self.office_world(floor_scene, pack, now),
            floor_scene,
            floor,
            &self.pets,
        )
    }

    /// The office's inputs over `scene`, as [`OfficeSession::render`] takes
    /// them: its weather and motion, and the petting the session keeps to
    /// its own floor.
    fn office_world<'a>(
        &'a self,
        scene: &'a SceneState,
        pack: &'a Pack,
        now: SystemTime,
    ) -> FloorInputs<'a> {
        FloorInputs {
            scene,
            pack,
            now,
            floor: FloorMeta::ground()
                .with_weather(self.weather)
                .with_motion(self.motion),
            pets: PetInputs {
                pet: None,
                petting: self.active_pet.as_ref(),
            },
        }
    }

    /// The footer's context over the FULL `scene`, for the floor `session`'s
    /// footer speaks for.
    fn footer<'a>(
        &'a self,
        session: &OfficeSession,
        scene: &SceneState,
    ) -> pixtuoid_scene::footer::FooterContext<'a> {
        crate::panels::widgets::footer_context(
            scene,
            session.footer_floor(scene),
            self.audio.is_audible(),
            self.volume_flash,
            self.source_warning.as_deref(),
        )
    }

    fn overlays(&self, popup_scale: f32) -> crate::panels::OverlayFrame<'_> {
        crate::panels::OverlayFrame {
            theme_picker: self.theme_picker,
            dashboard: &self.dashboard,
            connection: &self.connection,
            popup_scale,
            help_open: self.help_open,
            onboarding: &self.onboarding,
        }
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    pub fn new(
        terminal: Terminal<B>,
        theme: &'static pixtuoid_scene::theme::Theme,
        pets: Vec<pixtuoid_scene::pet::Pet>,
        pack: Arc<Pack>,
    ) -> Self {
        Self {
            terminal,
            session: OfficeSession::new(Arc::clone(&pack)),
            pack,
            jank: crate::jank::Jank::new(std::time::Instant::now()),
            frame_out: None,
            redraw_owed: false,
            refusing: false,
            last_extent: None,
            mouse_pos: None,
            cached_layout: None,
            last_hovers: Hovers::default(),
            last_star: None,
            pointer: pixtuoid_scene::interact::Pointer::default(),
            last_geometry: None,
            debug_walkable: false,
            chrome: Chrome {
                theme,
                theme_picker: None,
                active_pet: None,
                pets,
                popup: PopupState::default(),
                help_open: false,
                source_warning: None,
                dashboard: Default::default(),
                connection: Default::default(),
                onboarding: crate::panels::welcome::OnboardingFrame::default(),
                audio: crate::audio::AudioHandle::disabled(),
                volume_flash: None,
                weather: pixtuoid_scene::sky::WeatherPolicy::Clock,
                motion: pixtuoid_scene::anim::Motion::Full,
            },
            flash: pixtuoid_scene::flash::FlashHold::on(pixtuoid_scene::flash::monotonic()),
            #[cfg(feature = "graphics")]
            cutaway: None,
        }
    }

    /// Paint `cutaway` from the next frame on.
    #[cfg(feature = "graphics")]
    pub(crate) fn set_cutaway(&mut self, cutaway: crate::tui::cutaway::TileCutaway) {
        self.cutaway = Some(cutaway);
    }

    /// Draw the first frame of `scene` unseen, in the look the first frame
    /// shown will take, so the caches it fills (the cloud rasters above all)
    /// are warm when it is shown; what it shows is unchanged.
    pub(crate) fn warm(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) {
        let Ok(size) = self.terminal.size() else {
            return;
        };
        let full = Rect::new(0, 0, size.width, size.height);
        let scene_area = crate::tui::renderer::scene_rect(full);
        if crate::tui::renderer::scene_too_small(scene_area) {
            return;
        }
        let (buf_w, buf_h) = crate::tui::renderer::scene_buf_size(size.width, size.height);
        let classic = (Look::Classic, Size { w: buf_w, h: buf_h });
        #[cfg(feature = "graphics")]
        let look = self.cutaway_look(scene_area).unwrap_or(classic);
        #[cfg(not(feature = "graphics"))]
        let look = classic;
        self.session.office_mut().raster.warm();
        let _ = self.render_office(scene, pack, now, look.0, look.1, false);
    }

    /// One frame of `scene` drawn through the session in `look` at `size`:
    /// the floor showing, or a slide's two floors composed. The layout
    /// drawn; `None` in a slide or when `size` can't lay out.
    fn render_office(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        look: Look,
        size: Size,
        debug_walkable: bool,
    ) -> Option<Arc<SceneLayout>> {
        self.session.render(
            look,
            RenderInputs {
                world: self.chrome.office_world(scene, pack, now),
                theme: self.chrome.theme,
                size,
                place: Place {
                    gateway: pixtuoid_scene::tally::office_gateway(scene),
                    floor: None,
                },
                debug_walkable,
            },
            &self.chrome.pets,
            self.chrome.theme.surface.bg_fallback,
        )
    }

    /// The cutaway's look and office extent over `scene_area`, as its next
    /// frame fits them; `None` while classic paints.
    #[cfg(feature = "graphics")]
    fn cutaway_look(&mut self, scene_area: Rect) -> Option<(Look, Size)> {
        let window = self
            .terminal
            .backend_mut()
            .window_size()
            .ok()
            .and_then(crate::graphics::CellSize::of_window);
        let fitted = self.cutaway.as_mut()?.fit_to(scene_area, window)?;
        Some((fitted.look(), fitted.fit.logical()))
    }

    /// Report the frames since the last pacing summary, at exit.
    pub(crate) fn finish_pacing(&self) {
        self.jank.finish();
    }

    /// The interval the loop now schedules frames at: what a frame's pacing
    /// is judged against.
    pub(crate) fn scheduled_every(&mut self, interval: std::time::Duration) {
        self.jank.scheduled_every(interval);
    }

    /// Hold each frame's output in `out` and present it whole.
    pub(crate) fn present_through(&mut self, out: crate::tui::FrameOut) {
        self.frame_out = Some(out);
    }

    /// Name what draws the frames, for their pacing summaries.
    pub(crate) fn painted_by(&mut self, painter: crate::jank::Painter) {
        self.jank.painted_by(painter);
    }

    /// Paint and send the next frame whole, for the pacing bench's worst case.
    #[cfg(feature = "graphics")]
    pub(crate) fn forget_frame(&mut self) {
        for floor in self.session.floors_mut() {
            floor.raster.forget_shown();
        }
        if let Some(cutaway) = &mut self.cutaway {
            cutaway.forget();
        }
    }

    /// Clear the terminal and repaint every cell and every image.
    pub(crate) fn redraw(&mut self) -> Result<()> {
        #[cfg(feature = "graphics")]
        if let Some(cutaway) = &mut self.cutaway {
            cutaway.forget();
        }
        self.terminal.clear()?;
        Ok(())
    }

    /// The logical extent the current floor last laid out on: what a resize
    /// changes, and a slide does not.
    fn office_extent(&self) -> (u16, u16) {
        #[cfg(feature = "graphics")]
        if let Some(office) = self.cutaway.as_ref().and_then(|c| c.office()) {
            return (office.w, office.h);
        }
        self.buf().map_or((0, 0), |b| (b.width(), b.height()))
    }

    /// A frame laid out at a new extent — a resize — re-routes every floor and
    /// lands any slide.
    fn follow_resize(&mut self) {
        let extent = self.office_extent();
        if self
            .last_extent
            .replace(extent)
            .is_some_and(|was| was != extent)
        {
            self.invalidate_routes();
            self.cancel_transition();
        }
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.chrome.audio = audio;
    }

    pub(crate) fn set_volume_flash(&mut self, flash: Option<u8>) {
        self.chrome.volume_flash = flash;
    }

    pub fn set_dashboard_frame(&mut self, frame: crate::panels::dashboard::DashboardFrame) {
        self.chrome.dashboard = frame;
    }

    pub fn set_connection_frame(&mut self, frame: crate::panels::connection::ConnectionFrame) {
        self.chrome.connection = frame;
    }

    pub fn set_onboarding_frame(&mut self, frame: crate::panels::welcome::OnboardingFrame) {
        self.chrome.onboarding = frame;
    }

    pub fn help_open(&self) -> bool {
        self.chrome.help_open
    }

    pub fn set_help_open(&mut self, v: bool) {
        self.chrome.help_open = v;
    }

    pub fn debug_walkable(&self) -> bool {
        self.debug_walkable
    }

    pub fn set_debug_walkable(&mut self, v: bool) {
        self.debug_walkable = v;
    }

    pub fn current_floor(&self) -> usize {
        self.session.nav().current()
    }

    #[cfg(test)]
    pub fn floor_history(&self, floor: usize) -> Option<&pixtuoid_scene::pose::PoseHistory> {
        self.session.floor(floor).map(|f| &f.ctx.history)
    }

    #[cfg(test)]
    pub fn floor_walks(
        &self,
        floor: usize,
    ) -> Option<&std::collections::HashMap<pixtuoid_core::AgentId, pixtuoid_scene::walk::WalkState>>
    {
        self.session.floor(floor).map(|f| &f.ctx.walks)
    }

    #[cfg(test)]
    pub fn floor_buf(&self, floor: usize) -> Option<&RgbBuffer> {
        self.session.floor(floor)?.raster.pixels()
    }

    /// Seed coffee-carrier state directly: the production path needs a full pantry
    /// wander trip, so this injects the end state to exercise steam rendering.
    #[cfg(test)]
    pub fn inject_coffee(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.session.office_mut().coffee.insert(id, fetched_at);
    }

    pub fn cached_layout(&self) -> Option<&SceneLayout> {
        self.cached_layout.as_deref()
    }

    /// The pixels cell `(col, row)` showed in the last frame drawn.
    pub(crate) fn scene_area_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<crate::tui::geometry::CellArea> {
        self.last_geometry?.area_at(col, row)
    }

    /// A left press at cell `(col, row)` at `now`: on what the last frame
    /// showed there, a click or a drag follows.
    pub(crate) fn press(&mut self, col: u16, row: u16, now: std::time::SystemTime) {
        let Some(cell) = self.scene_area_at(col, row) else {
            // As any press does: a carry whose release never came ends here.
            if let Some(ended) = self.pointer.cancel() {
                self.grip(&ended);
            }
            return;
        };
        let hit = self.cached_layout.as_deref().and_then(|layout| {
            crate::tui::hit_test::scene_hit(&self.last_hovers, self.last_star, layout, cell)
        });
        let bounds = cell.bounds();
        // A cell is the terminal's one step: any move to another lifts.
        let slop = pixtuoid_scene::interact::Slop {
            x: bounds.width,
            y: bounds.height,
        };
        let down = self.pointer.down(
            hit,
            centre(bounds),
            slop,
            self.chrome.active_pet.as_ref(),
            now,
        );
        if let Some(ended) = &down.ended {
            self.grip(ended);
        }
    }

    /// The pointer dragged to cell `(col, row)`: a figure lifted follows it.
    pub(crate) fn drag(&mut self, col: u16, row: u16) {
        let Some(cell) = self.scene_area_at(col, row) else {
            return;
        };
        if let Some(gesture) = self.pointer.moved(centre(cell.bounds())) {
            self.grip(&gesture);
        }
    }

    /// The press released at cell `(col, row)`: the click's action, the
    /// caller's to carry out; a figure carried is set down.
    pub(crate) fn release(&mut self, col: u16, row: u16) -> Option<pixtuoid_scene::hit::HitAction> {
        let at = self
            .scene_area_at(col, row)
            .map(|cell| centre(cell.bounds()));
        match self.pointer.up(at)? {
            pixtuoid_scene::interact::Gesture::Click(action) => Some(action),
            gesture => {
                self.grip(&gesture);
                None
            }
        }
    }

    /// Where a tooltip follows the pointer: nowhere while it carries a
    /// figure, which has none.
    fn tooltip_pos(&self) -> Option<(u16, u16)> {
        self.mouse_pos.filter(|_| !self.pointer.carrying())
    }

    fn grip(&mut self, gesture: &pixtuoid_scene::interact::Gesture) {
        self.session.grip(gesture);
    }

    /// What cell `(col, row)` showed the pointer in the last frame drawn.
    #[cfg(test)]
    pub(crate) fn scene_hit_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<crate::tui::hit_test::SceneHit<'_>> {
        let layout = self.cached_layout.as_deref()?;
        crate::tui::hit_test::scene_hit(
            &self.last_hovers,
            self.last_star,
            layout,
            self.scene_area_at(col, row)?,
        )
    }

    /// The agent topmost at cell `(col, row)` in the last frame drawn.
    #[cfg(test)]
    pub(crate) fn hit_test_agent_at(&self, col: u16, row: u16) -> Option<pixtuoid_core::AgentId> {
        match self.scene_hit_at(col, row)? {
            crate::tui::hit_test::SceneHit::Figure(
                pixtuoid_scene::display::HoverTarget::Agent(id),
            ) => Some(*id),
            _ => None,
        }
    }

    pub fn current_floor_seed(&self) -> u64 {
        FloorMeta::for_floor(self.session.nav().current(), self.session.n_floors()).floor_seed
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.session.nav().transition()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        self.session.navigate(target, now);
    }

    pub fn cancel_transition(&mut self) {
        self.session.cancel_slide();
    }

    pub fn set_mouse_pos(&mut self, pos: Option<(u16, u16)>) {
        self.mouse_pos = pos;
    }

    /// The current floor's last frame, `None` before its first.
    pub fn buf(&self) -> Option<&RgbBuffer> {
        self.session
            .floor(self.session.nav().current())?
            .raster
            .pixels()
    }

    pub fn set_theme(&mut self, theme: &'static pixtuoid_scene::theme::Theme) {
        if !std::ptr::eq(self.chrome.theme, theme) {
            self.chrome.theme = theme;
            for pf in self.session.floors_mut() {
                pf.raster.reset_sprite_cache();
            }
        }
    }

    /// Which weather every floor shows from the next frame on.
    pub fn set_weather(&mut self, weather: pixtuoid_scene::sky::WeatherPolicy) {
        self.chrome.weather = weather;
    }

    /// How every floor moves from the next frame on.
    pub fn set_motion(&mut self, motion: pixtuoid_scene::anim::Motion) {
        self.chrome.motion = motion;
    }

    pub fn set_theme_picker(&mut self, picker: Option<usize>) {
        self.chrome.theme_picker = picker;
    }

    pub fn set_source_warning(&mut self, warning: Option<String>) {
        self.chrome.source_warning = warning;
    }

    pub fn set_version_popup(&mut self, v: bool, now: SystemTime) {
        if v != self.chrome.popup.open {
            self.chrome.popup.scale_at_edge = self.version_popup_scale(now);
            self.chrome.popup.started_at = Some(now);
            self.chrome.popup.open = v;
        }
    }

    pub fn version_popup_started_at(&self) -> Option<SystemTime> {
        self.chrome.popup.started_at
    }

    pub fn version_popup_scale(&self, now: SystemTime) -> f32 {
        self.chrome.popup.scale(now)
    }

    /// The scale computed during the most recent `render()`.
    pub fn last_popup_scale(&self) -> f32 {
        self.chrome.popup.last_scale
    }

    pub fn set_active_pet(&mut self, pet: Option<PetState>) {
        self.chrome.active_pet = pet;
    }

    pub fn active_pet_ref(&self) -> Option<&PetState> {
        self.chrome.active_pet.as_ref()
    }

    /// The pet the last frame drew, found as the pointer finds it: on some
    /// logical pixel it is the topmost hover.
    #[cfg(test)]
    pub(crate) fn drawn_pet(&self) -> Option<pixtuoid_scene::display::PetHover> {
        let layout = self.cached_layout.as_deref()?;
        (0..layout.buf_h)
            .flat_map(|y| (0..layout.buf_w).map(move |x| (x, y)))
            .find_map(|(x, y)| {
                let pixel = pixtuoid_scene::layout::Bounds {
                    x,
                    y,
                    width: 1,
                    height: 1,
                };
                match self.last_hovers.at(pixel)? {
                    pixtuoid_scene::display::HoverTarget::Pet(pet) => Some(*pet),
                    _ => None,
                }
            })
    }

    /// Drop the agents gone from `scene`: [`OfficeSession::evict_missing`],
    /// which every frame's `prepare` runs.
    #[cfg(test)]
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.session.evict_missing(scene);
    }

    #[cfg(test)]
    pub fn coffee_contains(&self, id: AgentId) -> bool {
        self.session.office().coffee.map().contains_key(&id)
    }

    fn invalidate_routes(&mut self) {
        for pf in self.session.floors_mut() {
            pf.ctx.router.invalidate();
        }
    }

    /// The slide the session just composed, as classic half-blocks, under the
    /// destination's footer.
    fn flush_classic_slide(&mut self, scene: &SceneState, now: SystemTime) -> Result<()> {
        use crate::panels::paint_overlays;
        use crate::tui::renderer::{flush_buffer_to_term, paint_footer, scene_rect};
        self.forget_drawn();
        let term_size = self.terminal.size()?;
        let footer_scene = self.session.footer_scene(scene);
        let footer = pixtuoid_scene::footer::FooterInputs::new(
            &footer_scene,
            self.chrome.footer(&self.session, scene),
        );
        let popup_scale = self.version_popup_scale(now);
        let overlays = self.chrome.overlays(popup_scale);
        let theme = self.chrome.theme;
        let flashes = self.session.flashes();
        let Some(slide) = self.session.slide_mut() else {
            return Ok(());
        };
        // Modal backdrop: the same multiply a floor's frame takes.
        crate::tui::renderer::apply_dim(slide, self.chrome.onboarding.dim);
        if self.flash.holds(flashes, term_size) {
            return Ok(());
        }
        let slide = &*slide;
        self.terminal.draw(|f| {
            let full = f.area();
            paint_footer(f, &footer, full, theme);
            flush_buffer_to_term(f, slide, scene_rect(full));
            paint_overlays(f, &overlays, now, full, theme);
        })?;
        self.flash.shown(flashes, term_size);
        self.chrome.popup.last_scale = popup_scale;
        Ok(())
    }

    /// The footer-only frame of a terminal too small for the office; a slide
    /// it would hide ends at once rather than run its course unseen.
    fn draw_too_small(&mut self, scene: &SceneState, now: SystemTime) -> Result<()> {
        self.session.prepare(scene, now);
        self.cancel_transition();
        let footer_scene = self.session.footer_scene(scene);
        let footer = pixtuoid_scene::footer::FooterInputs::new(
            &footer_scene,
            self.chrome.footer(&self.session, scene),
        );
        let popup_scale = self.version_popup_scale(now);
        let drawn = crate::tui::renderer::draw_footer_only_frame(
            &mut self.terminal,
            &footer,
            self.chrome.theme,
            &self.chrome.overlays(popup_scale),
            now,
        );
        self.session.drew_no_office();
        self.record_drawn(
            scene,
            crate::tui::renderer::DrawOut::default(),
            popup_scale,
            now,
        );
        self.rest_floor(now);
        drawn
    }

    /// What the mouse handler hit-tests is gone from the screen, so a click
    /// must not land on its ghost.
    fn forget_drawn(&mut self) {
        self.cached_layout = None;
        self.last_hovers = Hovers::default();
        self.last_star = None;
        self.last_geometry = None;
    }

    /// A refused frame steps nothing, but the door's clamp still keeps time, as
    /// [`step_floor`](pixtuoid_scene::floor::step_floor) keeps it for a stepped one.
    fn rest_floor(&mut self, now: SystemTime) {
        if let Some((floor, _)) = self.session.floor_mut(self.session.nav().current()) {
            floor.ctx.recompute_door_anim_max_ms(now);
        }
    }

    /// What a floor frame leaves behind, whichever painter drew it: what the
    /// mouse hit-tests and the floor's audio.
    fn record_drawn(
        &mut self,
        scene: &SceneState,
        out: crate::tui::renderer::DrawOut,
        popup_scale: f32,
        now: SystemTime,
    ) {
        self.last_hovers = out.hovers;
        self.last_star = out.star;
        self.last_geometry = out.geometry;
        // Ambient audio: one AudioFrame per rendered frame, floor-scoped (you hear
        // the floor you're LOOKING AT; rain stays global), from the session's own
        // last frame of it.
        let office_floor = self.chrome.office_world(scene, &self.pack, now).floor;
        let audio_frame = self.session.audio_frame(scene, office_floor, now);
        // Composed even when disabled or muted: `AudioObserver::frame`'s contract.
        self.chrome.audio.frame(audio_frame);
        self.cached_layout = out.layout;
        // The popup's click rect derives from the terminal bounds — NOT the
        // office layout — so the painted scale IS the clickable one on both
        // draw paths.
        self.chrome.popup.last_scale = popup_scale;
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    /// Draw one frame of `scene`, then follow a terminal resize.
    ///
    /// # Errors
    ///
    /// If querying the terminal size, drawing the frame to the backend, or
    /// writing the held frame fails; a full terminal (`WouldBlock`) drops the
    /// frame instead.
    pub fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        let begun = std::time::Instant::now();
        if let Some(out) = &self.frame_out {
            out.begin();
        }
        let redrawn = if std::mem::take(&mut self.redraw_owed) {
            self.redraw()
        } else {
            Ok(())
        };
        let drawn = redrawn.and_then(|()| self.draw_frame(scene, pack, now));
        self.follow_resize();
        // Presented even when the draw failed, so nothing stays held.
        let presenting = std::time::Instant::now();
        let presented = self
            .frame_out
            .as_ref()
            .map_or(Ok(()), crate::tui::FrameOut::present);
        let present = presenting.elapsed();
        #[cfg(feature = "graphics")]
        if let Some(cutaway) = &mut self.cutaway {
            if presented.is_ok() {
                cutaway.landed();
            } else {
                cutaway.lost();
            }
        }
        let refused = std::mem::replace(&mut self.refusing, presented.is_err());
        if let Err(e) = presented {
            // A full terminal: the frame is dropped, and the next repaints
            // every cell and every tile.
            if e.kind() != std::io::ErrorKind::WouldBlock {
                return Err(e.into());
            }
            if refused {
                tracing::debug!(error = %e, "frame write failed");
            } else {
                tracing::warn!(error = %e, "frame write failed");
            }
            self.redraw_owed = true;
        }
        drawn?;
        #[cfg(feature = "graphics")]
        let send = self
            .cutaway
            .as_ref()
            .map(crate::tui::cutaway::TileCutaway::last_send);
        #[cfg(not(feature = "graphics"))]
        let send = None;
        let note = self
            .session
            .floor(self.session.nav().current())
            .and_then(|f| f.raster.note());
        self.jank.record(
            begun.elapsed(),
            present,
            send,
            note,
            std::time::Instant::now(),
        );
        Ok(())
    }

    fn draw_frame(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        if self
            .chrome
            .active_pet
            .as_ref()
            .is_some_and(|p| !p.is_active(now))
        {
            self.chrome.active_pet = None;
        }

        #[cfg(feature = "graphics")]
        if let Some(mut cutaway) = self.cutaway.take() {
            cutaway.begin_frame();
            cutaway.share_with_audio(self.chrome.audio.is_enabled());
            let size = self.terminal.size()?;
            let scene_area =
                crate::tui::renderer::scene_rect(Rect::new(0, 0, size.width, size.height));
            let window = self
                .terminal
                .backend_mut()
                .window_size()
                .ok()
                .and_then(crate::graphics::CellSize::of_window);
            let drawn = cutaway
                .fit_to(scene_area, window)
                .map(|fitted| self.render_cutaway(&mut cutaway, fitted, scene, pack, now));
            self.cutaway = Some(cutaway);
            if let Some(drawn) = drawn {
                return drawn;
            }
        }

        let term_size = self.terminal.size()?;
        let full_rect = Rect::new(0, 0, term_size.width, term_size.height);
        if crate::tui::renderer::scene_too_small(crate::tui::renderer::scene_rect(full_rect)) {
            return self.draw_too_small(scene, now);
        }
        let (buf_w, buf_h) =
            crate::tui::renderer::scene_buf_size(full_rect.width, full_rect.height);
        let layout = self.render_office(
            scene,
            pack,
            now,
            Look::Classic,
            Size { w: buf_w, h: buf_h },
            self.debug_walkable,
        );
        if self.session.nav().transition().is_some() {
            return self.flush_classic_slide(scene, now);
        }
        let Some(layout) = layout else {
            return self.draw_too_small(scene, now);
        };
        let flashes = self.session.flashes();
        if self.flash.holds(flashes, term_size) {
            // The terminal still shows the last frame, and its hit targets.
            return Ok(());
        }
        let current = self.session.nav().current();
        let floor_scene = project_floor_scene(scene, current);
        let footer_scene = self.session.footer_scene(scene);
        let footer = pixtuoid_scene::footer::FooterInputs::new(
            &footer_scene,
            self.chrome.footer(&self.session, scene),
        );
        let popup_scale = self.version_popup_scale(now);
        let mouse_pos = self.tooltip_pos();
        let world = self
            .chrome
            .world(&self.session, &floor_scene, pack, now, current);
        let overlays = self.chrome.overlays(popup_scale);
        let frame = crate::tui::renderer::ClassicFrame {
            footer: &footer,
            overlays: &overlays,
            theme: self.chrome.theme,
            world: &world,
            mouse_pos,
            dim: self.chrome.onboarding.dim,
        };
        let Some((floor, _)) = self.session.floor_mut(current) else {
            return Ok(());
        };
        let out = crate::tui::renderer::flush_classic(&mut self.terminal, &frame, layout, floor)?;
        self.flash.shown(flashes, term_size);
        self.record_drawn(scene, out, popup_scale, now);
        Ok(())
    }
}

#[cfg(feature = "graphics")]
impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    /// [`Self::render`] under the cutaway: the session's frame (the floor
    /// showing, or a slide's two floors composed with their text baked) as
    /// the image in place of the half-blocks, and the floor showing's badges,
    /// bubbles, wall board and floor indicator over it as terminal text, as
    /// the footer, tooltips and modals are.
    fn render_cutaway(
        &mut self,
        cutaway: &mut crate::tui::cutaway::TileCutaway,
        fitted: crate::tui::cutaway::Fitted,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
    ) -> Result<()> {
        use crate::panels::paint_overlays;
        use crate::panels::widgets::{paint_world, star_area};
        use crate::tui::hit_test::SceneHit;
        use crate::tui::renderer::{DrawOut, TooltipAt, paint_footer, scene_hit, scene_rect};
        use pixtuoid_scene::display::HoverTarget;
        if crate::tui::renderer::scene_too_small(fitted.scene) {
            return self.draw_too_small(scene, now);
        }
        let layout =
            self.render_office(scene, pack, now, fitted.look(), fitted.fit.logical(), false);
        let sliding = self.session.nav().transition().is_some();
        if sliding {
            self.forget_drawn();
            match self.session.buf() {
                Some(composed) => {
                    cutaway.paint_slide(fitted, composed, self.session.flashes(), now);
                }
                None => return self.draw_too_small(scene, now),
            }
        }
        let current = self.session.nav().current();
        let floor_scene = project_floor_scene(scene, current);
        let footer_scene = self.session.footer_scene(scene);
        let footer = pixtuoid_scene::footer::FooterInputs::new(
            &footer_scene,
            self.chrome.footer(&self.session, scene),
        );
        let popup_scale = self.version_popup_scale(now);
        let theme = self.chrome.theme;
        let world = self
            .chrome
            .world(&self.session, &floor_scene, pack, now, current);
        let overlays = self.chrome.overlays(popup_scale);
        /// What a frame sets over its image: the world's text, with the agent
        /// the pointer is on, and the tooltip by the pointer.
        struct Over<'w> {
            text: Option<(
                pixtuoid_scene::display::World<'w>,
                Option<pixtuoid_core::AgentId>,
            )>,
            tooltip: Option<(u16, u16, pixtuoid_scene::tooltip::Tooltip)>,
        }
        let shown = if sliding {
            None
        } else {
            let (Some(layout), Some(pixels)) = (layout, self.session.buf()) else {
                return self.draw_too_small(scene, now);
            };
            cutaway.paint(
                fitted,
                current,
                pixels,
                self.session.dirty().clone(),
                self.session.flash(),
                now,
            );
            let raster = self.session.floor(current).map(|floor| &floor.raster);
            let hovers = raster.and_then(|r| r.hovers().cloned()).unwrap_or_default();
            let geometry = fitted.geometry();
            let text = raster.and_then(pixtuoid_scene::look::Raster::world);
            let star = text.and_then(|text| star_area(text.signs, geometry.map()));
            let mouse = self.tooltip_pos().and_then(|(mx, my)| {
                let hit = scene_hit(&hovers, star, &layout, geometry.area_at(mx, my)?)?;
                Some((mx, my, hit))
            });
            let hovered = match mouse {
                Some((_, _, SceneHit::Figure(HoverTarget::Agent(id)))) => Some(*id),
                _ => None,
            };
            let tooltip = mouse.and_then(|(mx, my, hit)| {
                pixtuoid_scene::tooltip::for_hit(hit, &world).map(|tip| (mx, my, tip))
            });
            let drawn = DrawOut {
                layout: Some(layout),
                hovers,
                star,
                geometry: Some(geometry),
                // The cutaway holds its own tiles; the terminal's text draws.
                held: false,
            };
            let over = Over {
                text: text.map(|t| (t, hovered)),
                tooltip,
            };
            Some((drawn, over))
        };
        cutaway.before_flush(now);
        let mut carves = Vec::new();
        self.terminal.draw(|f| {
            let full = f.area();
            let scene_area = scene_rect(full);
            paint_footer(f, &footer, full, theme);
            cutaway.place(f.buffer_mut(), scene_area);
            let over = shown.as_ref().map(|(_, over)| over);
            if let Some((text, hovered)) = over.and_then(|o| o.text) {
                paint_world(f, text, (scene_area, fitted.geometry().map()), hovered);
            }
            if let Some((mx, my, tip)) = over.and_then(|o| o.tooltip.as_ref()) {
                let at = TooltipAt {
                    mx: *mx,
                    my: *my,
                    scene_rect: scene_area,
                };
                crate::tui::renderer::paint_tooltip(f, tip, at, theme);
            }
            paint_overlays(f, &overlays, now, full, theme);
            carves = cutaway.carve(f.buffer_mut(), scene_area);
        })?;
        cutaway.after_flush(&carves, now);
        match shown {
            Some((drawn, _)) => self.record_drawn(scene, drawn, popup_scale, now),
            None => self.chrome.popup.last_scale = popup_scale,
        }
        Ok(())
    }
}

/// Test-only access to the rendered ratatui frame, through a `TestBackend`
/// because only it exposes the post-draw cell buffer.
#[cfg(test)]
impl<B> TuiRenderer<B>
where
    B: Backend<Error: Send + Sync + 'static> + std::borrow::Borrow<ratatui::backend::TestBackend>,
{
    pub fn frame_buffer(&self) -> &ratatui::buffer::Buffer {
        self.terminal.backend().borrow().buffer()
    }
}

#[cfg(test)]
mod harness;

/// The pixel a cell showing `bounds` points at: its middle.
fn centre(bounds: pixtuoid_scene::layout::Bounds) -> pixtuoid_scene::layout::Point {
    pixtuoid_scene::layout::Point {
        x: bounds.x + bounds.width / 2,
        y: bounds.y + bounds.height / 2,
    }
}
