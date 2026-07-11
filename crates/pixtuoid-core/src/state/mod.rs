use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

mod correlation;
mod fsm;
pub mod reducer;
mod scope;

pub const MAX_FLOORS: usize = 10;

// serde adapters for the `Arc<str>` / `Arc<Path>` slot fields (#279). serde has
// no blanket `Arc<T>` impl, and its opt-in `rc` feature wouldn't cover
// `Arc<Path>` anyway (no `Box<Path>: Deserialize`), so the snapshot crosses
// through an owned `String` / `PathBuf`. These derives back the full-scene
// regression snapshot (`tests/reducer/snapshot.rs`) today; a future debug
// state dump / daemon snapshot would build on the same shape. That shape is
// NOT a stable wire contract — a new field is free to add (the golden just
// flags it for review), not a breaking change.
mod arc_str_serde {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(v: &Arc<str>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<str>, D::Error> {
        Ok(Arc::from(String::deserialize(d)?.as_str()))
    }
}

mod opt_arc_str_serde {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(v: &Option<Arc<str>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_deref().serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<str>>, D::Error> {
        Ok(Option::<String>::deserialize(d)?.map(|s| Arc::from(s.as_str())))
    }
}

mod arc_path_serde {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(v: &Arc<Path>, s: S) -> Result<S::Ok, S::Error> {
        let p: &Path = v;
        p.serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Path>, D::Error> {
        Ok(Arc::from(PathBuf::deserialize(d)?.as_path()))
    }
}

/// Global desk index — the reducer's allocation space across ALL floors.
///
/// This is the space `AgentSlot.desk_index` lives in (allocated once by
/// `SceneState::next_free_desk`, never mutated). It is NOT a valid index
/// into a single floor's `SceneLayout::home_desks`; convert through
/// `SceneState::floor_local_desk` (the one legal bridge) first.
///
/// The inner `usize` stays `pub` for construction in tests and for raw
/// arithmetic at documented sites — the safety comes from this type being
/// distinct from `FloorLocalDeskIndex`, not from hiding the integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GlobalDeskIndex(pub usize);

/// Floor-local desk index — indexes a single floor's
/// `SceneLayout::home_desks` (see `SceneLayout::home_desk`).
///
/// Produced by `SceneState::floor_local_desk` (the arithmetic bridge) or —
/// inside a single-floor projected scene — by
/// `GlobalDeskIndex::single_floor_local` (a documented identity).
///
/// Deliberately NOT `Serialize` (its twin `GlobalDeskIndex` is): this is a
/// transient bridge value, never a stored `SceneState` field — only
/// `GlobalDeskIndex` (`AgentSlot.desk_index`) is reachable from the
/// serialized tree, so deriving serde here would widen the surface for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FloorLocalDeskIndex(pub usize);

impl GlobalDeskIndex {
    /// The floor-local view of this index **within a single-floor scene**.
    ///
    /// Valid only for slots in a per-floor projection (the output of
    /// `project_floor_scene` in pixtuoid-scene — whose `build_floor_scene`
    /// carries the local desk as a typed `FloorLocalDeskIndex` pair until the
    /// re-host — or any
    /// `uniform(cap)` scene standing in for one floor): there the scene's
    /// global space coincides with its floor-0 local space
    /// (`floor_of(g) == 0`, `floor_local_desk(g).0 == g.0`), so this cast
    /// is the identity by construction. For a multi-floor scene go through
    /// `SceneState::floor_local_desk` — the arithmetic bridge — instead.
    pub fn single_floor_local(self) -> FloorLocalDeskIndex {
        FloorLocalDeskIndex(self.0)
    }
}

/// Semantic category of the tool an `Active` slot is running, carried IN the
/// slot so downstream deciders — the reducer's stale-window policy and the
/// pixel painter's monitor-glow tint — match on a typed kind instead of
/// re-parsing the human-facing `detail` string. Derived ONCE at slot entry
/// ([`ToolKind::from_detail`]); `detail` stays purely HUD text.
///
/// The variant set is exactly what those two deciders discriminate today
/// (the glow's five color buckets + the policy's Task test) — don't add
/// variants nothing matches on. Deliberately NOT `#[non_exhaustive]`: the
/// painter's glow map matches every variant, so adding a kind is a compile
/// error there — the new category must consciously pick a glow color rather
/// than silently falling into a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    /// Subagent dispatch (`ToolDetail::Task` — CC `Agent`/legacy `Task`,
    /// displayed "Delegating"). Only the TYPED detail maps here; a Generic
    /// display spelling one of those words does not (see `from_display`).
    Task,
    /// Edit / Write / MultiEdit.
    Edit,
    Read,
    Bash,
    /// Grep / Glob.
    Search,
    /// Anything else, including a detail-less Active.
    Other,
}

impl ToolKind {
    /// The one production derivation, run by the reducer at slot entry.
    pub fn from_detail(detail: &crate::source::ToolDetail) -> Self {
        match detail {
            crate::source::ToolDetail::Task => ToolKind::Task,
            crate::source::ToolDetail::Generic { display } => Self::from_display(display),
        }
    }

