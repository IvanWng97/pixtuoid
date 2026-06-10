use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Result;
use notify::{Config, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::source::exit_watch::ExitWatch;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

pub type LineDecoder = fn(&str, &str, serde_json::Value) -> Result<Vec<AgentEvent>>;
pub type LabelDeriver = fn(&Path, &str, &Path) -> String;
pub type SessionEndChecker = fn(&[u8]) -> bool;

/// Derives the opaque session-id string used to build the generic
/// `SessionStart`'s `AgentId`. The default (`default_id_from_path`) returns
/// the normalized transcript file path — used by **Antigravity** (its hook
/// keys on the path via `IdKey::TranscriptPathThenSessionId`). **CC**
/// overrides to `cc_id_from_path` (the transcript filename stem = the session
/// UUID), and **Codex** overrides to `codex_id_from_path` (the rollout UUID),
/// so that both sources coalesce hook↔JSONL on the session UUID rather than
/// the full path.
pub type IdDeriver = fn(&Path) -> String;

fn default_id_from_path(p: &Path) -> String {
    crate::source::decoder::normalize_path_key(&p.to_string_lossy())
}

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

/// The per-source decode/label/end/id fn-pointers (the invariant-#3 seam)
/// bundled so the seed/scan/walk helpers thread ONE Copy value, not four.
#[derive(Clone, Copy)]
struct SourceDecoders {
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_ended: SessionEndChecker,
    id_derive: IdDeriver,
}

/// Shared per-run watch state, borrowed by the scan/walk helpers.
#[derive(Clone, Copy)]
struct WatchCtx<'a> {
    source: &'a Arc<str>,
    cursors: &'a Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &'a Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &'a TaggedSender,
    /// Recency window for the first-sight gate (a file older than this is
    /// seeded at EOF without a SessionStart). The whole watch shares one window
    /// so every path that can first-see a file gates identically (see #85).
    window: Duration,
    /// Most recent liveness-probe snapshot (session ids in `IdDeriver` space).
    /// Refreshed once per scan pass (initial seed / 250ms rescan / 60s poll);
    /// notify-driven single-file walks reuse it — seconds of staleness is fine
    /// because the probe is ADDITIVE-ONLY (it can only admit, never gate).
    live: &'a Arc<Mutex<HashSet<String>>>,
}

pub struct JsonlWatcher {
    root: PathBuf,
    initial_window: Duration,
    source_name: String,
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_session_ended: SessionEndChecker,
    id_derive: IdDeriver,
    liveness_probe: Option<LivenessProbe>,
    poll_interval: Duration,
    negative_vouch_min_span: Duration,
}

const DEFAULT_INITIAL_WINDOW: Duration = Duration::from_secs(3600);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Negative vouch (#223): a previously-vouched id must be MISSING from two
/// healthy probe snapshots at least this far apart before its exit is
/// confirmed. Two observations ≥60s apart make the signal immune to Codex's
/// brief drop-and-reopen fd gap on a write failure and to the initial-seed /
/// 250ms-rescan adjacency (back-to-back snapshots seconds apart can never
/// confirm on their own).
const NEGATIVE_VOUCH_MIN_SPAN: Duration = Duration::from_secs(60);

/// Test-only seam: forces every `JsonlWatcher` in this process onto a polling
/// backend (`notify::PollWatcher`) at `interval`, instead of the native
/// FSEvents/inotify watcher. Set once — later calls are ignored. Integration
/// tests use this so they don't spin up + tear down a real FSEvents stream per
/// test; on macOS that setup/teardown is tens of seconds per `TempDir` and was
/// the bulk of the watcher tests' runtime (the gate logic itself is already
/// covered by deterministic, watcher-free unit tests below). Never called in
/// production, so the default (native watcher + 60s poll backstop) is unchanged.
#[doc(hidden)]
pub fn force_polling_backend_for_tests(interval: Duration) {
    let _ = TEST_POLL_OVERRIDE.set(interval);
}

static TEST_POLL_OVERRIDE: OnceLock<Duration> = OnceLock::new();

impl JsonlWatcher {
    pub fn new(
        root: PathBuf,
        source: String,
        decode_line: LineDecoder,
        derive_label: LabelDeriver,
        check_session_ended: SessionEndChecker,
    ) -> Self {
        Self {
            root,
            initial_window: DEFAULT_INITIAL_WINDOW,
            source_name: source,
            decode_line,
            derive_label,
            check_session_ended,
            id_derive: default_id_from_path,
            liveness_probe: None,
            poll_interval: DEFAULT_POLL_INTERVAL,
            negative_vouch_min_span: NEGATIVE_VOUCH_MIN_SPAN,
        }
    }

    pub fn with_initial_window(mut self, window: Duration) -> Self {
        self.initial_window = window;
        self
    }

    /// Test-only seam (mirrors the `with_initial_window` builder shape):
    /// shrinks the 60s `scan_root` poll backstop so the poll arm's probe
    /// refresh + `ProofOfLife` re-emission are testable — at the production
    /// cadence a test would have to wait a minute per tick. Production never
    /// calls this; the default stays [`DEFAULT_POLL_INTERVAL`].
    #[doc(hidden)]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Test-only seam (mirrors `with_poll_interval`): shrinks the
    /// [`NEGATIVE_VOUCH_MIN_SPAN`] confirmation window so the negative-vouch
    /// exit path is testable — at the production 60s span a test would wait
    /// over a minute per confirmation. Production never calls this.
    #[doc(hidden)]
    pub fn with_negative_vouch_min_span(mut self, span: Duration) -> Self {
        self.negative_vouch_min_span = span;
        self
    }

    pub fn with_id_deriver(mut self, id_derive: IdDeriver) -> Self {
        self.id_derive = id_derive;
        self
    }

    pub fn with_liveness_probe(mut self, probe: LivenessProbe) -> Self {
        self.liveness_probe = Some(probe);
        self
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        let cursors: Arc<Mutex<HashMap<PathBuf, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let seen_sessions: Arc<Mutex<HashMap<PathBuf, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Refreshed once per scan pass (the initial seed below, then the
        // rescan/poll arms) via `refresh_probe_snapshot`; notify walks read
        // the latest snapshot. Starts empty — the seed refresh fills it before
        // the first scan.
        let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let mut vouch = NegativeVouch::new(self.negative_vouch_min_span);

        // Instant exit (#223 rung 2): a probed watcher spawns ONE detached
        // ExitWatch thread (kqueue NOTE_EXIT / pidfd+poll) so a bound OS
        // process dying becomes a SessionEnd in milliseconds — ahead of the
        // negative vouch (~60–120s) and the TTL/stale sweeps. Purely
        // additive: spawn() is None on unsupported platforms or backend-init
        // failure, and a dead thread just stops sending — the slower rungs
        // still cover.
        let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
        let exit_watch = if self.liveness_probe.is_some() {
            ExitWatch::spawn(exit_tx.clone())
        } else {
            None
        };
        // TRAP: the only long-lived sender is owned by the ExitWatch thread.
        // With no probe wired or a failed spawn, every sender would drop
        // right here and `exit_rx.recv()` would resolve `Ready(None)` on
        // every select! pass (a pattern-miss disables the branch per call —
        // not a spin, but a wasted poll on every loop iteration, forever).
        // Park one clone so the arm stays forever-pending in exactly those
        // cases. (A LATER thread death — pidfd ENOSYS, kevent error — does
        // reintroduce the wasted poll; that residual is accepted.)
        let _exit_keepalive = exit_watch.is_none().then(|| exit_tx.clone());
        drop(exit_tx);
        // pid → the session ids a healthy probe snapshot bound to it
        // (`ProbeSnapshot::pid_of` folded by `refresh_probe_snapshot`) — the
        // join the instant-exit arm uses to translate an OS exit into
        // SessionEnds. Entries leave on exit (whole pid) or negative-vouch
        // confirm (single id, see `unbind_session`).
        let mut pid_bindings: HashMap<i32, HashSet<String>> = HashMap::new();

        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let event_handler = move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                for path in event.paths {
                    if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        let _ = notify_tx.send(path);
                    }
                }
            }
        };
        let _ = tokio::fs::create_dir_all(&self.root).await;
        // Native (FSEvents/inotify/…) in production; a fast `PollWatcher` in tests
        // (see `force_polling_backend_for_tests`). Both impl `notify::Watcher` and
        // feed the SAME `notify_tx`, so the select! loop below is backend-agnostic.
        let mut watcher: Box<dyn Watcher + Send> = match TEST_POLL_OVERRIDE.get().copied() {
            // `with_compare_contents` makes the poll detect changes by hashing
            // file contents, not just mtime/size — appends and truncate-rewrites
            // (the partial-line / cursor-reset tests) are caught reliably.
            Some(interval) => Box::new(PollWatcher::new(
                event_handler,
                Config::default()
                    .with_poll_interval(interval)
                    .with_compare_contents(true),
            )?),
            None => Box::new(RecommendedWatcher::new(event_handler, Config::default())?),
        };
        watcher.watch(&self.root, RecursiveMode::Recursive)?;

        let source_arc: Arc<str> = Arc::from(self.source_name.as_str());
        let decoders = SourceDecoders {
            decode_line: self.decode_line,
            derive_label: self.derive_label,
            check_ended: self.check_session_ended,
            id_derive: self.id_derive,
        };

        // Initial seed: the same `scan_root` → `walk_jsonl` path every later scan
        // uses, so a file is gated identically (recency + session_end) no matter
        // which pass first sees it. (Previously a separate `initial_seed_walk`
        // owned the gate and `walk_jsonl` had none — the divergence behind #85.)
        {
            let ctx = WatchCtx {
                source: &source_arc,
                cursors: &cursors,
                seen: &seen_sessions,
                tx: &tx,
                window: self.initial_window,
                live: &live,
            };
            let healthy = refresh_probe_snapshot(
                self.liveness_probe.as_ref(),
                &mut vouch,
                &mut pid_bindings,
                exit_watch.as_ref(),
                decoders,
                &ctx,
            )
            .await;
            scan_root(&self.root, decoders, &ctx).await;
            if healthy {
                emit_proof_of_life(&live, &source_arc, &tx).await;
            }
        }

        // Re-scan shortly after startup to catch files that APFS read_dir
        // missed during the initial seed walk (metadata propagation race).
        // walk_jsonl is idempotent (cursor == file_len → no-op).
        let mut rescan_done = false;
        let rescan_delay = tokio::time::sleep(Duration::from_millis(250));
        tokio::pin!(rescan_delay);

        // The 60s poll backstop is an INTERVAL hoisted outside the loop — a
        // sleep re-created per iteration resets its deadline on every notify
        // event, so sustained notify traffic starves scan_root (and the probe
        // refresh + re-vouch sweep riding it) indefinitely. An interval keeps
        // ticking under load; Delay (not the Burst default) so a long stall
        // doesn't fire catch-up scans back-to-back.
        let mut poll = tokio::time::interval(self.poll_interval);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // An interval's first tick completes immediately; the initial seed
        // above already scanned, so consume it.
        poll.tick().await;

        loop {
            let source_arc = source_arc.clone();
            let ctx = WatchCtx {
                source: &source_arc,
                cursors: &cursors,
                seen: &seen_sessions,
                tx: &tx,
                window: self.initial_window,
                live: &live,
            };
            tokio::select! {
                Some(path) = notify_rx.recv() => {
                    walk_jsonl(&path, decoders, &ctx).await;
                }
                _ = &mut rescan_delay, if !rescan_done => {
                    rescan_done = true;
                    let healthy = refresh_probe_snapshot(
                        self.liveness_probe.as_ref(), &mut vouch, &mut pid_bindings,
                        exit_watch.as_ref(), decoders, &ctx,
                    ).await;
                    scan_root(&self.root, decoders, &ctx).await;
                    if healthy {
                        emit_proof_of_life(&live, &source_arc, &tx).await;
                    }
                }
                _ = poll.tick() => {
                    let healthy = refresh_probe_snapshot(
                        self.liveness_probe.as_ref(), &mut vouch, &mut pid_bindings,
                        exit_watch.as_ref(), decoders, &ctx,
                    ).await;
                    scan_root(&self.root, decoders, &ctx).await;
                    if healthy {
                        emit_proof_of_life(&live, &source_arc, &tx).await;
                    }
                }
                Some(pid) = exit_rx.recv() => {
                    // Instant exit (#223 rung 2): the watched OS process
                    // died. Translate through the pid→ids binding; an
                    // unknown pid (already unbound by a negative-vouch
                    // confirm, or a duplicate event) is a no-op.
                    if let Some(ids) = pid_bindings.remove(&pid) {
                        for id in ids {
                            debug!("instant exit: pid {pid} died; emitting SessionEnd for {id}");
                            emit_session_exit(&id, decoders, &ctx).await;
                            // The next healthy snapshot will see the id gone
                            // anyway; forget it NOW so the negative vouch
                            // can't re-confirm the exit we just emitted.
                            vouch.forget(&id);
                        }
                    }
                }
            }
        }
    }
}

