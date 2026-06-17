//! Backend-agnostic render + simulation engine shared by every front-end.
//!
//! `scene` owns the office world: layout, pose/motion/pathfinding, the pixel
//! painter (`render_to_rgb_buffer` — the shared world render), themes, pets,
//! chitchat, and the embedded sprite pack. It has **no** terminal or window
//! dependency — `tui` (ratatui half-block) and `floating` (winit/softbuffer)
//! are thin painters layered on top, and neither depends on the other.

pub mod anim;
pub mod chitchat;
pub mod embedded_pack;
pub mod floor;
pub mod font;
pub mod frame_cache;
pub mod layout;
pub mod motion;
pub mod overlay;
pub mod pathfind;
pub mod pet;
pub mod pixel_painter;
pub mod pose;
pub mod theme;

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`XDG_CONFIG_HOME`). The crate's unit tests share one test binary,
/// so two env-mutating tests can otherwise race under plain `cargo test`
/// (nextest isolates per-process, but the `justfile` falls back to `cargo test`
/// when nextest is absent). Lock it for the whole test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
