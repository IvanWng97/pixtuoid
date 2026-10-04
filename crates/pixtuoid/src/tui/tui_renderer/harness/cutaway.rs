//! The cutaway through the production render path: what reaches the
//! terminal as escapes, and what the cells show.
use super::*;
use crate::graphics::{CellSize, Fit, ImageProtocol};
use crate::tui::cutaway::TileCutaway;
use pixtuoid_core::sprite::format::Density;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cell whose natural scale is the bundled art's density, so the image is
/// the density render itself, one cell per 4×8 image pixels.
const CELL: CellSize = CellSize { w: 4, h: 8 };
const PLACEHOLDER: char = '\u{10EEEE}';
const TRANSMIT: &str = "\x1b_Ga=T,";
const SIXEL: &str = "\x1bP9;1q";
const ITERM2: &str = "\x1b]1337;File=";

/// The terminal's side of the transmits; set `fail` to make it refuse them.
#[derive(Clone, Default)]
struct Wire {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("terminal gone"));
        }
        self.bytes.lock().expect("lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Wire {
    /// What arrived since the last take.
    fn take(&self) -> String {
        let bytes = std::mem::take(&mut *self.bytes.lock().expect("lock"));
        String::from_utf8(bytes).expect("escapes are ascii")
    }
}

/// A terminal whose window reports each cell `cell` pixels big, as a real
/// one's does.
struct Window {
    inner: TestBackend,
    cell: CellSize,
}

impl Window {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            inner: TestBackend::new(cols, rows),
            cell: CELL,
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.inner.resize(cols, rows);
    }

    /// Zoom the font to `cell`, the window keeping its cells: only the pixels
    /// change.
    fn zoom(&mut self, cell: CellSize) {
        self.cell = cell;
    }
}

impl std::borrow::Borrow<TestBackend> for Window {
    fn borrow(&self) -> &TestBackend {
        &self.inner
    }
}

impl ratatui::backend::Backend for Window {
    type Error = <TestBackend as ratatui::backend::Backend>::Error;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.inner.draw(content)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
        self.inner.get_cursor_position()
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
        self.inner.size()
    }
    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        let columns_rows = self.inner.size()?;
        let cell = self.cell;
        Ok(ratatui::backend::WindowSize {
            columns_rows,
            pixels: ratatui::layout::Size::new(
                columns_rows.width * cell.w,
                columns_rows.height * cell.h,
            ),
        })
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

/// The plan's fit over a `cols`×`rows` terminal's scene.
fn fit(cols: u16, rows: u16) -> Fit {
    let area = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows)).as_size();
    let fit = Fit::new(CELL, area, pack_arc().max_density_variant()).expect("fits");
    assert_eq!(fit.upscale(), 1);
    assert_eq!(fit.density(), Density::new(4).expect("nonzero"));
    fit
}

/// A renderer painting the cutaway over `protocol` into a `cols`×`rows`
/// terminal.
fn painter(cols: u16, rows: u16, protocol: ImageProtocol) -> (TuiRenderer<Window>, Wire) {
    let (r, wire, _) = armed(cols, rows, protocol);
    (r, wire)
}

/// [`painter`], and the flag it sets where the process's would tell the
/// unwind that grid pixels were drawn: its own, so no test leaks into
/// another.
fn armed(
    cols: u16,
    rows: u16,
    protocol: ImageProtocol,
) -> (TuiRenderer<Window>, Wire, &'static AtomicBool) {
    let mut r = TuiRenderer::new(
        Terminal::new(Window::new(cols, rows)).expect("terminal"),
        normal_theme(),
        vec![],
        pack_arc(),
    );
    let (wire, in_grid) = (Wire::default(), Box::leak(Box::new(AtomicBool::new(false))));
    r.set_cutaway(
        TileCutaway::new(
            fit(cols, rows),
            CELL,
            protocol,
            false,
            Box::new(wire.clone()),
        )
        .arming(in_grid),
    );
    (r, wire, in_grid)
}

fn kitty(cols: u16, rows: u16) -> (TuiRenderer<Window>, Wire) {
    painter(cols, rows, ImageProtocol::Kitty)
}

fn office() -> SceneState {
    scene_with(vec![idle("/k/0.jsonl", 0, t0())], 16)
}

fn placeholders_in_row(r: &TuiRenderer<Window>, y: u16) -> usize {
    let buf = r.frame_buffer();
    (0..buf.area.width)
        .filter(|&x| buf[(x, y)].symbol().starts_with(PLACEHOLDER))
        .count()
}

