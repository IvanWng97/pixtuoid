//! The procedural sound recipes. Every constant is a measured/ratified value
//! from the Phase 0 audition spec (docs/superpowers/specs/
//! 2026-07-16-ambient-sound-phase0/); change them THERE first (re-audition),
//! then mirror here. All functions are PURE sample-buffer generators (mono f32
//! @ 44_100 Hz) run once at startup — playback never synthesizes.

use super::dsp::{bandpass, lowpass, shaped_noise_loop, NoiseStream, SAMPLE_RATE};

const SR: f32 = SAMPLE_RATE as f32;

fn n_samples(dur_s: f32) -> usize {
    (dur_s * SR).round() as usize
}

fn env_ar(n: usize, attack_s: f32, release_s: f32) -> impl Fn(usize) -> f32 {
    let na = n_samples(attack_s).min(n).max(1);
    let nr = n_samples(release_s).min(n).max(1);
    move |i| {
        let a = (i as f32 / na as f32).min(1.0);
        let r = ((n - i) as f32 / nr as f32).min(1.0);
        a.min(r)
    }
}

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

/// One mechanical-keyboard stroke — the BRIGHT office clack, most of its energy
/// in 1-4kHz. The ASMR-lore deep thock measured OPPOSITE to the owner's
/// reference, so don't retune this darker.
pub fn keystroke(rng: &mut NoiseStream) -> Vec<f32> {
    let d = 0.05;
    let n = n_samples(d);
    let f_lo = 1250.0 + 450.0 * rng.unit();
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let click = bandpass(&raw, f_lo, f_lo + 2100.0);
    let f_b = 1000.0 + 400.0 * rng.unit();
    let raw_b: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let body = bandpass(&raw_b, f_b, f_b + 900.0);
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

const MIDI_A4: f32 = 440.0;
fn midi_freq(m: f32) -> f32 {
    MIDI_A4 * 2f32.powf((m - 69.0) / 12.0)
}

/// Door chime — a DESCENDING ding-dong (E5 → C5), warm harmonic bells.
pub fn door_chime() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(2.0)];
    let tau = std::f32::consts::TAU;
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
        let f =
            80.0 + 50.0 * (t / 0.25).clamp(0.0, 1.0) - 30.0 * ((t - 1.15) / 0.35).clamp(0.0, 1.0);
        phase += tau * f / SR;
        let motor = phase.sin() + 0.45 * (2.0 * phase).sin() + 0.2 * (3.0 * phase).sin();
        buf[i] = motor * env(i) * 0.5 + texture[i] * env_tex(i) * 0.16;
    }
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

/// Vending machine: mechanism click → beat → the can DROPS.
pub fn vending_drop(rng: &mut NoiseStream) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(0.7)];
    let cn = n_samples(0.03);
    let raw_c: Vec<f32> = (0..cn).map(|_| rng.norm()).collect();
    let click: Vec<f32> = bandpass(&raw_c, 800.0, 2000.0)
        .iter()
        .enumerate()
        .map(|(i, &v)| v * (-(i as f32 / SR) * 200.0).exp())
        .collect();
    place(&mut buf, &click, 0.0, 0.5);
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

/// The gentle-rain octave-band envelope `(lo_hz, hi_hz, energy %)`, measured
/// from the owner's chosen reference recording.
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

/// Seamless-loop length for the noise beds. Must stay a power of two: the
/// FFT-domain shaping is circular, so the block loops click-free.
const BED_LOOP_SAMPLES: usize = 1 << 19;

/// The rain WASH only — audible foreground drops are scattered at runtime from
/// [`rain_drop`]'s pool, so rain never repeats.
pub fn rain_bed(rng: &mut NoiseStream) -> Vec<f32> {
    shaped_noise_loop(BED_LOOP_SAMPLES, &GENTLE_RAIN_BANDS, rng)
}

