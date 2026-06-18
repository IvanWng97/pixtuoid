//! Pure window/monitor geometry for the floating desktop window.
//!
//! These are the only pieces of branching logic in `window.rs`'s `winit` handler that don't
//! need a live `ActiveEventLoop` / cursor to exercise. Extracted here so they're unit-testable
//! (and counted by coverage) while the surrounding event-loop glue stays codecov-ignored — the
//! same split as `offscreen.rs` (testable render seam) vs `window.rs` (platform glue).

/// Does the saved window rect `(x, y, w, h)` overlap ANY currently-connected monitor?
///
/// Guards against restoring onto a now-disconnected monitor (the off-screen-unrecoverable
/// case: frameless + always-on-top + no taskbar → no way to drag a fully off-screen window
/// back). All values are physical px (saved position per `persist_geometry`, monitor
/// position/size from winit), so they compare directly; `w`/`h` are the saved LOGICAL dims
/// used here only as an approximate extent — a few px of HiDPI slop is irrelevant for an
/// on/off-screen test. Standard axis-aligned-rect overlap (any non-empty intersection counts;
/// an edge-touching window with a zero-area intersection does NOT). Defensive: an EMPTY
/// monitor iterator (winit reports none) returns `true` so we still honor the saved position
/// rather than second-guessing the OS.
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
    // No monitor overlapped — but if winit reported NONE, don't override the OS.
    !any_monitor
}

/// Is the cursor `(cx, cy)` within `corner_px` of the bottom-right corner of a `(w, h)`
/// window? A left-press there resizes the frameless window (SouthEast); elsewhere it drags.
/// Pure so the move-vs-resize split is testable without a real cursor.
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
        // Saved at x=3000 but only the single 1920-wide monitor remains.
        assert!(!window_visible_on_monitors((3000, 0, 800, 600), [HD]));
    }

    #[test]
    fn partial_overlap_counts_as_visible() {
        // Straddles the right edge — still partly on-screen.
        assert!(window_visible_on_monitors((1800, 100, 400, 300), [HD]));
    }

    #[test]
    fn edge_touching_is_not_overlap() {
        // Window's left edge == monitor's right edge → zero-area intersection.
        assert!(!window_visible_on_monitors((1920, 0, 100, 100), [HD]));
    }

    #[test]
    fn lands_on_a_negative_origin_second_monitor() {
        // A monitor left of the primary (negative x); the window sits on it.
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
        assert!(near_resize_corner((795.0, 595.0), size, 18.0)); // inside the corner band
        assert!(!near_resize_corner((400.0, 300.0), size, 18.0)); // center → drag
        assert!(!near_resize_corner((795.0, 100.0), size, 18.0)); // right edge, high up → drag
        assert!(!near_resize_corner((100.0, 595.0), size, 18.0)); // bottom edge, far left → drag
    }
}
