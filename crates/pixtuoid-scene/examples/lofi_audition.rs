//! The generator's STATISTICAL listen gate: render N random-seed
//! generated takes to wav so the owner can blind-audition the COMPOSER
//! (not one take). All seeds passing = the generator is ratified; a dud
//! = tighten `compose`'s constraints and re-batch. Renders through the
//! REAL `synth::gen_beds` chain, so what you hear is what ships.
//!
//! Usage:
//!   cargo run --release -p pixtuoid-scene --example lofi_audition -- \
//!     [--mood day|night] [--seeds N] [--start S] [--out DIR]

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;

use pixtuoid_scene::audio::compose::{
    compose, compose_day_probe_chromatic, compose_night_probe_bright, Mood,
};
use pixtuoid_scene::audio::dsp::{NoiseStream, SAMPLE_RATE};
use pixtuoid_scene::audio::synth::gen_beds;

/// The ratified audition mix gains per mood (pad, sparkle, keys, drums,
/// texture — `TRACK_STEMS` order), from the frozen takes' LISTEN mixes.
const DAY_MIX: [f32; 5] = [0.70, 0.60, 0.55, 0.45, 0.30];
const NIGHT_MIX: [f32; 5] = [0.75, 0.55, 0.50, 0.30, 0.30];

/// Soak length target per take — long enough to hear the loop breathe.
const SOAK_SECS: f32 = 90.0;

/// Every take renders at ONE loudness (playlist consistency — the mix
/// audit measured 1.6 LU spread under peak normalization).
const TARGET_RMS_DBFS: f32 = -16.0;

/// A/B probe variants — each changes exactly ONE variable vs the default
/// render, for the owner's ear-vote.
#[derive(Clone, Copy, PartialEq)]
enum Probe {
    /// Chromatic-color day templates (V7/vi, borrowed iv).
    Chroma,
    /// Night with its closed hats at ×2 gain.
    NightBright,
    /// A gentle top-end tilt shelf on the mixdown (~+5dB above ~2kHz) —
    /// APPROXIMATES retuning the tape/stem LPF stack; a vote for this
    /// means implementing the real chain change next.
    Bright,
    /// The texture (hiss+crackle) lane at +9dB — the "air bed audible"
    /// candidate, crackle RATE untouched.
    Air,
}

