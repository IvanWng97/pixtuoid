//! Multi-floor office partitioning.
//!
//! When more agents are active than `max_desks` can seat on a single floor,
//! the scene is split into multiple floors. This module provides the pure
//! arithmetic (which floor does desk N belong to? how many floors exist?),
//! the per-floor rendering context (`FloorCtx`) so each floor owns its own
//! router, overlay, pose history, and frame cache — and, since #423, the
//! shared headless frame seam ([`render_floor`]) plus the per-office
//! [`CoffeeState`] bookkeeping every painter routes through.

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

/// Fibonacci hash multiplier for floor seed derivation. Used in both
/// `FloorMeta::for_floor` and the TUI auto-compute loop.
pub const FLOOR_SEED_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

/// Derive a floor's layout seed from its index — `floor_idx * FLOOR_SEED_MULTIPLIER`
/// (Fibonacci hash). The ONE definition the engine and every binary call site
/// (boot-capacity seeding, the per-frame `compute_with_seed`, `FloorMeta`) share,
/// so a floor's look + capacity can't drift between paths.
pub fn floor_seed(floor_idx: usize) -> u64 {
    (floor_idx as u64).wrapping_mul(FLOOR_SEED_MULTIPLIER)
}

/// How many home desks a floor of buffer size `buf_w × buf_h` with `floor_seed`
/// fits — the auto-capacity the boot seeding + `fetch_max` growth read. Returns
/// `0` when the buffer is too small for even one cubicle (`compute_with_seed`
/// returns `None`), matching the existing `unwrap_or(0)` capacity callers.
pub fn floor_capacity(buf_w: u16, buf_h: u16, floor_seed: u64) -> usize {
    crate::layout::SceneLayout::compute_with_seed(buf_w, buf_h, None, floor_seed)
        .map(|l| l.home_desks.len())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct FloorMeta {
    pub floor_idx: usize,
    pub altitude: f32,
    pub floor_seed: u64,
}

impl FloorMeta {
    pub fn for_floor(floor_idx: usize, total_floors: usize) -> Self {
        let altitude = if total_floors <= 1 {
            0.0
        } else {
            floor_idx as f32 / (total_floors - 1) as f32
        };
        // Indoor lighting is uniform across floors — building interiors share the
        // same overhead lighting regardless of altitude (the night floor-dim is a
        // flat constant in the pixel painter's floor pass, no per-floor offset).
        // The `altitude` field still drives skyline depth in the windows.
        Self {
            floor_idx,
            altitude,
            floor_seed: floor_seed(floor_idx),
        }
    }

    pub fn ground() -> Self {
        Self::for_floor(0, 1)
    }
}

/// Per-floor rendering state. Each floor gets its own pathfinder,
/// occupancy overlay, pose history, recolored-frame cache, lighting
/// fade state, and motion map so floors are fully independent.
pub struct FloorCtx {
    pub router: AStarRouter,
    pub overlay: OccupancyOverlay,
    pub history: PoseHistory,
    pub cache: FrameCache,
    pub light: LightingState,
    /// Per-agent walk-timing state (physics profiles for entry/exit/wander).
    /// Evicted alongside `history` and `cache` in [`FloorCtx::evict_missing`]
    /// when the agent leaves the scene.
    pub motion: HashMap<AgentId, MotionState>,
    /// Longest in-flight entry- or exit-walk `duration_ms + pause_ms` on
    /// this floor (ms). Recomputed each frame by `recompute_door_anim_max_ms`
    /// (the shared frame epilogue on both the `render_floor` and `observe`
    /// paths); read by `compute_door_frame_idx` to drive door-open cosmetics
    /// without a hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
    /// Memo of the last per-frame layout, keyed by the ONLY inputs
    /// `Layout::compute_with_seed` reads on the frame path (buf dims + floor
    /// seed; `max_desks` is always `None` there). The compute is pure and
    /// deterministic (byte-stable snapshots depend on that), but rebuilding it
    /// every frame re-allocs + re-stamps the walkable mask and re-runs the
    /// coarse BFS — the dominant fixed per-frame CPU, quadratic in buffer
    /// area. One entry: a resize / floor switch changes the key and recomputes;
    /// memory cost is one `Layout`. Private — everything rides
    /// [`FloorCtx::frame_layout`].
    layout_memo: Option<((u16, u16, u64), Arc<crate::layout::Layout>)>,
}

impl Default for FloorCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorCtx {
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
    /// the router corridor re-point, the ONE frame prologue the engine
    /// (`render_floor`/`observe`) and the TUI painter both ride. Returns a cheap
    /// `Arc` handle — a refcount bump, NOT a deep copy of the mask + reach-set +
    /// layout Vecs — so callers hold it across later `&mut self` uses (the paint
    /// pass reads through the `Arc` while the disjoint router/cache/motion stores
    /// are `&mut`-borrowed) without re-cloning the whole `Layout` every frame.
    /// A too-small buffer returns `None` without poisoning the memo.
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

    /// Drop per-agent render state for agents no longer in `scene` — cached
    /// frames, pose history, and motion (walk-path/profile) entries. Call with
    /// the live snapshot before rendering. Load-bearing wherever agent ids can
    /// RECUR (the web hero's looped script): a returning id would find its
    /// previous life's entry/exit legs (they gate on `is_none()`) and teleport
    /// in instead of walking.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.cache.evict_missing(scene);
        self.history.evict_missing(scene);
        self.motion.retain(|id, _| scene.agents.contains_key(id));
    }

    /// Borrow this floor's routing state as a [`crate::pose::RouteCtx`] — the
    /// disjoint `&mut router / &overlay / &mut history / &mut motion` bundle the
    /// pose router + label overlay need. One method so a new store added to
    /// `RouteCtx` lands here, not re-typed (with its per-field &-vs-&mut split) at
    /// every painter call site.
    pub fn route_ctx(&mut self) -> crate::pose::RouteCtx<'_> {
        crate::pose::RouteCtx {
            router: &mut self.router,
            overlay: &self.overlay,
            history: &mut self.history,
            motion: &mut self.motion,
        }
    }

    /// Recompute `door_anim_max_ms` from the current `motion` map: the max
    /// `duration_ms + pause_ms` over the **in-flight** entry/exit profiles only.
    /// Called after each render (normal + transition paths) so the door cosmetic
    /// on the NEXT frame matches the actual physics walk windows.
    ///
    /// An ARRIVED profile is excluded (gated on `walk_arrived`): `MotionState`
    /// keeps an agent's `entry` profile for the agent's whole lifetime (it is
    /// only re-snapshotted, never cleared, to avoid re-walking entry), so
    /// without this gate the door would stay "open" for as long as the agent
    /// lives rather than just while they're actually walking through it.
    pub fn recompute_door_anim_max_ms(&mut self, now: SystemTime) {
        // entry is (started_at, profile); exit is (started_at, profile, from).
        // Take the two shared fields so one closure handles both shapes.
        let in_flight = |started_at: SystemTime, p: &WalkProfile| -> u64 {
            let elapsed = crate::anim::elapsed_ms(now, started_at);
            if walk_arrived(p, elapsed) {
                0
            } else {
                p.duration_ms + p.pause_ms
            }
        };
        self.door_anim_max_ms = self.motion.values().fold(0u64, |acc, ms| {
            let entry = ms.entry.as_ref().map_or(0, |(s, p)| in_flight(*s, p));
            let exit = ms
                .exit
                .as_ref()
                .map_or(0, |leg| in_flight(leg.started_at, &leg.profile));
            acc.max(entry).max(exit)
        });
    }
}

