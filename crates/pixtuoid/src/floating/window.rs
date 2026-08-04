//! The `winit` + `softbuffer` window for `pixtuoid floating`.
//!
//! `FloatingApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface, renders the latest `watch`ed scene to a
//! DOWNSCALED office buffer, then nearest-neighbor upscales it into the surface so the
//! pixel-art office stays chunky/legible instead of 1:1-tiny.
//!
//! Platform glue — codecov-ignored; the testable seams are `floating::offscreen`
//! (render), `floating::geometry` (the window/monitor rect math), and
//! `floating::cadence` (the animation throttle).

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::DaemonLiveness;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{ResizeDirection, Window, WindowId, WindowLevel};

use super::offscreen::OfficeRenderer;
use crate::config::{self, FloatingConfig};
use pixtuoid_scene::floor::FloorMeta;
use pixtuoid_scene::theme::Theme;

/// Wake reasons delivered to the winit loop from the background tokio pipeline.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FloatingEvent {
    SceneChanged,
}

pub(crate) struct FloatingApp {
    cfg: FloatingConfig,
    theme: &'static Theme,
    pack: Pack,
    config_path: PathBuf,
    /// The configured office pets — one is selected per floor (v1 shows floor 0's).
    pets: Vec<pixtuoid_scene::pet::Pet>,
    renderer: OfficeRenderer,
    audio_ctl: crate::audio::AudioController,
    /// The pipeline inputs, held until `resumed` can supply the REAL window size
    /// (the `[floating]` config size is LOGICAL and would over-seed on HiDPI).
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
    /// The animation-tick deadline — see [`super::cadence`] for why the redraw
    /// REQUEST (not just the wait) has to be gated on it.
    clock: super::cadence::FrameClock,
    window: Option<Rc<Window>>,
    // softbuffer's `Context` must outlive the `Surface` it spawned, so keep both.
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

/// Click within this many physical px of the bottom-right corner = resize, else move.
const RESIZE_CORNER_PX: f64 = 18.0;

impl FloatingApp {
    #[allow(clippy::too_many_arguments)] // flat construction inputs; bundling adds no clarity
    pub(crate) fn new(
        cfg: FloatingConfig,
        theme: &'static Theme,
        pack: Pack,
        config_path: PathBuf,
        pets: Vec<pixtuoid_scene::pet::Pet>,
        boot: super::PipelineBoot,
        audio_muted: bool,
        audio_volume: f32,
    ) -> Self {
        // Built here, AFTER floating::run's fallible boot steps, so a boot
        // failure means no device thread ever existed and every later exit drops
        // `app` → the join runs. See `AudioController`.
        let audio_ctl =
            crate::audio::AudioController::new(audio_muted, audio_volume, config_path.clone());
        let mut renderer = OfficeRenderer::new();
        renderer.set_audio(audio_ctl.handle().clone());
        Self {
            cfg,
            theme,
            pack,
            config_path,
            pets,
            renderer,
            audio_ctl,
            boot: Some(boot),
            live: None,
            last_caps_size: None,
            cursor: PhysicalPosition::new(0.0, 0.0),
            clock: super::cadence::FrameClock::new(Instant::now()),
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
            tracing::warn!("pixtuoid floating: could not persist window geometry: {e}");
        }
    }

    fn redraw(&mut self) {
        // Clone the Rc to release the `self.window` borrow before touching `self.surface`.
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let (win_w, win_h) = (size.width, size.height);
        let (Some(nw), Some(nh)) = (NonZeroU32::new(win_w), NonZeroU32::new(win_h)) else {
            return; // a 0-area window: nothing to draw
        };
        // Cloned out so the `self.live` borrow ends before the `&mut self` writes below.
        let Some((scene, floor_caps)) = self
            .live
            .as_ref()
            .map(|l| (l.scene_rx.borrow().clone(), Arc::clone(&l.floor_caps)))
        else {
            return;
        };
        // Audio state for the footer's ♩ suffix, resolved BEFORE the surface
        // borrow below.
        let audio_now = Instant::now();
        self.audio_ctl.tick(audio_now);
        let audio_audible = self.audio_ctl.handle().is_audible();
        let volume_flash = self.audio_ctl.volume_flash(audio_now);
        // The ONE projection helper, shared with the boot seed so the two can't drift.
        let (scale, buf_w, buf_h) = super::offscreen::window_buffer_geometry(size);
        // Keep the reducer's desk capacity in lockstep with the office actually rendered at
        // this BUFFER size.
        super::offscreen::sync_floor_caps(&mut self.last_caps_size, &floor_caps, buf_w, buf_h);
        let floor_meta = FloorMeta::ground();
        let floor_pet =
            pixtuoid_scene::pet::select_pet_for_floor(floor_meta.floor_seed, &self.pets);
        let office = self.renderer.render(
            &scene,
            &self.pack,
            self.theme,
            SystemTime::now(),
            buf_w,
            buf_h,
            floor_meta,
            floor_pet,
        );
        let (ow, oh) = (office.width() as usize, office.height() as usize);
        let opx: Vec<u32> = office
            .as_slice()
            .iter()
            .map(|p| super::offscreen::pack_xrgb(*p))
            .collect();

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut sb) = surface.buffer_mut() else {
            return;
        };
        // Nearest-neighbor upscale. Source indices are clamped so the
        // integer-division remainder edge repeats the last office pixel.
        let (win_w, win_h, scale) = (win_w as usize, win_h as usize, scale as usize);
        if ow == 0 || oh == 0 || sb.len() < win_w * win_h {
            return; // nothing rendered / a transient resize race — skip this frame
        }
        for wy in 0..win_h {
            let src_row = (wy / scale).min(oh - 1) * ow;
            let dst_row = wy * win_w;
            for wx in 0..win_w {
                sb[dst_row + wx] = opx[src_row + (wx / scale).min(ow - 1)];
            }
        }
        // Name badges + the neon wall board, drawn POST-upscale at native surface
        // res so the text stays crisply anti-aliased.
        let labels = self.renderer.labels(&scene, SystemTime::now());
        super::offscreen::paint_labels_into_surface(
            &mut sb,
            win_w,
            win_h,
            &labels,
            scale as i32,
            self.theme,
        );
        let board = self.renderer.board(&scene, SystemTime::now());
        super::offscreen::paint_wall_board_into_surface(
            &mut sb,
            win_w,
            win_h,
            &board,
            scale as i32,
            self.theme,
        );
        let budget = super::offscreen::footer_budget(win_w);
        let footer = self
            .renderer
            .footer(&scene, budget, audio_audible, volume_flash);
        super::offscreen::paint_footer_into_surface(&mut sb, win_w, win_h, &footer, self.theme);
        window.pre_present_notify();
        let _ = sb.present();
    }
}

