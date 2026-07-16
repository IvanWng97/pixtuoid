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
pub(crate) fn keystroke(rng: &mut NoiseStream) -> Vec<f32> {
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

/// Elevator arrival — a struck chime bar: INHARMONIC transverse modes at
/// 1 : 2.76 : 5.40 (glockenspiel ratios; a 1:2:3 harmonic stack reads
/// "organ"), detuned fundamental pair for the slow beat, strike transient.
pub(crate) fn elevator_ding(rng: &mut NoiseStream) -> Vec<f32> {
    let dur = 1.6;
    let n = n_samples(dur);
    let f0 = 870.0;
    let mut buf = vec![0.0f32; n];
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let strike = bandpass(&raw, 2500.0, 7000.0);
    let tau = std::f32::consts::TAU;
    for (i, b) in buf.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let bar = (tau * (f0 - 0.8) * t).sin() * (-t * 2.2).exp()
            + (tau * (f0 + 0.8) * t).sin() * (-t * 2.2).exp()
            + 0.55 * (tau * f0 * 2.76 * t).sin() * (-t * 8.0).exp()
            + 0.25 * (tau * f0 * 5.40 * t).sin() * (-t * 16.0).exp();
        *b = bar * 0.5 + strike[i] * (-t * 300.0).exp() * 0.3;
    }
    normalize(&mut buf, 0.6);
    buf
}

/// Door chime — a DESCENDING ding-dong (E5 → C5), warm harmonic bells with
/// slow decay: deliberately DISTINCT from the elevator's bright inharmonic
/// strike (centroid ~556Hz vs ~872Hz — one-shots must be spectrally
/// distinct per cue role).
pub(crate) fn door_chime() -> Vec<f32> {
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
pub(crate) fn printer_whir(rng: &mut NoiseStream) -> Vec<f32> {
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
pub(crate) fn vending_drop(rng: &mut NoiseStream) -> Vec<f32> {
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

/// Water-cooler glug — Minnaert bubble physics: three large bubbles, each a
/// damped sine near its resonance whose pitch bends UP as it rises, the
/// sequence stepping up as the air column shortens; a quiet pour wash.
pub(crate) fn cooler_glug(rng: &mut NoiseStream) -> Vec<f32> {
    let dur = 0.85;
    let n = n_samples(dur);
    let mut buf = vec![0.0f32; n];
    let tau = std::f32::consts::TAU;
    let f_base = 340.0;
    for (i, &at) in [0.02f32, 0.28, 0.55].iter().enumerate() {
        let d = 0.16;
        let bn = n_samples(d);
        let f0 = f_base * (1.0 + 0.18 * i as f32);
        let env = env_ar(bn, 0.004, 0.05);
        let mut phase = 0.0f32;
        let bubble: Vec<f32> = (0..bn)
            .map(|k| {
                let t = k as f32 / SR;
                let f = f0 * (1.0 + 0.12 * t / d);
                phase += tau * f / SR;
                phase.sin() * (-t * 22.0).exp() * env(k)
            })
            .collect();
        place(&mut buf, &bubble, at, 0.9 - 0.1 * i as f32);
    }
    let raw: Vec<f32> = (0..n).map(|_| rng.norm()).collect();
    let pour = bandpass(&raw, 600.0, 2000.0);
    let env_p = env_ar(n, 0.1, 0.3);
    for (i, b) in buf.iter_mut().enumerate() {
        *b += pour[i] * env_p(i) * 0.06;
    }
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
pub(crate) fn rain_bed(rng: &mut NoiseStream) -> Vec<f32> {
    shaped_noise_loop(BED_LOOP_SAMPLES, &GENTLE_RAIN_BANDS, rng)
}

/// One audible raindrop for the runtime scatter pool. Three surface
/// populations (measured ~640-1730Hz centroid spread): dull plop on
/// wood/soil, water plip (the classic Minnaert chirp), bright ping on
/// metal/glass — weights 20/55/25 (gentle rain skews away from dull thuds).
pub(crate) fn rain_drop(rng: &mut NoiseStream) -> Vec<f32> {
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
/// per-stem hiss stacking bug lives in the spec's cautionary tales.
/// UNWIRED until Phase 2's music lands (owner call: no floor noise without
/// music) — the expect flips to an error the moment Phase 2 reconnects it.
#[cfg_attr(not(test), expect(dead_code))] // tests exercise it; prod re-wires at Phase 2
pub(crate) fn texture_bed(rng: &mut NoiseStream) -> Vec<f32> {
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

        let ding = elevator_ding(&mut rng);
        let chime = door_chime();
        let (c_ding, c_chime) = (centroid_hz(&ding), centroid_hz(&chime));
        assert!(
            c_chime < c_ding,
            "door chime ({c_chime}) must sit warmer/lower than the elevator ding ({c_ding})"
        );

        let glug = cooler_glug(&mut rng);
        let c_glug = centroid_hz(&glug);
        assert!(
            c_glug < 700.0,
            "glug centroid {c_glug} must be a low bubble"
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
            ("ding", elevator_ding(&mut rng)),
            ("chime", door_chime()),
            ("printer", printer_whir(&mut rng)),
            ("vending", vending_drop(&mut rng)),
            ("glug", cooler_glug(&mut rng)),
            ("drop", rain_drop(&mut rng)),
            ("texture", texture_bed(&mut rng)),
        ] {
            assert!(!buf.is_empty(), "{name} empty");
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
                "{name} has NaN/over-peak samples"
            );
        }
    }
}
