//! Multi-floor office partitioning: the floor arithmetic, the per-floor
//! rendering context ([`FloorCtx`]), the shared headless frame seam
//! ([`render_floor`]), and the per-office [`CoffeeState`] bookkeeping.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use crate::physics::{walk_arrived, WalkProfile};
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{AgentSlot, FloorLocalDeskIndex, GlobalDeskIndex, SceneState};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::audio::{AudioCueTracker, AudioFrame};
use crate::chitchat::{ActiveChitchat, VenueKey};
use crate::frame_cache::FrameCache;
use crate::layout::Size;
use crate::motion::MotionState;
use crate::pathfind::{AStarRouter, Router};
use crate::pet::{Pet, PetState};
use crate::pixel_painter::{render_to_rgb_buffer, sim_step, PixelCtx, SimFrame, SimStores};
use crate::pose::PoseHistory;
use crate::theme::Theme;

pub use pixtuoid_core::state::MAX_FLOORS;

/// Fibonacci hash multiplier for floor seed derivation.
pub const FLOOR_SEED_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

/// Derive a floor's layout seed from its index — the ONE definition every call
/// site shares, so a floor's look + capacity can't drift between paths.
pub fn floor_seed(floor_idx: usize) -> u64 {
    (floor_idx as u64).wrapping_mul(FLOOR_SEED_MULTIPLIER)
}

/// How many home desks a floor of buffer size `buf_w × buf_h` with `floor_seed`
/// fits. Returns `0` when the buffer is too small for even one cubicle.
pub fn floor_capacity(buf_w: u16, buf_h: u16, floor_seed: u64) -> usize {
    crate::layout::SceneLayout::compute_with_seed(buf_w, buf_h, None, floor_seed)
        .map(|l| l.home_desks.len())
        .unwrap_or(0)
}

/// How many home desks a floor fits when `buf_w × buf_h` PIXELS are painted at
/// `scale` — i.e. the capacity of the logical office those pixels cover.
///
/// [`floor_capacity`] is the `RenderScale::ONE` case. The two are separate
/// functions rather than one defaulted parameter because every existing caller
/// means "buffer pixels are layout units" and must keep meaning it; a painter
/// that adopts a scale opts in at its own call site.
///
/// `#[cfg(test)]` because no painter has opted in yet: this is the vehicle for
/// `render_scale`'s scale-invariance proof, not shipped behaviour. Un-gate it
/// the day a painter passes a scale — the proof is what makes that safe, and
/// leaving it compiled-but-uncalled would have been the same unreached-surface
/// claim the seam itself was flagged for.
#[cfg(test)]
pub(crate) fn floor_capacity_scaled(
    buf_w: u16,
    buf_h: u16,
    scale: crate::render_scale::RenderScale,
    floor_seed: u64,
) -> usize {
    floor_capacity(scale.logical(buf_w), scale.logical(buf_h), floor_seed)
}

/// Per-floor identity + look: index, altitude, and derived layout seed.
#[derive(Debug, Clone, Copy)]
pub struct FloorMeta {
    /// Zero-based floor index.
    pub floor_idx: usize,
    /// Height fraction: 0.0 (ground) → 1.0 (top floor); drives skyline depth in the windows.
    pub altitude: f32,
    /// This floor's layout seed (`floor_seed(floor_idx)`).
    pub floor_seed: u64,
}

impl FloorMeta {
    /// Metadata for floor `floor_idx` of `total_floors` — altitude spreads 0.0 (ground) → 1.0 (top).
    pub fn for_floor(floor_idx: usize, total_floors: usize) -> Self {
        let altitude = if total_floors <= 1 {
            0.0
        } else {
            floor_idx as f32 / (total_floors - 1) as f32
        };
        // Indoor lighting is deliberately uniform across floors — `altitude`
        // drives only skyline depth in the windows, never a lighting offset.
        Self {
            floor_idx,
            altitude,
            floor_seed: floor_seed(floor_idx),
        }
    }

    /// The lone floor of a single-floor office (index 0, altitude 0.0).
    pub fn ground() -> Self {
        Self::for_floor(0, 1)
    }
}

