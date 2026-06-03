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
pub const STALE_CODEX_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The state-adaptive stale timeout for one slot. Unknown-cwd ghosts reap on the
/// shortest window (almost always startup-seeding artifacts). Otherwise the
/// timeout follows the activity state — with one carve-out: an idle **Codex**
/// slot uses [`STALE_CODEX_IDLE_TIMEOUT`] (not the long [`STALE_IDLE_TIMEOUT`])
/// because Codex exposes no exit signal of any kind, so the sweep is its only
/// reaper; the lone false positive (a live-but-idle Codex past the window)
/// self-heals on its next `UserPromptSubmit`. CC keeps the long window — its
/// real `SessionEnd` signals make a short reaper all cost, no benefit.
fn stale_threshold(slot: &AgentSlot) -> Duration {
    if slot.unknown_cwd {
        return STALE_UNKNOWN_CWD_TIMEOUT;
    }
    match &slot.state {
        ActivityState::Active { .. } => STALE_ACTIVE_TIMEOUT,
        ActivityState::Idle if slot.source.as_ref() == crate::source::codex::SOURCE_NAME => {
            STALE_CODEX_IDLE_TIMEOUT
        }
        ActivityState::Idle => STALE_IDLE_TIMEOUT,
        ActivityState::Waiting { .. } => STALE_WAITING_TIMEOUT,
    }
}

/// Display prefix for a source's labels (`cc·`, `ag·`, `cx·`). Single source of
/// truth applied at `SessionStart`. The JSONL `LabelDeriver` Renames (e.g.
/// `cc_derive_label`/`derive_ag_label`) produce the same prefixed string and so
/// reinforce this idempotently; Codex arrives only via the shared hook socket
/// (no JSONL Rename), so this is the sole place its `cx·` label is established.
fn source_label_prefix(source: &str) -> &str {
    match source {
        crate::source::claude_code::SOURCE_NAME => "cc",
        crate::source::antigravity::SOURCE_NAME => "ag",
        crate::source::codex::SOURCE_NAME => "cx",
        other => other,
    }
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
        if matches!(
            &event,
            AgentEvent::ActivityStart { .. }
                | AgentEvent::ActivityEnd { .. }
                | AgentEvent::Waiting { .. }
        ) {
            scope::refresh_lineage(scene, id, now);
        }

        // Subagent-leak suppression: if this AgentId currently has any Task
        // tool in flight, hook ActivityStart/End events for it are almost
        // certainly subagent work misattributed to the parent. Drop them and
        // defer to JSONL, which targets the subagent's own AgentId. The
        // Task's own PostToolUse is exempt — its tool_use_id matches one we
        // are tracking, so it passes through and clears the slot.
        if from == Transport::Hook {
            let in_task = self.active_tasks.get(&id).is_some_and(|s| !s.is_empty());
            let suppress = match &event {
                AgentEvent::ActivityStart { .. } => in_task,
                AgentEvent::ActivityEnd { tool_use_id, .. } => {
                    let is_task_self_end = tool_use_id
                        .as_ref()
                        .is_some_and(|t| self.active_tasks.get(&id).is_some_and(|s| s.contains(t)));
                    in_task && !is_task_self_end
                }
                _ => false,
            };
            if suppress {
                // The misattributed subagent event already refreshed the
                // parent's lineage above (liveness flows up), keeping the
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
                let task_tuid = self
                    .active_tasks
                    .get(&id)
                    .and_then(|s| s.iter().next())
                    .map(|t| Arc::<str>::from(t.as_str()));
                if let Some(slot) = scene.agents.get_mut(&id) {
                    if matches!(slot.state, ActivityState::Waiting { .. }) {
                        fsm::enter_delegating(slot, task_tuid, now);
                        self.gated_before_waiting.remove(&id);
                    }
                }
                return;
            }
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

        // Track active Task tool_use_ids from either transport. HashSet is
        // idempotent so duplicate inserts from both hook+jsonl are harmless.
        //
        // Side effect: when the parent gains a Task, also mark it as
        // Active("Delegating") so it doesn't look idle/asleep while its
        // subagents do the visible work. When the last Task drains, the
        // next normal hook/JSONL event will reset its state.
        //
        // `handled_by_task_tracking`: when an ActivityEnd drains
        // active_tasks, the general ActivityEnd arm below must be
        // skipped — otherwise it would redundantly re-arm
        // pending_idle_at or arm it while tasks are still in flight.
        let mut handled_by_task_tracking = false;
        let mut handled_by_task_start = false;
        // b1 subagent-completion inference (CC writes no completion marker): set
        // when the parent's LAST Task drains on this event, so the delegated
        // subtree can be marked exiting below.
        let mut completed_subtree_root: Option<AgentId> = None;
        match &event {
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

        // b1: a drained parent Task means the delegated subtree returned —
        // cascade EXIT to the parent's descendants (not the parent, which keeps
        // running) so completed subagents leave promptly instead of lingering to
        // the 30-min idle stale-sweep. CC infers completion from the Task drain
        // here; a source with a clean "subagent finished" signal (e.g. Codex)
        // would drive the same cascade through its own decoder.
        if let Some(parent) = completed_subtree_root {
            scope::cascade_exit(scene, parent, now);
        }

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
                self.next_label_n += 1;
                let base = cwd
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty());
                let has_cwd = base.is_some();
                let prefix = source_label_prefix(&source);
                let label: Arc<str> = match base {
                    Some(b) => Arc::<str>::from(format!("{prefix}·{b}").as_str()),
                    None => Arc::<str>::from(format!("{prefix}#{}", self.next_label_n).as_str()),
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

    /// Every registered source needs an explicit 2-char prefix arm. The
    /// `other => other` catch-all silently degrades a missing arm to the long
    /// source name (e.g. "opencode·proj" instead of "oc·proj"), which then
    /// collides visually with another source sharing a cwd. Driven by the same
    /// REGISTERED_SOURCES list as the fixture conformance test.
    #[test]
    fn every_registered_source_has_two_char_label_prefix() {
        for src in REGISTERED_SOURCES {
            let prefix = source_label_prefix(src);
            assert_eq!(
                prefix.chars().count(),
                2,
                "source {src:?} has no 2-char label prefix (got {prefix:?}) — add an arm to source_label_prefix"
            );
        }
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
