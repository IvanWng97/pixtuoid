//! Pure mixing math — target-chasing gain ramps plus the typing/raindrop
//! schedulers. No devices, no clocks: time arrives as `dt`/`now_s` parameters.

use super::dsp::NoiseStream;
use crate::audio::StemLevels;

/// The looped stems the sink plays. `typing` is NOT here — it is a scheduled
/// one-shot voice (`TypingScheduler`), not a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoopStem {
    Pad,
    Sparkle,
    Keys,
    Drums,
    Texture,
    Rain,
}

impl LoopStem {
    pub const ALL: [LoopStem; 6] = [
        LoopStem::Pad,
        LoopStem::Sparkle,
        LoopStem::Keys,
        LoopStem::Drums,
        LoopStem::Texture,
        LoopStem::Rain,
    ];

    fn level_of(self, s: &StemLevels) -> f32 {
        match self {
            LoopStem::Pad => s.pad,
            LoopStem::Sparkle => s.sparkle,
            LoopStem::Keys => s.keys,
            LoopStem::Drums => s.drums,
            LoopStem::Texture => s.texture,
            LoopStem::Rain => s.rain,
        }
    }

    pub(super) fn field_mut(self, s: &mut StemLevels) -> &mut f32 {
        match self {
            LoopStem::Pad => &mut s.pad,
            LoopStem::Sparkle => &mut s.sparkle,
            LoopStem::Keys => &mut s.keys,
            LoopStem::Drums => &mut s.drums,
            LoopStem::Texture => &mut s.texture,
            LoopStem::Rain => &mut s.rain,
        }
    }
}

/// Full-scale gain travel per second — a tier change crossfades rather than
/// stepping (the "office gets busier" feel, not a cut).
pub const RAMP_PER_S: f32 = 0.5;

/// Master-bus trim under the user volume: ambient office sound must sit UNDER
/// the user's real work/music, and the stems SUM on the live path with no soft
/// clip.
const BUS_TRIM: f32 = 0.35;

/// Per-stem gain ramps chasing the scene's target levels. Mute ramps to
/// silence through the same slew (no click).
pub struct Mixer {
    current: [f32; LoopStem::ALL.len()],
    target: StemLevels,
    muted: bool,
    master: f32,
}

impl Mixer {
    pub fn new(master: f32) -> Self {
        Self {
            current: [0.0; LoopStem::ALL.len()],
            target: StemLevels::default(),
            muted: false,
            master,
        }
    }

    pub fn set_target(&mut self, stems: StemLevels) {
        self.target = stems;
    }

    /// Live master-volume update (the +/- keys) — targets rescale next
    /// step, riding the same slew as any level change (no zipper).
    pub fn set_master(&mut self, master: f32) {
        self.master = master.clamp(0.0, 1.0);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// The USER volume (0..1) mapped to bus amplitude: a squared perceptual curve
    /// (loudness is logarithmic) under the ambient BUS_TRIM. The ONE
    /// volume→amplitude site; the footer keeps showing the user's linear percent.
    fn master_amp(&self) -> f32 {
        self.master * self.master * BUS_TRIM
    }

    /// The scalar every one-shot's gain multiplies through — mute silences them
    /// instantly (one-shots are transient; no ramp needed).
    pub fn one_shot_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.master_amp()
        }
    }

    /// Advance every gain toward its target; returns `(stem, gain)` pairs
    /// for the sink. Never overshoots.
    pub fn step(&mut self, dt_s: f32) -> [(LoopStem, f32); LoopStem::ALL.len()] {
        let max_delta = RAMP_PER_S * dt_s;
        let mut out = [(LoopStem::Pad, 0.0f32); LoopStem::ALL.len()];
        for (i, stem) in LoopStem::ALL.into_iter().enumerate() {
            let goal = if self.muted {
                0.0
            } else {
                stem.level_of(&self.target) * self.master_amp()
            };
            let cur = self.current[i];
            let next = if (goal - cur).abs() <= max_delta {
                goal
            } else if goal > cur {
                cur + max_delta
            } else {
                cur - max_delta
            };
            self.current[i] = next;
            out[i] = (stem, next);
        }
        out
    }
}

/// Typing-burst scheduler driven by the scene's `typing` level: 0 = silence,
/// higher = more bursts.
pub struct TypingScheduler {
    rng: NoiseStream,
    burst_left: u32,
    next_at_s: f64,
}

const BURSTS_PER_MIN_AT_FULL: f64 = 28.0;

impl TypingScheduler {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: NoiseStream::new(seed),
            burst_left: 0,
            next_at_s: 0.0,
        }
    }

    /// Advance to `now_s`; returns how many keystrokes fire this tick.
    pub fn tick(&mut self, now_s: f64, level: f32) -> u32 {
        if level <= 0.0 {
            self.burst_left = 0;
            // hold the clock so a later level>0 doesn't replay a backlog
            self.next_at_s = now_s;
            return 0;
        }
        let mut fired = 0u32;
        while self.next_at_s <= now_s {
            if self.burst_left == 0 {
                let per_s = BURSTS_PER_MIN_AT_FULL / 60.0 * level as f64;
                let gap = (0.5 + self.rng.unit() as f64) / per_s.max(1e-6);
                self.burst_left = 8 + (self.rng.unit() * 14.0) as u32;
                self.next_at_s += gap;
            } else {
                fired += 1;
                self.burst_left -= 1;
                let mut gap = 0.066 + 0.030 * self.rng.unit() as f64;
                if self.rng.unit() < 0.08 {
                    gap += 0.18;
                }
                self.next_at_s += gap;
            }
        }
        fired
    }
}