/// Per-floor rendering state — each floor owns its stores, so floors are
/// fully independent.
pub struct FloorCtx {
    /// This floor's A\* pathfinder.
    pub router: AStarRouter,
    /// Per-tick walkable-cell occupancy (routing steers around occupied cells).
    pub overlay: OccupancyOverlay,
    /// Per-agent pose history for the routed pose derivation.
    pub history: PoseHistory,
    /// Per-agent recolored-sprite cache.
    pub cache: FrameCache,
    /// This floor's indoor-lighting fade state.
    pub light: LightingState,
    /// Per-agent walk-timing state (physics profiles for entry/exit/wander).
    pub motion: HashMap<AgentId, MotionState>,
    /// Longest in-flight entry- or exit-walk `duration_ms + pause_ms` on this
    /// floor (ms) — drives the door-open cosmetic without a hardcoded window.
    pub door_anim_max_ms: u64,
    /// Memo of the last per-frame layout, keyed by the ONLY inputs
    /// `Layout::compute_with_seed` reads on the frame path. Rebuilding it every
    /// frame re-allocs + re-stamps the walkable mask and re-runs the coarse BFS
    /// — the dominant fixed per-frame CPU, quadratic in buffer area.
    layout_memo: Option<((u16, u16, u64), Arc<crate::layout::Layout>)>,
}

impl Default for FloorCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorCtx {
    /// Fresh per-floor state.
    pub fn new() -> Self {
        Self {
            router: AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: PoseHistory::new(),
            cache: FrameCache::new(),
            light: LightingState::new(),
            motion: HashMap::new(),
            door_anim_max_ms: 0,
            layout_memo: None,
        }
    }

    /// The per-frame layout — memoized `compute_with_seed(w, h, None, seed)` +
    /// the router corridor re-point, the ONE frame prologue every painter rides.
    /// Returns a cheap `Arc` handle so callers can hold it across later
    /// `&mut self` uses without re-cloning the whole `Layout` every frame. A
    /// too-small buffer returns `None` without poisoning the memo.
    pub fn frame_layout(
        &mut self,
        buf_w: u16,
        buf_h: u16,
        floor_seed: u64,
    ) -> Option<Arc<crate::layout::Layout>> {
        let key = (buf_w, buf_h, floor_seed);
        let layout = match &self.layout_memo {
            Some((k, l)) if *k == key => Arc::clone(l),
            _ => {
                let l = Arc::new(crate::layout::Layout::compute_with_seed(
                    buf_w, buf_h, None, floor_seed,
                )?);
                self.layout_memo = Some((key, Arc::clone(&l)));
                l
            }
        };
        self.router.set_preferred_zone(layout.corridor);
        Some(layout)
    }

    /// Drop per-agent render state for agents no longer in `scene`. Load-bearing
    /// wherever agent ids can RECUR (the web hero's looped script): a returning
    /// id would find its previous life's entry/exit legs (they gate on
    /// `is_none()`) and teleport in instead of walking.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.cache.evict_missing(scene);
        self.history.evict_missing(scene);
        self.motion.retain(|id, _| scene.agents.contains_key(id));
    }

    /// Borrow this floor's routing state as a [`crate::pose::RouteCtx`] — the
    /// disjoint `&mut router / &overlay / &mut history / &mut motion` bundle the
    /// pose router + label overlay need.
    pub fn route_ctx(&mut self) -> crate::pose::RouteCtx<'_> {
        crate::pose::RouteCtx {
            router: &mut self.router,
            overlay: &self.overlay,
            history: &mut self.history,
            motion: &mut self.motion,
        }
    }

    /// Recompute `door_anim_max_ms`: the max `duration_ms + pause_ms` over the
    /// **in-flight** entry/exit profiles only. An ARRIVED profile is excluded
    /// because `MotionState` keeps an agent's `entry` profile for its whole
    /// lifetime — without the gate the door would stay "open" for as long as the
    /// agent lives rather than just while they walk through it.
    pub fn recompute_door_anim_max_ms(&mut self, now: SystemTime) {
        let in_flight = |started_at: SystemTime, p: &WalkProfile| -> u64 {
            let elapsed = crate::anim::elapsed_ms(now, started_at);
            if walk_arrived(p, elapsed) {
                0
            } else {
                p.duration_ms + p.pause_ms
            }
        };
        self.door_anim_max_ms = self.motion.values().fold(0u64, |acc, ms| {
            let entry = ms
                .entry
                .as_ref()
                .map_or(0, |l| in_flight(l.started_at, &l.profile));
            let exit = ms
                .exit
                .as_ref()
                .map_or(0, |leg| in_flight(leg.started_at, &leg.profile));
            acc.max(entry).max(exit)
        });
    }
}

