//! Mouse hit-testing: which agent, pet, mascot or piece of furniture is painted
//! under a terminal cell.

use pixtuoid_scene::display::{HoverTarget, Hovers};
use pixtuoid_scene::layout::{Bounds, Point, SceneLayout};

use crate::tui::geometry::CellArea;

/// What a cell shows the pointer. A tooltip and a click both resolve a cell
/// through [`scene_hit`], so a click acts on exactly what the tooltip names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SceneHit<'a> {
    Figure(&'a HoverTarget),
    /// The wall board's star, a link to the repo.
    Star,
    Coffee,
    Furniture(&'static str),
}

/// The topmost of `hovers` at `cell`, else the board's `star`, else the
/// coffee machine, else a labelled fixture of `layout`.
pub(crate) fn scene_hit<'a>(
    hovers: &'a Hovers,
    star: Option<Bounds>,
    layout: &SceneLayout,
    cell: CellArea,
) -> Option<SceneHit<'a>> {
    if let Some(target) = hovers.at(cell.bounds()) {
        Some(SceneHit::Figure(target))
    } else if star.is_some_and(|b| cell.overlaps(Point { x: b.x, y: b.y }, b.width, b.height)) {
        Some(SceneHit::Star)
    } else if hit_test_coffee_machine(layout, cell) {
        Some(SceneHit::Coffee)
    } else {
        hit_test_furniture(layout, cell).map(SceneHit::Furniture)
    }
}

/// Whether `cell` shows the coffee-machine section of the pantry counter
/// sprite.
pub(crate) fn hit_test_coffee_machine(layout: &SceneLayout, cell: CellArea) -> bool {
    layout
        .coffee_machine()
        .is_some_and(|b| cell.overlaps(Point { x: b.x, y: b.y }, b.width, b.height))
}

/// The label of the fixture hovering `cell` points at, if it carries one. The
/// coffee machine is handled separately for its click-to-open behavior.
pub(crate) fn hit_test_furniture(layout: &SceneLayout, cell: CellArea) -> Option<&'static str> {
    layout.fixture_at(cell.bounds())?.hover_label()
}

#[cfg(test)]
mod tests;
