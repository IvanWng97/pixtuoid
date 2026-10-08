//! The `winit` + `softbuffer` window for `pixtuoid floating`.
//!
//! `FloatingApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface, renders the latest `watch`ed scene's
//! cutaway at the pack's densest art, then upscales it a whole number of times into the
//! surface ([`super::offscreen::window_geometry`]).
//!
//! Platform glue — codecov-ignored; the testable seams are `floating::offscreen`
//! (render), `floating::geometry` (the window/monitor rect math), and
//! `floating::cadence` (the animation throttle).

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use pixtuoid_core::state::DaemonLiveness;
use pixtuoid_scene::pack::OfficeArt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{ResizeDirection, Window, WindowId, WindowLevel};

use super::offscreen::{OfficeRenderer, WindowFrame};
use crate::config::{self, FloatingConfig};
use pixtuoid_scene::cutaway::Face;
use pixtuoid_scene::floor::{FloorInputs, FloorMeta, PetInputs};
use pixtuoid_scene::look::Place;
use pixtuoid_scene::theme::Theme;

/// Wake reasons delivered to the winit loop from the background tokio pipeline.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FloatingEvent {
    SceneChanged,
}

pub(crate) struct FloatingApp {
    cfg: FloatingConfig,
    theme: &'static Theme,
    pack: std::sync::Arc<OfficeArt>,
    config_path: PathBuf,
    /// The panels, the `[p]ause` and the theme picker, as the TUI holds them.
    ui: crate::panels::ui_state::UiState,
    /// The Sources panel's mutation seam, as the TUI's.
    connected: crate::runtime::ConnectedSources,
    /// The modifier keys held, which a key is read with.
    modifiers: winit::keyboard::ModifiersState,
    /// What the frame on screen shows beside the office.
    screen: super::offscreen::Screen,
    /// The frames' times and janks, reported as the TUI's are.
    jank: crate::jank::Jank,
    /// How the office moves.
    motion: pixtuoid_scene::anim::Motion,
    renderer: OfficeRenderer,
    audio_ctl: crate::audio::AudioController,
    /// The pipeline inputs, held until `resumed` can supply the REAL window size
    /// (the `[floating]` config size is LOGICAL and would mis-seed on HiDPI).
    /// `take`n exactly once; `None` afterwards.
    boot: Option<super::PipelineBoot>,
    /// The live pipeline — `None` until `resumed` boots it. `about_to_wait` DOES
    /// fire before then, so it reads `None` as an idle office. `redraw` cannot
    /// reach that state because `resumed` sets `live` BEFORE `window` and
    /// `redraw`'s window guard runs first. Don't reorder those assignments.
    live: Option<super::LivePipeline>,
    /// The buffer size the capacity atomics were last synced for — capacity only changes
    /// with the window size, so re-sync only on a size change (not every frame).
    last_caps_size: Option<(u16, u16)>,
    /// Latest cursor position (physical px) — for the corner resize hit-test on click.
    cursor: PhysicalPosition<f64>,
    /// The geometry of the frame on screen, which a press maps back through.
    shown: Option<pixtuoid_scene::render_scale::PixelFit>,
    /// Whether the pointer is over the window, where a hover shows its tooltip.
    cursor_in: bool,
    /// Whether the pointer last hovered something, so a move off it redraws
    /// once to drop its tooltip.
    hovered: bool,
    /// The last petting, played while it lasts.
    petting: Option<pixtuoid_scene::pet::PetState>,
    /// Where a clicked agent's transcript roots are, for its focus jump:
    /// (CC projects root, Codex sessions root).
    focus_roots: (Option<PathBuf>, Option<PathBuf>),
    /// The animation-tick deadline — see [`super::cadence`] for why the redraw
    /// REQUEST (not just the wait) has to be gated on it.
    clock: super::cadence::FrameClock,
    window: Option<Rc<Window>>,
    // softbuffer's `Context` must outlive the `Surface` it spawned, so keep both.
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

/// What the run keeps beside the office: the config it persists to, its
/// audio, the log the Sources panel's drift history comes from, and whether
/// this is the first run, which opens the onboarding.
pub(crate) struct Settings {
    pub(crate) config_path: PathBuf,
    pub(crate) audio: config::AudioConfig,
    pub(crate) log: Option<crate::run_log::LogLocation>,
    pub(crate) first_run: bool,
}

/// How the office looks and moves.
pub(crate) struct Appearance {
    pub(crate) theme: &'static Theme,
    pub(crate) motion: pixtuoid_scene::anim::Motion,
}

impl FloatingApp {
    pub(crate) fn new(
        cfg: FloatingConfig,
        Appearance { theme, motion }: Appearance,
        pack: OfficeArt,
        pets: Vec<pixtuoid_scene::pet::Pet>,
        Settings {
            config_path,
            audio,
            log,
            first_run,
        }: Settings,
        boot: super::PipelineBoot,
    ) -> Self {
        let audio_ctl = crate::audio::AudioController::new(audio, config_path.clone());
        let pack = std::sync::Arc::new(pack);
        let mut renderer = OfficeRenderer::new(std::sync::Arc::clone(&pack));
        renderer.set_audio(audio_ctl.handle().clone());
        renderer.set_pets(pets);
        let focus_roots = boot.focus_roots();
        let ui = crate::panels::ui_state::UiState::boot(
            theme,
            first_run,
            &config_path,
            boot.socket_path.clone(),
            log,
            boot.drift.clone(),
        );
        let connected = boot.connected.clone();
        Self {
            cfg,
            theme,
            pack,
            config_path,
            ui,
            connected,
            modifiers: winit::keyboard::ModifiersState::empty(),
            // Until `resumed` names the platform: presenting every frame is
            // safe on any.
            screen: super::offscreen::Screen::new(false),
            jank: crate::jank::Jank::new(Instant::now()),
            motion,
            renderer,
            audio_ctl,
            boot: Some(boot),
            live: None,
            last_caps_size: None,
            cursor: PhysicalPosition::new(0.0, 0.0),
            shown: None,
            cursor_in: false,
            hovered: false,
            petting: None,
            focus_roots,
            clock: super::cadence::FrameClock::new(Instant::now(), motion),
            window: None,
            context: None,
            surface: None,
        }
    }