/// Cross-frame coffee bookkeeping: ONE map — an agent holds a desk cup iff its
/// id is a key, and the value is WHEN it was fetched (drives the steam window).
/// Deliberately a single map, not a `HashSet` + `HashMap` pair: cup-without-stamp
/// and stamp-without-cup are unrepresentable instead of merely maintained. One
/// per OFFICE, not per floor — an agent's cup survives floor navigation.
#[derive(Debug, Default)]
pub struct CoffeeState(HashMap<AgentId, SystemTime>);

impl CoffeeState {
    /// Desk-cup steam window (secs) — ONE source of truth for the pixel pass's
    /// steam gate and [`record`](CoffeeState::record)'s refetch-refresh.
    pub const STEAM_WINDOW_SECS: u64 = 120;

    /// Empty coffee state — no cups held.
    pub fn new() -> Self {
        Self::default()
    }

    /// The map view the pixel pass borrows: key = carrier, value = fetch time.
    pub fn map(&self) -> &HashMap<AgentId, SystemTime> {
        &self.0
    }

    /// Force a carrier with a chosen fetch stamp (overwrites) — a seeding seam;
    /// production detection goes through [`record`](CoffeeState::record), which
    /// never restamps.
    pub fn insert(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.0.insert(id, fetched_at);
    }

    /// Drop coffee state for agents no longer in `scene` — the cup leaves with
    /// the agent.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.0.retain(|id, _| scene.agents.contains_key(id));
    }

    /// Persist newly detected coffee carriers. A carrier re-reported WITHIN the
    /// steam window keeps its stamp — carriers re-report every frame of a
    /// walk-back, and a re-render must not restart an old cup's steam.
    pub fn record(&mut self, carriers: impl IntoIterator<Item = AgentId>, now: SystemTime) {
        for id in carriers {
            match self.0.entry(id) {
                Entry::Occupied(mut e) => {
                    // Backward clock (duration_since err) reads as not-expired:
                    // keep the old stamp rather than restamping on a clock step.
                    let expired = now
                        .duration_since(*e.get())
                        .is_ok_and(|d| d.as_secs() >= Self::STEAM_WINDOW_SECS);
                    if expired {
                        e.insert(now);
                    }
                }
                Entry::Vacant(v) => {
                    v.insert(now);
                }
            }
        }
    }
}

/// The shared per-frame EPILOGUE: stamp this frame's new coffee carriers and
/// refresh the door-cosmetic clamp. `pub` so the TUI's `draw_scene` — which
/// can't call [`render_floor`]/`observe` — runs THIS seam instead of
/// re-inlining the pair.
pub fn frame_epilogue(
    fctx: &mut FloorCtx,
    coffee: &mut CoffeeState,
    carriers: impl IntoIterator<Item = pixtuoid_core::AgentId>,
    now: SystemTime,
) {
    coffee.record(carriers, now);
    fctx.recompute_door_anim_max_ms(now);
}

/// The IMMUTABLE per-frame render inputs threaded through [`render_floor`] /
/// [`FloorSession::render`]. The MUTABLE per-floor stores
/// (`fctx`/`buf`/`coffee`/`chitchat`) stay SEPARATE params on `render_floor`: a
/// painter that composes floors (the TUI) borrows those disjointly per floor via
/// `split_at_mut`, so they can't fold into one bundle.
pub struct FrameInputs<'a> {
    /// The scene to render (the full live scene, or a projected single-floor one).
    pub scene: &'a SceneState,
    /// The character sprite pack.
    pub pack: &'a Pack,
    /// The active color theme.
    pub theme: &'static Theme,
    /// This frame's wall-clock time.
    pub now: SystemTime,
    /// Target pixel-buffer size — BUFFER pixels, which `scale` converts to the
    /// logical extent the office is laid out in.
    pub size: Size,
    /// How many buffer pixels one layout unit paints as.
    ///
    /// A compile-forced field rather than a defaulted one: `size` alone is
    /// ambiguous once the two spaces differ, so every painter states which it
    /// means. `RenderScale::ONE` is the classic path and keeps `size` the
    /// office's extent exactly as before.
    /// This floor's index, altitude, and layout seed.
    pub floor_meta: FloorMeta,
    /// The pet's live interaction state, if a pet is present.
    pub active_pet: Option<&'a PetState>,
    /// This floor's configured pet, if any.
    pub floor_pet: Option<&'a Pet>,
    /// Composite the walkable / approach / route debug layer (the `w` toggle).
    pub debug_walkable: bool,
}

