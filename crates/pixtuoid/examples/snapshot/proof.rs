//! `--proof`: the §3 split-screen causal-proof renderer. ONE committed CC session
//! fixture drives BOTH sides of every frame: the left panel types the session
//! (terminal chrome, 8x8 pixel font), the right side is the REAL draw_scene pass
//! replaying the SAME decoded AgentEvent stream through the real Reducer — the two
//! sides structurally cannot desync. Annotations, the connector, and the coda
//! strip are burned into the frames; scripts/gen-media.py (kind:"proof") encodes them.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context as _, Result};
use image::{Rgba, RgbaImage};
use pixtuoid::tui::renderer::{draw_scene, DrawCtx};
use pixtuoid_core::source::claude_code::{
    cc_derive_label, cc_id_from_path, decode_cc_line, SOURCE_NAME,
};
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
use pixtuoid_scene::font;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::encode::cells_to_rgba;
use crate::{CELL_H, CELL_W};

// ── geometry (px); every canvas dim is even so yuv420p never crops ──
// PANEL_W targets a ~44/56 typed-panel/office split (the pinned mock aesthetic)
// against the reference render this feature ships at (--cols 120 --rows 52 ->
// office_w = 960px): 760 / (760 + 960) ≈ 0.44.
const PANEL_W: u32 = 760;
const TALL_PANEL_H: u32 = 400; // terminal panel height (tall layout)
const HEADER_H: u32 = 32; // chrome strip: "captured..." / "pixtuoid"
const PAD: u32 = 16;
const LINE_H: u32 = 28;
const TEXT_SCALE: i32 = 2; // 8x8 glyphs → 16px
const TYPE_CPS: u64 = 30; // typewriter reveal, chars/sec
const PREAMBLE_MS: u64 = 6000; // "$ claude" + session start precede the first fixture line

// the burned coda strip — a full-width caption, theme-independent like the panel
const CODA_SCALE: i32 = 1; // 8px glyphs; the caption is long, keep it compact
const CODA_LINE_H: u32 = 14;
const CODA_PAD: u32 = 10;
const CODA_TEXT: &str = "the left pane happened in a terminal. the right pane is the same \
event stream, drawn by the same engine -- nothing is mocked.";

// burned panel palette — theme-independent (the office side carries the theme)
const PANEL_BG: Rgba<u8> = Rgba([13, 15, 19, 255]);
const CHROME_BG: Rgba<u8> = Rgba([24, 27, 33, 255]);
const EDGE: Rgba<u8> = Rgba([70, 74, 84, 255]);
const INK: Rgba<u8> = Rgba([214, 214, 208, 255]);
const PROMPT: Rgba<u8> = Rgba([139, 196, 138, 255]);
// coral — the pinned connector/annotation color (sampled from the approved mock).
const ANNOT: Rgba<u8> = Rgba([224, 122, 85, 255]);
const CODA_BG: Rgba<u8> = Rgba([10, 9, 8, 255]);
const CODA_INK: Rgba<u8> = Rgba([150, 145, 135, 255]);

pub(crate) enum ProofLayout {
    Wide,
    Tall,
}

pub(crate) struct PanelLine {
    pub(crate) at_ms: u64,
    pub(crate) text: String,
    pub(crate) prompt: bool,
    /// Burned office-side callout, lit while this line is the newest annotated one.
    pub(crate) annotation: Option<&'static str>,
}

pub(crate) struct ProofScript {
    pub(crate) events: Vec<(u64, AgentEvent)>,
    pub(crate) lines: Vec<PanelLine>,
    /// The fixture's own capture date (from its first timestamp) — the left
    /// panel titles itself as a past-tense archive, not the live ticker.
    pub(crate) capture_date: String,
}

