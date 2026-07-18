//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! This renders the office to a raw pixel `RgbBuffer` via the shared scene seam
//! (`pixtuoid_scene::floor::render_floor`, #423) — NOT the half-block terminal emulation
//! `examples/snapshot/` saves (snapshot writes the ratatui `TestBackend` → a ▀-compressed
//! PNG via `save_backend_as_png`). A floating-only surface: no `draw_scene`, no `Terminal`,
//! no shared output with snapshot. `floating::window` renders at a DOWNSCALED buffer
//! (~window/SCALE) and nearest-neighbor upscales it, so the pixel-art office stays
//! chunky/legible instead of 8×12-px-tiny at 1:1. This module just paints the buffer at
//! whatever dims it's handed, owning one `pixtuoid_scene::floor::FloorSession` (the
//! per-frame caches + persistent office state — coffee cups, group chitchat — plus the
//! dual eviction) across frames so motion stays continuous.

use std::time::SystemTime;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::SceneState;

use pixtuoid_scene::floor::{FloorMeta, FloorSession, FrameInputs};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::theme::Theme;

/// Pack an `Rgb` into the softbuffer word format, `0x00RRGGBB` (XRGB) — the ONE
/// definition of the floating painter's surface pixel format. The office blit
/// (`window.rs`) and this label overlay write into the SAME surface, so they must
/// agree on channel order / shift widths; a lone edit to one would color-swap the
/// badges while the office renders correctly, with no compile error. (The test
/// oracle re-derives the packing independently ON PURPOSE, so a bug here can't
/// hide behind a shared helper — don't route it through this.)
pub(crate) fn pack_xrgb(c: Rgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

/// Owns everything needed to render the live office to a reusable `RgbBuffer`
/// across frames: a [`FloorSession`] — the scene-owned painter session (sim
/// stores + buffer + coffee + chitchat + the dual eviction, written once).
/// One per window — keeping it alive across frames is what keeps motion/pose
/// continuous (no walk-flash).
pub struct OfficeRenderer {
    session: FloorSession,
    /// Ambient-audio gateway + cue tracker (#633). Inert unless installed.
    /// Full TUI cue parity: stems + door chime + the appliance one-shots
    /// (the session's `occupied_waypoints` + this frame's layout feed the
    /// tracker's occupancy edges). The window always renders
    /// `FloorMeta::ground()`, so no cross-floor re-prime exists here —
    /// if floating ever gains floor nav, mirror the TUI's `audio_floor` guard
    /// or switching floors fires a chime volley for the new floor's agents.
    audio: crate::audio::AudioHandle,
    audio_cues: pixtuoid_scene::audio::AudioCueTracker,
}

impl OfficeRenderer {
    pub fn new() -> Self {
        Self {
            session: FloorSession::new(),
            audio: crate::audio::AudioHandle::disabled(),
            audio_cues: pixtuoid_scene::audio::AudioCueTracker::new(),
        }
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Render `scene`'s floor (per `floor_meta`) into the owned buffer at `buf_w`×`buf_h`
    /// PIXELS — the caller maps window px → cells → pixels (`buf_w = cols`,
    /// `buf_h = rows * 2`, the half-block 1:2 cell aspect; floating has no footer row to
    /// subtract, unlike `draw_scene`). Returns the rendered buffer (a borrow of the
    /// reused allocation). On a too-small / uncomputable layout it returns the buffer
    /// unchanged — never panics.
    #[allow(clippy::too_many_arguments)] // the render inputs are genuinely flat (scene/pack/theme/clock/size/floor)
    pub fn render(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        theme: &'static Theme,
        now: SystemTime,
        buf_w: u16,
        buf_h: u16,
        floor_meta: FloorMeta,
        floor_pet: Option<&pixtuoid_scene::pet::Pet>,
    ) -> &RgbBuffer {
        // active_pet stays None: click-to-pet needs window pointer hit-testing
        // (deferred); the WANDERING floor pet is wired. The rest is the session's.
        let layout = self.session.render(FrameInputs {
            scene,
            pack,
            theme,
            now,
            size: Size { w: buf_w, h: buf_h },
            floor_meta,
            active_pet: None,
            floor_pet,
            debug_walkable: false,
        });
        if self.audio.is_enabled() {
            // floor-scoped like the TUI: you hear the floor the window shows
            let counts = pixtuoid_scene::board::per_floor_counts(scene)[floor_meta
                .floor_idx
                .min(pixtuoid_core::state::MAX_FLOORS - 1)];
            let precipitation = pixtuoid_scene::pixel_painter::precipitation_level(now);
            let floor_ids = scene
                .agents
                .iter()
                .filter(|(_, slot)| slot.floor_idx == floor_meta.floor_idx)
                .map(|(id, _)| id);
            // The session's occupancy + THIS frame's layout give the tracker
            // the appliance edges — printer/vending cues, TUI parity (#633).
            let events = self.audio_cues.observe(
                floor_ids,
                self.session.occupied_waypoints(),
                |idx| {
                    layout
                        .as_deref()
                        .and_then(|l| l.waypoints.get(idx))
                        .map(|w| w.kind)
                },
                now,
            );
            self.audio.frame(pixtuoid_scene::audio::AudioFrame {
                stems: pixtuoid_scene::audio::stem_levels(&counts, precipitation),
                events,
                track: pixtuoid_scene::audio::select_track(
                    pixtuoid_scene::pixel_painter::is_day_at(now),
                    precipitation,
                ),
            });
        }
        self.session.buf()
    }

    /// Build the name-badge overlay for the LAST rendered frame (call right after `render`).
    /// Uses the SAME layout + per-floor route state the sprite pass used, so labels align 1:1
    /// with the painted characters. Floating has no agent-hover yet → `hovered = None`.
    pub fn labels(
        &mut self,
        scene: &SceneState,
        now: SystemTime,
    ) -> Vec<pixtuoid_scene::overlay::LabelElement> {
        self.session.overlay(scene, now, None)
    }

    /// The neon wall-board model for the current scene — one floor, so `floor =
    /// None`. Delegates to `FloorSession::board` (shared with the web painter).
    pub fn board(&self, scene: &SceneState, now: SystemTime) -> pixtuoid_scene::board::BoardModel {
        self.session.board(scene, now, None)
    }
}

impl Default for OfficeRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Integer upscale factor: render the office at `win_h / SCALE` so the buffer stays around
/// `OFFICE_TARGET_H` px tall, keeping pixel-art sprites chunky + legible (a native 1:1 blit
/// renders 8×12 sprites at 8×12 px — unreadably tiny). Min 1 (never downscale-and-blur).
/// Shared by `window::redraw` and the `floating_snapshot` example so their downscale —
/// and thus the label `anchor_px × scale` placement — can't drift.
pub fn office_scale(win_h: u32) -> u32 {
    const OFFICE_TARGET_H: u32 = 180;
    (win_h as f64 / OFFICE_TARGET_H as f64).round().max(1.0) as u32
}

/// The window→office-buffer projection for a `win_w`×`win_h` px window: the
/// integer `office_scale` plus the downscaled buffer dims (`window / scale`,
/// clamped non-zero, NO footer row). The ONE place this geometry lives — shared
/// by `window::redraw` (which needs `scale` for the upscale blit and the buffer
/// dims for `sync_floor_caps` + the render) and the boot seed
/// (`boot_capacities_for_window`) — so the desk capacity they derive can't drift
/// on an `office_scale`/clamp change.
pub(crate) fn window_buffer_geometry(win_w: u32, win_h: u32) -> (u32, u16, u16) {
    let scale = office_scale(win_h);
    let buf_w = (win_w / scale).clamp(1, u16::MAX as u32) as u16;
    let buf_h = (win_h / scale).clamp(1, u16::MAX as u32) as u16;
    (scale, buf_w, buf_h)
}

/// Per-floor boot desk-capacities for the FLOATING window. Uses the SAME
/// `window_buffer_geometry` the first redraw's `window::sync_floor_caps` does —
/// the office buffer is `window / office_scale` with NO footer row — so the boot
/// seed and the first redraw AGREE. The TUI's `runtime::boot_capacities_for`
/// instead subtracts a footer row AND ignores the window upscale, so reusing it
/// here OVER-seeds: in the sub-frame boot race before the first redraw, a
/// `SessionStart` could land at a `desk_index` the smaller real layout lacks
/// (immutable → invisible-but-alive until a resize). A floor whose layout rejects
/// the size falls back to `FALLBACK_DESKS`, matching the TUI boot helper.
pub(crate) fn boot_capacities_for_window(
    win_w: u32,
    win_h: u32,
) -> [usize; pixtuoid_core::state::MAX_FLOORS] {
    let (_scale, buf_w, buf_h) = window_buffer_geometry(win_w, win_h);
    std::array::from_fn(|i| {
        let seed = pixtuoid_scene::floor::floor_seed(i);
        let cap = pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed);
        if cap == 0 {
            crate::runtime::FALLBACK_DESKS
        } else {
            cap
        }
    })
}

/// The bundled character sprite width (px), from the ONE cross-crate authority
/// `scene::layout::CHARACTER_SPRITE_W`. Labels only center ±half a glyph, so the
/// default width (not a custom pack's real `frame.width`) is fine here — ±1px on
/// a non-8-wide pack is cosmetically irrelevant (same rationale as `character_anchor`).
const FLOATING_SPRITE_W: i32 = pixtuoid_scene::layout::CHARACTER_SPRITE_W as i32;

/// Name-badge AA font size (px), drawn at NATIVE surface res (not upscaled by the
/// office `scale`) so a badge stays a crisp fixed-height caption over the chunky
/// sprites — the same "fixed px, not upscaled" intent the old 8px bitmap had, now
/// anti-aliased. Tuned by eye against `examples/floating_snapshot`.
const LABEL_FONT_PX: f32 = 12.0;
/// Near-black badge drop-shadow (`0x00RRGGBB`) — the AA text draws straight over
/// the office (no TUI cell background), so a 1px offset shadow keeps it legible
/// over bright windows / plants.
const BADGE_SHADOW: u32 = 0x0000_0000;
/// The near-white AA ink for foreground captions with no theme cell behind them
/// — the hovered name badge AND the volume-flash readout share it (one
/// definition so a future softening can't split them).
const HOVER_INK: Rgb = Rgb {
    r: 240,
    g: 240,
    b: 240,
};

/// Alpha-composite `color` over the surface pixel at `(x, y)` by `coverage` (the
/// AA rasterizer's per-pixel strength), a straight linear blend in `0x00RRGGBB`
/// space — the badge/board sit on opaque office pixels, no alpha channel to keep.
fn blend_xrgb(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    x: i32,
    y: i32,
    color: u32,
    coverage: f32,
) {
    if x < 0 || y < 0 || (x as usize) >= win_w || (y as usize) >= win_h {
        return;
    }
    let idx = y as usize * win_w + x as usize;
    let bg = sb[idx];
    // the ONE blend curve — see aa_text::blend_channel
    let chan = |v: u32, sh: u32| ((v >> sh) & 0xff) as u8;
    let mix =
        |sh: u32| crate::aa_text::blend_channel(chan(bg, sh), chan(color, sh), coverage) as u32;
    sb[idx] = (mix(16) << 16) | (mix(8) << 8) | mix(0);
}

/// Rasterize `text` at `(x, top_y)` in the shared AA face, `color` over a 1px
/// down-right near-black shadow (shadow drawn first, both coverage-composited).
#[allow(clippy::too_many_arguments)] // flat surface + placement + style inputs, like paint_labels
fn draw_badge_text(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    text: &str,
    x: i32,
    top_y: i32,
    px: f32,
    color: u32,
) {
    crate::aa_text::draw_text_at(text, x + 1, top_y + 1, px, |gx, gy, cov| {
        blend_xrgb(sb, win_w, win_h, gx, gy, BADGE_SHADOW, cov)
    });
    crate::aa_text::draw_text_at(text, x, top_y, px, |gx, gy, cov| {
        blend_xrgb(sb, win_w, win_h, gx, gy, color, cov)
    });
}

/// Paint name badges into the upscaled `u32` surface (`0x00RRGGBB`). Each label's `anchor_px`
/// is office-buffer space → multiply by `scale` for screen space; the badge is centered
/// horizontally over the anchor and sits just above the head. Crisp anti-aliased Monaspace
/// Neon (drawn at native surface res, not upscaled) keeps it a sharp caption over the chunky
/// sprites. Shared by the live window (`window::redraw`) and the `floating_snapshot` verify
/// example, so both blit identically.
pub fn paint_labels_into_surface(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    labels: &[pixtuoid_scene::overlay::LabelElement],
    scale: i32,
    theme: &Theme,
) {
    for el in labels {
        let rgb = if el.hovered {
            HOVER_INK
        } else {
            // Tone→role map is single-sourced in `scene::overlay`.
            pixtuoid_scene::overlay::label_tone_rgb(el.tone, theme)
        };
        let color = pack_xrgb(rgb);
        // A ● state dot leads the badge (▸ when hovered — dead today: `labels()` passes
        // `hovered: None`, floating has no agent-hover). The AA face renders any glyph, so
        // ▸ needs no bitmap registration (unlike the old 8×8 font).
        let marker = if el.hovered { "\u{25b8}" } else { "\u{25cf}" };
        let text = format!("{marker}{}", el.text);
        let tw = crate::aa_text::text_width(&text, LABEL_FONT_PX);
        // anchor_px is the sprite TOP-LEFT in office space; center the badge over the sprite
        // and lift it a badge-height + gap above the head.
        const BADGE_LIFT_PX: i32 = 12;
        let cx = el.anchor_px.x as i32 * scale + (FLOATING_SPRITE_W * scale) / 2 - tw / 2;
        let cy = el.anchor_px.y as i32 * scale - BADGE_LIFT_PX;
        // The CLI-identity split (#657, owner-ratified, all three painters):
        // the ● dot keeps the activity tone (status), the name paints in the
        // source's badge hue (identity) via the SAME SourceColors::by_prefix
        // the dashboard badges ride. Unregistered prefix / hover → one run in
        // the tone/hover ink, unchanged.
        let badge = (!el.hovered)
            .then(|| el.text.split_once('\u{b7}'))
            .flatten()
            .and_then(|(prefix, _)| theme.source.by_prefix(prefix));
        match badge {
            Some(hue) => {
                let mw = crate::aa_text::text_width(marker, LABEL_FONT_PX);
                draw_badge_text(sb, win_w, win_h, marker, cx, cy, LABEL_FONT_PX, color);
                draw_badge_text(
                    sb,
                    win_w,
                    win_h,
                    &el.text,
                    cx + mw,
                    cy,
                    LABEL_FONT_PX,
                    pack_xrgb(hue),
                );
            }
            None => draw_badge_text(sb, win_w, win_h, &text, cx, cy, LABEL_FONT_PX, color),
        }
    }
}

/// Paint the neon wall-board text over the already-painted panel, into the upscaled
/// surface. The panel interior is `NEON_PANEL_INNER_*` in office-buffer px, so the
/// board text ANCHORS to it and SCALES with the office `scale` (unlike the fixed-height
/// name badges) — the three rows always fit inside the glowing frame. At a very small
/// office scale the rows would be sub-legible; there we leave the panel empty rather
/// than paint mush (the footer/TUI carry nothing critical the board owns). Shared by
/// the live window and the `floating_snapshot` verify example.
pub fn paint_wall_board_into_surface(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    board: &pixtuoid_scene::board::BoardModel,
    scale: i32,
    theme: &Theme,
) {
    use pixtuoid_scene::pixel_painter::{
        NEON_PANEL_INNER_H, NEON_PANEL_INNER_W, NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y,
    };
    if scale <= 0 {
        return;
    }
    let inner_x = NEON_PANEL_INNER_X as i32 * scale;
    let inner_y = NEON_PANEL_INNER_Y as i32 * scale;
    let inner_w = NEON_PANEL_INNER_W as i32 * scale;
    let row_h = NEON_PANEL_INNER_H as i32 * scale / 3;
    // Below this a row can't hold a legible glyph — leave the empty glowing panel.
    const MIN_ROW_PX: i32 = 4;
    if row_h < MIN_ROW_PX {
        return;
    }
    // Fill ~85% of the row so descenders don't collide with the next row.
    let font_px = row_h as f32 * 0.85;
    // Tone→role map is single-sourced in `scene::board`; the painter only packs
    // the resolved `Rgb` into the surface's XRGB.
    let glow = |tone| pack_xrgb(pixtuoid_scene::board::tone_rgb(tone, theme));

    // L1: brand left, ★ Star right-flushed to the interior's right edge.
    draw_badge_text(
        sb,
        win_w,
        win_h,
        &board.brand.text,
        inner_x,
        inner_y,
        font_px,
        glow(board.brand.tone),
    );
    let star_w = crate::aa_text::text_width(&board.star.text, font_px);
    let star_x = inner_x + (inner_w - star_w).max(0);
    draw_badge_text(
        sb,
        win_w,
        win_h,
        &board.star.text,
        star_x,
        inner_y,
        font_px,
        glow(board.star.tone),
    );

    // L2 (mood) + L3 (context): tone-mapped segments laid left-to-right on their row.
    for (row, segs) in [(1, &board.mood), (2, &board.context)] {
        let mut x = inner_x;
        let y = inner_y + row * row_h;
        for seg in segs {
            draw_badge_text(sb, win_w, win_h, &seg.text, x, y, font_px, glow(seg.tone));
            x += crate::aa_text::text_width(&seg.text, font_px);
        }
    }
}

/// The transient readout's TEXT (`♩ N%` unmuted / `♩ off` muted) — pure so
/// the muted/percent branch is unit-tested off the codecov-ignored redraw.
pub fn volume_flash_text(muted: bool, volume: f32) -> String {
    if muted {
        "\u{2669} off".to_string()
    } else {
        format!("\u{2669} {}%", (volume * 100.0).round() as u8)
    }
}

/// Paint the transient volume readout (`♩ 45%` / `♩ off`) into the window's
/// bottom-right corner — the floating twin of the TUI footer's flash and the
/// m/+/- keys' ONLY visual feedback (this window has no footer). Fixed
/// caption height like the name badges (crisp at any office scale), the
/// shared [`HOVER_INK`] near-white over the shared 1px shadow.
pub fn paint_volume_flash_into_surface(sb: &mut [u32], win_w: usize, win_h: usize, text: &str) {
    // breathing room from the window edges
    const FLASH_MARGIN_PX: i32 = 6;
    let color = pack_xrgb(HOVER_INK);
    let tw = crate::aa_text::text_width(text, LABEL_FONT_PX);
    let x = (win_w as i32 - tw - FLASH_MARGIN_PX).max(0);
    let y = (win_h as i32 - crate::aa_text::line_height(LABEL_FONT_PX) - FLASH_MARGIN_PX).max(0);
    draw_badge_text(sb, win_w, win_h, text, x, y, LABEL_FONT_PX, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_xrgb_is_0x00rrggbb() {
        // Pin the surface pixel format (channel order + shift widths) so the two
        // production packers (office blit + label overlay) can't re-drift. The
        // per-tone label test below independently cross-checks it via `as_u32`.
        assert_eq!(
            pack_xrgb(Rgb {
                r: 255,
                g: 128,
                b: 0
            }),
            0x00FF_8000
        );
        assert_eq!(pack_xrgb(Rgb { r: 0, g: 0, b: 0 }), 0x0000_0000);
        assert_eq!(pack_xrgb(Rgb { r: 1, g: 2, b: 3 }), 0x0001_0203);
    }

    #[test]
    fn renders_a_sized_nonblank_office_buffer() {
        // A fresh empty office still paints floor/walls/windows → never all-black, and the
        // buffer is sized to the requested pixel dims. Pins the floating render seam end-to-end.
        let scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();
        let buf = renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        assert_eq!((buf.width(), buf.height()), (160, 96));
        // Assert PAINTED content, not the pre-fill: `ensure_size` fills the buffer with
        // `bg_fallback` (non-black) BEFORE the painter runs, so "any non-black pixel" would
        // pass even if the painter no-op'd. Require a pixel that is neither black NOR
        // `bg_fallback` → the floor/walls/windows pass actually ran.
        let bg = theme.surface.bg_fallback;
        assert!(
            buf.as_slice()
                .iter()
                .any(|p| *p != Rgb { r: 0, g: 0, b: 0 } && *p != bg),
            "the painter draws office content beyond the cleared background"
        );
    }

    #[test]
    fn office_scale_keeps_the_office_chunky_and_never_zero() {
        // Downscale so the office buffer stays ~OFFICE_TARGET_H (180px) tall.
        assert_eq!(office_scale(180), 1);
        assert_eq!(office_scale(360), 2);
        assert_eq!(office_scale(720), 4);
        // A short window still renders at scale 1 — never 0 (redraw divides by it).
        assert_eq!(office_scale(90), 1);
        assert_eq!(office_scale(0), 1);
    }

    #[test]
    fn boot_capacities_for_window_match_the_first_redraw_geometry_not_the_tui_overseed() {
        // A 4x-upscaled window (720px tall → office_scale 4): the boot seed must
        // match what the first redraw's `sync_floor_caps` stores — `floor_capacity`
        // at the DOWNSCALED buffer (win/scale), no footer — not the full-window
        // over-seed the TUI helper produces.
        let (w, h) = (1280u32, 720u32);
        let scale = office_scale(h);
        let buf_w = (w / scale) as u16;
        let buf_h = (h / scale) as u16;
        let boot = boot_capacities_for_window(w, h);
        for (i, &got) in boot.iter().enumerate() {
            let cap = pixtuoid_scene::floor::floor_capacity(
                buf_w,
                buf_h,
                pixtuoid_scene::floor::floor_seed(i),
            );
            let want = if cap == 0 {
                crate::runtime::FALLBACK_DESKS
            } else {
                cap
            };
            assert_eq!(
                got, want,
                "floor {i} boot cap must match the rendered geometry"
            );
        }
        // The old TUI helper (footer subtraction + no office_scale) over-seeds the
        // ground floor — the bug this fix removes.
        let overseed = crate::runtime::boot_capacities_for(w as u16, (h / 2) as u16);
        assert!(
            overseed[0] >= boot[0],
            "TUI helper over-seeds ({} vs {})",
            overseed[0],
            boot[0]
        );
    }

    #[test]
    fn paint_labels_uses_the_right_color_per_tone_and_overrides_with_white_on_hover() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let as_u32 = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        let badge = |tone, hovered| {
            vec![LabelElement {
                anchor_px: Point { x: 20, y: 20 },
                text: "cc".into(),
                tone,
                hovered,
            }]
        };
        // Each tone must paint its OWN theme color — not merely "some pixel". The
        // leading ● disc reaches FULL AA coverage, so its exact tone color appears; a
        // wrong match arm (e.g. Idle returning the Active color) would fail this.
        let badge_dot = |tone, hovered| {
            vec![LabelElement {
                anchor_px: Point { x: 20, y: 20 },
                // A leading ● (the non-hover marker) guarantees a solid full-coverage glyph.
                text: "\u{25cf}cc".into(),
                tone,
                hovered,
            }]
        };
        for (tone, expected) in [
            (LabelTone::Active, theme.ui.label_active),
            (LabelTone::Waiting, theme.ui.label_waiting),
            (LabelTone::Idle, theme.ui.label_idle),
            (LabelTone::Exiting, theme.ui.label_exiting),
        ] {
            let mut sb = vec![0u32; 100 * 100];
            paint_labels_into_surface(&mut sb, 100, 100, &badge_dot(tone, false), 2, theme);
            assert!(
                sb.contains(&as_u32(expected)),
                "tone {tone:?} must paint its theme color {expected:?}"
            );
        }
        // Hover OVERRIDES the tone color with white. AA curve strokes don't reach
        // coverage EXACTLY 1.0 (the old 8×8 bitmap did), so assert via brightness:
        // painting the SAME glyphs, the white hover ink must be brighter than the
        // dim-grey Idle ink — which is only true if hover replaced the tone color.
        let brightness = |sb: &[u32]| {
            sb.iter()
                .map(|&p| (p & 0xff) + ((p >> 8) & 0xff) + ((p >> 16) & 0xff))
                .max()
                .unwrap_or(0)
        };
        let mut hover_sb = vec![0u32; 100 * 100];
        paint_labels_into_surface(
            &mut hover_sb,
            100,
            100,
            &badge(LabelTone::Idle, true),
            2,
            theme,
        );
        let mut idle_sb = vec![0u32; 100 * 100];
        paint_labels_into_surface(
            &mut idle_sb,
            100,
            100,
            &badge(LabelTone::Idle, false),
            2,
            theme,
        );
        assert!(
            brightness(&hover_sb) > brightness(&idle_sb),
            "hover paints brighter (white) ink than the idle grey tone it overrides"
        );
    }

    #[test]
    fn paint_labels_split_the_status_dot_tone_from_the_cli_name_hue() {
        // #657 owner-ratified split: the ● dot keeps the activity tone while the
        // NAME paints in the source's by_prefix badge hue. A registered prefix
        // (`cc·`) exercises the `Some(hue)` arm the tone-only tests above skip.
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let as_u32 = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        // Idle grey dot vs the cc badge hue — deliberately distinct colors, so
        // "both present" proves a genuine split, not one color bleeding into both.
        let tone_rgb = theme.ui.label_idle;
        let name_rgb = theme.source.claude_code;
        assert_ne!(tone_rgb, name_rgb, "premise: idle tone != cc badge hue");
        let label = vec![LabelElement {
            anchor_px: Point { x: 20, y: 20 },
            text: "cc\u{b7}api".into(),
            tone: LabelTone::Idle,
            hovered: false,
        }];
        let mut sb = vec![0u32; 120 * 120];
        paint_labels_into_surface(&mut sb, 120, 120, &label, 2, theme);
        assert!(
            sb.contains(&as_u32(tone_rgb)),
            "the ● dot must paint the activity tone {tone_rgb:?}"
        );
        assert!(
            sb.contains(&as_u32(name_rgb)),
            "the name must paint the cc badge hue {name_rgb:?}"
        );
    }

    #[test]
    fn paint_labels_render_antialiased_partial_coverage_not_binary_pixels() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        // Paint over a WHITE ground: an AA glyph's edges emit partial coverage, so
        // some pixels land STRICTLY between white and any fully-lit ink — the exact
        // thing the old all-or-nothing 8×8 bitmap font could never produce.
        let white = 0x00FF_FFFFu32;
        let mut sb = vec![white; 200 * 60];
        let badge = vec![LabelElement {
            anchor_px: Point { x: 20, y: 20 },
            text: "active".into(),
            tone: LabelTone::Active,
            hovered: false,
        }];
        paint_labels_into_surface(&mut sb, 200, 60, &badge, 2, theme);
        let ink = pack_xrgb(theme.ui.label_active);
        let shadow = 0x0000_0000u32;
        let intermediate = sb.iter().any(|&p| p != white && p != ink && p != shadow);
        assert!(
            intermediate,
            "AA text must blend edge pixels between the ground and the ink"
        );
        // And a fully-covered stroke interior still reaches the exact tone color.
        assert!(
            sb.contains(&ink),
            "glyph interior reaches full-coverage tone color"
        );
    }

    #[test]
    fn wall_board_paints_brand_and_mood_tones_into_the_panel() {
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        // 2 work + 1 wait + 1 idle, a busy gateway → the board carries the brand, a
        // ●work mood segment, and the ⬢gw chip. Rendered at a generous scale so the
        // full-coverage stroke interiors reach the exact tone colors.
        let counts = pixtuoid_scene::board::StateCounts {
            active: 2,
            waiting: 1,
            idle: 1,
            exiting: 0,
            total: 4,
        };
        let board = pixtuoid_scene::board::build_board(counts, 90, None, None);
        let scale = 8i32;
        let (w, h) = (320usize, 96usize);
        let mut sb = vec![0u32; w * h];
        paint_wall_board_into_surface(&mut sb, w, h, &board, scale, theme);
        assert!(
            sb.contains(&pack_xrgb(theme.ui.neon_brand)),
            "L1 brand paints the neon-brand hue"
        );
        assert!(
            sb.contains(&pack_xrgb(theme.ui.label_active)),
            "the ● work mood segment paints the active hue"
        );
        // Below the min row size the board leaves the panel empty (no mush).
        let mut tiny = vec![0u32; w * h];
        paint_wall_board_into_surface(&mut tiny, w, h, &board, 1, theme);
        assert!(
            tiny.iter().all(|&p| p == 0),
            "a scale-1 office suppresses the sub-legible board"
        );
    }

    /// Local twin of the TUI harness's `active_on` — `tui` and `floating` are
    /// sibling painters that don't share code, test helpers included.
    fn active_on(path: &str, floor_idx: usize, desk: usize) -> pixtuoid_core::state::AgentSlot {
        use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, ToolKind};
        use std::sync::Arc;
        let started = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        AgentSlot {
            agent_id: pixtuoid_core::AgentId::from_transcript_path(path),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(std::path::Path::new("/repo")),
            label: "a".into(),
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("Edit")),
                kind: ToolKind::from_display("Edit"),
            },
            state_started_at: started,
            created_at: started,
            last_event_at: started,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(desk),
            floor_idx,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        }
    }

    fn scene_with(agents: Vec<pixtuoid_core::state::AgentSlot>, cap: usize) -> SceneState {
        let mut s = SceneState::uniform(cap);
        for a in agents {
            s.agents.insert(a.agent_id, a);
        }
        s
    }

    #[test]
    fn floating_stems_count_only_the_rendered_floor() {
        // The floating twin of the TUI harness's floor-scoping pin: 1 active on
        // the rendered ground floor vs 3 on floor 1 must read MODERATE typing,
        // not the BUSY a global count would produce.
        let cap = 16;
        let scene = scene_with(
            vec![
                active_on("/a/f0.jsonl", 0, 0),
                active_on("/a/f1a.jsonl", 1, cap),
                active_on("/a/f1b.jsonl", 1, cap + 1),
                active_on("/a/f1c.jsonl", 1, cap + 2),
            ],
            cap,
        );
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        let frames = crate::audio::drain_frames(&rx);
        assert!(!frames.is_empty(), "an enabled handle receives frames");
        let stems = frames.last().unwrap().stems;
        let moderate = pixtuoid_scene::audio::stem_levels(
            &pixtuoid_scene::board::StateCounts {
                active: 1,
                waiting: 0,
                idle: 0,
                exiting: 0,
                total: 1,
            },
            0.0,
        );
        assert_eq!(
            stems.typing, moderate.typing,
            "typing level must reflect the RENDERED floor's 1 active, not all 4"
        );
    }

    #[test]
    fn volume_flash_text_reads_off_when_muted_else_the_rounded_percent() {
        assert_eq!(volume_flash_text(true, 0.42), "\u{2669} off");
        assert_eq!(volume_flash_text(false, 0.45), "\u{2669} 45%");
        assert_eq!(volume_flash_text(false, 0.0), "\u{2669} 0%");
        assert_eq!(volume_flash_text(false, 1.0), "\u{2669} 100%");
        // rounds, not truncates (0.455 → 46%, the footer-percent convention)
        assert_eq!(volume_flash_text(false, 0.455), "\u{2669} 46%");
    }

    #[test]
    fn volume_flash_blits_into_the_bottom_right_quadrant() {
        // The m/+/- keys' only feedback surface: the readout must actually
        // land pixels, and land them where the doc says (bottom-right, inside
        // the margins) — the phantom-feedback twin of the label blit tests.
        let (w, h) = (200usize, 120usize);
        let mut sb = vec![0u32; w * h];
        paint_volume_flash_into_surface(&mut sb, w, h, "\u{2669} 45%");
        let changed: Vec<usize> = sb
            .iter()
            .enumerate()
            .filter(|(_, p)| **p != 0)
            .map(|(i, _)| i)
            .collect();
        assert!(!changed.is_empty(), "the readout painted something");
        assert!(
            changed.iter().all(|&i| {
                let (x, y) = (i % w, i / w);
                x >= w / 2 && y >= h / 2
            }),
            "the readout stays in the bottom-right quadrant"
        );
    }

    #[test]
    fn floating_appliance_cues_fire_from_the_sessions_occupancy() {
        // TUI cue parity (#633 close-out): the tracker now receives the
        // session's occupied_waypoints + this frame's waypoint kinds, so a
        // wanderer standing at the printer / vending machine fires the
        // appliance one-shot in the floating window too. Deterministic —
        // fixed agent id + a hand-stepped clock; the loop bound mirrors the
        // scene crate's occupancy sim pin.
        use pixtuoid_scene::audio::OneShot;
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now0 = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut idle = active_on("/w/wanderer.jsonl", 0, 0);
        idle.state = pixtuoid_core::state::ActivityState::Idle;
        let scene = scene_with(vec![idle], 16);
        let mut renderer = OfficeRenderer::new();
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);
        let mut heard = Vec::new();
        for step in 0..900u64 {
            let now = now0 + std::time::Duration::from_secs(2 * step);
            // 192x160: tall enough that the corridor hosts BOTH appliances
            // (the vending/printer height gates in layout::compute).
            renderer.render(
                &scene,
                &pack,
                theme,
                now,
                192,
                160,
                FloorMeta::ground(),
                None,
            );
            heard.extend(
                crate::audio::drain_frames(&rx)
                    .into_iter()
                    .flat_map(|f| f.events),
            );
            if heard
                .iter()
                .any(|e| matches!(e, OneShot::PrinterWhir | OneShot::VendingDrop))
            {
                break;
            }
        }
        assert!(
            heard
                .iter()
                .any(|e| matches!(e, OneShot::PrinterWhir | OneShot::VendingDrop)),
            "a wander through the appliance strip must fire a printer/vending cue; heard: {heard:?}"
        );
    }

    #[test]
    fn floating_door_chime_fires_only_for_rendered_floor_arrivals() {
        let cap = 16;
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let mut now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);

        let mut agents = vec![active_on("/d/f0.jsonl", 0, 0)];
        let scene = scene_with(agents.clone(), cap);
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        crate::audio::drain_frames(&rx); // discard the priming frames

        // an arrival on ANOTHER floor: silent in the ground-floor window
        agents.push(active_on("/d/f1-new.jsonl", 1, cap));
        let scene = scene_with(agents.clone(), cap);
        now += std::time::Duration::from_millis(33);
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        let off_floor: Vec<_> = crate::audio::drain_frames(&rx)
            .into_iter()
            .flat_map(|f| f.events)
            .collect();
        assert!(
            off_floor.is_empty(),
            "a floor-1 walk-in must not chime the ground-floor window: {off_floor:?}"
        );

        // an arrival on the rendered floor chimes
        agents.push(active_on("/d/f0-new.jsonl", 0, 1));
        let scene = scene_with(agents, cap);
        now += std::time::Duration::from_millis(33);
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        let on_floor: Vec<_> = crate::audio::drain_frames(&rx)
            .into_iter()
            .flat_map(|f| f.events)
            .collect();
        assert!(
            on_floor.contains(&pixtuoid_scene::audio::OneShot::DoorChime),
            "a ground-floor walk-in must chime the floating window: {on_floor:?}"
        );
    }

    #[test]
    fn labels_is_empty_before_render_then_builds_a_positioned_badge_for_a_seeded_agent() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::{AgentId, Reducer, Transport};
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();

        // One real agent, seeded the production way: a SessionStart through the reducer
        // registers the slot and assigns it a desk on floor 0.
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let mut reducer = Reducer::new();
        reducer.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: AgentId::from_parts("claude-code", "offscreen-labels-test"),
                source: "claude-code".to_string(),
                session_id: "offscreen-labels-test".to_string(),
                cwd: std::path::PathBuf::from("/home/user/demo-project"),
                parent_id: None,
            },
            now,
            Transport::Jsonl,
        );

        // No frame rendered yet → no cached layout → the guard returns empty.
        assert!(renderer.labels(&scene, now).is_empty());
        // After a render, labels() builds the overlay off the cached layout → one badge for the
        // seeded agent, anchored inside the rendered 160×96 office buffer (proves the seam wires
        // render's geometry into build_overlay, not just that the line executed).
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        let labels = renderer.labels(&scene, now);
        assert_eq!(labels.len(), 1, "one seeded agent → one name badge");
        let anchor = labels[0].anchor_px;
        assert!(
            (0..160).contains(&(anchor.x as i32)) && (0..96).contains(&(anchor.y as i32)),
            "badge anchor {anchor:?} lands inside the rendered office buffer"
        );
    }
}
