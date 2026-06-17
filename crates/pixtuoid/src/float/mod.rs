//! `pixtuoid float` — the frameless, always-on-top desktop window that renders the
//! live office (every agent across every connected CLI) without opening the TUI.
//!
//! A binary-only front-end on the shared engine: it runs the SAME
//! `source → reducer → SceneState` pipeline the TUI uses, but presents each frame as a
//! full-resolution [`crate::tui::offscreen::OfficeRenderer`] `RgbBuffer` blitted into a
//! `winit` + `softbuffer` window instead of half-block terminal cells. `pixtuoid-core`
//! stays window-free (invariant #1) — all windowing lives here.

use anyhow::Result;

use crate::runtime::RunConfig;

/// Open the float window and drive it until the user closes it.
///
/// Stub: the window + live pipeline land in the next tasks. Takes the resolved
/// [`RunConfig`] (theme/pack/sources/socket — same as the TUI run) so the window can
/// reuse the engine wiring verbatim.
pub fn run(_cfg: RunConfig) -> Result<()> {
    Ok(())
}
