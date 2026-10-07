//! What the last frame shows a pointer over an area of the office, for every
//! painter: a figure, the board's star, the coffee machine or a labelled
//! fixture. A tooltip and a click both resolve an area through [`scene_hit`],
//! so a click acts on exactly what the tooltip names.

use crate::display::{HoverTarget, Hovers};
use crate::layout::{Bounds, SceneLayout};

/// The repository the board's star links to.
pub const REPO_URL: &str = "https://github.com/IvanWng97/pixtuoid";

/// Where the coffee machine links.
const COFFEE_URL: &str = "https://buymeacoffee.com/IvanWng97";

/// What an area shows the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneHit<'a> {
    /// An agent, a pet or a mascot.
    Figure(&'a HoverTarget),
    /// The wall board's star, a link to the repo.
    Star,
    /// The pantry's coffee machine, a link to buy the author a coffee.
    Coffee,
    /// A fixture, by its label.
    Furniture(&'static str),
}

impl SceneHit<'_> {
    /// The page a click on it opens, if it is a link.
    pub fn link(self) -> Option<&'static str> {
        match self {
            Self::Star => Some(REPO_URL),
            Self::Coffee => Some(COFFEE_URL),
            Self::Figure(_) | Self::Furniture(_) => None,
        }
    }
}

/// The topmost of `hovers` at `area` (in layout units), else the board's
/// `star`, else the coffee machine, else a labelled fixture of `layout`.
pub fn scene_hit<'a>(
    hovers: &'a Hovers,
    star: Option<Bounds>,
    layout: &SceneLayout,
    area: Bounds,
) -> Option<SceneHit<'a>> {
    figure_or_fixture(hovers.at(area), star, layout, area)
}

/// `figure`, the topmost hover at `area`, else the board's `star`, else the
/// coffee machine, else a labelled fixture of `layout`.
fn figure_or_fixture<'a>(
    figure: Option<&'a HoverTarget>,
    star: Option<Bounds>,
    layout: &SceneLayout,
    area: Bounds,
) -> Option<SceneHit<'a>> {
    if let Some(target) = figure {
        Some(SceneHit::Figure(target))
    } else if star.is_some_and(|b| b.overlaps(area)) {
        Some(SceneHit::Star)
    } else if coffee_at(layout, area) {
        Some(SceneHit::Coffee)
    } else {
        fixture_label_at(layout, area).map(SceneHit::Furniture)
    }
}

/// Whether `area` shows the coffee-machine section of the pantry counter.
pub fn coffee_at(layout: &SceneLayout, area: Bounds) -> bool {
    layout.coffee_machine().is_some_and(|b| b.overlaps(area))
}

/// The label of the fixture `area` shows, if it carries one; the coffee
/// machine is [`coffee_at`]'s, for its link.
pub fn fixture_label_at(layout: &SceneLayout, area: Bounds) -> Option<&'static str> {
    layout.fixture_at(area)?.hover_label()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A figure over the coffee machine is the hit, and the machine is
    /// without one.
    #[test]
    fn a_figure_over_the_coffee_machine_is_the_hit() {
        let layout = SceneLayout::compute(160, 200, Some(4)).expect("layout");
        let coffee = layout
            .coffee_machine()
            .expect("a pantry with a coffee machine");
        let mid = Bounds {
            x: coffee.x + coffee.width / 2,
            y: coffee.y + coffee.height / 2,
            width: 1,
            height: 1,
        };
        let cat = HoverTarget::Pet(crate::display::PetHover {
            kind: crate::pet::PetKind::Cat,
            centre: crate::layout::Point { x: mid.x, y: mid.y },
            anim: "cat_walk",
        });
        assert!(matches!(
            figure_or_fixture(Some(&cat), None, &layout, mid),
            Some(SceneHit::Figure(t)) if *t == cat
        ));
        assert_eq!(
            figure_or_fixture(None, None, &layout, mid),
            Some(SceneHit::Coffee)
        );
        assert_eq!(SceneHit::Coffee.link(), Some(COFFEE_URL));
        assert_eq!(SceneHit::Star.link(), Some(REPO_URL));
    }
}
