//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! Paints the buffer at whatever dims it's handed, owning one
//! `pixtuoid_scene::floor::FloorSession` across frames so walks stay continuous.

use std::sync::atomic::{AtomicUsize, Ordering};

use pixtuoid_core::sprite::format::Density;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{MAX_FLOORS, SceneState};

use pixtuoid_scene::flash::{FlashHold, FlashPhase};
use pixtuoid_scene::floor::{FloorInputs, OfficeSession};
use pixtuoid_scene::footer::{FooterContext, FooterInputs, FooterModel, build_footer};
use pixtuoid_scene::layout::Size;
use pixtuoid_scene::look::{Look, RenderInputs};
use pixtuoid_scene::render_scale::PixelFit;
use pixtuoid_scene::theme::Theme;
use winit::dpi::{LogicalSize, PhysicalSize};

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
    session: OfficeSession,
    /// The configured pets: each floor shows the one its seed picks.
    pets: Vec<pixtuoid_scene::pet::Pet>,
    /// Ambient-audio gateway. Inert unless installed.
    audio: crate::audio::AudioHandle,
    /// The flash the window shows.
    flash: FlashHold<FlashPhase, (u32, u32)>,
    /// The flash the last [`render_live`](Self::render_live) handed out, and
    /// the window it was for.
    rendered: (FlashPhase, (u32, u32)),
}

impl OfficeRenderer {
    /// A renderer drawing with `pack`, which every frame's `world.pack` must be.
    pub fn new(pack: std::sync::Arc<pixtuoid_core::sprite::format::Pack>) -> Self {
        Self {
            session: OfficeSession::new(pack),
            pets: Vec::new(),
            audio: crate::audio::AudioHandle::disabled(),
            flash: FlashHold::on(pixtuoid_scene::flash::monotonic()),
            rendered: (FlashPhase::default(), (0, 0)),
        }
    }

    /// The pets each floor picks its own from.
    pub(crate) fn set_pets(&mut self, pets: Vec<pixtuoid_scene::pet::Pet>) {
        self.pets = pets;
    }

    /// [`OfficeSession::showing_pet`] of this window's pets.
    pub(crate) fn showing_pet(&self) -> Option<&pixtuoid_scene::pet::Pet> {
        self.session.showing_pet(&self.pets)
    }

    /// Which floor shows, and the slide under way.
    pub(crate) fn nav(&self) -> &pixtuoid_scene::floor::FloorNav {
        self.session.nav()
    }

    /// The floors the last frame's scene filled.
    pub(crate) fn n_floors(&self) -> usize {
        self.session.n_floors()
    }

    /// Slide to floor `target`.
    pub(crate) fn navigate(&mut self, target: usize, now: std::time::SystemTime) {
        self.session.navigate(target, now);
    }

    /// [`OfficeSession::moves_off_beat`].
    pub(crate) fn moves_off_beat(&self) -> bool {
        self.session.moves_off_beat()
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Render the office's floor showing, or its slide to another, into the
    /// owned buffer in `at`'s look, the office `at`'s logical extent, with no
    /// footer row subtracted. `frame.world.scene` is the FULL scene, which the
    /// office projects onto each floor. A too-small layout leaves the buffer
    /// filled with the theme's `bg_fallback`.
    pub fn render(&mut self, at: WindowGeometry, frame: WindowFrame<'_>) -> Option<&RgbBuffer> {
        let WindowFrame {
            world,
            theme,
            place,
        } = frame;
        let FloorInputs {
            scene, floor, now, ..
        } = world;
        let gap = theme.surface.bg_fallback;
        self.session.render(
            at.look,
            RenderInputs {
                world,
                theme,
                size: at.office,
                place,
                debug_walkable: false,
            },
            &self.pets,
            gap,
        );
        // Composed even when disabled or muted: `AudioObserver::frame`'s contract.
        self.audio
            .frame(self.session.audio_frame(scene, floor, now));
        self.session.buf()
    }

    /// [`render`](Self::render) for the window on screen, `window` physical
    /// pixels, and whether its frame ([`buf`](Self::buf)) is to show: not
    /// when the flash hold keeps it back, so the window keeps the last. A
    /// frame shown is [`presented`](Self::presented) once it shows.
    pub fn render_live(
        &mut self,
        at: WindowGeometry,
        frame: WindowFrame<'_>,
        window: (u32, u32),
    ) -> bool {
        self.render(at, frame);
        let flash = self.session.flash();
        if self.flash.holds(flash, window) {
            return false;
        }
        self.rendered = (flash, window);
        true
    }

    /// The last frame's pixels, `None` before the first.
    pub fn buf(&self) -> Option<&RgbBuffer> {
        self.session.buf()
    }

    /// The frame [`render_live`](Self::render_live) last handed out finished
    /// presenting just now.
    pub fn presented(&mut self) {
        self.flash.shown(self.rendered.0, self.rendered.1);
    }

    /// What a left press at `cursor` (physical px) in a `window`-sized window
    /// drawn at `at` does at `now`, with `petting` the last one: resize from
    /// the bottom-right corner, carry out what the last frame shows there
    /// ([`SceneHit::action`](pixtuoid_scene::hit::SceneHit::action)), or else
    /// drag the frameless window — from a fixture as from the bare floor.
    pub(crate) fn press_at(
        &self,
        cursor: (f64, f64),
        window: (u32, u32),
        at: WindowGeometry,
        (petting, now): (
            Option<&pixtuoid_scene::pet::PetState>,
            std::time::SystemTime,
        ),
    ) -> Press {
        if super::geometry::near_resize_corner(cursor, window, RESIZE_CORNER_PX) {
            return Press::Resize;
        }
        self.hit_at(cursor, at)
            .and_then(|hit| hit.action(petting, now))
            .map_or(Press::Drag, Press::Act)
    }

    /// What the last frame, drawn at `at`, shows the pointer at `cursor`
    /// (physical px).
    pub fn hit_at(
        &self,
        cursor: (f64, f64),
        at: WindowGeometry,
    ) -> Option<pixtuoid_scene::hit::SceneHit<'_>> {
        let unit = |px: f64| {
            (px.max(0.0) as u32 / u32::from(at.unit_px.max(1))).min(u32::from(u16::MAX)) as u16
        };
        self.session.hit_at(pixtuoid_scene::layout::Bounds {
            x: unit(cursor.0),
            y: unit(cursor.1),
            width: 1,
            height: 1,
        })
    }

