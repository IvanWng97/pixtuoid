//! The `winit` + `softbuffer` window for `pixtuoid float`.
//!
//! `FloatApp` is the `ApplicationHandler`: on `Resumed` it creates ONE frameless,
//! always-on-top window + a `softbuffer` surface; on `RedrawRequested` it renders the
//! current scene to a full-resolution office `RgbBuffer` via [`OfficeRenderer`] and
//! blits it (CPU, `0x00RRGGBB`). Platform glue — codecov-ignored like `driver.rs`; the
//! testable render seam is `tui::offscreen`. The live source pipeline (Task 5) replaces
//! the static empty `scene`; for now it proves the window + render path end-to-end.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::SystemTime;

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::SceneState;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::config::{self, FloatConfig};
use crate::tui::floor::FloorMeta;
use crate::tui::offscreen::OfficeRenderer;
use crate::tui::theme::Theme;

/// The float window app: window + surface (created lazily on `Resumed`), the office
/// renderer (owns cross-frame caches), and the scene to draw.
pub struct FloatApp {
    cfg: FloatConfig,
    theme: &'static Theme,
    pack: Pack,
    renderer: OfficeRenderer,
    /// The office to render. Static empty office for now; the live pipeline replaces
    /// this with the latest `watch`ed scene in Task 5.
    scene: SceneState,
    window: Option<Rc<Window>>,
    // softbuffer's `Context` must outlive the `Surface` it spawned, so keep both.
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl FloatApp {
    pub fn new(cfg: FloatConfig, theme: &'static Theme, pack: Pack) -> Self {
        Self {
            cfg,
            theme,
            pack,
            renderer: OfficeRenderer::new(),
            scene: SceneState::new([0; pixtuoid_core::state::MAX_FLOORS]),
            window: None,
            context: None,
            surface: None,
        }
    }

    /// Render the current scene at the window's physical pixel size and blit it.
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
        let office = self.renderer.render(
            &self.scene,
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
        // Defensive: blit the overlap. The office buffer is sized to (buf_w, buf_h) and
        // the surface to (nw, nh) = the same physical size, so this is a full copy; the
        // `min` only guards a transient resize race.
        let n = sb.len().min(frame.len());
        sb[..n].copy_from_slice(&frame[..n]);
        window.pre_present_notify();
        let _ = sb.present();
    }
}

impl ApplicationHandler for FloatApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already created — a re-resume must not spawn a second window
        }
        let attrs = Window::default_attributes()
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
        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}