/// One frame's outward-facing results from [`render_floor`]: the computed
/// layout plus the sim's occupancy observation.
pub struct FloorFrame {
    /// The frame's computed layout (callers cache it for overlays / hit-testing).
    pub layout: Arc<crate::layout::Layout>,
    /// The occupied-waypoint indices this frame — the appliance audio-cue feed.
    pub occupied_waypoints: std::collections::HashSet<usize>,
}

/// THE shared headless frame seam: scene → `RgbBuffer`, one floor, one frame.
/// `None` when the size can't lay out (buffer left cleared, no panic).
/// Per-agent eviction stays caller-side — a projected-scene consumer (the TUI
/// floor slide) hands this fn a PROJECTED scene, so evicting in here would wipe
/// every OTHER floor's state.
pub fn render_floor(
    fctx: &mut FloorCtx,
    buf: &mut RgbBuffer,
    coffee: &mut CoffeeState,
    chitchat: &mut HashMap<VenueKey, ActiveChitchat>,
    inputs: FrameInputs,
) -> Option<FloorFrame> {
    let FrameInputs {
        scene,
        pack,
        theme,
        now,
        size,
        floor_meta,
        active_pet,
        floor_pet,
        debug_walkable,
    } = inputs;
    // The two spaces part here: the buffer is sized in PIXELS, the office is
    // laid out in LOGICAL units. At `RenderScale::ONE` they coincide and this
    // is byte-identical to the pre-seam behaviour.
    buf.resize_fill(size.w, size.h, theme.surface.bg_fallback);
    let layout = fctx.frame_layout(size.w, size.h, floor_meta.floor_seed)?;
    let result = render_to_rgb_buffer(&mut PixelCtx {
        // Reborrow: `frame_epilogue` uses `fctx` after this render.
        store: &mut *fctx,
        buf,
        scene,
        layout: &layout,
        pack,
        now,
        theme,
        floor: floor_meta,
        active_pet,
        floor_pet,
        coffee: coffee.map(),
        chitchat_state: chitchat,
        debug_walkable,
    });
    let occupied_waypoints = result.occupied_waypoints;
    frame_epilogue(fctx, coffee, result.new_coffee_carriers, now);
    Some(FloorFrame {
        layout,
        occupied_waypoints,
    })
}

/// The per-FLOOR half of a painter's persistent session state: the sim/paint
/// stores ([`FloorCtx`]) plus the reusable pixel buffer that floor renders into.
pub struct PerFloor {
    /// This floor's sim/paint stores.
    pub ctx: FloorCtx,
    /// The reused pixel buffer this floor renders into.
    pub buf: RgbBuffer,
}

impl PerFloor {
    /// Fresh floor stores + an empty (zero-sized) pixel buffer.
    pub fn new() -> Self {
        Self {
            ctx: FloorCtx::new(),
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
        }
    }

    /// The per-floor half of the dual per-agent eviction protocol. Run with the
    /// FULL live scene.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.ctx.evict_missing(scene);
    }
}

