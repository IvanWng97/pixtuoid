use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Result;
use notify::{Config, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::source::exit_watch::ExitWatch;
use crate::source::TaggedSender;

mod health;
mod liveness;
#[cfg(test)]
mod tests;
mod unclaim;
mod walk;

pub use liveness::{LivenessProbe, ProbeSnapshot};
pub use unclaim::ChildEndUnclaims;
// `is_subagent_path` + `LineDecoder` moved to the always-compiled `decoder`
// module (a pure CC decoder + the registry name them in the wasm build). Only
// this module's own tests still reach it by the `jsonl::` path, so the
// re-export is test-scoped (prod callers use `decoder::is_subagent_path`).
#[cfg(test)]
pub(crate) use crate::source::decoder::is_subagent_path;

// Re-exported crate-wide: the hook socket's accept loop latches its
// persistent-error warn the same way (one warn per failure streak, one
// recovery info) — a second local latch would drift from this one.
pub(crate) use health::FailureLatch;
use liveness::{
    emit_proof_of_life, emit_session_exit, refresh_probe_snapshot, NegativeVouch,
    NEGATIVE_VOUCH_MIN_SPAN,
};
use unclaim::drain_child_end_unclaims;
use walk::{scan_root, walk_jsonl};

// The ONE `LineDecoder` definition lives in the always-compiled `decoder`
// module (the registry + a pure CC decoder name it in the wasm build);
// re-exported so watcher-facing callers keep the `jsonl::LineDecoder` path —
// a second local alias would be the two-copies drift bug.
pub use crate::source::decoder::LineDecoder;
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
    crate::id::normalize_path_key(&p.to_string_lossy())
}

/// Decides which `.jsonl` FILES a watcher walks — checked after the extension
/// gate in `walk_jsonl`, so it filters files, never directories. The default
/// (`accept_all_paths`) admits every transcript. **Antigravity** overrides it to
/// skip the sibling `transcript_full.jsonl`: the CLI writes BOTH a truncated
/// `transcript.jsonl` and an untruncated `transcript_full.jsonl` per conversation
/// in one logs dir, carrying the SAME step stream, so without the filter one
/// conversation mints two path-keyed `AgentId`s and double-renders.
pub type PathFilter = fn(&Path) -> bool;

fn accept_all_paths(_p: &Path) -> bool {
    true
}

/// The per-source decode/label/end/id fn-pointers (the invariant-#3 seam)
/// bundled so the seed/scan/walk helpers thread ONE Copy value, not four.
#[derive(Clone, Copy)]
struct SourceDecoders {
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_ended: SessionEndChecker,
    id_derive: IdDeriver,
    path_filter: PathFilter,
}

/// Shared per-run watch state, borrowed by the scan/walk helpers.
#[derive(Clone, Copy)]
struct WatchCtx<'a> {
    source: &'a Arc<str>,
    cursors: &'a Arc<Mutex<HashMap<PathBuf, u64>>>,
    /// First-sight claims: path → claim-held. `true` = the registration pair
    /// was emitted and the claim is HELD (appends decode without
    /// re-registering). `false` = the claim was RELEASED by the child-end
    /// un-claim (#246): the path stays KNOWN — so `revouch_gated_files` won't
    /// replay it however live the probe says the (still-open) rollout is —
    /// but its next append re-registers through `emit_first_sight`. Absent =
    /// never registered, or fully retired by an exit/terminator un-claim.
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
    /// Second writer: `emit_session_exit` purges a confirmed-dead id, so a
    /// probe-failure pass can't re-admit a session the exit rung just ended.
    live: &'a Arc<Mutex<HashSet<String>>>,
}

