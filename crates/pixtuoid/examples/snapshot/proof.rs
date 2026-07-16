//! `--proof`: the §3 split-screen causal-proof renderer. ONE committed CC session
//! fixture drives BOTH sides of every frame: the left panel types the session
//! (terminal chrome, an anti-aliased Monaspace Neon face — see `fonts/`), the
//! right side is the REAL draw_scene pass replaying the SAME decoded AgentEvent
//! stream through the real Reducer — the two sides structurally cannot desync.
//! The coral office annotations + connector dot render in the SAME AA face as
//! the panel (the 8x8 pixel-font split was retired with the bitmap font — the
//! user reversed it: "no 8x8 stand-in at all"); the burned coda strip likewise.
//! scripts/gen-media.py (kind:"proof") encodes the frames.

use anyhow::{anyhow, Context as _, Result};
use image::{Rgba, RgbaImage};
use pixtuoid::tui::renderer::{draw_scene, DrawCtx};
use pixtuoid_core::source::claude_code::{
    cc_derive_label, cc_id_from_path, decode_cc_line, SOURCE_NAME,
};
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

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
const ANNOT_FONT_PX: f32 = 16.0; // coral annotation callouts — same AA face as the panel
const TYPE_CPS: u64 = 30; // typewriter reveal, chars/sec
const PREAMBLE_MS: u64 = 6000; // "$ claude" + session start precede the first fixture line

// Every callout holds at most this long from its OWN at_ms, independent of
// when the next annotated line appears. Without a ceiling, "a sprite walks
// in" held for the full 8.2s gap to the NEXT annotation — front-loaded by
// PREAMBLE_MS, since that annotation's at_ms=800 predates the preamble that
// pads every later beat. 4200ms is the verified upper edge of the other four
// transitions' natural gaps (3500/3900/4200ms, `dbg_print_annotation_gaps`
// against the committed fixture) — high enough that none of them are
// affected, low enough to meaningfully shorten the one outlier.
const ANNOTATION_MAX_HOLD_MS: u64 = 4200;

// the PANEL's own text (Font C, user-picked): anti-aliased Monaspace Neon
// (OFL 1.1, `fonts/`) — the title, the typed body lines, and the coda strip.
// Sizes target the retired 8×8 font's 16px line metrics so LINE_H/wrap math
// stays proportioned; a pure-Rust rasterizer (ab_glyph) composites its
// grayscale coverage onto the panel's dark ground.
const PROOF_FONT_PX: f32 = 16.0;
const CODA_FONT_PX: f32 = 12.0;

// the burned coda strip — a full-width caption, theme-independent like the panel
const CODA_LINE_H: u32 = 18;
const CODA_PAD: u32 = 10;
// The "captured"/"happened in a terminal" framing (here + the `~ captured
// claude code session` panel title below) is DELIBERATE: the timeline is
// authored — the statusline-ticker disjointness pin REQUIRES authored strings
// — and the load-bearing half of the claim is engine truth (the right pane
// replays through the real decode_cc_line → Reducer → renderer).
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

