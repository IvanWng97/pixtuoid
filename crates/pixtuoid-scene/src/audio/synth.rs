//! The procedural sound recipes — a 1:1 Rust port of the OWNER-RATIFIED
//! Phase 0 audition prototype (docs/superpowers/specs/
//! 2026-07-16-ambient-sound-phase0/synth_audition.py, #633). Every constant
//! is a measured/ratified value from that spec; change them there first
//! (re-audition), then mirror here. All functions are PURE sample-buffer
//! generators (mono f32 @ 44_100 Hz) run once at startup — playback never
//! synthesizes.

use super::dsp::{bandpass, lowpass, shaped_noise_loop, NoiseStream, SAMPLE_RATE};

const SR: f32 = SAMPLE_RATE as f32;

fn n_samples(dur_s: f32) -> usize {
    (dur_s * SR).round() as usize
}

/// Linear attack/release envelope over `n` samples.
fn env_ar(n: usize, attack_s: f32, release_s: f32) -> impl Fn(usize) -> f32 {
    let na = n_samples(attack_s).min(n).max(1);
    let nr = n_samples(release_s).min(n).max(1);
    move |i| {
        let a = (i as f32 / na as f32).min(1.0);
        let r = ((n - i) as f32 / nr as f32).min(1.0);
        a.min(r)
    }
}

/// Overlap-add `snippet` into `buf` starting at `at_s`, clipped to `buf`.
fn place(buf: &mut [f32], snippet: &[f32], at_s: f32, gain: f32) {
    let start = (at_s * SR) as usize;
    for (i, &s) in snippet.iter().enumerate() {
        if let Some(slot) = buf.get_mut(start + i) {
            *slot += s * gain;
        }
    }
}

fn normalize(buf: &mut [f32], peak: f32) {
    let m = buf.iter().fold(0.0f32, |a, &v| a.max(v.abs())).max(1e-9);
    buf.iter_mut().for_each(|v| *v *= peak / m);
}

// ---------------------------------------------------------------- keystroke

/// One mechanical-keyboard stroke — the v4 BRIGHT office clack matched to
/// the owner's reference (yt 2BUNHd7ENZk): 82.6% of energy in 1-4kHz,
/// centroid ~2.4kHz, ~8ms decay, soft up-stroke tick 55-85ms later. (The
/// ASMR-lore deep thock measured OPPOSITE to the reference — see the spec's
/// "reference outranks the literature" note.)
pub fn keystroke(rng: &mut NoiseStream) -> Vec<f32> {
    let d = 0.05;
    let n = n_samples(d);
    // main click: tight noise burst, band 1250-3700Hz, fast decay
    let f_lo = 1250.0 + 450.0 * rng.unit();
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let click = bandpass(&raw, f_lo, f_lo + 2100.0);
    // body: the 1-2kHz substance under the click
    let f_b = 1000.0 + 400.0 * rng.unit();
    let raw_b: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let body = bandpass(&raw_b, f_b, f_b + 900.0);
    // spice: a whisper above 4k so the top octave reads natural
    let raw_s: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let spice = bandpass(&raw_s, 4200.0, 7500.0);

    let up_at = 0.055 + 0.03 * rng.unit();
    let du = 0.02;
    let nu = n_samples(du);
    let raw_u: Vec<f32> = (0..nu).map(|_| rng.norm()).collect();
    let up = bandpass(&raw_u, 2000.0, 4500.0);

    let total = n_samples(up_at) + nu + 8;
    let mut buf = vec![0.0f32; total];
    for i in 0..n {
        let t = i as f32 / SR;
        buf[i] = click[i] * (-t * 330.0).exp()
            + body[i] * (-t * 280.0).exp() * 1.1
            + spice[i] * (-t * 500.0).exp() * 0.18;
    }
    let up_scaled: Vec<f32> = up
        .iter()
        .enumerate()
        .map(|(i, &v)| v * (-(i as f32 / SR) * 520.0).exp())
        .collect();
    place(&mut buf, &up_scaled, up_at, 0.35);
    normalize(&mut buf, 0.8);
    buf
}

// ------------------------------------------------------------------- dings

const MIDI_A4: f32 = 440.0;
fn midi_freq(m: f32) -> f32 {
    MIDI_A4 * 2f32.powf((m - 69.0) / 12.0)
}

/// Door chime — a DESCENDING ding-dong (E5 → C5), warm harmonic bells with
/// slow decay (centroid ~556Hz, the ratified warm-bell character — pinned
/// by the spectral test below).
pub fn door_chime() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(2.0)];
    let tau = std::f32::consts::TAU;
    // (onset s, midi note, gain): E5 then C5, the falling pair
    for &(at, m, g) in &[(0.0f32, 76.0f32, 0.8f32), (0.42, 72.0, 1.0)] {
        let d = 1.5;
        let nn = n_samples(d);
        let f = midi_freq(m);
        let note: Vec<f32> = (0..nn)
            .map(|i| {
                let t = i as f32 / SR;
                (tau * f * t).sin() * (-t * 2.6).exp()
                    + 0.35 * (tau * 2.0 * f * t).sin() * (-t * 5.0).exp()
                    + 0.10 * (tau * 3.0 * f * t).sin() * (-t * 9.0).exp()
            })
            .collect();
        place(&mut buf, &note, at, g * 0.7);
    }
    let mut out = lowpass(&buf, 4000.0);
    normalize(&mut out, 0.55);
    out
}

// -------------------------------------------------------------- appliances

