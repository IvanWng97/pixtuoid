//! What a frame shows, layer by layer, and whether the screen already shows it.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::overlays::Overlays;

/// Pack an `Rgb` into the softbuffer word format, `0x00RRGGBB` (XRGB) — the ONE
/// definition of the floating surface pixel format; the office blit (`window.rs`)
/// and this label overlay write into the SAME surface, so a lone edit to one would
/// color-swap the badges with no compile error. The test oracle re-derives the
/// packing independently ON PURPOSE — don't route it through this.
pub(crate) fn pack_xrgb(c: Rgb) -> u32 {
    u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
}

/// The window's row-major `0x00RRGGBB` pixel surface, `w`×`h`, that the text
/// overlays composite into.
#[derive(Debug)]
pub struct XrgbSurface<'a> {
    pub(super) px: &'a mut [u32],
    pub(super) w: usize,
    pub(super) h: usize,
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
}

impl pixtuoid_scene::cutaway::Canvas for XrgbSurface<'_> {
    fn pixel(&self, x: i32, y: i32) -> Option<Rgb> {
        let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
        (x < self.w && y < self.h).then(|| {
            let v = self.px[y * self.w + x];
            Rgb {
                r: (v >> 16) as u8,
                g: (v >> 8) as u8,
                b: v as u8,
            }
        })
    }

    fn set(&mut self, x: i32, y: i32, rgb: Rgb) {
        if let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y))
            && x < self.w
            && y < self.h
        {
            self.px[y * self.w + x] = pack_xrgb(rgb);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_scene::footer::FooterModel;

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
            panels: None,
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
}
