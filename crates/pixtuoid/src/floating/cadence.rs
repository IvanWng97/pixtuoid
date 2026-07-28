//! The floating window's PAINT CADENCE — pure, so the animation throttle is
//! unit-testable (and coverage-counted) while `window.rs` stays codecov-ignored
//! winit glue, the same split as `geometry.rs` and `input.rs`.
//!
//! The subtlety this module exists for: `ApplicationHandler::about_to_wait` runs
//! on EVERY event-loop iteration, so an unconditional `Window::request_redraw()`
//! there leaves a redraw pending whenever the loop reaches its wait — the
//! `ControlFlow::WaitUntil` deadline set beside it then never sleeps, and the
//! window renders + presents back-to-back at 100% of a CPU core in every office
//! state. Gating the redraw REQUEST on a deadline (not just arming the wait) is
//! what makes the FPS constants below take effect.

use std::time::{Duration, Instant};

/// Animation tick rate WHILE agents (or a live gateway daemon) are present —
/// motion (walk/breathe) is time-driven, so the office must repaint continuously.
const ACTIVE_FPS: u32 = 30;
/// Slow ambient tick when the office is EMPTY — keeps the time-driven ambient layer
/// (clock/weather/lightning/day-night/pet) moving without the 30fps cost of the
/// active path. Never 0fps: a frozen clock reads as a dead/broken window.
const IDLE_AMBIENT_FPS: u32 = 1;

/// The gap between animation frames for the current office state.
fn tick(office_idle: bool) -> Duration {
    Duration::from_secs(1)
        / if office_idle {
            IDLE_AMBIENT_FPS
        } else {
            ACTIVE_FPS
        }
}

/// The next-paint deadline for the floating window's animation tick.
pub(crate) struct FrameClock {
    next: Instant,
}

impl FrameClock {
    /// Armed to paint immediately, so the first `about_to_wait` after window
    /// creation still draws.
    pub(crate) fn new(now: Instant) -> Self {
        Self { next: now }
    }

    /// One `about_to_wait` pass: `(paint, deadline)` — whether to request a
    /// redraw NOW, and the instant the loop should wait until.
    pub(crate) fn poll(&mut self, now: Instant, office_idle: bool) -> (bool, Instant) {
        let tick = tick(office_idle);
        if now >= self.next {
            self.next = now + tick;
            return (true, self.next);
        }
        // A cadence SPEED-UP (an agent arriving mid-ambient-tick) must not sit
        // out the slow deadline already armed.
        self.next = self.next.min(now + tick);
        (false, self.next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive `poll` at `step` intervals for `span` and count the paints.
    fn paints_over(office_idle: bool, span: Duration, step: Duration) -> usize {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0);
        let mut painted = 0;
        let mut elapsed = Duration::ZERO;
        while elapsed <= span {
            if clock.poll(t0 + elapsed, office_idle).0 {
                painted += 1;
            }
            elapsed += step;
        }
        painted
    }

    #[test]
    fn the_first_pass_paints_so_the_window_is_never_blank() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0);
        assert_eq!(clock.poll(t0, false), (true, t0 + tick(false)));
    }

    #[test]
    fn an_active_office_paints_at_active_fps_not_once_per_event_loop_iteration() {
        // 1000 passes over one simulated second — an event loop that is never
        // idle. Unthrottled (a `request_redraw` on every pass) this is 1000.
        let painted = paints_over(false, Duration::from_secs(1), Duration::from_millis(1));
        // 1ms poll quantization rounds each 33.3ms tick up to 34ms, so an exact
        // ACTIVE_FPS is not reachable — the teeth are the ORDER of magnitude.
        assert!(
            (ACTIVE_FPS as usize - 2..=ACTIVE_FPS as usize + 1).contains(&painted),
            "an active office must paint ~ACTIVE_FPS times per second, not once per \
             event-loop iteration (got {painted} over 1001 passes)"
        );
    }

    #[test]
    fn an_empty_office_drops_to_the_ambient_tick() {
        let painted = paints_over(true, Duration::from_secs(3), Duration::from_millis(1));
        assert_eq!(
            painted,
            IDLE_AMBIENT_FPS as usize * 3 + 1,
            "an empty office must paint at ~IDLE_AMBIENT_FPS per second"
        );
    }

    #[test]
    fn an_idle_to_active_transition_does_not_wait_out_the_slow_deadline() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0);
        assert!(clock.poll(t0, true).0); // ambient paint → armed a full second out
        let (paint, deadline) = clock.poll(t0 + Duration::from_millis(10), false);
        assert!(!paint, "10ms after a paint there is nothing to draw yet");
        assert_eq!(
            deadline,
            t0 + Duration::from_millis(10) + tick(false),
            "an agent arriving mid-ambient-tick must pull the deadline in to the \
             active cadence, not sit out the remaining ~990ms"
        );
    }

    #[test]
    fn the_returned_deadline_is_when_the_next_paint_becomes_due() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0);
        let (_, deadline) = clock.poll(t0, false);
        assert!(!clock.poll(deadline - Duration::from_nanos(1), false).0);
        assert!(clock.poll(deadline, false).0);
    }
}
