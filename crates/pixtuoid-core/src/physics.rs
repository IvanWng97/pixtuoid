//! Pure physics model for character walking.
//!
//! Imports only `crate::AgentId`. No router, no layout, no terminal deps.
//! All kinematics are f32; screen is ≤ ~4096 px → ≤ ~57k octile, well
//! within f32's 24-bit mantissa.

use crate::AgentId;

// ── Intent ────────────────────────────────────────────────────────────────────

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

// ── Constants ─────────────────────────────────────────────────────────────────

/// Cruise speed for Entry / Exit walks (octile/ms). SnapBack has its own faster
/// [`V_CRUISE_SNAPBACK`].
///
/// Calibrated against measured door→desk octile distances in the real office
/// geometry (916–1436 octile on an 8-desk floor, 206–1436 on a 16-desk floor).
/// Goal: keep the *effective average* walk pace ≈ the old flat 4 s baseline while
/// making duration distance-proportional.  Resulting durations: near ≈ 3.1 s,
/// avg ≈ 3.8 s, far ≈ 4.5 s (8-desk floor); ≈ 1.1 s for very-near desks on a
/// busy floor → a 1.4–3.4 s staggered-arrival spread.
pub const V_CRUISE_COMMUTE: f32 = 0.36;
/// Cruise speed for WanderOut / WanderBack walks (octile/ms).
///
/// Ambling speed, slower than commute.  Calibrated in proportion to
/// `V_CRUISE_COMMUTE` to preserve the commute-vs-wander pace contrast.
pub const V_CRUISE_WANDER: f32 = 0.25;
/// Cruise speed for SnapBack walks (octile/ms) — faster than commute.
///
/// A snap-back is an URGENT return to the desk after an interruption (Idle→Active
/// mid-wander): the agent visibly *hurries* back. Paired with the higher
/// [`WALK_ACCEL_SNAPBACK`] so that BOTH short snap-backs (accel-limited) and far
/// ones (cruise-limited) stay brisk under pure physics — replacing the old
/// fixed-time compression (`eff_elapsed = elapsed · duration / SNAP_BACK_MS`).
/// Net: near snap-backs ≈ 0.4 s, far ones ≈ 1.3 s (a real, fast walk — not a
/// hard-compressed 900 ms dash).
pub const V_CRUISE_SNAPBACK: f32 = 0.65;
/// Shared acceleration/deceleration constant (octile/ms²).
///
/// Gives a ~0.55 s accel ramp (`t_a = v/a`).  Critical lengths:
/// `L_crit = v²/a ≈ 199` octile (commute), `≈ 96` octile (wander).
pub const WALK_ACCEL: f32 = 6.5e-4;
/// Acceleration for SnapBack walks (octile/ms²) — ~3× [`WALK_ACCEL`].
///
/// Short snap-backs are acceleration-limited (triangular: `T = 2·√(L/a)`,
/// cruise-independent), so the urgent return *accelerates harder* to stay snappy.
/// Paired with [`V_CRUISE_SNAPBACK`] for the far (cruise-limited) case.
pub const WALK_ACCEL_SNAPBACK: f32 = 2.0e-3;

/// Minimum per-agent speed multiplier.
pub const SPEED_MULT_MIN: f32 = 0.85;
/// Maximum per-agent speed multiplier.
pub const SPEED_MULT_MAX: f32 = 1.20;

/// Minimum arrival settle pause (ms).
pub const PAUSE_MS_MIN: u64 = 200;
/// Maximum arrival settle pause (ms).
pub const PAUSE_MS_MAX: u64 = 400;

// ── Profile ───────────────────────────────────────────────────────────────────

/// Frozen kinematic profile for one walk leg, computed once at walk-start.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkProfile {
    /// Accel → cruise → decel total time, **excluding** arrival pause.
    pub duration_ms: u64,
    /// Per-agent arrival settle before the pose flips to seated/at-waypoint.
    pub pause_ms: u64,
    /// Snapshotted A* path length (octile units).
    pub path_len_octile: u32,
    /// Effective cruise speed after `speed_mult` applied.
    pub v_cruise: f32,
    /// Acceleration constant (same as `WALK_ACCEL`; stored for walk_progress).
    pub accel: f32,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Deterministic per-agent speed multiplier in [SPEED_MULT_MIN, SPEED_MULT_MAX].
