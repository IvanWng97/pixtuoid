//! Terminal-coupled rendering: orchestrator (`draw_scene`), half-block
//! flush, and label/tooltip/notice widget overlays.
//!
//! The pure-pixel pass (floor/walls/decor/characters -> `RgbBuffer`) lives
//! in the `pixtuoid_scene::pixel_painter` engine crate. This file is the
//! integrator that calls into
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

use std::sync::Arc;

use pixtuoid_scene::layout::Layout;
use pixtuoid_scene::pet::PetFrame;
use pixtuoid_scene::pixel_painter::{render_to_rgb_buffer, MascotFrame, PixelCtx};

// Re-exports so tui_renderer.rs and tui/mod.rs import from one place.
pub(crate) use crate::tui::hit_test::hit_test_agent;
pub use crate::tui::hit_test::{
    hit_test_coffee_machine, hit_test_furniture, hit_test_mascot, hit_test_pet,
};
pub(crate) use crate::tui::widgets::paint_hover_tooltip;
pub(super) use crate::tui::widgets::{
    paint_chitchat_bubbles, paint_coffee_tooltip, paint_connection_panel, paint_dashboard,
    paint_elevator_indicator, paint_footer, paint_furniture_tooltip, paint_help_overlay,
    paint_label_widgets, paint_mascot_tooltip, paint_pet_tooltip, paint_theme_picker,
    paint_version_popup, paint_wall_display, paint_welcome,
};

pub use pixtuoid_scene::pet::PetState;

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
    /// The per-floor sim/paint STORES borrowed as ONE group — was seven flat
    /// fields (`cache`/`router`/`overlay`/`history`/`motion`/`door_anim_max_ms`/
    /// `light`), each a `FloorCtx` field. `draw_scene` forwards it straight into
    /// `PixelCtx.store` and reads `store.router`/`store.overlay`/… for its
    /// RouteCtx hit-tests + label overlay. `buf` stays a SEPARATE field: it is a
    /// sibling of the `FloorCtx` on a `PerFloor`, borrowed disjointly.
    pub store: &'a mut pixtuoid_scene::floor::FloorCtx,
    pub mouse_pos: Option<(u16, u16)>,
    /// Live walkable/approach/route debug layer toggle (`w`). Threaded into the
    /// pixel pass; off by default, transient (not persisted to config).
    pub debug_walkable: bool,
    pub theme: &'a pixtuoid_scene::theme::Theme,
    pub theme_picker: Option<usize>,
    /// Multi-floor display state. `Some` iff there's more than one floor.
    /// Carries both the navigation breadcrumb (`current/total_floors`) and
    /// the system-wide agent count so the footer can render `n/total` and
    /// the elevator indicator can highlight the active floor.
    pub floor_info: Option<FloorInfo>,
    /// Office-wide per-floor state tally (bucketed from the FULL, un-projected
    /// scene by `AgentSlot.floor_idx`) — feeds the footer's cross-floor `▲F{n}`
    /// cue. ALWAYS present (C1): unlike `floor_info` it never vanishes for a
    /// single-floor office, so the chip + cue render there too. Distinct from the
    /// footer's per-state integers (`scene_stats` on the PROJECTED floor) — C8:
    /// don't derive one from the other.
    pub per_floor: [crate::tui::widgets::StateCounts; pixtuoid_core::state::MAX_FLOORS],
    /// Worst-of daemon-liveness rollup (the OpenClaw gateway) for the footer +
    /// board `⬢gw` chip. `None` = no daemon configured (chip suppressed). Computed
    /// from the full scene, independent of `floor_info` (C1).
    pub gateway: Option<pixtuoid_core::state::DaemonState>,
    /// "You would hear sound right now" — drives the footer's ♩ glyph.
    pub audio_audible: bool,
    /// Transient +/- volume readout (percent) — appended to ♩ for ~1s.
    pub volume_flash: Option<u8>,
    pub floor: pixtuoid_scene::floor::FloorMeta,
    pub active_pet: Option<&'a PetState>,
    pub last_pet_pos: Option<PetFrame>,
    /// The gateway mascot's frame this render (for hover identity). Set from the
    /// pixel pass; `None` when no gateway is present.
    pub last_mascot_pos: Option<MascotFrame>,
    /// The pet assigned to this floor — its kind AND resolved display name.
    /// `None` when no pets are configured or none maps to this floor seed.
    /// Replaces the former `floor_pet_kind` + `pet_names` pair: the name rides
    /// along, so the tooltip reads `floor_pet.name` directly (no lookup).
    pub floor_pet: Option<&'a pixtuoid_scene::pet::Pet>,
    pub chitchat_state: &'a mut std::collections::HashMap<
        pixtuoid_scene::chitchat::VenueKey,
        pixtuoid_scene::chitchat::ActiveChitchat,
    >,
    pub chitchat_bubbles: Vec<pixtuoid_scene::chitchat::ChitchatBubble>,
    /// Carrier → fetch-time view of the renderer's `CoffeeState` (one map:
    /// key present = has a desk cup, value = steam-window anchor).
    pub coffee: &'a std::collections::HashMap<pixtuoid_core::AgentId, std::time::SystemTime>,
    /// New coffee carriers detected this frame — caller records these into
    /// the persistent `CoffeeState`.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
    /// Waypoint occupancy this frame (out-param like the carriers) — the
    /// audio cue tracker's appliance feed (#633).
    pub occupied_waypoints: std::collections::HashSet<usize>,
    /// Animated scale for the version popup (0.0 = hidden, 1.0 = fully shown).
    /// Drives entrance (EaseOutCubic/200ms) and dismissal (EaseInQuad/120ms).
    pub popup_scale: f32,
    pub help_open: bool,
    /// Footer warning when a source has died (#157); `None` while healthy.
    pub source_warning: Option<&'a str>,
    /// Agent dashboard overlay frame (borrowed from `TuiRenderer`, disjoint from
    /// the floor borrows). Modal, mutually exclusive with the theme picker by
    /// dispatch precedence; painted last.
    pub dashboard: &'a crate::tui::dashboard::DashboardFrame,
    /// Sources panel overlay frame (borrowed from `TuiRenderer`): the cached
    /// hook-facet rows + the per-frame live facet + selection / armed-confirm /
    /// last-action result / socket line. Modal, mutually exclusive with the others.
    pub connection: &'a crate::tui::connection::ConnectionFrame,
    /// First-run onboarding overlay frame (borrowed from `TuiRenderer`): the open
    /// flag, roster snapshot, selection, and elapsed-ms clock. Modal and TOP of the
    /// precedence chain — painted last (topmost).
    pub onboarding: &'a crate::tui::welcome::OnboardingFrame,
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

