//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! Paints the buffer at whatever dims it's handed, owning one
//! `pixtuoid_scene::floor::FloorSession` across frames so walks stay continuous.

use std::sync::atomic::{AtomicUsize, Ordering};

use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{MAX_FLOORS, SceneState};

use pixtuoid_scene::display::{Badge, TextRun};
use pixtuoid_scene::floor::{FloorInputs, FloorSession};
use pixtuoid_scene::footer::{
    FooterContext, FooterInputs, FooterModel, build_footer, footer_tone_rgb,
};
use pixtuoid_scene::look::{Look, RenderInputs};
use pixtuoid_scene::theme::Theme;
use winit::dpi::PhysicalSize;

/// Pack an `Rgb` into the softbuffer word format, `0x00RRGGBB` (XRGB) — the ONE
/// definition of the floating surface pixel format; the office blit (`window.rs`)
/// and this label overlay write into the SAME surface, so a lone edit to one would
/// color-swap the badges with no compile error. The test oracle re-derives the
/// packing independently ON PURPOSE — don't route it through this.
pub(crate) fn pack_xrgb(c: Rgb) -> u32 {
    u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
}

/// Renders the live office to a reusable `RgbBuffer`. One per window — keeping it
/// alive across frames is what keeps walks/poses continuous (no walk-flash).
#[derive(Debug)]
pub struct OfficeRenderer {
    session: FloorSession,
    /// Ambient-audio gateway. Inert unless installed.
    audio: crate::audio::AudioHandle,
}

impl OfficeRenderer {
    /// A renderer drawing with `pack`, which every frame's `world.pack` must be.
    pub fn new(pack: std::sync::Arc<pixtuoid_core::sprite::format::Pack>) -> Self {
        Self {
            session: FloorSession::new(pack),
            audio: crate::audio::AudioHandle::disabled(),
        }
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Render the floor into the owned buffer at `inputs.size` office-buffer pixels —
    /// the window downscaled by `window_buffer_geometry`, with no footer row
    /// subtracted. A too-small layout leaves the buffer filled with the theme's
    /// `bg_fallback`.
    pub fn render(&mut self, inputs: RenderInputs<'_>) -> Option<&RgbBuffer> {
        let FloorInputs {
            scene, floor, now, ..
        } = inputs.world;
        self.session.render(Look::Classic, inputs);
        // Composed even when disabled or muted: `AudioObserver::frame`'s contract.
        self.audio
            .frame(self.session.audio_frame(scene, floor, now));
        self.session.buf()
    }

    /// The badges of the LAST rendered frame (call right after `render`).
    pub fn badges(&self) -> &[Badge] {
        self.session.badges()
    }

    /// The board's lines and the floor indicator of the LAST rendered frame
    /// (call right after `render`).
    pub fn signs(&self) -> &[TextRun] {
        self.session.signs()
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
        let inputs = FooterInputs::new(
            scene,
            FooterContext::new(
                scene,
                None,
                audio_audible,
                volume_flash,
                None,
                FOOTER_KEYS,
                FOOTER_KEYS,
            ),
        );
        build_footer(&inputs, budget)
    }
}

/// Integer upscale factor keeping the office buffer near `OFFICE_TARGET_H` px tall, so
/// pixel-art sprites stay chunky and legible. Min 1: never downscale-and-blur.
pub(crate) fn office_scale(win_h: u32) -> u32 {
    const OFFICE_TARGET_H: u32 = 180;
    (f64::from(win_h) / f64::from(OFFICE_TARGET_H))
        .round()
        .max(1.0) as u32
}

/// The window→office-buffer projection for a PHYSICAL-px window: the integer
/// `office_scale` plus the downscaled buffer dims (`window / scale`, clamped
/// non-zero, NO footer row). The ONE place this geometry lives, so the desk capacity
/// derived from it can't drift on an `office_scale`/clamp change.
///
/// Takes winit's `PhysicalSize` rather than two bare `u32`s so the UNIT is carried by
/// the type: the `[floating]` config size is LOGICAL, and handing it here is a compile
/// error instead of a silent HiDPI over-seed (#803).
pub fn window_buffer_geometry(size: PhysicalSize<u32>) -> (u32, u16, u16) {
    let scale = office_scale(size.height);
    let buf_w = (size.width / scale).clamp(1, u32::from(u16::MAX)) as u16;
    let buf_h = (size.height / scale).clamp(1, u32::from(u16::MAX)) as u16;
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
/// WRONG way, admitting `FALLBACK_DESKS` agents onto desks that do not exist.
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
/// "harmonize" the two — the direction is deliberate, and pinned by
/// `a_shrink_lowers_the_published_capacity_it_is_store_not_fetch_max`.
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

/// Name-badge AA font size (px), drawn at NATIVE surface res (not upscaled by the office
/// `scale`) so a badge stays a crisp fixed-height caption over the chunky sprites. Tuned
/// by eye against `examples/floating_snapshot`.
const LABEL_FONT_PX: f32 = 12.0;
/// Badge drop-shadow — the AA text draws straight over the office (no TUI
/// cell background), so a 1px offset shadow keeps it legible over bright windows/plants.
const BADGE_SHADOW: u32 = 0x0000_0000;

/// The floating footer's keybind-hint tail — floating's REAL controls (no terminal
/// `[q]uit`/`[t]heme`/`[?]help` chrome). The ONE painter-specific input to the shared
/// footer model; everything else is TUI-identical.
const FOOTER_KEYS: &str = " [m]ute [+/-]vol ";
/// Breathing room from the window edges for the footer band — both the paint and the
/// [`footer_budget`] column math read it, so they can't drift.
const FOOTER_MARGIN_PX: i32 = 6;

/// The window's row-major `0x00RRGGBB` pixel surface, `w`×`h`, that the text
/// overlays composite into.
#[derive(Debug)]
pub struct XrgbSurface<'a> {
    px: &'a mut [u32],
    w: usize,
    h: usize,
}

impl<'a> XrgbSurface<'a> {
    /// Wrap `px` as a `w`×`h` surface; `None` when it holds fewer than `w * h`
    /// pixels (a transient resize race on the live window).
    pub fn new(px: &'a mut [u32], w: usize, h: usize) -> Option<Self> {
        (px.len() >= w * h).then_some(Self { px, w, h })
    }

