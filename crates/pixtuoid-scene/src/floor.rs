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
use std::time::SystemTime;

use crate::physics::{walk_arrived, WalkProfile};
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{AgentSlot, FloorLocalDeskIndex, GlobalDeskIndex, SceneState};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::chitchat::{ActiveChitchat, VenueKey};
use crate::frame_cache::FrameCache;
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
    pub sunlight_boost: f32,
}

impl FloorMeta {
    pub fn for_floor(floor_idx: usize, total_floors: usize) -> Self {
        let altitude = if total_floors <= 1 {
            0.0
        } else {
            floor_idx as f32 / (total_floors - 1) as f32
        };
        Self {
            floor_idx,
            altitude,
            floor_seed: floor_seed(floor_idx),
            // Indoor lighting is uniform across floors — building interiors
            // share the same overhead lighting regardless of altitude. The
            // `altitude` field still drives skyline depth in the windows.
            sunlight_boost: 0.0,
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
    /// this floor (ms). Written each frame by `derive_with_routing`; read by
    /// `compute_door_frame_idx` to drive door-open cosmetics without a
    /// hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
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
        }
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

/// The shared per-frame PROLOGUE: lay the floor out and point the router at
/// its corridor. ONE definition so `render_floor` and `FloorSession::observe`
/// can't drift (the mirrored-epilogue class #423 exists to kill).
fn frame_prologue(
    fctx: &mut FloorCtx,
    buf_w: u16,
    buf_h: u16,
    floor_seed: u64,
) -> Option<crate::layout::Layout> {
    let layout = crate::layout::Layout::compute_with_seed(buf_w, buf_h, None, floor_seed)?;
    fctx.router.set_preferred_zone(layout.corridor);
    Some(layout)
}

/// The shared per-frame EPILOGUE: stamp this frame's new coffee carriers and
/// refresh the door-cosmetic clamp. Same ONE-definition rationale as
/// [`frame_prologue`].
fn frame_epilogue(
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
/// refresh) in ONE compiler-owned place. Before this seam the epilogue was
/// mirrored by convention across the consumers and drifted twice for real
/// (a dropped-carriers bug in the TUI transition path; the web hero shipping
/// without eviction at all — the loop-2 teleport, #423).
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
#[allow(clippy::too_many_arguments)] // the render inputs are genuinely flat (same shape as the pass itself)
pub fn render_floor(
    fctx: &mut FloorCtx,
    buf: &mut RgbBuffer,
    coffee: &mut CoffeeState,
    chitchat: &mut HashMap<VenueKey, ActiveChitchat>,
    scene: &SceneState,
    pack: &Pack,
    theme: &'static Theme,
    now: SystemTime,
    buf_w: u16,
    buf_h: u16,
    floor_meta: FloorMeta,
    active_pet: Option<&PetState>,
    floor_pet: Option<&Pet>,
    debug_walkable: bool,
) -> Option<crate::layout::Layout> {
    buf.ensure_size(buf_w, buf_h, theme.surface.bg_fallback);
    let layout = frame_prologue(fctx, buf_w, buf_h, floor_meta.floor_seed)?;
    let result = render_to_rgb_buffer(&mut PixelCtx {
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
        chitchat_state: chitchat,
        coffee: coffee.map(),
        light: &mut fctx.light,
        debug_walkable,
    });
    // The epilogue the consumers used to mirror by hand — now ONE definition
    // (carrier stamping + the door-cosmetic clamp) shared with observe().
    frame_epilogue(fctx, coffee, result.new_coffee_carriers, now);
    Some(layout)
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

/// The per-OFFICE half: cross-frame state that survives floor navigation —
/// an agent's desk cup ([`CoffeeState`]) and the venue chitchat map (its
/// `VenueKey` already carries `floor_idx`). ONE per painter surface, shared
/// across every floor, so a cup follows its agent through a floor switch.
#[derive(Default)]
pub struct PerOffice {
    pub coffee: CoffeeState,
    pub chitchat: HashMap<VenueKey, ActiveChitchat>,
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

/// The OWNED painter session: the persistent bundle every painter used to
/// hand-roll — {[`FloorCtx`], `RgbBuffer`, [`CoffeeState`], chitchat map} plus
/// the dual `evict_missing` protocol — hoisted behind one type. Each painter
/// drifted on exactly that convention once: the floating window never evicted
/// (a slow per-agent leak for the window's lifetime) and the web hero shipped
/// without eviction at all (the loop-2 teleport, #423). One floor + one
/// office: the single-floor painters (`floating::offscreen::OfficeRenderer`,
/// `pixtuoid-web::Office`) own a `FloorSession`; a multi-floor painter (the
/// TUI) composes `Vec<`[`PerFloor`]`>` + one [`PerOffice`] and drives
/// [`render_floor`] / `draw_scene` itself.
pub struct FloorSession {
    pub floor: PerFloor,
    pub office: PerOffice,
}

impl FloorSession {
    pub fn new() -> Self {
        Self {
            floor: PerFloor::new(),
            office: PerOffice::default(),
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
    /// so a painter can no longer forget the eviction (the drift class behind
    /// the floating leak and the web loop-2 teleport). A consumer rendering
    /// PROJECTED single-floor scenes (the TUI floor slide) stays on
    /// [`render_floor`] directly and runs the eviction against the full scene
    /// itself.
    #[allow(clippy::too_many_arguments)] // the render inputs are genuinely flat (same shape as render_floor)
    pub fn render(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        theme: &'static Theme,
        now: SystemTime,
        buf_w: u16,
        buf_h: u16,
        floor_meta: FloorMeta,
        active_pet: Option<&PetState>,
        floor_pet: Option<&Pet>,
        debug_walkable: bool,
    ) -> Option<crate::layout::Layout> {
        self.evict_missing(scene);
        render_floor(
            &mut self.floor.ctx,
            &mut self.floor.buf,
            &mut self.office.coffee,
            &mut self.office.chitchat,
            scene,
            pack,
            theme,
            now,
            buf_w,
            buf_h,
            floor_meta,
            active_pet,
            floor_pet,
            debug_walkable,
        )
    }

    /// The rendered pixel buffer (a borrow of the reused allocation).
    pub fn buf(&self) -> &RgbBuffer {
        &self.floor.buf
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
        let layout = frame_prologue(&mut self.floor.ctx, buf_w, buf_h, floor_meta.floor_seed)?;
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
        .max(1)
}

/// One agent projected onto a floor by [`build_floor_scene`]: the slot — its
/// `desk_index` still the ORIGINAL global allocation — paired with its desk in
/// the floor's OWN local space, typed as such (`FloorLocalDeskIndex`, the #209
/// currency). The projection used to write the floor-local offset back into
/// `AgentSlot.desk_index` mid-flight, making that field's GLOBAL type a lie
/// until [`project_floor_scene`] re-hosted the slot; the pair keeps the
/// currency honest up to the re-host.
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
mod tests {
    use super::*;
    use pixtuoid_core::id::AgentId;
    use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn daemons_projects_onto_the_ground_floor_only() {
        // The gateway mascot is global, not per-floor — the projection carries
        // daemons onto floor 0 ONLY, so a multi-floor office renders the lobster
        // exactly once (a regression dropping the gate / flipping the index would
        // duplicate him on every floor).
        use pixtuoid_core::state::{DaemonPresence, DaemonState};
        let mut scene = SceneState::uniform(16);
        scene.floor_capacities[1] = 16; // a second floor exists
        scene.daemons_mut().insert(
            pixtuoid_core::source::openclaw::SOURCE_NAME.to_string(),
            DaemonPresence {
                state: DaemonState::Idle,
                active_sessions: 0,
                last_seen: SystemTime::UNIX_EPOCH,
                entered_at: SystemTime::UNIX_EPOCH,
                in_flight_run_keys: Default::default(),
                current_pid: Some(1),
            },
        );
        assert!(
            !project_floor_scene(&scene, 0).daemons().is_empty(),
            "floor 0 carries the mascot"
        );
        assert!(
            project_floor_scene(&scene, 1).daemons().is_empty(),
            "floor 1+ must NOT (render-once invariant)"
        );
    }

    #[test]
    fn door_anim_excludes_arrived_entry_profiles() {
        use crate::motion::MotionState;
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let id = AgentId::from_transcript_path("/p/door.jsonl");
        let mut fctx = FloorCtx::new();
        let mut ms = MotionState::new(id);
        // Entry walk: duration 2000ms + pause 300ms → walk_arrived at 2300ms.
        ms.entry = Some((
            t0,
            WalkProfile {
                duration_ms: 2000,
                pause_ms: 300,
                path_len_octile: 500,
                v_cruise: 0.36,
                accel: 6.5e-4,
            },
        ));
        fctx.motion.insert(id, ms);

        // Mid-walk → profile is in-flight → it sets the door window.
        fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(1000));
        assert_eq!(
            fctx.door_anim_max_ms, 2300,
            "in-flight entry walk should drive the door cosmetic window"
        );

        // Past arrival (>= duration + pause) → excluded so the door closes,
        // even though MotionState.entry is never cleared for this agent.
        fctx.recompute_door_anim_max_ms(t0 + Duration::from_millis(3000));
        assert_eq!(
            fctx.door_anim_max_ms, 0,
            "an arrived entry profile must not hold the door open for the agent's lifetime"
        );
    }

    #[test]
    fn floor_ctx_default_equals_new() {
        // Both Default impls delegate to new(); pin the equivalence so a future
        // field addition can't make `default()` diverge silently.
        let d = FloorCtx::default();
        assert_eq!(
            d.door_anim_max_ms, 0,
            "FloorCtx::default() must match new() (door_anim_max_ms == 0)"
        );
        assert!(
            d.motion.is_empty(),
            "default FloorCtx has no in-flight motion"
        );
    }

    #[test]
    fn lighting_state_default_equals_new() {
        // LightingState::default() delegates to new() — both start fully lit.
        assert_eq!(
            LightingState::default().level(),
            LightingState::new().level(),
            "LightingState::default() must equal new()"
        );
        assert_eq!(
            LightingState::default().level(),
            1.0,
            "a fresh LightingState is fully lit"
        );
    }

    fn make_scene(n: usize, max_desks: usize) -> SceneState {
        let mut s = SceneState::uniform(max_desks);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for i in 0..n {
            let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
            let floor_idx = s.floor_of(GlobalDeskIndex(i));
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("cc"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/repo")),
                    label: format!("a{i}").into(),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,

                    desk_index: GlobalDeskIndex(i),
                    floor_idx,
                    tool_call_count: 0,
                    active_ms: 0,
                    unknown_cwd: false,
                    parent_id: None,
                },
            );
        }
        s
    }

    #[test]
    fn floor_of_maps_desk_to_floor() {
        let s = SceneState::uniform(16);
        assert_eq!(s.floor_of(GlobalDeskIndex(0)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(15)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(16)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(31)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(32)), 2);
    }

    #[test]
    fn floor_local_desk_remaps_to_floor_range() {
        let s = SceneState::uniform(16);
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(0)),
            FloorLocalDeskIndex(0)
        );
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(16)),
            FloorLocalDeskIndex(0)
        );
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(17)),
            FloorLocalDeskIndex(1)
        );
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(31)),
            FloorLocalDeskIndex(15)
        );
    }

    #[test]
    fn num_floors_with_overflow() {
        let scene = make_scene(20, 16);
        assert_eq!(num_floors(&scene), 2);
    }

    #[test]
    fn num_floors_exact_fit() {
        let scene = make_scene(16, 16);
        assert_eq!(num_floors(&scene), 1);
    }

    #[test]
    fn num_floors_empty() {
        let scene = make_scene(0, 16);
        assert_eq!(num_floors(&scene), 1);
    }

    #[test]
    fn build_floor_scene_filters_and_remaps() {
        let scene = make_scene(20, 16);

        let floor0 = build_floor_scene(&scene, 0);
        assert_eq!(floor0.len(), 16);
        for p in &floor0 {
            assert!(p.desk.0 < 16, "local desk {} out of range", p.desk.0);
        }

        let floor1 = build_floor_scene(&scene, 1);
        assert_eq!(floor1.len(), 4);
        let mut indices: Vec<usize> = floor1.iter().map(|p| p.desk.0).collect();
        indices.sort();
        assert_eq!(indices, vec![0, 1, 2, 3]);
        // The pair keeps the currency honest (#13): the LOCAL desk lives in
        // the typed FloorLocalDeskIndex, while the slot's GLOBAL desk_index
        // is untouched — floor 1's agents keep their real allocation (16..20)
        // until project_floor_scene's documented re-host.
        let mut globals: Vec<usize> = floor1.iter().map(|p| p.slot.desk_index.0).collect();
        globals.sort();
        assert_eq!(globals, vec![16, 17, 18, 19]);
    }

    #[test]
    fn build_floor_scene_remap_is_local_global_coincident() {
        // The doc-comment-backed property on `build_floor_scene`: within a
        // projected `uniform(cap)` scene the global desk space coincides with
        // its (only) floor's local space, so the remapped `GlobalDeskIndex`
        // is simultaneously a valid global index for the smaller scene AND —
        // through the typed bridge — the floor-local index the render path
        // needs. This is what makes `single_floor_local` an identity there.
        let scene = make_scene(20, 16);
        for floor_idx in 0..num_floors(&scene) {
            let projected = project_floor_scene(&scene, floor_idx);
            for slot in projected.agents.values() {
                assert_eq!(projected.floor_of(slot.desk_index), 0);
                assert_eq!(
                    projected.floor_local_desk(slot.desk_index).0,
                    slot.desk_index.0,
                    "projected scene: bridge must be the identity"
                );
                assert_eq!(
                    projected.floor_local_desk(slot.desk_index),
                    slot.desk_index.single_floor_local(),
                    "typed bridge and identity cast must agree in a projection"
                );
            }
        }
    }

    #[test]
    fn build_floor_scene_skips_agent_below_grown_offset() {
        // Agent assigned desk 5 on floor 1 when floor 0 had capacity 4.
        // Floor 0 later grows to capacity 8. floor_range(1).start = 8,
        // so desk 5 < 8 and the agent should be invisible on floor 1.
        let mut s = SceneState::new([4, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let id = AgentId::from_transcript_path("/p/stale.jsonl");
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("cc"),
                session_id: Arc::from("s"),
                cwd: Arc::from(Path::new("/repo")),
                label: "stale".into(),
                state: ActivityState::Idle,
                state_started_at: now,
                created_at: now,
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(5),
                floor_idx: 1,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
        // Simulate floor 0 capacity growth
        s.floor_capacities = [8, 4, 0, 0, 0, 0, 0, 0, 0, 0];
        let floor1 = build_floor_scene(&s, 1);
        assert!(
            floor1.is_empty(),
            "agent below grown offset must be skipped, not mapped to desk 0"
        );
    }

    #[test]
    fn num_floors_variable_capacities() {
        // F0: 0..4, F1: 4..12 — 6 agents span 2 floors
        let mut s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for i in 0..6 {
            let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
            let floor_idx = s.floor_of(GlobalDeskIndex(i));
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("cc"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/repo")),
                    label: format!("a{i}").into(),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,
                    desk_index: GlobalDeskIndex(i),
                    floor_idx,
                    tool_call_count: 0,
                    active_ms: 0,
                    unknown_cwd: false,
                    parent_id: None,
                },
            );
        }
        assert_eq!(num_floors(&s), 2);
    }

    #[test]
    fn transition_t_progresses() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, start);

        assert!((tr.t(start) - 0.0).abs() < f32::EPSILON);

        let mid = start + Duration::from_millis(450);
        let t_mid = tr.t(mid);
        assert!(
            t_mid > 0.0 && t_mid < 1.0,
            "mid should be between 0 and 1, got {t_mid}"
        );

        let end = start + Duration::from_millis(900);
        assert!((tr.t(end) - 1.0).abs() < f32::EPSILON);
        assert!(!tr.is_done(start + Duration::from_millis(450)));
        assert!(tr.is_done(end));
    }

    #[test]
    fn transition_t_clamps_past_duration() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, start);

        let past = start + Duration::from_millis(1000);
        assert!((tr.t(past) - 1.0).abs() < f32::EPSILON);
        assert!(tr.is_done(past));
    }

    // ---- LightingState ----------------------------------------------------

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn light_steady_state_populated() {
        let mut light = LightingState::new();
        let start = t0();
        // Many frames over multiple seconds with `empty=false` should not
        // move the level away from 1.0.
        for ms in (0..3_000).step_by(33) {
            let level = light.tick(false, start + Duration::from_millis(ms));
            assert!(
                (level - 1.0).abs() < 1e-6,
                "populated steady state drifted: ms={ms} level={level}"
            );
        }
    }

    #[test]
    fn light_holds_during_debounce_window() {
        let mut light = LightingState::new();
        let start = t0();
        light.tick(true, start);
        // 4 s after going empty (< 5 s debounce) — target should still be
        // 1.0 so level holds.
        let level = light.tick(true, start + Duration::from_millis(4_000));
        assert!(
            (level - 1.0).abs() < 1e-6,
            "level dropped before debounce expired: {level}"
        );
    }

    #[test]
    fn light_eases_toward_min_after_debounce() {
        let mut light = LightingState::new();
        let start = t0();
        light.tick(true, start);
        // Sample at 6 s (debounce expired 1 s ago, ~1.25 tau of fade).
        let level = light.tick(true, start + Duration::from_millis(6_000));
        assert!(level < 0.95, "no fade started after debounce: {level}");
        assert!(level > LightingState::MIN_LEVEL, "overshot floor: {level}");
    }

    #[test]
    fn light_converges_to_min_when_empty_long_enough() {
        let mut light = LightingState::new();
        let start = t0();
        // Step the tick at a realistic frame cadence for 30 s so the
        // exponential ease has fully landed.
        for ms in (0..30_000).step_by(33) {
            light.tick(true, start + Duration::from_millis(ms));
        }
        let level = light.level();
        assert!(
            (level - LightingState::MIN_LEVEL).abs() < 1e-3,
            "did not converge to MIN_LEVEL: {level}"
        );
    }

    #[test]
    fn light_rises_back_when_repopulated() {
        let mut light = LightingState::new();
        let start = t0();
        // Drive level all the way down.
        for ms in (0..20_000).step_by(33) {
            light.tick(true, start + Duration::from_millis(ms));
        }
        assert!(light.level() < 0.2);
        // Populated → target snaps to 1.0; verify the ease climbs back.
        let later = start + Duration::from_millis(20_000);
        for ms in (0..3_000).step_by(33) {
            light.tick(false, later + Duration::from_millis(ms));
        }
        let level = light.level();
        assert!(level > 0.95, "did not rise back when repopulated: {level}");
    }

    #[test]
    fn light_resets_empty_since_when_repopulated() {
        let mut light = LightingState::new();
        let start = t0();
        // Empty for 3 s (within debounce).
        light.tick(true, start);
        light.tick(true, start + Duration::from_millis(3_000));
        // Briefly populated — should clear the debounce timer.
        light.tick(false, start + Duration::from_millis(3_500));
        // Empty again — debounce timer must restart from this moment, so
        // 4 s later we should STILL be holding at 1.0, not faded.
        light.tick(true, start + Duration::from_millis(3_600));
        let level = light.tick(true, start + Duration::from_millis(7_500));
        assert!(
            (level - 1.0).abs() < 1e-6,
            "empty_since did not reset on repopulate: {level}"
        );
    }

    #[test]
    fn light_large_dt_does_not_overshoot_or_nan() {
        let mut light = LightingState::new();
        let start = t0();
        light.tick(true, start);
        // Huge dt (1 day) past the debounce. exp(-dt/tau) underflows to 0
        // so alpha = 1.0; level should land exactly at target (MIN_LEVEL),
        // not overshoot or produce NaN.
        let later = start + Duration::from_millis(LightingState::EMPTY_DEBOUNCE_MS + 1_000);
        let level = light.tick(true, later);
        assert!(level.is_finite(), "level went non-finite: {level}");
        assert!(
            level >= LightingState::MIN_LEVEL - 1e-6,
            "level undershot floor: {level}"
        );
    }

    #[test]
    fn light_backward_clock_jump_does_not_move_level() {
        let mut light = LightingState::new();
        let start = t0();
        // Bring level to a known mid value via a real tick.
        light.tick(false, start);
        let before = light.level();
        // A backward "now" makes duration_since() error; the impl uses
        // `.ok()` so dt collapses to 0 and the level should not change.
        let backward = start - Duration::from_millis(500);
        let level = light.tick(true, backward);
        assert!(
            (level - before).abs() < 1e-9,
            "backward clock jump moved level: before={before} after={level}"
        );
    }

    #[test]
    fn light_snap_to_empty_forces_min_level() {
        let mut light = LightingState::new();
        light.snap_to_empty();
        assert!((light.level() - LightingState::MIN_LEVEL).abs() < f32::EPSILON);
    }

    #[test]
    fn coffee_record_stamps_only_new_carriers_and_evict_follows_the_scene() {
        let id = AgentId::from_parts("claude-code", "coffee-test");
        let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let t1 = t0 + std::time::Duration::from_secs(60);
        let mut coffee = CoffeeState::new();
        coffee.record([id], t0);
        assert_eq!(coffee.map().get(&id), Some(&t0), "a new carrier is stamped");
        // Re-recording an existing carrier must NOT restart its steam window.
        coffee.record([id], t1);
        assert_eq!(
            coffee.map().get(&id),
            Some(&t0),
            "an already-recorded carrier keeps its original fetch stamp"
        );
        // The agent leaving the scene evicts the cup + stamp (one entry).
        let empty = SceneState::new([8; MAX_FLOORS]);
        coffee.evict_missing(&empty);
        assert!(coffee.map().is_empty());
    }

    #[test]
    fn coffee_second_trip_after_steam_window_restamps() {
        let id = AgentId::from_parts("claude-code", "coffee-refetch");
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut coffee = CoffeeState::new();
        coffee.record([id], t0);
        // Within the window: per-frame walk-back re-reports keep the stamp.
        let within = t0 + Duration::from_secs(CoffeeState::STEAM_WINDOW_SECS - 1);
        coffee.record([id], within);
        assert_eq!(
            coffee.map().get(&id),
            Some(&t0),
            "a re-report within the steam window keeps the original stamp"
        );
        // A report past the window is a genuinely NEW pantry fetch (the old
        // cup's steam long expired) — the stamp must refresh so the fresh cup
        // steams again instead of landing permanently steam-less.
        let refetch = t0 + Duration::from_secs(CoffeeState::STEAM_WINDOW_SECS * 3);
        coffee.record([id], refetch);
        assert_eq!(
            coffee.map().get(&id),
            Some(&refetch),
            "a fetch after the steam window expired must restamp"
        );
    }

    #[test]
    fn transition_escapes_a_backward_clock_step() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, start);
        // A small backward wobble (within one transition duration) is clock
        // jitter: hold at t = 0 and let the clock catch up.
        let wobble = start - Duration::from_millis(100);
        assert!(
            !tr.is_done(wobble),
            "a small wobble must not abort the slide"
        );
        assert!((tr.t(wobble) - 0.0).abs() < f32::EPSILON);
        // A step to before started_at by MORE than the transition's own
        // duration can't be render-loop jitter — without an escape the
        // renderer stays wedged in the transition composite (no labels,
        // tooltips, or hit-testing) until the wall clock re-passes started_at.
        let stepped = start - Duration::from_millis(tr.duration_ms * 2);
        assert!(
            tr.is_done(stepped),
            "a large backward clock step must complete the transition"
        );
    }

    #[test]
    fn render_floor_paints_records_coffee_state_and_survives_a_tiny_buffer() {
        let pack = crate::embedded_pack::test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let scene = SceneState::new([8; MAX_FLOORS]);
        let mut fctx = FloorCtx::new();
        let mut buf = RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 });
        let mut coffee = CoffeeState::new();
        let mut chitchat = HashMap::new();

        // A too-small buffer: no layout, `None`, no panic — the buffer is
        // resized+cleared but unpainted.
        let none = render_floor(
            &mut fctx,
            &mut buf,
            &mut coffee,
            &mut chitchat,
            &scene,
            &pack,
            theme,
            now,
            8,
            8,
            FloorMeta::ground(),
            None,
            None,
            false,
        );
        assert!(none.is_none(), "an unlayoutable size returns None");
        assert_eq!(
            (buf.width(), buf.height()),
            (8, 8),
            "the buffer was still sized"
        );

        // A real size: the layout comes back and the pass painted content
        // beyond the cleared background fill.
        let layout = render_floor(
            &mut fctx,
            &mut buf,
            &mut coffee,
            &mut chitchat,
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
            None,
            false,
        );
        assert!(layout.is_some(), "a layoutable size returns the layout");
        let bg = theme.surface.bg_fallback;
        assert!(
            buf.as_slice()
                .iter()
                .any(|p| *p != pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 } && *p != bg),
            "the pixel pass painted office content"
        );
    }

    // ---- FloorSession -----------------------------------------------------

    #[test]
    fn floor_session_render_owns_the_dual_eviction() {
        // The drift classes that actually shipped: the floating painter never
        // evicted (per-agent motion/cache/coffee leaked for the window's
        // lifetime) and the web hero forgot eviction entirely (the loop-2
        // teleport). FloorSession::render runs BOTH halves itself, so a
        // painter can no longer forget either one.
        let pack = crate::embedded_pack::test_default_pack();
        let theme = crate::theme::theme_by_name("normal").expect("normal theme exists");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let gone = AgentId::from_parts("claude-code", "session-evict");
        let mut session = FloorSession::new();
        session
            .floor
            .ctx
            .motion
            .insert(gone, MotionState::new(gone));
        session.office.coffee.insert(gone, now);

        // `gone` is not in the scene → one render() must drop both entries.
        let scene = SceneState::new([8; MAX_FLOORS]);
        let layout = session.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
            None,
            false,
        );
        assert!(layout.is_some(), "a layoutable size renders");
        assert!(
            !session.floor.ctx.motion.contains_key(&gone),
            "render() evicts the floor half (motion) — the floating-leak class"
        );
        assert!(
            !session.office.coffee.map().contains_key(&gone),
            "render() evicts the office half (coffee) — the cup leaves with the agent"
        );
    }

    #[test]
    fn floor_session_observe_advances_the_world_without_a_pixel_buffer() {
        // The headless observation seam the sim/paint split (#450) prepared:
        // eviction + layout prologue + sim_step + the coffee/door epilogue,
        // with NO pixel buffer touched. A fresh agent's entry walk must
        // populate motion and the door-anim clamp, and the frame must carry
        // its pose.
        let pack = crate::embedded_pack::test_default_pack();
        let scene = make_scene(1, 8);
        let id = AgentId::from_transcript_path("/p/0.jsonl");
        let t = t0() + Duration::from_millis(100); // 100ms in: entry walk in flight
        let mut session = FloorSession::new();

        let frame = session
            .observe(&scene, &pack, 160, 96, FloorMeta::ground(), t)
            .expect("a layoutable size observes");
        assert!(
            frame.poses.contains_key(&id),
            "the frame carries the agent's routed pose"
        );
        assert!(
            session.floor.ctx.motion.contains_key(&id),
            "the sim advanced: the entry leg was snapshotted into motion"
        );
        assert!(
            session.floor.ctx.door_anim_max_ms > 0,
            "the epilogue ran headlessly: the in-flight entry drives the door clamp"
        );
        assert_eq!(
            (session.buf().width(), session.buf().height()),
            (0, 0),
            "no pixel buffer was bought"
        );

        // Too small for any layout: None, never a panic.
        assert!(
            session
                .observe(&scene, &pack, 8, 8, FloorMeta::ground(), t)
                .is_none(),
            "an unlayoutable size observes nothing"
        );
    }

    #[test]
    fn session_types_default_equals_new() {
        // Same convention pin as FloorCtx/LightingState above: Default and
        // new() must not diverge on a future field addition.
        assert_eq!(PerFloor::default().ctx.door_anim_max_ms, 0);
        assert_eq!(
            (
                PerFloor::default().buf.width(),
                PerFloor::default().buf.height()
            ),
            (0, 0)
        );
        assert!(PerOffice::default().coffee.map().is_empty());
        assert!(PerOffice::default().chitchat.is_empty());
        let s = FloorSession::default();
        assert!(s.floor.ctx.motion.is_empty());
        assert!(s.office.coffee.map().is_empty());
    }
}