#[test]
fn the_first_frame_transmits_and_an_identical_one_sends_nothing() {
    let (mut r, wire) = kitty(120, 40);
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(wire.take().contains(TRANSMIT));
    r.render(&scene, pack(), t0()).expect("render");
    assert_eq!(wire.take(), "");
}

/// Text and image never share a cell: the footer row stays text.
#[test]
fn placeholders_fill_the_scene_and_never_the_footer() {
    let (cols, rows) = (120, 40);
    let (mut r, _wire) = kitty(cols, rows);
    r.render(&office(), pack(), t0()).expect("render");
    let scene = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows));
    for y in 0..scene.height {
        assert_eq!(placeholders_in_row(&r, y), usize::from(cols), "row {y}");
    }
    assert_eq!(placeholders_in_row(&r, rows - 1), 0);
}

/// Its star link would launch a browser from a cell showing the image.
#[test]
fn the_cutaway_star_is_not_clickable() {
    let (mut r, _wire) = kitty(120, 40);
    r.render(&office(), pack(), t0()).expect("render");
    assert!(!r.star_clickable());
    let mut classic = build(120, 40, vec![]);
    classic.render(&office(), pack(), t0()).expect("render");
    assert!(classic.star_clickable());
}

/// A refused frame sends no tiles and leaves no hit targets behind.
#[test]
fn shrinking_under_the_minimum_refuses_the_cutaway_frame() {
    let (cols, rows) = (120, 40);
    let (mut r, wire) = kitty(cols, rows);
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(r.scene_area_at(cols / 2, rows / 2).is_some());
    wire.take();
    let (small_cols, small_rows) = too_small_terminal();
    r.terminal.backend_mut().resize(small_cols, small_rows);
    r.render(&scene, pack(), t0()).expect("render");
    assert!(!wire.take().contains(TRANSMIT));
    assert_eq!(r.scene_area_at(cols / 2, rows / 2), None);
    assert!(r.cached_pet_pos().is_none());
}

/// A resize clears the screen ratatui redraws, SIXEL and iTerm2 pixels with
/// it: a terminal shrunk under the minimum and restored gets every tile again.
/// Both refusal arms: the layout's, and the scene minimum's.
#[test]
fn a_terminal_restored_from_too_small_gets_every_tile_again() {
    use crate::tui::renderer::{FOOTER_ROWS, MIN_SCENE_HEIGHT, min_terminal_size};
    let (cols, rows) = min_terminal_size();
    let smalls = [
        too_small_terminal(),
        (cols, MIN_SCENE_HEIGHT + FOOTER_ROWS - 1),
    ];
    for (protocol, intro) in [
        (ImageProtocol::Sixel, SIXEL),
        (ImageProtocol::Iterm2, ITERM2),
    ] {
        // Every tile, then those the last of three frames sends.
        let sent = |between: (u16, u16)| {
            let (mut r, wire) = painter(cols, rows, protocol);
            let scene = office();
            let cadence = protocol.cadence();
            r.render(&scene, pack(), t0()).expect("render");
            let all = wire.take().matches(intro).count();
            r.terminal.backend_mut().resize(between.0, between.1);
            r.render(&scene, pack(), t0() + cadence).expect("render");
            r.terminal.backend_mut().resize(cols, rows);
            r.render(&scene, pack(), t0() + cadence * 2)
                .expect("render");
            (all, wire.take().matches(intro).count())
        };
        let (all, steady) = sent((cols, rows));
        assert!(
            steady < all,
            "{protocol:?}: {steady} of {all} change anyway"
        );
        for (small_cols, small_rows) in smalls {
            assert_eq!(
                sent((small_cols, small_rows)).1,
                all,
                "{protocol:?} from {small_cols}x{small_rows}"
            );
        }
    }
}

/// The image's top-left SIXEL tile, whole, for a `cell`-sized cell.
fn first_tile(cell: CellSize) -> String {
    let shape = ImageProtocol::Sixel.tile();
    format!(
        "\x1b[1;1H{SIXEL}\"1;1;{};{}",
        shape.cols * cell.w,
        shape.rows * cell.h
    )
}

