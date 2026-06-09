use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::id::AgentId;

mod fsm;
pub mod reducer;
mod scope;

pub const MAX_FLOORS: usize = 10;

/// `AgentSlot` strings (label, source, session_id) and paths (cwd) are
/// stored as `Arc<str>` / `Arc<Path>` so `SceneState::clone()` is a series
/// of pointer copies instead of heap allocations. At 30 fps with N agents
/// this turns ~5N allocations/frame into 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    Idle,
    Active {
        tool_use_id: Option<Arc<str>>,
        detail: Option<Arc<str>>,
    },
    Waiting {
        reason: Arc<str>,
    },
}

#[derive(Debug, Clone)]
pub struct AgentSlot {
    pub agent_id: AgentId,
    pub source: Arc<str>,
    pub session_id: Arc<str>,
    pub cwd: Arc<Path>,
    pub label: Arc<str>,
    pub state: ActivityState,
    pub state_started_at: SystemTime,
    /// Wall-clock time of the most recent event (any type) from this
    /// agent. The stale-agent sweep uses this as the primary liveness
    /// signal — if `now - last_event_at` exceeds a state-dependent
    /// threshold, the agent is presumed dead and begins the exit
    /// animation. Updated on every `reducer::apply` that touches the slot.
    pub last_event_at: SystemTime,
    /// Wall-clock time the slot was first created. Distinct from
    /// `state_started_at` (updated on every state change) so the renderer
    /// can play a one-shot entry animation for the first few seconds of
    /// an agent's life regardless of later state transitions.
    pub created_at: SystemTime,
    /// Set when the reducer has received `SessionEnd` for this agent but
    /// is keeping the slot alive long enough for the exit animation to
    /// play. The reducer sweeps expired slots on subsequent events.
    pub exiting_at: Option<SystemTime>,
    /// Active→Idle debounce mark. Set by `ActivityEnd` instead of an
    /// immediate state flip; cleared by any later `ActivityStart`/Waiting.
    /// `reducer.tick` expires it after `ACTIVE_GRACE_WINDOW` and flips
    /// state to Idle. Hides the per-tool-call Active flicker that rapid
    /// PreToolUse → PostToolUse chains produce in CC.
    pub pending_idle_at: Option<SystemTime>,
    /// GLOBAL flat desk index across all floors (assigned once at `SessionStart`,
    /// never mutated). NOT a floor-local index: `build_floor_scene` (tui/floor.rs)
    /// remaps it to a floor-local index before any `layout.home_desks` lookup —
    /// indexing `home_desks` with this raw value is a bug. `floor_idx` derives
    /// from it via `floor_of()`.
    pub desk_index: usize,
    /// Floor assigned at desk allocation time. Immutable for the agent's
    /// lifetime so capacity growth never silently migrates agents between
    /// floors.
    pub floor_idx: usize,
    pub tool_call_count: u32,
    pub active_ms: u64,
    pub unknown_cwd: bool,
    pub parent_id: Option<AgentId>,
}

#[derive(Debug, Clone)]
pub struct SceneState {
    pub agents: BTreeMap<AgentId, AgentSlot>,
    pub floor_capacities: [usize; MAX_FLOORS],
}

impl Default for SceneState {
    fn default() -> Self {
        Self {
            agents: BTreeMap::new(),
            floor_capacities: [0; MAX_FLOORS],
        }
    }
}

impl SceneState {
    pub fn new(floor_capacities: [usize; MAX_FLOORS]) -> Self {
        Self {
            agents: BTreeMap::new(),
            floor_capacities,
        }
    }

    pub fn uniform(cap: usize) -> Self {
        Self::new([cap; MAX_FLOORS])
    }

    pub fn total_capacity(&self) -> usize {
        self.floor_capacities.iter().sum()
    }

    /// Cumulative desk offsets: entry `i` = sum of capacities for floors `0..i`.
    fn cumulative_offsets(&self) -> [usize; MAX_FLOORS] {
        let mut offsets = [0usize; MAX_FLOORS];
        for i in 1..MAX_FLOORS {
            offsets[i] = offsets[i - 1] + self.floor_capacities[i - 1];
        }
        offsets
    }