/// Cross-frame coffee bookkeeping: ONE map — an agent holds a desk cup iff
/// its id is a key (the cup paints while they're seated), and the value is
/// WHEN it was fetched (drives the 120s steam window). Deliberately a single
/// map, not a `HashSet` + `HashMap` pair: cup-without-stamp and
/// stamp-without-cup are unrepresentable instead of merely maintained (#431).
/// One per OFFICE, not per floor: an agent's cup survives floor navigation,
/// which is why it lives in [`PerOffice`] — the TUI shares one across its
/// `Vec<PerFloor>`; the floating window and the web hero each own one inside
/// their [`FloorSession`].
#[derive(Debug, Default)]
pub struct CoffeeState(HashMap<AgentId, SystemTime>);

impl CoffeeState {
    /// Desk-cup steam window (secs): a freshly fetched cup steams this long on
    /// the desk. ONE source of truth — the pixel pass's steam gate
    /// (`pixel_painter`) and [`record`](CoffeeState::record)'s refetch-refresh
    /// both read it, so the paint and the bookkeeping can't drift.
    pub const STEAM_WINDOW_SECS: u64 = 120;

    pub fn new() -> Self {
        Self::default()
    }

    /// The map view the pixel pass borrows (`PixelCtx.coffee`): key = carrier,
    /// value = fetch time.
    pub fn map(&self) -> &HashMap<AgentId, SystemTime> {
        &self.0
    }

