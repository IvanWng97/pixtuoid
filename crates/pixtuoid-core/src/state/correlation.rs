//! The reducer's cross-slot correlation maps, extracted into one struct.
//! Don't move these maps onto `AgentSlot`: they span slots and are
//! deliberately not a semver surface.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::AgentId;

/// Window in which a Hook event suppresses a later Jsonl event with the same
/// tool_use_id. The suppression is asymmetric by event kind — see
/// [`ToolEventKind`] (#150).
///
// `pub` + `#[doc(hidden)]`: these tuning knobs are visible only so the
// `tests/reducer/` binary (a separate crate) can derive its timing offsets
// from them, while staying off the rendered docs and cargo-semver-checks.
#[doc(hidden)]
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

/// How long a hook `SessionEnd` for an UNKNOWN id suppresses hook-synthesis
/// for that id AND child (`parent_id`-carrying) `SessionStart` registration
/// (#242, either transport). Hook connections are per-connection spawned
/// tasks, so a session's SessionEnd and a trailing Stop/ActivityEnd can be
/// DELIVERED reordered, and the straggler would otherwise synthesize a blank
/// Idle ghost with NO SessionEnd left to remove it. 5s covers same-machine
/// scheduling jitter while never visibly delaying a genuine revival.
#[doc(hidden)]
pub const HOOK_SESSION_END_TOMBSTONE_TTL: Duration = Duration::from_secs(5);

/// How long a child-ledger entry's `ended_at` keeps gating a PARENTED
/// re-registration of that child after it ended (#244). Sized past the
/// watcher's 60s poll backstop, the worst case for a late transcript
/// first-sight; child ids are per-spawn unique, so a parented Start inside
/// the window is never a legitimate new child, only the dead one's late echo.
/// Parentless Starts are deliberately NOT gated — a Codex resurrect-on-prompt
/// is a legitimate same-id new life and re-links via the ledger (#246).
#[doc(hidden)]
pub const CHILD_END_LEDGER_TTL: Duration = Duration::from_secs(90);

/// How long the ledger entry itself is RETAINED after the child ended — the
/// `parent_id` memory the #246 parentless-revival re-link reads.
///
/// Deliberately LONGER than [`CHILD_END_LEDGER_TTL`]: the GATE is bounded by
/// the watcher's 60s poll backstop, while the MEMORY must span a TURN gap,
/// which is unbounded. Sharing one clock meant a child idle >90s came back an
/// ORPHAN, the exact phantom #246 exists to eliminate. Aligned with
/// `jsonl::unclaim::CHILD_END_UNCLAIM_TTL`, the sibling half of this flow —
/// the re-link cannot outlive the memory it depends on. The gate is
/// unaffected: [`Correlation::child_recently_ended`] applies its OWN
/// freshness check, so a retained-but-stale entry gates nothing.
#[doc(hidden)]
pub const CHILD_END_RELINK_TTL: Duration = Duration::from_secs(300);

/// How long a drained Task `tool_use_id` is remembered so a lagged JSONL
/// replay of its Start cannot re-fire `enter_delegating`. After the drain the
/// transcript's batched Start+End pair replays into an EMPTY set, so the
/// first-insert gate reads it as a fresh dispatch and would clobber a Waiting
/// the parent raised in the gap. Sized past the 60s `scan_root` poll backstop;
/// a `tool_use_id` is never legitimately re-dispatched, so generosity costs
/// only the tombstone's map entry.
#[doc(hidden)]
pub const DRAINED_TASK_TOMBSTONE_TTL: Duration = Duration::from_secs(90);

/// How long an [`AgentEvent::ProofOfLife`] vouch exempts its slot from the
/// staleness sweeps (#220). The probe is ground truth that the OWNING PROCESS
/// is alive, while every `STALE_*` window only models event silence — so a
/// vouched slot must not be swept on silence alone. Sized 2.5× the watcher's
/// 60s poll cadence: two missed polls plus slack.
#[doc(hidden)]
pub const PROOF_OF_LIFE_TTL: Duration = Duration::from_secs(150);

