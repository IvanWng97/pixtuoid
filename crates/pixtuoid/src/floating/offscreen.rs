//! Headless office → `RgbBuffer` rendering for the `pixtuoid floating` desktop window.
//!
//! This renders the office to a raw pixel `RgbBuffer` via the shared scene seam
//! (`pixtuoid_scene::floor::render_floor`, #423) — NOT the half-block terminal emulation
//! `examples/snapshot.rs` saves (snapshot writes the ratatui `TestBackend` → a ▀-compressed
//! PNG via `save_backend_as_png`). A floating-only surface: no `draw_scene`, no `Terminal`,
//! no shared output with snapshot. `floating::window` renders at a DOWNSCALED buffer
//! (~window/SCALE) and nearest-neighbor upscales it, so the pixel-art office stays
//! chunky/legible instead of 8×12-px-tiny at 1:1. This module just paints the buffer at
//! whatever dims it's handed, owning the per-frame caches plus the persistent office state
//! (coffee cups, group chitchat) across frames so motion stays continuous.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::{format::Pack, Rgb, RgbBuffer};
use pixtuoid_core::state::SceneState;

use pixtuoid_scene::chitchat::{ActiveChitchat, VenueKey};
use pixtuoid_scene::floor::{render_floor, CoffeeState, FloorCtx, FloorMeta};
use pixtuoid_scene::layout::Layout;
use pixtuoid_scene::theme::Theme;

/// Owns everything needed to render the live office to a reusable `RgbBuffer` across
/// frames: the per-floor render caches (`FloorCtx`) plus the persistent office state
/// the pixel pass reads and updates (`CoffeeState` drives desk cups + steam;
/// `chitchat` drives group speech bubbles). One per window — keeping it
/// alive across frames is what keeps motion/pose continuous (no walk-flash).
pub struct OfficeRenderer {
    floor: FloorCtx,
    buf: RgbBuffer,
    chitchat: HashMap<VenueKey, ActiveChitchat>,
    coffee: CoffeeState,
    /// The layout the LAST `render` computed — captured so `labels` can build the
    /// name-badge overlay against the SAME geometry the sprite pass used (labels
    /// align 1:1 with the painted characters). `None` before the first render.
    last_layout: Option<Layout>,
}

impl OfficeRenderer {
    pub fn new() -> Self {
        Self {
            floor: FloorCtx::new(),
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            chitchat: HashMap::new(),
            coffee: CoffeeState::new(),
            last_layout: None,
        }
    }

    /// Render `scene`'s floor (per `floor_meta`) into the owned buffer at `buf_w`×`buf_h`
    /// PIXELS — the caller maps window px → cells → pixels (`buf_w = cols`,
    /// `buf_h = rows * 2`, the half-block 1:2 cell aspect; floating has no footer row to
    /// subtract, unlike `draw_scene`). Returns the rendered buffer (a borrow of the
    /// reused allocation). On a too-small / uncomputable layout it returns the buffer
    /// unchanged — never panics.
    #[allow(clippy::too_many_arguments)] // the render inputs are genuinely flat (scene/pack/theme/clock/size/floor)
    pub fn render(
        &mut self,
        scene: &SceneState,
        pack: &Pack,
        theme: &'static Theme,
        now: SystemTime,
        buf_w: u16,
        buf_h: u16,
        floor_meta: FloorMeta,
        floor_pet: Option<&pixtuoid_scene::pet::Pet>,
    ) -> &RgbBuffer {
        // Drop per-agent state for agents no longer in the scene (#423: the
        // shared eviction; previously the floating painter never evicted, so
        // exited agents' motion/cache/coffee entries accumulated for the
        // window's lifetime — a slow leak, invisible in pixels because gone
        // agents aren't painted).
        self.floor.evict_missing(scene);
        self.coffee.evict_missing(scene);
        // The shared scene seam (#423): buffer sizing, layout, the pixel pass,
        // and the coffee/door-anim epilogue in one place. The returned layout
        // is captured for `labels` — the name-badge overlay must be built
        // against the SAME geometry the sprite pass used. active_pet stays
        // None: click-to-pet needs window pointer hit-testing (deferred); the
        // WANDERING floor pet is wired.
        self.last_layout = render_floor(
            &mut self.floor,
            &mut self.buf,
            &mut self.coffee,
            &mut self.chitchat,
            scene,
            pack,
            theme,
            now,
            buf_w,
            buf_h,
            floor_meta,
            None,
            floor_pet,
            false,
        );
        &self.buf
    }

