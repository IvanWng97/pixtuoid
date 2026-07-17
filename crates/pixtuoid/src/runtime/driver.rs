//! The async runtime glue: builds the tokio runtime, spawns the reducer
//! task + sources, binds the hook socket, and drives either the TUI or the
//! headless summary loop until Ctrl-C.
//!
//! Split out of `runtime/mod.rs` so this file — structurally unreachable by
//! any headless test (real tokio runtime + `block_on` + `ctrl_c` + socket
//! bind; see issue #103) — can be excluded from coverage on its own, while
//! the tested helpers (`RunConfig`, capacity math, `summarize`) keep full
//! coverage accounting in the parent module. One exception: `headless_loop`'s
//! signal handling takes the ctrl_c future as an injected seam, so its
//! registration-failure arm IS unit-tested here (the file stays
//! coverage-excluded regardless).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::copilot::CopilotSource;
use pixtuoid_core::source::daemon::{self, PresenceMsg};
use pixtuoid_core::source::grok::GrokSource;
use pixtuoid_core::source::hook::HookRouter;
use pixtuoid_core::source::jsonl::ChildEndUnclaims;
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::source::omp::OmpSource;
use pixtuoid_core::source::registry;
use pixtuoid_core::source::DynSource;
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
use tokio::sync::{mpsc, watch};

use super::{
    boot_capacities_for, cap_boot_capacities, summarize, ConnectedSources, RunConfig, SceneRx,
    FALLBACK_DESKS,
};

pub fn run(cfg: RunConfig) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move { run_async(cfg).await })
}

async fn run_async(cfg: RunConfig) -> Result<()> {
    let RunConfig {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
        desk_cap,
        headless,
        config_path,
        theme,
        pets,
        connected,
        log_path,
        first_run,
        audio,
    } = cfg;
    // Ambient audio (#633): spawn at boot only when a PERSISTED unmute says
    // so — the muted default costs nothing (no device, no thread, no
    // buffers); run_tui lazily spawns on the first `m` unmute instead.
    let audio_handle = if !audio.muted && !headless {
        crate::audio::spawn(audio.volume)
    } else {
        crate::audio::AudioHandle::disabled()
    };
    // Focus-jump pid point-query roots (cloned: build_source_set consumes the
    // originals). CC = projects root (sessions registry derived in-core);
    // Codex = the rollout tree the fd probe walks.
    let focus_roots = (projects_root.clone(), codex_sessions_root.clone());
    // The live, shared connected-source set: the reducer-task gate reads it, the
    // Sources panel mutates it. Seeded from the resolved boot flags.
    let connected = ConnectedSources::new(connected);
    // The runtime source set, built in ONE place (`build_source_set`): the
    // `HookRouter` (the shared-socket owner — every source's hooks ride it) plus
    // the transcript-bearing watchers (CC / Antigravity / Codex / Copilot / omp).
    // The hook-only sources (Reasonix / CodeWhale / opencode / Cursor / Hermes) and the daemon
    // (OpenClaw) have no watchable JSONL — their payloads ride the router's socket,
    // attributed per-payload by `_pixtuoid_source` — so they have no entry here.
    // Resolve the bound socket (Unix) / pipe (Windows) the HookRouter binds; the
    // Sources panel shows the same path (explicit --socket override, else default).
    let socket_path = socket.unwrap_or_else(ClaudeCodeSource::default_socket_path);
    // Daemon-presence SIDE channel (invariant #2: NOT the AgentEvent channel). The
    // HookRouter demux decodes daemon payloads into source-tagged presence deltas
    // sent here; the shared exit watch drains gateway-pid deaths into the SAME
    // channel as `(source, PidExited)`; the reducer task merges both into
    // SceneState::daemons.
    let (presence_tx, presence_rx) = tokio::sync::mpsc::unbounded_channel::<PresenceMsg>();
    let presence_exit_watch = daemon::spawn_presence_exit_watch(presence_tx.clone());
    let sources = build_source_set(
        socket_path.clone(),
        projects_root,
        codex_sessions_root,
        Some(presence_tx),
    );

    let (tx, rx) =
        mpsc::channel::<(Transport, AgentEvent)>(pixtuoid_core::source::EVENT_CHANNEL_CAPACITY);
    let boot_caps: [usize; MAX_FLOORS] = match (desk_cap, headless) {
        // Headless: no terminal to measure. Honor the cap as-is, else the fallback.
        (Some(cap), true) => [cap; MAX_FLOORS],
        (None, true) => [FALLBACK_DESKS; MAX_FLOORS],
        // Interactive: measure the real per-floor layout capacity FIRST, then clamp
        // to the optional cap. Clamping (not `[cap; _]`) keeps the boot atomics from
        // being seeded above the layout's real capacity — `fetch_max` only grows, so
        // an over-seed strands agents on non-existent desks until the terminal grows.
        (cap, false) => cap_boot_capacities(compute_boot_capacities(), cap),
    };
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));

    let floor_caps: Arc<[AtomicUsize; MAX_FLOORS]> =
        Arc::new(std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])));

    tokio::spawn(reducer_task(
        rx,
        scene_tx,
        Arc::clone(&floor_caps),
        connected.clone(),
        presence_rx,
        presence_exit_watch,
    ));

    // Source-health side channel (#157): a fatal source exit must reach the
    // TUI footer — in default TUI mode tracing only reaches the log file, and
    // the office silently freezing is the worst failure class. Deliberately
    // NOT an AgentEvent: the one event channel carries agent activity (its
    // Transport tag drives hook-wins dedup), not source lifecycle.
    let (health_tx, health_rx) = tokio::sync::watch::channel(Vec::new());
    let mut manager = SourceManager::new();
    for src in sources {
        manager = manager.with_source(src);
    }
    let _source_handles = manager.spawn_with_health(tx, health_tx);

    if headless {
        headless_loop(scene_rx, health_rx).await
    } else {
        crate::tui::run_tui(crate::tui::TuiSession {
            scene_rx,
            pack_dir,
            floor_caps,
            theme,
            config_path,
            desk_cap,
            pets,
            source_health: health_rx,
            socket_path,
            connected,
            log_path,
            first_run,
            focus_roots,
            audio: audio_handle,
            audio_cfg: audio,
        })
        .await
    }
}