/// One child's remembered lifecycle in [`Correlation::child_ledger`].
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChildLedgerEntry {
    /// The last APPLIED parent link — `None` when the child was only ever
    /// seen ending (a Stop-before-Start reorder blocked its Start).
    pub(super) parent_id: Option<AgentId>,
    /// When the child ended (`as_child` SessionEnd) or its slot was removed,
    /// whichever came first. Starts TWO clocks: the [`CHILD_END_RELINK_TTL`]
    /// GC clock and the shorter [`CHILD_END_LEDGER_TTL`] gate.
    pub(super) ended_at: Option<SystemTime>,
}

/// Kind half of a hook-wins dedup record, driving the asymmetric drop matrix
/// (#150): an End record suppresses BOTH JSONL kinds — the tool is over, so a
/// lagged JSONL Start replay would falsely re-Activate and cancel the armed
/// idle debounce — while a Start record suppresses only Starts. A JSONL End
/// must never be eaten by its own tool's dispatch record: when the best-effort
/// PostToolUse hook drops, an eaten Task self-End leaks `active_tasks` for the
/// rest of the session. Don't "simplify" this to exact-kind matching — it
/// would orphan the lagged-pair case (pinned by
/// `late_batched_jsonl_pair_after_delivered_hook_end_is_fully_dropped`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ToolEventKind {
    Start,
    End,
}

/// The seven reducer-private correlation maps. In/out criterion for a future
/// map: PASSIVE cross-event memory (consulted to interpret a later event)
/// lives here; ARMED actions that mutate the scene on a schedule
/// (`pending_b1_cascades` and its fire pass) stay on `Reducer`.
#[derive(Debug, Default)]
pub(super) struct Correlation {
    /// Recent hook-derived events, so JSONL duplicates can be dropped. A hook
    /// End overwrites its tool's Start entry (kind-in-the-VALUE, not the key),
    /// which is what lets one End record cover the whole lagged JSONL pair.
    pub(super) recent_hook_tool_uses: HashMap<(AgentId, String), (SystemTime, ToolEventKind)>,
    /// Short-TTL tombstones for hook `SessionEnd`s that arrived for an id
    /// with NO slot — an invisible (unregistered) session ending. A reordered
    /// trailing hook event must not re-synthesize the session, and a reordered
    /// CHILD `SessionStart` must not register it (#242, both transports).
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
    /// a Waiting raised since.
    pub(super) recent_task_drains: HashMap<(AgentId, String), SystemTime>,
    /// Memory of CHILD (subagent) lifecycles, surviving the slots themselves
    /// (#244/#246). `parent_id` is upserted whenever a parent link is APPLIED,
    /// `ended_at` by an `as_child` SessionEnd (regardless of slot existence,
    /// covering the Stop-before-Start reorder) and by `sweep_exited` removing
    /// the child's slot. Consumed by the `SessionStart` arm: a fresh
    /// `ended_at` gates a PARENTED re-registration (the dead child's late
    /// echo), while a PARENTLESS start ADOPTS the remembered parent.
    pub(super) child_ledger: HashMap<AgentId, ChildLedgerEntry>,
    /// Sweep-exemption timestamps from [`AgentEvent::ProofOfLife`] (#220):
    /// a slot vouched for within [`PROOF_OF_LIFE_TTL`] is skipped by
    /// `sweep_stale`'s candidate collection.
    pub(super) recent_proof_of_life: HashMap<AgentId, SystemTime>,
    /// `tool_use_id` that was Active immediately before an agent entered
    /// `Waiting` (a CC permission `Notification` fires mid-tool). When THAT
    /// tool's `ActivityEnd` arrives the permission has been resolved, so the
    /// Waiting resolves instead of lingering until the agent's next tool; a
    /// *parallel* tool ending carries a different id and cannot false-clear a
    /// still-pending permission. Codex never populates this — its tool events
    /// carry no `tool_use_id`.
    pub(super) gated_before_waiting: HashMap<AgentId, Arc<str>>,
}

/// Freshness under a TTL, clock-regression-safe: `duration_since` returns
/// `Err` when `ts` is in the future, which folds to NOT-fresh. The ONE
/// spelling of the `elapsed < ttl` policy every correlation map and predicate
/// routes through, so the strict-`<` boundary can't drift across the sites.
fn is_fresh(now: SystemTime, ts: SystemTime, ttl: Duration) -> bool {
    now.duration_since(ts).is_ok_and(|d| d < ttl)
}

