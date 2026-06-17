//! Render ONE frame of the `pixtuoid float` office to a PNG — visual verification for
//! the float window (the desktop-window twin of the `snapshot` example, which captures
//! the half-block TUI). It drives the SAME `float::offscreen::OfficeRenderer` the live
//! window uses, so the PNG is byte-faithful to what the window blits (full-resolution
//! `RgbBuffer`, NOT a ▀-compressed terminal grab).
//!
//! Usage:
//!   cargo run --release --example float_snapshot -- <out.png> [WxH] [--theme <name>]
//! e.g. `... -- /tmp/float.png 720x480` (Retina default), `... -- /tmp/f.png 360x240`.

use anyhow::{anyhow, Context, Result};
use image::{Rgb as ImgRgb, RgbImage};
use pixtuoid::float::offscreen::OfficeRenderer;
use pixtuoid::tui::floor::FloorMeta;
use pixtuoid::tui::theme::theme_by_name;
use pixtuoid_core::state::SceneState;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .ok_or_else(|| anyhow!("usage: float_snapshot <out.png> [WxH] [--theme <name>]"))?;

    let mut size = (720u16, 480u16); // Retina default (360x240 logical @2x)
    let mut theme_name = "normal".to_string();
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
    let pack = pixtuoid::tui::embedded_pack::load_sprite_pack(None)?;
    let now = std::time::SystemTime::now();

    // Empty office: shows the layout / walls / windows / desks / pantry / corridor — the
    // surfaces a polish pass cares about. (Agents ride the live scene; not needed here.)
    let scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
    let mut renderer = OfficeRenderer::new();
    // Mirror float::window: render the office at window/SCALE, then nearest-neighbor
    // upscale — so the PNG is byte-faithful to what the live window blits at this size.
    let (win_w, win_h) = (size.0 as u32, size.1 as u32);
    let scale = (win_h as f64 / 180.0).round().max(1.0) as u32; // window::OFFICE_TARGET_H
    let ow = (win_w / scale).max(1).min(u16::MAX as u32) as u16;
    let oh = (win_h / scale).max(1).min(u16::MAX as u32) as u16;
    let buf = renderer.render(&scene, &pack, theme, now, ow, oh, FloorMeta::ground(), None);
    let (bw, bh) = (buf.width as u32, buf.height as u32);

    let mut img = RgbImage::new(win_w, win_h);
    for wy in 0..win_h {
        let oy = (wy / scale).min(bh - 1);
        for wx in 0..win_w {
            let ox = (wx / scale).min(bw - 1);
            let p = buf.pixels[(oy * bw + ox) as usize];
            img.put_pixel(wx, wy, ImgRgb([p.r, p.g, p.b]));
        }
    }
    img.save(&out).with_context(|| format!("writing {out}"))?;
    eprintln!("wrote {out} ({win_w}x{win_h}, office buffer {bw}x{bh} @{scale}x)");
    Ok(())
}
