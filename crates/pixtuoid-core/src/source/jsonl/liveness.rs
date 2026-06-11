use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::debug;

use crate::source::exit_watch::ExitWatch;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

use super::walk::walk_jsonl;
use super::{SourceDecoders, WatchCtx};

/// One healthy liveness-probe observation: which agent processes are verified
/// alive RIGHT NOW, and which OS pid owns each.
#[derive(Debug, Clone, Default)]
pub struct ProbeSnapshot {
    /// Session ids (`IdDeriver` id-space) of agent processes verified alive
    /// right now.
    pub ids: HashSet<String>,
    /// id → owning OS pid, for the exit watch (many ids may share one pid —
    /// one codex process holds every rollout it has open).
    pub pid_of: HashMap<String, i32>,
}

/// Optional first-party liveness probe: returns the session ids — in the
/// source's `IdDeriver` id-space — of agent processes known to be ALIVE right
/// now (e.g. CC's `~/.claude/sessions/<pid>.json` registry). ADDITIVE-ONLY for
/// admission: membership bypasses the first-sight recency/ended gate (a
/// live-but-idle session is read from the top however old its mtime).
/// Failure is EXPLICIT: `None` means the probe itself FAILED (the enumeration
/// errored — unreadable registry dir, proc-table failure) and callers must
/// change NOTHING; `Some` with empty `ids` means the probe ran fine and
/// nothing is alive (meaningful!). Absence of an id is therefore only
/// meaningful in a `Some` snapshot — which is what lets a previously-vouched
/// id MISSING from two healthy snapshots count as a high-confidence exit (the
/// negative vouch, #223). `Arc<dyn Fn>` rather than a fn pointer like the
/// other seams because the real probe captures its registry dir (the others
/// are stateless).
pub type LivenessProbe = Arc<dyn Fn() -> Option<ProbeSnapshot> + Send + Sync>;

/// Negative vouch (#223): a previously-vouched id must be MISSING from two
/// healthy probe snapshots at least this far apart before its exit is
/// confirmed. Two observations ≥60s apart make the signal immune to Codex's
/// brief drop-and-reopen fd gap on a write failure and to the initial-seed /
/// 250ms-rescan adjacency (back-to-back snapshots seconds apart can never
/// confirm on their own).
pub(super) const NEGATIVE_VOUCH_MIN_SPAN: Duration = Duration::from_secs(60);

/// Whether the liveness probe vouches for this transcript: its derived session
/// id appears in the most recent live-session snapshot. A vouched-for file is
/// a RUNNING agent however old its mtime (long-idle, delegating to subagents,
/// or stuck in a long tool call), so the first-sight gate must not hide it.
/// Subagent transcripts can never match — their stems are agent ids
/// (`agent-<id>`), not session UUIDs, so only the root transcript is admitted.
/// The empty-set check short-circuits the id derivation (an allocation) in the
/// no-probe case.
pub(super) async fn probe_admits(
    path: &Path,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) -> bool {
    let live = ctx.live.lock().await;
    !live.is_empty() && live.contains(&(decoders.id_derive)(path))
}

/// #220: the probe is ONGOING liveness, not just admission. After each probe
/// refresh (initial seed / 250ms rescan / 60s poll — the same three sites that
/// re-snapshot `live`) emit a `ProofOfLife` per vouched id so the reducer can
/// hold its sweep exemption while the process lives; when the live signal
/// disappears the emissions stop and the exemption ages out. Runs AFTER
/// `scan_root` so freshly admitted sessions already have slots (ordering is
/// cosmetic — an unknown-id ProofOfLife is a reducer no-op — but it spares a
/// wasted pass). An empty snapshot (no probe wired / nothing live) sends
/// nothing. A closed channel is ignored like the other sends (shutdown path).
pub(super) async fn emit_proof_of_life(
    live: &Arc<Mutex<HashSet<String>>>,
    source: &Arc<str>,
    tx: &TaggedSender,
) {
    // Snapshot before sending: holding the lock across `tx.send` would block
    // probe refreshes on a slow consumer for no reason.
    let ids: Vec<AgentId> = live
        .lock()
        .await
        .iter()
        .map(|sid| AgentId::from_parts(source, sid))
        .collect();
    for agent_id in ids {
        let _ = tx
            .send((Transport::Jsonl, AgentEvent::ProofOfLife { agent_id }))
            .await;
    }
}

