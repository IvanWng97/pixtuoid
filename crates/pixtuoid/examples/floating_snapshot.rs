//! Render ONE frame of the `pixtuoid floating` office to a PNG — visual verification for
//! the floating window. It drives the SAME `OfficeRenderer`, `XrgbSurface` upscale and
//! overlay painters the live window uses, so the PNG is byte-faithful to what it blits.
//!
//! Usage:
//!   `cargo run --release --example floating_snapshot -- <out.png> [WxH] [--theme <name>] [--agents N]`
//! e.g. `... -- /tmp/f.png --agents 6` (`config::FLOATING_DEFAULT_{W,H}` × `RETINA_SCALE_FACTOR`),
//! `... -- /tmp/f.png 360x240`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use image::{Rgb as ImgRgb, RgbImage};
use pixtuoid::floating::offscreen::{
    OfficeRenderer, XrgbSurface, paint_labels_into_surface, window_buffer_geometry,
};
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::floor::{FloorInputs, FloorMeta, PetInputs};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::look::RenderInputs;
use pixtuoid_scene::theme::theme_by_name;

/// The two `cc` labels are a DELIBERATE collision, so the snapshot exercises the
/// `·<id4>` disambiguation as well as every label tone.
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
    // Back-dated past the entry animation with a recent `last_event_at`, so every agent is
    // SEATED and spread out rather than clustered walking in from the elevator, which
    // overlaps their badges.
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
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or_else(|| {
        anyhow!("usage: floating_snapshot <out.png> [WxH] [--theme <name>] [--agents N]")
    })?;

    const RETINA_SCALE_FACTOR: u32 = 2;
    let mut size = (
        u16::try_from(pixtuoid::config::FLOATING_DEFAULT_W * RETINA_SCALE_FACTOR)?,
        u16::try_from(pixtuoid::config::FLOATING_DEFAULT_H * RETINA_SCALE_FACTOR)?,
    );
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
    let pack = std::sync::Arc::new(pixtuoid_scene::pack::load_bundled_pack()?);
    let now = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let mut scene = SceneState::uniform(64);
    populate_demo_agents(&mut scene, now, n_agents);
    let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
    let (win_w, win_h) = (size.0 as u32, size.1 as u32);
    let (scale, ow, oh) = window_buffer_geometry(winit::dpi::PhysicalSize::new(win_w, win_h));
    let buf = renderer
        .render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w: ow, h: oh },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        })
        .expect("a frame");
    let (ww, wh) = (win_w as usize, win_h as usize);
    let mut sb: Vec<u32> = vec![0; ww * wh];
    let mut surf = XrgbSurface::new(&mut sb, ww, wh).expect("sized to the window");
    surf.fill_upscaled(buf, scale as usize);
    let (bw, bh) = (buf.width(), buf.height());
    paint_labels_into_surface(&mut surf, renderer.texts(), scale as i32);
    let board = renderer.board(
        &scene,
        pixtuoid_scene::floor::FloorMeta::ground().motion,
        now,
    );
    pixtuoid::floating::offscreen::paint_wall_board_into_surface(
        &mut surf,
        &board,
        scale as i32,
        theme,
    );
    // Audible so the ♩ suffix shows; no transient flash in a static snapshot.
    let budget = pixtuoid::floating::offscreen::footer_budget(ww);
    let footer = renderer.footer(&scene, budget, true, None);
    pixtuoid::floating::offscreen::paint_footer_into_surface(&mut surf, &footer, theme);

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
