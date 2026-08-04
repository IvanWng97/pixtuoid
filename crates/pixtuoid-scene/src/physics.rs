//! Pure physics model for character walking.
//!
//! Imports only `pixtuoid_core::AgentId`. No router, no layout, no terminal deps.
//! All kinematics are f32; screen is ≤ ~4096 px → ≤ ~57k octile, well
//! within f32's 24-bit mantissa.

use pixtuoid_core::AgentId;

/// Why is this walk happening? Determines which cruise speed is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkIntent {
    /// Agent spawned, walking door → desk. Brisk commute speed.
    Entry,
    /// Session ended, walking desk → door. Brisk commute speed.
    Exit,
    /// Idle wander: desk → waypoint leg. Ambling speed.
    WanderOut,
    /// Idle wander: waypoint → desk leg. Ambling speed.
    WanderBack,
    /// Routing correction snap-back. Brisk commute speed.
    SnapBack,
}

/// Cruise speed for Entry / Exit walks (octile/ms), tuned to keep the effective
/// average walk pace ≈ 4 s while making duration distance-proportional.
pub const V_CRUISE_COMMUTE: f32 = 0.36;
/// Cruise speed for WanderOut / WanderBack walks (octile/ms) — ambling, tuned in
/// proportion to [`V_CRUISE_COMMUTE`] to preserve the commute-vs-wander contrast.
pub const V_CRUISE_WANDER: f32 = 0.25;
/// Cruise speed for SnapBack walks (octile/ms) — faster than commute, since the
/// agent visibly *hurries* back. Paired with the higher [`WALK_ACCEL_SNAPBACK`]
/// so short (accel-limited) and far (cruise-limited) snap-backs both stay brisk.
pub const V_CRUISE_SNAPBACK: f32 = 0.65;
/// Shared acceleration/deceleration constant (octile/ms²), tuned for a ~0.55 s
/// accel ramp (`t_a = v/a`).
pub const WALK_ACCEL: f32 = 6.5e-4;
/// Acceleration for SnapBack walks (octile/ms²) — ~3× [`WALK_ACCEL`]. Short
/// snap-backs are acceleration-limited (`T = 2·√(L/a)`, cruise-independent), so
/// the urgent return has to *accelerate harder* to stay snappy.
pub const WALK_ACCEL_SNAPBACK: f32 = 2.0e-3;

/// Minimum per-agent speed multiplier.
pub const SPEED_MULT_MIN: f32 = 0.85;
/// Maximum per-agent speed multiplier.
pub const SPEED_MULT_MAX: f32 = 1.20;

/// Minimum arrival settle pause (ms).
pub const PAUSE_MS_MIN: u64 = 200;
/// Maximum arrival settle pause (ms).
pub const PAUSE_MS_MAX: u64 = 400;

/// Frozen kinematic profile for one walk leg, computed once at walk-start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkProfile {
    /// Accel → cruise → decel total time, **excluding** arrival pause.
    pub duration_ms: u64,
    /// Per-agent arrival settle before the pose flips to seated/at-waypoint.
    pub pause_ms: u64,
    /// Snapshotted A* path length (octile units).
    pub path_len_octile: u32,
    /// Effective cruise speed after `speed_mult` applied.
    pub v_cruise: f32,
    /// Acceleration constant for this leg's intent; stored so `walk_progress`
    /// replays the same kinematics.
    pub accel: f32,
}

/// Deterministic per-agent speed multiplier in [SPEED_MULT_MIN, SPEED_MULT_MAX].
///
/// Uses bits 24..34 of the agent's hash — disjoint from `personality_for`
/// (bits 0..14) and from the low-16 bits used by `stale_resume_gap_ms`.
pub fn speed_mult(agent_id: AgentId) -> f32 {
    // Finalize with splitmix64 before slicing so distinct agents get distinct
    // speeds (raw FNV-1a doesn't avalanche the high bits).
    let z = pixtuoid_core::id::splitmix64(agent_id.raw());
    let bits = (z >> 24) & 0x3FF;
    let t = bits as f32 / 1023.0;
    SPEED_MULT_MIN + t * (SPEED_MULT_MAX - SPEED_MULT_MIN)
}

