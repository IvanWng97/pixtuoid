//! The async runtime glue: builds the tokio runtime, spawns the reducer
//! task + sources, binds the hook socket, and drives either the TUI or the
//! headless summary loop until Ctrl-C.
//!
//! Split out of `runtime/mod.rs` so this file — structurally unreachable by
//! any headless test (real tokio runtime + `block_on` + `ctrl_c` + socket
//! bind; see issue #103) — can be excluded from coverage on its own, while
//! the tested helpers (`RunConfig`, capacity math, `summarize`) keep full
//! coverage accounting in the parent module.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use pixtuoid_core::source::antigravity::AntigravitySource;
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::codex::CodexSource;
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
use tokio::sync::{mpsc, watch};

use super::{
    boot_capacities_for, cap_boot_capacities, summarize, RunConfig, SceneRx, FALLBACK_DESKS,
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
    } = cfg;
    let mut cc_src = ClaudeCodeSource::default_paths();
    if let Some(s) = socket {
        cc_src.socket_path = s;
    }
    if let Some(p) = projects_root {
        cc_src.projects_root = p;
    }

    let ag_src = AntigravitySource::default_paths();

    let mut codex_src = CodexSource::default_paths();
    if let Some(p) = codex_sessions_root {
        codex_src.sessions_root = p;
    }

    // No ReasonixSource here: Reasonix is HOOK-ONLY (no watchable JSONL — see
    // source/reasonix.rs). Its hook payloads ride the shared hook socket that
    // ClaudeCodeSource binds, attributed per-payload by `_pixtuoid_source`.
    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
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

    tokio::spawn(reducer_task(rx, scene_tx, Arc::clone(&floor_caps)));

    // Source-health side channel (#157): a fatal source exit must reach the
    // TUI footer — in default TUI mode tracing only reaches the log file, and
    // the office silently freezing is the worst failure class. Deliberately
    // NOT an AgentEvent: the one event channel carries agent activity (its
    // Transport tag drives hook-wins dedup), not source lifecycle.
    let (health_tx, health_rx) = tokio::sync::watch::channel(Vec::new());
    let _source_handles = SourceManager::new()
        .with_source(Box::new(cc_src))
        .with_source(Box::new(ag_src))
        .with_source(Box::new(codex_src))
        .spawn_with_health(tx, health_tx);

    if headless {
        headless_loop(scene_rx).await
    } else {
        crate::tui::run_tui(
            scene_rx,
            pack_dir,
            floor_caps,
            theme,
            config_path,
            desk_cap,
            pets,
            health_rx,
        )
        .await
    }
}

async fn reducer_task(
    mut rx: TaggedReceiver,
    scene_tx: watch::Sender<Arc<SceneState>>,
    floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
) {
    let mut reducer = Reducer::new();
    let initial_caps: [usize; MAX_FLOORS] =
        std::array::from_fn(|i| floor_caps[i].load(Ordering::Relaxed));
    let mut scene = SceneState::new(initial_caps);
    // 1-Hz tick so exit-grace sweeps run even when no new events arrive.
    let mut sweep_interval = tokio::time::interval(Duration::from_secs(1));
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
                tracing::debug!(?transport, ?ev, "event");
                reducer.apply(&mut scene, ev, now, transport);
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
            _ = sweep_interval.tick() => {
                reducer.tick(&mut scene, SystemTime::now());
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
        }
    }
}

async fn headless_loop(mut scene_rx: SceneRx) -> Result<()> {
    tracing::info!("pixtuoid headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let snapshot = scene_rx.borrow_and_update().clone();
                let summary = summarize(&snapshot);
                if summary != prev_summary {
                    println!("{summary}");
                    prev_summary = summary;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
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
