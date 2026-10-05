//! Back-to-front ordering for the display list.
//!
//! The painter's algorithm needs a total order, and the office does not hand it
//! one: what it hands over is a set of PAIRWISE facts ("this desk is behind that
//! walker"). Deriving the order from a dependency graph rather than from a
//! single sort key is the standard treatment — a sprite is a node, "must be
//! drawn behind" is an edge, and a topological sort produces the display list.
//!
//! ## Why not just sort by the base row
//!
//! Today it WOULD be equivalent. [`Span::behind`] derives its edges from the
//! base row and then the layer, a total order, so the graph is acyclic by
//! construction and a plain sort produces the same list. The graph earns its
//! place two other ways:
//!
//! - `check_order` (test-only) turns every pairwise fact into an assertion. While
//!   [`Span::behind`] is acyclic it guards the sort itself; it becomes the
//!   detector the day an edge can contradict the base-row order.
//! - The relation is pairwise, so it still holds if the draw order ever stops
//!   being a function of screen y (elevation would do that), where a single key
//!   cannot express it.
//!
//! ## The one thing a graph cannot fix
//!
//! A LONG object has no meaningful base row — a room's east wall runs the whole
//! height of the room, so its south edge would sort it in front of everything
//! inside. No predicate rescues that; the object has to be SPLIT into pieces
//! each of which does have a base row (the canonical "split a block to prevent
//! a cycle"). `compose.rs` splits wall runs; this module assumes it happened, and
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

    fn overlaps_x(self, other: Self) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1
    }

    /// Whether `self` must be drawn BEFORE `other` — i.e. it is further from the
    /// viewer where their columns overlap.
    ///
    /// Pieces that do not overlap horizontally impose no constraint at all,
    /// which is what keeps the graph sparse: a desk on the west wall and a
    /// walker on the east one can be drawn in either order.
    fn behind(self, other: Self) -> bool {
        self.overlaps_x(other) && self.key() < other.key()
    }
}

/// Order `items` back to front.
///
/// Kahn's algorithm over the [`Span::behind`] graph, with the ready set kept in
/// (base row, layer) order so the result is deterministic (a topological order
/// is not unique, and a render that reshuffles equal-depth pieces between
/// frames flickers).
///
/// A cycle cannot arise from the current predicate, so the recovery arm is a
/// backstop rather than a live path: the pieces still in the graph are emitted
/// in (base row, layer) order. That degrades to a plain sort instead of
/// dropping them, which is the one outcome a renderer must never have.
pub(crate) fn depth_sort<T>(items: Vec<(Span, T)>) -> Vec<T> {
    let n = items.len();
    if n <= 1 {
        return items.into_iter().map(|(_, t)| t).collect();
    }
    let spans: Vec<Span> = items.iter().map(|(s, _)| *s).collect();

    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    for a in 0..n {
        for b in 0..n {
            if a != b && spans[a].behind(spans[b]) {
                edges[a].push(b);
                indegree[b] += 1;
            }
        }
    }

    // A min-heap on (base row, layer, index): among pieces that are mutually
    // unconstrained the shallower one wins, so the result matches the plain
    // sort the office produces today; the index is the fn doc's determinism.
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut ready: BinaryHeap<Reverse<((u16, Layer), usize)>> = (0..n)
        .filter(|&i| indegree[i] == 0)
        .map(|i| Reverse((spans[i].key(), i)))
        .collect();

    let mut out = Vec::with_capacity(n);
    let mut drawn = vec![false; n];
    while let Some(Reverse((_, i))) = ready.pop() {
        drawn[i] = true;
        out.push(i);
        for &j in &edges[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.push(Reverse((spans[j].key(), j)));
            }
        }
    }

    if out.len() < n {
        // Unreachable with the current predicate; see the fn doc.
        debug_assert!(
            false,
            "cutaway depth sort found a cycle among {} pieces",
            n - out.len()
        );
        let mut rest: Vec<usize> = (0..n).filter(|&i| !drawn[i]).collect();
        rest.sort_by_key(|&i| (spans[i].key(), i));
        out.extend(rest);
    }

    let mut slots: Vec<Option<T>> = items.into_iter().map(|(_, t)| Some(t)).collect();
    out.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// Every pairwise "must be behind" fact the geometry states, checked against the
/// order actually produced.
///
/// A correct sort satisfies every edge of an acyclic [`Span::behind`], so today
/// this guards [`depth_sort`] itself; an edge a future predicate (elevation)
/// leaves unsatisfied comes back as the returned pair, a failing test rather
/// than a render nobody looks at.
///
/// Test-only deliberately. It is O(n²) on top of the sort's own O(n²), which is
/// affordable once over a fixture and not per frame; `compose.rs`'s tests drive it over
/// a REAL laid-out office, which is the case a synthetic fixture would miss.
#[cfg(test)]
pub(crate) fn check_order(spans: &[Span], order: &[usize]) -> Option<(usize, usize)> {
    let mut position = vec![0usize; spans.len()];
    for (slot, &i) in order.iter().enumerate() {
        position[i] = slot;
    }
    for a in 0..spans.len() {
        for b in 0..spans.len() {
            if a != b && spans[a].behind(spans[b]) && position[a] > position[b] {
                return Some((a, b));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(x: u16, y: u16, w: u16, h: u16) -> Span {
        Span::new(x, y, w, h, 0)
    }

    /// The canonical rule, stated as a test: the piece whose FEET are further
    /// north is drawn first. Sorting by the sprite's top instead is the classic
    /// error — a tall piece and a short one standing on the same row would swap.
    #[test]
    fn a_piece_whose_feet_are_further_north_is_drawn_first() {
        // Same base row region, wildly different heights: only the feet matter.
        let tall = span(10, 0, 4, 20); // feet at 19
        let short = span(10, 18, 4, 3); // feet at 20
        let out = depth_sort(vec![(short, "short"), (tall, "tall")]);
        assert_eq!(out, vec!["tall", "short"]);
    }

    #[test]
    fn pieces_that_do_not_overlap_horizontally_impose_no_order() {
        let west = span(0, 50, 4, 4);
        let east = span(90, 0, 4, 4);
        // West has the SOUTHERN feet, so a pure base-row sort would put it last.
        // They never overlap, so either order renders identically — what the
        // test pins is that the result is total.
        let out = depth_sort(vec![(west, "west"), (east, "east")]);
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"west") && out.contains(&"east"));
    }

    #[test]
    fn every_pairwise_constraint_holds_for_a_dense_office() {
        // A pod grid plus walkers between the rows — the shape the office
        // actually produces, at a size that exercises real overlap.
        let mut items = Vec::new();
        let mut spans = Vec::new();
        for row in 0..6u16 {
            for col in 0..5u16 {
                let s = span(col * 14, row * 17, 14, 8);
                spans.push(s);
                items.push((s, spans.len() - 1));
                let w = span(col * 14 + 3, row * 17 + 4, 8, 12);
                spans.push(w);
                items.push((w, spans.len() - 1));
            }
        }
        let order = depth_sort(items);
        assert_eq!(order.len(), spans.len());
        assert_eq!(
            check_order(&spans, &order),
            None,
            "the produced order must satisfy every 'is behind' fact"
        );
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
