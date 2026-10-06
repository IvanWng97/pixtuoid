//! The floating window's PAINT CADENCE, kept pure so the animation throttle is
//! unit-testable while `window.rs` stays winit glue.
//!
//! The subtlety this module exists for: `ApplicationHandler::about_to_wait` runs
//! on EVERY event-loop iteration, so an unconditional `Window::request_redraw()`
//! there leaves a redraw pending whenever the loop reaches its wait — the
//! `ControlFlow::WaitUntil` deadline set beside it then never sleeps, and the
//! window renders + presents back-to-back at 100% of a CPU core. Gating the
//! redraw REQUEST on a deadline, not just arming the wait, is what makes the FPS
//! constants below take effect.

use std::time::{Duration, Instant, SystemTime};

use pixtuoid_scene::anim::Motion;

/// Motion is time-driven, so a populated office must repaint continuously.
const ACTIVE_FPS: u32 = pixtuoid_scene::anim::PAINT_FPS;
/// Never 0fps: a frozen clock reads as a dead/broken window, so an empty
/// office whose tier holds every loop still repaints this often.
const REST_FPS: u32 = 1;
/// How far past its beat's turn an empty office paints: the wait is armed on
/// the monotonic clock and the beat turns on the wall clock, and the frame
/// must land on the new beat.
const PAST_THE_TURN: Duration = Duration::from_millis(1);

/// One frame of a populated office.
pub(super) fn frame() -> Duration {
    Duration::from_secs(1) / ACTIVE_FPS
}

/// How long after `wall` an empty office next paints: just past the turn of
/// `motion`'s beat, which all it shows steps on while its floor reports
/// nothing [moving off the beat](pixtuoid_scene::floor::FloorSession::moves_off_beat),
/// so every beat is shown and no frame between two repeats one —
/// rendering on demand, as Unity's `OnDemandRendering` does
/// (docs.unity3d.com/ScriptReference/Rendering.OnDemandRendering.html). At
/// rest, a [`REST_FPS`] tick.
fn until_beat(wall: SystemTime, motion: Motion) -> Duration {
    let Some(beat) = motion.beat_period() else {
        return Duration::from_secs(1) / REST_FPS;
    };
    let since = wall
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let into = since.as_nanos() % beat.as_nanos();
    beat.saturating_sub(Duration::from_nanos(u64::try_from(into).unwrap_or(0))) + PAST_THE_TURN
}

pub(crate) struct FrameClock {
    next: Instant,
    motion: Motion,
}

impl FrameClock {
    /// Armed to paint immediately, so the first `about_to_wait` after window
    /// creation still draws, on `motion`'s beat.
    pub(crate) fn new(now: Instant, motion: Motion) -> Self {
        Self { next: now, motion }
    }

    /// One `about_to_wait` pass at `now`, `wall` on the wall clock:
    /// `(paint, deadline)` — whether to request a redraw NOW, and the instant
    /// the loop should wait until.
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        wall: SystemTime,
        office_idle: bool,
    ) -> (bool, Instant) {
        let wait = if office_idle {
            until_beat(wall, self.motion)
        } else {
            frame()
        };
        if now >= self.next {
            self.next = now + wait;
            return (true, self.next);
        }
        // A cadence SPEED-UP (an agent arriving mid-beat) must not sit out the
        // slow deadline already armed.
        self.next = self.next.min(now + wait);
        (false, self.next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall clock `ms` into a Full beat, far from the epoch.
    fn wall_at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000) + Duration::from_millis(ms)
    }

    /// The wall instants `clock` paints at over `span`, polled every `step`,
    /// the office idle or not.
    fn paints_over(
        motion: Motion,
        office_idle: bool,
        span: Duration,
        step: Duration,
    ) -> Vec<SystemTime> {
        let (t0, wall0) = (Instant::now(), wall_at(37));
        let mut clock = FrameClock::new(t0, motion);
        let mut painted = Vec::new();
        let mut elapsed = Duration::ZERO;
        while elapsed <= span {
            if clock.poll(t0 + elapsed, wall0 + elapsed, office_idle).0 {
                painted.push(wall0 + elapsed);
            }
            elapsed += step;
        }
        painted
    }

    fn into_beat(wall: SystemTime, beat: Duration) -> Duration {
        let since = wall
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch");
        Duration::from_nanos((since.as_nanos() % beat.as_nanos()) as u64)
    }

    #[test]
    fn the_first_pass_paints_so_the_window_is_never_blank() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0, Motion::Full);
        assert_eq!(clock.poll(t0, wall_at(0), false), (true, t0 + frame()));
    }

    #[test]
    fn an_active_office_paints_at_active_fps_not_once_per_event_loop_iteration() {
        let painted = paints_over(
            Motion::Full,
            false,
            Duration::from_secs(1),
            Duration::from_millis(1),
        )
        .len();
        // A band, not equality: 1ms poll quantization can slip a tick by a ms.
        assert!(
            (ACTIVE_FPS as usize - 2..=ACTIVE_FPS as usize + 1).contains(&painted),
            "an active office must paint ~ACTIVE_FPS times per second, not once per \
             event-loop iteration (got {painted} over 1001 passes)"
        );
    }

    /// An empty office paints each beat of its tier just past the beat's
    /// turn, so every beat its sky and weather step through is shown, and
    /// once a second at rest.
    #[test]
    fn an_empty_office_paints_each_beat_just_past_its_turn() {
        let span = Duration::from_secs(3);
        for motion in [Motion::Full, Motion::Calm] {
            let beat = motion.beat_period().expect("a moving tier beats");
            let painted = paints_over(motion, true, span, Duration::from_millis(1));
            let beats = (span.as_millis() / beat.as_millis()) as usize;
            assert!(
                (beats..=beats + 1).contains(&painted.len()),
                "{motion:?}: {} paints over {beats} beats",
                painted.len()
            );
            for wall in &painted[1..] {
                assert!(
                    into_beat(*wall, beat) <= PAST_THE_TURN + Duration::from_millis(1),
                    "{motion:?} painted {:?} into its beat",
                    into_beat(*wall, beat)
                );
            }
        }
        let resting = paints_over(Motion::Still, true, span, Duration::from_millis(1));
        assert_eq!(
            resting.len(),
            REST_FPS as usize * 3 + 1,
            "at rest, once a second"
        );
    }

    #[test]
    fn an_idle_to_active_transition_does_not_wait_out_the_slow_deadline() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0, Motion::Still);
        assert!(clock.poll(t0, wall_at(0), true).0); // a rest paint, armed a second out
        let soon = Duration::from_millis(10);
        let (paint, deadline) = clock.poll(t0 + soon, wall_at(10), false);
        assert!(!paint, "10ms after a paint there is nothing to draw yet");
        assert_eq!(
            deadline,
            t0 + soon + frame(),
            "an agent arriving mid-rest must pull the deadline in to the active \
             cadence, not sit out the rest of the second"
        );
    }

    #[test]
    fn the_returned_deadline_is_when_the_next_paint_becomes_due() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(t0, Motion::Full);
        let (_, deadline) = clock.poll(t0, wall_at(0), false);
        let wall = wall_at(0) + (deadline - t0);
        assert!(
            !clock
                .poll(deadline - Duration::from_nanos(1), wall, false)
                .0
        );
        assert!(clock.poll(deadline, wall, false).0);
    }
}
