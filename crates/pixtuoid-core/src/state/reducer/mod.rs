use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::source::decoder::label_prefix_for;
use crate::source::{AgentEvent, Transport};
use crate::state::correlation::{elapsed_at_least, elapsed_past, Correlation, ToolEventKind};
use crate::state::{fsm, scope, ActivityState, AgentSlot, SceneState, ToolKind};
use crate::AgentId;

#[doc(hidden)]
pub use crate::state::correlation::{
    CHILD_END_LEDGER_TTL, CHILD_END_RELINK_TTL, DRAINED_TASK_TOMBSTONE_TTL,
    HOOK_SESSION_END_TOMBSTONE_TTL, HOOK_WINS_WINDOW, PROOF_OF_LIFE_TTL,
};

/// How long to keep an exiting agent's slot alive after `SessionEnd` so the
/// walkout-to-door animation has time to play before the slot is removed.
pub const EXIT_GRACE_WINDOW: Duration = Duration::from_millis(4500);

/// How long a drained parent's b1 completion cascade is deferred before the
/// delegated subtree is marked exiting (#151). A parallel SECOND Task dispatch
/// is suppressed as a subagent leak and tracked ONLY via its JSONL copy, so an
/// immediate cascade would evict that still-live subtree unrecoverably —
/// `exiting_at` has no clearer. Any Task insert inside the grace cancels the
/// pending cascade. Sized to one FSEvents coalescing hop, deliberately NOT to
/// the 60s scan_root poll backstop, which would cost a minute's linger on
/// EVERY completed delegation.
#[doc(hidden)]
pub const B1_CASCADE_GRACE: Duration = Duration::from_millis(2500);

/// How long the slot stays visually Active after an `ActivityEnd` before the
/// reducer's tick flips it to Idle — hides the per-tool-call Active flicker
/// that rapid PreToolUse → PostToolUse chains produce. Any `ActivityStart`
/// inside the window cancels the pending idle.
#[doc(hidden)]
pub const ACTIVE_GRACE_WINDOW: Duration = Duration::from_millis(1500);

/// State-adaptive stale-agent thresholds: if `now - last_event_at` exceeds the
/// threshold for the agent's current state, the reducer marks it exiting. The
/// spread is deliberate — Active silence means the process died mid-tool, while
/// Idle and Waiting must not reap a human on a break or reading a permission
/// prompt, and an unknown-cwd slot is almost always a startup-seeding ghost
/// whose false-positive cost is one freed desk.
#[doc(hidden)]
pub const STALE_ACTIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
#[doc(hidden)]
pub const STALE_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
#[doc(hidden)]
pub const STALE_WAITING_TIMEOUT: Duration = Duration::from_secs(60 * 60);
#[doc(hidden)]
pub const STALE_UNKNOWN_CWD_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// Idle timeout for sources with `SourceCaps::short_idle_reap()`
/// (`!has_exit_signal && resurrects_on_prompt`). Codex is the motivating case:
/// its `SessionEnd` hook covers only graceful teardown, its payloads carry no
/// PID, and its `ShutdownComplete` is not persisted to the rollout, so the
/// stale-sweep is the only reaper an abruptly-closed session gets. Safe only
/// for this capability pair, because the lone false-positive — a live session
/// idle past the threshold — self-heals on its next `UserPromptSubmit`.
#[doc(hidden)]
pub const STALE_SHORT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

fn stale_threshold(slot: &AgentSlot) -> Duration {
    stale_threshold_with_caps(
        slot,
        crate::source::registry::descriptor_for(&slot.source).map(|d| d.caps()),
    )
}

/// Policy half of [`stale_threshold`], split from the registry lookup so caps
/// combinations no registered source has YET are unit-testable with a synthetic
/// [`SourceCaps`].
fn stale_threshold_with_caps(
    slot: &AgentSlot,
    caps: Option<crate::source::registry::SourceCaps>,
) -> Duration {
    if slot.unknown_cwd {
        return STALE_UNKNOWN_CWD_TIMEOUT;
    }
    match &slot.state {
        // A Delegating slot on a source whose delegations are hook-silent
        // (in-process subagents that fire no hooks) emits NOTHING until the
        // dispatch tool's PostToolUse — `last_event_at` freezes for the whole
        // delegation, so a long run would be swept mid-turn on the Active
        // timer. Keyed on the typed `ToolKind::Task`, never the display string:
        // a Generic tool spelling "Delegating" is not a delegation.
        ActivityState::Active {
            kind: ToolKind::Task,
            ..
        } if caps.is_some_and(|c| c.delegations_are_hook_silent) => STALE_WAITING_TIMEOUT,
        ActivityState::Active { .. } => STALE_ACTIVE_TIMEOUT,
        ActivityState::Idle if caps.is_some_and(|c| c.short_idle_reap()) => {
            STALE_SHORT_IDLE_TIMEOUT
        }
        ActivityState::Idle => STALE_IDLE_TIMEOUT,
        ActivityState::Waiting { .. } => STALE_WAITING_TIMEOUT,
    }
}

/// Classify an incoming `Rename` label's provenance: exactly the registry
/// prefix (a `LabelDeriver`'s empty-cwd fallback) is a
/// [`LabelProvenance::PrefixFallback`] the back-fill may still upgrade;
/// anything else is a real display name. Judged HERE at the mint, not at
/// back-fill time — a bare-prefix Rename always lands on a slot whose source is
/// already set, so the slot's prefix is the right yardstick.
fn classify_rename(label: &str, source: &str) -> crate::state::SlotLabel {
    let prefix = label_prefix_for(source);
    if !prefix.is_empty() && label == prefix {
        crate::state::SlotLabel::prefix_fallback(label)
    } else {
        crate::state::SlotLabel::renamed(label)
    }
}

/// Borrowed identity context threaded into slot registration/back-fill, bundled
/// so the two `&str`s (`source`/`session_id`, positionally interchangeable)
/// can't be silently swapped at a call site.
#[derive(Clone, Copy)]
struct IdentityCtx<'a> {
    source: &'a str,
    session_id: &'a str,
    cwd: &'a std::path::Path,
}