    /// Force a carrier with a chosen fetch stamp (overwrites an existing one).
    /// A seeding seam — production detection goes through
    /// [`record`](CoffeeState::record), which never restamps.
    pub fn insert(&mut self, id: AgentId, fetched_at: SystemTime) {
        self.0.insert(id, fetched_at);
    }

    /// Drop coffee state for agents no longer in `scene` (the cup leaves with
    /// the agent). The coffee half of the per-agent eviction that
    /// [`FloorCtx::evict_missing`] does for render state.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.0.retain(|id, _| scene.agents.contains_key(id));
    }

    /// Persist newly detected coffee carriers. A carrier re-reported WITHIN
    /// the steam window keeps its stamp (`new_coffee_carriers` re-reports on
    /// every frame of a carrying-coffee walk-back — a re-render must not
    /// restart an old cup's steam); a report arriving AFTER the window
    /// expired is a genuinely NEW pantry fetch, so the stamp refreshes and
    /// the fresh cup steams again instead of landing permanently steam-less.
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
/// refresh the door-cosmetic clamp. The frame PROLOGUE twin is
/// [`FloorCtx::frame_layout`] itself (the memoized layout + router corridor
/// re-point all three frame paths ride directly — no wrapper). This bundles
/// TWO ops, so it stays a named seam — `pub` so the TUI's `draw_scene` (on the
/// raw `render_to_rgb_buffer` path, which can't call `render_floor`/`observe`)
/// runs THIS seam instead of re-inlining the pair (the #423 drift class).
pub fn frame_epilogue(
    fctx: &mut FloorCtx,
    coffee: &mut CoffeeState,
    carriers: impl IntoIterator<Item = pixtuoid_core::AgentId>,
    now: SystemTime,
) {
    coffee.record(carriers, now);
    fctx.recompute_door_anim_max_ms(now);
}

/// THE shared headless frame seam: scene → `RgbBuffer`, one floor, one frame —
/// prologue (buffer sizing, layout, router zone), the pixel pass, and the
/// bookkeeping epilogue (coffee-carrier persistence + the door-anim clamp
/// refresh) in ONE compiler-owned place, because a convention-mirrored
/// epilogue drifts across consumers — the #423 class, concrete enough to
/// have bitten twice (a dropped-carriers bug in the TUI transition path; the
/// web hero without eviction — the loop-2 teleport).
///
/// Consumers: the TUI floor-slide (`TuiRenderer::render_transition`), the
/// floating window (`OfficeRenderer::render`), and the web hero
/// (`pixtuoid-web::Office`). The TUI's NORMAL draw path (`draw_scene`) is the
/// deliberate exception — it needs the full `PixelPassResult` (pet/mascot
/// positions, chitchat bubbles) and holds only immutable coffee borrows
/// mid-flush — so it stays on raw `render_to_rgb_buffer` and routes its
/// bookkeeping through [`CoffeeState`]/[`FloorCtx::evict_missing`] instead.
///
/// Returns the computed layout (callers cache it for label overlays /
/// hit-testing), or `None` when the size can't lay out — the buffer is left
/// cleared and nothing panics.
///
/// Per-agent EVICTION deliberately stays CALLER-side — `FloorCtx::evict_missing`
/// and `CoffeeState::evict_missing`, run against the FULL live scene: the TUI
/// transition path hands this fn PROJECTED single-floor scenes
/// (`project_floor_scene`), so evicting in here would wipe every OTHER
/// floor's motion/cache/coffee on each slide frame. Don't "finish the seam"
/// by moving eviction inside — it would pass every single-floor test and
/// break multi-floor. For the single-floor painters "the caller" is now
/// [`FloorSession`], whose `render` runs the dual eviction once (its scene IS
/// the full live scene by contract); only a projected-scene consumer like the
/// TUI slide still calls this fn raw and owns its own eviction.
/// The IMMUTABLE per-frame render inputs — the read-only cluster threaded
/// through [`render_floor`] / [`FloorSession::render`]. The MUTABLE per-floor stores
/// (`fctx`/`buf`/`coffee`/`chitchat`) stay SEPARATE params on `render_floor`: a
/// painter that composes floors (the TUI) borrows those disjointly per floor via
/// `split_at_mut`, so they can't fold into one bundle. `buf_w`/`buf_h` fold into
/// [`Size`].
pub struct FrameInputs<'a> {
    pub scene: &'a SceneState,
    pub pack: &'a Pack,
    pub theme: &'static Theme,
    pub now: SystemTime,
    pub size: Size,
    pub floor_meta: FloorMeta,
    pub active_pet: Option<&'a PetState>,
    pub floor_pet: Option<&'a Pet>,
    pub debug_walkable: bool,
}

