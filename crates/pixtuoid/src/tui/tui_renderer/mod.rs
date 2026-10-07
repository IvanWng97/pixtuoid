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

use crate::tui::renderer::{DrawCtx, PetState, draw_scene, flush_buffer_to_term_at_offset};
use pixtuoid_scene::display::Hovers;
use pixtuoid_scene::floor::{
    FloorInputs, FloorMeta, FloorTransition, OfficeStores, PerFloor, PerOffice, PetInputs,
    num_floors, project_floor_scene,
};
use pixtuoid_scene::layout::{SceneLayout, Size};
#[cfg(feature = "graphics")]
use pixtuoid_scene::look::Rendered;
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
    floors: Vec<PerFloor>,
    nav: pixtuoid_scene::floor::FloorNav,
    /// [`Self::office_extent`] after the last frame.
    last_extent: Option<(u16, u16)>,
    mouse_pos: Option<(u16, u16)>,
    cached_layout: Option<Arc<SceneLayout>>,
    last_hovers: Hovers,
    last_star: Option<pixtuoid_scene::layout::Bounds>,
    /// The left button's gesture over the office, and the floor it grips.
    pointer: pixtuoid_scene::interact::Pointer,
    gripped: pixtuoid_scene::interact::GripFloor,
    last_geometry: Option<crate::tui::geometry::SceneGeometry>,
    /// Coffee + venue chitchat, ONE per office — shared across every floor so a
    /// cup survives floor navigation.
    office: PerOffice,
    /// Live walkable/approach/route debug layer toggle (`w`); not persisted.
    debug_walkable: bool,
    chrome: Chrome,
    /// The flashes the half-blocks show; the cutaway holds its own.
    flash: pixtuoid_scene::flash::FlashHold<pixtuoid_scene::flash::Flashes, ratatui::layout::Size>,
    /// The cutaway, painted as terminal images in place of the half-blocks.
    #[cfg(feature = "graphics")]
    cutaway: Option<crate::tui::cutaway::TileCutaway>,
}

/// Everything a frame shows besides the floor: kept apart from `floors` and
/// `office` so a frame borrows it beside them ([`Chrome::frame`]).
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
    dashboard: crate::tui::dashboard::DashboardFrame,
    connection: crate::tui::connection::ConnectionFrame,
    onboarding: crate::tui::welcome::OnboardingFrame,
    /// Ambient-audio gateway; inert unless installed.
    audio: crate::audio::AudioHandle,
    /// Transient +/- volume readout (percent); `None` past [`crate::audio::VOLUME_FLASH_MS`].
    volume_flash: Option<u8>,
    weather: pixtuoid_scene::sky::WeatherPolicy,
    motion: pixtuoid_scene::anim::Motion,
}