    /// Fill the whole surface with `office` upscaled by `scale`, nearest-neighbour.
    /// Source indices clamp, so the integer-division remainder at the right and
    /// bottom edges repeats the last office pixel. An empty `office` leaves the
    /// surface as it was.
    pub fn fill_upscaled(&mut self, office: &RgbBuffer, scale: usize) {
        let (ow, oh) = (usize::from(office.width()), usize::from(office.height()));
        if ow == 0 || oh == 0 {
            return;
        }
        let scale = scale.max(1);
        let src = office.as_slice();
        for wy in 0..self.h {
            let src_row = (wy / scale).min(oh - 1) * ow;
            let dst_row = wy * self.w;
            for wx in 0..self.w {
                self.px[dst_row + wx] = pack_xrgb(src[src_row + (wx / scale).min(ow - 1)]);
            }
        }
    }

    /// Alpha-composite `color` over the pixel at `(x, y)` by `coverage` — a straight
    /// linear blend in `0x00RRGGBB` space; the badge/board sit on opaque office
    /// pixels, so there is no alpha channel to keep. Off-surface is a no-op.
    fn blend(&mut self, x: i32, y: i32, color: u32, coverage: f32) {
        if x < 0 || y < 0 || (x as usize) >= self.w || (y as usize) >= self.h {
            return;
        }
        let idx = y as usize * self.w + x as usize;
        let bg = self.px[idx];
        let chan = |v: u32, sh: u32| ((v >> sh) & 0xff) as u8;
        let mix = |sh: u32| crate::aa_text::blend_channel(chan(bg, sh), chan(color, sh), coverage);
        self.px[idx] = pack_xrgb(Rgb {
            r: mix(16),
            g: mix(8),
            b: mix(0),
        });
    }

