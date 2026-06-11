//! The reducer's cross-slot correlation maps, extracted into one struct.
//!
//! INTRA-LAYER bookkeeping extraction, not a new layer: the layering rule
//! (Layer A `fsm.rs` mutates ONE slot; Layer B `scope.rs` owns the
//! parent↔subagent tree; cross-slot correlation lives in the REDUCER's layer)
//! is satisfied — [`Correlation`] is owned by and consulted only from
//! `Reducer`, the DECISIONS (which arm fires, what gets suppressed, when a
//! sweep cascades) stay in `reducer.rs`, and this module owns the seven maps'
//! bookkeeping: the entry types, the TTL constants, the freshness predicates,
//! and the one [`Correlation::gc`] pruning entry point. Don't move these maps
//! onto `AgentSlot` (they span slots and are deliberately not a semver
//! surface — see `state/mod.rs`'s "Where to look").

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::AgentId;

/// Window in which a Hook event suppresses a later Jsonl event with the same
/// tool_use_id. The suppression is asymmetric by event kind — a recorded hook
/// End drops both JSONL kinds, a recorded hook Start drops only JSONL Starts
/// (see `ToolEventKind`, #150).
///
// These reducer tuning constants are `pub` ONLY so the integration test
// (the `tests/reducer/` binary, a separate crate) can derive its timing offsets from
// them instead of hardcoding ms. They are internal knobs, not a stable API:
// `#[doc(hidden)]` keeps the cross-crate visibility the test needs while
// excluding them from the rendered docs AND from cargo-semver-checks, so a
// future retune/rename is not a breaking change. (`EXIT_GRACE_WINDOW` in
// `reducer.rs` is deliberately NOT hidden — the binary's pose module is a real
// consumer.)
#[doc(hidden)]
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

/// How long a hook `SessionEnd` for an UNKNOWN id suppresses hook-synthesis
/// for that id ([`Reducer::synthesize_hook_registration`]) AND child
/// (`parent_id`-carrying) `SessionStart` registration (#242, either
/// transport). Hook connections are per-connection spawned tasks, so a
/// session's SessionEnd and a trailing Stop/ActivityEnd can be DELIVERED
/// reordered — for an invisible (never-registered) session ending at /exit,
/// the reordered straggler would otherwise synthesize a blank Idle ghost with
/// NO SessionEnd left to ever remove it (it lives out the full 30-min idle
/// sweep); a short-lived subagent's SubagentStart decoded after its own
/// SubagentStop would likewise register a phantom child. 5s is generous next
/// to [`HOOK_WINS_WINDOW`]'s modeled transport skew — reordering here is
/// same-machine task-scheduling jitter, so the headroom costs nothing —
/// while short enough that a genuinely revived session on the same id is
/// never visibly delayed.
#[doc(hidden)]
pub const HOOK_SESSION_END_TOMBSTONE_TTL: Duration = Duration::from_secs(5);

/// How long a child-ledger entry's `ended_at` keeps gating a PARENTED
/// re-registration of that child after it ended ([`Reducer::apply`]'s
/// `SessionStart` arm, #244). The #242 hook tombstone above covers only the
/// 5s reorder window for UNKNOWN-id ends; this covers the residual windows it
/// can't: a child that ended on a KNOWN slot (no tombstone minted) whose
/// transcript first-sight arrives LATE — a notify outage defers discovery to
/// the watcher's 60s poll backstop, well past both the 5s tombstone and the
/// 4.5s [`EXIT_GRACE_WINDOW`] GC. Sized like
/// [`DRAINED_TASK_TOMBSTONE_TTL`]: past the 60s poll plus slack, and
/// generosity costs nothing — child ids are per-spawn unique, so a parented
/// Start inside the window is never a legitimate new child, only the dead
/// one's late echo. (Parentless Starts are deliberately NOT gated: a Codex
/// resurrect-on-prompt is a legitimate same-id new life — they RE-LINK via
/// the ledger's remembered parent instead, #246.)
#[doc(hidden)]
pub const CHILD_END_LEDGER_TTL: Duration = Duration::from_secs(90);