impl Default for PerFloor {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve an occupied-waypoint index to its [`WaypointKind`](crate::layout::WaypointKind)
/// against `layout` — the ONE authored form of the audio cue tracker's kind lookup.
pub fn waypoint_kind_of(
    layout: Option<&crate::layout::Layout>,
    idx: usize,
) -> Option<crate::layout::WaypointKind> {
    layout.and_then(|l| l.waypoints.get(idx)).map(|w| w.kind)
}

/// The mood [`TrackId`](crate::audio::TrackId) for `now` — the ONE place the
/// day/precip/epoch input wiring lives. Lives here (not `audio`) because it
/// reaches the lighting layer's `is_day_at`/`precipitation_level`, which `audio`
/// must not depend on.
pub fn track_for(now: std::time::SystemTime) -> crate::audio::TrackId {
    crate::audio::select_track(
        crate::pixel_painter::is_day_at(now),
        crate::pixel_painter::precipitation_level(now),
        crate::audio::track_epoch(now),
    )
}

/// Wraps the pure `crate::audio` model into the ONE per-frame [`AudioFrame`]
/// composition every painter shares. Holds the [`AudioCueTracker`] plus the
/// floor it is primed for, so a floor switch reprimes silently.
#[derive(Debug, Default)]
pub struct AudioObserver {
    cues: AudioCueTracker,
    primed_floor: Option<usize>,
}

impl AudioObserver {
    /// A fresh observer, primed for no floor yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compose one frame of audio intent for the floor being VIEWED, advancing
    /// the cross-frame cue edges. Call it EVERY world-frame regardless of mute
    /// (the painter gates only DELIVERY): a muted stretch keeps
    /// `seen_agents`/`occupied` warm, so re-enabling never fires a
    /// door/appliance volley for what arrived while silent.
    pub fn frame(
        &mut self,
        scene: &SceneState,
        occupied: &std::collections::HashSet<usize>,
        waypoint_kind: impl Fn(usize) -> Option<crate::layout::WaypointKind>,
        floor_idx: usize,
        now: SystemTime,
    ) -> AudioFrame {
        // Reprime on floor switch: a fresh tracker primes silently next observe,
        // so riding to a new floor never fires a cue volley for agents /
        // appliances already there.
        if self.primed_floor != Some(floor_idx) {
            self.cues = AudioCueTracker::new();
            self.primed_floor = Some(floor_idx);
        }
        // You hear the floor you're LOOKING AT — but rain stays global, since
        // it's weather, not agent activity.
        let counts = crate::board::per_floor_counts(scene)[floor_idx.min(MAX_FLOORS - 1)];
        let precipitation = crate::pixel_painter::precipitation_level(now);
        let floor_ids = scene
            .agents
            .iter()
            .filter(|(_, slot)| slot.floor_idx == floor_idx)
            .map(|(id, _)| id);
        let events = self.cues.observe(floor_ids, occupied, waypoint_kind);
        AudioFrame {
            stems: crate::audio::stem_levels(&counts, precipitation),
            events,
            track: track_for(now),
        }
    }

    /// The floor this observer's cue tracker is currently primed for.
    #[cfg(test)]
    pub(crate) fn primed_floor(&self) -> Option<usize> {
        self.primed_floor
    }
}

/// The per-OFFICE half: cross-frame state that survives floor navigation — ONE
/// per painter surface, shared across every floor.
#[derive(Default)]
pub struct PerOffice {
    /// Every agent's desk cup + fetch time — survives floor navigation.
    pub coffee: CoffeeState,
    /// Active speech bubbles keyed by venue (the `VenueKey` carries `floor_idx`).
    pub chitchat: HashMap<VenueKey, ActiveChitchat>,
    /// The office-wide [`AudioObserver`] — one cue tracker + reprime latch,
    /// shared across floors.
    pub audio: AudioObserver,
}

impl PerOffice {
    /// Empty office state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The office half of the dual eviction. `chitchat` is deliberately
    /// untouched — conversations self-expire inside
    /// `chitchat::update_and_collect`, so there is no per-agent entry to leak.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.coffee.evict_missing(scene);
    }
}

/// The OWNED single-floor painter session: one [`PerFloor`] + one [`PerOffice`]
/// plus the dual `evict_missing` protocol behind one type, so a painter can't
/// hand-roll (and silently skip) the eviction — a skipped eviction leaks
/// per-agent state or teleports a recurring agent.
pub struct FloorSession {
    /// This session's single floor — its sim/paint stores + pixel buffer.
    pub floor: PerFloor,
    /// The office-wide cross-frame state (coffee, chitchat, audio) shared across floors.
    pub office: PerOffice,
    /// The layout the last `render` laid out — [`FloorSession::overlay`] builds
    /// labels against IT (not a caller-supplied one), so a painter can't pass a
    /// layout that disagrees with the sprite pass.
    last_layout: Option<Arc<crate::layout::Layout>>,
    /// The occupancy the last `render` observed, so a painter reads the SAME
    /// frame's occupancy it just painted.
    last_occupied: std::collections::HashSet<usize>,
}

