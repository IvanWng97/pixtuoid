//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! Paints the buffer at whatever dims it's handed, owning one
//! `pixtuoid_scene::floor::FloorSession` across frames so walks stay continuous.

use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::state::SceneState;

use pixtuoid_scene::flash::{FlashHold, FlashPhase};
use pixtuoid_scene::floor::{FloorInputs, OfficeSession};
use pixtuoid_scene::footer::{FooterInputs, FooterModel, build_footer};
use pixtuoid_scene::interact::{Gesture, Pointer, Pressed};
use pixtuoid_scene::look::RenderInputs;
use pixtuoid_scene::render_scale::PixelFit;

/// Renders the live office to a reusable `RgbBuffer`. One per window — keeping it
/// alive across frames is what keeps walks/poses continuous (no walk-flash).
#[derive(Debug)]
pub struct OfficeRenderer {
    session: OfficeSession,
    /// The configured pets: each floor shows the one its seed picks.
    pets: Vec<pixtuoid_scene::pet::Pet>,
    /// The left button's gesture over the office.
    pointer: Pointer,
    /// Ambient-audio gateway. Inert unless installed.
    audio: crate::audio::AudioHandle,
    /// The flash the window shows.
    flash: FlashHold<FlashPhase, (u32, u32)>,
    /// The flash the last [`render_live`](Self::render_live) handed out, and
    /// the window it was for.
    rendered: (FlashPhase, (u32, u32)),
    /// The walkable debug layer, the TUI's `w`; not persisted.
    pub(super) debug_walkable: bool,
}

impl OfficeRenderer {
    /// A renderer drawing with `pack`, which every frame's `world.pack` must be.
    pub fn new(pack: std::sync::Arc<pixtuoid_scene::pack::OfficeArt>) -> Self {
        Self {
            session: OfficeSession::new(pack),
            pets: Vec::new(),
            pointer: Pointer::default(),
            audio: crate::audio::AudioHandle::disabled(),
            flash: FlashHold::on(pixtuoid_scene::flash::monotonic()),
            rendered: (FlashPhase::default(), (0, 0)),
            debug_walkable: false,
        }
    }

    /// Show or hide the walkable debug layer.
    pub(crate) fn toggle_walkable_debug(&mut self) {
        self.debug_walkable = !self.debug_walkable;
    }