/// The persistent scan-state bundle threaded through every `run_scan_pass`
/// (initial seed / 250ms rescan / 60s poll). Created once in `run` and borrowed
/// `&mut` by each pass; the vouch and pid-binding maps also serve the
/// instant-exit arm directly (see `run`'s `exit_rx` branch).
struct ScanState {
    /// Negative-vouch confirmations: a probe-missing id must stay missing for
    /// `NEGATIVE_VOUCH_MIN_SPAN` before its exit is emitted (guards a probe
    /// blip from ending a live session).
    vouch: NegativeVouch,
    /// pid → the session ids a healthy probe snapshot bound to it
    /// (`ProbeSnapshot::pid_of` folded by `refresh_probe_snapshot`) — the
    /// join the instant-exit arm uses to translate an OS exit into
    /// SessionEnds. Entries leave on exit (whole pid), negative-vouch
    /// confirm (single id, see `unbind_session`), or a rebind migration
    /// (the snapshot observes the id under a different pid).
    pid_bindings: HashMap<i32, HashSet<String>>,
    /// Latches the root-scan's persistent-error breadcrumb (warn once per
    /// failure streak, info on recovery — the `FailureLatch` convention).
    root_health: FailureLatch,
}

impl ScanState {
    fn new(negative_vouch_min_span: Duration) -> Self {
        Self {
            vouch: NegativeVouch::new(negative_vouch_min_span),
            pid_bindings: HashMap::new(),
            root_health: FailureLatch::default(),
        }
    }
}

pub struct JsonlWatcher {
    root: PathBuf,
    initial_window: Duration,
    source_name: String,
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_session_ended: SessionEndChecker,
    id_derive: IdDeriver,
    path_filter: PathFilter,
    liveness_probe: Option<LivenessProbe>,
    poll_interval: Duration,
    negative_vouch_min_span: Duration,
    child_end_unclaims: Option<ChildEndUnclaims>,
}

