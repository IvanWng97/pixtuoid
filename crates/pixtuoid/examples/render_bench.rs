//! Frame-cost measurement for the 2.5D design gate: how does the shared scene
//! render scale with buffer pixel count, and what would a rich-graphics
//! (Kitty/iTerm2/SIXEL) resolution cost per frame?
//!
//! NOT a committed gate — a design-gate instrument. Run:
//!   cargo run --release --example render_bench

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use pixtuoid::floating::offscreen::OfficeRenderer;
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::floor::FloorMeta;
use pixtuoid_scene::theme::theme_by_name;

fn populate(scene: &mut SceneState, now: SystemTime, n: usize) {
    let seated = now.checked_sub(Duration::from_secs(120)).unwrap_or(now);
    let recent = now.checked_sub(Duration::from_secs(3)).unwrap_or(now);
    for i in 0..n {
        let id = AgentId::from_transcript_path(&format!("/bench/a{i}.jsonl"));
        let state = match i % 3 {
            0 => ActivityState::Active {
                tool_use_id: Some(format!("tu_{i}").into()),
                detail: Some("Edit: src/lib.rs".into()),
                kind: ToolKind::Edit,
            },
            1 => ActivityState::Idle,
            _ => ActivityState::Waiting {
                reason: "permission?".into(),
            },
        };
        scene.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("bench-{i:04x}").as_str()),
                cwd: Arc::from(PathBuf::from("/bench").as_path()),
                label: "cc".into(),
                state,
                state_started_at: seated,
                created_at: seated,
                last_event_at: recent,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: scene.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                model: None,
                pid: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
}

