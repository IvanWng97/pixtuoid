//! Render ONE frame of the `pixtuoid floating` office to a PNG — visual verification for
//! the floating window. It drives the SAME `window_geometry`, `OfficeRenderer`,
//! `XrgbSurface` upscale and footer painter the live window uses, so the PNG is
//! byte-faithful to what it blits.
//!
//! Usage:
//!   `cargo run --release --example floating_snapshot -- <out.png> [WxH] [--theme <name>] [--agents N] [--hover X,Y] [--pet cat|dog] [--drag X0,Y0:X1,Y1 [--drop-after MS]]`
//! `--drag` presses at the first window point and carries what it lifts to the second,
//! drawing it in hand; with `--drop-after` it releases there and draws `MS` later.
//! e.g. `... -- /tmp/f.png --agents 6` (`config::FLOATING_DEFAULT_{W,H}` × `RETINA_SCALE_FACTOR`),
//! `... -- /tmp/f.png 960x640`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use image::{Rgb as ImgRgb, RgbImage};
use pixtuoid::dev::{OfficeRenderer, WindowFrame, XrgbSurface, window_geometry};
use pixtuoid_core::state::{ActivityState, SceneState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex};
use pixtuoid_scene::cutaway::Face;
use pixtuoid_scene::floor::{FloorInputs, FloorMeta, PetInputs};
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
        u16::try_from(pixtuoid::dev::FLOATING_DEFAULT_W * RETINA_SCALE_FACTOR)?,
        u16::try_from(pixtuoid::dev::FLOATING_DEFAULT_H * RETINA_SCALE_FACTOR)?,
    );
    let mut theme_name = "normal".to_string();
    let mut n_agents = 0usize;
    let mut hover: Option<(f64, f64)> = None;
    let mut pet: Option<pixtuoid_scene::pet::PetKind> = None;
    let mut drag: Option<((f64, f64), (f64, f64))> = None;
    let mut drop_after: Option<u64> = None;
    let point = |v: &str, flag: &str| -> Result<(f64, f64)> {
        let (x, y) = v
            .split_once(',')
            .ok_or_else(|| anyhow!("{flag} needs X,Y window px"))?;
        Ok((x.parse().context("bad x")?, y.parse().context("bad y")?))
    };
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
            "--hover" => {
                let (x, y) = rest
                    .get(i + 1)
                    .and_then(|v| v.split_once(','))
                    .ok_or_else(|| anyhow!("--hover needs X,Y window px"))?;
                hover = Some((
                    x.parse().context("bad --hover x")?,
                    y.parse().context("bad --hover y")?,
                ));
                i += 2;
            }
            "--pet" => {
                pet = Some(match rest.get(i + 1).map(String::as_str) {
                    Some("cat") => pixtuoid_scene::pet::PetKind::Cat,
                    Some("dog") => pixtuoid_scene::pet::PetKind::Dog,
                    _ => return Err(anyhow!("--pet needs cat or dog")),
                });
                i += 2;
            }
            "--drag" => {
                let (from, to) = rest
                    .get(i + 1)
                    .and_then(|v| v.split_once(':'))
                    .ok_or_else(|| anyhow!("--drag needs X0,Y0:X1,Y1"))?;
                drag = Some((point(from, "--drag")?, point(to, "--drag")?));
                i += 2;
            }
            "--drop-after" => {
                drop_after = Some(
                    rest.get(i + 1)
                        .ok_or_else(|| anyhow!("--drop-after needs ms"))?
                        .parse()
                        .context("bad --drop-after")?,
                );
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
    renderer.set_pets(
        pet.map(|kind| pixtuoid_scene::pet::Pet {
            kind,
            name: kind.default_name().to_string(),
        })
        .into_iter()
        .collect(),
    );
    let (win_w, win_h) = (u32::from(size.0), u32::from(size.1));
    let at = window_geometry(
        winit::dpi::PhysicalSize::new(win_w, win_h),
        pack.max_density_variant(),
    );
    let render = |renderer: &mut OfficeRenderer, now| {
        renderer.render(
            at,
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place {
                    gateway: pixtuoid_scene::tally::office_gateway(&scene),
                    floor: None,
                },
            },
        );
    };
    render(&mut renderer, now);
    let mut now = now;
    if let Some((from, to)) = drag {
        let window = (win_w, win_h);
        renderer.press_at(
            from,
            window,
            at,
            pixtuoid::dev::Pressing {
                scale_factor: 1.0,
                petting: None,
                now,
            },
        );
        if !renderer.pointer_moved(to, at) {
            return Err(anyhow!("--drag lifted nothing at {from:?}"));
        }
        now += Duration::from_millis(100);
        render(&mut renderer, now);
        if let Some(ms) = drop_after {
            renderer.release(to, at);
            // The drop lands on the frame after; the walk then runs `ms`.
            now += Duration::from_millis(100);
            render(&mut renderer, now);
            let step = Duration::from_millis(100);
            let end = now + Duration::from_millis(ms);
            while now < end {
                now = (now + step).min(end);
                render(&mut renderer, now);
            }
        }
    }
    let buf = renderer.buf().context("a frame")?;
    let (ww, wh) = (win_w as usize, win_h as usize);
    let mut sb: Vec<u32> = vec![0; ww * wh];
    let mut surf = XrgbSurface::new(&mut sb, ww, wh).expect("sized to the window");
    surf.fill_upscaled(buf, usize::from(at.upscale()));
    let (bw, bh) = (buf.width(), buf.height());
    // Audible so the ♩ suffix shows; no transient flash in a static snapshot.
    let cell = Face::chrome(at);
    let budget = pixtuoid::dev::footer_budget(ww, cell);
    let footer = renderer.footer(&scene, budget, true, None, None);
    pixtuoid::dev::paint_footer_into_surface(&mut surf, &footer, theme, at);
    if let Some(cursor) = hover {
        let world = FloorInputs {
            scene: &scene,
            pack: &pack,
            now,
            floor: FloorMeta::ground(),
            pets: PetInputs::default(),
        };
        if let Some(tip) = renderer
            .hit_at(cursor, at)
            .and_then(|hit| pixtuoid_scene::tooltip::for_hit(hit, &world))
        {
            pixtuoid::dev::paint_tooltip_into_surface(&mut surf, &tip, cursor, theme, cell);
        }
    }

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
        "wrote {out} ({win_w}x{win_h}, office buffer {bw}x{bh} upscaled {}x, {n_agents} agents)",
        at.upscale()
    );
    Ok(())
}
