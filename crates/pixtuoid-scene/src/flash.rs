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

/// The phases a painter's screen shows, `P`, on a screen of shape `S`, and when
/// they finished reaching it on the painter's [`ScreenClock`]: a write showing
/// others goes out the moment its frame is painted, but not before those on
/// screen have shown [`PHOTOSENSITIVE_PHASE_MIN_MS`]. A held frame is still
/// painted; only its write waits. A screen of a new shape shows nothing to
/// hold, so its write goes out whatever it shows.
pub struct FlashHold<P, S> {
    shown: Option<(P, S, Duration)>,
    clock: ScreenClock,
}

impl<P: std::fmt::Debug, S: std::fmt::Debug> std::fmt::Debug for FlashHold<P, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlashHold")
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl<P: Copy + PartialEq, S: Copy + PartialEq> FlashHold<P, S> {
    /// A hold on `clock`.
    pub fn on(clock: ScreenClock) -> Self {
        Self { shown: None, clock }
    }

    /// Whether a write showing `phases` to a screen shaped `shape` waits, now.
    /// A clock run backward reads as elapsed, so the screen never freezes.
    pub fn holds(&self, phases: P, shape: S) -> bool {
        let now = (self.clock)();
        self.shown.is_some_and(|(on, on_shape, since)| {
            on != phases
                && on_shape == shape
                && now
                    .checked_sub(since)
                    .is_some_and(|d| d < Duration::from_millis(PHOTOSENSITIVE_PHASE_MIN_MS))
        })
    }

    /// Whether a write showing `phases` changes the screen's: it goes out at
    /// once, whatever the painter's cadence.
    pub fn changes(&self, phases: P) -> bool {
        self.shown.is_none_or(|(on, ..)| on != phases)
    }

    /// A write showing `phases` finished reaching a screen shaped `shape` just
    /// now.
    pub fn shown(&mut self, phases: P, shape: S) {
        match &mut self.shown {
            Some((on, on_shape, _)) if *on == phases => *on_shape = shape,
            _ => self.shown = Some((phases, shape, (self.clock)())),
        }
    }
}

/// A [`ScreenClock`] a test moves by hand.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
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
    const SHAPE: (u16, u16) = (80, 24);

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
        assert!(!hold.holds(1, SHAPE), "an empty screen holds nothing");
        assert!(hold.changes(1));
        hold.shown(1, SHAPE);
        screen.at(at(50));
        hold.shown(1, SHAPE);
        let floor = FLOOR.as_millis() as u64;
        screen.at(at(floor - 1));
        assert!(hold.holds(2, SHAPE), "timed from the first showing");
        assert!(!hold.holds(1, SHAPE), "the phases on screen never wait");
        assert!(!hold.changes(1) && hold.changes(2));
        screen.at(at(floor));
        assert!(!hold.holds(2, SHAPE));
        screen.at(at(0) - Duration::from_millis(1));
        assert!(!hold.holds(2, SHAPE), "a clock run backward");
    }

    /// A screen of a new shape (a resize, a font zoom) shows nothing to hold:
    /// its write goes out at once. Phases shown on it are timed from their
    /// first showing, on whatever shape.
    #[test]
    fn a_reshaped_screen_is_never_held() {
        let screen = ManualClock::default();
        let mut hold = FlashHold::on(screen.clock());
        screen.at(at(0));
        hold.shown(1, SHAPE);
        screen.at(at(10));
        assert!(!hold.holds(2, (SHAPE.0 + 1, SHAPE.1)), "resized");
        hold.shown(1, (SHAPE.0 + 1, SHAPE.1));
        assert!(
            hold.holds(2, (SHAPE.0 + 1, SHAPE.1)),
            "the new shape holds once shown"
        );
        screen.at(at(FLOOR.as_millis() as u64));
        assert!(
            !hold.holds(2, (SHAPE.0 + 1, SHAPE.1)),
            "timed from the phase's first showing, not the reshape"
        );
    }

    /// The phase is the composed frame's own: its sky's strike level, each a
    /// phase of its own and dark at none, and its neon's catch.
    #[test]
    fn a_flash_phase_is_the_frames_strike_level_and_catch() {
        let now = at(0);
        let sky =
            |strike| crate::sky::Sky::at_with(now, crate::sky::Weather::Storm).with_strike(strike);
        let layout = crate::display::compose::tests::lively_office();
        let mut frame = crate::display::compose::tests::empty_frame(&layout);
        assert_eq!(FlashPhase::of(&sky(None), &frame), FlashPhase::default());
        let (dim, bright) = (
            FlashPhase::of(&sky(Some(crate::sky::StrikePhase::Dim)), &frame),
            FlashPhase::of(&sky(Some(crate::sky::StrikePhase::After)), &frame),
        );
        assert!(dim != bright && dim != FlashPhase::default());
        frame.neon_stutter = true;
        assert_ne!(FlashPhase::of(&sky(None), &frame), FlashPhase::default());
    }
}
