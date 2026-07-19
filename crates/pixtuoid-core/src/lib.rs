//! pixtuoid-core: headless logic for the pixtuoid TUI.

// Invariant #1: this crate is headless and must never write to a terminal.
// `just arch` greps the dep tree (ratatui/crossterm), but a raw `println!`
// pulls no dep and slips past it — this clippy restriction lint closes that
// gap (a hard error under the workspace `-D warnings`). Non-test builds only,
// so test diagnostics may print freely.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]

pub mod grid;
pub mod id;
pub mod platform;
pub mod source;
pub mod sprite;
pub mod state;
// Coherence-bound residue of the sim-geometry move to `pixtuoid-scene`:
// `WalkableMask` is an ALIAS for `Grid<bool>` whose obstacle ops are an
// inherent `impl Grid<bool>`, and the orphan rule pins that impl to the
// crate that owns `Grid` — so the mask vocabulary stays here even though
// its producers (layout) and consumers (pathfind/pose) live in the scene crate.
pub mod walkable;

pub use grid::Grid;
pub use id::AgentId;
pub use source::{AgentEvent, ToolDetail, Transport};
// The `Source` trait + its tagged tokio channel are the async transport seam —
// native-only (they don't exist in a `--no-default-features` wasm build).
#[cfg(feature = "native")]
pub use source::{Source, TaggedReceiver, TaggedSender};
pub use sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer, Sprite};
pub use state::reducer::Reducer;
pub use state::{
    ActivityState, AgentSlot, FloorLocalDeskIndex, GlobalDeskIndex, LabelProvenance, SceneState,
    SlotLabel, ToolKind,
};
pub use walkable::{OccupancyOverlay, WalkableMask};

/// Test-only tracing capture (`CaptureWriter` + `capture_logs`/`capture_warns`)
/// shared by the unit-test mods that assert on log breadcrumbs.
#[cfg(test)]
pub(crate) mod test_capture;

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`CLAUDE_CONFIG_DIR` / `PIXTUOID_SOCKET` / …). The crate's unit
/// tests share one test binary, so two env-mutating tests can otherwise race
/// under plain `cargo test` (nextest isolates per-process, but the `justfile`
/// falls back to `cargo test` when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