/// One frame's outward-facing results from [`render_floor`]: the computed
/// layout (callers cache it for overlays/hit-testing) plus the sim's occupancy
/// observation — `SimFrame::occupied_waypoints` carried through the paint
/// pass, the appliance audio-cue feed a windowed painter can't otherwise
/// reach (#633; the TUI reads the same set off its `DrawCtx` out-param).
pub struct FloorFrame {
    pub layout: Arc<crate::layout::Layout>,
    pub occupied_waypoints: std::collections::HashSet<usize>,
}

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
    buf.ensure_size(size.w, size.h, theme.surface.bg_fallback);
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
    // The shared epilogue (carrier stamping + the door-cosmetic clamp) — ONE
    // definition shared with observe().
    frame_epilogue(fctx, coffee, result.new_coffee_carriers, now);
    Some(FloorFrame {
        layout,
        occupied_waypoints,
    })
}

/// The per-FLOOR half of a painter's persistent session state: the sim/paint
/// stores ([`FloorCtx`]) plus the reusable pixel buffer that floor renders
/// into. A multi-floor painter composes `Vec<PerFloor>` (the TUI); the
/// single-floor painters hold one inside a [`FloorSession`].
pub struct PerFloor {
    pub ctx: FloorCtx,
    pub buf: RgbBuffer,
}

impl PerFloor {
    pub fn new() -> Self {
        Self {
            ctx: FloorCtx::new(),
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
        }
    }

    /// The per-floor half of the dual per-agent eviction protocol (cached
    /// frames, pose history, motion legs). Run with the FULL live scene.
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
/// against `layout` — the ONE authored form of the audio cue tracker's kind
/// lookup, shared by [`FloorSession::audio_frame`] and the multi-floor TUI's own
/// call site so the formula can't drift between them (was the floating-re-inlined
/// vs web-getter divergence).
pub fn waypoint_kind_of(
    layout: Option<&crate::layout::Layout>,
    idx: usize,
) -> Option<crate::layout::WaypointKind> {
    layout.and_then(|l| l.waypoints.get(idx)).map(|w| w.kind)
}

/// Office-wide cross-frame AUDIO bookkeeping — the sound twin of [`CoffeeState`].
/// Wraps the pure `crate::audio` model (`stem_levels`/`select_track`/`observe`)
/// into the ONE per-frame [`AudioFrame`] composition all three painters used to
/// hand-roll. Holds the [`AudioCueTracker`] (cross-frame edge state) plus the
/// floor it is primed for, so a floor switch reprimes silently — the reprime the
/// TUI once spelled out, now automatic for every painter.
#[derive(Debug, Default)]
pub struct AudioObserver {
    cues: AudioCueTracker,
    primed_floor: Option<usize>,
}

impl AudioObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compose one frame of audio intent for the floor being VIEWED, advancing
    /// the cross-frame cue edges. `waypoint_kind` resolves an occupied index to
    /// its kind — a CLOSURE exactly like [`AudioCueTracker::observe`], so this
    /// seam never holds a `Layout` and tests need none.
    ///
    /// Call it EVERY world-frame regardless of mute (the painter gates only
    /// DELIVERY): a muted stretch keeps `seen_agents`/`occupied` warm, so
    /// re-enabling never fires a door/appliance volley for what arrived while
    /// silent. `floor_idx` is the floor being viewed; counts are per-floor
    /// (`per_floor_counts[floor_idx]` == `scene_stats` for a single-floor scene).
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
        // appliances already there (was the TUI audio_floor != current_floor block).
        if self.primed_floor != Some(floor_idx) {
            self.cues = AudioCueTracker::new();
            self.primed_floor = Some(floor_idx);
        }
        // You hear the floor you're LOOKING AT: stems + door/appliance cues come
        // from that floor only; rain stays global (weather, not agent activity).
        let counts = crate::board::per_floor_counts(scene)[floor_idx.min(MAX_FLOORS - 1)];
        let precipitation = crate::pixel_painter::precipitation_level(now);
        let floor_ids = scene
            .agents
            .iter()
            .filter(|(_, slot)| slot.floor_idx == floor_idx)
            .map(|(id, _)| id);
        let events = self.cues.observe(floor_ids, occupied, waypoint_kind, now);
        AudioFrame {
            stems: crate::audio::stem_levels(&counts, precipitation),
            events,
            track: crate::audio::select_track(
                crate::pixel_painter::is_day_at(now),
                precipitation,
                crate::audio::epoch_hours(now),
            ),
        }
    }

    /// The floor this observer's cue tracker is currently primed for — the
    /// reprime latch, exposed for the floor-switch test.
    #[cfg(test)]
    pub(crate) fn primed_floor(&self) -> Option<usize> {
        self.primed_floor
    }
}

