//! pixtuoid-core: headless logic for the pixtuoid TUI.

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

pub use id::AgentId;
pub use render::Renderer;
pub use source::{AgentEvent, Source, TaggedReceiver, TaggedSender, ToolDetail, Transport};
pub use sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer, Sprite};
pub use state::reducer::Reducer;
pub use state::{ActivityState, AgentSlot, SceneState};
pub use walkable::{OccupancyOverlay, WalkableMask};

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`CLAUDE_CONFIG_DIR` / `PIXTUOID_SOCKET` / …). The crate's unit
/// tests share one test binary, so two env-mutating tests can otherwise race
/// under plain `cargo test` (nextest isolates per-process, but the `justfile`
/// falls back to `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