/// Greedy word-wrap of `text` at `scale`-px 8x8 glyphs to fit within `max_width`
/// px. A single over-long word is kept whole (never split mid-word) rather than
/// looping forever; never returns an empty vec.
fn wrap_text(text: &str, max_width: i32, scale: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || font::text_width(&candidate, scale) <= max_width {
            cur = candidate;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

fn coda_lines(canvas_w: u32) -> Vec<String> {
    let max_w = (canvas_w as i32 - 2 * CODA_PAD as i32).max(8 * CODA_SCALE);
    wrap_text(CODA_TEXT, max_w, CODA_SCALE)
}

/// Pixel height of the coda strip for a canvas of width `canvas_w` — a pure
/// function of the (fixed) caption + width, so `canvas_dims` can stay pure too.
fn coda_height(canvas_w: u32) -> u32 {
    let n = coda_lines(canvas_w).len() as u32;
    2 * CODA_PAD + n * CODA_LINE_H
}

pub(crate) fn canvas_dims(layout: &ProofLayout, office_w: u32, office_h: u32) -> (u32, u32) {
    match layout {
        ProofLayout::Wide => {
            let w = PANEL_W + office_w;
            (w, HEADER_H + office_h + coda_height(w))
        }
        ProofLayout::Tall => {
            let h = HEADER_H + TALL_PANEL_H + HEADER_H + office_h;
            (office_w, h + coda_height(office_w))
        }
    }
}

pub(crate) fn revealed_chars(at_ms: u64, elapsed_ms: u64, len: usize) -> usize {
    if elapsed_ms < at_ms {
        return 0;
    }
    (((elapsed_ms - at_ms) * TYPE_CPS) / 1000).min(len as u64) as usize
}

/// The newest annotated line already on screen — its connector + callout are lit.
pub(crate) fn active_annotation(lines: &[PanelLine], elapsed_ms: u64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| l.annotation.is_some() && l.at_ms <= elapsed_ms)
        .map(|(i, _)| i)
}

fn ts_ms(v: &serde_json::Value) -> Result<i64> {
    let ts = v
        .get("timestamp")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("fixture line missing timestamp"))?;
    Ok(chrono::DateTime::parse_from_rfc3339(ts)
        .with_context(|| format!("bad fixture timestamp {ts:?}"))?
        .timestamp_millis())
}

/// The fixture's capture date (`YYYY-MM-DD`), from its first line's timestamp —
/// the panel title's "past-tense archive" date.
fn capture_date_str(v: &serde_json::Value) -> Result<String> {
    let ts = v
        .get("timestamp")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("fixture line missing timestamp"))?;
    Ok(chrono::DateTime::parse_from_rfc3339(ts)
        .with_context(|| format!("bad fixture timestamp {ts:?}"))?
        .format("%Y-%m-%d")
        .to_string())
}