/// The plan's cell is the terminal's own answer, which outranks the window's
/// where they differ, so `doctor`, the boot seed and the painted office all
/// read it until a font zoom moves the window's cell.
#[test]
fn the_plans_cell_holds_until_the_windows_moves() {
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let padded = CellSize {
        w: CELL.w + 1,
        h: CELL.h + 2,
    };
    let (mut r, wire) = painter(cols, rows, ImageProtocol::Sixel);
    r.terminal.backend_mut().zoom(padded);
    let scene = office();
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(wire.take().contains(&first_tile(CELL)), "the plan's cell");
    let plan = crate::graphics::Plan::Cutaway {
        fit: fit(cols, rows),
        protocol: ImageProtocol::Sixel,
        cell: CELL,
        tmux: false,
        forced: false,
    };
    let office = plan.office_extent(ratatui::layout::Size::new(cols, rows));
    assert_eq!(
        r.office_extent(),
        (office.w, office.h),
        "the boot seed's office"
    );

    let zoomed = CellSize {
        w: CELL.w * 2,
        h: CELL.h * 2,
    };
    r.terminal.backend_mut().zoom(zoomed);
    r.render(&scene, pack(), t0() + cadence).expect("render");
    assert!(wire.take().contains(&first_tile(zoomed)), "zoomed in");
    r.terminal.backend_mut().zoom(padded);
    r.render(&scene, pack(), t0() + cadence * 2)
        .expect("render");
    assert!(wire.take().contains(&first_tile(CELL)), "zoomed back");
}

/// A window that reports 0 px on the first frame has read nothing, so the
/// plan's cell holds when the real cell arrives: it is no font zoom.
#[test]
fn a_first_frame_with_no_pixels_does_not_pose_as_the_windows_baseline() {
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let padded = CellSize {
        w: CELL.w + 1,
        h: CELL.h + 2,
    };
    let (mut r, wire) = painter(cols, rows, ImageProtocol::Sixel);
    r.terminal.backend_mut().zoom(CellSize { w: 0, h: 0 });
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(wire.take().contains(&first_tile(CELL)), "the plan's cell");
    r.terminal.backend_mut().zoom(padded);
    r.render(&scene, pack(), t0() + ImageProtocol::Sixel.cadence())
        .expect("render");
    assert_eq!(wire.take(), "", "the plan's cell holds, nothing re-cut");
    let plan = crate::graphics::Plan::Cutaway {
        fit: fit(cols, rows),
        protocol: ImageProtocol::Sixel,
        cell: CELL,
        tmux: false,
        forced: false,
    };
    let office = plan.office_extent(ratatui::layout::Size::new(cols, rows));
    assert_eq!(
        r.office_extent(),
        (office.w, office.h),
        "the boot seed's office"
    );
}

/// A font zoom changes the cell's pixels: the window's cell cuts the tiles,
/// classic paints while the cell is too small for the art, and the cutaway
/// returns once it fits.
#[test]
fn a_font_zoom_refits_the_cutaway_to_the_windows_cell() {
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let (mut r, wire) = painter(cols, rows, ImageProtocol::Sixel);
    let scene = office();
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(wire.take().contains(&first_tile(CELL)));

    let zoomed = CellSize {
        w: CELL.w * 2,
        h: CELL.h * 2,
    };
    r.terminal.backend_mut().zoom(zoomed);
    r.render(&scene, pack(), t0() + cadence).expect("render");
    assert!(wire.take().contains(&first_tile(zoomed)), "zoomed in");

    let tiny = CellSize {
        w: CELL.w / 2,
        h: CELL.h / 2,
    };
    let area = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows)).as_size();
    assert!(Fit::new(tiny, area, pack_arc().max_density_variant()).is_none());
    r.terminal.backend_mut().zoom(tiny);
    r.render(&scene, pack(), t0() + cadence * 2)
        .expect("render");
    assert!(!wire.take().contains(SIXEL), "too small for the art");
    assert!(
        frame_text(r.frame_buffer()).contains('\u{2580}'),
        "classic paints meanwhile"
    );

    r.terminal.backend_mut().zoom(zoomed);
    r.render(&scene, pack(), t0() + cadence * 3)
        .expect("render");
    assert!(wire.take().contains(&first_tile(zoomed)), "zoomed back in");
}

