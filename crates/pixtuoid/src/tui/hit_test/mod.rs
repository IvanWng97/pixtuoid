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

/// What the bare office shows at `cell`, no figure or star over it: the
/// probe the tests read the layout's fixtures through.
#[cfg(test)]
fn bare_hit(layout: &SceneLayout, cell: CellArea) -> Option<SceneHit<'static>> {
    static NONE: std::sync::LazyLock<Hovers> = std::sync::LazyLock::new(Hovers::default);
    pixtuoid_scene::hit::scene_hit(&NONE, None, layout, cell.bounds())
}

/// Whether the bare office shows the coffee machine at `cell`.
#[cfg(test)]
pub(crate) fn hit_test_coffee_machine(layout: &SceneLayout, cell: CellArea) -> bool {
    bare_hit(layout, cell) == Some(SceneHit::Coffee)
}

/// The fixture label the bare office shows at `cell`.
#[cfg(test)]
pub(crate) fn hit_test_furniture(layout: &SceneLayout, cell: CellArea) -> Option<&'static str> {
    match bare_hit(layout, cell) {
        Some(SceneHit::Furniture(label)) => Some(label),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