/// Whether `ttl` has ELAPSED since `ts` — the `>=` (inclusive) complement of
/// [`is_fresh`], clock-regression-safe (a backward clock reads as NOT-yet-
/// elapsed rather than panicking like a hand-rolled `duration_since().unwrap()`
/// would). See [`elapsed_past`] for the strict `>` variant: the boundary case
/// differs and is separately test-pinned, so the two stay distinct.
pub(super) fn elapsed_at_least(now: SystemTime, ts: SystemTime, ttl: Duration) -> bool {
    now.duration_since(ts).is_ok_and(|d| d >= ttl)
}

/// [`elapsed_at_least`] with a STRICT `>`. The exit-grace GC's boundary rides
/// this, so it stays distinct from the inclusive variant.
pub(super) fn elapsed_past(now: SystemTime, ts: SystemTime, ttl: Duration) -> bool {
    now.duration_since(ts).is_ok_and(|d| d > ttl)
}

impl Correlation {
    /// An arbitrary in-flight Task tuid for `id`, as the `Arc<str>` the FSM
    /// takes. The CHOICE among several is deliberately unspecified —
    /// `fsm::enter_delegating` only needs proof that *some* delegation is live,
    /// and `active_tasks` is a `HashSet` with no ordering to promise.
    pub(super) fn any_active_task(&self, id: &AgentId) -> Option<Arc<str>> {
        self.active_tasks
            .get(id)
            .and_then(|s| s.iter().next())
            .map(|t| Arc::<str>::from(t.as_str()))
    }

    /// Whether a hook `SessionEnd` for `id` (which had no slot) is still inside
    /// its [`HOOK_SESSION_END_TOMBSTONE_TTL`]: a trailing hook event delivered
    /// reordered after the end must not re-register the dead session.
    pub(super) fn hook_session_end_tombstoned(&self, id: AgentId, now: SystemTime) -> bool {
        self.recent_hook_session_ends
            .get(&id)
            .is_some_and(|ts| is_fresh(now, *ts, HOOK_SESSION_END_TOMBSTONE_TTL))
    }

    /// Whether the child ledger records `id` as ENDED within
    /// [`CHILD_END_LEDGER_TTL`] — the #244 gate's predicate.
    pub(super) fn child_recently_ended(&self, id: AgentId, now: SystemTime) -> bool {
        self.child_ledger.get(&id).is_some_and(|e| {
            e.ended_at
                .is_some_and(|ts| is_fresh(now, ts, CHILD_END_LEDGER_TTL))
        })
    }

    /// Record an APPLIED child→parent link, REVIVING the ledger entry: clearing
    /// `ended_at` marks this life alive so [`Correlation::gc`] can't prune the
    /// memory while the child still lives. The ONE home of that coupled write —
    /// both `SessionStart` link arms call it, so a future third link site can't
    /// set `parent_id` and forget the clear.
    pub(super) fn link_applied_parent(&mut self, child: AgentId, parent: AgentId) {
        let entry = self.child_ledger.entry(child).or_default();
        entry.parent_id = Some(parent);
        entry.ended_at = None;
    }

    /// TTL-prune every correlation map.
    ///
    /// [`Correlation::child_ledger`] is the odd retain: not-yet-ended entries
    /// ride until an end/sweep stamps `ended_at`, and the TTL applied is the
    /// RELINK budget, not the GATE's — dropping the entry at the gate's TTL
    /// also dropped the `parent_id` the #246 revival reads.
    pub(super) fn gc(&mut self, now: SystemTime) {
        self.recent_hook_tool_uses
            .retain(|_, (ts, _)| is_fresh(now, *ts, HOOK_WINS_WINDOW));
        self.recent_hook_session_ends
            .retain(|_, ts| is_fresh(now, *ts, HOOK_SESSION_END_TOMBSTONE_TTL));
        self.recent_proof_of_life
            .retain(|_, ts| is_fresh(now, *ts, PROOF_OF_LIFE_TTL));
        self.recent_task_drains
            .retain(|_, ts| is_fresh(now, *ts, DRAINED_TASK_TOMBSTONE_TTL));
        self.child_ledger.retain(|_, e| match e.ended_at {
            None => true,
            Some(ts) => is_fresh(now, ts, CHILD_END_RELINK_TTL),
        });
    }