/// While classic paints in the cutaway's place, a click hit-tests classic's
/// frame, never the canvas's last: an agent gone since is gone.
#[test]
fn a_cell_too_small_hit_tests_the_classic_frame() {
    let (cols, rows) = crate::tui::renderer::min_terminal_size();
    let (mut r, _wire) = kitty(cols, rows);
    let id = AgentId::from_transcript_path("/k/0.jsonl");
    r.render(&office(), pack(), t0()).expect("render");
    hover_agent(&mut r, id);
    let (col, row) = r.mouse_pos.expect("hovered");
    r.set_mouse_pos(None);
    r.terminal.backend_mut().zoom(CellSize {
        w: CELL.w / 2,
        h: CELL.h / 2,
    });
    r.render(&scene_with(vec![], 16), pack(), t0())
        .expect("render");
    assert_eq!(r.hit_test_agent_at(col, row), None);
}

/// A slide the terminal shrinks under mid-way is cancelled, as classic's is:
/// it lands on the destination floor, with nothing left to hit-test.
#[test]
fn shrinking_mid_slide_lands_on_the_destination() {
    let (cols, rows) = (120, 40);
    let (mut r, _wire) = kitty(cols, rows);
    let scene = two_floor_scene();
    r.render(&scene, pack(), t0()).expect("render");
    r.navigate_floor(1, t0());
    let (small_cols, small_rows) = too_small_terminal();
    r.terminal.backend_mut().resize(small_cols, small_rows);
    r.render(&scene, pack(), t0() + Duration::from_millis(100))
        .expect("render");
    assert!(r.transition().is_none());
    assert_eq!(r.current_floor(), 1);
    assert!(r.cached_layout().is_none());
    assert_eq!(r.scene_area_at(small_cols / 2, small_rows / 2), None);
}

#[test]
fn a_modal_over_the_image_shows_its_text() {
    let (mut r, _wire) = kitty(120, 40);
    r.set_help_open(true);
    r.render(&office(), pack(), t0()).expect("render");
    assert!(frame_text(r.frame_buffer()).contains("? Keyboard"));
}

#[test]
fn a_failed_write_re_sends_next_frame() {
    let (mut r, wire) = kitty(120, 40);
    let scene = office();
    wire.fail.store(true, Ordering::Relaxed);
    r.render(&scene, pack(), t0())
        .expect("a lost transmit is no render error");
    wire.fail.store(false, Ordering::Relaxed);
    r.render(&scene, pack(), t0()).expect("render");
    let sent = wire.take();
    assert!(sent.starts_with("\x1b\\"), "an ST first ends a torn escape");
    assert!(sent.contains(TRANSMIT));
}

#[test]
fn a_redraw_re_sends_every_tile() {
    let (mut r, wire) = kitty(120, 40);
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    let tiles = wire.take().matches(TRANSMIT).count();
    r.redraw().expect("redraw");
    r.render(&scene, pack(), t0()).expect("render");
    assert_eq!(wire.take().matches(TRANSMIT).count(), tiles);
}

#[test]
fn an_agent_under_the_cutaway_is_hit_tested_on_the_canvas() {
    let (mut r, _wire) = kitty(120, 40);
    let scene = scene_with(vec![active("/k/0.jsonl", 0, "Bash", t0())], 16);
    r.render(&scene, pack(), t0()).expect("render");
    assert!(!frame_text(r.frame_buffer()).contains("Active"));
    hover_agent(&mut r, AgentId::from_transcript_path("/k/0.jsonl"));
    r.render(&scene, pack(), t0()).expect("render");
    assert!(
        frame_text(r.frame_buffer()).contains("Active"),
        "the hovered agent's tooltip"
    );
}

#[test]
fn sixel_and_iterm2_draw_changed_tiles_and_nothing_on_an_identical_frame() {
    for (protocol, intro) in [
        (ImageProtocol::Sixel, SIXEL),
        (ImageProtocol::Iterm2, ITERM2),
    ] {
        let (mut r, wire) = painter(120, 40, protocol);
        let scene = office();
        r.render(&scene, pack(), t0()).expect("render");
        assert!(wire.take().contains(intro), "{protocol:?}");
        r.render(&scene, pack(), t0()).expect("render");
        assert_eq!(wire.take(), "", "{protocol:?}");
    }
}