/// One audible raindrop for the runtime scatter pool: dull plop on wood/soil,
/// water plip, or bright ping on metal/glass.
pub fn rain_drop(rng: &mut NoiseStream) -> Vec<f32> {
    let d = 0.10;
    let n = n_samples(d);
    let kind = rng.unit();
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

/// Vinyl-crackle pop density shared by BOTH room-tone beds — one knob, so the
/// day and night textures age identically.
const CRACKLE_POPS_PER_SEC: f32 = 0.18;

/// The vinyl/room texture bed: tape hiss + a faint warm room hum + sparse soft
/// crackle. Its crackle is per-boot random ON PURPOSE — unpitched, so variation
/// is a feature; everything melodic is frozen in `score`.
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
    let n_pops = (n as f32 / SR * CRACKLE_POPS_PER_SEC) as usize;
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

// The `#[cfg(test)]` stems below are fingerprint ANCHORS, not dead code: their
// pins prove the shared cores `gen_beds` ships still render the ratified sound.

use super::score;

#[cfg(test)]
fn beat_s() -> f32 {
    score::beat_s()
}

#[cfg(test)]
fn bar_s() -> f32 {
    score::beat_s() * score::BEATS_PER_BAR
}

/// The lofi master chain, applied per MUSICAL stem (texture/rain skip it — they
/// ARE the medium). No hiss here: per-stem hiss STACKS across the four stems, so
/// the medium noise is `texture_bed`'s job, exactly once.
fn lofi_post(buf: &[f32], drive: f32) -> Vec<f32> {
    let warped = crate::audio::dsp::warp_resample(buf, &[(0.7, 0.0025), (8.0, 0.0006)]);
    let t = drive.tanh();
    let sat: Vec<f32> = warped.iter().map(|&x| (x * drive).tanh() / t).collect();
    let bump = bandpass(&sat, 80.0, 120.0);
    let bumped: Vec<f32> = sat.iter().zip(&bump).map(|(&x, &b)| x + 0.35 * b).collect();
    lowpass(&bumped, 6500.0)
}

/// ONE site, so a stem can't silently skip the ratified post.
fn master(buf: &[f32], drive: f32, peak: f32) -> Vec<f32> {
    let mut out = lofi_post(buf, drive);
    normalize(&mut out, peak);
    out
}

/// One soft electric-piano note; `h2` is the 2nd-harmonic weight — the one axis
/// the day and night voices differ on.
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

/// The day EP voice — the ratified FIXED 2nd harmonic.
#[cfg(test)]
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

#[cfg(test)]
pub fn stem_pad() -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for bar in 0..score::LOOP_BARS {
        let chord = score::chord_at_bar(bar);
        // the held chord rings past the bar; the LAST bar's tail is clipped by
        // place() — part of the ratified render, don't wrap it
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

#[cfg(test)]
pub fn stem_sparkle() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for &(beats, note, vel) in &score::SPARKLE_SCORE {
        place(&mut buf, &ep_pluck(note, 1.6, vel), beats * beat_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 3200.0);
    normalize(&mut buf, 0.6);
    master(&buf, 1.6, 0.6)
}

#[cfg(test)]
pub fn stem_keys() -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(score::loop_secs())];
    for &(beats, note, vel) in &score::KEYS_SCORE {
        place(&mut buf, &ep_pluck(note, 0.9, vel), beats * beat_s(), 1.0);
    }
    let mut buf = lowpass(&buf, 2400.0);
    normalize(&mut buf, 0.8);
    master(&buf, 1.6, 0.8)
}

/// Kick/snare/swung-hat groove. Hat velocities are the frozen score's; each
/// hit's NOISE content is fresh per call.
#[cfg(test)]
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
    let mut buf = lowpass(&buf, 7500.0);
    normalize(&mut buf, 0.85);
    master(&buf, 2.2, 0.85)
}

/// The velocity-keyed 2nd-harmonic EP voice (the Rhodes "bell vs bark"), shared
/// by the night events AND the day takes' sparkle/keys.
fn ep_pluck_vel(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    ep_pluck_h2(midi, dur_s, vel, 0.18 + 0.45 * vel * vel)
}

/// The plucked-string lead voice ("nylon" family).
fn pluck_note(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    let n = n_samples(dur_s);
    let f = midi_freq(midi as f32);
    let tau = std::f32::consts::TAU;
    let h2 = 0.30 + 0.30 * vel;
    // decay follows pitch (higher strings die faster) and breathes per note; the
    // velocity BITS are the jitter source, not an rng, so identical inputs still
    // render identical buffers (the purity contract)
    let pitch_k = 1.0 + (midi as f32 - 72.0) * 0.02;
    let breath = 0.85 + 0.30 * (vel * 137.5).fract();
    let d = 3.4 * pitch_k * breath;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let attack = (t / 0.004).min(1.0);
            let sig = (tau * f * t).sin() * (-t * d).exp()
                + h2 * (tau * 2.0 * f * t).sin() * (-t * d * 1.76).exp()
                + 0.18 * (tau * 3.004 * f * t).sin() * (-t * d * 2.94).exp()
                + 0.07 * (tau * 4.21 * f * t).sin() * (-t * 22.0).exp();
            sig * attack * vel
        })
        .collect();
    let pn = n_samples(0.006);
    let mut prng = NoiseStream::new(((midi as u64) << 32) ^ vel.to_bits() as u64);
    let raw: Vec<f32> = (0..pn).map(|_| prng.norm()).collect();
    let pick_noise = bandpass(&raw, 2000.0, 5000.0);
    for (i, &v) in pick_noise.iter().enumerate() {
        if let Some(slot) = buf.get_mut(i) {
            *slot += v * (-(i as f32 / SR) * 700.0).exp() * 0.10 * vel;
        }
    }
    buf
}