    /// The pets each floor picks its own from.
    pub fn set_pets(&mut self, pets: Vec<pixtuoid_scene::pet::Pet>) {
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
    pub fn render(&mut self, at: PixelFit, frame: WindowFrame<'_>) -> Option<&RgbBuffer> {
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
            at.look(pixtuoid_scene::look::WorldText::Baked),
            RenderInputs {
                world,
                theme,
                size: at.logical(),
                place,
                debug_walkable: self.debug_walkable,
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
        at: PixelFit,
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

    /// A left press at `cursor` (physical px) in a `window`-sized window
    /// drawn at `at`, at `now` with `petting` the last one: resize from the
    /// bottom-right corner, else the pointer's — on what the last frame shows
    /// there, a click or a drag follows ([`Pointer`]) — else, on the bare
    /// office or a fixture, drag the frameless window.
    pub fn press_at(
        &mut self,
        cursor: (f64, f64),
        window: (u32, u32),
        at: PixelFit,
        pressing: Pressing<'_>,
    ) -> Press {
        if super::geometry::near_resize_corner(cursor, window, RESIZE_CORNER_PX) {
            // As any press does: a carry whose release never came ends here.
            self.cancel_pointer();
            return Press::Resize;
        }
        let unit = unit_at(cursor, at);
        let hit = self.session.hit_at(unit_bounds(unit));
        // The slop in this window's units, at least one.
        let slop_px = DRAG_SLOP_DIP * pressing.scale_factor;
        let units = (slop_px / f64::from(at.scale().get().max(1)))
            .ceil()
            .max(1.0) as u16;
        let slop = pixtuoid_scene::interact::Slop { x: units, y: units };
        let down = self
            .pointer
            .down(hit, unit, slop, pressing.petting, pressing.now);
        if let Some(ended) = &down.ended {
            self.session.grip(ended);
        }
        match down.pressed {
            Pressed::Something => Press::Pointer,
            Pressed::Bare => Press::Drag,
        }
    }

    /// End the pointer's gesture without a release, as when the window loses
    /// focus mid-drag; whether it set a figure down, which the window redraws.
    pub(crate) fn cancel_pointer(&mut self) -> bool {
        let ended = self.pointer.cancel();
        if let Some(gesture) = &ended {
            self.session.grip(gesture);
        }
        ended.is_some()
    }

    /// The pointer moved to `cursor` over a frame drawn at `at`; whether a
    /// figure it carries moved, which the window redraws.
    pub fn pointer_moved(&mut self, cursor: (f64, f64), at: PixelFit) -> bool {
        let gesture = self.pointer.moved(unit_at(cursor, at));
        if let Some(gesture) = &gesture {
            self.session.grip(gesture);
        }
        gesture.is_some()
    }

    /// The press released at `cursor` over a frame drawn at `at`: the click's
    /// action, the window's to carry out; a figure carried is set down.
    pub fn release(
        &mut self,
        cursor: (f64, f64),
        at: PixelFit,
    ) -> Option<pixtuoid_scene::hit::HitAction> {
        match self.pointer.up(unit_in(cursor, at))? {
            Gesture::Click(action) => Some(action),
            gesture => {
                self.session.grip(&gesture);
                None
            }
        }
    }

    /// Whether the pointer carries a figure.
    pub(crate) fn carrying(&self) -> bool {
        self.pointer.carrying()
    }

    /// [`release`](Self::release) under the panels `modal`: a carried figure
    /// is set down either way, and the click lands only with none open.
    pub(crate) fn release_under(
        &mut self,
        cursor: (f64, f64),
        at: PixelFit,
        modal: &crate::panels::ModalState,
    ) -> Option<pixtuoid_scene::hit::HitAction> {
        self.release(cursor, at).filter(|_| !modal.any_open())
    }

    /// Whether a pointer `cursor_in` the window shows its tooltip: not over a
    /// figure in hand, nor over the office under a panel.
    pub(crate) fn tooltip_shows(&self, cursor_in: bool, modal: &crate::panels::ModalState) -> bool {
        cursor_in && !self.carrying() && !modal.any_open()
    }

    /// What the last frame, drawn at `at`, shows the pointer at `cursor`
    /// (physical px).
    pub fn hit_at(
        &self,
        cursor: (f64, f64),
        at: PixelFit,
    ) -> Option<pixtuoid_scene::hit::SceneHit<'_>> {
        self.session.hit_at(unit_bounds(unit_at(cursor, at)))
    }

    /// Where the last frame may differ from the one on screen before it.
    pub(crate) fn dirty(&self) -> &pixtuoid_scene::cutaway::canvas::Dirty {
        self.session.dirty()
    }

    /// The status-footer model for the current scene, with the office's floor
    /// breadcrumb. `budget` is the caller's column budget ([`footer_budget`](super::overlays::footer_budget) at
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
            crate::panels::widgets::footer_context(
                scene,
                self.session.footer_floor(scene),
                audio_audible,
                volume_flash,
                warning,
            ),
        );
        build_footer(&inputs, budget)
    }
}

/// What a left press does: [`OfficeRenderer::press_at`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Resize,
    /// The pointer's: a click or a drag follows.
    Pointer,
    Drag,
}

/// How far, in logical px, a press may wander and still click: Android's
/// touch slop, "distance a touch can wander before we think the user is
/// scrolling" (`ViewConfiguration.TOUCH_SLOP`, 8 dips).
const DRAG_SLOP_DIP: f64 = 8.0;

/// What a press is made under, beside where: the display's scale factor,
/// the last petting, and the time.
#[derive(Debug, Clone, Copy)]
pub struct Pressing<'a> {
    pub scale_factor: f64,
    pub petting: Option<&'a pixtuoid_scene::pet::PetState>,
    pub now: std::time::SystemTime,
}