    /// Where the last frame may differ from the one on screen before it.
    pub(crate) fn dirty(&self) -> &pixtuoid_scene::cutaway::canvas::Dirty {
        self.session.dirty()
    }

    /// The status-footer model for the current scene, with the office's floor
    /// breadcrumb. `budget` is the caller's column budget ([`footer_budget`] at
    /// the live width).
    pub fn footer(
        &self,
        scene: &SceneState,
        budget: u16,
        audio_audible: bool,
        volume_flash: Option<u8>,
        warning: Option<&str>,
    ) -> FooterModel {
        let floor_scene = self.session.footer_scene(scene);
        let inputs = FooterInputs::new(
            &floor_scene,
            FooterContext::new(
                scene,
                self.session.footer_floor(scene),
                audio_audible,
                volume_flash,
                warning,
                FOOTER_KEYS,
                FOOTER_KEYS,
            ),
        );
        build_footer(&inputs, budget)
    }
}

/// Everything a presented frame shows beside the office: the window's size,
/// its footer, and the tooltip by the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Overlays {
    pub(crate) window: (u32, u32),
    pub(crate) footer: FooterModel,
    pub(crate) tooltip: Option<(pixtuoid_scene::tooltip::Tooltip, (i32, i32))>,
}

/// What the window shows, as far as a frame may skip presenting: the overlays
/// of the frame on screen, known only while the screen holds the last frame
/// rendered, which an office's dirt is measured against.
#[derive(Debug)]
pub(crate) struct Screen {
    shown: Option<Overlays>,
    /// Whether the platform keeps the window's pixels between presents. X11
    /// does not ("X does not guarantee to preserve the contents of windows",
    /// Xlib's overview), and winit hands its `Expose` over as the same
    /// `RedrawRequested` a paint tick asks for, so no frame may skip there.
    retains: bool,
}

impl Screen {
    /// Whether a frame showing `next` over an office `dirty` against the last
    /// rendered must be presented: an unchanged office under the same overlays
    /// is the frame on screen, and presenting it again only spends the copy
    /// and the present.
    pub(crate) fn needs(
        &self,
        next: &Overlays,
        dirty: &pixtuoid_scene::cutaway::canvas::Dirty,
    ) -> bool {
        !self.retains
            || *dirty != pixtuoid_scene::cutaway::canvas::Dirty::Unchanged
            || self.shown.as_ref() != Some(next)
    }

    /// A window on a platform that keeps its pixels (`retains`) or not.
    pub(crate) fn new(retains: bool) -> Self {
        Self {
            shown: None,
            retains,
        }
    }

    /// The screen of the window whose handle is `raw`: X11's (Xlib or XCB)
    /// keeps no pixels, and a window that names no handle is taken as one
    /// that might not.
    pub(crate) fn of_window(raw: Option<winit::raw_window_handle::RawWindowHandle>) -> Self {
        use winit::raw_window_handle::RawWindowHandle;
        Self::new(!matches!(
            raw,
            None | Some(RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_))
        ))
    }

    /// A frame was rendered that may not reach the screen — held back, or
    /// about to present: until one shows, the screen matches nothing rendered.
    pub(crate) fn stale(&mut self) {
        self.shown = None;
    }

    /// A frame with `overlays` reached the screen.
    pub(crate) fn shown(&mut self, overlays: Overlays) {
        self.shown = Some(overlays);
    }
}

/// What a left press does: [`OfficeRenderer::press_at`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    Resize,
    Act(pixtuoid_scene::hit::HitAction),
    Drag,
}

/// Window pixels from the bottom-right corner within which a press resizes.
const RESIZE_CORNER_PX: f64 = 18.0;

/// One floor's frame for the window: a [`RenderInputs`] whose office extent
/// the window's [`WindowGeometry`] owns.
#[derive(Debug, Clone, Copy)]
pub struct WindowFrame<'a> {
    pub world: FloorInputs<'a>,
    pub theme: &'static pixtuoid_scene::theme::Theme,
    pub place: pixtuoid_scene::look::Place,
}

