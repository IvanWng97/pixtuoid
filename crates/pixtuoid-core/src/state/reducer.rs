use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::source::{AgentEvent, Transport};
use crate::state::{fsm, scope, ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Window in which a Hook event suppresses a later Jsonl event with the same
/// tool_use_id. The suppression is asymmetric by event kind — a recorded hook
/// End drops both JSONL kinds, a recorded hook Start drops only JSONL Starts
/// (see `ToolEventKind`, #150).
///
// These reducer tuning constants are `pub` ONLY so the integration test
// (`tests/reducer.rs`, a separate crate) can derive its timing offsets from
// them instead of hardcoding ms. They are internal knobs, not a stable API:
// `#[doc(hidden)]` keeps the cross-crate visibility the test needs while
// excluding them from the rendered docs AND from cargo-semver-checks, so a
// future retune/rename is not a breaking change. (`EXIT_GRACE_WINDOW` below
// is deliberately NOT hidden — the binary's pose module is a real consumer.)
#[doc(hidden)]
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

/// How long to keep an exiting agent's slot alive after `SessionEnd` so the
/// walkout-to-door animation has time to play before the slot is removed.
pub const EXIT_GRACE_WINDOW: Duration = Duration::from_millis(4500);

/// How long a drained parent's b1 completion cascade is deferred before the
/// delegated subtree is marked exiting (#151). A parallel SECOND Task
/// dispatch arriving via hook is suppressed as a subagent leak and tracked
/// ONLY via its JSONL copy — if the FIRST Task's END drains `active_tasks`
/// while that copy is still in watcher latency, an immediate cascade would
/// evict the second Task's LIVE subtree, unrecoverably (`exiting_at` has no
/// clearer; after [`EXIT_GRACE_WINDOW`] the GC'd child's JSONL events no-op
/// forever). Any Task insert inside the grace cancels the pending cascade.
///
/// Sizing: ≥5× [`HOOK_WINS_WINDOW`] (the modeled hook↔JSONL skew) to cover
/// the FSEvents coalescing tail — the drain's own tool_result write triggers
/// the notify that replays the backlogged dispatch line, so one hop is all
/// the grace must cover; > [`ACTIVE_GRACE_WINDOW`] so b1 is not the
/// twitchiest timer in the system; < [`EXIT_GRACE_WINDOW`] so the added
/// linger stays visually dominated by the exit walk. Deliberately NOT sized
/// to the 60s scan_root poll backstop — covering that double-missed-notify
/// outlier would cost a minute's linger on EVERY completed delegation
/// (residual documented in #151).
#[doc(hidden)]
pub const B1_CASCADE_GRACE: Duration = Duration::from_millis(2500);

/// How long the slot stays visually Active after an `ActivityEnd` before
/// the reducer's tick flips it to Idle. Hides the per-tool-call Active
/// flicker that rapid PreToolUse → PostToolUse chains produce in CC; any
/// `ActivityStart` arriving within this window cancels the pending idle,
/// so the slot reads as continuously Active for chained tool work.
#[doc(hidden)]
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
#[doc(hidden)]
pub const STALE_ACTIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
#[doc(hidden)]
pub const STALE_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
#[doc(hidden)]
pub const STALE_WAITING_TIMEOUT: Duration = Duration::from_secs(60 * 60);
#[doc(hidden)]
pub const STALE_UNKNOWN_CWD_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// Idle timeout for sources with `SourceCaps::short_idle_reap()` — much
/// shorter than the generic [`STALE_IDLE_TIMEOUT`]. The capability is
/// `!has_exit_signal && resurrects_on_prompt`, and the motivating case is
/// **Codex**, which exposes **no session-end signal of any kind**: it has no
/// `SessionEnd` hook (its `HookEventName` enum has none — only `Stop`, which
/// is *turn* end), its payloads carry no PID, and its internal
/// `ShutdownComplete` event is not persisted to the rollout (so there is no
/// durable marker to tail-scan). All three were verified against upstream
/// `openai/codex`. The stale-sweep is therefore the ONLY reaper such a closed
/// session ever gets — at the 30-min generic timeout it lingers as a ghost
/// long after the process is gone.
///
/// The shorter window is safe specifically for this capability pair: the only
/// false-positive is a *live* session that sits idle between turns past the
/// threshold, and that is **self-healing** — its next `UserPromptSubmit`
/// re-emits `SessionStart` and the sprite walks back in. CC keeps the long
/// [`STALE_IDLE_TIMEOUT`]: it has real `SessionEnd` signals (best-effort
/// hook plus the durable `/exit` marker) for the common clean exit, so a
/// short reaper there would only evict genuinely live-but-idle sessions
/// (lunch-break idle) with no upside.
#[doc(hidden)]
pub const STALE_SHORT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The state-adaptive stale timeout for one slot. Unknown-cwd ghosts reap on the
/// shortest window (almost always startup-seeding artifacts). Otherwise the
/// timeout follows the activity state — with one carve-out: an idle slot whose
/// source has `caps.short_idle_reap()` (today: Codex — no exit signal of any
/// kind, so the sweep is its only reaper, AND the lone false positive — a
/// live-but-idle session past the window — self-heals on its next
/// `UserPromptSubmit`) uses [`STALE_SHORT_IDLE_TIMEOUT`] instead of the long
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
        // (in-process subagents that fire no hooks — reasonix is the first
        // such row) emits NOTHING until the dispatch tool's PostToolUse —
        // `last_event_at` freezes for the whole delegation, so a long
        // research/review run would be swept mid-turn on the Active timer.
        // Give it the Waiting-class window instead. (CC is immune by
        // construction: its subagents' misattributed hooks drive
        // `refresh_lineage`. The false-positive ghost self-heals on the next
        // UserPromptSubmit — same argument as the Codex idle carve-out.)
        ActivityState::Active { detail, .. }
            if caps.is_some_and(|c| c.delegations_are_hook_silent)
                && detail.as_deref() == Some(crate::source::ToolDetail::Task.display()) =>
        {
            STALE_WAITING_TIMEOUT
        }
        ActivityState::Active { .. } => STALE_ACTIVE_TIMEOUT,
        ActivityState::Idle if caps.is_some_and(|c| c.short_idle_reap()) => {
            STALE_SHORT_IDLE_TIMEOUT
        }
        ActivityState::Idle => STALE_IDLE_TIMEOUT,
        ActivityState::Waiting { .. } => STALE_WAITING_TIMEOUT,
    }
}

