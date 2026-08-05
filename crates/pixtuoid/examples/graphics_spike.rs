//! Phase 0 spike — does `ratatui-image` carry the dirty-rect design?
//!
//! The 2.5D plan leans on one unverified claim: that a static room can be
//! transmitted ONCE and thereafter only re-placed, while the moving parts ride
//! as a handful of small cell-aligned rectangles. If instead every frame
//! re-transmits its payload, the plan's cheap path does not exist and the
//! per-frame cost is the full encode.
//!
//! Reading `kitty.rs` says `make_transmit()` returns the payload only the first
//! time. Reading is not measuring, so this drives the real widget through a
//! ratatui `TestBackend` and counts the bytes each frame actually emits.
//!
//! Run: cargo run --release --example graphics_spike

use image::{DynamicImage, Rgb, RgbImage};
use ratatui::backend::TestBackend;
use ratatui::layout::{Rect, Size};
use ratatui::Terminal;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::{Image, Resize};

/// Kitty's transmit introducer. Its presence in a frame means that frame paid
/// to ship pixels; its absence means the frame only re-placed what was already
/// resident — which is the whole question.
const KITTY_TRANSMIT: &str = "\x1b_G";

fn solid(w: u32, h: u32, c: [u8; 3]) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb(c)))
}

/// Everything the backend wrote this frame, as one string. The kitty backend
/// stores its escape sequences in the cell symbols, so the buffer IS the wire.
fn frame_bytes(term: &mut Terminal<TestBackend>, draw: impl FnOnce(&mut ratatui::Frame)) -> String {
    term.draw(|f| draw(f)).expect("draw");
    term.backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// A REAL font size, or the cell<->pixel mapping is a fiction and every byte
/// count below measures the wrong image. `halfblocks()` sets a half-block
/// cell, which is not what a kitty terminal has.
fn main() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize((8, 16).into());
    picker.set_protocol_type(ProtocolType::Kitty);
    println!(
        "protocol {:?}, font cell {:?}\n",
        picker.protocol_type(),
        picker.font_size()
    );

    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("terminal");

    // The room: one big image, the thing we never want to re-send.
    let room = picker
        .new_protocol(
            solid(960, 640, [36, 40, 59]),
            Size::new(96, 32),
            Resize::Fit(None),
        )
        .expect("room protocol");

    // The moving parts: small cell-aligned rectangles, one per changed region.
    let tiles: Vec<_> = (0..6)
        .map(|i| {
            picker
                .new_protocol(
                    solid(64, 64, [80 + i * 20, 120, 200]),
                    Size::new(8, 4),
                    Resize::Fit(None),
                )
                .expect("tile protocol")
        })
        .collect();

    // --- claim 1: distinct images get distinct ids -------------------------
    // Two images sharing an id would clobber each other in the terminal's
    // image store, so the whole tile scheme depends on this.
    // Each probe gets an area AT LEAST its own size. `Image` is a fixed-size
    // widget: given a smaller area it renders NOTHING — no error, no clip, no
    // scale. The first version of this probe passed a 40x12 area to a 96x32
    // room and silently measured an empty buffer. That silent no-op is also a
    // live constraint on the tile scheme: a dirty rect smaller than its tile
    // makes the whole sprite vanish.
    let probe: Vec<String> = std::iter::once((&room, 96u16, 32u16))
        .chain(tiles.iter().map(|t| (t, 8u16, 4u16)))
        .map(|(p, w, h)| {
            let mut t = Terminal::new(TestBackend::new(120, 40)).expect("t");
            frame_bytes(&mut t, |f| {
                f.render_widget(Image::new(p), Rect::new(0, 0, w, h));
            })
        })
        .collect();
    // The id is carried as an RGB colour escape (plus a diacritic for the high
    // byte), not as an `i=` key in the placement — so probe the colour.
    println!("claim 1 — distinct ids per image");
    let mut seen = std::collections::HashSet::new();
    for (n, s) in probe.iter().enumerate() {
        let who = if n == 0 {
            "room".to_string()
        } else {
            format!("tile{}", n - 1)
        };
        let colour = s
            .find("\x1b[38;2;")
            .map(|i| s[i + 7..].split('m').next().unwrap_or("?").to_string())
            .unwrap_or_else(|| "<none>".into());
        println!(
            "    {who:<7} id colour: {colour:<20} (len {}, transmit {}, placeholders {})",
            s.len(),
            s.contains(KITTY_TRANSMIT),
            s.matches('\u{10EEEE}').count()
        );
        if colour != "<none>" {
            seen.insert(colour);
        }
    }
    // An id we failed to EXTRACT must not be scored as an id we found — that
    // was the first version of this check, and it reported OK on a miss.
    let extracted = probe.iter().filter(|s| s.contains("\x1b[38;2;")).count();
    println!(
        "    -> extracted {}/{}, {} distinct: {}",
        extracted,
        probe.len(),
        seen.len(),
        if extracted == probe.len() && seen.len() == probe.len() {
            "OK"
        } else {
            "INCONCLUSIVE — not all ids were read back"
        }
    );

    // --- claim 2: transmit-once, then place only ---------------------------
    println!("\nclaim 2 — is the payload re-sent every frame?");
    let mut sizes = Vec::new();
    for frame in 0..4 {
        let bytes = frame_bytes(&mut term, |f| {
            f.render_widget(Image::new(&room), Rect::new(0, 0, 96, 32));
            for (i, t) in tiles.iter().enumerate() {
                let x = 4 + (i as u16 % 3) * 30;
                let y = 4 + (i as u16 / 3) * 12;
                f.render_widget(Image::new(t), Rect::new(x, y, 8, 4));
            }
        });
        let transmits = bytes.matches(KITTY_TRANSMIT).count();
        println!(
            "    frame {frame}: {:>8} bytes, {transmits} transmit sequence(s)",
            bytes.len()
        );
        sizes.push((bytes.len(), transmits));
    }

    // --- claim 3: does a tile-only frame cost less than a full-room frame? --
    println!("\nclaim 3 — per-frame PLACEMENT cost (payload already resident)");
    let mut t2 = Terminal::new(TestBackend::new(120, 40)).expect("t2");
    let room_only = frame_bytes(&mut t2, |f| {
        f.render_widget(Image::new(&room), Rect::new(0, 0, 96, 32));
    });
    let mut t3 = Terminal::new(TestBackend::new(120, 40)).expect("t3");
    let tiles_only = frame_bytes(&mut t3, |f| {
        for (i, t) in tiles.iter().enumerate() {
            let x = 4 + (i as u16 % 3) * 30;
            let y = 4 + (i as u16 / 3) * 12;
            f.render_widget(Image::new(t), Rect::new(x, y, 8, 4));
        }
    });
    println!("    room placement    : {:>8} bytes/frame", room_only.len());
    println!(
        "    6 tiles placement : {:>8} bytes/frame",
        tiles_only.len()
    );

    // The tiles above never changed their pixels, so they only ever paid one
    // transmit. A walking sprite changes pixels every frame, so its region must
    // be re-transmitted. That is the real steady state.
    println!("\nclaim 4 — tiles whose PIXELS change every frame (a moving sprite)");
    for (cells_w, cells_h, count) in [(8u16, 4u16, 6usize), (12, 8, 20)] {
        let mut total = 0usize;
        let mut t = Terminal::new(TestBackend::new(120, 40)).expect("t");
        for frame in 0..3u8 {
            let fresh: Vec<_> = (0..count)
                .map(|i| {
                    picker
                        .new_protocol(
                            solid(
                                u32::from(cells_w) * 8,
                                u32::from(cells_h) * 16,
                                [40 + frame * 5, 60 + (i as u8 % 8) * 10, 120],
                            ),
                            Size::new(cells_w, cells_h),
                            Resize::Fit(None),
                        )
                        .expect("fresh tile")
                })
                .collect();
            let bytes = frame_bytes(&mut t, |f| {
                for (i, p) in fresh.iter().enumerate() {
                    let x = 2 + (i as u16 % 5) * 14;
                    let y = 2 + (i as u16 / 5) * 9;
                    f.render_widget(Image::new(p), Rect::new(x, y, cells_w, cells_h));
                }
            });
            if frame == 2 {
                total = bytes.len();
            }
        }
        println!(
            "    {count:>2} x {cells_w}x{cells_h}-cell tiles re-sent: {:>8} bytes/frame  -> {:>6.1} MiB/s @30fps",
            total,
            total as f64 * 30.0 / (1024.0 * 1024.0)
        );
    }

    println!("\nverdict");
    // The question is whether pixels are re-shipped, so COUNT TRANSMITS. An
    // earlier version compared frame byte sizes, which silently inverted once
    // the claim-1 probe warmed the protocols — a shrinking byte count is a
    // proxy, the transmit count is the fact.
    let resend = sizes.iter().any(|(_, t)| *t > 0);
    let steady = sizes.last().map(|(b, _)| *b).unwrap_or(0);
    if resend {
        println!("    a steady-state frame still carries a transmit — pixels ARE re-shipped.");
    } else {
        println!(
            "    {} steady frames, 0 transmits, {steady} bytes each ({:.2} MiB/s @30fps).",
            sizes.len(),
            steady as f64 * 30.0 / (1024.0 * 1024.0)
        );
        println!("    transmit-once + re-place CONFIRMED: the design's cheap path exists.");
    }
}
