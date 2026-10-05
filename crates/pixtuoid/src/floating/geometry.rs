//! Pure window/monitor geometry for the floating desktop window — the pieces of
//! `window.rs`'s `winit` handler that don't need a live `ActiveEventLoop` /
//! cursor, so they can be unit-tested.

/// Does the saved window rect `(x, y, w, h)` overlap ANY currently-connected monitor?
///
/// Guards against restoring onto a now-disconnected monitor: frameless +
/// always-on-top + no taskbar means a fully off-screen window can never be dragged
/// back. `w`/`h` are the saved LOGICAL dims, used here only as an approximate
/// extent — a few px of HiDPI slop is irrelevant for an on/off-screen test. An
/// edge-touching window with a zero-area intersection does NOT count, and an EMPTY
/// monitor iterator returns `true` so we honor the saved position rather than
/// second-guessing the OS.
pub(crate) fn window_visible_on_monitors(
    win: (i32, i32, u32, u32),
    monitors: impl IntoIterator<Item = (i32, i32, u32, u32)>,
) -> bool {
    let (wx, wy, ww, wh) = win;
    let (win_l, win_t) = (wx as i64, wy as i64);
    let (win_r, win_b) = (win_l + ww as i64, win_t + wh as i64);
    let mut any_monitor = false;
    for (mx, my, mw, mh) in monitors {
        any_monitor = true;
        let (mon_l, mon_t) = (mx as i64, my as i64);
        let (mon_r, mon_b) = (mon_l + mw as i64, mon_t + mh as i64);
        if win_l < mon_r && win_r > mon_l && win_t < mon_b && win_b > mon_t {
            return true;
        }
    }
    !any_monitor
}

/// Is the cursor `(cx, cy)` within `corner_px` of the bottom-right corner of a `(w, h)`
/// window? A left-press there resizes the frameless window (SouthEast); elsewhere it drags.
pub(crate) fn near_resize_corner(cursor: (f64, f64), size: (u32, u32), corner_px: f64) -> bool {
    let (cx, cy) = cursor;
    let (w, h) = size;
    cx >= w as f64 - corner_px && cy >= h as f64 - corner_px
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: (i32, i32, u32, u32) = (0, 0, 1920, 1080);

    #[test]
    fn overlapping_window_is_visible() {
        assert!(window_visible_on_monitors((100, 100, 800, 600), [HD]));
    }

    #[test]
    fn fully_offscreen_after_a_monitor_disconnect_is_not_visible() {
        assert!(!window_visible_on_monitors((3000, 0, 800, 600), [HD]));
    }

    #[test]
    fn partial_overlap_counts_as_visible() {
        assert!(window_visible_on_monitors((1800, 100, 400, 300), [HD]));
    }

    #[test]
    fn edge_touching_is_not_overlap() {
        assert!(!window_visible_on_monitors((1920, 0, 100, 100), [HD]));
    }

    #[test]
    fn lands_on_a_negative_origin_second_monitor() {
        assert!(window_visible_on_monitors(
            (-1500, 100, 400, 300),
            [HD, (-1920, 0, 1920, 1080)],
        ));
    }

    #[test]
    fn empty_monitor_list_honors_the_saved_position() {
        let none: [(i32, i32, u32, u32); 0] = [];
        assert!(window_visible_on_monitors((100, 100, 800, 600), none));
    }

    #[test]
    fn near_resize_corner_only_in_the_bottom_right() {
        let size = (800, 600);
        assert!(near_resize_corner((795.0, 595.0), size, 18.0));
        assert!(!near_resize_corner((400.0, 300.0), size, 18.0));
        assert!(!near_resize_corner((795.0, 100.0), size, 18.0));
        assert!(!near_resize_corner((100.0, 595.0), size, 18.0));
    }
}