/// Deterministic per-agent arrival pause in [PAUSE_MS_MIN, PAUSE_MS_MAX].
///
/// Uses bits 40..52 of the agent's hash — a disjoint window from `speed_mult`,
/// so a fast walker is not always a brief pauser.
pub fn pause_ms_for(agent_id: AgentId) -> u64 {
    let z = pixtuoid_core::id::splitmix64(agent_id.raw());
    let bits = (z >> 40) & 0xFFF;
    // f64 (not f32 like speed_mult): the output is a u64 ms count, so f64 keeps
    // the bits→[0,1]→ms integer round-trip exact across the full 200..=400 range.
    let t = bits as f64 / 4095.0;
    PAUSE_MS_MIN + (t * (PAUSE_MS_MAX - PAUSE_MS_MIN) as f64) as u64
}

/// The motion SHAPE of a walk leg: the regime the kinematics resolve to for
/// `(length, cruise, accel)`, plus its phase boundaries. `walk_profile` sums it
/// to a duration and `walk_progress` evaluates position against the SAME
/// boundaries, so the two can't disagree — a disagreement renders as a sprite
/// teleporting mid-leg.
enum WalkKinematics {
    /// `L < L_crit`: accel-limited, never reaches cruise. `t_half` = ms to the
    /// peak (mid-leg); total = `2·t_half`.
    Triangular { t_half: f32 },
    /// `L ≥ L_crit`: accel `t_a`, cruise `t_c`, decel `t_a`; total = `2·t_a + t_c`.
    Trapezoidal { t_a: f32, t_c: f32 },
}

impl WalkKinematics {
    /// Resolve the regime + boundaries for a leg of octile length `l` at cruise
    /// `v` and accel `a` (both > 0). `L_crit = v²/a` is the length below which the
    /// leg is accel-limited (triangular).
    fn resolve(l: f32, v: f32, a: f32) -> Self {
        let l_crit = v * v / a;
        if l < l_crit {
            WalkKinematics::Triangular {
                t_half: (l / a).sqrt(),
            }
        } else {
            WalkKinematics::Trapezoidal {
                t_a: v / a,
                t_c: (l - l_crit) / v,
            }
        }
    }

    fn total_ms(&self) -> f32 {
        match self {
            WalkKinematics::Triangular { t_half } => 2.0 * t_half,
            WalkKinematics::Trapezoidal { t_a, t_c } => 2.0 * t_a + t_c,
        }
    }
}

/// Freeze a [`WalkProfile`] for one walk leg over `path_len_octile`, with cruise
/// speed picked by `intent` and per-agent speed/pause personality seeded from
/// `agent_id`.
pub fn walk_profile(path_len_octile: u32, intent: WalkIntent, agent_id: AgentId) -> WalkProfile {
    let v_base = match intent {
        WalkIntent::SnapBack => V_CRUISE_SNAPBACK,
        WalkIntent::Entry | WalkIntent::Exit => V_CRUISE_COMMUTE,
        WalkIntent::WanderOut | WalkIntent::WanderBack => V_CRUISE_WANDER,
    };
    let v = v_base * speed_mult(agent_id);
    let a = match intent {
        WalkIntent::SnapBack => WALK_ACCEL_SNAPBACK,
        _ => WALK_ACCEL,
    };
    let l = path_len_octile as f32;

    let duration_ms = if path_len_octile == 0 {
        0u64
    } else {
        // Already in ms: a is octile/ms², so T = sqrt(octile / (octile/ms²)) = ms.
        WalkKinematics::resolve(l, v, a).total_ms().round() as u64
    };

    WalkProfile {
        duration_ms,
        pause_ms: pause_ms_for(agent_id),
        path_len_octile,
        v_cruise: v,
        accel: a,
    }
}

