//! Back-to-front ordering for the display list: the painter's algorithm keyed
//! on each piece's base row, then its [`Layer`].
//!
//! ## The one thing a key cannot fix
//!
//! A LONG object has no meaningful base row — a room's east wall runs the whole
//! height of the room, so its south edge would sort it in front of everything
//! inside. The object has to be SPLIT into pieces each of which does have a
//! base row. `compose.rs` splits wall runs; this module assumes it happened, and
//! `no_wall_segment_is_taller_than_the_cast` pins that no segment is tall
//! enough to straddle a figure.

use crate::layout::{Bounds, Tie};

/// Which of everything sorted at one row paints on top: a painter's tie key,
/// ordered as [`Tie`] orders the fixtures in it. The derived order is the
/// paint order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Layer {
    /// A fixture a figure at its row sits on or stands in front of.
    Under,
    /// A character, a pet or a mascot.
    Figure,
    /// A fixture that hides a figure at its row, and a glass wall band, which
    /// composites over whoever stands behind it.
    Over,
}

impl From<Tie> for Layer {
    fn from(tie: Tie) -> Self {
        match tie {
            Tie::FigureOver => Layer::Under,
            Tie::FixtureOver => Layer::Over,
        }
    }
}

/// A piece's painted bounds in LOGICAL units, inclusive on both ends, and the
/// row it sorts on.
///
/// The bounds hold EVERY pixel the piece's own paint writes — a repaint of the
/// pieces whose bounds meet a damaged rect is only complete if nothing a piece
/// draws falls outside its own. Its shadow is laid in a pass of its own under
/// every piece and reaches further ([`Piece::reach`](super::Piece::reach)). The depth is
/// a separate fact: a person sorts on the sim's sort row, which is not the south
/// edge of what they paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Span {
    /// Westmost column.
    pub x0: u16,
    /// Eastmost column.
    pub x1: u16,
    /// Northmost row.
    pub y0: u16,
    /// Southmost row.
    pub y1: u16,
    /// The row it sorts on (the module's "base row"); greater draws later.
    pub depth: u16,
    /// Which draws later among pieces at one `depth` ([`Layer`]).
    pub layer: Layer,
}

impl Span {
    /// A box of `w`x`h` whose top-left is `(x, y)`, plus `below` extra rows its
    /// painter draws underneath (a front face), sorted on its south edge.
    pub(crate) fn new(x: u16, y: u16, w: u16, h: u16, below: u16) -> Self {
        let y1 = y.saturating_add(h.saturating_sub(1)).saturating_add(below);
        Self {
            x0: x,
            x1: x.saturating_add(w.saturating_sub(1)),
            y0: y,
            y1,
            depth: y1,
            layer: Layer::Under,
        }
    }

    /// The same bounds, sorted on `depth` instead.
    pub(crate) fn with_depth(self, depth: u16) -> Self {
        Self { depth, ..self }
    }

    /// The same bounds, in `layer` at its depth.
    pub(crate) fn with_layer(self, layer: Layer) -> Self {
        Self { layer, ..self }
    }

    /// Whether it shares a cell with `area`, in the same logical units.
    #[cfg(test)]
    pub(crate) fn meets(self, area: Bounds) -> bool {
        self.bounds().overlaps(area)
    }

    /// The same cells as a [`Bounds`].
    pub(crate) fn bounds(self) -> Bounds {
        Bounds {
            x: self.x0,
            y: self.y0,
            width: (self.x1 - self.x0).saturating_add(1),
            height: (self.y1 - self.y0).saturating_add(1),
        }
    }

    fn key(self) -> (u16, Layer) {
        (self.depth, self.layer)
    }
}

/// Order `items` back to front: by base row, then layer. The sort is stable, so
/// pieces at one (row, layer) keep the order they were collected in — a render
/// that reshuffled them between frames would flicker.
pub(crate) fn depth_sort<T>(mut items: Vec<(Span, T)>) -> Vec<T> {
    items.sort_by_key(|(span, _)| span.key());
    items.into_iter().map(|(_, t)| t).collect()
}

