//! The `winit` + `softbuffer` window for `pixtuoid floating`.
//!
//! `FloatingApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface; it renders the latest `watch`ed scene
//! to a DOWNSCALED office `RgbBuffer` via [`OfficeRenderer`] (~window/SCALE) then
//! nearest-neighbor upscales it into the surface (CPU, `0x00RRGGBB`) so the pixel-art
//! office stays chunky/legible instead of 1:1-tiny. Redraw is event-driven (a
//! `FloatingEvent::SceneChanged` from the pipeline
//! bridge) plus a ~30fps animation tick WHILE agents OR a live gateway daemon (the OpenClaw
//! lobster mascot in `scene.daemons`) are present (motion is time-driven); with no agents and
//! every daemon Down it drops to a slow ~1fps ambient tick (keeping the time-driven
//! clock/weather/lightning/day-night/pet alive without the 30fps cost), never fully idle.
//! Platform glue — codecov-ignored like `driver.rs`; the testable seams are
//! `floating::offscreen` (render) and `floating::geometry` (the window/monitor rect math
//! pulled out of here: off-screen-recovery overlap + the corner-resize hit-test).

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{DaemonLiveness, SceneState, MAX_FLOORS};
use tokio::sync::watch;
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
    /// The reducer published a new scene — repaint.
    SceneChanged,
}