    /// The Generic-display half: first alphanumeric token → kind (the same
    /// token split the glow tint used to parse per frame). Deliberately has
    /// NO `"Agent" | "Task" | "Delegating"` arm — delegation is a semantic
    /// fact carried by the typed `ToolDetail::Task`, and a Generic tool whose
    /// display merely spells those words must not inherit delegation policy
    /// (the stale-window carve-out). Production dispatch tools are typed
    /// upstream (`decoder::make_tool_detail`), so no real display loses its
    /// glow to this.
    pub fn from_display(display: &str) -> Self {
        match display
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("")
        {
            "Edit" | "Write" | "MultiEdit" => ToolKind::Edit,
            "Read" => ToolKind::Read,
            "Bash" => ToolKind::Bash,
            "Grep" | "Glob" => ToolKind::Search,
            _ => ToolKind::Other,
        }
    }
}

/// `AgentSlot` strings (label, source, session_id) and paths (cwd) are
/// stored as `Arc<str>` / `Arc<Path>` so `SceneState::clone()` is a series
/// of pointer copies instead of heap allocations. At 30 fps with N agents
/// this turns ~5N allocations/frame into 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityState {
    Idle,
    Active {
        #[serde(with = "opt_arc_str_serde")]
        tool_use_id: Option<Arc<str>>,
        #[serde(with = "opt_arc_str_serde")]
        detail: Option<Arc<str>>,
        kind: ToolKind,
    },
    Waiting {
        #[serde(with = "arc_str_serde")]
        reason: Arc<str>,
    },
}

/// How an [`AgentSlot`]'s display label came to be — recorded at mint time so
/// the #221 blank-registration→back-fill state machine no longer rests on
/// string-shape sniffing (the old `is_fallback_label`). One variant per REAL
/// mint site:
///
/// - [`OrdinalGhost`](LabelProvenance::OrdinalGhost) — `register_slot`'s
///   no-cwd arm: the monotonic `{prefix}#N` ordinal (bare `#N` when the
///   hook-synthesis pre-pass registered under an empty source).
/// - [`PrefixFallback`](LabelProvenance::PrefixFallback) — the `Rename` arm,
///   when the incoming label is exactly the slot source's registry prefix (a
///   JSONL `LabelDeriver`'s empty-cwd fallback, e.g. bare `cx`).
/// - [`CwdDerived`](LabelProvenance::CwdDerived) — `register_slot`'s
///   cwd-basename arm and the duplicate-`SessionStart` back-fill upgrade
///   (`cc·repo`-style).
/// - [`Renamed`](LabelProvenance::Renamed) — the `Rename` arm otherwise: an
///   externally supplied display name (CC `attributionAgent` subagent names,
///   the JSONL derivers' cwd-derived renames).
///
/// The first two carry no information worth preserving
/// ([`SlotLabel::is_upgradable`]); the last two are real information and are
/// never clobbered by a back-fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelProvenance {
    OrdinalGhost,
    PrefixFallback,
    CwdDerived,
    Renamed,
}

/// An [`AgentSlot`]'s display label + the provenance it was minted with.
/// Derefs to `str` so read sites treat it as the label text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotLabel {
    #[serde(with = "arc_str_serde")]
    text: Arc<str>,
    provenance: LabelProvenance,
}

impl SlotLabel {
    pub fn new(text: impl Into<Arc<str>>, provenance: LabelProvenance) -> Self {
        Self {
            text: text.into(),
            provenance,
        }
    }

