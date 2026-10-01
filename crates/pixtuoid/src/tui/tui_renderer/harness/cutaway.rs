//! The cutaway through the production render path: what reaches the
//! terminal as escapes, and what the cells show.
use super::*;
use crate::graphics::{CellSize, Fit, ImageProtocol};
use crate::tui::cutaway::TileCutaway;
use pixtuoid_core::sprite::format::Density;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

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

fn arc_pack() -> Arc<Pack> {
    static PACK: OnceLock<Arc<Pack>> = OnceLock::new();
    Arc::clone(PACK.get_or_init(|| Arc::new(pack().clone())))
}

/// The plan's fit over a `cols`×`rows` terminal's scene.
fn fit(cols: u16, rows: u16) -> Fit {
    let area = crate::tui::renderer::scene_rect(Rect::new(0, 0, cols, rows)).as_size();
    let fit = Fit::new(CELL, area, arc_pack().max_density_variant()).expect("fits");
    assert_eq!(fit.upscale(), 1);
    assert_eq!(fit.density(), Density::new(4).expect("nonzero"));
    fit
}

/// A renderer painting the cutaway over `protocol` into a `cols`×`rows`
/// terminal.
fn painter(cols: u16, rows: u16, protocol: ImageProtocol) -> (TuiRenderer<TestBackend>, Wire) {
    let mut r = build(cols, rows, vec![]);
    let wire = Wire::default();
    r.set_cutaway(TileCutaway::new(
        arc_pack(),
        fit(cols, rows),
        CELL,
        protocol,
        false,
        Box::new(wire.clone()),
    ));
    (r, wire)
}

fn kitty(cols: u16, rows: u16) -> (TuiRenderer<TestBackend>, Wire) {
    painter(cols, rows, ImageProtocol::Kitty)
}

fn office() -> SceneState {
    scene_with(vec![idle("/k/0.jsonl", 0, t0())], 16)
}

fn placeholders_in_row(r: &TuiRenderer<TestBackend>, y: u16) -> usize {
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
fn the_cutaway_shows_no_wall_display() {
    let (mut r, _wire) = kitty(120, 40);
    r.render(&office(), pack(), t0()).expect("render");
    assert!(!r.shows_wall_display());
    let mut classic = build(120, 40, vec![]);
    classic.render(&office(), pack(), t0()).expect("render");
    assert!(classic.shows_wall_display());
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
