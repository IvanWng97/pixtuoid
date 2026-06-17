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
