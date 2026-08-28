//! The async runtime glue: builds the tokio runtime, spawns the reducer
//! task + sources, binds the hook socket, and drives either the TUI or the
//! headless summary loop until Ctrl-C.
//!
//! This file is structurally unreachable by any headless test (real tokio
//! runtime + `block_on` + `ctrl_c` + socket bind), so it is coverage-excluded on
//! its own and must stay a pure shell: the connection-gate DECISION lives in the
//! sibling [`super::gate`] module, which IS covered and mutation-tested.

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
use pixtuoid_core::source::omp::OmpSource;
use pixtuoid_core::source::DynSource;
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{Reducer, SceneState, TaggedReceiver};
use tokio::sync::watch;

use super::gate;
use super::{
    boot_capacities_for, resolve_boot_caps, summarize, ConnectedSources, RunConfig, SceneRx,
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
    // Audio owns no state here: `run_tui` builds the AudioController, which owns
    // the device thread and tears it down on Drop at any exit.
    // Focus-jump pid point-query roots (cloned: build_source_set consumes the
    // originals).
    let focus_roots = (projects_root.clone(), codex_sessions_root.clone());
    let connected = ConnectedSources::new(connected);
    let socket_path = socket.unwrap_or_else(ClaudeCodeSource::default_socket_path);
    // The terminal-size query stays here in the shell (the injected `measure`);
    // the policy is the covered + mutation-tested `resolve_boot_caps`.
    let boot_caps = resolve_boot_caps(desk_cap, headless, compute_boot_capacities);
    // The shared spine — ONE authority with `floating::run`. The tasks live on
    // this fn's runtime; `_source_handles` is an inert anchor (see Pipeline's doc).
    let super::pipeline::Pipeline {
        scene_rx,
        health_rx,
        floor_caps,
        _source_handles,
    } = super::pipeline::spawn_pipeline(
        socket_path.clone(),
        projects_root,
        codex_sessions_root,
        connected.clone(),
        boot_caps,
    );

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
            audio_cfg: audio,
        })
        .await
    }
}

/// Build the runtime source set `run_async` spawns — the ONE place that set is
/// constructed. Each transcript source carries different typed config (CC's
/// projects root, Codex's sessions root), so this stays imperative rather than a
/// registry-driven loop. Hook-only sources + the daemon (OpenClaw) are absent by
/// design — they ride the router's shared socket.
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

    // The hook tee is the sole PRODUCER, and each watcher drains only ids whose
    // transcripts it claims (AgentId is source-namespaced), so a Codex child waits
    // for the Codex watcher even though the router decoded its hook.
    let child_end_unclaims = ChildEndUnclaims::new();
    cc_src.child_end_unclaims = Some(child_end_unclaims.clone());
    codex_src.child_end_unclaims = Some(child_end_unclaims.clone());

    // grok consumes too: its subagent_stop/end hooks decode to Hook-transport
    // `SessionEnd{as_child:true}`, the tee's trigger.
    let mut grok_src = GrokSource::default_paths();
    grok_src.child_end_unclaims = Some(child_end_unclaims.clone());

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