/// First-sight decision, shared by EVERY path that can be the first to see a
/// file (the initial seed, the 250ms rescan, the 60s poll, a notify event):
/// seed the cursor at EOF — suppressing SessionStart — when the session is
/// historical (mtime outside `window`) OR already ended (a session_end marker in
/// its tail). Only a recent, not-yet-ended file is read from the top. Unifying
/// the gate here (rather than only in the old `initial_seed_walk`) is the #85
/// fix: the post-startup rescan used to bypass it and resurrect a missed
/// ended/stale session as a phantom live sprite.
async fn should_seed_at_eof(
    meta: &std::fs::Metadata,
    window: Duration,
    path: &Path,
    check_ended: SessionEndChecker,
) -> bool {
    let recent = meta
        .modified()
        .ok()
        .map(|mtime| {
            // elapsed() Errs when mtime is in the future (APFS nanosecond clock
            // jitter); a future mtime is necessarily within any recency window.
            mtime.elapsed().unwrap_or(Duration::ZERO) <= window
        })
        .unwrap_or(false);
    // Historical → seed EOF. Recent-but-ended → seed EOF. Recent & live → read.
    !recent || check_session_ended(path, check_ended).await
}

/// Whether the liveness probe vouches for this transcript: its derived session
/// id appears in the most recent live-session snapshot. A vouched-for file is
/// a RUNNING agent however old its mtime (long-idle, delegating to subagents,
/// or stuck in a long tool call), so the first-sight gate must not hide it.
/// Subagent transcripts can never match — their stems are agent ids
/// (`agent-<id>`), not session UUIDs, so only the root transcript is admitted.
/// The empty-set check short-circuits the id derivation (an allocation) in the
/// no-probe case.
async fn probe_admits(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) -> bool {
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
async fn emit_proof_of_life(
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
struct NegativeVouch {
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
    fn new(min_span: Duration) -> Self {
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
    fn forget(&mut self, id: &str) {
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
async fn emit_session_exit(id: &str, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
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
        .send((Transport::Jsonl, AgentEvent::SessionEnd { agent_id }))
        .await;
    let mut seen = ctx.seen.lock().await;
    for path in &claimed {
        seen.remove(path);
    }
}

/// Remove one session id from every pid's binding set, dropping pids whose
/// set empties — the keep-state-clean half of the instant-exit ↔ negative-
/// vouch handshake (its inverse is `NegativeVouch::forget`).
fn unbind_session(pid_bindings: &mut HashMap<i32, HashSet<String>>, id: &str) {
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
async fn refresh_probe_snapshot(
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
    // vouch ladder owns "gone" semantics). A pid is registered with the exit
    // watch only on its FIRST appearance; if that registration failed
    // kernel-side (EPERM), it is not retried — the slower rungs cover.
    for (id, pid) in &snap.pid_of {
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

async fn scan_root(root: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    revouch_gated_files(decoders, ctx).await;
    if let Ok(mut read) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = read.next_entry().await {
            walk_jsonl(&entry.path(), decoders, ctx).await;
        }
    }
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
async fn revouch_gated_files(decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
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

async fn walk_jsonl(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let WatchCtx {
        source,
        cursors,
        seen,
        tx,
        window,
        // `live` is consumed inside `probe_admits` (off `ctx` directly).
        live: _,
    } = *ctx;
    // `derive_label` / `id_derive` are consumed inside `emit_first_sight` (off
    // `decoders` directly); only the per-line decoder and the end-checker are
    // used directly here.
    let SourceDecoders {
        decode_line,
        check_ended,
        ..
    } = decoders;
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if meta.is_dir() {
        if let Ok(mut read) = tokio::fs::read_dir(path).await {
            while let Ok(Some(entry)) = read.next_entry().await {
                Box::pin(walk_jsonl(&entry.path(), decoders, ctx)).await;
            }
        }
        return;
    }
    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
        return;
    }

    let file_len = meta.len();
    const MAX_PENDING_BYTES: u64 = 1 << 20;

    // `known` = already tracked (an earlier pass seeded or read it); `cursor_now`
    // = where to resume (0 if untracked). One lock read for both.
    let (known, cursor_now): (bool, u64) = {
        let cursors_g = cursors.lock().await;
        let entry = cursors_g.get(path).copied();
        (entry.is_some(), entry.unwrap_or(0))
    };
    // First-sight gate (#85): a file we've never tracked is being seen for the
    // first time — by the initial seed, the 250ms rescan, a notify event, or the
    // 60s poll. Run ONE recency + session_end gate regardless of which pass got
    // here first, so a historical or already-ended session is seeded at EOF
    // instead of resurrected with a phantom SessionStart. (A later write makes it
    // `known` with cursor < len, so the documented revive-on-append still fires.)
    // The liveness probe pre-empts the gate: mtime is only a liveness PROXY,
    // and a long-idle / delegating / stuck-in-a-long-tool-call session writes
    // nothing for hours — when the probe has ground truth that the owning
    // process is alive, the file is read from the top (a > MAX_PENDING_BYTES
    // body falls into the oversized first-sight registration below). The
    // bypass deliberately skips the gate's ended tail-scan too: CC (the only
    // probe user) persists no structural end marker today, so there is
    // nothing to scan for — if the upstream drift watch fires (CC starts
    // writing one), admission needs an ended-check before bypassing.
    if !known
        && !probe_admits(path, decoders, ctx).await
        && should_seed_at_eof(&meta, window, path, check_ended).await
    {
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        return;
    }
    if cursor_now > file_len {
        warn!(
            "{} truncated below cursor ({} < {}), resetting cursor",
            path.display(),
            file_len,
            cursor_now
        );
        cursors.lock().await.insert(path.to_path_buf(), 0);
        return;
    }
    if cursor_now == file_len {
        return;
    }
    if file_len - cursor_now > MAX_PENDING_BYTES {
        warn!(
            "{} has > {} pending bytes; skipping backlog to end",
            path.display(),
            MAX_PENDING_BYTES
        );
        // A skipped span may bury a structural session-end marker (the
        // source's check_ended — CC's matches `subtype:"session_end"` /
        // `SessionEnd`; content never counts). Without a tail-scan here the
        // terminator is lost and the slot reaps only via the slow stale-sweep.
        // Checked UNCONDITIONALLY (one bounded 8 KB tail read on a branch
        // already doing head I/O): a KNOWN file's span can end mid-skip, and a
        // !known file lands here too — the liveness probe bypasses the
        // first-sight gate (should_seed_at_eof) INCLUDING its ended tail-scan,
        // so a probe-admitted ENDED transcript must be caught here or the
        // #204 registration below would mint a ghost for a session that is
        // over. (Codex/Antigravity check_ended no-op.) Scan reads the file
        // tail and is independent of the cursor, so compute it before seeding.
        let ended_in_skip = check_session_ended(path, check_ended).await;
        // Seed the cursor to EOF FIRST — before the awaited head-read +
        // registration below — so a concurrent walk_jsonl on this path (250ms
        // rescan / notify) sees `known` on its next read and won't re-enter this
        // branch. Mirrors the normal tail-read path, which also advances the
        // cursor before emitting. (`emit_first_sight` is idempotent via `seen`, so
        // the window only ever cost a redundant head read, never a duplicate
        // SessionStart — but matching the ordering closes it.)
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        if ended_in_skip {
            let id = AgentId::from_parts(source, &(decoders.id_derive)(path));
            let _ = tx
                .send((Transport::Jsonl, AgentEvent::SessionEnd { agent_id: id }))
                .await;
            // Un-claim first-sight AFTER forwarding the terminator: the
            // session is over, so a LATER append must re-register through
            // emit_first_sight (the documented revive). Leaving the claim in
            // place pinned the path "registered" forever — a resumed session
            // could never re-appear without a watcher restart.
            seen.lock().await.remove(path);
        }
        // #204: on the first oversized sight of a recent, live session, still
        // REGISTER the agent. Otherwise a >1 MB transcript stays invisible
        // until its next small append (a long session, or a delegating parent
        // whose subagents then render as flat roots). The giant backlog is NOT
        // replayed; cwd/label come from a BOUNDED head read (CC writes `cwd`
        // on the first line), never the whole 7.4 MB file. Registration keys
        // on `seen` (= "registered"), NOT `!known`: a first-sight-GATED file
        // (cursor seeded at EOF, no SessionStart) is `known`, yet its first
        // >1 MiB append lands here — keying on `!known` left that agent
        // invisible until a later ≤1 MiB append. The `seen` check also spares
        // already-registered files a redundant head read on every oversized
        // append. A span that itself ENDED stays unregistered — a SessionStart
        // after the SessionEnd just sent would resurrect a ghost.
        let registered = seen.lock().await.contains_key(path);
        if !registered && !ended_in_skip {
            let head_cwd = read_head_cwd(path, MAX_PENDING_BYTES).await;
            emit_first_sight(path, source, decoders, seen, tx, head_cwd).await;
        }
        // #222: the skipped span may bury an IN-FLIGHT Agent/Task dispatch —
        // tail-scan the last TASK_SCAN_BYTES for unmatched Task starts and
        // re-emit exactly those, restoring subagent-leak suppression + b1
        // (see scan_pending_tasks for the full WHY). Only when the session is
        // NOT ended (a terminator was just forwarded — seeding a Task after
        // it would animate a ghost delegation) AND the file is registered
        // after the decision above (no slot → JSONL events for an unknown id
        // are reducer no-ops; skip the wasted 256 KiB decode). Runs AFTER
        // emit_first_sight so the registration precedes the synthesized
        // starts on the channel.
        if !ended_in_skip && seen.lock().await.contains_key(path) {
            scan_pending_tasks(path, file_len, decoders, ctx).await;
        }
        return;
    }

    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            warn!("open {} failed: {e}", path.display());
            return;
        }
    };
    if let Err(e) = file.seek(SeekFrom::Start(cursor_now)).await {
        warn!("seek {} failed: {e}", path.display());
        return;
    }
    let mut new_chunk = Vec::with_capacity((file_len - cursor_now) as usize);
    if let Err(e) = file.read_to_end(&mut new_chunk).await {
        warn!("read tail of {} failed: {e}", path.display());
        return;
    }

    let safe_end_relative = match new_chunk.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    };
    if safe_end_relative == 0 {
        return;
    }
    let new_cursor = cursor_now + safe_end_relative as u64;
    {
        let mut cursors_g = cursors.lock().await;
        cursors_g.insert(path.to_path_buf(), new_cursor);
    }

    let new_bytes = &new_chunk[..safe_end_relative];
    // Passed to per-line decoders as the `transcript_path` argument. CC's
    // `decode_cc_line` re-derives the session UUID via `cc_id_from_path` on
    // this string; Codex's decoder extracts the rollout UUID similarly.
    // Antigravity keys on the normalized path directly. Must be normalized
    // (same form as `id_derive` above) so that on Windows the hook key and
    // per-line key agree — an un-normalized path here would land every JSONL
    // event on a phantom id (caught by the PR #160 security review).
    let transcript_path_str = crate::source::decoder::normalize_path_key(&path.to_string_lossy());

    // The first-sight cwd normally comes from the read span, but a GATED file
    // revived by an append only reads the tail — and Codex rollouts carry cwd
    // ONLY on the head session_meta line, so the revive would register with an
    // empty cwd (downstream: unknown cwd → the short reap). Fall back to a
    // bounded head read, gated on the `seen` check so an already-registered
    // append pays at most that one contains read, never the head I/O.
    let mut first_sight_cwd = extract_cwd(new_bytes);
    if first_sight_cwd.is_none() && !seen.lock().await.contains_key(path) {
        first_sight_cwd = read_head_cwd(path, MAX_PENDING_BYTES).await;
    }
    emit_first_sight(path, source, decoders, seen, tx, first_sight_cwd).await;

    // Used below to recognize a decoded SessionEnd for THIS transcript (the
    // decoder keys events the same way `id_derive` does — pinned by the
    // hook↔watcher coalesce tests).
    let path_agent_id = AgentId::from_parts(source, &(decoders.id_derive)(path));
    let mut session_ended = false;
    for line in new_bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let s = match std::str::from_utf8(line) {
            Ok(s) => s,
            Err(_) => {
                warn!("non-utf8 line in {}", path.display());
                continue;
            }
        };
        let v: serde_json::Value = match serde_json::from_str(s) {
            Ok(v) => v,
            Err(e) => {
                debug!("skip non-json line in {}: {e}", path.display());
                continue;
            }
        };
        match decode_line(&transcript_path_str, source, v) {
            Ok(events) => {
                for ev in events {
                    let ends_this_agent = matches!(
                        &ev,
                        AgentEvent::SessionEnd { agent_id } if *agent_id == path_agent_id
                    );
                    if tx.send((Transport::Jsonl, ev)).await.is_err() {
                        return;
                    }
                    session_ended |= ends_this_agent;
                }
            }
            Err(e) => warn!("decode error in {}: {e}", path.display()),
        }
    }
    if session_ended {
        // Un-claim first-sight: a decoded SessionEnd retires this path's claim
        // so a LATER append re-registers through emit_first_sight (the
        // documented revive) — otherwise `seen` stays claimed forever and the
        // agent can never re-register without a watcher restart. Runs AFTER
        // the whole chunk is forwarded (the terminator precedes any re-claim),
        // and in-pass emit_first_sight idempotence is unaffected: this pass's
        // claim already happened above; the NEXT pass re-emits the pair.
        seen.lock().await.remove(path);
    }
}

/// Claim first-sight for `path` and, if this is the first pass to see it, emit
/// the synthesized `SessionStart` + `Rename` (the registration pair). Shared by
/// the normal tail-read path and the #204 oversized-first-sight path so the two
/// emit IDENTICAL events from one place. `cwd` is the source-derived working dir
/// (from the tail in the normal path, from a bounded head read in the oversized
/// path); `None`/empty falls back to the project-dir label in `derive_label`.
///
/// Takes the `seen` lock ONLY to claim first-sight, then drops it before the
/// awaited sends — holding it across `tx.send` would block on a slow consumer
/// for no reason (the flag flip is the entire critical section). Mirrors the
/// narrow `cursors` locking in `walk_jsonl`.
async fn emit_first_sight(
    path: &Path,
    source: &Arc<str>,
    decoders: SourceDecoders,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &TaggedSender,
    cwd: Option<PathBuf>,
) {
    let is_first = seen.lock().await.insert(path.to_path_buf(), true).is_none();
    if !is_first {
        return;
    }
    // session_id comes from the SAME deriver as the AgentId — the hook
    // transport's slots carry the bare session UUID (CC/Codex), and
    // `backfill_identity` never heals a non-empty session_id, so a raw
    // file-stem here would leave a JSONL-created slot permanently
    // disagreeing with its hook-created twin (a Codex stem is
    // `rollout-<ts>-<uuid>`: every tooltip disambiguator suffix became the
    // constant `roll`).
    let session_id = (decoders.id_derive)(path);
    let id = AgentId::from_parts(source, &session_id);
    let cwd = cwd.unwrap_or_default();
    let parent_id = detect_parent_id(path, source);
    let _ = tx
        .send((
            Transport::Jsonl,
            AgentEvent::SessionStart {
                agent_id: id,
                source: source.to_string(),
                session_id,
                cwd: cwd.clone(),
                parent_id,
            },
        ))
        .await;

    let label = (decoders.derive_label)(path, source, &cwd);
    let _ = tx
        .send((
            Transport::Jsonl,
            AgentEvent::Rename {
                agent_id: id,
                label,
            },
        ))
        .await;
}

/// Read at most `limit` bytes from the START of a file and extract `cwd` from
/// the first complete JSONL line (CC writes `cwd` there). Used by the #204
/// oversized-first-sight path so registration never reads the whole multi-MB
/// file — the head is bounded by `MAX_PENDING_BYTES`. Returns `None` when the
/// file can't be opened/read or has no `cwd` in its head (an empty-cwd
/// SessionStart then falls back to the project-dir label).
async fn read_head_cwd(path: &Path, limit: u64) -> Option<PathBuf> {
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut head = vec![0u8; limit as usize];
    let n = file.read(&mut head).await.ok()?;
    head.truncate(n);
    extract_cwd(&head)
}

/// Read at most `bytes` from the END of a file (clamped to file size).
/// `None` on any I/O error — callers treat that as "nothing to scan" (log +
/// continue, never panic). Shared by `check_session_ended` (8 KiB ended-marker
/// scan) and `scan_pending_tasks` (the #222 Task scan) so the two bounded
/// tail reads can't drift apart.
async fn read_tail(path: &Path, bytes: u64) -> Option<Vec<u8>> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let file_len = meta.len();
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let start = file_len.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).await.ok()?;
    let mut buf = Vec::with_capacity(bytes.min(file_len) as usize);
    file.read_to_end(&mut buf).await.ok()?;
    Some(buf)
}

/// Read the tail of a file and delegate to the source-specific checker.
async fn check_session_ended(path: &Path, checker: SessionEndChecker) -> bool {
    const TAIL_BYTES: u64 = 8192;
    match read_tail(path, TAIL_BYTES).await {
        Some(buf) => checker(&buf),
        None => false,
    }
}

/// How far back from EOF the oversized-skip Task scan looks (#222). Bounds
/// both the I/O and the decode work; survivors are at most the parallel-
/// dispatch ceiling in practice, so no further cap is needed.
const TASK_SCAN_BYTES: u64 = 256 * 1024;

/// #222: tail-scan an oversized skipped span for IN-FLIGHT Task dispatches
/// and re-emit exactly their `ActivityStart`s. Mid-attach to a delegating
/// session whose backlog exceeds `MAX_PENDING_BYTES` seeds the cursor at EOF,
/// so the in-flight `Agent` dispatch tool_use line is never decoded — and its
/// PreToolUse hook predates attach — leaving the reducer's `active_tasks`
/// empty: subagent-leak suppression stays OFF (the parent animates the
/// subagent's misattributed hook tools instead of showing Delegating) and the
/// b1 completion cascade never arms (the finished subagent lingers Idle up to
/// the 30-min stale sweep). Re-sending the unmatched Task starts restores
/// both: `track_active_tasks` seeds `active_tasks` from any transport's Task
/// ActivityStart, so the reducer needs no change.
///
/// Tail-window geometry guarantees no false leak: a completion is always
/// LATER in the file than its start, so any windowed start's completion (if
/// one exists) is also in the window — a synthesized start is only ever a
/// genuinely in-flight dispatch OR one whose completion raced in beyond the
/// `file_len` snapshot (the next walk re-decodes that span; `active_tasks` is
/// a HashSet, so the duplicate insert is idempotent). A dispatch buried
/// deeper than `TASK_SCAN_BYTES` of subsequent traffic keeps the pre-#222
/// skip behavior — bounded, documented residual.
///
/// `decode_line` also emits OTHER events from these lines (Rename, plain
/// ActivityStarts, SessionStart…) — everything except the unmatched Task
/// starts is DISCARDED. This is a Task-seeding scan, not a replay: replaying
/// 256 KiB of activity would animate a burst of stale tools.
///
/// Hook-wins dedup (#150): the synthesized events are Jsonl-tagged. On
/// mid-attach no hook record for these tuids exists (the hooks predate the
/// listener), so they pass the dedup. A mid-RUN oversized skip (> 1 MiB
/// appended between scans on an attached session) can race a recent hook
/// record — the dedup eating the synthesized start then is CORRECT: the hook
/// copy already seeded `active_tasks`.
///
/// Codex/Antigravity rollouts produce no Task ActivityStarts from line
/// decode (Codex subagents wire via the SubagentStart/Stop hooks), so the
/// scan is a structural no-op for them.
async fn scan_pending_tasks(
    path: &Path,
    file_len: u64,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) {
    let Some(buf) = read_tail(path, TASK_SCAN_BYTES).await else {
        return;
    };
    // Same per-line keying as the walk loop: the decoder re-derives the agent
    // id from this normalized path string (see `transcript_path_str` there).
    let transcript_path_str = crate::source::decoder::normalize_path_key(&path.to_string_lossy());
    let mut lines = buf.split(|b| *b == b'\n');
    if file_len > TASK_SCAN_BYTES {
        // The window starts mid-file, so its first chunk is almost always a
        // partial line — skip through the first newline rather than decode a
        // fragment (which could even parse as JSON by accident).
        let _ = lines.next();
    }
    // Unmatched Task dispatches in file order. A Vec keeps the order; a
    // duplicate ActivityStart for a seen tuid is skipped (HashSet semantics)
    // and a completion removes its start wherever it sits.
    let mut pending: Vec<(String, AgentEvent)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        let events = match (decoders.decode_line)(&transcript_path_str, ctx.source, v) {
            Ok(events) => events,
            Err(e) => {
                debug!("task-scan decode error in {}: {e}", path.display());
                continue;
            }
        };
        for ev in events {
            match &ev {
                AgentEvent::ActivityStart {
                    tool_use_id: Some(tuid),
                    detail: Some(d),
                    ..
                } if d.is_task() => {
                    if !pending.iter().any(|(t, _)| t == tuid) {
                        pending.push((tuid.clone(), ev));
                    }
                }
                AgentEvent::ActivityEnd {
                    tool_use_id: Some(tuid),
                    ..
                } => {
                    pending.retain(|(t, _)| t != tuid);
                }
                _ => {}
            }
        }
    }
    for (tuid, ev) in pending {
        debug!(
            "re-emitting in-flight Task dispatch {tuid} from the oversized tail of {}",
            path.display()
        );
        if ctx.tx.send((Transport::Jsonl, ev)).await.is_err() {
            return;
        }
    }
}

/// The directory a CC subagent transcript sits under: `<parent>/subagents/
/// agent-*.jsonl`. Matched as a whole path COMPONENT (never a substring) so a
/// project dir merely *containing* the word (e.g. `subagents-paper`) is not
/// mistaken for one, and so Windows backslash-separated paths match too (the
/// old `"/subagents/"` string scan was '/'-literal — found by the windows-test
/// CI job). Single source of truth for both `is_subagent_path` and
/// `detect_parent_id` so they cannot diverge (they did once: see the
/// `bug_004` fix in `cc_derive_label`).
const SUBAGENTS_DIR: &str = "subagents";

/// Whether a transcript path is a CC subagent transcript (vs a top-level
/// session). Codex subagents are FLAT (no such segment) — they're linked via the
/// `SubagentStart` hook instead, so this predicate is CC-layout-specific.
pub(crate) fn is_subagent_path(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == SUBAGENTS_DIR)
}