/// Office laser printer: motor spin-UP (80→130Hz with harmonics),
/// quasi-regular feed-roller ticks, a paper-slide swoosh, spin-down tail.
pub fn printer_whir(rng: &mut NoiseStream) -> Vec<f32> {
    let dur = 1.5;
    let n = n_samples(dur);
    let mut buf = vec![0.0f32; n];
    let env = env_ar(n, 0.12, 0.3);
    let env_tex = env_ar(n, 0.15, 0.35);
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let texture = bandpass(&raw, 400.0, 2600.0);
    let tau = std::f32::consts::TAU;
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / SR;
        // motor pitch: ramp up over 0.25s, sag from 1.15s
        let f =
            80.0 + 50.0 * (t / 0.25).clamp(0.0, 1.0) - 30.0 * ((t - 1.15) / 0.35).clamp(0.0, 1.0);
        phase += tau * f / SR;
        let motor = phase.sin() + 0.45 * (2.0 * phase).sin() + 0.2 * (3.0 * phase).sin();
        buf[i] = motor * env(i) * 0.5 + texture[i] * env_tex(i) * 0.16;
    }
    // feed-roller ticks, ~11/s with jitter through the feed window
    let mut at = 0.28f32;
    while at < 1.05 {
        let dn = n_samples(0.014);
        let raw_t: Vec<f32> = (0..dn).map(|_| rng.norm()).collect();
        let tick: Vec<f32> = bandpass(&raw_t, 1500.0, 4200.0)
            .iter()
            .enumerate()
            .map(|(i, &v)| v * (-(i as f32 / SR) * 260.0).exp())
            .collect();
        place(&mut buf, &tick, at, 0.5 + 0.2 * rng.unit());
        at += 0.09 + 0.02 * rng.unit();
    }
    // paper slide: a swoosh through the middle
    let sw_n = n_samples(0.5);
    let raw_s: Vec<f32> = (0..sw_n).map(|_| rng.norm()).collect();
    let sw: Vec<f32> = bandpass(&raw_s, 900.0, 3000.0)
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let w = (std::f32::consts::PI * i as f32 / sw_n as f32).sin();
            v * w * w * 0.35
        })
        .collect();
    place(&mut buf, &sw, 0.55, 1.0);
    let mut out = lowpass(&buf, 5000.0);
    normalize(&mut out, 0.55);
    out
}

/// Vending machine: mechanism click → beat → the can DROPS (meaty low thud
/// plus two smaller settle bounces). The ONE unit with no Phase 0 audition;
/// flagged for the LISTEN gate.
pub fn vending_drop(rng: &mut NoiseStream) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(0.7)];
    // mechanism click
    let cn = n_samples(0.03);
    let raw_c: Vec<f32> = (0..cn).map(|_| rng.norm()).collect();
    let click: Vec<f32> = bandpass(&raw_c, 800.0, 2000.0)
        .iter()
        .enumerate()
        .map(|(i, &v)| v * (-(i as f32 / SR) * 200.0).exp())
        .collect();
    place(&mut buf, &click, 0.0, 0.5);
    // the can: thud + rattle, then two settle bounces
    let tau = std::f32::consts::TAU;
    let thud = |rng: &mut NoiseStream| -> Vec<f32> {
        let dn = n_samples(0.12);
        let raw: Vec<f32> = (0..dn).map(|_| rng.norm()).collect();
        let rattle = bandpass(&raw, 300.0, 900.0);
        (0..dn)
            .map(|i| {
                let t = i as f32 / SR;
                (tau * 170.0 * t).sin() * (-t * 70.0).exp() + rattle[i] * (-t * 150.0).exp() * 0.4
            })
            .collect()
    };
    let b0 = thud(rng);
    let b1 = thud(rng);
    let b2 = thud(rng);
    place(&mut buf, &b0, 0.18, 1.0);
    place(&mut buf, &b1, 0.27, 0.5);
    place(&mut buf, &b2, 0.33, 0.25);
    let mut out = lowpass(&buf, 3500.0);
    normalize(&mut out, 0.55);
    out
}

// -------------------------------------------------------------------- beds

/// The gentle-rain octave-band envelope, measured from the owner's chosen
/// reference (yt 42M3esYyHdw live capture, 2026-07-16): energy lives
/// 500Hz-2k, rumble <12%, real 2-8k air. `(lo_hz, hi_hz, energy %)`.
const GENTLE_RAIN_BANDS: [(f32, f32, f32); 9] = [
    (20.0, 60.0, 1.5),
    (60.0, 125.0, 3.5),
    (125.0, 250.0, 6.9),
    (250.0, 500.0, 14.3),
    (500.0, 1000.0, 24.4),
    (1000.0, 2000.0, 25.3),
    (2000.0, 4000.0, 10.2),
    (4000.0, 8000.0, 8.9),
    (8000.0, 16000.0, 4.0),
];

/// Seamless-loop length for the noise beds: 2^19 samples ≈ 11.9s — the
/// FFT-domain shaping is circular, so the block loops click-free.
const BED_LOOP_SAMPLES: usize = 1 << 19;

/// The rain WASH (bed only — audible foreground drops are scattered at
/// runtime by the mixer from [`rain_drop`]'s pool, so rain never repeats).
pub fn rain_bed(rng: &mut NoiseStream) -> Vec<f32> {
    shaped_noise_loop(BED_LOOP_SAMPLES, &GENTLE_RAIN_BANDS, rng)
}

/// One audible raindrop for the runtime scatter pool. Three surface
/// populations (measured ~640-1730Hz centroid spread): dull plop on
/// wood/soil, water plip (the classic Minnaert chirp), bright ping on
/// metal/glass — weights 20/55/25 (gentle rain skews away from dull thuds).
pub fn rain_drop(rng: &mut NoiseStream) -> Vec<f32> {
    let d = 0.10;
    let n = n_samples(d);
    let kind = rng.unit();
    // (f0 range, decay, splash gain, splash band)
    let (f0, decay, spl_gain, spl_lo, spl_hi) = if kind < 0.20 {
        (320.0 + 300.0 * rng.unit(), 55.0, 0.12, 900.0, 2200.0)
    } else if kind < 0.75 {
        (700.0 + 700.0 * rng.unit(), 62.0, 0.25, 1200.0, 4000.0)
    } else {
        (1800.0 + 1200.0 * rng.unit(), 80.0, 0.15, 2500.0, 6000.0)
    };
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let splash = bandpass(&raw, spl_lo, spl_hi);
    let tau = std::f32::consts::TAU;
    let mut phase = 0.0f32;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let f = f0 * (1.0 + 0.12 * t / d); // the rising Minnaert chirp
            phase += tau * f / SR;
            phase.sin() * (-t * decay).exp() + splash[i] * (-t * 180.0).exp() * spl_gain
        })
        .collect();
    normalize(&mut buf, 1.0);
    buf
}

