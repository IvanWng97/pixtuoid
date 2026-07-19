use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::debug;

use crate::source::exit_watch::ExitWatch;
use crate::source::{fd_probe, AgentEvent, TaggedSender, Transport};
use crate::AgentId;

use super::walk::{park_if_truncated_below_cursor, walk_jsonl};
use super::{SourceDecoders, WatchCtx};

/// One healthy liveness-probe observation: which agent processes are verified
/// alive RIGHT NOW, and which OS pid owns each. The set of live session ids IS
/// `pid_of`'s key set — a live id always has an owning pid (both probes bind
/// them together), so a separate `ids` set could only denormalize it into a
/// representable "id without pid" illegal state. Read the id set via the
/// `ids()` / `contains()` / `is_empty()` accessors.
#[derive(Debug, Clone, Default)]
pub struct ProbeSnapshot {
    /// id → owning OS pid, for the exit watch (many ids may share one pid —
    /// one codex process holds every rollout it has open). The keys are the
    /// live session ids (`IdDeriver` id-space).
    pub pid_of: HashMap<String, i32>,
}

impl ProbeSnapshot {
    /// The live session ids (`pid_of`'s keys) — the former `ids` field.
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.pid_of.keys()
    }
    /// Whether `id` is verified alive in this snapshot.
    pub fn contains(&self, id: &str) -> bool {
        self.pid_of.contains_key(id)
    }
    /// Whether the probe saw no live sessions (ran fine, nothing alive).
    pub fn is_empty(&self) -> bool {
        self.pid_of.is_empty()
    }

    /// Bind `id` to its owning `pid`, resolving a contested id (two live
    /// processes holding one file open — a resume overlap) deterministically:
    /// the LARGER pid wins — arbitrary but stable, never proc-enumeration
    /// order (#252). Shared by every probe that binds ids to pids (the two
    /// open-FD sources via [`from_open_fd_pairs`], grok's registry join).
    pub(crate) fn bind_pid(&mut self, id: String, pid: i32) {
        let bound = self.pid_of.entry(id).or_insert(pid);
        if pid > *bound {
            *bound = pid;
        }
    }

    /// Build a snapshot from the regular files that live `comm_names`-named
    /// processes hold open under `root` — THE producer-side skeleton the
    /// open-FD liveness sources (Codex, omp) share. `accept` is the ONLY
    /// per-source knowledge (invariant #3): it decides whether a held-open
    /// path is one of this source's transcripts and, if so, returns its
    /// session id in the watcher's `IdDeriver` id-space (so probe ids join the
    /// first-sight gate directly). Everything mechanical lives here so a NEW
    /// fd-probe source can't re-derive it wrong: the #223 canonicalize-or-
    /// `Some(empty)` failure semantics, the under-root filter, the #252 bind.
    ///
    /// `None` ONLY when the proc-table enumeration itself fails
    /// (`fd_probe::pids_by_name` → `None`) — the watcher then changes nothing
    /// (#223). An ABSENT / un-canonicalizable `root` is NOT a failure (the
    /// source may simply never have run) → `Some(empty)`, a healthy "nothing
    /// alive" observation.
    pub(crate) fn from_open_fds(
        root: &Path,
        comm_names: &[&str],
        accept: impl Fn(&Path) -> Option<String>,
    ) -> Option<Self> {
        // Canonicalize once per call: kernel-reported fd paths are fully
        // resolved (/tmp → /private/tmp on macOS), so the under-root compare
        // must run against the canonical root or every match misses.
        let Ok(canonical) = root.canonicalize() else {
            debug!(
                "fd probe: root {} not canonicalizable; nothing alive there",
                root.display()
            );
            return Some(Self::default());
        };
        let mut pids = Vec::new();
        for name in comm_names {
            pids.extend(fd_probe::pids_by_name(name)?);
        }
        let pairs = pids.into_iter().flat_map(|pid| {
            fd_probe::open_vnode_paths(pid)
                .into_iter()
                .map(move |path| (pid, path))
        });
        Some(Self::from_open_fd_pairs(&canonical, pairs, accept))
    }

    /// The PURE join half of [`from_open_fds`] (unit-testable without FFI —
    /// drive it with synthetic `(pid, path)` pairs): keep each pair whose path
    /// is under `root` and `accept`ed, binding id → owning pid via
    /// [`bind_pid`] (the #252 tiebreak).
    pub(crate) fn from_open_fd_pairs(
        root: &Path,
        pairs: impl Iterator<Item = (i32, PathBuf)>,
        accept: impl Fn(&Path) -> Option<String>,
    ) -> Self {
        let mut snap = Self::default();
        for (pid, path) in pairs {
            if !path.starts_with(root) {
                continue;
            }
            if let Some(id) = accept(&path) {
                debug!(
                    "fd probe: pid {pid} holds {} open (id {id})",
                    path.display()
                );
                snap.bind_pid(id, pid);
            }
        }
        snap
    }
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

