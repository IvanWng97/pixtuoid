//! What a frame shows that flashes, and the hold every painter puts its
//! writes through so each phase stays on screen at least
//! [`PHOTOSENSITIVE_PHASE_MIN_MS`]. The hold runs on the painter's own
//! monotonic clock, stamped when a write finishes, never on a scene clock: a
//! paused or stepped one can neither shorten a phase nor wedge a repaint.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::anim::PHOTOSENSITIVE_PHASE_MIN_MS;

/// What of a floor's frame flashes: which of a strike's levels lights it, and
/// whether a starved neon tube is catching. Two frames flash alike iff equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlashPhase {
    // The sky's flash level's bits: a strike's levels are exact constants.
    strike: u32,
    stutter: bool,
}

impl FlashPhase {
    /// The phase of `frame` composed under `sky`.
    pub(crate) fn of(sky: &crate::sky::Sky, frame: &crate::sim::SimFrame) -> Self {
        Self {
            strike: sky.flash().to_bits(),
            stutter: frame.neon_stutter,
        }
    }
}

/// The phases a picture shows: a floor slide's two floors', the leaving one's
/// first, or one floor's twice.
pub type Flashes = [FlashPhase; 2];

/// A painter's screen clock: monotonic time since an origin of its own.
pub type ScreenClock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// The process's monotonic clock.
#[cfg(not(target_arch = "wasm32"))]
pub fn monotonic() -> ScreenClock {
    let origin = std::time::Instant::now();
    Arc::new(move || origin.elapsed())
}

/// The phases a painter's screen shows, `P`, and when they finished reaching
/// it on the painter's [`ScreenClock`]: a write showing others goes out the
/// moment its frame is painted, but not before those on screen have shown
/// [`PHOTOSENSITIVE_PHASE_MIN_MS`]. A held frame is still painted; only its
/// write waits.
pub struct FlashHold<P> {
    shown: Option<(P, Duration)>,
    clock: ScreenClock,
}

impl<P: Copy + PartialEq> FlashHold<P> {
    /// A hold on `clock`.
    pub fn on(clock: ScreenClock) -> Self {
        Self { shown: None, clock }
    }

    /// Whether a write showing `phases` waits, now. A clock run backward reads
    /// as elapsed, so the screen never freezes.
    pub fn holds(&self, phases: P) -> bool {
        let now = (self.clock)();
        self.shown.is_some_and(|(on, since)| {
            on != phases
                && now
                    .checked_sub(since)
                    .is_some_and(|d| d < Duration::from_millis(PHOTOSENSITIVE_PHASE_MIN_MS))
        })
    }

    /// Whether a write showing `phases` changes the screen's: it goes out at
    /// once, whatever the painter's cadence.
    pub fn changes(&self, phases: P) -> bool {
        self.shown.is_none_or(|(on, _)| on != phases)
    }

    /// A write showing `phases` finished reaching the screen just now.
    pub fn shown(&mut self, phases: P) {
        if self.changes(phases) {
            self.shown = Some((phases, (self.clock)()));
        }
    }
}

/// A [`ScreenClock`] a test moves by hand.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct ManualClock(Arc<Mutex<Duration>>);

impl ManualClock {
    /// The clock to hand a [`FlashHold`].
    pub fn clock(&self) -> ScreenClock {
        let manual = self.clone();
        Arc::new(move || manual.now())
    }

    /// Where it reads.
    pub fn now(&self) -> Duration {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Set to the frame instant `at`, so a test's frames and writes share one
    /// timeline.
    pub fn at(&self, at: SystemTime) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
    }

    /// Move on by `by`: a write that takes that long.
    pub fn advance(&self, by: Duration) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += by;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: Duration = Duration::from_millis(PHOTOSENSITIVE_PHASE_MIN_MS);

    fn at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(ms)
    }

    /// A change waits out the floor on the hold's own clock from the instant
    /// the phases on screen showed, on both sides of it; the same phases never
    /// wait.
    #[test]
    fn a_hold_keeps_a_phase_the_floor_on_its_own_clock() {
        let screen = ManualClock::default();
        let mut hold = FlashHold::on(screen.clock());
        screen.at(at(0));
        assert!(!hold.holds(1), "an empty screen holds nothing");
        assert!(hold.changes(1));
        hold.shown(1);
        screen.at(at(50));
        hold.shown(1);
        let floor = FLOOR.as_millis() as u64;
        screen.at(at(floor - 1));
        assert!(hold.holds(2), "timed from the first showing");
        assert!(!hold.holds(1), "the phases on screen never wait");
        assert!(!hold.changes(1) && hold.changes(2));
        screen.at(at(floor));
        assert!(!hold.holds(2));
        screen.at(at(0) - Duration::from_millis(1));
        assert!(!hold.holds(2), "a clock run backward");
    }

    /// The phase is the composed frame's own: its sky's strike level, each a
    /// phase of its own and dark at none, and its neon's catch.
    #[test]
    fn a_flash_phase_is_the_frames_strike_level_and_catch() {
        let now = at(0);
        let sky =
            |level| crate::sky::Sky::at_with(now, crate::sky::Weather::Storm).with_flash(level);
        let layout = crate::display::compose::tests::lively_office();
        let mut frame = crate::display::compose::tests::empty_frame(&layout);
        assert_eq!(FlashPhase::of(&sky(0.0), &frame), FlashPhase::default());
        let (dim, bright) = (
            FlashPhase::of(&sky(0.25), &frame),
            FlashPhase::of(&sky(0.5), &frame),
        );
        assert!(dim != bright && dim != FlashPhase::default());
        frame.neon_stutter = true;
        assert_ne!(FlashPhase::of(&sky(0.0), &frame), FlashPhase::default());
    }
}