/// The first pair drawn in the wrong order where it can be seen: their columns
/// overlap, yet the one further from the viewer is drawn last. `compose.rs`'s
/// tests drive it over a REAL laid-out office.
#[cfg(test)]
pub(crate) fn check_order(spans: &[Span], order: &[usize]) -> Option<(usize, usize)> {
    let mut position = vec![0usize; spans.len()];
    for (slot, &i) in order.iter().enumerate() {
        position[i] = slot;
    }
    let behind = |a: Span, b: Span| a.x0 <= b.x1 && b.x0 <= a.x1 && a.key() < b.key();
    for a in 0..spans.len() {
        for b in 0..spans.len() {
            if a != b && behind(spans[a], spans[b]) && position[a] > position[b] {
                return Some((a, b));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn span(x: u16, y: u16, w: u16, h: u16) -> Span {
        Span::new(x, y, w, h, 0)
    }

    fn any_span() -> impl Strategy<Value = Span> {
        (0u16..60, 0u16..60, 1u16..12, 1u16..12, 0u16..3, 0u8..3).prop_map(
            |(x, y, w, h, below, layer)| {
                Span::new(x, y, w, h, below).with_layer(match layer {
                    0 => Layer::Under,
                    1 => Layer::Figure,
                    _ => Layer::Over,
                })
            },
        )
    }

    proptest! {
        /// The order is fully determined: (base row, layer) ascending, pieces at
        /// one key in the order they came — so every pair whose columns overlap
        /// is drawn back to front.
        #[test]
        fn the_order_is_row_then_layer_then_arrival(
            spans in prop::collection::vec(any_span(), 0..40),
        ) {
            let tagged: Vec<(Span, usize)> = spans.iter().copied().zip(0..).collect();
            let order = depth_sort(tagged);
            prop_assert_eq!(order.len(), spans.len());
            for pair in order.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                prop_assert!(
                    (spans[a].key(), a) < (spans[b].key(), b),
                    "{a} drawn before {b} out of (row, layer, arrival) order"
                );
            }
            prop_assert_eq!(check_order(&spans, &order), None);
        }
    }

    /// The piece whose FEET are further north is drawn first; sorting by the
    /// sprite's top would swap a tall piece and a short one on the same row.
    #[test]
    fn a_piece_whose_feet_are_further_north_is_drawn_first() {
        // Same base row region, wildly different heights: only the feet matter.
        let tall = span(10, 0, 4, 20); // feet at 19
        let short = span(10, 18, 4, 3); // feet at 20
        let out = depth_sort(vec![(short, "short"), (tall, "tall")]);
        assert_eq!(out, vec!["tall", "short"]);
    }

    /// Why splitting is mandatory: a 40-row wall run and a thing standing halfway
    /// down it, in the same column. Unsplit, the run's only base row is its south
    /// end, so it paints in front of everything it encloses, its north half
    /// included. Split, each segment carries its own base row and lands on the
    /// correct side.
    #[test]
    fn a_long_run_must_be_split_to_order_correctly_against_its_contents() {
        let thing = span(0, 20, 6, 4); // feet at 23
        let wall = span(0, 0, 1, 40); // feet at 39 — the run's south end

        let unsplit = depth_sort(vec![(wall, "wall"), (thing, "thing")]);
        assert_eq!(
            unsplit,
            vec!["thing", "wall"],
            "unsplit, the ENTIRE run paints in front — including its north half, \
             which the thing should be occluding"
        );

        // The same run in 4-row segments.
        let mut items: Vec<(Span, String)> = (0..10)
            .map(|i| (span(0, i * 4, 1, 4), format!("seg{i}")))
            .collect();
        items.push((thing, "thing".to_string()));
        let out = depth_sort(items);
        let at = |name: &str| out.iter().position(|s| s == name).expect("present");
        assert!(
            at("seg0") < at("thing"),
            "a segment whose feet are north of the thing is BEHIND it: {out:?}"
        );
        assert!(
            at("thing") < at("seg9"),
            "a segment whose feet are south of it is in FRONT: {out:?}"
        );
    }
}