fn main() {
    let mut mood = Mood::Day;
    let mut seeds = 12u64;
    let mut start = 0u64;
    let mut out = PathBuf::from("audio-demos");
    let mut solo: Option<usize> = None;
    let mut probe: Option<Probe> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--mood" => {
                mood = match args.next().as_deref() {
                    Some("night") => Mood::Night,
                    _ => Mood::Day,
                }
            }
            "--seeds" => seeds = args.next().and_then(|v| v.parse().ok()).unwrap_or(12),
            "--start" => start = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            // fast voice/lane iteration: hear one stem alone
            "--solo" => {
                solo = args.next().as_deref().and_then(|v| {
                    ["pad", "sparkle", "keys", "drums", "texture"]
                        .iter()
                        .position(|&l| l == v)
                })
            }
            "--probe" => {
                probe = match args.next().as_deref() {
                    Some("chroma") => Some(Probe::Chroma),
                    Some("night") => Some(Probe::NightBright),
                    Some("bright") => Some(Probe::Bright),
                    Some("air") => Some(Probe::Air),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    std::fs::create_dir_all(&out).expect("create out dir");

    let tag = match mood {
        Mood::Day => "day",
        Mood::Night => "night",
    };
    let mix = match mood {
        Mood::Day => DAY_MIX,
        Mood::Night => NIGHT_MIX,
    };
    let mut listing = Vec::new();
    for seed in start..start + seeds {
        let score = match probe {
            Some(Probe::Chroma) => compose_day_probe_chromatic(seed),
            Some(Probe::NightBright) => compose_night_probe_bright(seed),
            _ => compose(mood, seed),
        };
        let mut rng = NoiseStream::new(9);
        let beds = gen_beds(&score, &mut rng);
        let take_len = beds[0].len();
        let loops = (SOAK_SECS * SAMPLE_RATE as f32 / take_len as f32).ceil() as usize;
        let total = take_len * loops.max(1);
        let mut mixdown = vec![0.0f32; total];
        for (lane, (bed, mut gain)) in beds.iter().zip(mix).enumerate() {
            if probe == Some(Probe::Air) && lane == 4 {
                gain *= 2.8; // +9dB texture: the air-bed probe
            }
            match solo {
                Some(s) if s != lane => continue,
                Some(_) => {
                    for (i, slot) in mixdown.iter_mut().enumerate() {
                        *slot += bed[i % bed.len()] * 0.8;
                    }
                }
                None => {
                    for (i, slot) in mixdown.iter_mut().enumerate() {
                        *slot += bed[i % bed.len()] * gain;
                    }
                }
            }
        }
        if probe == Some(Probe::Bright) {
            tilt_bright(&mut mixdown);
        }
        rms_normalize(&mut mixdown);
        let prefix = match probe {
            Some(Probe::Chroma) => "probe-chroma_",
            Some(Probe::NightBright) => "probe-night_",
            Some(Probe::Bright) => "probe-bright_",
            Some(Probe::Air) => "probe-air_",
            None => "",
        };
        let path = out.join(format!("{prefix}gen_{tag}_{seed:03}.wav"));
        write_wav(&path, &mixdown);
        println!(
            "  seed {seed:3}  {:>3.0} bpm  lead={:5}  {}",
            score.bpm,
            score.lead_voice_name(),
            path.display()
        );
        listing.push(path);
    }
    println!("\nblind-audition (shuffled order recommended):");
    for p in &listing {
        println!("  afplay {}", p.display());
    }
}

/// One-figure loudness: scale the mixdown to TARGET_RMS_DBFS (gain
/// capped ×4 so a sparse take can't be blown up into noise).
fn rms_normalize(x: &mut [f32]) {
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    let target = 10f32.powf(TARGET_RMS_DBFS / 20.0);
    let g = (target / rms.max(1e-9)).min(4.0);
    for v in x.iter_mut() {
        *v *= g;
    }
}

/// The bright-probe tilt: one-pole highs (~1.2kHz corner) added back at
/// 0.8× ≈ a gentle +5dB shelf above ~2kHz.
fn tilt_bright(x: &mut [f32]) {
    let a = (-std::f32::consts::TAU * 1200.0 / SAMPLE_RATE as f32).exp();
    let mut lp = 0.0f32;
    for s in x.iter_mut() {
        lp = a * lp + (1.0 - a) * *s;
        *s += 0.8 * (*s - lp);
    }
}

/// 16-bit stereo RIFF/WAVE (mono mixdown duplicated L/R) with the
/// audition soft clip — the same tanh(1.1)·0.85 the python gate used.
fn write_wav(path: &PathBuf, mono: &[f32]) {
    let mut w = BufWriter::new(File::create(path).expect("create wav"));
    let n = mono.len() as u32;
    let data_len = n * 4; // 2 channels × i16
    let byte_rate = SAMPLE_RATE * 2 * 2;
    w.write_all(b"RIFF").unwrap();
    w.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    w.write_all(b"WAVEfmt ").unwrap();
    w.write_all(&16u32.to_le_bytes()).unwrap();
    w.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    w.write_all(&2u16.to_le_bytes()).unwrap(); // stereo
    w.write_all(&SAMPLE_RATE.to_le_bytes()).unwrap();
    w.write_all(&byte_rate.to_le_bytes()).unwrap();
    w.write_all(&4u16.to_le_bytes()).unwrap(); // block align
    w.write_all(&16u16.to_le_bytes()).unwrap();
    w.write_all(b"data").unwrap();
    w.write_all(&data_len.to_le_bytes()).unwrap();
    for &s in mono {
        let clipped = (s * 1.1).tanh() * 0.85;
        let pcm = (clipped * 32767.0) as i16;
        w.write_all(&pcm.to_le_bytes()).unwrap();
        w.write_all(&pcm.to_le_bytes()).unwrap();
    }
}
