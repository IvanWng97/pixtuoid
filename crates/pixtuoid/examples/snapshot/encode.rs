use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame as GifFrame, Rgb as ImgRgb, RgbImage, Rgba, RgbaImage};
use pixtuoid::tui::renderer::{DrawCtx, draw_scene};
use pixtuoid_core::SceneState;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_scene::floor::{FloorMeta, PerFloor};
use pixtuoid_scene::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::{CELL_H, CELL_W, SnapshotArgs, due_navigations};

/// Print a connectedness report for the walkable mask: a BFS from the door threshold,
/// reachable vs total walkable pixels. If the two differ, the mask has an isolated
/// region and A* falls back to a straight line when crossing into it — the root cause
/// of any character teleport the user sees.
///
/// `floor_seed` MUST be the one the frame beside it was rendered with: the layout variants
/// have different obstacle placements, so a report computed for another
/// variant is a tick about an office nobody looked at.
pub(crate) fn print_walkability_report(
    term: &Terminal<TestBackend>,
    floor_seed: u64,
) -> Result<()> {
    use pixtuoid_scene::layout::SceneLayout;

    let size = term.size()?;
    let (buf_w, buf_h) = pixtuoid::tui::renderer::scene_buf_size(size.width, size.height);
    // `None` = the SAME fill the renderer's draw_scene passes — the overlay
    // must mirror the real layout exactly (desks stamp the walkable mask).
    let Some(layout) = SceneLayout::compute_with_seed(buf_w, buf_h, None, floor_seed) else {
        println!("(debug_walkable) layout too small to compute");
        return Ok(());
    };

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
        "--- walkability report (floor seed {floor_seed}) ---\n\
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
    }
    Ok(())
}

fn compute_reachable(layout: &pixtuoid_scene::layout::SceneLayout) -> Vec<bool> {
    use std::collections::VecDeque;
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut visited = vec![false; w * h];
    let start = layout.door_threshold;
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

pub(crate) fn compute_crop_rect(
    args: &SnapshotArgs,
    scene: &SceneState,
    history: &pixtuoid_scene::pose::PoseHistory,
    cols: u16,
    rows: u16,
    now: SystemTime,
) -> Result<Option<ratatui::layout::Rect>> {
    // Fail loudly, as an unknown --theme/--weather does: a typo'd crop target
    // silently writing the full uncropped PNG defeats the point of the flag.
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
        // `PoseHistory` records only waypoint and walking poses, so every
        // desk-seated agent — the majority, and every seated pose — has no entry.
        // Fall back to the seat the painter would draw them on rather than widen
        // a sim store for a dev tool (#909).
        match history.recent(slot.agent_id, u64::MAX, now) {
            Some(p) => p,
            None => {
                let (buf_w, buf_h) = pixtuoid::tui::renderer::scene_buf_size(cols, rows);
                // The agent's OWN floor: `desk_index` is global, and a scene with
                // more agents than `--max-desks` puts them on floor 1+, whose
                // geometry and seed both differ from floor 0's.
                let layout = pixtuoid_scene::layout::SceneLayout::compute_with_seed(
                    buf_w,
                    buf_h,
                    Some(
                        scene.floor_capacities
                            [slot.floor_idx.min(pixtuoid_core::state::MAX_FLOORS - 1)],
                    ),
                    pixtuoid_scene::floor::floor_seed(slot.floor_idx),
                )
                .ok_or_else(|| anyhow::anyhow!("scene too small to compute a layout"))?;
                let idx = scene.floor_local_desk(slot.desk_index);
                let desk = layout.home_desk(idx).ok_or_else(|| {
                    anyhow::anyhow!("agent {agent_label:?} is neither placed nor at a home desk")
                })?;
                pixtuoid_scene::sim::seated_top_left(
                    desk,
                    pixtuoid_scene::layout::CHARACTER_SPRITE_W,
                    layout.desk_facing(idx),
                )
            }
        }
    } else if let Some(ref furniture_str) = args.crop_furniture {
        let (buf_w, buf_h) = pixtuoid::tui::renderer::scene_buf_size(cols, rows);
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
                let kind = crate::scenes::waypoint_target(name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown --crop-furniture {name:?}; valid: pantry | couch | vending | printer | meeting | sofa | chair | island | snackshelf | desk"
                    )
                })?;
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

    Ok(Some(centered_crop(target_pixel, cols, rows)))
}

