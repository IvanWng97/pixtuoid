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
use pixtuoid_scene::floor::{
    FloorInputs, FloorMeta, FloorTransition, FrameInputs, PerFloor, PerOffice, PetInputs,
    num_floors, project_floor_scene, render_floor,
};
use pixtuoid_scene::layout::{SceneLayout, Size};
use pixtuoid_scene::pathfind::Router;
use pixtuoid_scene::pet::PetFrame;

/// Floors `a` and `b`, which differ, borrowed together.
fn floor_pair(floors: &mut [PerFloor], a: usize, b: usize) -> (&mut PerFloor, &mut PerFloor) {
    if a < b {
        let (lo, hi) = floors.split_at_mut(b);
        (&mut lo[a], &mut hi[0])
    } else {
        let (lo, hi) = floors.split_at_mut(a);
        (&mut hi[0], &mut lo[b])
    }
}

fn floor_info_for(
    current_idx: usize,
    nf: usize,
    total_agents: usize,
) -> Option<pixtuoid_scene::footer::FooterFloor> {
    (nf > 1).then(|| pixtuoid_scene::footer::FooterFloor {
        current: current_idx + 1,
        total_floors: nf,
        total_agents,
    })
}

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

pub struct TuiRenderer<B: Backend<Error: Send + Sync + 'static>> {
    pub terminal: Terminal<B>,
    floors: Vec<PerFloor>,
    current_floor: usize,
    transition: Option<FloorTransition>,
    /// [`Self::scene_extent`] after the last frame.
    laid_out: Option<(u16, u16)>,
    mouse_pos: Option<(u16, u16)>,
    cached_layout: Option<Arc<SceneLayout>>,
    last_pet_pos: Option<PetFrame>,
    last_agents: Vec<pixtuoid_scene::pixel_painter::AgentFrame>,
    last_geometry: Option<crate::tui::geometry::SceneGeometry>,
    /// Coffee + venue chitchat, ONE per office — shared across every floor so a
    /// cup survives floor navigation.
    office: PerOffice,
    /// Live walkable/approach/route debug layer toggle (`w`); not persisted.
    debug_walkable: bool,
    chrome: Chrome,
    /// The cutaway, painted as terminal images in place of the half-blocks.
    #[cfg(feature = "graphics")]
    cutaway: Option<crate::tui::cutaway::TileCutaway>,
}