/// The window's natural real pixels per logical unit: what keeps the office
/// near `OFFICE_TARGET_H` units tall, so its art stays chunky and legible.
/// Min 1.
pub(crate) fn office_scale(win_h: u32) -> u32 {
    const OFFICE_TARGET_H: u32 = 180;
    (f64::from(win_h) / f64::from(OFFICE_TARGET_H))
        .round()
        .max(1.0) as u32
}

/// What a window draws its office as: the look, the office's logical extent
/// (which the desk capacity is derived from), and the whole factor the
/// rendered buffer is upscaled by to the window. Everything the window paints
/// reads it, so another look is another [`window_geometry`] arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    pub look: Look,
    pub office: Size,
    pub upscale: u16,
    /// Window pixels per layout unit, which a pointer maps back through.
    pub unit_px: u16,
}

/// How a PHYSICAL-px window draws its office: the cutaway at the pack's
/// `density`, `office_scale` fitted to it and never below it, so the window
/// never falls back to the classic. The ONE place this geometry lives, so the
/// desk capacity derived from it can't drift from the office drawn.
///
/// Takes winit's `PhysicalSize` rather than two bare `u32`s so the UNIT is carried by
/// the type: the `[floating]` config size is LOGICAL, and handing it here is a compile
/// error instead of a silent HiDPI mis-seed (#803).
pub fn window_geometry(size: PhysicalSize<u32>, density: Density) -> WindowGeometry {
    let px = |p: u32| u16::try_from(p).unwrap_or(u16::MAX);
    let fit = PixelFit::at_least_density(
        px(office_scale(size.height)),
        density,
        Size {
            w: px(size.width),
            h: px(size.height),
        },
    );
    WindowGeometry {
        look: Look::Cutaway {
            scale: fit.render_scale(),
        },
        office: fit.logical(),
        upscale: fit.upscale(),
        unit_px: fit.scale().get(),
    }
}

/// The smallest window, in logical px, whose office lays out:
/// [`min_layout_size`](pixtuoid_scene::layout::min_layout_size) at the pack's
/// `density`, which [`window_geometry`] never draws below, on a display that
/// gives a logical px one physical px.
pub(crate) fn min_window(density: Density) -> LogicalSize<u32> {
    let min = pixtuoid_scene::layout::min_layout_size();
    let px = |units: u16| u32::from(units) * u32::from(density.get());
    LogicalSize::new(px(min.w), px(min.h))
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
pub(crate) fn boot_capacities_for_window(
    size: PhysicalSize<u32>,
    density: Density,
) -> [usize; MAX_FLOORS] {
    let office = window_geometry(size, density).office;
    floor_caps_for_buffer(office.w, office.h)
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

/// The footer's AA font size (px), drawn at NATIVE surface res so it stays a
/// crisp fixed-height caption whatever the office's scale.
const FOOTER_FONT_PX: f32 = 12.0;
/// The footer text's drop shadow: it draws straight over the office, so a 1px
/// offset shadow keeps it legible over bright windows and plants.
const TEXT_SHADOW: u32 = 0x0000_0000;

/// The floating footer's keybind-hint tail — floating's REAL controls (no terminal
/// `[q]uit`/`[t]heme`/`[?]help` chrome). The ONE painter-specific input to the shared
/// footer model; everything else is TUI-identical.
const FOOTER_KEYS: &str = " [p]ause [m]ute [+/-]vol ";
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
    /// linear blend in `0x00RRGGBB` space; the footer sits on opaque office
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

    /// Fill the `w`×`h` rect from `(x, y)` with `color`, clipped to the
    /// surface.
    fn fill(&mut self, (x, y): (i32, i32), (w, h): (i32, i32), color: u32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.w as i32), (y + h).min(self.h as i32));
        for py in y0..y1 {
            let row = py as usize * self.w;
            for px in x0..x1 {
                self.px[row + px as usize] = color;
            }
        }
    }

    /// `text` at `(x, top_y)` in `color`, over a one-pixel drop shadow.
    fn draw_shadowed_text(&mut self, text: &str, x: i32, top_y: i32, font_px: f32, color: u32) {
        crate::aa_text::draw_text_at(text, x + 1, top_y + 1, font_px, |gx, gy, cov| {
            self.blend(gx, gy, TEXT_SHADOW, cov)
        });
        crate::aa_text::draw_text_at(text, x, top_y, font_px, |gx, gy, cov| {
            self.blend(gx, gy, color, cov)
        });
    }
}

/// Window pixels between a tooltip's box and its text.
const TOOLTIP_PAD_PX: i32 = 6;
/// Window pixels from the pointer to a tooltip's box.
const TOOLTIP_GAP_PX: i32 = 14;