/// The floating window app: window + surface (created lazily on `Resumed`), the office
/// renderer (owns cross-frame caches), the live scene receiver, and the per-floor desk
/// capacity atomics it keeps in sync with the rendered office.
pub(crate) struct FloatingApp {
    cfg: FloatingConfig,
    theme: &'static Theme,
    pack: Pack,
    config_path: PathBuf,
    /// The configured office pets — one is selected per floor (v1 shows floor 0's).
    pets: Vec<pixtuoid_scene::pet::Pet>,
    renderer: OfficeRenderer,
    /// The whole mute/volume persist protocol (#633 close-out) — the SAME
    /// `AudioController` the TUI owns (was `audio`/`volume_flash`/`volume_dirty`
    /// duplicated here). The renderer holds its own handle clone, re-synced by
    /// `set_audio_ui` + after every lazy spawn. Flash is VOLUME-only now (was
    /// every-gesture): a mute toggle shows no transient overlay until a footer
    /// lands to display it — the accepted TUI-parity tradeoff.
    audio_ctl: crate::audio::AudioController,
    scene_rx: watch::Receiver<Arc<SceneState>>,
    floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    /// The buffer size the capacity atomics were last synced for — capacity only changes
    /// with the window size, so re-sync only on a size change (not every frame).
    last_caps_size: Option<(u16, u16)>,
    /// Latest cursor position (physical px) — for the corner resize hit-test on click.
    cursor: PhysicalPosition<f64>,
    window: Option<Rc<Window>>,
    // softbuffer's `Context` must outlive the `Surface` it spawned, so keep both.
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

/// Click within this many physical px of the bottom-right corner = resize, else move.
const RESIZE_CORNER_PX: f64 = 18.0;

/// Animation tick rate WHILE agents are present — motion (walk/breathe) is time-driven.
/// `1000 / 30 = 33ms`, the prior fixed cadence.
const ACTIVE_FPS: u64 = 30;
/// Slow ambient tick when the office is EMPTY — keeps the time-driven ambient layer
/// (clock/weather/lightning/day-night/pet) moving without the 30fps cost of the active path.
const IDLE_AMBIENT_FPS: u64 = 1;

impl FloatingApp {
    #[allow(clippy::too_many_arguments)] // flat construction inputs; bundling adds no clarity
    pub(crate) fn new(
        cfg: FloatingConfig,
        theme: &'static Theme,
        pack: Pack,
        config_path: PathBuf,
        pets: Vec<pixtuoid_scene::pet::Pet>,
        scene_rx: watch::Receiver<Arc<SceneState>>,
        floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    ) -> Self {
        let audio_ctl = crate::audio::AudioController::new(
            crate::audio::AudioUi {
                handle: crate::audio::AudioHandle::disabled(),
                muted: true,
                volume: 1.0,
            },
            config_path.clone(),
        );
        Self {
            cfg,
            theme,
            pack,
            config_path,
            pets,
            renderer: OfficeRenderer::new(),
            audio_ctl,
            scene_rx,
            floor_caps,
            last_caps_size: None,
            cursor: PhysicalPosition::new(0.0, 0.0),
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

    /// Render the latest scene to a DOWNSCALED office buffer, then nearest-neighbor
    /// upscale it into the window. The pixel-art office is tiny at 1:1 (8×12 sprites),
    /// so a native blit looks sparse + miniature; rendering at ~1/SCALE and blowing it
    /// back up keeps the sprites chunky + legible, like the TUI's half-block view.
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
        // Audio state for the footer's ♩ suffix + the expiry-driven debounced
        // volume persist, both owned by the controller — resolved BEFORE the
        // surface borrow below. `audio_audible` mirrors the TUI's gate EXACTLY
        // (`audio_audible` in tui_renderer): a LIVE handle AND not muted AND a
        // live level — so an opted-in-but-dead-device handle (no sink / audio
        // feature off → `AudioHandle::disabled`) shows no phantom ♩, matching the
        // TUI. `volume_flash` drives the transient `♩ N%` beat.
        let audio_now = Instant::now();
        self.audio_ctl.tick(audio_now);
        let audio_audible = self.audio_ctl.handle().is_enabled()
            && !self.audio_ctl.muted()
            && self.audio_ctl.volume() > 0.0;
        let volume_flash = self.audio_ctl.volume_flash(audio_now);
        // Office buffer = window / SCALE (kept ~OFFICE_TARGET_H tall → chunky sprites).
        // The ONE projection helper, shared with the boot seed so the two can't drift.
        let (scale, buf_w, buf_h) = super::offscreen::window_buffer_geometry(win_w, win_h);
        // Keep the reducer's desk capacity in lockstep with the office actually rendered at
        // this BUFFER size (authority = the layout's home-desk count, same as the TUI).
        if self.last_caps_size != Some((buf_w, buf_h)) {
            sync_floor_caps(&self.floor_caps, buf_w, buf_h);
            self.last_caps_size = Some((buf_w, buf_h));
        }
        // Arc clone releases the watch borrow before the (mutable) renderer borrow.
        let scene = self.scene_rx.borrow().clone();
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
        // Collect office pixels (release the `self.renderer` borrow) as `0x00RRGGBB`.
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
        // Nearest-neighbor upscale opx (ow×oh) → the window (win_w×win_h). Source indices
        // are clamped so the integer-division remainder edge repeats the last office pixel.
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
        // Name badges + the neon wall board, drawn POST-upscale at native surface res
        // (crisp anti-aliased Monaspace Neon) using the same layout/route state the office
        // pass just used. Badges are a fixed caption height; the board scales with the panel.
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
        // The status footer (full TUI parity) as a bottom-overlay band — carries
        // the ♩/♩N% audio suffix the standalone volume flash used to, plus the
        // office stats/rungs/tools/gateway. `win_w`/`win_h` are usize surface dims.
        let budget = super::offscreen::footer_budget(win_w);
        let footer = self
            .renderer
            .footer(&scene, budget, audio_audible, volume_flash);
        super::offscreen::paint_footer_into_surface(&mut sb, win_w, win_h, &footer, self.theme);
        window.pre_present_notify();
        let _ = sb.present();
    }
}

/// Sync the per-floor desk-capacity atomics to the office layout at `buf_w`×`buf_h` —
/// the authority is the layout's `home_desks` count (mirrors the TUI's per-frame sync,
/// `tui/mod.rs`). `store` (not `fetch_max`): floating tracks its window exactly, so a shrink
/// lowers capacity (excess agents become invisible-but-alive, like the TUI on shrink).
fn sync_floor_caps(floor_caps: &[AtomicUsize; MAX_FLOORS], buf_w: u16, buf_h: u16) {
    for (floor_idx, cap) in floor_caps.iter().enumerate() {
        let seed = pixtuoid_scene::floor::floor_seed(floor_idx);
        let capacity = pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed);
        cap.store(capacity, Ordering::Relaxed);
    }
}

/// Does the saved window rect `(x, y, w, h)` overlap ANY currently-connected monitor?
/// Thin winit binding over the pure [`super::geometry::window_visible_on_monitors`] (the
/// overlap logic + empty-list guard is unit-tested there; this just pulls the monitor rects).
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
        // Restore the saved position (physical px) ONLY if it still lands on a currently
        // connected monitor; else let the OS place it. A window last closed on a now-
        // disconnected monitor would otherwise restore fully off-screen and be
        // unrecoverable (frameless + no taskbar + always-on-top → no way to drag it back).
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
        // `cfg.opacity` is parsed + clamped but NOT applied in v1: winit 0.30 exposes no
        // per-window opacity, and softbuffer writes opaque XRGB (no alpha). Honest no-op —
        // real translucency needs a native shim or a wgpu surface (deferred, see spec §11).
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
                self.audio_ctl.flush_on_exit();
                self.persist_geometry();
                event_loop.exit();
            }
            // `is_synthetic: false`: winit fabricates a Pressed for every key
            // physically held when the window GAINS FOCUS (X11 + Windows). A
            // muted user holding `+`/`m` who clicks in would otherwise be
            // spuriously unmuted AND have it persisted (volume-up is the
            // un-mute gesture) — the focus-gain-replay twin of the TUI's
            // Windows Press/Release guard (should_dispatch_key).
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                if let Some(action) = super::input::audio_action(&event.logical_key, event.repeat) {
                    // floating has no [p]ause; effective mute == muted. The
                    // controller persists mute NOW + debounces the volume + arms
                    // the (volume-only) readout.
                    self.audio_ctl
                        .apply(action, false, Instant::now(), crate::audio::spawn);
                    // a lazy spawn mints a NEW handle — reinstall so the
                    // renderer's frame feed reaches the live thread
                    self.renderer.set_audio(self.audio_ctl.handle().clone());
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
                // corner, which resizes (the OS takes over until release). Errors are
                // non-fatal (some platforms refuse outside a real press).
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
        // Agents animate continuously (walk/breathe — time-driven), so tick ~30fps WHILE
        // any agent is present. When the office is EMPTY we don't go fully idle: the
        // time-driven AMBIENT layer (clock hands, weather cycle, lightning, day/night
        // lighting, the wandering pet) still advances, so a 0fps idle would freeze it and
        // an empty-office window would look dead/broken. Drop to a slow ~1fps ambient tick
        // instead — enough to keep the office alive while preserving the CPU-saving intent
        // (nowhere near the 30fps agents-present path). A LIVE gateway daemon (the OpenClaw
        // lobster) lives in `daemons`, not `agents`, and is a time-driven WANDERING mascot
        // — not slow ambient decor — so it keeps the 30fps path unless every daemon is Down
        // (a Down daemon is gone/leaving within MASCOT_LEAVE_MS, not a sustained wanderer, so
        // it stays on the ambient tick — same brief terminal transition as before this change).
        let scene = self.scene_rx.borrow();
        let office_idle = scene.agents.is_empty()
            && scene
                .daemons()
                .values()
                .all(|d| d.liveness == DaemonLiveness::Down);
        let next_tick = if office_idle {
            Duration::from_millis(1000 / IDLE_AMBIENT_FPS)
        } else {
            Duration::from_millis(1000 / ACTIVE_FPS)
        };
        drop(scene);
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + next_tick));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl FloatingApp {
    /// Install the ambient-audio state (#633): the renderer takes a handle
    /// clone (the per-frame feed), the app keeps the handle/muted/volume trio
    /// the m/+/- keys drive.
    pub(crate) fn set_audio_ui(&mut self, audio: crate::audio::AudioUi) {
        self.audio_ctl = crate::audio::AudioController::new(audio, self.config_path.clone());
        self.renderer.set_audio(self.audio_ctl.handle().clone());
    }
}