    /// Whether `id` holds a FRESH probe vouch. The single freshness predicate
    /// shared by `sweep_stale`'s own-id exemption and its delegating-ancestor
    /// walk, so the TTL logic can't fork.
    pub(super) fn vouch_fresh(&self, id: &AgentId, now: SystemTime) -> bool {
        self.recent_proof_of_life
            .get(id)
            .is_some_and(|t| is_fresh(now, *t, PROOF_OF_LIFE_TTL))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_helpers_pin_inclusive_vs_strict_and_survive_a_backward_clock() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let ttl = Duration::from_millis(1500);
        // Exactly AT the boundary: `>=` fires, strict `>` does NOT — the
        // load-bearing distinction (exit-grace GC uses `>`, grace timers `>=`).
        let at = t0 + ttl;
        assert!(elapsed_at_least(at, t0, ttl), ">= fires at the boundary");
        assert!(
            !elapsed_past(at, t0, ttl),
            "> does NOT fire at the boundary"
        );
        let past = t0 + ttl + Duration::from_millis(1);
        assert!(elapsed_at_least(past, t0, ttl) && elapsed_past(past, t0, ttl));
        let before = t0 + Duration::from_millis(1499);
        assert!(!elapsed_at_least(before, t0, ttl) && !elapsed_past(before, t0, ttl));
        let backward = t0 - Duration::from_secs(10);
        assert!(!elapsed_at_least(backward, t0, ttl) && !elapsed_past(backward, t0, ttl));
    }

