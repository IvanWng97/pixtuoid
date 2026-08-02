//! Render one frame of the CUTAWAY profile to a PNG — the first time the 2.5D
//! work is visible rather than merely tested.
//!
//! It drives the real seam end to end: the real sim produces a `SimFrame`, the
//! real layout is computed at LOGICAL size, and `render_cutaway` paints it into
//! a buffer sized in pixels at `--scale`. Nothing here is a stand-in, so what it
//! shows is what the profile actually does today.
//!
//! Usage:
//!   cargo run --release --example cutaway_snapshot -- <out.png> [--scale N]
//!                                                    [--agents N] [--theme T]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use image::{Rgb as ImgRgb, RgbImage};
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::cutaway::paint::render_cutaway;
use pixtuoid_scene::floor::{FloorMeta, FloorSession};
use pixtuoid_scene::layout::Layout;
use pixtuoid_scene::render_scale::RenderScale;
use pixtuoid_scene::theme::theme_by_name;

/// Logical office size — the SAME extent whatever `--scale` is, which is the
/// property the whole seam exists for: more pixels, not more desks.
const LOGICAL_W: u16 = 160;
/// Logical office height. See [`LOGICAL_W`].
const LOGICAL_H: u16 = 96;

fn populate(scene: &mut SceneState, now: SystemTime, n: usize) {
    let seated = now.checked_sub(Duration::from_secs(120)).unwrap_or(now);
    let recent = now.checked_sub(Duration::from_secs(3)).unwrap_or(now);
    for i in 0..n {
        let id = AgentId::from_transcript_path(&format!("/cutaway/a{i}.jsonl"));
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
                session_id: Arc::from(format!("cut-{i:04x}").as_str()),
                cwd: Arc::from(PathBuf::from("/cutaway").as_path()),
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
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .ok_or_else(|| anyhow!("usage: cutaway_snapshot <out.png> [--scale N] [--agents N]"))?;

    let (mut scale_n, mut agents, mut theme_name) = (4u16, 10usize, "tokyo-night".to_string());
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        let val = |k: &str| -> Result<String> {
            rest.get(i + 1)
                .cloned()
                .ok_or_else(|| anyhow!("{k} needs a value"))
        };
        match rest[i].as_str() {
            "--scale" => scale_n = val("--scale")?.parse().context("bad --scale")?,
            "--agents" => agents = val("--agents")?.parse().context("bad --agents")?,
            "--theme" => theme_name = val("--theme")?,
            other => return Err(anyhow!("unexpected arg: {other}")),
        }
        i += 2;
    }
    let scale = RenderScale::new(scale_n).ok_or_else(|| anyhow!("--scale must be nonzero"))?;
    let theme =
        theme_by_name(&theme_name).ok_or_else(|| anyhow!("unknown theme {theme_name:?}"))?;
    let pack = pixtuoid_scene::embedded_pack::load_sprite_pack(None)?;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let meta = FloorMeta::ground();

    let mut scene = SceneState::uniform(64);
    populate(&mut scene, now, agents);

    // The real sim, at LOGICAL size — the cutaway is its second reader.
    let mut session = FloorSession::new();
    let frame = session
        .observe(&scene, &pack, LOGICAL_W, LOGICAL_H, meta, now)
        .ok_or_else(|| anyhow!("{LOGICAL_W}x{LOGICAL_H} does not lay out"))?;
    let layout = Layout::compute_with_seed(LOGICAL_W, LOGICAL_H, None, meta.floor_seed)
        .ok_or_else(|| anyhow!("layout compute failed"))?;

    let (bw, bh) = (scale.to_buffer(LOGICAL_W), scale.to_buffer(LOGICAL_H));
    let mut buf = RgbBuffer::filled(bw, bh, theme.surface.bg_fallback);
    render_cutaway(&frame, &layout, &pack, theme, scale, &mut buf);

    let mut img = RgbImage::new(u32::from(bw), u32::from(bh));
    for (i, px) in buf.as_slice().iter().enumerate() {
        let (x, y) = (i as u32 % u32::from(bw), i as u32 / u32::from(bw));
        img.put_pixel(x, y, ImgRgb([px.r, px.g, px.b]));
    }
    img.save(&out).with_context(|| format!("writing {out}"))?;
    eprintln!(
        "wrote {out} ({bw}x{bh} = {LOGICAL_W}x{LOGICAL_H} logical @{scale_n}x, \
         {} desks, {} characters)",
        layout.home_desks.len(),
        frame.characters.len()
    );
    Ok(())
}