impl FloorSession {
    /// An empty session — fresh floor + office state, nothing laid out yet.
    pub fn new() -> Self {
        Self {
            floor: PerFloor::new(),
            office: PerOffice::default(),
            last_layout: None,
            last_occupied: std::collections::HashSet::new(),
        }
    }

    /// Drop per-agent state for agents no longer in `scene` — BOTH halves of the
    /// dual eviction. `scene` must be the FULL live scene; see [`render_floor`]
    /// for why a PROJECTED per-floor scene must never be evicted against.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.floor.evict_missing(scene);
        self.office.evict_missing(scene);
    }

    /// Render one frame: the dual eviction, then the shared [`render_floor`]
    /// seam. Returns the computed layout ([`FloorSession::buf`] holds the
    /// pixels), or `None` when the size can't lay out. `scene` MUST be the full
    /// live scene — the session evicts against it.
    pub fn render(&mut self, inputs: FrameInputs) -> Option<Arc<crate::layout::Layout>> {
        self.evict_missing(inputs.scene);
        let frame = render_floor(
            &mut self.floor.ctx,
            &mut self.floor.buf,
            &mut self.office.coffee,
            &mut self.office.chitchat,
            inputs,
        );
        match frame {
            Some(FloorFrame {
                layout,
                occupied_waypoints,
            }) => {
                self.last_layout = Some(Arc::clone(&layout));
                // REPLACE, never extend: the cue tracker fires on edges, so an
                // accumulating set would re-report stale waypoints forever.
                self.last_occupied = occupied_waypoints;
                Some(layout)
            }
            None => {
                self.last_layout = None;
                self.last_occupied.clear();
                None
            }
        }
    }

    /// Agent labels for the LAST rendered frame, built against THIS session's
    /// layout + route state — a painter can't hand a mismatched layout/route_ctx
    /// pair. Empty before the first `render`.
    pub fn overlay(
        &mut self,
        scene: &SceneState,
        now: SystemTime,
        hovered: Option<AgentId>,
    ) -> Vec<crate::overlay::LabelElement> {
        let Some(layout) = self.last_layout.as_deref() else {
            return Vec::new();
        };
        let mut rctx = self.floor.ctx.route_ctx();
        crate::overlay::build_overlay(scene, layout, now, &mut rctx, hovered)
    }

    /// The neon wall-board model for `scene`. `floor` is `(current, total)`, or
    /// `None` for a single-floor office (no cross-floor breadcrumb).
    pub fn board(
        &self,
        scene: &SceneState,
        now: SystemTime,
        floor: Option<(usize, usize)>,
    ) -> crate::board::BoardModel {
        crate::board::build_board(
            crate::board::scene_stats(scene),
            crate::board::scene_uptime_secs(scene, now),
            floor,
            crate::board::gateway_rollup(scene.daemons().map(|(_, _, p)| p)),
        )
    }

    /// The rendered pixel buffer (a borrow of the reused allocation).
    pub fn buf(&self) -> &RgbBuffer {
        &self.floor.buf
    }

    /// One frame of audio intent for THIS session's last render, fed from the
    /// session's OWN occupancy + layout so a painter can't hand a mismatched
    /// occupancy/kind pair. Call it EVERY frame regardless of mute (see
    /// [`AudioObserver::frame`]).
    pub fn audio_frame(
        &mut self,
        scene: &SceneState,
        floor_idx: usize,
        now: SystemTime,
    ) -> AudioFrame {
        // Bind the two shared fields to LOCALS first so the closure captures the
        // locals, not `self` — otherwise it collides with the `&mut
        // self.office.audio` receiver.
        let occupied = &self.last_occupied;
        let layout = self.last_layout.as_deref();
        self.office.audio.frame(
            scene,
            occupied,
            |idx| waypoint_kind_of(layout, idx),
            floor_idx,
            now,
        )
    }

    /// Flush the per-floor recolored-sprite cache. Call after a theme change so
    /// cached AGENT sprites don't render with the old palette; env
    /// (walls/floor/sky) needs no flush since it repaints fresh each frame.
    pub fn reset_frame_cache(&mut self) {
        self.floor.ctx.cache = crate::frame_cache::FrameCache::new();
    }

    /// Advance the world one tick WITHOUT painting — the same eviction, layout
    /// prologue, sim tick, and bookkeeping epilogue as
    /// [`FloorSession::render`], minus the paint pass. Returns the observed
    /// [`SimFrame`], or `None` when the size can't lay out.
    pub fn observe(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        buf_w: u16,
        buf_h: u16,
        floor_meta: FloorMeta,
        now: SystemTime,
    ) -> Option<SimFrame> {
        self.evict_missing(scene);
        let layout = self
            .floor
            .ctx
            .frame_layout(buf_w, buf_h, floor_meta.floor_seed)?;
        let frame = sim_step(
            &mut SimStores {
                router: &mut self.floor.ctx.router,
                overlay: &mut self.floor.ctx.overlay,
                history: &mut self.floor.ctx.history,
                motion: &mut self.floor.ctx.motion,
                light: &mut self.floor.ctx.light,
                chitchat: &mut self.office.chitchat,
            },
            scene,
            &layout,
            pack,
            self.office.coffee.map(),
            floor_meta.floor_idx,
            now,
        );
        frame_epilogue(
            &mut self.floor.ctx,
            &mut self.office.coffee,
            frame.new_coffee_carriers.iter().copied(),
            now,
        );
        Some(frame)
    }
}