/// The tiles go out after ratatui's flush, once the frame's text is known: a
/// modal that opens on the frame every tile is owed in withholds its tiles
/// there and then, and they follow once it closes. Meanwhile ratatui never
/// writes the image's cells, so the modal's text stays until its tiles
/// replace it.
#[test]
fn a_modal_withholds_the_tiles_under_it_until_it_closes() {
    let (mut r, wire) = painter(120, 40, ImageProtocol::Sixel);
    let scene = office();
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0()).expect("render");
    let all = wire.take().matches(SIXEL).count();
    r.set_help_open(true);
    r.redraw().expect("redraw");
    r.render(&scene, pack(), t0() + cadence).expect("render");
    assert!(frame_text(r.frame_buffer()).contains("? Keyboard"));
    let open = wire.take().matches(SIXEL).count();
    assert!(0 < open && open < all, "{open} of {all}");
    r.set_help_open(false);
    r.render(&scene, pack(), t0() + cadence * 2)
        .expect("render");
    let closed = wire.take().matches(SIXEL).count();
    assert!(closed >= all - open, "{closed} after {open} of {all}");
    assert!(
        frame_text(r.frame_buffer()).contains("? Keyboard"),
        "ratatui left the image's cells alone"
    );
}

#[test]
fn the_image_cells_are_never_written_by_ratatui() {
    let (cols, rows) = (120, 40);
    let (mut r, _wire) = painter(cols, rows, ImageProtocol::Sixel);
    r.render(&office(), pack(), t0()).expect("render");
    let text = frame_text(r.frame_buffer());
    let lines: Vec<&str> = text.lines().collect();
    let scene = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows));
    for line in &lines[..usize::from(scene.height)] {
        assert_eq!(line.trim(), "");
    }
    assert_ne!(
        lines[usize::from(rows - 1)].trim(),
        "",
        "the footer is text"
    );
}

#[test]
fn the_protocols_cadence_gates_its_transmits() {
    let (mut r, wire) = painter(120, 40, ImageProtocol::Sixel);
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    wire.take();
    r.redraw().expect("redraw");
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0() + cadence / 2)
        .expect("render");
    assert_eq!(wire.take(), "", "inside the cadence, the tiles stay owed");
    r.render(&scene, pack(), t0() + cadence).expect("render");
    assert!(wire.take().contains(SIXEL));
}

/// Each kitty image the wire carried, in order: its id and its pixels,
/// inflated.
fn kitty_images(wire: &str) -> Vec<(u32, Vec<u8>)> {
    let mut images = Vec::new();
    let mut open: Option<(u32, String)> = None;
    for escape in wire
        .split("\x1b\\")
        .filter_map(|e| e.split_once("\x1b_G").map(|(_, e)| e))
    {
        let (keys, payload) = escape.split_once(';').expect("a payload");
        if let Some(id) = keys.split(',').find_map(|k| k.strip_prefix("i=")) {
            open = Some((id.parse().expect("an id"), String::new()));
        }
        let (_, data) = open.as_mut().expect("an image under way");
        data.push_str(payload);
        if keys.contains("m=0") {
            let (id, data) = open.take().expect("an image under way");
            let zlib = base64_simd::STANDARD
                .decode_to_vec(data.as_bytes())
                .expect("base64");
            let rgb = miniz_oxide::inflate::decompress_to_vec_zlib(&zlib).expect("zlib");
            images.push((id, rgb));
        }
    }
    images
}

/// A floor switch slides the cutaway the way classic slides its half-blocks:
/// mid-slide, the middle tile shows neither floor and nothing is
/// hit-tested; then it settles on the destination.
#[test]
fn a_floor_switch_slides_the_cutaway_then_settles() {
    let (mut r, wire) = kitty(120, 40);
    let scene = two_floor_scene();
    let mut now = t0();
    r.render(&scene, pack(), now).expect("render");
    let across = 120u32.div_ceil(u32::from(ImageProtocol::Kitty.tile().cols));
    let middle = crate::graphics::kitty::process_base() + across * 10 + across / 2;
    let tile = |sent: &str| {
        kitty_images(sent)
            .into_iter()
            .rev()
            .find(|(id, _)| *id == middle)
            .map(|(_, rgb)| rgb)
    };
    let before = tile(&wire.take()).expect("the first frame sends every tile");

    r.navigate_floor(1, now);
    let half = Duration::from_millis(r.transition().expect("sliding").duration_ms / 2);
    r.render(&scene, pack(), now + half).expect("render");
    // What the terminal shows for the tile: its last transmit, or the one
    // before when none came.
    let mid = tile(&wire.take()).unwrap_or_else(|| before.clone());
    assert!(r.transition().is_some());
    assert!(r.cached_layout().is_none());
    let area = r.frame_buffer().area;
    assert!(
        area.positions()
            .all(|p| r.hit_test_agent_at(p.x, p.y).is_none())
    );

    now += half;
    render_until_settled(&mut r, &scene, pack(), &mut now, 1);
    let after = tile(&wire.take()).unwrap_or_else(|| mid.clone());
    assert!(mid != before && mid != after);
    assert_eq!(r.current_floor(), 1);
    hover_agent(&mut r, AgentId::from_transcript_path("/n/1.jsonl"));
}