/// How long a drained Task `tool_use_id` is remembered so a lagged JSONL
/// replay of its Start cannot re-fire `enter_delegating`. A fast Task (both
/// hooks delivered) drains `active_tasks` and its hook-End dedup record is
/// GC'd at [`HOOK_WINS_WINDOW`] — the transcript's batched Start+End pair
/// then replays into an EMPTY set, so the first-insert gate reads it as a
/// fresh dispatch and would clobber a Waiting the parent raised in the gap
/// (then settle the still-pending prompt to Idle via the replayed End).
/// Sized past the 60s `scan_root` poll backstop (the worst-case replay path
/// when notify drops the dispatch's write) plus slack; unlike
/// [`B1_CASCADE_GRACE`] generosity costs nothing here — a `tool_use_id` is
/// never legitimately re-dispatched, so the only cost is the tombstone's
/// map entry.
#[doc(hidden)]
pub const DRAINED_TASK_TOMBSTONE_TTL: Duration = Duration::from_secs(90);

/// How long an [`AgentEvent::ProofOfLife`] vouch exempts its slot from the
/// staleness sweeps (#220). The probe is ground truth that the OWNING PROCESS
/// is alive, while every `STALE_*` window above only models event silence — so
/// a vouched slot must not be swept on silence alone (the motivating case: a
/// probe-vouched CC session parked on a permission prompt renders Active after
/// attach-replay — its hook-only Waiting state is unreconstructable from JSONL
/// — and 10 min of silence is normal while the human decides). Sized 2.5× the
/// watcher's 60s poll cadence: two missed polls plus slack. When the live
/// signal disappears (registry entry removed / rollout fd closed) the
/// emissions stop and the normal sweeps resume after this lapse.
#[doc(hidden)]
pub const PROOF_OF_LIFE_TTL: Duration = Duration::from_secs(150);

/// One child's remembered lifecycle in [`Correlation::child_ledger`]. `Default`
/// is the "as_child end for a never-registered child" shape: parent unknown,
/// not yet ended (the end-site sets `ended_at` right after the upsert).
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChildLedgerEntry {
    /// The last APPLIED parent link — `None` when the child was only ever
    /// seen ending (the Stop-before-Start reorder blocked its Start, so no
    /// link was ever applied; an accepted residual: its later flat
    /// first-sight registers parentless, bounded by the sweeps).
    pub(super) parent_id: Option<AgentId>,
    /// When the child ended (`as_child` SessionEnd) or its slot was removed,
    /// whichever came first; `None` while a registered life is still alive.
    /// Starts the [`CHILD_END_LEDGER_TTL`] GC clock AND the #244-w2 gate.
    pub(super) ended_at: Option<SystemTime>,
}

/// Kind half of a hook-wins dedup record. Lives in the map VALUE (a hook End
/// overwrites its tool's Start entry) and drives the asymmetric drop matrix
/// (#150): an End record suppresses BOTH JSONL kinds — the tool is over, so a
/// lagged JSONL Start replay would falsely re-Activate and cancel the armed
/// idle debounce — while a Start record suppresses only Starts. A JSONL End
/// must never be eaten by its own tool's dispatch record: when the
/// PostToolUse hook drops (the shim is best-effort), that JSONL End is the
/// only completion signal left, and a Task self-End that gets eaten leaks
/// `active_tasks` for the rest of the session (suppression stuck on, b1
/// cascade disabled). Don't "simplify" this to exact-kind matching either —
/// it would orphan the lagged-pair case the End-dominates rule covers
/// (pinned by `late_batched_jsonl_pair_after_delivered_hook_end_is_fully_dropped`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ToolEventKind {
    Start,
    End,
}

