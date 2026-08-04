use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::debug;

use crate::source::exit_watch::ExitWatch;
use crate::source::{fd_probe, AgentEvent, TaggedSender, Transport};
use crate::AgentId;

use super::walk::{check_session_ended, park_if_truncated_below_cursor, walk_jsonl};
use super::{SourceDecoders, WatchCtx};

/// One healthy liveness-probe observation: which agent processes are verified
/// alive RIGHT NOW, and which OS pid owns each. The live-id set IS `pid_of`'s
/// key set; a separate `ids` set would only make "id without pid" representable.
#[derive(Debug, Clone, Default)]
pub struct ProbeSnapshot {
    /// id → owning OS pid, for the exit watch. Many ids may share one pid — one
    /// codex process holds every rollout it has open.
    pub pid_of: HashMap<String, i32>,
}

impl ProbeSnapshot {
    /// The ids this probe saw alive.
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.pid_of.keys()
    }
    /// Was `id` alive at probe time?
    pub fn contains(&self, id: &str) -> bool {
        self.pid_of.contains_key(id)
    }
    /// Did the probe see nothing alive? Distinct from a FAILED probe, which is `None`.
    pub fn is_empty(&self) -> bool {
        self.pid_of.is_empty()
    }

    /// Bind `id` to its owning `pid`. A contested id (two live processes holding
    /// one file open — a resume overlap) resolves to the LARGER pid: arbitrary
    /// but stable, never proc-enumeration order (#252).
    pub(crate) fn bind_pid(&mut self, id: String, pid: i32) {
        let bound = self.pid_of.entry(id).or_insert(pid);
        if pid > *bound {
            *bound = pid;
        }
    }

    /// Build a snapshot from the regular files that live `comm_names`-named
    /// processes hold open under `root` — the skeleton the open-FD liveness
    /// sources share.
    ///
    /// `None` ONLY when the proc-table enumeration itself fails — the watcher
    /// then changes nothing (#223). An ABSENT / un-canonicalizable `root` is NOT
    /// a failure (the source may simply never have run) → `Some(empty)`, a
    /// healthy "nothing alive" observation.
    pub(crate) fn from_open_fds(
        root: &Path,
        comm_names: &[&str],
        recognize: impl Fn(&Path) -> bool,
        id_derive: super::IdDeriver,
    ) -> Option<Self> {
        Self::from_open_fds_with(
            root,
            comm_names,
            recognize,
            id_derive,
            fd_probe::pids_by_name,
            fd_probe::open_vnode_paths,
        )
    }

    /// [`from_open_fds`](Self::from_open_fds) with the two proc-table calls
    /// INJECTED, so the shell's own decisions — canonicalize-or-`Some(empty)`,
    /// enumeration-failure-is-`None` (#223), and the pid→path fan-out — are
    /// reachable from a test without any FFI.
    pub(crate) fn from_open_fds_with(
        root: &Path,
        comm_names: &[&str],
        recognize: impl Fn(&Path) -> bool,
        id_derive: super::IdDeriver,
        pids_by_name: impl Fn(&str) -> Option<Vec<i32>>,
        open_vnode_paths: impl Fn(i32) -> Vec<PathBuf>,
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
            pids.extend(pids_by_name(name)?);
        }
        let pairs = pids.into_iter().flat_map(|pid| {
            open_vnode_paths(pid)
                .into_iter()
                .map(move |path| (pid, path))
        });
        Some(Self::from_open_fd_pairs(
            &canonical, pairs, recognize, id_derive,
        ))
    }

    /// The PURE join half of [`from_open_fds`], drivable with synthetic
    /// `(pid, path)` pairs.
    pub(crate) fn from_open_fd_pairs(
        root: &Path,
        pairs: impl Iterator<Item = (i32, PathBuf)>,
        recognize: impl Fn(&Path) -> bool,
        id_derive: super::IdDeriver,
    ) -> Self {
        let mut snap = Self::default();
        for (pid, path) in pairs {
            if !path.starts_with(root) {
                continue;
            }
            if recognize(&path) {
                // The fold lives at the seam (`walk::id_path`), NOT inside each
                // source's deriver — so the probe id-space matches the walk
                // seam's and a new fd-probe source can't forget it.
                let id = (id_derive)(&super::walk::id_path(&path));
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

/// Optional first-party liveness probe: the session ids — in the source's
/// `IdDeriver` id-space — of agent processes known to be ALIVE right now.
/// ADDITIVE-ONLY for admission: membership bypasses the first-sight
/// recency/ended gate.
///
/// Failure is EXPLICIT: `None` means the probe itself FAILED and callers must
/// change NOTHING; `Some` with an empty snapshot means it ran fine and nothing
/// is alive. Only that distinction lets a previously-vouched id MISSING from two
/// healthy snapshots count as a high-confidence exit (the negative vouch, #223).
/// `Arc<dyn Fn>` rather than a fn pointer because the real probe captures its
/// registry dir.
pub type LivenessProbe = Arc<dyn Fn() -> Option<ProbeSnapshot> + Send + Sync>;

/// Negative vouch (#223): a previously-vouched id must be MISSING from two
/// healthy probe snapshots at least this far apart before its exit is
/// confirmed. 60s makes the signal immune to Codex's brief drop-and-reopen fd
/// gap on a write failure and to the initial-seed / 250ms-rescan adjacency.
pub(super) const NEGATIVE_VOUCH_MIN_SPAN: Duration = Duration::from_secs(60);

/// Whether the liveness probe vouches for this transcript. A vouched-for file is
/// a RUNNING agent however old its mtime, so the first-sight gate must not hide
/// it. Subagent transcripts can never match — their stems are agent ids, not
/// session UUIDs.
pub(super) async fn probe_admits(
    path: &Path,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) -> bool {
    let live = ctx.live.lock().await;
    // Through `walk::id_path`, never raw: the producer side folds every path
    // before deriving, so an unfolded id here would query a different id-space.
    !live.is_empty() && live.contains(&(decoders.id_derive)(&super::walk::id_path(path)))
}

/// The probe is ONGOING liveness, not just admission: emit a `ProofOfLife` per
/// vouched id after each refresh so the reducer can hold its sweep exemption
/// while the process lives; when the live signal disappears the emissions stop
/// and the exemption ages out.
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

/// #223: the probe ladder's DERIVED liveness state. A session id the probe
/// previously vouched for that DISAPPEARS from a healthy snapshot is a
/// high-confidence exit — the registry entry was removed / the rollout fd
/// closed, signals only the OWNING process can produce — so the watcher can emit
/// the `SessionEnd` the CLI never writes instead of waiting out the 10–30 min
/// stale-sweep. Confirmation needs the id missing from two healthy observations
/// at least `min_span` apart; a probe FAILURE is never an observation.
///
/// A pure failure detector: [`fold`](ProbeLadder::fold) RETURNS the effects to
/// apply rather than emitting them, so it is unit-testable with synthetic time
/// and zero mocks.
pub(super) struct ProbeLadder {
    min_span: Duration,
    /// Ids vouched by an earlier healthy snapshot. An id stays "previously
    /// vouched" while its miss window runs, so the second observation can
    /// confirm it.
    prev_vouched: HashSet<String>,
    /// id → when a healthy snapshot FIRST came back without it. `Instant`
    /// (monotonic): a wall-clock jump must not fake a 60s span.
    miss_since: HashMap<String, std::time::Instant>,
    /// pid → the session ids a healthy snapshot bound to it. An id is bound
    /// under at most ONE pid (`fold`'s migration maintains this).
    pid_bindings: HashMap<i32, HashSet<String>>,
}

/// What one [`ProbeLadder::fold`] decided the imperative shell must DO.
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

    /// Fold one HEALTHY snapshot: advance the negative-vouch miss windows and
    /// the pid bindings, returning the effects for the shell to emit + watch.
    /// Never called on a probe FAILURE — the shell forwards only `Some`
    /// snapshots.
    pub(super) fn fold(&mut self, snap: &ProbeSnapshot, now: std::time::Instant) -> ProbeOutcome {
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
                Some(first_miss) if now.duration_since(*first_miss) >= self.min_span => {
                    debug!(
                        "negative vouch confirmed for {id}: probe stopped vouching; \
                         emitting SessionEnd"
                    );
                    // Drop the pid binding too: a codex-style process owns many
                    // rollouts, so it may outlive this session — its eventual OS
                    // exit must not re-emit a SessionEnd for an already-confirmed
                    // id.
                    self.forget(&id);
                    self.unbind(&id);
                    exits.push(id);
                }
                Some(_) => {}
                None => {
                    self.miss_since.insert(id, now);
                }
            }
        }
        // Ids whose miss window still runs must STAY vouched, or no later
        // snapshot can confirm them.
        self.prev_vouched = snap.pid_of.keys().cloned().collect();
        self.prev_vouched.extend(self.miss_since.keys().cloned());

        // Bindings are ADDITIVE per snapshot (ids leave via `pid_died` or the
        // confirm-unbind above, never by snapshot omission — the vouch ladder
        // owns "gone" semantics) — EXCEPT an observed rebind: an id seen under a
        // NEW pid (a codex `resume` of the same rollout in another process while
        // the old one lives) migrates between sets, because the negative vouch
        // can't clean that stale binding and the old pid's later death would
        // otherwise instant-exit a live session.
        let mut newly_watched = Vec::new();
        for (id, pid) in &snap.pid_of {
            // find-then-compare (not `any(p != pid && contains)`): an id is
            // bound under at most ONE pid, so the first holder is the only
            // holder and the single `!=` leaves no conjunction whose halves a
            // mutation could silently swap.
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
    /// whole binding and `forget` every id it held, so the slower negative-vouch
    /// rung can't re-confirm an exit this rung is about to emit.
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
    /// re-confirming an exit a faster rung already emitted. Always paired with
    /// `unbind`, its bindings inverse.
    fn forget(&mut self, id: &str) {
        self.prev_vouched.remove(id);
        self.miss_since.remove(id);
    }

    /// Remove one session id from every pid's binding set, dropping pids whose
    /// set empties. The bindings inverse of `forget`.
    fn unbind(&mut self, id: &str) {
        self.pid_bindings.retain(|_, ids| {
            ids.remove(id);
            !ids.is_empty()
        });
    }
}

/// ONE exit path for every watcher-synthesized session end, shared by the
/// negative-vouch confirmation and the instant-exit arm so the two can't fork.
/// The un-claim is what lets a LATER append re-register through
/// `emit_first_sight`.
///
/// The drain-FIRST ordering is load-bearing: the instant exit can beat a
/// pre-death write's notify event by orders of magnitude, and un-claiming with
/// bytes still pending would let that stale chunk re-enter `walk_jsonl` as a
/// first-sight and resurrect the dead session as a ghost, with every fast rung
/// already disarmed for it.
pub(super) async fn emit_session_exit(id: &str, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let claimed: Vec<PathBuf> = {
        let seen = ctx.seen.lock().await;
        seen.keys()
            // Folded via `walk::id_path` like every other derivation seam: a
            // raw key derives into a different id-space, the filter matches
            // nothing, and the `seen` entry never releases — so a later append
            // can never re-register the session.
            .filter(|p| (decoders.id_derive)(&super::walk::id_path(p)) == id)
            .cloned()
            .collect()
    };
    for path in &claimed {
        // A truncated-below-cursor file must be parked at its new EOF, not
        // handed to the walk's truncation arm (cursor→0, no drain).
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
    // HEALTHY probe refresh, so after an instant exit a probe-FAILURE pass would
    // keep vouching the id this watcher just declared dead, and the re-vouch
    // sweep would re-admit its parked file (cursor reset to 0 → full replay → a
    // phantom SessionStart).
    ctx.live.lock().await.remove(id);
}

/// ONE probe refresh (the imperative SHELL over `ProbeLadder::fold`), shared by
/// the three sites that re-snapshot `live`. Returns true so the caller re-emits
/// `ProofOfLife` after its scan. On a probe FAILURE (`None`) or no probe wired:
/// change NOTHING — `ctx.live` keeps the previous ids, the miss windows neither
/// advance nor confirm, no bindings move (the reducer's TTL absorbs the gap).
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
    // The probe is blocking std::fs/libproc that would occupy a tokio worker for
    // the walk's duration if run inline, stalling the render loop and every
    // source task sharing the runtime. `spawn_blocking` (not `block_in_place`)
    // because this crate's tokio has no `rt-multi-thread` and the watcher's own
    // tests run on a current-thread runtime, where `block_in_place` panics.
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
    // A pid whose kernel registration fails (EPERM) is not retried — the slower
    // rungs cover.
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
/// a TRANSIENT probe miss (registry file mid-rewrite, a read race) would gate a
/// live session PERMANENTLY — every later pass exits at `cursor == file_len` and
/// never asks again. On each SCAN pass, re-ask about every file that is
/// known-but-never-registered and reset a vouched one's cursor to 0.
///
/// Cannot loop: a re-vouched file that registers claims `seen` and drops out of
/// the candidate set; an ENDED one is refused here, and a terminator decoded
/// mid-replay RELEASES the claim rather than removing it, so neither re-enters.
pub(super) async fn revouch_gated_files(decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    // Empty snapshot = no probe wired, or nothing live: skip the sweep so a
    // probe-less source pays one lock check per pass, not a metadata read per
    // gated file.
    if ctx.live.lock().await.is_empty() {
        return;
    }
    let candidates: Vec<(PathBuf, u64)> = {
        let cursors = ctx.cursors.lock().await;
        cursors.iter().map(|(p, c)| (p.clone(), *c)).collect()
    };
    for (path, cursor) in candidates {
        // ANY entry skips — including a RELEASED claim: the probe legitimately
        // vouches a multi-turn child's still-open rollout, but replaying it
        // would re-register the just-ended child with a burst of stale activity.
        // A released path revives only on NEW bytes.
        if ctx.seen.lock().await.contains_key(&path) {
            continue;
        }
        // Only a file parked exactly at EOF is stuck — one with a pending
        // append revives through the normal walk on this same pass.
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    // Deleted transcript: prune, or a lost notify Remove event
                    // leaves the entry a permanent candidate — one failed stat
                    // per pass, forever, on a file that can never revive.
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
        // Last, so the bounded tail read costs only the handful of vouched
        // candidates. Resetting an ENDED transcript's cursor would undo the
        // first-sight gate's terminator decision and replay the whole dead
        // session once per scan pass, for as long as the vouch lasts.
        if check_session_ended(&path, decoders.check_ended).await {
            continue;
        }
        ctx.cursors.lock().await.insert(path, 0);
    }
}
