use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::source::{AgentEvent, Transport};
use crate::state::correlation::{Correlation, ToolEventKind};
use crate::state::{fsm, scope, ActivityState, AgentSlot, SceneState, ToolKind};
use crate::AgentId;

// The correlation-map TTL constants moved to `state/correlation.rs` with
// their maps; re-exported here so the `state::reducer::*` paths the
// integration tests (and any external user of the doc(hidden) knobs) consume
// are unchanged.
#[doc(hidden)]
pub use crate::state::correlation::{
    CHILD_END_LEDGER_TTL, DRAINED_TASK_TOMBSTONE_TTL, HOOK_SESSION_END_TOMBSTONE_TTL,
    HOOK_WINS_WINDOW, PROOF_OF_LIFE_TTL,
};

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
/// [`STALE_IDLE_TIMEOUT`]: it has a real `SessionEnd` signal (the
/// best-effort hook) for the common clean exit, so a short reaper there
/// would only evict genuinely live-but-idle sessions (lunch-break idle)
/// with no upside.
#[doc(hidden)]
pub const STALE_SHORT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The state-adaptive stale timeout for one slot. Unknown-cwd ghosts reap on the
/// shortest window (almost always startup-seeding artifacts). Otherwise the
/// timeout follows the activity state — with one carve-out: an idle slot whose
/// source has `caps.short_idle_reap()` (today: Codex and grok — no exit signal of any
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
        crate::source::registry::descriptor_for(&slot.source).map(|d| d.caps()),
    )
}

/// Policy half of [`stale_threshold`], split from the registry lookup so caps
/// combinations no registered source has YET are unit-testable with a
/// synthetic [`SourceCaps`] (the lookup half is pinned by the registered-
/// source tests in `tests/reducer/liveness.rs`).
fn stale_threshold_with_caps(
    slot: &AgentSlot,
    caps: Option<crate::source::registry::SourceCaps>,
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
        // Give it the Waiting-class window instead. Keyed on the typed
        // `ToolKind::Task` the slot carries, never the display string — a
        // Generic tool spelling "Delegating" is not a delegation. (CC is
        // immune by construction: its subagents' misattributed hooks drive
        // `refresh_lineage`. The false-positive ghost self-heals on the next
        // UserPromptSubmit — same argument as the Codex idle carve-out.)
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

/// Classify an incoming `Rename` label's provenance for `source`: exactly the
/// registry prefix (`cx`, a JSONL `LabelDeriver`'s empty-cwd fallback) is a
/// [`LabelProvenance::PrefixFallback`] the back-fill may still upgrade;
/// anything else is a real display name ([`LabelProvenance::Renamed`] — CC
/// `attributionAgent` subagent names, the derivers' cwd-derived renames). The
/// judgment happens HERE, at the mint, not at back-fill time: a bare-prefix
/// Rename always lands on a slot whose source is already set (its own
/// transcript's `SessionStart` precedes it on the same channel and back-fills
/// the source first), so the slot's prefix is the right yardstick.
fn classify_rename(label: &str, source: &str) -> crate::state::SlotLabel {
    let prefix = source_label_prefix(source);
    if !prefix.is_empty() && label == prefix {
        crate::state::SlotLabel::prefix_fallback(label)
    } else {
        crate::state::SlotLabel::renamed(label)
    }
}

/// Borrowed identity context threaded into slot registration/back-fill.
/// Groups the three identity fields so the two `&str`s (`source`/`session_id`,
/// positionally interchangeable) can't be silently swapped at a call site.
/// Purely a call-site bundle — never stored on `AgentSlot`, never public.
#[derive(Clone, Copy)]
struct IdentityCtx<'a> {
    source: &'a str,
    session_id: &'a str,
    cwd: &'a std::path::Path,
}