/// Paint `tip` by the pointer at `cursor` (physical px) — the floating twin
/// of the TUI's `paint_tooltip`, laying out the SAME shared model: an agent's
/// card opens below the pointer and a label above it, each flipping when the
/// window has no room, and each shifting left to stay inside it.
pub fn paint_tooltip_into_surface(
    sb: &mut XrgbSurface<'_>,
    tip: &pixtuoid_scene::tooltip::Tooltip,
    cursor: (f64, f64),
    theme: &Theme,
) {
    use pixtuoid_scene::tooltip::{TipAnchor, TipRow, TipSpan};
    let width = |spans: &[TipSpan]| -> i32 {
        spans
            .iter()
            .map(|s| crate::aa_text::text_width(&s.text, FOOTER_FONT_PX))
            .sum()
    };
    let gap = crate::aa_text::text_width("  ", FOOTER_FONT_PX);
    let content_w = tip
        .rows
        .iter()
        .map(|row| match row {
            TipRow::Spans(spans) => width(spans),
            TipRow::Heading { left, right } => {
                width(left) + gap + crate::aa_text::text_width(&right.text, FOOTER_FONT_PX)
            }
            TipRow::Rule => 0,
        })
        .max()
        .unwrap_or(0);
    let line_h = crate::aa_text::line_height(FOOTER_FONT_PX);
    let (box_w, box_h) = (
        content_w + 2 * TOOLTIP_PAD_PX,
        line_h * tip.rows.len() as i32 + 2 * TOOLTIP_PAD_PX,
    );
    let (cx, cy) = (cursor.0 as i32, cursor.1 as i32);
    let (sw, sh) = (sb.w as i32, sb.h as i32);
    let below = cy + TOOLTIP_GAP_PX;
    let above = cy - TOOLTIP_GAP_PX - box_h;
    let y = match tip.anchor {
        TipAnchor::Below if below + box_h > sh => above,
        TipAnchor::Below => below,
        TipAnchor::Above if above < 0 => below,
        TipAnchor::Above => above,
    }
    .clamp(0, (sh - box_h).max(0));
    let x = (cx + TOOLTIP_GAP_PX).min(sw - box_w).max(0);
    sb.fill((x, y), (box_w, box_h), pack_xrgb(theme.ui.tooltip_bg));
    let ink = |s: &TipSpan| pack_xrgb(s.tone.rgb(theme).unwrap_or(theme.ui.tooltip_text));
    let left = x + TOOLTIP_PAD_PX;
    for (i, row) in tip.rows.iter().enumerate() {
        let top = y + TOOLTIP_PAD_PX + line_h * i as i32;
        let mut run = |spans: &[TipSpan], mut at: i32| {
            for s in spans {
                sb.draw_shadowed_text(&s.text, at, top, FOOTER_FONT_PX, ink(s));
                at += crate::aa_text::text_width(&s.text, FOOTER_FONT_PX);
            }
        };
        match row {
            TipRow::Spans(spans) => run(spans, left),
            TipRow::Heading { left: l, right } => {
                run(l, left);
                let right_x =
                    left + content_w - crate::aa_text::text_width(&right.text, FOOTER_FONT_PX);
                run(std::slice::from_ref(right), right_x);
            }
            TipRow::Rule => sb.fill(
                (left, top + line_h / 2),
                (content_w, 1),
                pack_xrgb(theme.ui.tooltip_dim),
            ),
        }
    }
}

/// Column budget for the floating footer at `win_w` px — how many monospace Monaspace
/// advances fit between the margins. Monaspace is fixed-advance, so a column budget maps
/// cleanly to pixels.
pub fn footer_budget(win_w: usize) -> u16 {
    let advance = crate::aa_text::text_width("M", FOOTER_FONT_PX).max(1);
    (((win_w as i32 - 2 * FOOTER_MARGIN_PX).max(0)) / advance) as u16
}

