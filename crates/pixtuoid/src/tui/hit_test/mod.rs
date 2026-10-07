//! Mouse hit-testing at a terminal cell: the engine's [`pixtuoid_scene::hit`]
//! over the office pixels the cell shows.

use pixtuoid_scene::display::Hovers;
use pixtuoid_scene::layout::{Bounds, SceneLayout};

use crate::tui::geometry::CellArea;

pub(crate) use pixtuoid_scene::hit::SceneHit;

/// [`pixtuoid_scene::hit::scene_hit`] at the office pixels `cell` shows.
pub(crate) fn scene_hit<'a>(
    hovers: &'a Hovers,
    star: Option<Bounds>,
    layout: &SceneLayout,
    cell: CellArea,
) -> Option<SceneHit<'a>> {
    pixtuoid_scene::hit::scene_hit(hovers, star, layout, cell.bounds())
}

/// [`pixtuoid_scene::hit::coffee_at`] at the office pixels `cell` shows.
#[cfg(test)]
pub(crate) fn hit_test_coffee_machine(layout: &SceneLayout, cell: CellArea) -> bool {
    pixtuoid_scene::hit::coffee_at(layout, cell.bounds())
}

/// [`pixtuoid_scene::hit::fixture_label_at`] at the office pixels `cell` shows.
#[cfg(test)]
pub(crate) fn hit_test_furniture(layout: &SceneLayout, cell: CellArea) -> Option<&'static str> {
    pixtuoid_scene::hit::fixture_label_at(layout, cell.bounds())
}

#[cfg(test)]
mod tests;