/// The layout unit a frame drawn at `at` shows at `cursor`, or `None` off
/// the office.
pub(super) fn unit_in(cursor: (f64, f64), at: PixelFit) -> Option<pixtuoid_scene::layout::Point> {
    let unit = unit_at(cursor, at);
    (cursor.0 >= 0.0 && cursor.1 >= 0.0 && unit.x < at.logical().w && unit.y < at.logical().h)
        .then_some(unit)
}

/// The layout unit a frame drawn at `at` shows at `cursor` (physical px).
fn unit_at(cursor: (f64, f64), at: PixelFit) -> pixtuoid_scene::layout::Point {
    let unit = |px: f64| {
        (px.max(0.0) as u32 / u32::from(at.scale().get().max(1))).min(u32::from(u16::MAX)) as u16
    };
    pixtuoid_scene::layout::Point {
        x: unit(cursor.0),
        y: unit(cursor.1),
    }
}

fn unit_bounds(unit: pixtuoid_scene::layout::Point) -> pixtuoid_scene::layout::Bounds {
    pixtuoid_scene::layout::Bounds {
        x: unit.x,
        y: unit.y,
        width: 1,
        height: 1,
    }
}

/// Window pixels from the bottom-right corner within which a press resizes.
const RESIZE_CORNER_PX: f64 = 18.0;