/// #223: the negative-vouch ledger. A session id the probe previously vouched
/// for that DISAPPEARS from a healthy snapshot is a high-confidence exit — the
/// registry entry was removed / the rollout fd closed, signals only the OWNING
/// process can produce — so the watcher can emit the `SessionEnd` the CLI
/// never writes (Codex has no exit signal of any kind; CC's hook is
/// best-effort) instead of waiting out the 10–30 min stale-sweep.
/// Confirmation needs the id missing from two healthy observations at least
/// `min_span` apart (see [`NEGATIVE_VOUCH_MIN_SPAN`]); a probe FAILURE
/// (`None`) is never an observation — failure changes nothing.
pub(super) struct NegativeVouch {
    min_span: Duration,
    /// Ids vouched by an earlier healthy snapshot. An id stays "previously
    /// vouched" while its miss window runs, so the second observation can
    /// confirm it.
    prev_vouched: HashSet<String>,
    /// id → when a healthy snapshot FIRST came back without it. `Instant`
    /// (monotonic): a wall-clock jump must not fake a 60s span.
    miss_since: HashMap<String, std::time::Instant>,
}

impl NegativeVouch {
    pub(super) fn new(min_span: Duration) -> Self {
        Self {
            min_span,
            prev_vouched: HashSet::new(),
            miss_since: HashMap::new(),
        }
    }

    /// Fold one HEALTHY snapshot into the ledger, emitting a confirmed exit's
    /// `SessionEnd` (+ the `seen` un-claim) through `ctx` and dropping the
    /// confirmed id from `pid_bindings`. Never called on a probe failure —
    /// the caller (`refresh_probe_snapshot`) only forwards `Some` snapshots.
    async fn observe(
        &mut self,
        snap: &ProbeSnapshot,
        decoders: SourceDecoders,
        ctx: &WatchCtx<'_>,
        pid_bindings: &mut HashMap<i32, HashSet<String>>,
    ) {
        let now = std::time::Instant::now();
        // A re-appearing id (fd reopened, registry entry back) cancels its
        // pending miss window.
        self.miss_since.retain(|id, _| !snap.ids.contains(id));
        let missing: Vec<String> = self.prev_vouched.difference(&snap.ids).cloned().collect();
        for id in missing {
            match self.miss_since.get(&id) {
                // Second healthy miss past the span — confirmed exit.
                Some(first_miss) if now.duration_since(*first_miss) >= self.min_span => {
                    debug!(
                        "negative vouch confirmed for {id}: probe stopped vouching; \
                         emitting SessionEnd"
                    );
                    emit_session_exit(&id, decoders, ctx).await;
                    // Also drop the id's pid binding: a codex-style process
                    // owns many rollouts, so it may outlive this session —
                    // its eventual OS exit must not re-emit a SessionEnd for
                    // an id whose end was already confirmed here.
                    unbind_session(pid_bindings, &id);
                    self.prev_vouched.remove(&id);
                    self.miss_since.remove(&id);
                }
                // Window still running — wait for a later snapshot.
                Some(_) => {}
                // First miss — open the window, keep the id vouched.
                None => {
                    self.miss_since.insert(id, now);
                }
            }
        }
        // Previously-vouched = the current snapshot ∪ ids whose miss window
        // still runs (they must stay eligible for the confirming observation).
        self.prev_vouched = snap.ids.clone();
        self.prev_vouched.extend(self.miss_since.keys().cloned());
    }

    /// Remove `id` from the ledger WITHOUT confirming anything — the
    /// instant-exit arm already emitted its SessionEnd, so a later healthy
    /// snapshot must not open/age a miss window toward re-confirming it (a
    /// duplicate SessionEnd would be a reducer no-op, but the ledger should
    /// not be left armed for one).
    pub(super) fn forget(&mut self, id: &str) {
        self.prev_vouched.remove(id);
        self.miss_since.remove(id);
    }
}