    /// Build the name-badge overlay for the LAST rendered frame (call right after `render`).
    /// Uses the SAME layout + per-floor route state the sprite pass used, so labels align 1:1
    /// with the painted characters. Floating has no agent-hover yet → `hovered = None`.
    pub fn labels(
        &mut self,
        scene: &SceneState,
        now: SystemTime,
    ) -> Vec<pixtuoid_scene::overlay::LabelElement> {
        let Some(layout) = self.last_layout.as_ref() else {
            return Vec::new();
        };
        let mut rctx = pixtuoid_scene::pose::RouteCtx {
            router: &mut self.floor.router,
            overlay: &self.floor.overlay,
            history: &mut self.floor.history,
            motion: &mut self.floor.motion,
        };
        pixtuoid_scene::overlay::build_overlay(scene, layout, now, &mut rctx, None)
    }
}

impl Default for OfficeRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Integer upscale factor: render the office at `win_h / SCALE` so the buffer stays around
/// `OFFICE_TARGET_H` px tall, keeping pixel-art sprites chunky + legible (a native 1:1 blit
/// renders 8×12 sprites at 8×12 px — unreadably tiny). Min 1 (never downscale-and-blur).
/// Shared by `window::redraw` and the `floating_snapshot` example so their downscale —
/// and thus the label `anchor_px × scale` placement — can't drift.
pub fn office_scale(win_h: u32) -> u32 {
    const OFFICE_TARGET_H: u32 = 180;
    (win_h as f64 / OFFICE_TARGET_H as f64).round().max(1.0) as u32
}

/// The window→office-buffer projection for a `win_w`×`win_h` px window: the
/// integer `office_scale` plus the downscaled buffer dims (`window / scale`,
/// clamped non-zero, NO footer row). The ONE place this geometry lives — shared
/// by `window::redraw` (which needs `scale` for the upscale blit and the buffer
/// dims for `sync_floor_caps` + the render) and the boot seed
/// (`boot_capacities_for_window`) — so the desk capacity they derive can't drift
/// on an `office_scale`/clamp change.
pub(crate) fn window_buffer_geometry(win_w: u32, win_h: u32) -> (u32, u16, u16) {
    let scale = office_scale(win_h);
    let buf_w = (win_w / scale).clamp(1, u16::MAX as u32) as u16;
    let buf_h = (win_h / scale).clamp(1, u16::MAX as u32) as u16;
    (scale, buf_w, buf_h)
}

/// Per-floor boot desk-capacities for the FLOATING window. Uses the SAME
/// `window_buffer_geometry` the first redraw's `window::sync_floor_caps` does —
/// the office buffer is `window / office_scale` with NO footer row — so the boot
/// seed and the first redraw AGREE. The TUI's `runtime::boot_capacities_for`
/// instead subtracts a footer row AND ignores the window upscale, so reusing it
/// here OVER-seeds: in the sub-frame boot race before the first redraw, a
/// `SessionStart` could land at a `desk_index` the smaller real layout lacks
/// (immutable → invisible-but-alive until a resize). A floor whose layout rejects
/// the size falls back to `FALLBACK_DESKS`, matching the TUI boot helper.
pub(crate) fn boot_capacities_for_window(
    win_w: u32,
    win_h: u32,
) -> [usize; pixtuoid_core::state::MAX_FLOORS] {
    let (_scale, buf_w, buf_h) = window_buffer_geometry(win_w, win_h);
    std::array::from_fn(|i| {
        let seed = pixtuoid_scene::floor::floor_seed(i);
        let cap = pixtuoid_scene::floor::floor_capacity(buf_w, buf_h, seed);
        if cap == 0 {
            crate::runtime::FALLBACK_DESKS
        } else {
            cap
        }
    })
}

/// The bundled character sprite width (px). `CHARACTER_SPRITE_W` is `pub(super)` to
/// `scene::pixel_painter` (not re-exported through `scene::layout`), so the floating
/// painter uses the literal — labels only center ±half a glyph, so ±1px on a non-8-wide
/// custom pack is cosmetically irrelevant (same rationale as `character_anchor`).
const FLOATING_SPRITE_W: i32 = 8;