/// First-wins identity back-fill shared by the duplicate-`SessionStart` arm
/// and the hook [`AgentEvent::Identity`] arm (#221): heal EMPTY
/// source/session_id/cwd — an established value is never overwritten. Returns
/// the healed cwd's basename when THIS call healed the cwd; the SessionStart
/// arm alone upgrades a fallback label from it (`Identity` carries no label
/// authority — label upgrades stay on the SessionStart path).
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
    /// The seven cross-slot correlation maps + their TTL pruning — see
    /// `state/correlation.rs` (the maps moved there verbatim; decisions stay
    /// in the arms below).
    corr: Correlation,
    /// Parents whose last Task drained, awaiting the deferred b1 cascade
    /// ([`B1_CASCADE_GRACE`]): armed on drain, fired by
    /// `fire_pending_b1_cascades` on the tick/apply sweep path — but only if
    /// `active_tasks` is STILL empty at fire time, which is how a Task
    /// insert inside the grace (the suppressed parallel dispatch's JSONL
    /// copy — #151) defuses it. Evicted at BOTH sites like the other maps
    /// (tick's retain + `sweep_exited`'s remove).
    pending_b1_cascades: HashMap<AgentId, SystemTime>,

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
        self.corr.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        self.fire_pending_b1_cascades(scene, now);
        self.sweep_stale(scene, now);
        // Clean up active_tasks entries for agents that never got a
        // SessionStart (Task event arrived before JSONL created the slot).
        self.corr
            .active_tasks
            .retain(|id, _| scene.agents.contains_key(id));
        self.corr
            .gated_before_waiting
            .retain(|id, _| scene.agents.contains_key(id));
        self.pending_b1_cascades
            .retain(|id, _| scene.agents.contains_key(id));
    }

    /// Gracefully evict every live agent of `source_id` — mark each exiting and
    /// Reconcile the scene toward the `connected` set: mark exiting every
    /// non-exiting slot whose source is NOT connected, then cascade to its subtree
    /// (exactly as the `SessionEnd` arm does per agent), so the existing EXIT-walk
    /// animation + `sweep_exited` GC remove them. Driven by the Sources panel's
    /// DISCONNECT toggle (an explicit user action — the runtime calls this every
    /// sweep tick with the live connected snapshot), NOT by transcript content, so
    /// it does NOT violate the "content never drives lifecycle" invariant: it's the
    /// same authority class as a SessionEnd hook.
    ///
    /// Keying on `!connected.contains(source)` — the COMPLEMENT of the connected
    /// set — rather than iterating a known source list is load-bearing: it also
    /// evicts a BLANK-source slot (`source == ""`) synthesized for an identity-less
    /// hook event that slipped the per-event gate (an empty string is in no
    /// connected set), closing the gate-slip hole the per-event check leaves open.
    /// Already-exiting slots are left untouched (idempotent — a re-reconcile must
    /// not reset their walkout clock). Leftover `active_tasks`/`gated_before_waiting`
    /// entries are reaped by `sweep_exited` on GC, like any other exit.
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

    pub fn apply(
        &mut self,
        scene: &mut SceneState,
        event: AgentEvent,
        now: SystemTime,
        from: Transport,
    ) {
        self.corr.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        let id = event.agent_id();

        // PRE-PASS 0 — a hook event is PROOF OF LIFE: it can only come from a
        // live process. A hook tool/permission event whose id has no slot means
        // a live session is invisible — its transcript was gated at first sight
        // (mid-attach, idle >1h), so no JSONL SessionStart ever ran; parked on a
        // permission prompt (Notification appends nothing to the transcript) it
        // has no revival path at all. Synthesize the registration the missing
        // SessionStart would have performed, then let the event itself apply to
        // the fresh slot. JSONL events must NOT synthesize — a transcript line
        // can be a historical replay (the watcher's first-sight gate exists
        // precisely for those), so the unknown-id no-op stays load-bearing
        // there. SessionEnd/Rename don't synthesize either: an end/rename for
        // an unknown agent proves nothing worth showing.
        if from == Transport::Hook {
            // A SessionEnd for an UNKNOWN id tombstones it: the session ended
            // while invisible, and a reordered trailing event from the same
            // dying session (per-connection hook tasks) must not resurrect it
            // through the synthesis below. A KNOWN id keeps today's behavior
            // (the main arm marks the slot exiting; no tombstone needed).
            if matches!(event, AgentEvent::SessionEnd { .. }) && !scene.agents.contains_key(&id) {
                self.corr.recent_hook_session_ends.insert(id, now);
            }
            self.synthesize_hook_registration(scene, &event, id, now);
        }

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
                if let Some((_, recorded)) =
                    self.corr.recent_hook_tool_uses.get(&(id, tuid.to_string()))
                {
                    if !(*recorded == ToolEventKind::Start && kind == ToolEventKind::End) {
                        return;
                    }
                }
            }
        }

        // The record is gated on the slot EXISTING (post-synthesis): when the
        // hook synthesis above was REFUSED (desk exhaustion), the event applies
        // to nothing — but its record would outlive the refusal, and a desk can
        // free within HOOK_WINS_WINDOW (an exiting slot's grace elapsing). The
        // JSONL SessionStart + ActivityStart that then register the session
        // would have their ActivityStart dedup-eaten by the stale record,
        // rendering the freshly visible agent Idle through its first tool.
        if from == Transport::Hook && scene.agents.contains_key(&id) {
            if let Some((kind, tuid)) = event_tool_use_id(&event) {
                self.corr
                    .recent_hook_tool_uses
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
            } => {
                if !handled_by_task_start {
                    // Resuming to Active (next tool / Codex function_call_output)
                    // makes any pending gated-permission correlation moot.
                    self.corr.gated_before_waiting.remove(&agent_id);
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        if !detail.as_ref().is_some_and(|d| d.is_task()) {
                            slot.tool_call_count += 1;
                        }
                        // Derive the semantic category ONCE, from the typed
                        // detail, before it is erased to the HUD display
                        // string — downstream deciders match `kind`.
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
                                self.corr.gated_before_waiting.get(&agent_id).map(|g| &**g)
                                    == Some(tuid)
                            }
                            None => from == Transport::Hook,
                        };
                    if resolves_wait {
                        self.corr.gated_before_waiting.remove(&agent_id);
                    }
                    // While the agent is still DELEGATING (a non-empty
                    // active_tasks entry), its own parallel tool ending must
                    // not settle it to Idle — nothing would restore the
                    // Delegating display for the rest of the delegation (the
                    // suppress-restore fires only from Waiting; the eventual
                    // Task drain only arms idle / fires b1), so the parent
                    // would render asleep while its subagents do the visible
                    // work — exactly what `track_active_tasks`' Delegating
                    // marking exists to prevent. Re-enter Delegating instead.
                    let delegating_tuid = self
                        .corr
                        .active_tasks
                        .get(&agent_id)
                        .and_then(|s| s.iter().next())
                        .map(|t| Arc::<str>::from(t.as_str()));
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        // Arm the idle debounce when Active (normal tool end) or
                        // when a gated permission just resolved — in both cases
                        // the slot settles to Idle after ACTIVE_GRACE_WINDOW. A
                        // stale ActivityEnd while Idle, or a parallel tool ending
                        // while Waiting, leaves the timer alone.
                        if matches!(slot.state, ActivityState::Active { .. }) || resolves_wait {
                            match delegating_tuid {
                                Some(tuid) => fsm::enter_delegating(slot, Some(tuid), now),
                                None => fsm::arm_pending_idle(slot, now),
                            }
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
                        self.corr
                            .gated_before_waiting
                            .insert(agent_id, tuid.clone());
                    } else {
                        self.corr.gated_before_waiting.remove(&agent_id);
                    }
                    fsm::enter_waiting(slot, Arc::<str>::from(reason.as_str()), now);
                }
            }
            AgentEvent::Rename { agent_id, label } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    let label = classify_rename(&label, &slot.source);
                    fsm::rename(slot, label, now);
                }
            }
            AgentEvent::SessionEnd { agent_id, as_child } => {
                // A SubagentStop-derived end stamps the child ledger
                // REGARDLESS of slot existence (#244): the Stop-before-Start
                // reorder has no slot, yet its ended_at must arm the parented
                // gate (entry defaults parentless — the blocked Start never
                // applies a link). For a KNOWN slot this is the stamp that
                // outlives the 4.5s GC, covering the late-first-sight window
                // the #242 unknown-id tombstone structurally can't.
                if as_child {
                    self.corr.child_ledger.entry(agent_id).or_default().ended_at = Some(now);
                }
                scope::cascade_exit(scene, agent_id, scope::StampRoot::Yes, now);
            }
            // #220: refresh the sweep exemption — and NOTHING else. No slot
            // synthesis (only hook tool/permission events are proof of NEW
            // life; this only vouches for already-visible slots), no state
            // change, no `last_event_at` refresh (the Active→Idle debounce and
            // back-fill stay driven by real events). An exiting slot is left
            // alone so the vouch can't tug against SessionEnd/cascade_exit.
            AgentEvent::ProofOfLife { agent_id } => {
                if scene
                    .agents
                    .get(&agent_id)
                    .is_some_and(|s| s.exiting_at.is_none())
                {
                    self.corr.recent_proof_of_life.insert(agent_id, now);
                }
            }
            // #221: the identity context a hook decoder attaches ahead of a
            // tool/permission activity event — register-or-back-fill, NOTHING
            // else: no label change (Identity carries no label authority — see
            // `backfill_identity`), no activity-state change, no
            // `last_event_at` refresh (the paired activity event right behind
            // it carries those).
            AgentEvent::Identity {
                agent_id,
                source,
                session_id,
                cwd,
                pid,
            } => {
                // Boundary (1) made structural: JSONL must never synthesize —
                // a transcript line can be a historical replay. No in-tree
                // JSONL path emits Identity today; this guard IS the boundary,
                // not dead code (cf. the transport-relevant ProofOfLife arm).
                if from != Transport::Hook {
                    tracing::debug!(?agent_id, "ignoring Identity on a non-hook transport");
                    return;
                }
                let cwd = cwd.as_deref().unwrap_or_else(|| std::path::Path::new(""));
                let ctx = IdentityCtx {
                    source: &source,
                    session_id: &session_id,
                    cwd,
                };
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    backfill_identity(slot, ctx);
                    // Focus-jump pid cache: hook-transport Identity recurs
                    // ahead of every activity, so this stays fresh. A pid-less
                    // Identity never DOWNGRADES a cached Some (e.g. an
                    // opencode plugin event following a shim-stamped one).
                    if pid.is_some() {
                        slot.pid = pid;
                    }
                    // The same reap exemption the registration branch below
                    // honors, mirrored: Identity is hook-only (transport
                    // guard above), so the owning process is alive — a
                    // JSONL-seeded unknown-cwd ghost flag (e.g. a Codex
                    // revive ghost's empty cwd) must not keep the 3-min reap
                    // armed on it. A cwd-less Identity can't heal the cwd
                    // (`backfill_identity` clears the flag only on a real
                    // cwd), and the motivating permission-parked session
                    // emits nothing further within 3 min.
                    slot.unknown_cwd = false;
                } else if !self.corr.hook_session_end_tombstoned(agent_id, now)
                    && self.register_slot(scene, agent_id, ctx, None, now)
                {
                    // Only the 5s #242 tombstone is consulted — NOT the 90s
                    // child ledger, deliberately: a hook is proof of life and
                    // the reorder skew it guards is ms-scale. Residual: a dead
                    // child's hook straggler landing in the (5s, 90s] window
                    // re-registers it PARENTLESS here (and via the blank hook
                    // synthesis, which makes the same call); it self-heals to
                    // parent-linked when a later parentless SessionStart hits
                    // the enrichment adoption.
                    //
                    // The same reap exemption as the blank hook synthesis: a
                    // cwd-less Identity registers an ordinal-labeled slot that
                    // is process-proven alive, NOT a startup-seeding ghost —
                    // the 3-min unknown-cwd reap would kill it before any
                    // back-fill. No-op when the Identity carried a real cwd.
                    // A desk-capacity refusal does nothing further — Identity
                    // carries no tool_use_id, so the dedup map can't be
                    // poisoned (boundary 3 untouched).
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        slot.unknown_cwd = false;
                        // Focus-jump pid cache, same rule as the backfill branch.
                        if pid.is_some() {
                            slot.pid = pid;
                        }
                    }
                }
            }
            AgentEvent::ModelInfo {
                agent_id,
                model,
                effort,
            } => {
                // Pure observation cache — updates an EXISTING slot only (a
                // model line must never register a session; unknown id =
                // no-op). Legitimate on BOTH transports: model/effort are
                // wire data, not liveness. Known bounded residual: the
                // watcher reads RECENT/LIVE-probed files from the TOP, so a
                // first-sight replay can stamp a HISTORICAL effort marker
                // with apply-time `now` — a session that used max effort
                // earlier (but no longer) flames for up to the scene's
                // EFFORT_TTL_SECS (10 min) after attach, then self-heals. Purely
                // cosmetic; model is immune (last-seen-wins, no freshness).
                // `model` writes only on change (Arc churn); `effort`
                // re-stamps `now` per sighting — the freshness the scene
                // layer's TTL reads.
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    if let Some(m) = model {
                        if slot.model.as_deref() != Some(m.as_str()) {
                            slot.model = Some(std::sync::Arc::from(m.as_str()));
                        }
                    }
                    if let Some(e) = effort {
                        slot.effort = Some(crate::state::EffortObservation::new(
                            std::sync::Arc::from(e.as_str()),
                            now,
                        ));
                    }
                }
            }
            AgentEvent::Usage {
                agent_id,
                fresh_tokens,
            } => {
                // Pure counter accumulation — updates an EXISTING slot only
                // (usage never registers a session; unknown id = no-op, the
                // ModelInfo posture). Saturating: a hostile/corrupt transcript
                // can't overflow the counter. The delta + apply-time stamp
                // feed the scene's falling-sheet window; the cumulative total
                // feeds its tier ladder.
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    slot.tokens_used = slot.tokens_used.saturating_add(fresh_tokens);
                    slot.last_usage = Some(crate::state::UsageObservation::new(fresh_tokens, now));
                }
            }
        }
    }

    /// The `SessionStart` arm of [`Reducer::apply`], lifted whole so the
    /// match stays a one-line dispatch. Handles the #242/#244 tombstone
    /// gates, ledger parent adoption/#240 cycle filter, duplicate-start
    /// enrichment + resurrect-in-place, and first registration. Move-only.
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
        // #242: hook deliveries ride per-connection tasks, so a
        // short-lived subagent's SubagentStop can be DECODED before
        // its SubagentStart. The Stop's SessionEnd landed on an
        // unknown id and tombstoned it; registering this late CHILD
        // start (the event carries a parent link) would mint a slot
        // whose end already passed — no future SessionEnd is coming,
        // so only the stale sweeps would clear it. Degrade to a
        // no-op. Deliberately TRANSPORT-AGNOSTIC: the tombstone is
        // evidence the child already ended regardless of which
        // transport re-introduces it — a JSONL first-sight of the
        // child's transcript racing the hook Stop has the same
        // phantom shape (CC subagent transcripts carry no end
        // marker), and a historical replay never SessionStarts (the
        // watcher's first-sight gate), so no legitimate JSONL flow is
        // lost. Parentless starts are exempt BY CONSTRUCTION:
        // Reasonix's documented SessionEnd→SessionStart resurrect
        // rides the same (cwd-keyed, parentless) id and must keep
        // registering. (A tombstoned Codex child's flat-rollout
        // first-sight is itself parentless — detect_parent_id is
        // CC-layout-specific — and slips this exemption on purpose;
        // since #244 the ledger adoption below re-links it to its
        // remembered parent instead of leaving an orphan phantom.)
        // The tombstone is NOT consumed — a duplicate late Start
        // must no-op too.
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
        // #244-w2 — the ledger-keyed sibling of the #242 gate above,
        // for the windows the 5s tombstone can't cover: a child that
        // ended on a KNOWN slot mints no tombstone, so once the 4.5s
        // exit grace GC'd it, a LATE parented first-sight of its
        // transcript (notify outage → the watcher's 60s poll) would
        // re-register a dead child as a phantom no future SessionEnd
        // can remove. The ledger's `ended_at` survives the slot;
        // gate on it for CHILD_END_LEDGER_TTL. Same shape as #242:
        // PARENTED starts only (the event's OWN parent, judged
        // BEFORE the ledger adoption below — parentless revivals are
        // deliberately allowed through and re-linked instead, #246),
        // unknown ids only, tombstone not consumed.
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
        // Ledger adoption (#246 / #244-w1): a PARENTLESS start for an
        // id whose ledger entry remembers an applied parent is a
        // same-id new life of a known CHILD. It engages for every
        // parentless re-registration: a dead child's flat-rollout
        // first-sight (parentless by layout, #244-w1), a
        // post-un-claim revival (a negative vouch / instant exit /
        // decoded terminator un-claimed the rollout from `seen`, so
        // its next line re-emits a parentless SessionStart), and —
        // the #246 case this adoption was built for — the IN-FLIGHT
        // multi-turn child: upstream provides no SessionStart carrier
        // at turn N+1 (hook_runtime.rs verified 2026-06-11), so the
        // child-end un-claim side-channel (`source/jsonl/unclaim.rs`
        // ChildEndUnclaims: the CC hook tee observes the SubagentStop
        // and the owning watcher releases the rollout's `seen` claim)
        // manufactures the JSONL first-sight carrier this arm
        // re-links. Adopt the remembered parent so the start that
        // does arrive re-joins the scope tree (cascade/liveness/
        // readiness) instead of registering as an orphan. Revivals
        // are deliberately NOT
        // blocked the way parented re-registrations are: Codex
        // resurrect-on-prompt is a legitimate same-id new life, and
        // for a genuinely dead child a parent-linked slot rides the
        // parent cascade / the 5-min Codex short-idle reap — strictly
        // better than the orphan phantom. The adopted link runs
        // through the #240 cycle filter below exactly like a
        // wire-carried one (a poisoned ledger degrades to parentless).
        let parent_id = parent_id.or_else(|| {
            self.corr
                .child_ledger
                .get(&agent_id)
                .and_then(|e| e.parent_id)
        });
        // Refuse a parent link whose ancestor chain reaches the child
        // — the ONE seam where `parent_id` is set (registration below)
        // or enriched (the orphan arm), so a cycle can never EXIST and
        // the scope walks need cycle guards only for termination: a
        // 2-cycle whose members are BOTH Waiting would mutually
        // satisfy `has_waiting_ancestor` and exempt each other from
        // `sweep_stale` forever (#238). Degrade to parentless — the
        // session is real even when its claimed lineage is malformed.
        // The check runs up here (the walk needs `&scene.agents`
        // before the `get_mut` below) but is gated on a link actually
        // being APPLIED — against an already-parented slot the
        // enrichment is a no-op, so a duplicate's malformed parent
        // must neither warn nor change anything.
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
            // Already created — usually a harmless duplicate from the
            // other transport. But a Codex subagent's own rollout
            // (JSONL) can create the slot ORPHANED before its
            // SubagentStart hook arrives with the parent link; enrich it
            // so the subagent joins the scope tree regardless of arrival
            // order. Never re-parent an agent that already has a parent.
            if slot.parent_id.is_none() {
                if let Some(p) = parent_id {
                    slot.parent_id = Some(p);
                    // An APPLIED link feeds the child ledger
                    // (#244/#246); clearing ended_at marks this life
                    // alive so gc can't prune the memory while the
                    // child still lives (the end/sweep re-stamps it).
                    let entry = self.corr.child_ledger.entry(agent_id).or_default();
                    entry.parent_id = Some(p);
                    entry.ended_at = None;
                }
            }
            // G4 back-fill: a slot can exist with MISSING identity
            // context — the hook-synthesis pre-pass registers from
            // events that carry only the AgentId (empty source/
            // session_id/cwd), and a Codex revive ghost has an empty
            // cwd. The first SessionStart carrying the missing piece
            // heals it; an established value is never overwritten
            // (first-wins, via the shared `backfill_identity` — the
            // hook `Identity` arm runs the same heal). Upgradability
            // is the label's minted PROVENANCE, so the source
            // back-fill running first can't re-contextualize it (the
            // old string-shape sniff had to be judged before).
            let label_is_upgradable = slot.label.is_upgradable();
            if let Some(base) = backfill_identity(slot, ctx) {
                // Upgrade ONLY a fallback label — a basename- or
                // Rename-derived label is real information. This stays
                // on the SessionStart path: `Identity` carries no
                // label authority.
                if label_is_upgradable {
                    // `base` is the event cwd's basename — content
                    // that BYPASSES the `cwd_basename_label`
                    // chokepoint, so it carries the same
                    // decode-boundary cap.
                    slot.label = crate::state::SlotLabel::cwd_derived(format!(
                        "{}·{}",
                        source_label_prefix(&slot.source),
                        crate::source::decoder::ellipsize(
                            base,
                            crate::source::decoder::MAX_DECODED_FIELD_CHARS,
                        )
                    ));
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
            // duplicate can't un-exit a b1-cascaded subagent. All three
            // conjuncts are load-bearing (`resurrect_in_place` has no exiting
            // guard, so a `&&`→`||` here WOULD reset a LIVE root — NOT an
            // equivalent mutant): the exiting conjunct is pinned by
            // `duplicate_root_session_start_does_not_resurrect_a_live_session`.
            // The two parent conjuncts are belt-and-braces —
            // `session_start_on_exiting_subagent_does_not_resurrect` blocks on the
            // slot's OWN `parent_id.is_some()` regardless of the incoming one, so
            // no existing test isolates either from the other (#612).
            if slot.exiting_at.is_some() && slot.parent_id.is_none() && parent_id.is_none() {
                // Route through fsm so an in-flight Active span is folded
                // into active_ms before the reset (every other
                // Active-exit site does; a direct `state = Idle` here
                // silently dropped it).
                fsm::resurrect_in_place(slot, now);
                // Evict the dead life's correlation state, exactly as
                // `sweep_exited` would have if the corpse had GC'd before this
                // start: a leftover active_tasks entry keeps
                // suppress_subagent_leak eating the new life's hooks, a stale
                // gate could false-resolve a future Waiting, and an armed b1
                // cascade would fire into the fresh subtree. recent_proof_of_life
                // deliberately SURVIVES — a resurrecting slot is by definition
                // still alive; the TTL-bounded hook-recency maps are gc's, not
                // per-slot. Pinned by
                // resurrect_in_place_clears_stale_active_tasks_so_fresh_session_hooks_apply
                // + resurrect_in_place_cancels_stale_pending_b1_cascade.
                self.corr.active_tasks.remove(&agent_id);
                self.corr.gated_before_waiting.remove(&agent_id);
                self.pending_b1_cascades.remove(&agent_id);
            }
            return;
        }
        if self.register_slot(scene, agent_id, ctx, parent_id, now) {
            // A CHILD registration feeds the ledger with its APPLIED
            // parent (a desk-exhaustion refusal records nothing — the
            // session was dropped, not registered); ended_at clears
            // for the new life, mirroring the enrichment above.
            if let Some(p) = parent_id {
                let entry = self.corr.child_ledger.entry(agent_id).or_default();
                entry.parent_id = Some(p);
                entry.ended_at = None;
            }
        }
    }

    /// Pre-pass 0 of [`Reducer::apply`] (hook transport only) — hook events
    /// are proof of life: synthesize a registration for a tool/permission
    /// event whose id has no slot, so a session whose transcript was gated at
    /// first sight becomes visible the moment it fires a hook. Only
    /// `ActivityStart`/`ActivityEnd`/`Waiting` qualify — each unambiguously
    /// proves a live session; `SessionEnd` (nothing to remove) and `Rename`
    /// (nothing to relabel) stay no-ops for an unknown id.
    ///
    /// The decoded activity events carry no identity context beyond the
    /// `AgentId` (no source / session_id / cwd — and the id is a hash, not
    /// reversible), so the slot starts blank with the bare ordinal label
    /// (`#N`); a later real `SessionStart` back-fills it (see the
    /// duplicate-SessionStart arm). Since #221 the hook decoders attach an
    /// [`AgentEvent::Identity`] AHEAD of tool/permission events, so the slot
    /// normally already exists — with real identity — by the time the activity
    /// event applies; this blank path remains the fallback for identity-less
    /// hook events (`Stop`, directly-constructed events). Routed through
    /// [`Reducer::register_slot`] so the desk-capacity gate applies the same
    /// as for a real `SessionStart`.
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
        // A tombstoned id just had its hook SessionEnd arrive with no slot:
        // this event is a reordered trailing straggler from the DEAD session
        // (per-connection hook tasks reorder), not proof of new life.
        // Synthesizing would mint a blank Idle ghost that no future
        // SessionEnd can remove — only the 30-min idle sweep.
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
                // NOT an unknown-cwd ghost: the 3-min reap exists for startup
                // JSONL-seeding artifacts that never get a follow-up event.
                // This slot is process-proven alive, and the motivating case —
                // parked on a permission prompt, appending nothing — emits no
                // further event within 3 min, so the ghost reap would kill it
                // before any JSONL revive could back-fill. It rides the normal
                // state-adaptive timeouts instead; the cost is a normal-length
                // linger if the process dies right after — the same linger any
                // abrupt exit already has.
                slot.unknown_cwd = false;
            }
        }
    }

    /// The slot-creation half of the `SessionStart` arm, shared with
    /// [`Reducer::synthesize_hook_registration`] so both run the same
    /// desk-capacity gate + label derivation. Returns `false` when all desks
    /// are occupied (the session is dropped, consuming no ghost ordinal).
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
        let prefix = source_label_prefix(source);
        // The cwd is hook/transcript CONTENT — route the basename through the
        // `cwd_basename_label` chokepoint so the label is capped at the decode
        // boundary, exactly like the JSONL derivers and the duplicate-
        // SessionStart backfill upgrade (this is the sole mint site for
        // hook-only sources).
        let named = crate::source::decoder::cwd_basename_label(prefix, cwd);
        let has_cwd = named.is_some();
        let label = match named {
            Some(l) => crate::state::SlotLabel::cwd_derived(l),
            None => {
                // Only an unknown-cwd ghost consumes an ordinal, so labels
                // stay contiguous (cc#1, cc#2, …) instead of skipping the
                // count of preceding named sessions.
                self.next_label_n += 1;
                crate::state::SlotLabel::ordinal_ghost(format!("{prefix}#{}", self.next_label_n))
            }
        };
        // Disambiguation for multiple sessions sharing a cwd happens
        // at render time, not here — we don't want to suffix unique
        // sessions with a noisy `·xxxx` they don't need.
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
                // The 3-min unknown-cwd reap exists for startup JSONL-seeding
                // GHOSTS (a blind first-sight with no identity). A PARENTED slot
                // came from an explicit subagent signal (e.g. Copilot's
                // `subagent.started`, whose payload carries no cwd) — it is a
                // real, process-proven child, not a ghost, so it must ride the
                // normal lifecycle (parent cascade / child-end / Idle backstop),
                // never the 3-min reap. Same rationale as the hook-synthesis
                // exemption above; without it a sub-agent running > 3 min with no
                // cwd-bearing event is swept while alive.
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
            // The misattributed subagent event already refreshed the
            // parent's lineage in `apply` (liveness flows up), keeping the
            // delegating parent from being wrongly stale-swept.
            //
            // One state change still belongs to the parent: if it is
            // `Waiting` while delegating, that Waiting is usually the
            // SUBAGENT's permission gate (the `Notification` was
            // misattributed to the parent). A suppressed child event means
            // the subagent resumed work, so the gate resolved: restore
            // Active(Delegating) instead of leaving a stale "permission?"
            // Waiting until the 60-min stale-sweep. Then drop the spurious
            // display update.
            //
            // CONDITIONAL on the gate: a delegating parent CAN run its own
            // parallel ordinary tool (applied via JSONL — suppression is
            // hook-only; pinned by `task_drain_keeps_parallel_ordinary_tool_
            // gate`), and when THAT tool was the one gated at Waiting-entry,
            // the gate holds an ordinary tuid ∉ active_tasks — the prompt is
            // the PARENT's own and still pending in CC. The subagent working
            // on its independent loop proves nothing about it: keep the
            // Waiting and the gate, so the own tool's END can still resolve
            // it. A Task-tuid gate (∈ active_tasks — Waiting fired while
            // Active(Delegating)) or no gate means the subagent's prompt:
            // restore.
            if let Some(slot) = scene.agents.get_mut(&id) {
                if matches!(slot.state, ActivityState::Waiting { .. }) {
                    let gate_is_own_tool = self
                        .corr
                        .gated_before_waiting
                        .get(&id)
                        .is_some_and(|g| !tasks.is_some_and(|s| s.contains(&**g)));
                    if !gate_is_own_tool {
                        let task_tuid = tasks
                            .and_then(|s| s.iter().next())
                            .map(|t| Arc::<str>::from(t.as_str()));
                        fsm::enter_delegating(slot, task_tuid, now);
                        self.corr.gated_before_waiting.remove(&id);
                    }
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
                // Delegating only on the FIRST insert: the in-window JSONL copy
                // of a hook dispatch is dedup-dropped upstream, but real
                // hook↔JSONL skew runs past HOOK_WINS_WINDOW (B1_CASCADE_GRACE
                // models it at ~2.5s), and an out-of-window replay of an
                // already-tracked tuid (`insert() == false`) re-firing
                // enter_delegating would clobber a parent that went Waiting on
                // its own permission prompt since. The flag above stays
                // unconditional either way, so the duplicate can't fall through
                // to the main arm's enter_active instead. (A suppressed
                // parallel dispatch's JSONL copy is a genuine first insert —
                // the suppressed hook returned before reaching this tracker —
                // so it still enters Delegating.)
                //
                // The drained-tuid tombstone closes the gate's residual: once
                // the hook End DRAINS the tuid, a pair-replay's Start IS a
                // fresh first insert — re-insert it for End symmetry (the
                // replayed End re-drains it cleanly), but never re-enter
                // Delegating for a Task that already completed. `gc` (run at
                // apply-top with this same `now`) already pruned expired
                // tombstones, so membership means fresh.
                //
                // Not slot-gated (unlike the hook-wins dedup record): the orphan
                // is harmless + reaped by tick — see CLAUDE.md "active_tasks insert".
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
                        // #152: the gate recorded while Delegating holds THIS
                        // Task's tuid; the drain path deliberately skips the
                        // main arm (a Waiting parent must keep Waiting), so
                        // the entry would go stale — and a later
                        // out-of-window JSONL replay of this END would
                        // false-match it via resolves_wait and flip a
                        // still-pending permission to Idle. Clear only OUR
                        // tuid: a parallel ordinary tool's gate must survive
                        // the drain.
                        if self.corr.gated_before_waiting.get(agent_id).map(|g| &**g)
                            == Some(tuid.as_str())
                        {
                            self.corr.gated_before_waiting.remove(agent_id);
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
            if self
                .corr
                .active_tasks
                .get(&id)
                .is_some_and(|s| !s.is_empty())
            {
                continue;
            }
            tracing::debug!(agent_id = ?id, "b1 grace elapsed — cascading completed subtree");
            // StampRoot::No: the delegating parent keeps running; only its
            // finished subtree walks out.
            scope::cascade_exit(scene, id, scope::StampRoot::No, now);
        }
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
                // Probe-vouched exemption (#220): a recent ProofOfLife means
                // the owning process is alive RIGHT NOW — event silence is not
                // death (permission-parked after attach-replay, long-idle).
                // Once emissions stop the entry ages out and the normal sweep
                // resumes.
                if self.corr.vouch_fresh(&slot.agent_id, now) {
                    return None;
                }
                // The vouch extends to a vouched ancestor's DELEGATED subtree:
                // the probe never vouches subagent ids (their stems are
                // `agent-<id>`, not session UUIDs), and a permission-parked
                // parent renders Active after attach-replay (not Waiting), so
                // `has_waiting_ancestor` can't fire for its blocked-but-live
                // child — which would be swept unrecoverably (its JSONL events
                // become unknown-id no-ops; its hooks attribute to the
                // parent). Gated on the ancestor ACTIVELY delegating (a
                // non-empty `active_tasks` entry) so a completed lingering
                // child — the b1 chained-dispatch residual — keeps the 30-min
                // idle backstop.
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
            }
            scope::cascade_exit(scene, id, scope::StampRoot::Yes, now);
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
            self.corr.active_tasks.remove(&id);
            // Symmetric with active_tasks: sweep_exited runs on the apply path
            // (not just tick), where the tick-time `gated_before_waiting.retain`
            // doesn't run — so reclaim it here too, else a Waiting slot that was
            // swept mid-turn leaks its gated tool_use_id until the next tick.
            self.corr.gated_before_waiting.remove(&id);
            self.pending_b1_cascades.remove(&id);
            // The gc TTL retain bounds this map anyway; evicting with the slot
            // (like the per-agent siblings above) keeps a removed id from
            // exempting a same-id resurrect ghost inside the TTL window.
            self.corr.recent_proof_of_life.remove(&id);
            // A CHILD whose end wasn't `as_child` (stale-swept, parent-
            // cascaded, negative-vouched) starts its ledger GC clock at slot
            // removal, keeping the map bounded — and arming the #244-w2 gate
            // for those exits too. `get_or_insert` keeps an earlier as_child
            // stamp; roots have no entry, so this scopes itself to children.
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