/// ONE exit path for every watcher-synthesized session end — shared by the
/// negative-vouch confirmation and the instant-exit arm so the two can't
/// fork: drain the session's pending bytes, emit the `SessionEnd` the CLI
/// never wrote, then un-claim first-sight for every registered path of this
/// session so a LATER append re-registers through `emit_first_sight` (a
/// resumed session walks back in; a wrongly-ended live one self-heals on its
/// next write or re-vouch).
///
/// The drain-FIRST ordering is load-bearing: the decoded-SessionEnd path in
/// `walk_jsonl` un-claims with its cursor already at EOF past the terminator,
/// so any later bytes are genuinely post-end. The instant exit can beat a
/// pre-death write's notify event by orders of magnitude — un-claiming with
/// bytes still pending would let that stale chunk re-enter `walk_jsonl` as a
/// first-sight and resurrect the dead session as a ghost, with every fast
/// rung already disarmed for it (pid unbound, vouch forgotten): reaped only
/// by the 10–30 min stale sweep, the exact ladder #223 exists to climb.
/// Draining through the normal decode path parks the cursor at EOF, so the
/// straggler notify walk no-ops. (A drained chunk decoding its own
/// `SessionEnd` un-claims + terminates inside `walk_jsonl`; the duplicate
/// terminator below is a reducer no-op. For the negative-vouch caller the
/// drain is itself a no-op — its poll tick ran `scan_root` just before.)
///
/// Racing the #246 side-channel: a path the child-end drain already RELEASED
/// (`seen == false`) is still collected here, and its drain can transiently
/// RE-register the child from post-release bytes before the `SessionEnd`
/// below lands right behind it — a ≤`EXIT_GRACE_WINDOW` walkout ghost (or a
/// fully-swallowed one inside the grace), self-correcting, no claim leak.
pub(super) async fn emit_session_exit(id: &str, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let claimed: Vec<PathBuf> = {
        let seen = ctx.seen.lock().await;
        seen.keys()
            .filter(|p| (decoders.id_derive)(p) == id)
            .cloned()
            .collect()
    };
    for path in &claimed {
        walk_jsonl(path, decoders, ctx).await;
    }
    let agent_id = AgentId::from_parts(ctx.source, id);
    let _ = ctx
        .tx
        .send((
            Transport::Jsonl,
            AgentEvent::SessionEnd {
                agent_id,
                as_child: false,
            },
        ))
        .await;
    {
        let mut seen = ctx.seen.lock().await;
        for path in &claimed {
            seen.remove(path);
        }
    }
    // Purge the admission set too: `live` is otherwise only rewritten by a
    // HEALTHY probe refresh, so after an instant exit a probe-FAILURE pass
    // (failure changes nothing — the stale snapshot stays) would keep
    // vouching the id this watcher just declared dead, and the re-vouch
    // sweep would re-admit its parked file (cursor reset to 0 → full replay
    // → a phantom SessionStart) with every fast rung already disarmed for
    // it. For the negative-vouch caller this is a no-op — its refresh
    // already replaced `live` with a snapshot that lacks the id.
    ctx.live.lock().await.remove(id);
}

/// Remove one session id from every pid's binding set, dropping pids whose
/// set empties — the keep-state-clean half of the instant-exit ↔ negative-
/// vouch handshake (its inverse is `NegativeVouch::forget`).
pub(super) fn unbind_session(pid_bindings: &mut HashMap<i32, HashSet<String>>, id: &str) {
    pid_bindings.retain(|_, ids| {
        ids.remove(id);
        !ids.is_empty()
    });
}