/// Build the runtime source set `run_async` spawns — the ONE place that set is
/// constructed: the `HookRouter` (shared-socket owner) + the transcript-bearing
/// watchers. A transcript source registered in the core registry but missing here
/// (a silent "never spawns" no-op) is caught by
/// `build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router`.
/// Each transcript source carries different typed config (CC's projects root,
/// Codex's sessions root), so this stays imperative rather than a registry-driven
/// loop — invariant #3's per-source-typed seam. Hook-only sources + the daemon
/// (OpenClaw) are absent by design — they ride the router's shared socket.
pub(crate) fn build_source_set(
    socket_path: PathBuf,
    projects_root: Option<PathBuf>,
    codex_sessions_root: Option<PathBuf>,
    presence_tx: Option<daemon::PresenceSender>,
) -> Vec<Box<dyn DynSource>> {
    let mut cc_src = ClaudeCodeSource::default_paths();
    if let Some(p) = projects_root {
        cc_src.projects_root = p;
    }
    let ag_src = AntigravitySource::default_paths();
    let copilot_src = CopilotSource::default_paths();
    let omp_src = OmpSource::default_paths();

    let mut codex_src = CodexSource::default_paths();
    if let Some(p) = codex_sessions_root {
        codex_src.sessions_root = p;
    }

    // #246: ONE shared child-end un-claim handle. The HookRouter's hook tee is the
    // PRODUCER (every source's SubagentStop rides the one shared socket it owns);
    // both watchers CONSUME — each drains only the ids whose transcripts it claims
    // (AgentId is source-namespaced), so a Codex child's id waits for the Codex
    // watcher even though the router decoded its hook.
    let child_end_unclaims = ChildEndUnclaims::new();
    cc_src.child_end_unclaims = Some(child_end_unclaims.clone());
    codex_src.child_end_unclaims = Some(child_end_unclaims.clone());

    // grok consumes too: its subagent_stop/subagent_end hooks decode to
    // Hook-transport `SessionEnd{as_child:true}` (the tee's trigger), and the
    // grok watcher releases the ended child's flat-sibling transcript claim.
    let mut grok_src = GrokSource::default_paths();
    grok_src.child_end_unclaims = Some(child_end_unclaims.clone());

    // The HookRouter owns the ONE shared hook socket every source's hooks ride;
    // it is the tee producer + the daemon-presence demux. CC/Codex are now pure
    // transcript watchers (consumers of the un-claim handle).
    let hook_router = HookRouter::new(socket_path)
        .with_child_end_unclaims(Some(child_end_unclaims))
        .with_presence_tx(presence_tx);

    vec![
        Box::new(hook_router) as Box<dyn DynSource>,
        Box::new(cc_src),
        Box::new(ag_src),
        Box::new(codex_src),
        Box::new(copilot_src),
        Box::new(omp_src),
        Box::new(grok_src),
    ]
}

