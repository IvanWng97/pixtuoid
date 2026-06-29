//! pixtuoid-core: headless logic for the pixtuoid TUI.

// Invariant #1: this crate is headless and must never write to a terminal.
// `just arch` greps the dep tree (ratatui/crossterm), but a raw `println!`
// pulls no dep and slips past it — this clippy restriction lint closes that
// gap (a hard error under the workspace `-D warnings`). Non-test builds only,
// so test diagnostics may print freely.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]

pub mod grid;
pub mod id;
pub mod layout;
pub mod physics;
pub mod platform;
pub mod pose;
pub mod render;
pub mod source;
pub mod sprite;
pub mod state;
pub mod walkable;

pub use grid::Grid;
pub use id::AgentId;
pub use render::Renderer;
pub use source::{AgentEvent, Source, TaggedReceiver, TaggedSender, ToolDetail, Transport};
pub use sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer, Sprite};
pub use state::reducer::Reducer;
pub use state::{ActivityState, AgentSlot, FloorLocalDeskIndex, GlobalDeskIndex, SceneState};
pub use walkable::{OccupancyOverlay, WalkableMask};

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`CLAUDE_CONFIG_DIR` / `PIXTUOID_SOCKET` / …). The crate's unit
/// tests share one test binary, so two env-mutating tests can otherwise race
/// under plain `cargo test` (nextest isolates per-process, but the `justfile`
/// falls back to `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