///
/// Uses bits 24..34 of the agent's hash (10 bits → 1024 buckets), mapping
/// linearly to [0.85, 1.20]. Disjoint from `personality_for` (bits 0..14) and
/// from the low-16 bits used by `cycle_ms_for`.
pub fn speed_mult(agent_id: AgentId) -> f32 {
    // Finalize with splitmix64 before slicing so distinct agents get distinct
    // speeds (raw FNV-1a doesn't avalanche the high bits — see `splitmix64`).
    let z = crate::id::splitmix64(agent_id.raw());
    let bits = (z >> 24) & 0x3FF; // 0..=1023
    let t = bits as f32 / 1023.0; // [0.0, 1.0]
    SPEED_MULT_MIN + t * (SPEED_MULT_MAX - SPEED_MULT_MIN)
}

/// Deterministic per-agent arrival pause in [PAUSE_MS_MIN, PAUSE_MS_MAX].
///
/// Uses bits 40..52 of the agent's hash (12 bits → 4096 buckets), mapping
/// linearly to [200, 400]. Independent of `speed_mult` (bits 24..34).
pub fn pause_ms_for(agent_id: AgentId) -> u64 {
    // Same splitmix64 finalize as speed_mult, but a disjoint bit window so
    // pause is independent of speed (a fast walker is not always a brief pauser).
    let z = crate::id::splitmix64(agent_id.raw());
    let bits = (z >> 40) & 0xFFF; // 0..=4095
                                  // f64 (not f32 like speed_mult): the output is a u64 ms count, so f64 keeps
                                  // the bits→[0,1]→ms integer round-trip exact across the full 200..=400 range.
    let t = bits as f64 / 4095.0;
    PAUSE_MS_MIN + (t * (PAUSE_MS_MAX - PAUSE_MS_MIN) as f64) as u64
}

/// Compute the frozen kinematic profile for a walk of `path_len_octile` units.
///
/// Kinematics (all in octile and ms units):
///   L = path_len_octile, v = v_base(intent) * speed_mult(agent_id), a = WALK_ACCEL
///   L_crit = v²/a  (path must be ≥ L_crit to reach cruise)
///   Triangular  (L < L_crit): T = 2·sqrt(L/a)
///   Trapezoidal (L ≥ L_crit): T = 2·(v/a) + (L - L_crit)/v   [= 2·t_a + t_c]
///
/// Zero-length paths: duration_ms = 0 so walk_progress returns 1000 immediately.
pub fn walk_profile(path_len_octile: u32, intent: WalkIntent, agent_id: AgentId) -> WalkProfile {
    let v_base = match intent {
        WalkIntent::SnapBack => V_CRUISE_SNAPBACK,
        WalkIntent::Entry | WalkIntent::Exit => V_CRUISE_COMMUTE,
        WalkIntent::WanderOut | WalkIntent::WanderBack => V_CRUISE_WANDER,
    };
    let v = v_base * speed_mult(agent_id);
    // SnapBack rushes back (urgent return) — a higher accel + cruise keep BOTH
    // short (accel-limited) and far (cruise-limited) snap-backs brisk under pure
    // physics (no fixed-time compression).
    let a = match intent {
        WalkIntent::SnapBack => WALK_ACCEL_SNAPBACK,
        _ => WALK_ACCEL,
    };
    let l = path_len_octile as f32;

    let duration_ms = if path_len_octile == 0 {
        0u64
    } else {
        // L_crit = v²/a; compare in octile units.
        let l_crit = v * v / a;
        let t_ms = if l < l_crit {
            // Triangular: T = 2·sqrt(L/a)
            2.0 * (l / a).sqrt()
        } else {
            // Trapezoidal: T = 2·(v/a) + (L - L_crit)/v
            let t_a = v / a;
            let t_c = (l - l_crit) / v;
            2.0 * t_a + t_c
        };
        // Already in ms (a is octile/ms², so T = sqrt(octile / (octile/ms²)) = ms).
        t_ms.round() as u64
    };

    WalkProfile {
        duration_ms,
        pause_ms: pause_ms_for(agent_id),
        path_len_octile,
        v_cruise: v,
        accel: a,
    }
}