/// Display prefix for a source's labels (`cc·`, `ag·`, `cx·`, `rx·`), from the
/// source registry (the per-source fact table). Applied at `SessionStart`; the
/// JSONL `LabelDeriver` Renames (`cc_derive_label`/`derive_codex_label`/
/// `derive_ag_label`) produce the same prefixed string and so reinforce this
/// idempotently. A hook-only source (reasonix) has no JSONL Rename, so this is
/// the sole place its `rx·` label is established. An unregistered source falls
/// back to its own name (the same `other => other` contract as the old match).
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
    /// The value carries the recorded event's kind: the drop matrix is
    /// asymmetric — see [`ToolEventKind`]. A hook End overwrites its tool's
    /// Start entry (kind-in-the-VALUE, not the key), which is what lets one
    /// End record cover the tool's whole lagged JSONL pair.
    recent_hook_tool_uses: HashMap<(AgentId, String), (SystemTime, ToolEventKind)>,
    /// Per-agent set of Task tool_use_ids currently in flight. CC's hook
    /// payload sets `transcript_path` to the PARENT'S transcript even when a
    /// subagent is the actor, so subagent hook events hash to the parent's
    /// AgentId. While the parent has any Task in flight, hook
    /// ActivityStart/End events for that AgentId are dropped — JSONL has
    /// correct attribution to the subagent's own AgentId.
    active_tasks: HashMap<AgentId, HashSet<String>>,
    /// Parents whose last Task drained, awaiting the deferred b1 cascade
    /// ([`B1_CASCADE_GRACE`]): armed on drain, fired by
    /// `fire_pending_b1_cascades` on the tick/apply sweep path — but only if
    /// `active_tasks` is STILL empty at fire time, which is how a Task
    /// insert inside the grace (the suppressed parallel dispatch's JSONL
    /// copy — #151) defuses it. Evicted at BOTH sites like the other maps
    /// (tick's retain + `sweep_exited`'s remove).
    pending_b1_cascades: HashMap<AgentId, SystemTime>,
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
        self.fire_pending_b1_cascades(scene, now);
        self.sweep_stale(scene, now);
        // Clean up active_tasks entries for agents that never got a
        // SessionStart (Task event arrived before JSONL created the slot).
        self.active_tasks
            .retain(|id, _| scene.agents.contains_key(id));
        self.gated_before_waiting
            .retain(|id, _| scene.agents.contains_key(id));
        self.pending_b1_cascades
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
        // (2) Dedup before task tracking: a dropped JSONL duplicate must
        //     never reach the trackers or the main match — a duplicate Task
        //     dispatch reaching the tracker would re-fire enter_delegating
        //     and clobber a Waiting parent (pinned by
        //     `jsonl_task_start_duplicate_does_not_clobber_waiting_parent`).
        //     The drop itself is kind-ASYMMETRIC (#150): a Start record
        //     never eats a JSONL End — when the PostToolUse hook drops, that
        //     JSONL End is the only completion signal left, and a Task
        //     self-End that gets eaten leaks `active_tasks` for the rest of
        //     the session (pinned by
        //     `jsonl_task_self_end_drains_when_hook_end_drops`).
        if from == Transport::Hook && self.suppress_subagent_leak(scene, &event, id, now) {
            return;
        }

        // Dedup: drop JSONL events that match a recent Hook event by
        // tool_use_id — except a JSONL End against a Start-only record (the
        // asymmetric matrix; see `ToolEventKind`).
        if from == Transport::Jsonl {
            if let Some((kind, tuid)) = event_tool_use_id(&event) {
                if let Some((_, recorded)) = self.recent_hook_tool_uses.get(&(id, tuid.to_string()))
                {
                    if !(*recorded == ToolEventKind::Start && kind == ToolEventKind::End) {
                        return;
                    }
                }
            }
        }

        if from == Transport::Hook {
            if let Some((kind, tuid)) = event_tool_use_id(&event) {
                self.recent_hook_tool_uses
                    .insert((id, tuid.to_string()), (now, kind));
            }
        }

        let TaskTracking {
            handled_by_task_tracking,
            handled_by_task_start,
        } = self.track_active_tasks(scene, &event, now);

        // Fire due b1 cascades AFTER task tracking, not at apply-top: a
        // canceling Task dispatch arriving exactly at the grace boundary must
        // land in `active_tasks` before the due-check, or the fire would
        // evict the live subtree in the very apply call that carries its
        // cancel. An entry armed by THIS event's own drain has zero elapsed
        // time, so it can never self-fire.
        self.fire_pending_b1_cascades(scene, now);

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
                    // A duplicate SessionStart is still a genuine liveness
                    // signal from the session (Codex/Reasonix re-emit one per
                    // UserPromptSubmit) — refresh it so a prompt landing just
                    // under the stale threshold pushes the boundary out instead
                    // of losing the race to the sweep mid-turn.
                    slot.last_event_at = now;
                    // Resurrect-in-place: a SessionStart on an EXITING slot
                    // means the session lives — Reasonix's `/new` fires
                    // SessionEnd+SessionStart back-to-back on the SAME
                    // cwd-keyed id, and a Codex resurrect prompt can land
                    // inside the 4.5s walkout window. Without this the new
                    // session's start is swallowed and the whole first turn is
                    // invisible (every later arm is a no-op once the corpse is
                    // GC'd). Gated to root agents on BOTH sides so a late
                    // duplicate can't un-exit a b1-cascaded subagent.
                    if slot.exiting_at.is_some() && slot.parent_id.is_none() && parent_id.is_none()
                    {
                        slot.exiting_at = None;
                        slot.pending_idle_at = None;
                        slot.state = ActivityState::Idle;
                        slot.state_started_at = now;
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
                    //
                    // A None-id ActivityEnd ON THE HOOK TRANSPORT is a turn-end
                    // signal (Codex/Reasonix `Stop`; CC hook ends always carry
                    // ids), and a pending approval BLOCKS those CLIs' turns —
                    // so a slot still Waiting when Stop arrives can only be a
                    // stale (denied/abandoned) prompt. Resolve it rather than
                    // ghosting "waiting" until the 60-min sweep; Reasonix has
                    // no second transport to self-heal this. The Hook gate is
                    // load-bearing: Codex's JSONL emits None-id ends per tool
                    // (it opts out of dedup), and one can race in AFTER a fresh
                    // PermissionRequest — a JSONL None-id end must keep the
                    // prompt up, same as the parallel-tool protection above.
                    let is_waiting = matches!(
                        scene.agents.get(&agent_id).map(|s| &s.state),
                        Some(ActivityState::Waiting { .. })
                    );
                    let resolves_wait = is_waiting
                        && match tool_use_id.as_deref() {
                            Some(tuid) => {
                                self.gated_before_waiting.get(&agent_id).map(|g| &**g) == Some(tuid)
                            }
                            None => from == Transport::Hook,
                        };
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
    /// 30-min idle stale-sweep. The cascade is DEFERRED by
    /// [`B1_CASCADE_GRACE`] and canceled by any Task insert (#151): a
    /// suppressed parallel dispatch is tracked only via its JSONL copy, and
    /// an immediate cascade would evict its live subtree while that copy is
    /// still in watcher latency. CC infers completion from the Task drain
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
                        // #152: the gate recorded while Delegating holds THIS
                        // Task's tuid; the drain path deliberately skips the
                        // main arm (a Waiting parent must keep Waiting), so
                        // the entry would go stale — and a later
                        // out-of-window JSONL replay of this END would
                        // false-match it via resolves_wait and flip a
                        // still-pending permission to Idle. Clear only OUR
                        // tuid: a parallel ordinary tool's gate must survive
                        // the drain.
                        if self.gated_before_waiting.get(agent_id).map(|g| &**g)
                            == Some(tuid.as_str())
                        {
                            self.gated_before_waiting.remove(agent_id);
                        }
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
                                // Parent's last Task returned → arm the
                                // deferred b1 cascade (#151); it fires after
                                // B1_CASCADE_GRACE unless a Task insert
                                // cancels it.
                                self.pending_b1_cascades.insert(*agent_id, now);
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

        TaskTracking {
            handled_by_task_tracking,
            handled_by_task_start,
        }
    }

    /// Fire deferred b1 cascades whose [`B1_CASCADE_GRACE`] elapsed (#151).
    /// Runs on the tick/apply sweep path like the other expiries. The
    /// fire-time emptiness check IS the cancel mechanism: a Task insert any
    /// time inside the grace (e.g. the suppressed parallel dispatch's JSONL
    /// copy) re-populates `active_tasks`, so the due entry is discarded
    /// instead of fired — no separate cancel-on-insert bookkeeping to drift
    /// out of sync with the ledger.
    fn fire_pending_b1_cascades(&mut self, scene: &mut SceneState, now: SystemTime) {
        let due: Vec<AgentId> = self
            .pending_b1_cascades
            .iter()
            .filter(|(_, armed)| {
                now.duration_since(**armed)
                    .is_ok_and(|d| d >= B1_CASCADE_GRACE)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.pending_b1_cascades.remove(&id);
            if self.active_tasks.get(&id).is_some_and(|s| !s.is_empty()) {
                continue;
            }
            tracing::debug!(agent_id = ?id, "b1 grace elapsed — cascading completed subtree");
            scope::cascade_exit(scene, id, now);
        }
    }

    fn gc(&mut self, now: SystemTime) {
        // SystemTime::duration_since returns Err when `ts` is in the future
        // (clock went backwards). Drop those — stale entries either way.
        self.recent_hook_tool_uses
            .retain(|_, (ts, _)| now.duration_since(*ts).is_ok_and(|d| d < HOOK_WINS_WINDOW));
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
            self.pending_b1_cascades.remove(&id);
        }
    }
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
enum ToolEventKind {
    Start,
    End,
}