/// A slide is no resize: the extent a resize changes holds through it, so the
/// slide runs its course rather than landing after its first frame; a resize
/// lands it on the destination.
#[test]
fn a_cutaway_slide_runs_until_a_resize_lands_it() {
    let (cols, rows) = (120, 40);
    let (mut r, _wire) = kitty(cols, rows);
    let scene = two_floor_scene();
    let start = t0();
    r.render(&scene, pack(), start).expect("render");
    let extent = r.office_extent();
    r.navigate_floor(1, start);
    let slide = Duration::from_millis(r.transition().expect("sliding").duration_ms);
    let frames = 4;
    let at = |frame| start + slide * frame / frames;
    for frame in 1..frames - 1 {
        r.render(&scene, pack(), at(frame)).expect("render");
        assert_eq!(r.office_extent(), extent, "frame {frame}");
        assert!(r.transition().is_some(), "landed at frame {frame}");
    }
    r.terminal.backend_mut().resize(cols - 1, rows);
    r.render(&scene, pack(), at(frames - 1)).expect("render");
    assert!(r.transition().is_none());
    assert_eq!(r.current_floor(), 1);
}

/// Each floor slides out showing its own wall board: the first slide frame,
/// before anything has moved, re-sends none of the tiles over the board,
/// which a board borrowed from the other floor would change.
#[test]
fn a_sliding_floor_keeps_its_own_wall_board() {
    let tile = ImageProtocol::Kitty.tile();
    let across = 120u32.div_ceil(u32::from(tile.cols));
    let base = crate::graphics::kitty::process_base();
    // The neon sign and the rows its board's three lines take, in cells.
    let (cols, rows) = (
        u32::from(pixtuoid_scene::layout::NEON_PANEL_W + 2),
        u32::from(pixtuoid_scene::layout::NEON_PANEL_INNER_Y / 2 + 3),
    );
    let board: Vec<u32> = (0..rows.div_ceil(u32::from(tile.rows)))
        .flat_map(|ty| {
            (0..cols.div_ceil(u32::from(tile.cols))).map(move |tx| base + ty * across + tx)
        })
        .collect();
    let scene = two_floor_scene();
    for (from, to) in [(0, 1), (1, 0)] {
        let (mut r, wire) = kitty(120, 40);
        let mut now = t0();
        r.render(&scene, pack(), now).expect("render");
        if from == 1 {
            r.navigate_floor(1, now);
            render_until_settled(&mut r, &scene, pack(), &mut now, 1);
        }
        r.render(&scene, pack(), now).expect("render");
        wire.take();
        r.navigate_floor(to, now);
        r.render(&scene, pack(), now).expect("render");
        assert!(r.transition().is_some(), "{from} → {to}: sliding");
        let resent: Vec<u32> = kitty_images(&wire.take())
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| board.contains(id))
            .collect();
        assert_eq!(
            resent,
            Vec::<u32>::new(),
            "{from} → {to}: floor {from}'s board changed"
        );
    }
}

/// The image's cells, by tile, as `(tile col, tile row)` → the symbols there.
fn image_tiles(
    r: &TuiRenderer<Window>,
    rows: u16,
) -> std::collections::BTreeMap<(u16, u16), Vec<String>> {
    let shape = ImageProtocol::Sixel.tile();
    let buf = r.frame_buffer();
    let mut tiles = std::collections::BTreeMap::<_, Vec<String>>::new();
    for y in 0..rows - 1 {
        for x in 0..buf.area.width {
            let at = (x / shape.cols, y / shape.rows);
            tiles
                .entry(at)
                .or_default()
                .push(buf[(x, y)].symbol().to_string());
        }
    }
    tiles
}

