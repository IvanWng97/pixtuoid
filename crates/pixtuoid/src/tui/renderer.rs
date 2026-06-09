//! Terminal-coupled rendering: orchestrator (`draw_scene`), half-block
//! flush, and label/tooltip/notice widget overlays.
//!
//! The pure-pixel pass (floor/walls/decor/characters -> `RgbBuffer`) lives
//! in `tui::pixel_painter`. This file is the integrator that calls into
//! that pipeline and then hands the buffer to ratatui. Terminal lifecycle
//! (raw mode + alternate screen) lives with the event loop in `tui/mod.rs`.
//!
//! Widget paint functions live in `tui::widgets`; hit-test functions live
//! in `tui::hit_test`. Both are re-exported here for backwards compat.

use std::time::SystemTime;

use anyhow::Result;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::SceneState;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::Layout;
use crate::tui::motion::MotionState;
use crate::tui::pathfind::Router;
use crate::tui::pet::PetFrame;
use crate::tui::pixel_painter::{render_to_rgb_buffer, PixelCtx};
use crate::tui::pose;

// Re-exports so tui_renderer.rs and tui/mod.rs import from one place.
use crate::tui::dashboard::DashboardRow;
pub(crate) use crate::tui::hit_test::hit_test_agent;
pub use crate::tui::hit_test::{
    hit_test_coffee_machine, hit_test_from_tui, hit_test_furniture, hit_test_pet,
};
pub(crate) use crate::tui::widgets::paint_hover_tooltip;
pub use crate::tui::widgets::TickerQueue;
pub(super) use crate::tui::widgets::{
    paint_chitchat_bubbles, paint_coffee_tooltip, paint_dashboard, paint_elevator_indicator,
    paint_footer, paint_furniture_tooltip, paint_help_overlay, paint_label_widgets,
    paint_pet_tooltip, paint_theme_picker, paint_version_popup, paint_wall_display,
};

pub use crate::tui::pet::PetState;

/// Multi-floor display state. Combines the navigation breadcrumb
/// (current/total) with the global agent count so a renderer never sees
/// one without the other.
#[derive(Debug, Clone, Copy)]
pub struct FloorInfo {
    /// 1-indexed current floor for display (e.g. "F2/3").
    pub current: usize,
    pub total_floors: usize,
    /// Total agents across all floors. Used for the footer's `n/total`.
    pub total_agents: usize,
}

/// Mutable per-frame render state, borrowed from `TuiRenderer`. Replaces
/// the 14-parameter `draw_scene` signature with a single struct pass.
pub struct DrawCtx<'a> {
    pub buf: &'a mut RgbBuffer,
    pub cache: &'a mut FrameCache,
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut pixtuoid_core::walkable::OccupancyOverlay,
    pub history: &'a mut pose::PoseHistory,
    /// Per-floor motion state — threaded like `history`. Agents' `MotionState`
    /// entries are initialized and advanced by `derive_with_routing`.
    pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
    /// Per-floor max in-flight entry/exit physics duration (ms). Written
    /// each render tick by `tui_renderer.rs` from `fctx.motion`; read by
    /// `compute_door_frame_idx` so the door cosmetic scales with actual
    /// walk physics instead of the old hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
    /// Per-floor lighting fade state. Advanced inside the pixel pass and
    /// read by the indoor-light helpers. Borrowed mutably from the
    /// matching `FloorCtx`.
    pub light: &'a mut crate::tui::floor::LightingState,
    pub mouse_pos: Option<(u16, u16)>,
    pub pinned_agent: Option<pixtuoid_core::AgentId>,
    /// Live walkable/approach/route debug layer toggle (`w`). Threaded into the
    /// pixel pass; off by default, transient (not persisted to config).
    pub debug_walkable: bool,
    pub ticker: &'a TickerQueue,
    pub theme: &'a crate::tui::theme::Theme,
    pub theme_picker: Option<usize>,
    /// Multi-floor display state. `Some` iff there's more than one floor.
    /// Carries both the navigation breadcrumb (`current/total_floors`) and
    /// the system-wide agent count so the footer can render `n/total` and
    /// the elevator indicator can highlight the active floor.
    pub floor_info: Option<FloorInfo>,
    pub floor: crate::tui::floor::FloorMeta,
    pub active_pet: Option<&'a PetState>,
    pub last_pet_pos: Option<PetFrame>,
    /// The pet assigned to this floor — its kind AND resolved display name.
    /// `None` when no pets are configured or none maps to this floor seed.
    /// Replaces the former `floor_pet_kind` + `pet_names` pair: the name rides
    /// along, so the tooltip reads `floor_pet.name` directly (no lookup).
    pub floor_pet: Option<&'a crate::tui::pet::Pet>,
    pub chitchat_state: &'a mut std::collections::HashMap<
        crate::tui::chitchat::VenueKey,
        crate::tui::chitchat::ActiveChitchat,
    >,
    pub chitchat_bubbles: Vec<crate::tui::chitchat::ChitchatBubble>,
    pub coffee_holders: &'a std::collections::HashSet<pixtuoid_core::AgentId>,
    pub coffee_fetched_at:
        &'a std::collections::HashMap<pixtuoid_core::AgentId, std::time::SystemTime>,
    /// New coffee carriers detected this frame — caller uses these to
    /// update the persistent `coffee_holders` set.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
    /// Animated scale for the version popup (0.0 = hidden, 1.0 = fully shown).
    /// Drives entrance (EaseOutCubic/200ms) and dismissal (EaseInQuad/120ms).
    pub popup_scale: f32,
    pub help_open: bool,
    /// Footer warning when a source has died (#157); `None` while healthy.
    pub source_warning: Option<&'a str>,
    /// Agent dashboard overlay: open flag + the pre-built row snapshot
    /// (borrowed from `TuiRenderer`, disjoint from the floor borrows) +
    /// selection/scroll. Painted last, modal, mutually exclusive with the
    /// theme picker by dispatch precedence.
    pub dashboard_open: bool,
    pub dashboard_rows: &'a [DashboardRow],
    pub dashboard_selected: Option<pixtuoid_core::AgentId>,
    pub dashboard_scroll: usize,
}

