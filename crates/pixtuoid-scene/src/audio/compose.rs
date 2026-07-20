//! The theory-constrained COMPOSER — generative lofi as constrained
//! sampling ("the LLM feel", owner-directed 2026-07-19): a pure function
//! `seed -> GeneratedScore`. Harmony walks a vetted progression grammar,
//! the lead obeys melody rules (strong-beat chord tones, diatonic passing
//! tones, bounded leaps with stepwise resolution, two-phrase form with a
//! peak and a loop-closing resolution), grooves come from the humanized
//! templates the frozen takes ratified — so ANY seed is musically
//! well-formed BY CONSTRUCTION (the seed-sweep property suite pins it),
//! while WHICH song you get is the seed's draw.
//!
//! Quality contract: the frozen takes each passed a per-take LISTEN gate;
//! a generator with unbounded output can't, so its gate is STATISTICAL —
//! the owner blind-auditions a batch of seeds (`examples/lofi_audition`)
//! and the generator as a whole is ratified (or its constraints
//! tightened). The frozen takes are #[cfg(test)] fingerprint anchors,
//! not a runtime fallback (ALL-GENERATIVE owner decision 2026-07-20).
//!
//! Runtime model: the seed is the [`super::track_epoch`] block (10-min
//! cadence, owner-tuned for short agent sessions), so generation is
//! DETERMINISTIC — the same block renders the same song everywhere
//! (native, wasm, tests), and "never repeats" comes from the clock
//! advancing, not from entropy.

use super::dsp::NoiseStream;
use super::score::DrumKind;

/// Which mood grammar a generated score was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Day,
    Night,
}

/// The lead-instrument registry — WHICH voice sings the sparkle lane.
/// This is the add-an-instrument seam: a new timbre = one synth voice
/// fn, one variant here, a draw weight in `compose`, a listen batch. The
/// mix lanes (StemLevels/mixer/players) are instrument-blind by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeadVoice {
    /// The velocity-keyed EP (the ratified night/day-take Rhodes).
    EpVel,
    /// Plucked-string lead ("nylon" family): sharper attack, longer
    /// fundamental ring, brighter early harmonics.
    Pluck,
}

/// A generated 8-bar composition — the runtime-sized twin of the frozen
/// `score` tables (same event vocabulary, `Vec` instead of `'static`).
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedScore {
    pub(super) mood: Mood,
    pub bpm: f32,
    pub(super) chords: [[u8; 4]; 4],
    /// The key as pitch classes — the lead's invariant home (chord tones
    /// are a subset by template curation, pinned in tests).
    pub(super) scale_pcs: [u8; 7],
    /// The hand-written-lead slot: here, the rule-generated melody.
    pub(super) sparkle: Vec<(f32, u8, f32)>,
    pub(super) keys: Vec<(f32, u8, f32)>,
    pub(super) drums: Vec<(f32, DrumKind, f32)>,
    /// The sub-bass floor per template bar — ALWAYS derived (cheap);
    /// only the night renderer reads it, so no Option and no fallback
    /// (review finding: the old `unwrap_or` re-hardcoded a magic array).
    pub(super) bass_roots: [u8; 4],
    /// Night only: kick timestamps for the texture's baked duck
    /// (empty for day — the day texture free-runs).
    pub(super) kick_times: Vec<f32>,
    /// Which instrument sings the lead (drawn LAST in the seed stream so
    /// adding voices never redraws a blessed take's notes).
    pub(super) lead_voice: LeadVoice,
    /// The 8-bar harmonic TIMELINE every stem reads: the template played
    /// twice, with (day) bar 8 swapped for the turnaround dominant —
    /// LOFI-BIBLE R4's hinge, the anti-monotony seam.
    pub(super) bar_chords: [[u8; 4]; 8],
    /// Harmonic root per timeline bar (degree-aware: voicings may be
    /// inversions) — the shell voicings and the turnaround derive here.
    pub(super) bar_roots: [u8; 8],
    /// The TEMPLATE's roots (night's sub-bass floor reads these).
    pub(super) roots_pc: [u8; 4],
}

/// Every generated take is 8 bars of 4/4 — the anti-fatigue loop length
/// both ratified moods use.
pub(super) const GEN_LOOP_BARS: usize = 8;

impl GeneratedScore {
    pub(super) fn beat_s(&self) -> f32 {
        60.0 / self.bpm
    }

