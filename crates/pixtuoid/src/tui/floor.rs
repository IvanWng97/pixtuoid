//! Multi-floor office partitioning.
//!
//! When more agents are active than `max_desks` can seat on a single floor,
//! the scene is split into multiple floors. This module provides the pure
//! arithmetic (which floor does desk N belong to? how many floors exist?)
//! and the per-floor rendering context (`FloorCtx`) so each floor owns its
//! own router, overlay, pose history, and frame cache.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::physics::{walk_arrived, WalkProfile};
use pixtuoid_core::state::{AgentSlot, SceneState};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::tui::frame_cache::FrameCache;
use crate::tui::motion::MotionState;
use crate::tui::pathfind::AStarRouter;
use crate::tui::pose::PoseHistory;

pub use pixtuoid_core::state::MAX_FLOORS;

/// Fibonacci hash multiplier for floor seed derivation. Used in both
/// `FloorMeta::for_floor` and the TUI auto-compute loop.
pub const FLOOR_SEED_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

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
            floor_seed: (floor_idx as u64).wrapping_mul(FLOOR_SEED_MULTIPLIER),
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
    /// Evicted alongside `history` and `cache` when the agent leaves.
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
            let elapsed = now
                .duration_since(started_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;
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
            let elapsed = now.duration_since(since).unwrap_or_default().as_millis() as u64;
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
        crate::tui::anim::eased_progress(
            self.started_at,
            self.duration_ms as u32,
            crate::tui::anim::Easing::EaseInOutCubic,
            now,
        )
    }

    pub fn is_done(&self, now: SystemTime) -> bool {
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

/// Extract agents belonging to `floor_idx`, remapping their `desk_index`
/// into the `[0..capacity)` range so the layout engine sees a
/// self-contained floor. Uses the stored `floor_idx` on each slot so
/// capacity growth never migrates agents between floors.
pub fn build_floor_scene(scene: &SceneState, floor_idx: usize) -> Vec<AgentSlot> {
    let offset = scene.floor_range(floor_idx).start;
    scene
        .agents
        .values()
        .filter(|a| a.floor_idx == floor_idx)
        .filter_map(|a| {
            if a.desk_index < offset {
                return None;
            }
            let mut slot = a.clone();
            slot.desk_index = a.desk_index - offset;
            Some(slot)
        })
        .collect()
}

/// Build a self-contained `SceneState` for one floor: a `uniform(cap)` scene
/// (so floor arithmetic stays self-consistent with the remapped desk indices
/// in `[0..cap)`) populated with just that floor's agents. The normal and
/// floor-transition render paths both project the global scene this way.
pub fn project_floor_scene(scene: &SceneState, floor_idx: usize) -> SceneState {
    let mut s = SceneState::uniform(scene.floor_capacities[floor_idx]);
    for a in build_floor_scene(scene, floor_idx) {
        s.agents.insert(a.agent_id, a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::id::AgentId;
    use pixtuoid_core::state::ActivityState;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn door_anim_excludes_arrived_entry_profiles() {
        use crate::tui::motion::MotionState;
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

    fn make_scene(n: usize, max_desks: usize) -> SceneState {
        let mut s = SceneState::uniform(max_desks);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for i in 0..n {
            let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
            let floor_idx = s.floor_of(i);
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("cc"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/repo")),
                    label: Arc::from(format!("a{i}").as_str()),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,

                    desk_index: i,
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
        assert_eq!(s.floor_of(0), 0);
        assert_eq!(s.floor_of(15), 0);
        assert_eq!(s.floor_of(16), 1);
        assert_eq!(s.floor_of(31), 1);
        assert_eq!(s.floor_of(32), 2);
    }

    #[test]
    fn floor_local_desk_remaps_to_floor_range() {
        let s = SceneState::uniform(16);
        assert_eq!(s.floor_local_desk(0), 0);
        assert_eq!(s.floor_local_desk(16), 0);
        assert_eq!(s.floor_local_desk(17), 1);
        assert_eq!(s.floor_local_desk(31), 15);
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
        for a in &floor0 {
            assert!(
                a.desk_index < 16,
                "desk_index {} out of range",
                a.desk_index
            );
        }

        let floor1 = build_floor_scene(&scene, 1);
        assert_eq!(floor1.len(), 4);
        let mut indices: Vec<usize> = floor1.iter().map(|a| a.desk_index).collect();
        indices.sort();
        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn build_floor_scene_skips_agent_below_grown_offset() {
        // Agent assigned desk 5 on floor 1 when floor 0 had capacity 4.
        // Floor 0 later grows to capacity 8. floor_range(1).start = 8,
        // so desk 5 < 8 and the agent should be invisible on floor 1.
        let mut s = SceneState::new([4, 4, 0, 0, 0]);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let id = AgentId::from_transcript_path("/p/stale.jsonl");
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("cc"),
                session_id: Arc::from("s"),
                cwd: Arc::from(Path::new("/repo")),
                label: Arc::from("stale"),
                state: ActivityState::Idle,
                state_started_at: now,
                created_at: now,
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: 5,
                floor_idx: 1,
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
        // Simulate floor 0 capacity growth
        s.floor_capacities = [8, 4, 0, 0, 0];
        let floor1 = build_floor_scene(&s, 1);
        assert!(
            floor1.is_empty(),
            "agent below grown offset must be skipped, not mapped to desk 0"
        );
    }

    #[test]
    fn num_floors_variable_capacities() {
        // F0: 0..4, F1: 4..12 — 6 agents span 2 floors
        let mut s = SceneState::new([4, 8, 6, 4, 2]);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for i in 0..6 {
            let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
            let floor_idx = s.floor_of(i);
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("cc"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/repo")),
                    label: Arc::from(format!("a{i}").as_str()),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,
                    desk_index: i,
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
}