/// The `--crop-*` window, in cells.
const CROP_WINDOW: ratatui::layout::Size = ratatui::layout::Size::new(40, 24);

/// A [`CROP_WINDOW`] centered on `target`, clamped to stay inside the cols x rows
/// terminal (shrinks only when the terminal itself is smaller).
pub(crate) fn centered_crop(
    target: pixtuoid_scene::layout::Point,
    cols: u16,
    rows: u16,
) -> ratatui::layout::Rect {
    // `target` is a half-block buffer pixel: one per cell across, two per cell down.
    let (cell_x, cell_y) = (target.x, target.y / 2);
    let crop_w = CROP_WINDOW.width.min(cols);
    let crop_h = CROP_WINDOW.height.min(rows);

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
    area: ratatui::layout::Rect,
) -> Result<()> {
    let mut img = RgbImage::new(
        u32::from(area.width) * CELL_W,
        u32::from(area.height) * CELL_H,
    );
    rasterize_cells(&mut img, term.backend().buffer(), area, |c| c);
    img.save(path)?;
    Ok(())
}

pub(crate) fn cells_to_rgba(term_buf: &ratatui::buffer::Buffer) -> RgbaImage {
    let area = term_buf.area;
    let mut rgba = RgbaImage::new(
        u32::from(area.width) * CELL_W,
        u32::from(area.height) * CELL_H,
    );
    rasterize_cells(&mut rgba, term_buf, area, |c| Rgba([c[0], c[1], c[2], 255]));
    rgba
}

/// Paint `area`'s cells of `term_buf` onto `img` from its origin, one
/// [`CELL_W`]×[`CELL_H`] tile per cell: the ONE rasterizer behind the PNG and
/// RGBA outputs, which differ only in the pixel `px` makes of a color.
fn rasterize_cells<I: image::GenericImage>(
    img: &mut I,
    term_buf: &ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    px: impl Fn(ImgRgb<u8>) -> I::Pixel,
) {
    let (img_w, img_h) = (img.width(), img.height());
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &term_buf[(area.x + x, area.y + y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));
            let x0 = u32::from(x) * CELL_W;
            let y0 = u32::from(y) * CELL_H;

            let ch = symbol.chars().next().unwrap_or(' ');
            if symbol == "▀" {
                // The half-block splits the cell: top half = fg, bottom half = bg.
                fill_rect(img, x0, y0, CELL_W, CELL_H / 2, px(fg));
                fill_rect(img, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, px(bg));
            } else if symbol.trim().is_empty() {
                fill_rect(img, x0, y0, CELL_W, CELL_H, px(bg));
            } else if pixtuoid::aa_text::has_glyph(ch) {
                fill_rect(img, x0, y0, CELL_W, CELL_H, px(bg));
                draw_cell_text(ch, x0, y0, |tx, ty, cov| {
                    if tx < img_w && ty < img_h {
                        img.put_pixel(tx, ty, px(mix_rgb(bg, fg, cov)));
                    }
                });
            } else {
                // No glyph in any face (a decorative symbol): a centered block still
                // reads in the cell's fg color.
                fill_rect(img, x0, y0, CELL_W, CELL_H, px(bg));
                let pad_x = 1;
                let pad_y = 3;
                fill_rect(
                    img,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    px(fg),
                );
            }
        }
    }
}

/// A capture's frame clock — `secs` of frames at `fps` from `start` — shared by
/// the animation encoders and the proof frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeline {
    pub(crate) fps: u64,
    pub(crate) secs: u64,
    pub(crate) start: SystemTime,
}

impl Timeline {
    pub(crate) fn frame_count(&self) -> usize {
        (self.secs * self.fps) as usize
    }

