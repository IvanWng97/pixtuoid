//! `pixtuoid float` — the frameless, always-on-top desktop window that renders the
//! live office (every agent across every connected CLI) without opening the TUI.
//!
//! A binary-only front-end on the shared engine: it runs the SAME
//! `source → reducer → SceneState` pipeline the TUI uses, but presents each frame as a
//! full-resolution [`crate::tui::offscreen::OfficeRenderer`] `RgbBuffer` blitted into a
//! `winit` + `softbuffer` window instead of half-block terminal cells. `pixtuoid-core`
//! stays window-free (invariant #1) — all windowing lives here.

mod window;

use anyhow::{Context, Result};
use winit::event_loop::EventLoop;

use crate::config;
use crate::runtime::RunConfig;

/// Open the float window and drive it until the user closes it.
///
/// Takes the resolved [`RunConfig`] (theme/pack/sources/socket — same as the TUI run)
/// so the window reuses the engine wiring. The `[float]` geometry is re-resolved from
/// the on-disk config here (the warnings were already surfaced by `build_run_config`).
///
/// NOTE: `winit`'s event loop must own the main thread, so this BLOCKS until the window
/// closes. The live source pipeline (Task 5) is spawned onto a background runtime — it
/// must never `block_on` here.
pub fn run(cfg: RunConfig) -> Result<()> {
    let app_config = config::load(&cfg.config_path, &mut Vec::new());
    let float_cfg = config::resolve_float(&app_config);
    let pack = crate::tui::embedded_pack::load_sprite_pack(cfg.pack_dir.clone())
        .context("loading the sprite pack for the float window")?;
    let mut app = window::FloatApp::new(float_cfg, cfg.theme, pack);

    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    {
        // Accessory: no Dock icon, doesn't steal focus from the editor — the float
        // window is an ambient companion, not a foreground app.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().context("building the float event loop")?;
    event_loop
        .run_app(&mut app)
        .context("running the float window event loop")?;
    Ok(())
}