/// Fixed-point resolution of `walk_progress`: it returns `t_x1000 ∈ [0, 1000]`
/// (progress scaled by this, so integer math carries three fractional digits).
pub const PROGRESS_SCALE: u16 = 1000;

/// Render progress as `t_x1000 = round(1000 · s(elapsed_ms) / L)`.
///
/// - `elapsed_ms < duration_ms`: physics kinematics (accel/cruise/decel).
/// - `elapsed_ms >= duration_ms`: saturates at 1000 (also covers pause window).
/// - Zero-length profile: always returns 1000.
pub fn walk_progress(p: &WalkProfile, elapsed_ms: u64) -> u16 {
    if p.path_len_octile == 0 || elapsed_ms >= p.duration_ms {
        return PROGRESS_SCALE;
    }

    let l = p.path_len_octile as f32;
    let v = p.v_cruise;
    let a = p.accel;
    let t = elapsed_ms as f32;
    let s = match WalkKinematics::resolve(l, v, a) {
        WalkKinematics::Triangular { t_half } => {
            if t <= t_half {
                0.5 * a * t * t
            } else {
                let dt = 2.0 * t_half - t;
                l - 0.5 * a * dt * dt
            }
        }
        WalkKinematics::Trapezoidal { t_a, t_c } => {
            let t_cruise_end = t_a + t_c;
            let t_total = 2.0 * t_a + t_c;
            if t <= t_a {
                0.5 * a * t * t
            } else if t <= t_cruise_end {
                let d_a = 0.5 * a * t_a * t_a;
                d_a + v * (t - t_a)
            } else {
                let dt = t_total - t;
                l - 0.5 * a * dt * dt
            }
        }
    };

    // The early return above prevents reaching here at t ≥ t_total, but f32
    // rounding can still nudge s slightly outside [0, L] at phase boundaries.
    let s_clamped = s.max(0.0).min(l);
    (PROGRESS_SCALE as f32 * s_clamped / l).round() as u16
}