/// The source id an event would register/refresh — so the Connection gate can
/// drop a disconnected source's events BEFORE they reach the reducer. The source
/// is NOT recoverable from an `AgentId` (it's a hash), so read it off the two
/// variants that carry it (a hook `Identity` is emitted ahead of every activity
/// event since #221, so a fresh hook session's first event self-identifies);
/// otherwise fall back to the existing slot's source. `None` (an identity-less
/// event for an unknown id) slips the gate once and is evicted on the next tick.
fn event_source<'a>(scene: &'a SceneState, ev: &'a AgentEvent) -> Option<&'a str> {
    match ev {
        AgentEvent::SessionStart { source, .. } | AgentEvent::Identity { source, .. }
            if !source.is_empty() =>
        {
            Some(source)
        }
        _ => scene.agents.get(&ev.agent_id()).map(|s| s.source.as_ref()),
    }
}

pub(crate) async fn reducer_task(
    mut rx: TaggedReceiver,
    scene_tx: watch::Sender<Arc<SceneState>>,
    floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    connected: ConnectedSources,
    mut presence_rx: tokio::sync::mpsc::UnboundedReceiver<PresenceMsg>,
    presence_exit_watch: Option<daemon::PresenceExitWatch>,
) {
    let mut reducer = Reducer::new();
    // Disabled once the presence channel closes (all senders dropped) so its
    // `recv() -> None` branch can't busy-loop the select.
    let mut presence_open = true;
    let initial_caps: [usize; MAX_FLOORS] =
        std::array::from_fn(|i| floor_caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(initial_caps);
    // 1-Hz tick so exit-grace sweeps run even when no new events arrive.
    const SWEEP_TICK_INTERVAL_SECS: u64 = 1;
    let mut sweep_interval = tokio::time::interval(Duration::from_secs(SWEEP_TICK_INTERVAL_SECS));
    sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Sync per-floor capacities from the shared atomics so the
        // auto-computed layout capacity propagates to next_free_desk().
        for (i, a) in floor_caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
        }
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                let now = SystemTime::now();
                // Connection gate: drop a disconnected source's events before
                // they register/refresh a sprite. No scene change → no send.
                if event_source(&scene, &ev).is_some_and(|src| !connected.is_connected(src)) {
                    continue;
                }
                tracing::debug!(?transport, ?ev, "event");
                reducer.apply(&mut scene, ev, now, transport);
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
            // Daemon-presence deltas — source-tagged `(source, delta)` (hook-derived
            // + `(source, PidExited)` from the shared exit watch) — merged into
            // SceneState::daemons, NEVER through Reducer::apply (which is
            // AgentId-pure). Invariant #2. N daemons route by the tuple's source.
            update = presence_rx.recv(), if presence_open => {
                match update {
                    Some(PresenceMsg {
                        source,
                        delta: update,
                    }) => {
                        let now = SystemTime::now();
                        // CONNECTION GATE (mirrors the AgentEvent arm above): a
                        // daemon DISCONNECTED in the Sources panel has its presence
                        // DROPPED — don't arm the exit watch, don't apply. Any
                        // lingering entry is walked out by the sweep-tick reconcile.
                        if connected.is_connected(&source) {
                            // Arm the instant abrupt-down watch on the gateway pid —
                            // from GatewayUp (gateway_start) OR PidSeen (#318: a
                            // mid-attach / reconnect that never saw gateway_start, so
                            // the pid rides a later event). `watch` is idempotent per
                            // pid; `apply_presence` owns the None-only adoption.
                            if let Some(ew) = presence_exit_watch.as_ref() {
                                if let Some(pid) = update.armable_pid() {
                                    ew.watch(&source, pid);
                                }
                            }
                            daemon::apply_presence(&mut scene, &source, update, now);
                            if scene_tx.send(Arc::new(scene.clone())).is_err() {
                                tracing::warn!("scene channel closed — renderer dropped");
                                break;
                            }
                        }
                    }
                    None => presence_open = false,
                }
            }
            _ = sweep_interval.tick() => {
                let now = SystemTime::now();
                // Reconcile the scene toward the connected-set: walk out (idempotently)
                // every sprite whose source is NOT connected. Stateless on purpose —
                // no prev-set bookkeeping — and keyed on the COMPLEMENT of the set
                // (not a registered-source list), so it ALSO evicts a blank-source
                // slot synthesized for an identity-less event that slipped the
                // per-event gate, closing that hole within one tick.
                let cur = connected.snapshot();
                reducer.reconcile_connected(&mut scene, &cur, now);
                reducer.tick(&mut scene, now);
                // Per-daemon presence reconcile + decay (registry-DRIVEN, N daemons):
                // a panel-disconnected daemon walks its mascot out (Down → walk-out →
                // DOWN_REMOVE removal — the presence side-channel is separate from the
                // AgentEvent gate), and every daemon decays busy→idle / up→down on
                // silence per its own TTL. A 2nd daemon needs no edit here.
                for (source, ttl) in registry::daemon_sources() {
                    if !connected.is_connected(source) {
                        daemon::mark_presence_down(&mut scene, source, now);
                    }
                    daemon::sweep_presence_ttl(&mut scene, source, ttl, now);
                }
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
        }
    }
}