/// The bass voice: a sine fundamental with tanh-grown odd harmonics (the sub
/// carries the power, the harmonics carry audibility on small speakers), a
/// soft attack so the kick keeps the transient, and a plateau-then-release
/// envelope — a held floor note, not a pluck.
fn bass_note(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    let n = n_samples(dur_s);
    let f = midi_freq(midi as f32);
    let tau = std::f32::consts::TAU;
    let env = env_ar(n, 0.03, dur_s * 0.35);
    let drive = 1.1 + 1.6 * vel;
    let norm = drive.tanh();
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            // the note settles slightly after the onset, like a damped string
            let body = 0.75 + 0.25 * (-t * 2.0).exp();
            let sig = ((tau * f * t).sin() * drive).tanh() / norm;
            sig * env(i) * body * vel
        })
        .collect()
}

/// The kalimba lead voice: a plucked TINE — a cantilever bar, so its overtones
/// are inharmonic (modes near 1 : 5.9 : 16.5, nothing like a string's 1:2:3)
/// and die much faster than the fundamental, plus a soft thump off the
/// resonator body.
fn kalimba_note(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    let n = n_samples(dur_s);
    let f = midi_freq(midi as f32);
    let tau = std::f32::consts::TAU;
    // higher tines die faster; the vel BITS jitter decay per note, not an rng,
    // so identical inputs still render identical buffers (the purity contract)
    let pitch_k = 1.0 + (midi as f32 - 72.0) * 0.025;
    let breath = 0.9 + 0.2 * (vel * 91.7).fract();
    let d = 2.6 * pitch_k * breath;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let attack = (t / 0.002).min(1.0);
            let sig = (tau * f * t).sin() * (-t * d).exp()
                + 0.20 * (tau * 5.9 * f * t).sin() * (-t * (d * 5.0 + 14.0)).exp()
                + 0.08 * (tau * 16.5 * f * t).sin() * (-t * 40.0).exp();
            sig * attack * vel
        })
        .collect();
    let pn = n_samples(0.010);
    let mut prng = NoiseStream::new(0x4B41_4C49 ^ ((midi as u64) << 32) ^ vel.to_bits() as u64);
    let raw: Vec<f32> = (0..pn).map(|_| prng.norm()).collect();
    let thump = bandpass(&raw, 120.0, 700.0);
    for (i, &v) in thump.iter().enumerate() {
        if let Some(slot) = buf.get_mut(i) {
            *slot += v * (-(i as f32 / SR) * 350.0).exp() * 0.12 * vel;
        }
    }
    buf
}

/// The vibraphone lead voice: a struck aluminum bar TUNED to double-octave
/// partials (1 : 4 : ~9.8 — a mallet spectrum, not a string), ringing long,
/// with the rotating-fan tremolo baked into the note.
fn vibe_note(midi: u8, dur_s: f32, vel: f32) -> Vec<f32> {
    let n = n_samples(dur_s);
    let f = midi_freq(midi as f32);
    let tau = std::f32::consts::TAU;
    let pitch_k = 1.0 + (midi as f32 - 69.0) * 0.015;
    let d = 1.3 * pitch_k;
    // the vel BITS detune the fan rate per note (purity contract, as above)
    let trem_hz = 4.2 + 0.8 * (vel * 53.1).fract();
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let attack = (t / 0.008).min(1.0);
            let trem = 1.0 - 0.175 * (1.0 - (tau * trem_hz * t).cos());
            let sig = (tau * f * t).sin() * (-t * d).exp()
                + (0.15 + 0.20 * vel) * (tau * 4.0 * f * t).sin() * (-t * (d * 3.0 + 2.0)).exp()
                + 0.06 * (tau * 9.8 * f * t).sin() * (-t * 18.0).exp();
            sig * attack * trem * vel
        })
        .collect();
    let pn = n_samples(0.008);
    let mut prng = NoiseStream::new(0x5649_4245 ^ ((midi as u64) << 32) ^ vel.to_bits() as u64);
    let raw: Vec<f32> = (0..pn).map(|_| prng.norm()).collect();
    let mallet = bandpass(&raw, 600.0, 1800.0);
    for (i, &v) in mallet.iter().enumerate() {
        if let Some(slot) = buf.get_mut(i) {
            *slot += v * (-(i as f32 / SR) * 500.0).exp() * 0.06 * vel;
        }
    }
    buf
}

#[cfg(test)]
fn night_bar_s() -> f32 {
    score::night_beat_s() * score::BEATS_PER_BAR
}

/// The FROZEN night pad's baked sub level. The ratified v4 take renders the
/// chords and the sub-bass floor in ONE buffer; GENERATED takes pass
/// `sub_gain` 0.0 instead because their sub rides the separate bass lane (5).
#[cfg(test)]
const NIGHT_PAD_SUB_GAIN: f32 = 3.2;