/// Returns `true` when the full walk **and** its arrival pause have elapsed —
/// the pose flips to seated/at-waypoint only after the settle beat completes.
pub fn walk_arrived(p: &WalkProfile, elapsed_ms: u64) -> bool {
    elapsed_ms >= p.duration_ms + p.pause_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> AgentId {
        AgentId::from_parts("test", &format!("agent-{n}"))
    }

    #[test]
    fn commute_faster_than_wander() {
        // Runtime variables avoid the `assertions_on_constants` lint.
        let commute = V_CRUISE_COMMUTE;
        let wander = V_CRUISE_WANDER;
        assert!(
            commute > wander,
            "commute speed ({commute}) must exceed wander speed ({wander})"
        );
    }

    #[test]
    fn speed_mult_in_range() {
        for n in 0..=50u8 {
            let m = speed_mult(id(n));
            assert!(
                (SPEED_MULT_MIN..=SPEED_MULT_MAX).contains(&m),
                "agent {n}: speed_mult {m} out of [{SPEED_MULT_MIN}, {SPEED_MULT_MAX}]"
            );
        }
    }

    #[test]
    fn speed_mult_is_deterministic() {
        let a = id(7);
        assert_eq!(
            speed_mult(a),
            speed_mult(a),
            "speed_mult must be deterministic for the same AgentId"
        );
    }

    #[test]
    fn speed_mult_varies_across_agents() {
        let values: Vec<f32> = (0..20u8).map(|n| speed_mult(id(n))).collect();
        let distinct: std::collections::HashSet<u32> = values.iter().map(|v| v.to_bits()).collect();
        assert!(
            distinct.len() >= 5,
            "expected variance in speed_mult across agents, got {distinct:?}"
        );
    }

    #[test]
    fn pause_ms_in_range() {
        for n in 0..=50u8 {
            let p = pause_ms_for(id(n));
            assert!(
                (PAUSE_MS_MIN..=PAUSE_MS_MAX).contains(&p),
                "agent {n}: pause_ms {p} out of [{PAUSE_MS_MIN}, {PAUSE_MS_MAX}]"
            );
        }
    }

    #[test]
    fn pause_ms_independent_of_speed_mult() {
        let pairs: Vec<(f32, u64)> = (0..50u8)
            .map(|n| (speed_mult(id(n)), pause_ms_for(id(n))))
            .collect();
        let speed_mid = (SPEED_MULT_MIN + SPEED_MULT_MAX) / 2.0;
        let pause_mid = (PAUSE_MS_MIN + PAUSE_MS_MAX) / 2;
        let cross_a = pairs
            .iter()
            .filter(|(s, p)| *s < speed_mid && *p > pause_mid)
            .count();
        let cross_b = pairs
            .iter()
            .filter(|(s, p)| *s >= speed_mid && *p <= pause_mid)
            .count();
        assert!(
            cross_a + cross_b >= 4,
            "pause_ms should be independent of speed_mult; cross-quadrant count too low: {cross_a}+{cross_b}"
        );
    }

    /// L_crit = v²/a. A path shorter than L_crit never reaches cruise.
    fn l_crit(v: f32) -> f32 {
        v * v / WALK_ACCEL
    }

    #[test]
    fn triangular_duration_formula() {
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32; // guaranteed triangular for all agents

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_crit_v = l_crit(v);
            assert!(
                (l as f32) < l_crit_v,
                "agent {n}: L={l} should be < L_crit={l_crit_v}"
            );
            let t_expected_ms = (2.0 * ((l as f32) / WALK_ACCEL).sqrt()).round() as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: triangular T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    #[test]
    fn trapezoidal_duration_formula() {
        let l: u32 = 1200; // ≫ L_crit for every agent at commute speed

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_f = l as f32;
            let lc = l_crit(v);
            assert!(
                l_f >= lc,
                "agent {n}: L={l_f} should be >= L_crit={lc} for trapezoidal"
            );
            let t_a = v / WALK_ACCEL;
            let t_c = (l_f - lc) / v;
            let t_expected_ms = (2.0 * t_a + t_c) as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: trapezoidal T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    const EPS: u16 = 2;

    #[test]
    fn progress_at_zero_is_zero() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, 0);
        assert!(p <= EPS, "p(0) should be ≈0, got {p}");
    }

    #[test]
    fn progress_at_duration_is_1000() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, profile.duration_ms);
        assert!(p >= 1000 - EPS, "p(T) should be ≈1000, got {p}");
    }

    #[test]
    fn progress_at_half_duration_triangular_is_near_500() {
        // In the triangular regime, s(T/2) = L/2 exactly (symmetry).
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (500u16).abs_diff(p) <= EPS + 10,
            "triangular p(T/2) should be ≈500, got {p}"
        );
    }

    #[test]
    fn progress_in_triangular_decel_arm() {
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32; // guaranteed triangular for all agents
        let profile = walk_profile(l, WalkIntent::Entry, id(0));

        // 0.75·T is well past t_half (= T/2), so it lands in the decel arm.
        let t75 = profile.duration_ms * 3 / 4;
        let p75 = walk_progress(&profile, t75);
        assert!(
            (500..1000).contains(&p75),
            "triangular decel p(0.75T) should be in (500,1000), got {p75}"
        );

        let t60 = profile.duration_ms * 3 / 5;
        let p60 = walk_progress(&profile, t60);
        assert!(
            p75 > p60,
            "decel must keep advancing: p(0.75T)={p75} should exceed p(0.6T)={p60}"
        );
    }

    #[test]
    fn progress_at_half_duration_trapezoidal() {
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (400..=600).contains(&p),
            "trapezoidal p(T/2) should be in 400..=600, got {p}"
        );
    }

    #[test]
    fn cruise_plateau_has_constant_delta() {
        let l: u32 = 1200; // trapezoidal, with a clear cruise band
        let profile = walk_profile(l, WalkIntent::Entry, id(3));
        let v = profile.v_cruise;
        let lc = l_crit(v);
        let t_a_ms = (v / WALK_ACCEL) as u64;
        let cruise_start = t_a_ms + 50;
        let cruise_end = profile.duration_ms - t_a_ms - 50;
        assert!(
            cruise_start < cruise_end,
            "need a cruise band: t_a={t_a_ms}ms, T={}ms, L={l}, Lc={lc}",
            profile.duration_ms
        );
        let step = (cruise_end - cruise_start) / 5;
        assert!(step > 0, "cruise band too narrow to sample");
        let samples: Vec<u16> = (0..=5)
            .map(|i| walk_progress(&profile, cruise_start + i * step))
            .collect();
        let deltas: Vec<i32> = samples
            .windows(2)
            .map(|w| w[1] as i32 - w[0] as i32)
            .collect();
        let first = deltas[0];
        for (i, d) in deltas.iter().enumerate() {
            assert!(
                (d - first).abs() <= EPS as i32,
                "cruise Δ[{i}]={d} differs from Δ[0]={first} by more than {EPS} — not constant velocity"
            );
        }
    }

    #[test]
    fn progress_saturates_at_1000() {
        let profile = walk_profile(500, WalkIntent::Entry, id(1));
        let p = walk_progress(&profile, profile.duration_ms * 3);
        assert_eq!(p, 1000, "progress must saturate at 1000");
    }

    #[test]
    fn progress_is_monotone() {
        let profile = walk_profile(800, WalkIntent::WanderOut, id(2));
        let samples: Vec<u16> = (0..=20)
            .map(|i| walk_progress(&profile, i * profile.duration_ms / 20))
            .collect();
        for w in samples.windows(2) {
            assert!(
                w[1] >= w[0],
                "progress must be non-decreasing, got {} then {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn arrived_false_before_duration() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        assert!(
            !walk_arrived(&profile, profile.duration_ms - 1),
            "must not arrive before duration_ms elapses"
        );
    }

    #[test]
    fn arrived_false_during_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        assert!(
            !walk_arrived(&profile, profile.duration_ms),
            "must not arrive at duration_ms (still in pause)"
        );
        let mid_pause = profile.duration_ms + profile.pause_ms / 2;
        assert!(
            !walk_arrived(&profile, mid_pause),
            "must not arrive mid-pause"
        );
    }

    #[test]
    fn arrived_true_after_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        let after = profile.duration_ms + profile.pause_ms;
        assert!(
            walk_arrived(&profile, after),
            "must arrive once duration + pause elapsed"
        );
    }

    #[test]
    fn progress_holds_1000_during_pause_window() {
        let profile = walk_profile(700, WalkIntent::WanderBack, id(5));
        let during_pause = profile.duration_ms + profile.pause_ms / 2;
        let p = walk_progress(&profile, during_pause);
        assert_eq!(
            p, 1000,
            "progress during pause window must be 1000, got {p}"
        );
    }

    #[test]
    fn zero_length_no_panic() {
        let profile = walk_profile(0, WalkIntent::SnapBack, id(6));
        let p = walk_progress(&profile, 0);
        assert_eq!(
            p, 1000,
            "zero-length walk should report full progress at t=0"
        );
        assert!(
            walk_arrived(&profile, profile.pause_ms),
            "zero-length walk should arrive after its pause"
        );
    }

    #[test]
    fn commute_intents_faster_than_wander_intents() {
        let l: u32 = 800;
        let agent = id(9);
        let commute_dur = walk_profile(l, WalkIntent::Entry, agent).duration_ms;
        let wander_dur = walk_profile(l, WalkIntent::WanderOut, agent).duration_ms;
        assert!(
            commute_dur < wander_dur,
            "Entry ({commute_dur}ms) must be faster than WanderOut ({wander_dur}ms) for same path length"
        );
        let exit_dur = walk_profile(l, WalkIntent::Exit, agent).duration_ms;
        let back_dur = walk_profile(l, WalkIntent::WanderBack, agent).duration_ms;
        assert!(exit_dur < back_dur);
        let snap_dur = walk_profile(l, WalkIntent::SnapBack, agent).duration_ms;
        assert!(snap_dur < wander_dur);
    }

    #[test]
    fn exit_uses_commute_speed() {
        let l: u32 = 800;
        let a = id(0);
        let entry = walk_profile(l, WalkIntent::Entry, a);
        let exit = walk_profile(l, WalkIntent::Exit, a);
        assert_eq!(
            entry.v_cruise.to_bits(),
            exit.v_cruise.to_bits(),
            "Exit and Entry must use the same cruise speed (commute)"
        );
    }

    #[test]
    fn wander_out_and_back_use_same_speed() {
        let l: u32 = 600;
        let a = id(1);
        let out = walk_profile(l, WalkIntent::WanderOut, a);
        let back = walk_profile(l, WalkIntent::WanderBack, a);
        assert_eq!(
            out.v_cruise.to_bits(),
            back.v_cruise.to_bits(),
            "WanderOut and WanderBack must use the same cruise speed"
        );
    }
}

