//! pixtuoid-core: headless logic for the pixtuoid TUI.
//!
//! The crate turns a stream of decoded agent-CLI events into an office
//! [`SceneState`]: [`Reducer::apply`] folds each [`AgentEvent`] (tagged with the
//! [`Transport`] it arrived on) into the per-agent slots a renderer then paints.
//! It has no terminal dependency — the pixel/terminal painters live crate-up in
//! `pixtuoid-scene` and the `pixtuoid` binary.
//!
//! ```
//! use std::path::PathBuf;
//! use std::time::SystemTime;
//!
//! use pixtuoid_core::{AgentEvent, AgentId, Reducer, SceneState, Transport};
//!
//! let mut scene = SceneState::uniform(4); // one floor, 4 desks
//! let mut reducer = Reducer::new();
//! let id = AgentId::from_parts("claude-code", "session-1");
//!
//! reducer.apply(
//!     &mut scene,
//!     AgentEvent::SessionStart {
//!         agent_id: id,
//!         source: "claude-code".into(),
//!         session_id: "session-1".into(),
//!         cwd: PathBuf::from("/repo"),
//!         parent_id: None,
//!     },
//!     SystemTime::now(),
//!     Transport::Hook,
//! );
//!
//! // The session now occupies a desk, labelled `<source-prefix>·<cwd-basename>`.
//! let slot = scene.agents.get(&id).expect("the session took a desk");
//! assert_eq!(&*slot.label, "cc·repo");
//! ```

// Invariant #1: this crate is headless and must never write to a terminal.
// `just arch` greps the dep tree (ratatui/crossterm), but a raw `println!`
// pulls no dep and slips past it — this clippy restriction lint closes that
// gap (a hard error under the workspace `-D warnings`). Non-test builds only,
// so test diagnostics may print freely.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]
// Public-API doc gate. Scoped to this PUBLISHED crate (not `[workspace.lints]`)
// because the binary crates' `pub` items aren't a semver surface — the same
// two-crate scope as the semver-checks + api-surface gates. `-D warnings`
// (`just clippy`) promotes it to a hard gate; `#[doc(hidden)] pub` is exempt.
#![warn(missing_docs)]

pub mod grid;
/// Agent identity: the `AgentId` session key and its path/parts derivations.
pub mod id;
pub mod platform;
/// The source/decoder seam — the `Source` trait, per-CLI transcript/hook
/// decoders, listeners, and the `SourceManager`.
pub mod source;
/// Sprite vocabulary: `Frame`/`Sprite`/`Palette`, the `RgbBuffer` blit target,
/// and the `.sprite`/`pack.toml` pack loader.
pub mod sprite;
/// The reducer and `SceneState` — the event coordinator turning `AgentEvent`s
/// into per-agent slot state.
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
