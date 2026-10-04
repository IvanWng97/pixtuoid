//! Stateless easing curves for animations, and the wall-clock ms helpers every
//! layer shares.
//!
//! Wall-clock `SystemTime`, not `Instant`: matches the rest of the animation
//! state (FloorTransition, VacancyDim, PoseHistory) — serializable, no
//! out-of-process consumer today.

use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInQuad,
}

impl Easing {
    /// Apply the easing curve to a normalized `t ∈ [0.0, 1.0]`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::EaseInQuad => t * t,
        }
    }
}

/// Milliseconds elapsed from `since` to `now`, saturating to `0` on a backward
/// clock (`now < since`). Sites that deliberately SKIP or return `None` on a
/// backward clock mean something different — don't migrate those here.
pub(crate) fn elapsed_ms(now: SystemTime, since: SystemTime) -> u64 {
    now.duration_since(since)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

/// One hour in ms — the grid gen-media's fixed clocks sit on (`--now-hour`).
pub(crate) const HOUR_MS: u64 = 3_600_000;

/// Milliseconds since the Unix epoch for `now` (0 if the clock is before it).
/// It lives HERE, below both the model and the render layer, so a model module
/// never imports the renderer for it.
pub(crate) fn epoch_ms(now: SystemTime) -> u64 {
    elapsed_ms(now, SystemTime::UNIX_EPOCH)
}

/// How much of the office's ambient life moves: the loops that only make it
/// look alive — a flicker, a twinkle, an idle wander — never what an agent is
/// doing. Each tier steps those loops on its own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Motion {
    /// Every loop, stepped each [`FULL_TICK_MS`].
    #[default]
    Full,
    /// Every loop at a quarter of Full's pace: each [`CALM_TICK_MS`] it steps
    /// one Full tick, so a loop plays Full's very sequence, slower, and none
    /// aliases on the slower repaint.
    Calm,
    /// No loop moves and no lightning flashes: each holds its first frame.
    Still,
}

/// How often a [`Motion::Full`] loop steps.
pub const FULL_TICK_MS: u64 = 125;
/// How often a [`Motion::Calm`] loop steps.
pub const CALM_TICK_MS: u64 = 500;
/// The rate the TUI and the floating window repaint an office with agents in
/// it, on every tier: the sampling rate the walks' strides are sized for
/// (`a_walking_person_never_slides`).
pub const PAINT_FPS: u32 = 30;

/// The least any phase of a flash — a strike's level, a starved neon's catch,
/// the dark between — lasts in loop time: the project's own floor, beside
/// [WCAG 2.3.1]'s three flashes in any one second.
///
/// [WCAG 2.3.1]: https://www.w3.org/TR/WCAG22/#three-flashes-or-below-threshold
pub const PHOTOSENSITIVE_PHASE_MIN_MS: u64 = 100;

impl Motion {
    /// Every tier.
    pub(crate) const ALL: [Motion; 3] = [Motion::Full, Motion::Calm, Motion::Still];

    /// Wall-clock ms per ms of loop time; `None` at rest.
    pub(crate) const fn pace(self) -> Option<u64> {
        match self {
            Self::Full => Some(1),
            Self::Calm => Some(CALM_TICK_MS / FULL_TICK_MS),
            Self::Still => None,
        }
    }

    /// The ambient clock at `now`, a function of `now` alone so a frame is
    /// too: Full's is the instant floored to its tick, Calm's that many
    /// repaints of Full ticks.
    pub(crate) fn beat(self, now: SystemTime) -> Beat {
        match self.pace() {
            Some(pace) => Beat::looping(loop_time(now, pace), pace),
            None => Beat {
                loop_ms: None,
                pace: 1,
            },
        }
    }

    /// `now` and its [`beat`](Self::beat).
    pub(crate) fn timing(self, now: SystemTime) -> Timing {
        Timing {
            now,
            beat: self.beat(now),
        }
    }
}

/// Loop time at `wall` for a tier `pace` wall ms to the loop ms: whole Full
/// ticks, one each `pace` of them.
fn loop_time(wall: SystemTime, pace: u64) -> u64 {
    epoch_ms(wall) / (FULL_TICK_MS * pace) * FULL_TICK_MS
}