    /// `text` at `(x, top_y)` in `color`, over a one-pixel drop shadow.
    fn draw_shadowed_text(&mut self, text: &str, x: i32, top_y: i32, font_px: f32, color: u32) {
        crate::aa_text::draw_text_at(text, x + 1, top_y + 1, font_px, |gx, gy, cov| {
            self.blend(gx, gy, BADGE_SHADOW, cov)
        });
        crate::aa_text::draw_text_at(text, x, top_y, font_px, |gx, gy, cov| {
            self.blend(gx, gy, color, cov)
        });
    }
}

/// Paint `badges` into the upscaled [`XrgbSurface`]. Each badge's `at` is
/// office-buffer space → multiply by `scale` for screen space; the badge is
/// centered horizontally over it and sits just above the head.
pub fn paint_labels_into_surface(sb: &mut XrgbSurface<'_>, badges: &[Badge], scale: i32) {
    let marker = pixtuoid_scene::overlay::BADGE_MARKER.to_string();
    let mw = crate::aa_text::text_width(&marker, LABEL_FONT_PX);
    for Badge {
        at,
        marker: ink,
        name,
        ..
    } in badges
    {
        let tw = mw + crate::aa_text::text_width(&name.text, LABEL_FONT_PX);
        const BADGE_LIFT_PX: i32 = 12;
        let cx = i32::from(at.x) * scale - tw / 2;
        let cy = i32::from(at.y) * scale - BADGE_LIFT_PX;
        sb.draw_shadowed_text(&marker, cx, cy, LABEL_FONT_PX, pack_xrgb(*ink));
        sb.draw_shadowed_text(&name.text, cx + mw, cy, LABEL_FONT_PX, pack_xrgb(name.ink));
    }
}

/// Paint the neon wall-board text over the already-painted panel, into the upscaled
/// surface. The panel interior is `NEON_PANEL_INNER_*` in office-buffer px, so the board
/// text ANCHORS to it and SCALES with the office `scale` (unlike the fixed-height name
/// badges) — the three rows always fit inside the glowing frame. At a very small office
/// scale the rows would be sub-legible; there we leave the panel empty rather than mush.
pub fn paint_wall_board_into_surface(sb: &mut XrgbSurface<'_>, runs: &[TextRun], scale: i32) {
    use pixtuoid_scene::display::{Align, TextRole};
    use pixtuoid_scene::layout::NEON_PANEL_INNER_H;
    if scale <= 0 {
        return;
    }
    let row_h = i32::from(NEON_PANEL_INNER_H) * scale / 3;
    // Below this a row can't hold a legible glyph — leave the empty glowing panel.
    const MIN_ROW_PX: i32 = 4;
    if row_h < MIN_ROW_PX {
        return;
    }
    // Fill ~85% of the row so descenders don't collide with the next row.
    let font_px = row_h as f32 * 0.85;
    let board = runs
        .iter()
        .filter(|run| matches!(run.role, TextRole::Brand | TextRole::Star | TextRole::Board));
    for run in board {
        let width: i32 = run
            .spans
            .iter()
            .map(|s| crate::aa_text::text_width(&s.text, font_px))
            .sum();
        let at = i32::from(run.at.x) * scale;
        let mut x = match run.align {
            Align::Right => (at - width).max(0),
            Align::Over | Align::Centre => at - width / 2,
            Align::Left => at,
        };
        let y = i32::from(run.at.y) * scale;
        for span in &run.spans {
            sb.draw_shadowed_text(&span.text, x, y, font_px, pack_xrgb(span.ink));
            x += crate::aa_text::text_width(&span.text, font_px);
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
pub fn paint_footer_into_surface(sb: &mut XrgbSurface<'_>, model: &FooterModel, theme: &Theme) {
    let y = (sb.h as i32 - crate::aa_text::line_height(LABEL_FONT_PX) - FOOTER_MARGIN_PX).max(0);
    let mut x = FOOTER_MARGIN_PX;
    for seg in &model.segments {
        let color = pack_xrgb(footer_tone_rgb(seg.tone, theme));
        sb.draw_shadowed_text(&seg.text, x, y, LABEL_FONT_PX, color);
        x += crate::aa_text::text_width(&seg.text, LABEL_FONT_PX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_scene::floor::{FloorMeta, PetInputs};

    /// A badge reading `name` in `tone` under `theme`, hung from `at`.
    fn badge(
        at: pixtuoid_scene::layout::Point,
        name: &str,
        tone: pixtuoid_scene::overlay::LabelTone,
        theme: &Theme,
    ) -> Badge {
        let ink = pixtuoid_scene::overlay::badge_ink(name, tone, theme);
        Badge {
            agent: pixtuoid_core::AgentId::from_transcript_path("/badge/0.jsonl"),
            at,
            marker: ink.marker,
            name: pixtuoid_scene::display::TextSpan {
                text: name.into(),
                ink: ink.name,
            },
            plate: theme.ui.tooltip_bg,
        }
    }
    use pixtuoid_scene::layout::Size;
    use winit::dpi::LogicalSize;

    #[test]
    fn fill_upscaled_repeats_the_last_office_pixel_into_the_remainder_edge() {
        let px = |v: u8| Rgb { r: v, g: v, b: v };
        let mut office = RgbBuffer::filled(2, 2, px(0));
        for (x, y, v) in [(0, 0, 10), (1, 0, 20), (0, 1, 30), (1, 1, 40)] {
            office.put(x, y, px(v));
        }
        // 5 = 2 office px × scale 2, plus a 1-px remainder at the right/bottom edge.
        let mut sb = vec![0u32; 5 * 5];
        XrgbSurface::new(&mut sb, 5, 5)
            .expect("sized")
            .fill_upscaled(&office, 2);
        let at = |x: usize, y: usize| sb[y * 5 + x];
        assert_eq!(at(0, 0), pack_xrgb(px(10)));
        assert_eq!(at(3, 0), pack_xrgb(px(20)));
        assert_eq!(
            at(4, 0),
            pack_xrgb(px(20)),
            "right remainder repeats the last column"
        );
        assert_eq!(
            at(0, 4),
            pack_xrgb(px(30)),
            "bottom remainder repeats the last row"
        );
        assert_eq!(at(4, 4), pack_xrgb(px(40)));
    }

    #[test]
    fn a_surface_shorter_than_its_extent_is_refused() {
        let mut sb = vec![0u32; 3];
        assert!(XrgbSurface::new(&mut sb, 2, 2).is_none());
    }

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
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        let buf = renderer
            .render(RenderInputs {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                size: Size { w: 160, h: 96 },
                place: pixtuoid_scene::look::Place::default(),
                debug_walkable: false,
            })
            .expect("a frame");
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
        let overseed = crate::runtime::boot_capacities_for(
            crate::graphics::Plan::Classic {
                reason: crate::graphics::ClassicReason::Disabled,
            },
            ratatui::layout::Size::new(w as u16, (h / 2) as u16),
        );
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
            f64::from(crate::config::FLOATING_DEFAULT_W),
            f64::from(crate::config::FLOATING_DEFAULT_H),
        );
        // The logical size read as physical — the defect.
        let as_if_physical = boot_capacities_for_window(PhysicalSize::new(
            logical.width as u32,
            logical.height as u32,
        ));

        // MEASURED buffers for the default 360×240 logical window. `office_scale`
        // ROUNDS, so this is NOT monotone in sf — no logical-side seed is sound.
        // The sf-1.0 row is also what pins `render_frame.rs`'s `FLOATING_DEFAULT`
        // copy of these constants: a change to either reds this row first.
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
                (u32::from(buf_w), u32::from(buf_h)),
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
        // DERIVED, not a pinned 64x48: the floor dropped below it and the premise
        // assert below went red. Derive so the next move can't reach it.
        let min = pixtuoid_scene::layout::min_layout_size();
        let tiny = PhysicalSize::new(u32::from(min.w), u32::from(min.h - 1));
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

    /// One fixture per divergence class: the derived sub-floor window is the only one that
    /// can catch a re-introduced `cap == 0 → FALLBACK_DESKS` fallback (the other two have
    /// capacity on every floor), and 853×480 (`office_scale` 3) is the one
    /// whose capacity moves under a few px of one-sided buffer drift — the other two
    /// absorb it.
    #[test]
    fn the_first_redraws_publish_agrees_with_the_boot_seed() {
        // DERIVED as in the sibling above, which reds by name if this stops seeding zero.
        let min = pixtuoid_scene::layout::min_layout_size();
        let unlayoutable = PhysicalSize::new(u32::from(min.w), u32::from(min.h - 1));
        for window in [
            PhysicalSize::new(1280u32, 720u32),
            PhysicalSize::new(853, 480),
            unlayoutable,
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
    fn paint_labels_uses_the_right_color_per_tone() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::LabelTone;
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let as_u32 = |c: Rgb| u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b);
        let badge_dot = |tone| {
            // A leading ● guarantees a solid full-coverage glyph.
            vec![badge(Point { x: 20, y: 20 }, "\u{25cf}cc", tone, theme)]
        };
        for (tone, expected) in [
            (LabelTone::Active, theme.ui.label_active),
            (LabelTone::Waiting, theme.ui.label_waiting),
            (LabelTone::Idle, theme.ui.label_idle),
            (LabelTone::Exiting, theme.ui.label_exiting),
        ] {
            let mut sb = vec![0u32; 100 * 100];
            paint_labels_into_surface(
                &mut XrgbSurface::new(&mut sb, 100, 100).expect("sized"),
                &badge_dot(tone),
                2,
            );
            assert!(
                sb.contains(&as_u32(expected)),
                "tone {tone:?} must paint its theme color {expected:?}"
            );
        }
    }

    /// The badge's ink centres on the anchor scaled to the surface: the run's `at`
    /// is already the sprite's top-centre, so any extra offset walks it off the sprite.
    #[test]
    fn a_badge_centres_its_ink_on_the_scaled_anchor() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::LabelTone;
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let (w, h, scale) = (240usize, 60usize, 3i32);
        let ground = 0x0080_8080u32;
        let mut sb = vec![ground; w * h];
        let anchor = Point { x: 40, y: 15 };
        paint_labels_into_surface(
            &mut XrgbSurface::new(&mut sb, w, h).expect("sized"),
            &[badge(anchor, "idle-x", LabelTone::Idle, theme)],
            scale,
        );
        let cols: Vec<i32> = (0..w)
            .filter(|&x| (0..h).any(|y| sb[y * w + x] != ground))
            .map(|x| x as i32)
            .collect();
        let (Some(&left), Some(&right)) = (cols.first(), cols.last()) else {
            panic!("the badge painted nothing");
        };
        let centre = i32::from(anchor.x) * scale;
        // Glyph side bearings and the 1-px drop shadow, not an offset.
        const ROUNDING_PX: i32 = 2;
        assert!(
            (i32::midpoint(left, right) - centre).abs() <= ROUNDING_PX,
            "ink spans {left}..={right}, centred off the anchor's {centre}"
        );
    }

    #[test]
    fn paint_labels_ink_the_marker_and_the_name_as_the_model_says() {
        // A registered prefix (`cc·`), so the marker's ink differs from the name's.
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{BADGE_MARKER, LabelTone, badge_ink};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let text = "cc\u{b7}api";
        let ink = badge_ink(text, LabelTone::Idle, theme);
        assert_ne!(ink.marker, ink.name, "premise: the two parts differ");
        let (w, h, scale, anchor) = (120usize, 120usize, 2, Point { x: 20, y: 20 });
        let mut sb = vec![0u32; w * h];
        paint_labels_into_surface(
            &mut XrgbSurface::new(&mut sb, w, h).expect("sized"),
            &[badge(anchor, text, LabelTone::Idle, theme)],
            scale,
        );
        // The marker's columns, then the name's, as `paint_labels_into_surface`
        // lays them.
        let marker = BADGE_MARKER.to_string();
        let tw = crate::aa_text::text_width(&format!("{marker}{text}"), LABEL_FONT_PX);
        let mw = crate::aa_text::text_width(&marker, LABEL_FONT_PX);
        let left = i32::from(anchor.x) * scale - tw / 2;
        let colours = |cols: std::ops::Range<i32>| -> std::collections::HashSet<u32> {
            sb.iter()
                .enumerate()
                .filter(|(i, _)| cols.contains(&((i % w) as i32)))
                .map(|(_, &p)| p)
                .collect()
        };
        let (dot, name) = (colours(left..left + mw), colours(left + mw..left + tw));
        let (m, n) = (pack_xrgb(ink.marker), pack_xrgb(ink.name));
        assert!(
            dot.contains(&m) && !dot.contains(&n),
            "the ● takes the marker ink"
        );
        assert!(
            name.contains(&n) && !name.contains(&m),
            "the name takes the name ink"
        );
    }

    #[test]
    fn paint_labels_render_antialiased_partial_coverage_not_binary_pixels() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::LabelTone;
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        // A WHITE ground: AA edges land STRICTLY between the ground and any fully-lit ink.
        let white = 0x00FF_FFFFu32;
        let mut sb = vec![white; 200 * 60];
        let active = vec![badge(
            Point { x: 20, y: 20 },
            "active",
            LabelTone::Active,
            theme,
        )];
        paint_labels_into_surface(
            &mut XrgbSurface::new(&mut sb, 200, 60).expect("sized"),
            &active,
            2,
        );
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
        let board = pixtuoid_scene::board::build_board(
            counts,
            90,
            None,
            None,
            pixtuoid_scene::anim::Motion::Full,
            std::time::SystemTime::UNIX_EPOCH,
        );
        let runs = board.runs(theme);
        let scale = 8i32;
        let (w, h) = (320usize, 96usize);
        let mut sb = vec![0u32; w * h];
        paint_wall_board_into_surface(
            &mut XrgbSurface::new(&mut sb, w, h).expect("sized"),
            &runs,
            scale,
        );
        assert!(
            sb.contains(&pack_xrgb(theme.ui.neon_brand)),
            "L1 brand paints the neon-brand hue"
        );
        assert!(
            sb.contains(&pack_xrgb(theme.ui.label_active)),
            "the ● work mood segment paints the active hue"
        );
        let mut tiny = vec![0u32; w * h];
        paint_wall_board_into_surface(
            &mut XrgbSurface::new(&mut tiny, w, h).expect("sized"),
            &runs,
            1,
        );
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
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);
        renderer.render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w: 160, h: 96 },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        });
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
        use pixtuoid_scene::footer::{FooterTone, RungKind};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let slot = active_on("/p/a.jsonl", 0, 0);
        scene.agents.insert(slot.agent_id, slot);
        let inputs = FooterInputs::new(
            &scene,
            FooterContext::new(&scene, None, true, None, None, FOOTER_KEYS, FOOTER_KEYS),
        );
        let (w, h) = (400usize, 160usize);
        let model = build_footer(&inputs, footer_budget(w));
        let mut sb = vec![0u32; w * h];
        paint_footer_into_surface(
            &mut XrgbSurface::new(&mut sb, w, h).expect("sized"),
            &model,
            theme,
        );
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
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now0 = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut idle = active_on("/w/wanderer.jsonl", 0, 0);
        idle.state = pixtuoid_core::state::ActivityState::Idle;
        let scene = scene_with(vec![idle], 16);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);
        let mut heard = Vec::new();
        // A BUDGET, not part of the assertion: the wait rides random wander over live desk positions.
        for step in 0..9_000u64 {
            let now = now0 + std::time::Duration::from_secs(2 * step);
            // 192x160: tall enough that the corridor hosts BOTH appliances
            // (the vending/printer height gates in layout::compute).
            renderer.render(RenderInputs {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                size: Size { w: 192, h: 160 },
                place: pixtuoid_scene::look::Place::default(),
                debug_walkable: false,
            });
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
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let mut now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        let (handle, rx) = crate::audio::AudioHandle::test_pair();
        renderer.set_audio(handle);

        let mut agents = vec![active_on("/d/f0.jsonl", 0, 0)];
        let scene = scene_with(agents.clone(), cap);
        renderer.render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w: 160, h: 96 },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        });
        crate::audio::drain_frames(&rx); // discard the priming frames

        agents.push(active_on("/d/f1-new.jsonl", 1, cap));
        let scene = scene_with(agents.clone(), cap);
        now += std::time::Duration::from_millis(pixtuoid_scene::anim::PAINT_FRAME_MS);
        renderer.render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w: 160, h: 96 },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        });
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
        now += std::time::Duration::from_millis(pixtuoid_scene::anim::PAINT_FRAME_MS);
        renderer.render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w: 160, h: 96 },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        });
        let on_floor: Vec<_> = crate::audio::drain_frames(&rx)
            .into_iter()
            .flat_map(|f| f.events)
            .collect();
        assert!(
            on_floor.contains(&pixtuoid_scene::audio::OneShot::DoorChime),
            "a ground-floor walk-in must chime the floating window: {on_floor:?}"
        );
    }

    /// Every badge the frame drew paints into the surface: its marker and its
    /// name, each in its own ink, inside the box it is centred in.
    #[test]
    fn every_badge_the_frame_drew_paints_into_the_surface() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::{AgentId, Reducer, Transport};
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));

        // Seeded the production way: a SessionStart through the reducer assigns the desk.
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let mut reducer = Reducer::new();
        for (session, cwd) in [
            ("offscreen-a", "/home/user/demo"),
            ("offscreen-b", "/srv/api"),
        ] {
            reducer.apply(
                &mut scene,
                AgentEvent::SessionStart {
                    agent_id: AgentId::from_parts("claude-code", session),
                    source: "claude-code".to_string(),
                    session_id: session.to_string(),
                    cwd: std::path::PathBuf::from(cwd),
                    parent_id: None,
                },
                now,
                Transport::Jsonl,
            );
        }

        // No frame rendered yet → no drawn sprites → no badges.
        assert!(renderer.badges().is_empty());
        let (w, h, scale) = (160u16, 96u16, 3i32);
        renderer.render(RenderInputs {
            world: FloorInputs {
                scene: &scene,
                pack: &pack,
                now,
                floor: FloorMeta::ground(),
                pets: PetInputs::default(),
            },
            theme,
            size: Size { w, h },
            place: pixtuoid_scene::look::Place::default(),
            debug_walkable: false,
        });
        let badges = renderer.badges();
        assert_eq!(badges.len(), 2, "two seeded agents → two name badges");
        let (sw, sh) = (usize::from(w) * 3, usize::from(h) * 3);
        let ground = 0x0080_8080u32;
        let mut sb = vec![ground; sw * sh];
        paint_labels_into_surface(
            &mut XrgbSurface::new(&mut sb, sw, sh).expect("sized"),
            badges,
            scale,
        );
        let marker = pixtuoid_scene::overlay::BADGE_MARKER.to_string();
        for badge in badges {
            let mw = crate::aa_text::text_width(&marker, LABEL_FONT_PX);
            let tw = mw + crate::aa_text::text_width(&badge.name.text, LABEL_FONT_PX);
            let left = i32::from(badge.at.x) * scale - tw / 2;
            let top = i32::from(badge.at.y) * scale - crate::aa_text::line_height(LABEL_FONT_PX);
            let inks: std::collections::HashSet<u32> = sb
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let (x, y) = ((i % sw) as i32, (i / sw) as i32);
                    (left..left + tw).contains(&x)
                        && (top..i32::from(badge.at.y) * scale).contains(&y)
                })
                .map(|(_, &p)| p)
                .collect();
            for ink in [badge.marker, badge.name.ink] {
                assert!(
                    inks.contains(&pack_xrgb(ink)),
                    "{:?}'s badge lacks {ink:?}",
                    badge.name.text
                );
            }
        }
    }
}