/// Clip a widget rect to fit inside `bounds`. Returns `None` if the rect
/// falls fully outside or has zero width/height after clipping -- callers
/// use that to skip the render entirely. Prevents ratatui's
/// "index outside of buffer" panic when label/notice widgets land near
/// the right or bottom edge.
pub(crate) fn clip_widget_rect(rect: Rect, bounds: Rect) -> Option<Rect> {
    if rect.x >= bounds.x + bounds.width || rect.y >= bounds.y + bounds.height {
        return None;
    }
    if rect.x + rect.width <= bounds.x || rect.y + rect.height <= bounds.y {
        return None;
    }
    let x = rect.x.max(bounds.x);
    let y = rect.y.max(bounds.y);
    let right = (rect.x + rect.width).min(bounds.x + bounds.width);
    let bot = (rect.y + rect.height).min(bounds.y + bounds.height);
    if right <= x || bot <= y {
        return None;
    }
    Some(Rect {
        x,
        y,
        width: right - x,
        height: bot - y,
    })
}

/// The drawable scene rect: the full terminal area minus the 1-row footer.
/// Single source of truth for the "everything but the footer" geometry that
/// both `draw_scene` and the floor-transition path re-derive each frame.
pub(crate) fn scene_rect(full: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: full.width,
        height: full.height.saturating_sub(1),
    }
}

