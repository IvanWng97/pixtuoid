//! `TuiRenderer` — the half-block terminal painter. Its inherent `render` method
//! is the production flush entry point. It owns the cross-frame mutable state (`RgbBuffer`,
//! `FrameCache`, `AStarRouter`, `OccupancyOverlay`, `PoseHistory`) per floor and
//! forwards to `draw_scene`, which recomputes its own layout per frame from
//! `terminal.size()` because the user can resize at any time.

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

/// `FloorInfo` for a 1-based floor index, or `None` when there is only one floor
/// (no elevator indicator). The one source behind both the normal and the
/// floor-transition draw paths (was a closure duplicated byte-for-byte in each).
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

/// The version popup's animation state machine — the four values move as a unit
/// (`set_version_popup` stamps `open` + `started_at` + `scale_at_edge` together;
/// `version_popup_scale` reads all three; `last_scale` caches the per-frame
/// result). Internal to `TuiRenderer`; only the computed scale reaches `DrawCtx`.
#[derive(Debug, Default)]
struct PopupState {
    /// In its "shown" state — drives the scale toward 1.0 (vs 0.0 when hidden).
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
    /// Per-floor session halves (sim/paint stores + pixel buffer), the
    /// scene-owned `PerFloor` type — the multi-floor composition of the
    /// single-floor painters' `FloorSession` (this painter drives
    /// `draw_scene`/`render_floor` itself, so it composes the halves).
    floors: Vec<PerFloor>,
    current_floor: usize,
    transition: Option<FloorTransition>,
    mouse_pos: Option<(u16, u16)>,
    theme: &'static pixtuoid_scene::theme::Theme,
    theme_picker: Option<usize>,
    cached_layout: Option<Arc<Layout>>,
    active_pet: Option<PetState>,
    last_pet_pos: Option<PetFrame>,
    /// Configured pets (kind + resolved display name), in order. Resolved once
    /// at startup by `config::resolve_pets`. `select_pet_for_floor` picks one
    /// per floor; the picked `&Pet` flows into `DrawCtx.floor_pet`. Replaces the
    /// former `enabled_pets: Vec<PetKind>` + `pet_names: HashMap` pair.
    pets: Vec<pixtuoid_scene::pet::Pet>,
    /// The per-OFFICE session half (scene-owned `PerOffice`): persistent
    /// coffee bookkeeping + venue chitchat, ONE per office, shared across
    /// every floor so a cup survives floor navigation (#423). Coffee is
    /// evicted on agent exit through `PerOffice::evict_missing`.
    office: PerOffice,
    /// The version popup's animation state machine (open flag + edge clock +
    /// edge-scale + last-rendered-scale all move as a unit — see `PopupState`).
    popup: PopupState,
    help_open: bool,
    /// Footer warning when a source has died (#157); `None` while healthy.
    /// Set per-frame from the runtime's health channel, like the popup state.
    source_warning: Option<String>,
    /// Live walkable/approach/route debug layer toggle (`w`). Off by default,
    /// transient (not persisted).
    debug_walkable: bool,
    /// Agent-dashboard frame mirror, pushed each tick by the event loop via
    /// `set_dashboard_frame` (one snapshot, rows pre-built from the live scene).
    /// Kept here — disjoint from the floor buffers — so the painter can borrow it
    /// into the `DrawCtx` without fighting the `floors` (per-floor session) borrows.
    dashboard: crate::tui::dashboard::DashboardFrame,
    /// Sources-panel frame mirror, pushed each tick via `set_connection_frame`.
    /// One snapshot the painter reads: the event loop builds the HOOK facet
    /// (`rows`, cached on open/after-actions) + the LIVE facet (`live`, per-frame)
    /// then hands both over together. Disjoint from the floor buffers.
    connection: crate::tui::connection::ConnectionFrame,
    /// First-run onboarding overlay frame, pushed by the event loop via
    /// `set_onboarding_frame` (the four values bundled in one `OnboardingFrame` —
    /// they always move together). Kept here, disjoint from the floor buffers, for
    /// borrow-free `DrawCtx` assembly.
    onboarding: crate::tui::welcome::OnboardingFrame,
    /// Ambient-audio gateway (#633). The per-frame `AudioFrame` is composed
    /// through the shared office [`AudioObserver`](pixtuoid_scene::floor::AudioObserver)
    /// (`self.office.audio`), which owns the cue tracker + floor-reprime latch.
    /// Inert unless installed.
    audio: crate::audio::AudioHandle,
    /// Transient +/- volume readout (percent), pushed per frame by the
    /// event loop; `None` outside the ~1s flash window.
    volume_flash: Option<u8>,
}

