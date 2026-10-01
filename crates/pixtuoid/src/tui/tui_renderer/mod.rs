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
use pixtuoid_scene::layout::{Layout, Size};
use pixtuoid_scene::pathfind::Router;
use pixtuoid_scene::pet::PetFrame;

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
    mouse_pos: Option<(u16, u16)>,
    cached_layout: Option<Arc<Layout>>,
    last_pet_pos: Option<PetFrame>,
    last_agents: Vec<pixtuoid_scene::pixel_painter::AgentFrame>,
    last_geometry: Option<crate::tui::geometry::SceneGeometry>,
    /// Coffee + venue chitchat, ONE per office — shared across every floor so a
    /// cup survives floor navigation.
    office: PerOffice,
    /// Live walkable/approach/route debug layer toggle (`w`); not persisted.
    debug_walkable: bool,
    chrome: Chrome,
    /// The cutaway, painted over kitty's protocol in place of the half-blocks.
    #[cfg(feature = "graphics")]
    kitty: Option<crate::tui::cutaway::KittyCutaway>,
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
        let meta = FloorMeta::for_floor(floor, nf);
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
            },
            #[cfg(feature = "graphics")]
            kitty: None,
        }
    }

    /// Paint the cutaway through `kitty` from the next frame on.
    #[cfg(feature = "graphics")]
    pub(crate) fn set_kitty(&mut self, kitty: crate::tui::cutaway::KittyCutaway) {
        self.kitty = Some(kitty);
    }

    /// Clear the terminal and repaint every cell and every image.
    pub(crate) fn redraw(&mut self) -> Result<()> {
        #[cfg(feature = "graphics")]
        if let Some(kitty) = &mut self.kitty {
            kitty.forget();
        }
        self.terminal.clear()?;
        Ok(())
    }

    /// The logical extent the current floor last laid out on: what a resize
    /// changes.
    pub(crate) fn scene_extent(&self) -> (u16, u16) {
        #[cfg(feature = "graphics")]
        if self.kitty.is_some() {
            return self
                .cached_layout
                .as_deref()
                .map_or((0, 0), |l| (l.buf_w, l.buf_h));
        }
        (self.buf().width(), self.buf().height())
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
    pub fn floor_motion(
        &self,
        floor: usize,
    ) -> Option<
        &std::collections::HashMap<pixtuoid_core::AgentId, pixtuoid_scene::motion::MotionState>,
    > {
        self.floors.get(floor).map(|f| &f.ctx.motion)
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

    pub fn cached_layout(&self) -> Option<&Layout> {
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
        if let Some(kitty) = &self.kitty {
            return kitty.hover_at(area.bounds());
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
            if let Some(kitty) = &mut self.kitty {
                kitty.reset_cache();
            }
        }
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
            // signature never changes and the event loop's resize detector can't
            // fire cancel_transition: the slide would stay live for its whole
            // `FloorTransition::duration_ms`.
            self.cancel_transition();
            return Ok(());
        }

        let (buf_w, buf_h) =
            crate::tui::renderer::scene_buf_size(full_rect.width, full_rect.height);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);
        let onboarding_dim = self.chrome.onboarding.dim;

        let (lo, hi) = if from_floor < to_floor {
            (from_floor, to_floor)
        } else {
            (to_floor, from_floor)
        };

        let (floors_lo, floors_hi) = self.floors.split_at_mut(hi);
        let lo_floor = &mut floors_lo[lo];
        let hi_floor = &mut floors_hi[0];
        let (from_floor_half, to_floor_half) = if from_floor < to_floor {
            (lo_floor, hi_floor)
        } else {
            (hi_floor, lo_floor)
        };
        let PerFloor {
            ctx: from_ctx,
            buf: from_buf,
        } = from_floor_half;
        let PerFloor {
            ctx: to_ctx,
            buf: to_buf,
        } = to_floor_half;

        let from_meta = FloorMeta::for_floor(from_floor, nf);
        let to_meta = FloorMeta::for_floor(to_floor, nf);

        // Transitions hide *text* overlays (tooltips, bubbles, labels) but keep
        // every pixel-level visual, so the slide reads as a continuous scene.
        let mut transition_chitchat = std::collections::HashMap::new();

        let from_active_pet = self
            .chrome
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == from_floor && p.is_active(now));
        let to_active_pet = self
            .chrome
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == to_floor && p.is_active(now));
        let from_pet =
            pixtuoid_scene::pet::select_pet_for_floor(from_meta.floor_seed, &self.chrome.pets);
        let to_pet =
            pixtuoid_scene::pet::select_pet_for_floor(to_meta.floor_seed, &self.chrome.pets);

        // Recording the from-floor's carriers before the to-floor render can't
        // change the to-floor's pixels: an agent lives on exactly ONE floor, and
        // each projected floor scene paints only its own agents' coffee state.
        render_floor(
            from_ctx,
            from_buf,
            &mut self.office.coffee,
            &mut transition_chitchat,
            FrameInputs {
                world: FloorInputs {
                    scene: &from_scene,
                    pack,
                    now,
                    floor: from_meta,
                    pets: PetInputs {
                        pet: from_pet,
                        petting: from_active_pet,
                    },
                },
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
                world: FloorInputs {
                    scene: &to_scene,
                    pack,
                    now,
                    floor: to_meta,
                    pets: PetInputs {
                        pet: to_pet,
                        petting: to_active_pet,
                    },
                },
                theme: self.chrome.theme,
                size: Size { w: buf_w, h: buf_h },
                debug_walkable: self.debug_walkable,
            },
        );

        // Modal backdrop: dim BOTH sliding buffers, the same multiply draw_scene
        // applies to its single buffer.
        crate::tui::renderer::apply_dim(from_buf, onboarding_dim);
        crate::tui::renderer::apply_dim(to_buf, onboarding_dim);

        // `t` applies to the total travel (screen height + divider gap) so the
        // easing covers the full distance including the gap.
        const FLOOR_SLIDE_DIVIDER_FRACTION: f32 = 5.0;
        let h = scene_rect.height as f32;
        let divider_h = (scene_rect.height as f32) / FLOOR_SLIDE_DIVIDER_FRACTION;
        let total = h + divider_h;
        let (from_offset, to_offset) = if going_down {
            // Higher floor: current slides DOWN, new enters from TOP
            let from_y = (t * total) as i32;
            let to_y = -(total - t * total) as i32;
            (from_y, to_y)
        } else {
            // Lower floor: current slides UP, new enters from BOTTOM
            let from_y = -(t * total) as i32;
            let to_y = (total - t * total) as i32;
            (from_y, to_y)
        };

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

    /// What a floor frame leaves behind, whichever painter drew it: what the
    /// mouse hit-tests, the floor's audio, and the sim's epilogue.
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
            self.current_floor,
            now,
        );
        // Composed even when disabled or muted: `AudioObserver::frame`'s contract.
        self.chrome.audio.frame(audio_frame);
        pixtuoid_scene::floor::frame_epilogue(
            &mut self.floors[self.current_floor].ctx,
            &mut self.office.coffee,
            out.new_coffee_carriers,
            now,
        );
        self.cached_layout = out.layout;
        // The popup's click rect derives from the terminal bounds — NOT the
        // office layout — so the painted scale IS the clickable one on both
        // draw paths.
        self.chrome.popup.last_scale = popup_scale;
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    pub fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
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
        if let Some(mut kitty) = self.kitty.take() {
            // No slide yet: the floor changes at once.
            self.cancel_transition();
            let drawn = self.render_kitty(&mut kitty, scene, pack, now, nf);
            self.kitty = Some(kitty);
            return drawn;
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
        let out = draw_scene(&mut self.terminal, &mut draw_ctx)?;
        self.record_drawn(scene, out, popup_scale, now);
        Ok(())
    }
}