/// Everything a frame shows besides the floor: kept apart from `floors` and
/// `office` so a frame borrows it beside them ([`Chrome::frame`]).
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
    weather: pixtuoid_scene::pixel_painter::WeatherPolicy,
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
                floor_info_for(floor, nf, scene.agents.len()),
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
    ) -> Self {
        Self {
            terminal,
            floors: vec![PerFloor::new()],
            current_floor: 0,
            transition: None,
            laid_out: None,
            mouse_pos: None,
            cached_layout: None,
            last_pet_pos: None,
            last_agents: Vec::new(),
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
                weather: pixtuoid_scene::pixel_painter::WeatherPolicy::Clock,
                motion: pixtuoid_scene::anim::Motion::Full,
            },
            #[cfg(feature = "graphics")]
            cutaway: None,
        }
    }

    /// Paint `cutaway` from the next frame on.
    #[cfg(feature = "graphics")]
    pub(crate) fn set_cutaway(&mut self, cutaway: crate::tui::cutaway::TileCutaway) {
        self.cutaway = Some(cutaway);
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
    fn scene_extent(&self) -> (u16, u16) {
        #[cfg(feature = "graphics")]
        if let Some(office) = self.cutaway.as_ref().and_then(|c| c.extent()) {
            return (office.w, office.h);
        }
        (self.buf().width(), self.buf().height())
    }

    /// A frame laid out at a new extent — a resize — re-routes every floor and
    /// lands any slide.
    fn follow_resize(&mut self) {
        let extent = self.scene_extent();
        if self
            .laid_out
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
        self.current_floor
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
        self.floors.get(floor).map(|f| &f.buf)
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

    /// Whether the last frame drew the wall display's text: only a half-block
    /// frame does, and not a too-small one or a floor slide.
    pub(crate) fn shows_wall_display(&self) -> bool {
        matches!(
            self.last_geometry,
            Some(crate::tui::geometry::SceneGeometry::HalfBlock { .. })
        )
    }

    /// The pixels cell `(col, row)` showed in the last frame drawn.
    pub(crate) fn scene_area_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<crate::tui::geometry::CellArea> {
        self.last_geometry?.area_at(col, row)
    }

    /// [`hit_test_agent`](crate::tui::hit_test::hit_test_agent) against the last
    /// frame drawn.
    pub(crate) fn hit_test_agent_at(&self, col: u16, row: u16) -> Option<pixtuoid_core::AgentId> {
        let area = self.scene_area_at(col, row)?;
        #[cfg(feature = "graphics")]
        if let (Some(crate::tui::geometry::SceneGeometry::Cutaway { .. }), Some(cutaway)) =
            (self.last_geometry, &self.cutaway)
        {
            return cutaway.hover_at(area.bounds());
        }
        crate::tui::hit_test::hit_test_agent(&self.last_agents, area)
    }

    pub fn current_floor_seed(&self) -> u64 {
        let nf = self.floors.len();
        FloorMeta::for_floor(self.current_floor, nf).floor_seed
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.transition.as_ref()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        if target == self.current_floor || self.transition.is_some() {
            return;
        }
        self.transition = Some(FloorTransition::new(self.current_floor, target, now));
    }

    pub fn cancel_transition(&mut self) {
        if let Some(tr) = self.transition.take() {
            // Land on the destination floor: a resize-induced cancel must not
            // silently revert a user-initiated navigation.
            let nf = self.floors.len().max(1);
            self.current_floor = tr.to_floor.min(nf - 1);
        }
    }

    pub fn set_mouse_pos(&mut self, pos: Option<(u16, u16)>) {
        self.mouse_pos = pos;
    }

    pub fn buf(&self) -> &RgbBuffer {
        &self.floors[self.current_floor].buf
    }

    pub fn set_theme(&mut self, theme: &'static pixtuoid_scene::theme::Theme) {
        if !std::ptr::eq(self.chrome.theme, theme) {
            self.chrome.theme = theme;
            for pf in &mut self.floors {
                pf.ctx.cache = pixtuoid_scene::frame_cache::FrameCache::new();
            }
            #[cfg(feature = "graphics")]
            if let Some(cutaway) = &mut self.cutaway {
                cutaway.reset_cache();
            }
        }
    }

    /// Which weather every floor shows from the next frame on.
    pub fn set_weather(&mut self, weather: pixtuoid_scene::pixel_painter::WeatherPolicy) {
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

    pub fn cached_pet_pos(&self) -> Option<PetFrame> {
        self.last_pet_pos
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

    /// Call when the static walkable mask changes (terminal resize, floor capacity).
    pub fn invalidate_routes(&mut self) {
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
        let Some((from_floor, to_floor, t, going_down)) = self.transition.as_ref().map(|tr| {
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
                floor_info_for(to_floor, nf, scene.agents.len()),
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

        if scene_rect.width < crate::tui::renderer::MIN_SCENE_WIDTH
            || scene_rect.height < crate::tui::renderer::MIN_SCENE_HEIGHT
        {
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
            // This returns before ensure_size, so the floor buffer's size
            // signature never changes and `follow_resize` can't fire
            // cancel_transition: the slide would stay live for its whole
            // `FloorTransition::duration_ms`.
            self.cancel_transition();
            return Ok(());
        }

        let (buf_w, buf_h) =
            crate::tui::renderer::scene_buf_size(full_rect.width, full_rect.height);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);
        let onboarding_dim = self.chrome.onboarding.dim;

        let (
            PerFloor {
                ctx: from_ctx,
                buf: from_buf,
            },
            PerFloor {
                ctx: to_ctx,
                buf: to_buf,
            },
        ) = floor_pair(&mut self.floors, from_floor, to_floor);

        // Transitions hide *text* overlays (tooltips, bubbles, labels) but keep
        // every pixel-level visual, so the slide reads as a continuous scene.
        let mut transition_chitchat = std::collections::HashMap::new();

        // Recording the from-floor's carriers before the to-floor render can't
        // change the to-floor's pixels: an agent lives on exactly ONE floor, and
        // each projected floor scene paints only its own agents' coffee state.
        render_floor(
            from_ctx,
            from_buf,
            &mut self.office.coffee,
            &mut transition_chitchat,
            FrameInputs {
                world: self
                    .chrome
                    .slide_world(&from_scene, pack, now, from_floor, nf),
                theme: self.chrome.theme,
                size: Size { w: buf_w, h: buf_h },
                debug_walkable: self.debug_walkable,
            },
        );
        render_floor(
            to_ctx,
            to_buf,
            &mut self.office.coffee,
            &mut transition_chitchat,
            FrameInputs {
                world: self.chrome.slide_world(&to_scene, pack, now, to_floor, nf),
                theme: self.chrome.theme,
                size: Size { w: buf_w, h: buf_h },
                debug_walkable: self.debug_walkable,
            },
        );

        // Modal backdrop: dim BOTH sliding buffers, the same multiply draw_scene
        // applies to its single buffer.
        crate::tui::renderer::apply_dim(from_buf, onboarding_dim);
        crate::tui::renderer::apply_dim(to_buf, onboarding_dim);

        let (from_offset, to_offset) =
            crate::tui::geometry::slide_offsets(t, going_down, f32::from(scene_rect.height));

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

        self.chrome.popup.last_scale = popup_scale;
        Ok(())
    }

    /// What the mouse handler hit-tests is gone from the screen, so a click
    /// must not land on its ghost.
    fn forget_drawn(&mut self) {
        self.cached_layout = None;
        self.last_pet_pos = None;
        self.last_agents.clear();
        self.last_geometry = None;
    }

    /// The sim's per-frame epilogue for a frame
    /// [`pixtuoid_scene::floor::step_floor`] did not step: classic's, and a
    /// refused cutaway frame.
    fn sim_epilogue(&mut self, carriers: Vec<pixtuoid_core::AgentId>, now: SystemTime) {
        pixtuoid_scene::floor::frame_epilogue(
            &mut self.floors[self.current_floor].ctx,
            &mut self.office.coffee,
            carriers,
            now,
        );
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
        self.last_pet_pos = out.pet_pos;
        self.last_agents = out.agents;
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
                self.current_floor,
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
    pub fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        self.draw_frame(scene, pack, now)?;
        self.follow_resize();
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

        let nf = num_floors(scene).min(pixtuoid_scene::floor::MAX_FLOORS);

        while self.floors.len() < nf {
            self.floors.push(PerFloor::new());
        }

        if let Some(ref tr) = self.transition
            && (tr.from_floor >= nf || tr.to_floor >= nf)
        {
            self.transition = None;
            self.cached_layout = None;
        }

        if let Some(ref tr) = self.transition
            && tr.is_done(now)
        {
            self.current_floor = tr.to_floor;
            self.transition = None;
        }

        if self.current_floor >= nf {
            self.current_floor = nf.saturating_sub(1);
        }

        #[cfg(feature = "graphics")]
        if let Some(mut cutaway) = self.cutaway.take() {
            let size = self.terminal.size()?;
            let scene_area =
                crate::tui::renderer::scene_rect(Rect::new(0, 0, size.width, size.height));
            // The window's cell, not the plan's: a font zoom changes it.
            let cell = self
                .terminal
                .backend_mut()
                .window_size()
                .ok()
                .and_then(crate::graphics::CellSize::of_window);
            let drawn = cutaway.fit_to(scene_area, cell).map(|fitted| {
                if self.transition.is_some() {
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

        if self.transition.is_some() {
            return self.render_transition(scene, pack, now, nf);
        }

        let floor_scene = project_floor_scene(scene, self.current_floor);
        let Frame {
            world,
            footer,
            overlays,
        } = self
            .chrome
            .frame(scene, &floor_scene, pack, now, self.current_floor, nf);
        let popup_scale = overlays.popup_scale;
        let pf = &mut self.floors[self.current_floor];
        let mut draw_ctx = DrawCtx {
            world,
            buf: &mut pf.buf,
            store: &mut pf.ctx,
            mouse_pos: self.mouse_pos,
            debug_walkable: self.debug_walkable,
            theme: self.chrome.theme,
            theme_picker: overlays.theme_picker,
            footer,
            chitchat_state: &mut self.office.chitchat,
            coffee: self.office.coffee.map(),
            popup_scale: overlays.popup_scale,
            help_open: overlays.help_open,
            dashboard: overlays.dashboard,
            connection: overlays.connection,
            onboarding: overlays.onboarding,
        };
        let mut out = draw_scene(&mut self.terminal, &mut draw_ctx)?;
        let carriers = std::mem::take(&mut out.new_coffee_carriers);
        self.record_drawn(scene, out, popup_scale, now);
        self.sim_epilogue(carriers, now);
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
        crate::tui::cutaway::Fitted {
            scene: scene_area,
            office,
        }: crate::tui::cutaway::Fitted,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        nf: usize,
    ) -> Result<()> {
        use crate::tui::renderer::{
            draw_footer_only_frame, paint_footer, paint_overlays, scene_rect,
        };
        let Some((from_floor, to_floor, t, going_down)) = self.transition.as_ref().map(|tr| {
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
        // The destination floor's footer for the whole slide, as classic's.
        let Frame {
            footer, overlays, ..
        } = self.chrome.frame(scene, &to_scene, pack, now, to_floor, nf);
        let popup_scale = overlays.popup_scale;
        let footer = pixtuoid_scene::footer::FooterInputs::new(&to_scene, footer);
        let theme = self.chrome.theme;
        let too_small = scene_area.width < crate::tui::renderer::MIN_SCENE_WIDTH
            || scene_area.height < crate::tui::renderer::MIN_SCENE_HEIGHT;
        let (leaving, arriving) = floor_pair(&mut self.floors, from_floor, to_floor);
        let mut transition_chitchat = std::collections::HashMap::new();
        let mut step = |pf: &mut PerFloor, world| {
            pixtuoid_scene::floor::step_floor(
                &mut pf.ctx,
                &mut self.office.coffee,
                &mut transition_chitchat,
                world,
                office,
            )
        };
        let from_world = self
            .chrome
            .slide_world(&from_scene, pack, now, from_floor, nf);
        let to_world = self.chrome.slide_world(&to_scene, pack, now, to_floor, nf);
        let stepped = (!too_small)
            .then(|| Some((step(leaving, from_world)?, step(arriving, to_world)?)))
            .flatten();
        let Some((from_stepped, to_stepped)) = stepped else {
            let drawn = draw_footer_only_frame(&mut self.terminal, &footer, theme, &overlays, now);
            self.chrome.popup.last_scale = popup_scale;
            // As classic's: a slide nothing shows would otherwise run its course.
            self.cancel_transition();
            return drawn;
        };
        // Each floor shows its own board; only the footer is the destination's.
        let boards = [
            (&from_scene, from_floor, from_world),
            (&to_scene, to_floor, to_world),
        ]
        .map(|(floor_scene, i, world)| {
            let ctx = self
                .chrome
                .frame(scene, floor_scene, pack, now, i, nf)
                .footer;
            let footer = pixtuoid_scene::footer::FooterInputs::new(floor_scene, ctx);
            crate::tui::renderer::wall_board(&footer, floor_scene, world.floor.motion, now)
        });
        let showing = |floor, board| pixtuoid_scene::cutaway::paint::Showing { floor, now, board };
        cutaway.paint_slide(
            crate::tui::cutaway::Slide {
                leaving: (&from_stepped, showing(from_world.floor, &boards[0])),
                arriving: (&to_stepped, showing(to_world.floor, &boards[1])),
                t,
                going_down,
            },
            theme,
            now,
            scene_area.as_position(),
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
        crate::tui::cutaway::Fitted {
            scene: scene_area,
            office,
        }: crate::tui::cutaway::Fitted,
        scene: &SceneState,
        pack: &Pack,
        now: SystemTime,
        nf: usize,
    ) -> Result<()> {
        use crate::tui::renderer::{
            DrawOut, TooltipAt, draw_footer_only_frame, hit_test_coffee_machine,
            hit_test_furniture, paint_coffee_tooltip, paint_footer, paint_furniture_tooltip,
            paint_hover_tooltip, paint_overlays, scene_rect,
        };
        let floor_scene = project_floor_scene(scene, self.current_floor);
        let Frame {
            world,
            footer,
            overlays,
        } = self
            .chrome
            .frame(scene, &floor_scene, pack, now, self.current_floor, nf);
        let popup_scale = overlays.popup_scale;
        let footer = pixtuoid_scene::footer::FooterInputs::new(&floor_scene, footer);
        let theme = self.chrome.theme;
        let pf = &mut self.floors[self.current_floor];
        let too_small = scene_area.width < crate::tui::renderer::MIN_SCENE_WIDTH
            || scene_area.height < crate::tui::renderer::MIN_SCENE_HEIGHT;
        let stepped = (!too_small)
            .then(|| {
                pixtuoid_scene::floor::step_floor(
                    &mut pf.ctx,
                    &mut self.office.coffee,
                    &mut self.office.chitchat,
                    world,
                    office,
                )
            })
            .flatten();
        let Some(stepped) = stepped else {
            let drawn = draw_footer_only_frame(&mut self.terminal, &footer, theme, &overlays, now);
            self.record_drawn(scene, DrawOut::default(), popup_scale, now);
            self.sim_epilogue(Vec::new(), now);
            return drawn;
        };
        let board =
            crate::tui::renderer::wall_board(&footer, &floor_scene, world.floor.motion, now);
        let showing = pixtuoid_scene::cutaway::paint::Showing {
            floor: world.floor,
            now,
            board: &board,
        };
        cutaway.paint(&stepped, theme, showing, scene_area.as_position());
        let geometry = cutaway.geometry(scene_area);
        let layout = &stepped.layout;
        let mouse = self
            .mouse_pos
            .and_then(|(mx, my)| Some((mx, my, geometry.area_at(mx, my)?)));
        let hovered = mouse.and_then(|(.., cell)| cutaway.hover_at(cell.bounds()));
        cutaway.before_flush(now);
        let mut covered = Vec::new();
        self.terminal.draw(|f| {
            let full = f.area();
            let scene_area = scene_rect(full);
            paint_footer(f, &footer, full, theme);
            cutaway.place(f.buffer_mut(), scene_area);
            if let Some((mx, my, cell)) = mouse {
                let at = TooltipAt {
                    mx,
                    my,
                    scene_rect: scene_area,
                };
                // The agent first, then the classic's fall-through, less the
                // pet and mascots the cutaway does not report.
                if let Some(id) = hovered {
                    paint_hover_tooltip(f, &floor_scene, id, at, now, theme);
                } else if hit_test_coffee_machine(layout, cell) {
                    paint_coffee_tooltip(f, at, theme);
                } else if let Some(label) = hit_test_furniture(layout, cell) {
                    paint_furniture_tooltip(f, label, at, theme);
                }
            }
            paint_overlays(f, &overlays, now, full, theme);
            covered = cutaway.cover(f.buffer_mut(), scene_area);
        })?;
        cutaway.after_flush(&covered, now);
        self.record_drawn(
            scene,
            DrawOut {
                layout: Some(stepped.layout),
                occupied_waypoints: stepped.frame.occupied_waypoints,
                geometry: Some(geometry),
                ..DrawOut::default()
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
