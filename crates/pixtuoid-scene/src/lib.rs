//! Backend-agnostic render + simulation engine shared by every front-end.
//!
//! It has **no** terminal or window dependency — `tui` (ratatui half-block) and
//! `floating` (winit/softbuffer) are thin painters layered on top.
//!
//! ```
//! use pixtuoid_scene::layout::SceneLayout;
//! use pixtuoid_scene::theme::theme_by_name;
//!
//! let theme = theme_by_name("dracula").expect("bundled theme");
//! assert_eq!(theme.name, "dracula");
//!
//! // `None` fills the buffer with as many desk pods as physically fit.
//! let office = SceneLayout::compute_with_seed(192, 64, None, 0)
//!     .expect("viewport is large enough for an office");
//! assert!(!office.home_desks.is_empty());
//! ```

// Terminal- and window-free (invariant #1): the dep boundary can't see a raw
// `println!` (std, no dep); this restriction lint can.
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]
// Lives here, not in Cargo.toml: `[lints] workspace = true` cannot be combined
// with a per-crate `[lints.rust]` table.
#![forbid(unsafe_code)]
// Scoped to this PUBLISHED crate, not `[workspace.lints]` — the binary crates'
// `pub` items aren't a semver surface.
#![warn(missing_docs)]

#[doc(hidden)]
pub mod anim;
#[doc(hidden)]
pub mod audio;
#[doc(hidden)]
pub mod board;
#[doc(hidden)]
pub mod burn;
pub mod chitchat;
pub(crate) mod creatures;
pub mod cutaway;
pub mod embedded_pack;
pub mod floor;
#[doc(hidden)]
pub mod footer;
#[doc(hidden)]
pub mod frame_cache;
pub mod layout;
pub mod motion;
#[doc(hidden)]
pub mod overlay;
pub mod pathfind;
/// Office pets — the `Pet`/`PetKind` model and per-floor selection.
pub mod pet;
pub mod physics;
pub mod pixel_painter;
pub mod pose;
pub mod render_scale;
/// The color-theme MODEL: the `Theme` role palette and the bundled themes.
pub mod theme;
pub mod token_meter;

/// Test-only mutex serializing tests that mutate process-global environment
/// variables (`XDG_CONFIG_HOME`) — the crate's unit tests share one binary under
/// plain `cargo test`, which the `justfile` falls back to when nextest is absent.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
