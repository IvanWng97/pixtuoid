//! Walkable mask visualization snapshots.
//!
//! Renders the walkable mask as a compact ASCII grid (`.` = walkable, `#` =
//! blocked) and snapshots it with `insta`. More visual than the BFS
//! connectivity test — you can SEE where the blocked areas are, so a changed
//! obstacle pad or wall placement shows up as a clear diff.

use pixtuoid_scene::layout::SceneLayout;

fn mask_to_ascii(layout: &SceneLayout) -> String {
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut s = String::with_capacity(w * h + h);
    for y in 0..h {
        for x in 0..w {
            s.push(if layout.walkable.is_walkable(x as u16, y as u16) {
                '.'
            } else {
                '#'
            });
        }
        s.push('\n');
    }
    s
}

#[test]
fn walkable_mask_standard_96x70() {
    let l = SceneLayout::compute(96, 70, Some(2)).unwrap();
    insta::assert_snapshot!("walkable_mask_standard_96x70", mask_to_ascii(&l));
}

#[test]
fn walkable_mask_open_plan_96x70() {
    let l = SceneLayout::compute_with_seed(96, 70, Some(2), 2).unwrap();
    insta::assert_snapshot!("walkable_mask_open_plan_96x70", mask_to_ascii(&l));
}