const DEFAULT_INITIAL_WINDOW: Duration = Duration::from_secs(3600);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

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
            path_filter: accept_all_paths,
            liveness_probe: None,
            poll_interval: DEFAULT_POLL_INTERVAL,
            negative_vouch_min_span: NEGATIVE_VOUCH_MIN_SPAN,
            child_end_unclaims: None,
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

    /// Restrict which `.jsonl` FILES this watcher walks (default: every
    /// transcript). See [`PathFilter`] — Antigravity uses it to skip the
    /// duplicate `transcript_full.jsonl` sibling and avoid double-rendering.
    pub fn with_path_filter(mut self, path_filter: PathFilter) -> Self {
        self.path_filter = path_filter;
        self
    }

    pub fn with_liveness_probe(mut self, probe: LivenessProbe) -> Self {
        self.liveness_probe = Some(probe);
        self
    }

    /// Attach the #246 child-end un-claim side-channel (see
    /// [`ChildEndUnclaims`]). The watcher becomes the CONSUMER: on each pass
    /// it drains the handle's ids that match its own claimed transcripts and
    /// releases those claims so the next append re-registers. `doc(hidden)`
    /// like the registry: an internal seam wired by the in-crate sources
    /// (`ClaudeCodeSource`/`CodexSource` own the production wiring), pub only
    /// so the integration tests can drive it.
    #[doc(hidden)]
    pub fn with_child_end_unclaims(mut self, unclaims: ChildEndUnclaims) -> Self {
        self.child_end_unclaims = Some(unclaims);
        self
    }

    /// One scan pass: refresh the liveness probe → (optionally) drain child-end
    /// un-claims → re-scan the root → re-emit proof-of-life when the probe is
    /// healthy. The initial seed + the 250ms rescan + the 60s poll all run this
    /// SAME sequence; only the seed skips the un-claim drain (`drain = false` —
    /// nothing has been pushed at startup). `decoders` is `Copy`.
    async fn run_scan_pass(
        &self,
        ctx: &WatchCtx<'_>,
        scan_state: &mut ScanState,
        exit_watch: Option<&ExitWatch>,
        unclaims: Option<&ChildEndUnclaims>,
        decoders: SourceDecoders,
        drain: bool,
    ) {
        let healthy = refresh_probe_snapshot(
            self.liveness_probe.as_ref(),
            &mut scan_state.vouch,
            &mut scan_state.pid_bindings,
            exit_watch,
            decoders,
            ctx,
        )
        .await;
        if drain {
            drain_child_end_unclaims(unclaims, decoders, ctx).await;
        }
        scan_root(&self.root, decoders, ctx, &mut scan_state.root_health).await;
        if healthy {
            emit_proof_of_life(ctx.live, ctx.source, ctx.tx).await;
        }
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
        // The persistent scan-state bundle (vouch + pid→ids bindings + root-scan
        // health latch), threaded `&mut` through every `run_scan_pass` and read
        // directly by the instant-exit arm below.
        let mut scan_state = ScanState::new(self.negative_vouch_min_span);

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

        // Bound on buffered notify PATHS (#585) so a reducer stall can't grow it
        // without limit. 1024 = generous burst headroom (the loop drains ~instantly
        // normally), ~256KB-capped; drop-on-Full is safe — see the sharp edge.
        const NOTIFY_PATH_CHANNEL_CAP: usize = 1024;
        let (notify_tx, mut notify_rx) =
            tokio::sync::mpsc::channel::<PathBuf>(NOTIFY_PATH_CHANNEL_CAP);
        let mut notify_health = FailureLatch::default();
        let mut notify_backpressure = FailureLatch::default();
        let event_handler = move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if notify_health.on_success() {
                    tracing::info!("file-watch backend is delivering events again");
                }
                for path in event.paths {
                    if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        // try_send off notify's own thread. Full → drop (the poll
                        // re-walks), latched so a sustained stall warns once, not per path.
                        match notify_tx.try_send(path) {
                            Ok(()) => {
                                if notify_backpressure.on_success() {
                                    tracing::info!("notify path channel draining again");
                                }
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                if notify_backpressure.on_failure() {
                                    tracing::warn!(
                                        "notify path channel saturated under load — dropping paths; the 60s poll re-walks them"
                                    );
                                }
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
                        }
                    }
                }
            }
            // A backend error means events were LOST (inotify queue overflow,
            // an FSEvents failure) — the 60s poll papers over the gap, but
            // the user must get a breadcrumb. Latched (warn once per failure
            // streak, info on recovery — the FailureLatch convention the
            // root-scan and accept-loop breadcrumbs share): a persistently
            // broken backend must not warn on every delivery.
            Err(e) => {
                if notify_health.on_failure() {
                    warn!(
                        "file-watch backend error ({e}); events may have been lost — \
                         the poll backstop covers until it recovers"
                    );
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
        let unclaims = self.child_end_unclaims.clone();
        let decoders = SourceDecoders {
            decode_line: self.decode_line,
            derive_label: self.derive_label,
            check_ended: self.check_session_ended,
            id_derive: self.id_derive,
            path_filter: self.path_filter,
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
            self.run_scan_pass(
                &ctx,
                &mut scan_state,
                exit_watch.as_ref(),
                unclaims.as_ref(),
                decoders,
                false,
            )
            .await;
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
                    // Drain BEFORE the walk (not only on scan passes): the
                    // un-claim then typically lands on the first notify after
                    // the hook Stop — often a sibling file's event, well
                    // before turn N+1 — instead of waiting out the 60s poll
                    // while turn-N+1 bytes stream past as unknown-id no-ops.
                    drain_child_end_unclaims(unclaims.as_ref(), decoders, &ctx).await;
                    walk_jsonl(&path, decoders, &ctx).await;
                }
                _ = &mut rescan_delay, if !rescan_done => {
                    rescan_done = true;
                    self.run_scan_pass(
                        &ctx, &mut scan_state,
                        exit_watch.as_ref(), unclaims.as_ref(), decoders, true,
                    ).await;
                }
                _ = poll.tick() => {
                    self.run_scan_pass(
                        &ctx, &mut scan_state,
                        exit_watch.as_ref(), unclaims.as_ref(), decoders, true,
                    ).await;
                }
                Some(pid) = exit_rx.recv() => {
                    // Instant exit (#223 rung 2): the watched OS process
                    // died. Translate through the pid→ids binding; an
                    // unknown pid (already unbound by a negative-vouch
                    // confirm, or a duplicate event) is a no-op.
                    if let Some(ids) = scan_state.pid_bindings.remove(&pid) {
                        for id in ids {
                            debug!("instant exit: pid {pid} died; emitting SessionEnd for {id}");
                            emit_session_exit(&id, decoders, &ctx).await;
                            // The next healthy snapshot will see the id gone
                            // anyway; forget it NOW so the negative vouch
                            // can't re-confirm the exit we just emitted.
                            scan_state.vouch.forget(&id);
                        }
                    }
                }
            }
        }
    }
}
