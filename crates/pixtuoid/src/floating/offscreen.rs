//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! Paints the buffer at whatever dims it's handed, owning one
//! `pixtuoid_scene::floor::FloorSession` across frames so motion stays continuous.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::{SceneState, MAX_FLOORS};

use pixtuoid_scene::floor::{FloorMeta, FloorSession, FrameInputs};
use pixtuoid_scene::footer::{
    build_footer, footer_tone_rgb, footer_tool_tally, FooterInputs, FooterModel,
};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::theme::Theme;
use winit::dpi::PhysicalSize;

/// Pack an `Rgb` into the softbuffer word format, `0x00RRGGBB` (XRGB) — the ONE
/// definition of the floating surface pixel format; the office blit (`window.rs`)
/// and this label overlay write into the SAME surface, so a lone edit to one would
/// color-swap the badges with no compile error. The test oracle re-derives the
/// packing independently ON PURPOSE — don't route it through this.
pub(crate) fn pack_xrgb(c: Rgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

/// Renders the live office to a reusable `RgbBuffer`. One per window — keeping it
/// alive across frames is what keeps motion/pose continuous (no walk-flash).
pub struct OfficeRenderer {
    session: FloorSession,
    /// Ambient-audio gateway. Inert unless installed.
    audio: crate::audio::AudioHandle,
}

impl OfficeRenderer {
    pub fn new() -> Self {
        Self {
            session: FloorSession::new(),
            audio: crate::audio::AudioHandle::disabled(),
        }
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Render `scene`'s floor into the owned buffer at `buf_w`×`buf_h` PIXELS — the
    /// caller maps window px → cells → pixels (`buf_h = rows * 2`, the half-block 1:2
    /// cell aspect; floating has no footer row to subtract, unlike `draw_scene`). On a
    /// too-small / uncomputable layout it returns the buffer unchanged — never panics.
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
        // active_pet stays None: click-to-pet needs window pointer hit-testing (deferred).
        self.session.render(FrameInputs {
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
        // Compose EVERY frame, even muted, so the observer's cue edges stay warm —
        // re-enabling then fires no volley; only DELIVERY is gated.
        let audio_frame = self.session.audio_frame(scene, floor_meta.floor_idx, now);
        if self.audio.is_enabled() {
            self.audio.frame(audio_frame);
        }
        self.session.buf()
    }

    /// Build the name-badge overlay for the LAST rendered frame (call right after
    /// `render`). Floating has no agent-hover yet → `hovered = None`.
    pub fn labels(
        &mut self,
        scene: &SceneState,
        now: SystemTime,
    ) -> Vec<pixtuoid_scene::overlay::LabelElement> {
        self.session.overlay(scene, now, None)
    }

    /// The neon wall-board model for the current scene — one floor, so `floor = None`.
    pub fn board(&self, scene: &SceneState, now: SystemTime) -> pixtuoid_scene::board::BoardModel {
        self.session.board(scene, now, None)
    }

    /// The status-footer model for the current scene — single-floor, so `floor = None`
    /// (no breadcrumb). `budget` is the caller's column budget ([`footer_budget`] at the
    /// live width). Source-death is deferred (`source_warning: None`) — floating doesn't
    /// thread the `SourceDeath` health channel yet.
    pub fn footer(
        &self,
        scene: &SceneState,
        budget: u16,
        audio_audible: bool,
        volume_flash: Option<u8>,
    ) -> FooterModel {
        let per_floor = pixtuoid_scene::board::per_floor_counts(scene);
        let tools = footer_tool_tally(scene);
        let inputs = FooterInputs {
            counts: pixtuoid_scene::board::scene_stats(scene),
            per_floor: &per_floor,
            gateway: pixtuoid_scene::board::gateway_rollup(scene.daemons().map(|(_, _, p)| p)),
            floor: None,
            tools: &tools,
            audio_audible,
            volume_flash,
            source_warning: None,
            keys_stats: FOOTER_KEYS,
            keys_alert: FOOTER_KEYS,
        };
        build_footer(&inputs, budget)
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
pub fn office_scale(win_h: u32) -> u32 {
    const OFFICE_TARGET_H: u32 = 180;
    (win_h as f64 / OFFICE_TARGET_H as f64).round().max(1.0) as u32
}

/// The window→office-buffer projection for a PHYSICAL-px window: the integer
/// `office_scale` plus the downscaled buffer dims (`window / scale`, clamped
/// non-zero, NO footer row). The ONE place this geometry lives, so the desk capacity
/// derived from it can't drift on an `office_scale`/clamp change.
///
/// Takes winit's `PhysicalSize` rather than two bare `u32`s so the UNIT is carried by
/// the type: the `[floating]` config size is LOGICAL, and handing it here is a compile
/// error instead of a silent HiDPI over-seed (#803).
pub(crate) fn window_buffer_geometry(size: PhysicalSize<u32>) -> (u32, u16, u16) {
    let scale = office_scale(size.height);
    let buf_w = (size.width / scale).clamp(1, u16::MAX as u32) as u16;
    let buf_h = (size.height / scale).clamp(1, u16::MAX as u32) as u16;
    (scale, buf_w, buf_h)
}

/// Per-floor desk capacities for an office buffer of `buf_w`×`buf_h`. THE one
/// derivation: the boot seed and every redraw's [`sync_floor_caps`] both call it, so
/// their agreement is structural rather than two loops that happen to agree.
pub(crate) fn floor_caps_for_buffer(buf_w: u16, buf_h: u16) -> [usize; MAX_FLOORS] {
    std::array::from_fn(|i| {
        pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, pixtuoid_scene::floor::floor_seed(i))
    })
}

/// Per-floor boot desk-capacities for the FLOATING window, from the REAL
/// `window.inner_size()`. Do NOT reuse the TUI's `runtime::boot_capacities_for` — it
/// subtracts a footer row AND ignores the window upscale, so it OVER-seeds: in the
/// sub-frame boot race before the first redraw, a `SessionStart` could land at a
/// `desk_index` the smaller real layout lacks (invisible-but-alive until a resize).
///
/// There is deliberately NO `cap == 0 → FALLBACK_DESKS` clause: `sync_floor_caps`
/// `store`s the honest 0 for a window too small to lay out, and a fallback points the
/// WRONG way, admitting 16 agents onto desks that do not exist.
pub(crate) fn boot_capacities_for_window(size: PhysicalSize<u32>) -> [usize; MAX_FLOORS] {
    let (_scale, buf_w, buf_h) = window_buffer_geometry(size);
    floor_caps_for_buffer(buf_w, buf_h)
}

/// Publish [`floor_caps_for_buffer`]'s answer into the reducer's per-floor capacity
/// atomics, keeping admission in lockstep with the office actually rendered at
/// `buf_w`×`buf_h`. Returns whether it recomputed — `false` means `last` already held
/// this buffer size and the publish was skipped.
///
/// `store`, NOT the TUI's monotone `fetch_max`: the floating window's pixel size is
/// exact and authoritative on every redraw, so a shrink genuinely LOWERS capacity and
/// the reducer must stop admitting agents onto desks that no longer exist. Don't
/// "harmonize" the two — the direction is deliberate.
///
/// The resize DETECTION rides along with the publish because `floor_capacity` runs a
/// full layout compute per floor, so this must not run per frame. Both live here rather
/// than at the `window::redraw` call site because `window.rs` is excluded from BOTH
/// codecov and cargo-mutants, so a guard there is measured by nothing.
pub(crate) fn sync_floor_caps(
    last: &mut Option<(u16, u16)>,
    floor_caps: &[AtomicUsize; MAX_FLOORS],
    buf_w: u16,
    buf_h: u16,
) -> bool {
    if *last == Some((buf_w, buf_h)) {
        return false;
    }
    *last = Some((buf_w, buf_h));
    for (cap, capacity) in floor_caps.iter().zip(floor_caps_for_buffer(buf_w, buf_h)) {
        cap.store(capacity, Ordering::Relaxed);
    }
    true
}

/// The bundled character sprite width (px). Labels only center ±half a glyph, so the
/// default width (not a custom pack's real `frame.width`) is fine here — ±1px on a
/// non-8-wide pack is cosmetically irrelevant.
const FLOATING_SPRITE_W: i32 = pixtuoid_scene::layout::CHARACTER_SPRITE_W as i32;

/// Name-badge AA font size (px), drawn at NATIVE surface res (not upscaled by the office
/// `scale`) so a badge stays a crisp fixed-height caption over the chunky sprites. Tuned
/// by eye against `examples/floating_snapshot`.
const LABEL_FONT_PX: f32 = 12.0;
/// Near-black badge drop-shadow — the AA text draws straight over the office (no TUI
/// cell background), so a 1px offset shadow keeps it legible over bright windows/plants.
const BADGE_SHADOW: u32 = 0x0000_0000;
/// The near-white AA ink for foreground captions with no theme cell behind them —
/// shared by the hovered name badge and the volume-flash readout.
const HOVER_INK: Rgb = Rgb {
    r: 240,
    g: 240,
    b: 240,
};

/// The floating footer's keybind-hint tail — floating's REAL controls (no terminal
/// `[q]uit`/`[t]heme`/`[?]help` chrome). The ONE painter-specific input to the shared
/// footer model; everything else is TUI-identical.
const FOOTER_KEYS: &str = " [m]ute [+/-]vol ";
/// Breathing room from the window edges for the footer band — both the paint and the
/// [`footer_budget`] column math read it, so they can't drift.
const FOOTER_MARGIN_PX: i32 = 6;

/// Alpha-composite `color` over the surface pixel at `(x, y)` by `coverage` — a straight
/// linear blend in `0x00RRGGBB` space; the badge/board sit on opaque office pixels, so
/// there is no alpha channel to keep.
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
    let chan = |v: u32, sh: u32| ((v >> sh) & 0xff) as u8;
    let mix =
        |sh: u32| crate::aa_text::blend_channel(chan(bg, sh), chan(color, sh), coverage) as u32;
    sb[idx] = (mix(16) << 16) | (mix(8) << 8) | mix(0);
}

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

/// Paint name badges into the upscaled `u32` surface (`0x00RRGGBB`). Each label's
/// `anchor_px` is office-buffer space → multiply by `scale` for screen space; the badge
/// is centered horizontally over the anchor and sits just above the head. Drawn at
/// native surface res, not upscaled, so it stays a sharp caption over the chunky sprites.
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
            pixtuoid_scene::overlay::label_tone_rgb(el.tone, theme)
        };
        let color = pack_xrgb(rgb);
        // The hovered ▸ is dead today: `labels()` passes `hovered: None`.
        let marker = if el.hovered { "\u{25b8}" } else { "\u{25cf}" };
        let text = format!("{marker}{}", el.text);
        let tw = crate::aa_text::text_width(&text, LABEL_FONT_PX);
        // anchor_px is the sprite TOP-LEFT in office space.
        const BADGE_LIFT_PX: i32 = 12;
        let cx = el.anchor_px.x as i32 * scale + (FLOATING_SPRITE_W * scale) / 2 - tw / 2;
        let cy = el.anchor_px.y as i32 * scale - BADGE_LIFT_PX;
        // The CLI-identity split: the ● dot keeps the activity tone (status), the name
        // paints in the source's badge hue (identity). Unregistered prefix / hover →
        // one run in the tone/hover ink.
        let badge = (!el.hovered)
            .then(|| pixtuoid_scene::overlay::badge_hue(&el.text, theme))
            .flatten();
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
/// surface. The panel interior is `NEON_PANEL_INNER_*` in office-buffer px, so the board
/// text ANCHORS to it and SCALES with the office `scale` (unlike the fixed-height name
/// badges) — the three rows always fit inside the glowing frame. At a very small office
/// scale the rows would be sub-legible; there we leave the panel empty rather than mush.
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
    let glow = |tone| pack_xrgb(pixtuoid_scene::board::tone_rgb(tone, theme));

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

    for (row, segs) in [(1, &board.mood), (2, &board.context)] {
        let mut x = inner_x;
        let y = inner_y + row * row_h;
        for seg in segs {
            draw_badge_text(sb, win_w, win_h, &seg.text, x, y, font_px, glow(seg.tone));
            x += crate::aa_text::text_width(&seg.text, font_px);
        }
    }
}

/// Column budget for the floating footer at `win_w` px — how many monospace Monaspace
/// advances fit between the margins. Monaspace is fixed-advance, so a column budget maps
/// cleanly to pixels.
pub fn footer_budget(win_w: usize) -> u16 {
    let advance = crate::aa_text::text_width("M", LABEL_FONT_PX).max(1);
    (((win_w as i32 - 2 * FOOTER_MARGIN_PX).max(0)) / advance) as u16
}

/// Paint the shared status footer as a bottom-overlay band — the floating twin of the
/// TUI's status row, rendering the SAME [`build_footer`] model so the two can't drift.
/// An OVERLAY over the office's bottom rows: it never insets the buffer (that would
/// shift the desk-capacity lockstep). Fixed caption height like the name badges, so it
/// stays crisp at any office scale.
pub fn paint_footer_into_surface(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    model: &FooterModel,
    theme: &Theme,
) {
    let y = (win_h as i32 - crate::aa_text::line_height(LABEL_FONT_PX) - FOOTER_MARGIN_PX).max(0);
    let mut x = FOOTER_MARGIN_PX;
    for seg in &model.segments {
        let color = pack_xrgb(footer_tone_rgb(seg.tone, theme));
        draw_badge_text(sb, win_w, win_h, &seg.text, x, y, LABEL_FONT_PX, color);
        x += crate::aa_text::text_width(&seg.text, LABEL_FONT_PX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::LogicalSize;

    #[test]
    fn pack_xrgb_is_0x00rrggbb() {
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
        // `ensure_size` pre-fills with `bg_fallback` (non-black) BEFORE the painter runs,
        // so "any non-black pixel" would pass even if the painter no-op'd.
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
        assert_eq!(office_scale(180), 1);
        assert_eq!(office_scale(360), 2);
        assert_eq!(office_scale(720), 4);
        // Never 0 — redraw divides by it.
        assert_eq!(office_scale(90), 1);
        assert_eq!(office_scale(0), 1);
    }

    #[test]
    fn boot_capacities_for_window_match_the_first_redraw_geometry_not_the_tui_overseed() {
        let (w, h) = (1280u32, 720u32);
        let scale = office_scale(h);
        let buf_w = (w / scale) as u16;
        let buf_h = (h / scale) as u16;
        let boot = boot_capacities_for_window(PhysicalSize::new(w, h));
        for (i, &got) in boot.iter().enumerate() {
            let want = pixtuoid_scene::floor::floor_capacity(
                buf_w,
                buf_h,
                pixtuoid_scene::floor::floor_seed(i),
            );
            assert_eq!(
                got, want,
                "floor {i} boot cap must match the rendered geometry"
            );
        }
        let overseed = crate::runtime::boot_capacities_for(w as u16, (h / 2) as u16);
        assert!(
            overseed[0] >= boot[0],
            "TUI helper over-seeds ({} vs {})",
            overseed[0],
            boot[0]
        );
    }

    /// Crosses the logical/physical boundary using winit's own conversion, at scale
    /// factors where the two disagree — the test above feeds the SAME numbers to both
    /// sides, so it can never see a UNITS mismatch (#803).
    #[test]
    fn the_boot_seed_tracks_the_physical_window_not_the_logical_config() {
        let logical = LogicalSize::new(
            crate::config::FLOATING_DEFAULT_W as f64,
            crate::config::FLOATING_DEFAULT_H as f64,
        );
        // The logical size read as physical — the defect.
        let as_if_physical = boot_capacities_for_window(PhysicalSize::new(
            logical.width as u32,
            logical.height as u32,
        ));

        // MEASURED buffers for the default 360×240 logical window. `office_scale`
        // ROUNDS, so this is NOT monotone in sf — no logical-side seed is sound.
        let measured = [
            (1.00_f64, (360u32, 240u32), 80usize),
            (1.25, (225, 150), 30),
            (1.50, (270, 180), 42),
            (1.75, (315, 210), 56),
            (2.00, (240, 160), 30),
            (3.00, (270, 180), 42),
        ];
        for (sf, want_buf, want_floor0) in measured {
            let physical: PhysicalSize<u32> = logical.to_physical(sf);
            let (_scale, buf_w, buf_h) = window_buffer_geometry(physical);
            assert_eq!(
                (buf_w as u32, buf_h as u32),
                want_buf,
                "office buffer at {sf}× of {logical:?}"
            );
            assert_eq!(
                boot_capacities_for_window(physical)[0],
                want_floor0,
                "floor-0 seed at {sf}×"
            );
        }
        let at_2x = boot_capacities_for_window(logical.to_physical(2.0));
        assert!(
            at_2x[0] < as_if_physical[0],
            "logical-as-physical over-seeds at 2×: {} vs the real {}",
            as_if_physical[0],
            at_2x[0],
        );
    }

    #[test]
    fn an_unlayoutable_window_seeds_zero_not_a_fallback() {
        let tiny = PhysicalSize::new(64u32, 48u32);
        let (_scale, buf_w, buf_h) = window_buffer_geometry(tiny);
        assert_eq!(
            pixtuoid_scene::floor::floor_capacity(
                buf_w,
                buf_h,
                pixtuoid_scene::floor::floor_seed(0)
            ),
            0,
            "fixture must actually be unlayoutable, else this asserts nothing"
        );
        assert_eq!(
            boot_capacities_for_window(tiny)[0],
            0,
            "the seed must agree with what the redraw stores, not invent desks"
        );
    }

    #[test]
    fn a_shrink_lowers_the_published_capacity_it_is_store_not_fetch_max() {
        let caps: [AtomicUsize; MAX_FLOORS] = std::array::from_fn(|_| AtomicUsize::new(0));
        let (big, small) = ((360u16, 240u16), (240u16, 160u16));
        let want_big = floor_caps_for_buffer(big.0, big.1);
        let want_small = floor_caps_for_buffer(small.0, small.1);
        assert!(
            want_small[0] < want_big[0] && want_small[0] > 0,
            "fixture must shrink floor 0 to a smaller NON-zero capacity: {} → {}",
            want_big[0],
            want_small[0]
        );

        let mut last = None;
        sync_floor_caps(&mut last, &caps, big.0, big.1);
        for (floor, want) in want_big.iter().enumerate() {
            assert_eq!(
                caps[floor].load(Ordering::Relaxed),
                *want,
                "floor {floor} must publish the layout's own capacity at {big:?}"
            );
        }

        sync_floor_caps(&mut last, &caps, small.0, small.1);
        for (floor, want) in want_small.iter().enumerate() {
            assert_eq!(
                caps[floor].load(Ordering::Relaxed),
                *want,
                "floor {floor} must FALL to the smaller window's capacity — a `fetch_max` \
                 publish would strand it at the larger one"
            );
        }
    }

    /// One fixture per divergence class: 1280×720 and 64×48 catch a re-introduced
    /// `cap == 0 → FALLBACK_DESKS` fallback, and 853×480 (`office_scale` 3) is the one
    /// whose capacity moves under a few px of one-sided buffer drift — the other two
    /// absorb it.
    #[test]
    fn the_first_redraws_publish_agrees_with_the_boot_seed() {
        for window in [
            PhysicalSize::new(1280u32, 720u32),
            PhysicalSize::new(853, 480),
            PhysicalSize::new(64, 48),
        ] {
            let seed = boot_capacities_for_window(window);
            let caps: [AtomicUsize; MAX_FLOORS] = std::array::from_fn(|_| AtomicUsize::new(0));
            let (_scale, buf_w, buf_h) = window_buffer_geometry(window);
            sync_floor_caps(&mut None, &caps, buf_w, buf_h);
            let published: [usize; MAX_FLOORS] =
                std::array::from_fn(|i| caps[i].load(Ordering::Relaxed));
            assert_eq!(
                published, seed,
                "the first redraw's publish must store what {window:?} seeded"
            );
        }
    }

    #[test]
    fn the_resize_memo_publishes_on_a_change_and_skips_a_repeat() {
        let caps: [AtomicUsize; MAX_FLOORS] = std::array::from_fn(|_| AtomicUsize::new(0));
        let mut last = None;
        assert!(
            sync_floor_caps(&mut last, &caps, 360, 240),
            "the FIRST call has no previous size, so it must publish"
        );
        assert_eq!(
            last,
            Some((360, 240)),
            "the memo must record what it published"
        );
        assert!(
            !sync_floor_caps(&mut last, &caps, 360, 240),
            "an unchanged buffer size must skip the per-floor layout compute"
        );
        assert!(
            sync_floor_caps(&mut last, &caps, 240, 160),
            "a resize must republish"
        );
        caps[0].store(999, Ordering::Relaxed);
        assert!(!sync_floor_caps(&mut last, &caps, 240, 160));
        assert_eq!(
            caps[0].load(Ordering::Relaxed),
            999,
            "a skipped publish must not touch the atomics"
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
        // AA curve strokes don't reach coverage EXACTLY 1.0, so assert hover via
        // brightness rather than an exact ink color.
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
        // A registered prefix (`cc·`) exercises the `Some(hue)` arm the tone-only
        // tests above skip.
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let as_u32 = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
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
        // A WHITE ground: AA edges land STRICTLY between the ground and any fully-lit ink.
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
        assert!(
            sb.contains(&ink),
            "glyph interior reaches full-coverage tone color"
        );
    }

    #[test]
    fn wall_board_paints_brand_and_mood_tones_into_the_panel() {
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        // A generous scale, so full-coverage stroke interiors reach the exact tone colors.
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
        let mut tiny = vec![0u32; w * h];
        paint_wall_board_into_surface(&mut tiny, w, h, &board, 1, theme);
        assert!(
            tiny.iter().all(|&p| p == 0),
            "a scale-1 office suppresses the sub-legible board"
        );
    }

    /// Local twin of the TUI harness's `active_on` — `tui` and `floating` are sibling
    /// painters that don't share code, test helpers included.
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
    fn paint_footer_blits_into_the_bottom_band_and_tones_via_the_shared_authority() {
        use pixtuoid_scene::board::{per_floor_counts, scene_stats};
        use pixtuoid_scene::footer::{FooterTone, RungKind};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let slot = active_on("/p/a.jsonl", 0, 0);
        scene.agents.insert(slot.agent_id, slot);
        let per_floor = per_floor_counts(&scene);
        let tools = footer_tool_tally(&scene);
        let inputs = FooterInputs {
            counts: scene_stats(&scene),
            per_floor: &per_floor,
            gateway: None,
            floor: None,
            tools: &tools,
            audio_audible: true,
            volume_flash: None,
            source_warning: None,
            keys_stats: FOOTER_KEYS,
            keys_alert: FOOTER_KEYS,
        };
        let (w, h) = (400usize, 160usize);
        let model = build_footer(&inputs, footer_budget(w));
        let mut sb = vec![0u32; w * h];
        paint_footer_into_surface(&mut sb, w, h, &model, theme);
        let changed: Vec<usize> = sb
            .iter()
            .enumerate()
            .filter(|(_, p)| **p != 0)
            .map(|(i, _)| i)
            .collect();
        assert!(!changed.is_empty(), "the footer painted something");
        assert!(
            changed.iter().all(|&i| i / w >= h / 2),
            "the footer stays in the bottom band"
        );
        assert!(
            sb.contains(&pack_xrgb(footer_tone_rgb(
                FooterTone::Rung(RungKind::Active),
                theme
            ))),
            "the ●A rung paints the shared label_active hue"
        );
    }

    #[test]
    fn floating_appliance_cues_fire_from_the_sessions_occupancy() {
        // Deterministic: fixed agent id + a hand-stepped clock; the loop bound mirrors
        // the scene crate's occupancy sim pin.
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
        // A BUDGET, not part of the assertion: the wait rides random wander over live desk positions.
        for step in 0..9_000u64 {
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

        // Seeded the production way: a SessionStart through the reducer assigns the desk.
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
