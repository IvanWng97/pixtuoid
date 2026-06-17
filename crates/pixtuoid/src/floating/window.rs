//! The `winit` + `softbuffer` window for `pixtuoid floating`.
//!
//! `FloatingApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface; it renders the latest `watch`ed scene
//! to a DOWNSCALED office `RgbBuffer` via [`OfficeRenderer`] (~window/SCALE) then
//! nearest-neighbor upscales it into the surface (CPU, `0x00RRGGBB`) so the pixel-art
//! office stays chunky/legible instead of 1:1-tiny. Redraw is event-driven (a
//! `FloatingEvent::SceneChanged` from the pipeline
//! bridge) plus a ~30fps animation tick WHILE agents are present (motion is time-driven);
//! it idles to zero frames when the office is empty. Platform glue — codecov-ignored like
//! `driver.rs`; the testable render seam is `floating::offscreen`.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{SceneState, MAX_FLOORS};
use tokio::sync::watch;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{ResizeDirection, Window, WindowId, WindowLevel};

use super::offscreen::OfficeRenderer;
use crate::config::{self, FloatingConfig};
use crate::scene::floor::FloorMeta;
use crate::scene::theme::Theme;

/// Wake reasons delivered to the winit loop from the background tokio pipeline.
#[derive(Debug, Clone, Copy)]
pub enum FloatingEvent {
    /// The reducer published a new scene — repaint.
    SceneChanged,
}

/// The floating window app: window + surface (created lazily on `Resumed`), the office
/// renderer (owns cross-frame caches), the live scene receiver, and the per-floor desk
/// capacity atomics it keeps in sync with the rendered office.
pub struct FloatingApp {
    cfg: FloatingConfig,
    theme: &'static Theme,
    pack: Pack,
    config_path: PathBuf,
    /// The configured office pets — one is selected per floor (v1 shows floor 0's).
    pets: Vec<crate::scene::pet::Pet>,
    renderer: OfficeRenderer,
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

impl FloatingApp {
    #[allow(clippy::too_many_arguments)] // flat construction inputs; bundling adds no clarity
    pub fn new(
        cfg: FloatingConfig,
        theme: &'static Theme,
        pack: Pack,
        config_path: PathBuf,
        pets: Vec<crate::scene::pet::Pet>,
        scene_rx: watch::Receiver<Arc<SceneState>>,
        floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    ) -> Self {
        Self {
            cfg,
            theme,
            pack,
            config_path,
            pets,
            renderer: OfficeRenderer::new(),
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
        // Office buffer = window / SCALE (kept ~OFFICE_TARGET_H tall → chunky sprites).
        let scale = super::offscreen::office_scale(win_h);
        let buf_w = (win_w / scale).clamp(1, u16::MAX as u32) as u16;
        let buf_h = (win_h / scale).clamp(1, u16::MAX as u32) as u16;
        // Keep the reducer's desk capacity in lockstep with the office actually rendered at
        // this BUFFER size (authority = the layout's home-desk count, same as the TUI).
        if self.last_caps_size != Some((buf_w, buf_h)) {
            sync_floor_caps(&self.floor_caps, buf_w, buf_h);
            self.last_caps_size = Some((buf_w, buf_h));
        }
        // Arc clone releases the watch borrow before the (mutable) renderer borrow.
        let scene = self.scene_rx.borrow().clone();
        let floor_meta = FloorMeta::ground();
        let floor_pet = crate::scene::pet::select_pet_for_floor(floor_meta.floor_seed, &self.pets);
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
        let (ow, oh) = (office.width as usize, office.height as usize);
        let opx: Vec<u32> = office
            .pixels
            .iter()
            .map(|p| (p.r as u32) << 16 | (p.g as u32) << 8 | p.b as u32)
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
        // Name badges, drawn POST-upscale at native surface res (crisp 8px text proportional
        // to the chunky sprites) using the same layout/route state the office pass just used.
        let labels = self.renderer.labels(&scene, SystemTime::now());
        super::offscreen::paint_labels_into_surface(
            &mut sb,
            win_w,
            win_h,
            &labels,
            scale as i32,
            self.theme,
        );
        window.pre_present_notify();
        let _ = sb.present();
    }
}

/// Sync the per-floor desk-capacity atomics to the office layout at `buf_w`×`buf_h` —
/// the authority is the layout's `home_desks` count (mirrors the TUI's per-frame sync,
/// `tui/mod.rs`). `store` (not `fetch_max`): floating tracks its window exactly, so a shrink
/// lowers capacity (excess agents become invisible-but-alive, like the TUI on shrink).
fn sync_floor_caps(floor_caps: &[AtomicUsize; MAX_FLOORS], buf_w: u16, buf_h: u16) {
    use pixtuoid_core::layout::{SceneLayout, MAX_VISIBLE_DESKS};
    for (floor_idx, cap) in floor_caps.iter().enumerate() {
        let seed = (floor_idx as u64).wrapping_mul(crate::scene::floor::FLOOR_SEED_MULTIPLIER);
        let capacity = SceneLayout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, seed)
            .map(|l| l.home_desks.len())
            .unwrap_or(0);
        cap.store(capacity, Ordering::Relaxed);
    }
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
        // Restore the saved position (physical px); else the OS places it.
        if let (Some(x), Some(y)) = (self.cfg.x, self.cfg.y) {
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
                self.persist_geometry();
                event_loop.exit();
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
                    let near_corner = self.cursor.x >= size.width as f64 - RESIZE_CORNER_PX
                        && self.cursor.y >= size.height as f64 - RESIZE_CORNER_PX;
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
        // any agent is present; idle to zero frames (event-driven only) when empty.
        let animating = !self.scene_rx.borrow().agents.is_empty();
        if animating {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(33),
            ));
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