    /// Which floor does `desk_index` belong to, given precomputed `offsets`?
    fn floor_of_with_offsets(&self, desk_index: usize, offsets: &[usize; MAX_FLOORS]) -> usize {
        for i in (0..MAX_FLOORS).rev() {
            if self.floor_capacities[i] > 0 && desk_index >= offsets[i] {
                return i;
            }
        }
        0
    }

    /// Which floor does `desk_index` belong to?
    pub fn floor_of(&self, desk_index: usize) -> usize {
        self.floor_of_with_offsets(desk_index, &self.cumulative_offsets())
    }

    /// Local desk offset within the floor.
    pub fn floor_local_desk(&self, desk_index: usize) -> usize {
        let offsets = self.cumulative_offsets();
        let floor = self.floor_of_with_offsets(desk_index, &offsets);
        desk_index - offsets[floor]
    }

    /// Global desk index range `[lo, hi)` for a given floor.
    /// Clamps `floor_idx` to `MAX_FLOORS - 1` to avoid panics.
    pub fn floor_range(&self, floor_idx: usize) -> std::ops::Range<usize> {
        let idx = floor_idx.min(MAX_FLOORS - 1);
        let offsets = self.cumulative_offsets();
        let lo = offsets[idx];
        let hi = lo + self.floor_capacities[idx];
        lo..hi
    }

    /// Lowest free desk index, or `None` if all desks are occupied.
    pub fn next_free_desk(&self) -> Option<usize> {
        let occupied: std::collections::BTreeSet<usize> =
            self.agents.values().map(|a| a.desk_index).collect();
        (0..self.total_capacity()).find(|i| !occupied.contains(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_slot(id: AgentId, desk_index: usize) -> AgentSlot {
        let now = SystemTime::now();
        AgentSlot {
            agent_id: id,
            source: Arc::from("cc"),
            session_id: Arc::from("s0"),
            cwd: Arc::from(Path::new("/repo")),
            label: Arc::from("a0"),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        }
    }

    #[test]
    fn next_free_desk_starts_at_zero() {
        let s = SceneState::uniform(4);
        assert_eq!(s.next_free_desk(), Some(0));
    }

    #[test]
    fn next_free_desk_returns_none_when_full() {
        let mut s = SceneState::uniform(2);
        let total = s.total_capacity();
        for i in 0..total {
            let id = AgentId::from_transcript_path(&format!("p{i}"));
            s.agents.insert(id, make_slot(id, i));
        }
        assert_eq!(s.next_free_desk(), None);
    }

    #[test]
    fn next_free_desk_overflows_to_second_floor() {
        let mut s = SceneState::uniform(4);
        for i in 0..4 {
            let id = AgentId::from_transcript_path(&format!("f{i}"));
            s.agents.insert(id, make_slot(id, i));
        }
        assert_eq!(
            s.next_free_desk(),
            Some(4),
            "should overflow to desk 4 (floor 1)"
        );
    }

    #[test]
    fn floor_of_uniform() {
        let s = SceneState::uniform(8);
        assert_eq!(s.floor_of(0), 0);
        assert_eq!(s.floor_of(7), 0);
        assert_eq!(s.floor_of(8), 1);
        assert_eq!(s.floor_of(15), 1);
        assert_eq!(s.floor_of(16), 2);
    }

    #[test]
    fn floor_of_variable_capacities() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        // F0: 0..4, F1: 4..12, F2: 12..18, F3: 18..22, F4: 22..24
        assert_eq!(s.floor_of(0), 0);
        assert_eq!(s.floor_of(3), 0);
        assert_eq!(s.floor_of(4), 1);
        assert_eq!(s.floor_of(11), 1);
        assert_eq!(s.floor_of(12), 2);
        assert_eq!(s.floor_of(17), 2);
        assert_eq!(s.floor_of(18), 3);
        assert_eq!(s.floor_of(22), 4);
        assert_eq!(s.floor_of(23), 4);
    }

    #[test]
    fn floor_local_desk_variable() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        assert_eq!(s.floor_local_desk(0), 0);
        assert_eq!(s.floor_local_desk(3), 3);
        assert_eq!(s.floor_local_desk(4), 0); // first desk on F1
        assert_eq!(s.floor_local_desk(11), 7); // last desk on F1
        assert_eq!(s.floor_local_desk(12), 0); // first desk on F2
    }

    #[test]
    fn floor_range_variable() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        assert_eq!(s.floor_range(0), 0..4);
        assert_eq!(s.floor_range(1), 4..12);
        assert_eq!(s.floor_range(2), 12..18);
        assert_eq!(s.floor_range(3), 18..22);
        assert_eq!(s.floor_range(4), 22..24);
    }