    /// Frame `i`'s offset from `start`. Exact, not `i * frame_ms`: the truncated
    /// `frame_ms` accumulates, drifting every time-derived element off the wall
    /// clock by the last frame — and a late --navigate-at then never fires.
    ///
    /// This does NOT make the site's `loop`ed clip seam-free, and no timing choice
    /// can: each creature walk's destination is a seeded `walkable_target` draw
    /// numbered by its roam, so the roaming is aperiodic BY DESIGN and frame N is never frame 0
    /// however the duration is chosen. Closing it would mean a scripted
    /// (non-wandering) timeline for the demo — a media decision, not a rendering one.
    pub(crate) fn elapsed_ms(&self, i: usize) -> u64 {
        i as u64 * 1000 / self.fps.max(1)
    }

    pub(crate) fn now(&self, i: usize) -> SystemTime {
        self.start + Duration::from_millis(self.elapsed_ms(i))
    }
}

/// Where an animation's frames go.
pub(crate) enum FrameSink {
    Gif {
        encoder: GifEncoder<std::fs::File>,
        delay: Delay,
    },
    /// Lossless PNGs for a consumer that re-encodes (gen-media's clips and their
    /// posters): the GIF encoder NeuQuant-quantises every frame past 256 colours
    /// (`gif::Frame::from_rgba_speed`), and a re-encode of the GIF inherits that loss.
    /// Named `f%04d.png` from 1, as gen-media.py's `poster_frame` reads.
    Pngs { dir: PathBuf, written: usize },
}

impl FrameSink {
    pub(crate) fn open(gif_path: &Path, frames_dir: Option<&Path>, frame_ms: u64) -> Result<Self> {
        if let Some(dir) = frames_dir {
            return Self::pngs(dir);
        }
        let mut encoder = GifEncoder::new(std::fs::File::create(gif_path)?);
        encoder.set_repeat(Repeat::Infinite)?;
        Ok(Self::Gif {
            encoder,
            delay: Delay::from_numer_denom_ms(frame_ms as u32, 1),
        })
    }

    pub(crate) fn pngs(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self::Pngs {
            dir: dir.to_path_buf(),
            written: 0,
        })
    }

    pub(crate) fn push(&mut self, rgba: RgbaImage) -> Result<()> {
        match self {
            Self::Gif { encoder, delay } => {
                encoder.encode_frame(GifFrame::from_parts(rgba, 0, 0, *delay))?;
            }
            Self::Pngs { dir, written } => {
                *written += 1;
                rgba.save(dir.join(format!("f{written:04}.png")))?;
            }
        }
        Ok(())
    }
}

/// One animation capture. `scene`, `pack` and `theme` are what each path's per-frame
/// render reads; [`AnimJob::encode`] itself only clocks and encodes.
pub(crate) struct AnimJob<'a> {
    pub(crate) path: &'a Path,
    pub(crate) frames_dir: Option<&'a Path>,
    pub(crate) timeline: Timeline,
    pub(crate) scene: &'a SceneState,
    pub(crate) pack: &'a std::sync::Arc<Pack>,
    pub(crate) theme: &'static Theme,
    pub(crate) weather: pixtuoid_scene::sky::WeatherPolicy,
}

impl AnimJob<'_> {
    /// Call `render` once per frame on `state` — `skip_ms` of pre-roll first,
    /// rendered but not encoded — then encode the cell buffer `cells` reads back.
    fn encode<S>(
        &self,
        skip_ms: u64,
        state: &mut S,
        mut render: impl FnMut(&mut S, SystemTime, u64) -> Result<()>,
        cells: impl Fn(&S) -> &ratatui::buffer::Buffer,
    ) -> Result<()> {
        let Timeline { fps, secs, .. } = self.timeline;
        let frame_count = self.timeline.frame_count();
        let frame_ms = 1000 / fps.max(1);
        let skip_frames = (skip_ms / frame_ms.max(1)) as usize;

        let mut sink = FrameSink::open(self.path, self.frames_dir, frame_ms)?;
        for i in 0..(skip_frames + frame_count) {
            let elapsed_ms = self.timeline.elapsed_ms(i);
            render(state, self.timeline.now(i), elapsed_ms)?;
            if i < skip_frames {
                continue;
            }
            sink.push(cells_to_rgba(cells(state)))?;
            let cap = i + 1 - skip_frames;
            if cap.is_multiple_of(fps as usize) {
                eprint!("\r  encoding: {}/{secs}s", cap / fps as usize);
            }
        }
        eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
        Ok(())
    }
}

