//! `TuiRenderer` — the half-block terminal painter; its inherent `render` is
//! the production flush entry point.

use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::state::SceneState;
#[cfg(test)]
use pixtuoid_core::AgentId;

use ratatui::backend::Backend;
use ratatui::Terminal;

use ratatui::layout::Rect;

use crate::tui::renderer::{draw_scene, flush_buffer_to_term_at_offset, DrawCtx, PetState};
use pixtuoid_scene::floor::{
    num_floors, project_floor_scene, render_floor, FloorMeta, FloorTransition, FrameInputs,
    PerFloor, PerOffice,
};
use pixtuoid_scene::layout::{Layout, Size};
use pixtuoid_scene::pathfind::Router;
use pixtuoid_scene::pet::PetFrame;

fn floor_info_for(
    current_idx: usize,
    nf: usize,
    total_agents: usize,
) -> Option<crate::tui::renderer::FloorInfo> {
    (nf > 1).then(|| crate::tui::renderer::FloorInfo {
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
    theme: &'static pixtuoid_scene::theme::Theme,
    theme_picker: Option<usize>,
    cached_layout: Option<Arc<Layout>>,
    active_pet: Option<PetState>,
    last_pet_pos: Option<PetFrame>,
    pets: Vec<pixtuoid_scene::pet::Pet>,
    /// Coffee + venue chitchat, ONE per office — shared across every floor so a
    /// cup survives floor navigation.
    office: PerOffice,
    popup: PopupState,
    help_open: bool,
    /// Footer warning when a source has died; `None` while healthy.
    source_warning: Option<String>,
    /// Live walkable/approach/route debug layer toggle (`w`); not persisted.
    debug_walkable: bool,
    /// Agent-dashboard frame mirror. Kept here — disjoint from the floor buffers
    /// — so the painter can borrow it into the `DrawCtx` without fighting `floors`.
    dashboard: crate::tui::dashboard::DashboardFrame,
    connection: crate::tui::connection::ConnectionFrame,
    onboarding: crate::tui::welcome::OnboardingFrame,
    /// Ambient-audio gateway; inert unless installed.
    audio: crate::audio::AudioHandle,
    /// Transient +/- volume readout (percent); `None` outside the ~1s flash window.
    volume_flash: Option<u8>,
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
            theme,
            theme_picker: None,
            cached_layout: None,
            active_pet: None,
            last_pet_pos: None,
            pets,
            office: PerOffice::new(),
            popup: PopupState::default(),
            help_open: false,
            source_warning: None,
            debug_walkable: false,
            dashboard: Default::default(),
            connection: Default::default(),
            onboarding: crate::tui::welcome::OnboardingFrame::default(),
            audio: crate::audio::AudioHandle::disabled(),
            volume_flash: None,
        }
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    pub(crate) fn set_volume_flash(&mut self, flash: Option<u8>) {
        self.volume_flash = flash;
    }

    pub fn set_dashboard_frame(&mut self, frame: crate::tui::dashboard::DashboardFrame) {
        self.dashboard = frame;
    }

    pub fn set_connection_frame(&mut self, frame: crate::tui::connection::ConnectionFrame) {
        self.connection = frame;
    }

    #[cfg(test)]
    pub fn set_dashboard_frame_parts(
        &mut self,
        open: bool,
        rows: Vec<crate::tui::dashboard::DashboardRow>,
        selected: Option<pixtuoid_core::AgentId>,
        scroll: usize,
    ) {
        self.dashboard = crate::tui::dashboard::DashboardFrame {
            open,
            rows,
            selected,
            scroll,
        };
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_connection_frame_parts(
        &mut self,
        open: bool,
        rows: Vec<crate::tui::connection::ConnectionRow>,
        live: Vec<crate::tui::connection::LiveInfo>,
        selected: usize,
        confirm: Option<usize>,
        result: Option<String>,
        socket_line: String,
    ) {
        self.connection = crate::tui::connection::ConnectionFrame {
            open,
            rows,
            live,
            selected,
            confirm,
            result,
            socket_line,
        };
    }

    pub fn set_onboarding_frame(&mut self, frame: crate::tui::welcome::OnboardingFrame) {
        self.onboarding = frame;
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    pub fn set_help_open(&mut self, v: bool) {
        self.help_open = v;
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

    /// The click twin of the hover hit-test: anchors on `character_anchor`, so it
    /// follows a walking / wandering / entry / exit sprite. `floor_scene` must be
    /// projected to the visible floor — its `desk_index.single_floor_local()` reads
    /// need floor-local indices.
    pub(crate) fn hit_test_agent_at(
        &mut self,
        floor_scene: &SceneState,
        now: SystemTime,
        col: u16,
        row: u16,
    ) -> Option<pixtuoid_core::AgentId> {
        // Disjoint struct fields: this shared layout borrow coexists with the
        // `&mut route_ctx` below.
        let layout = self.cached_layout.as_deref()?;
        let mut rctx = self.floors[self.current_floor].ctx.route_ctx();
        crate::tui::hit_test::hit_test_agent(floor_scene, layout, now, &mut rctx, col, row)
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
        if !std::ptr::eq(self.theme, theme) {
            self.theme = theme;
            for pf in &mut self.floors {
                pf.ctx.cache = pixtuoid_scene::frame_cache::FrameCache::new();
            }
        }
    }

    pub fn set_theme_picker(&mut self, picker: Option<usize>) {
        self.theme_picker = picker;
    }

    pub fn set_source_warning(&mut self, warning: Option<String>) {
        self.source_warning = warning;
    }

    pub fn set_version_popup(&mut self, v: bool, now: SystemTime) {
        if v != self.popup.open {
            self.popup.scale_at_edge = self.version_popup_scale(now);
            self.popup.started_at = Some(now);
            self.popup.open = v;
        }
    }

    pub fn version_popup_started_at(&self) -> Option<SystemTime> {
        self.popup.started_at
    }

    pub fn version_popup_scale(&self, now: SystemTime) -> f32 {
        use pixtuoid_scene::anim::{eased_progress, Easing};
        const VERSION_POPUP_GROW_MS: u32 = 200;
        const VERSION_POPUP_SHRINK_MS: u32 = 120;
        match (self.popup.open, self.popup.started_at) {
            (true, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_GROW_MS, Easing::EaseOutCubic, now);
                self.popup.scale_at_edge + (1.0 - self.popup.scale_at_edge) * progress
            }
            (false, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_SHRINK_MS, Easing::EaseInQuad, now);
                self.popup.scale_at_edge * (1.0 - progress)
            }
            (true, None) => 1.0,
            (false, None) => 0.0,
        }
    }

    /// The scale computed during the most recent `render()`. Prefer this over
    /// `version_popup_scale(SystemTime::now())` in the mouse handler so click
    /// geometry matches what was painted.
    pub fn last_popup_scale(&self) -> f32 {
        self.popup.last_scale
    }

    pub fn set_active_pet(&mut self, pet: Option<PetState>) {
        self.active_pet = pet;
    }

    pub fn active_pet_ref(&self) -> Option<&PetState> {
        self.active_pet.as_ref()
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
    /// live floor count from [`render`].
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
        let from_scene = project_floor_scene(scene, from_floor);
        let to_scene = project_floor_scene(scene, to_floor);

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
            // Too small to render this frame: clear the interaction state the
            // mouse handler reads, so a click doesn't hit-test against a stale
            // layout / pet left over from a larger prior frame.
            self.cached_layout = None;
            self.last_pet_pos = None;
            // Paint the SAME footer-only frame draw_scene's gate does, not
            // nothing — else the stale pre-shrink frame stays frozen on screen.
            // AND land the transition: this returns before ensure_size, so the
            // floor buffer's size signature never changes and the event loop's
            // resize detector can't fire cancel_transition — the slide would
            // otherwise stay live for its whole ~400 ms timer.
            let floor_info = floor_info_for(to_floor, nf, scene.agents.len());
            let theme = self.theme;
            let source_warning = self.source_warning.clone();
            let per_floor = crate::tui::widgets::per_floor_counts(scene);
            let footer_stats = crate::tui::widgets::FooterStats {
                counts: per_floor[to_floor.min(pixtuoid_core::state::MAX_FLOORS - 1)],
                per_floor: &per_floor,
                gateway: crate::tui::widgets::gateway_rollup(scene.daemons().map(|(_, _, p)| p)),
                audio_audible: self.audio.is_audible(),
                volume_flash: self.volume_flash,
            };
            // The modals survive the slide (`Tab`/`s` aren't transition-gated), so
            // they paint here too — their key handlers stay live at every size.
            let popup_scale = self.version_popup_scale(now);
            self.popup.last_scale = popup_scale;
            let overlays = crate::tui::renderer::OverlayFrame {
                theme_picker: self.theme_picker,
                dashboard: &self.dashboard,
                connection: &self.connection,
                popup_scale,
                help_open: self.help_open,
                onboarding: &self.onboarding,
            };
            crate::tui::renderer::draw_footer_only_frame(
                &mut self.terminal,
                scene,
                &footer_stats,
                theme,
                floor_info,
                source_warning.as_deref(),
                &overlays,
                now,
            )?;
            self.cancel_transition();
            return Ok(());
        }

        let buf_w = scene_rect.width;
        let buf_h = scene_rect.height.saturating_mul(2);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);
        let onboarding_dim = self.onboarding.dim;

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
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == from_floor && p.is_active(now));
        let to_active_pet = self
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == to_floor && p.is_active(now));
        let from_pet = pixtuoid_scene::pet::select_pet_for_floor(from_meta.floor_seed, &self.pets);
        let to_pet = pixtuoid_scene::pet::select_pet_for_floor(to_meta.floor_seed, &self.pets);

        // Recording the from-floor's carriers before the to-floor render can't
        // change the to-floor's pixels: an agent lives on exactly ONE floor, and
        // each projected floor scene paints only its own agents' coffee state.
        render_floor(
            from_ctx,
            from_buf,
            &mut self.office.coffee,
            &mut transition_chitchat,
            FrameInputs {
                scene: &from_scene,
                pack,
                theme: self.theme,
                now,
                size: Size { w: buf_w, h: buf_h },
                scale: pixtuoid_scene::render_scale::RenderScale::ONE,
                floor_meta: from_meta,
                active_pet: from_active_pet,
                floor_pet: from_pet,
                debug_walkable: self.debug_walkable,
            },
        );
        render_floor(
            to_ctx,
            to_buf,
            &mut self.office.coffee,
            &mut transition_chitchat,
            FrameInputs {
                scene: &to_scene,
                pack,
                theme: self.theme,
                now,
                size: Size { w: buf_w, h: buf_h },
                scale: pixtuoid_scene::render_scale::RenderScale::ONE,
                floor_meta: to_meta,
                active_pet: to_active_pet,
                floor_pet: to_pet,
                debug_walkable: self.debug_walkable,
            },
        );

        // Modal backdrop: dim BOTH sliding buffers, the same multiply draw_scene
        // applies to its single buffer.
        if onboarding_dim < 0.999 {
            crate::tui::renderer::apply_dim(from_buf, onboarding_dim);
            crate::tui::renderer::apply_dim(to_buf, onboarding_dim);
        }

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

        let theme = self.theme;
        let theme_picker = self.theme_picker;
        let source_warning = self.source_warning.clone();
        let help_open = self.help_open;
        // Clone the frames for the brief transition rather than thread disjoint
        // borrows through the split_at_mut buffers.
        let dashboard = self.dashboard.clone();
        let connection = self.connection.clone();
        let onboarding = self.onboarding.clone();
        // Floor label tracks the destination floor for the whole slide so the
        // footer's per-floor agent count matches the label.
        let transition_floor_info = floor_info_for(to_floor, nf, scene.agents.len());
        let transition_per_floor = crate::tui::widgets::per_floor_counts(scene);
        let footer_stats = crate::tui::widgets::FooterStats {
            counts: crate::tui::widgets::scene_stats(&to_scene),
            per_floor: &transition_per_floor,
            gateway: crate::tui::widgets::gateway_rollup(scene.daemons().map(|(_, _, p)| p)),
            audio_audible: self.audio.is_audible(),
            volume_flash: self.volume_flash,
        };

        self.terminal.draw(|f| {
            let actual_full = f.area();
            let actual_scene = crate::tui::renderer::scene_rect(actual_full);
            crate::tui::renderer::paint_footer(
                f,
                &to_scene,
                &footer_stats,
                actual_full,
                theme,
                transition_floor_info,
                source_warning.as_deref(),
            );
            flush_buffer_to_term_at_offset(f, from_buf, actual_scene, from_offset);
            flush_buffer_to_term_at_offset(f, to_buf, actual_scene, to_offset);

            crate::tui::renderer::paint_overlays(
                f,
                &crate::tui::renderer::OverlayFrame {
                    theme_picker,
                    dashboard: &dashboard,
                    connection: &connection,
                    popup_scale,
                    help_open,
                    onboarding: &onboarding,
                },
                now,
                actual_full,
                theme,
            );
        })?;

        self.popup.last_scale = popup_scale;
        self.cached_layout = None;
        // The pet has no single interactable position mid-slide; clear the stale
        // one so the mouse handler can't "pet" a ghost at last frame's location.
        self.last_pet_pos = None;
        Ok(())
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    pub fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        if self.active_pet.as_ref().is_some_and(|p| !p.is_active(now)) {
            self.active_pet = None;
        }

        let nf = num_floors(scene).min(pixtuoid_scene::floor::MAX_FLOORS);

        while self.floors.len() < nf {
            self.floors.push(PerFloor::new());
        }

        if let Some(ref tr) = self.transition {
            if tr.from_floor >= nf || tr.to_floor >= nf {
                self.transition = None;
                self.cached_layout = None;
            }
        }

        if let Some(ref tr) = self.transition {
            if tr.is_done(now) {
                self.current_floor = tr.to_floor;
                self.transition = None;
            }
        }

        if self.current_floor >= nf {
            self.current_floor = nf.saturating_sub(1);
        }

        let floor_info = floor_info_for(self.current_floor, nf, scene.agents.len());

        if self.transition.is_some() {
            return self.render_transition(scene, pack, now, nf);
        }

        let floor_scene = project_floor_scene(scene, self.current_floor);

        let floor_meta = FloorMeta::for_floor(self.current_floor, nf);
        // Compute popup scale before the mutable borrows below.
        let popup_scale = self.version_popup_scale(now);
        let pf = &mut self.floors[self.current_floor];
        let mut draw_ctx = DrawCtx {
            buf: &mut pf.buf,
            store: &mut pf.ctx,
            mouse_pos: self.mouse_pos,
            debug_walkable: self.debug_walkable,
            theme: self.theme,
            theme_picker: self.theme_picker,
            floor_info,
            // Office-wide truth from the FULL un-projected scene: the footer's
            // cross-floor cue + gateway chip render even single-floor.
            per_floor: crate::tui::widgets::per_floor_counts(scene),
            gateway: crate::tui::widgets::gateway_rollup(scene.daemons().map(|(_, _, p)| p)),
            audio_audible: self.audio.is_audible(),
            volume_flash: self.volume_flash,
            floor: floor_meta,
            active_pet: self.active_pet.as_ref(),
            last_pet_pos: None,
            last_mascots: Vec::new(),
            floor_pet: pixtuoid_scene::pet::select_pet_for_floor(floor_meta.floor_seed, &self.pets),
            chitchat_state: &mut self.office.chitchat,
            chitchat_bubbles: Vec::new(),
            coffee: self.office.coffee.map(),
            new_coffee_carriers: Vec::new(),
            occupied_waypoints: Default::default(),
            popup_scale,
            help_open: self.help_open,
            source_warning: self.source_warning.as_deref(),
            dashboard: &self.dashboard,
            connection: &self.connection,
            onboarding: &self.onboarding,
        };
        let result = draw_scene(&mut self.terminal, &floor_scene, pack, now, &mut draw_ctx);
        self.last_pet_pos = draw_ctx.last_pet_pos;
        // `take` avoids a partial move so the explicit `drop` below can follow.
        let new_coffee_carriers = std::mem::take(&mut draw_ctx.new_coffee_carriers);
        let occupied_waypoints = std::mem::take(&mut draw_ctx.occupied_waypoints);
        drop(draw_ctx);
        // Ambient audio: one AudioFrame per rendered frame, floor-scoped (you hear
        // the floor you're LOOKING AT; rain stays global). The observer runs EVERY
        // frame, even muted, so its cue edges stay warm — re-enabling audio fires
        // no volley for what arrived while silent; only DELIVERY is gated. The
        // kind-map resolves against THIS frame's layout (the `result` handle, not
        // `self.cached_layout`, which is still last frame's until set below).
        let frame_layout = result.as_ref().ok().and_then(|o| o.as_deref());
        let audio_frame = self.office.audio.frame(
            scene,
            &occupied_waypoints,
            |idx| pixtuoid_scene::floor::waypoint_kind_of(frame_layout, idx),
            self.current_floor,
            now,
        );
        if self.audio.is_enabled() {
            self.audio.frame(audio_frame);
        }
        pixtuoid_scene::floor::frame_epilogue(
            &mut self.floors[self.current_floor].ctx,
            &mut self.office.coffee,
            new_coffee_carriers,
            now,
        );
        if let Ok(ref layout_opt) = result {
            self.cached_layout = layout_opt.clone();
            // The popup's click rect derives from the terminal bounds — NOT the
            // office layout — so the painted scale IS the clickable one on both
            // draw paths.
            self.popup.last_scale = popup_scale;
        } else {
            self.popup.last_scale = 0.0;
        }
        result.map(|_| ())
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