/// One floor's frame for the window: a [`RenderInputs`] whose office extent
/// the window's [`window_geometry`](super::geometry::window_geometry) owns.
#[derive(Debug, Clone, Copy)]
pub struct WindowFrame<'a> {
    pub world: FloorInputs<'a>,
    pub theme: &'static pixtuoid_scene::theme::Theme,
    pub place: pixtuoid_scene::look::Place,
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::super::fixtures::{active_on, cutaway, density, scene_with};
    use super::super::geometry::{Zoom, window_geometry};
    use super::super::overlays::footer_budget;
    use super::*;
    use pixtuoid_core::sprite::Rgb;
    use pixtuoid_scene::cutaway::Face;
    use pixtuoid_scene::floor::{FloorMeta, PetInputs};
    use winit::dpi::PhysicalSize;

    use pixtuoid_scene::layout::Size;
    use std::time::Duration;

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
        use crate::test_flash::{SLOW, storm_strike};
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
        let at = window_geometry(window, pack.max_density_variant(), Zoom::default());
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
        let centre =
            |u: u16| f64::from(u) * f64::from(at.scale().get()) + f64::from(at.scale().get()) / 2.0;
        let size = (window.width, window.height);
        let (mut hit_agent, mut dragged, mut fixture_drags, mut on_agent) =
            (false, false, false, None);
        for y in 0..at.logical().h {
            for x in 0..at.logical().w {
                let cursor = (centre(x), centre(y));
                match renderer.press_at(
                    cursor,
                    size,
                    at,
                    Pressing {
                        scale_factor: 1.0,
                        petting: None,
                        now,
                    },
                ) {
                    Press::Pointer => {
                        if let Some(HitAction::Focus(hit)) = renderer.release(cursor, at) {
                            assert_eq!(hit, id, "a click hit another agent");
                            hit_agent = true;
                            on_agent = Some(cursor);
                        }
                    }
                    Press::Drag => dragged = true,
                    Press::Resize => {}
                }
                if matches!(renderer.hit_at(cursor, at), Some(SceneHit::Furniture(_))) {
                    assert_eq!(
                        renderer.press_at(
                            cursor,
                            size,
                            at,
                            Pressing {
                                scale_factor: 1.0,
                                petting: None,
                                now
                            }
                        ),
                        Press::Drag,
                        "a fixture holds the window"
                    );
                    fixture_drags = true;
                }
            }
        }
        assert!(hit_agent, "no click found the agent the frame drew");
        assert!(dragged, "no bare unit to drag the window by");
        assert!(fixture_drags, "the frame drew no labelled fixture");
        let corner = (
            f64::from(window.width) - 1.0,
            f64::from(window.height) - 1.0,
        );
        assert_eq!(
            renderer.press_at(
                corner,
                size,
                at,
                Pressing {
                    scale_factor: 1.0,
                    petting: None,
                    now
                }
            ),
            Press::Resize
        );
        // A press on the agent that moves lifts it, carries it, and sets it
        // down on release, which clicks nothing.
        let on_agent = on_agent.expect("the agent's unit");
        assert_eq!(
            renderer.press_at(
                on_agent,
                size,
                at,
                Pressing {
                    scale_factor: 1.0,
                    petting: None,
                    now
                }
            ),
            Press::Pointer
        );
        let away = (on_agent.0 + 10.0 * f64::from(at.scale().get()), on_agent.1);
        assert!(renderer.pointer_moved(away, at), "the move lifts it");
        assert!(renderer.carrying());
        assert_eq!(renderer.release(away, at), None, "a drop clicks nothing");
        assert!(!renderer.carrying());
        // Under a panel a carry still sets down, and a click lands nowhere.
        let open = crate::panels::ModalState {
            help_open: true,
            ..closed_modal()
        };
        assert!(renderer.tooltip_shows(true, &closed_modal()));
        assert!(
            !renderer.tooltip_shows(true, &open),
            "no tooltip under a panel"
        );
        assert!(!renderer.tooltip_shows(false, &closed_modal()));
        let press = |renderer: &mut OfficeRenderer| {
            renderer.press_at(
                on_agent,
                size,
                at,
                Pressing {
                    scale_factor: 1.0,
                    petting: None,
                    now,
                },
            )
        };
        press(&mut renderer);
        assert!(renderer.pointer_moved(away, at));
        assert!(
            !renderer.tooltip_shows(true, &closed_modal()),
            "nor in hand"
        );
        assert_eq!(renderer.release_under(away, at, &open), None);
        assert!(
            !renderer.carrying(),
            "a panel opened mid-carry sets it down"
        );
        press(&mut renderer);
        assert_eq!(renderer.release_under(on_agent, at, &open), None);
        press(&mut renderer);
        assert!(
            renderer
                .release_under(on_agent, at, &closed_modal())
                .is_some(),
            "with no panel the click lands"
        );
        // A carry whose release goes elsewhere is set down when the window
        // loses focus, or at the next press.
        renderer.press_at(
            on_agent,
            size,
            at,
            Pressing {
                scale_factor: 1.0,
                petting: None,
                now,
            },
        );
        assert!(renderer.pointer_moved(away, at));
        assert!(
            renderer.cancel_pointer(),
            "focus lost mid-carry sets it down"
        );
        assert!(!renderer.carrying() && !renderer.cancel_pointer());
        renderer.press_at(
            on_agent,
            size,
            at,
            Pressing {
                scale_factor: 1.0,
                petting: None,
                now,
            },
        );
        assert!(renderer.pointer_moved(away, at));
        renderer.press_at(
            corner,
            size,
            at,
            Pressing {
                scale_factor: 1.0,
                petting: None,
                now,
            },
        );
        assert!(!renderer.carrying(), "the next press sets it down");
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
        let budget = footer_budget(960, Face::Screen.cell(1));
        let calm = renderer.footer(&scene, budget, true, None, None).text();
        let warned = renderer
            .footer(&scene, budget, true, None, warning.as_deref())
            .text();
        assert!(warned.contains("decode drift: cc·"), "{warned}");
        assert!(!calm.contains("decode drift"), "{calm}");
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

    fn closed_modal() -> crate::panels::ModalState {
        crate::panels::ModalState {
            onboarding_open: false,
            help_open: false,
            version_popup: false,
            theme_picker: None,
            dashboard_open: false,
            connection_open: false,
            connection_confirm: false,
            n_themes: pixtuoid_scene::theme::ALL_THEMES.len(),
        }
    }
}