/// #223: the probe ladder's DERIVED liveness state, given one owner. A session
/// id the probe previously vouched for that DISAPPEARS from a healthy snapshot
/// is a high-confidence exit — the registry entry was removed / the rollout fd
/// closed, signals only the OWNING process can produce — so the watcher can
/// emit the `SessionEnd` the CLI never writes (Codex has no exit signal of any
/// kind; CC's hook is best-effort) instead of waiting out the 10–30 min
/// stale-sweep. Confirmation needs the id missing from two healthy observations
/// at least `min_span` apart (see [`NEGATIVE_VOUCH_MIN_SPAN`]); a probe FAILURE
/// (`None`) is never an observation — failure changes nothing.
///
/// This is a **pure failure-detector module** (functional core / imperative
/// shell): [`fold`](ProbeLadder::fold) takes a snapshot + `now` and RETURNS the
/// effects to apply ([`ProbeOutcome`]) — it never emits, never touches the
/// watcher channel or the exit watch. The imperative shell
/// (`refresh_probe_snapshot`) performs those effects, so the ladder is
/// unit-testable with synthetic time and zero mocks. It owns BOTH the vouch
/// ledger AND the `pid → ids` bindings the instant-exit rung joins on, so the
/// `forget`↔`unbind` inverse (once a prose contract split across a free fn + a
/// method) is now two private methods one owner keeps in lockstep.
pub(super) struct ProbeLadder {
    min_span: Duration,
    /// Ids vouched by an earlier healthy snapshot. An id stays "previously
    /// vouched" while its miss window runs, so the second observation can
    /// confirm it.
    prev_vouched: HashSet<String>,
    /// id → when a healthy snapshot FIRST came back without it. `Instant`
    /// (monotonic): a wall-clock jump must not fake a 60s span.
    miss_since: HashMap<String, std::time::Instant>,
    /// pid → the session ids a healthy snapshot bound to it, for the exit watch
    /// (many ids may share one pid — one codex process holds every rollout it
    /// has open). An id is bound under at most ONE pid (`fold`'s migration
    /// maintains this). Entries leave on a whole-pid death (`pid_died`), a
    /// negative-vouch confirm (single id, `unbind`), or a rebind migration.
    pid_bindings: HashMap<i32, HashSet<String>>,
}

/// What one [`ProbeLadder::fold`] decided the imperative shell must DO
/// (functional-core → imperative-shell): the confirmed-exit ids to emit
/// `SessionEnd` for, and the newly-seen pids to register with the exit watch.
/// Both are DATA; the shell applies them in the pre-owner order (emit BEFORE
/// watch — `exit_watch.watch` sends nothing synchronously to the channel), so
/// the `AgentEvent` stream stays byte-identical.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ProbeOutcome {
    pub exits: Vec<String>,
    pub newly_watched: Vec<i32>,
}

impl ProbeLadder {
    pub(super) fn new(min_span: Duration) -> Self {
        Self {
            min_span,
            prev_vouched: HashSet::new(),
            miss_since: HashMap::new(),
            pid_bindings: HashMap::new(),
        }
    }