/// A withheld tile's cells the modal leaves free show the frame as
/// half-blocks, and only those: a tile no modal reaches would be half-blocks
/// throughout. Once the modal closes, each such tile is drawn again.
#[test]
fn a_withheld_tiles_free_cells_show_half_blocks_until_it_returns() {
    const HALF_BLOCK: &str = "\u{2580}";
    let (cols, rows) = (120, 40);
    let (mut r, wire) = painter(cols, rows, ImageProtocol::Sixel);
    let scene = office();
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0()).expect("render");
    wire.take();
    r.set_help_open(true);
    r.render(&scene, pack(), t0() + cadence).expect("render");
    let filled: Vec<(u16, u16)> = image_tiles(&r, rows)
        .into_iter()
        .filter(|(_, cells)| cells.iter().any(|s| s == HALF_BLOCK))
        .inspect(|(at, cells)| {
            assert!(
                cells.iter().any(|s| s != HALF_BLOCK),
                "tile {at:?} is all half-blocks: no modal reaches it"
            );
        })
        .map(|(at, _)| at)
        .collect();
    assert!(!filled.is_empty());

    r.set_help_open(false);
    r.render(&scene, pack(), t0() + cadence * 2)
        .expect("render");
    let sent = wire.take();
    let shape = ImageProtocol::Sixel.tile();
    for (tx, ty) in &filled {
        let to = format!(
            "\x1b[{};{}H{SIXEL}",
            ty * shape.rows + 1,
            tx * shape.cols + 1
        );
        assert!(sent.contains(&to), "tile ({tx}, {ty}) drawn again");
    }
    r.redraw().expect("redraw");
    r.render(&scene, pack(), t0() + cadence * 3)
        .expect("render");
    assert!(!frame_text(r.frame_buffer()).contains(HALF_BLOCK));
}

#[test]
fn a_grid_image_arms_the_unwinds_erase() {
    let erases = |drew: &AtomicBool| {
        crate::graphics::grid_unwind(drew.load(Ordering::Relaxed))
            .windows(4)
            .any(|w| w == b"\x1b[2J")
    };
    let (mut r, _wire, in_grid) = armed(120, 40, ImageProtocol::Sixel);
    assert!(!erases(in_grid), "nothing drawn yet");
    r.render(&office(), pack(), t0()).expect("render");
    assert!(erases(in_grid));
}

/// While a modal stays open, the tile under its title is never sent, tick
/// after tick, though every tile is owed.
#[test]
fn a_covered_tile_is_never_sent() {
    let (mut r, wire) = painter(120, 40, ImageProtocol::Sixel);
    let scene = office();
    let cadence = ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), t0()).expect("render");
    wire.take();
    r.set_help_open(true);
    r.redraw().expect("redraw");
    let shape = ImageProtocol::Sixel.tile();
    let mut under = None;
    for tick in 1..4 {
        r.render(&scene, pack(), t0() + cadence * tick)
            .expect("render");
        let at = *under.get_or_insert_with(|| {
            let text = frame_text(r.frame_buffer());
            let (y, line) = text
                .lines()
                .enumerate()
                .find(|(_, l)| l.contains("? Keyboard"))
                .expect("the modal's title");
            let x = line[..line.find("? Keyboard").expect("title")]
                .chars()
                .count();
            (
                x as u16 / shape.cols * shape.cols,
                y as u16 / shape.rows * shape.rows,
            )
        });
        let sent = wire.take();
        if tick == 1 {
            assert!(
                sent.contains(SIXEL),
                "the tiles the modal leaves free are sent"
            );
        }
        let to = format!("\x1b[{};{}H{SIXEL}", at.1 + 1, at.0 + 1);
        assert!(!sent.contains(&to), "tick {tick}");
    }
}

/// A backend that marks on `wire` where each flush of cells lands
/// among the transmits.
struct Logged {
    inner: Window,
    wire: Wire,
}

const FLUSH: &str = "<flush>";

impl ratatui::backend::Backend for Logged {
    type Error = <TestBackend as ratatui::backend::Backend>::Error;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let cells: Vec<_> = content.collect();
        if !cells.is_empty() {
            self.wire
                .bytes
                .lock()
                .expect("lock")
                .extend_from_slice(FLUSH.as_bytes());
        }
        self.inner.draw(cells.into_iter())
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
        self.inner.get_cursor_position()
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
        self.inner.size()
    }
    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        self.inner.window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