    #[test]
    fn total_capacity_sums_all_floors() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        assert_eq!(s.total_capacity(), 24);

        let u = SceneState::uniform(8);
        assert_eq!(u.total_capacity(), 80);
    }

    #[test]
    fn next_free_desk_with_variable_capacities() {
        let mut s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        // Fill F0 (desks 0..4)
        for i in 0..4 {
            let id = AgentId::from_transcript_path(&format!("f{i}"));
            s.agents.insert(id, make_slot(id, i));
        }
        // Next free should be desk 4 (first desk on F1)
        assert_eq!(s.next_free_desk(), Some(4));
    }

    #[test]
    fn zero_capacity_floor_skipped_by_next_free_desk() {
        let s = SceneState::new([4, 0, 6, 0, 2, 0, 0, 0, 0, 0]);
        // F0: 0..4, F1: 4..4 (empty), F2: 4..10, F3: 10..10, F4: 10..12
        assert_eq!(s.total_capacity(), 12);
        assert_eq!(s.floor_range(0), 0..4);
        assert_eq!(s.floor_range(1), 4..4);
        assert_eq!(s.floor_range(2), 4..10);
        assert_eq!(s.next_free_desk(), Some(0));
    }

    #[test]
    fn floor_of_skips_zero_capacity_floors() {
        let s = SceneState::new([4, 0, 6, 0, 2, 0, 0, 0, 0, 0]);
        // Desk 4 is first desk of F2 (F1 has zero capacity)
        assert_eq!(s.floor_of(4), 2);
        assert_eq!(s.floor_local_desk(4), 0);
        assert_eq!(s.floor_of(9), 2);
        assert_eq!(s.floor_of(10), 4);
    }

    #[test]
    fn floor_of_leading_zero_capacity_floors() {
        let s = SceneState::new([0, 0, 6, 4, 2, 0, 0, 0, 0, 0]);
        // F0 and F1 have zero capacity, desk 0 belongs to F2
        assert_eq!(s.floor_of(0), 2);
        assert_eq!(s.floor_of(5), 2);
        assert_eq!(s.floor_of(6), 3);
    }

    #[test]
    fn floor_range_clamps_oob_index() {
        let s = SceneState::uniform(4);
        // floor_idx >= MAX_FLOORS should clamp to last floor
        let last = s.floor_range(MAX_FLOORS - 1);
        let oob = s.floor_range(MAX_FLOORS + 10);
        assert_eq!(last, oob);
    }

    #[test]
    fn floor_local_desk_oob_lands_on_last_nonempty_floor() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        let total = s.total_capacity(); // 24
                                        // desk_index 100 is beyond capacity — floor_of returns the last
                                        // floor with nonzero capacity (floor 4, offset 22).
        let oob = total + 76; // 100
        let floor = s.floor_of(oob);
        assert_eq!(floor, 4, "OOB desk lands on last nonempty floor");
        let local = s.floor_local_desk(oob);
        // offsets[4] = 22, so local = 100 - 22 = 78
        assert_eq!(local, oob - 22);
    }

    #[test]
    fn scene_supports_up_to_ten_floors() {
        // Raising MAX_FLOORS to 10: a uniform office spans ten floors, seats
        // 10× a single floor's desks, and a desk on the tenth floor (index 9)
        // resolves there rather than clamping to a lower floor.
        let s = SceneState::uniform(2);
        assert_eq!(s.floor_capacities.len(), 10, "office spans ten floors");
        assert_eq!(s.total_capacity(), 20, "ten floors × 2 desks");
        assert_eq!(
            s.floor_of(18),
            9,
            "desk 18 is the first seat on the tenth floor"
        );
        assert_eq!(s.floor_of(19), 9);
    }
}