/// Minimum drawable scene size (cells) below which the world render is skipped
/// for a footer-only draw. Shared by `draw_scene` and the floor-transition path
/// (`tui_renderer`) so the two "too small" gates can't drift apart.
pub(crate) const MIN_SCENE_WIDTH: u16 = 20;
pub(crate) const MIN_SCENE_HEIGHT: u16 = 12;

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

/// Paint the footer-only frame shown when the terminal is too small to render the
/// office — the SHARED body of BOTH too-small gates (`draw_scene`'s and the
/// floor-transition path's `render_transition`), so the shared `MIN_SCENE_*`
/// threshold implies shared BEHAVIOR (one clean footer-only frame) instead of two
/// divergent on-hit paths (one painting a footer, the other painting nothing —
/// which left a stale, clipped, footer-less frame frozen on screen).
pub(crate) fn draw_footer_only_frame<B: Backend<Error: Send + Sync + 'static>>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    stats: &crate::tui::widgets::FooterStats<'_>,
    theme: &pixtuoid_scene::theme::Theme,
    floor_info: Option<FloorInfo>,
    source_warning: Option<&str>,
) -> Result<()> {
    term.draw(|f| {
        let actual = f.area();
        paint_footer(f, scene, stats, actual, theme, floor_info, source_warning);
    })?;
    Ok(())
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
//   * `draw_scene` -- ratatui half-block compression + label overlay
//     + bulletin notice + footer. Terminal-specific, runs inside
//     `term.draw`.

pub fn draw_scene<B: Backend<Error: Send + Sync + 'static>>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    ctx: &mut DrawCtx<'_>,
) -> Result<Option<Arc<Layout>>> {
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

    // The footer's tallies, assembled ONCE (spine 1): per-state counts from the
    // projected floor `scene`, office-wide `per_floor`/`gateway` copied out of
    // `ctx` before the mutable buffer borrows below. `per_floor` is a small Copy
    // array, so the local owns it and `FooterStats` can borrow it across the
    // too-small early-returns and the main paint.
    let footer_per_floor = ctx.per_floor;
    let footer_stats = crate::tui::widgets::FooterStats {
        counts: crate::tui::widgets::scene_stats(scene),
        per_floor: &footer_per_floor,
        gateway: ctx.gateway,
        audio_audible: ctx.audio_audible,
        volume_flash: ctx.volume_flash,
    };

    if scene_rect.width < MIN_SCENE_WIDTH || scene_rect.height < MIN_SCENE_HEIGHT {
        draw_footer_only_frame(
            term,
            scene,
            &footer_stats,
            theme,
            floor_info,
            source_warning,
        )?;
        return Ok(None);
    }

    let buf_w = scene_rect.width;
    let buf_h = scene_rect.height.saturating_mul(2);
    ctx.buf.resize_fill(buf_w, buf_h, theme.surface.bg_fallback);
    // Always compute maximum layout capacity — floor overflow handles the rest.
    // The shared memoized prologue (FloorCtx::frame_layout): compute + the router
    // corridor re-point, cached on (w, h, seed) so a steady frame skips the
    // mask-stamp + coarse BFS.
    let Some(layout) = ctx.store.frame_layout(buf_w, buf_h, floor.floor_seed) else {
        draw_footer_only_frame(
            term,
            scene,
            &footer_stats,
            theme,
            floor_info,
            source_warning,
        )?;
        return Ok(None);
    };

    let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
        // Reborrow: `ctx.store`/`ctx.buf` are used again below (RouteCtx
        // hit-tests, the dim, the flush).
        store: &mut *ctx.store,
        buf: &mut *ctx.buf,
        scene,
        layout: &layout,
        pack,
        now,
        theme,
        floor,
        active_pet: ctx.active_pet,
        floor_pet: ctx.floor_pet,
        coffee: ctx.coffee,
        chitchat_state: ctx.chitchat_state,
        debug_walkable: ctx.debug_walkable,
    });
    ctx.last_pet_pos = pixel_result.pet_pos;
    ctx.last_mascot_pos = pixel_result.mascot_pos;
    ctx.chitchat_bubbles = pixel_result.chitchat_bubbles;
    ctx.new_coffee_carriers = pixel_result.new_coffee_carriers;
    ctx.occupied_waypoints = pixel_result.occupied_waypoints;

    let mouse_pos = ctx.mouse_pos;
    let hovered = mouse_pos.and_then(|(mx, my)| {
        hit_test_agent(scene, &layout, now, &mut ctx.store.route_ctx(), mx, my)
    });

    // Modal backdrop: DIM the office by the loop-computed `onboarding.dim` (ramps
    // in on open, back out on the close fade) — the room "lowers the lights" so the
    // welcome card pops. The card (`onboarding.open`) is decoupled from the dim, so
    // the office keeps fading back up for a beat AFTER the card is gone. The card
    // itself paints opaque on top.
    if ctx.onboarding.dim < 0.999 {
        apply_dim(ctx.buf, ctx.onboarding.dim);
    }

    let buf = &ctx.buf;
    let theme_picker = ctx.theme_picker;
    let chitchat_bubbles = &ctx.chitchat_bubbles;
    term.draw(|f| {
        // Re-derive rects from the actual frame buffer to guard against
        // terminal resize between term.size() and term.draw().
        let actual_full = f.area();
        let actual_scene = crate::tui::renderer::scene_rect(actual_full);
        paint_footer(
            f,
            scene,
            &footer_stats,
            actual_full,
            theme,
            floor_info,
            source_warning,
        );
        flush_buffer_to_term(f, buf, actual_scene);
        paint_label_widgets(
            f,
            scene,
            &layout,
            now,
            &mut ctx.store.route_ctx(),
            actual_scene,
            hovered,
            theme,
        );
        paint_chitchat_bubbles(f, chitchat_bubbles, actual_scene, theme);
        paint_wall_display(
            f,
            scene,
            actual_scene,
            now,
            footer_stats.counts,
            floor_info,
            footer_stats.gateway,
            theme,
        );
        if let Some(door) = layout.door {
            let current = floor_info.map(|fi| fi.current).unwrap_or(1);
            paint_elevator_indicator(f, door, current, actual_scene, theme);
        }
        if let (Some(agent_id), Some((mx, my))) = (hovered, mouse_pos) {
            paint_hover_tooltip(f, scene, agent_id, mx, my, actual_scene, now, theme);
        }
        if hovered.is_none() {
            if let Some((mx, my)) = mouse_pos {
                // Priority chain: coffee machine > pet (cursor-over only) >
                // gateway mascot > furniture. `.filter` keeps the pet/mascot
                // arms single branches so a present-but-not-hit hover falls
                // through to the next arm (no per-branch duplication).
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
                } else if let Some(m) = ctx
                    .last_mascot_pos
                    .filter(|m| hit_test_mascot(m.pos, mx, my))
                {
                    paint_mascot_tooltip(
                        f,
                        m.name,
                        m.busy,
                        m.degraded,
                        m.active_sessions,
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
        paint_overlays(
            f,
            theme_picker,
            ctx.dashboard,
            ctx.connection,
            ctx.popup_scale,
            ctx.help_open,
            ctx.onboarding,
            now,
            actual_full,
            theme,
        );
    })?;
    Ok(Some(layout))
}

/// The modal-overlay dispatch shared by `draw_scene` (normal path) and
/// `TuiRenderer::render_transition` (the floor-slide path): theme picker → dashboard →
/// Sources panel → version popup → help, each gated by its own state and drawn
/// at the same `bounds` (the full terminal area). Centralized so the two draw
/// paths can't drift in ordering or args; behavior-identical to the inlined
/// blocks it replaced.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_overlays(
    f: &mut ratatui::Frame<'_>,
    theme_picker: Option<usize>,
    dashboard: &crate::tui::dashboard::DashboardFrame,
    connection: &crate::tui::connection::ConnectionFrame,
    popup_scale: f32,
    help_open: bool,
    onboarding: &crate::tui::welcome::OnboardingFrame,
    now: SystemTime,
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    if let Some(idx) = theme_picker {
        paint_theme_picker(f, idx, bounds, theme);
    }
    if dashboard.open {
        paint_dashboard(
            f,
            &dashboard.rows,
            dashboard.selected,
            dashboard.scroll,
            now,
            bounds,
            theme,
        );
    }
    if connection.open {
        paint_connection_panel(
            f,
            &connection.rows,
            &connection.live,
            connection.selected,
            connection.confirm,
            connection.result.as_deref(),
            &connection.socket_line,
            now,
            bounds,
            theme,
        );
    }
    if popup_scale > 0.0 {
        if let Some(notes) = crate::version::release_notes(env!("CARGO_PKG_VERSION")) {
            paint_version_popup(
                f,
                env!("CARGO_PKG_VERSION"),
                notes,
                bounds,
                theme,
                popup_scale,
            );
        }
    }
    if help_open {
        // Center in `bounds` (the full terminal area, not the scene rect) so the
        // overlay sits at the same vertical center as the theme picker / version
        // popup, which both use the full area.
        paint_help_overlay(f, bounds, theme);
    }
    // Onboarding is the TOP of the precedence chain — painted last so it covers
    // every other overlay (it's modal-exclusive by dispatch, so in practice no
    // other overlay is open underneath, but topmost is the safe order).
    if onboarding.open {
        paint_welcome(f, onboarding, bounds, theme);
    }
}

pub(super) fn flush_buffer_to_term_at_offset(
    f: &mut ratatui::Frame<'_>,
    buf: &RgbBuffer,
    scene_rect: Rect,
    y_offset: i32,
) {
    let term_buf = f.buffer_mut();
    let term_area = term_buf.area;
    let w = buf.width() as usize;
    let cell_rows = (buf.height() / 2) as usize;
    for cy in 0..cell_rows {
        let target_y = cy as i32 + y_offset;
        if target_y < 0 || target_y >= scene_rect.height as i32 {
            continue;
        }
        for cx in 0..(buf.width() as usize) {
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
            let fg = buf.as_slice()[py_top * w + cx];
            let bg = buf.as_slice()[py_bot * w + cx];
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

/// Multiply every pixel of `buf` down by `factor` — the modal-backdrop dim the
/// onboarding overlay lowers the office to so the opaque welcome card pops.
/// Shared by BOTH render paths: `draw_scene`'s single composited buffer and
/// `render_transition`'s two sliding per-floor buffers (a floor change mid-open
/// dims both halves), so the "lights lowered" look can't drift between them. The
/// `factor >= 0.999` no-op short-circuit is the caller's (both gate on it).
pub(crate) fn apply_dim(buf: &mut RgbBuffer, factor: f32) {
    for px in buf.as_mut_slice() {
        px.r = (px.r as f32 * factor) as u8;
        px.g = (px.g as f32 * factor) as u8;
        px.b = (px.b as f32 * factor) as u8;
    }
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

    // A zero-HEIGHT rect that sits STRICTLY inside both entry guards (its x/y are
    // well inside `bounds`, and `rect.x + rect.width > bounds.x`,
    // `rect.y + rect.height == rect.y > bounds.y`) clears lines 149/152 and is
    // only rejected by the line-160 collapse guard (`bot <= y`). The existing
    // zero-SIZE test uses a zero-WIDTH rect, which exits early at line 152, so it
    // never reaches 160.
    #[test]
    fn clip_widget_rect_zero_height_inside_bounds_returns_none() {
        let zero_h = Rect {
            x: 2,
            y: 2,
            width: 4,
            height: 0,
        };
        let b = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(clip_widget_rect(zero_h, b), None);

        // Mutation-killer: a non-degenerate rect at the SAME position still
        // clips to itself. If the line-160 guard were deleted (so `zero_h`
        // returned `Some` with height 0), this pair would still pass — so the
        // first assert is what pins the guard, and this one pins that the guard
        // doesn't over-reject a real rect.
        let inside = Rect {
            x: 2,
            y: 2,
            width: 4,
            height: 3,
        };
        assert_eq!(clip_widget_rect(inside, b), Some(inside));
    }

    fn rgb(r: u8, g: u8, b: u8) -> pixtuoid_core::sprite::Rgb {
        pixtuoid_core::sprite::Rgb { r, g, b }
    }

    /// The cell symbol a flushed half-block carries (upper-half block).
    const HALF_BLOCK: &str = "\u{2580}";

    // Lines 499-500: columns whose terminal x falls past the scene rect's right
    // edge are skipped (a buffer wider than the rect). Deleting the `continue`
    // would paint half-blocks past the rect onto the footer column band.
    #[test]
    fn flush_skips_columns_past_scene_right_edge() {
        let mut term =
            Terminal::new(ratatui::backend::TestBackend::new(10, 6)).expect("test backend");
        // buf is WIDER (6) than the scene rect (width 4); cell_rows = 4/2 = 2.
        let buf = RgbBuffer::filled(6, 4, rgb(10, 20, 30));
        let rect = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        term.draw(|f| flush_buffer_to_term_at_offset(f, &buf, rect, 0))
            .expect("draw");
        let term_buf = term.backend().buffer();
        // In-rect columns 0..4 carry the half-block.
        for x in 0..4u16 {
            assert_eq!(
                term_buf.cell((x, 0)).unwrap().symbol(),
                HALF_BLOCK,
                "column {x} is inside the rect and must be painted"
            );
        }
        // Columns 4 and 5 (the buffer overhang) are left untouched.
        for x in 4..6u16 {
            assert_ne!(
                term_buf.cell((x, 0)).unwrap().symbol(),
                HALF_BLOCK,
                "column {x} is past the rect's right edge and must be skipped"
            );
        }
    }

    // Lines 502-503: cells whose x/y fall outside the actual terminal buffer are
    // skipped (a scene rect larger than the frame, a resize race). Deleting the
    // `continue` would index `term_buf[(x, y)]` out of bounds and panic.
    #[test]
    fn flush_skips_cells_past_terminal_bounds() {
        let mut term =
            Terminal::new(ratatui::backend::TestBackend::new(4, 3)).expect("test backend");
        // buf 8x8 (cell_rows = 4) flushed into a rect that EXCEEDS the 4x3 backend.
        let buf = RgbBuffer::filled(8, 8, rgb(40, 50, 60));
        let rect = Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 6,
        };
        // The draw must not panic despite the oversized rect.
        term.draw(|f| flush_buffer_to_term_at_offset(f, &buf, rect, 0))
            .expect("draw must not panic on an oversized rect");
        let term_buf = term.backend().buffer();
        // Exactly the in-bounds intersection (0..4 × 0..3) is painted.
        for y in 0..3u16 {
            for x in 0..4u16 {
                assert_eq!(
                    term_buf.cell((x, y)).unwrap().symbol(),
                    HALF_BLOCK,
                    "in-bounds cell ({x},{y}) must be painted"
                );
            }
        }
    }
}