impl Default for FloorSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-floor indoor-lighting fade state: an emptied floor holds full light for
/// `EMPTY_DEBOUNCE_MS` (so agents briefly disappearing between transcripts don't
/// flicker it) then eases toward `MIN_LEVEL`; repopulating snaps the target
/// straight back to 1.0.
pub struct LightingState {
    level: f32,
    empty_since: Option<SystemTime>,
    last_update: Option<SystemTime>,
}

impl Default for LightingState {
    fn default() -> Self {
        Self::new()
    }
}

impl LightingState {
    /// Floor of the smoothed lit level — an empty floor dims to here, never to black.
    pub const MIN_LEVEL: f32 = 0.10;
    /// How long an emptied floor holds full light before it starts fading (ms).
    pub const EMPTY_DEBOUNCE_MS: u64 = 5_000;
    /// Time constant of the exponential lit-level ease (ms).
    pub const FADE_TAU_MS: u64 = 800;
    /// Multiplier on the time-of-day floor-darken overlay when the floor is
    /// fully empty — the knob for how dark "empty" reads.
    pub const EMPTY_FLOOR_DIM_BOOST: f32 = 2.4;

    /// A fully-lit floor (level 1.0), no fade in progress.
    pub fn new() -> Self {
        Self {
            level: 1.0,
            empty_since: None,
            last_update: None,
        }
    }

    /// Current smoothed lit level in `[MIN_LEVEL, 1.0]`.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Force the lit level straight to `MIN_LEVEL`, bypassing the debounce +
    /// ease — static snapshots want the steady-state empty look, not frame-0 of
    /// the fade.
    pub fn snap_to_empty(&mut self) {
        self.level = Self::MIN_LEVEL;
    }

    /// Advance the fade one frame. Returns the new lit level in
    /// `[MIN_LEVEL, 1.0]`.
    pub fn tick(&mut self, empty: bool, now: SystemTime) -> f32 {
        let target = if empty {
            let since = *self.empty_since.get_or_insert(now);
            let elapsed = crate::anim::elapsed_ms(now, since);
            if elapsed >= Self::EMPTY_DEBOUNCE_MS {
                Self::MIN_LEVEL
            } else {
                1.0
            }
        } else {
            self.empty_since = None;
            1.0
        };

        let dt_ms = self
            .last_update
            .and_then(|prev| now.duration_since(prev).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_update = Some(now);

        let alpha = 1.0 - (-(dt_ms as f32) / Self::FADE_TAU_MS as f32).exp();
        self.level += (target - self.level) * alpha.clamp(0.0, 1.0);
        self.level
    }
}

/// Animated floor-switch transition.
pub struct FloorTransition {
    /// The floor being slid away FROM.
    pub from_floor: usize,
    /// The floor being slid TO.
    pub to_floor: usize,
    /// When the slide began.
    pub started_at: SystemTime,
    /// Slide duration (ms).
    pub duration_ms: u64,
}

const TRANSITION_DURATION_MS: u64 = 900;

impl FloorTransition {
    /// Start a slide from floor `from` to floor `to` at `now`.
    pub fn new(from: usize, to: usize, now: SystemTime) -> Self {
        Self {
            from_floor: from,
            to_floor: to,
            started_at: now,
            duration_ms: TRANSITION_DURATION_MS,
        }
    }