/// An instant, and the [`Beat`] its ambient loops read at it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) now: SystemTime,
    pub(crate) beat: Beat,
}

/// The clock every ambient loop reads instead of the wall clock, so two
/// instants on one beat paint the same frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Beat {
    /// Loop time, in ms; `None` at rest.
    loop_ms: Option<u64>,
    /// Wall-clock ms per ms of loop time.
    pace: u64,
}

impl Beat {
    fn looping(loop_ms: u64, pace: u64) -> Self {
        Self {
            loop_ms: Some(loop_ms),
            pace,
        }
    }

    /// Milliseconds of loop time on the beat; 0 at rest, where every loop
    /// holds its first frame.
    pub(crate) fn ms(self) -> u64 {
        self.loop_ms.unwrap_or(0)
    }

    /// Whether nothing moves: a source with no frame to hold, a flash, shows
    /// none.
    pub(crate) fn is_rest(self) -> bool {
        self.loop_ms.is_none()
    }

    /// The loop time this beat's tier reads at `wall`: what [`ms`](Self::ms)
    /// is when `wall` is now, so an ambient walk times its start and its
    /// present on one clock.
    pub(crate) fn loop_at(self, wall: SystemTime) -> u64 {
        loop_time(wall, self.pace)
    }

    /// The wall-clock ms loop time `loop_ms` plays at, where what keeps real
    /// time — the weather — is read.
    pub(crate) fn wall_ms(self, loop_ms: u64) -> u64 {
        loop_ms.saturating_mul(self.pace)
    }

    /// A beat at exactly `ms`, off any tier's tick.
    #[cfg(test)]
    pub(crate) fn at_ms(ms: u64) -> Self {
        Self::looping(ms, 1)
    }
}