/// One floor frame's inputs, the same under either painter.
struct Frame<'a> {
    world: FloorInputs<'a>,
    footer: pixtuoid_scene::footer::FooterContext<'a>,
    overlays: crate::tui::renderer::OverlayFrame<'a>,
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
    /// Floor `floor` of `nf`, under this office's weather, moving as it does.
    fn floor_meta(&self, floor: usize, nf: usize) -> FloorMeta {
        FloorMeta::for_floor(floor, nf)
            .with_weather(self.weather)
            .with_motion(self.motion)
    }

    /// Floor `floor` of `nf` in `scene`, whose projection is `floor_scene`.
    fn frame<'a>(
        &'a self,
        scene: &'a SceneState,
        floor_scene: &'a SceneState,
        pack: &'a Pack,
        now: SystemTime,
        floor: usize,
        nf: usize,
    ) -> Frame<'a> {
        let meta = self.floor_meta(floor, nf);
        Frame {
            world: FloorInputs {
                scene: floor_scene,
                pack,
                now,
                floor: meta,
                pets: PetInputs {
                    pet: pixtuoid_scene::pet::select_pet_for_floor(meta.floor_seed, &self.pets),
                    petting: self.active_pet.as_ref(),
                },
            },
            footer: crate::tui::widgets::footer_context(
                scene,
                pixtuoid_scene::floor::footer_floor(floor, nf, scene.agents.len()),
                self.audio.is_audible(),
                self.volume_flash,
                self.source_warning.as_deref(),
            ),
            overlays: self.overlays(self.popup.scale(now)),
        }
    }

    /// Floor `floor` of `nf` while it slides, whose projection is
    /// `floor_scene`: its pet is petted only if the petting is there and live.
    fn slide_world<'a>(
        &'a self,
        floor_scene: &'a SceneState,
        pack: &'a Pack,
        now: SystemTime,
        floor: usize,
        nf: usize,
    ) -> FloorInputs<'a> {
        let meta = self.floor_meta(floor, nf);
        FloorInputs {
            scene: floor_scene,
            pack,
            now,
            floor: meta,
            pets: PetInputs {
                pet: pixtuoid_scene::pet::select_pet_for_floor(meta.floor_seed, &self.pets),
                petting: self
                    .active_pet
                    .as_ref()
                    .filter(|p| p.floor_idx == floor && p.is_active(now)),
            },
        }
    }

    fn overlays(&self, popup_scale: f32) -> crate::tui::renderer::OverlayFrame<'_> {
        crate::tui::renderer::OverlayFrame {
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
            floors: vec![PerFloor::new(Arc::clone(&pack))],
            pack,
            jank: crate::jank::Jank::new(std::time::Instant::now()),
            frame_out: None,
            redraw_owed: false,
            refusing: false,
            nav: pixtuoid_scene::floor::FloorNav::default(),
            last_extent: None,
            mouse_pos: None,
            cached_layout: None,
            last_hovers: Hovers::default(),
            last_star: None,
            pointer: pixtuoid_scene::interact::Pointer::default(),
            gripped: pixtuoid_scene::interact::GripFloor::default(),
            last_geometry: None,
            office: PerOffice::new(),
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
                onboarding: crate::tui::welcome::OnboardingFrame::default(),
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
        let nf = self.grow_floors(scene);
        let floor_scene = project_floor_scene(scene, self.nav.current());
        let Frame { world, footer, .. } =
            self.chrome
                .frame(scene, &floor_scene, pack, now, self.nav.current(), nf);
        let footer = pixtuoid_scene::footer::FooterInputs::new(&floor_scene, footer);
        self.office.raster.warm();
        let _ = pixtuoid_scene::look::render(
            &mut self.floors[self.nav.current()],
            self.office.stores(),
            look.0,
            RenderInputs {
                world,
                theme: self.chrome.theme,
                size: look.1,
                place: Place {
                    gateway: footer.context.gateway,
                    floor: footer.context.floor,
                },
                debug_walkable: false,
            },
        );
    }

    /// The floors `scene` fills, a raster each: what a frame, and the unseen
    /// boot frame, both stand on.
    fn grow_floors(&mut self, scene: &SceneState) -> usize {
        let nf = num_floors(scene).min(pixtuoid_scene::floor::MAX_FLOORS);
        while self.floors.len() < nf {
            self.floors.push(PerFloor::new(Arc::clone(&self.pack)));
        }
        nf
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
        Some((
            Look::Cutaway {
                scale: fitted.fit.render_scale(),
            },
            fitted.fit.logical(),
        ))
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
        for floor in &mut self.floors {
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

    pub fn set_dashboard_frame(&mut self, frame: crate::tui::dashboard::DashboardFrame) {
        self.chrome.dashboard = frame;
    }

    pub fn set_connection_frame(&mut self, frame: crate::tui::connection::ConnectionFrame) {
        self.chrome.connection = frame;
    }

    pub fn set_onboarding_frame(&mut self, frame: crate::tui::welcome::OnboardingFrame) {
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
        self.nav.current()
    }

    #[cfg(test)]
    pub fn floor_history(&self, floor: usize) -> Option<&pixtuoid_scene::pose::PoseHistory> {
        self.floors.get(floor).map(|f| &f.ctx.history)
    }

    #[cfg(test)]
    pub fn floor_walks(
        &self,
        floor: usize,
    ) -> Option<&std::collections::HashMap<pixtuoid_core::AgentId, pixtuoid_scene::walk::WalkState>>
    {
        self.floors.get(floor).map(|f| &f.ctx.walks)
    }

    #[cfg(test)]
    pub fn floor_buf(&self, floor: usize) -> Option<&RgbBuffer> {
        self.floors.get(floor)?.raster.pixels()
    }

    /// Seed coffee-carrier state directly: the production path needs a full pantry
    /// wander trip, so this injects the end state to exercise steam rendering.
    #[cfg(test)]
    pub fn inject_coffee(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.office.coffee.insert(id, fetched_at);
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
        if let Some(floor) = self
            .gripped
            .of(gesture, &self.nav)
            .and_then(|f| self.floors.get_mut(f))
        {
            floor.grip(gesture);
        }
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
        let nf = self.floors.len();
        FloorMeta::for_floor(self.nav.current(), nf).floor_seed
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.nav.transition()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        self.nav.navigate(target, now);
    }

    pub fn cancel_transition(&mut self) {
        self.nav.cancel(self.floors.len());
    }

    pub fn set_mouse_pos(&mut self, pos: Option<(u16, u16)>) {
        self.mouse_pos = pos;
    }

    /// The current floor's last frame, `None` before its first.
    pub fn buf(&self) -> Option<&RgbBuffer> {
        self.floors[self.nav.current()].raster.pixels()
    }

    pub fn set_theme(&mut self, theme: &'static pixtuoid_scene::theme::Theme) {
        if !std::ptr::eq(self.chrome.theme, theme) {
            self.chrome.theme = theme;
            for pf in &mut self.floors {
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

    /// Drop per-agent state for agents no longer in `scene` — BOTH halves: the
    /// per-floor caches on EVERY floor (an agent's floor need not be the current
    /// one) and the office-wide coffee cup. Keeping both on this ONE seam is what
    /// stops the transition render path, which short-circuits the normal frame
    /// body, from skipping either.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for pf in &mut self.floors {
            pf.evict_missing(scene);
        }
        self.office.evict_missing(scene);
    }

    #[cfg(test)]
    pub fn coffee_contains(&self, id: AgentId) -> bool {
        self.office.coffee.map().contains_key(&id)
    }

    fn invalidate_routes(&mut self) {
        for pf in &mut self.floors {
            pf.ctx.router.invalidate();
        }
    }

    /// Composite two floors sliding in/out during a `FloorTransition`. `nf` is the
    /// live floor count from [`Self::render`].
    fn render_transition(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        nf: usize,
    ) -> Result<()> {
        let Some((from_floor, to_floor, t, going_down)) = self.nav.transition().map(|tr| {
            (
                tr.from_floor,
                tr.to_floor,
                tr.t(now),
                tr.to_floor > tr.from_floor,
            )
        }) else {
            return Ok(());
        };
        self.forget_drawn();
        let from_scene = project_floor_scene(scene, from_floor);
        let to_scene = project_floor_scene(scene, to_floor);
        // The destination floor's footer for the whole slide, so its count matches
        // the breadcrumb.
        let footer = pixtuoid_scene::footer::FooterInputs::new(
            &to_scene,
            crate::tui::widgets::footer_context(
                scene,
                pixtuoid_scene::floor::footer_floor(to_floor, nf, scene.agents.len()),
                self.chrome.audio.is_audible(),
                self.chrome.volume_flash,
                self.chrome.source_warning.as_deref(),
            ),
        );

        let term_size = self.terminal.size()?;
        let full_rect = Rect {
            x: 0,
            y: 0,
            width: term_size.width,
            height: term_size.height,
        };
        let scene_rect = crate::tui::renderer::scene_rect(full_rect);

        if crate::tui::renderer::scene_too_small(scene_rect) {
            let popup_scale = self.version_popup_scale(now);
            self.chrome.popup.last_scale = popup_scale;
            let overlays = self.chrome.overlays(popup_scale);
            crate::tui::renderer::draw_footer_only_frame(
                &mut self.terminal,
                &footer,
                self.chrome.theme,
                &overlays,
                now,
            )?;
            // This returns before ensure_size, so `office_extent` holds and
            // `follow_resize` can't land the slide: it would stay live for its
            // whole `FloorTransition::duration_ms`.
            self.cancel_transition();
            return Ok(());
        }

        let (buf_w, buf_h) =
            crate::tui::renderer::scene_buf_size(full_rect.width, full_rect.height);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);
        let onboarding_dim = self.chrome.onboarding.dim;

        let Ok([from, to]) = self.floors.get_disjoint_mut([from_floor, to_floor]) else {
            tracing::warn!(
                from_floor,
                to_floor,
                "a slide between floors it cannot borrow"
            );
            self.cancel_transition();
            return Ok(());
        };

        // Transitions hide *text* overlays (tooltips, bubbles, labels) but keep
        // every pixel-level visual, so the slide reads as a continuous scene.
        let mut transition_chitchat = std::collections::HashMap::new();

        // Recording the from-floor's carriers before the to-floor render can't
        // change the to-floor's pixels: an agent lives on exactly ONE floor, and
        // each projected floor scene paints only its own agents' coffee state.
        let mut flashes = pixtuoid_scene::flash::Flashes::default();
        for (flash, (floor, world)) in flashes.iter_mut().zip([
            (
                &mut *from,
                self.chrome
                    .slide_world(&from_scene, pack, now, from_floor, nf),
            ),
            (
                &mut *to,
                self.chrome.slide_world(&to_scene, pack, now, to_floor, nf),
            ),
        ]) {
            *flash = pixtuoid_scene::look::render(
                floor,
                OfficeStores {
                    coffee: &mut self.office.coffee,
                    chitchat: &mut transition_chitchat,
                    raster: &mut self.office.raster,
                },
                Look::Classic,
                RenderInputs {
                    world,
                    theme: self.chrome.theme,
                    size: Size { w: buf_w, h: buf_h },
                    place: Place::default(),
                    debug_walkable: self.debug_walkable,
                },
            )
            .map(|frame| frame.flash)
            .unwrap_or_default();
            // Modal backdrop: dim BOTH sliding buffers, the same multiply
            // draw_scene applies to its single buffer.
            if let Some(drawn) = floor.raster.classic_drawn() {
                crate::tui::renderer::apply_dim(drawn.pixels, onboarding_dim);
            }
        }
        if self.flash.holds(flashes, term_size) {
            return Ok(());
        }
        let (Some(from_buf), Some(to_buf)) = (from.raster.pixels(), to.raster.pixels()) else {
            return Ok(());
        };

        let (from_offset, to_offset) =
            pixtuoid_scene::floor::slide_offsets(t, going_down, f32::from(scene_rect.height));

        let overlays = self.chrome.overlays(popup_scale);
        let theme = self.chrome.theme;
        self.terminal.draw(|f| {
            let actual_full = f.area();
            let actual_scene = crate::tui::renderer::scene_rect(actual_full);
            crate::tui::renderer::paint_footer(f, &footer, actual_full, theme);
            flush_buffer_to_term_at_offset(f, from_buf, actual_scene, from_offset);
            flush_buffer_to_term_at_offset(f, to_buf, actual_scene, to_offset);
            crate::tui::renderer::paint_overlays(f, &overlays, now, actual_full, theme);
        })?;
        self.flash.shown(flashes, term_size);

        self.chrome.popup.last_scale = popup_scale;
        Ok(())
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
        self.floors[self.nav.current()]
            .ctx
            .recompute_door_anim_max_ms(now);
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
        // the floor you're LOOKING AT; rain stays global). The kind-map resolves against
        // THIS frame's layout (`out.layout`, not `self.cached_layout`, which is
        // still last frame's until set below).
        let audio_frame = self.office.audio.frame(
            scene,
            &out.occupied_waypoints,
            |idx| pixtuoid_scene::floor::waypoint_kind_of(out.layout.as_deref(), idx),
            self.chrome.floor_meta(
                self.nav.current(),
                num_floors(scene).min(pixtuoid_scene::floor::MAX_FLOORS),
            ),
            now,
        );
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
            .floors
            .get(self.nav.current())
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

        let nf = self.grow_floors(scene);

        if self.nav.settle(nf, now) {
            self.cached_layout = None;
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
            let drawn = cutaway.fit_to(scene_area, window).map(|fitted| {
                if self.nav.transition().is_some() {
                    self.render_cutaway_slide(&mut cutaway, fitted, scene, pack, now, nf)
                } else {
                    self.render_cutaway(&mut cutaway, fitted, scene, pack, now, nf)
                }
            });
            self.cutaway = Some(cutaway);
            if let Some(drawn) = drawn {
                return drawn;
            }
        }

        if self.nav.transition().is_some() {
            return self.render_transition(scene, pack, now, nf);
        }

        let floor_scene = project_floor_scene(scene, self.nav.current());
        let Frame {
            world,
            footer,
            overlays,
        } = self
            .chrome
            .frame(scene, &floor_scene, pack, now, self.nav.current(), nf);
        let popup_scale = overlays.popup_scale;
        let mouse_pos = self.tooltip_pos();
        let mut draw_ctx = DrawCtx {
            world,
            floor: &mut self.floors[self.nav.current()],
            office: self.office.stores(),
            mouse_pos,
            debug_walkable: self.debug_walkable,
            theme: self.chrome.theme,
            theme_picker: overlays.theme_picker,
            footer,
            popup_scale: overlays.popup_scale,
            help_open: overlays.help_open,
            dashboard: overlays.dashboard,
            connection: overlays.connection,
            onboarding: overlays.onboarding,
            flash: Some(&mut self.flash),
        };
        let out = draw_scene(&mut self.terminal, &mut draw_ctx)?;
        if out.held {
            return Ok(());
        }
        if out.layout.is_none() {
            self.rest_floor(now);
        }
        self.record_drawn(scene, out, popup_scale, now);
        Ok(())
    }
}

#[cfg(feature = "graphics")]
impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    /// [`Self::render_transition`] under the cutaway: both floors' frames
    /// composed into one image that slides as classic's half-blocks do.
    fn render_cutaway_slide(
        &mut self,
        cutaway: &mut crate::tui::cutaway::TileCutaway,
        fitted: crate::tui::cutaway::Fitted,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        nf: usize,
    ) -> Result<()> {
        use crate::tui::renderer::{
            draw_footer_only_frame, paint_footer, paint_overlays, scene_rect,
        };
        let Some((from_floor, to_floor, t, going_down)) = self.nav.transition().map(|tr| {
            (
                tr.from_floor,
                tr.to_floor,
                tr.t(now),
                tr.to_floor > tr.from_floor,
            )
        }) else {
            return Ok(());
        };
        self.forget_drawn();
        let from_scene = project_floor_scene(scene, from_floor);
        let to_scene = project_floor_scene(scene, to_floor);
        let scene_area = fitted.scene;
        // The destination floor's footer for the whole slide, as classic's.
        let Frame {
            footer, overlays, ..
        } = self.chrome.frame(scene, &to_scene, pack, now, to_floor, nf);
        let popup_scale = overlays.popup_scale;
        let footer = pixtuoid_scene::footer::FooterInputs::new(&to_scene, footer);
        let theme = self.chrome.theme;
        let too_small = crate::tui::renderer::scene_too_small(scene_area);
        // Each floor shows its own board; only the footer is the destination's.
        let [from_place, to_place] =
            [(&from_scene, from_floor), (&to_scene, to_floor)].map(|(floor_scene, i)| {
                let ctx = self
                    .chrome
                    .frame(scene, floor_scene, pack, now, i, nf)
                    .footer;
                Place {
                    gateway: ctx.gateway,
                    floor: ctx.floor,
                }
            });
        let from_world = self
            .chrome
            .slide_world(&from_scene, pack, now, from_floor, nf);
        let to_world = self.chrome.slide_world(&to_scene, pack, now, to_floor, nf);
        let Ok([leaving, arriving]) = self.floors.get_disjoint_mut([from_floor, to_floor]) else {
            tracing::warn!(
                from_floor,
                to_floor,
                "a slide between floors it cannot borrow"
            );
            self.cancel_transition();
            return Ok(());
        };
        let mut transition_chitchat = std::collections::HashMap::new();
        let look = Look::Cutaway {
            scale: fitted.fit.render_scale(),
        };
        let mut render = |floor: &mut PerFloor, world, place| {
            let rendered = pixtuoid_scene::look::render(
                floor,
                OfficeStores {
                    coffee: &mut self.office.coffee,
                    chitchat: &mut transition_chitchat,
                    raster: &mut self.office.raster,
                },
                look,
                RenderInputs {
                    world,
                    theme,
                    size: fitted.fit.logical(),
                    place,
                    debug_walkable: false,
                },
            );
            rendered.map(|frame| frame.flash)
        };
        let flashes = (!too_small)
            .then(|| {
                Some([
                    render(&mut *leaving, from_world, from_place)?,
                    render(&mut *arriving, to_world, to_place)?,
                ])
            })
            .flatten();
        let (Some(flashes), Some(leaving), Some(arriving)) =
            (flashes, leaving.raster.pixels(), arriving.raster.pixels())
        else {
            let drawn = draw_footer_only_frame(&mut self.terminal, &footer, theme, &overlays, now);
            self.chrome.popup.last_scale = popup_scale;
            // As classic's: a slide nothing shows would otherwise run its course.
            self.cancel_transition();
            return drawn;
        };
        cutaway.paint_slide(
            fitted,
            crate::tui::cutaway::Slide {
                leaving,
                arriving,
                flashes,
                t,
                going_down,
            },
            theme,
            now,
        );
        cutaway.before_flush(now);
        let mut covered = Vec::new();
        self.terminal.draw(|f| {
            let full = f.area();
            let scene_area = scene_rect(full);
            paint_footer(f, &footer, full, theme);
            cutaway.place(f.buffer_mut(), scene_area);
            paint_overlays(f, &overlays, now, full, theme);
            covered = cutaway.cover(f.buffer_mut(), scene_area);
        })?;
        cutaway.after_flush(&covered, now);
        self.chrome.popup.last_scale = popup_scale;
        Ok(())
    }

    /// [`Self::render`] under the cutaway: the image in place of the
    /// half-blocks, its badges, wall board and floor indicator painted in it,
    /// and only the footer, tooltips and modals as terminal text.
    fn render_cutaway(
        &mut self,
        cutaway: &mut crate::tui::cutaway::TileCutaway,
        fitted: crate::tui::cutaway::Fitted,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        nf: usize,
    ) -> Result<()> {
        use crate::tui::renderer::{
            DrawOut, TooltipAt, draw_footer_only_frame, paint_footer, paint_overlays,
            paint_scene_tooltip, scene_hit, scene_rect,
        };
        let scene_area = fitted.scene;
        let floor_scene = project_floor_scene(scene, self.nav.current());
        let Frame {
            world,
            footer,
            overlays,
        } = self
            .chrome
            .frame(scene, &floor_scene, pack, now, self.nav.current(), nf);
        let popup_scale = overlays.popup_scale;
        let footer = pixtuoid_scene::footer::FooterInputs::new(&floor_scene, footer);
        let theme = self.chrome.theme;
        let too_small = crate::tui::renderer::scene_too_small(scene_area);
        let rendered = (!too_small)
            .then(|| {
                pixtuoid_scene::look::render(
                    &mut self.floors[self.nav.current()],
                    self.office.stores(),
                    Look::Cutaway {
                        scale: fitted.fit.render_scale(),
                    },
                    RenderInputs {
                        world,
                        theme,
                        size: fitted.fit.logical(),
                        place: Place {
                            gateway: footer.context.gateway,
                            floor: footer.context.floor,
                        },
                        debug_walkable: false,
                    },
                )
            })
            .flatten();
        let Some(Rendered {
            pixels,
            dirty,
            layout: frame_layout,
            occupied_waypoints,
            flash,
        }) = rendered
        else {
            let drawn = draw_footer_only_frame(&mut self.terminal, &footer, theme, &overlays, now);
            self.record_drawn(scene, DrawOut::default(), popup_scale, now);
            self.rest_floor(now);
            return drawn;
        };
        cutaway.paint(fitted, self.nav.current(), pixels, dirty, flash, now);
        let raster = &self.floors[self.nav.current()].raster;
        let hovers = raster.hovers().cloned().unwrap_or_default();
        let star = raster.star();
        let geometry = fitted.geometry();
        let mouse = self.tooltip_pos().and_then(|(mx, my)| {
            let hit = scene_hit(&hovers, star, &frame_layout, geometry.area_at(mx, my)?)?;
            Some((mx, my, hit))
        });
        cutaway.before_flush(now);
        let mut covered = Vec::new();
        self.terminal.draw(|f| {
            let full = f.area();
            let scene_area = scene_rect(full);
            paint_footer(f, &footer, full, theme);
            cutaway.place(f.buffer_mut(), scene_area);
            if let Some((mx, my, hit)) = &mouse {
                let at = TooltipAt {
                    mx: *mx,
                    my: *my,
                    scene_rect: scene_area,
                };
                paint_scene_tooltip(f, hit, &world, at, theme);
            }
            paint_overlays(f, &overlays, now, full, theme);
            covered = cutaway.cover(f.buffer_mut(), scene_area);
        })?;
        cutaway.after_flush(&covered, now);
        self.record_drawn(
            scene,
            DrawOut {
                layout: Some(frame_layout),
                hovers,
                star,
                occupied_waypoints,
                geometry: Some(geometry),
                // The cutaway holds its own tiles; the terminal's text draws.
                held: false,
            },
            popup_scale,
            now,
        );
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
