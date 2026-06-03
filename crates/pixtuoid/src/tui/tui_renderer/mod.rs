//! `Renderer` trait impl that drives the half-block terminal TUI.
//!
//! Closes the v1 gap where production code called the free function
//! `draw_scene` directly, leaving the core `Renderer` trait exercised only
//! by `TestRenderer` in tests. `TuiRenderer` is the production impl: it
//! owns the cross-frame mutable state (`RgbBuffer`, `FrameCache`,
//! `AStarRouter`, `OccupancyOverlay`, `PoseHistory`) per floor and forwards
//! to `draw_scene`, which recomputes its own layout per frame from
//! `terminal.size()` because the user can resize at any time.

use std::time::SystemTime;

use anyhow::Result;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::SceneState;
#[cfg(test)]
use pixtuoid_core::AgentId;
use pixtuoid_core::Renderer;

use ratatui::backend::Backend;
use ratatui::Terminal;

use ratatui::layout::Rect;

use crate::tui::floor::{num_floors, project_floor_scene, FloorCtx, FloorMeta, FloorTransition};
use crate::tui::layout::{Layout, MAX_VISIBLE_DESKS};
use crate::tui::pathfind::Router;
use crate::tui::pet::PetFrame;
use crate::tui::pixel_painter::{render_to_rgb_buffer, PixelCtx};
use crate::tui::renderer::{draw_scene, flush_buffer_to_term_at_offset, DrawCtx, PetState};