#[cfg(feature = "graphics")]
impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    /// [`Self::render`] under the kitty cutaway: the image in place of the
    /// half-blocks, and the text a later PR does not move onto the canvas.
    fn render_kitty(
        &mut self,
        kitty: &mut crate::tui::cutaway::KittyCutaway,
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
        let size = self.terminal.size()?;
        let scene_area = scene_rect(Rect::new(0, 0, size.width, size.height));
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
        let observed = (!too_small)
            .then(|| {
                pixtuoid_scene::floor::observe_floor(
                    &mut pf.ctx,
                    &mut self.office.coffee,
                    &mut self.office.chitchat,
                    world,
                    kitty.fit_to(scene_area),
                )
            })
            .flatten();
        let Some(observed) = observed else {
            let drawn = draw_footer_only_frame(&mut self.terminal, &footer, theme, &overlays, now);
            self.record_drawn(scene, DrawOut::default(), popup_scale, now);
            return drawn;
        };
        kitty.paint(&observed, theme, world.floor, now);
        let geometry = kitty.geometry(scene_area);
        let layout = &observed.layout;
        let mouse = self
            .mouse_pos
            .and_then(|(mx, my)| Some((mx, my, geometry.area_at(mx, my)?)));
        let hovered = mouse.and_then(|(.., cell)| kitty.hover_at(cell.bounds()));
        let kitty = &*kitty;
        self.terminal.draw(|f| {
            let full = f.area();
            let scene_area = scene_rect(full);
            paint_footer(f, &footer, full, theme);
            kitty.place(f.buffer_mut(), scene_area);
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
        })?;
        self.record_drawn(
            scene,
            DrawOut {
                layout: Some(observed.layout),
                occupied_waypoints: observed.frame.occupied_waypoints,
                geometry: Some(geometry),
                ..DrawOut::default()
            },
            popup_scale,
            now,
        );
        Ok(())
    }
}

/// Test-only access to the rendered ratatui frame. Specialised to `TestBackend`
/// because only it exposes the post-draw cell buffer.
#[cfg(test)]
impl TuiRenderer<ratatui::backend::TestBackend> {
    pub fn frame_buffer(&self) -> &ratatui::buffer::Buffer {
        self.terminal.backend().buffer()
    }
}

#[cfg(test)]
mod harness;