/// Paint name badges into the upscaled `u32` surface (`0x00RRGGBB`). Each label's `anchor_px`
/// is office-buffer space → multiply by `scale` for screen space; the badge is centered
/// horizontally over the anchor and sits just above the head. Crisp 8px text (drawn at native
/// surface res, not upscaled) keeps it proportional to the chunky sprites. Shared by the live
/// window (`window::redraw`) and the `floating_snapshot` verify example, so both blit identically.
pub fn paint_labels_into_surface(
    sb: &mut [u32],
    win_w: usize,
    win_h: usize,
    labels: &[pixtuoid_scene::overlay::LabelElement],
    scale: i32,
    theme: &Theme,
) {
    use pixtuoid_scene::overlay::LabelTone;
    for el in labels {
        let rgb = if el.hovered {
            Rgb {
                r: 240,
                g: 240,
                b: 240,
            }
        } else {
            match el.tone {
                LabelTone::Exiting => theme.ui.label_exiting,
                LabelTone::Active => theme.ui.label_active,
                LabelTone::Waiting => theme.ui.label_waiting,
                LabelTone::Idle => theme.ui.label_idle,
            }
        };
        let color = (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32;
        // `\u{25cf}` (●) is in `scene::font`; `\u{25b8}` (▸) is NOT yet — the hovered branch
        // is dead today (`labels()` passes `hovered: None`, floating has no agent-hover). If
        // floating hover is ever wired, add a ▸ bitmap to `font::custom_glyph` first (its
        // absence currently renders a blank gap, not the marker).
        let text = if el.hovered {
            format!("\u{25b8}{}", el.text)
        } else {
            format!("\u{25cf}{}", el.text)
        };
        let tw = pixtuoid_scene::font::text_width(&text, 1);
        // anchor_px is the sprite TOP-LEFT in office space; center the badge over the sprite
        // and lift it one glyph-height (8px) + a 2px gap above the head.
        const BADGE_LIFT_PX: i32 = 10;
        let cx = el.anchor_px.x as i32 * scale + (FLOATING_SPRITE_W * scale) / 2 - tw / 2;
        let cy = el.anchor_px.y as i32 * scale - BADGE_LIFT_PX;
        let mut put = |x: i32, y: i32, c: u32| {
            if x >= 0 && y >= 0 && (x as usize) < win_w && (y as usize) < win_h {
                sb[y as usize * win_w + x as usize] = c;
            }
        };
        // A 1px near-black drop-shadow under the badge — the text draws straight over the
        // office (no cell background like the TUI), so a shadow keeps it legible over bright
        // windows / plants / furniture. Floating-painter-only (the tui surface has cell bg).
        const SHADOW: u32 = 0x0000_0000;
        pixtuoid_scene::font::draw_text(&text, cx + 1, cy + 1, 1, |x, y| put(x, y, SHADOW));
        pixtuoid_scene::font::draw_text(&text, cx, cy, 1, |x, y| put(x, y, color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_sized_nonblank_office_buffer() {
        // A fresh empty office still paints floor/walls/windows → never all-black, and the
        // buffer is sized to the requested pixel dims. Pins the floating render seam end-to-end.
        let scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();
        let buf = renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        assert_eq!((buf.width(), buf.height()), (160, 96));
        // Assert PAINTED content, not the pre-fill: `ensure_size` fills the buffer with
        // `bg_fallback` (non-black) BEFORE the painter runs, so "any non-black pixel" would
        // pass even if the painter no-op'd. Require a pixel that is neither black NOR
        // `bg_fallback` → the floor/walls/windows pass actually ran.
        let bg = theme.surface.bg_fallback;
        assert!(
            buf.as_slice()
                .iter()
                .any(|p| *p != Rgb { r: 0, g: 0, b: 0 } && *p != bg),
            "the painter draws office content beyond the cleared background"
        );
    }

    #[test]
    fn office_scale_keeps_the_office_chunky_and_never_zero() {
        // Downscale so the office buffer stays ~OFFICE_TARGET_H (180px) tall.
        assert_eq!(office_scale(180), 1);
        assert_eq!(office_scale(360), 2);
        assert_eq!(office_scale(720), 4);
        // A short window still renders at scale 1 — never 0 (redraw divides by it).
        assert_eq!(office_scale(90), 1);
        assert_eq!(office_scale(0), 1);
    }

    #[test]
    fn boot_capacities_for_window_match_the_first_redraw_geometry_not_the_tui_overseed() {
        // A 4x-upscaled window (720px tall → office_scale 4): the boot seed must
        // match what the first redraw's `sync_floor_caps` stores — `floor_capacity`
        // at the DOWNSCALED buffer (win/scale), no footer — not the full-window
        // over-seed the TUI helper produces.
        let (w, h) = (1280u32, 720u32);
        let scale = office_scale(h);
        let buf_w = (w / scale) as u16;
        let buf_h = (h / scale) as u16;
        let boot = boot_capacities_for_window(w, h);
        for (i, &got) in boot.iter().enumerate() {
            let cap = pixtuoid_scene::floor::floor_capacity(
                buf_w,
                buf_h,
                pixtuoid_scene::floor::floor_seed(i),
            );
            let want = if cap == 0 {
                crate::runtime::FALLBACK_DESKS
            } else {
                cap
            };
            assert_eq!(
                got, want,
                "floor {i} boot cap must match the rendered geometry"
            );
        }
        // The old TUI helper (footer subtraction + no office_scale) over-seeds the
        // ground floor — the bug this fix removes.
        let overseed = crate::runtime::boot_capacities_for(w as u16, (h / 2) as u16);
        assert!(
            overseed[0] >= boot[0],
            "TUI helper over-seeds ({} vs {})",
            overseed[0],
            boot[0]
        );
    }

    #[test]
    fn paint_labels_uses_the_right_color_per_tone_and_overrides_with_white_on_hover() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::overlay::{LabelElement, LabelTone};
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let as_u32 = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        let badge = |tone, hovered| {
            vec![LabelElement {
                anchor_px: Point { x: 20, y: 20 },
                text: "cc".into(),
                tone,
                hovered,
            }]
        };
        // Each tone must paint its OWN theme color — not merely "some pixel". A wrong match
        // arm (e.g. Idle returning the Active color) would fail this.
        for (tone, expected) in [
            (LabelTone::Active, theme.ui.label_active),
            (LabelTone::Waiting, theme.ui.label_waiting),
            (LabelTone::Idle, theme.ui.label_idle),
            (LabelTone::Exiting, theme.ui.label_exiting),
        ] {
            let mut sb = vec![0u32; 100 * 100];
            paint_labels_into_surface(&mut sb, 100, 100, &badge(tone, false), 2, theme);
            assert!(
                sb.contains(&as_u32(expected)),
                "tone {tone:?} must paint its theme color {expected:?}"
            );
        }
        // Hover OVERRIDES the tone color with white (240,240,240). Use Idle (a dim grey) so the
        // negative assertion is meaningful: white present AND the idle grey absent.
        let mut sb = vec![0u32; 100 * 100];
        paint_labels_into_surface(&mut sb, 100, 100, &badge(LabelTone::Idle, true), 2, theme);
        assert!(
            sb.contains(&as_u32(Rgb {
                r: 240,
                g: 240,
                b: 240
            })),
            "a hovered badge paints white"
        );
        assert!(
            !sb.contains(&as_u32(theme.ui.label_idle)),
            "hover must override the tone color, not paint the idle grey"
        );
    }

    #[test]
    fn labels_is_empty_before_render_then_builds_a_positioned_badge_for_a_seeded_agent() {
        use pixtuoid_core::source::AgentEvent;
        use pixtuoid_core::{AgentId, Reducer, Transport};
        let pack =
            pixtuoid_scene::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let theme = pixtuoid_scene::theme::theme_by_name("normal").expect("normal theme exists");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut renderer = OfficeRenderer::new();

        // One real agent, seeded the production way: a SessionStart through the reducer
        // registers the slot and assigns it a desk on floor 0.
        let mut scene = SceneState::new([8; pixtuoid_core::state::MAX_FLOORS]);
        let mut reducer = Reducer::new();
        reducer.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: AgentId::from_parts("claude-code", "offscreen-labels-test"),
                source: "claude-code".to_string(),
                session_id: "offscreen-labels-test".to_string(),
                cwd: std::path::PathBuf::from("/home/user/demo-project"),
                parent_id: None,
            },
            now,
            Transport::Jsonl,
        );

        // No frame rendered yet → no cached layout → the guard returns empty.
        assert!(renderer.labels(&scene, now).is_empty());
        // After a render, labels() builds the overlay off the cached layout → one badge for the
        // seeded agent, anchored inside the rendered 160×96 office buffer (proves the seam wires
        // render's geometry into build_overlay, not just that the line executed).
        renderer.render(
            &scene,
            &pack,
            theme,
            now,
            160,
            96,
            FloorMeta::ground(),
            None,
        );
        let labels = renderer.labels(&scene, now);
        assert_eq!(labels.len(), 1, "one seeded agent → one name badge");
        let anchor = labels[0].anchor_px;
        assert!(
            (0..160).contains(&(anchor.x as i32)) && (0..96).contains(&(anchor.y as i32)),
            "badge anchor {anchor:?} lands inside the rendered office buffer"
        );
    }
}
