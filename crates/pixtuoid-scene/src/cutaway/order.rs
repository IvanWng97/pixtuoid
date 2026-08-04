//! Back-to-front ordering for the cutaway's draw list.
//!
//! The painter's algorithm needs a total order, and the office does not hand it
//! one: what it hands over is a set of PAIRWISE facts ("this desk is behind that
//! walker"). Deriving the order from a dependency graph rather than from a
//! single sort key is the standard treatment — a sprite is a node, "must be
//! drawn behind" is an edge, and a topological sort produces the draw list.
//!
//! ## Why not just sort by the base row
//!
//! Today it WOULD be equivalent, and that is worth stating plainly rather than
//! discovering later. [`Span::behind`] derives its edges from the base row,
//! which is a total order, so the graph is acyclic by construction and a plain
//! sort produces the same list. The graph earns its place two other ways:
//!
//! - [`check_order`] turns every pairwise fact into an assertion. A sort key
//!   silently mis-orders whatever it cannot express; a constraint that is
//!   CHECKED tells you the day something stops fitting.
//! - The relation is pairwise, so it still holds if depth ever stops being a
//!   function of screen y (elevation would do that), where a single key cannot
//!   express it.
//!
//! ## The one thing a graph cannot fix
//!
//! A LONG object has no meaningful base row — a room's west wall runs the whole
//! height of the room, so its south edge would sort it in front of everything
//! inside. No predicate rescues that; the object has to be SPLIT into pieces
//! each of which does have a base row (the canonical "split a block to prevent
//! a cycle"). `paint.rs` splits wall runs; this module assumes it happened, and
//! [`check_order`] is what notices when it did not.

/// A piece's screen footprint in LOGICAL units, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    /// Westmost column.
    pub x0: u16,
    /// Eastmost column.
    pub x1: u16,
    /// Northmost row.
    pub y0: u16,
    /// Southmost row — the BASE row, the "feet" the order is built on.
    pub y1: u16,
}

impl Span {
    /// A box of `w`x`h` whose top-left is `(x, y)`, plus `below` extra rows its
    /// painter draws underneath (a front face, a contact shadow, a chair back).
    pub(crate) fn new(x: u16, y: u16, w: u16, h: u16, below: u16) -> Self {
        Self {
            x0: x,
            x1: x.saturating_add(w.saturating_sub(1)),
            y0: y,
            y1: y.saturating_add(h.saturating_sub(1)).saturating_add(below),
        }
    }

    fn overlaps_x(self, other: Self) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1
    }

    /// Whether `self` must be drawn BEFORE `other` — i.e. it is further from the
    /// viewer where the two actually overlap on screen.
    ///
    /// Pieces that do not overlap horizontally impose no constraint at all,
    /// which is what keeps the graph sparse: a desk on the west wall and a
    /// walker on the east one can be drawn in either order.
    fn behind(self, other: Self) -> bool {
        self.overlaps_x(other) && self.y1 < other.y1
    }
}

/// Order `items` back to front.
///
/// Kahn's algorithm over the [`Span::behind`] graph, with the ready set kept in
/// base-row order so the result is deterministic (a topological order is not
/// unique, and a render that reshuffles equal-depth pieces between frames
/// flickers).
///
/// A cycle cannot arise from the current predicate, so the recovery arm is a
/// backstop rather than a live path: the pieces still in the graph are emitted
/// in base-row order. That degrades to the pre-graph behaviour instead of
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

    // A min-heap on (base row, index): among pieces that are mutually
    // unconstrained the shallower one wins, so the result matches the plain
    // base-row order the office produces today, and the index tie-break keeps
    // it deterministic — a topological order is not unique, and a render that
    // reshuffles equal-depth pieces between frames flickers.
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut ready: BinaryHeap<Reverse<(u16, usize)>> = (0..n)
        .filter(|&i| indegree[i] == 0)
        .map(|i| Reverse((spans[i].y1, i)))
        .collect();

    let mut out = Vec::with_capacity(n);
    let mut drawn = vec![false; n];
    while let Some(Reverse((_, i))) = ready.pop() {
        drawn[i] = true;
        out.push(i);
        for &j in &edges[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.push(Reverse((spans[j].y1, j)));
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
        rest.sort_by_key(|&i| (spans[i].y1, i));
        out.extend(rest);
    }

    let mut slots: Vec<Option<T>> = items.into_iter().map(|(_, t)| Some(t)).collect();
    out.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// Every pairwise "must be behind" fact the geometry states, checked against the
/// order actually produced.
///
/// This is the half of the graph that pays for itself today. A sort key cannot
/// express a constraint it gets wrong, so a violation is invisible; here it is a
/// returned pair — an object that stops fitting the model (one too long to have
/// a base row, one at a different elevation) becomes a failing test rather than
/// a render nobody looks at.
///
/// Test-only deliberately. It is O(n²) on top of the sort's own O(n²), which is
/// affordable once over a fixture and not per frame; `paint.rs` drives it over
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
        // test pins is that the result is deterministic and total.
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

    /// The property that makes splitting mandatory, pinned so the reason cannot
    /// be lost: ONE tall span covering a whole room is behind nothing and in
    /// front of nothing it contains, so it lands wherever its own feet fall.
    /// Split into segments, each lands correctly.
    /// A 40-row wall run and a thing standing halfway down it, in the same
    /// column. Unsplit, the run's only base row is its south end, so it paints
    /// in front of everything it encloses — including what is south of it in
    /// the part of the wall that is genuinely BEHIND. Split, each segment
    /// carries its own base row and lands on the correct side.
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