/// Does the saved window rect `(x, y, w, h)` overlap ANY currently-connected monitor?
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
        let mut attrs = Window::default_attributes()
            .with_title("pixtuoid")
            .with_decorations(false)
            .with_resizable(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(
                self.cfg.width as f64,
                self.cfg.height as f64,
            ))
            .with_min_inner_size(LogicalSize::new(
                config::FLOATING_MIN_W as f64,
                config::FLOATING_MIN_H as f64,
            ));
        // Restore the saved position ONLY if it still lands on a connected monitor;
        // else let the OS place it. A window last closed on a now-disconnected
        // monitor would otherwise restore fully off-screen and be unrecoverable
        // (frameless + no taskbar + always-on-top → no way to drag it back).
        if let (Some(x), Some(y)) = (self.cfg.x, self.cfg.y) {
            if position_on_a_monitor(event_loop, x, y, self.cfg.width, self.cfg.height) {
                attrs = attrs.with_position(PhysicalPosition::new(x, y));
            }
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
                tracing::error!("pixtuoid floating: failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("pixtuoid floating: failed to create softbuffer context: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("pixtuoid floating: failed to create softbuffer surface: {e}");
                event_loop.exit();
                return;
            }
        };
        // Seeded from the REAL window — the first physical size there is. Past
        // the window/surface failure arms, so a failed boot binds no socket.
        if let Some(boot) = self.boot.take() {
            self.live = Some(boot.spawn(window.inner_size()));
        }
        // `cfg.opacity` is parsed + clamped but NOT applied: winit 0.30 exposes no
        // per-window opacity, and softbuffer writes opaque XRGB (no alpha). Real
        // translucency needs a native shim or a wgpu surface.
        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: FloatingEvent) {
        match event {
            FloatingEvent::SceneChanged => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
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
                if let Some(action) = super::input::audio_action(&event.logical_key, event.repeat) {
                    // floating has no [p]ause; effective mute == muted.
                    self.audio_ctl
                        .apply(action, false, Instant::now(), crate::audio::respawn);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = position,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Frameless: a left-press drags the window, EXCEPT near the bottom-right
                // corner, which resizes. Errors are non-fatal — some platforms refuse
                // outside a real press.
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    let near_corner = super::geometry::near_resize_corner(
                        (self.cursor.x, self.cursor.y),
                        (size.width, size.height),
                        RESIZE_CORNER_PX,
                    );
                    let _ = if near_corner {
                        window.drag_resize_window(ResizeDirection::SouthEast)
                    } else {
                        window.drag_window()
                    };
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // An EMPTY office must NOT go fully idle: the time-driven ambient layer
        // (clock hands, weather, lightning, day/night, the wandering pet) still
        // advances, and a 0fps idle would freeze it into a dead-looking window.
        // A LIVE gateway daemon lives in `daemons`, not `agents`, and is a
        // time-driven WANDERING mascot, so it too holds the fast cadence.
        let office_idle = self.live.as_ref().is_none_or(|live| {
            let scene = live.scene_rx.borrow();
            scene.agents.is_empty()
                && scene
                    .daemons()
                    .all(|(_, _, d)| d.liveness == DaemonLiveness::Down)
        });
        // The redraw REQUEST rides the same deadline as the wait: requesting one
        // unconditionally here leaves winit a pending redraw, so `WaitUntil` never
        // sleeps and both cadences collapse to max-rate (see `super::cadence`).
        let (paint, deadline) = self.clock.poll(Instant::now(), office_idle);
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        if paint {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}