fn main() -> Result<()> {
    let theme = theme_by_name("normal").expect("normal theme");
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(None)?;
    let base = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // (label, buffer w, h). The rich sizes are what a 192x80-cell terminal needs
    // at real cell resolution (8x16 px/cell) vs the half-block buffer today.
    let cases: [(&str, u16, u16); 5] = [
        ("classic TUI    192x80 cells", 192, 160),
        ("floating today ~360x240", 360, 240),
        ("rich  2x linear", 384, 320),
        ("rich  4x linear", 768, 640),
        ("rich  8x linear (Kitty native)", 1536, 1280),
    ];

    // min-of-N is the estimator here: the machine is shared, and contention only
    // ever ADDS time, so the minimum is the closest honest read of the real cost.
    println!(
        "{:<32} {:>11} {:>10} {:>10} {:>9} {:>10} {:>7}",
        "case", "pixels", "min ms", "p50 ms", "fps@min", "ns/px", "desks"
    );

    for (label, w, h) in cases {
        let mut scene = SceneState::uniform(64);
        populate(&mut scene, base, 12);
        let mut r = OfficeRenderer::new();
        // Warm the caches (layout memo + recolored frames) — steady state is
        // what a running office pays, not the first frame.
        for i in 0..15u64 {
            let now = base + Duration::from_millis(i * 33);
            let _ = r.render(&scene, &pack, theme, now, w, h, FloorMeta::ground(), None);
        }

        const ITERS: u64 = 120;
        let mut samples = Vec::with_capacity(ITERS as usize);
        for i in 0..ITERS {
            // Advance time each frame so animation/motion actually re-derives.
            let now = base + Duration::from_millis((15 + i) * 33);
            let t = Instant::now();
            let _ = r.render(&scene, &pack, theme, now, w, h, FloorMeta::ground(), None);
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        let min = samples[0];
        let p50 = samples[samples.len() / 2];
        let px = w as f64 * h as f64;
        // The office the layout DERIVES at this buffer size — the confound the
        // spec names: buffer pixels ARE layout units today, so a bigger
        // buffer builds a BIGGER office rather than a sharper one.
        let seed = pixtuoid_scene::floor::floor_seed(0);
        let capacity = pixtuoid_scene::floor::floor_capacity(w, h, seed);
        println!(
            "{:<32} {:>11} {:>10.3} {:>10.3} {:>9.0} {:>10.1} {:>7}",
            label,
            format!("{}x{}", w, h),
            min,
            p50,
            1000.0 / min,
            min * 1e6 / px,
            capacity
        );
    }

    // Does frame cost track the AGENTS or the static room? If cost is flat in
    // agent count the room dominates, which makes caching the static layers the
    // one high-leverage optimization (the spec lists it only as a "candidate").
    println!("\ncost vs agent count at 768x640 (is the room or the cast dominant?):");
    for n in [0usize, 4, 12, 30, 60] {
        let mut scene = SceneState::uniform(64);
        populate(&mut scene, base, n);
        let mut r = OfficeRenderer::new();
        for i in 0..15u64 {
            let now = base + Duration::from_millis(i * 33);
            let _ = r.render(
                &scene,
                &pack,
                theme,
                now,
                768,
                640,
                FloorMeta::ground(),
                None,
            );
        }
        let mut best = f64::MAX;
        for i in 0..60u64 {
            let now = base + Duration::from_millis((15 + i) * 33);
            let t = Instant::now();
            let _ = r.render(
                &scene,
                &pack,
                theme,
                now,
                768,
                640,
                FloorMeta::ground(),
                None,
            );
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("  {n:>3} agents: {best:>7.3} ms");
    }

    // Transport arithmetic: what a rich Adapter must push per frame.
    println!("\ntransport cost per frame (uncompressed RGB -> base64, the Kitty/iTerm2 wire):");
    for (label, w, h) in cases {
        let raw = w as f64 * h as f64 * 3.0;
        let b64 = raw * 4.0 / 3.0;
        println!(
            "  {:<32} raw {:>8.2} MiB  base64 {:>8.2} MiB  @30fps {:>8.1} MiB/s",
            label,
            raw / (1024.0 * 1024.0),
            b64 / (1024.0 * 1024.0),
            b64 * 30.0 / (1024.0 * 1024.0)
        );
    }

    // SIXEL is the one protocol that can neither scale nor reuse an uploaded
    // image, so it pays full freight every frame. The RGB->base64 arithmetic
    // above does NOT describe it: sixel is palette-indexed and run-length
    // encoded, so the real wire cost has to be encoded to be known.
    println!("\nSIXEL wire cost (fixed startup palette, full-frame encode):");
    println!(
        "  {:<24} {:>10} {:>11} {:>12} {:>11}",
        "buffer", "encode ms", "KiB/frame", "MiB/s @30fps", "vs raw RGB"
    );
    let mut out = Vec::new();
    for (_, w, h) in cases {
        let mut scene = SceneState::uniform(64);
        populate(&mut scene, base, 12);
        let mut r = OfficeRenderer::new();
        let mut enc_ms = f64::MAX;
        let mut bytes = 0usize;
        for i in 0..12u64 {
            let now = base + Duration::from_millis(i * 33);
            let buf = r.render(&scene, &pack, theme, now, w, h, FloorMeta::ground(), None);
            let (bw, bh) = (buf.width() as usize, buf.height() as usize);
            let t = Instant::now();
            sixel_encode(buf.as_slice(), bw, bh, &mut out);
            enc_ms = enc_ms.min(t.elapsed().as_secs_f64() * 1000.0);
            bytes = out.len();
        }
        let raw = w as f64 * h as f64 * 3.0;
        println!(
            "  {:<24} {:>10.2} {:>11.1} {:>12.1} {:>10.2}x",
            format!("{}x{}", w, h),
            enc_ms,
            bytes as f64 / 1024.0,
            bytes as f64 * 30.0 / (1024.0 * 1024.0),
            bytes as f64 / raw
        );
        // Dump the EXACT pixels just encoded, so an independent encoder
        // (img2sixel) can be run over the same input as a cross-check. Our
        // encoder is an instrument; an instrument nobody validated is a guess.
        if let Ok(dir) = std::env::var("PIXTUOID_BENCH_DUMP") {
            let buf = r.render(&scene, &pack, theme, base, w, h, FloorMeta::ground(), None);
            let (bw, bh) = (buf.width() as u32, buf.height() as u32);
            let mut img = image::RgbImage::new(bw, bh);
            for (i, p) in buf.as_slice().iter().enumerate() {
                img.put_pixel(i as u32 % bw, i as u32 / bw, image::Rgb([p.r, p.g, p.b]));
            }
            let path = format!("{dir}/frame_{w}x{h}.png");
            img.save(&path).ok();
            println!("      dumped {path}");
        }
    }

    Ok(())
}

/// A minimal but faithful SIXEL encoder — enough to measure the real wire cost.
///
/// The palette is a FIXED 6x7x6 colour cube resolved by a shift, not a
/// per-frame quantisation search. That is the measurement's whole premise: if
/// the render scale is pinned at startup, the palette can be pinned with it,
/// so quantising is one table lookup per pixel instead of a nearest-colour
/// search. Output shape follows the DEC format: `DCS q`, palette definitions,
/// then one band per six pixel rows, each band emitting one run-length-encoded
/// row per colour present in it.
fn sixel_encode(px: &[pixtuoid_core::sprite::Rgb], w: usize, h: usize, out: &mut Vec<u8>) {
    /// 6x7x6 = 252 entries, under the 256 registers a sixel palette addresses.
    const LEVELS_R: usize = 6;
    const LEVELS_G: usize = 7;
    const LEVELS_B: usize = 6;
    const PALETTE_LEN: usize = LEVELS_R * LEVELS_G * LEVELS_B;
    /// A sixel data byte is the 6-bit column pattern offset into printable ASCII.
    const SIXEL_BASE: u8 = 63;
    /// Below this a literal run is shorter than the `!<count><char>` form.
    const RLE_MIN_RUN: usize = 4;

    let index_of = |p: &pixtuoid_core::sprite::Rgb| -> usize {
        let r = p.r as usize * LEVELS_R / 256;
        let g = p.g as usize * LEVELS_G / 256;
        let b = p.b as usize * LEVELS_B / 256;
        (r * LEVELS_G + g) * LEVELS_B + b
    };

    out.clear();
    out.extend_from_slice(b"\x1bPq");
    for i in 0..PALETTE_LEN {
        let r = (i / (LEVELS_G * LEVELS_B)) * 100 / (LEVELS_R - 1);
        let g = (i / LEVELS_B % LEVELS_G) * 100 / (LEVELS_G - 1);
        let b = (i % LEVELS_B) * 100 / (LEVELS_B - 1);
        out.extend_from_slice(format!("#{i};2;{r};{g};{b}").as_bytes());
    }

    let mut band = vec![0usize; w * 6];
    let mut row = vec![0u8; w];
    let mut y = 0;
    while y < h {
        let rows = 6.min(h - y);
        let mut present = [false; PALETTE_LEN];
        for i in 0..rows {
            for x in 0..w {
                let idx = index_of(&px[(y + i) * w + x]);
                band[i * w + x] = idx;
                present[idx] = true;
            }
        }
        let mut first = true;
        for (color, _) in present.iter().enumerate().filter(|(_, p)| **p) {
            if !first {
                out.push(b'$'); // carriage return: overlay the next colour
            }
            first = false;
            out.extend_from_slice(format!("#{color}").as_bytes());
            for x in 0..w {
                let mut bits = 0u8;
                for i in 0..rows {
                    if band[i * w + x] == color {
                        bits |= 1 << i;
                    }
                }
                row[x] = SIXEL_BASE + bits;
            }
            let mut x = 0;
            while x < w {
                let c = row[x];
                let mut run = 1;
                while x + run < w && row[x + run] == c {
                    run += 1;
                }
                if run >= RLE_MIN_RUN {
                    out.extend_from_slice(format!("!{run}").as_bytes());
                    out.push(c);
                } else {
                    for _ in 0..run {
                        out.push(c);
                    }
                }
                x += run;
            }
        }
        out.push(b'-'); // next band
        y += 6;
    }
    out.extend_from_slice(b"\x1b\\");
}