/// The vinyl/room texture bed: tape hiss + a faint warm room hum + sparse
/// soft crackle. Mixed 25-35dB below the music (the noise-floor rule); the
/// per-stem hiss stacking bug lives in the spec's cautionary tales. Its
/// crackle is per-boot random ON PURPOSE (unpitched — variation is a
/// feature); everything melodic is frozen in `score`. Re-wired in Phase 2
/// alongside the music it textures (the Phase 1 owner call: no floor noise
/// without music).
pub fn texture_bed(rng: &mut NoiseStream) -> Vec<f32> {
    let n = BED_LOOP_SAMPLES;
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let hiss = lowpass(&raw, 3800.0);
    let tau = std::f32::consts::TAU;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let room = (tau * 90.0 * t).sin() * (1.0 + 0.2 * (tau * 0.4 * t).sin()) * 0.006;
            hiss[i] * 0.010 + room
        })
        .collect();
    // sparse soft crackle: ~4 pops/s
    let n_pops = (n as f32 / SR * 4.0) as usize;
    for _ in 0..n_pops {
        let at = rng.unit() * (n as f32 / SR - 0.01);
        let pn = n_samples(0.003 + 0.004 * rng.unit());
        let pop: Vec<f32> = (0..pn)
            .map(|i| rng.norm() * (-(i as f32 / SR) * 800.0).exp())
            .collect();
        place(&mut buf, &pop, at, 0.03 + 0.06 * rng.unit());
    }
    normalize(&mut buf, 0.45);
    buf
}

// ------------------------------------------------- musical stems (Phase 2)
// The ratified 8-bar lofi composition (#633) — the frozen `score` tables
// realized 1:1 from the Phase 0 python (`p3_*` renders, owner LISTEN-passed
// 2026-07-16). All-procedural by owner decision: no open model emits 4
// semantic phase-locked seamless-loop stems, and the ratified sound IS this
// synthesis. Fingerprint pins below anchor each stem to the p3 measurement.

use super::score;

fn beat_s() -> f32 {
    score::beat_s()
}

fn bar_s() -> f32 {
    score::beat_s() * score::BEATS_PER_BAR
}

/// The lofi master chain, applied per MUSICAL stem (texture/rain skip it —
/// they ARE the medium): tape wow+flutter warp, tanh saturation (highs
/// soften first), an 80-120Hz head bump, gentle HF rolloff. No hiss here —
/// per-stem hiss STACKS (+6dB over 4 stems); the medium noise is
/// `texture_bed`'s job, exactly once.
fn lofi_post(buf: &[f32], drive: f32) -> Vec<f32> {
    let warped = crate::audio::dsp::warp_resample(buf, &[(0.7, 0.0025), (8.0, 0.0006)]);
    let t = drive.tanh();
    let sat: Vec<f32> = warped.iter().map(|&x| (x * drive).tanh() / t).collect();
    let bump = bandpass(&sat, 80.0, 120.0);
    let bumped: Vec<f32> = sat.iter().zip(&bump).map(|(&x, &b)| x + 0.35 * b).collect();
    lowpass(&bumped, 6500.0)
}

/// The shared musical-stem mastering: the lofi tape chain then peak
/// normalize. ONE site so a stem can't silently skip the ratified post
/// (its saturation/bump/rolloff signatures are pinned by
/// `lofi_post_saturates_bumps_and_rolls_off`).
fn master(buf: &[f32], drive: f32, peak: f32) -> Vec<f32> {
    let mut out = lofi_post(buf, drive);
    normalize(&mut out, peak);
    out
}

/// One soft electric-piano note — the shared voice core; `h2` is the
/// 2nd-harmonic weight (the one axis the day/night voices differ on).
fn ep_pluck_h2(midi: u8, dur_s: f32, vel: f32, h2: f32) -> Vec<f32> {
    let n = n_samples(dur_s);
    let f = midi_freq(midi as f32);
    let tau = std::f32::consts::TAU;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let sig = (tau * f * t).sin() * (-t * 5.5).exp()
                + h2 * (tau * 2.0 * f * t).sin() * (-t * 9.0).exp()
                + 0.12 * (tau * 3.01 * f * t).sin() * (-t * 14.0).exp();
            sig * vel
        })
        .collect()
}

/// The day EP voice — the ratified FIXED 2nd harmonic (identity guarded
/// by the day fingerprint pins).
fn ep_pluck(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    ep_pluck_h2(midi, dur_s, vel, 0.35)
}

fn kick(rng: &mut NoiseStream) -> Vec<f32> {
    let n = n_samples(0.32);
    let tau = std::f32::consts::TAU;
    let mut phase = 0.0f32;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let f = 110.0 * (-t * 9.0).exp() + 42.0;
            phase += tau * f / SR;
            phase.sin() * (-t * 11.0).exp()
        })
        .collect();
    for (i, slot) in buf.iter_mut().enumerate().take(n_samples(0.006)) {
        let t = i as f32 / SR;
        *slot += rng.norm() * 0.25 * (-t * 300.0).exp();
    }
    buf
}

fn snare(rng: &mut NoiseStream) -> Vec<f32> {
    let n = n_samples(0.22);
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let noise = bandpass(&raw, 400.0, 3200.0);
    let tau = std::f32::consts::TAU;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let tone = (tau * 185.0 * t).sin() * (-t * 25.0).exp() * 0.5;
            (noise[i] * (-t * 22.0).exp() + tone) * 0.8
        })
        .collect()
}

fn hat(rng: &mut NoiseStream, open: bool) -> Vec<f32> {
    let (dur, decay) = if open { (0.35, 9.0) } else { (0.06, 60.0) };
    let n = n_samples(dur);
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let hp = crate::audio::dsp::highpass(&raw, 6000.0);
    (0..n)
        .map(|i| hp[i] * (-(i as f32 / SR) * decay).exp() * 0.5)
        .collect()
}

