//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! Paints the buffer at whatever dims it's handed, owning one
//! `pixtuoid_scene::floor::FloorSession` across frames so walks stay continuous.

use std::sync::atomic::{AtomicUsize, Ordering};

use pixtuoid_core::sprite::format::Density;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{MAX_FLOORS, SceneState};

use pixtuoid_scene::flash::{FlashHold, FlashPhase};
use pixtuoid_scene::floor::{FloorInputs, FloorSession};
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
    session: FloorSession,
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
            session: FloorSession::new(pack),
            audio: crate::audio::AudioHandle::disabled(),
            flash: FlashHold::on(pixtuoid_scene::flash::monotonic()),
            rendered: (FlashPhase::default(), (0, 0)),
        }
    }

    /// [`FloorSession::moves_off_beat`].
    pub(crate) fn moves_off_beat(&self) -> bool {
        self.session.moves_off_beat()
    }

    pub(crate) fn set_audio(&mut self, audio: crate::audio::AudioHandle) {
        self.audio = audio;
    }

    /// Render the floor into the owned buffer in `at`'s look, the office
    /// `at`'s logical extent, with no footer row subtracted. A too-small
    /// layout leaves the buffer filled with the theme's `bg_fallback`.
    pub fn render(&mut self, at: WindowGeometry, frame: WindowFrame<'_>) -> Option<&RgbBuffer> {
        let WindowFrame {
            world,
            theme,
            place,
        } = frame;
        let FloorInputs {
            scene, floor, now, ..
        } = world;
        self.session.render(
            at.look,
            RenderInputs {
                world,
                theme,
                size: at.office,
                place,
                debug_walkable: false,
            },
        );
        // Composed even when disabled or muted: `AudioObserver::frame`'s contract.
        self.audio
            .frame(self.session.audio_frame(scene, floor, now));
        self.session.buf()
    }

    /// [`render`](Self::render) for the window on screen, `window` physical
    /// pixels: `None` also when the flash hold keeps the frame back, so the
    /// window keeps the last. A frame handed out is
    /// [`presented`](Self::presented) once it shows.
    pub fn render_live(
        &mut self,
        at: WindowGeometry,
        frame: WindowFrame<'_>,
        window: (u32, u32),
    ) -> Option<&RgbBuffer> {
        self.render(at, frame);
        let flash = self.session.flash();
        if self.flash.holds(flash, window) {
            return None;
        }
        self.rendered = (flash, window);
        self.session.buf()
    }

    /// The frame [`render_live`](Self::render_live) last handed out finished
    /// presenting just now.
    pub fn presented(&mut self) {
        self.flash.shown(self.rendered.0, self.rendered.1);
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
                scale: pixtuoid_scene::render_scale::RenderScale::new(density.get())
                    .expect("nonzero"),
            },
            office: size,
            upscale: 1,
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
            let frame = self.renderer.render_live(
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
            if let Some(frame) = frame {
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
    /// minimum on the short axis: an office that seats every floor.
    #[test]
    fn a_saved_size_below_the_minimum_opens_where_every_floor_seats() {
        let min = min_window(density());
        let cfg: crate::config::AppConfig =
            toml::from_str("[floating]\nwidth = 240\nheight = 900\n").expect("parses");
        let f = crate::config::resolve_floating(&cfg).at_least(min.width, min.height);
        assert_eq!((f.width, f.height), (min.width.max(240), 900));
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
