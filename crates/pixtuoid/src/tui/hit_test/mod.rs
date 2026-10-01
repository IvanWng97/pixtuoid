//! Mouse hit-testing: which agent, pet, mascot or piece of furniture is painted
//! under a terminal cell.

use pixtuoid_core::AgentId;

use pixtuoid_scene::layout::{Bounds, Layout, Pivot, Point, Size, anchored_top_left};
use pixtuoid_scene::pet::PetKind;
use pixtuoid_scene::pixel_painter::{AgentFrame, MascotFrame};

use crate::tui::geometry::CellArea;

/// The agent whose sprite shows at `cell`: `agents` is in paint order, so the
/// last hit is the one on top.
pub(crate) fn hit_test_agent(agents: &[AgentFrame], cell: CellArea) -> Option<AgentId> {
    agents
        .iter()
        .rev()
        .find(|a| box_hit(Pivot::TopLeft, a.anchor, Size { w: a.w, h: a.h }, cell))
        .map(|a| a.agent_id)
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
        Pivot::TopLeft,
        Point { x: b.x, y: b.y },
        Size {
            w: b.width,
            h: b.height,
        },
        cell,
    )
}

/// Whether `cell` shows the office pet's sprite.
pub(crate) fn hit_test_pet(kind: PetKind, pet_pos: Point, anim_name: &str, cell: CellArea) -> bool {
    box_hit(Pivot::Center, pet_pos, kind.hitbox(anim_name), cell)
}

/// Whether `cell` shows a `size`-px box placed at `pos` (pixel coords) by
/// `pivot` — through [`anchored_top_left`], the painter's own placement.
fn box_hit(pivot: Pivot, pos: Point, size: Size, cell: CellArea) -> bool {
    cell.overlaps(
        anchored_top_left(pivot, pos, size.w, size.h),
        size.w,
        size.h,
    )
}

/// True if `cell` shows the gateway mascot's `w`×`h`-px sprite, centered at
/// `pos` (pixel coords). `w`/`h` must come from the PAINTED frame
/// (`MascotFrame`, which reads the pack's real size), so a re-tuned or
/// custom-pack mascot keeps its click box aligned with what's drawn.
pub(crate) fn hit_test_mascot(pos: Point, w: u16, h: u16, cell: CellArea) -> bool {
    box_hit(Pivot::Center, pos, Size { w, h }, cell)
}

/// The mascot under `cell` painted on TOP: `mascots` is in `sort_drawables`'
/// paint order, so the last hit.
pub(crate) fn topmost_mascot_at(mascots: &[MascotFrame], cell: CellArea) -> Option<&MascotFrame> {
    mascots
        .iter()
        .rev()
        .find(|m| hit_test_mascot(m.pos, m.w, m.h, cell))
}

#[cfg(test)]
mod tests;