    pub fn ordinal_ghost(text: impl Into<Arc<str>>) -> Self {
        Self::new(text, LabelProvenance::OrdinalGhost)
    }

    pub fn prefix_fallback(text: impl Into<Arc<str>>) -> Self {
        Self::new(text, LabelProvenance::PrefixFallback)
    }

    pub fn cwd_derived(text: impl Into<Arc<str>>) -> Self {
        Self::new(text, LabelProvenance::CwdDerived)
    }

    pub fn renamed(text: impl Into<Arc<str>>) -> Self {
        Self::new(text, LabelProvenance::Renamed)
    }

    /// The label text as a shared handle (for cheap clones into UI rows).
    pub fn text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    pub fn provenance(&self) -> LabelProvenance {
        self.provenance
    }

    /// Whether the duplicate-`SessionStart` back-fill may upgrade this label:
    /// only a derivation fallback (ordinal ghost / bare-prefix) carries no
    /// information; a cwd-basename- or Rename-derived label is real
    /// information and is never clobbered.
    pub fn is_upgradable(&self) -> bool {
        matches!(
            self.provenance,
            LabelProvenance::OrdinalGhost | LabelProvenance::PrefixFallback
        )
    }
}

impl std::ops::Deref for SlotLabel {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl AsRef<str> for SlotLabel {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for SlotLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Test-fixture convenience: a plain string reads as an externally supplied
/// (non-upgradable) display name. The reducer's mint sites use the explicit
/// constructors — reach for those when provenance matters.
impl From<&str> for SlotLabel {
    fn from(text: &str) -> Self {
        Self::renamed(text)
    }
}

impl From<String> for SlotLabel {
    fn from(text: String) -> Self {
        Self::renamed(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSlot {
    pub agent_id: AgentId,
    #[serde(with = "arc_str_serde")]
    pub source: Arc<str>,
    #[serde(with = "arc_str_serde")]
    pub session_id: Arc<str>,
    #[serde(with = "arc_path_serde")]
    pub cwd: Arc<Path>,
    pub label: SlotLabel,
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
    /// GLOBAL desk index (assigned once at `SessionStart`, never mutated).
    /// The `GlobalDeskIndex` newtype encodes the index space — see its docs
    /// for the bridge to a floor's `home_desks`. `floor_idx` derives from it
    /// via `floor_of()`.
    pub desk_index: GlobalDeskIndex,
    /// Floor assigned at desk allocation time. Immutable for the agent's
    /// lifetime so capacity growth never silently migrates agents between
    /// floors.
    pub floor_idx: usize,
    pub tool_call_count: u32,
    pub active_ms: u64,
    pub unknown_cwd: bool,
    pub parent_id: Option<AgentId>,
    /// The agent process's pid + recycle marker — the focus-jump channel for
    /// hook-only sources (filled from the shim/plugin `_pid` riding each
    /// hook-transport `Identity`; refreshed per event, never downgraded to
    /// `None`). The click-time guard re-reads the marker and refuses a
    /// recycled pid (#527). Transcript-family sources stay `None` here —
    /// their pid channel is the liveness probe, queried at click time.
    /// serde-skipped so the scene serialization golden doesn't churn on
    /// `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pid: Option<crate::source::PidIdentity>,
    /// The RAW model string last observed on this agent's wire (CC assistant
    /// lines / Codex turn_context / copilot per-tool / opencode
    /// session.created / omp assistant messages) — last-seen-wins, so a
    /// mid-session `/model` switch tracks. Interpretation (the burn-tier
    /// tables) lives in the scene layer; this stays uninterpreted wire truth.
    /// serde-skipped (goldens quiet).
    #[serde(
        with = "opt_arc_str_serde",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub model: Option<Arc<str>>,
    /// The RAW effort observation last seen (Codex per-turn `effort`
    /// verbatim; CC's periodic ultra-marker as a synthesized label). One
    /// freshness semantic for both cadences: the scene layer treats the value
    /// as live only within its TTL — no sighting means the boost decays,
    /// which is honest (an idle agent isn't burning). serde-skipped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortObservation>,
}

/// A RAW effort string + WHEN it was last observed — the freshness the scene
/// layer's burn-tier TTL reads (see `AgentSlot::effort`). `non_exhaustive`
/// like `PidIdentity`: a future field (e.g. the observing source) lands
/// non-breaking; cross-crate construction via [`EffortObservation::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EffortObservation {
    #[serde(with = "arc_str_serde")]
    pub value: Arc<str>,
    pub seen_at: SystemTime,
}

impl EffortObservation {
    pub fn new(value: Arc<str>, seen_at: SystemTime) -> Self {
        Self { value, seen_at }
    }
}

/// Liveness of a daemon-style source (the OpenClaw gateway). Drives the
/// wandering lobster mascot's behaviour (idle ambles, busy shuttles, down
/// walks out). A daemon is NOT an `AgentSlot` (it has no desk / no agent
/// activity), so its presence lives in `SceneState::daemons`, read
/// directly by the geometry pass. `Down` is distinct from *absent* (no map
/// entry): absent = not configured / plugin not loaded (the lobster not on the
/// floor); `Down` = the daemon was seen and then died (the lobster walks out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonState {
    Idle,
    Busy,
    /// Gateway is UP but its model backend is failing every run (#317) — the
    /// Apr-2026 Anthropic-ban failure mode: `gateway_start`/`session_start`/
    /// `before_agent_run` all fire normally, but each `agent_end` reports
    /// `success: false`, so the daemon is alive-but-broken, NOT idle. Entered on
    /// a failed run; self-heals on the next successful run (or a new run start /
    /// gateway restart). The mascot renders distressed (sickly red, sluggish).
    Degraded,
    Down,
}

/// The two ORTHOGONAL liveness axes a daemon mascot actually STORES, so that
/// "busy" can never drift from the run set (#460). `Up { degraded }` is the
/// alive gateway (healthy, or `degraded` = its model backend is failing every
/// run, #317); `Down` is seen-then-died. The remaining render distinction —
/// Idle vs Busy — is deliberately NOT a field here: it is a pure function of
/// [`DaemonPresence::in_flight_run_keys`], projected by
/// [`DaemonPresence::display_state`]. Storing `Busy`/`Idle` separately was the
/// hand-synced duplication this split removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonLiveness {
    /// The gateway is alive. `degraded` (#317): alive-but-broken (auth revoked /
    /// provider down), rendered distressed; healed by the next clean run / new
    /// attempt / restart.
    Up { degraded: bool },
    /// The gateway was seen and then died (the mascot walks out). Distinct from
    /// *absent* (no map entry = never configured / plugin not loaded).
    Down,
}

impl DaemonLiveness {
    /// The healthy alive state (`Up { degraded: false }`) — the common case,
    /// named once so construction sites don't repeat the struct literal.
    pub const UP: DaemonLiveness = DaemonLiveness::Up { degraded: false };
}

/// Per-daemon presence for the gateway mascot (the P-A representation): lives on
/// `SceneState` so the serializable scene snapshot the renderer reads carries
/// the mascot's state + concurrency (bubble) intensity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonPresence {
    /// The stored liveness axes ([`DaemonLiveness`]). The 4-way render state
    /// (incl. Idle/Busy) is PROJECTED via [`display_state`](Self::display_state),
    /// never stored — Busy is derived from `in_flight_run_keys`.
    pub liveness: DaemonLiveness,
    /// Concurrent sessions the gateway is multiplexing (bubble intensity).
    pub active_sessions: u32,
    /// Last time ANY presence event arrived — drives the busy→idle decay and
    /// the presence-TTL stale-down sweep (a daemon has no per-session pid).
    /// Also the leave-animation anchor: when `state == Down`, this is the
    /// moment the gateway died, so the mascot's walk-to-the-elevator exit is
    /// timed `now − last_seen`.
    pub last_seen: SystemTime,
    /// When the gateway first appeared (absent/Down → up). Anchors the
    /// mascot's enter animation (walk in from the elevator) and is the steady
    /// wander clock — process-local timing only, like `AgentSlot.state_started_at`.
    pub entered_at: SystemTime,
    /// In-flight run keys (busy iff non-empty). Transient process state: a
    /// daemon restart resets it and a dropped `agent_end` self-heals via the
    /// TTL decay, so it is NOT serialized (a restored dump must not strand a
    /// perpetual Busy).
    #[serde(skip)]
    pub in_flight_run_keys: std::collections::HashSet<String>,
    /// The gateway pid currently armed for `ExitWatch` (None until first seen).
    /// Kept for debug dumps + the restart pid-rebind guard; not a wire contract.
    pub current_pid: Option<i32>,
}

impl DaemonPresence {
    /// The 4-way render vocabulary ([`DaemonState`]) projected from the stored
    /// axes — the SINGLE place the `Degraded > Busy > Idle` priority is encoded.
    /// Every renderer reads this instead of a stored `state` field, so Busy can't
    /// drift from the run set: a `Degraded` gateway renders Degraded even with
    /// runs still in flight (the fan-out-with-one-failure case), because
    /// `degraded` is checked BEFORE the run set.
    pub fn display_state(&self) -> DaemonState {
        match self.liveness {
            DaemonLiveness::Down => DaemonState::Down,
            DaemonLiveness::Up { degraded: true } => DaemonState::Degraded,
            DaemonLiveness::Up { degraded: false } => {
                if self.in_flight_run_keys.is_empty() {
                    DaemonState::Idle
                } else {
                    DaemonState::Busy
                }
            }
        }
    }

