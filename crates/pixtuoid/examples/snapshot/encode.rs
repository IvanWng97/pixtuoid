use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame as GifFrame, Rgb as ImgRgb, RgbImage, Rgba, RgbaImage};
use pixtuoid::tui::renderer::{draw_scene, DrawCtx};
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::SceneState;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

use crate::{due_navigations, SnapshotArgs, CELL_H, CELL_W};

/// Tint every non-walkable terminal cell red and print a connectedness
/// report. A non-walkable cell = either of its two half-block pixels is
/// blocked in the mask. Bright red FG = top pixel blocked; bright red BG
/// = bottom pixel blocked.
///
/// Also runs a BFS from the door threshold and prints how many walkable
/// pixels are reachable vs total — if the two numbers differ, the mask
/// has an isolated region and A* will fall back to a straight line when
/// crossing into it. That's the root cause of any remaining "闪现"
/// (character teleport) the user sees.
pub(crate) fn debug_paint_walkable_overlay(term: &mut Terminal<TestBackend>) -> Result<()> {
    use pixtuoid_scene::layout::SceneLayout;

    let size = term.size()?;
    let scene_w = size.width;
    let scene_h = size.height.saturating_sub(1);
    let buf_w = scene_w;
    let buf_h = scene_h * 2;
    // `None` = the SAME fill the renderer's draw_scene passes — the overlay
    // must mirror the real layout exactly (desks stamp the walkable mask).
    let Some(layout) = SceneLayout::compute(buf_w, buf_h, None) else {
        println!("(debug_walkable) layout too small to compute");
        return Ok(());
    };

    // BFS reachability from door_threshold (always inside the corridor,
    // always walkable by construction).
    let reach_mask = compute_reachable(&layout);
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut reachable = 0usize;
    let mut walkable_total = 0usize;
    let mut sample_disconnects: Vec<(u16, u16)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if layout.is_walkable(x as u16, y as u16) {
                walkable_total += 1;
                if reach_mask[y * w + x] {
                    reachable += 1;
                } else if sample_disconnects.len() < 10 {
                    sample_disconnects.push((x as u16, y as u16));
                }
            }
        }
    }
    let disconnected = walkable_total.saturating_sub(reachable);
    println!(
        "--- walkability report ---\n\
        total walkable pixels   : {walkable_total}\n\
        reachable from threshold: {reachable}\n\
        disconnected pixels     : {disconnected}{}",
        if disconnected == 0 {
            "  ✓ all open areas connected"
        } else {
            "  ⚠ disconnected components present"
        }
    );
    if !sample_disconnects.is_empty() {
        print!("sample disconnected   : ");
        for (i, (x, y)) in sample_disconnects.iter().enumerate() {
            if i > 0 {
                print!(", ");
            }
            print!("({x},{y})");
        }
        println!();
        // Probe the door-threshold neighborhood + the suspected bridge
        // pixel so we can spot which step of the chain is actually blocked.
        let probe = |x: u16, y: u16, name: &str| {
            let wk = layout.is_walkable(x, y);
            let r = is_reachable(&reach_mask, &layout, x, y);
            println!("  probe {name} ({x},{y}): walkable={wk} reachable={r}");
        };
        if let Some(t) = layout.door_threshold {
            probe(t.x, t.y, "threshold");
        }
        probe(0, layout.top_margin, "MR top-left");
        // Probe the row y=66 (pantry's last row above baseboard).
        println!("row y=66 walkability:");
        for x in 0..30u16 {
            let w = layout.is_walkable(x, 66);
            let r = is_reachable(&reach_mask, &layout, x, 66);
            println!("  x={x}: walk={w} reach={r}");
        }
    }

    // No cell-level redraw: the live `w` pixel overlay (painted into the
    // RgbBuffer in draw_scene) already visualizes the mask + approach/seat
    // markers + routes at pixel resolution. A crude full-cell wash here would
    // just overwrite it. The text report above is the unique value this pass
    // adds (the BFS isolated-region "闪现" detector), so keep that and stop.
    Ok(())
}

