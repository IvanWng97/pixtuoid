//! `pixtuoid floating` — the frameless, always-on-top desktop window that renders the
//! live office (every agent across every connected CLI) without opening the TUI.
//!
//! A binary-only front-end on the shared engine: it boots the SAME
//! `runtime::pipeline::spawn_pipeline` spine the TUI uses — from
//! `window::resumed` rather than [`run`], because the desk-capacity seed needs
//! the REAL window size (see `PipelineBoot`) — but presents each frame as a
//! full-resolution [`offscreen::OfficeRenderer`] `RgbBuffer` blitted into a
//! `winit` + `softbuffer` window instead of half-block terminal cells.
//! `pixtuoid-core` stays window-free (invariant #1) — all windowing lives here.

mod cadence;
mod geometry;
mod input;
pub mod offscreen;
mod window;

use anyhow::{Context, Result};
use pixtuoid_core::source::claude_code::ClaudeCodeSource;
use winit::event_loop::EventLoop;

use crate::config;
use crate::runtime::{ConnectedSources, RunConfig};
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
        audio,
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
    // The pipeline boots in `resumed` (`PipelineBoot::spawn`), which enters the
    // runtime explicitly — nothing left on this path spawns, so `run` holds no
    // `rt.enter()` guard.
    let connected = ConnectedSources::new(connected);
    let socket_path = socket.unwrap_or_else(ClaudeCodeSource::default_socket_path);

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

    // FloatingApp OWNS the audio device thread via its AudioController. Constructed
    // HERE, after every fallible `?` boot step, so a boot failure means no thread
    // ever existed — and once `app` exists, its Drop joins the device thread on
    // EVERY exit (run_app returning normally OR a window-creation failure), with no
    // manual shutdown call.
    let mut app = FloatingApp::new(
        floating_cfg,
        theme,
        pack,
        config_path,
        pets,
        PipelineBoot {
            socket_path,
            projects_root,
            codex_sessions_root,
            connected,
            proxy,
            rt: rt.handle().clone(),
        },
        audio.muted,
        audio.volume,
    );
    event_loop
        .run_app(&mut app)
        .context("running the floating window event loop")
}

/// Everything the pipeline needs, held by [`FloatingApp`] until `resumed` can
/// supply the one input `run` cannot: the REAL window size.
///
/// `run` genuinely cannot do better: winit 0.30 exposes `primary_monitor` only
/// on `ActiveEventLoop`, which does not exist until `run_app` is already
/// driving, and `office_scale` ROUNDS, so no conservative logical-side seed is
/// sound either — the buffer for one logical size is NOT monotone in the scale
/// factor.
///
/// The accepted cost: the hook socket binds AFTER window + surface creation
/// rather than before the event loop. A hook landing inside that window fails to
/// connect and exits 0 silently — the shim's documented never-block contract. In
/// exchange, a window-creation failure no longer leaves a bound socket and a
/// live source set behind.
pub(crate) struct PipelineBoot {
    socket_path: std::path::PathBuf,
    projects_root: Option<std::path::PathBuf>,
    codex_sessions_root: Option<std::path::PathBuf>,
    connected: ConnectedSources,
    proxy: winit::event_loop::EventLoopProxy<FloatingEvent>,
    /// A cheap handle so `resumed` can `enter()` the runtime explicitly instead
    /// of leaning on `run`'s ambient guard surviving across `run_app` (it does —
    /// but a `tokio::spawn` with no runtime PANICS).
    rt: tokio::runtime::Handle,
}

/// The pipeline handles, available only once [`PipelineBoot::spawn`] has run.
pub(crate) struct LivePipeline {
    pub(crate) scene_rx: tokio::sync::watch::Receiver<std::sync::Arc<pixtuoid_core::SceneState>>,
    pub(crate) floor_caps:
        std::sync::Arc<[std::sync::atomic::AtomicUsize; pixtuoid_core::state::MAX_FLOORS]>,
    /// Inert anchor — the tasks are kept alive by the RUNTIME, not by these
    /// handles (dropping a tokio `JoinHandle` detaches). See `Pipeline`'s doc.
    _source_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl PipelineBoot {
    /// Boot the source pipeline seeded from the REAL window size, and wire the
    /// two background consumers that need the event-loop proxy.
    pub(crate) fn spawn(self, window_size: winit::dpi::PhysicalSize<u32>) -> LivePipeline {
        let _guard = self.rt.enter(); // spawn_pipeline's internal spawns need it
        let boot_caps = offscreen::boot_capacities_for_window(window_size);
        tracing::debug!(
            ?window_size,
            floor0_desks = boot_caps[0],
            "pixtuoid floating: seeding desk capacity from the real window"
        );
        let crate::runtime::pipeline::Pipeline {
            scene_rx,
            health_rx,
            floor_caps,
            _source_handles,
        } = crate::runtime::pipeline::spawn_pipeline(
            self.socket_path,
            self.projects_root,
            self.codex_sessions_root,
            self.connected,
            boot_caps,
        );

        {
            let mut scene_rx = scene_rx.clone();
            let proxy = self.proxy;
            self.rt.spawn(async move {
                while scene_rx.changed().await.is_ok() {
                    if proxy.send_event(FloatingEvent::SceneChanged).is_err() {
                        break;
                    }
                }
            });
        }
        // Deduped by count: the watch value is a grow-only Vec, so logging the whole
        // borrow on every change would re-warn all prior deaths.
        {
            let mut health_rx = health_rx;
            self.rt.spawn(async move {
                let mut deaths_seen = 0usize;
                while health_rx.changed().await.is_ok() {
                    let deaths = health_rx.borrow_and_update().clone();
                    for death in crate::runtime::unseen_deaths(&deaths, &mut deaths_seen) {
                        tracing::warn!(
                            source = %death.source,
                            error = %death.error,
                            "pixtuoid floating: source exited"
                        );
                    }
                }
            });
        }

        LivePipeline {
            scene_rx,
            floor_caps,
            _source_handles,
        }
    }
}