async fn headless_loop(
    scene_rx: SceneRx,
    health_rx: tokio::sync::watch::Receiver<Vec<pixtuoid_core::source::manager::SourceDeath>>,
) -> Result<()> {
    // ONE SIGINT listener for the loop's lifetime. A fresh `ctrl_c()` per
    // select! iteration drops the old listener while the sleep arm runs, and
    // tokio's process-global handler (installed once) suppresses default
    // termination — so a SIGINT landing in that gap notifies zero listeners
    // and is silently lost (the user must Ctrl-C twice). Created once here,
    // the subscription is continuous; the future is boxed so the loop can
    // disarm a registration FAILURE (a resolved future must never be polled
    // again). The signal future is INJECTED into the loop body so the
    // registration-failure arm is testable in-process (the real ctrl_c()
    // registers an unfakeable process-global handler).
    headless_loop_with_signal(scene_rx, health_rx, Box::pin(tokio::signal::ctrl_c())).await
}

async fn headless_loop_with_signal(
    mut scene_rx: SceneRx,
    mut health_rx: tokio::sync::watch::Receiver<Vec<pixtuoid_core::source::manager::SourceDeath>>,
    mut ctrl_c: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>,
) -> Result<()> {
    tracing::info!("pixtuoid headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    // Headless has no TUI footer (#157's sink for source deaths) and no
    // stderr subscriber guarantee — surface them in the summary stream, or a
    // dead transport reads as a silently empty office. Tracked by count: the
    // watch Vec only grows.
    let mut deaths_seen = 0usize;
    // How often the headless driver re-samples the scene watch and prints a diff.
    const HEADLESS_SUMMARY_POLL_INTERVAL_MS: u64 = 200;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(HEADLESS_SUMMARY_POLL_INTERVAL_MS)) => {
                let snapshot = scene_rx.borrow_and_update().clone();
                let summary = summarize(&snapshot);
                if summary != prev_summary {
                    println!("{summary}");
                    prev_summary = summary;
                }
            }
            Ok(()) = health_rx.changed() => {
                let deaths = health_rx.borrow_and_update().clone();
                for d in deaths.iter().skip(deaths_seen) {
                    println!("{}", super::format_source_death(d));
                }
                deaths_seen = deaths.len();
            }
            res = &mut ctrl_c => match res {
                Ok(()) => {
                    tracing::info!("shutting down");
                    return Ok(());
                }
                Err(e) => {
                    // A failed handler registration resolves Err on the FIRST
                    // poll. A wildcard match here exited headless mode
                    // instantly — silently, status 0 (the #157 class), on the
                    // exact CI/container surface where registration can be
                    // denied. Disarm the arm and keep serving: the default
                    // SIGINT disposition was never replaced, so Ctrl-C still
                    // terminates the process.
                    tracing::error!(
                        %e,
                        "Ctrl-C handler registration failed — headless loop \
                         continues; SIGINT falls back to the default disposition"
                    );
                    ctrl_c = Box::pin(std::future::pending());
                }
            }
        }
    }
}