    pub(super) fn bar_s(&self) -> f32 {
        self.beat_s() * super::score::BEATS_PER_BAR
    }

    pub fn loop_secs(&self) -> f32 {
        self.bar_s() * GEN_LOOP_BARS as f32
    }

    /// The lead instrument's display name (audition listings).
    pub fn lead_voice_name(&self) -> &'static str {
        match self.lead_voice {
            LeadVoice::EpVel => "ep",
            LeadVoice::Pluck => "pluck",
        }
    }
}

// ---------------------------------------------------------------- grammar

/// One vetted progression template, pre-voiced in C in the ratified pad
/// register. `roots_pc` carries the harmonic ROOT per chord (voicings may
/// be inversions, so `chord[0]` is not authoritative) — night's sub-bass
/// derives from it.
struct Progression {
    chords: [[u8; 4]; 4],
    roots_pc: [u8; 4],
    scale_pcs: [u8; 7],
    /// Carries ONE deliberate out-of-scale color move (secondary
    /// dominant / borrowed chord) — exempt from the diatonic pin, which
    /// instead asserts the color tone EXISTS. Read only by that pin.
    #[cfg_attr(not(test), allow(dead_code))]
    chromatic: bool,
}

/// C major pitch classes.
const C_MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// F major pitch classes (the day-1 modal set lives here: Dm7-Gm7-B♭-Am).
const F_MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 10];