/// The per-OFFICE half: cross-frame state that survives floor navigation —
/// an agent's desk cup ([`CoffeeState`]), the venue chitchat map (its
/// `VenueKey` already carries `floor_idx`), and the audio cue tracker +
/// reprime latch ([`AudioObserver`]). ONE per painter surface, shared across
/// every floor, so a cup follows its agent through a floor switch and the audio
/// cue edges stay warm (the observer reprimes on a switch).
#[derive(Default)]
pub struct PerOffice {
    pub coffee: CoffeeState,
    pub chitchat: HashMap<VenueKey, ActiveChitchat>,
    /// The office-wide audio observer (the sound twin of `coffee`): one cue
    /// tracker + reprime latch, shared across floors, so every painter composes
    /// its [`AudioFrame`] through the same seam. See [`AudioObserver`].
    pub audio: AudioObserver,
}

impl PerOffice {
    pub fn new() -> Self {
        Self::default()
    }

    /// The office half of the dual eviction: the cup leaves with the agent.
    /// `chitchat` is deliberately untouched — conversations self-expire inside
    /// `chitchat::update_and_collect` (participants are refreshed per frame),
    /// so there is no per-agent entry to leak.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.coffee.evict_missing(scene);
    }
}

/// The OWNED painter session: bundles {[`FloorCtx`], `RgbBuffer`,
/// [`CoffeeState`], chitchat map} plus the dual `evict_missing` protocol behind
/// one type, so a painter can't hand-roll (and silently skip) the eviction — a
/// skipped eviction leaks per-agent state or teleports a recurring agent. One
/// floor + one office: the single-floor painters (`floating::offscreen::OfficeRenderer`,
/// `pixtuoid-web::Office`) own a `FloorSession`; a multi-floor painter (the
/// TUI) composes `Vec<`[`PerFloor`]`>` + one [`PerOffice`] and drives
/// [`render_floor`] / `draw_scene` itself.
pub struct FloorSession {
    pub floor: PerFloor,
    pub office: PerOffice,
    /// The layout the last `render` laid out — [`FloorSession::overlay`] builds
    /// labels against IT (not a caller-supplied one), so a painter can't pass a
    /// layout that disagrees with the sprite pass.
    last_layout: Option<Arc<crate::layout::Layout>>,
    /// The occupancy the last `render` observed ([`FloorFrame`]'s
    /// `occupied_waypoints`) — the `last_layout` pattern, so a painter reads
    /// the SAME frame's occupancy it just painted. Empty before the first
    /// render and after an unlayoutable size.
    last_occupied: std::collections::HashSet<usize>,
}

impl FloorSession {
    pub fn new() -> Self {
        Self {
            floor: PerFloor::new(),
            office: PerOffice::default(),
            last_layout: None,
            last_occupied: std::collections::HashSet::new(),
        }
    }

