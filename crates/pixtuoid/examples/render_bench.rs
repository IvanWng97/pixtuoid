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
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = samples[0];
        let p50 = samples[samples.len() / 2];
        let px = w as f64 * h as f64;
        // The office the layout DERIVES at this buffer size — the confound the
        // spec's §19.1 names: buffer pixels ARE layout units today, so a bigger
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

    Ok(())
}
