//! Thin façade over `pixtuoid_core::layout`. The binary re-exports the
//! core types under their familiar names so existing renderer code keeps
//! working unchanged; the core module is what owns the actual layout
//! computation, walkability mask, and primitive geometry.

pub use pixtuoid_core::layout::{
    anchored_top_left, desk_furniture_def, desk_walk_anchor, furniture_def, z_sort_row, Anchor,
    Bounds, Facing, Furniture, FurnitureDef, MeetingFurniture, PlantItem, PlantKind, PodDecor,
    PodDecorItem, Point, SceneLayout, Size, WallDecor, WallDecorItem, WallSegment, Waypoint,
    WaypointKind, DESK_GAP_X, DESK_GAP_Y, DESK_H, DESK_W, ELEVATOR_H, ELEVATOR_W, MIN_TOP_MARGIN,
    OBSTACLE_PAD_PX, TEST_DEFAULT_DESKS,
};
// The pre-rename name was published in scene 0.11.x too — keep re-exporting the
// deprecated core alias so BOTH published crates drop it together at the next
// minor (the CI semver gate only covers core; this one is on the honor system).
#[allow(deprecated)]
pub use pixtuoid_core::layout::MAX_VISIBLE_DESKS;

/// Backwards-compat alias — existing call sites construct `Layout::compute()`.
pub type Layout = SceneLayout;
