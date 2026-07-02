//! Render ONE deterministic frame of the live-office hero to a PNG — the
//! site backdrop's poster (#425).
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
//!
//! Driven by the `wasm-still` job kind in `scripts/media.json`; not part of
//! the shipped wasm artifact (an example, native-only). The committed
//! poster's manifest values decode to: t0 = 2026-01-15T17:30:00Z (evening —
//! night skyline, city lights) and advance = 100s (the script's populated
//! plateau: all 7 walk-ins land by 19s, the first walkout fires at 104s).

use std::process::ExitCode;

use pixtuoid_web::Office;

fn arg(args: &[String], name: &str) -> Option<u64> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(out), Some(w), Some(h), Some(t0_ms), Some(advance_ms)) = (
        args.first().filter(|a| !a.starts_with("--")),
        arg(&args, "--width"),
        arg(&args, "--height"),
        arg(&args, "--t0-ms"),
        arg(&args, "--advance-ms"),
    ) else {
        eprintln!(
            "usage: hero_still <out.png> --width W --height H --t0-ms EPOCH_MS --advance-ms MS"
        );
        return ExitCode::FAILURE;
    };

    let mut office = match Office::new(3) {
        Ok(o) => o,
        Err(_) => {
            eprintln!("embedded sprite pack failed to parse (build bug)");
            return ExitCode::FAILURE;
        }
    };
    // First step anchors the script epoch at t0 (only the at_ms=0 beat is due);
    // the second advances the loop so the cast walks in / seats per the beats.
    office.step(t0_ms as f64, w as u32, h as u32);
    office.step((t0_ms + advance_ms) as f64, w as u32, h as u32);

    // The same RGBA contract the JS canvas blit uses (w*h*4, opaque alpha).
    let px = unsafe { std::slice::from_raw_parts(office.frame_ptr(), office.frame_len()) }.to_vec();
    let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, px) else {
        eprintln!("frame length didn't match {w}x{h}*4");
        return ExitCode::FAILURE;
    };
    if let Err(e) = img.save(out) {
        eprintln!("write {out}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
