//! `pixtuoid floating` — the frameless, always-on-top desktop window that renders the
//! live office (every agent across every connected CLI) without opening the TUI.
//!
//! A binary-only front-end on the shared engine: it runs the SAME
//! `source → reducer → SceneState` pipeline the TUI uses (reusing
//! `runtime::driver::build_source_set` — the ONE source-construction site — and
//! `reducer_task`), but presents each frame as a full-resolution
//! [`offscreen::OfficeRenderer`] `RgbBuffer` blitted into a `winit` +
//! `softbuffer` window instead of half-block terminal cells. `pixtuoid-core` stays
//! window-free (invariant #1) — all windowing lives here.

mod geometry;
pub mod offscreen;
mod window;

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use anyhow::{Context, Result};
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use pixtuoid_core::source::daemon;
use pixtuoid_core::source::manager::SourceManager;
use pixtuoid_core::state::{SceneState, MAX_FLOORS};
use pixtuoid_core::{AgentEvent, Transport};
use tokio::sync::{mpsc, watch};
use winit::event_loop::EventLoop;

use crate::config;
use crate::runtime::driver::{build_source_set, reducer_task};
use crate::runtime::{boot_capacities_for, ConnectedSources, RunConfig};
use window::{FloatingApp, FloatingEvent};

/// Open the floating window and drive it until the user closes it.
///
/// `winit`'s event loop must own the main thread, so the source pipeline runs on a
/// background tokio runtime (spawned, NEVER `block_on` — that would stall the window),
/// and scene changes reach the loop via an `EventLoopProxy`. BLOCKS until the window
/// closes; the runtime + source handles are held alive across the call.
pub fn run(cfg: RunConfig) -> Result<()> {
    let RunConfig {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
        theme,
        pets,
        connected,
        config_path,
        ..
    } = cfg;

    let app_config = config::load(&config_path, &mut Vec::new());
    let floating_cfg = config::resolve_floating(&app_config);
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(pack_dir)
        .context("loading the sprite pack for the floating window")?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the floating tokio runtime")?;
    // Enter the runtime on the main thread so the source set's internal `tokio::spawn`s
    // (presence watch, source manager) have a runtime context. We never `block_on` here.
    let _guard = rt.enter();

    // --- the live pipeline (mirrors runtime::driver::run_async; build_source_set is the
    //     shared ONE source-construction site, reused not duplicated) ---
    let connected = ConnectedSources::new(connected);
    let socket_path = socket.unwrap_or_else(ClaudeCodeSource::default_socket_path);
    let (presence_tx, presence_rx) = mpsc::unbounded_channel();
    let presence_exit_watch = daemon::spawn_presence_exit_watch(presence_tx.clone());
    let sources = build_source_set(
        socket_path,
        projects_root,
        codex_sessions_root,
        Some(presence_tx),
    );
    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    // Boot capacity from the WINDOW, not crossterm: the office canvas is the window
    // pixels (buf_w = cols, buf_h = rows*2), so cols = width, rows = height/2. Refined to
    // the exact layout on the first redraw (window::sync_floor_caps).
    let boot_caps =
        boot_capacities_for(floating_cfg.width as u16, (floating_cfg.height / 2) as u16);
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));
    let floor_caps: Arc<[AtomicUsize; MAX_FLOORS]> =
        Arc::new(std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])));
    rt.spawn(reducer_task(
        rx,
        scene_tx,
        Arc::clone(&floor_caps),
        connected.clone(),
        presence_rx,
        presence_exit_watch,
    ));
    let (health_tx, health_rx) = watch::channel(Vec::new());
    let mut manager = SourceManager::new();
    for src in sources {
        manager = manager.with_source(src);
    }
    // Held until `run` returns — dropping the handles would drop the source tasks.
    let _source_handles = manager.spawn_with_health(tx, health_tx);

    // --- the window event loop (main thread) ---
    let mut builder = EventLoop::<FloatingEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // Accessory: no Dock icon, doesn't steal focus — an ambient companion.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder
        .build()
        .context("building the floating event loop")?;
    let proxy = event_loop.create_proxy();

    // Bridge: a new scene → a repaint. Breaks cleanly when the window closes
    // (`send_event` → `EventLoopClosed`) or the reducer drops its sender — never unwraps.
    {
        let mut scene_rx = scene_rx.clone();
        rt.spawn(async move {
            while scene_rx.changed().await.is_ok() {
                if proxy.send_event(FloatingEvent::SceneChanged).is_err() {
                    break;
                }
            }
        });
    }
    // Source deaths have no footer in floating — log them (the office partially freezes).
    {
        let mut health_rx = health_rx;
        rt.spawn(async move {
            while health_rx.changed().await.is_ok() {
                for death in health_rx.borrow().iter() {
                    tracing::warn!("pixtuoid floating: source exited: {death:?}");
                }
            }
        });
    }

    let mut app = FloatingApp::new(
        floating_cfg,
        theme,
        pack,
        config_path,
        pets,
        scene_rx,
        floor_caps,
    );
    event_loop
        .run_app(&mut app)
        .context("running the floating window event loop")?;
    Ok(())
}