    /// Persist the current window geometry into `[floating]` (best-effort — a save error
    /// must not block quitting). Size is stored LOGICAL (HiDPI-stable); position PHYSICAL.
    fn persist_geometry(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let logical = window.inner_size().to_logical::<f64>(window.scale_factor());
        let pos = window.outer_position().ok();
        if let Err(e) = config::save_floating(
            &self.config_path,
            logical.width.round() as u32,
            logical.height.round() as u32,
            pos.map(|p| p.x),
            pos.map(|p| p.y),
        ) {
            tracing::warn!(error = ?e, "pixtuoid floating: could not persist window geometry");
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Redraw when the pointer hovers something, or stops, so its tooltip
    /// follows within a move rather than at the next paint tick, which an
    /// idle office spaces a second apart.
    fn rehover(&mut self) {
        let hovered = self
            .shown
            .filter(|_| self.cursor_in)
            .and_then(|at| self.renderer.hit_at((self.cursor.x, self.cursor.y), at))
            .is_some();
        // A shown tooltip follows the pointer, so it redraws on every move.
        if hovered || self.hovered {
            self.hovered = hovered;
            self.request_redraw();
        }
    }

    /// A left press: the open panels' first, as the TUI's ladder
    /// ([`crate::panels::modal_mouse`]); then resize from the corner, hand the
    /// pointer what the frame on screen shows under it (a click or a drag
    /// follows), or drag the frameless window. Errors are non-fatal — some
    /// platforms refuse a drag outside a real press.
    fn press(&mut self) {
        use super::offscreen::Press;
        use crate::panels::ModalMouse;
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let cursor = (self.cursor.x, self.cursor.y);
        match super::offscreen::modal_press(
            &mut self.ui,
            cursor,
            (size.width, size.height),
            self.shown,
        ) {
            ModalMouse::Office => {}
            // A panel the press did nothing in leaves the window to drag.
            ModalMouse::Inert => {
                let _ = window.drag_window();
                return;
            }
            ModalMouse::Took => {
                window.request_redraw();
                return;
            }
        }
        let now = self.ui.now();
        let press = match self.shown {
            Some(at) => self.renderer.press_at(
                cursor,
                (size.width, size.height),
                at,
                super::offscreen::Pressing {
                    scale_factor: window.scale_factor(),
                    petting: self.petting.as_ref(),
                    now,
                },
            ),
            None => Press::Drag,
        };
        match press {
            Press::Resize => {
                let _ = window.drag_resize_window(ResizeDirection::SouthEast);
            }
            Press::Drag => {
                let _ = window.drag_window();
            }
            Press::Pointer => {}
        }
    }

    /// The left button released: carry out a click as the TUI's does, or set
    /// a carried figure down, even one a panel opened over mid-carry.
    fn release(&mut self) {
        use pixtuoid_scene::hit::HitAction;
        let Some(at) = self.shown else {
            return;
        };
        let action =
            self.renderer
                .release_under((self.cursor.x, self.cursor.y), at, &self.ui.modal());
        let now = self.ui.now();
        match action {
            Some(HitAction::Focus(id)) => {
                let slot = self
                    .live
                    .as_ref()
                    .and_then(|l| l.scene_rx.borrow().agents.get(&id).cloned());
                if let Some(slot) = slot {
                    crate::focus::focus_slot(&slot, &self.focus_roots);
                }
            }
            Some(HitAction::Pet(kind)) => {
                self.petting = Some(pixtuoid_scene::pet::PetState {
                    petted_at: now,
                    kind,
                    floor_idx: self.renderer.nav().current(),
                });
            }
            Some(HitAction::Open(url)) => {
                let _ = open::that(url);
            }
            None => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// A pressed key, through the TUI's dispatch and its panels; whether it
    /// asked to quit.
    fn key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some(key) = super::input::key(&event.logical_key, self.modifiers, event.repeat) else {
            return false;
        };
        let Some(snapshot) = self.live.as_ref().map(|l| Arc::clone(&l.scene_rx.borrow())) else {
            return false;
        };
        let now = self.ui.now();
        super::offscreen::press_key(
            key,
            &mut crate::panels::KeyCtx {
                ui: &mut self.ui,
                host: &mut super::offscreen::WindowHost {
                    renderer: &mut self.renderer,
                    theme: &mut self.theme,
                    screen: &mut self.screen,
                },
                audio_ctl: &mut self.audio_ctl,
                config_path: &self.config_path,
                connected: &self.connected,
                snapshot: &snapshot,
                focus_roots: &self.focus_roots,
                now,
                respawn: crate::audio::respawn,
            },
        )
    }

    fn redraw(&mut self) {
        let started = Instant::now();
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        let (win_w, win_h) = (size.width, size.height);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(win_w), NonZeroU32::new(win_h)) else {
            return; // a 0-area window: nothing to draw
        };
        // Cloned out: a held `watch::Ref` read-locks the channel, stalling the sender.
        let Some((scene, floor_caps)) = self
            .live
            .as_ref()
            .map(|l| (l.scene_rx.borrow().clone(), Arc::clone(&l.floor_caps)))
        else {
            return;
        };
        let audio_now = Instant::now();
        self.audio_ctl.tick(audio_now);
        let audio_audible = self.audio_ctl.handle().is_audible();
        let volume_flash = self.audio_ctl.volume_flash(audio_now);
        let at = super::offscreen::window_geometry(size, self.pack.max_density_variant());
        super::offscreen::sync_floor_caps(
            &mut self.last_caps_size,
            &floor_caps,
            at.logical().w,
            at.logical().h,
        );
        // The office's weather and motion; the office picks each floor's own.
        let floor_meta = FloorMeta::ground().with_motion(self.motion);
        // ONE clock read, so the overlays below annotate the frame actually rendered.
        let now = self.ui.now();
        let world = FloorInputs {
            scene: &scene,
            pack: &self.pack,
            now,
            floor: floor_meta,
            pets: PetInputs {
                pet: None,
                petting: self.petting.as_ref(),
            },
        };
        let live = self.renderer.render_live(
            at,
            WindowFrame {
                world,
                theme: self.theme,
                place: Place {
                    gateway: pixtuoid_scene::tally::office_gateway(&scene),
                    floor: None,
                },
            },
            (win_w, win_h),
        );
        self.shown = Some(at);
        if !live
            || self
                .renderer
                .buf()
                .is_none_or(|o| o.width() == 0 || o.height() == 0)
        {
            // Nothing rendered, or held: the window keeps the last frame, which
            // the next one's dirt is no longer measured against.
            self.screen.stale();
            return;
        }
        let cursor = (self.cursor.x, self.cursor.y);
        let next = super::offscreen::Overlays {
            window: (win_w, win_h),
            footer: self.renderer.footer(
                &scene,
                super::offscreen::footer_budget(win_w as usize, Face::chrome(at)),
                audio_audible,
                volume_flash,
                self.live
                    .as_ref()
                    .and_then(super::LivePipeline::footer_warning)
                    .as_deref(),
            ),
            tooltip: self
                .renderer
                .tooltip_shows(self.cursor_in, &self.ui.modal())
                .then(|| self.renderer.hit_at(cursor, at))
                .flatten()
                .and_then(|hit| {
                    // The office picks each floor's pet as it renders; the
                    // tooltip names the one the floor showing drew.
                    let shown = FloorInputs {
                        pets: PetInputs {
                            pet: self.renderer.showing_pet(),
                            ..world.pets
                        },
                        ..world
                    };
                    pixtuoid_scene::tooltip::for_hit(hit, &shown)
                })
                .map(|tip| (tip, (cursor.0 as i32, cursor.1 as i32))),
            panels: {
                let health = self
                    .live
                    .as_ref()
                    .map(super::LivePipeline::health)
                    .unwrap_or_default();
                let frames = self.ui.build_frames(now, &scene, &health);
                super::offscreen::panels_grid(
                    &frames,
                    super::offscreen::panel_cells((win_w, win_h), Face::chrome(at)),
                    now,
                    self.theme,
                )
            },
        };
        let dirty = self.renderer.dirty();
        let painted = crate::jank::Painted::from(dirty);
        if !self.screen.needs(&next, dirty) {
            // The frame on screen is this one: its flash phase shows.
            self.renderer.presented();
            return;
        }
        // Until this one presents, the screen matches no frame rendered.
        self.screen.stale();
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut sb) = surface.buffer_mut() else {
            return;
        };
        let (win_w, win_h) = (win_w as usize, win_h as usize);
        let Some(mut surf) = super::offscreen::XrgbSurface::new(&mut sb, win_w, win_h) else {
            return;
        };
        if let Some(office) = self.renderer.buf() {
            surf.fill_upscaled(office, usize::from(at.upscale()));
        }
        let cell = Face::chrome(at);
        let look = (self.theme, &*self.pack);
        super::offscreen::paint_footer_into_surface(&mut surf, &next.footer, look, at);
        if let Some((tip, _)) = &next.tooltip {
            super::offscreen::paint_tooltip_into_surface(&mut surf, tip, cursor, look, cell);
        }
        if let Some(panels) = &next.panels {
            super::offscreen::paint_panels_into_surface(&mut surf, panels, look, cell);
        }
        window.pre_present_notify();
        let presenting = Instant::now();
        if sb.present().is_ok() {
            self.renderer.presented();
            self.screen.shown(next);
        }
        let now = Instant::now();
        self.jank.painted_by(crate::jank::Painter {
            look: "floating",
            scale: at.scale().get(),
            ..crate::jank::Painter::default()
        });
        self.jank.record(
            now - started,
            now - presenting,
            Some(crate::jank::FrameSend {
                dirty: painted,
                ..crate::jank::FrameSend::default()
            }),
            None,
            now,
        );
    }
}

fn position_on_a_monitor(event_loop: &ActiveEventLoop, x: i32, y: i32, w: u32, h: u32) -> bool {
    super::geometry::window_visible_on_monitors(
        (x, y, w, h),
        event_loop.available_monitors().map(|m| {
            let (pos, size) = (m.position(), m.size());
            (pos.x, pos.y, size.width, size.height)
        }),
    )
}

impl ApplicationHandler<FloatingEvent> for FloatingApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already created — a re-resume must not spawn a second window
        }
        let min = super::offscreen::min_window(self.pack.max_density_variant());
        let mut attrs = Window::default_attributes()
            .with_title("pixtuoid")
            .with_decorations(false)
            .with_resizable(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(self.cfg.width, self.cfg.height))
            .with_min_inner_size(min);
        // A spot on a since-disconnected monitor would open the frameless window unreachably.
        if let (Some(x), Some(y)) = (self.cfg.x, self.cfg.y)
            && position_on_a_monitor(event_loop, x, y, self.cfg.width, self.cfg.height)
        {
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_has_shadow(true).with_titlebar_hidden(true);
        }
        #[cfg(target_os = "windows")]
        {
            // No taskbar button — it's an ambient overlay, not a primary window.
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_skip_taskbar(true);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                tracing::error!(error = %e, "pixtuoid floating: failed to create window");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "pixtuoid floating: failed to create softbuffer context");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "pixtuoid floating: failed to create softbuffer surface");
                event_loop.exit();
                return;
            }
        };
        // Seeded from the REAL window — the first PHYSICAL size there is, never the
        // logical config size (#803: buffer size is not monotone in scale factor).
        // Past the window/surface failure arms, so a failed boot binds no socket.
        // Pinned by `the_boot_seed_tracks_the_physical_window_not_the_logical_config`.
        if let Some(boot) = self.boot.take() {
            self.live = Some(boot.spawn(window.inner_size(), self.pack.max_density_variant()));
        }
        // `cfg.opacity` is parsed + clamped but NOT applied: winit 0.30 exposes no
        // per-window opacity, and softbuffer writes opaque XRGB (no alpha). Real
        // translucency needs a native shim or a wgpu surface.
        self.screen = super::offscreen::Screen::of_window(
            winit::raw_window_handle::HasWindowHandle::window_handle(&*window)
                .ok()
                .map(|h| h.as_raw()),
        );
        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: FloatingEvent) {
        match event {
            FloatingEvent::SceneChanged => self.request_redraw(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                // Geometry MUST persist HERE — the window is gone once `run_app`
                // returns.
                self.persist_geometry();
                // A run shorter than a summary's window still reports its spread.
                self.jank.finish();
                event_loop.exit();
            }
            // `is_synthetic: false`: winit fabricates a Pressed for every key
            // physically held when the window GAINS FOCUS (X11 + Windows). A
            // muted user holding `+`/`m` who clicks in would otherwise be
            // spuriously unmuted AND have it persisted.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                if self.key(&event) {
                    self.persist_geometry();
                    self.jank.finish();
                    event_loop.exit();
                    return;
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => self.request_redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.cursor_in = true;
                let carried = self
                    .shown
                    .is_some_and(|at| self.renderer.pointer_moved((position.x, position.y), at));
                if carried && let Some(window) = &self.window {
                    window.request_redraw();
                }
                self.rehover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_in = false;
                self.rehover();
            }
            // Its release goes elsewhere now: a figure in hand is set down.
            WindowEvent::Focused(false) => {
                if self.renderer.cancel_pointer()
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.press(),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.release(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // An EMPTY office still steps on its beat (clock hands, weather,
        // lightning, day/night), so it paints each beat, never 0fps. A LIVE
        // gateway daemon lives in `daemons`, not `agents`, and is a WANDERING
        // mascot, so it holds the fast cadence, as does whatever the floor
        // reports moving between beats: a walk, or a light mid-fade.
        let office_idle = self.live.as_ref().is_none_or(|live| {
            let scene = live.scene_rx.borrow();
            scene.agents.is_empty()
                && scene
                    .daemons()
                    .all(|(_, _, d)| d.liveness == DaemonLiveness::Down)
        }) && !self.renderer.moves_off_beat()
            && self.renderer.nav().transition().is_none();
        // The redraw REQUEST rides the same deadline as the wait: requesting one
        // unconditionally here leaves winit a pending redraw, so `WaitUntil` never
        // sleeps and both cadences collapse to max-rate (see `super::cadence`).
        let (paint, deadline) = self
            .clock
            .poll(Instant::now(), SystemTime::now(), office_idle);
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        if let Some(due) = paint {
            self.jank.due_at(due);
            self.request_redraw();
        }
    }
}