/// Detect a CC subagent by the `subagents` path component and link it to its
/// parent via the parent's session UUID — the directory component immediately
/// before `subagents` (`<parent-uuid>`). That UUID equals the parent's own id
/// (`cc_id_from_path` of the parent transcript), so the link resolves even when
/// the subagent transcript lands under a DIFFERENT project dir than the parent
/// (a git-worktree cwd-split): only the cwd-derived project-dir prefix differs;
/// the `<parent-uuid>` component is identical. CC-layout-specific — Codex links
/// subagents via the SubagentStart hook instead.
fn detect_parent_id(path: &Path, source: &str) -> Option<AgentId> {
    let mut prev: Option<&str> = None;
    for c in path.components() {
        if c.as_os_str() == SUBAGENTS_DIR {
            return prev.map(|uuid| AgentId::from_parts(source, uuid));
        }
        prev = c.as_os_str().to_str();
    }
    None
}

fn extract_cwd(bytes: &[u8]) -> Option<PathBuf> {
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
            return Some(PathBuf::from(cwd));
        }
        if let Some(cwd) = v
            .get("payload")
            .and_then(|p| p.get("cwd"))
            .and_then(|c| c.as_str())
        {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unbind_session_drops_only_emptied_pid_entries() {
        let mut bindings: HashMap<i32, HashSet<String>> = HashMap::new();
        bindings
            .entry(100)
            .or_default()
            .extend(["a".to_string(), "b".to_string()]);
        bindings.entry(200).or_default().insert("a".to_string());
        unbind_session(&mut bindings, "a");
        // pid 200 emptied → dropped; pid 100 keeps its other session.
        assert!(!bindings.contains_key(&200));
        assert_eq!(
            bindings.get(&100).map(|ids| ids.len()),
            Some(1),
            "the sibling id on a shared pid must survive the unbind"
        );
    }

    #[test]
    fn default_id_from_path_returns_normalized_path_key() {
        // Lowercase literal: identity on every platform (the Windows fold is
        // pinned by decoder.rs's normalize_path_key unit tests + the backslash
        // test below).
        let p = Path::new("/users/me/.claude/projects/x/abc.jsonl");
        assert_eq!(
            default_id_from_path(p),
            "/users/me/.claude/projects/x/abc.jsonl"
        );
    }

    // These call the REAL detect_parent_id/is_subagent_path (they're private —
    // an integration test can't reach them; an old decoder.rs test re-simulated
    // the algorithm inline and silently pinned the superseded string-scan).
    #[test]
    fn detect_parent_id_derives_grandparent_transcript_key() {
        // THE contract: the derived parent_id keys on the `<parent-uuid>`
        // component (the dir immediately before `subagents`), which equals the
        // parent's own id (`cc_id_from_path` of `<parent-uuid>.jsonl`). The
        // project-dir prefix is cwd-derived and intentionally NOT part of the
        // key, so the link survives a git-worktree cwd-split.
        let parent: PathBuf = ["projects", "x", "abc123"].iter().collect();
        let p = parent.join("subagents").join("agent-1.jsonl");
        let expected = AgentId::from_parts("claude-code", "abc123");
        assert_eq!(detect_parent_id(&p, "claude-code"), Some(expected));
        assert!(is_subagent_path(&p));
    }

    #[test]
    fn detect_parent_id_none_for_regular_and_lookalike_paths() {
        assert_eq!(
            detect_parent_id(
                Path::new("/Users/me/.claude/projects/x/ses.jsonl"),
                "claude-code"
            ),
            None
        );
        // Component matching: a dir merely CONTAINING the word never matches.
        let lookalike = Path::new("/Users/me/.claude/projects/subagents-paper/ses.jsonl");
        assert_eq!(detect_parent_id(lookalike, "claude-code"), None);
        assert!(!is_subagent_path(lookalike));
        // A bare relative path starting AT `subagents` has no parent to derive.
        assert_eq!(
            detect_parent_id(Path::new("subagents/agent-1.jsonl"), "claude-code"),
            None
        );
    }

    #[test]
    fn detect_parent_id_keys_on_parent_uuid_component() {
        let sub =
            Path::new("/Users/me/.claude/projects/-Users-me-proj/abc123/subagents/agent-1.jsonl");
        let expected = AgentId::from_parts("claude-code", "abc123");
        assert_eq!(detect_parent_id(sub, "claude-code"), Some(expected));
    }

    #[test]
    fn detect_parent_id_survives_cwd_split() {
        // THE bug: parent + subagent under DIFFERENT project dirs (a worktree
        // cwd-split). Only the project-dir component differs; the <parent-uuid>
        // component is identical, so BOTH must resolve to the same parent link.
        let under_a =
            Path::new("/Users/me/.claude/projects/-PROJECT-A/abc123/subagents/agent-1.jsonl");
        let under_b =
            Path::new("/Users/me/.claude/projects/-PROJECT-B/abc123/subagents/agent-1.jsonl");
        let expected = AgentId::from_parts("claude-code", "abc123");
        assert_eq!(detect_parent_id(under_a, "claude-code"), Some(expected));
        assert_eq!(detect_parent_id(under_b, "claude-code"), Some(expected));
        assert_eq!(
            detect_parent_id(under_a, "claude-code"),
            detect_parent_id(under_b, "claude-code"),
            "same <parent-uuid> under different project dirs resolves to the same parent"
        );
    }

    #[test]
    fn detect_parent_id_handles_workflow_nesting() {
        let sub = Path::new(
            "/Users/me/.claude/projects/p/abc123/subagents/workflows/wf_0d/agent-y.jsonl",
        );
        let expected = AgentId::from_parts("claude-code", "abc123");
        assert_eq!(detect_parent_id(sub, "claude-code"), Some(expected));
    }

    // Only RUNS on the windows-test CI job (backslashes are ordinary filename
    // bytes on Unix, so this shape is only meaningful there) — pins the
    // components rewrite's whole reason to exist.
    #[cfg(windows)]
    #[test]
    fn detect_parent_id_handles_backslash_paths() {
        // Backslash separators are split into ordinary components on Windows, so
        // the `<parent-uuid>` component (`abc123`) before `subagents` is
        // extracted just as it is on Unix — pins the component-walk reason to
        // exist. CC session UUIDs are lowercase, so no casefold is needed on the
        // key (mirrors `cc_id_from_path`).
        let p = Path::new(r"C:\Users\Me\.claude\projects\x\abc123\subagents\agent-1.jsonl");
        let expected = AgentId::from_parts("claude-code", "abc123");
        assert_eq!(detect_parent_id(p, "claude-code"), Some(expected));
        assert!(is_subagent_path(p));
    }

    #[test]
    fn extract_cwd_reads_top_level_and_nested_payload() {
        // CC/AG shape: top-level cwd.
        let top = br#"{"cwd":"/repo/a"}"#;
        assert_eq!(extract_cwd(top), Some(PathBuf::from("/repo/a")));
        // Codex shape: cwd nested under payload (session_meta).
        let nested = br#"{"type":"session_meta","payload":{"cwd":"/repo/b","id":"u"}}"#;
        assert_eq!(extract_cwd(nested), Some(PathBuf::from("/repo/b")));
    }

    fn t_decode(_t: &str, _s: &str, _v: serde_json::Value) -> Result<Vec<AgentEvent>> {
        Ok(vec![])
    }
    /// Minimal lifecycle decoder: a structural `session_end` line decodes to
    /// `SessionEnd` keyed exactly like the harness's default `id_derive`
    /// (`transcript_path` == `default_id_from_path(path)` here), mirroring how
    /// the real CC pair (`decode_cc_line` + `cc_id_from_path`) agrees.
    fn t_decode_lifecycle(t: &str, s: &str, v: serde_json::Value) -> Result<Vec<AgentEvent>> {
        if v.get("subtype").and_then(|x| x.as_str()) == Some("session_end") {
            return Ok(vec![AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(s, t),
            }]);
        }
        Ok(vec![])
    }
    fn t_label(_p: &Path, _s: &str, _c: &Path) -> String {
        "t".to_string()
    }
    fn t_ended(buf: &[u8]) -> bool {
        std::str::from_utf8(buf).is_ok_and(|s| s.contains("session_end"))
    }

    /// Drive `walk_jsonl` once over `path` against caller-owned cursor/seen
    /// maps, so multi-pass scenarios (gate → append → revive) share state the
    /// way the real watch loop does. Returns the emitted events.
    async fn walk_once_with(
        path: &Path,
        window: Duration,
        decode_line: LineDecoder,
        check_ended: SessionEndChecker,
        cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    ) -> Vec<(Transport, AgentEvent)> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line,
            derive_label: t_label,
            check_ended,
            id_derive: default_id_from_path,
        };
        let live = Arc::new(Mutex::new(HashSet::new()));
        let ctx = WatchCtx {
            source: &source,
            cursors,
            seen,
            tx: &tx,
            window,
            live: &live,
        };
        walk_jsonl(path, decoders, &ctx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        events
    }

    /// `walk_once` against a NON-EMPTY liveness snapshot, using the CC stem
    /// deriver (`cc_id_from_path`) — the id-space the real probe joins on
    /// (the registry carries session UUIDs; transcripts are `<uuid>.jsonl`).
    async fn walk_once_live(
        path: &Path,
        window: Duration,
        live_ids: &[&str],
        cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    ) -> Vec<(Transport, AgentEvent)> {
        let live: Arc<Mutex<HashSet<String>>> =
            Arc::new(Mutex::new(live_ids.iter().map(|s| s.to_string()).collect()));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended: t_ended,
            id_derive: crate::source::claude_code::cc_id_from_path,
        };
        let ctx = WatchCtx {
            source: &source,
            cursors,
            seen,
            tx: &tx,
            window,
            live: &live,
        };
        walk_jsonl(path, decoders, &ctx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        events
    }

    fn backdate_one_hour(path: &Path) {
        filetime::set_file_mtime(
            path,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - Duration::from_secs(3600),
            ),
        )
        .unwrap();
    }

    /// `walk_once_with` with the no-op decoder — the common case for tests
    /// that exercise the gate / cursor / registration paths, not decoding.
    async fn walk_once(
        path: &Path,
        window: Duration,
        check_ended: SessionEndChecker,
        cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    ) -> Vec<(Transport, AgentEvent)> {
        walk_once_with(path, window, t_decode, check_ended, cursors, seen).await
    }

    /// Drive `walk_jsonl` once over a fresh (never-seeded) file — the
    /// deterministic, timing-free repro of the #85 race. When the watcher's
    /// `walk_jsonl` (rescan / 60s poll / notify) is the FIRST to see a file,
    /// does it gate (ended/stale) or resurrect it? Returns the emitted events +
    /// the cursor it left.
    async fn first_sight_walk(
        path: &Path,
        window: Duration,
        check_ended: SessionEndChecker,
    ) -> (Vec<(Transport, AgentEvent)>, Option<u64>) {
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let events = walk_once(path, window, check_ended, &cursors, &seen).await;
        let cursor = cursors.lock().await.get(path).copied();
        (events, cursor)
    }

    /// Build the G2 fixture: a file GATED at first sight (old mtime → cursor
    /// seeded at EOF, `seen` unclaimed), returning the shared maps for the
    /// follow-up walk.
    async fn gated_fixture(
        path: &Path,
        initial: &str,
    ) -> (
        Arc<Mutex<HashMap<PathBuf, u64>>>,
        Arc<Mutex<HashMap<PathBuf, bool>>>,
    ) {
        tokio::fs::write(path, initial).await.unwrap();
        backdate_one_hour(path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let gated = walk_once(path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
        assert!(
            gated.is_empty(),
            "stale first sight must gate silently, got {gated:?}"
        );
        assert!(
            !seen.lock().await.contains_key(path),
            "a gated file must not claim `seen`"
        );
        (cursors, seen)
    }

    #[tokio::test]
    async fn gated_file_registers_on_oversized_first_append() {
        // G2: a file gated at first sight (cursor at EOF, never registered)
        // then appends > MAX_PENDING_BYTES in one burst. The oversized branch
        // used to key registration on `!known`, but a gated file IS known —
        // the agent stayed invisible until a later ≤1 MiB append.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gated-big.jsonl");
        let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
        let (cursors, seen) = gated_fixture(&path, initial).await;

        let mut full = String::from(initial);
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        tokio::fs::write(&path, &full).await.unwrap();
        assert!(
            (full.len() - initial.len()) as u64 > (1 << 20),
            "the appended span must exceed MAX_PENDING_BYTES"
        );

        let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
        let expected = AgentId::from_parts("test", &default_id_from_path(&path));
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
            )),
            "a gated file's oversized first append must register the agent, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "cursor must advance to EOF"
        );
    }

    #[tokio::test]
    async fn gated_file_oversized_ended_append_stays_unregistered() {
        // Same shape as above, but the burst ENDS the session: registering
        // would emit SessionStart AFTER the buried SessionEnd and resurrect a
        // ghost slot. The terminator must still be emitted; registration must
        // not.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gated-big-ended.jsonl");
        let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
        let (cursors, seen) = gated_fixture(&path, initial).await;

        let mut full = String::from(initial);
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
        tokio::fs::write(&path, &full).await.unwrap();

        let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
        let expected = AgentId::from_parts("test", &default_id_from_path(&path));
        assert!(
            events.iter().any(
                |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id } if *agent_id == expected)
            ),
            "the buried terminator must still emit SessionEnd, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "an ended oversized span must not register a ghost, got {events:?}"
        );
    }

    #[tokio::test]
    async fn session_end_unclaims_seen_so_a_later_append_re_registers() {
        // Self-heal layer: once a decoded line yields SessionEnd for this
        // path's agent, the path must be UN-claimed from `seen` so a LATER
        // append re-registers through the documented emit_first_sight revive.
        // Today `seen` stays claimed forever — the agent can never re-register
        // without a watcher restart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resumed.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        // Pass 1: first-sight registration.
        let window = Duration::from_secs(3600);
        let events =
            walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "live first sight must register, got {events:?}"
        );

        // Pass 2: a structural session_end line decodes to SessionEnd.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut f,
            b"{\"type\":\"system\",\"subtype\":\"session_end\"}\n",
        )
        .await
        .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
        drop(f);
        let events =
            walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
            "the structural end must decode to SessionEnd, got {events:?}"
        );
        assert!(
            !seen.lock().await.contains_key(&path),
            "SessionEnd must un-claim `seen` so a revival can re-register"
        );

        // Pass 3: the session resumes (normal lines again) — a SECOND
        // SessionStart must be emitted via the revive path.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut f, b"{\"type\":\"assistant\"}\n")
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
        drop(f);
        let events =
            walk_once_with(&path, window, t_decode_lifecycle, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a post-end append must re-register the agent, got {events:?}"
        );
    }

    /// The instant-exit ↔ pre-death-write race (#223 review finding): a write
    /// landing just before the process dies can have its notify event delivered
    /// AFTER the exit arm runs. `emit_session_exit` must drain those pending
    /// bytes (cursor → EOF) BEFORE un-claiming `seen`, or the straggler walk
    /// re-enters as a first-sight and resurrects the dead session as a ghost —
    /// with every fast rung already disarmed for it.
    #[tokio::test]
    async fn session_exit_drains_pending_bytes_so_a_straggler_walk_cannot_resurrect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "{\"type\":\"assistant\"}\n").unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let window = Duration::from_secs(3600);

        // Register normally (recent file → SessionStart, cursor at EOF).
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(events
            .iter()
            .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })));

        // The pre-death write: appended, but its notify walk has NOT run.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"assistant\"}\n")
            .unwrap();
        let pre_exit_cursor = *cursors.lock().await.get(&path).unwrap();
        let file_len = std::fs::metadata(&path).unwrap().len();
        assert!(pre_exit_cursor < file_len, "fixture: bytes must be pending");

        // The instant exit fires (process died) before the notify event lands.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended: t_ended,
            id_derive: default_id_from_path,
        };
        let live = Arc::new(Mutex::new(HashSet::new()));
        let ctx = WatchCtx {
            source: &source,
            cursors: &cursors,
            seen: &seen,
            tx: &tx,
            window,
            live: &live,
        };
        let id = default_id_from_path(&path);
        emit_session_exit(&id, decoders, &ctx).await;
        drop(tx);
        let mut exit_events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            exit_events.push(ev);
        }
        assert!(
            matches!(
                exit_events.last(),
                Some((Transport::Jsonl, AgentEvent::SessionEnd { .. }))
            ),
            "the terminator must be emitted (last), got {exit_events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(file_len),
            "the exit must drain pending bytes to EOF before un-claiming"
        );
        assert!(
            !seen.lock().await.contains_key(&path),
            "seen must be un-claimed so a genuine post-death append revives"
        );

        // The straggler notify walk: must be a no-op, NOT a ghost first-sight.
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(
            events.is_empty(),
            "a straggler walk after the exit must not resurrect, got {events:?}"
        );

        // A genuinely post-death append still revives — the self-heal contract.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"assistant\"}\n")
            .unwrap();
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a post-exit append must re-register, got {events:?}"
        );
    }

    #[tokio::test]
    async fn oversized_ended_skip_unclaims_seen_so_a_later_append_re_registers() {
        // Same self-heal for the oversized branch: a REGISTERED file whose
        // > MAX_PENDING_BYTES skipped span buries a session_end emits the
        // terminator AND un-claims `seen`, so a later small append revives the
        // agent with a fresh SessionStart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big-resumed.jsonl");
        let initial = "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n";
        tokio::fs::write(&path, initial).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        // Pass 1: first-sight registration (file is small + live).
        let window = Duration::from_secs(3600);
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "live first sight must register, got {events:?}"
        );

        // Pass 2: an oversized span ending in session_end → terminator + skip.
        let mut full = String::from(initial);
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
        tokio::fs::write(&path, &full).await.unwrap();
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
            "the buried terminator must emit SessionEnd, got {events:?}"
        );
        assert!(
            !seen.lock().await.contains_key(&path),
            "the oversized-ended skip must un-claim `seen`"
        );

        // Pass 3: a small live append revives the agent.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut f, b"{\"type\":\"assistant\"}\n")
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
        drop(f);
        let events = walk_once(&path, window, t_ended, &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a post-end append must re-register the agent, got {events:?}"
        );
    }

    #[tokio::test]
    async fn walk_jsonl_gates_a_first_sight_ended_file() {
        // #85: an ENDED session the initial read_dir missed must NOT be
        // resurrected when the rescan's walk_jsonl is the first to see it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ended.jsonl");
        let content = "{\"type\":\"system\",\"subtype\":\"session_start\"}\n\
                       {\"type\":\"system\",\"subtype\":\"session_end\"}\n";
        tokio::fs::write(&path, content).await.unwrap();
        let len = tokio::fs::metadata(&path).await.unwrap().len();

        let (events, cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
        assert!(
            events.is_empty(),
            "a never-seeded ENDED file must not emit SessionStart, got {events:?}"
        );
        assert_eq!(cursor, Some(len), "ended file must be seeded at EOF");
    }

    #[tokio::test]
    async fn walk_jsonl_gates_a_first_sight_stale_file() {
        // The stale-on-startup flake's root: an OLD file the initial read_dir
        // missed must be seeded at EOF by the rescan, not read from the top.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
            .await
            .unwrap();
        backdate_one_hour(&path);
        let len = tokio::fs::metadata(&path).await.unwrap().len();

        let (events, cursor) = first_sight_walk(&path, Duration::from_secs(60), t_ended).await;
        assert!(
            events.is_empty(),
            "a never-seeded STALE file must not emit SessionStart, got {events:?}"
        );
        assert_eq!(cursor, Some(len), "stale file must be seeded at EOF");
    }

    #[tokio::test]
    async fn known_oversized_tail_emits_session_end_if_the_skipped_span_ended() {
        // A tracked file grows by > MAX_PENDING_BYTES between passes, and that
        // skipped span buries a structural session_end marker. The watcher
        // must still emit SessionEnd before skipping to EOF — otherwise the
        // terminator is lost and the slot reaps only via the slow stale-sweep.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.jsonl");
        let initial = "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n";
        tokio::fs::write(&path, initial).await.unwrap();
        let seeded = initial.len() as u64;

        // Overwrite with the same prefix + > 1 MiB of filler + a trailing
        // session_end line (lands in the tail-scan window).
        let mut full = String::from(initial);
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
        tokio::fs::write(&path, &full).await.unwrap();
        let len = full.len() as u64;
        assert!(
            len - seeded > (1 << 20),
            "the appended span must exceed MAX_PENDING_BYTES"
        );

        // Pre-seed the cursor so the file is KNOWN at `seeded`.
        let cursors = Arc::new(Mutex::new(HashMap::from([(path.clone(), seeded)])));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended: t_ended,
            id_derive: default_id_from_path,
        };
        let live = Arc::new(Mutex::new(HashSet::new()));
        let ctx = WatchCtx {
            source: &source,
            cursors: &cursors,
            seen: &seen,
            tx: &tx,
            window: Duration::from_secs(3600),
            live: &live,
        };
        walk_jsonl(&path, decoders, &ctx).await;
        drop(tx);

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let expected = AgentId::from_parts("test", &default_id_from_path(&path));
        assert!(
            events.iter().any(
                |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id } if *agent_id == expected)
            ),
            "a buried session_end in the skipped span must still emit SessionEnd, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(len),
            "cursor must advance to EOF"
        );
    }

    #[tokio::test]
    async fn gated_revive_falls_back_to_head_cwd_when_tail_has_none() {
        // G4: Codex rollouts carry cwd ONLY on the head session_meta line. A
        // file gated at first sight then revived by a small cwd-less append
        // used to register with an EMPTY cwd (downstream: unknown cwd → the
        // short reap), because the revive read cwd only from the appended
        // tail. The revive must fall back to a bounded head read.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-gated.jsonl");
        let head =
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/repo/head\",\"id\":\"u\"}}\n";
        let (cursors, seen) = gated_fixture(&path, head).await;

        let mut full = String::from(head);
        full.push_str("{\"type\":\"assistant\"}\n");
        tokio::fs::write(&path, &full).await.unwrap();

        let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
        let cwds: Vec<PathBuf> = events
            .iter()
            .filter_map(|(_, e)| match e {
                AgentEvent::SessionStart { cwd, .. } => Some(cwd.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            cwds,
            vec![PathBuf::from("/repo/head")],
            "the revive SessionStart must carry the head cwd, got {events:?}"
        );
    }

    #[tokio::test]
    async fn gated_file_revives_on_small_append_with_tail_cwd() {
        // S1 (the audit's never-pinned plain case): a file GATED at first sight
        // (stale mtime → cursor seeded at EOF, no SessionStart) then revived by
        // a SMALL newline-terminated append must register — SessionStart +
        // Rename — and the registration carries the APPEND's cwd (the tail
        // read wins; the head read is only the G4 fallback when the tail
        // carries none, pinned by the head-vs-tail value split below).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gated-small.jsonl");
        let head = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";
        let (cursors, seen) = gated_fixture(&path, head).await;

        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut f,
            b"{\"type\":\"assistant\",\"cwd\":\"/repo/tail\"}\n",
        )
        .await
        .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut f).await.unwrap();
        drop(f);

        let events = walk_once(&path, Duration::from_secs(60), t_ended, &cursors, &seen).await;
        let expected = AgentId::from_parts("test", &default_id_from_path(&path));
        let starts: Vec<(AgentId, PathBuf)> = events
            .iter()
            .filter_map(|(_, e)| match e {
                AgentEvent::SessionStart { agent_id, cwd, .. } => Some((*agent_id, cwd.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            vec![(expected, PathBuf::from("/repo/tail"))],
            "the small-append revive must register exactly once, carrying the APPEND's cwd, got {events:?}"
        );
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                AgentEvent::Rename { agent_id, .. } if *agent_id == expected
            )),
            "the revive must emit the Rename half of the registration pair, got {events:?}"
        );
    }

    #[tokio::test]
    async fn walk_jsonl_emits_for_a_first_sight_recent_live_file() {
        // The gate must NOT over-suppress: a recent, not-ended file seen first by
        // any path is a live session and must still get its SessionStart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
            .await
            .unwrap();

        let (events, _cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a recent, not-ended file seen first must still emit SessionStart, got {events:?}"
        );
    }

    const LIVE_UUID: &str = "01000000-0000-7000-8000-0000000000aa";

    #[tokio::test]
    async fn probe_live_stale_file_registers_at_first_sight() {
        // T4: pixtuoid starts AFTER a long-idle live session. mtime says
        // historical (outside the window), but the first-party liveness probe
        // says the owning process is ALIVE — the gate must not hide it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        backdate_one_hour(&path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(60),
            &[LIVE_UUID],
            &cursors,
            &seen,
        )
        .await;
        let expected = AgentId::from_parts("test", LIVE_UUID);
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
            )),
            "a probe-live stale transcript must register at first sight, got {events:?}"
        );
    }

    #[tokio::test]
    async fn probe_miss_keeps_the_stale_gate() {
        // A non-empty live set that does NOT contain this transcript's id
        // changes nothing: the recency gate applies as today.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        backdate_one_hour(&path);
        let len = tokio::fs::metadata(&path).await.unwrap().len();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(60),
            &["99999999-9999-7999-8999-999999999999"],
            &cursors,
            &seen,
        )
        .await;
        assert!(
            events.is_empty(),
            "a stale transcript the probe does not vouch for must stay gated, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(len),
            "gated file must be seeded at EOF"
        );
    }

    #[tokio::test]
    async fn probe_never_gates_a_recent_file() {
        // ADDITIVE-ONLY: a recent file absent from a non-empty live set still
        // registers — the probe can only admit, never hide what mtime admits.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(3600),
            &["99999999-9999-7999-8999-999999999999"],
            &cursors,
            &seen,
        )
        .await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a recent file must register regardless of the probe, got {events:?}"
        );
    }

    #[tokio::test]
    async fn probe_live_oversized_stale_file_registers_via_head_read() {
        // A probe-live stale transcript whose whole body exceeds
        // MAX_PENDING_BYTES at first sight skips the gate and lands in the
        // #204 oversized first-sight branch: registered from a bounded head
        // read (cwd off line 1), backlog skipped to EOF.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        let mut full = String::from("{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n");
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        assert!(full.len() as u64 > (1 << 20), "body must exceed 1 MiB");
        tokio::fs::write(&path, &full).await.unwrap();
        backdate_one_hour(&path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(60),
            &[LIVE_UUID],
            &cursors,
            &seen,
        )
        .await;
        let cwds: Vec<PathBuf> = events
            .iter()
            .filter_map(|(_, e)| match e {
                AgentEvent::SessionStart { cwd, .. } => Some(cwd.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            cwds,
            vec![PathBuf::from("/repo/head")],
            "the oversized probe-live first sight must register with the head cwd, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "backlog must be skipped to EOF, not replayed"
        );
    }

    #[tokio::test]
    async fn scan_pass_re_vouches_a_transiently_gated_live_file() {
        // F1: a transient probe miss at first sight (registry file
        // mid-rewrite, a read race) gates a LIVE session — and without a
        // re-check every later pass exits at cursor == file_len and never
        // asks the probe again, hiding the session permanently. Each SCAN
        // pass (whose probe snapshot was just refreshed) must re-ask about
        // gated-but-never-registered files and replay a vouched one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        backdate_one_hour(&path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let live: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended: t_ended,
            id_derive: crate::source::claude_code::cc_id_from_path,
        };
        let ctx = WatchCtx {
            source: &source,
            cursors: &cursors,
            seen: &seen,
            tx: &tx,
            window: Duration::from_secs(60),
            live: &live,
        };

        // Pass 1: empty probe snapshot (the transient miss) → gated.
        scan_root(dir.path(), decoders, &ctx).await;
        assert!(rx.try_recv().is_err(), "pass 1 must gate silently");
        assert!(
            !seen.lock().await.contains_key(&path),
            "gated, not registered"
        );

        // The next probe refresh sees the session — simulate it by mutating
        // the shared snapshot the way the run loop's refresh arms do.
        live.lock().await.insert(LIVE_UUID.to_string());

        // Pass 2: the scan must re-vouch the gated file and register it.
        scan_root(dir.path(), decoders, &ctx).await;
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let expected = AgentId::from_parts("test", LIVE_UUID);
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                AgentEvent::SessionStart { agent_id, .. } if *agent_id == expected
            )),
            "a re-vouched scan pass must register the gated live session, got {events:?}"
        );

        // Pass 3 (loop guard): the file registered → claimed `seen` → out of
        // the candidate set; nothing is re-emitted while the probe vouches.
        scan_root(dir.path(), decoders, &ctx).await;
        assert!(
            rx.try_recv().is_err(),
            "a registered file must not be re-vouched/replayed again"
        );
    }

    #[tokio::test]
    async fn probe_live_oversized_ended_first_sight_stays_unregistered() {
        // M1: the probe bypasses the first-sight gate — INCLUDING its ended
        // tail-scan — so a probe-admitted !known >1MiB ENDED transcript
        // reaches the oversized branch. Its ended check used to be gated on
        // `known` (assuming should_seed_at_eof had already filtered !known
        // ended files, which the probe bypass breaks): the terminator was
        // never emitted AND the #204 path registered a ghost for a session
        // that is over.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{LIVE_UUID}.jsonl"));
        let mut full = String::from("{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n");
        full.push_str(&"{\"type\":\"assistant\"}\n".repeat(60_000));
        full.push_str("{\"type\":\"system\",\"subtype\":\"session_end\"}\n");
        assert!(full.len() as u64 > (1 << 20), "body must exceed 1 MiB");
        tokio::fs::write(&path, &full).await.unwrap();
        backdate_one_hour(&path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(60),
            &[LIVE_UUID],
            &cursors,
            &seen,
        )
        .await;
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "an ended oversized probe-admitted first sight must not register a ghost, got {events:?}"
        );
        let expected = AgentId::from_parts("test", LIVE_UUID);
        assert!(
            events.iter().any(
                |(_, e)| matches!(e, AgentEvent::SessionEnd { agent_id } if *agent_id == expected)
            ),
            "the buried terminator must still emit SessionEnd (a reducer no-op for an unknown id), got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "backlog must be skipped to EOF, not replayed"
        );
    }

    #[tokio::test]
    async fn probe_parent_uuid_does_not_admit_subagent_transcript() {
        // Subagent transcripts (<parent-uuid>/subagents/agent-*.jsonl) are NOT
        // in the registry; the join key is the file STEM (an agent id, not a
        // session UUID), so the parent's registry entry must not admit them —
        // they keep today's mtime gate.
        let dir = tempfile::tempdir().unwrap();
        let sub_dir = dir.path().join(LIVE_UUID).join("subagents");
        tokio::fs::create_dir_all(&sub_dir).await.unwrap();
        let path = sub_dir.join("agent-deadbeef.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/repo\"}\n")
            .await
            .unwrap();
        backdate_one_hour(&path);
        let len = tokio::fs::metadata(&path).await.unwrap().len();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_once_live(
            &path,
            Duration::from_secs(60),
            &[LIVE_UUID],
            &cursors,
            &seen,
        )
        .await;
        assert!(
            events.is_empty(),
            "a stale subagent transcript must stay gated even when its parent is probe-live, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(len),
            "gated subagent transcript must be seeded at EOF"
        );
    }

    // ── #222: oversized-skip Task scan ──────────────────────────────────────
    // Mid-attach to a delegating session with > MAX_PENDING_BYTES pending
    // skips the backlog, losing the in-flight Agent dispatch (its PreToolUse
    // hook predates attach too) — active_tasks stays empty, so subagent-leak
    // suppression is off and b1 never arms. The oversized branch must
    // tail-scan the last TASK_SCAN_BYTES and re-emit exactly the UNMATCHED
    // Task ActivityStarts. These drive the REAL decode_cc_line so the line
    // shapes (Agent tool_use with subagent_type / tool_result) are wire-true.

    const FILLER_LINE: &str = "{\"type\":\"assistant\"}\n";
    const CC_HEAD_LINE: &str = "{\"type\":\"assistant\",\"cwd\":\"/repo/head\"}\n";

    fn cc_task_dispatch_line(tuid: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "cwd": "/repo/head",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": tuid, "name": "Agent",
                      "input": { "description": "explore",
                                 "subagent_type": "code-explorer",
                                 "prompt": "go" } }
                ]
            }
        })
        .to_string()
            + "\n"
    }

    fn cc_task_result_line(tuid: &str) -> String {
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": tuid, "content": "done" }
                ]
            }
        })
        .to_string()
            + "\n"
    }

    /// `CC_HEAD_LINE` + filler past MAX_PENDING_BYTES + the given tail lines —
    /// the whole body is one oversized first-sight pending span.
    fn oversized_body(tail_lines: &[String]) -> String {
        let mut full = String::from(CC_HEAD_LINE);
        while full.len() <= (1usize << 20) + 4096 {
            full.push_str(FILLER_LINE);
        }
        for l in tail_lines {
            full.push_str(l);
        }
        full
    }

    /// The Jsonl-tagged Task ActivityStarts among `events`, as tuids.
    fn task_start_tuids(events: &[(Transport, AgentEvent)]) -> Vec<String> {
        events
            .iter()
            .filter_map(|(t, e)| match e {
                AgentEvent::ActivityStart {
                    tool_use_id: Some(tuid),
                    detail: Some(d),
                    ..
                } if d.is_task() => {
                    assert_eq!(*t, Transport::Jsonl, "synthesized starts are Jsonl-tagged");
                    Some(tuid.clone())
                }
                _ => None,
            })
            .collect()
    }

    async fn walk_oversized_cc(
        path: &Path,
        window: Duration,
        cursors: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    ) -> Vec<(Transport, AgentEvent)> {
        walk_once_with(
            path,
            window,
            crate::source::claude_code::decode_cc_line,
            t_ended,
            cursors,
            seen,
        )
        .await
    }

    #[tokio::test]
    async fn oversized_attach_seeds_unmatched_task_dispatch() {
        // The headline #222 case: a recent > 1 MiB transcript whose tail holds
        // an Agent dispatch with NO matching tool_result — the walk must
        // register the agent AND re-emit that dispatch as a Task
        // ActivityStart (after the SessionStart, so the reducer has a slot).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-big.jsonl");
        let full = oversized_body(&[cc_task_dispatch_line("tu_task")]);
        tokio::fs::write(&path, &full).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
        let start_pos = events
            .iter()
            .position(|(_, e)| matches!(e, AgentEvent::SessionStart { .. }))
            .expect("the oversized first sight must register the agent (#204)");
        assert_eq!(
            task_start_tuids(&events),
            vec!["tu_task".to_string()],
            "the unmatched in-flight dispatch must be re-emitted, got {events:?}"
        );
        let task_pos = events
            .iter()
            .position(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. }))
            .expect("checked above");
        assert!(
            start_pos < task_pos,
            "registration must precede the synthesized Task start (a JSONL event for an unknown id is a reducer no-op)"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "cursor must land at EOF — the scan seeds tasks, it must not replay the backlog"
        );
    }

    #[tokio::test]
    async fn oversized_attach_matched_task_is_not_seeded() {
        // A dispatch whose tool_result also sits in the window has RETURNED —
        // re-emitting it would pin the parent Delegating forever (no further
        // completion is coming). Window geometry makes this exact: a
        // completion is always later in the file than its start.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-done.jsonl");
        let full = oversized_body(&[
            cc_task_dispatch_line("tu_task"),
            cc_task_result_line("tu_task"),
        ]);
        tokio::fs::write(&path, &full).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "registration still fires, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. })),
            "a matched (returned) dispatch must not be seeded — and no other backlog event may leak from the scan, got {events:?}"
        );
    }

    #[tokio::test]
    async fn oversized_attach_ended_session_skips_task_scan() {
        // Ended wins: the buried terminator just emitted SessionEnd, so
        // seeding a Task afterwards would animate a ghost delegation. The
        // file is pre-seeded KNOWN at the head — a recent ENDED file at FIRST
        // sight is gated by should_seed_at_eof and never reaches the
        // oversized branch at all.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-ended.jsonl");
        let full = oversized_body(&[
            cc_task_dispatch_line("tu_task"),
            "{\"type\":\"system\",\"subtype\":\"session_end\"}\n".to_string(),
        ]);
        tokio::fs::write(&path, &full).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::from([(
            path.clone(),
            CC_HEAD_LINE.len() as u64,
        )])));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionEnd { .. })),
            "the buried terminator must still emit SessionEnd, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. })),
            "an ended span must not seed Task starts, got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "an ended span must not register either (ghost), got {events:?}"
        );
    }

    #[tokio::test]
    async fn oversized_attach_unregistered_skips_task_scan() {
        // A stale, probe-less oversized file is gated unregistered (no slot)
        // — JSONL events for an unknown id are reducer no-ops, so the scan
        // must not run (no wasted decode, no orphan Task events).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-stale.jsonl");
        let full = oversized_body(&[cc_task_dispatch_line("tu_task")]);
        tokio::fs::write(&path, &full).await.unwrap();
        backdate_one_hour(&path);
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(60), &cursors, &seen).await;
        assert!(
            events.is_empty(),
            "a gated unregistered oversized file must emit NOTHING — no Task seeding without a slot, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "gated file must still be seeded at EOF"
        );
    }

    #[tokio::test]
    async fn oversized_attach_dispatch_outside_window_is_missed() {
        // THE documented residual: a dispatch buried deeper than
        // TASK_SCAN_BYTES of subsequent traffic keeps the pre-#222 behavior
        // (skipped — the parent re-enters Delegating only via live signals).
        // Pinned explicitly so a window-size change is a conscious decision.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-buried.jsonl");
        let mut full = String::from(CC_HEAD_LINE);
        let dispatch = cc_task_dispatch_line("tu_buried");
        full.push_str(&dispatch);
        let dispatch_end = full.len();
        while full.len() <= (1usize << 20) + 4096 {
            full.push_str(FILLER_LINE);
        }
        assert!(
            (full.len() - dispatch_end) as u64 > TASK_SCAN_BYTES,
            "the dispatch must sit deeper than the scan window"
        );
        tokio::fs::write(&path, &full).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "registration still fires, got {events:?}"
        );
        assert!(
            task_start_tuids(&events).is_empty(),
            "a dispatch outside the tail window is consciously missed (bounded residual), got {events:?}"
        );
    }

    #[tokio::test]
    async fn task_scan_handles_partial_first_line() {
        // The window boundary (file_len - TASK_SCAN_BYTES) almost never lands
        // on a line boundary. Engineer it to split a Task dispatch mid-JSON:
        // the straddled fragment must be skipped (not decoded, no panic) and
        // a complete dispatch inside the window still seeds.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleg-straddle.jsonl");
        let task_a = cc_task_dispatch_line("tu_straddle");
        let task_b = cc_task_dispatch_line("tu_inside");

        let mut full = String::from(CC_HEAD_LINE);
        // Deep enough that suffix + window keeps the total > MAX_PENDING_BYTES.
        while full.len() < (1usize << 20) {
            full.push_str(FILLER_LINE);
        }
        let offset_a = full.len();
        full.push_str(&task_a);
        full.push_str(&task_b);
        // Pad the tail so the boundary lands strictly inside task_a's bytes.
        let delta = task_a.len() / 2;
        let target_len = offset_a + delta + TASK_SCAN_BYTES as usize;
        let pad = target_len - full.len();
        assert!(pad > FILLER_LINE.len(), "padding must fit one JSON line");
        full.push_str("{\"type\":\"assistant\"}");
        full.push_str(&" ".repeat(pad - FILLER_LINE.len()));
        full.push('\n');
        assert_eq!(full.len(), target_len);
        let boundary = full.len() - TASK_SCAN_BYTES as usize;
        assert!(
            boundary > offset_a && boundary < offset_a + task_a.len(),
            "the window boundary must split the straddled dispatch mid-line"
        );
        tokio::fs::write(&path, &full).await.unwrap();
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));

        let events = walk_oversized_cc(&path, Duration::from_secs(3600), &cursors, &seen).await;
        assert_eq!(
            task_start_tuids(&events),
            vec!["tu_inside".to_string()],
            "the complete in-window dispatch seeds; the straddled fragment is skipped, got {events:?}"
        );
        assert_eq!(
            cursors.lock().await.get(&path).copied(),
            Some(full.len() as u64),
            "cursor must land at EOF"
        );
    }
}