/// Render progress as `t_x1000 = round(1000 · s(elapsed_ms) / L)`.
///
/// - `elapsed_ms < duration_ms`: physics kinematics (accel/cruise/decel).
/// - `elapsed_ms >= duration_ms`: saturates at 1000 (also covers pause window).
/// - Zero-length profile: always returns 1000.
pub fn walk_progress(p: &WalkProfile, elapsed_ms: u64) -> u16 {
    if p.path_len_octile == 0 || elapsed_ms >= p.duration_ms {
        return 1000;
    }

    let l = p.path_len_octile as f32;
    let v = p.v_cruise;
    let a = p.accel;
    // t in ms; a in octile/ms² → s in octile
    let t = elapsed_ms as f32;
    let l_crit = v * v / a;

    let s = if l < l_crit {
        // Triangular regime.
        let t_half = (l / a).sqrt(); // ms to peak velocity
        if t <= t_half {
            0.5 * a * t * t
        } else {
            let t_total = 2.0 * t_half;
            let dt = t_total - t;
            l - 0.5 * a * dt * dt
        }
    } else {
        // Trapezoidal regime.
        let t_a = v / a; // accel time (ms)
        let t_c = (l - l_crit) / v; // cruise time (ms)
        let t_cruise_end = t_a + t_c;
        let t_total = 2.0 * t_a + t_c;

        if t <= t_a {
            // Accel phase.
            0.5 * a * t * t
        } else if t <= t_cruise_end {
            // Cruise phase: constant velocity.
            let d_a = 0.5 * a * t_a * t_a;
            d_a + v * (t - t_a)
        } else {
            // Decel phase.
            let dt = t_total - t;
            l - 0.5 * a * dt * dt
        }
    };

    // INVARIANT: the `elapsed_ms >= duration_ms` guard above prevents reaching
    // here at t ≥ t_total, but f32 rounding can still nudge s slightly outside
    // [0, L] at phase boundaries — clamp defensively (two-layer defence).
    let s_clamped = s.max(0.0).min(l);
    (1000.0 * s_clamped / l).round() as u16
}