/// The day grammar — every chord tone is in the template's scale
/// (pinned by `template_chords_are_diatonic`).
const DAY_PROGRESSIONS: [Progression; 8] = [
    // royal road: Fmaj7 G7 Em7 Am7 (the Day2 ratified changes)
    Progression {
        chords: [
            [53, 57, 60, 64],
            [55, 59, 62, 65],
            [52, 55, 59, 62],
            [57, 60, 64, 67],
        ],
        roots_pc: [5, 7, 4, 9],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // I-vi-ii-V turnaround with voice-led inversions (the Day3 changes;
    // bar 2 is Am7/C as a TRUE 7th-chord pc set — C E G A — the original
    // C-E-A-C voicing had no 7th and broke the shell derivation)
    Progression {
        chords: [
            [48, 52, 55, 59],
            [48, 52, 55, 57],
            [50, 53, 57, 60],
            [53, 55, 59, 62],
        ],
        roots_pc: [0, 9, 2, 7],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // the original day take's modal set: Dm7 Gm7 B♭maj7 Am7
    Progression {
        chords: [
            [50, 53, 57, 60],
            [55, 58, 62, 65],
            [58, 62, 65, 69],
            [57, 60, 64, 67],
        ],
        roots_pc: [2, 7, 10, 9],
        scale_pcs: F_MAJOR,
        chromatic: false,
    },
    // stepwise descent: Fmaj7 Em7 Dm7 Cmaj7
    Progression {
        chords: [
            [53, 57, 60, 64],
            [52, 55, 59, 62],
            [50, 53, 57, 60],
            [48, 52, 55, 59],
        ],
        roots_pc: [5, 4, 2, 0],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // pop cadence: Am7 Fmaj7 Cmaj7 G7
    Progression {
        chords: [
            [57, 60, 64, 67],
            [53, 57, 60, 64],
            [48, 52, 55, 59],
            [55, 59, 62, 65],
        ],
        roots_pc: [9, 5, 0, 7],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // gentle lift: Cmaj7 Fmaj7 Em7 Am7
    Progression {
        chords: [
            [48, 52, 55, 59],
            [53, 57, 60, 64],
            [52, 55, 59, 62],
            [57, 60, 64, 67],
        ],
        roots_pc: [0, 5, 4, 9],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // V7/vi: Fmaj7 E7 Am7 G7 — the E7's G# pulls into Am (owner-adopted
    // chromatic color, the composition critic's curated-template route)
    Progression {
        chords: [
            [53, 57, 60, 64],
            [52, 56, 59, 62],
            [57, 60, 64, 67],
            [55, 59, 62, 65],
        ],
        roots_pc: [5, 4, 9, 7],
        scale_pcs: C_MAJOR,
        chromatic: true,
    },
    // borrowed iv: Cmaj7 Fmaj7 Fm7 Cmaj7 — the Ab/Eb minor-plagal fade,
    // the single most beloved lofi cadence (owner-adopted)
    Progression {
        chords: [
            [48, 52, 55, 59],
            [53, 57, 60, 64],
            [53, 56, 60, 63],
            [48, 52, 55, 59],
        ],
        roots_pc: [0, 5, 5, 0],
        scale_pcs: C_MAJOR,
        chromatic: true,
    },
];

/// The night grammar — minor-leaning, root-position (the sub floor reads
/// `roots_pc`), the sleepy register.
const NIGHT_PROGRESSIONS: [Progression; 4] = [
    // the ratified night changes: Am7 Fmaj7 Cmaj7 Em7
    Progression {
        chords: [
            [57, 60, 64, 67],
            [53, 57, 60, 65],
            [48, 52, 55, 59],
            [52, 55, 59, 62],
        ],
        roots_pc: [9, 5, 0, 4],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // minor descent: Am7 G7 Fmaj7 Em7
    Progression {
        chords: [
            [57, 60, 64, 67],
            [55, 59, 62, 65],
            [53, 57, 60, 64],
            [52, 55, 59, 62],
        ],
        roots_pc: [9, 7, 5, 4],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
    // suspended drift: Dm7 Am7 B♭maj7 Fmaj7
    Progression {
        chords: [
            [50, 53, 57, 60],
            [57, 60, 64, 67],
            [58, 62, 65, 69],
            [53, 57, 60, 64],
        ],
        roots_pc: [2, 9, 10, 5],
        scale_pcs: F_MAJOR,
        chromatic: false,
    },
    // deep water: Am7 Em7 Fmaj7 Cmaj7
    Progression {
        chords: [
            [57, 60, 64, 67],
            [52, 55, 59, 62],
            [53, 57, 60, 64],
            [48, 52, 55, 59],
        ],
        roots_pc: [9, 4, 5, 0],
        scale_pcs: C_MAJOR,
        chromatic: false,
    },
];

/// Transposition window, semitones — keeps every template inside the
/// ratified pad register after the shift.
const TRANSPOSE_MIN: i8 = -3;
const TRANSPOSE_MAX: i8 = 2;

/// Day tempo window (the ratified takes sit at 72/74/76).
const DAY_BPM: (f32, f32) = (70.0, 82.0);
/// Night tempo window (the ratified night take sits at 68).
const NIGHT_BPM: (f32, f32) = (64.0, 72.0);

/// Lead register clamps (MIDI).
const DAY_LEAD_LO: u8 = 65;
const DAY_LEAD_HI: u8 = 81;
const NIGHT_LEAD_LO: u8 = 60;
const NIGHT_LEAD_HI: u8 = 79;

/// v4 humanization (the constants every ratified take bakes in).
const SWING_HATS_DAY: f32 = 0.56;
const SWING_HATS_NIGHT: f32 = 0.58;
const SWING_KICK: f32 = 0.53;
const DRAG_KICK_S: f32 = 0.015;
/// Night drags harder than day (the ratified night v4 value) — a
/// separate lag on purpose, not a drifted copy of [`DRAG_KICK_S`].
const DRAG_KICK_NIGHT_S: f32 = 0.018;

// ------------------------------------------------------------- rng helpers

fn pick(rng: &mut NoiseStream, n: usize) -> usize {
    ((rng.unit() * n as f32) as usize).min(n.saturating_sub(1))
}

fn range_f(rng: &mut NoiseStream, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * rng.unit()
}

fn chance(rng: &mut NoiseStream, p: f32) -> bool {
    rng.unit() < p
}

// ------------------------------------------------------------- pitch tools

/// Transpose a template by `t` semitones (chords, roots, scale).
fn transpose(p: &Progression, t: i8) -> ([[u8; 4]; 4], [u8; 4], [u8; 7]) {
    let mut chords = p.chords;
    for chord in &mut chords {
        for n in chord.iter_mut() {
            *n = (*n as i16 + t as i16) as u8;
        }
    }
    let mut roots = p.roots_pc;
    for r in &mut roots {
        *r = ((*r as i16 + t as i16).rem_euclid(12)) as u8;
    }
    let mut scale = p.scale_pcs;
    for s in &mut scale {
        *s = ((*s as i16 + t as i16).rem_euclid(12)) as u8;
    }
    scale.sort_unstable();
    (chords, roots, scale)
}

/// A root-position dominant 7th targeting `target_root_pc` (i.e. its V7),
/// voiced inside the ratified pad register.
fn dominant7_of(target_root_pc: u8) -> [u8; 4] {
    let dom_pc = (target_root_pc + 7) % 12;
    let mut r = 48 + dom_pc;
    if r > 57 {
        r -= 12;
    }
    [r, r + 4, r + 7, r + 10]
}

/// The 8-bar harmonic timeline + per-bar roots: template ×2, with (day)
/// bar 8 substituted by the dominant of the returning bar-1 root.
fn timeline(chords: &[[u8; 4]; 4], roots_pc: &[u8; 4], mood: Mood) -> ([[u8; 4]; 8], [u8; 8]) {
    let mut bars = [[0u8; 4]; 8];
    let mut roots = [0u8; 8];
    for bar in 0..GEN_LOOP_BARS {
        bars[bar] = chords[bar % 4];
        roots[bar] = roots_pc[bar % 4];
    }
    if mood == Mood::Day {
        bars[7] = dominant7_of(roots_pc[0]);
        roots[7] = (roots_pc[0] + 7) % 12;
    }
    (bars, roots)
}

/// The rootless comp shell: the chord's 3rd + 7th, found degree-aware
/// from the harmonic root (voicings may be inversions). Every template
/// chord is a 7th chord; the fallbacks only guard a future non-7th row.
fn shell_of(chord: &[u8; 4], root_pc: u8) -> (u8, u8) {
    let find = |offsets: [u8; 2]| {
        chord
            .iter()
            .copied()
            .find(|&n| offsets.iter().any(|&o| n % 12 == (root_pc + o) % 12))
    };
    let third = find([3, 4]).unwrap_or(chord[1]);
    let seventh = find([10, 11])
        .or_else(|| chord.iter().copied().find(|&n| n % 12 != third % 12))
        .unwrap_or(third.saturating_add(7));
    (third, seventh)
}

/// The pitch with class `pc` nearest to `around` (the 9th's realization).
fn nearest_with_pc(around: u8, pc: u8) -> u8 {
    let a = around as i16;
    for d in 0..=6i16 {
        for cand in [a - d, a + d] {
            if cand > 0 && cand.rem_euclid(12) as u8 == pc {
                return cand as u8;
            }
        }
    }
    around
}

/// The chord's tones realized across a register window (every octave
/// copy inside `[lo, hi]`).
fn chord_tones_in(chord: &[u8; 4], lo: u8, hi: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for &c in chord {
        let mut n = c;
        while n < lo {
            n += 12;
        }
        while n <= hi {
            out.push(n);
            n += 12;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Nearest member of `pool` to `target` (ties resolve low — the mellower
/// choice).
fn nearest(pool: &[u8], target: u8) -> u8 {
    *pool
        .iter()
        .min_by_key(|&&n| ((n as i16 - target as i16).abs(), n))
        .expect("pool is never empty within the register windows")
}

/// Nearest in-scale note to `n` inside `[lo, hi]` — a diatonic scale has
/// no gap wider than a whole tone, so the outward search always lands
/// within ±2 of any pitch.
fn snap_to_scale(n: i16, scale: &[u8; 7], lo: u8, hi: u8) -> u8 {
    for d in 0..=6i16 {
        for cand in [n - d, n + d] {
            if cand >= lo as i16
                && cand <= hi as i16
                && scale.contains(&(cand.rem_euclid(12) as u8))
            {
                return cand as u8;
            }
        }
    }
    lo // unreachable for any 7-pc scale with hi-lo >= 12; safe fallback
}

/// Step `k` scale degrees from `note` (k may be negative), clamped to the
/// register window. Stays in-scale by construction.
fn scale_step(note: u8, k: i8, scale: &[u8; 7], lo: u8, hi: u8) -> u8 {
    // walk semitone-by-semitone counting scale members
    let mut n = note as i16;
    let mut remaining = k.abs();
    let dir = if k >= 0 { 1 } else { -1 };
    while remaining > 0 {
        n += dir;
        if n < lo as i16 || n > hi as i16 {
            n -= dir; // hit the clamp: stay
            break;
        }
        if scale.contains(&((n.rem_euclid(12)) as u8)) {
            remaining -= 1;
        }
    }
    snap_to_scale(n, scale, lo, hi)
}

// ------------------------------------------------------------- the lead

/// One melody event before humanization: (bar, beat-in-bar, note, vel).
type LeadEvent = (usize, f32, u8, f32);

/// The friendly beat grid a lead note may land on (0.0 is the kick's).
const LEAD_GRID: [f32; 6] = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0];

/// Rule-generated lead melody: two 4-bar phrases — statement, then a
/// varied answer with a peak — closing on a tone that resolves the loop.
fn lead_events(
    rng: &mut NoiseStream,
    bar_chords: &[[u8; 4]; 8],
    scale: &[u8; 7],
    mood: Mood,
) -> Vec<LeadEvent> {
    let (lo, hi, silent_bar_p, max_per_bar) = match mood {
        Mood::Day => (DAY_LEAD_LO, DAY_LEAD_HI, 0.0, 3usize),
        Mood::Night => (NIGHT_LEAD_LO, NIGHT_LEAD_HI, 0.5, 1usize),
    };

    // rhythm skeleton for bars 0-3
    let mut phrase_rhythm: Vec<Vec<f32>> = Vec::new();
    for _ in 0..4 {
        if chance(rng, silent_bar_p) {
            phrase_rhythm.push(Vec::new());
            continue;
        }
        let n = 1
            + usize::from(max_per_bar > 1 && chance(rng, 0.6))
            + usize::from(max_per_bar > 2 && chance(rng, 0.25));
        let mut beats: Vec<f32> = (0..n)
            .map(|_| LEAD_GRID[pick(rng, LEAD_GRID.len())])
            .collect();
        // the lofi push (R13): occasionally a note lands a 16th AHEAD of
        // a strong beat — the anticipation the straight-8th grid lacked.
        // (In the answer phrase, a ±0.5 mutation of an inherited push can
        // land on x.25 — the anticipated OFF-8th, same push family, legal
        // on the grid.)
        for b in beats.iter_mut() {
            if max_per_bar > 1 && b.fract() == 0.0 && chance(rng, 0.15) {
                *b -= 0.25;
            }
        }
        beats.sort_by(f32::total_cmp);
        beats.dedup_by(|a, b| (*a - *b).abs() < 0.75); // breathe: min gap
        phrase_rhythm.push(beats);
    }
    // a take with (almost) no lead has lost its singable identity —
    // night's 50% silent bars can conspire to empty the whole phrase
    // (0.5^4 ≈ 6% of seeds); guarantee a minimal statement instead
    if phrase_rhythm.iter().map(Vec::len).sum::<usize>() < 2 {
        phrase_rhythm[0] = vec![1.0];
        phrase_rhythm[2] = vec![2.0];
    }

    // phrase 2 rhythm: the answer varies the statement
    let mut rhythm: Vec<Vec<f32>> = phrase_rhythm.clone();
    for statement in &phrase_rhythm {
        let mut beats = statement.clone();
        for b in beats.iter_mut() {
            if chance(rng, 0.2) {
                *b = (*b + if chance(rng, 0.5) { 0.5 } else { -0.5 }).clamp(0.5, 3.0);
            }
            if max_per_bar > 1 && b.fract() == 0.0 && chance(rng, 0.15) {
                *b -= 0.25;
            }
        }
        if !beats.is_empty() && chance(rng, 0.10) {
            beats.remove(pick(rng, beats.len()));
        }
        beats.sort_by(f32::total_cmp);
        beats.dedup_by(|a, b| (*a - *b).abs() < 0.75);
        rhythm.push(beats);
    }

    // pitch walk
    let mut out: Vec<LeadEvent> = Vec::new();
    let start_pool = chord_tones_in(&bar_chords[0], lo.max(hi.saturating_sub(12)), hi);
    let mut prev = nearest(&start_pool, (lo + hi) / 2 + 2);
    let mut last_leap: i16 = 0;
    // bar 5: the answer's lift (bar 4 is reserved for the motif quote)
    let peak_bar = 5;
    let total_bars = rhythm.len();
    let mut statement_head: Vec<u8> = Vec::new();
    for (bar, beats) in rhythm.iter().enumerate() {
        let chord = bar_chords[bar];
        let tones = chord_tones_in(&chord, lo, hi);
        let n_beats = beats.len();
        for (i, &beat) in beats.iter().enumerate() {
            let is_last_event = bar == total_bars - 1 && i == n_beats - 1;
            // an anticipation (x.75) targets the COMING harmony's tone —
            // it is a pushed strong beat, not a passing tone
            let strong = beat.fract() == 0.0 || (beat.fract() - 0.75).abs() < 1e-3;
            let note = if bar == 4 && i < statement_head.len() && !is_last_event {
                // the answer QUOTES the statement's opening (call-and-
                // answer): same pitches, whatever rhythm bar 4 drew
                statement_head[i]
            } else if is_last_event {
                // close the loop: a bar-3 chord tone near the opening note
                let opening = out.first().map_or(prev, |&(_, _, n, _)| n);
                nearest(&tones, opening)
            } else if bar == peak_bar && i == 0 {
                // the peak: push up 3-5 semis, snapped to a chord tone
                let target = (prev as i16 + 3 + pick(rng, 3) as i16).min(hi as i16) as u8;
                nearest(&tones, target)
            } else if last_leap.abs() > 4 {
                // resolve a leap: stepwise contrary motion, in-scale
                let k = if last_leap > 0 { -1 } else { 1 };
                scale_step(prev, k, scale, lo, hi)
            } else if strong || chance(rng, 0.55) {
                // chord tone near the walk, leap bounded to a fifth
                let drift = pick(rng, 9) as i16 - 4;
                let target = (prev as i16 + drift).clamp(lo as i16, hi as i16) as u8;
                let n = nearest(&tones, target);
                if (n as i16 - prev as i16).abs() > 7 {
                    nearest(&tones, prev)
                } else {
                    n
                }
            } else {
                // diatonic passing step
                let k = if chance(rng, 0.5) { 1 } else { -1 };
                scale_step(prev, k * (1 + i8::from(chance(rng, 0.3))), scale, lo, hi)
            };
            last_leap = note as i16 - prev as i16;
            prev = note;
            if bar == 0 && statement_head.len() < 2 {
                statement_head.push(note);
            }
            let base = match mood {
                Mood::Day => 0.34 + 0.10 * rng.unit(),
                Mood::Night => 0.25 + 0.12 * rng.unit(),
            };
            let vel = if bar == peak_bar && i == 0 {
                base + 0.06
            } else {
                base
            };
            out.push((bar, beat, note, vel));
        }
    }
    out
}

// ------------------------------------------------------------- the groove

struct GrooveTemplate {
    half_time: bool,
    kick2_even: f32,
    kick2_odd: f32,
    ghost_pickup: bool,
    hat_skip: f32,
    open_hat_bar_mod: Option<usize>,
    kick_gain: (f32, f32),
}

const DAY_GROOVES: [GrooveTemplate; 3] = [
    // boom-bap: backbeats on 2&4 (the Day2 ratified feel)
    GrooveTemplate {
        half_time: false,
        kick2_even: 2.25,
        kick2_odd: 2.5,
        ghost_pickup: true,
        hat_skip: 0.15,
        open_hat_bar_mod: Some(4),
        kick_gain: (1.0, 0.75),
    },
    // half-time: the lazy backbeat (the Day3 ratified feel)
    GrooveTemplate {
        half_time: true,
        kick2_even: 2.75,
        kick2_odd: 2.75,
        ghost_pickup: false,
        hat_skip: 0.30,
        open_hat_bar_mod: Some(4),
        kick_gain: (1.0, 0.7),
    },
    // sparse: quiet-focus (the E-candidate feel)
    GrooveTemplate {
        half_time: true,
        kick2_even: 2.5,
        kick2_odd: 2.5,
        ghost_pickup: false,
        hat_skip: 0.45,
        open_hat_bar_mod: None,
        kick_gain: (0.9, 0.6),
    },
];

fn swing_delay(s: f32, eighth_s: f32) -> f32 {
    (s - 0.5) * eighth_s
}

/// The day kit from a groove template — humanized boom-bap/half-time,
/// clamped non-negative (a bar-0 jitter can't leave the loop).
fn day_drums(
    rng: &mut NoiseStream,
    g: &GrooveTemplate,
    beat_s: f32,
) -> (Vec<(f32, DrumKind, f32)>, Vec<f32>) {
    let bar_s = beat_s * super::score::BEATS_PER_BAR;
    let eighth = beat_s / 2.0;
    let mut out = Vec::new();
    let mut kicks = Vec::new();
    for bar in 0..GEN_LOOP_BARS {
        let b0 = bar as f32 * bar_s;
        let wobble = 0.9 + 0.2 * rng.unit();
        let k2 = if bar % 2 == 0 {
            g.kick2_even
        } else {
            g.kick2_odd
        };
        let mut kick_beats = vec![(0.0, g.kick_gain.0), (k2, g.kick_gain.1)];
        if g.ghost_pickup && bar % 4 == 3 {
            kick_beats.push((3.75, 0.35));
        }
        for (at_beat, gain) in kick_beats {
            let mut at = b0 + at_beat * beat_s + DRAG_KICK_S;
            if (at_beat * 2.0) % 2.0 != 0.0 {
                at += swing_delay(SWING_KICK, eighth);
            }
            let jit = (rng.unit() - 0.3) * 0.010;
            let at = (at + jit).max(0.0);
            out.push((
                at,
                DrumKind::Kick,
                gain * wobble * (0.95 + 0.1 * rng.unit()),
            ));
            kicks.push(at);
        }
        let snare_beats: &[f32] = if g.half_time { &[2.0] } else { &[1.0, 3.0] };
        for &sb in snare_beats {
            let at = b0 + sb * beat_s + 0.008 + (rng.unit() - 0.3) * 0.008;
            out.push((
                at.max(0.0),
                DrumKind::Snare,
                0.85 * wobble * (0.95 + 0.1 * rng.unit()),
            ));
        }
        for e in 0..8 {
            if chance(rng, g.hat_skip) {
                continue;
            }
            let mut at = b0 + e as f32 * eighth;
            if e % 2 == 1 {
                at += swing_delay(SWING_HATS_DAY, eighth);
            }
            at += (rng.unit() - 0.5) * 0.012;
            let open = g
                .open_hat_bar_mod
                .is_some_and(|m| e == 7 && bar % m == m - 1);
            let kind = if open {
                DrumKind::OpenHat
            } else {
                DrumKind::Hat
            };
            out.push((at.max(0.0), kind, (0.4 + 0.2 * rng.unit()) * wobble));
        }
    }
    (out, kicks)
}

/// The night groove: kick + soft closed hats only (the sleepy register),
/// the ratified night swing/drag values.
fn night_drums(rng: &mut NoiseStream, beat_s: f32) -> (Vec<(f32, DrumKind, f32)>, Vec<f32>) {
    let bar_s = beat_s * super::score::BEATS_PER_BAR;
    let eighth = beat_s / 2.0;
    let mut out = Vec::new();
    let mut kicks = Vec::new();
    for bar in 0..GEN_LOOP_BARS {
        let b0 = bar as f32 * bar_s;
        let wobble = 0.9 + 0.2 * rng.unit();
        for (at_beat, g) in [(0.0f32, 0.6f32), (2.5, 0.4)] {
            let mut at = b0 + at_beat * beat_s + DRAG_KICK_NIGHT_S;
            if (at_beat * 2.0) % 2.0 != 0.0 {
                at += swing_delay(SWING_KICK, eighth);
            }
            let jit = (rng.unit() - 0.3) * 0.010;
            let at = (at + jit).max(0.0);
            out.push((at, DrumKind::Kick, g * wobble * (0.95 + 0.1 * rng.unit())));
            kicks.push(at);
        }
        for e in 0..8 {
            if e % 2 == 0 || chance(rng, 0.45) {
                continue;
            }
            let mut at = b0 + e as f32 * eighth + swing_delay(SWING_HATS_NIGHT, eighth);
            at += (rng.unit() - 0.5) * 0.012;
            // ×2 vs v1 (owner-adopted articulation fix: hats measured
            // ~50dB under the sub — an inaudible tick, no groove)
            out.push((
                at.max(0.0),
                DrumKind::Hat,
                (0.40 + 0.24 * rng.unit()) * wobble,
            ));
        }
    }
    (out, kicks)
}

// ------------------------------------------------------------- keys

fn keys_events(
    rng: &mut NoiseStream,
    bar_chords: &[[u8; 4]; 8],
    bar_roots: &[u8; 8],
    beat_s: f32,
    mood: Mood,
) -> Vec<(f32, u8, f32)> {
    let bar_s = beat_s * super::score::BEATS_PER_BAR;
    let eighth = beat_s / 2.0;
    let (density, vel_base, vel_span) = match mood {
        Mood::Day => (range_f(rng, 0.28, 0.38), 0.42, 0.30),
        Mood::Night => (0.15, 0.35, 0.25),
    };
    let mut out = Vec::new();
    for bar in 0..GEN_LOOP_BARS {
        let chord = bar_chords[bar];
        for e in 0..8 {
            if !chance(rng, density) {
                continue;
            }
            let mut at = bar as f32 * bar_s + e as f32 * eighth;
            if e % 2 == 1 {
                at += swing_delay(SWING_KICK, eighth);
            }
            at += 0.010 + 0.020 * rng.unit();
            match mood {
                Mood::Day => {
                    // COMPING means chords: a rolled rootless SHELL
                    // (3rd+7th; sometimes the 9th replaces the 7th, R6)
                    // — the pad/bass own the low end, so two voices keep
                    // the 250-500Hz band controlled while reading as a
                    // real EP comp instead of a music box
                    let (third, seventh) = shell_of(&chord, bar_roots[bar]);
                    let upper = if chance(rng, 0.2) {
                        nearest_with_pc(seventh, (bar_roots[bar] + 2) % 12)
                    } else {
                        seventh
                    };
                    let vel = vel_base + vel_span * rng.unit();
                    out.push((at, third, vel));
                    // a hand rolls the dyad; a sequencer stamps it
                    out.push((at + 0.012 + 0.008 * rng.unit(), upper, vel * 0.9));
                }
                Mood::Night => {
                    // night stays single sparse tones — the sleepy
                    // register must not thicken
                    let note = chord[pick(rng, chord.len())];
                    out.push((at, note, vel_base + vel_span * rng.unit()));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------- entry point

/// Compose one full take from a seed — deterministic, allocation-only
/// (no clock, no I/O): the pure composer both painters and every test
/// share. Synthesis lives in `synth::gen_beds`.
pub fn compose(mood: Mood, seed: u64) -> GeneratedScore {
    let mut rng = NoiseStream::new(pixtuoid_core::id::splitmix64(seed ^ 0x10F1_C0DE));

    let (progs, bpm_win): (&[Progression], (f32, f32)) = match mood {
        Mood::Day => (&DAY_PROGRESSIONS, DAY_BPM),
        Mood::Night => (&NIGHT_PROGRESSIONS, NIGHT_BPM),
    };
    let prog = &progs[pick(&mut rng, progs.len())];
    let t = TRANSPOSE_MIN + pick(&mut rng, (TRANSPOSE_MAX - TRANSPOSE_MIN + 1) as usize) as i8;
    let (chords, roots_pc, scale_pcs) = transpose(prog, t);
    let (bar_chords, bar_roots) = timeline(&chords, &roots_pc, mood);
    let bpm = range_f(&mut rng, bpm_win.0, bpm_win.1).round();
    let beat_s = 60.0 / bpm;
    let bar_s = beat_s * super::score::BEATS_PER_BAR;

    // lead: rule-walked melody, humanized played-not-sequenced lag
    let lead = lead_events(&mut rng, &bar_chords, &scale_pcs, mood);
    let sparkle: Vec<(f32, u8, f32)> = lead
        .into_iter()
        .map(|(bar, beat, note, vel)| {
            let at = bar as f32 * bar_s + beat * beat_s + 0.008 + 0.014 * rng.unit();
            (at, note, vel * (0.92 + 0.16 * rng.unit()))
        })
        .collect();

    let keys = keys_events(&mut rng, &bar_chords, &bar_roots, beat_s, mood);

    let (drums, kicks) = match mood {
        Mood::Day => {
            let g = &DAY_GROOVES[pick(&mut rng, DAY_GROOVES.len())];
            day_drums(&mut rng, g, beat_s)
        }
        Mood::Night => night_drums(&mut rng, beat_s),
    };

    let mut bass_roots = [0u8; 4];
    for (i, &pc) in roots_pc.iter().enumerate() {
        // place the root pc in the ratified sub window (26..=38)
        let mut b = 24 + pc;
        while b < 26 {
            b += 12;
        }
        while b > 38 {
            b -= 12;
        }
        bass_roots[i] = b;
    }

    // drawn AFTER every musical draw, so a voice-registry change can
    // never silently recompose an already-blessed seed's notes. Night
    // keeps the ratified EP (the sleepy identity); day draws variety.
    let lead_voice = if mood == Mood::Day && chance(&mut rng, 0.35) {
        LeadVoice::Pluck
    } else {
        LeadVoice::EpVel
    };

    GeneratedScore {
        mood,
        bpm,
        chords,
        scale_pcs,
        sparkle,
        keys,
        drums,
        bass_roots,
        kick_times: if mood == Mood::Night {
            kicks
        } else {
            Vec::new()
        },
        lead_voice,
        bar_chords,
        bar_roots,
        roots_pc,
    }
}

#[cfg(test)]
mod tests;
