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
use std::process::ExitCode;

use pixtuoid_scene::audio::compose::{compose, Mood};
use pixtuoid_scene::audio::dsp::{NoiseStream, SAMPLE_RATE};
use pixtuoid_scene::audio::synth::gen_beds;

/// The ratified audition mix gains per mood (pad, sparkle, keys, drums,
/// texture — `TRACK_STEMS` order), from the frozen takes' LISTEN mixes.
const DAY_MIX: [f32; 5] = [0.70, 0.60, 0.55, 0.45, 0.84];
const NIGHT_MIX: [f32; 5] = [0.75, 0.55, 0.50, 0.30, 0.84];

/// Soak length target per take — long enough to hear the loop breathe.
const SOAK_SECS: f32 = 90.0;

/// Every take renders at ONE loudness (playlist consistency — the mix
/// audit measured 1.6 LU spread under peak normalization).
const TARGET_RMS_DBFS: f32 = -16.0;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lofi_audition: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::io::Result<()> {
    let mut mood = Mood::Day;
    let mut seeds = 12u64;
    let mut start = 0u64;
    let mut out = PathBuf::from("audio-demos");
    let mut solo: Option<usize> = None;
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
            _ => {}
        }
    }
    std::fs::create_dir_all(&out)?;

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
        let score = compose(mood, seed);
        let mut rng = NoiseStream::new(9);
        let beds = gen_beds(&score, &mut rng);
        let take_len = beds[0].len();
        let loops = (SOAK_SECS * SAMPLE_RATE as f32 / take_len as f32).ceil() as usize;
        let total = take_len * loops.max(1);
        let mut mixdown = vec![0.0f32; total];
        for (lane, (bed, gain)) in beds.iter().zip(mix).enumerate() {
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
        rms_normalize(&mut mixdown);
        let path = out.join(format!("gen_{tag}_{seed:03}.wav"));
        write_wav(&path, &mixdown)?;
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
    Ok(())
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

/// 16-bit stereo RIFF/WAVE (mono mixdown duplicated L/R) with the
/// audition soft clip — the same tanh(1.1)·0.85 the python gate used.
fn write_wav(path: &PathBuf, mono: &[f32]) -> std::io::Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let n = mono.len() as u32;
    let data_len = n * 4; // 2 channels × i16
    let byte_rate = SAMPLE_RATE * 2 * 2;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&2u16.to_le_bytes())?; // stereo
    w.write_all(&SAMPLE_RATE.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&4u16.to_le_bytes())?; // block align
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for &s in mono {
        let clipped = (s * 1.1).tanh() * 0.85;
        let pcm = (clipped * 32767.0) as i16;
        w.write_all(&pcm.to_le_bytes())?;
        w.write_all(&pcm.to_le_bytes())?;
    }
    Ok(())
}