    /// Whether the mascot renders as Busy (alive, not degraded, ≥1 run in flight).
    /// Derived from [`display_state`](Self::display_state) so the Degraded-first
    /// priority has exactly one definition.
    pub fn is_busy(&self) -> bool {
        self.display_state() == DaemonState::Busy
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneState {
    pub agents: BTreeMap<AgentId, AgentSlot>,
    pub floor_capacities: [usize; MAX_FLOORS],
    /// Daemon-style sources (the OpenClaw gateway is instance #1) rendered as
    /// wandering mascots, keyed on the registry source name. Empty for an
    /// all-agent scene. PRIVATE (with `daemons`/`daemons_mut` accessors) on
    /// purpose: a pub mutable `BTreeMap` is a leaky surface — the renderer reads
    /// via the accessor. `pub(crate)` so the reducer/source modules still touch
    /// it directly while the field stays out of the external public API.
    #[serde(default)]
    pub(crate) daemons: BTreeMap<String, DaemonPresence>,
}

impl SceneState {
    /// Daemon-presence map (the gateway mascots) — read access for the renderer.
    pub fn daemons(&self) -> &BTreeMap<String, DaemonPresence> {
        &self.daemons
    }

    /// Mutable daemon-presence map — for the shared `daemon::apply_presence`
    /// merge and the per-floor projection.
    pub fn daemons_mut(&mut self) -> &mut BTreeMap<String, DaemonPresence> {
        &mut self.daemons
    }
}

impl Default for SceneState {
    fn default() -> Self {
        Self::new([0; MAX_FLOORS])
    }
}

impl SceneState {
    pub fn new(floor_capacities: [usize; MAX_FLOORS]) -> Self {
        Self {
            agents: BTreeMap::new(),
            floor_capacities,
            daemons: BTreeMap::new(),
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
    fn floor_of_with_offsets(
        &self,
        desk_index: GlobalDeskIndex,
        offsets: &[usize; MAX_FLOORS],
    ) -> usize {
        for i in (0..MAX_FLOORS).rev() {
            if self.floor_capacities[i] > 0 && desk_index.0 >= offsets[i] {
                return i;
            }
        }
        0
    }

    /// Which floor does `desk_index` belong to?
    pub fn floor_of(&self, desk_index: GlobalDeskIndex) -> usize {
        self.floor_of_with_offsets(desk_index, &self.cumulative_offsets())
    }

    /// Local desk offset within the floor — THE bridge from the reducer's
    /// global allocation space to a floor's `home_desks` index space.
    pub fn floor_local_desk(&self, desk_index: GlobalDeskIndex) -> FloorLocalDeskIndex {
        let offsets = self.cumulative_offsets();
        let floor = self.floor_of_with_offsets(desk_index, &offsets);
        FloorLocalDeskIndex(desk_index.0 - offsets[floor])
    }

    /// Global desk index range `[lo, hi)` for a given floor.
    /// Clamps `floor_idx` to `MAX_FLOORS - 1` to avoid panics.
    /// Stays `Range<usize>` over raw global indices — a `Range` of newtypes
    /// is painful (no `Step` impl) and the consumers only need the offsets.
    pub fn floor_range(&self, floor_idx: usize) -> std::ops::Range<usize> {
        let idx = floor_idx.min(MAX_FLOORS - 1);
        let offsets = self.cumulative_offsets();
        let lo = offsets[idx];
        let hi = lo + self.floor_capacities[idx];
        lo..hi
    }

    /// Lowest free desk index, or `None` if all desks are occupied.
    pub fn next_free_desk(&self) -> Option<GlobalDeskIndex> {
        let occupied: std::collections::BTreeSet<GlobalDeskIndex> =
            self.agents.values().map(|a| a.desk_index).collect();
        (0..self.total_capacity())
            .map(GlobalDeskIndex)
            .find(|i| !occupied.contains(i))
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
            label: "a0".into(),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(desk_index),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
        }
    }

    #[test]
    fn scene_state_json_round_trips_losslessly() {
        // #279: the whole SceneState tree serializes and restores without loss
        // — the basis for debug state dumps and the full-scene regression
        // snapshot. The tree has no PartialEq (deliberate), so round-trip
        // stability is asserted via canonical-JSON equality; the Arc-backed
        // fields (Arc<str> / Arc<Path>, and the Option<Arc<str>> Active
        // variant) are the ones that cross through owned String/PathBuf.
        let mut s = SceneState::uniform(8);

        let a = AgentId::from_transcript_path("/p/a.jsonl");
        let mut slot_a = make_slot(a, 0);
        slot_a.state = ActivityState::Active {
            tool_use_id: Some(Arc::from("tuid-1")),
            detail: Some(Arc::from("Read · src/main.rs")),
            kind: ToolKind::Read,
        };
        s.agents.insert(a, slot_a);

        let b = AgentId::from_transcript_path("/p/b.jsonl");
        let mut slot_b = make_slot(b, 1);
        slot_b.state = ActivityState::Waiting {
            reason: Arc::from("permission: Bash"),
        };
        slot_b.parent_id = Some(a);
        s.agents.insert(b, slot_b);

        // An Idle slot too: Idle is a unit variant today, but pinning it here
        // (and in the golden) catches a future Idle field silently reshaping
        // the wire form from `"Idle"` to `{"Idle": {..}}`.
        let c = AgentId::from_transcript_path("/p/c.jsonl");
        s.agents.insert(c, make_slot(c, 2)); // make_slot defaults to Idle

        let json = serde_json::to_string(&s).expect("serialize");
        let back: SceneState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            json,
            serde_json::to_string(&back).expect("re-serialize"),
            "round-trip must be byte-stable"
        );
        assert_eq!(back.agents.len(), 3);
        assert!(matches!(
            back.agents[&a].state,
            ActivityState::Active { .. }
        ));
        assert_eq!(back.agents[&c].state, ActivityState::Idle);
        assert_eq!(&*back.agents[&a].cwd, Path::new("/repo"));
        assert_eq!(back.agents[&b].parent_id, Some(a));
    }

    #[test]
    fn daemon_presence_round_trips_and_skips_in_flight_keys() {
        // The openclaw daemon-presence (mascot) lives on SceneState (P-A) so the
        // geometry pass can read it. It serializes like the rest of the tree
        // (#279). `in_flight_run_keys` is transient process state — a daemon
        // restart resets it — so it is `#[serde(skip)]` and restores empty.
        // Consequence of the #460 split: since Busy is DERIVED from the run set
        // (never a serialized field), a restored dump with a drained run set reads
        // Idle, NOT Busy — the correct fix for the old "restore strands a
        // perpetual Busy" drift (this test used to assert the stranded Busy).
        let p = DaemonPresence {
            liveness: DaemonLiveness::UP,
            active_sessions: 3,
            last_seen: SystemTime::now(),
            entered_at: SystemTime::now(),
            in_flight_run_keys: ["run-1".to_string(), "run-2".to_string()]
                .into_iter()
                .collect(),
            current_pid: Some(4242),
        };
        assert_eq!(
            p.display_state(),
            DaemonState::Busy,
            "a non-empty run set reads Busy before serialization"
        );
        let json = serde_json::to_string(&p).expect("serialize");
        assert!(
            !json.contains("run-1"),
            "in_flight_run_keys must be skipped on the wire: {json}"
        );
        let back: DaemonPresence = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.liveness, DaemonLiveness::UP);
        assert_eq!(
            back.display_state(),
            DaemonState::Idle,
            "skipped run set restores empty ⇒ Idle, never a stranded Busy"
        );
        assert_eq!(back.active_sessions, 3);
        assert_eq!(back.current_pid, Some(4242));
        assert!(
            back.in_flight_run_keys.is_empty(),
            "skipped field restores empty"
        );

        // Every liveness value round-trips (it IS the serialized axis).
        let mut q = back;
        for liveness in [
            DaemonLiveness::UP,
            DaemonLiveness::Up { degraded: true },
            DaemonLiveness::Down,
        ] {
            q.liveness = liveness;
            let j = serde_json::to_string(&q).unwrap();
            assert_eq!(
                serde_json::from_str::<DaemonPresence>(&j).unwrap().liveness,
                liveness
            );
        }
    }

    #[test]
    fn scene_state_daemons_round_trips() {
        // A SceneState carrying an openclaw daemon-presence entry round-trips
        // byte-stably alongside the agents tree.
        let mut s = SceneState::uniform(8);
        s.daemons.insert(
            "openclaw".to_string(),
            DaemonPresence {
                liveness: DaemonLiveness::UP,
                active_sessions: 0,
                last_seen: SystemTime::now(),
                entered_at: SystemTime::now(),
                in_flight_run_keys: Default::default(),
                current_pid: Some(900),
            },
        );
        let json = serde_json::to_string(&s).expect("serialize");
        let back: SceneState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            json,
            serde_json::to_string(&back).expect("re-serialize"),
            "round-trip must be byte-stable"
        );
        assert_eq!(back.daemons["openclaw"].liveness, DaemonLiveness::UP);
        assert_eq!(back.daemons["openclaw"].current_pid, Some(900));
    }