fn compute_reachable(layout: &pixtuoid_scene::layout::SceneLayout) -> Vec<bool> {
    use std::collections::VecDeque;
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut visited = vec![false; w * h];
    let Some(start) = layout.door_threshold else {
        return visited;
    };
    if !layout.is_walkable(start.x, start.y) {
        return visited;
    }
    let (sx, sy) = (start.x as usize, start.y as usize);
    visited[sy * w + sx] = true;
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((sx, sy));
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if nx >= w || ny >= h || visited[ny * w + nx] {
                continue;
            }
            if !layout.is_walkable(nx as u16, ny as u16) {
                continue;
            }
            visited[ny * w + nx] = true;
            queue.push_back((nx, ny));
        }
    }
    visited
}

fn is_reachable(
    mask: &[bool],
    layout: &pixtuoid_scene::layout::SceneLayout,
    x: u16,
    y: u16,
) -> bool {
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let (xi, yi) = (x as usize, y as usize);
    if xi >= w || yi >= h {
        return false;
    }
    mask[yi * w + xi]
}

pub(crate) fn compute_crop_rect(
    args: &SnapshotArgs,
    scene: &SceneState,
    history: &pixtuoid_scene::pose::PoseHistory,
    cols: u16,
    rows: u16,
    now: SystemTime,
) -> Result<Option<ratatui::layout::Rect>> {
    use pixtuoid_scene::layout::WaypointKind;

    // Fail loudly like --theme/--weather above — a typo'd crop target silently
    // writing the full uncropped PNG defeats the point of the flag.
    let target_pixel: pixtuoid_scene::layout::Point = if let Some(ref agent_label) = args.crop_agent
    {
        let slot = scene
            .agents
            .values()
            .find(|s| s.label.as_ref() == agent_label)
            .ok_or_else(|| {
                let labels: Vec<&str> = scene.agents.values().map(|s| s.label.as_ref()).collect();
                anyhow::anyhow!(
                    "--crop-agent {agent_label:?} not found in scene; labels: {}",
                    labels.join(", ")
                )
            })?;
        history
            .recent(slot.agent_id, u64::MAX, now)
            .ok_or_else(|| anyhow::anyhow!("agent {agent_label:?} has no visual position"))?
    } else if let Some(ref furniture_str) = args.crop_furniture {
        let buf_w = cols;
        let buf_h = rows.saturating_sub(1).saturating_mul(2);
        let layout = pixtuoid_scene::layout::SceneLayout::compute_with_seed(
            buf_w,
            buf_h,
            Some(scene.floor_capacities[0]),
            args.floor_seed,
        )
        .ok_or_else(|| anyhow::anyhow!("scene too small to compute a layout"))?;
        let found = match furniture_str.to_lowercase().as_str() {
            "desk" => layout.home_desks.first().copied(),
            name => {
                let kind = match name {
                    "pantry" => WaypointKind::Pantry,
                    "couch" => WaypointKind::Couch,
                    "vending" => WaypointKind::VendingMachine,
                    "printer" => WaypointKind::Printer,
                    "meeting" | "sofa" => WaypointKind::MeetingSofa,
                    other => anyhow::bail!(
                        "unknown --crop-furniture {other:?}; valid: pantry | couch | vending | printer | meeting | sofa | desk"
                    ),
                };
                layout
                    .waypoints
                    .iter()
                    .find(|w| w.kind == kind)
                    .map(|w| w.pos)
            }
        };
        found.ok_or_else(|| {
            anyhow::anyhow!("no {furniture_str:?} waypoint in this layout (terminal too small?)")
        })?
    } else {
        return Ok(None);
    };

    // Positions are in the LOGICAL half-block buffer (1 px per cell across,
    // 2 px per cell down — the same buf_w/buf_h fed to compute_with_seed
    // above), NOT in PNG pixels: the 8x16 px-per-cell scaling happens later
    // in save_backend_as_png.
    Ok(Some(centered_crop(
        target_pixel.x,
        target_pixel.y / 2,
        cols,
        rows,
    )))
}

