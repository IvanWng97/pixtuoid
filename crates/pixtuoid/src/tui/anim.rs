//! Stateless easing curves for animations.
//!
//! `Easing::apply` maps a normalized `t ∈ [0.0, 1.0]` through a chosen curve.
//! `eased_progress` (added in Task 2) is the convenience wrapper that takes a
//! wall-clock `started_at` + `duration_ms` and returns the eased progress.
//!
//! SystemTime: matches existing animation state (FloorTransition,
//! LightingState, PoseHistory) for v2 daemon-split compatibility.
//! See CLAUDE.md "Known sharp edges".

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
    /// Inputs outside that range are clamped.
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

/// Compute the eased progress of an animation `[0.0, 1.0]` given its
/// `started_at` wall-clock time, total `duration_ms`, and `easing` curve.
///
/// Clamps to `0.0` if `now` is before `started_at`, and to `1.0` if
/// `duration_ms` has fully elapsed.
pub fn eased_progress(
    started_at: SystemTime,
    duration_ms: u32,
    easing: Easing,
    now: SystemTime,
) -> f32 {
    let elapsed = now
        .duration_since(started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as f32;
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
        // Should overshoot midpoint (fast start, slow end)
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
        // A zero-length animation reads as instantly complete (raw = 1.0),
        // never divides by zero — covers the `duration_ms == 0` guard.
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