/// Warm EP-chord bed at pitch — the "someone left the radio on" floor.
pub fn stem_pad() -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for bar in 0..score::LOOP_BARS {
        let chord = score::chord_at_bar(bar);
        // the held chord rings past the bar; the LAST bar's tail is clipped
        // by place() — part of the ratified render, don't wrap it
        let dur = bar_s() + 0.9;
        let nd = n_samples(dur);
        let mut chord_sig = vec![0.0f32; nd];
        for (i, &m) in chord.iter().enumerate() {
            let f = midi_freq(m as f32);
            let env = env_ar(nd, 0.25 + 0.08 * i as f32, 1.2);
            for (j, slot) in chord_sig.iter_mut().enumerate() {
                let t = j as f32 / SR;
                let tone = (tau * f * t).sin()
                    + 0.30 * (tau * 2.0 * f * t).sin()
                    + 0.08 * (tau * 3.0 * f * t).sin();
                *slot += tone * env(j);
            }
        }
        place(&mut buf, &chord_sig, bar as f32 * bar_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 2600.0);
    for (i, v) in buf.iter_mut().enumerate() {
        *v *= 1.0 + 0.05 * (tau * 0.22 * i as f32 / SR).sin();
    }
    normalize(&mut buf, 0.7);
    master(&buf, 1.6, 0.7)
}

/// Sparse high EP notes over the pad — the empty-office humanity layer.
pub fn stem_sparkle() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for &(beats, note, vel) in &score::SPARKLE_SCORE {
        place(&mut buf, &ep_pluck(note, 1.6, vel), beats * beat_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 3200.0);
    normalize(&mut buf, 0.6);
    master(&buf, 1.6, 0.6)
}

/// The swung mid-register EP comping that joins at moderate busy-ness.
pub fn stem_keys() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for &(beats, note, vel) in &score::KEYS_SCORE {
        place(&mut buf, &ep_pluck(note, 0.9, vel), beats * beat_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 2400.0);
    normalize(&mut buf, 0.8);
    master(&buf, 1.6, 0.8)
}

/// Kick/snare/swung-hat groove — the busy-office layer. Hat velocities are
/// the frozen score's; each hit's NOISE content is fresh per call (rng),
/// matching the python render's per-bar draws.
pub fn stem_drums(rng: &mut NoiseStream) -> Vec<f32> {
    let swing = 0.10 * beat_s();
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for bar in 0..score::LOOP_BARS {
        let b0 = bar as f32 * bar_s();
        place(&mut buf, &kick(rng), b0, 1.0);
        place(&mut buf, &kick(rng), b0 + 2.5 * beat_s(), 0.8);
        place(&mut buf, &snare(rng), b0 + 2.0 * beat_s(), 1.0);
        for eighth in 0..8 {
            let at =
                b0 + eighth as f32 * (beat_s() / 2.0) + if eighth % 2 == 1 { swing } else { 0.0 };
            let open = eighth == 7 && bar % 2 == 1;
            let vel = score::DRUM_HAT_VELS[bar * 8 + eighth];
            place(&mut buf, &hat(rng, open), at, vel);
        }
    }
    let mut buf = lowpass(&buf, 7500.0); // lofi: shave the top
    normalize(&mut buf, 0.85);
    master(&buf, 2.2, 0.85)
}

// ------------------------------------------------- night track (#644)
// The v4 realization, owner LISTEN-passed 2026-07-17 — Lofi Girl-anchored
// (sub-bass floor, differential swing baked into score.rs timestamps).
// Day fns above are UNTOUCHED: the day take's identity is guarded by its
// fingerprint pins, which this addition must not move.

/// The velocity-keyed 2nd-harmonic EP voice (the Rhodes "bell vs bark" —
/// LOFI-BIBLE.md delta 4), shared by the night events AND the day takes'
/// sparkle/keys. The ORIGINAL day take keeps its ratified fixed-harmonic
/// [`ep_pluck`].
fn ep_pluck_vel(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    ep_pluck_h2(midi, dur_s, vel, 0.18 + 0.45 * vel * vel)
}

fn night_bar_s() -> f32 {
    score::night_beat_s() * score::BEATS_PER_BAR
}

/// Night pad: soft slow chords + the SUB-BASS floor in one buffer (the
/// harmonic floor moves as one — no new LoopStem, no scene change).
pub fn night_pad() -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(score::night_loop_secs())];
    for bar in 0..score::NIGHT_LOOP_BARS {
        let (chord, root) = score::night_chord_at_bar(bar);
        let dur = night_bar_s() + 1.2;
        let nd = n_samples(dur);
        let mut sig = vec![0.0f32; nd];
        for (i, &m) in chord.iter().enumerate() {
            let f = midi_freq(m as f32);
            let env = env_ar(nd, 0.5 + 0.1 * i as f32, 1.8);
            for (j, slot) in sig.iter_mut().enumerate() {
                let t = j as f32 / SR;
                let tone = (tau * f * t).sin()
                    + 0.22 * (tau * 2.0 * f * t).sin()
                    + 0.05 * (tau * 3.0 * f * t).sin();
                *slot += tone * env(j) * 0.8;
            }
        }
        // the sub floor: half-note pulses, the second softer (a breath)
        let fb = midi_freq(root as f32);
        for half in 0..2 {
            let hdur = night_bar_s() / 2.0 + 0.4;
            let hn = n_samples(hdur);
            let env = env_ar(hn, 0.06, 0.9);
            let g = if half == 0 { 1.0 } else { 0.7 };
            let bass: Vec<f32> = (0..hn)
                .map(|i| {
                    let t = i as f32 / SR;
                    ((tau * fb * t).sin() + 0.15 * (tau * 2.0 * fb * t).sin()) * env(i) * g * 3.2
                })
                .collect();
            place(&mut sig, &bass, half as f32 * night_bar_s() / 2.0, 1.0);
        }
        place(&mut buf, &sig, bar as f32 * night_bar_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 2200.0);
    for (i, v) in buf.iter_mut().enumerate() {
        *v *= 1.0 + 0.04 * (tau * 0.15 * i as f32 / SR).sin();
    }
    normalize(&mut buf, 0.75);
    master(&buf, 1.6, 0.75)
}

fn night_events_stem(events: &[(f32, u8, f32)], dur_s: f32, cutoff_hz: f32, peak: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::night_loop_secs())];
    for &(at, note, vel) in events {
        place(&mut buf, &ep_pluck_vel(note, dur_s, vel), at, 1.0);
    }
    let mut buf = lowpass(&buf, cutoff_hz);
    normalize(&mut buf, peak);
    master(&buf, 1.6, peak)
}

