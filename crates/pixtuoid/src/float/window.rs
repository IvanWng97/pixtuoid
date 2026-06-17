//! The `winit` + `softbuffer` window for `pixtuoid float`.
//!
//! `FloatApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface; it renders the latest `watch`ed scene
//! to a full-resolution office `RgbBuffer` via [`OfficeRenderer`] and blits it (CPU,
//! `0x00RRGGBB`). Redraw is event-driven (a `FloatEvent::SceneChanged` from the pipeline
//! bridge) plus a ~30fps animation tick WHILE agents are present (motion is time-driven);
//! it idles to zero frames when the office is empty. Platform glue — codecov-ignored like
//! `driver.rs`; the testable render seam is `tui::offscreen`.

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

use crate::config::{self, FloatConfig};
use crate::tui::floor::FloorMeta;
use crate::tui::offscreen::OfficeRenderer;
use crate::tui::theme::Theme;

/// Wake reasons delivered to the winit loop from the background tokio pipeline.
#[derive(Debug, Clone, Copy)]
pub enum FloatEvent {
    /// The reducer published a new scene — repaint.
    SceneChanged,
}

/// The float window app: window + surface (created lazily on `Resumed`), the office
/// renderer (owns cross-frame caches), the live scene receiver, and the per-floor desk
/// capacity atomics it keeps in sync with the rendered office.
pub struct FloatApp {
    cfg: FloatConfig,
    theme: &'static Theme,
    pack: Pack,
    config_path: PathBuf,
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

impl FloatApp {
    pub fn new(
        cfg: FloatConfig,
        theme: &'static Theme,
        pack: Pack,
        config_path: PathBuf,
        scene_rx: watch::Receiver<Arc<SceneState>>,
        floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    ) -> Self {
        Self {
            cfg,
            theme,
            pack,
            config_path,
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

    /// Persist the current window geometry into `[float]` (best-effort — a save error
    /// must not block quitting). Size is stored LOGICAL (HiDPI-stable); position PHYSICAL.
    fn persist_geometry(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let logical = window.inner_size().to_logical::<f64>(window.scale_factor());
        let pos = window.outer_position().ok();
        if let Err(e) = config::save_float(
            &self.config_path,
            logical.width.round() as u32,
            logical.height.round() as u32,
            pos.map(|p| p.x),
            pos.map(|p| p.y),
        ) {
            tracing::warn!("pixtuoid float: could not persist window geometry: {e}");
        }
    }

    /// Render the latest scene at the window's physical pixel size and blit it.
    fn redraw(&mut self) {
        // Clone the Rc to release the `self.window` borrow before touching `self.surface`.
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return; // a 0-area window: nothing to draw
        };
        // The office buffer IS the window canvas: render at physical px → 1:1 blit, no
        // scaling. The size-adaptive layout fills whatever dims it's given.
        let buf_w = size.width.min(u16::MAX as u32) as u16;
        let buf_h = size.height.min(u16::MAX as u32) as u16;
        // Keep the reducer's desk capacity in lockstep with the office actually rendered
        // at this size (authority = the layout's home-desk count, same as the TUI). Only
        // on a size change — capacity is otherwise size-invariant.
        if self.last_caps_size != Some((buf_w, buf_h)) {
            sync_floor_caps(&self.floor_caps, buf_w, buf_h);
            self.last_caps_size = Some((buf_w, buf_h));
        }
        // Arc clone releases the watch borrow before the (mutable) renderer borrow.
        let scene = self.scene_rx.borrow().clone();
        let office = self.renderer.render(
            &scene,
            &self.pack,
            self.theme,
            SystemTime::now(),
            buf_w,
            buf_h,
            FloorMeta::ground(),
        );
        // Collect into a local (release the `self.renderer` borrow) before borrowing
        // `self.surface`. softbuffer wants `0x00RRGGBB` (alpha byte ignored).
        let frame: Vec<u32> = office
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
        // The office buffer and the surface are both (buf_w, buf_h) physical px, so this
        // is a full copy; the `min` only guards a transient resize race.
        let n = sb.len().min(frame.len());
        sb[..n].copy_from_slice(&frame[..n]);
        window.pre_present_notify();
        let _ = sb.present();
    }
}

/// Sync the per-floor desk-capacity atomics to the office layout at `buf_w`×`buf_h` —
/// the authority is the layout's `home_desks` count (mirrors the TUI's per-frame sync,
/// `tui/mod.rs`). `store` (not `fetch_max`): float tracks its window exactly, so a shrink
/// lowers capacity (excess agents become invisible-but-alive, like the TUI on shrink).
fn sync_floor_caps(floor_caps: &[AtomicUsize; MAX_FLOORS], buf_w: u16, buf_h: u16) {
    use pixtuoid_core::layout::{SceneLayout, MAX_VISIBLE_DESKS};
    for (floor_idx, cap) in floor_caps.iter().enumerate() {
        let seed = (floor_idx as u64).wrapping_mul(crate::tui::floor::FLOOR_SEED_MULTIPLIER);
        let capacity = SceneLayout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, seed)
            .map(|l| l.home_desks.len())
            .unwrap_or(0);
        cap.store(capacity, Ordering::Relaxed);
    }
}

impl ApplicationHandler<FloatEvent> for FloatApp {
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
                config::FLOAT_MIN_W as f64,
                config::FLOAT_MIN_H as f64,
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
                tracing::error!("pixtuoid float: failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("pixtuoid float: failed to create softbuffer context: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("pixtuoid float: failed to create softbuffer surface: {e}");
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

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: FloatEvent) {
        match event {
            FloatEvent::SceneChanged => {
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
