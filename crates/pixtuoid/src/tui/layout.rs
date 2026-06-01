//! Thin façade over `pixtuoid_core::layout`. The binary re-exports the
//! core types under their familiar names so existing renderer code keeps
//! working unchanged; the core module is what owns the actual layout
//! computation, walkability mask, and primitive geometry.

pub use pixtuoid_core::layout::{
    anchored_top_left, desk_furniture_def, desk_walk_anchor, furniture_def, z_sort_row, Anchor,
    Bounds, Facing, Furniture, FurnitureDef, PlantKind, PodDecor, Point, SceneLayout, WallDecor,
    Waypoint, WaypointKind, DESK_GAP_X, DESK_GAP_Y, DESK_H, DESK_W, MAX_VISIBLE_DESKS,
    MIN_TOP_MARGIN, OBSTACLE_PAD_PX,
};

/// Backwards-compat alias — existing call sites construct `Layout::compute()`.
pub type Layout = SceneLayout;

/// Convert a core `Bounds` to a ratatui `Rect` for widget rendering. Same
/// shape, just a different type to keep the core crate free of ratatui.
pub fn bounds_to_rect(b: Bounds) -> ratatui::layout::Rect {
    ratatui::layout::Rect {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
    }
}
