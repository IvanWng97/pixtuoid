//! What a frame shows that flashes, and the hold every painter puts its frames
//! through so each phase stays on screen at least
//! [`PHOTOSENSITIVE_PHASE_MIN_MS`]. Only a painter knows when a frame reaches
//! its screen, so the hold is the painter's; its rule is this one.

use std::time::{Duration, SystemTime};

use crate::anim::PHOTOSENSITIVE_PHASE_MIN_MS;
use crate::floor::{FloorCtx, FloorMeta};

/// What of a floor's frame flashes: which of a strike's levels lights it, and
/// whether a starved neon tube is catching. Two frames flash alike iff equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlashPhase {
    // The sky's flash level's bits: a strike's levels are exact constants.
    strike: u32,
    stutter: bool,
}

/// The [`FlashPhase`] of the frame `ctx` was last stepped to at `now` on
/// `floor`, under the sky that frame is painted under.
pub fn flash_phase(floor: FloorMeta, ctx: &FloorCtx, now: SystemTime) -> FlashPhase {
    let sky = crate::sky::Sky::at(floor.motion.timing(now), floor.weather);
    FlashPhase {
        strike: sky.flash().to_bits(),
        stutter: ctx.neon.stutters(),
    }
}

/// The phases a picture shows: a floor slide's two floors', the leaving one's
/// first, or one floor's twice.
pub type Flashes = [FlashPhase; 2];

/// The phases a painter's screen shows, `P`, and since when: a frame showing
/// others goes out the moment it is painted, but not before those on screen
/// have shown [`PHOTOSENSITIVE_PHASE_MIN_MS`].
#[derive(Debug)]
pub struct FlashHold<P> {
    shown: Option<(P, SystemTime)>,
}

impl<P> Default for FlashHold<P> {
    fn default() -> Self {
        Self { shown: None }
    }
}

impl<P: Copy + PartialEq> FlashHold<P> {
    /// Whether a frame showing `phases` waits. A clock run backward reads as
    /// elapsed, so the screen never freezes.
    pub fn holds(&self, phases: P, now: SystemTime) -> bool {
        self.shown.is_some_and(|(on, since)| {
            on != phases
                && now
                    .duration_since(since)
                    .is_ok_and(|d| d < Duration::from_millis(PHOTOSENSITIVE_PHASE_MIN_MS))
        })
    }

    /// Whether a frame showing `phases` changes the screen's: it goes out at
    /// once, whatever the painter's cadence.
    pub fn changes(&self, phases: P) -> bool {
        self.shown.is_none_or(|(on, _)| on != phases)
    }

    /// A frame showing `phases` reached the screen at `now`.
    pub fn shown(&mut self, phases: P, now: SystemTime) {
        if self.changes(phases) {
            self.shown = Some((phases, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: Duration = Duration::from_millis(PHOTOSENSITIVE_PHASE_MIN_MS);

    fn at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(ms)
    }

    /// A change waits out the floor from the instant the phases on screen
    /// showed, on both sides of it; the same phases never wait.
    #[test]
    fn a_hold_keeps_a_phase_the_floor_from_when_it_showed() {
        let mut hold = FlashHold::default();
        assert!(!hold.holds(1, at(0)), "an empty screen holds nothing");
        assert!(hold.changes(1));
        hold.shown(1, at(0));
        hold.shown(1, at(50));
        let floor = FLOOR.as_millis() as u64;
        assert!(hold.holds(2, at(floor - 1)), "timed from the first showing");
        assert!(!hold.holds(2, at(floor)));
        assert!(!hold.holds(1, at(1)), "the phases on screen never wait");
        assert!(!hold.changes(1) && hold.changes(2));
        assert!(
            !hold.holds(2, at(0) - Duration::from_millis(1)),
            "a clock run backward"
        );
    }

    /// A floor's phase changes exactly where its sky's flash level does, over
    /// a minute of storm, and is dark where the sky is.
    #[test]
    fn a_flash_phase_follows_the_skys_flash_level() {
        const MINUTE_MS: u64 = 60_000;
        let storm = FloorMeta::ground().with_weather(crate::sky::WeatherPolicy::Forced(
            crate::sky::Weather::Storm,
        ));
        let ctx = FloorCtx::new();
        let sky = |ms| crate::sky::Sky::at(storm.motion.timing(at(ms)), storm.weather).flash();
        let phase = |ms| flash_phase(storm, &ctx, at(ms));
        let tick = crate::anim::FULL_TICK_MS;
        let mut lit = 0;
        for n in 1..MINUTE_MS / tick {
            let (was, ms) = ((n - 1) * tick, n * tick);
            assert_eq!(phase(ms) != phase(was), sky(ms) != sky(was), "{ms} ms");
            assert_eq!(
                phase(ms) == FlashPhase::default(),
                sky(ms) == 0.0,
                "{ms} ms"
            );
            lit += usize::from(sky(ms) > 0.0);
        }
        assert!(lit > 0, "a minute of storm strikes");
    }
}
