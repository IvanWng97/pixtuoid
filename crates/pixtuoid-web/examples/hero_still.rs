//! Render ONE deterministic frame of the live-office hero to a PNG — the
//! site backdrop's poster (#425), and the VIBING-playground poster (#468).
//!
//! The poster used to be a terminal-cell render at a ~1.18:1 aspect; under
//! `object-fit: cover` a wide viewport cropped ~60% of its height and the
//! poster→canvas crossfade visibly reframed. This renders the ACTUAL wasm
//! hero — same `Office`, same seed-3 layout, same looped script, at the
//! 320×180 buffer a 16:9 viewport's canvas computes — so the fade dissolves
//! in place.
//!
//! Determinism: a FIXED `--t0-ms` (calendar epoch — pins the wall clock,
//! day/night, and the 10-min weather slot; run under TZ=UTC, which
//! `scripts/gen-media.py` pins process-wide) + a fixed `--advance-ms` (the
//! loop phase: which beats have fired, who is seated). Same args → the same
//! bytes, so `gen-check` pixel-gates the committed poster like every still.
//! `--hour <0-23>` is a convenience alternative to `--t0-ms`: it maps to a
//! `t0_ms` on a FIXED reference calendar date (not "now"), so the same hour
//! always resolves to the same epoch regardless of the machine or day the
//! render runs — see `hour_to_t0_ms`. `--weather <name>` forces
//! `Office::set_weather` before stepping, for a specific-condition poster
//! (e.g. a clear dusk) independent of whatever the natural weather clock
//! would pick at that instant.
//!
//! Driven by the `wasm-still` job kind in `scripts/media.json`; not part of
//! the shipped wasm artifact (an example, native-only). The committed
//! poster's manifest values decode to: t0 = 2026-01-15T17:30:00Z (evening —
//! night skyline, city lights) and advance = 100s (the script's populated
//! plateau: all 7 walk-ins land by 19s, the first walkout fires at 104s).

use std::process::ExitCode;

use chrono::TimeZone;
use pixtuoid_web::Office;

const USAGE: &str = "usage: hero_still <out.png> [--width W] [--height H] \
(--t0-ms EPOCH_MS | --hour 0-23) [--advance-ms MS] [--weather NAME] [--seed N] \
(<out.png> and the flags may appear in any order)";

// Recognized flags, each followed by one value token — used to skip past
// flag/value pairs when hunting for the lone positional (<out.png>), since
// callers put it first (the original contract) or last (the --hour/--weather
// verify in the task brief) interchangeably.
const FLAGS_WITH_VALUE: &[&str] = &[
    "--width",
    "--height",
    "--t0-ms",
    "--advance-ms",
    "--hour",
    "--weather",
    "--seed",
];

fn positional_out(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if FLAGS_WITH_VALUE.contains(&a) {
            i += 2;
        } else if !a.starts_with("--") {
            return Some(a);
        } else {
            i += 1;
        }
    }
    None
}

// The wasm hero's canvas buffer for a 16:9 viewport (see the module doc) —
// the default so `--hour`/`--weather` runs don't have to spell out the same
// dimensions the committed poster already uses.
const DEFAULT_WIDTH: u32 = 320;
const DEFAULT_HEIGHT: u32 = 180;
// Matches the committed poster's "populated plateau" advance (see the module
// doc) — a reasonable default so an `--hour` render also shows a seated cast
// rather than an empty office at t0.
const DEFAULT_ADVANCE_MS: u64 = 100_000;
// Layout seed. The hero backdrop (OfficeBackdrop.astro) is seed 3, so the
// default keeps `hero-wide.png` byte-identical; a caller passes `--seed` to
// match a DIFFERENT live canvas (e.g. the VIBING channel is seed 11) so its
// poster shows the same office layout the live office will paint.
const DEFAULT_SEED: u32 = 3;

// A fixed reference calendar date (arbitrary but FIXED — never "today") used
// to turn `--hour` into a deterministic `t0_ms`. Only the hour-of-day drives
// the poster's sun-disc height / sky color; pinning day/month/year keeps the
// render byte-identical across machines and days, the same guarantee
// `--t0-ms` gives callers who spell out the epoch by hand.
const T0_REFERENCE_YMD: (i32, u32, u32) = (2024, 6, 1);

fn arg<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

/// `hour` (0-23) → epoch millis on the fixed reference date, at `:00:00` UTC.
/// The office's sky decodes `t0_ms` via `chrono::Local`
/// (`pixtuoid_scene::pixel_painter::background`); the gen pipeline pins the
/// process to `TZ=UTC`, so `Utc` here lands on that same local hour.
fn hour_to_t0_ms(hour: u32) -> Option<f64> {
    let (y, m, d) = T0_REFERENCE_YMD;
    chrono::Utc
        .with_ymd_and_hms(y, m, d, hour, 0, 0)
        .single()
        .map(|dt| dt.timestamp_millis() as f64)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = positional_out(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let width: u32 = arg(&args, "--width").unwrap_or(DEFAULT_WIDTH);
    let height: u32 = arg(&args, "--height").unwrap_or(DEFAULT_HEIGHT);
    let advance_ms: u64 = arg(&args, "--advance-ms").unwrap_or(DEFAULT_ADVANCE_MS);
    let seed: u32 = arg(&args, "--seed").unwrap_or(DEFAULT_SEED);
    let weather: Option<String> = arg(&args, "--weather");

    let t0_ms: f64 = match (arg::<u64>(&args, "--t0-ms"), arg::<u32>(&args, "--hour")) {
        (Some(t0), _) => t0 as f64,
        (None, Some(hour)) => match hour_to_t0_ms(hour) {
            Some(t0) => t0,
            None => {
                eprintln!("--hour must be 0-23, got {hour}");
                return ExitCode::FAILURE;
            }
        },
        (None, None) => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let mut office = match Office::new(seed) {
        Ok(o) => o,
        Err(_) => {
            eprintln!("embedded sprite pack failed to parse (build bug)");
            return ExitCode::FAILURE;
        }
    };
    if let Some(name) = weather {
        office.set_weather(Some(name));
    }
    // First step anchors the script epoch at t0 (only the at_ms=0 beat is due);
    // the second advances the loop so the cast walks in / seats per the beats.
    office.step(t0_ms, width, height);
    office.step(t0_ms + advance_ms as f64, width, height);

    // The same RGBA contract the JS canvas blit uses (w*h*4, opaque alpha) —
    // via the safe native accessor; only the wasm-JS boundary reads ptr/len.
    let px = office.frame().to_vec();
    let Some(img) = image::RgbaImage::from_raw(width, height, px) else {
        eprintln!("frame length didn't match {width}x{height}*4");
        return ExitCode::FAILURE;
    };
    if let Err(e) = img.save(out) {
        eprintln!("write {out}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