// --- draw_scene ----------------------------------------------------------
//
// `draw_scene` is the orchestrator: get terminal geometry, compute the
// layout, run the pure pixel pass, then flush to the terminal. The two
// helpers below are deliberately split:
//
//   * `render_to_rgb_buffer` -- pure RGB output. No ratatui types, no
//     terminal I/O. Can be called by any renderer (web canvas, PNG
//     snapshot, GIF capture).
//   * `flush_to_terminal` -- ratatui half-block compression + label overlay
//     + bulletin notice + footer. Terminal-specific, runs inside
//     `term.draw`.
pub fn draw_scene<B: Backend<Error: Send + Sync + 'static>>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    ctx: &mut DrawCtx<'_>,
) -> Result<Option<Layout>> {
    let term_size = term.size()?;
    let full_rect = Rect {
        x: 0,
        y: 0,
        width: term_size.width,
        height: term_size.height,
    };
    let scene_rect = scene_rect(full_rect);
    let theme = ctx.theme;
    let floor_info = ctx.floor_info;
    let source_warning = ctx.source_warning;
    let floor = ctx.floor;

    if scene_rect.width < 20 || scene_rect.height < 12 {
        term.draw(|f| {
            let actual = f.area();
            paint_footer(f, scene, actual, theme, floor_info, ctx.source_warning);
        })?;
        return Ok(None);
    }

    let buf_w = scene_rect.width;
    let buf_h = scene_rect.height * 2;
    ctx.buf.ensure_size(buf_w, buf_h, theme.surface.bg_fallback);
    use crate::tui::layout::MAX_VISIBLE_DESKS;
    // Always compute maximum layout capacity — floor overflow handles the rest.
    let Some(layout) = Layout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, floor.floor_seed)
    else {
        term.draw(|f| {
            let actual = f.area();
            paint_footer(f, scene, actual, theme, floor_info, ctx.source_warning);
        })?;
        return Ok(None);
    };

    ctx.router.set_preferred_zone(layout.corridor);

    let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
        scene,
        layout: &layout,
        pack,
        now,
        buf: ctx.buf,
        cache: ctx.cache,
        router: ctx.router,
        overlay: ctx.overlay,
        history: ctx.history,
        motion: ctx.motion,
        theme,
        floor,
        active_pet: ctx.active_pet,
        floor_pet: ctx.floor_pet,
        chitchat_state: ctx.chitchat_state,
        coffee_holders: ctx.coffee_holders,
        coffee_fetched_at: ctx.coffee_fetched_at,
        light: ctx.light,
        door_anim_max_ms: ctx.door_anim_max_ms,
        debug_walkable: ctx.debug_walkable,
    });
    ctx.last_pet_pos = pixel_result.pet_pos;
    ctx.chitchat_bubbles = pixel_result.chitchat_bubbles;
    ctx.new_coffee_carriers = pixel_result.new_coffee_carriers;

    let mouse_pos = ctx.mouse_pos;
    let pinned_agent = ctx.pinned_agent;
    let hovered = mouse_pos.and_then(|(mx, my)| {
        hit_test_agent(
            scene,
            &layout,
            now,
            &mut crate::tui::pose::RouteCtx {
                router: &mut *ctx.router,
                overlay: &*ctx.overlay,
                history: &mut *ctx.history,
                motion: &mut *ctx.motion,
            },
            mx,
            my,
        )
    });

    let buf = &ctx.buf;
    let ticker = ctx.ticker;
    let theme_picker = ctx.theme_picker;
    let chitchat_bubbles = &ctx.chitchat_bubbles;
    term.draw(|f| {
        // Re-derive rects from the actual frame buffer to guard against
        // terminal resize between term.size() and term.draw().
        let actual_full = f.area();
        let actual_scene = crate::tui::renderer::scene_rect(actual_full);
        paint_footer(f, scene, actual_full, theme, floor_info, source_warning);
        flush_buffer_to_term(f, buf, actual_scene);
        paint_label_widgets(
            f,
            scene,
            &layout,
            now,
            &mut crate::tui::pose::RouteCtx {
                router: &mut *ctx.router,
                overlay: &*ctx.overlay,
                history: &mut *ctx.history,
                motion: &mut *ctx.motion,
            },
            actual_scene,
            hovered,
            theme,
        );
        paint_chitchat_bubbles(f, chitchat_bubbles, actual_scene, theme);
        paint_wall_display(f, scene, actual_scene, now, ticker, theme);
        if let Some(door) = layout.door {
            let current = floor_info.map(|fi| fi.current).unwrap_or(1);
            paint_elevator_indicator(f, door, current, actual_scene, theme);
        }
        let tooltip_agent = hovered.or(pinned_agent);
        if let (Some(agent_id), Some((mx, my))) = (tooltip_agent, mouse_pos) {
            paint_hover_tooltip(f, scene, agent_id, mx, my, actual_scene, now, theme);
        } else if let Some(agent_id) = pinned_agent {
            paint_hover_tooltip(
                f,
                scene,
                agent_id,
                actual_scene.width / 2,
                actual_scene.height / 2,
                actual_scene,
                now,
                theme,
            );
        }
        if tooltip_agent.is_none() && pinned_agent.is_none() {
            if let Some((mx, my)) = mouse_pos {
                // Priority chain: coffee machine > pet (only when the cursor is
                // over it) > furniture. `.filter` keeps the pet arm a single
                // branch so a present-but-not-hit pet falls through to the ONE
                // furniture fallthrough below (no per-branch duplication).
                let pet_hit = ctx
                    .last_pet_pos
                    .filter(|f| hit_test_pet(f.kind, f.pos, f.anim, mx, my));
                if hit_test_coffee_machine(&layout, mx, my) {
                    paint_coffee_tooltip(f, mx, my, actual_scene, theme);
                } else if let Some(PetFrame { anim, kind, .. }) = pet_hit {
                    let on_cooldown = ctx.active_pet.is_some_and(|p| p.is_active(now));
                    // `last_pet_pos` is only Some on the normal render path,
                    // where it was written from `floor_pet` — so their kinds
                    // agree and `floor_pet.name` is the right label. The
                    // `default_name` arm is defense-in-depth, not a live path.
                    let display_name = ctx
                        .floor_pet
                        .map(|p| p.name.as_str())
                        .unwrap_or_else(|| kind.default_name());
                    paint_pet_tooltip(
                        f,
                        kind,
                        anim,
                        on_cooldown,
                        display_name,
                        mx,
                        my,
                        actual_scene,
                        theme,
                    );
                } else if let Some(label) = hit_test_furniture(&layout, mx, my) {
                    paint_furniture_tooltip(f, label, mx, my, actual_scene, theme);
                }
            }
        }
        if let Some(idx) = theme_picker {
            paint_theme_picker(f, idx, actual_full, theme);
        }
        if ctx.dashboard_open {
            paint_dashboard(
                f,
                ctx.dashboard_rows,
                ctx.dashboard_selected,
                ctx.dashboard_scroll,
                actual_full,
                theme,
            );
        }
        if ctx.popup_scale > 0.0 {
            if let Some(notes) = crate::version::release_notes(env!("CARGO_PKG_VERSION")) {
                paint_version_popup(
                    f,
                    env!("CARGO_PKG_VERSION"),
                    notes,
                    actual_full,
                    theme,
                    ctx.popup_scale,
                    now,
                );
            }
        }
        if ctx.help_open {
            // Center in actual_full (not actual_scene) so the overlay sits
            // at the same vertical center as the theme picker / version
            // popup, which both use actual_full.
            paint_help_overlay(f, actual_full, theme);
        }
    })?;
    Ok(Some(layout))
}