    fn presence_at(liveness: DaemonLiveness) -> DaemonPresence {
        DaemonPresence {
            liveness,
            active_sessions: 0,
            last_seen: SystemTime::UNIX_EPOCH,
            entered_at: SystemTime::UNIX_EPOCH,
            in_flight_run_keys: Default::default(),
            current_pid: None,
        }
    }

    #[test]
    fn display_state_derives_busy_from_the_run_set_not_a_stored_flag() {
        // Busy is a pure function of `in_flight_run_keys`, never a separately
        // stored field that could drift from the set (the #460 invariant).
        let mut p = presence_at(DaemonLiveness::Up { degraded: false });
        assert_eq!(p.display_state(), DaemonState::Idle);
        assert!(!p.is_busy());
        p.in_flight_run_keys.insert("r".into());
        assert_eq!(p.display_state(), DaemonState::Busy);
        assert!(p.is_busy());
        p.in_flight_run_keys.clear();
        assert_eq!(p.display_state(), DaemonState::Idle, "drained ⇒ Idle");
    }

    #[test]
    fn display_state_degraded_wins_over_busy_with_a_run_still_in_flight() {
        // The reachable fan-out-with-one-failure case: one run FAILED (degraded)
        // while ANOTHER is still in flight. Degraded must render over Busy — this
        // is exactly why the projection checks `degraded` BEFORE the run set (a
        // naive `Up && !empty ⇒ Busy` would regress it).
        let mut p = presence_at(DaemonLiveness::Up { degraded: true });
        p.in_flight_run_keys.insert("still-running".into());
        assert_eq!(p.display_state(), DaemonState::Degraded);
        assert!(!p.is_busy(), "a degraded daemon never reads as Busy");
    }