/// 40x24-cell window centered on (cell_x, cell_y), clamped to stay inside the
/// cols x rows buffer (shrinks only when the terminal itself is smaller).
pub(crate) fn centered_crop(
    cell_x: u16,
    cell_y: u16,
    cols: u16,
    rows: u16,
) -> ratatui::layout::Rect {
    let crop_w = 40u16.min(cols);
    let crop_h = 24u16.min(rows);

    let crop_x = cell_x
        .saturating_sub(crop_w / 2)
        .min(cols.saturating_sub(crop_w));
    let crop_y = cell_y
        .saturating_sub(crop_h / 2)
        .min(rows.saturating_sub(crop_h));

    ratatui::layout::Rect {
        x: crop_x,
        y: crop_y,
        width: crop_w,
        height: crop_h,
    }
}

pub(crate) fn save_backend_as_png(
    term: &Terminal<TestBackend>,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    crop: Option<ratatui::layout::Rect>,
) -> Result<()> {
    let buf = term.backend().buffer();
    let (start_x, start_y, render_w, render_h) = match crop {
        Some(r) => (r.x, r.y, r.width, r.height),
        None => (0, 0, cols, rows),
    };
    let img_w = render_w as u32 * CELL_W;
    let img_h = render_h as u32 * CELL_H;
    let mut img = RgbImage::new(img_w, img_h);

    for y in 0..render_h {
        for x in 0..render_w {
            let cell = &buf[(start_x + x, start_y + y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));

            // For the half-block character "▀", the cell is split: top half = fg, bottom half = bg.
            // Other characters are rasterized as real anti-aliased text via `pixtuoid::aa_text`
            // (Monaspace Neon — what a real terminal shows, not a bitmap
            // stand-in); a glyph neither face covers falls back to a centered fg block.
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;

            let ch = symbol.chars().next().unwrap_or(' ');
            if symbol == "▀" {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rect(&mut img, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
            } else if pixtuoid::aa_text::has_glyph(ch) {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                draw_cell_text(ch, x0, y0, |px, py, cov| {
                    if px < img_w && py < img_h {
                        img.put_pixel(px, py, mix_rgb(bg, fg, cov));
                    }
                });
            } else {
                // No glyph in any font set (a decorative symbol): keep the old
                // centered block so the cell still reads in its fg color.
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                let pad_x = 1;
                let pad_y = 3;
                fill_rect(
                    &mut img,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    fg,
                );
            }
        }
    }

    img.save(path)?;
    Ok(())
}

/// Rasterize a post-draw ratatui cell buffer to RGBA: half-block cells become
/// two stacked pixels (fg = top, bg = bottom); text cells are drawn as real
/// anti-aliased glyphs via `pixtuoid::aa_text` — same path as the PNG rasterizer.
pub(crate) fn cells_to_rgba(
    term_buf: &ratatui::buffer::Buffer,
    cols: u16,
    rows: u16,
    img_w: u32,
    img_h: u32,
) -> RgbaImage {
    let mut rgba = RgbaImage::new(img_w, img_h);
    for y in 0..rows {
        for x in 0..cols {
            let cell = &term_buf[(x, y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;
            let ch = symbol.chars().next().unwrap_or(' ');
            if symbol == "▀" {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rgba_rect(&mut rgba, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
            } else if pixtuoid::aa_text::has_glyph(ch) {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                draw_cell_text(ch, x0, y0, |px, py, cov| {
                    if px < img_w && py < img_h {
                        let m = mix_rgb(bg, fg, cov);
                        rgba.put_pixel(px, py, Rgba([m[0], m[1], m[2], 255]));
                    }
                });
            } else {
                fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                let pad_x = 1;
                let pad_y = 3;
                fill_rgba_rect(
                    &mut rgba,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    fg,
                );
            }
        }
    }
    rgba
}

/// Drive the real TuiRenderer (slide transition, footer floor chip, pet motion)
/// frame by frame and encode its TestBackend cell buffer. Covers multi-floor
/// captures (via `navigations`) and pet clips (via `pets`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn save_renderer_gif(
    term: Terminal<TestBackend>,
    scene: &SceneState,
    pack: &pixtuoid_core::sprite::format::Pack,
    start_now: SystemTime,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    fps: u64,
    duration_secs: u64,
    theme: &'static pixtuoid_scene::theme::Theme,
    navigations: &[(u64, usize)],
    pets: Vec<pixtuoid_scene::pet::Pet>,
) -> Result<()> {
    let frame_count = (duration_secs * fps) as usize;
    let frame_ms = 1000 / fps.max(1);
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;

    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;

    let mut r = pixtuoid::tui::tui_renderer::TuiRenderer::new(term, theme, pets);
    let mut fired = vec![false; navigations.len()];
    for i in 0..frame_count {
        // Exact, not i * frame_ms: the truncated frame_ms accumulates (15fps → a
        // "10s" gif spans only 9834ms, so a late --navigate-at would never fire).
        let elapsed_ms = i as u64 * 1000 / fps.max(1);
        let now = start_now + Duration::from_millis(elapsed_ms);
        for floor in due_navigations(navigations, &mut fired, elapsed_ms) {
            r.navigate_floor(floor, now);
        }
        r.render(scene, pack, now)?;
        let rgba = cells_to_rgba(r.terminal.backend().buffer(), cols, rows, img_w, img_h);
        let delay = Delay::from_numer_denom_ms(frame_ms as u32, 1);
        encoder.encode_frame(GifFrame::from_parts(rgba, 0, 0, delay))?;
        let cap = i + 1;
        if cap.is_multiple_of(fps as usize) {
            eprint!("\r  encoding: {}/{}s", cap / fps as usize, duration_secs);
        }
    }
    eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_as_gif(
    term: &mut Terminal<TestBackend>,
    scene: &SceneState,
    pack: &pixtuoid_core::sprite::format::Pack,
    start_now: SystemTime,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    buf: &mut RgbBuffer,
    store: &mut pixtuoid_scene::floor::FloorCtx,
    fps: u64,
    duration_secs: u64,
    theme: &pixtuoid_scene::theme::Theme,
    floor_seed: u64,
    skip_ms: u64,
    debug_walkable: bool,
) -> Result<()> {
    let frame_count = (duration_secs * fps) as usize;
    let frame_ms = 1000 / fps.max(1);
    // Pre-roll: render (advancing the persistent motion state) WITHOUT encoding
    // for `skip_ms`, so an `--anim` capture starts at the agent's walk-out
    // instead of its long seated dwell. 0 for normal GIFs.
    let skip_frames = (skip_ms / frame_ms.max(1)) as usize;
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;

    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;

    let mut chitchat_state = std::collections::HashMap::new();
    for i in 0..(skip_frames + frame_count) {
        let now = start_now + Duration::from_millis(i as u64 * frame_ms);
        let mut draw_ctx = DrawCtx {
            buf,
            store,
            mouse_pos: None,
            pinned_agent: None,
            debug_walkable,
            theme,
            theme_picker: None,
            floor_info: None,
            per_floor: Default::default(),
            gateway: None,
            floor: {
                let mut m = pixtuoid_scene::floor::FloorMeta::ground();
                m.floor_seed = floor_seed;
                m
            },
            active_pet: None,
            last_pet_pos: None,
            last_mascot_pos: None,
            floor_pet: None,
            chitchat_state: &mut chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee: &std::collections::HashMap::new(),
            new_coffee_carriers: Vec::new(),
            popup_scale: 0.0,
            help_open: false,
            source_warning: None,
            dashboard: &pixtuoid::tui::dashboard::DashboardFrame::default(),
            connection: &pixtuoid::tui::connection::ConnectionFrame::default(),
            onboarding: &pixtuoid::tui::welcome::OnboardingFrame::default(),
        };
        draw_scene(term, scene, pack, now, &mut draw_ctx)?;
        if i < skip_frames {
            continue; // pre-roll: advance the motion state, don't encode
        }

        let rgba = cells_to_rgba(term.backend().buffer(), cols, rows, img_w, img_h);
        let delay = Delay::from_numer_denom_ms(frame_ms as u32, 1);
        let frame = GifFrame::from_parts(rgba, 0, 0, delay);
        encoder.encode_frame(frame)?;
        let cap = i + 1 - skip_frames;
        if cap.is_multiple_of(fps as usize) {
            eprint!("\r  encoding: {}/{}s", cap / fps as usize, duration_secs);
        }
    }
    eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
    Ok(())
}

/// Bounded rect fill shared by the RGB + RGBA paths — they differ only in pixel
/// type, so the loop is generic over `image::GenericImage` and can't drift between
/// the two wrappers below.
fn fill_rect_px<I: image::GenericImage>(img: &mut I, x: u32, y: u32, w: u32, h: u32, px: I::Pixel) {
    let (img_w, img_h) = (img.width(), img.height());
    for j in 0..h {
        for i in 0..w {
            let (px_x, px_y) = (x + i, y + j);
            if px_x < img_w && px_y < img_h {
                img.put_pixel(px_x, px_y, px);
            }
        }
    }
}

fn fill_rgba_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    fill_rect_px(img, x, y, w, h, Rgba([color[0], color[1], color[2], 255]));
}

fn fill_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    fill_rect_px(img, x, y, w, h, color);
}

// Size chosen so the face fits the cell: line_height(14.7) rounds to CELL_H and
// the Monaspace advance (7.96px) ≤ CELL_W — both pinned by `cell_font_px_fits_the_cell`.
const CELL_FONT_PX: f32 = 14.7;

/// Anti-aliased cell text at the terminal grid: one char per 8×16 cell, drawn
/// in `pixtuoid::aa_text` (Monaspace Neon) at CELL_FONT_PX,
/// horizontally centered on the cell's advance and CLIPPED to the cell rect so
/// a wide fallback glyph can't bleed into a neighbor. Per-cell origins (never a
/// running cursor) keep the raster locked to the terminal grid.
fn draw_cell_text(ch: char, x0: u32, y0: u32, mut put: impl FnMut(u32, u32, f32)) {
    let s = ch.to_string();
    let adv = pixtuoid::aa_text::text_width(&s, CELL_FONT_PX);
    let dx = ((CELL_W as i32 - adv) / 2).max(0);
    pixtuoid::aa_text::draw_text_at(
        &s,
        x0 as i32 + dx,
        y0 as i32,
        CELL_FONT_PX,
        |px, py, cov| {
            if cov <= 0.0 || px < x0 as i32 || py < y0 as i32 {
                return;
            }
            let (px, py) = (px as u32, py as u32);
            if px < x0 + CELL_W && py < y0 + CELL_H {
                put(px, py, cov.clamp(0.0, 1.0));
            }
        },
    );
}

/// Per-channel mix of `fg` over `bg` by AA coverage — wraps the ONE blend
/// curve (`aa_text::blend_channel`) for the `ImgRgb` pixel type.
fn mix_rgb(bg: ImgRgb<u8>, fg: ImgRgb<u8>, cov: f32) -> ImgRgb<u8> {
    let mix = |b: u8, f: u8| pixtuoid::aa_text::blend_channel(b, f, cov);
    ImgRgb([mix(bg[0], fg[0]), mix(bg[1], fg[1]), mix(bg[2], fg[2])])
}

fn color_to_rgb(c: Color, default: ImgRgb<u8>) -> ImgRgb<u8> {
    match c {
        Color::Rgb(r, g, b) => ImgRgb([r, g, b]),
        Color::Black => ImgRgb([0, 0, 0]),
        Color::Red => ImgRgb([180, 50, 50]),
        Color::Green => ImgRgb([60, 180, 60]),
        Color::Yellow => ImgRgb([220, 200, 50]),
        Color::Blue => ImgRgb([60, 120, 220]),
        Color::Magenta => ImgRgb([200, 60, 200]),
        Color::Cyan => ImgRgb([50, 200, 220]),
        Color::Gray => ImgRgb([160, 160, 160]),
        Color::DarkGray => ImgRgb([80, 80, 80]),
        Color::White => ImgRgb([240, 240, 240]),
        Color::LightRed => ImgRgb([230, 100, 100]),
        Color::LightGreen => ImgRgb([100, 230, 100]),
        Color::LightYellow => ImgRgb([240, 230, 100]),
        Color::LightBlue => ImgRgb([130, 180, 250]),
        Color::LightMagenta => ImgRgb([240, 130, 240]),
        Color::LightCyan => ImgRgb([130, 240, 240]),
        Color::Indexed(_) | Color::Reset => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_cell_text_stays_inside_its_cell_and_lights_ink() {
        // Every emitted pixel must land INSIDE the 8×16 cell at the given origin
        // (the clip is what keeps a wide glyph — ★ ink can exceed the face's
        // advance — from bleeding into the neighbor cell), with coverage in
        // [0,1], and a real glyph must light SOME ink.
        for (ch, ox, oy) in [('M', 0u32, 0u32), ('g', 8, 16), ('\u{2605}', 24, 32)] {
            let mut lit = 0usize;
            draw_cell_text(ch, ox, oy, |px, py, cov| {
                assert!(
                    px >= ox && px < ox + CELL_W && py >= oy && py < oy + CELL_H,
                    "{ch:?} pixel ({px},{py}) escaped its cell at ({ox},{oy})"
                );
                assert!((0.0..=1.0).contains(&cov));
                lit += 1;
            });
            assert!(lit > 0, "{ch:?} lit no pixels");
        }
    }

    #[test]
    fn cell_font_px_fits_the_cell() {
        // The WHY behind CELL_FONT_PX (14.7): the face's line height must fill the
        // 8×16 cell exactly and its monospace advance must fit the cell width —
        // a face/metric drift would silently clip descenders (the cell clip
        // masks it visually), so pin both halves of the claim.
        assert_eq!(
            pixtuoid::aa_text::line_height(CELL_FONT_PX),
            CELL_H as i32,
            "line height fills the cell"
        );
        assert!(
            pixtuoid::aa_text::text_width("M", CELL_FONT_PX) <= CELL_W as i32,
            "the primary face's advance fits the cell width"
        );
    }

    #[test]
    fn mix_rgb_endpoints_and_midpoint() {
        let bg = ImgRgb([0u8, 100, 200]);
        let fg = ImgRgb([200u8, 100, 0]);
        assert_eq!(mix_rgb(bg, fg, 0.0), bg);
        assert_eq!(mix_rgb(bg, fg, 1.0), fg);
        assert_eq!(mix_rgb(bg, fg, 0.5), ImgRgb([100, 100, 100]));
    }

    #[test]
    fn centered_crop_centers_in_the_open() {
        let r = centered_crop(96, 32, 192, 64);
        assert_eq!((r.x, r.y, r.width, r.height), (76, 20, 40, 24));
    }

    #[test]
    fn centered_crop_clamps_at_origin_and_far_edge() {
        let near_origin = centered_crop(2, 1, 192, 64);
        assert_eq!((near_origin.x, near_origin.y), (0, 0));
        let near_edge = centered_crop(191, 63, 192, 64);
        assert_eq!((near_edge.x, near_edge.y), (152, 40));
    }

    #[test]
    fn centered_crop_shrinks_to_a_small_terminal() {
        let r = centered_crop(10, 5, 30, 20);
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 30, 20));
    }
}