/// The reducer event loop: gate + apply incoming `AgentEvent`s, merge daemon
/// presence, and run the 1-Hz reconcile sweep — an async shell over
/// [`super::gate`], which owns every decision.
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
    // `&'static str`, NOT the raw wire name: `_pixtuoid_source` arrives verbatim
    // from socket JSON with no registry check and no length cap, so keying on it
    // lets a long-lived `run` accumulate an entry per distinct name seen.
    let mut gate_logged: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
    let initial_caps: [usize; MAX_FLOORS] =
        std::array::from_fn(|i| floor_caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(initial_caps);
    // Ticks so exit-grace sweeps run even when no new events arrive.
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
                // A gated event mutates nothing, so publish only when it applied.
                if gate::apply_gated_event(
                    &mut reducer,
                    &mut scene,
                    ev,
                    transport,
                    &connected,
                    SystemTime::now(),
                    &mut gate_logged,
                ) && scene_tx.send(Arc::new(scene.clone())).is_err()
                {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
            // Merged into SceneState::daemons, NEVER through the AgentId-pure
            // `Reducer::apply`. Routed by `DaemonInstanceKey`, so N instances of one
            // daemon need no per-source special-casing.
            update = presence_rx.recv(), if presence_open => {
                match update {
                    Some(PresenceMsg { key, delta }) => {
                        if let gate::PresenceGate::Applied { arm_pid } = gate::apply_gated_presence(
                            &mut scene,
                            &key,
                            delta,
                            &connected,
                            SystemTime::now(),
                        ) {
                            // Arming AFTER apply_presence is safe: the two touch
                            // DISJOINT state (the exit-watch pid map vs
                            // scene.daemons), and a dead pid's synthesized PidExited
                            // re-enters only on a LATER select iteration.
                            if let (Some(ew), Some(pid)) = (presence_exit_watch.as_ref(), arm_pid) {
                                ew.watch(&key, pid);
                            }
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
                gate::reconcile_sweep_tick(&mut reducer, &mut scene, &connected, SystemTime::now());
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
    // ONE SIGINT listener for the loop's lifetime. A fresh `ctrl_c()` per select!
    // iteration would drop the old listener while the sleep arm runs, and tokio's
    // process-global handler suppresses default termination — so a SIGINT landing
    // in that gap notifies zero listeners and is silently lost. Boxed so the loop
    // can disarm a registration FAILURE (a resolved future must never be polled
    // again), and injected so that arm is testable in-process.
    headless_loop_with_signal(scene_rx, health_rx, Box::pin(tokio::signal::ctrl_c())).await
}

async fn headless_loop_with_signal(
    mut scene_rx: SceneRx,
    mut health_rx: tokio::sync::watch::Receiver<Vec<pixtuoid_core::source::manager::SourceDeath>>,
    mut ctrl_c: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>,
) -> Result<()> {
    tracing::info!("pixtuoid headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    // Headless has no TUI footer and no stderr subscriber guarantee, so source
    // deaths must surface in the summary stream or a dead transport reads as a
    // silently empty office. Tracked by count: the watch Vec only grows.
    let mut deaths_seen = 0usize;
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
                for d in super::unseen_deaths(&deaths, &mut deaths_seen) {
                    println!("{}", super::format_source_death(d));
                }
            }
            res = &mut ctrl_c => match res {
                Ok(()) => {
                    tracing::info!("shutting down");
                    return Ok(());
                }
                Err(e) => {
                    // A failed handler registration resolves Err on the FIRST poll,
                    // so a wildcard match here exits headless mode instantly and
                    // silently with status 0. Disarm and keep serving: the default
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

    #[test]
    fn build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router() {
        use pixtuoid_core::source::registry::{self, descriptor_for};
        use std::collections::BTreeSet;

        let sources = build_source_set(PathBuf::from("/tmp/pixtuoid-test.sock"), None, None, None);
        let built: BTreeSet<&str> = sources.iter().map(|s| s.name()).collect();

        // The HookRouter is infrastructure, not a registered CLI: it has no
        // descriptor, so it is excluded from the transcript check below.
        assert!(
            built.contains("hook-router"),
            "the shared-socket HookRouter must be spawned (else hook signals never decode)"
        );

        let transcript_built: BTreeSet<&str> = built
            .iter()
            .copied()
            .filter(|&n| n != "hook-router")
            .collect();
        let expected: BTreeSet<&str> = registry::registered_source_names()
            .filter(|&name| descriptor_for(name).is_some_and(|d| d.line_decoder().is_some()))
            .collect();
        assert_eq!(
            transcript_built, expected,
            "run_async's transcript-source wiring diverged from the registry: a \
             transcript-bearing source is registered but not built (it would never \
             spawn), or a built source isn't registered"
        );
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
        // Still-serving is proved by the timeout ELAPSING — on the paused clock
        // that is instant.
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
