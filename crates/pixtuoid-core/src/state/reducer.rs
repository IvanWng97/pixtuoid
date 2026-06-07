use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::source::{AgentEvent, Transport};
use crate::state::{fsm, scope, ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Window in which a Hook event suppresses a later Jsonl event with the same tool_use_id.
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

/// How long to keep an exiting agent's slot alive after `SessionEnd` so the
/// walkout-to-door animation has time to play before the slot is removed.
pub const EXIT_GRACE_WINDOW: Duration = Duration::from_millis(4500);

/// How long the slot stays visually Active after an `ActivityEnd` before
/// the reducer's tick flips it to Idle. Hides the per-tool-call Active
/// flicker that rapid PreToolUse → PostToolUse chains produce in CC; any
/// `ActivityStart` arriving within this window cancels the pending idle,
/// so the slot reads as continuously Active for chained tool work.
pub const ACTIVE_GRACE_WINDOW: Duration = Duration::from_millis(1500);

/// State-adaptive stale-agent thresholds. If `now - last_event_at`
/// exceeds the threshold for the agent's current state, the reducer
/// marks it exiting. Modeled after Kubernetes liveness probes (detect
/// failure to respond, not the act of dying) + Prometheus staleness
/// (5-min scrape gap = stale target).
///
/// Active: CC fires tool events every few seconds when working. 10 min
///   of silence means the process died mid-tool.
/// Idle: users legitimately pause for breaks. 30 min catches "closed
///   terminal" without reaping lunch-break idle.
/// Waiting: user could be in a meeting reviewing the permission prompt.
///   60 min is generous but still GCs eventually.
/// Unknown cwd (cc#N label): almost always a ghost from startup JSONL
///   seeding that never gets a follow-up event. 3 min is aggressive
///   but the false-positive cost is low (just a desk slot freed).
pub const STALE_ACTIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const STALE_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const STALE_WAITING_TIMEOUT: Duration = Duration::from_secs(60 * 60);
pub const STALE_UNKNOWN_CWD_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// Idle timeout for **Codex** agents — much shorter than the generic
/// [`STALE_IDLE_TIMEOUT`] because Codex exposes **no session-end signal of any
/// kind**: it has no `SessionEnd` hook (its `HookEventName` enum has none — only
/// `Stop`, which is *turn* end), its payloads carry no PID, and its internal
/// `ShutdownComplete` event is not persisted to the rollout (so there is no
/// durable marker to tail-scan). All three were verified against upstream
/// `openai/codex`. The stale-sweep is therefore the ONLY reaper a closed Codex
/// session ever gets — at the 30-min generic timeout it lingers as a ghost long
/// after the process is gone.
///
/// The shorter window is safe specifically for Codex: the only false-positive is
/// a *live* Codex session that sits idle between turns past the threshold, and
/// that is **self-healing** — its next `UserPromptSubmit` re-emits `SessionStart`
/// and the sprite walks back in. CC keeps the long [`STALE_IDLE_TIMEOUT`]: it has
/// real `SessionEnd` signals (best-effort hook + durable `/exit` marker) for the
/// common clean exit, so a short reaper there would only evict genuinely
/// live-but-idle sessions (lunch-break idle) with no upside.
// Naming note: selection is now caps-generic (`SourceCaps::short_idle_reap`),
// so "CODEX" in the name is the historical motivating case, not a gate. It is
// a pub const in a published crate — fold a rename (STALE_SHORT_IDLE_TIMEOUT)
// into the next breaking-version batch rather than burning one on it.
pub const STALE_CODEX_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The state-adaptive stale timeout for one slot. Unknown-cwd ghosts reap on the
/// shortest window (almost always startup-seeding artifacts). Otherwise the
/// timeout follows the activity state — with one carve-out: an idle slot whose
/// source has `caps.short_idle_reap()` (today: Codex — no exit signal of any
/// kind, so the sweep is its only reaper, AND the lone false positive — a
/// live-but-idle session past the window — self-heals on its next
/// `UserPromptSubmit`) uses [`STALE_CODEX_IDLE_TIMEOUT`] instead of the long
/// [`STALE_IDLE_TIMEOUT`]. CC keeps the long window — its real `SessionEnd`
/// signals make a short reaper all cost, no benefit; Antigravity also lacks an
/// exit signal but CANNOT resurrect on a prompt, so a short reap would vanish
/// a live session permanently (see `SourceCaps::short_idle_reap`).
fn stale_threshold(slot: &AgentSlot) -> Duration {
    stale_threshold_with_caps(
        slot,
        crate::source::registry::descriptor_for(&slot.source).map(|d| &d.caps),
    )
}

/// Policy half of [`stale_threshold`], split from the registry lookup so caps
/// combinations no registered source has YET are unit-testable with a
/// synthetic [`SourceCaps`] (the lookup half is pinned by the registered-
/// source tests in `tests/reducer.rs`).
fn stale_threshold_with_caps(
    slot: &AgentSlot,
    caps: Option<&crate::source::registry::SourceCaps>,
) -> Duration {
    if slot.unknown_cwd {
        return STALE_UNKNOWN_CWD_TIMEOUT;
    }
    match &slot.state {
        // A Delegating slot on a source whose delegations are hook-silent
        // (in-process subagents that fire no hooks) emits NOTHING until the
        // dispatch tool's PostToolUse — `last_event_at` freezes for the whole
        // delegation, so a long research/review run would be swept mid-turn
        // on the Active timer. Give it the Waiting-class window instead. (CC
        // is immune by construction: its subagents' misattributed hooks
        // drive `refresh_lineage`.)
        ActivityState::Active { detail, .. }
            if caps.is_some_and(|c| c.delegations_are_hook_silent)
                && detail.as_deref() == Some(crate::source::ToolDetail::Task.display()) =>
        {
            STALE_WAITING_TIMEOUT
        }
        ActivityState::Active { .. } => STALE_ACTIVE_TIMEOUT,
        ActivityState::Idle if caps.is_some_and(|c| c.short_idle_reap()) => {
            STALE_CODEX_IDLE_TIMEOUT
        }
        ActivityState::Idle => STALE_IDLE_TIMEOUT,
        ActivityState::Waiting { .. } => STALE_WAITING_TIMEOUT,
    }
}

/// Display prefix for a source's labels (`cc·`, `ag·`, `cx·`), from the source
/// registry (the per-source fact table). Applied at `SessionStart`; the JSONL
/// `LabelDeriver` Renames (`cc_derive_label`/`derive_codex_label`/
/// `derive_ag_label`) produce the same prefixed string and so reinforce this
/// idempotently. An unregistered source falls back to its own name (the same
/// `other => other` contract as the old match).
fn source_label_prefix(source: &str) -> &str {
    crate::source::registry::descriptor_for(source)
        .map(|d| d.label_prefix)
        .unwrap_or(source)
}

/// Outcome flags from [`Reducer::track_active_tasks`], consumed by `apply`'s
/// main event match.
struct TaskTracking {
    /// An `ActivityEnd` drained a tracked Task: the general ActivityEnd arm
    /// must be skipped — otherwise it would redundantly re-arm
    /// `pending_idle_at` or arm it while tasks are still in flight.
    handled_by_task_tracking: bool,
    /// An `ActivityStart` dispatched a Task (applied as Active(Delegating)
    /// by the pre-pass when the slot exists; in the Task-before-SessionStart
    /// race nothing is applied — the skipped general arm would no-op too):
    /// the general ActivityStart arm must be skipped.
    handled_by_task_start: bool,
}

#[derive(Debug, Default)]
pub struct Reducer {
    /// Track recent hook-derived events so JSONL duplicates can be dropped.
    recent_hook_tool_uses: HashMap<(AgentId, String), SystemTime>,
    /// Per-agent set of Task tool_use_ids currently in flight. CC's hook
    /// payload sets `transcript_path` to the PARENT'S transcript even when a
    /// subagent is the actor, so subagent hook events hash to the parent's
    /// AgentId. While the parent has any Task in flight, hook
    /// ActivityStart/End events for that AgentId are dropped — JSONL has
    /// correct attribution to the subagent's own AgentId.
    active_tasks: HashMap<AgentId, HashSet<String>>,
    /// `tool_use_id` that was Active immediately before an agent entered
    /// `Waiting` (a CC permission `Notification` fires mid-tool). When THAT
    /// tool's `ActivityEnd` (its `PostToolUse`) arrives, the permission has been
    /// resolved and the gated tool ran — so the Waiting resolves (debounced to
    /// Idle) instead of lingering until the agent's next tool. A *parallel*
    /// tool ending carries a different id, so it can't false-clear a still-
    /// pending permission (preserves `parallel_tool_end_while_waiting_keeps_waiting`).
    /// Codex never populates this (its tool events carry no `tool_use_id`), so
    /// its permission resume stays on the `ActivityStart` path.
    gated_before_waiting: HashMap<AgentId, Arc<str>>,
    /// Monotonic counter for human-readable labels.
    next_label_n: u32,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the GC + exit-sweep + Active→Idle debounce expiry without
    /// applying an event. Must be called periodically (e.g. on each
    /// render tick) so exiting slots are reclaimed and pending-idle
    /// timers actually fire even when no new event arrives to drive
    /// `apply`.
    pub fn tick(&mut self, scene: &mut SceneState, now: SystemTime) {
        self.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        self.sweep_stale(scene, now);
        // Clean up active_tasks entries for agents that never got a
        // SessionStart (Task event arrived before JSONL created the slot).
        self.active_tasks
            .retain(|id, _| scene.agents.contains_key(id));
        self.gated_before_waiting
            .retain(|id, _| scene.agents.contains_key(id));
    }

    pub fn apply(
        &mut self,
        scene: &mut SceneState,
        event: AgentEvent,
        now: SystemTime,
        from: Transport,
    ) {
        self.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        let id = event.agent_id();

        // Liveness flows UP the tree: any activity by a descendant keeps its
        // ancestors alive, so a parent isn't stale-swept (and its subtree
        // cascaded out) while a subagent is still working — even if the parent's
        // own hooks dropped or a subagent's hook was misattributed to it. The
        // mirror of `cascade_exit` (which pushes EXIT down): liveness flows UP.
        // refresh_lineage stamps `last_event_at = now` on the ACTOR too (it walks
        // from `id` upward, not just the ancestors). The per-arm `last_event_at =
        // now` writes in enter_active/enter_waiting and the ActivityEnd arms below
        // therefore re-stamp the same `now` for these three events — harmless. They
        // are load-bearing only for the event paths NOT matched here (Rename and the
        // SessionStart-enrich path don't call refresh_lineage), so don't drop them.
        if matches!(
            &event,
            AgentEvent::ActivityStart { .. }
                | AgentEvent::ActivityEnd { .. }
                | AgentEvent::Waiting { .. }
        ) {
            scope::refresh_lineage(scene, id, now);
        }

        // PRE-PASS ORDER IS LOAD-BEARING: suppression → hook-wins dedup →
        // task tracking.
        // (1) Suppress before the dedup RECORD: a suppressed hook event must
        //     not record its tool_use_id, or it would dedup-drop its own JSONL
        //     copy — the only transport left to track that Task (e.g. a
        //     parallel second dispatch suppressed as a leak; pinned by
        //     `suppressed_parallel_task_dispatch_jsonl_copy_survives_dedup_and_tracks`).
        // (2) Dedup before task tracking: a JSONL duplicate must be dropped
        //     before it can drain `active_tasks` and fire `cascade_exit`
        //     mid-delegation (pinned by
        //     `jsonl_duplicate_task_end_inside_dedup_window_does_not_fire_cascade`).
        //     Caveat: that guarded window is synthetic-narrow — a real CC
        //     JSONL Task END only exists once the Task returned — so the pin
        //     freezes this refactor's ordering, not the kind-blind dedup
        //     keying (revisited in #150).
        if from == Transport::Hook && self.suppress_subagent_leak(scene, &event, id, now) {
            return;
        }

        // Dedup: drop JSONL events that match a recent Hook event by tool_use_id.
        if from == Transport::Jsonl {
            if let Some(tuid) = event_tool_use_id(&event) {
                if self
                    .recent_hook_tool_uses
                    .contains_key(&(id, tuid.to_string()))
                {
                    return;
                }
            }
        }

        if from == Transport::Hook {
            if let Some(tuid) = event_tool_use_id(&event) {
                self.recent_hook_tool_uses
                    .insert((id, tuid.to_string()), now);
            }
        }

        let TaskTracking {
            handled_by_task_tracking,
            handled_by_task_start,
        } = self.track_active_tasks(scene, &event, now);

        match event {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    // Already created — usually a harmless duplicate from the
                    // other transport. But a Codex subagent's own rollout
                    // (JSONL) can create the slot ORPHANED before its
                    // SubagentStart hook arrives with the parent link; enrich it
                    // so the subagent joins the scope tree regardless of arrival
                    // order. Never re-parent an agent that already has a parent.
                    if slot.parent_id.is_none() {
                        if let Some(p) = parent_id {
                            slot.parent_id = Some(p);
                        }
                    }
                    return;
                }
                let Some(desk_index) = scene.next_free_desk() else {
                    tracing::warn!(
                        ?agent_id,
                        cwd = %cwd.display(),
                        session_id = %session_id,
                        total_capacity = scene.total_capacity(),
                        "dropped SessionStart — all desks occupied; bump --max-desks"
                    );
                    return;
                };
                let floor_idx = scene.floor_of(desk_index);
                let base = cwd
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty());
                let has_cwd = base.is_some();
                let prefix = source_label_prefix(&source);
                let label: Arc<str> = match base {
                    Some(b) => Arc::<str>::from(format!("{prefix}·{b}").as_str()),
                    None => {
                        // Only an unknown-cwd ghost consumes an ordinal, so labels
                        // stay contiguous (cc#1, cc#2, …) instead of skipping the
                        // count of preceding named sessions.
                        self.next_label_n += 1;
                        Arc::<str>::from(format!("{prefix}#{}", self.next_label_n).as_str())
                    }
                };
                // Disambiguation for multiple sessions sharing a cwd happens
                // at render time, not here — we don't want to suffix unique
                // sessions with a noisy `·xxxx` they don't need.
                scene.agents.insert(
                    agent_id,
                    AgentSlot {
                        agent_id,
                        source: Arc::<str>::from(source.as_str()),
                        session_id: Arc::<str>::from(session_id.as_str()),
                        cwd: Arc::<std::path::Path>::from(cwd.as_path()),
                        label,
                        state: ActivityState::Idle,
                        state_started_at: now,
                        last_event_at: now,
                        created_at: now,
                        exiting_at: None,
                        pending_idle_at: None,
                        desk_index,
                        floor_idx,
                        tool_call_count: 0,
                        active_ms: 0,
                        unknown_cwd: !has_cwd,
                        parent_id,
                    },
                );
            }
            AgentEvent::ActivityStart {
                agent_id,
                activity,
                tool_use_id,
                detail,
            } => {
                if !handled_by_task_start {
                    // Resuming to Active (next tool / Codex function_call_output)
                    // makes any pending gated-permission correlation moot.
                    self.gated_before_waiting.remove(&agent_id);
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        if !detail.as_ref().is_some_and(|d| d.is_task()) {
                            slot.tool_call_count += 1;
                        }
                        fsm::enter_active(
                            slot,
                            activity,
                            tool_use_id.map(|s| Arc::<str>::from(s.as_str())),
                            detail.map(|d| Arc::<str>::from(d.display())),
                            now,
                        );
                    }
                }
            }
            AgentEvent::ActivityEnd {
                agent_id,
                ref tool_use_id,
            } => {
                // Skip if this end was already processed by task tracking above.
                if !handled_by_task_tracking {
                    // A CC permission's *gated* tool finishing resolves the
                    // Wait: its tool_use_id matches the one that was Active when
                    // Waiting began. A parallel tool ending has a different id,
                    // so it can't false-clear a still-pending permission.
                    let resolves_wait = matches!(
                        scene.agents.get(&agent_id).map(|s| &s.state),
                        Some(ActivityState::Waiting { .. })
                    ) && tool_use_id.is_some()
                        && self.gated_before_waiting.get(&agent_id).map(|g| &**g)
                            == tool_use_id.as_deref();
                    if resolves_wait {
                        self.gated_before_waiting.remove(&agent_id);
                    }
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        // Arm the idle debounce when Active (normal tool end) or
                        // when a gated permission just resolved — in both cases
                        // the slot settles to Idle after ACTIVE_GRACE_WINDOW. A
                        // stale ActivityEnd while Idle, or a parallel tool ending
                        // while Waiting, leaves the timer alone.
                        if matches!(slot.state, ActivityState::Active { .. }) || resolves_wait {
                            fsm::arm_pending_idle(slot, now);
                        }
                        slot.last_event_at = now;
                    }
                }
            }
            AgentEvent::Waiting { agent_id, reason } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    // Remember the mid-flight tool so its later PostToolUse
                    // (same tool_use_id) can resolve this permission Wait.
                    if let ActivityState::Active {
                        tool_use_id: Some(tuid),
                        ..
                    } = &slot.state
                    {
                        self.gated_before_waiting.insert(agent_id, tuid.clone());
                    } else {
                        self.gated_before_waiting.remove(&agent_id);
                    }
                    fsm::enter_waiting(slot, Arc::<str>::from(reason.as_str()), now);
                }
            }
            AgentEvent::Rename { agent_id, label } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    fsm::rename(slot, &label, now);
                }
            }
            AgentEvent::SessionEnd { agent_id } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    fsm::mark_exiting(slot, now);
                }
                scope::cascade_exit(scene, agent_id, now);
            }
        }
    }

    /// Pre-pass 1 of [`Reducer::apply`] — subagent-leak suppression (hook
    /// transport only): if this AgentId currently has any Task tool in
    /// flight, hook ActivityStart/End events for it are almost certainly
    /// subagent work misattributed to the parent. Drop them (returns `true`)
    /// and defer to JSONL, which targets the subagent's own AgentId. The
    /// Task's own PostToolUse is exempt — its tool_use_id matches one we are
    /// tracking, so it passes through and clears the slot.
    fn suppress_subagent_leak(
        &mut self,
        scene: &mut SceneState,
        event: &AgentEvent,
        id: AgentId,
        now: SystemTime,
    ) -> bool {
        let tasks = self.active_tasks.get(&id);
        let in_task = tasks.is_some_and(|s| !s.is_empty());
        let suppress = match event {
            AgentEvent::ActivityStart { .. } => in_task,
            AgentEvent::ActivityEnd { tool_use_id, .. } => {
                let is_task_self_end = tool_use_id
                    .as_ref()
                    .is_some_and(|t| tasks.is_some_and(|s| s.contains(t)));
                in_task && !is_task_self_end
            }
            _ => false,
        };
        if suppress {
            // The misattributed subagent event already refreshed the
            // parent's lineage in `apply` (liveness flows up), keeping the
            // delegating parent from being wrongly stale-swept.
            //
            // One state change still belongs to the parent: if it is
            // `Waiting` while delegating, that Waiting is the SUBAGENT's
            // permission gate (the `Notification` was misattributed to the
            // parent) — a parent blocked on a Task isn't running its own
            // tools. A suppressed child event means the subagent resumed
            // work, so the gate resolved: restore Active(Delegating) instead
            // of leaving a stale "permission?" Waiting until the 60-min
            // stale-sweep. Then drop the spurious display update.
            if let Some(slot) = scene.agents.get_mut(&id) {
                if matches!(slot.state, ActivityState::Waiting { .. }) {
                    let task_tuid = tasks
                        .and_then(|s| s.iter().next())
                        .map(|t| Arc::<str>::from(t.as_str()));
                    fsm::enter_delegating(slot, task_tuid, now);
                    self.gated_before_waiting.remove(&id);
                }
            }
        }
        suppress
    }

    /// Last pre-pass of [`Reducer::apply`] (after the inline hook-wins
    /// dedup) — track active Task tool_use_ids from either transport.
    /// HashSet is idempotent so duplicate inserts from both hook+jsonl are
    /// harmless.
    ///
    /// Side effect: when the parent gains a Task, also mark it as
    /// Active("Delegating") so it doesn't look idle/asleep while its
    /// subagents do the visible work. When the last Task drains, the next
    /// normal hook/JSONL event will reset its state.
    ///
    /// b1 subagent-completion inference (CC writes no completion marker): a
    /// drained parent Task means the delegated subtree returned — cascade
    /// EXIT to the parent's descendants (not the parent, which keeps running)
    /// so completed subagents leave promptly instead of lingering to the
    /// 30-min idle stale-sweep. CC infers completion from the Task drain
    /// here; a source with a clean "subagent finished" signal (e.g. Codex)
    /// would drive the same cascade through its own decoder.
    fn track_active_tasks(
        &mut self,
        scene: &mut SceneState,
        event: &AgentEvent,
        now: SystemTime,
    ) -> TaskTracking {
        let mut handled_by_task_tracking = false;
        let mut handled_by_task_start = false;
        // Set when the parent's LAST Task drains on this event, so the
        // delegated subtree can be marked exiting below (b1).
        let mut completed_subtree_root: Option<AgentId> = None;
        match event {
            AgentEvent::ActivityStart {
                agent_id,
                tool_use_id: Some(tuid),
                detail: Some(d),
                ..
            } if d.is_task() => {
                handled_by_task_start = true;
                self.active_tasks
                    .entry(*agent_id)
                    .or_default()
                    .insert(tuid.clone());
                if let Some(slot) = scene.agents.get_mut(agent_id) {
                    fsm::enter_delegating(slot, Some(Arc::<str>::from(tuid.as_str())), now);
                }
            }
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: Some(tuid),
            } => {
                if let Some(set) = self.active_tasks.get_mut(agent_id) {
                    if set.remove(tuid) {
                        handled_by_task_tracking = true;
                        if let Some(slot) = scene.agents.get_mut(agent_id) {
                            slot.last_event_at = now;
                            // Debounce: stay visually Active for
                            // ACTIVE_GRACE_WINDOW; expire_pending_idles flips to
                            // Idle if no new tool starts inside the window. Only
                            // arm when actually Active — if the parent is Waiting
                            // (its own permission prompt fired during delegation)
                            // a Task drain must NOT arm the idle-resolve, or the
                            // expiry would false-clear a still-pending permission.
                            if set.is_empty() {
                                // Parent's last Task returned → the delegated
                                // subtree is done; mark it exiting after this
                                // match (b1).
                                completed_subtree_root = Some(*agent_id);
                                if matches!(slot.state, ActivityState::Active { .. }) {
                                    fsm::arm_pending_idle(slot, now);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if let Some(parent) = completed_subtree_root {
            scope::cascade_exit(scene, parent, now);
        }

        TaskTracking {
            handled_by_task_tracking,
            handled_by_task_start,
        }
    }

    fn gc(&mut self, now: SystemTime) {
        // SystemTime::duration_since returns Err when `ts` is in the future
        // (clock went backwards). Drop those — stale entries either way.
        self.recent_hook_tool_uses
            .retain(|_, ts| now.duration_since(*ts).is_ok_and(|d| d < HOOK_WINS_WINDOW));
    }

    /// Walk through agents with `pending_idle_at` set and flip their
    /// state to Idle if the debounce window has elapsed. Resets
    /// `state_started_at` to `now` so the Idle wander state machine
    /// starts fresh from the visible transition, not from the
    /// (now-stale) original ActivityEnd time. Applies to Active slots
    /// (normal tool end) and to a Waiting slot whose *gated* permission
    /// tool resolved (the ActivityEnd arm armed the timer). A Waiting
    /// slot with a still-pending or parallel-tool prompt never has the
    /// timer set, so it is left alone.
    fn expire_pending_idles(&mut self, scene: &mut SceneState, now: SystemTime) {
        for slot in scene.agents.values_mut() {
            let Some(pending) = slot.pending_idle_at else {
                continue;
            };
            if now
                .duration_since(pending)
                .is_ok_and(|d| d >= ACTIVE_GRACE_WINDOW)
            {
                // A Waiting slot only carries `pending_idle_at` when its gated
                // permission tool resolved (ActivityEnd arm); a *parallel*-prompt
                // Waiting never gets the timer armed, so it isn't reached here.
                fsm::settle_to_idle(slot, pending, now);
            }
        }
    }

    /// Mark agents as exiting when they haven't emitted any event for
    /// longer than their state-adaptive threshold. Uses `last_event_at`
    /// (updated on every reducer event) as the liveness signal, NOT
    /// `state_started_at` (which only tracks the current state's age).
    ///
    /// Unknown-cwd agents (label starts with "cc#") get a much shorter
    /// timeout — they're almost always ghosts from JSONL startup seeding.
    fn sweep_stale(&mut self, scene: &mut SceneState, now: SystemTime) {
        // Pass 1 — collect agents crossing their stale threshold this tick.
        // Immutable borrow: we can't cascade (which re-borrows `scene` mutably)
        // while it's held, so gather ids first, mutate in pass 2. Mirrors
        // `sweep_exited`'s collect-then-mutate shape.
        // Readiness exemption: a node blocked under a `Waiting` ancestor (e.g. a
        // subagent whose permission Notification was attributed to the parent) is
        // paused on a human gate, not dead — skip it on the aggressive timer.
        // Liveness vs readiness (k8s): a "not ready" pod isn't killed.
        let agents = &scene.agents;
        let stale: Vec<(AgentId, Duration, Duration)> = agents
            .values()
            .filter(|slot| slot.exiting_at.is_none())
            .filter_map(|slot| {
                if scope::has_waiting_ancestor(agents, slot.agent_id) {
                    return None;
                }
                let age = now
                    .duration_since(slot.last_event_at)
                    .unwrap_or(Duration::ZERO);
                let threshold = stale_threshold(slot);
                (age > threshold).then_some((slot.agent_id, age, threshold))
            })
            .collect();

        // Pass 2 — mark each stale agent exiting, then cascade to its subagents
        // so a stale-swept (or abruptly-exited, SessionEnd-less) parent never
        // leaves orphaned children behind. Skip any slot a prior cascade in this
        // same sweep already marked (keeps the log + `exiting_at` write-once).
        for (id, age, threshold) in stale {
            {
                // Defensive: `id` was just collected from `scene.agents` in pass 1
                // and nothing removes a slot between the passes (a prior cascade only
                // SETS `exiting_at`), so this `continue` is unreachable today —
                // single-threaded `&mut SceneState`. Kept to harden against a future
                // refactor that mutates membership mid-sweep.
                let Some(slot) = scene.agents.get_mut(&id) else {
                    continue;
                };
                if slot.exiting_at.is_some() {
                    continue;
                }
                tracing::info!(
                    agent_id = ?id,
                    label = %slot.label,
                    age_secs = age.as_secs(),
                    threshold_secs = threshold.as_secs(),
                    "stale agent — marking exiting"
                );
                slot.exiting_at = Some(now);
            }
            scope::cascade_exit(scene, id, now);
        }
    }

    /// Remove agents whose exit animation has finished. Called at the top
    /// of every event apply, so any subsequent event naturally triggers
    /// the cleanup of expired slots.
    ///
    /// Removing a parent does NOT null any surviving child's `parent_id` — that
    /// pointer is left dangling intentionally. The scope walks tolerate it (the
    /// `None => break` guards in `scope::{refresh_lineage, has_waiting_ancestor}`),
    /// so it never crashes; in practice `cascade_exit` reaps the subtree alongside
    /// the parent, so a lingering dangle is only observable for a true orphan
    /// (JSONL-first child of a never-created parent). Scanning every child on each
    /// parent removal to null the pointer would add cost with no behavioral benefit.
    fn sweep_exited(&mut self, scene: &mut SceneState, now: SystemTime) {
        let expired: Vec<AgentId> = scene
            .agents
            .iter()
            .filter_map(|(id, slot)| {
                slot.exiting_at
                    .filter(|t| now.duration_since(*t).is_ok_and(|d| d > EXIT_GRACE_WINDOW))
                    .map(|_| *id)
            })
            .collect();
        for id in expired {
            scene.agents.remove(&id);
            self.active_tasks.remove(&id);
            // Symmetric with active_tasks: sweep_exited runs on the apply path
            // (not just tick), where the tick-time `gated_before_waiting.retain`
            // doesn't run — so reclaim it here too, else a Waiting slot that was
            // swept mid-turn leaks its gated tool_use_id until the next tick.
            self.gated_before_waiting.remove(&id);
        }
    }
}

fn event_tool_use_id(ev: &AgentEvent) -> Option<&str> {
    match ev {
        AgentEvent::ActivityStart { tool_use_id, .. }
        | AgentEvent::ActivityEnd { tool_use_id, .. } => tool_use_id.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::source_label_prefix;
    use crate::source::REGISTERED_SOURCES;

    /// Every registered source needs a 2-char prefix. The unregistered-source
    /// fallback silently degrades a missing/short prefix to the long source
    /// name (e.g. "opencode·proj" instead of "oc·proj"), which then collides
    /// visually with another source sharing a cwd. End-to-end through the
    /// REAL `source_label_prefix` (registry lookup included) — stronger than
    /// the registry-local shape check, which can't see a name↔row mismatch.
    #[test]
    fn every_registered_source_has_two_char_label_prefix() {
        for src in REGISTERED_SOURCES {
            let prefix = source_label_prefix(src);
            assert_eq!(
                prefix.chars().count(),
                2,
                "source {src:?} has no 2-char label prefix (got {prefix:?}) — fix its SourceDescriptor row in source/registry.rs"
            );
        }
    }

    // The Delegating stale carve-out is caps-driven; no REGISTERED source
    // sets `delegations_are_hook_silent` yet (the first is Reasonix, PR
    // #134), so pin the policy half with a synthetic caps value — that's
    // what the lookup/policy split exists for.
    #[test]
    fn delegating_slot_with_hook_silent_caps_gets_waiting_window() {
        use super::{stale_threshold_with_caps, STALE_ACTIVE_TIMEOUT, STALE_WAITING_TIMEOUT};
        use crate::source::registry::SourceCaps;
        use crate::source::{AgentEvent, ToolDetail, Transport};
        use crate::{AgentId, Reducer, SceneState};
        use std::time::SystemTime;
        let caps = SourceCaps {
            has_exit_signal: true,
            resurrects_on_prompt: true,
            delegations_are_hook_silent: true,
        };
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let id = AgentId::from_parts("hook-silent-cli", "/p");
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "hook-silent-cli".into(),
                session_id: "/p".into(),
                cwd: "/p".into(),
                parent_id: None,
            },
            SystemTime::UNIX_EPOCH,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: id,
                activity: crate::source::Activity::Typing,
                tool_use_id: None,
                detail: Some(ToolDetail::Task),
            },
            SystemTime::UNIX_EPOCH,
            Transport::Hook,
        );
        let slot = scene.agents.get(&id).unwrap();
        assert_eq!(
            stale_threshold_with_caps(slot, Some(&caps)),
            STALE_WAITING_TIMEOUT,
            "hook-silent Delegating slot must get the Waiting-class window"
        );
        assert_eq!(
            stale_threshold_with_caps(slot, None),
            STALE_ACTIVE_TIMEOUT,
            "without the cap, Delegating reaps on the normal Active timer"
        );

        // Detail-gate negative: caps on + an ORDINARY tool active must stay on
        // the Active timer — the cap widens the window for delegations only.
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: id,
                activity: crate::source::Activity::Typing,
                tool_use_id: None,
                detail: Some(ToolDetail::Generic {
                    display: "bash: ls".into(),
                }),
            },
            SystemTime::UNIX_EPOCH,
            Transport::Hook,
        );
        let slot = scene.agents.get(&id).unwrap();
        assert_eq!(
            stale_threshold_with_caps(slot, Some(&caps)),
            STALE_ACTIVE_TIMEOUT,
            "caps-on but non-Task detail must keep the Active timer"
        );
    }

    // White-box: `gated_before_waiting` is reclaimed in TWO places — `tick`'s
    // retain and `sweep_exited`'s explicit remove (the apply path, where tick's
    // retain never runs). All existing reducer tests go through `tick`; this
    // pins the apply-path eviction so a future refactor can't silently drop it
    // and leak a swept Waiting slot's gated tool_use_id.
    #[test]
    fn gated_before_waiting_evicted_on_apply_path_sweep() {
        use crate::source::{Activity, AgentEvent, ToolDetail, Transport};
        use crate::state::SceneState;
        use crate::AgentId;
        use std::path::PathBuf;
        use std::time::{Duration, SystemTime};

        let mut r = super::Reducer::new();
        let mut scene = SceneState::uniform(4);
        let id = AgentId::from_transcript_path("/p/a.jsonl");
        let t0 = SystemTime::now();
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "claude-code".into(),
                session_id: "s".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
        // Active mid-tool, then a permission Waiting → gate records the tool id.
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: id,
                activity: Activity::Typing,
                tool_use_id: Some("toolT".into()),
                detail: Some(ToolDetail::from("Bash")),
            },
            t0,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::Waiting {
                agent_id: id,
                reason: "perm".into(),
            },
            t0,
            Transport::Hook,
        );
        assert!(
            r.gated_before_waiting.contains_key(&id),
            "gate recorded while Waiting mid-tool"
        );

        // End it; advance past the grace window; apply an UNRELATED event so
        // sweep_exited runs on the APPLY path (not tick).
        r.apply(
            &mut scene,
            AgentEvent::SessionEnd { agent_id: id },
            t0,
            Transport::Hook,
        );
        let later = t0 + super::EXIT_GRACE_WINDOW + Duration::from_secs(1);
        let other = AgentId::from_transcript_path("/p/other.jsonl");
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: other,
                source: "claude-code".into(),
                session_id: "s2".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            later,
            Transport::Hook,
        );

        assert!(
            !scene.agents.contains_key(&id),
            "exited slot swept on the apply path"
        );
        assert!(
            !r.gated_before_waiting.contains_key(&id),
            "apply-path sweep_exited must evict the gated entry (not only tick's retain)"
        );
    }
}