/// Drive the real TuiRenderer (slide transition, footer floor chip, pet motion) frame by
/// frame and encode its TestBackend cell buffer.
pub(crate) fn save_renderer_animation(
    job: &AnimJob,
    term: Terminal<TestBackend>,
    navigations: &[(u64, usize)],
    pets: Vec<pixtuoid_scene::pet::Pet>,
) -> Result<()> {
    let mut r = pixtuoid::tui::tui_renderer::TuiRenderer::new(
        term,
        job.theme,
        pets,
        std::sync::Arc::clone(job.pack),
    );
    r.set_weather(job.weather);
    let mut fired = vec![false; navigations.len()];
    // 0, not the caller's skip_ms: clap keeps every pre-roll flag off this path
    // (`conflicts_with` on --navigate-at / --pets), and a pre-roll would shift the
    // --navigate-at schedule off the encoded clip's t=0.
    job.encode(
        0,
        &mut r,
        |r, now, elapsed_ms| {
            for floor in due_navigations(navigations, &mut fired, elapsed_ms) {
                r.navigate_floor(floor, now);
            }
            r.render(job.scene, job.pack, now)
        },
        |r| r.terminal.backend().buffer(),
    )
}

/// Drive `draw_scene` over one floor and encode it; `skip_ms` (from --anim,
/// --meeting or --warmup-secs) starts the clip mid-action.
pub(crate) fn save_animation(
    job: &AnimJob,
    term: &mut Terminal<TestBackend>,
    floor: &mut PerFloor,
    floor_meta: FloorMeta,
    skip_ms: u64,
    debug_walkable: bool,
) -> Result<()> {
    let scene = job.scene;
    let mut office = pixtuoid_scene::floor::PerOffice::new();
    job.encode(
        skip_ms,
        term,
        |term, now, _| {
            let mut draw_ctx = DrawCtx {
                debug_walkable,
                ..DrawCtx::offscreen(
                    floor,
                    office.stores(),
                    job.theme,
                    scene,
                    job.pack,
                    now,
                    floor_meta,
                )
            };
            draw_scene(term, &mut draw_ctx).map(drop)
        },
        |term| term.backend().buffer(),
    )
}

/// Fill a rect, clipped to `img`.
pub(crate) fn fill_rect<I: image::GenericImage>(
    img: &mut I,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    px: I::Pixel,
) {
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

// Chosen so the face fits the cell: its line height rounds to CELL_H and the Monaspace
// advance is ≤ CELL_W.
const CELL_FONT_PX: f32 = 14.7;

/// Anti-aliased cell text at the terminal grid: one char per [`CELL_W`]×[`CELL_H`] cell,
/// centered on the cell's advance and CLIPPED to the cell rect so ink wider than the
/// advance (★) can't bleed into a neighbor. Per-cell origins (never a running cursor)
/// keep the raster locked to the grid.
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

/// Per-channel mix of `fg` over `bg` by AA coverage.
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
    use pixtuoid_scene::layout::Point;

    #[test]
    fn draw_cell_text_stays_inside_its_cell_and_lights_ink() {
        // ★ ink can exceed the face's advance — the clip is what keeps it out of the
        // neighbor cell.
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
        // A face/metric drift would silently clip descenders — the cell clip masks it
        // visually, so pin both halves of the claim.
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
        let r = centered_crop(Point { x: 96, y: 64 }, 192, 64);
        assert_eq!((r.x, r.y, r.width, r.height), (76, 20, 40, 24));
    }

    #[test]
    fn centered_crop_clamps_at_origin_and_far_edge() {
        let near_origin = centered_crop(Point { x: 2, y: 2 }, 192, 64);
        assert_eq!((near_origin.x, near_origin.y), (0, 0));
        let near_edge = centered_crop(Point { x: 191, y: 126 }, 192, 64);
        assert_eq!((near_edge.x, near_edge.y), (152, 40));
    }

    #[test]
    fn centered_crop_shrinks_to_a_small_terminal() {
        let r = centered_crop(Point { x: 10, y: 10 }, 30, 20);
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 30, 20));
    }
}