/// The seven reducer-private correlation maps (see the module doc). Fields
/// are `pub(super)` on purpose: the reducer's arms keep their map-touching
/// logic verbatim (`self.corr.<map>`), while the named predicates/pruning
/// below carry the helper logic that is purely map-shaped.
///
/// In/out criterion for a future map: PASSIVE cross-event memory (consulted
/// to interpret a later event) lives here; ARMED actions that mutate the
/// scene on a schedule (`pending_b1_cascades` and its fire pass) stay on
/// `Reducer`.
#[derive(Debug, Default)]
pub(super) struct Correlation {
    /// Track recent hook-derived events so JSONL duplicates can be dropped.
    /// The value carries the recorded event's kind: the drop matrix is
    /// asymmetric — see [`ToolEventKind`]. A hook End overwrites its tool's
    /// Start entry (kind-in-the-VALUE, not the key), which is what lets one
    /// End record cover the tool's whole lagged JSONL pair.
    pub(super) recent_hook_tool_uses: HashMap<(AgentId, String), (SystemTime, ToolEventKind)>,
    /// Short-TTL tombstones for hook `SessionEnd`s that arrived for an id
    /// with NO slot — an invisible (unregistered) session ending. A reordered
    /// trailing hook event for a tombstoned id must not re-synthesize the
    /// session, and a reordered CHILD `SessionStart` (`parent_id`-carrying —
    /// a SubagentStart decoded after its own SubagentStop, #242; gated on
    /// BOTH transports) must not register it (see
    /// [`HOOK_SESSION_END_TOMBSTONE_TTL`]). Mirrors `recent_hook_tool_uses`,
    /// including its `gc` tick-time pruning.
    pub(super) recent_hook_session_ends: HashMap<AgentId, SystemTime>,
    /// Per-agent set of Task tool_use_ids currently in flight. CC's hook
    /// payload sets `transcript_path` to the PARENT'S transcript even when a
    /// subagent is the actor, so subagent hook events hash to the parent's
    /// AgentId. While the parent has any Task in flight, hook
    /// ActivityStart/End events for that AgentId are dropped — JSONL has
    /// correct attribution to the subagent's own AgentId.
    pub(super) active_tasks: HashMap<AgentId, HashSet<String>>,
    /// Tombstones for Task tool_use_ids whose drain already completed: a
    /// lagged JSONL pair-replay after the drain re-inserts the tuid as a
    /// FRESH first insert (set empty, dedup record GC'd), so the tracker's
    /// first-insert gate alone can't stop `enter_delegating` from clobbering
    /// a Waiting raised since. Consulted by the Start arm; TTL-pruned by
    /// `gc` ([`DRAINED_TASK_TOMBSTONE_TTL`]) like the hook-recency maps —
    /// not per-slot state, a tuid can't recur across lives.
    pub(super) recent_task_drains: HashMap<(AgentId, String), SystemTime>,
    /// Memory of CHILD (subagent) lifecycles, surviving the slots themselves
    /// (#244/#246). Keyed by the child's id; `parent_id` is upserted whenever
    /// a parent link is APPLIED (registration or orphan-enrichment — never a
    /// cycle-refused or tombstone-blocked one), `ended_at` is stamped by an
    /// `as_child` SessionEnd (a SubagentStop decode — regardless of slot
    /// existence, covering the Stop-before-Start reorder) and by
    /// `sweep_exited` removing the child's slot (so a stale-swept/cascaded
    /// child starts the GC clock too and the map stays bounded; a new life's
    /// link-upsert clears it). Consumed by the `SessionStart` arm: a fresh
    /// `ended_at` gates a PARENTED re-registration (the dead child's late
    /// echo, #244-w2), while a PARENTLESS start ADOPTS the remembered parent
    /// (a post-un-claim revival start re-links — #246's adoption seam; a
    /// tombstoned child's flat first-sight registers parent-linked, #244-w1).
    /// Deliberately reducer-private like `recent_proof_of_life` — not an
    /// `AgentSlot` field, no semver surface; pruned by `gc` on
    /// [`CHILD_END_LEDGER_TTL`] once ended.
    pub(super) child_ledger: HashMap<AgentId, ChildLedgerEntry>,
    /// Sweep-exemption timestamps from [`AgentEvent::ProofOfLife`] (#220):
    /// a slot vouched for within [`PROOF_OF_LIFE_TTL`] is skipped by
    /// `sweep_stale`'s candidate collection. Deliberately reducer-private
    /// state, NOT a field on the public `AgentSlot` (no semver surface
    /// change); pruned by `gc` on TTL like its hook-recency siblings and
    /// evicted with the slot in `sweep_exited`.
    pub(super) recent_proof_of_life: HashMap<AgentId, SystemTime>,
    /// `tool_use_id` that was Active immediately before an agent entered
    /// `Waiting` (a CC permission `Notification` fires mid-tool). When THAT
    /// tool's `ActivityEnd` (its `PostToolUse`) arrives, the permission has been
    /// resolved and the gated tool ran — so the Waiting resolves (debounced to
    /// Idle) instead of lingering until the agent's next tool. A *parallel*
    /// tool ending carries a different id, so it can't false-clear a still-
    /// pending permission (preserves `parallel_tool_end_while_waiting_keeps_waiting`).
    /// Codex never populates this (its tool events carry no `tool_use_id`), so
    /// its permission resume stays on the `ActivityStart` path.
    pub(super) gated_before_waiting: HashMap<AgentId, Arc<str>>,
}