    #[test]
    fn display_state_down_wins_over_a_stray_run_key() {
        // Down is terminal for the projection; a defensively non-empty run set
        // (enter_down clears it, but the type permits it) can't read as Busy.
        let mut p = presence_at(DaemonLiveness::Down);
        p.in_flight_run_keys.insert("stray".into());
        assert_eq!(p.display_state(), DaemonState::Down);
        assert!(!p.is_busy());
    }

    #[test]
    fn single_floor_local_is_the_identity_cast() {
        // The documented coincidence: in a uniform(cap) scene standing in for
        // ONE floor, the global space == the floor-0 local space, so the
        // typed identity cast agrees with the arithmetic bridge.
        let g = GlobalDeskIndex(7);
        assert_eq!(g.single_floor_local(), FloorLocalDeskIndex(7));
    }

    #[test]
    fn next_free_desk_starts_at_zero() {
        let s = SceneState::uniform(4);
        assert_eq!(s.next_free_desk(), Some(GlobalDeskIndex(0)));
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
            Some(GlobalDeskIndex(4)),
            "should overflow to desk 4 (floor 1)"
        );
    }

    #[test]
    fn floor_of_uniform() {
        let s = SceneState::uniform(8);
        assert_eq!(s.floor_of(GlobalDeskIndex(0)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(7)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(8)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(15)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(16)), 2);
    }

