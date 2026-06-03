//! Thin façade over `pixtuoid_core::layout`. The binary re-exports the
//! core types under their familiar names so existing renderer code keeps
//! working unchanged; the core module is what owns the actual layout
//! computation, walkability mask, and primitive geometry.

pub use pixtuoid_core::layout::{
    anchored_top_left, desk_furniture_def, desk_walk_anchor, furniture_def, z_sort_row, Anchor,
    Bounds, Facing, Furniture, FurnitureDef, PlantItem, PlantKind, PodDecor, PodDecorItem, Point,
    SceneLayout, Size, WallDecor, WallDecorItem, WallSegment, Waypoint, WaypointKind, DESK_GAP_X,
    DESK_GAP_Y, DESK_H, DESK_W, ELEVATOR_H, ELEVATOR_W, MAX_VISIBLE_DESKS, MIN_TOP_MARGIN,
    OBSTACLE_PAD_PX,
};

/// Backwards-compat alias — existing call sites construct `Layout::compute()`.
pub type Layout = SceneLayout;