    /// Drop per-agent state for agents no longer in `scene` — BOTH halves of
    /// the dual eviction (render caches + pose history + motion legs, and the
    /// coffee cup), written once. `scene` must be the FULL live scene; see
    /// [`render_floor`]'s eviction note for why a PROJECTED per-floor scene
    /// must never be evicted against.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.floor.evict_missing(scene);
        self.office.evict_missing(scene);
    }

    /// Render one frame: the dual eviction, then the shared [`render_floor`]
    /// seam (prologue → pixel pass → coffee/door-anim epilogue). Returns the
    /// computed layout ([`FloorSession::buf`] holds the pixels), or `None`
    /// when the size can't lay out.
    ///
    /// `scene` MUST be the full live scene — the session evicts against it,
    /// so a painter can't skip the eviction. A consumer rendering
    /// PROJECTED single-floor scenes (the TUI floor slide) stays on
    /// [`render_floor`] directly and runs the eviction against the full scene
    /// itself.
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
                // REPLACE, never extend: occupancy must track THIS frame's set
                // (the cue tracker fires on edges; an accumulating set would
                // re-report stale waypoints forever — the frame-accuracy tooth).
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
    /// pair (the coherence the seam otherwise proves by hand, per painter). Empty
    /// before the first `render`. `hovered` highlights one agent; the single-floor
    /// painters pass `None`.
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

    /// The neon wall-board model for `scene` — the same `board` feeders the TUI
    /// footer reads, single-sourced. `floor` is `(current, total)`, or `None` for
    /// a single-floor office (no cross-floor breadcrumb).
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
            crate::board::gateway_rollup(scene.daemons()),
        )
    }

    /// The rendered pixel buffer (a borrow of the reused allocation).
    pub fn buf(&self) -> &RgbBuffer {
        &self.floor.buf
    }

    /// One frame of audio intent for THIS session's last render — the audio twin
    /// of [`FloorSession::board`]/[`FloorSession::overlay`]. Fed from the
    /// session's OWN occupancy + layout (so a painter can't hand a mismatched
    /// occupancy/kind pair) through the shared [`AudioObserver`] in [`PerOffice`].
    /// `floor_idx` is the floor this single-floor session shows (0 for the web
    /// hero). Primes (no cues) before the first render.
    ///
    /// Call it EVERY frame regardless of mute — the painter gates only DELIVERY —
    /// so the cue tracker stays warm and re-enabling audio fires no volley.
    pub fn audio_frame(
        &mut self,
        scene: &SceneState,
        floor_idx: usize,
        now: SystemTime,
    ) -> AudioFrame {
        // Disjoint field borrows: &mut self.office.audio (receiver) alongside
        // & self.last_occupied (arg) and & self.last_layout (closure). Bind the
        // two shared fields to LOCALS first so the closure captures the locals,
        // not `self` — robust regardless of closure-capture edition.
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
    /// cached AGENT sprites don't render with the old palette — mirrors the TUI's
    /// `pf.ctx.cache = FrameCache::new()` (tui_renderer::set_theme). Env (walls/
    /// floor/sky) recolors on its own since it's painted fresh each frame.
    pub fn reset_frame_cache(&mut self) {
        self.floor.ctx.cache = crate::frame_cache::FrameCache::new();
    }

    /// Advance the world one tick WITHOUT painting — the headless observation
    /// seam a native/windowless consumer drives: the same eviction, layout
    /// prologue, sim tick (`pixel_painter::sim_step`), and bookkeeping
    /// epilogue (coffee-carrier persistence + the door-anim clamp) as
    /// [`FloorSession::render`], minus the paint pass — no pixel buffer is
    /// touched. Returns the observed [`SimFrame`], or `None` when the size
    /// can't lay out.
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
        // The same epilogue as the painted path — literally: one definition.
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

/// Per-floor indoor-lighting fade state.
///
/// Behavior:
/// * Populated → empty: hold the lights for `EMPTY_DEBOUNCE_MS`, then ease
///   toward `MIN_LEVEL` with time constant `FADE_TAU_MS`. This avoids
///   flicker when agents briefly disappear between transcripts.
/// * Empty → populated: snap target to 1.0 immediately (motion-sensor
///   feel). The same ease still smooths the rise over a frame or two.
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
    pub const MIN_LEVEL: f32 = 0.10;
    pub const EMPTY_DEBOUNCE_MS: u64 = 5_000;
    pub const FADE_TAU_MS: u64 = 800;
    /// Multiplier applied to the time-of-day floor-darken overlay when
    /// the floor is fully empty. Tunes "how dark" empty looks; the only
    /// knob to reach for if empty floors read as too dark / too bright.
    pub const EMPTY_FLOOR_DIM_BOOST: f32 = 2.4;

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

    /// Force the lit level straight to `MIN_LEVEL`, bypassing the
    /// debounce + ease. Static snapshots use this so the rendered PNG
    /// catches the steady-state empty look instead of frame-0 of the fade.
    pub fn snap_to_empty(&mut self) {
        self.level = Self::MIN_LEVEL;
    }

    /// Advance the fade one frame. `empty` is the current per-floor
    /// occupancy. Returns the new lit level in `[MIN_LEVEL, 1.0]`.
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
    pub from_floor: usize,
    pub to_floor: usize,
    pub started_at: SystemTime,
    pub duration_ms: u64,
}