/// Returns `true` when the full walk + pause has elapsed.
///
/// `elapsed_ms >= duration_ms + pause_ms` — the pose flip to seated/at-waypoint
/// happens only after the arrival settle beat completes.
pub fn walk_arrived(p: &WalkProfile, elapsed_ms: u64) -> bool {
    elapsed_ms >= p.duration_ms + p.pause_ms
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ids ──────────────────────────────────────────────────────────

    fn id(n: u8) -> AgentId {
        AgentId::from_parts("test", &format!("agent-{n}"))
    }

    // ── Constant sanity ─────────────────────────────────────────────────────

    #[test]
    fn commute_faster_than_wander() {
        // Use runtime variables to avoid the `assertions_on_constants` lint.
        let commute = V_CRUISE_COMMUTE;
        let wander = V_CRUISE_WANDER;
        assert!(
            commute > wander,
            "commute speed ({commute}) must exceed wander speed ({wander})"
        );
    }

    // ── speed_mult ──────────────────────────────────────────────────────────

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

    // ── pause_ms_for ────────────────────────────────────────────────────────

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
        // Verify at least some agents have different pause_ms while sharing
        // the same broad speed bucket — i.e. the two values are not identical
        // linear functions of each other.
        let pairs: Vec<(f32, u64)> = (0..50u8)
            .map(|n| (speed_mult(id(n)), pause_ms_for(id(n))))
            .collect();
        // Correlation: count agents whose speed_mult is in the lower half of
        // the range but whose pause_ms is in the upper half, and vice versa.
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

    // ── walk_profile: triangular regime ─────────────────────────────────────

    /// L_crit = v²/a. A path shorter than L_crit never reaches cruise.
    fn l_crit(v: f32) -> f32 {
        v * v / WALK_ACCEL
    }

    #[test]
    fn triangular_duration_formula() {
        // For L < L_crit: T = 2·sqrt(L/a). Use v_commute with speed_mult=1.0
        // by choosing an agent whose speed_mult is exactly 1.0 … which we
        // can't guarantee. Instead use a SHORT path and verify the formula
        // relationship rather than an absolute value.
        //
        // Strategy: pick L = L_crit/4 (well into triangular regime for any
        // agent speed in [0.85,1.20]·V_CRUISE_COMMUTE).
        // T_expected = 2·sqrt(L/a); allow ±5ms for rounding.
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
            // T = 2·sqrt(L / a). Match walk_profile's `.round()` so the
            // expected value uses identical rounding to the implementation.
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
        // For L >= L_crit: T = v/a + (L - L_crit)/v.
        // Use L = 1200 (≫ L_crit for all agents under commute speed).
        let l: u32 = 1200;

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_f = l as f32;
            let lc = l_crit(v);
            assert!(
                l_f >= lc,
                "agent {n}: L={l_f} should be >= L_crit={lc} for trapezoidal"
            );
            // T = t_a + t_c + t_a = v/a + (L-L_crit)/v
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

    // ── walk_progress: boundary values ──────────────────────────────────────

    const EPS: u16 = 2; // tolerance on t_x1000

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
        // In the triangular regime, s(T/2) = L/2 exactly (symmetry), so p=500.
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
    fn progress_at_half_duration_trapezoidal() {
        // In the trapezoidal regime, T/2 falls somewhere in the cruise band
        // (for long paths). p should be > 400 and < 600 (symmetry; doesn't
        // need to be exactly 500 because accel != decel fractions differ).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (400..=600).contains(&p),
            "trapezoidal p(T/2) should be in 400..=600, got {p}"
        );
    }

    // ── walk_progress: cruise plateau proves constant velocity ───────────────

    #[test]
    fn cruise_plateau_has_constant_delta() {
        // During cruise, Δs per Δt is constant → equal Δ(t_x1000) for equal Δt.
        // Use L=1200 (trapezoidal, clear cruise band).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(3));
        let v = profile.v_cruise;
        let lc = l_crit(v);
        // t_a = time to reach cruise (ms)
        let t_a_ms = (v / WALK_ACCEL) as u64;
        // sample 5 points in the cruise band
        let cruise_start = t_a_ms + 50;
        let cruise_end = profile.duration_ms - t_a_ms - 50; // symmetric decel
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

    // ── walk_progress: saturation and monotonicity ───────────────────────────

    #[test]
    fn progress_saturates_at_1000() {
        let profile = walk_profile(500, WalkIntent::Entry, id(1));
        // Well past duration
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

    // ── walk_arrived ─────────────────────────────────────────────────────────

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
        // At exactly duration_ms we are in the pause window.
        assert!(
            !walk_arrived(&profile, profile.duration_ms),
            "must not arrive at duration_ms (still in pause)"
        );
        // Midway through pause.
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
        // During [duration_ms, duration_ms+pause_ms), t_x1000 should be 1000
        // (agent is standing at the destination in the walk sprite).
        let profile = walk_profile(700, WalkIntent::WanderBack, id(5));
        let during_pause = profile.duration_ms + profile.pause_ms / 2;
        let p = walk_progress(&profile, during_pause);
        assert_eq!(
            p, 1000,
            "progress during pause window must be 1000, got {p}"
        );
    }

    // ── zero-length path ─────────────────────────────────────────────────────

    #[test]
    fn zero_length_no_panic() {
        let profile = walk_profile(0, WalkIntent::SnapBack, id(6));
        // Must not panic; progress should immediately be 1000.
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

    // ── intent ordering ──────────────────────────────────────────────────────

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