pub fn night_keys() -> Vec<f32> {
    night_events_stem(&score::NIGHT_KEYS, 1.1, 2000.0, 0.7)
}

pub fn night_sparkle() -> Vec<f32> {
    night_events_stem(&score::NIGHT_SPARKLE, 2.2, 2800.0, 0.5)
}

/// One frozen drum event's hit buffer — the kind dispatch shared by every
/// event-table drum stem (night + the day takes). NOISE content is fresh
/// per call; only timing/gain are frozen.
fn drum_hit(rng: &mut NoiseStream, kind: score::DrumKind) -> Vec<f32> {
    match kind {
        score::DrumKind::Kick => kick(rng),
        score::DrumKind::Snare => snare(rng),
        score::DrumKind::Hat => hat(rng, false),
        score::DrumKind::OpenHat => hat(rng, true),
    }
}

/// Kick + soft closed hats only — timing and gains frozen in the score
/// (humanization baked); each hit's NOISE content is fresh per call.
pub fn night_drums(rng: &mut NoiseStream) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::night_loop_secs())];
    for &(at, kind, gain) in &score::NIGHT_DRUMS {
        let hit = drum_hit(rng, kind);
        place(&mut buf, &hit, at, gain);
    }
    let mut buf = lowpass(&buf, 6000.0);
    normalize(&mut buf, 0.8);
    master(&buf, 2.0, 0.8)
}

/// Length of the loop-seam splice on the night texture: its buffer is the
/// night loop length (NOT a power of two), so the FFT-shaped hiss is not
/// circularly seamless — a short equal-power crossfade closes the seam.
const NIGHT_TEXTURE_SPLICE_S: f32 = 0.03;

/// The night texture: the day room-tone recipe at the NIGHT loop length
/// with the kick-duck BAKED IN (LOFI-BIBLE delta 5) — pre-multiplying the
/// frozen NIGHT_KICK_TIMES envelope keeps it phase-locked with the drums
/// by construction, no runtime sidechain machinery. (The audition tiled a
/// longer free-running bed instead; the per-loop repetition difference is
/// inaudible-class quiet noise — re-verified at the LISTEN gate.)
pub fn night_texture(rng: &mut NoiseStream) -> Vec<f32> {
    let n = n_samples(score::night_loop_secs());
    let f = n_samples(NIGHT_TEXTURE_SPLICE_S);
    // synthesize f EXTRA samples past the loop end: the splice blends a
    // genuine CONTINUATION of the tail into the head, so the wrap
    // buf[n-1] -> buf[0] is adjacent samples of one stream (a head-only
    // crossfade of unrelated content leaves the discontinuity — the
    // review refuted the first attempt empirically)
    let total = n + f;
    let raw: Vec<f32> = (0..total).map(|_| rng.norm()).collect();
    let hiss = lowpass(&raw, 3800.0);
    let tau = std::f32::consts::TAU;
    let mut buf: Vec<f32> = (0..total)
        .map(|i| {
            let t = i as f32 / SR;
            let room = (tau * 90.0 * t).sin() * (1.0 + 0.2 * (tau * 0.4 * t).sin()) * 0.006;
            hiss[i] * 0.010 + room
        })
        .collect();
    let n_pops = (n as f32 / SR * 4.0) as usize;
    for _ in 0..n_pops {
        let at = rng.unit() * (n as f32 / SR - 0.01);
        let pn = n_samples(0.003 + 0.004 * rng.unit());
        let pop: Vec<f32> = (0..pn)
            .map(|i| rng.norm() * (-(i as f32 / SR) * 800.0).exp())
            .collect();
        place(&mut buf[..n], &pop, at, 0.03 + 0.06 * rng.unit());
    }
    // linear blend of CORRELATED content (the pre-roll continues the same
    // lowpassed stream + the same sines), so no equal-power law is needed
    for j in 0..f {
        let a = j as f32 / f as f32;
        buf[j] = buf[n + j] * (1.0 - a) + buf[j] * a;
    }
    buf.truncate(n);
    // normalize BEFORE the duck (the python reference ducks the tiled,
    // already-normalized bed — duck-then-normalize re-inflated the level
    // by up to the duck depth, a review catch)
    normalize(&mut buf, 0.45);
    let depth = 10f32.powf(-4.0 / 20.0);
    let rel = n_samples(0.15);
    for &kt in &score::NIGHT_KICK_TIMES {
        let i0 = n_samples(kt);
        for j in 0..rel {
            let Some(slot) = buf.get_mut(i0 + j) else {
                break;
            };
            let g = depth + (1.0 - depth) * (j as f32 / rel as f32);
            *slot *= g;
        }
    }
    buf
}

// ------------------------------------------------- day takes
// Additional day-mood compositions, owner LISTEN-passed 2026-07-19 — the
// SAME production chain as the ratified original day take (its pad recipe,
// kit, tape post), parameterized over the frozen `score::DayTake` tables;
// sparkle/keys ride the velocity-keyed EP voice (`ep_pluck_vel`). The
// original day fns above are UNTOUCHED: their identity stays guarded by
// the day fingerprint pins. Like night, a take's identity is its frozen
// score + spectral pins, not rng-stream byte equality.

/// The original day pad recipe (harmonic stack, staggered onsets, slow AM)
/// over a take's changes.
pub(super) fn day_take_pad(take: &score::DayTake) -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(take.loop_secs())];
    for bar in 0..score::DAY_TAKE_LOOP_BARS {
        let chord = take.chord_at_bar(bar);
        let dur = take.bar_s() + 0.9;
        let nd = n_samples(dur);
        let mut chord_sig = vec![0.0f32; nd];
        for (i, &m) in chord.iter().enumerate() {
            let f = midi_freq(m as f32);
            let env = env_ar(nd, 0.25 + 0.08 * i as f32, 1.2);
            for (j, slot) in chord_sig.iter_mut().enumerate() {
                let t = j as f32 / SR;
                let tone = (tau * f * t).sin()
                    + 0.30 * (tau * 2.0 * f * t).sin()
                    + 0.08 * (tau * 3.0 * f * t).sin();
                *slot += tone * env(j);
            }
        }
        place(&mut buf, &chord_sig, bar as f32 * take.bar_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 2600.0);
    for (i, v) in buf.iter_mut().enumerate() {
        *v *= 1.0 + 0.05 * (tau * 0.22 * i as f32 / SR).sin();
    }
    normalize(&mut buf, 0.7);
    master(&buf, 1.6, 0.7)
}