    #[test]
    fn floor_of_variable_capacities() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        // F0: 0..4, F1: 4..12, F2: 12..18, F3: 18..22, F4: 22..24
        assert_eq!(s.floor_of(GlobalDeskIndex(0)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(3)), 0);
        assert_eq!(s.floor_of(GlobalDeskIndex(4)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(11)), 1);
        assert_eq!(s.floor_of(GlobalDeskIndex(12)), 2);
        assert_eq!(s.floor_of(GlobalDeskIndex(17)), 2);
        assert_eq!(s.floor_of(GlobalDeskIndex(18)), 3);
        assert_eq!(s.floor_of(GlobalDeskIndex(22)), 4);
        assert_eq!(s.floor_of(GlobalDeskIndex(23)), 4);
    }

    #[test]
    fn floor_local_desk_variable() {
        let s = SceneState::new([4, 8, 6, 4, 2, 0, 0, 0, 0, 0]);
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(0)),
            FloorLocalDeskIndex(0)
        );
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(3)),
            FloorLocalDeskIndex(3)
        );
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(4)),
            FloorLocalDeskIndex(0)
        ); // first desk on F1
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(11)),
            FloorLocalDeskIndex(7)
        ); // last desk on F1
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(12)),
            FloorLocalDeskIndex(0)
        ); // first desk on F2
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
        assert_eq!(s.next_free_desk(), Some(GlobalDeskIndex(4)));
    }

    #[test]
    fn zero_capacity_floor_skipped_by_next_free_desk() {
        let s = SceneState::new([4, 0, 6, 0, 2, 0, 0, 0, 0, 0]);
        // F0: 0..4, F1: 4..4 (empty), F2: 4..10, F3: 10..10, F4: 10..12
        assert_eq!(s.total_capacity(), 12);
        assert_eq!(s.floor_range(0), 0..4);
        assert_eq!(s.floor_range(1), 4..4);
        assert_eq!(s.floor_range(2), 4..10);
        assert_eq!(s.next_free_desk(), Some(GlobalDeskIndex(0)));
    }

    #[test]
    fn floor_of_skips_zero_capacity_floors() {
        let s = SceneState::new([4, 0, 6, 0, 2, 0, 0, 0, 0, 0]);
        // Desk 4 is first desk of F2 (F1 has zero capacity)
        assert_eq!(s.floor_of(GlobalDeskIndex(4)), 2);
        assert_eq!(
            s.floor_local_desk(GlobalDeskIndex(4)),
            FloorLocalDeskIndex(0)
        );
        assert_eq!(s.floor_of(GlobalDeskIndex(9)), 2);
        assert_eq!(s.floor_of(GlobalDeskIndex(10)), 4);
    }

    #[test]
    fn floor_of_leading_zero_capacity_floors() {
        let s = SceneState::new([0, 0, 6, 4, 2, 0, 0, 0, 0, 0]);
        // F0 and F1 have zero capacity, desk 0 belongs to F2
        assert_eq!(s.floor_of(GlobalDeskIndex(0)), 2);
        assert_eq!(s.floor_of(GlobalDeskIndex(5)), 2);
        assert_eq!(s.floor_of(GlobalDeskIndex(6)), 3);
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
        let floor = s.floor_of(GlobalDeskIndex(oob));
        assert_eq!(floor, 4, "OOB desk lands on last nonempty floor");
        let local = s.floor_local_desk(GlobalDeskIndex(oob));
        // offsets[4] = 22, so local = 100 - 22 = 78
        assert_eq!(local, FloorLocalDeskIndex(oob - 22));
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
            s.floor_of(GlobalDeskIndex(18)),
            9,
            "desk 18 is the first seat on the tenth floor"
        );
        assert_eq!(s.floor_of(GlobalDeskIndex(19)), 9);
    }
}
