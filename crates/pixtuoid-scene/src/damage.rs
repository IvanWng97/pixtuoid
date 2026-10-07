//! A frame's damage, the rects it may differ from the last one inside, as
//! any painter repaints it.

use crate::layout::Bounds;

/// `rects` merged into at most `max` disjoint boxes that cover them all, each
/// merge the pair whose box adds the least area: Mozilla's
/// `nsRegion::SimplifyOutward`, "at most aMaxRects by adding area to it ...
/// a superset of the original region" (gecko-dev `gfx/src/nsRegion.h`).
/// Overlapping boxes merge first whatever the cap, so no pixel is painted, or
/// lit, twice. The pairs wait in a heap whose stale ones are skipped as they
/// surface: O(n² log n) in the rects, not a scan of every pair a merge.
pub(crate) fn simplify_outward(rects: &[Bounds], max: usize) -> Vec<Bounds> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let area = |b: &Bounds| i64::from(b.width) * i64::from(b.height);
    let union = |a: &Bounds, b: &Bounds| {
        let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
        let (x1, y1) = (
            (a.x + a.width).max(b.x + b.width),
            (a.y + a.height).max(b.y + b.height),
        );
        Bounds {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    };
    let overlap = |a: &Bounds, b: &Bounds| {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    };
    // Overlapping pairs first, then by the area a merge adds.
    let key = |a: &Bounds, b: &Bounds| (!overlap(a, b), area(&union(a, b)) - area(a) - area(b));
    let mut boxes: Vec<Option<Bounds>> = rects.iter().copied().map(Some).collect();
    let mut heap = BinaryHeap::new();
    for i in 0..boxes.len() {
        for j in i + 1..boxes.len() {
            if let (Some(a), Some(b)) = (&boxes[i], &boxes[j]) {
                heap.push(Reverse((key(a, b), i, j)));
            }
        }
    }
    let mut alive = boxes.len();
    while let Some(Reverse(((apart, _), i, j))) = heap.pop() {
        let (Some(a), Some(b)) = (boxes[i], boxes[j]) else {
            continue;
        };
        if apart && alive <= max.max(1) {
            break;
        }
        let merged = union(&a, &b);
        boxes[i] = None;
        boxes[j] = None;
        let k = boxes.len();
        for (o, other) in boxes.iter().enumerate() {
            if let Some(other) = other {
                heap.push(Reverse((key(other, &merged), o, k)));
            }
        }
        boxes.push(Some(merged));
        alive -= 1;
    }
    boxes.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's rects merge into at most the cap's boxes that cover every
    /// one of them, the nearest merging first: rects far apart stay apart,
    /// and overlapping ones become one.
    #[test]
    fn rects_merge_into_few_boxes_that_cover_them_all() {
        let b = |x, y, width, height| Bounds {
            x,
            y,
            width,
            height,
        };
        let covers = |boxes: &[Bounds], r: &Bounds| {
            boxes.iter().any(|o| {
                o.x <= r.x
                    && o.y <= r.y
                    && r.x + r.width <= o.x + o.width
                    && r.y + r.height <= o.y + o.height
            })
        };
        // a row of windows across the top, a light far below, an overlap
        let rects: Vec<Bounds> = (0..7)
            .map(|i| b(i * 40, 0, 20, 10))
            .chain([b(150, 200, 30, 30), b(160, 210, 30, 30)])
            .collect();
        for max in [1, 2, 3, 8, 64] {
            let boxes = simplify_outward(&rects, max);
            assert!(boxes.len() <= max.max(1), "{max}: {boxes:?}");
            assert!(rects.iter().all(|r| covers(&boxes, r)), "{max}: {boxes:?}");
        }
        let two = simplify_outward(&rects, 2);
        assert!(
            two.iter().all(|o| o.y >= 200 || o.y + o.height <= 10),
            "the windows' row and the light stay apart: {two:?}"
        );
        assert_eq!(
            simplify_outward(&rects, 8).len(),
            8,
            "the overlapping pair is one box"
        );
        // Boxes never overlap, whatever the cap: a scatter that merging
        // forces into overlap, as many rects as a transition frame's.
        let scatter: Vec<Bounds> = (0..96u16)
            .map(|i| b((i * 37) % 600, (i * 53) % 400, 10 + i % 30, 8 + i % 20))
            .collect();
        for max in [1, 4, 8, 64, 200] {
            let boxes = simplify_outward(&scatter, max);
            assert!(scatter.iter().all(|r| covers(&boxes, r)), "{max}");
            for (i, a) in boxes.iter().enumerate() {
                for b in &boxes[i + 1..] {
                    let meet = a.x < b.x + b.width
                        && b.x < a.x + a.width
                        && a.y < b.y + b.height
                        && b.y < a.y + a.height;
                    assert!(!meet, "{max}: {a:?} overlaps {b:?}");
                }
            }
        }
    }
}