    /// A fixed anchor well past the epoch so `t0 + TTL` never under/overflows.
    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000)
    }

    #[test]
    fn session_end_tombstone_expires_at_exactly_its_ttl() {
        let id = AgentId::from_parts("claude-code", "s1");
        let mut corr = Correlation::default();
        corr.recent_hook_session_ends.insert(id, t0());
        let just_inside = t0() + HOOK_SESSION_END_TOMBSTONE_TTL - Duration::from_millis(1);
        assert!(corr.hook_session_end_tombstoned(id, just_inside));
        assert!(
            !corr.hook_session_end_tombstoned(id, t0() + HOOK_SESSION_END_TOMBSTONE_TTL),
            "freshness is strict: elapsed == TTL is expired"
        );
    }

    #[test]
    fn child_ledger_end_gate_expires_at_exactly_its_ttl() {
        let id = AgentId::from_parts("claude-code", "child");
        let mut corr = Correlation::default();
        corr.child_ledger.insert(
            id,
            ChildLedgerEntry {
                parent_id: None,
                ended_at: Some(t0()),
            },
        );
        let just_inside = t0() + CHILD_END_LEDGER_TTL - Duration::from_millis(1);
        assert!(corr.child_recently_ended(id, just_inside));
        assert!(
            !corr.child_recently_ended(id, t0() + CHILD_END_LEDGER_TTL),
            "freshness is strict: elapsed == TTL is expired"
        );
    }

    #[test]
    fn link_applied_parent_sets_the_link_and_revives_an_ended_entry() {
        let child = AgentId::from_parts("claude-code", "child");
        let parent = AgentId::from_parts("claude-code", "parent");
        let mut corr = Correlation::default();
        corr.child_ledger.insert(
            child,
            ChildLedgerEntry {
                parent_id: None,
                ended_at: Some(t0()),
            },
        );
        corr.link_applied_parent(child, parent);
        let entry = &corr.child_ledger[&child];
        assert_eq!(entry.parent_id, Some(parent));
        assert_eq!(
            entry.ended_at, None,
            "a re-link must clear ended_at (revive)"
        );
        assert!(
            !corr.child_recently_ended(child, t0() + Duration::from_secs(1)),
            "a revived entry is no longer 'recently ended'"
        );
    }

    #[test]
    fn vouch_freshness_expires_at_exactly_its_ttl() {
        let id = AgentId::from_parts("claude-code", "s1");
        let mut corr = Correlation::default();
        corr.recent_proof_of_life.insert(id, t0());
        let just_inside = t0() + PROOF_OF_LIFE_TTL - Duration::from_millis(1);
        assert!(corr.vouch_fresh(&id, just_inside));
        assert!(
            !corr.vouch_fresh(&id, t0() + PROOF_OF_LIFE_TTL),
            "freshness is strict: elapsed == TTL is expired"
        );
    }

    #[test]
    fn gc_prunes_each_map_at_exactly_its_ttl() {
        let old = AgentId::from_parts("claude-code", "old");
        let young = AgentId::from_parts("claude-code", "young");
        let step = Duration::from_millis(1);
        let mut corr = Correlation::default();
        corr.recent_hook_tool_uses
            .insert((old, "t1".into()), (t0(), ToolEventKind::Start));
        corr.recent_hook_tool_uses
            .insert((young, "t2".into()), (t0() + step, ToolEventKind::End));
        corr.gc(t0() + HOOK_WINS_WINDOW);
        assert!(!corr.recent_hook_tool_uses.contains_key(&(old, "t1".into())));
        assert!(corr
            .recent_hook_tool_uses
            .contains_key(&(young, "t2".into())));

        let mut corr = Correlation::default();
        corr.recent_hook_session_ends.insert(old, t0());
        corr.recent_hook_session_ends.insert(young, t0() + step);
        corr.gc(t0() + HOOK_SESSION_END_TOMBSTONE_TTL);
        assert!(!corr.recent_hook_session_ends.contains_key(&old));
        assert!(corr.recent_hook_session_ends.contains_key(&young));

        let mut corr = Correlation::default();
        corr.recent_proof_of_life.insert(old, t0());
        corr.recent_proof_of_life.insert(young, t0() + step);
        corr.gc(t0() + PROOF_OF_LIFE_TTL);
        assert!(!corr.recent_proof_of_life.contains_key(&old));
        assert!(corr.recent_proof_of_life.contains_key(&young));

        let mut corr = Correlation::default();
        corr.recent_task_drains.insert((old, "t1".into()), t0());
        corr.recent_task_drains
            .insert((young, "t2".into()), t0() + step);
        corr.gc(t0() + DRAINED_TASK_TOMBSTONE_TTL);
        assert!(!corr.recent_task_drains.contains_key(&(old, "t1".into())));
        assert!(corr.recent_task_drains.contains_key(&(young, "t2".into())));

        let mut corr = Correlation::default();
        corr.child_ledger.insert(
            old,
            ChildLedgerEntry {
                parent_id: None,
                ended_at: Some(t0()),
            },
        );
        corr.child_ledger.insert(
            young,
            ChildLedgerEntry {
                parent_id: None,
                ended_at: Some(t0() + step),
            },
        );
        let alive = AgentId::from_parts("claude-code", "alive");
        corr.child_ledger.insert(alive, ChildLedgerEntry::default());
        corr.gc(t0() + CHILD_END_RELINK_TTL);
        assert!(!corr.child_ledger.contains_key(&old));
        assert!(corr.child_ledger.contains_key(&young));
        assert!(corr.child_ledger.contains_key(&alive));
    }

    #[test]
    fn the_parent_link_outlives_the_end_gate_that_stops_gating() {
        let child = AgentId::from_parts("claude-code", "child");
        let parent = AgentId::from_parts("claude-code", "parent");
        let mut corr = Correlation::default();
        corr.child_ledger.insert(
            child,
            ChildLedgerEntry {
                parent_id: Some(parent),
                ended_at: Some(t0()),
            },
        );

        let after_gate = t0() + CHILD_END_LEDGER_TTL + Duration::from_secs(1);
        corr.gc(after_gate);
        assert!(
            !corr.child_recently_ended(child, after_gate),
            "the end gate must have lapsed"
        );
        assert_eq!(
            corr.child_ledger.get(&child).and_then(|e| e.parent_id),
            Some(parent),
            "the re-link memory must survive the gate it does not share a purpose with"
        );

        corr.gc(t0() + CHILD_END_RELINK_TTL);
        assert!(
            !corr.child_ledger.contains_key(&child),
            "the relink memory must still expire at its own budget"
        );
    }
}