/// "You would hear sound right now": live handle, not effectively muted
/// (m OR pause — the combined state on the handle), and volume above zero
/// (a 0% ♩ would advertise sound that isn't playing). A free fn over the
/// handle FIELD so call sites keep their disjoint borrows of the rest of
/// the renderer (a `&self` method would borrow all of `*self`).
fn audio_audible(audio: &crate::audio::AudioHandle) -> bool {
    audio.is_enabled() && !audio.is_muted() && audio.volume() > 0.0
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

    /// Install the ambient-audio gateway (inert by default — the disabled
    /// handle swallows everything, so render code never needs a cfg/if).
    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Per-frame mirror of the transient +/- readout: `Some(percent)` for
    /// ~1s after a volume nudge, else `None` (the footer appends it to ♩).
    pub(crate) fn set_volume_flash(&mut self, flash: Option<u8>) {
        self.volume_flash = flash;
    }

    /// Mirror the dashboard frame the event loop built this tick (a pre-built row
    /// snapshot — a cheap clone, `AgentId` is `Copy`, strings are `Arc` — rather
    /// than the whole `DashboardUi`, avoiding a per-frame clone of the fold sets).
    pub fn set_dashboard_frame(&mut self, frame: crate::tui::dashboard::DashboardFrame) {
        self.dashboard = frame;
    }

    /// Mirror the Sources-panel frame the event loop built this tick (the cached
    /// HOOK facet `rows` + the per-frame LIVE facet `live`, handed over together).
    pub fn set_connection_frame(&mut self, frame: crate::tui::connection::ConnectionFrame) {
        self.connection = frame;
    }

    /// Test seam: build + push a `DashboardFrame` from positional parts so the
    /// harness fixtures stay terse. Production uses `set_dashboard_frame`.
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

    /// Test seam: build + push a `ConnectionFrame` from positional parts.
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

    /// Mirror the first-run onboarding frame the event loop built this tick
    /// (`OnboardingFrame::elapsed_ms` is the time since the overlay opened, which
    /// drives the painter's typewriter).
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

    /// Read a floor's pose history (test harness only) — used to assert that
    /// departed agents are evicted on every floor.
    #[cfg(test)]
    pub fn floor_history(&self, floor: usize) -> Option<&pixtuoid_scene::pose::PoseHistory> {
        self.floors.get(floor).map(|f| &f.ctx.history)
    }

    /// Read a floor's per-agent motion map (test harness only) — used to
    /// assert that an off-screen floor freezes and resyncs on return.
    #[cfg(test)]
    pub fn floor_motion(
        &self,
        floor: usize,
    ) -> Option<
        &std::collections::HashMap<pixtuoid_core::AgentId, pixtuoid_scene::motion::MotionState>,
    > {
        self.floors.get(floor).map(|f| &f.ctx.motion)
    }

    /// Read a specific floor's pixel buffer (test harness only). `None` if the
    /// floor isn't allocated — lets a test assert vector growth and that the
    /// transition path paints both the from- and to-floor buffers.
    #[cfg(test)]
    pub fn floor_buf(&self, floor: usize) -> Option<&RgbBuffer> {
        self.floors.get(floor).map(|f| &f.buf)
    }

    /// Seed coffee-carrier state directly (test harness only). The production
    /// path sets these on the coffee-carrier edge inside `render`, which
    /// requires driving a full pantry wander trip; this injects the end state
    /// so steam-window rendering can be exercised in isolation.
    #[cfg(test)]
    pub fn inject_coffee(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.office.coffee.insert(id, fetched_at);
    }

    pub fn cached_layout(&self) -> Option<&Layout> {
        self.cached_layout.as_deref()
    }

    /// The click twin of the hover hit-test (`renderer.rs`): anchors on
    /// `character_anchor`, so it follows a walking / wandering / entry / exit
    /// sprite — unlike home-desk-only `hit_test_from_tui`. Pass the event loop's
    /// current `now` (the same clock `render()` runs on); `derive_with_routing` is
    /// idempotent per `now`, so re-deriving here is a state no-op. `floor_scene`
    /// must be projected to the visible floor (its `desk_index.single_floor_local()`
    /// reads need floor-local indices). `None` on a too-small/transition frame.
    pub(crate) fn hit_test_agent_at(
        &mut self,
        floor_scene: &SceneState,
        now: SystemTime,
        col: u16,
        row: u16,
    ) -> Option<pixtuoid_core::AgentId> {
        // cached_layout / floors / current_floor are disjoint struct fields, so
        // this shared layout borrow coexists with the &mut route_ctx below.
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
            // Land on the destination floor: a resize-induced cancel should
            // not silently revert a user-initiated navigation. Clamp against
            // the current floor count in case to_floor is now stale.
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
            // Capture current scale so the new animation starts from the
            // visible position (no snap-back when interrupting mid-animation).
            self.popup.scale_at_edge = self.version_popup_scale(now);
            self.popup.started_at = Some(now);
            self.popup.open = v;
        }
    }

    pub fn version_popup_started_at(&self) -> Option<SystemTime> {
        self.popup.started_at
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
        use pixtuoid_scene::anim::{eased_progress, Easing};
        // Version popup entrance (grow) and dismissal (shrink) durations.
        const VERSION_POPUP_GROW_MS: u32 = 200;
        const VERSION_POPUP_SHRINK_MS: u32 = 120;
        match (self.popup.open, self.popup.started_at) {
            (true, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_GROW_MS, Easing::EaseOutCubic, now);
                // Lerp from the scale at edge time to the target (1.0)
                self.popup.scale_at_edge + (1.0 - self.popup.scale_at_edge) * progress
            }
            (false, Some(start)) => {
                let progress =
                    eased_progress(start, VERSION_POPUP_SHRINK_MS, Easing::EaseInQuad, now);
                // Lerp from the scale at edge time to the target (0.0)
                self.popup.scale_at_edge * (1.0 - progress)
            }
            (true, None) => 1.0,
            (false, None) => 0.0,
        }
    }

    /// Returns the scale value computed during the most recent `render()`.
    /// Prefer this over calling `version_popup_scale(SystemTime::now())` in
    /// the mouse handler to keep click geometry in sync with what was painted.
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

    /// Drop per-agent state for agents no longer in `scene` — cached frames,
    /// pose history, and motion (walk-path/profile) entries — across EVERY
    /// floor (an agent's state lives on its own floor, which need not be the
    /// current one). The event loop calls this with the live snapshot before
    /// each render; keeping all per-agent eviction on this one seam means the
    /// transition render path (which short-circuits the normal frame body)
    /// can't skip it.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for pf in &mut self.floors {
            pf.evict_missing(scene);
        }
    }

    /// Whether an agent is a recorded coffee carrier (test harness only).
    #[cfg(test)]
    pub fn coffee_contains(&self, id: AgentId) -> bool {
        self.office.coffee.map().contains_key(&id)
    }

    /// Invalidate all floors' router path caches. Call when the static
    /// walkable mask changes (terminal resize, floor capacity change).
    pub fn invalidate_routes(&mut self) {
        for pf in &mut self.floors {
            pf.ctx.router.invalidate();
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

        if scene_rect.width < crate::tui::renderer::MIN_SCENE_WIDTH
            || scene_rect.height < crate::tui::renderer::MIN_SCENE_HEIGHT
        {
            // Too small to render this frame: clear the interaction state the
            // mouse handler reads, so a click doesn't hit-test against a stale
            // layout / pet / popup left over from a larger prior frame.
            self.cached_layout = None;
            self.last_pet_pos = None;
            self.popup.last_scale = 0.0;
            // Paint the SAME footer-only frame draw_scene's gate does (shared
            // MIN_SCENE_* threshold ⇒ shared behavior), not nothing — else the
            // stale pre-shrink frame stays frozen on screen. AND land the
            // transition: this returns before render_floor/ensure_size, so the
            // floor buffer's size signature never changes and the event loop's
            // resize detector can't fire cancel_transition — the slide would
            // otherwise stay live hitting this path for its whole ~400 ms timer.
            let floor_info = floor_info_for(to_floor, nf, scene.agents.len());
            let theme = self.theme;
            let source_warning = self.source_warning.clone();
            // Office-wide tallies from the full scene; the footer's rungs are the
            // DESTINATION floor's slice (matches `floor_info`'s to_floor breadcrumb).
            let per_floor = crate::tui::widgets::per_floor_counts(scene);
            let footer_stats = crate::tui::widgets::FooterStats {
                counts: per_floor[to_floor.min(pixtuoid_core::state::MAX_FLOORS - 1)],
                per_floor: &per_floor,
                gateway: crate::tui::widgets::gateway_rollup(scene.daemons()),
                audio_audible: audio_audible(&self.audio),
                volume_flash: self.volume_flash,
            };
            crate::tui::renderer::draw_footer_only_frame(
                &mut self.terminal,
                scene,
                &footer_stats,
                theme,
                floor_info,
                source_warning.as_deref(),
            )?;
            self.cancel_transition();
            return Ok(());
        }

        let buf_w = scene_rect.width;
        let buf_h = scene_rect.height.saturating_mul(2);
        // Compute popup scale before the split_at_mut borrows.
        let popup_scale = self.version_popup_scale(now);
        // The onboarding modal-backdrop dim (same field draw_scene reads) — captured
        // before the split_at_mut borrows `self.floors`, applied to BOTH sliding
        // buffers below so a floor change mid-open lowers the lights on the whole
        // office, matching the single-buffer path (#R0d82-item4).
        let onboarding_dim = self.onboarding.dim;

        // Render both floors into their respective buffers.
        // Use split_at_mut to get mutable access to two different indices.
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
        let from_pet = pixtuoid_scene::pet::select_pet_for_floor(from_meta.floor_seed, &self.pets);
        let to_pet = pixtuoid_scene::pet::select_pet_for_floor(to_meta.floor_seed, &self.pets);

        // The shared scene seam (#423) renders each sliding floor AND owns the
        // coffee/door-anim epilogue — a pantry trip completed mid-slide lands
        // its cup without the old hand-threaded `new_coffee_carriers` return
        // (once a real dropped-carriers bug). Recording the from-floor's
        // carriers before the to-floor render can't change the to-floor's
        // pixels: an agent lives on exactly ONE floor, and each projected
        // floor scene paints only its own agents' coffee state.
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
                floor_meta: to_meta,
                active_pet: to_active_pet,
                floor_pet: to_pet,
                debug_walkable: self.debug_walkable,
            },
        );

        // Modal backdrop: dim BOTH sliding buffers by the onboarding factor, the
        // same multiply draw_scene applies to its single buffer (the transition
        // path previously threaded the OnboardingFrame but silently dropped its
        // dim, so a floor change mid-open flashed the office to full brightness).
        if onboarding_dim < 0.999 {
            crate::tui::renderer::apply_dim(from_buf, onboarding_dim);
            crate::tui::renderer::apply_dim(to_buf, onboarding_dim);
        }

        // Compute y-offsets for vertical slide with divider gap.
        // t applies to total travel = screen_height + divider_height
        // so the easing covers the full distance including the gap.
        // Divider gap between floors during the slide = 1/5 of the screen height.
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
        // Dashboard + Sources panel can be opened mid floor-slide (Tab / `s` aren't
        // transition-gated), so paint them here too. Clone their frames for the brief
        // transition rather than thread disjoint borrows through the split_at_mut buffers.
        let dashboard = self.dashboard.clone();
        let connection = self.connection.clone();
        // Onboarding can't be opened mid-slide (it's first-run only), but clone its
        // frame for the brief transition so the overlay survives a floor change.
        let onboarding = self.onboarding.clone();
        // Floor label tracks the destination floor for the duration of the
        // slide so the per-floor agent count in the footer matches the
        // label (otherwise users see "F1/3 ... 5 agents" with floor 2's
        // count for ~400 ms).
        let transition_floor_info = floor_info_for(to_floor, nf, scene.agents.len());
        // Office-wide tallies from the full scene; the footer's rungs come from
        // the destination projected scene (`to_scene`) — spine 1, computed once.
        let transition_per_floor = crate::tui::widgets::per_floor_counts(scene);
        let footer_stats = crate::tui::widgets::FooterStats {
            counts: crate::tui::widgets::scene_stats(&to_scene),
            per_floor: &transition_per_floor,
            gateway: crate::tui::widgets::gateway_rollup(scene.daemons()),
            audio_audible: audio_audible(&self.audio),
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
                theme_picker,
                &dashboard,
                &connection,
                popup_scale,
                help_open,
                &onboarding,
                now,
                actual_full,
                theme,
            );
        })?;

        self.popup.last_scale = popup_scale;
        self.cached_layout = None;
        // The pet isn't rendered to a single interactable position mid-slide;
        // clear the stale position so the mouse handler can't "pet" a ghost at
        // last frame's location during the transition.
        self.last_pet_pos = None;
        Ok(())
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    /// The production terminal flush (an inherent method — there is no core
    /// render trait).
    pub fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        // Auto-expire pet state.
        if self.active_pet.as_ref().is_some_and(|p| !p.is_active(now)) {
            self.active_pet = None;
        }

        // Compute how many floors the current scene needs.
        let nf = num_floors(scene).min(pixtuoid_scene::floor::MAX_FLOORS);

        // Grow the per-floor sessions if needed.
        while self.floors.len() < nf {
            self.floors.push(PerFloor::new());
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

        let floor_info = floor_info_for(self.current_floor, nf, scene.agents.len());

        // --- Transition path: composite two floors sliding in/out ----------
        if self.transition.is_some() {
            return self.render_transition(scene, pack, now, nf);
        }

        // --- Normal path: single floor ------------------------------------
        let floor_scene = project_floor_scene(scene, self.current_floor);

        // Evict coffee state for agents no longer in the scene (the office
        // half of the session split). (History, motion, and frame-cache
        // eviction live in `evict_missing`, which the event loop calls with
        // the live snapshot before every render.)
        self.office.evict_missing(scene);

        let floor_meta = FloorMeta::for_floor(self.current_floor, nf);
        // Compute popup scale before the mutable borrows below.
        let popup_scale = self.version_popup_scale(now);
        let pf = &mut self.floors[self.current_floor];
        let mut draw_ctx = DrawCtx {
            // buf + the FloorCtx store are disjoint fields of the PerFloor.
            buf: &mut pf.buf,
            store: &mut pf.ctx,
            mouse_pos: self.mouse_pos,
            debug_walkable: self.debug_walkable,
            theme: self.theme,
            theme_picker: self.theme_picker,
            floor_info,
            // Office-wide truth computed from the FULL un-projected scene (C1):
            // the footer's cross-floor cue + gateway chip render even single-floor.
            per_floor: crate::tui::widgets::per_floor_counts(scene),
            gateway: crate::tui::widgets::gateway_rollup(scene.daemons()),
            audio_audible: audio_audible(&self.audio),
            volume_flash: self.volume_flash,
            floor: floor_meta,
            active_pet: self.active_pet.as_ref(),
            last_pet_pos: None,
            last_mascot_pos: None,
            // Borrows `self.pets` immutably — disjoint from the `&mut fctx`
            // (self.floors) above, so the field-split borrow is fine (same
            // as `self.office.coffee.map()` here). The picked `&Pet`
            // carries the name, so the tooltip needs no separate map.
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
        // Consume draw_ctx fields before the mutable borrow of self.floors below.
        // std::mem::take avoids a partial move so drop(draw_ctx) can follow.
        let new_coffee_carriers = std::mem::take(&mut draw_ctx.new_coffee_carriers);
        let occupied_waypoints = std::mem::take(&mut draw_ctx.occupied_waypoints);
        // drop draw_ctx here so we can re-borrow the floors freely.
        drop(draw_ctx);
        // Ambient audio: compose one AudioFrame per rendered frame through
        // the shared office observer (floor-scoped: you hear the floor you're
        // LOOKING AT — an agent typing two floors up is silent until you ride to
        // it; rain stays global). The observer runs EVERY frame, even muted, so
        // its cue edges stay warm — re-enabling audio fires no door/appliance
        // volley for what arrived while silent; only DELIVERY is gated.
        // Resolve the kind-map against THIS frame's layout (the `result` handle,
        // not `self.cached_layout` which is still last frame's until set below),
        // so occupancy and kind-map are the same frame's — matching
        // `FloorSession::audio_frame`, which reads both from the render it just
        // ran (the last cross-painter non-uniformity). On a too-small frame
        // (`Ok(None)`) no waypoints are occupied, so the `None` layout is never
        // queried.
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
        // The shared after-frame seam (coffee stamp + door-anim clamp) — the same
        // frame_epilogue render_floor/observe run, not a hand-copy (#423).
        pixtuoid_scene::floor::frame_epilogue(
            &mut self.floors[self.current_floor].ctx,
            &mut self.office.coffee,
            new_coffee_carriers,
            now,
        );
        if let Ok(ref layout_opt) = result {
            self.cached_layout = layout_opt.clone();
            // Ok(None) = draw_scene painted footer-only (compute failed at this
            // size): no popup was drawn, so zero the popup-click hit-box rather
            // than leave a stale scale the mouse handler reads as "popup on
            // screen". Mirrors the too-small early-return + the transition path.
            self.popup.last_scale = if layout_opt.is_some() {
                popup_scale
            } else {
                0.0
            };
        } else {
            self.popup.last_scale = 0.0;
        }
        result.map(|_| ())
    }
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