/// Greedy word-wrap of `text` to fit within `max_width` px, measuring each
/// candidate line via the caller-supplied `width_fn` (the AA font's
/// metric-derived advance at the caller's own size). A single over-long word is
/// kept whole (never split mid-word) rather than looping forever; never returns
/// an empty vec.
fn wrap_text(text: &str, max_width: i32, width_fn: impl Fn(&str) -> i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || width_fn(&candidate) <= max_width {
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

/// Sum of the AA font's per-glyph pixel-scaled advances — `wrap_text`'s width
/// function for the panel/coda (both AA now). Monaspace Neon is monospace, but
/// summing real advances (rather than `chars * one_advance`) stays correct
/// even for a future proportional face.
fn aa_text_width_at(s: &str, px: f32) -> i32 {
    pixtuoid::aa_text::text_width(s, px)
}

/// Draws `s` in the AA face at pixel size `px`, top-left at `(x, top_y)`
/// (matching the office-side `text()`/`text_at()` call convention), alpha-
/// composited onto the existing pixels via `blend_px`. Returns the total
/// advance width, so callers needing it (the typing-cursor block) don't
/// recompute via a second `aa_text_width_at` call.
fn aa_draw_text_at(
    img: &mut RgbaImage,
    s: &str,
    x: i32,
    top_y: i32,
    px: f32,
    color: Rgba<u8>,
) -> i32 {
    pixtuoid::aa_text::draw_text_at(s, x, top_y, px, |gx, gy, coverage| {
        blend_px(img, gx, gy, color, coverage);
    })
}

/// Alpha-composite `color` onto the existing pixel at `coverage` (the AA
/// rasterizer's per-pixel grayscale strength) — the panel's text sits on the
/// dark `PANEL_BG`/`CODA_BG` ground, never a transparent surface, so a
/// straight linear blend (no separate alpha channel to preserve) is correct.
fn blend_px(img: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>, coverage: f32) {
    if x < 0 || y < 0 || (x as u32) >= img.width() || (y as u32) >= img.height() {
        return;
    }
    let bg = *img.get_pixel(x as u32, y as u32);
    // the ONE blend curve — see aa_text::blend_channel
    let mix = |fg: u8, bg: u8| pixtuoid::aa_text::blend_channel(bg, fg, coverage);
    img.put_pixel(
        x as u32,
        y as u32,
        Rgba([
            mix(color[0], bg[0]),
            mix(color[1], bg[1]),
            mix(color[2], bg[2]),
            255,
        ]),
    );
}

fn coda_lines(canvas_w: u32) -> Vec<String> {
    let floor = aa_text_width_at("M", CODA_FONT_PX);
    let max_w = (canvas_w as i32 - 2 * CODA_PAD as i32).max(floor);
    wrap_text(CODA_TEXT, max_w, |s| aa_text_width_at(s, CODA_FONT_PX))
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

/// The newest annotated line already on screen — its connector + callout are
/// lit for at most `ANNOTATION_MAX_HOLD_MS` past its OWN `at_ms`, UNLESS it's
/// the last annotated line in the script, which holds indefinitely. The cap
/// exists to stop a callout over-holding while it waits on a FUTURE beat
/// (that's the bug: "a sprite walks in" front-loaded by PREAMBLE_MS, so its
/// next beat was 8.2s out); the final beat has no next beat to wait on, so
/// capping it too would just go quiet for the clip's idle tail instead of
/// riding out to the coda, an unrelated regression the fixture's f0300 caught
/// (last annotation at_ms=20600, cap would expire it at 24800 — 1.1s before
/// the 26s clip ends). Once a non-final annotation's cap expires, this
/// returns `None` rather than falling back to an OLDER annotated line — an
/// expired callout goes quiet, it doesn't resurrect the previous one.
pub(crate) fn active_annotation(lines: &[PanelLine], elapsed_ms: u64) -> Option<usize> {
    let (i, line) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| l.annotation.is_some() && l.at_ms <= elapsed_ms)?;
    let is_final_annotation = lines[i + 1..].iter().all(|l| l.annotation.is_none());
    (is_final_annotation || elapsed_ms - line.at_ms <= ANNOTATION_MAX_HOLD_MS).then_some(i)
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

fn text(img: &mut RgbaImage, s: &str, x: i32, y: i32, c: Rgba<u8>) {
    aa_draw_text_at(img, s, x, y, ANNOT_FONT_PX, c);
}

/// A small filled disc, centered on `(cx, cy)` by its own metrics — reuses the
/// AA face's `●` rather than a bespoke circle rasterizer. `px` picks the size:
/// the window-chrome traffic lights run small (8px), the connector anchor runs
/// at the annotation size.
fn dot(img: &mut RgbaImage, cx: i32, cy: i32, px: f32, c: Rgba<u8>) {
    let w = aa_text_width_at("\u{25CF}", px);
    let h = pixtuoid::aa_text::line_height(px);
    aa_draw_text_at(img, "\u{25CF}", cx - w / 2, cy - h / 2, px, c);
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

// Mac-style traffic-light dots — the pinned mock's terminal chrome carries
// them so the left panel reads as a window, not a bare text box.
const DOT_RED: Rgba<u8> = Rgba([255, 95, 86, 255]);
const DOT_YELLOW: Rgba<u8> = Rgba([255, 189, 46, 255]);
const DOT_GREEN: Rgba<u8> = Rgba([39, 201, 63, 255]);
const CHROME_DOT_PX: f32 = 8.0; // traffic-light diameter (the ● glyph at this size)
const DOT_PITCH: i32 = CHROME_DOT_PX as i32 + 1; // dot advance + a 1px gap
const DOT_GAP_AFTER: i32 = 6; // clearance between the 3rd dot and the title

/// `is_panel` gates the traffic-light dots + the title size — only the left
/// "captured..." panel is a typed terminal window; the "pixtuoid" office chrome
/// renders the SAME AA face at the annotation size, without the dots.
fn chrome(img: &mut RgbaImage, x: u32, y: u32, w: u32, title: &str, is_panel: bool) {
    fill(img, x, y, w, HEADER_H, CHROME_BG);
    fill(img, x, y + HEADER_H - 1, w, 1, EDGE);
    if is_panel {
        let cy = (y + HEADER_H / 2) as i32;
        let mut cx = x as i32 + PAD as i32 + 4;
        for c in [DOT_RED, DOT_YELLOW, DOT_GREEN] {
            dot(img, cx, cy, CHROME_DOT_PX, c);
            cx += DOT_PITCH;
        }
        let title_x = cx + DOT_GAP_AFTER;
        aa_draw_text_at(img, title, title_x, (y + 8) as i32, PROOF_FONT_PX, INK);
    } else {
        text(img, title, (x + PAD) as i32, (y + 8) as i32, INK);
    }
}

fn panel_body(
    img: &mut RgbaImage,
    origin: (u32, u32),
    size: (u32, u32),
    script: &ProofScript,
    elapsed_ms: u64,
) {
    fill(img, origin.0, origin.1, size.0, size.1, PANEL_BG);
    let floor = aa_text_width_at("M", PROOF_FONT_PX);
    let max_w = (size.0 as i32 - 2 * PAD as i32).max(floor);
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
        let wrapped = wrap_text(&line.text, max_w, |s| aa_text_width_at(s, PROOF_FONT_PX));
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
            let advance = aa_draw_text_at(
                img,
                &visible,
                (origin.0 + PAD) as i32,
                y as i32,
                PROOF_FONT_PX,
                color,
            );
            if take < sub_len {
                let cx = origin.0 as i32 + PAD as i32 + advance;
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
            chrome(&mut img, 0, 0, PANEL_W, &panel_title, true);
            chrome(&mut img, PANEL_W, 0, ow, "pixtuoid", false);
            ((0, HEADER_H), (PANEL_W, oh), (PANEL_W, HEADER_H))
        }
        ProofLayout::Tall => {
            chrome(&mut img, 0, 0, ow, &panel_title, true);
            chrome(&mut img, 0, HEADER_H + TALL_PANEL_H, ow, "pixtuoid", false);
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
            // Sits above the desk, clear of the ceiling halo `paint_ceiling_halos`
            // (pixtuoid_scene::pixel_painter::ambient) burns over a lit monitor: a
            // 2-buffer-row band starting one row above the desk, i.e. up to 16px
            // above desk.1 in PNG space (buffer row -> 8px here, same halving the
            // desk_px conversion above uses). GLOW_CLEARANCE clears its top edge
            // with an 8px margin so the connector/dot never sits inside the glow.
            const GLOW_CLEARANCE: i32 = 24;
            let anchor_y = desk.1 - GLOW_CLEARANCE;
            match layout {
                ProofLayout::Wide => {
                    let text_w = aa_text_width_at(label, ANNOT_FONT_PX);
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
                    dot(&mut img, desk.0 - 6, anchor_y, ANNOT_FONT_PX, ANNOT);
                }
                ProofLayout::Tall => {
                    // no cross-panel connector line (the panel sits above, not
                    // beside) — just the callout well clear of the sprite's
                    // head/name-tag, plus a dot marking the desk itself.
                    let text_w = aa_text_width_at(label, ANNOT_FONT_PX);
                    let label_x = (desk.0 - text_w - 16).max(PAD as i32);
                    let label_y = anchor_y - 22;
                    text(&mut img, label, label_x, label_y + 1, Rgba([0, 0, 0, 255]));
                    text(&mut img, label, label_x, label_y, ANNOT);
                    dot(&mut img, desk.0 - 6, anchor_y, ANNOT_FONT_PX, ANNOT);
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
        let lw = aa_text_width_at(cline, CODA_FONT_PX);
        let x = ((w as i32 - lw) / 2).max(0);
        let y = (coda_y0 + CODA_PAD) as i32 + i as i32 * CODA_LINE_H as i32;
        aa_draw_text_at(&mut img, cline, x, y, CODA_FONT_PX, CODA_INK);
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
            occupied_waypoints: Default::default(),
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
        assert_eq!(s.capture_date, "2026-06-30");
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
            PanelLine {
                at_ms: 900 + ANNOTATION_MAX_HOLD_MS + 50_000,
                text: "d".into(),
                prompt: false,
                annotation: Some("z"),
            },
        ];
        assert_eq!(active_annotation(&lines, 100), Some(0));
        assert_eq!(active_annotation(&lines, 899), Some(0));
        assert_eq!(active_annotation(&lines, 900), Some(2));
        // the max-hold ceiling on a MIDDLE annotation ("y" — "z" is still to
        // come): right at the edge it's still lit, one ms past it goes quiet
        // — and does NOT fall back to the older annotation (0).
        assert_eq!(
            active_annotation(&lines, 900 + ANNOTATION_MAX_HOLD_MS),
            Some(2)
        );
        assert_eq!(
            active_annotation(&lines, 900 + ANNOTATION_MAX_HOLD_MS + 1),
            None
        );
        // "z" is the LAST annotated line — exempt from the cap, it holds
        // indefinitely (there's no future beat it could be over-holding for).
        let z_at = 900 + ANNOTATION_MAX_HOLD_MS + 50_000;
        assert_eq!(active_annotation(&lines, z_at), Some(3));
        assert_eq!(
            active_annotation(&lines, z_at + ANNOTATION_MAX_HOLD_MS + 1_000_000),
            Some(3)
        );
    }

    #[test]
    fn canvas_dims_are_even_and_stack_correctly() {
        let (ww, wh) = canvas_dims(&ProofLayout::Wide, 960, 832);
        assert_eq!((ww, wh), (1720, 902));
        let (tw, th) = canvas_dims(&ProofLayout::Tall, 960, 832);
        assert_eq!((tw, th), (960, 1334));
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
    fn coda_fits_one_line_at_both_reference_canvas_widths() {
        // The AA font's narrower per-char advance (vs font8x8) means the
        // caption now fits on one line at BOTH the wide (1720px) and tall
        // (960px, cols=120) reference canvases.
        for w in [1720, 960] {
            let lines = coda_lines(w);
            assert_eq!(lines.len(), 1, "canvas_w={w}");
            assert_eq!(lines[0], CODA_TEXT);
        }
    }

    #[test]
    fn coda_wraps_a_narrow_canvas_without_dropping_words() {
        let narrow = coda_lines(500);
        assert!(narrow.len() > 1, "500px must force a wrap");
        assert_eq!(
            narrow.join(" "),
            CODA_TEXT,
            "wrapping must not drop or reorder words"
        );
    }

    #[test]
    fn wrap_text_never_produces_an_empty_line_list() {
        let unit_width = |s: &str| s.chars().count() as i32;
        assert_eq!(wrap_text("", 100, unit_width), vec![String::new()]);
        assert_eq!(wrap_text("hi", 1, unit_width), vec!["hi".to_string()]); // single word, never split
    }
}