pub(super) fn flush_buffer_to_term_at_offset(
    f: &mut ratatui::Frame<'_>,
    buf: &RgbBuffer,
    scene_rect: Rect,
    y_offset: i32,
) {
    let term_buf = f.buffer_mut();
    let term_area = term_buf.area;
    let w = buf.width as usize;
    let cell_rows = (buf.height / 2) as usize;
    for cy in 0..cell_rows {
        let target_y = cy as i32 + y_offset;
        if target_y < 0 || target_y >= scene_rect.height as i32 {
            continue;
        }
        for cx in 0..(buf.width as usize) {
            let x = scene_rect.x + cx as u16;
            let y = scene_rect.y + target_y as u16;
            if x >= scene_rect.x + scene_rect.width {
                continue;
            }
            if x >= term_area.width || y >= term_area.height {
                continue;
            }
            let py_top = cy * 2;
            let py_bot = cy * 2 + 1;
            let fg = buf.pixels[py_top * w + cx];
            let bg = buf.pixels[py_bot * w + cx];
            let cell = &mut term_buf[(x, y)];
            cell.set_symbol("\u{2580}");
            cell.fg = Color::Rgb(fg.r, fg.g, fg.b);
            cell.bg = Color::Rgb(bg.r, bg.g, bg.b);
        }
    }
}

fn flush_buffer_to_term(f: &mut ratatui::Frame<'_>, buf: &RgbBuffer, scene_rect: Rect) {
    flush_buffer_to_term_at_offset(f, buf, scene_rect, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_widget_rect_fully_inside() {
        let r = Rect {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };
        let b = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(clip_widget_rect(r, b), Some(r));
    }

    #[test]
    fn clip_widget_rect_fully_outside_right() {
        let r = Rect {
            x: 80,
            y: 0,
            width: 10,
            height: 5,
        };
        let b = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(clip_widget_rect(r, b), None);
    }

    #[test]
    fn clip_widget_rect_partially_overflows_right() {
        let r = Rect {
            x: 75,
            y: 0,
            width: 10,
            height: 5,
        };
        let b = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let clipped = clip_widget_rect(r, b).unwrap();
        assert_eq!(clipped.x, 75);
        assert_eq!(clipped.width, 5);
    }

    #[test]
    fn clip_widget_rect_zero_size_returns_none() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 5,
        };
        let b = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(clip_widget_rect(r, b), None);
    }
}
