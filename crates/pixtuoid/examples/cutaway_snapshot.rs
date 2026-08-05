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

/// Default logical office size — the SAME extent whatever `--scale` is, which
/// is the property the whole seam exists for: more pixels, not more desks.
/// `--logical WxH` overrides it; a bigger office is how the size-gated pieces
/// (corridor appliances) get placed at all.
const DEFAULT_LOGICAL: (u16, u16) = (160, 96);

/// Badge type size per unit of render scale. At 4x this is ~10px, which is the
/// smallest Monaspace Neon stays legible at.
const LABEL_PX_PER_SCALE: f32 = 2.6;

/// Working directories the fixture cycles through. Fewer than the desk count on
/// purpose: two agents sharing a repo share an outfit, which is the grouping
/// Team Palette exists to show.
const REPOS: &[&str] = &["/w/pixtuoid", "/w/site", "/w/raycast", "/w/notes"];

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
                // VARIED cwds, like the classic `snapshot` example's fixture:
                // the outfit is cwd-keyed (Team Palette), so one cwd for
                // everyone renders the whole office in a single shirt and hides
                // exactly the grouping the recolor exists to show.
                cwd: Arc::from(PathBuf::from(REPOS[i % REPOS.len()]).as_path()),
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
    let (mut lw, mut lh) = DEFAULT_LOGICAL;
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
            "--logical" => {
                let v = val("--logical")?;
                let (w, h) = v
                    .split_once('x')
                    .ok_or_else(|| anyhow!("--logical wants WxH"))?;
                lw = w.parse().context("bad --logical width")?;
                lh = h.parse().context("bad --logical height")?;
            }
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
        .observe(&scene, &pack, lw, lh, meta, now)
        .ok_or_else(|| anyhow!("{lw}x{lh} does not lay out"))?;
    let layout = Layout::compute_with_seed(lw, lh, None, meta.floor_seed)
        .ok_or_else(|| anyhow!("layout compute failed"))?;

    let (bw, bh) = (scale.to_buffer(lw), scale.to_buffer(lh));
    let mut buf = RgbBuffer::filled(bw, bh, theme.surface.bg_fallback);
    // The recolor cache the classic painter uses — the cutaway blits the SAME
    // per-agent sprites, so it needs the same cache rather than recoloring
    // twelve characters afresh every frame.
    let mut cache = pixtuoid_scene::frame_cache::FrameCache::new();
    let labels = render_cutaway(
        &frame, &layout, &pack, theme, scale, now, &mut cache, &mut buf,
    );

    // Name badges: the engine reports WHERE, the binary owns the font. Drawn
    // straight into the RGB buffer here (the real painters blend post-upscale
    // with a drop shadow — this only has to be legible enough to judge
    // placement).
    let label_px = f32::from(scale.get()) * LABEL_PX_PER_SCALE;
    for l in &labels {
        let Some(agent) = frame.agents.get(l.agent_idx) else {
            continue;
        };
        let text: &str = &agent.label.text();
        // The SHARED tone authority every other label painter uses, so the
        // cutaway cannot invent its own state colours. `label_idle` alone was
        // rgb(65,72,104) on a rgb(36,40,59) floor — technically drawn, visually
        // absent, which is exactly the class of bug a dev tool should not have.
        let tone = pixtuoid_scene::overlay::label_tone_rgb(
            if agent.exiting_at.is_some() {
                pixtuoid_scene::overlay::LabelTone::Exiting
            } else {
                match agent.state {
                    ActivityState::Active { .. } => pixtuoid_scene::overlay::LabelTone::Active,
                    ActivityState::Waiting { .. } => pixtuoid_scene::overlay::LabelTone::Waiting,
                    _ => pixtuoid_scene::overlay::LabelTone::Idle,
                }
            },
            theme,
        );
        let half = pixtuoid::aa_text::text_width(text, label_px) / 2;
        // `draw_text_at` takes a TOP y and draws downward, so the anchor (which
        // marks where the badge should END, just above the head) has to be
        // lifted by a full line or the name lands on the sprite's face.
        let (ox, oy) = (
            i32::from(l.anchor_px.x) - half,
            i32::from(l.anchor_px.y) - pixtuoid::aa_text::line_height(label_px),
        );
        pixtuoid::aa_text::draw_text_at(text, ox, oy, label_px, |x, y, cov| {
            if cov <= 0.0 || x < 0 || y < 0 {
                return;
            }
            let (x, y) = (x as u16, y as u16);
            if x >= buf.width() || y >= buf.height() {
                return;
            }
            let under = buf.get(x, y);
            let a = cov.clamp(0.0, 1.0);
            let mix = |u: u8, t: u8| (f32::from(u) * (1.0 - a) + f32::from(t) * a) as u8;
            let t = tone;
            buf.put(
                x,
                y,
                pixtuoid_core::sprite::Rgb {
                    r: mix(under.r, t.r),
                    g: mix(under.g, t.g),
                    b: mix(under.b, t.b),
                },
            );
        });
    }

    let mut img = RgbImage::new(u32::from(bw), u32::from(bh));
    for (i, px) in buf.as_slice().iter().enumerate() {
        let (x, y) = (i as u32 % u32::from(bw), i as u32 / u32::from(bw));
        img.put_pixel(x, y, ImgRgb([px.r, px.g, px.b]));
    }
    img.save(&out).with_context(|| format!("writing {out}"))?;
    eprintln!(
        "wrote {out} ({bw}x{bh} = {lw}x{lh} logical @{scale_n}x, \
         {} desks, {} characters)",
        layout.home_desks.len(),
        frame.characters.len()
    );
    Ok(())
}