    /// Progress ratio 0.0 → 1.0 with ease-in-out curve.
    pub fn t(&self, now: SystemTime) -> f32 {
        crate::anim::eased_progress(
            self.started_at,
            self.duration_ms as u32,
            crate::anim::Easing::EaseInOutCubic,
            now,
        )
    }

    /// Whether the slide has finished (or a backward clock step past its duration ends it).
    pub fn is_done(&self, now: SystemTime) -> bool {
        // Backward-clock escape: `t` saturates to 0 while `now < started_at`, so
        // a wall-clock step back (NTP correction, suspend) would otherwise wedge
        // the renderer in the transition composite — no labels, tooltips,
        // chitchat, or hit-testing — until the clock re-passes started_at. A step
        // larger than the transition's own duration can't be render-loop jitter;
        // treat it as done. Smaller wobbles keep the saturate-to-0 convention
        // every other animation uses.
        if let Ok(behind) = self.started_at.duration_since(now) {
            if behind.as_millis() as u64 > self.duration_ms {
                return true;
            }
        }
        self.t(now) >= 1.0
    }
}

/// How many floors are needed to seat all agents?
pub fn num_floors(scene: &SceneState) -> usize {
    scene
        .agents
        .values()
        .map(|a| a.floor_idx + 1)
        .max()
        .unwrap_or(1)
}

/// One agent projected onto a floor by [`build_floor_scene`]. The floor-local
/// offset rides a SEPARATE `desk` field rather than being written back into
/// `AgentSlot.desk_index`, which keeps that field's GLOBAL type honest until
/// [`project_floor_scene`] re-hosts the slot.
pub struct ProjectedSlot {
    /// The projected agent — its `desk_index` still the ORIGINAL global allocation.
    pub slot: AgentSlot,
    /// The agent's desk remapped into this floor's local `[0..capacity)` space.
    pub desk: FloorLocalDeskIndex,
}

/// Extract agents belonging to `floor_idx`, pairing each with its desk remapped
/// into the floor's `[0..capacity)` LOCAL space so the layout engine sees a
/// self-contained floor. Uses the stored `floor_idx` on each slot so capacity
/// growth never migrates agents between floors.
pub fn build_floor_scene(scene: &SceneState, floor_idx: usize) -> Vec<ProjectedSlot> {
    let offset = scene.floor_range(floor_idx).start;
    scene
        .agents
        .values()
        .filter(|a| a.floor_idx == floor_idx)
        .filter_map(|a| {
            if a.desk_index.0 < offset {
                return None;
            }
            Some(ProjectedSlot {
                slot: a.clone(),
                desk: FloorLocalDeskIndex(a.desk_index.0 - offset),
            })
        })
        .collect()
}

/// Build a self-contained `SceneState` for one floor: a `uniform(cap)` scene, so
/// floor arithmetic stays self-consistent with the remapped desk indices in
/// `[0..cap)`.
pub fn project_floor_scene(scene: &SceneState, floor_idx: usize) -> SceneState {
    let mut s = SceneState::uniform(scene.floor_capacities[floor_idx]);
    for p in build_floor_scene(scene, floor_idx) {
        let mut slot = p.slot;
        // The RE-HOST, not a space mix-up: this `uniform(cap)` single-floor
        // scene's global desk space coincides with its floor-0 local space by
        // construction, so the floor-local desk IS a genuinely valid
        // `GlobalDeskIndex` FOR THIS SMALLER SCENE.
        slot.desk_index = GlobalDeskIndex(p.desk.0);
        s.agents.insert(slot.agent_id, slot);
    }
    // Daemon presences are global, not per-desk — ground floor only, so the
    // mascot renders exactly once.
    if floor_idx == 0 {
        s.clone_daemons_from(scene);
    }
    s
}

#[cfg(test)]
mod tests;