/// First human-meaningful arg of a tool_use input, for the panel line.
fn tool_arg(input: Option<&serde_json::Value>) -> String {
    let Some(obj) = input.and_then(|i| i.as_object()) else {
        return String::new();
    };
    for key in ["file_path", "command", "pattern", "path"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

pub(crate) fn build_script(fixture: &Path) -> Result<ProofScript> {
    let raw = fs::read_to_string(fixture)
        .with_context(|| format!("read proof fixture {}", fixture.display()))?;
    let stem = cc_id_from_path(fixture);
    anyhow::ensure!(!stem.is_empty(), "fixture path has no filename stem");
    let agent_id = AgentId::from_parts(SOURCE_NAME, &stem);
    let path_str = fixture.to_string_lossy().into_owned();

    let parsed: Vec<serde_json::Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .context("proof fixture is not valid JSONL")?;
    let first = parsed
        .first()
        .ok_or_else(|| anyhow!("empty proof fixture"))?;
    let t0 = ts_ms(first)? - PREAMBLE_MS as i64;
    let capture_date = capture_date_str(first)?;
    let cwd = first
        .get("cwd")
        .and_then(|s| s.as_str())
        .unwrap_or("/")
        .to_string();

    let mut events: Vec<(u64, AgentEvent)> = Vec::new();
    let mut lines: Vec<PanelLine> = Vec::new();
    lines.push(PanelLine {
        at_ms: 0,
        text: "$ claude".into(),
        prompt: true,
        annotation: None,
    });
    lines.push(PanelLine {
        at_ms: 800,
        text: "* session started".into(),
        prompt: false,
        annotation: Some("a sprite walks in"),
    });
    // Registration is the WATCHER's job in production, not the decoder's — the
    // render synthesizes it once, exactly like sample_scene fabricates its roster.
    events.push((
        800,
        AgentEvent::SessionStart {
            agent_id,
            source: SOURCE_NAME.to_string(),
            session_id: stem.clone(),
            cwd: cwd.clone().into(),
            parent_id: None,
        },
    ));
    events.push((
        800,
        AgentEvent::Rename {
            agent_id,
            label: cc_derive_label(fixture, SOURCE_NAME, Path::new(&cwd)),
        },
    ));

    let mut tool_idx = 0usize;
    let mut last_ms = 800u64;
    for v in &parsed {
        let rel = (ts_ms(v)? - t0).max(0) as u64;
        last_ms = last_ms.max(rel);
        let ty = v.get("type").and_then(|s| s.as_str()).unwrap_or("");
        let content = v.get("message").and_then(|m| m.get("content"));
        if ty == "user" {
            if let Some(text) = content.and_then(|c| c.as_str()) {
                lines.push(PanelLine {
                    at_ms: rel,
                    text: format!("> {text}"),
                    prompt: true,
                    annotation: None,
                });
                continue; // plain prompt: decode_cc_line emits nothing for it
            }
        }
        if ty == "assistant" {
            if let Some(blocks) = content.and_then(|c| c.as_array()) {
                for b in blocks {
                    if b.get("type").and_then(|s| s.as_str()) == Some("tool_use") {
                        let name = b.get("name").and_then(|s| s.as_str()).unwrap_or("?");
                        let annotation = Some(match tool_idx {
                            0 => "-- that's you",
                            1 => "monitor flips",
                            _ => "monitor flips again",
                        });
                        tool_idx += 1;
                        lines.push(PanelLine {
                            at_ms: rel,
                            text: format!("[{name}] {}", tool_arg(b.get("input"))),
                            prompt: false,
                            annotation,
                        });
                    }
                }
            }
        }
        // BOTH tool_use and tool_result lines decode through the REAL decoder —
        // the right side replays exactly what production would have seen.
        for ev in decode_cc_line(&path_str, SOURCE_NAME, v.clone())? {
            events.push((rel, ev));
        }
    }
    lines.push(PanelLine {
        at_ms: last_ms + 600,
        text: "ok - done".into(),
        prompt: false,
        annotation: Some("back to idle"),
    });
    events.sort_by_key(|(at, _)| *at);
    Ok(ProofScript {
        events,
        lines,
        capture_date,
    })
}

fn put(img: &mut RgbaImage, x: i32, y: i32, c: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for j in y..(y + h).min(img.height()) {
        for i in x..(x + w).min(img.width()) {
            img.put_pixel(i, j, c);
        }
    }
}

fn text_at(img: &mut RgbaImage, s: &str, x: i32, y: i32, scale: i32, c: Rgba<u8>) {
    font::draw_text(s, x, y, scale, |px, py| put(img, px, py, c));
}

fn text(img: &mut RgbaImage, s: &str, x: i32, y: i32, c: Rgba<u8>) {
    text_at(img, s, x, y, TEXT_SCALE, c);
}

/// A small filled disc anchoring the connector to an exact scene point — reuses
/// the office's own status-dot glyph vocabulary (`font::glyph8x8`'s `●`) rather
/// than a bespoke circle rasterizer.
fn dot(img: &mut RgbaImage, cx: i32, cy: i32, scale: i32, c: Rgba<u8>) {
    text_at(img, "\u{25CF}", cx - 4 * scale, cy - 4 * scale, scale, c);
}

/// 2px-thick dashed horizontal connector (4-on/4-off), the burned "wire".
fn dashed_h(img: &mut RgbaImage, x0: i32, x1: i32, y: i32, c: Rgba<u8>) {
    for x in x0..x1 {
        if (x - x0) / 4 % 2 == 0 {
            put(img, x, y, c);
            put(img, x, y + 1, c);
        }
    }
}

fn chrome(img: &mut RgbaImage, x: u32, y: u32, w: u32, title: &str) {
    fill(img, x, y, w, HEADER_H, CHROME_BG);
    fill(img, x, y + HEADER_H - 1, w, 1, EDGE);
    text(img, title, (x + PAD) as i32, (y + 8) as i32, INK);
}

fn panel_body(
    img: &mut RgbaImage,
    origin: (u32, u32),
    size: (u32, u32),
    script: &ProofScript,
    elapsed_ms: u64,
) {
    fill(img, origin.0, origin.1, size.0, size.1, PANEL_BG);
    let max_w = (size.0 as i32 - 2 * PAD as i32).max(8 * TEXT_SCALE);
    let mut row = 0u32;
    for line in &script.lines {
        let total_len = line.text.chars().count();
        let shown = revealed_chars(line.at_ms, elapsed_ms, total_len);
        if shown == 0 && line.at_ms > elapsed_ms {
            continue;
        }
        // Wrapped purely at render time: the typewriter reveal walks the FLAT
        // string's character stream (build_script/reveal timing untouched); a
        // long line simply pushes later lines down as more of it becomes
        // visible, like a real terminal.
        let wrapped = wrap_text(&line.text, max_w, TEXT_SCALE);
        let color = if line.prompt { PROMPT } else { INK };
        let mut remaining = shown;
        for sub in &wrapped {
            if remaining == 0 {
                break;
            }
            let sub_len = sub.chars().count();
            let take = remaining.min(sub_len);
            let y = origin.1 + PAD + row * LINE_H;
            if y + LINE_H > origin.1 + size.1 {
                return; // panel full — the timeline is authored to fit; guard anyway
            }
            let visible: String = sub.chars().take(take).collect();
            text(img, &visible, (origin.0 + PAD) as i32, y as i32, color);
            if take < sub_len {
                let cx = origin.0 as i32 + PAD as i32 + font::text_width(&visible, TEXT_SCALE);
                fill(img, cx.max(0) as u32, y, 10, 16, INK);
            }
            row += 1;
            remaining -= take;
        }
    }
}

pub(crate) fn compose_frame(
    layout: &ProofLayout,
    office: &RgbaImage,
    script: &ProofScript,
    elapsed_ms: u64,
    desk_px: (u32, u32),
) -> RgbaImage {
    let (ow, oh) = (office.width(), office.height());
    let (w, h) = canvas_dims(layout, ow, oh);
    let mut img = RgbaImage::from_pixel(w, h, PANEL_BG);
    let panel_title = format!("~ captured claude code session · {}", script.capture_date);
    let (panel_origin, panel_size, office_origin) = match layout {
        ProofLayout::Wide => {
            chrome(&mut img, 0, 0, PANEL_W, &panel_title);
            chrome(&mut img, PANEL_W, 0, ow, "pixtuoid");
            ((0, HEADER_H), (PANEL_W, oh), (PANEL_W, HEADER_H))
        }
        ProofLayout::Tall => {
            chrome(&mut img, 0, 0, ow, &panel_title);
            chrome(&mut img, 0, HEADER_H + TALL_PANEL_H, ow, "pixtuoid");
            (
                (0, HEADER_H),
                (ow, TALL_PANEL_H),
                (0, HEADER_H + TALL_PANEL_H + HEADER_H),
            )
        }
    };
    panel_body(&mut img, panel_origin, panel_size, script, elapsed_ms);
    image::imageops::overlay(
        &mut img,
        office,
        office_origin.0 as i64,
        office_origin.1 as i64,
    );
    // divider between the halves (the coda strip, drawn last, trims its own
    // bottom slice back off)
    match layout {
        ProofLayout::Wide => fill(&mut img, PANEL_W - 1, 0, 2, HEADER_H + oh, EDGE),
        ProofLayout::Tall => fill(&mut img, 0, HEADER_H + TALL_PANEL_H, w, 1, EDGE),
    }

    // burned connector + callout for the newest annotated line, anchored to the
    // ACTUAL working sprite's desk (no hand-placed coordinates)
    if let Some(i) = active_annotation(&script.lines, elapsed_ms) {
        if let Some(label) = script.lines[i].annotation {
            let desk = (
                (office_origin.0 + desk_px.0) as i32,
                (office_origin.1 + desk_px.1) as i32,
            );
            // sits on the floor tile just above the desk — off the sprite itself
            let anchor_y = desk.1 - 10;
            match layout {
                ProofLayout::Wide => {
                    let text_w = font::text_width(label, TEXT_SCALE);
                    let label_x = (desk.0 - text_w - 16).max((PANEL_W + PAD) as i32);
                    dashed_h(
                        &mut img,
                        (PANEL_W - PAD) as i32,
                        desk.0 - 10,
                        anchor_y,
                        ANNOT,
                    );
                    text(
                        &mut img,
                        label,
                        label_x,
                        anchor_y - 22 + 1,
                        Rgba([0, 0, 0, 255]),
                    );
                    text(&mut img, label, label_x, anchor_y - 22, ANNOT);
                    dot(&mut img, desk.0 - 6, anchor_y, 2, ANNOT);
                }
                ProofLayout::Tall => {
                    // no cross-panel connector line (the panel sits above, not
                    // beside) — just the callout well clear of the sprite's
                    // head/name-tag, plus a dot marking the desk itself.
                    let text_w = font::text_width(label, TEXT_SCALE);
                    let label_x = (desk.0 - text_w - 16).max(PAD as i32);
                    let label_y = desk.1 - 44;
                    text(&mut img, label, label_x, label_y + 1, Rgba([0, 0, 0, 255]));
                    text(&mut img, label, label_x, label_y, ANNOT);
                    dot(&mut img, desk.0 - 6, desk.1 - 10, 2, ANNOT);
                }
            }
        }
    }

    // the burned coda strip — a full-width caption below everything
    let ch = coda_height(w);
    let coda_y0 = h - ch;
    fill(&mut img, 0, coda_y0, w, ch, CODA_BG);
    fill(&mut img, 0, coda_y0, w, 1, EDGE);
    for (i, cline) in coda_lines(w).iter().enumerate() {
        let lw = font::text_width(cline, CODA_SCALE);
        let x = ((w as i32 - lw) / 2).max(0);
        let y = (coda_y0 + CODA_PAD) as i32 + i as i32 * CODA_LINE_H as i32;
        text_at(&mut img, cline, x, y, CODA_SCALE, CODA_INK);
    }
    img
}

pub(crate) struct ProofJob<'a> {
    pub(crate) fixture: &'a Path,
    pub(crate) frames_dir: &'a Path,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) fps: u64,
    pub(crate) secs: u64,
    pub(crate) max_desks: usize,
    pub(crate) theme: &'static pixtuoid_scene::theme::Theme,
    pub(crate) pack: &'a pixtuoid_core::sprite::format::Pack,
    pub(crate) start: SystemTime,
}

pub(crate) fn render_proof(job: &ProofJob) -> Result<()> {
    let script = build_script(job.fixture)?;
    let mut pending: VecDeque<(u64, AgentEvent)> = script.events.iter().cloned().collect();

    // The first agent takes desk 0 — anchor the burned callout to home_desks[0]
    // in the SAME layout draw_scene computes (buf = cols x (rows-1)*2, the footer
    // row excluded — the compute_crop_rect convention, encode.rs:190-198).
    let buf_h = job.rows.saturating_sub(1).saturating_mul(2);
    let layout = pixtuoid_scene::layout::SceneLayout::compute_with_seed(
        job.cols,
        buf_h,
        Some(job.max_desks),
        0,
    )
    .ok_or_else(|| anyhow!("scene too small for a proof layout"))?;
    let desk = layout
        .home_desks
        .first()
        .copied()
        .ok_or_else(|| anyhow!("layout has no home desks"))?;
    // half-block buffer → PNG px: 1 buf-px per cell across, 2 per cell down
    let desk_px = (desk.x as u32 * CELL_W, (desk.y as u32 / 2) * CELL_H);

    let backend = TestBackend::new(job.cols, job.rows);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 });
    let mut store = pixtuoid_scene::floor::FloorCtx::new();
    let mut scene = SceneState::uniform(job.max_desks);
    let mut reducer = Reducer::new();
    let mut chitchat_state = std::collections::HashMap::new();

    let wide_dir = job.frames_dir.join("wide");
    let tall_dir = job.frames_dir.join("tall");
    fs::create_dir_all(&wide_dir)?;
    fs::create_dir_all(&tall_dir)?;

    let office_w = job.cols as u32 * CELL_W;
    let office_h = job.rows as u32 * CELL_H;
    let frames = (job.secs * job.fps) as usize;
    for i in 0..frames {
        // exact math, not accumulated frame_ms — same rationale as save_renderer_gif
        let elapsed = i as u64 * 1000 / job.fps.max(1);
        let now = job.start + Duration::from_millis(elapsed);
        while pending.front().is_some_and(|(at, _)| *at <= elapsed) {
            if let Some((_, ev)) = pending.pop_front() {
                reducer.apply(&mut scene, ev, now, Transport::Jsonl);
            }
        }
        // `apply` only runs its debounce/expiry pass as a side effect of an
        // incoming event — once the fixture's events are drained, nothing
        // would ever settle Active -> Idle for the rest of the idle tail
        // without this (see `Reducer::tick`'s own doc comment). The real
        // runtime calls this every render tick independent of new events.
        reducer.tick(&mut scene, now);
        let mut draw_ctx = DrawCtx {
            buf: &mut buf,
            store: &mut store,
            mouse_pos: None,
            pinned_agent: None,
            debug_walkable: false,
            theme: job.theme,
            theme_picker: None,
            floor_info: None,
            per_floor: Default::default(),
            gateway: None,
            floor: pixtuoid_scene::floor::FloorMeta::ground(),
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
        draw_scene(&mut term, &scene, job.pack, now, &mut draw_ctx)?;
        let office = cells_to_rgba(
            term.backend().buffer(),
            job.cols,
            job.rows,
            office_w,
            office_h,
        );
        for (kind, dir) in [
            (ProofLayout::Wide, &wide_dir),
            (ProofLayout::Tall, &tall_dir),
        ] {
            compose_frame(&kind, &office, &script, elapsed, desk_px)
                .save(dir.join(format!("f{:04}.png", i + 1)))?;
        }
        if (i + 1).is_multiple_of(job.fps as usize) {
            eprint!("\r  proof: {}/{}s", (i + 1) / job.fps as usize, job.secs);
        }
    }
    eprintln!("\r  proof: {frames} frames x2 layouts @ {}fps", job.fps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        // The example lives in crates/pixtuoid; the fixture is core's — one hop up.
        Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../pixtuoid-core/tests/sources/fixtures/claude-code/proof-session/01000000-0000-7000-8000-0000000000f4.jsonl",
        )
    }

    #[test]
    fn build_script_pins_the_fixture_beats() {
        let s = build_script(&fixture_path()).unwrap();
        // 1 SessionStart + 1 Rename + 3 ActivityStart + 3 ActivityEnd
        assert_eq!(s.events.len(), 8);
        assert!(matches!(s.events[0].1, AgentEvent::SessionStart { .. }));
        assert!(matches!(s.events[1].1, AgentEvent::Rename { .. }));
        let starts = s
            .events
            .iter()
            .filter(|(_, e)| matches!(e, AgentEvent::ActivityStart { .. }))
            .count();
        let ends = s
            .events
            .iter()
            .filter(|(_, e)| matches!(e, AgentEvent::ActivityEnd { .. }))
            .count();
        assert_eq!((starts, ends), (3, 3));
        // at_ms is monotonic — the replay drains a front-ordered queue
        assert!(s.events.windows(2).all(|w| w[0].0 <= w[1].0));
        // panel: $ claude, session started, prompt, 3 tool lines, done = 7
        assert_eq!(s.lines.len(), 7);
        assert_eq!(s.lines[0].text, "$ claude");
        assert!(s.lines[2].prompt, "the user prompt renders as a prompt row");
        assert!(s.lines[6].text.contains("done"));
        // the first fixture line lands PREAMBLE_MS in
        assert_eq!(s.lines[2].at_ms, PREAMBLE_MS);
        // the fixture's own capture date — the panel title's past-tense archive
        assert_eq!(s.capture_date, "2026-01-01");
    }

    #[test]
    fn reveal_and_annotation_math() {
        assert_eq!(revealed_chars(1000, 999, 10), 0);
        assert_eq!(revealed_chars(1000, 1000, 10), 0);
        assert_eq!(revealed_chars(1000, 1100, 10), 3); // 30 cps → 3 chars in 100ms
        assert_eq!(revealed_chars(1000, 9000, 10), 10); // clamped to len
        let lines = vec![
            PanelLine {
                at_ms: 0,
                text: "a".into(),
                prompt: false,
                annotation: Some("x"),
            },
            PanelLine {
                at_ms: 500,
                text: "b".into(),
                prompt: false,
                annotation: None,
            },
            PanelLine {
                at_ms: 900,
                text: "c".into(),
                prompt: false,
                annotation: Some("y"),
            },
        ];
        assert_eq!(active_annotation(&lines, 100), Some(0));
        assert_eq!(active_annotation(&lines, 899), Some(0));
        assert_eq!(active_annotation(&lines, 900), Some(2));
    }

    #[test]
    fn canvas_dims_are_even_and_stack_correctly() {
        let (ww, wh) = canvas_dims(&ProofLayout::Wide, 960, 832);
        assert_eq!((ww, wh), (1720, 898));
        let (tw, th) = canvas_dims(&ProofLayout::Tall, 960, 832);
        assert_eq!((tw, th), (960, 1344));
        for d in [ww, wh, tw, th] {
            assert_eq!(d % 2, 0, "yuv420p needs even dims");
        }
    }

    #[test]
    fn compose_frame_matches_canvas_dims() {
        let s = build_script(&fixture_path()).unwrap();
        let office = RgbaImage::new(960, 832);
        for layout in [ProofLayout::Wide, ProofLayout::Tall] {
            let (w, h) = canvas_dims(&layout, 960, 832);
            let f = compose_frame(&layout, &office, &s, 10_000, (400, 300));
            assert_eq!((f.width(), f.height()), (w, h));
        }
    }

    #[test]
    fn coda_wraps_to_fit_narrow_canvas_but_not_wide_canvas() {
        // Wide reference canvas (1720px) fits the caption on one line; the
        // narrower Tall canvas (960px, cols=120) must wrap without dropping words.
        let wide = coda_lines(1720);
        let tall = coda_lines(960);
        assert_eq!(wide.len(), 1);
        assert_eq!(tall.len(), 2);
        for lines in [&wide, &tall] {
            let rejoined = lines.join(" ");
            assert_eq!(
                rejoined, CODA_TEXT,
                "wrapping must not drop or reorder words"
            );
        }
    }

    #[test]
    fn wrap_text_never_produces_an_empty_line_list() {
        assert_eq!(wrap_text("", 100, 1), vec![String::new()]);
        assert_eq!(wrap_text("hi", 4, 1), vec!["hi".to_string()]); // single word, never split
    }
}