/// First-wins identity back-fill (#221): heal EMPTY source/session_id/cwd — an
/// established value is never overwritten. Returns the healed cwd's basename
/// when THIS call healed the cwd; only the SessionStart arm upgrades a fallback
/// label from it, since `Identity` carries no label authority.
fn backfill_identity<'a>(slot: &mut AgentSlot, ctx: IdentityCtx<'a>) -> Option<&'a str> {
    if slot.source.is_empty() && !ctx.source.is_empty() {
        slot.source = Arc::<str>::from(ctx.source);
    }
    if slot.session_id.is_empty() && !ctx.session_id.is_empty() {
        slot.session_id = Arc::<str>::from(ctx.session_id);
    }
    if slot.unknown_cwd || slot.cwd.as_os_str().is_empty() {
        if let Some(base) = ctx
            .cwd
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.is_empty())
        {
            slot.cwd = Arc::<std::path::Path>::from(ctx.cwd);
            slot.unknown_cwd = false;
            return Some(base);
        }
    }
    None
}

/// Verdict of [`Reducer::preprocess`]. `#[must_use]` because every `Drop` arm
/// exists to STOP the event reaching the dispatch match — silently ignoring one
/// re-opens the leak or the double-apply the pre-pass was added to close.
#[must_use]
enum Preprocessed {
    /// Suppressed as a subagent leak, or dropped by hook-wins dedup.
    Drop,
    /// Survived the pre-pass; carries what task tracking already applied, so
    /// the general arms can skip work the tracker has done.
    Dispatch(TaskTracking),
}

struct TaskTracking {
    /// An `ActivityEnd` drained a tracked Task: the general ActivityEnd arm
    /// must be skipped, or it would arm `pending_idle_at` while tasks are
    /// still in flight.
    handled_by_task_tracking: bool,
    /// An `ActivityStart` dispatched a Task (already applied as
    /// Active(Delegating) by the pre-pass): the general arm must be skipped.
    handled_by_task_start: bool,
}

/// The event coordinator: folds `AgentEvent`s into `SceneState`, owning the
/// cross-slot correlation maps, the Active→Idle debounce, and the stale/exit
/// sweeps.
#[derive(Debug, Default)]
pub struct Reducer {
    corr: Correlation,
    /// Parents whose last Task drained, awaiting the deferred b1 cascade
    /// ([`B1_CASCADE_GRACE`]) — fired only if `active_tasks` is STILL empty at
    /// fire time, which is how a Task insert inside the grace defuses it
    /// (#151).
    pending_b1_cascades: HashMap<AgentId, SystemTime>,

    next_label_n: u32,
}

impl Reducer {
    /// A reducer with empty correlation state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the GC + exit-sweep + Active→Idle debounce expiry without applying
    /// an event. Must be called periodically so exiting slots are reclaimed and
    /// pending-idle timers fire even when no event arrives to drive `apply`.
    pub fn tick(&mut self, scene: &mut SceneState, now: SystemTime) {
        self.corr.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        self.fire_pending_b1_cascades(scene, now);
        self.sweep_stale(scene, now);
        // Reap entries for agents that never got a SessionStart — a Task event
        // can arrive before JSONL creates the slot.
        self.corr
            .active_tasks
            .retain(|id, _| scene.agents.contains_key(id));
        self.corr
            .gated_before_waiting
            .retain(|id, _| scene.agents.contains_key(id));
        self.pending_b1_cascades
            .retain(|id, _| scene.agents.contains_key(id));
    }

    /// Reconcile the scene toward the `connected` set: mark exiting every
    /// non-exiting slot whose source is NOT connected, then cascade to its
    /// subtree. Driven by the Sources panel's DISCONNECT toggle — an explicit
    /// user action, not transcript content, so it does NOT violate the "content
    /// never drives lifecycle" invariant.
    ///
    /// Keying on the COMPLEMENT of the connected set rather than iterating a
    /// known source list is load-bearing: it also evicts a BLANK-source slot
    /// synthesized for an identity-less hook event that slipped the per-event
    /// gate. Already-exiting slots are left untouched so a re-reconcile can't
    /// reset their walkout clock.
    pub fn reconcile_connected(
        &self,
        scene: &mut SceneState,
        connected: &HashSet<String>,
        now: SystemTime,
    ) {
        let ids: Vec<AgentId> = scene
            .agents
            .values()
            .filter(|s| s.exiting_at.is_none() && !connected.contains(s.source.as_ref()))
            .map(|s| s.agent_id)
            .collect();
        for id in ids {
            scope::cascade_exit(scene, id, scope::StampRoot::Yes, now);
        }
    }