pub struct TuiRenderer<B: Backend<Error: Send + Sync + 'static>> {
    pub terminal: Terminal<B>,
    floor_bufs: Vec<RgbBuffer>,
    floor_ctxs: Vec<FloorCtx>,
    current_floor: usize,
    transition: Option<FloorTransition>,
    mouse_pos: Option<(u16, u16)>,
    pinned_agent: Option<pixtuoid_core::AgentId>,
    pub ticker: crate::tui::renderer::TickerQueue,
    theme: &'static crate::tui::theme::Theme,
    theme_picker: Option<usize>,
    cached_layout: Option<Layout>,
    active_pet: Option<PetState>,
    last_pet_pos: Option<PetFrame>,
    /// Configured pets (kind + resolved display name), in order. Resolved once
    /// at startup by `config::resolve_pets`. `select_pet_for_floor` picks one
    /// per floor; the picked `&Pet` flows into `DrawCtx.floor_pet`. Replaces the
    /// former `enabled_pets: Vec<PetKind>` + `pet_names: HashMap` pair.
    pets: Vec<crate::tui::pet::Pet>,
    chitchat_state: std::collections::HashMap<
        crate::tui::chitchat::VenueKey,
        crate::tui::chitchat::ActiveChitchat,
    >,
    /// Persistent set of agents that have visited the pantry and carry a
    /// coffee cup back to their desk. Replaces the stateless
    /// `has_desk_coffee` cycle-scanning. Cleared on agent exit.
    coffee_holders: std::collections::HashSet<pixtuoid_core::AgentId>,
    /// Timestamp when each agent first returned with coffee (for steam).
    coffee_fetched_at: std::collections::HashMap<pixtuoid_core::AgentId, SystemTime>,
    version_popup: bool,
    version_popup_started_at: Option<SystemTime>,
    help_open: bool,
    /// Live walkable/approach/route debug layer toggle (`w`). Off by default,
    /// transient (not persisted).
    debug_walkable: bool,
    /// Scale captured at the moment of the last visible↔hidden edge so that
    /// an interrupted animation continues from its current position instead
    /// of snapping back to the start/end.
    version_popup_scale_at_edge: f32,
    /// Scale computed during the most recent `render()` call. The mouse
    /// handler reads this instead of re-computing with a fresh `SystemTime`
    /// so both sides always agree on whether the popup is above a threshold.
    last_popup_scale: f32,
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    pub fn new(
        terminal: Terminal<B>,
        theme: &'static crate::tui::theme::Theme,
        pets: Vec<crate::tui::pet::Pet>,
    ) -> Self {
        Self {
            terminal,
            floor_bufs: vec![RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 })],
            floor_ctxs: vec![FloorCtx::new()],
            current_floor: 0,
            transition: None,
            mouse_pos: None,
            pinned_agent: None,
            ticker: crate::tui::renderer::TickerQueue::new(),
            theme,
            theme_picker: None,
            cached_layout: None,
            active_pet: None,
            last_pet_pos: None,
            pets,
            chitchat_state: std::collections::HashMap::new(),
            coffee_holders: std::collections::HashSet::new(),
            coffee_fetched_at: std::collections::HashMap::new(),
            version_popup: false,
            version_popup_started_at: None,
            help_open: false,
            debug_walkable: false,
            version_popup_scale_at_edge: 0.0,
            last_popup_scale: 0.0,
        }
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

    /// Read a floor's per-agent motion map (test harness only) — used to
    /// assert that an off-screen floor freezes and resyncs on return.
    #[cfg(test)]
    pub fn floor_motion(
        &self,
        floor: usize,
    ) -> Option<&std::collections::HashMap<pixtuoid_core::AgentId, crate::tui::motion::MotionState>>
    {
        self.floor_ctxs.get(floor).map(|f| &f.motion)
    }

    /// Read a specific floor's pixel buffer (test harness only). `None` if the
    /// floor isn't allocated — lets a test assert vector growth and that the
    /// transition path paints both the from- and to-floor buffers.
    #[cfg(test)]
    pub fn floor_buf(&self, floor: usize) -> Option<&RgbBuffer> {
        self.floor_bufs.get(floor)
    }

    /// Seed coffee-carrier state directly (test harness only). The production
    /// path sets these on the coffee-carrier edge inside `render`, which
    /// requires driving a full pantry wander trip; this injects the end state
    /// so steam-window rendering can be exercised in isolation.
    #[cfg(test)]
    pub fn inject_coffee(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.coffee_holders.insert(id);
        self.coffee_fetched_at.insert(id, fetched_at);
    }

    pub fn cached_layout(&self) -> Option<&Layout> {
        self.cached_layout.as_ref()
    }

    pub fn current_floor_seed(&self) -> u64 {
        let nf = self.floor_ctxs.len();
        FloorMeta::for_floor(self.current_floor, nf).floor_seed
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.transition.as_ref()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        if target == self.current_floor || self.transition.is_some() {
            return;
        }
        self.set_pinned_agent(None);
        self.transition = Some(FloorTransition::new(self.current_floor, target, now));
    }

    pub fn cancel_transition(&mut self) {
        if let Some(tr) = self.transition.take() {
            // Land on the destination floor: a resize-induced cancel should
            // not silently revert a user-initiated navigation. Clamp against
            // the current floor count in case to_floor is now stale.
            let nf = self.floor_ctxs.len().max(1);
            self.current_floor = tr.to_floor.min(nf - 1);
        }
    }

    pub fn set_mouse_pos(&mut self, pos: Option<(u16, u16)>) {
        self.mouse_pos = pos;
    }

    pub fn pinned_agent(&self) -> Option<pixtuoid_core::AgentId> {
        self.pinned_agent
    }

    pub fn set_pinned_agent(&mut self, id: Option<pixtuoid_core::AgentId>) {
        self.pinned_agent = id;
    }

    pub fn buf(&self) -> &RgbBuffer {
        &self.floor_bufs[self.current_floor]
    }

    pub fn set_theme(&mut self, theme: &'static crate::tui::theme::Theme) {
        if !std::ptr::eq(self.theme, theme) {
            self.theme = theme;
            for ctx in &mut self.floor_ctxs {
                ctx.cache = crate::tui::frame_cache::FrameCache::new();
            }
        }
    }

    pub fn set_theme_picker(&mut self, picker: Option<usize>) {
        self.theme_picker = picker;
    }

    pub fn set_version_popup(&mut self, v: bool, now: SystemTime) {
        if v != self.version_popup {
            // Capture current scale so the new animation starts from the
            // visible position (no snap-back when interrupting mid-animation).
            self.version_popup_scale_at_edge = self.version_popup_scale(now);
            self.version_popup_started_at = Some(now);
            self.version_popup = v;
        }
    }

    pub fn version_popup_started_at(&self) -> Option<SystemTime> {
        self.version_popup_started_at
    }

    /// Compute the entrance/dismissal scale for the version popup based on
    /// the current state and the time since the last edge. Range 0.0..=1.0.
    ///
    /// - false → true (entrance): EaseOutCubic over 200ms, scale_at_edge → 1
    /// - true → false (dismissal): EaseInQuad over 120ms, scale_at_edge → 0
    /// - steady state: 1.0 if visible, 0.0 if hidden
    ///
    /// Using `scale_at_edge` as the interpolation start means an interrupted
    /// animation continues from its current visual position rather than
    /// snapping to 0 or 1 and re-animating from scratch.
    pub fn version_popup_scale(&self, now: SystemTime) -> f32 {
        use crate::tui::anim::{eased_progress, Easing};
        match (self.version_popup, self.version_popup_started_at) {
            (true, Some(start)) => {
                let progress = eased_progress(start, 200, Easing::EaseOutCubic, now);
                // Lerp from the scale at edge time to the target (1.0)
                self.version_popup_scale_at_edge
                    + (1.0 - self.version_popup_scale_at_edge) * progress
            }
            (false, Some(start)) => {
                let progress = eased_progress(start, 120, Easing::EaseInQuad, now);
                // Lerp from the scale at edge time to the target (0.0)
                self.version_popup_scale_at_edge * (1.0 - progress)
            }
            (true, None) => 1.0,
            (false, None) => 0.0,
        }
    }

    /// Returns the scale value computed during the most recent `render()`.
    /// Prefer this over calling `version_popup_scale(SystemTime::now())` in
    /// the mouse handler to keep click geometry in sync with what was painted.
    pub fn last_popup_scale(&self) -> f32 {
        self.last_popup_scale
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

    /// Drop the cached frame entries for agents no longer in `scene`.
    /// Forwarded so the render loop doesn't reach into the cache directly.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for ctx in &mut self.floor_ctxs {
            ctx.cache.evict_missing(scene);
        }
    }

    /// Whether an agent is a recorded coffee carrier (test harness only).
    #[cfg(test)]
    pub fn coffee_holders_contains(&self, id: AgentId) -> bool {
        self.coffee_holders.contains(&id)
    }

    /// Invalidate all floors' router path caches. Call when the static
    /// walkable mask changes (terminal resize, floor capacity change).
    pub fn invalidate_routes(&mut self) {
        for ctx in &mut self.floor_ctxs {
            ctx.router.invalidate();
        }
    }
    /// Composite two floors sliding in/out during a `FloorTransition` — the
    /// self-contained early-return arm split out of [`render`]. The transition's
    /// Copy fields are re-read up front (the caller only dispatches here when it
    /// is `Some`; a `None` slips through as a no-op `Ok`) so the rest of the body
    /// can borrow `&mut self` freely. `nf` is the live floor count from `render`.
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
        let make_floor_info = |current_idx: usize| {
            if nf > 1 {
                Some(crate::tui::renderer::FloorInfo {
                    current: current_idx + 1,
                    total_floors: nf,
                    total_agents: scene.agents.len(),
                })
            } else {
                None
            }
        };

        // Build floor-scoped scenes for both floors.
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

        if scene_rect.width < 20 || scene_rect.height < 12 {
            // Too small to render this frame: clear the interaction state the
            // mouse handler reads, so a click doesn't hit-test against a stale
            // layout / pet / popup left over from a larger prior frame.
            self.cached_layout = None;
            self.last_pet_pos = None;
            self.last_popup_scale = 0.0;
            return Ok(());
        }

        let buf_w = scene_rect.width;
        let buf_h = scene_rect.height.saturating_mul(2);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);

        // Render both floors into their respective buffers.
        // Use split_at_mut to get mutable access to two different indices.
        let (lo, hi) = if from_floor < to_floor {
            (from_floor, to_floor)
        } else {
            (to_floor, from_floor)
        };

        let (bufs_lo, bufs_hi) = self.floor_bufs.split_at_mut(hi);
        let lo_buf = &mut bufs_lo[lo];
        let hi_buf = &mut bufs_hi[0];
        let (from_buf, to_buf) = if from_floor < to_floor {
            (lo_buf, hi_buf)
        } else {
            (hi_buf, lo_buf)
        };

        let (ctxs_lo, ctxs_hi) = self.floor_ctxs.split_at_mut(hi);
        let lo_ctx = &mut ctxs_lo[lo];
        let hi_ctx = &mut ctxs_hi[0];
        let (from_ctx, to_ctx) = if from_floor < to_floor {
            (lo_ctx, hi_ctx)
        } else {
            (hi_ctx, lo_ctx)
        };

        from_buf.ensure_size(buf_w, buf_h, self.theme.surface.bg_fallback);
        to_buf.ensure_size(buf_w, buf_h, self.theme.surface.bg_fallback);

        let from_meta = FloorMeta::for_floor(from_floor, nf);
        let to_meta = FloorMeta::for_floor(to_floor, nf);

        // Transitions hide *text* overlays (tooltips, chitchat bubbles,
        // labels) but keep all pixel-level visuals — including pets,
        // coffee cups, and steam — so the slide reads as a continuous
        // scene rather than two stripped-down stand-ins.
        let mut transition_chitchat = std::collections::HashMap::new();

        let from_active_pet = self
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == from_floor && p.is_active(now));
        let to_active_pet = self
            .active_pet
            .as_ref()
            .filter(|p| p.floor_idx == to_floor && p.is_active(now));
        let from_pet = crate::tui::pet::select_pet_for_floor(from_meta.floor_seed, &self.pets);
        let to_pet = crate::tui::pet::select_pet_for_floor(to_meta.floor_seed, &self.pets);

        let from_carriers = render_transition_floor(
            &from_scene,
            from_ctx,
            from_buf,
            from_meta,
            buf_w,
            buf_h,
            from_active_pet,
            from_pet,
            self.theme,
            &self.coffee_holders,
            &self.coffee_fetched_at,
            &mut transition_chitchat,
            pack,
            now,
            self.debug_walkable,
        );
        let to_carriers = render_transition_floor(
            &to_scene,
            to_ctx,
            to_buf,
            to_meta,
            buf_w,
            buf_h,
            to_active_pet,
            to_pet,
            self.theme,
            &self.coffee_holders,
            &self.coffee_fetched_at,
            &mut transition_chitchat,
            pack,
            now,
            self.debug_walkable,
        );

        // Compute y-offsets for vertical slide with divider gap.
        // t applies to total travel = screen_height + divider_height
        // so the easing covers the full distance including the gap.
        let h = scene_rect.height as f32;
        let divider_h = (scene_rect.height as f32) / 5.0;
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
        let help_open = self.help_open;
        // Floor label tracks the destination floor for the duration of the
        // slide so the per-floor agent count in the footer matches the
        // label (otherwise users see "F1/3 ... 5 agents" with floor 2's
        // count for ~400 ms).
        let transition_floor_info = make_floor_info(to_floor);

        self.terminal.draw(|f| {
            let actual_full = f.area();
            let actual_scene = crate::tui::renderer::scene_rect(actual_full);
            crate::tui::renderer::paint_footer(
                f,
                &to_scene,
                actual_full,
                theme,
                transition_floor_info,
            );
            flush_buffer_to_term_at_offset(f, from_buf, actual_scene, from_offset);
            flush_buffer_to_term_at_offset(f, to_buf, actual_scene, to_offset);

            if let Some(idx) = theme_picker {
                crate::tui::renderer::paint_theme_picker(f, idx, actual_full, theme);
            }
            if popup_scale > 0.0 {
                if let Some(notes) = crate::version::release_notes(env!("CARGO_PKG_VERSION")) {
                    crate::tui::renderer::paint_version_popup(
                        f,
                        env!("CARGO_PKG_VERSION"),
                        notes,
                        actual_full,
                        theme,
                        popup_scale,
                        now,
                    );
                }
            }
            if help_open {
                // actual_full (not actual_scene) to match the theme
                // picker / version popup centering on this path too.
                crate::tui::renderer::paint_help_overlay(f, actual_full, theme);
            }
        })?;

        self.last_popup_scale = popup_scale;
        self.cached_layout = None;
        // The pet isn't rendered to a single interactable position mid-slide;
        // clear the stale position so the mouse handler can't "pet" a ghost at
        // last frame's location during the transition.
        self.last_pet_pos = None;
        // Persist coffee carriers detected on EITHER floor during the slide,
        // same EDGE logic as the normal path (insert returns true once per
        // pantry trip). Without this a coffee run that completes
        // mid-transition loses its cup.
        for id in from_carriers.into_iter().chain(to_carriers) {
            if self.coffee_holders.insert(id) {
                self.coffee_fetched_at.insert(id, now);
            }
        }
        Ok(())
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> Renderer for TuiRenderer<B> {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        // Auto-expire pet state.
        if self.active_pet.as_ref().is_some_and(|p| !p.is_active(now)) {
            self.active_pet = None;
        }

        self.ticker.update(scene);

        // Compute how many floors the current scene needs.
        let nf = num_floors(scene).min(crate::tui::floor::MAX_FLOORS);

        // Grow vectors if needed.
        while self.floor_bufs.len() < nf {
            self.floor_bufs
                .push(RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }));
        }
        while self.floor_ctxs.len() < nf {
            self.floor_ctxs.push(FloorCtx::new());
        }

        // Cancel transition if target floors no longer exist.
        if let Some(ref tr) = self.transition {
            if tr.from_floor >= nf || tr.to_floor >= nf {
                self.transition = None;
                self.cached_layout = None;
            }
        }

        // Complete transition if done.
        if let Some(ref tr) = self.transition {
            if tr.is_done(now) {
                self.current_floor = tr.to_floor;
                self.transition = None;
            }
        }

        // Clamp current_floor after transition completion.
        if self.current_floor >= nf {
            self.current_floor = nf.saturating_sub(1);
        }

        let make_floor_info = |current_idx: usize| {
            if nf > 1 {
                Some(crate::tui::renderer::FloorInfo {
                    current: current_idx + 1,
                    total_floors: nf,
                    total_agents: scene.agents.len(),
                })
            } else {
                None
            }
        };
        let floor_info = make_floor_info(self.current_floor);

        // --- Transition path: composite two floors sliding in/out ----------
        if self.transition.is_some() {
            return self.render_transition(scene, pack, now, nf);
        }

        // --- Normal path: single floor ------------------------------------
        let floor_scene = project_floor_scene(scene, self.current_floor);

        // Evict coffee state for agents no longer in the scene.
        self.coffee_holders
            .retain(|id| scene.agents.contains_key(id));
        self.coffee_fetched_at
            .retain(|id, _| scene.agents.contains_key(id));
        // Evict per-agent motion state for departed agents (mirrors the
        // coffee/history/cache eviction above).
        self.floor_ctxs[self.current_floor]
            .motion
            .retain(|id, _| scene.agents.contains_key(id));

        let floor_meta = FloorMeta::for_floor(self.current_floor, nf);
        // Compute popup scale before the mutable borrows below.
        let popup_scale = self.version_popup_scale(now);
        let fctx = &mut self.floor_ctxs[self.current_floor];
        let door_anim_max_ms = fctx.door_anim_max_ms;
        let mut draw_ctx = DrawCtx {
            buf: &mut self.floor_bufs[self.current_floor],
            cache: &mut fctx.cache,
            router: &mut fctx.router,
            overlay: &mut fctx.overlay,
            history: &mut fctx.history,
            motion: &mut fctx.motion,
            door_anim_max_ms,
            light: &mut fctx.light,
            mouse_pos: self.mouse_pos,
            pinned_agent: self.pinned_agent,
            debug_walkable: self.debug_walkable,
            ticker: &self.ticker,
            theme: self.theme,
            theme_picker: self.theme_picker,
            floor_info,
            floor: floor_meta,
            active_pet: self.active_pet.as_ref(),
            last_pet_pos: None,
            // Borrows `self.pets` immutably — disjoint from the `&mut fctx`
            // (self.floor_ctxs) above, so the field-split borrow is fine (same
            // as `&self.ticker`/`&self.coffee_holders` here). The picked `&Pet`
            // carries the name, so the tooltip needs no separate map.
            floor_pet: crate::tui::pet::select_pet_for_floor(floor_meta.floor_seed, &self.pets),
            chitchat_state: &mut self.chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee_holders: &self.coffee_holders,
            coffee_fetched_at: &self.coffee_fetched_at,
            new_coffee_carriers: Vec::new(),
            popup_scale,
            help_open: self.help_open,
        };
        let result = draw_scene(&mut self.terminal, &floor_scene, pack, now, &mut draw_ctx);
        self.last_pet_pos = draw_ctx.last_pet_pos;
        // Consume draw_ctx fields before the mutable borrow of floor_ctxs below.
        // std::mem::take avoids a partial move so drop(draw_ctx) can follow.
        let new_coffee_carriers = std::mem::take(&mut draw_ctx.new_coffee_carriers);
        // drop draw_ctx here so we can re-borrow floor_ctxs freely.
        drop(draw_ctx);
        // Recompute door_anim_max_ms from the motion map for the NEXT frame.
        self.floor_ctxs[self.current_floor].recompute_door_anim_max_ms(now);
        // Persist newly detected coffee carriers. The `insert` returns
        // `true` only on the EDGE (first time this agent enters the set
        // for this pantry trip).
        for id in new_coffee_carriers {
            if self.coffee_holders.insert(id) {
                self.coffee_fetched_at.insert(id, now);
            }
        }
        if let Ok(ref layout_opt) = result {
            self.cached_layout = layout_opt.clone();
        }
        self.last_popup_scale = popup_scale;
        result.map(|_| ())
    }
}