    /// Fold one HEALTHY snapshot: advance the negative-vouch miss windows
    /// (confirming an id missing from two snapshots ≥ `min_span` apart) and the
    /// pid bindings (migrate a rebind, note a newly-seen pid), returning the
    /// effects — confirmed-exit ids + newly-seen pids — for the shell to emit +
    /// watch. PURE: no `ctx`, no `tx`, no `exit_watch`; `now` is a parameter so
    /// a test drives the span with synthetic time. Never called on a probe
    /// FAILURE — the shell forwards only `Some` snapshots (failure changes
    /// nothing).
    pub(super) fn fold(&mut self, snap: &ProbeSnapshot, now: std::time::Instant) -> ProbeOutcome {
        // A re-appearing id (fd reopened, registry entry back) cancels its
        // pending miss window.
        self.miss_since.retain(|id, _| !snap.contains(id));
        let missing: Vec<String> = self
            .prev_vouched
            .iter()
            .filter(|id| !snap.contains(id))
            .cloned()
            .collect();
        let mut exits = Vec::new();
        for id in missing {
            match self.miss_since.get(&id) {
                // Second healthy miss past the span — confirmed exit.
                Some(first_miss) if now.duration_since(*first_miss) >= self.min_span => {
                    debug!(
                        "negative vouch confirmed for {id}: probe stopped vouching; \
                         emitting SessionEnd"
                    );
                    // Drop the pid binding too: a codex-style process owns many
                    // rollouts, so it may outlive this session — its eventual OS
                    // exit must not re-emit a SessionEnd for an already-confirmed
                    // id. `forget`↔`unbind` are the ledger↔bindings inverse.
                    self.forget(&id);
                    self.unbind(&id);
                    exits.push(id);
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
        self.prev_vouched = snap.pid_of.keys().cloned().collect();
        self.prev_vouched.extend(self.miss_since.keys().cloned());

        // Bindings are ADDITIVE per snapshot (ids leave via `pid_died` or the
        // confirm-unbind above, never by snapshot omission — the vouch ladder
        // owns "gone" semantics) — EXCEPT an observed rebind: `snap.pid_of` is
        // the probe's ownership ground truth, so an id seen under a NEW pid (a
        // codex `resume` of the same rollout in another process while the old
        // one lives) migrates between sets. The negative vouch can't clean that
        // stale binding (the id stays vouched under the new pid), and the old
        // pid's later death would otherwise instant-exit a live session. A pid
        // is RETURNED for exit-watch registration only on its FIRST appearance.
        let mut newly_watched = Vec::new();
        for (id, pid) in &snap.pid_of {
            // find-then-compare (not `any(p != pid && contains)`): an id is
            // bound under at most ONE pid (this very loop maintains that —
            // unbind on migration, insert once), so the first holder is the
            // only holder, and the single `!=` leaves no conjunction whose
            // halves a mutation could silently swap.
            let bound_elsewhere = self
                .pid_bindings
                .iter()
                .find(|(_, ids)| ids.contains(id))
                .is_some_and(|(p, _)| p != pid);
            if bound_elsewhere {
                self.unbind(id);
            }
            let newly_seen = !self.pid_bindings.contains_key(pid);
            self.pid_bindings
                .entry(*pid)
                .or_default()
                .insert(id.clone());
            if newly_seen {
                newly_watched.push(*pid);
            }
        }
        ProbeOutcome {
            exits,
            newly_watched,
        }
    }

    /// The instant-exit rung: the watched OS process `pid` died. Remove its
    /// whole binding and `forget` every id it held (so the slower negative-vouch
    /// rung can't re-confirm an exit this rung is about to emit), returning
    /// those ids for the shell to emit `SessionEnd` for. An unknown pid (already
    /// unbound by a negative-vouch confirm, or a duplicate event) returns empty.
    pub(super) fn pid_died(&mut self, pid: i32) -> Vec<String> {
        let ids: Vec<String> = self
            .pid_bindings
            .remove(&pid)
            .into_iter()
            .flatten()
            .collect();
        for id in &ids {
            self.forget(id);
        }
        ids
    }

    /// Disarm the negative-vouch ledger for `id` WITHOUT confirming anything —
    /// a later healthy snapshot must not open/age a miss window toward
    /// re-confirming an exit a faster rung already emitted. Private: only
    /// `fold`/`pid_died` drive it, always paired with `unbind` (its bindings
    /// inverse) so the two halves can't drift.
    fn forget(&mut self, id: &str) {
        self.prev_vouched.remove(id);
        self.miss_since.remove(id);
    }

    /// Remove one session id from every pid's binding set, dropping pids whose
    /// set empties — the keep-bindings-clean half of the instant-exit ↔
    /// negative-vouch handshake (its ledger inverse is `forget`).
    fn unbind(&mut self, id: &str) {
        self.pid_bindings.retain(|_, ids| {
            ids.remove(id);
            !ids.is_empty()
        });
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
        // R0612-04: a truncated-below-cursor file must be parked at its new
        // EOF, not handed to the walk's truncation arm (cursor→0, no drain) —
        // see park_if_truncated_below_cursor for the full mechanism.
        park_if_truncated_below_cursor(path, ctx).await;
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

/// ONE probe refresh (the imperative SHELL over `ProbeLadder::fold`), shared by
/// the three sites that re-snapshot `live` (the initial seed, the 250ms rescan,
/// the 60s poll). On a HEALTHY snapshot (`Some`): replace the admission set,
/// then apply the ladder's fold — emit each confirmed exit and register each
/// newly-seen pid with the exit watch (#223 rung 2); returns true so the caller
/// re-emits `ProofOfLife` after its scan. On a probe FAILURE (`None`) or no
/// probe wired: change NOTHING — `ctx.live` keeps the previous ids (admission
/// stays additive), the miss windows neither advance nor confirm, no bindings
/// move, no `ProofOfLife` is emitted (the reducer's TTL absorbs the gap).
pub(super) async fn refresh_probe_snapshot(
    probe: Option<&LivenessProbe>,
    ladder: &mut ProbeLadder,
    exit_watch: Option<&ExitWatch>,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) -> bool {
    let Some(probe) = probe else {
        return false;
    };
    // The probe walks the OS process table + each agent process's open fds
    // (fd_probe: `read_dir("/proc")` + a per-pid `comm` read on Linux;
    // `proc_listallpids`/`proc_name` per pid on macOS) — blocking std::fs/libproc
    // that would occupy a tokio worker for the walk's duration if run inline,
    // stalling the render loop, input, and every source task sharing the runtime
    // on a low-core host (this fn runs on the watcher's select loop every seed /
    // 250ms rescan / 60s poll). Offload to the blocking pool — the probe is an
    // `Arc<dyn Fn + Send + Sync>`, cheap to clone. `spawn_blocking` (not
    // `block_in_place`) because this crate's tokio has no `rt-multi-thread` and
    // the watcher's own tests run on a current-thread runtime, where
    // `block_in_place` panics. Every outcome stays fail-safe (change nothing) —
    // both a probe failure and a probe panic keep the previous snapshot — but a
    // panic is surfaced at `warn` (an fd_probe BUG, not a transient enumeration
    // miss) so a recurring crash isn't buried at debug.
    let probe = Arc::clone(probe);
    let snap = match tokio::task::spawn_blocking(move || probe()).await {
        Ok(Some(snap)) => snap,
        Ok(None) => {
            debug!(
                "liveness probe failed; keeping the previous snapshot (failure changes nothing)"
            );
            return false;
        }
        Err(join_err) => {
            tracing::warn!(%join_err, "liveness probe task panicked; keeping the previous snapshot");
            return false;
        }
    };
    *ctx.live.lock().await = snap.pid_of.keys().cloned().collect();
    // Functional core → imperative shell: the ladder DECIDES (pure), we APPLY.
    // Emit the confirmed exits BEFORE registering the new pids — the pre-owner
    // order — so the channel sees the same `SessionEnd` sequence: `watch` sends
    // nothing synchronously, and each just-confirmed id is absent from
    // `snap.pid_of`, so the watch loop never touches it. A pid whose kernel
    // registration fails (EPERM) is not retried — the slower rungs cover.
    let outcome = ladder.fold(&snap, std::time::Instant::now());
    for id in &outcome.exits {
        emit_session_exit(id, decoders, ctx).await;
    }
    for pid in outcome.newly_watched {
        if let Some(watch) = exit_watch {
            watch.watch(pid);
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
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    // Deleted transcript: prune. This sweep already stats
                    // every gated candidate each scan pass, so a lost notify
                    // Remove event (the walk-side eviction's trigger) would
                    // otherwise leave the entry a permanent candidate — one
                    // failed stat per pass, forever, on a file that can never
                    // revive.
                    ctx.cursors.lock().await.remove(&path);
                }
                continue;
            }
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