// These sweep the per-frame f32 kinematics across generated (length, intent,
// agent, elapsed) inputs, INCLUDING lengths within rounding of `L_crit = v²/a`,
// where walk_profile's and walk_progress's branch selection could disagree — a
// violation renders as a live sprite teleporting mid-leg, so a failure here is a
// real regression, not flake.
#[cfg(test)]
mod prop {
    use super::*;
    use proptest::prelude::*;

    const ALL_INTENTS: [WalkIntent; 5] = [
        WalkIntent::Entry,
        WalkIntent::Exit,
        WalkIntent::WanderOut,
        WalkIntent::WanderBack,
        WalkIntent::SnapBack,
    ];

    fn prop_id(seed: u64) -> pixtuoid_core::AgentId {
        pixtuoid_core::AgentId::from_parts("prop", &seed.to_string())
    }

    /// Sweep one profile's whole timeline: progress is bounded to [0, 1000],
    /// non-decreasing, exactly 1000 at/after duration_ms, and never panics.
    fn assert_progress_invariants(p: &WalkProfile) -> Result<(), TestCaseError> {
        let mut prev = walk_progress(p, 0);
        prop_assert!(prev <= PROGRESS_SCALE, "p(0)={} out of range", prev);
        // 32 in-flight samples + 4 past-duration ones (saturation).
        const STEPS: u64 = 32;
        for i in 1..=STEPS + 4 {
            // Saturating: duration_ms can be 0 (zero-length legs).
            let t = p.duration_ms.saturating_mul(i) / STEPS;
            let cur = walk_progress(p, t);
            prop_assert!(cur <= PROGRESS_SCALE, "p({})={} out of range", t, cur);
            prop_assert!(
                cur >= prev,
                "progress regressed: p({})={} < previous {}",
                t,
                cur,
                prev
            );
            prev = cur;
        }
        prop_assert_eq!(
            walk_progress(p, p.duration_ms),
            PROGRESS_SCALE,
            "p(duration) must saturate"
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn walk_progress_is_bounded_monotone_and_saturating(
            l in 0u32..=60_000, intent_idx in 0usize..ALL_INTENTS.len(), seed in any::<u64>(),
        ) {
            let p = walk_profile(l, ALL_INTENTS[intent_idx], prop_id(seed));
            assert_progress_invariants(&p)?;
        }

        // L_crit comes off the profile's own frozen v_cruise/accel — the SAME
        // formula the branch uses — so ±3 octile really straddles the crossover.
        #[test]
        fn regime_boundary_lengths_keep_the_invariants(
            intent_idx in 0usize..ALL_INTENTS.len(), seed in any::<u64>(), delta in -3i64..=3,
        ) {
            let intent = ALL_INTENTS[intent_idx];
            let agent = prop_id(seed);
            // Any length yields the same frozen per-agent v/a pair.
            let probe = walk_profile(1, intent, agent);
            let l_crit = probe.v_cruise * probe.v_cruise / probe.accel;
            let l = (l_crit.round() as i64 + delta).max(0) as u32;
            let p = walk_profile(l, intent, agent);
            assert_progress_invariants(&p)?;
        }
    }
}