const TRANSITION_DURATION_MS: u64 = 900;

impl FloorTransition {
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

    pub fn is_done(&self, now: SystemTime) -> bool {
        // Backward-clock escape: `t` saturates to 0 while `now < started_at`
        // (eased_progress), so a wall-clock step to before the transition
        // start (NTP correction, suspend) would otherwise hold is_done false
        // and wedge the renderer in the transition composite — no labels,
        // tooltips, chitchat, or mouse hit-testing — until the clock re-passes
        // started_at. A backward step larger than the transition's own
        // duration can't be render-loop jitter; treat it as done so the
        // caller lands on to_floor (mirroring cancel_transition). Smaller
        // wobbles keep the saturate-to-0 convention every other animation uses.
        if let Ok(behind) = self.started_at.duration_since(now) {
            if behind.as_millis() as u64 > self.duration_ms {
                return true;
            }
        }
        self.t(now) >= 1.0
    }
}

// ---------------------------------------------------------------------------
// Pure arithmetic helpers
// ---------------------------------------------------------------------------

/// How many floors are needed to seat all agents?
pub fn num_floors(scene: &SceneState) -> usize {
    scene
        .agents
        .values()
        .map(|a| a.floor_idx + 1)
        .max()
        .unwrap_or(1)
}

/// One agent projected onto a floor by [`build_floor_scene`]: the slot — its
/// `desk_index` still the ORIGINAL global allocation — paired with its desk in
/// the floor's OWN local space, typed as such (`FloorLocalDeskIndex`). Holding
/// the floor-local offset in a SEPARATE `desk` field, rather than writing it
/// back into `AgentSlot.desk_index`, keeps that field's GLOBAL type honest
/// until [`project_floor_scene`] re-hosts the slot.
pub struct ProjectedSlot {
    pub slot: AgentSlot,
    pub desk: FloorLocalDeskIndex,
}

/// Extract agents belonging to `floor_idx`, pairing each with its desk
/// remapped into the floor's `[0..capacity)` LOCAL space (typed
/// `FloorLocalDeskIndex`) so the layout engine sees a self-contained floor.
/// Uses the stored `floor_idx` on each slot so capacity growth never migrates
/// agents between floors. The slot's own `desk_index` is left at its global
/// value; [`project_floor_scene`] performs the documented local→global
/// re-host when it builds the single-floor scene.
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

/// Build a self-contained `SceneState` for one floor: a `uniform(cap)` scene
/// (so floor arithmetic stays self-consistent with the remapped desk indices
/// in `[0..cap)`) populated with just that floor's agents. The normal and
/// floor-transition render paths both project the global scene this way.
pub fn project_floor_scene(scene: &SceneState, floor_idx: usize) -> SceneState {
    let mut s = SceneState::uniform(scene.floor_capacities[floor_idx]);
    for p in build_floor_scene(scene, floor_idx) {
        let mut slot = p.slot;
        // The RE-HOST, not a space mix-up: this `uniform(cap)` single-floor
        // scene's global desk space coincides with its floor-0 local space by
        // construction (`floor_of(g) == 0`, `floor_local_desk(g).0 == g.0` —
        // pinned by `build_floor_scene_remap_is_local_global_coincident`
        // below), so the floor-local desk IS a genuinely valid
        // `GlobalDeskIndex` FOR THIS SMALLER SCENE — the inverse of the
        // `GlobalDeskIndex::single_floor_local` identity the render path
        // reads back through.
        slot.desk_index = GlobalDeskIndex(p.desk.0);
        s.agents.insert(slot.agent_id, slot);
    }
    // Daemon presences (the OpenClaw gateway mascot) are global, not per-desk —
    // carry them onto the GROUND floor only so the mascot renders exactly once.
    if floor_idx == 0 {
        *s.daemons_mut() = scene.daemons().clone();
    }
    s
}

#[cfg(test)]
mod tests;