/// Paint the shared status footer as a bottom-overlay band — the floating twin of the
/// TUI's status row, rendering the SAME [`build_footer`] model so the two can't drift.
/// An OVERLAY over the office's bottom rows: it never insets the buffer (that would
/// shift the desk-capacity lockstep). Fixed caption height, so it stays crisp at any
/// office scale.
pub fn paint_footer_into_surface(sb: &mut XrgbSurface<'_>, model: &FooterModel, theme: &Theme) {
    let y = (sb.h as i32 - crate::aa_text::line_height(FOOTER_FONT_PX) - FOOTER_MARGIN_PX).max(0);
    let mut x = FOOTER_MARGIN_PX;
    for seg in &model.segments {
        let color = pack_xrgb(seg.tone.rgb(theme));
        sb.draw_shadowed_text(&seg.text, x, y, FOOTER_FONT_PX, color);
        x += crate::aa_text::text_width(&seg.text, FOOTER_FONT_PX);
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use pixtuoid_scene::floor::{FloorMeta, PetInputs};

    /// The bundled pack's densest art, which the window draws at.
    fn density() -> Density {
        pixtuoid_scene::pack::load_bundled_pack()
            .expect("bundled pack loads")
            .max_density_variant()
    }

    /// The window's geometry for an office `size` units big, at the test
    /// pack's densest art and no upscale.
    fn cutaway(size: Size) -> WindowGeometry {
        let density = density();
        WindowGeometry {
            look: Look::Cutaway {
                scale: pixtuoid_scene::render_scale::RenderScale::from(density),
            },
            office: size,
            upscale: 1,
            unit_px: density.get(),
        }
    }

    use pixtuoid_scene::layout::Size;
    use std::time::Duration;

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
            .render(
                cutaway(Size { w: 160, h: 96 }),
                WindowFrame {
                    world: FloorInputs {
                        scene: &scene,
                        pack: &pack,
                        now,
                        floor: FloorMeta::ground(),
                        pets: PetInputs::default(),
                    },
                    theme,
                    place: pixtuoid_scene::look::Place::default(),
                },
            )
            .expect("a frame");
        let d = density().get();
        assert_eq!(
            (buf.width(), buf.height()),
            (160 * d, 96 * d),
            "the office at its densest art"
        );
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

    /// An empty office's window, under `weather`, moving fully, its flashes
    /// held on a screen clock the test moves.
    struct Window {
        renderer: OfficeRenderer,
        scene: SceneState,
        floor: FloorMeta,
        screen: pixtuoid_scene::flash::ManualClock,
        /// How long a present takes on the screen clock.
        present_takes: Duration,
        /// What the window shows.
        shown: RgbBuffer,
        /// The window's physical pixels.
        px: (u32, u32),
    }

    impl Window {
        fn new(weather: pixtuoid_scene::sky::WeatherPolicy) -> Self {
            let screen = pixtuoid_scene::flash::ManualClock::default();
            let mut renderer = OfficeRenderer::new(crate::test_flash::pack_arc());
            renderer.flash = FlashHold::on(screen.clock());
            Self {
                renderer,
                scene: SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]),
                floor: FloorMeta::ground()
                    .with_weather(weather)
                    .with_motion(pixtuoid_scene::anim::Motion::Full),
                screen,
                present_takes: Duration::ZERO,
                shown: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
                px: (160, 96),
            }
        }

        /// What the window shows after its redraw at `now`, as `window.rs`
        /// presents it.
        fn present(&mut self, now: SystemTime) -> RgbBuffer {
            let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme");
            self.screen.at(now);
            let live = self.renderer.render_live(
                cutaway(Size { w: 160, h: 96 }),
                WindowFrame {
                    world: FloorInputs {
                        scene: &self.scene,
                        pack: crate::test_flash::pack(),
                        now,
                        floor: self.floor,
                        pets: PetInputs::default(),
                    },
                    theme,
                    place: pixtuoid_scene::look::Place::default(),
                },
                self.px,
            );
            if let Some(frame) = self.renderer.buf().filter(|_| live) {
                self.shown = frame.clone();
                self.screen.advance(self.present_takes);
                self.renderer.presented();
            }
            self.shown.clone()
        }
    }

    /// A window resized under a hold gets its frame at once: a screen of a new
    /// shape shows nothing to hold. The same frame unresized is held.
    #[test]
    fn a_resized_window_is_never_held() {
        use crate::test_flash::{held_frames, storm_strike};
        let strike = storm_strike();
        let [dark, late, held, _] = held_frames(&strike);
        for resized in [false, true] {
            let mut window = Window::new(strike.weather);
            window.present(dark);
            let before = window.present(late);
            if resized {
                window.px = (window.px.0 * 2, window.px.1 * 2);
            }
            let after = window.present(held);
            assert_eq!(after.as_slice() != before.as_slice(), resized);
        }
    }

    /// A phase holds the floor from when its present lands, not from when its
    /// redraw began: after a slow present, the next phase waits for the floor
    /// to pass on the screen clock, though its frame's own clock says it has.
    #[test]
    fn a_slow_presents_phase_holds_the_floor_from_when_it_lands() {
        use crate::test_flash::storm_strike;
        const SLOW: Duration = Duration::from_millis(60);
        let floor = Duration::from_millis(pixtuoid_scene::anim::PHOTOSENSITIVE_PHASE_MIN_MS);
        let strike = storm_strike();
        let [first, second] = [strike.changes[0], strike.changes[1]];
        let mut window = Window::new(strike.weather);
        window.present(first - 2 * floor);
        window.present_takes = SLOW;
        let shown = window.present(first);
        let landed = window.screen.now();
        window.present_takes = Duration::ZERO;
        assert!(
            second.duration_since(first).expect("in order") >= floor,
            "the frame clock says the floor has passed"
        );
        assert!(
            window.present(second).as_slice() == shown.as_slice(),
            "held until the slow present's phase shows the floor"
        );
        let after = std::time::UNIX_EPOCH + landed + floor;
        assert!(
            window.present(after).as_slice() != shown.as_slice(),
            "shown once it has"
        );
    }

    /// Each phase of a strike stays on screen at least the photosensitive
    /// floor in the window, at its active cadence and each frame grid: from
    /// the frame that first shows it to the one that replaces it. A strike
    /// lifts the whole room, so a frame that changes most of the office's
    /// pixels is a change of phase.
    #[test]
    fn each_strike_phase_holds_the_floor_on_screen_in_the_window() {
        use crate::test_flash::{
            assert_each_phase_holds_the_floor, frame_grid, lead, storm_strike,
        };
        let strike = storm_strike();
        for (frame, offset) in frame_grid(super::super::cadence::frame()) {
            let mut window = Window::new(strike.weather);
            let now = strike.start - lead(frame) + offset;
            let mut shown = window.present(now);
            let mut changed = Vec::new();
            for now in crate::test_flash::frames_after(now, frame, strike.end + lead(frame)) {
                let presented = window.present(now);
                let differ = presented
                    .as_slice()
                    .iter()
                    .zip(shown.as_slice())
                    .filter(|(a, b)| a != b)
                    .count();
                if 2 * differ > presented.as_slice().len() {
                    changed.push(now);
                }
                shown = presented;
            }
            let at = format!("a frame each {frame:?} from +{offset:?}");
            assert_each_phase_holds_the_floor(&changed, strike.changes.len(), &at);
        }
    }

    /// A starved neon's every catch, and every dark between, stays on screen
    /// at least the photosensitive floor in the window, at its active cadence
    /// and each frame grid. The pixels by the tube's west side change only
    /// with it.
    #[test]
    fn each_stutter_phase_holds_the_floor_on_screen_in_the_window() {
        use crate::test_flash::{
            assert_each_phase_holds_the_floor, frame_grid, lead, neon_tube, starved_stutter,
        };
        let stutter = starved_stutter();
        let tube = |buf: &RgbBuffer| -> Vec<Rgb> {
            (0..buf.height())
                .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| neon_tube(x, y))
                .map(|(x, y)| buf.get(x, y))
                .collect()
        };
        for (frame, offset) in frame_grid(super::super::cadence::frame()) {
            let mut window = Window::new(stutter.weather);
            for at in stutter.setup {
                window.present(at);
            }
            let now = stutter.start - lead(frame) + offset;
            let mut shown = tube(&window.present(now));
            let mut changed = Vec::new();
            for now in crate::test_flash::frames_after(now, frame, stutter.end + lead(frame)) {
                let presented = tube(&window.present(now));
                if presented != shown {
                    changed.push(now);
                }
                shown = presented;
            }
            let at = format!("a frame each {frame:?} from +{offset:?}");
            assert_each_phase_holds_the_floor(&changed, stutter.changes, &at);
        }
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

    /// A saved size below the pack's minimum opens, and is placed, at the
    /// minimum: an office that seats every floor.
    #[test]
    fn a_saved_size_below_the_minimum_opens_where_every_floor_seats() {
        let min = min_window(density());
        let cfg: crate::config::AppConfig =
            toml::from_str("[floating]\nwidth = 1\nheight = 1\n").expect("parses");
        let f = crate::config::resolve_floating(&cfg).at_least(min.width, min.height);
        assert_eq!((f.width, f.height), (min.width, min.height));
        let caps = boot_capacities_for_window(PhysicalSize::new(f.width, f.height), density());
        assert!(caps.iter().all(|&c| c > 0), "{caps:?}");
    }

    #[test]
    fn the_smallest_window_seats_every_floor_at_any_scale_factor() {
        let min = min_window(density());
        for factor in 1..=3 {
            let size = PhysicalSize::new(min.width * factor, min.height * factor);
            let caps = boot_capacities_for_window(size, density());
            assert!(caps.iter().all(|&c| c > 0), "{factor}x: {caps:?}");
        }
    }

    #[test]
    fn boot_capacities_for_window_match_the_first_redraw_geometry_not_the_tui_overseed() {
        let (w, h) = (1280u32, 720u32);
        let office = window_geometry(PhysicalSize::new(w, h), density()).office;
        let boot = boot_capacities_for_window(PhysicalSize::new(w, h), density());
        for (i, &got) in boot.iter().enumerate() {
            let want = pixtuoid_scene::floor::floor_capacity(
                office.w,
                office.h,
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
        let as_if_physical = boot_capacities_for_window(
            PhysicalSize::new(logical.width as u32, logical.height as u32),
            density(),
        );

        // MEASURED offices for the default 480×320 logical window. `office_scale`
        // ROUNDS before the density fit, so the office is NOT monotone in sf (at
        // 4× it shrinks to 240×160) — no logical-side seed is sound.
        let measured = [
            (1.00_f64, (120u32, 80u32), 6usize),
            (1.25, (150, 100), 12),
            (1.50, (180, 120), 20),
            (1.75, (210, 140), 24),
            (2.00, (240, 160), 30),
            (3.00, (360, 240), 80),
        ];
        for (sf, want_buf, want_floor0) in measured {
            let physical: PhysicalSize<u32> = logical.to_physical(sf);
            let office = window_geometry(physical, density()).office;
            assert_eq!(
                (u32::from(office.w), u32::from(office.h)),
                want_buf,
                "office at {sf}× of {logical:?}"
            );
            assert_eq!(
                boot_capacities_for_window(physical, density())[0],
                want_floor0,
                "floor-0 seed at {sf}×"
            );
        }
        let at_2x = boot_capacities_for_window(logical.to_physical(2.0), density());
        assert!(
            at_2x[0] > as_if_physical[0],
            "logical-as-physical under-seeds at 2×: {} vs the real {}",
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
        let office = window_geometry(tiny, density()).office;
        assert_eq!(
            pixtuoid_scene::floor::floor_capacity(
                office.w,
                office.h,
                pixtuoid_scene::floor::floor_seed(0)
            ),
            0,
            "fixture must actually be unlayoutable, else this asserts nothing"
        );
        assert_eq!(
            boot_capacities_for_window(tiny, density())[0],
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
            let seed = boot_capacities_for_window(window, density());
            let caps: [AtomicUsize; MAX_FLOORS] = std::array::from_fn(|_| AtomicUsize::new(0));
            let office = window_geometry(window, density()).office;
            sync_floor_caps(&mut None, &caps, office.w, office.h);
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

    /// A press acts on what the frame on screen shows under it: an agent's
    /// units hit that agent, a bare unit drags, and the bottom-right corner
    /// resizes.
    #[test]
    fn a_press_hits_what_the_frame_shows_there() {
        use pixtuoid_scene::hit::{HitAction, SceneHit};
        let pack = std::sync::Arc::new(
            pixtuoid_scene::pack::load_bundled_pack().expect("bundled pack loads"),
        );
        let agent = active_on("/p/a.jsonl", 0, 0);
        let id = agent.agent_id;
        let scene = scene_with(vec![agent], 16);
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let window = PhysicalSize::new(960u32, 640u32);
        let at = window_geometry(window, pack.max_density_variant());
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        renderer
            .render(
                at,
                WindowFrame {
                    world: FloorInputs {
                        scene: &scene,
                        pack: &pack,
                        now,
                        floor: FloorMeta::ground(),
                        pets: PetInputs::default(),
                    },
                    theme,
                    place: pixtuoid_scene::look::Place::default(),
                },
            )
            .expect("a frame");
        let centre = |u: u16| f64::from(u) * f64::from(at.unit_px) + f64::from(at.unit_px) / 2.0;
        let press =
            |cursor| renderer.press_at(cursor, (window.width, window.height), at, (None, now));
        let (mut hit_agent, mut dragged, mut fixture_drags) = (false, false, false);
        for y in 0..at.office.h {
            for x in 0..at.office.w {
                let cursor = (centre(x), centre(y));
                match press(cursor) {
                    Press::Act(HitAction::Focus(hit)) => {
                        assert_eq!(hit, id, "a press hit another agent");
                        hit_agent = true;
                    }
                    Press::Drag => dragged = true,
                    Press::Act(_) | Press::Resize => {}
                }
                if matches!(renderer.hit_at(cursor, at), Some(SceneHit::Furniture(_))) {
                    assert_eq!(press(cursor), Press::Drag, "a fixture holds the window");
                    fixture_drags = true;
                }
            }
        }
        assert!(hit_agent, "no press found the agent the frame drew");
        assert!(dragged, "no bare unit to drag the window by");
        assert!(fixture_drags, "the frame drew no labelled fixture");
        let corner = (
            f64::from(window.width) - 1.0,
            f64::from(window.height) - 1.0,
        );
        assert_eq!(press(corner), Press::Resize);
    }

    /// A tooltip paints its box in the theme's tooltip background, inside the
    /// window wherever the pointer is: a label near the top flips below it,
    /// and a card at the right edge shifts left.
    #[test]
    fn a_tooltip_stays_in_the_window_and_flips_off_the_edges() {
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let bg = pack_xrgb(theme.ui.tooltip_bg);
        let (w, h) = (320usize, 200usize);
        let painted = |tip: &pixtuoid_scene::tooltip::Tooltip, cursor: (f64, f64)| {
            let mut px = vec![0u32; w * h];
            let mut sb = XrgbSurface::new(&mut px, w, h).expect("sized");
            paint_tooltip_into_surface(&mut sb, tip, cursor, theme);
            let rows: Vec<usize> = (0..h)
                .filter(|&y| px[y * w..(y + 1) * w].contains(&bg))
                .collect();
            let cols: Vec<usize> = (0..w)
                .filter(|&x| (0..h).any(|y| px[y * w + x] == bg))
                .collect();
            (rows, cols)
        };
        let label = pixtuoid_scene::tooltip::coffee();
        let (rows, _) = painted(&label, (100.0, 4.0));
        assert!(!rows.is_empty(), "the label painted nothing");
        assert!(
            rows[0] > 4,
            "a label at the top flips below the pointer: {rows:?}"
        );
        let (rows, _) = painted(&label, (100.0, 150.0));
        assert!(
            *rows.last().expect("painted") < 150,
            "a label opens above the pointer"
        );
        let (_, cols) = painted(&label, (318.0, 100.0));
        assert!(
            *cols.last().expect("painted") < w,
            "the label shifted inside the right edge"
        );
        assert!(
            cols[0] < 318,
            "the label shifted left of a right-edge pointer"
        );
        // An agent's card opens below the pointer, flips above at the bottom
        // edge, and shifts left at the right one.
        let agent = active_on("/p/a.jsonl", 0, 0);
        let id = agent.agent_id;
        let scene = scene_with(vec![agent], 16);
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let card = pixtuoid_scene::tooltip::agent(&scene, id, now).expect("the agent's card");
        let (rows, _) = painted(&card, (100.0, 20.0));
        assert!(rows[0] > 20, "a card opens below the pointer: {rows:?}");
        let (rows, _) = painted(&card, (100.0, 195.0));
        assert!(
            *rows.last().expect("painted") < 195 && rows[0] > 0,
            "a card at the bottom flips above, inside the window: {rows:?}"
        );
        let (_, cols) = painted(&card, (318.0, 20.0));
        assert!(
            cols[0] < 318 && *cols.last().expect("painted") < w,
            "a card at the right edge shifts left, inside the window"
        );
    }

    /// A frame presents when its office changed or anything over it did, an
    /// unchanged office under the same overlays is the frame on screen, and a
    /// frame rendered but not shown leaves nothing to skip against.
    #[test]
    fn only_a_changed_frame_presents() {
        use pixtuoid_scene::cutaway::canvas::Dirty;
        let overlays = |w: u32, text: &str| Overlays {
            window: (w, 100),
            footer: FooterModel {
                segments: vec![pixtuoid_scene::footer::FooterSegment {
                    text: text.into(),
                    tone: pixtuoid_scene::footer::FooterTone::Neutral,
                }],
            },
            tooltip: None,
        };
        let shown = overlays(200, "a");
        let mut screen = Screen::new(true);
        assert!(screen.needs(&shown, &Dirty::Unchanged), "the first frame");
        screen.shown(shown.clone());
        assert!(!screen.needs(&shown, &Dirty::Unchanged));
        assert!(screen.needs(&shown, &Dirty::All));
        assert!(screen.needs(&overlays(201, "a"), &Dirty::Unchanged));
        assert!(screen.needs(&overlays(200, "b"), &Dirty::Unchanged));
        let tipped = Overlays {
            tooltip: Some((pixtuoid_scene::tooltip::coffee(), (3, 4))),
            ..shown.clone()
        };
        assert!(screen.needs(&tipped, &Dirty::Unchanged));
        // A held frame changed the office off screen: the next, unchanged
        // against it, still presents.
        screen.stale();
        assert!(
            screen.needs(&shown, &Dirty::Unchanged),
            "after a held frame"
        );
        // Where the platform keeps no pixels, every frame presents: X11's
        // windows, and one that names no handle.
        use winit::raw_window_handle::{
            RawWindowHandle, Win32WindowHandle, XcbWindowHandle, XlibWindowHandle,
        };
        let x11 = [
            RawWindowHandle::Xlib(XlibWindowHandle::new(1)),
            RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::MIN)),
        ];
        assert!(
            x11.into_iter()
                .all(|raw| !Screen::of_window(Some(raw)).retains)
        );
        assert!(!Screen::of_window(None).retains);
        let win32 = Win32WindowHandle::new(std::num::NonZeroIsize::MIN);
        assert!(Screen::of_window(Some(RawWindowHandle::Win32(win32))).retains);
        let mut forgetful = Screen::new(false);
        forgetful.shown(shown.clone());
        assert!(
            forgetful.needs(&shown, &Dirty::Unchanged),
            "X11 retains nothing"
        );
    }

    /// The footer counts the floor showing, beside the breadcrumb's whole
    /// office: one agent of four on the ground floor reads 1/4.
    #[test]
    fn the_footer_counts_the_floor_showing() {
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
        renderer.render(
            cutaway(Size { w: 160, h: 96 }),
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place::default(),
            },
        );
        let text: String = renderer
            .footer(&scene, u16::MAX, false, None, None)
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(text.contains("1/4"), "{text:?}");
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
        renderer.render(
            cutaway(Size { w: 160, h: 96 }),
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place::default(),
            },
        );
        let frames = crate::audio::drain_frames(&rx);
        assert!(!frames.is_empty(), "an enabled handle receives frames");
        let stems = frames.last().unwrap().stems;
        let moderate = pixtuoid_scene::audio::stem_levels(
            &pixtuoid_scene::tally::StateCounts {
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

    /// The window's footer says what the TUI's does: a source's death or
    /// this run's decode drift, from the one shared message.
    #[test]
    fn the_footer_shows_the_shared_warning_in_place_of_the_stats() {
        let renderer = OfficeRenderer::new(crate::test_flash::pack_arc());
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let slot = active_on("/p/a.jsonl", 0, 0);
        scene.agents.insert(slot.agent_id, slot);
        let warning = crate::doctor::footer_warning(&[], &["cc"]);
        let budget = footer_budget(960);
        let calm = renderer.footer(&scene, budget, true, None, None).text();
        let warned = renderer
            .footer(&scene, budget, true, None, warning.as_deref())
            .text();
        assert!(warned.contains("decode drift: cc·"), "{warned}");
        assert!(!calm.contains("decode drift"), "{calm}");
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
            sb.contains(&pack_xrgb(FooterTone::Rung(RungKind::Active).rgb(theme))),
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
            renderer.render(
                cutaway(Size { w: 192, h: 160 }),
                WindowFrame {
                    world: FloorInputs {
                        scene: &scene,
                        pack: &pack,
                        now,
                        floor: FloorMeta::ground(),
                        pets: PetInputs::default(),
                    },
                    theme,
                    place: pixtuoid_scene::look::Place::default(),
                },
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
        renderer.render(
            cutaway(Size { w: 160, h: 96 }),
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place::default(),
            },
        );
        crate::audio::drain_frames(&rx); // discard the priming frames

        agents.push(active_on("/d/f1-new.jsonl", 1, cap));
        let scene = scene_with(agents.clone(), cap);
        now += std::time::Duration::from_millis(pixtuoid_scene::anim::PAINT_FRAME_MS);
        renderer.render(
            cutaway(Size { w: 160, h: 96 }),
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place::default(),
            },
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
        now += std::time::Duration::from_millis(pixtuoid_scene::anim::PAINT_FRAME_MS);
        renderer.render(
            cutaway(Size { w: 160, h: 96 }),
            WindowFrame {
                world: FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now,
                    floor: FloorMeta::ground(),
                    pets: PetInputs::default(),
                },
                theme,
                place: pixtuoid_scene::look::Place::default(),
            },
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
}
