//! Render ONE frame of the `pixtuoid floating` office to a PNG — visual verification for
//! the floating window (the desktop-window twin of the `snapshot` example, which captures
//! the half-block TUI). It drives the SAME `floating::offscreen::OfficeRenderer` the live
//! window uses AND the same `paint_labels_into_surface`, so the PNG is byte-faithful to what
//! the window blits (full-resolution `RgbBuffer` + name badges, NOT a ▀-compressed grab).
//!
//! Usage:
//!   cargo run --release --example floating_snapshot -- <out.png> [WxH] [--theme <name>] [--agents N]
//! e.g. `... -- /tmp/floating.png 720x480 --agents 6` (Retina default), `... -- /tmp/f.png 360x240`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use image::{Rgb as ImgRgb, RgbImage};
use pixtuoid::floating::offscreen::{paint_labels_into_surface, OfficeRenderer};
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::floor::FloorMeta;
use pixtuoid_scene::theme::theme_by_name;

/// A few demo agents at desks 0..n with varied states + a deliberate label collision
/// (two `cc`) so the snapshot exercises every label tone AND the `·<id4>` disambiguation.
fn populate_demo_agents(scene: &mut SceneState, now: SystemTime, n: usize) {
    let archetypes: [(&str, ActivityState); 6] = [
        (
            "claude-code",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write: src/foo.rs".into()),
                kind: ToolKind::Edit,
            },
        ),
        ("codex", ActivityState::Idle),
        (
            "cc",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
        ),
        (
            "cc",
            ActivityState::Active {
                tool_use_id: Some("tu_d".into()),
                detail: Some("Bash: cargo test".into()),
                kind: ToolKind::Bash,
            },
        ),
        ("reasonix", ActivityState::Idle),
        (
            "opencode",
            ActivityState::Active {
                tool_use_id: Some("tu_e".into()),
                detail: Some("Grep: TODO".into()),
                kind: ToolKind::Search,
            },
        ),
    ];
    // Back-date well past the entry animation + keep `last_event_at` recent so each agent is
    // SEATED at its own desk (Active → typing, Idle → thinking-pose within the 20s window) —
    // spread out, not clustered walking in from the elevator (which overlaps their badges).
    let seated_since = now.checked_sub(Duration::from_secs(120)).unwrap_or(now);
    let recent = now.checked_sub(Duration::from_secs(3)).unwrap_or(now);
    for i in 0..n {
        let (label, state) = &archetypes[i % archetypes.len()];
        let key = format!("{label}-{i}");
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        scene.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: Arc::from("claude-code"),
                session_id: Arc::from(format!("demo-{key}-{i:04x}").as_str()),
                cwd: Arc::from(PathBuf::from("/demo").as_path()),
                label: (*label).into(),
                state: state.clone(),
                state_started_at: seated_since,
                created_at: seated_since,
                last_event_at: recent,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: scene.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
            },
        );
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or_else(|| {
        anyhow!("usage: floating_snapshot <out.png> [WxH] [--theme <name>] [--agents N]")
    })?;

    let mut size = (720u16, 480u16); // Retina default (360x240 logical @2x)
    let mut theme_name = "normal".to_string();
    let mut n_agents = 0usize;
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--theme" => {
                theme_name = rest
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| anyhow!("--theme needs a value"))?;
                i += 2;
            }
            "--agents" => {
                n_agents = rest
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--agents needs a value"))?
                    .parse()
                    .context("bad --agents")?;
                i += 2;
            }
            s if s.contains('x') => {
                let (w, h) = s.split_once('x').unwrap();
                size = (
                    w.parse().context("bad width")?,
                    h.parse().context("bad height")?,
                );
                i += 1;
            }
            other => return Err(anyhow!("unexpected arg: {other}")),
        }
    }

    let theme =
        theme_by_name(&theme_name).ok_or_else(|| anyhow!("unknown --theme {theme_name:?}"))?;
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(None)?;
    let now = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Empty office (default) shows the layout / walls / windows / desks; `--agents N` seats
    // demo agents so the name-badge overlay is exercised too.
    let mut scene = SceneState::uniform(64);
    populate_demo_agents(&mut scene, now, n_agents);
    let mut renderer = OfficeRenderer::new();
    // Mirror floating::window EXACTLY: render the office at window/SCALE, nearest-neighbor
    // upscale into a `u32` surface, then blit the name badges — so the PNG is byte-faithful.
    let (win_w, win_h) = (size.0 as u32, size.1 as u32);
    let scale = pixtuoid::floating::offscreen::office_scale(win_h); // shared with the live window
    let ow = (win_w / scale).max(1).min(u16::MAX as u32) as u16;
    let oh = (win_h / scale).max(1).min(u16::MAX as u32) as u16;
    let buf = renderer.render(&scene, &pack, theme, now, ow, oh, FloorMeta::ground(), None);
    let (bw, bh) = (buf.width() as u32, buf.height() as u32);

    let (ww, wh) = (win_w as usize, win_h as usize);
    let mut sb: Vec<u32> = vec![0; ww * wh];
    for wy in 0..win_h {
        let oy = (wy / scale).min(bh - 1);
        for wx in 0..win_w {
            let ox = (wx / scale).min(bw - 1);
            let p = buf.as_slice()[(oy * bw + ox) as usize];
            sb[wy as usize * ww + wx as usize] =
                (p.r as u32) << 16 | (p.g as u32) << 8 | p.b as u32;
        }
    }
    let labels = renderer.labels(&scene, now);
    paint_labels_into_surface(&mut sb, ww, wh, &labels, scale as i32, theme);

    let mut img = RgbImage::new(win_w, win_h);
    for wy in 0..win_h {
        for wx in 0..win_w {
            let px = sb[wy as usize * ww + wx as usize];
            img.put_pixel(
                wx,
                wy,
                ImgRgb([(px >> 16) as u8, (px >> 8) as u8, px as u8]),
            );
        }
    }
    img.save(&out).with_context(|| format!("writing {out}"))?;
    eprintln!(
        "wrote {out} ({win_w}x{win_h}, office buffer {bw}x{bh} @{scale}x, {n_agents} agents)"
    );
    Ok(())
}