/// An at-seconds EP event table rendered at a take's loop length (the
/// day-take twin of `night_events_stem`).
fn day_take_events_stem(
    take: &score::DayTake,
    events: &[(f32, u8, f32)],
    dur_s: f32,
    cutoff_hz: f32,
    peak: f32,
) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(take.loop_secs())];
    for &(at, note, vel) in events {
        place(&mut buf, &ep_pluck_vel(note, dur_s, vel), at, 1.0);
    }
    let mut buf = lowpass(&buf, cutoff_hz);
    normalize(&mut buf, peak);
    master(&buf, 1.6, peak)
}

/// The hand-written lead melody — a day take's singable identity.
pub(super) fn day_take_sparkle(take: &score::DayTake) -> Vec<f32> {
    day_take_events_stem(take, take.sparkle, 2.0, 3200.0, 0.6)
}

/// RNG-comped mid-register EP, frozen with the v4 swing feel baked in.
pub(super) fn day_take_keys(take: &score::DayTake) -> Vec<f32> {
    day_take_events_stem(take, take.keys, 0.9, 2400.0, 0.8)
}

/// The full day kit (kick/snare/hats incl. the bar-4/8 open hat) from the
/// frozen event table; each hit's NOISE content is fresh per call.
pub(super) fn day_take_drums(take: &score::DayTake, rng: &mut NoiseStream) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(take.loop_secs())];
    for &(at, kind, gain) in take.drums {
        let hit = drum_hit(rng, kind);
        place(&mut buf, &hit, at, gain);
    }
    let mut buf = lowpass(&buf, 7500.0);
    normalize(&mut buf, 0.85);
    master(&buf, 2.2, 0.85)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::{band_energy_share, centroid_hz};

    #[test]
    fn one_shot_synth_spectral_sanity() {
        // the ratified characters can't silently regress — the Phase 0
        // fingerprint checks as unit pins (pure buffers, no playback)
        let mut rng = NoiseStream::new(42);
        let key = keystroke(&mut rng);
        let c_key = centroid_hz(&key);
        assert!(
            (1800.0..=3200.0).contains(&c_key),
            "keystroke centroid {c_key} outside the bright-clack band"
        );

        let chime = door_chime();
        let c_chime = centroid_hz(&chime);
        assert!(
            c_chime < 700.0,
            "door chime centroid {c_chime} must stay the warm low bell (~556Hz ratified)"
        );
    }

    #[test]
    fn musical_stems_match_the_ratified_p3_fingerprints() {
        // reference = the ratified p3 composition measured on the python
        // FLOAT chain (normalize∘lofi_post∘stem, the exact chain the buffers
        // replicate — NOT the wav files, whose write_wav adds a stereo
        // interleave + audition soft clip that once poisoned these numbers).
        // Centroid ±20%, shares ±0.10 absolute — wide enough for the
        // brickwall-vs-sinc filter difference, tight enough that a wrong
        // register/voicing/score fails.
        type Case = (&'static str, Vec<f32>, f32, &'static [(f32, f32, f32)]);
        let cases: [Case; 4] = [
            (
                "pad",
                stem_pad(),
                291.1,
                &[(125.0, 250.0, 0.438), (250.0, 500.0, 0.516)],
            ),
            (
                "sparkle",
                stem_sparkle(),
                607.9,
                &[(250.0, 500.0, 0.313), (500.0, 1000.0, 0.641)],
            ),
            (
                "keys",
                stem_keys(),
                350.1,
                &[(125.0, 250.0, 0.312), (250.0, 500.0, 0.542)],
            ),
            (
                "drums",
                stem_drums(&mut NoiseStream::new(4)),
                214.6,
                &[(62.5, 125.0, 0.519), (125.0, 250.0, 0.408)],
            ),
        ];
        for (name, buf, ref_centroid, bands) in cases {
            let c = centroid_hz(&buf);
            assert!(
                (c - ref_centroid).abs() <= ref_centroid * 0.20,
                "{name}: centroid {c:.1} vs ratified {ref_centroid}"
            );
            // drums get a wider band tolerance: their noise content is
            // fresh per build AND the brickwall-vs-sinc head-bump gap hits
            // the kick-dominant low end hardest (measured +0.11)
            let tol = if name == "drums" { 0.15 } else { 0.10 };
            for &(lo, hi, ref_share) in bands {
                let s = band_energy_share(&buf, lo, hi);
                assert!(
                    (s - ref_share).abs() <= tol,
                    "{name} band {lo}-{hi}: {s:.3} vs ratified {ref_share}"
                );
            }
        }
        // the kick-dominant signature: low bands strictly descending
        let drums = stem_drums(&mut NoiseStream::new(4));
        let low = band_energy_share(&drums, 62.5, 125.0);
        let mid = band_energy_share(&drums, 125.0, 250.0);
        let high = band_energy_share(&drums, 250.0, 500.0);
        assert!(
            low > mid && mid > high,
            "drums must stay kick-dominant: {low:.3} > {mid:.3} > {high:.3}"
        );
        // the hi-hat layer is the ONLY content in 3.5-6.5k (kick/snare live
        // below 3.2k): measured 0.0043 with hats vs 0.0003 with the hat
        // loop deleted — the pin the octave-band tolerances can't provide
        // (review finding: the groove was invisible to the coarse bands)
        let hats = band_energy_share(&drums, 3500.0, 6500.0);
        assert!(
            hats > 0.0015,
            "the swung-hat groove must be audible in 3.5-6.5k: {hats:.5}"
        );
    }

    #[test]
    fn lofi_post_saturates_bumps_and_rolls_off() {
        // the tape chain's three audible signatures, pinned directly on the
        // pure fn (the stems bake it in via `master`, where a per-stem drop
        // is a single-site diff — review finding: the octave-band pins
        // alone couldn't see a dropped lofi_post):
        let tau = std::f32::consts::TAU;
        // 1) tanh saturation writes odd harmonics: a clean 440 tone gains
        //    a visible 3rd harmonic (~1320Hz)
        let tone: Vec<f32> = (0..n_samples(2.0))
            .map(|i| (tau * 440.0 * i as f32 / SR).sin() * 0.8)
            .collect();
        let posted = lofi_post(&tone, 1.6);
        let third = band_energy_share(&posted, 1200.0, 1450.0);
        assert!(
            third > 0.005,
            "tanh drive must write a 3rd harmonic: {third:.4}"
        );
        // 2) the 80-120Hz head bump lifts that band on broadband material
        let mut rng = NoiseStream::new(21);
        let noise: Vec<f32> = (0..n_samples(2.0)).map(|_| rng.norm() * 0.3).collect();
        let posted_n = lofi_post(&noise, 1.6);
        let bump_in = band_energy_share(&noise, 80.0, 120.0);
        let bump_out = band_energy_share(&posted_n, 80.0, 120.0);
        assert!(
            bump_out > bump_in * 1.3,
            "head bump must lift 80-120Hz: {bump_in:.4} -> {bump_out:.4}"
        );
        // 3) the top end rolls off at 6.5k
        let top = band_energy_share(&posted_n, 7000.0, 12000.0);
        assert!(top < 0.001, "HF must die past the 6.5k rolloff: {top:.5}");
    }

    #[test]
    fn musical_stems_share_one_loop_length() {
        // the phase-lock precondition: all four tile in lockstep because
        // they are the SAME sample count, started together by the sink
        let n = stem_pad().len();
        assert_eq!(n, n_samples(score::loop_secs()));
        assert_eq!(stem_sparkle().len(), n);
        assert_eq!(stem_keys().len(), n);
        assert_eq!(stem_drums(&mut NoiseStream::new(4)).len(), n);
    }

    #[test]
    fn night_stems_match_the_ratified_v4_fingerprints() {
        // reference = the owner-LISTEN-passed v4 float chain (2026-07-17);
        // same tolerances as the day pins: centroid ±20%, shares ±0.10
        // (drums ±0.15 — fresh noise per build)
        type Case = (&'static str, Vec<f32>, f32, f32, &'static [(f32, f32, f32)]);
        let cases: [Case; 4] = [
            (
                "night_pad",
                night_pad(),
                103.6,
                0.10,
                &[
                    (31.0, 62.0, 0.538),
                    (62.0, 125.0, 0.191),
                    (125.0, 250.0, 0.172),
                ],
            ),
            (
                "night_sparkle",
                night_sparkle(),
                541.3,
                0.10,
                &[(250.0, 500.0, 0.527), (500.0, 1000.0, 0.449)],
            ),
            (
                "night_keys",
                night_keys(),
                209.2,
                0.10,
                &[(125.0, 250.0, 0.729), (250.0, 500.0, 0.253)],
            ),
            (
                "night_drums",
                night_drums(&mut NoiseStream::new(4)),
                118.8,
                0.15,
                &[(62.0, 125.0, 0.561), (125.0, 250.0, 0.412)],
            ),
        ];
        for (name, buf, ref_centroid, tol, bands) in cases {
            let c = centroid_hz(&buf);
            assert!(
                (c - ref_centroid).abs() <= ref_centroid * 0.20,
                "{name}: centroid {c:.1} vs ratified {ref_centroid}"
            );
            for &(lo, hi, ref_share) in bands {
                let s = band_energy_share(&buf, lo, hi);
                assert!(
                    (s - ref_share).abs() <= tol,
                    "{name} band {lo}-{hi}: {s:.3} vs ratified {ref_share}"
                );
            }
        }
    }

    #[test]
    fn day_take_stems_match_the_ratified_fingerprints() {
        // reference = the owner-LISTEN-passed candidates A/B (2026-07-19)
        // measured on the python float chain with the RUST formulas
        // (export_day2_score.py); same tolerances as the day/night pins:
        // centroid ±20%, shares ±0.10 (drums ±0.15 — fresh noise per build)
        type Case = (&'static str, Vec<f32>, f32, f32, &'static [(f32, f32, f32)]);
        let cases: [Case; 8] = [
            (
                "day2_pad",
                day_take_pad(&score::DAY2),
                277.7,
                0.10,
                &[(125.0, 250.0, 0.497), (250.0, 500.0, 0.462)],
            ),
            (
                "day2_sparkle",
                day_take_sparkle(&score::DAY2),
                580.4,
                0.10,
                &[(250.0, 500.0, 0.312), (500.0, 1000.0, 0.659)],
            ),
            (
                "day2_keys",
                day_take_keys(&score::DAY2),
                355.6,
                0.10,
                &[(125.0, 250.0, 0.323), (250.0, 500.0, 0.556)],
            ),
            (
                "day2_drums",
                day_take_drums(&score::DAY2, &mut NoiseStream::new(4)),
                235.5,
                0.15,
                &[(62.5, 125.0, 0.492), (125.0, 250.0, 0.420)],
            ),
            (
                "day3_pad",
                day_take_pad(&score::DAY3),
                217.5,
                0.10,
                &[(125.0, 250.0, 0.746), (250.0, 500.0, 0.235)],
            ),
            (
                "day3_sparkle",
                day_take_sparkle(&score::DAY3),
                611.3,
                0.10,
                &[(250.0, 500.0, 0.218), (500.0, 1000.0, 0.751)],
            ),
            (
                "day3_keys",
                day_take_keys(&score::DAY3),
                303.7,
                0.10,
                &[(125.0, 250.0, 0.404), (250.0, 500.0, 0.558)],
            ),
            (
                "day3_drums",
                day_take_drums(&score::DAY3, &mut NoiseStream::new(4)),
                192.2,
                0.15,
                &[(62.5, 125.0, 0.521), (125.0, 250.0, 0.414)],
            ),
        ];
        for (name, buf, ref_centroid, tol, bands) in cases {
            let c = centroid_hz(&buf);
            assert!(
                (c - ref_centroid).abs() <= ref_centroid * 0.20,
                "{name}: centroid {c:.1} vs ratified {ref_centroid}"
            );
            for &(lo, hi, ref_share) in bands {
                let s = band_energy_share(&buf, lo, hi);
                assert!(
                    (s - ref_share).abs() <= tol,
                    "{name} band {lo}-{hi}: {s:.3} vs ratified {ref_share}"
                );
            }
        }
        // the full-kit signatures night deliberately lacks: the snare's
        // 400-3200 noise body and the open hat's long top-end ring both
        // put audible energy in 3.5-6.5k
        for take in [&score::DAY2, &score::DAY3] {
            let drums = day_take_drums(take, &mut NoiseStream::new(4));
            let hats = band_energy_share(&drums, 3500.0, 6500.0);
            assert!(
                hats > 0.0015,
                "day-take hats/snare must be audible in 3.5-6.5k: {hats:.5}"
            );
        }
    }

    #[test]
    fn day_take_stems_share_one_loop_length_per_take() {
        // the phase-lock precondition per take; and each take's loop is
        // its OWN length (76/74 BPM vs day 72) — takes never cross-tile
        for take in [&score::DAY2, &score::DAY3] {
            let n = day_take_pad(take).len();
            assert_eq!(n, n_samples(take.loop_secs()));
            assert_eq!(day_take_sparkle(take).len(), n);
            assert_eq!(day_take_keys(take).len(), n);
            assert_eq!(day_take_drums(take, &mut NoiseStream::new(4)).len(), n);
            assert_ne!(n, n_samples(score::loop_secs()));
            assert_ne!(n, n_samples(score::night_loop_secs()));
        }
        assert_ne!(
            n_samples(score::DAY2.loop_secs()),
            n_samples(score::DAY3.loop_secs())
        );
    }

    #[test]
    fn night_stems_share_one_loop_length_including_texture() {
        // night phase-lock covers TEXTURE too: its kick-duck is baked at
        // frozen kick times, so it must tile in lockstep with the drums
        let n = night_pad().len();
        assert_eq!(n, n_samples(score::night_loop_secs()));
        assert_eq!(night_sparkle().len(), n);
        assert_eq!(night_keys().len(), n);
        assert_eq!(night_drums(&mut NoiseStream::new(4)).len(), n);
        assert_eq!(night_texture(&mut NoiseStream::new(4)).len(), n);
        // and the night loop is deliberately NOT the day loop
        assert_ne!(n, n_samples(score::loop_secs()));
    }

    #[test]
    fn night_texture_seam_is_spliced_and_duck_dips_after_kicks() {
        let tex = night_texture(&mut NoiseStream::new(4));
        let n = tex.len();
        // the wrap must be a CONTINUATION, not merely "no huge jump" (the
        // first splice attempt passed the loose body-max check while the
        // discontinuity survived — review-refuted empirically). Two pins:
        // adjacent-sample continuity at the wrap, and no level step across
        // it (the 0.4Hz AM's phase mismatch was a ~19% RMS step broken).
        let body_median = {
            let mut d: Vec<f32> = tex.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            d.sort_by(f32::total_cmp);
            d[d.len() / 2]
        };
        let seam = (tex[0] - tex[n - 1]).abs();
        assert!(
            seam <= body_median * 8.0,
            "wrap must read as adjacent samples: seam {seam} vs median {body_median}"
        );
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let w = n_samples(0.01);
        let head = rms(&tex[..w]);
        let tail = rms(&tex[n - w..]);
        assert!(
            (head - tail).abs() <= 0.10 * head.max(tail),
            "no level step across the wrap: head {head:.5} vs tail {tail:.5}"
        );
        // duck: RMS right after a kick is lower than just before it
        let kt = score::NIGHT_KICK_TIMES[2];
        let i = n_samples(kt);
        let w = n_samples(0.05);
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let before = rms(&tex[i - w..i]);
        let after = rms(&tex[i..i + w]);
        assert!(
            after < before * 0.85,
            "texture must duck after the kick: {before:.5} -> {after:.5}"
        );
    }

    #[test]
    fn rain_bed_matches_the_gentle_reference_envelope() {
        let mut rng = NoiseStream::new(3);
        let bed = rain_bed(&mut rng);
        for &(lo, hi, pct) in &GENTLE_RAIN_BANDS {
            let share = band_energy_share(&bed, lo, hi) * 100.0;
            assert!(
                (share - pct).abs() < 4.0,
                "band {lo}-{hi}: {share:.1}% vs target {pct}%"
            );
        }
    }

    #[test]
    fn every_buffer_is_finite_and_peak_bounded() {
        let mut rng = NoiseStream::new(9);
        for (name, buf) in [
            ("keystroke", keystroke(&mut rng)),
            ("chime", door_chime()),
            ("printer", printer_whir(&mut rng)),
            ("vending", vending_drop(&mut rng)),
            ("drop", rain_drop(&mut rng)),
            ("texture", texture_bed(&mut rng)),
            ("pad", stem_pad()),
            ("sparkle", stem_sparkle()),
            ("keys", stem_keys()),
            ("drums", stem_drums(&mut rng)),
            ("night_pad", night_pad()),
            ("night_sparkle", night_sparkle()),
            ("night_keys", night_keys()),
            ("night_drums", night_drums(&mut rng)),
            ("night_texture", night_texture(&mut rng)),
            ("day2_pad", day_take_pad(&score::DAY2)),
            ("day2_sparkle", day_take_sparkle(&score::DAY2)),
            ("day2_keys", day_take_keys(&score::DAY2)),
            ("day2_drums", day_take_drums(&score::DAY2, &mut rng)),
            ("day3_pad", day_take_pad(&score::DAY3)),
            ("day3_sparkle", day_take_sparkle(&score::DAY3)),
            ("day3_keys", day_take_keys(&score::DAY3)),
            ("day3_drums", day_take_drums(&score::DAY3, &mut rng)),
        ] {
            assert!(!buf.is_empty(), "{name} empty");
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                "{name} has NaN/over-peak samples"
            );
        }
    }
}
