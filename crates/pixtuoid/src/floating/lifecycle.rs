//! What the window loop does around each redraw and at exit, apart from winit
//! and the GPU, so a test drives it with fakes: `FloatingApp` cannot be built
//! without a live event loop.

use crate::jank::Jank;

/// What one redraw touches.
pub(super) trait Frame {
    fn jank(&mut self) -> &mut Jank;
    /// Paint one frame, recording it with [`Jank::record`] when it presents.
    fn paint(&mut self);
    fn recursor(&mut self);
}

/// One redraw: the frame, then the cursor it leaves under the pointer. Every
/// way `paint` ends without presenting skips `record`, the only thing that
/// spends a scheduled frame's due, so it is spent here.
pub(super) fn redraw(frame: &mut impl Frame) {
    frame.paint();
    frame.jank().skipped();
    frame.recursor();
}

/// What the loop's end releases.
pub(super) trait Teardown {
    /// Hide and release the window and the surface drawn onto it.
    fn close_surface(&mut self);
    /// Flush audio state and join the device thread, which can wait out a
    /// synthesis build ([`crate::audio::AudioHandle::shutdown`]).
    fn stop_audio(&mut self);
}

/// The loop's end: the window goes before the audio join, so a quit never
/// leaves it on screen, frozen, while the device thread is waited out.
pub(super) fn tear_down(parts: &mut impl Teardown) {
    parts.close_surface();
    parts.stop_audio();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Fake {
        jank: Jank,
        log: Vec<&'static str>,
        presents: bool,
    }

    impl Frame for Fake {
        fn jank(&mut self) -> &mut Jank {
            &mut self.jank
        }
        fn paint(&mut self) {
            self.log.push("paint");
            if self.presents {
                self.jank.record(
                    Duration::from_millis(1),
                    Duration::ZERO,
                    None,
                    None,
                    Instant::now(),
                );
            }
        }
        fn recursor(&mut self) {
            self.log.push("recursor");
        }
    }

    /// A scheduled frame `paint` skipped spends its due: the redraw after it,
    /// which no schedule asked for, is not booked as scheduled or late.
    #[test]
    fn a_skipped_scheduled_frame_is_not_booked_against_the_next_redraw() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let logged = crate::test_capture::capture(|| {
            let mut app = Fake {
                jank: Jank::new(),
                log: Vec::new(),
                presents: false,
            };
            app.jank.scheduled_every(ms(33));
            app.jank.due_at(t0);
            redraw(&mut app);
            assert_eq!(app.log, ["paint", "recursor"]);
            app.presents = true;
            redraw(&mut app);
            app.jank.finish(t0);
        });
        let line = logged
            .lines()
            .find(|l| l.contains("frame pacing"))
            .unwrap_or_default();
        assert!(line.contains("scheduled=0 "), "{line}");
        assert!(line.contains("hitch_ms=0.0 "), "{line}");
    }

    /// A frame that presents spends its due itself, so it still counts.
    #[test]
    fn a_presented_scheduled_frame_still_counts() {
        let logged = crate::test_capture::capture(|| {
            let mut app = Fake {
                jank: Jank::new(),
                log: Vec::new(),
                presents: true,
            };
            app.jank.due_at(Instant::now());
            redraw(&mut app);
            app.jank.finish(Instant::now());
        });
        assert!(logged.contains("scheduled=1 "), "{logged}");
    }

    struct Parts {
        log: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl Teardown for Parts {
        fn close_surface(&mut self) {
            self.log.lock().unwrap().push("window closed");
        }
        fn stop_audio(&mut self) {
            // A join that waits out a synthesis build.
            self.log.lock().unwrap().push("audio join begins");
            std::thread::sleep(Duration::from_millis(20));
            self.log.lock().unwrap().push("audio join ends");
        }
    }

    /// The window is gone before the audio join starts, whatever it takes.
    #[test]
    fn the_window_closes_before_the_audio_join_begins() {
        let log = std::sync::Arc::default();
        let mut parts = Parts {
            log: std::sync::Arc::clone(&log),
        };
        tear_down(&mut parts);
        assert_eq!(
            *log.lock().unwrap(),
            ["window closed", "audio join begins", "audio join ends"]
        );
    }
}
