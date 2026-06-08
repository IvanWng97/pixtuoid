use std::time::{Duration, SystemTime};

use pixtuoid_core::sprite::animator::frame_index_at;

#[test]
fn frame_index_advances_with_time() {
    let start = SystemTime::now();
    let frame_ms: u32 = 100;
    let n_frames: usize = 3;

    assert_eq!(frame_index_at(start, start, frame_ms, n_frames), 0);
    assert_eq!(
        frame_index_at(start, start + Duration::from_millis(99), frame_ms, n_frames),
        0
    );
    assert_eq!(
        frame_index_at(
            start,
            start + Duration::from_millis(100),
            frame_ms,
            n_frames
        ),
        1
    );
    assert_eq!(
        frame_index_at(
            start,
            start + Duration::from_millis(250),
            frame_ms,
            n_frames
        ),
        2
    );
    assert_eq!(
        frame_index_at(
            start,
            start + Duration::from_millis(300),
            frame_ms,
            n_frames
        ),
        0
    );
}

#[test]
fn single_frame_always_returns_zero() {
    let start = SystemTime::now();
    assert_eq!(
        frame_index_at(start, start + Duration::from_secs(60), 50, 1),
        0
    );
}