/// Runtime raindrop scatter — the bed loops, the drops never repeat.
pub struct DropScheduler {
    rng: NoiseStream,
    next_at_s: f64,
}

const DROPS_PER_S: f64 = 0.9;

impl DropScheduler {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: NoiseStream::new(seed),
            next_at_s: 0.0,
        }
    }

    /// Returns how many drops fire this tick (0 while dry).
    pub fn tick(&mut self, now_s: f64, rain_level: f32) -> u32 {
        if rain_level <= 0.0 {
            self.next_at_s = now_s;
            return 0;
        }
        let mut fired = 0u32;
        while self.next_at_s <= now_s {
            fired += 1;
            let gap = (0.4 + 1.2 * self.rng.unit() as f64) / DROPS_PER_S;
            self.next_at_s += gap;
            if self.rng.unit() < 0.35 {
                fired += 1;
                self.next_at_s += 0.20 + 0.15 * self.rng.unit() as f64;
            }
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(pad: f32, rain: f32, typing: f32) -> StemLevels {
        StemLevels {
            pad,
            rain,
            typing,
            ..Default::default()
        }
    }

    #[test]
    fn mixer_ramps_toward_targets_without_overshoot() {
        let mut m = Mixer::new(1.0);
        m.set_target(levels(0.8, 0.0, 0.0));
        let g1 = m.step(0.1);
        let pad1 = g1.iter().find(|(s, _)| *s == LoopStem::Pad).unwrap().1;
        assert!((pad1 - 0.05).abs() < 1e-6, "ramp step {pad1}");
        for _ in 0..100 {
            m.step(0.1);
        }
        let g = m.step(0.1);
        let pad = g.iter().find(|(s, _)| *s == LoopStem::Pad).unwrap().1;
        assert_eq!(pad, 0.8 * BUS_TRIM, "converged without overshoot");
    }

    #[test]
    fn mute_ramps_all_loops_to_zero_and_zeroes_one_shots() {
        let mut m = Mixer::new(1.0);
        m.set_target(levels(0.8, 0.5, 0.5));
        for _ in 0..100 {
            m.step(0.1);
        }
        m.set_muted(true);
        assert_eq!(m.one_shot_gain(), 0.0, "one-shots cut instantly");
        for _ in 0..100 {
            m.step(0.1);
        }
        assert!(
            m.step(0.1).iter().all(|(_, g)| *g == 0.0),
            "loops ramped out"
        );
    }

    #[test]
    fn master_volume_scales_every_loop_target() {
        let mut m = Mixer::new(0.5);
        m.set_target(levels(0.8, 0.0, 0.0));
        for _ in 0..200 {
            m.step(0.1);
        }
        let g = m.step(0.1);
        let pad = g.iter().find(|(s, _)| *s == LoopStem::Pad).unwrap().1;
        let want = 0.8 * 0.25 * BUS_TRIM;
        assert!((pad - want).abs() < 1e-6, "curved master: {pad} vs {want}");
    }

    #[test]
    fn volume_curve_makes_low_percents_near_silent() {
        let mut quiet = Mixer::new(0.05);
        quiet.set_target(levels(1.0, 0.0, 0.0));
        for _ in 0..300 {
            quiet.step(0.1);
        }
        let g = quiet.step(0.1);
        let pad = g.iter().find(|(s, _)| *s == LoopStem::Pad).unwrap().1;
        assert!(pad < 0.001, "5% must be near-silent, got {pad}");

        let mut full = Mixer::new(1.0);
        full.set_target(levels(1.0, 0.0, 0.0));
        for _ in 0..300 {
            full.step(0.1);
        }
        let g = full.step(0.1);
        let pad = g.iter().find(|(s, _)| *s == LoopStem::Pad).unwrap().1;
        assert!(
            (pad - BUS_TRIM).abs() < 1e-6,
            "100% = the trimmed bus: {pad}"
        );
    }

    #[test]
    fn typing_scheduler_is_silent_at_zero_and_types_at_level() {
        let mut t = TypingScheduler::new(1);
        let mut total = 0;
        for i in 0..600 {
            total += t.tick(i as f64 * 0.1, 0.0);
        }
        assert_eq!(total, 0, "level 0 must never type");
        let first = t.tick(60.05, 0.5);
        assert!(first <= 1, "no backlog replay, got {first}");
        let mut typed = 0;
        for i in 0..1200 {
            typed += t.tick(60.1 + i as f64 * 0.05, 0.5);
        }
        assert!(
            (60..=400).contains(&typed),
            "moderate typing rate out of band: {typed} keys/min"
        );
    }

    #[test]
    fn drop_scheduler_matches_the_reference_density_band() {
        let mut d = DropScheduler::new(2);
        let mut drops = 0;
        for i in 0..600 {
            drops += d.tick(i as f64 * 0.1, 0.55);
        }
        assert!(
            (30..=110).contains(&drops),
            "drop density out of band: {drops}/min"
        );
        assert_eq!(d.tick(61.0, 0.0), 0, "dry sky drops nothing");
    }
}