    /// Fold one `AgentEvent` into the scene at `now`, tagged with the
    /// `Transport` it arrived on (load-bearing for hook-wins dedup).
    pub fn apply(
        &mut self,
        scene: &mut SceneState,
        event: AgentEvent,
        now: SystemTime,
        from: Transport,
    ) {
        let id = event.agent_id();
        let Preprocessed::Dispatch(tracking) = self.preprocess(scene, &event, id, now, from) else {
            return;
        };

        match event {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => self.apply_session_start(
                scene,
                agent_id,
                IdentityCtx {
                    source: &source,
                    session_id: &session_id,
                    cwd: &cwd,
                },
                parent_id,
                now,
            ),
            AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail,
            } => self.apply_activity_start(
                scene,
                agent_id,
                tool_use_id,
                detail,
                tracking.handled_by_task_start,
                now,
            ),
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            } => self.apply_activity_end(
                scene,
                agent_id,
                tool_use_id.as_deref(),
                tracking.handled_by_task_tracking,
                from,
                now,
            ),
            AgentEvent::Waiting { agent_id, reason } => {
                self.apply_waiting(scene, agent_id, &reason, now);
            }
            AgentEvent::Rename { agent_id, label } => {
                Self::apply_rename(scene, agent_id, &label, now);
            }
            AgentEvent::SessionEnd { agent_id, as_child } => {
                self.apply_session_end(scene, agent_id, as_child, now);
            }
            AgentEvent::ProofOfLife { agent_id } => {
                self.apply_proof_of_life(scene, agent_id, now);
            }
            AgentEvent::Identity {
                agent_id,
                source,
                session_id,
                cwd,
                pid,
            } => {
                // Defaulted HERE rather than in the handler because
                // `IdentityCtx` BORROWS the path, mirroring the `SessionStart`
                // arm above.
                let cwd = cwd.as_deref().unwrap_or_else(|| std::path::Path::new(""));
                self.apply_identity(
                    scene,
                    agent_id,
                    IdentityCtx {
                        source: &source,
                        session_id: &session_id,
                        cwd,
                    },
                    pid,
                    from,
                    now,
                );
            }
            AgentEvent::ModelInfo {
                agent_id,
                model,
                effort,
            } => Self::apply_model_info(scene, agent_id, model.as_deref(), effort.as_deref(), now),
            AgentEvent::Usage {
                agent_id,
                fresh_tokens,
            } => Self::apply_usage(scene, agent_id, fresh_tokens, now),
        }
    }

    /// The pre-pass every event runs before dispatch: GC + sweeps, hook
    /// proof-of-life synthesis, lineage refresh, subagent-leak suppression,
    /// hook-wins dedup, task tracking, and the deferred b1 cascade.
    ///
    /// ORDER IS LOAD-BEARING throughout; the WHY of each step is at its own
    /// site below. Lifted whole out of [`Reducer::apply`] so the match stays a
    /// dispatch — move-only.
    fn preprocess(
        &mut self,
        scene: &mut SceneState,
        event: &AgentEvent,
        id: AgentId,
        now: SystemTime,
        from: Transport,
    ) -> Preprocessed {
        self.corr.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);

        // PRE-PASS 0 — a hook event is PROOF OF LIFE: it can only come from a
        // live process, so a hook tool/permission event whose id has no slot
        // means a live session is invisible (its transcript was gated at first
        // sight, or it is parked on a permission prompt that appends nothing).
        // Synthesize the registration the missing SessionStart would have
        // performed. JSONL must NOT synthesize — a transcript line can be a
        // historical replay, so the unknown-id no-op stays load-bearing there.
        if from == Transport::Hook {
            // A SessionEnd for an UNKNOWN id tombstones it: the session ended
            // while invisible, and a reordered trailing event from the same
            // dying session must not resurrect it through the synthesis below.
            if matches!(event, AgentEvent::SessionEnd { .. }) && !scene.agents.contains_key(&id) {
                self.corr.recent_hook_session_ends.insert(id, now);
            }
            self.synthesize_hook_registration(scene, event, id, now);
        }

        // Liveness flows UP the tree: any activity by a descendant keeps its
        // ancestors alive, so a parent isn't stale-swept while a subagent is
        // still working. The mirror of `cascade_exit`, which pushes EXIT down.
        // `refresh_lineage` stamps the ACTOR too, so the per-arm
        // `last_event_at = now` writes below are redundant for these three
        // events — but NOT for Rename or the SessionStart-enrich path, which
        // never reach here, so don't drop them.
        if matches!(
            event,
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
        //     copy — the only transport left to track that Task.
        // (2) Dedup before task tracking: a duplicate Task dispatch reaching
        //     the tracker would re-fire enter_delegating and clobber a Waiting
        //     parent. The drop is kind-ASYMMETRIC (#150): a Start record never
        //     eats a JSONL End — when the PostToolUse hook drops, that End is
        //     the only completion signal left, and eating it leaks
        //     `active_tasks` for the rest of the session.
        if from == Transport::Hook && self.suppress_subagent_leak(scene, event, id, now) {
            return Preprocessed::Drop;
        }

        if from == Transport::Jsonl {
            if let Some((kind, tuid)) = event_tool_use_id(event) {
                if let Some((_, recorded)) =
                    self.corr.recent_hook_tool_uses.get(&(id, tuid.to_string()))
                {
                    if !(*recorded == ToolEventKind::Start && kind == ToolEventKind::End) {
                        return Preprocessed::Drop;
                    }
                }
            }
        }

        // Gated on the slot EXISTING (post-synthesis): when synthesis was
        // REFUSED (desk exhaustion) the event applies to nothing, but its
        // record would outlive the refusal — and a desk freeing within
        // HOOK_WINS_WINDOW would let that stale record dedup-eat the
        // ActivityStart of the JSONL registration that follows.
        if from == Transport::Hook && scene.agents.contains_key(&id) {
            if let Some((kind, tuid)) = event_tool_use_id(event) {
                self.corr
                    .recent_hook_tool_uses
                    .insert((id, tuid.to_string()), (now, kind));
            }
        }

        let tracking = self.track_active_tasks(scene, event, now);

        // AFTER task tracking, not at apply-top: a canceling Task dispatch
        // arriving exactly at the grace boundary must land in `active_tasks`
        // before the due-check, or the fire would evict the live subtree in the
        // very apply call that carries its cancel.
        self.fire_pending_b1_cascades(scene, now);

        Preprocessed::Dispatch(tracking)
    }

    /// The `ActivityStart` arm. Skipped entirely when the pre-pass tracker
    /// already applied this event as a Task dispatch.
    fn apply_activity_start(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        tool_use_id: Option<String>,
        detail: Option<crate::source::ToolDetail>,
        handled_by_task_start: bool,
        now: SystemTime,
    ) {
        if handled_by_task_start {
            return;
        }
        // Resuming to Active makes any pending gated-permission correlation
        // moot.
        self.corr.gated_before_waiting.remove(&agent_id);
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            if !detail.as_ref().is_some_and(|d| d.is_task()) {
                slot.tool_call_count += 1;
            }
            // Derive the category from the typed detail BEFORE it is erased to
            // the HUD display string.
            let kind = detail
                .as_ref()
                .map_or(ToolKind::Other, ToolKind::from_detail);
            fsm::enter_active(
                slot,
                tool_use_id.map(|s| Arc::<str>::from(s.as_str())),
                detail.map(|d| Arc::<str>::from(d.display())),
                kind,
                now,
            );
        }
    }

    /// Whether this `ActivityEnd` resolves the slot's pending permission Wait.
    ///
    /// A CC permission's *gated* tool finishing resolves the Wait: its
    /// tool_use_id matches the one that was Active when Waiting began, so a
    /// parallel tool ending (different id) can't false-clear a still-pending
    /// permission.
    ///
    /// A None-id `ActivityEnd` ON THE HOOK TRANSPORT is a turn-end signal
    /// (Codex/Reasonix `Stop`), and a pending approval BLOCKS those CLIs'
    /// turns — so a slot still Waiting when Stop arrives can only be a stale
    /// prompt. The Hook gate is load-bearing: Codex's JSONL emits None-id ends
    /// per tool, and one racing in after a fresh PermissionRequest must keep
    /// the prompt up.
    fn wait_resolved_by(
        &self,
        scene: &SceneState,
        agent_id: AgentId,
        tool_use_id: Option<&str>,
        from: Transport,
    ) -> bool {
        let is_waiting = matches!(
            scene.agents.get(&agent_id).map(|s| &s.state),
            Some(ActivityState::Waiting { .. })
        );
        is_waiting
            && match tool_use_id {
                Some(tuid) => {
                    self.corr.gated_before_waiting.get(&agent_id).map(|g| &**g) == Some(tuid)
                }
                None => from == Transport::Hook,
            }
    }

    /// The `ActivityEnd` arm. Skipped entirely when the pre-pass tracker
    /// already drained a tracked Task with this event — arming
    /// `pending_idle_at` while tasks are still in flight would settle a
    /// delegating parent to Idle.
    fn apply_activity_end(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        tool_use_id: Option<&str>,
        handled_by_task_tracking: bool,
        from: Transport,
        now: SystemTime,
    ) {
        if handled_by_task_tracking {
            return;
        }
        let resolves_wait = self.wait_resolved_by(scene, agent_id, tool_use_id, from);
        if resolves_wait {
            self.corr.gated_before_waiting.remove(&agent_id);
        }
        // While the agent is still DELEGATING, its own parallel tool ending
        // must not settle it to Idle — nothing would restore the Delegating
        // display for the rest of the delegation, so the parent would render
        // asleep while its subagents do the visible work. Re-enter Delegating.
        let delegating_tuid = self.corr.any_active_task(&agent_id);
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            if matches!(slot.state, ActivityState::Active { .. }) || resolves_wait {
                match delegating_tuid {
                    Some(tuid) => fsm::enter_delegating(slot, Some(tuid), now),
                    None => fsm::arm_pending_idle(slot, now),
                }
            }
            slot.last_event_at = now;
        }
    }

    /// The `Waiting` arm.
    fn apply_waiting(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        reason: &str,
        now: SystemTime,
    ) {
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            // Remember the mid-flight tool so its later PostToolUse (same
            // tool_use_id) can resolve this permission Wait.
            if let ActivityState::Active {
                tool_use_id: Some(tuid),
                ..
            } = &slot.state
            {
                self.corr
                    .gated_before_waiting
                    .insert(agent_id, tuid.clone());
            } else {
                self.corr.gated_before_waiting.remove(&agent_id);
            }
            fsm::enter_waiting(slot, Arc::<str>::from(reason), now);
        }
    }

    /// The `Rename` arm.
    fn apply_rename(scene: &mut SceneState, agent_id: AgentId, label: &str, now: SystemTime) {
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            let label = classify_rename(label, &slot.source);
            fsm::rename(slot, label, now);
        }
    }

    /// The `SessionEnd` arm.
    fn apply_session_end(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        as_child: bool,
        now: SystemTime,
    ) {
        // Stamped REGARDLESS of slot existence (#244): a Stop-before-Start
        // reorder has no slot, yet its `ended_at` must arm the parented gate.
        // For a KNOWN slot this stamp outlives the exit-grace GC, covering the
        // late-first-sight window the #242 tombstone structurally can't.
        if as_child {
            self.corr.child_ledger.entry(agent_id).or_default().ended_at = Some(now);
        }
        scope::cascade_exit(scene, agent_id, scope::StampRoot::Yes, now);
    }

    /// The `ProofOfLife` arm — #220: refresh the sweep exemption, and NOTHING
    /// else. No slot synthesis (this only vouches for already-visible slots),
    /// no state change, no `last_event_at` refresh. An exiting slot is left
    /// alone so the vouch can't tug against SessionEnd/cascade_exit.
    fn apply_proof_of_life(&mut self, scene: &SceneState, agent_id: AgentId, now: SystemTime) {
        if scene
            .agents
            .get(&agent_id)
            .is_some_and(|s| s.exiting_at.is_none())
        {
            self.corr.recent_proof_of_life.insert(agent_id, now);
        }
    }

    /// The `Identity` arm — #221: the identity context a hook decoder attaches
    /// ahead of a tool/permission event. Register-or-back-fill, NOTHING else:
    /// no label change, no activity-state change, no `last_event_at` refresh.
    /// The paired activity event right behind it carries those.
    fn apply_identity(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        ctx: IdentityCtx,
        pid: Option<crate::source::PidIdentity>,
        from: Transport,
        now: SystemTime,
    ) {
        // JSONL must never synthesize — a transcript line can be a historical
        // replay. No in-tree JSONL path emits Identity today; this guard IS the
        // boundary, not dead code.
        if from != Transport::Hook {
            tracing::debug!(?agent_id, "ignoring Identity on a non-hook transport");
            return;
        }
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            backfill_identity(slot, ctx);
            // A pid-less Identity must never DOWNGRADE a cached Some (e.g. an
            // opencode plugin event following a shim-stamped one).
            if pid.is_some() {
                slot.pid = pid;
            }
            // Identity is hook-only, so the owning process is alive: a
            // JSONL-seeded unknown-cwd ghost flag must not keep the 3-min reap
            // armed on it. A cwd-less Identity can't heal the cwd, and the
            // motivating permission-parked session emits nothing further within
            // 3 min.
            slot.unknown_cwd = false;
        } else if !self.corr.hook_session_end_tombstoned(agent_id, now)
            && self.register_slot(scene, agent_id, ctx, None, now)
        {
            // Only the #242 tombstone is consulted, NOT the child ledger: a
            // hook is proof of life and the reorder skew it guards is ms-scale.
            // A cwd-less Identity registers a slot that is process-proven
            // alive, NOT a startup-seeding ghost, so clear the flag its reap
            // keys on.
            if let Some(slot) = scene.agents.get_mut(&agent_id) {
                slot.unknown_cwd = false;
                if pid.is_some() {
                    slot.pid = pid;
                }
            }
        }
    }

    /// The `ModelInfo` arm — updates an EXISTING slot only; a model line must
    /// never register a session. Legitimate on BOTH transports: model/effort
    /// are wire data, not liveness. Known cosmetic residual: a first-sight
    /// replay stamps a HISTORICAL effort marker with apply-time `now`, so a
    /// session that used max effort earlier flames until the scene's effort TTL
    /// expires.
    fn apply_model_info(
        scene: &mut SceneState,
        agent_id: AgentId,
        model: Option<&str>,
        effort: Option<&str>,
        now: SystemTime,
    ) {
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            if let Some(m) = model {
                if slot.model.as_deref() != Some(m) {
                    slot.model = Some(Arc::from(m));
                }
            }
            if let Some(e) = effort {
                slot.effort = Some(crate::state::EffortObservation::new(Arc::from(e), now));
            }
        }
    }

    /// The `Usage` arm — updates an EXISTING slot only, the `ModelInfo`
    /// posture. Saturating so a hostile/corrupt transcript can't overflow it.
    fn apply_usage(scene: &mut SceneState, agent_id: AgentId, fresh_tokens: u64, now: SystemTime) {
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            slot.tokens_used = slot.tokens_used.saturating_add(fresh_tokens);
            slot.last_usage = Some(crate::state::UsageObservation::new(fresh_tokens, now));
        }
    }

    /// The `SessionStart` arm of [`Reducer::apply`], lifted out so the match
    /// stays a one-line dispatch.
    fn apply_session_start(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        ctx: IdentityCtx,
        parent_id: Option<AgentId>,
        now: SystemTime,
    ) {
        let IdentityCtx {
            session_id, cwd, ..
        } = ctx;
        // #242: hook deliveries ride per-connection tasks, so a short-lived
        // subagent's SubagentStop can be DECODED before its SubagentStart.
        // Registering this late CHILD start would mint a slot whose end already
        // passed, which only the stale sweeps could ever clear. Deliberately
        // TRANSPORT-AGNOSTIC, and parentless starts are exempt BY CONSTRUCTION:
        // Reasonix's SessionEnd→SessionStart resurrect rides the same cwd-keyed
        // parentless id and must keep registering. The tombstone is NOT
        // consumed — a duplicate late Start must no-op too.
        if parent_id.is_some()
            && !scene.agents.contains_key(&agent_id)
            && self.corr.hook_session_end_tombstoned(agent_id, now)
        {
            tracing::warn!(
                ?agent_id,
                %session_id,
                proposed_parent = ?parent_id,
                "skipped child SessionStart — its hook SessionEnd already passed \
                 (a late or reordered start, #242)"
            );
            return;
        }
        // #244-w2 — the ledger-keyed sibling of the #242 gate, for the windows
        // the 5s tombstone can't cover: a child that ended on a KNOWN slot
        // mints no tombstone, so once the exit grace GC'd it a LATE parented
        // first-sight would re-register a dead child as an unremovable phantom.
        // The ledger's `ended_at` survives the slot. Judged BEFORE the ledger
        // adoption below, so parentless revivals still pass and get re-linked
        // (#246).
        if parent_id.is_some()
            && !scene.agents.contains_key(&agent_id)
            && self.corr.child_recently_ended(agent_id, now)
        {
            tracing::warn!(
                ?agent_id,
                %session_id,
                proposed_parent = ?parent_id,
                "skipped child SessionStart — the child already ended \
                 (child ledger, #244)"
            );
            return;
        }
        // Ledger adoption (#246 / #244-w1): a PARENTLESS start for an id whose
        // ledger entry remembers an applied parent is a same-id new life of a
        // known CHILD — adopt the remembered parent so it re-joins the scope
        // tree instead of registering as an orphan. Revivals are deliberately
        // NOT blocked the way parented re-registrations are: for a genuinely
        // dead child a parent-linked slot rides the parent cascade, strictly
        // better than an orphan phantom. The adopted link still runs through
        // the #240 cycle filter, so a poisoned ledger degrades to parentless.
        let parent_id = parent_id.or_else(|| {
            self.corr
                .child_ledger
                .get(&agent_id)
                .and_then(|e| e.parent_id)
        });
        // Refuse a parent link whose ancestor chain reaches the child. This is
        // the ONE seam where `parent_id` is set or enriched, so a cycle can
        // never EXIST: a 2-cycle whose members are BOTH Waiting would mutually
        // satisfy `has_waiting_ancestor` and exempt each other from
        // `sweep_stale` forever (#238). Degrade to parentless — the session is
        // real even when its claimed lineage is malformed. Gated on a link
        // actually being APPLIED, so a duplicate's malformed parent neither
        // warns nor changes anything.
        let link_would_apply = scene
            .agents
            .get(&agent_id)
            .is_none_or(|slot| slot.parent_id.is_none());
        let parent_id = parent_id.filter(|&p| {
                    if !link_would_apply {
                        return true;
                    }
                    let cycle = scope::would_create_cycle(&scene.agents, agent_id, p);
                    if cycle {
                        tracing::warn!(
                            ?agent_id,
                            proposed_parent = ?p,
                            %session_id,
                            cwd = %cwd.display(),
                            "refused parent_id link — it would close a parent cycle; degrading to parentless"
                        );
                    }
                    !cycle
                });
        if let Some(slot) = scene.agents.get_mut(&agent_id) {
            // A subagent's own rollout (JSONL) can create the slot ORPHANED
            // before its SubagentStart hook arrives with the parent link;
            // enrich it so the subagent joins the scope tree regardless of
            // arrival order, but never re-parent one that already has a parent.
            if slot.parent_id.is_none() {
                if let Some(p) = parent_id {
                    slot.parent_id = Some(p);
                    // An APPLIED link revives the ledger entry so gc can't
                    // prune a still-live re-linked child (#244/#246).
                    self.corr.link_applied_parent(agent_id, p);
                }
            }
            // A slot can exist with MISSING identity context: the
            // hook-synthesis pre-pass registers from events carrying only the
            // AgentId, and a Codex revive ghost has an empty cwd.
            let label_is_upgradable = slot.label.is_upgradable();
            if let Some(base) = backfill_identity(slot, ctx) {
                // A basename- or Rename-derived label is real information.
                if label_is_upgradable {
                    // `base` BYPASSES the `cwd_basename_label` chokepoint, so
                    // it needs the same decode-boundary cap applied here.
                    slot.label = crate::state::SlotLabel::cwd_derived(format!(
                        "{}·{}",
                        label_prefix_for(&slot.source),
                        crate::source::decoder::ellipsize(
                            base,
                            crate::source::decoder::MAX_DECODED_FIELD_CHARS,
                        )
                    ));
                }
            }
            // A duplicate SessionStart is still a genuine liveness signal
            // (Codex/Reasonix re-emit one per UserPromptSubmit) — refresh it so
            // a prompt landing just under the stale threshold pushes the
            // boundary out instead of losing the race to the sweep mid-turn.
            slot.last_event_at = now;
            // A SessionStart on an EXITING slot means the session lives —
            // Reasonix's `/new` fires SessionEnd+SessionStart back-to-back on
            // the SAME cwd-keyed id, and without this the new session's whole
            // first turn is invisible. Gated to root agents on BOTH sides so a
            // late duplicate can't un-exit a b1-cascaded subagent;
            // `resurrect_in_place` has no exiting guard of its own, so relaxing
            // the conjunction here WOULD reset a live root.
            if slot.exiting_at.is_some() && slot.parent_id.is_none() && parent_id.is_none() {
                // Route through fsm so an in-flight Active span is folded into
                // active_ms before the reset — a direct `state = Idle` here
                // silently dropped it.
                fsm::resurrect_in_place(slot, now);
                // Evict the dead life's correlation state, as `sweep_exited`
                // would have if the corpse had GC'd first: a leftover
                // active_tasks entry keeps suppress_subagent_leak eating the
                // new life's hooks, and an armed b1 cascade would fire into the
                // fresh subtree. recent_proof_of_life deliberately SURVIVES — a
                // resurrecting slot is by definition still alive.
                self.remove_agent_correlation(&agent_id);
            }
            return;
        }
        if self.register_slot(scene, agent_id, ctx, parent_id, now) {
            // A desk-exhaustion refusal records nothing — the session was
            // dropped, not registered.
            if let Some(p) = parent_id {
                self.corr.link_applied_parent(agent_id, p);
            }
        }
    }

    /// Pre-pass 0 of [`Reducer::apply`] (hook transport only): synthesize a
    /// registration for a tool/permission event whose id has no slot, so a
    /// session whose transcript was gated at first sight becomes visible the
    /// moment it fires a hook. Only `ActivityStart`/`ActivityEnd`/`Waiting`
    /// qualify — each unambiguously proves a live session, while `SessionEnd`
    /// and `Rename` for an unknown id prove nothing worth showing.
    ///
    /// Since #221 the hook decoders attach an [`AgentEvent::Identity`] AHEAD of
    /// tool/permission events, so this blank ordinal-labeled path is only the
    /// fallback for identity-less hook events.
    fn synthesize_hook_registration(
        &mut self,
        scene: &mut SceneState,
        event: &AgentEvent,
        id: AgentId,
        now: SystemTime,
    ) {
        if scene.agents.contains_key(&id)
            || !matches!(
                event,
                AgentEvent::ActivityStart { .. }
                    | AgentEvent::ActivityEnd { .. }
                    | AgentEvent::Waiting { .. }
            )
        {
            return;
        }
        // A tombstoned id just had its hook SessionEnd arrive with no slot, so
        // this event is a reordered straggler from the DEAD session, not proof
        // of new life. Synthesizing would mint a blank Idle ghost no future
        // SessionEnd can remove.
        if self.corr.hook_session_end_tombstoned(id, now) {
            return;
        }
        if self.register_slot(
            scene,
            id,
            IdentityCtx {
                source: "",
                session_id: "",
                cwd: std::path::Path::new(""),
            },
            None,
            now,
        ) {
            if let Some(slot) = scene.agents.get_mut(&id) {
                // NOT an unknown-cwd ghost: the short reap exists for startup
                // JSONL-seeding artifacts, and this slot is process-proven
                // alive. The motivating case — parked on a permission prompt,
                // appending nothing — would be reaped before any back-fill.
                slot.unknown_cwd = false;
            }
        }
    }

    /// The slot-creation half of the `SessionStart` arm, shared with
    /// [`Reducer::synthesize_hook_registration`] so both run the same
    /// desk-capacity gate and label derivation. Returns `false` when all desks
    /// are occupied.
    fn register_slot(
        &mut self,
        scene: &mut SceneState,
        agent_id: AgentId,
        ctx: IdentityCtx,
        parent_id: Option<AgentId>,
        now: SystemTime,
    ) -> bool {
        let IdentityCtx {
            source,
            session_id,
            cwd,
        } = ctx;
        let Some(desk_index) = scene.next_free_desk() else {
            tracing::warn!(
                ?agent_id,
                cwd = %cwd.display(),
                session_id = %session_id,
                total_capacity = scene.total_capacity(),
                "dropped SessionStart — all desks occupied; bump --max-desks"
            );
            return false;
        };
        let floor_idx = scene.floor_of(desk_index);
        // A hook-only source has no JSONL Rename, so this registration is the
        // sole place its prefix is established; the JSONL derivers reinforce it
        // idempotently.
        let prefix = label_prefix_for(source);
        // The cwd is hook/transcript CONTENT — route the basename through the
        // `cwd_basename_label` chokepoint so the label is capped at the decode
        // boundary.
        let named = crate::source::decoder::cwd_basename_label(prefix, cwd);
        let has_cwd = named.is_some();
        let label = match named {
            Some(l) => crate::state::SlotLabel::cwd_derived(l),
            None => {
                // Only an unknown-cwd ghost consumes an ordinal, so labels stay
                // contiguous instead of skipping the preceding named sessions.
                self.next_label_n += 1;
                crate::state::SlotLabel::ordinal_ghost(format!("{prefix}#{}", self.next_label_n))
            }
        };
        // Disambiguation for multiple sessions sharing a cwd happens at render
        // time, not here — a unique session must not carry a noisy `·xxxx`.
        scene.agents.insert(
            agent_id,
            AgentSlot {
                agent_id,
                source: Arc::<str>::from(source),
                session_id: Arc::<str>::from(session_id),
                cwd: Arc::<std::path::Path>::from(cwd),
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
                // A PARENTED slot came from an explicit subagent signal (e.g.
                // Copilot's `subagent.started`, whose payload carries no cwd),
                // not a startup-seeding ghost; without this exemption a
                // long-running subagent with no cwd-bearing event is swept
                // while alive.
                unknown_cwd: !has_cwd && parent_id.is_none(),
                parent_id,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
        true
    }

    /// Pre-pass 1 of [`Reducer::apply`] — subagent-leak suppression (hook
    /// transport only): if this AgentId has any Task tool in flight, hook
    /// ActivityStart/End events for it are almost certainly subagent work
    /// misattributed to the parent. Drop them and defer to JSONL, which targets
    /// the subagent's own AgentId. The Task's own PostToolUse is exempt — its
    /// tool_use_id matches a tracked one, so it passes through.
    fn suppress_subagent_leak(
        &mut self,
        scene: &mut SceneState,
        event: &AgentEvent,
        id: AgentId,
        now: SystemTime,
    ) -> bool {
        let tasks = self.corr.active_tasks.get(&id);
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
            // One state change still belongs to the parent: a `Waiting` while
            // delegating is usually the SUBAGENT's permission gate,
            // misattributed. A suppressed child event means the subagent
            // resumed, so the gate resolved — restore Active(Delegating).
            //
            // CONDITIONAL on the gate: a delegating parent CAN run its own
            // parallel ordinary tool, and when THAT tool was gated at
            // Waiting-entry the gate holds a tuid ∉ active_tasks — the prompt is
            // the PARENT's own and still pending, so keep the Waiting and the
            // gate for that tool's own END to resolve.
            if let Some(slot) = scene.agents.get_mut(&id) {
                if matches!(slot.state, ActivityState::Waiting { .. }) {
                    let gate_is_own_tool = self
                        .corr
                        .gated_before_waiting
                        .get(&id)
                        .is_some_and(|g| !tasks.is_some_and(|s| s.contains(&**g)));
                    if !gate_is_own_tool {
                        let task_tuid = self.corr.any_active_task(&id);
                        fsm::enter_delegating(slot, task_tuid, now);
                        self.corr.gated_before_waiting.remove(&id);
                    }
                }
            }
        }
        suppress
    }

    /// Last pre-pass of [`Reducer::apply`] — track active Task tool_use_ids
    /// from either transport, marking a parent that gains a Task
    /// Active("Delegating") so it doesn't look asleep while its subagents do
    /// the visible work.
    ///
    /// b1 subagent-completion inference (CC writes no completion marker): a
    /// drained parent Task means the delegated subtree returned — cascade EXIT
    /// to the parent's descendants, not the parent itself, so completed
    /// subagents leave promptly instead of lingering to the idle stale-sweep.
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
                // Delegating only on the FIRST insert: real hook↔JSONL skew
                // runs past HOOK_WINS_WINDOW, and an out-of-window replay of an
                // already-tracked tuid re-firing enter_delegating would clobber
                // a parent that went Waiting on its own permission prompt
                // since. The flag above stays unconditional either way, so the
                // duplicate can't fall through to the main arm's enter_active.
                //
                // The drained-tuid tombstone closes the residual: once the hook
                // End DRAINS the tuid, a pair-replay's Start IS a fresh first
                // insert — re-insert it for End symmetry, but never re-enter
                // Delegating for a Task that already completed.
                if self
                    .corr
                    .active_tasks
                    .entry(*agent_id)
                    .or_default()
                    .insert(tuid.clone())
                    && !self
                        .corr
                        .recent_task_drains
                        .contains_key(&(*agent_id, tuid.clone()))
                {
                    if let Some(slot) = scene.agents.get_mut(agent_id) {
                        fsm::enter_delegating(slot, Some(Arc::<str>::from(tuid.as_str())), now);
                    }
                }
            }
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: Some(tuid),
            } => {
                if let Some(set) = self.corr.active_tasks.get_mut(agent_id) {
                    if set.remove(tuid) {
                        handled_by_task_tracking = true;
                        self.corr
                            .recent_task_drains
                            .insert((*agent_id, tuid.clone()), now);
                        // #152: the drain path skips the main arm, so a gate
                        // holding THIS Task's tuid would go stale — and a later
                        // out-of-window JSONL replay of this END would
                        // false-match it via resolves_wait and flip a
                        // still-pending permission to Idle. Clear only OUR
                        // tuid: a parallel ordinary tool's gate must survive.
                        if self.corr.gated_before_waiting.get(agent_id).map(|g| &**g)
                            == Some(tuid.as_str())
                        {
                            self.corr.gated_before_waiting.remove(agent_id);
                        }
                        if let Some(slot) = scene.agents.get_mut(agent_id) {
                            slot.last_event_at = now;
                            // Only arm the idle debounce when actually Active —
                            // if the parent is Waiting (its own permission
                            // prompt fired during delegation) the expiry would
                            // false-clear a still-pending permission.
                            if set.is_empty() {
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

    /// Fire deferred b1 cascades whose [`B1_CASCADE_GRACE`] elapsed (#151). The
    /// fire-time emptiness check IS the cancel mechanism: a Task insert inside
    /// the grace re-populates `active_tasks`, so the due entry is discarded
    /// instead of fired — no separate cancel-on-insert bookkeeping that could
    /// drift out of sync with the ledger.
    fn fire_pending_b1_cascades(&mut self, scene: &mut SceneState, now: SystemTime) {
        let due: Vec<AgentId> = self
            .pending_b1_cascades
            .iter()
            .filter(|(_, armed)| elapsed_at_least(now, **armed, B1_CASCADE_GRACE))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.pending_b1_cascades.remove(&id);
            if self
                .corr
                .active_tasks
                .get(&id)
                .is_some_and(|s| !s.is_empty())
            {
                continue;
            }
            tracing::debug!(agent_id = ?id, "b1 grace elapsed — cascading completed subtree");
            // The delegating parent keeps running; only its subtree walks out.
            scope::cascade_exit(scene, id, scope::StampRoot::No, now);
        }
    }

    /// Flip agents with an elapsed `pending_idle_at` to Idle. Resets
    /// `state_started_at` to `now` so the Idle wander state machine starts from
    /// the visible transition, not the now-stale original ActivityEnd time.
    fn expire_pending_idles(&mut self, scene: &mut SceneState, now: SystemTime) {
        for slot in scene.agents.values_mut() {
            let Some(pending) = slot.pending_idle_at else {
                continue;
            };
            if elapsed_at_least(now, pending, ACTIVE_GRACE_WINDOW) {
                // A Waiting slot only carries `pending_idle_at` when its gated
                // permission tool resolved; a parallel-prompt Waiting never
                // gets the timer armed, so it isn't reached here.
                fsm::settle_to_idle(slot, pending, now);
            }
        }
    }

    /// Mark agents as exiting when they haven't emitted any event for longer
    /// than their state-adaptive threshold. Uses `last_event_at` as the
    /// liveness signal, NOT `state_started_at`, which only tracks the current
    /// state's age.
    fn sweep_stale(&mut self, scene: &mut SceneState, now: SystemTime) {
        // Readiness exemption: a node blocked under a `Waiting` ancestor (e.g.
        // a subagent whose permission Notification was attributed to the
        // parent) is paused on a human gate, not dead.
        let agents = &scene.agents;
        let stale: Vec<(AgentId, Duration, Duration)> = agents
            .values()
            .filter(|slot| slot.exiting_at.is_none())
            .filter_map(|slot| {
                if scope::has_waiting_ancestor(agents, slot.agent_id) {
                    return None;
                }
                // Probe-vouched exemption (#220): a recent ProofOfLife means
                // the owning process is alive RIGHT NOW, so event silence is
                // not death.
                if self.corr.vouch_fresh(&slot.agent_id, now) {
                    return None;
                }
                // The vouch extends to a vouched ancestor's DELEGATED subtree:
                // the probe never vouches subagent ids, and a permission-parked
                // parent renders Active, so `has_waiting_ancestor` can't fire
                // for its blocked-but-live child — which would be swept
                // unrecoverably. Gated on the ancestor ACTIVELY delegating so a
                // completed lingering child keeps the idle backstop.
                if scope::has_ancestor_where(agents, slot.agent_id, |a| {
                    self.corr.vouch_fresh(&a.agent_id, now)
                        && self
                            .corr
                            .active_tasks
                            .get(&a.agent_id)
                            .is_some_and(|t| !t.is_empty())
                }) {
                    return None;
                }
                let age = now
                    .duration_since(slot.last_event_at)
                    .unwrap_or(Duration::ZERO);
                let threshold = stale_threshold(slot);
                (age > threshold).then_some((slot.agent_id, age, threshold))
            })
            .collect();

        // Cascading to each stale agent's subagents keeps a stale-swept (or
        // abruptly-exited, SessionEnd-less) parent from leaving orphans behind.
        // Skipping a slot a prior cascade in this same sweep already marked
        // keeps the log and `exiting_at` write-once.
        for (id, age, threshold) in stale {
            {
                // Unreachable today — nothing removes a slot between the two
                // passes — but kept to harden against a future refactor that
                // mutates membership mid-sweep.
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
            }
            scope::cascade_exit(scene, id, scope::StampRoot::Yes, now);
        }
    }

    /// Drop the per-slot correlation a departing agent id owns, reclaimed on
    /// BOTH a resurrect-in-place and a slot removal. Death-only teardown
    /// (`recent_proof_of_life`, the `child_ledger` stamp) deliberately stays at
    /// the sweep site — it must NOT run on a resurrect.
    fn remove_agent_correlation(&mut self, id: &AgentId) {
        self.corr.active_tasks.remove(id);
        self.corr.gated_before_waiting.remove(id);
        self.pending_b1_cascades.remove(id);
    }

    /// Remove agents whose exit animation has finished.
    ///
    /// Removing a parent does NOT null any surviving child's `parent_id` — that
    /// pointer is left dangling intentionally. The scope walks tolerate it via
    /// their `None => break` guards, and scanning every child on each parent
    /// removal would add cost with no behavioral benefit either way.
    fn sweep_exited(&mut self, scene: &mut SceneState, now: SystemTime) {
        let expired: Vec<AgentId> = scene
            .agents
            .iter()
            .filter_map(|(id, slot)| {
                slot.exiting_at
                    .filter(|t| elapsed_past(now, *t, EXIT_GRACE_WINDOW))
                    .map(|_| *id)
            })
            .collect();
        for id in expired {
            scene.agents.remove(&id);
            // This sweep runs on the apply path too, where the tick-time
            // `retain` doesn't, so a mid-turn-swept Waiting slot's gated
            // tool_use_id must be reclaimed here, not left until the next tick.
            self.remove_agent_correlation(&id);
            // Evicting with the slot keeps a removed id from exempting a
            // same-id resurrect ghost inside the TTL window.
            self.corr.recent_proof_of_life.remove(&id);
            // A CHILD whose end wasn't `as_child` starts its ledger GC clock at
            // slot removal, which also arms the #244-w2 gate for those exits.
            // `get_or_insert` keeps an earlier as_child stamp; roots have no
            // entry, so this scopes itself to children.
            if let Some(entry) = self.corr.child_ledger.get_mut(&id) {
                entry.ended_at.get_or_insert(now);
            }
        }
    }
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
mod tests;