#[allow(clippy::too_many_arguments)]
fn render_transition_floor(
    scene: &SceneState,
    fctx: &mut FloorCtx,
    buf: &mut RgbBuffer,
    floor_meta: FloorMeta,
    buf_w: u16,
    buf_h: u16,
    active_pet: Option<&PetState>,
    floor_pet: Option<&crate::tui::pet::Pet>,
    theme: &'static crate::tui::theme::Theme,
    coffee_holders: &std::collections::HashSet<pixtuoid_core::AgentId>,
    coffee_fetched_at: &std::collections::HashMap<pixtuoid_core::AgentId, SystemTime>,
    chitchat_state: &mut std::collections::HashMap<
        crate::tui::chitchat::VenueKey,
        crate::tui::chitchat::ActiveChitchat,
    >,
    pack: &Pack,
    now: SystemTime,
    debug_walkable: bool,
) -> Vec<pixtuoid_core::AgentId> {
    let Some(layout) =
        Layout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, floor_meta.floor_seed)
    else {
        return Vec::new();
    };
    fctx.router.set_preferred_zone(layout.corridor);
    let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
        scene,
        layout: &layout,
        pack,
        now,
        buf,
        cache: &mut fctx.cache,
        router: &mut fctx.router,
        overlay: &mut fctx.overlay,
        history: &mut fctx.history,
        motion: &mut fctx.motion,
        door_anim_max_ms: fctx.door_anim_max_ms,
        theme,
        floor: floor_meta,
        active_pet,
        floor_pet,
        chitchat_state,
        coffee_holders,
        coffee_fetched_at,
        light: &mut fctx.light,
        debug_walkable,
    });
    // Mirror the normal-path refresh: render_to_rgb_buffer may have
    // snapshotted new entry/exit profiles into fctx.motion during the
    // ≤900ms slide, so refresh door_anim_max_ms for the next frame.
    fctx.recompute_door_anim_max_ms(now);
    // Return the coffee carriers detected this frame so the caller can
    // persist them — the normal path does this via DrawCtx.new_coffee_carriers
    // (renderer.rs); a transition bypasses draw_scene, so without threading
    // this back a pantry trip completed mid-slide is silently dropped (the cup
    // never lands on the desk).
    pixel_result.new_coffee_carriers
}

/// Test-only access to the rendered ratatui frame. This is rendered OUTPUT
/// (what the terminal would show), so widget/HUD/tooltip/footer assertions
/// inspect it rather than internal state. Specialised to `TestBackend` because
/// only it exposes the post-draw cell buffer.
#[cfg(test)]
impl TuiRenderer<ratatui::backend::TestBackend> {
    pub fn frame_buffer(&self) -> &ratatui::buffer::Buffer {
        self.terminal.backend().buffer()
    }
}

#[cfg(test)]
mod harness;
