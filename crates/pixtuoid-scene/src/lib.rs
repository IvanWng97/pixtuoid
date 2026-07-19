//! Backend-agnostic render + simulation engine shared by every front-end.
//!
//! `scene` owns the office world: layout, pose/motion/pathfinding, the pixel
//! painter (`render_to_rgb_buffer` — the shared world render), themes, pets,
//! chitchat, and the embedded sprite pack. It has **no** terminal or window
//! dependency — `tui` (ratatui half-block) and `floating` (winit/softbuffer)
//! are thin painters layered on top, and neither depends on the other.

// Terminal- and window-free (invariant #1, crate-boundary enforced). The dep
// boundary can't see a raw `println!` (std, no dep); this restriction lint does
// (a hard error under the workspace `-D warnings`). Non-test builds only, so
// test diagnostics may print freely.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]
// The engine crate has ZERO unsafe — lock that in so a future "just this once"
// block can't slip in (raw-pixel hot paths belong behind core's checked
// Grid/RgbBuffer seams). Lives here, not in Cargo.toml: `[lints] workspace =
// true` cannot be combined with a per-crate `[lints.rust]` table.
#![forbid(unsafe_code)]

// Easing curves for the binary's floor-slide/popup animations — in-workspace
// painter plumbing, not a stable engine API.
#[doc(hidden)]
pub mod anim;
#[doc(hidden)] // shared workspace model (the overlay/board pattern), not stable engine API
pub mod audio;
// The neon wall-board MODEL + shared scene-stats tally the three in-workspace
// painters consume — their shared single source of truth, not a stable engine API.
#[doc(hidden)]
pub mod board;
// Burn-tier interpretation (model/effort → hair color, dossier effort row)
// the in-workspace painters + the binary's tooltip consume — shared single
// source of truth, not a stable engine API.
#[doc(hidden)]
pub mod burn;
pub mod chitchat;
pub(crate) mod creatures;
pub mod embedded_pack;
pub mod floor;
// The status-footer MODEL the two in-workspace painters consume — their shared
// single source of truth (like board/overlay), not a stable engine API.
#[doc(hidden)]
pub mod footer;
// Per-agent recolored-sprite cache owned by each painter's `FloorCtx` — an
// in-workspace render internal, not a stable engine API.
#[doc(hidden)]
pub mod frame_cache;
pub mod layout;
pub mod motion;
// The name-badge label MODEL the two in-workspace painters consume — their
// shared single source of truth, not a stable engine API.
#[doc(hidden)]
pub mod overlay;
pub mod pathfind;
pub mod pet;
pub mod physics;
pub mod pixel_painter;
pub mod pose;
pub mod theme;
pub mod token_meter;

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`XDG_CONFIG_HOME`). The crate's unit tests share one test binary,
/// so two env-mutating tests can otherwise race under plain `cargo test`
/// (nextest isolates per-process, but the `justfile` falls back to `cargo test`
/// when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