/// kitty's images go out before the placeholders that show them; SIXEL's
/// after the cells they must avoid.
#[test]
fn kitty_transmits_before_the_flush_and_sixel_after() {
    for (protocol, intro, first) in [
        (ImageProtocol::Kitty, TRANSMIT, true),
        (ImageProtocol::Sixel, SIXEL, false),
    ] {
        let wire = Wire::default();
        let backend = Logged {
            inner: Window::new(120, 40),
            wire: wire.clone(),
        };
        let mut r = TuiRenderer::new(
            Terminal::new(backend).expect("terminal"),
            normal_theme(),
            vec![],
            pack_arc(),
        );
        r.set_cutaway(
            TileCutaway::new(fit(120, 40), CELL, protocol, false, Box::new(wire.clone()))
                .arming(Box::leak(Box::new(AtomicBool::new(false)))),
        );
        r.render(&office(), pack(), t0()).expect("render");
        let sent = wire.take();
        let (image, flush) = (
            sent.find(intro).expect("an image"),
            sent.find(FLUSH).expect("a flush"),
        );
        assert_eq!(image < flush, first, "{protocol:?}");
    }
}

/// The cutaway's twin of the classic's refused-frame clamp: the door closes
/// on time while the frame is refused.
#[test]
fn a_refused_cutaway_frame_keeps_the_doors_clamp_on_time() {
    let (mut r, _wire) = kitty(120, 40);
    let scene = office();
    r.render(&scene, pack(), t0()).expect("render");
    assert!(
        r.floors[0].ctx.door_anim_max_ms > 0,
        "the entry walk holds the door"
    );
    let (small_cols, small_rows) = too_small_terminal();
    r.terminal.backend_mut().resize(small_cols, small_rows);
    r.render(&scene, pack(), t0() + Duration::from_secs(600))
        .expect("render");
    assert_eq!(r.floors[0].ctx.door_anim_max_ms, 0, "the walk long arrived");
}

/// A slide cancelled on its first frame never paints, so nothing marks the
/// screen as the slide's; the destination floor drawn before still repaints
/// whole once the terminal is back, not as a diff of its own last frame.
#[test]
fn a_slide_cancelled_on_its_first_frame_repaints_the_destination_whole() {
    let (cols, rows) = (120, 40);
    let (mut r, wire) = painter(cols, rows, ImageProtocol::Sixel);
    let scene = two_floor_scene();
    let mut now = t0();
    r.render(&scene, pack(), now).expect("render");
    let every = wire.take().matches(SIXEL).count();
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, pack(), &mut now, 1);
    r.navigate_floor(0, now);
    render_until_settled(&mut r, &scene, pack(), &mut now, 0);
    r.navigate_floor(1, now);
    let (small_cols, small_rows) = too_small_terminal();
    r.terminal.backend_mut().resize(small_cols, small_rows);
    now += ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), now).expect("render");
    assert_eq!(r.current_floor(), 1);
    r.terminal.backend_mut().resize(cols, rows);
    wire.take();
    now += ImageProtocol::Sixel.cadence();
    r.render(&scene, pack(), now).expect("render");
    assert_eq!(wire.take().matches(SIXEL).count(), every);
}

/// A floor can leave the screen without a slide: the top floor's last agent
/// ends, the floor count drops, and the view clamps to the floor below. That
/// floor's raster diffs against its own last frame, not the vanished floor
/// on screen, so the terminal ends up holding exactly what a fresh painter of
/// that floor sends.
#[test]
fn a_floor_clamped_into_view_repaints_whole() {
    let held = |wire: &str, tiles: &mut std::collections::BTreeMap<u32, Vec<u8>>| {
        tiles.extend(kitty_images(wire));
    };
    let (cols, rows) = (120, 40);
    let (mut r, wire) = kitty(cols, rows);
    let mut scene = two_floor_scene();
    let mut now = t0();
    let mut terminal = std::collections::BTreeMap::new();
    r.render(&scene, pack(), now).expect("render");
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, pack(), &mut now, 1);
    scene
        .agents
        .remove(&AgentId::from_transcript_path("/n/1.jsonl"));
    now += ImageProtocol::Kitty.cadence();
    r.render(&scene, pack(), now).expect("render");
    assert_eq!(r.current_floor(), 0, "the view clamped");
    held(&wire.take(), &mut terminal);

    let (mut fresh, fresh_wire) = kitty(cols, rows);
    fresh.render(&scene, pack(), now).expect("render");
    let mut want = std::collections::BTreeMap::new();
    held(&fresh_wire.take(), &mut want);
    assert!(
        terminal == want,
        "the terminal still shows the vanished floor"
    );
}