fn event_tool_use_id(ev: &AgentEvent) -> Option<(ToolEventKind, &str)> {
    match ev {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            tool_use_id.as_deref().map(|t| (ToolEventKind::Start, t))
        }
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            tool_use_id.as_deref().map(|t| (ToolEventKind::End, t))
        }
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

    // Accepted-equivalent mutation residuals (cargo-mutants, state files):
    // three boundary flips survive deliberately — `< → <=` in `gc`'s dedup
    // retain (reducer.rs:659), `> → >=` in `sweep_stale` (716) and
    // `sweep_exited` (767). Each only changes behavior at the EXACT threshold
    // instant (age == timeout, to the nanosecond), a measure-zero event in
    // wall-clock time and immaterial to a stale-sweep (one tick either way).
    // Pinning them needs a hand-built exact-boundary SystemTime, which is
    // brittle for no product value — left as documented equivalents, not gaps.

    /// Pin the deliberate stale-timeout DURATIONS. Every timing test correctly
    /// derives its offsets FROM these constants (hardcoded ms make leg tests
    /// vacuous), so mutating `10 * 60` also mutates each test's own
    /// expectation — leaving the literal value unguarded. A direct pin is the
    /// only thing that catches `*`→`/` collapsing a window to 0s (everything
    /// reaped on the next tick) or a typo'd minute count. The values ARE the
    /// product decision (see the doc comments on each const); change this test
    /// deliberately when a window changes, never to make it pass.
    #[test]
    fn stale_timeout_constants_have_their_intended_durations() {
        use super::{
            STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT, STALE_SHORT_IDLE_TIMEOUT,
            STALE_UNKNOWN_CWD_TIMEOUT, STALE_WAITING_TIMEOUT,
        };
        use std::time::Duration;
        assert_eq!(STALE_ACTIVE_TIMEOUT, Duration::from_secs(600)); // 10 min
        assert_eq!(STALE_IDLE_TIMEOUT, Duration::from_secs(1800)); // 30 min
        assert_eq!(STALE_WAITING_TIMEOUT, Duration::from_secs(3600)); // 60 min
        assert_eq!(STALE_UNKNOWN_CWD_TIMEOUT, Duration::from_secs(180)); // 3 min
        assert_eq!(STALE_SHORT_IDLE_TIMEOUT, Duration::from_secs(300)); // 5 min
    }

    // The Delegating stale carve-out is caps-driven; pin the POLICY half with
    // a synthetic caps value so caps combinations beyond the registered rows
    // stay covered — that's what the lookup/policy split exists for. (The
    // registered path — reasonix is the row that sets
    // `delegations_are_hook_silent` — is pinned end-to-end by
    // `reasonix_delegating_slot_survives_the_active_timeout` in
    // tests/reducer.rs.)
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
        use crate::source::{AgentEvent, ToolDetail, Transport};
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