/// Night pad: chords + the SUB-BASS floor in ONE buffer, so the harmonic floor
/// moves as one — the FROZEN anchor predates the bass stem and keeps its pins.
#[cfg(test)]
pub fn night_pad() -> Vec<f32> {
    night_pad_core(
        &score::NIGHT_CHORDS,
        &score::NIGHT_BASS_ROOTS,
        night_bar_s(),
        score::NIGHT_LOOP_BARS,
        NIGHT_PAD_SUB_GAIN,
    )
}

fn night_pad_core(
    chords: &[[u8; 4]],
    bass_roots: &[u8; 4],
    bar_s: f32,
    loop_bars: usize,
    sub_gain: f32,
) -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(bar_s * loop_bars as f32)];
    for bar in 0..loop_bars {
        let (chord, root) = (
            chords[bar % chords.len()],
            bass_roots[bar % bass_roots.len()],
        );
        let dur = bar_s + 1.2;
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
        let fb = midi_freq(root as f32);
        for half in 0..2 {
            let hdur = bar_s / 2.0 + 0.4;
            let hn = n_samples(hdur);
            let env = env_ar(hn, 0.06, 0.9);
            let g = if half == 0 { 1.0 } else { 0.7 };
            let bass: Vec<f32> = (0..hn)
                .map(|i| {
                    let t = i as f32 / SR;
                    ((tau * fb * t).sin() + 0.15 * (tau * 2.0 * fb * t).sin())
                        * env(i)
                        * g
                        * sub_gain
                })
                .collect();
            place(&mut sig, &bass, half as f32 * bar_s / 2.0, 1.0);
        }
        place(&mut buf, &sig, bar as f32 * bar_s, 1.0);
    }
    let mut buf = lowpass(&buf, 2200.0);
    for (i, v) in buf.iter_mut().enumerate() {
        *v *= 1.0 + 0.04 * (tau * 0.15 * i as f32 / SR).sin();
    }
    normalize(&mut buf, 0.75);
    master(&buf, 1.6, 0.75)
}

#[cfg(test)]
fn night_events_stem(events: &[(f32, u8, f32)], dur_s: f32, cutoff_hz: f32, peak: f32) -> Vec<f32> {
    events_stem_core(score::night_loop_secs(), events, dur_s, cutoff_hz, peak)
}

fn events_stem_core(
    loop_secs: f32,
    events: &[(f32, u8, f32)],
    dur_s: f32,
    cutoff_hz: f32,
    peak: f32,
) -> Vec<f32> {
    events_stem_voiced(loop_secs, events, dur_s, cutoff_hz, peak, ep_pluck_vel)
}

/// The dispatch point a new lead voice plugs into: `note_fn` renders one note
/// from `(midi, dur_s, vel)`.
fn events_stem_voiced(
    loop_secs: f32,
    events: &[(f32, u8, f32)],
    dur_s: f32,
    cutoff_hz: f32,
    peak: f32,
    note_fn: fn(u8, f32, f32) -> Vec<f32>,
) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(loop_secs)];
    for &(at, note, vel) in events {
        place(&mut buf, &note_fn(note, dur_s, vel), at, 1.0);
    }
    let mut buf = lowpass(&buf, cutoff_hz);
    normalize(&mut buf, peak);
    master(&buf, 1.6, peak)
}

#[cfg(test)]
pub fn night_keys() -> Vec<f32> {
    let (dur, cutoff, peak) = lane_recipe(Mood::Night, 2);
    night_events_stem(&score::NIGHT_KEYS, dur, cutoff, peak)
}

#[cfg(test)]
pub fn night_sparkle() -> Vec<f32> {
    let (dur, cutoff, peak) = lane_recipe(Mood::Night, 1);
    night_events_stem(&score::NIGHT_SPARKLE, dur, cutoff, peak)
}

/// NOISE content is fresh per call; only timing/gain are frozen.
fn drum_hit(rng: &mut NoiseStream, kind: score::DrumKind) -> Vec<f32> {
    match kind {
        score::DrumKind::Kick => kick(rng),
        score::DrumKind::Snare => snare(rng),
        score::DrumKind::Hat => hat(rng, false),
        score::DrumKind::OpenHat => hat(rng, true),
    }
}

#[cfg(test)]
pub fn night_drums(rng: &mut NoiseStream) -> Vec<f32> {
    let (cutoff, drive, peak) = lane_recipe(Mood::Night, 3);
    drums_core(
        score::night_loop_secs(),
        &score::NIGHT_DRUMS,
        cutoff,
        drive,
        peak,
        rng,
    )
}

fn drums_core(
    loop_secs: f32,
    table: &[(f32, score::DrumKind, f32)],
    cutoff_hz: f32,
    drive: f32,
    peak: f32,
    rng: &mut NoiseStream,
) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples(loop_secs)];
    for &(at, kind, gain) in table {
        let hit = drum_hit(rng, kind);
        place(&mut buf, &hit, at, gain);
    }
    let mut buf = lowpass(&buf, cutoff_hz);
    normalize(&mut buf, peak);
    master(&buf, drive, peak)
}