impl Correlation {
    /// Whether a hook `SessionEnd` for `id` (which had no slot) is still inside
    /// its [`HOOK_SESSION_END_TOMBSTONE_TTL`]: a trailing hook event delivered
    /// reordered after the end must not re-register the dead session. Shared by
    /// [`Reducer::synthesize_hook_registration`], the `Identity` arm, and the
    /// `SessionStart` arm's child-registration gate (#242).
    pub(super) fn hook_session_end_tombstoned(&self, id: AgentId, now: SystemTime) -> bool {
        self.recent_hook_session_ends.get(&id).is_some_and(|ts| {
            now.duration_since(*ts)
                .is_ok_and(|d| d < HOOK_SESSION_END_TOMBSTONE_TTL)
        })
    }

    /// Whether the child ledger records `id` as ENDED within
    /// [`CHILD_END_LEDGER_TTL`] — the #244-w2 gate's predicate (the ledger
    /// sibling of [`Correlation::hook_session_end_tombstoned`]). Clock-regression
    /// safe like the `gc` retains (a future timestamp is not fresh).
    pub(super) fn child_recently_ended(&self, id: AgentId, now: SystemTime) -> bool {
        self.child_ledger.get(&id).is_some_and(|e| {
            e.ended_at.is_some_and(|ts| {
                now.duration_since(ts)
                    .is_ok_and(|d| d < CHILD_END_LEDGER_TTL)
            })
        })
    }

    pub(super) fn gc(&mut self, now: SystemTime) {
        // SystemTime::duration_since returns Err when `ts` is in the future
        // (clock went backwards). Drop those — stale entries either way.
        self.recent_hook_tool_uses
            .retain(|_, (ts, _)| now.duration_since(*ts).is_ok_and(|d| d < HOOK_WINS_WINDOW));
        self.recent_hook_session_ends.retain(|_, ts| {
            now.duration_since(*ts)
                .is_ok_and(|d| d < HOOK_SESSION_END_TOMBSTONE_TTL)
        });
        self.recent_proof_of_life
            .retain(|_, ts| now.duration_since(*ts).is_ok_and(|d| d < PROOF_OF_LIFE_TTL));
        self.recent_task_drains.retain(|_, ts| {
            now.duration_since(*ts)
                .is_ok_and(|d| d < DRAINED_TASK_TOMBSTONE_TTL)
        });
        // Not-yet-ended entries ride until their end/sweep stamps ended_at
        // (every slot removal goes through sweep_exited, which stamps it), so
        // the map is bounded by live children + the TTL's trailing window.
        self.child_ledger.retain(|_, e| match e.ended_at {
            None => true,
            Some(ts) => now
                .duration_since(ts)
                .is_ok_and(|d| d < CHILD_END_LEDGER_TTL),
        });
    }

    /// Whether `id` holds a FRESH probe vouch — an [`AgentEvent::ProofOfLife`]
    /// recorded within [`PROOF_OF_LIFE_TTL`]. The single freshness predicate
    /// shared by `sweep_stale`'s own-id exemption and its delegating-ancestor
    /// walk, so the TTL logic can't fork. Clock-regression-safe like the `gc`
    /// retains (`duration_since` Errs on a future timestamp → not fresh).
    pub(super) fn vouch_fresh(&self, id: &AgentId, now: SystemTime) -> bool {
        self.recent_proof_of_life
            .get(id)
            .is_some_and(|t| now.duration_since(*t).is_ok_and(|d| d < PROOF_OF_LIFE_TTL))
    }
}