/// ONE probe refresh, shared by the three sites that re-snapshot `live` (the
/// initial seed, the 250ms rescan, the 60s poll). On a HEALTHY snapshot
/// (`Some`): replace the admission set, fold the snapshot into the
/// negative-vouch ledger, then fold the id→pid bindings — registering every
/// newly-seen pid with the exit watch (#223 rung 2); returns true so the
/// caller re-emits `ProofOfLife` after its scan. On a probe FAILURE (`None`)
/// or no probe wired: change NOTHING — `ctx.live` keeps the previous ids
/// (admission stays additive), the miss windows neither advance nor confirm,
/// no bindings move, no `ProofOfLife` is emitted (the reducer's TTL absorbs
/// the gap).
pub(super) async fn refresh_probe_snapshot(
    probe: Option<&LivenessProbe>,
    vouch: &mut NegativeVouch,
    pid_bindings: &mut HashMap<i32, HashSet<String>>,
    exit_watch: Option<&ExitWatch>,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) -> bool {
    let Some(probe) = probe else {
        return false;
    };
    let Some(snap) = probe() else {
        debug!("liveness probe failed; keeping the previous snapshot (failure changes nothing)");
        return false;
    };
    *ctx.live.lock().await = snap.ids.clone();
    vouch.observe(&snap, decoders, ctx, pid_bindings).await;
    // Bindings are ADDITIVE per snapshot (ids leave via the instant-exit arm
    // or the negative-vouch unbind above, never by snapshot omission — the
    // vouch ladder owns "gone" semantics) — EXCEPT an observed rebind: the
    // snapshot's `pid_of` is the probe's current ownership ground truth, so
    // an id seen under a NEW pid (a codex `resume` of the same rollout in
    // another process while the old one lives) migrates between sets. The
    // negative vouch can't clean that stale binding (the id stays vouched
    // under the new pid, so no miss window ever opens), and the old pid's
    // later death would otherwise instant-exit a session that is alive. A
    // pid is registered with the exit watch only on its FIRST appearance; if
    // that registration failed kernel-side (EPERM), it is not retried — the
    // slower rungs cover.
    for (id, pid) in &snap.pid_of {
        let bound_elsewhere = pid_bindings
            .iter()
            .any(|(p, ids)| p != pid && ids.contains(id));
        if bound_elsewhere {
            unbind_session(pid_bindings, id);
        }
        let newly_seen = !pid_bindings.contains_key(pid);
        pid_bindings.entry(*pid).or_default().insert(id.clone());
        if newly_seen {
            if let Some(watch) = exit_watch {
                watch.watch(*pid);
            }
        }
    }
    true
}

/// The probe is consulted only in `walk_jsonl`'s !known first-sight branch, so
/// a TRANSIENT probe miss (registry file mid-rewrite, a read race) would gate
/// a live session PERMANENTLY — every later pass exits at `cursor == file_len`
/// and never asks again. On each SCAN pass (the snapshot in `ctx.live` was
/// just refreshed; notify single-file walks don't run this), re-ask about
/// every file that is known-but-never-registered (cursor parked at EOF, no
/// `seen` claim) and reset a vouched one's cursor to 0 so this same pass's
/// walk replays/registers it (≤1 MiB replays; an oversized body lands in the
/// #204 head-read registration branch).
///
/// Cannot loop: a re-vouched file that registers claims `seen` and drops out
/// of the candidate set; one whose replay turns out ENDED is re-parked at EOF
/// unregistered (the oversized branch's ended skip) — it re-enters at most
/// once per scan pass, and only while the probe actively (mis)vouches for it.
/// Locking is sequential short locks on the sibling maps, never nested — the
/// watcher is a single task, so a snapshot race is theoretical.
pub(super) async fn revouch_gated_files(decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    // Empty snapshot = no probe wired, or nothing live: skip the sweep so
    // probe-less sources (Antigravity) pay one lock check per pass,
    // not a metadata read per gated file.
    if ctx.live.lock().await.is_empty() {
        return;
    }
    let candidates: Vec<(PathBuf, u64)> = {
        let cursors = ctx.cursors.lock().await;
        cursors.iter().map(|(p, c)| (p.clone(), *c)).collect()
    };
    for (path, cursor) in candidates {
        // ANY entry skips — including a RELEASED claim (`false`, #246): the
        // probe legitimately vouches a multi-turn child's still-open rollout,
        // but replaying it would re-register the just-ended child with a
        // burst of stale activity. A released path revives only on NEW bytes.
        if ctx.seen.lock().await.contains_key(&path) {
            continue;
        }
        // Only a file parked exactly at EOF is stuck — one with a pending
        // append revives through the normal walk on this same pass.
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if meta.len() != cursor {
            continue;
        }
        if !probe_admits(&path, decoders, ctx).await {
            continue;
        }
        ctx.cursors.lock().await.insert(path, 0);
    }
}