/// Compute the eased progress of an animation `[0.0, 1.0]` given its
/// `started_at` wall-clock time, total `duration_ms`, and `easing` curve.
pub fn eased_progress(
    started_at: SystemTime,
    duration_ms: u32,
    easing: Easing,
    now: SystemTime,
) -> f32 {
    let elapsed = elapsed_ms(now, started_at) as f32;
    let raw = if duration_ms == 0 {
        1.0
    } else {
        (elapsed / duration_ms as f32).clamp(0.0, 1.0)
    };
    easing.apply(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn each_moving_tier_steps_its_loops_once_a_tick() {
        const T0: u64 = 1_700_000_000_000;
        for (motion, tick) in [(Motion::Full, FULL_TICK_MS), (Motion::Calm, CALM_TICK_MS)] {
            let beat = |ms| motion.beat(at(T0 + ms));
            assert_eq!(beat(0), beat(tick - 1), "{motion:?} moved inside a tick");
            assert_ne!(beat(tick - 1), beat(tick), "{motion:?} held past its tick");
        }
    }

    /// Calm's repaint advances its loops by exactly one Full tick, so a loop
    /// whose frames hold whole Full ticks visits each in turn: a two-frame loop
    /// of any whole-tick period flips within that many repaints, where a plain
    /// 500 ms sample of a 250 ms loop would land on one frame forever.
    #[test]
    fn calm_plays_every_whole_tick_loop_without_aliasing() {
        const T0: u64 = 1_700_000_000_000;
        for ticks in 1..=10 {
            let period = ticks * FULL_TICK_MS;
            let frames: std::collections::HashSet<u64> = (0..=ticks)
                .map(|repaint| Motion::Calm.beat(at(T0 + repaint * CALM_TICK_MS)).ms() / period % 2)
                .collect();
            assert_eq!(frames.len(), 2, "a {period} ms two-frame loop aliased");
        }
        let step = |n: u64| Motion::Calm.beat(at(T0 + n * CALM_TICK_MS)).ms();
        assert_eq!(step(1) - step(0), FULL_TICK_MS, "one Full tick a repaint");
    }

    #[test]
    fn still_rests_at_every_instant() {
        let beat = |ms| Motion::Still.beat(at(ms));
        assert!(beat(0).is_rest());
        assert_eq!(beat(1_700_000_000_000), beat(1_700_003_600_000));
        assert_eq!(beat(0).ms(), 0, "a loop at rest holds its first frame");
    }

    #[test]
    fn linear_endpoints() {
        assert!(approx_eq(Easing::Linear.apply(0.0), 0.0));
        assert!(approx_eq(Easing::Linear.apply(1.0), 1.0));
        assert!(approx_eq(Easing::Linear.apply(0.5), 0.5));
    }

    #[test]
    fn ease_out_cubic_endpoints() {
        assert!(approx_eq(Easing::EaseOutCubic.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseOutCubic.apply(1.0), 1.0));
        assert!(Easing::EaseOutCubic.apply(0.5) > 0.5);
    }

    #[test]
    fn ease_in_out_cubic_endpoints() {
        assert!(approx_eq(Easing::EaseInOutCubic.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(1.0), 1.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(0.5), 0.5));
    }

    #[test]
    fn ease_in_quad_endpoints() {
        assert!(approx_eq(Easing::EaseInQuad.apply(0.0), 0.0));
        assert!(approx_eq(Easing::EaseInQuad.apply(1.0), 1.0));
        assert!(approx_eq(Easing::EaseInQuad.apply(0.5), 0.25));
    }

    #[test]
    fn all_curves_are_monotone_non_decreasing() {
        for curve in [
            Easing::Linear,
            Easing::EaseOutCubic,
            Easing::EaseInOutCubic,
            Easing::EaseInQuad,
        ] {
            let mut prev = -1.0_f32;
            for i in 0..=100 {
                let t = i as f32 / 100.0;
                let v = curve.apply(t);
                assert!(v >= prev, "{:?} not monotone at t={t}: {v} < {prev}", curve);
                prev = v;
            }
        }
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        assert!(approx_eq(Easing::Linear.apply(-1.0), 0.0));
        assert!(approx_eq(Easing::Linear.apply(2.0), 1.0));
        assert!(approx_eq(Easing::EaseOutCubic.apply(-0.5), 0.0));
        assert!(approx_eq(Easing::EaseInOutCubic.apply(1.5), 1.0));
    }

    #[test]
    fn eased_progress_at_start_is_zero() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let p = eased_progress(start, 200, Easing::Linear, start);
        assert!(approx_eq(p, 0.0));
    }

    #[test]
    fn eased_progress_at_end_is_one() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let now = start + Duration::from_millis(200);
        let p = eased_progress(start, 200, Easing::Linear, now);
        assert!(approx_eq(p, 1.0));
    }

    #[test]
    fn eased_progress_past_end_clamps_to_one() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let now = start + Duration::from_secs(60);
        let p = eased_progress(start, 200, Easing::Linear, now);
        assert!(approx_eq(p, 1.0));
    }

    #[test]
    fn eased_progress_now_before_start_clamps_to_zero() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let now = start - Duration::from_secs(5);
        let p = eased_progress(start, 200, Easing::Linear, now);
        assert!(approx_eq(p, 0.0));
    }

    #[test]
    fn eased_progress_zero_duration_is_complete() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(approx_eq(
            eased_progress(start, 0, Easing::Linear, start),
            1.0
        ));
        assert!(approx_eq(
            eased_progress(start, 0, Easing::EaseOutCubic, start),
            1.0
        ));
    }

    #[test]
    fn elapsed_ms_counts_forward_and_saturates_backward() {
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(elapsed_ms(since + Duration::from_millis(250), since), 250);
        assert_eq!(elapsed_ms(since, since), 0);
        assert_eq!(elapsed_ms(since - Duration::from_secs(5), since), 0);
    }

    #[test]
    fn eased_progress_applies_curve() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let now = start + Duration::from_millis(100);
        let linear = eased_progress(start, 200, Easing::Linear, now);
        let eased = eased_progress(start, 200, Easing::EaseOutCubic, now);
        assert!(approx_eq(linear, 0.5));
        assert!(
            eased > 0.8,
            "expected ease-out to be past 80% at midpoint; got {eased}"
        );
    }
}