fn compute_boot_capacities() -> [usize; MAX_FLOORS] {
    match crossterm::terminal::size().ok() {
        Some((cols, rows)) => boot_capacities_for(cols, rows),
        None => [FALLBACK_DESKS; MAX_FLOORS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::source::manager::SourceDeath;

    type HealthPair = (
        watch::Sender<Vec<SourceDeath>>,
        watch::Receiver<Vec<SourceDeath>>,
    );

    fn channels() -> (watch::Sender<Arc<SceneState>>, SceneRx, HealthPair) {
        let (scene_tx, scene_rx) =
            watch::channel(Arc::new(SceneState::new([FALLBACK_DESKS; MAX_FLOORS])));
        (scene_tx, scene_rx, watch::channel(Vec::new()))
    }

    // The documented "a source registered in the core registry but NOT wired
    // into run_async passes every conformance/manifest test yet never spawns"
    // gap (core/CLAUDE.md) — closed. `build_source_set` constructs the shared-
    // socket `HookRouter` PLUS every transcript-bearing registered source
    // (`line_decoder().is_some()`); it reads names off the real boxes, so it
    // can't drift from a hand-maintained second list. Hook-only sources + the
    // daemon (OpenClaw) are absent by design (they ride the router's socket).
    #[test]
    fn build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router() {
        use pixtuoid_core::source::{registry::descriptor_for, REGISTERED_SOURCES};
        use std::collections::BTreeSet;

        let sources = build_source_set(PathBuf::from("/tmp/pixtuoid-test.sock"), None, None, None);
        let built: BTreeSet<&str> = sources.iter().map(|s| s.name()).collect();

        // The HookRouter (infrastructure — owns the shared socket, NOT a
        // registered CLI) must be in the set so its fatal-bind death surfaces via
        // `spawn_with_health` (#157); it has no descriptor, so it's excluded from
        // the transcript-coverage check below.
        assert!(
            built.contains("hook-router"),
            "the shared-socket HookRouter must be spawned (else hook signals never decode)"
        );

        let transcript_built: BTreeSet<&str> = built
            .iter()
            .copied()
            .filter(|&n| n != "hook-router")
            .collect();
        let expected: BTreeSet<&str> = REGISTERED_SOURCES
            .iter()
            .copied()
            .filter(|&name| descriptor_for(name).is_some_and(|d| d.line_decoder().is_some()))
            .collect();
        assert_eq!(
            transcript_built, expected,
            "run_async's transcript-source wiring diverged from the registry: a \
             transcript-bearing source is registered but not built (it would never \
             spawn), or a built source isn't registered"
        );
    }

    // The Connection-gate seam: `event_source` decides which source an incoming
    // event belongs to so reducer_task can drop a disconnected source's events.
    // Carrying variants (SessionStart/Identity) self-identify; everything else
    // resolves via the existing slot; an unknown id with no carried source slips
    // the gate once (None) and is swept by the per-tick reconciler.
    #[test]
    fn event_source_extracts_source_for_the_connection_gate() {
        use pixtuoid_core::AgentId;
        let now = SystemTime::now();
        let mut scene = SceneState::new([FALLBACK_DESKS; MAX_FLOORS]);
        let mut reducer = Reducer::new();
        let id = AgentId::from_transcript_path("/p/a.jsonl");

        // SessionStart carries the source directly — even before the slot exists.
        let ss = AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        };
        assert_eq!(event_source(&scene, &ss), Some("claude-code"));

        // Identity likewise self-identifies.
        let idy = AgentEvent::Identity {
            agent_id: id,
            source: "codex".into(),
            session_id: "s".into(),
            cwd: None,
            pid: None,
        };
        assert_eq!(event_source(&scene, &idy), Some("codex"));

        // A non-carrying event for an UNKNOWN id slips the gate (None) — the
        // reconciler is the safety net.
        let act = AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: None,
        };
        assert_eq!(event_source(&scene, &act), None);

        // Once registered, the same event resolves via the slot's source.
        reducer.apply(&mut scene, ss, now, Transport::Jsonl);
        assert_eq!(event_source(&scene, &act), Some("claude-code"));

        // An EMPTY source on a carrying variant falls through to the slot.
        let empty = AgentEvent::Identity {
            agent_id: id,
            source: String::new(),
            session_id: "s".into(),
            cwd: None,
            pid: None,
        };
        assert_eq!(event_source(&scene, &empty), Some("claude-code"));
    }

    #[tokio::test(start_paused = true)]
    async fn headless_loop_shuts_down_on_a_delivered_signal() {
        let (_scene_tx, scene_rx, (_health_tx, health_rx)) = channels();
        headless_loop_with_signal(scene_rx, health_rx, Box::pin(async { Ok(()) }))
            .await
            .expect("a delivered Ctrl-C is a clean shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn headless_loop_keeps_serving_after_a_failed_signal_registration() {
        // A failed ctrl_c registration resolves Err on the first poll. The old
        // wildcard arm (`_ = &mut ctrl_c`) matched it and returned Ok(()) —
        // headless mode exited instantly, silently, status 0. The Err arm must
        // disarm itself instead: the loop keeps serving the scene/health arms,
        // so the timeout elapses (paused-clock time, so this is instant).
        let (_scene_tx, scene_rx, (_health_tx, health_rx)) = channels();
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            headless_loop_with_signal(
                scene_rx,
                health_rx,
                Box::pin(async { Err(std::io::Error::other("sigaction denied")) }),
            ),
        )
        .await;
        assert!(
            res.is_err(),
            "the loop must still be running after a failed signal registration, got {res:?}"
        );
    }
}
