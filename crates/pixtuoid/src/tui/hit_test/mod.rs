//! Mouse hit-testing: which agent, pet, mascot or piece of furniture is painted
//! under a terminal cell.

use std::time::SystemTime;

use pixtuoid_core::{AgentId, SceneState};

use pixtuoid_scene::layout::{anchored_top_left, Anchor, Bounds, Layout, Point, Size};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::character_anchor;
use pixtuoid_scene::pose;

use crate::tui::geometry::CellArea;

/// Hover and click box: the default sprite `character_anchor` places, not a
/// custom pack's frame.
const AGENT_BOX: Size = Size {
    w: pixtuoid_scene::layout::CHARACTER_SPRITE_W,
    h: pixtuoid_scene::layout::CHARACTER_SPRITE_H,
};

/// Hit-test `cell` against each agent's current sprite box, anchored on
/// `character_anchor`.
pub(crate) fn hit_test_agent(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut pose::RouteCtx<'_>,
    cell: CellArea,
) -> Option<AgentId> {
    scene
        .agents
        .values()
        .find(|agent| {
            character_anchor(agent, layout, now, rctx)
                .is_some_and(|anchor| box_hit(Anchor::TopLeft, anchor, AGENT_BOX, cell))
        })
        .map(|agent| agent.agent_id)
}

/// Home-desk-only agent hit-test (no router/overlay state) — the deterministic
/// seated-agent locator for the test harness, which has no populated `route_ctx`.
/// A seated agent's `character_anchor` is `seated_anchor_facing` of its desk,
/// which this reads directly, so the two agree.
///
/// `scene` must be a SINGLE-FLOOR scene matching `layout` (the caller projects via
/// `project_floor_scene` first): indexing `layout.home_desks` with a raw
/// multi-floor `desk_index` can pin an invisible agent from another floor.
#[cfg(test)]
pub(crate) fn hit_test_from_tui(
    scene: &SceneState,
    layout: &Layout,
    cell: CellArea,
) -> Option<AgentId> {
    for agent in scene.agents.values() {
        // `single_floor_local()`, NOT the arithmetic bridge: on an out-of-range
        // desk the bridge would wrap onto a synthetic later floor and could land
        // back in `[0..len)` — hit-testable while invisible to the renderer.
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // The painter's own anchor, not the desk box: a south-facing desk seats
        // its sitter north of the desk.
        let a = pixtuoid_scene::pixel_painter::seated_anchor_facing(
            desk,
            AGENT_BOX.w,
            layout.desk_facing(agent.desk_index.single_floor_local()),
        );
        if box_hit(Anchor::TopLeft, a, AGENT_BOX, cell) {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Whether `cell` shows the coffee-machine section of the pantry counter
/// sprite.
pub(crate) fn hit_test_coffee_machine(layout: &Layout, cell: CellArea) -> bool {
    layout.coffee_machine().is_some_and(|b| on_rect(b, cell))
}

/// The label of the fixture hovering `cell` points at, if it carries one. The
/// coffee machine is handled separately for its click-to-open behavior.
pub(crate) fn hit_test_furniture(layout: &Layout, cell: CellArea) -> Option<&'static str> {
    layout.fixture_at(cell.bounds())?.hover_label()
}

fn on_rect(b: Bounds, cell: CellArea) -> bool {
    box_hit(
        Anchor::TopLeft,
        Point { x: b.x, y: b.y },
        Size {
            w: b.width,
            h: b.height,
        },
        cell,
    )
}

/// Whether `cell` shows the office pet's sprite. `pet_pos` is its center
/// anchor in pixel coordinates; `anim_name` selects the bounding-box size via
/// `PetKind::hitbox`.
pub(crate) fn hit_test_pet(kind: PetKind, pet_pos: Point, anim_name: &str, cell: CellArea) -> bool {
    box_hit(Anchor::Center, pet_pos, kind.hitbox(anim_name), cell)
}

/// Whether `cell` shows a `size`-px box placed at `pos` (pixel coords) by
/// `anchor` — through [`anchored_top_left`], the painter's own placement.
fn box_hit(anchor: Anchor, pos: Point, size: Size, cell: CellArea) -> bool {
    cell.overlaps(
        anchored_top_left(anchor, pos, size.w, size.h),
        size.w,
        size.h,
    )
}

/// True if `cell` shows the gateway mascot's `w`×`h`-px sprite, centered at
/// `pos` (pixel coords). `w`/`h` must come from the PAINTED frame
/// (`MascotFrame`, which reads the pack's real size), so a re-tuned or
/// custom-pack mascot keeps its click box aligned with what's drawn.
pub(crate) fn hit_test_mascot(pos: Point, w: u16, h: u16, cell: CellArea) -> bool {
    box_hit(Anchor::Center, pos, Size { w, h }, cell)
}

#[cfg(test)]
mod tests;