/// Loop-seam splice length: the night texture's buffer is the night loop length,
/// NOT a power of two, so its FFT-shaped hiss is not circularly seamless.
const NIGHT_TEXTURE_SPLICE_S: f32 = 0.03;

/// The night texture: the day room-tone recipe at the NIGHT loop length with the
/// kick-duck BAKED IN — pre-multiplying the frozen kick envelope keeps it
/// phase-locked with the drums by construction, no runtime sidechain machinery.
#[cfg(test)]
pub fn night_texture(rng: &mut NoiseStream) -> Vec<f32> {
    night_texture_core(score::night_loop_secs(), &score::NIGHT_KICK_TIMES, rng)
}

fn night_texture_core(loop_secs: f32, kick_times: &[f32], rng: &mut NoiseStream) -> Vec<f32> {
    let n = n_samples(loop_secs);
    let f = n_samples(NIGHT_TEXTURE_SPLICE_S);
    // synthesize f EXTRA samples past the loop end so the splice blends a genuine
    // CONTINUATION of the tail into the head; a head-only crossfade of unrelated
    // content leaves the discontinuity in place
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
    let n_pops = (n as f32 / SR * CRACKLE_POPS_PER_SEC) as usize;
    for _ in 0..n_pops {
        let at = rng.unit() * (n as f32 / SR - 0.01);
        let pn = n_samples(0.003 + 0.004 * rng.unit());
        let pop: Vec<f32> = (0..pn)
            .map(|i| rng.norm() * (-(i as f32 / SR) * 800.0).exp())
            .collect();
        place(&mut buf[..n], &pop, at, 0.03 + 0.06 * rng.unit());
    }
    // CORRELATED content (the pre-roll continues the same stream), so a linear
    // blend suffices — no equal-power law needed
    for j in 0..f {
        let a = j as f32 / f as f32;
        buf[j] = buf[n + j] * (1.0 - a) + buf[j] * a;
    }
    buf.truncate(n);
    // normalize BEFORE the duck: duck-then-normalize re-inflates the level by up
    // to the duck depth
    normalize(&mut buf, 0.45);
    let depth = 10f32.powf(-4.0 / 20.0);
    let rel = n_samples(0.15);
    for &kt in kick_times {
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

#[cfg(test)]
pub(super) fn day_take_pad(take: &score::DayTake) -> Vec<f32> {
    day_pad_core(take.chords, take.bar_s(), score::DAY_TAKE_LOOP_BARS)
}

fn day_pad_core(chords: &[[u8; 4]], bar_s: f32, loop_bars: usize) -> Vec<f32> {
    let tau = std::f32::consts::TAU;
    let mut buf = vec![0.0f32; n_samples(bar_s * loop_bars as f32)];
    for bar in 0..loop_bars {
        let chord = chords[bar % chords.len()];
        let dur = bar_s + 0.9;
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
        place(&mut buf, &chord_sig, bar as f32 * bar_s, 1.0);
    }
    let mut buf = lowpass(&buf, 2600.0);
    for (i, v) in buf.iter_mut().enumerate() {
        *v *= 1.0 + 0.05 * (tau * 0.22 * i as f32 / SR).sin();
    }
    normalize(&mut buf, 0.7);
    master(&buf, 1.6, 0.7)
}

#[cfg(test)]
fn day_take_events_stem(
    take: &score::DayTake,
    events: &[(f32, u8, f32)],
    dur_s: f32,
    cutoff_hz: f32,
    peak: f32,
) -> Vec<f32> {
    events_stem_core(take.loop_secs(), events, dur_s, cutoff_hz, peak)
}

#[cfg(test)]
pub(super) fn day_take_sparkle(take: &score::DayTake) -> Vec<f32> {
    let (dur, cutoff, peak) = lane_recipe(Mood::Day, 1);
    day_take_events_stem(take, take.sparkle, dur, cutoff, peak)
}

#[cfg(test)]
pub(super) fn day_take_keys(take: &score::DayTake) -> Vec<f32> {
    let (dur, cutoff, peak) = lane_recipe(Mood::Day, 2);
    day_take_events_stem(take, take.keys, dur, cutoff, peak)
}

#[cfg(test)]
pub(super) fn day_take_drums(take: &score::DayTake, rng: &mut NoiseStream) -> Vec<f32> {
    let (cutoff, drive, peak) = lane_recipe(Mood::Day, 3);
    drums_core(take.loop_secs(), take.drums, cutoff, drive, peak, rng)
}

use super::compose::{GeneratedScore, LeadVoice, Mood};

/// The "bright" master for GENERATED takes: a one-pole tilt shelf, then
/// re-normalize to the bed's pre-tilt peak. FROZEN takes keep their darker
/// chain — brightening them would invalidate their fingerprint pins.
fn brighten(buf: &mut [f32]) {
    let pre_peak = buf.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    let a = (-std::f32::consts::TAU * 1200.0 / SR).exp();
    let mut lp = 0.0f32;
    for s in buf.iter_mut() {
        lp = a * lp + (1.0 - a) * *s;
        *s += 0.8 * (*s - lp);
    }
    normalize(buf, pre_peak.max(1e-6));
}

fn lead_voice_fn(score: &GeneratedScore) -> fn(u8, f32, f32) -> Vec<f32> {
    match score.lead_voice {
        LeadVoice::EpVel => ep_pluck_vel,
        LeadVoice::Pluck => pluck_note,
        LeadVoice::Kalimba => kalimba_note,
        LeadVoice::Vibraphone => vibe_note,
    }
}

/// The per-`(mood, lane)` render recipe — the ONE authority both the [`gen_bed`]
/// arms and the frozen `#[cfg(test)]` delegations read, so the frozen
/// fingerprint pins transitively guard the params that actually ship. The triple
/// is that lane's core-call args IN ORDER: sparkle/keys/bass (lanes 1,2,5) =
/// `(dur_s, cutoff_hz, peak)`; drums (lane 3) = `(cutoff_hz, drive, peak)`.
/// Pad/texture lanes (0,4) read the `(0.0, 0.0, 0.0)` fallback and ignore it.
fn lane_recipe(mood: Mood, lane: usize) -> (f32, f32, f32) {
    match (mood, lane) {
        (Mood::Day, 1) => (2.0, 3200.0, 0.6),
        (Mood::Day, 2) => (0.9, 2400.0, 0.8),
        (Mood::Day, 3) => (7500.0, 2.2, 0.85),
        (Mood::Day, 5) => (1.8, 900.0, 0.8),
        (Mood::Night, 1) => (2.2, 2800.0, 0.5),
        (Mood::Night, 2) => (1.1, 2000.0, 0.7),
        (Mood::Night, 3) => (6000.0, 2.0, 0.8),
        (Mood::Night, 5) => (2.0, 700.0, 0.8),
        _ => (0.0, 0.0, 0.0),
    }
}

/// One lane of a generated take (`bank::TRACK_STEMS` order: pad, sparkle, keys,
/// drums, texture, bass) — the CHUNKED build unit the wasm driver spreads across
/// ticks, because a full six-bed build in one rAF tick hitches the page. An
/// out-of-range lane returns silence rather than panicking.
pub fn gen_bed(score: &GeneratedScore, lane: usize, rng: &mut NoiseStream) -> Vec<f32> {
    let loop_s = score.loop_secs();
    let (r0, r1, r2) = lane_recipe(score.mood, lane);
    let mut bed = match (score.mood, lane) {
        (Mood::Day, 0) => day_pad_core(
            &score.bar_chords,
            score.bar_s(),
            super::compose::GEN_LOOP_BARS,
        ),
        (Mood::Day, 1) => {
            events_stem_voiced(loop_s, &score.sparkle, r0, r1, r2, lead_voice_fn(score))
        }
        (Mood::Day, 2) => events_stem_core(loop_s, &score.keys, r0, r1, r2),
        (Mood::Day, 3) => drums_core(loop_s, &score.drums, r0, r1, r2, rng),
        (Mood::Day, 4) => texture_bed(rng),
        // the generated night pad renders NO baked sub — lane 5 owns that register
        (Mood::Night, 0) => night_pad_core(
            &score.bar_chords,
            &score.bass_roots,
            score.bar_s(),
            super::compose::GEN_LOOP_BARS,
            0.0,
        ),
        (Mood::Night, 1) => {
            events_stem_voiced(loop_s, &score.sparkle, r0, r1, r2, lead_voice_fn(score))
        }
        (Mood::Night, 2) => events_stem_core(loop_s, &score.keys, r0, r1, r2),
        (Mood::Night, 3) => drums_core(loop_s, &score.drums, r0, r1, r2, rng),
        (Mood::Night, 4) => night_texture_core(loop_s, &score.kick_times, rng),
        (Mood::Day | Mood::Night, 5) => {
            events_stem_voiced(loop_s, &score.bass, r0, r1, r2, bass_note)
        }
        _ => Vec::new(),
    };
    brighten(&mut bed);
    bed
}

/// Synthesize a generated take's six beds in `bank::TRACK_STEMS` order.
pub fn gen_beds(score: &GeneratedScore, rng: &mut NoiseStream) -> [Vec<f32>; 6] {
    std::array::from_fn(|lane| gen_bed(score, lane, rng))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::{band_energy_share, centroid_hz};

    #[test]
    fn lane_recipe_is_the_ratified_shipping_params() {
        use crate::audio::compose::Mood;
        assert_eq!(lane_recipe(Mood::Day, 1), (2.0, 3200.0, 0.6));
        assert_eq!(lane_recipe(Mood::Day, 2), (0.9, 2400.0, 0.8));
        assert_eq!(lane_recipe(Mood::Day, 3), (7500.0, 2.2, 0.85));
        assert_eq!(lane_recipe(Mood::Day, 5), (1.8, 900.0, 0.8));
        assert_eq!(lane_recipe(Mood::Night, 1), (2.2, 2800.0, 0.5));
        assert_eq!(lane_recipe(Mood::Night, 2), (1.1, 2000.0, 0.7));
        assert_eq!(lane_recipe(Mood::Night, 3), (6000.0, 2.0, 0.8));
        assert_eq!(lane_recipe(Mood::Night, 5), (2.0, 700.0, 0.8));
    }

    #[test]
    fn new_lead_voices_are_pure_finite_and_keep_their_decay_character() {
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let tail_ratio = |b: &[f32]| {
            let q = b.len() / 4;
            rms(&b[b.len() - q..]) / rms(&b[..q]).max(1e-9)
        };
        let (kal, vib) = (kalimba_note(72, 2.0, 0.5), vibe_note(72, 2.0, 0.5));
        for (name, buf) in [("kalimba", &kal), ("vibe", &vib)] {
            assert_eq!(buf.len(), n_samples(2.0), "{name}: wrong note length");
            assert!(buf.iter().all(|v| v.is_finite()), "{name}: non-finite");
            assert!(rms(buf) > 1e-4, "{name}: silent note");
        }
        // purity contract: identical inputs render identical buffers
        assert_eq!(kal, kalimba_note(72, 2.0, 0.5));
        assert_eq!(vib, vibe_note(72, 2.0, 0.5));
        // the tine dies fast, the bar rings — that ordering IS the two timbres
        assert!(
            tail_ratio(&vib) > 2.0 * tail_ratio(&kal),
            "vibe tail {} must ring well past the kalimba tail {}",
            tail_ratio(&vib),
            tail_ratio(&kal)
        );
    }

    #[test]
    fn one_shot_synth_spectral_sanity() {
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
        // reference = the ratified composition measured on the python FLOAT
        // chain, NOT the wav files — write_wav's stereo interleave + soft clip
        // once poisoned these numbers.
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
            // drums need a wider band tolerance: their noise content is fresh
            // per build
            let tol = if name == "drums" { 0.15 } else { 0.10 };
            for &(lo, hi, ref_share) in bands {
                let s = band_energy_share(&buf, lo, hi);
                assert!(
                    (s - ref_share).abs() <= tol,
                    "{name} band {lo}-{hi}: {s:.3} vs ratified {ref_share}"
                );
            }
        }
        let drums = stem_drums(&mut NoiseStream::new(4));
        let low = band_energy_share(&drums, 62.5, 125.0);
        let mid = band_energy_share(&drums, 125.0, 250.0);
        let high = band_energy_share(&drums, 250.0, 500.0);
        assert!(
            low > mid && mid > high,
            "drums must stay kick-dominant: {low:.3} > {mid:.3} > {high:.3}"
        );
        // the hi-hat layer is the ONLY content in 3.5-6.5k (kick/snare live below
        // 3.2k), so the coarse octave bands cannot see the groove
        let hats = band_energy_share(&drums, 3500.0, 6500.0);
        assert!(
            hats > 0.0015,
            "the swung-hat groove must be audible in 3.5-6.5k: {hats:.5}"
        );
    }

    #[test]
    fn lofi_post_saturates_bumps_and_rolls_off() {
        let tau = std::f32::consts::TAU;
        let tone: Vec<f32> = (0..n_samples(2.0))
            .map(|i| (tau * 440.0 * i as f32 / SR).sin() * 0.8)
            .collect();
        let posted = lofi_post(&tone, 1.6);
        let third = band_energy_share(&posted, 1200.0, 1450.0);
        assert!(
            third > 0.005,
            "tanh drive must write a 3rd harmonic: {third:.4}"
        );
        let mut rng = NoiseStream::new(21);
        let noise: Vec<f32> = (0..n_samples(2.0)).map(|_| rng.norm() * 0.3).collect();
        let posted_n = lofi_post(&noise, 1.6);
        let bump_in = band_energy_share(&noise, 80.0, 120.0);
        let bump_out = band_energy_share(&posted_n, 80.0, 120.0);
        assert!(
            bump_out > bump_in * 1.3,
            "head bump must lift 80-120Hz: {bump_in:.4} -> {bump_out:.4}"
        );
        let top = band_energy_share(&posted_n, 7000.0, 12000.0);
        assert!(top < 0.001, "HF must die past the 6.5k rolloff: {top:.5}");
    }

    #[test]
    fn musical_stems_share_one_loop_length() {
        let n = stem_pad().len();
        assert_eq!(n, n_samples(score::loop_secs()));
        assert_eq!(stem_sparkle().len(), n);
        assert_eq!(stem_keys().len(), n);
        assert_eq!(stem_drums(&mut NoiseStream::new(4)).len(), n);
    }

    #[test]
    fn night_stems_match_the_ratified_v4_fingerprints() {
        // reference = the owner-LISTEN-passed v4 float chain
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
        // reference = the owner-LISTEN-passed candidates, measured on the python
        // float chain with the RUST formulas
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
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                "{name}: NaN/over-peak"
            );
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
            // the full-kit signatures night deliberately lacks: the snare body
            // and the open-hat ring both put audible energy in 3.5-6.5k
            if name.ends_with("drums") {
                let hats = band_energy_share(&buf, 3500.0, 6500.0);
                assert!(
                    hats > 0.0015,
                    "{name} hats/snare must be audible in 3.5-6.5k: {hats:.5}"
                );
            }
        }
    }

    #[test]
    fn brighten_lifts_the_top_and_preserves_the_peak() {
        let mut rng = NoiseStream::new(17);
        let noise: Vec<f32> = (0..n_samples(2.0)).map(|_| rng.norm() * 0.3).collect();
        let mut posted = noise.clone();
        brighten(&mut posted);
        // a hi/lo RATIO, not a share-of-total: the tilt lifts most of the
        // spectrum, so the denominator grows too
        let ratio =
            |b: &[f32]| band_energy_share(b, 4000.0, 8000.0) / band_energy_share(b, 100.0, 500.0);
        let (r_in, r_out) = (ratio(&noise), ratio(&posted));
        assert!(
            r_out > r_in * 2.0,
            "tilt must lift hi/lo by the shelf: {r_in:.3} -> {r_out:.3}"
        );
        let peak = |b: &[f32]| b.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
        assert!(
            (peak(&posted) - peak(&noise)).abs() < 1e-3,
            "brighten must re-normalize to the pre-tilt peak"
        );
    }

    #[test]
    fn generated_beds_are_finite_phase_locked_and_in_the_sound_world() {
        // the musical invariants are the compose seed-sweep's job; a couple of
        // seeds per mood suffice as the synthesis smoke
        for mood in [Mood::Day, Mood::Night] {
            for seed in [0u64, 7] {
                let score = super::super::compose::compose(mood, seed);
                let mut rng = NoiseStream::new(9);
                let beds = gen_beds(&score, &mut rng);
                let n = n_samples(score.loop_secs());
                for (i, bed) in beds.iter().enumerate() {
                    assert!(
                        bed.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                        "{mood:?} seed {seed} bed {i}: NaN/over-peak"
                    );
                }
                // every musical lane phase-locks; only texture (4) runs free by day
                for i in [0usize, 1, 2, 3, 5] {
                    assert_eq!(beds[i].len(), n, "{mood:?} seed {seed} bed {i} length");
                }
                match mood {
                    Mood::Day => assert_eq!(beds[4].len(), BED_LOOP_SAMPLES),
                    Mood::Night => assert_eq!(beds[4].len(), n),
                }
                let low = band_energy_share(&beds[3], 62.5, 125.0);
                let mid = band_energy_share(&beds[3], 125.0, 250.0);
                assert!(
                    low > 0.2 && low > mid * 0.8,
                    "{mood:?} seed {seed}: drums lost the kick floor ({low:.3}/{mid:.3})"
                );
                let sub = band_energy_share(&beds[5], 31.0, 125.0);
                assert!(
                    sub > 0.35 && sub > band_energy_share(&beds[5], 250.0, 1000.0),
                    "{mood:?} seed {seed}: the bass lane must own the sub ({sub:.3})"
                );
            }
        }
    }

    #[test]
    fn day_take_stems_share_one_loop_length_per_take() {
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
        // night phase-lock covers TEXTURE too: its kick-duck is baked at frozen
        // kick times, so it must tile in lockstep with the drums
        let n = night_pad().len();
        assert_eq!(n, n_samples(score::night_loop_secs()));
        assert_eq!(night_sparkle().len(), n);
        assert_eq!(night_keys().len(), n);
        assert_eq!(night_drums(&mut NoiseStream::new(4)).len(), n);
        assert_eq!(night_texture(&mut NoiseStream::new(4)).len(), n);
        assert_ne!(n, n_samples(score::loop_secs()));
    }

    #[test]
    fn night_texture_seam_is_spliced_and_duck_dips_after_kicks() {
        let tex = night_texture(&mut NoiseStream::new(4));
        let n = tex.len();
        // the wrap must read as a CONTINUATION, not merely "no huge jump": a
        // loose body-max check passed while the discontinuity survived
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
            // The 8 day2/day3 take-stems are checked in
            // `day_take_stems_match_the_ratified_fingerprints`; rebuilding them
            // here would be a duplicate synthesis pass.
        ] {
            assert!(!buf.is_empty(), "{name} empty");
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                "{name} has NaN/over-peak samples"
            );
        }
    }
}
