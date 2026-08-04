//! Terminal-coupled rendering: the `draw_scene` orchestrator, the half-block
//! flush, and the label/tooltip/notice widget overlays. The pure-pixel pass
//! (floor/walls/decor/characters -> `RgbBuffer`) lives in the
//! `pixtuoid_scene::pixel_painter` engine crate.

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

/// Multi-floor display state, bundled so a renderer never sees the navigation
/// breadcrumb without the global agent count.
#[derive(Debug, Clone, Copy)]
pub struct FloorInfo {
    /// 1-indexed, for display (e.g. "F2/3").
    pub current: usize,
    pub total_floors: usize,
    pub total_agents: usize,
}

pub struct DrawCtx<'a> {
    pub buf: &'a mut RgbBuffer,
    /// `buf` is deliberately NOT part of this group: it is a sibling of the
    /// `FloorCtx` on a `PerFloor`, borrowed disjointly.
    pub store: &'a mut pixtuoid_scene::floor::FloorCtx,
    pub mouse_pos: Option<(u16, u16)>,
    /// Walkable/approach/route debug layer toggle (`w`) — transient, never
    /// persisted to config.
    pub debug_walkable: bool,
    pub theme: &'a pixtuoid_scene::theme::Theme,
    pub theme_picker: Option<usize>,
    /// `Some` iff there's more than one floor.
    pub floor_info: Option<FloorInfo>,
    /// Office-wide per-floor state tally, bucketed from the FULL, un-projected
    /// scene. ALWAYS present — unlike `floor_info` it never vanishes for a
    /// single-floor office. Distinct from the footer's per-state integers
    /// (`scene_stats` on the PROJECTED floor): don't derive one from the other.
    pub per_floor: [crate::tui::widgets::StateCounts; pixtuoid_core::state::MAX_FLOORS],
    /// Worst-of daemon-liveness rollup; `None` = no daemon configured (chip
    /// suppressed).
    pub gateway: Option<pixtuoid_core::state::DaemonState>,
    /// "You would hear sound right now" — drives the footer's ♩ glyph.
    pub audio_audible: bool,
    /// Transient volume readout, in percent.
    pub volume_flash: Option<u8>,
    pub floor: pixtuoid_scene::floor::FloorMeta,
    pub active_pet: Option<&'a PetState>,
    pub last_pet_pos: Option<PetFrame>,
    /// Every gateway mascot's frame this render, for hover identity.
    pub last_mascots: Vec<MascotFrame>,
    /// `None` when no pets are configured or none maps to this floor seed.
    pub floor_pet: Option<&'a pixtuoid_scene::pet::Pet>,
    pub chitchat_state: &'a mut std::collections::HashMap<
        pixtuoid_scene::chitchat::VenueKey,
        pixtuoid_scene::chitchat::ActiveChitchat,
    >,
    pub chitchat_bubbles: Vec<pixtuoid_scene::chitchat::ChitchatBubble>,
    /// Key present = has a desk cup; value = the steam-window anchor.
    pub coffee: &'a std::collections::HashMap<pixtuoid_core::AgentId, std::time::SystemTime>,
    /// Out-param: the caller records these into the persistent `CoffeeState`.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
    /// Out-param: the audio cue tracker's appliance feed.
    pub occupied_waypoints: std::collections::HashSet<usize>,
    /// Animated scale for the version popup (0.0 = hidden, 1.0 = fully shown).
    pub popup_scale: f32,
    pub help_open: bool,
    /// Footer warning when a source has died; `None` while healthy.
    pub source_warning: Option<&'a str>,
    pub dashboard: &'a crate::tui::dashboard::DashboardFrame,
    pub connection: &'a crate::tui::connection::ConnectionFrame,
    pub onboarding: &'a crate::tui::welcome::OnboardingFrame,
}

/// Clip a widget rect to fit inside `bounds`; `None` when nothing survives.
/// Prevents ratatui's "index outside of buffer" panic when label/notice widgets
/// land near the right or bottom edge.
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
/// for a footer-only draw. Shared by both "too small" gates so they can't drift.
pub(crate) const MIN_SCENE_WIDTH: u16 = 20;
pub(crate) const MIN_SCENE_HEIGHT: u16 = 12;

/// How many rows at the bottom of the terminal the status footer owns — THE
/// authority for that count, not three copies of a bare `1`.
pub(crate) const FOOTER_ROWS: u16 = 1;

pub(crate) fn scene_rect(full: Rect) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: full.width,
        height: full.height.saturating_sub(FOOTER_ROWS),
    }
}

pub(crate) struct OverlayFrame<'a> {
    pub theme_picker: Option<usize>,
    pub dashboard: &'a crate::tui::dashboard::DashboardFrame,
    pub connection: &'a crate::tui::connection::ConnectionFrame,
    pub popup_scale: f32,
    pub help_open: bool,
    pub onboarding: &'a crate::tui::welcome::OnboardingFrame,
}

/// Paint the footer-only frame shown when the terminal is too small to render the
/// office — the SHARED body of BOTH too-small gates, so a hit paints one clean
/// footer-only frame instead of leaving a stale, clipped, footer-less one frozen.
///
/// It paints the modal overlays too: every modal's key handler stays live at any
/// size, so suppressing the overlay here made `?`/`s`/`Tab` toggle something
/// invisible on any terminal below the office layout's minimum — and first run
/// opens the onboarding modal there.
// The footer half travels as one `FooterStats` and the modal half as one
// `OverlayFrame`; what remains is irreducible.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_footer_only_frame<B: Backend<Error: Send + Sync + 'static>>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    stats: &crate::tui::widgets::FooterStats<'_>,
    theme: &pixtuoid_scene::theme::Theme,
    floor_info: Option<FloorInfo>,
    source_warning: Option<&str>,
    overlays: &OverlayFrame<'_>,
    now: SystemTime,
) -> Result<()> {
    term.draw(|f| {
        let actual = f.area();
        paint_footer(f, scene, stats, actual, theme, floor_info, source_warning);
        paint_overlays(f, overlays, now, actual, theme);
    })?;
    Ok(())
}

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

    // `per_floor` is copied out of `ctx` before the mutable buffer borrows below,
    // so `FooterStats` can borrow it across the early-returns and the main paint.
    let footer_per_floor = ctx.per_floor;
    let footer_stats = crate::tui::widgets::FooterStats {
        counts: crate::tui::widgets::scene_stats(scene),
        per_floor: &footer_per_floor,
        gateway: ctx.gateway,
        audio_audible: ctx.audio_audible,
        volume_flash: ctx.volume_flash,
    };
    let overlays = OverlayFrame {
        theme_picker: ctx.theme_picker,
        dashboard: ctx.dashboard,
        connection: ctx.connection,
        popup_scale: ctx.popup_scale,
        help_open: ctx.help_open,
        onboarding: ctx.onboarding,
    };

    if scene_rect.width < MIN_SCENE_WIDTH || scene_rect.height < MIN_SCENE_HEIGHT {
        draw_footer_only_frame(
            term,
            scene,
            &footer_stats,
            theme,
            floor_info,
            source_warning,
            &overlays,
            now,
        )?;
        return Ok(None);
    }

    let buf_w = scene_rect.width;
    let buf_h = scene_rect.height.saturating_mul(2);
    ctx.buf.resize_fill(buf_w, buf_h, theme.surface.bg_fallback);
    let Some(layout) = ctx.store.frame_layout(buf_w, buf_h, floor.floor_seed) else {
        draw_footer_only_frame(
            term,
            scene,
            &footer_stats,
            theme,
            floor_info,
            source_warning,
            &overlays,
            now,
        )?;
        return Ok(None);
    };

    let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
        store: &mut *ctx.store,
        buf: &mut *ctx.buf,
        // The half-block flush is the classic profile: buffer pixels ARE
        // layout units, so the terminal painter stays at ONE.
        scale: pixtuoid_scene::render_scale::RenderScale::ONE,
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
    ctx.last_mascots = pixel_result.mascots;
    ctx.chitchat_bubbles = pixel_result.chitchat_bubbles;
    ctx.new_coffee_carriers = pixel_result.new_coffee_carriers;
    ctx.occupied_waypoints = pixel_result.occupied_waypoints;

    let mouse_pos = ctx.mouse_pos;
    let hovered = mouse_pos.and_then(|(mx, my)| {
        hit_test_agent(scene, &layout, now, &mut ctx.store.route_ctx(), mx, my)
    });

    // The dim is decoupled from `onboarding.open`, so the office keeps fading back
    // up for a beat AFTER the card is gone.
    if ctx.onboarding.dim < 0.999 {
        apply_dim(ctx.buf, ctx.onboarding.dim);
    }

    let buf = &ctx.buf;
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
                // `.filter` keeps the pet arm a single branch, so a
                // present-but-not-hit pet falls through to the next arm.
                let pet_hit = ctx
                    .last_pet_pos
                    .filter(|f| hit_test_pet(f.kind, f.pos, f.anim, mx, my));
                if hit_test_coffee_machine(&layout, mx, my) {
                    paint_coffee_tooltip(f, mx, my, actual_scene, theme);
                } else if let Some(PetFrame { anim, kind, .. }) = pet_hit {
                    let on_cooldown = ctx.active_pet.is_some_and(|p| p.is_active(now));
                    // `last_pet_pos` is only `Some` on the normal render path,
                    // where it was written from `floor_pet` — so the kinds agree
                    // and the `default_name` arm is not a live path.
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
                } else if let Some(m) = topmost_mascot_at(&ctx.last_mascots, mx, my) {
                    paint_mascot_tooltip(
                        f,
                        m.name,
                        m.instance.as_deref(),
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
        paint_overlays(f, &overlays, now, actual_full, theme);
    })?;
    Ok(Some(layout))
}

/// The modal-overlay dispatch, centralized so the three draw paths can't drift in
/// ordering or args. `bounds` is the FULL terminal area — a modal is centered over
/// the whole frame, and `PanelGeometry` keeps it off the footer row itself.
pub(super) fn paint_overlays(
    f: &mut ratatui::Frame<'_>,
    ov: &OverlayFrame<'_>,
    now: SystemTime,
    bounds: Rect,
    theme: &pixtuoid_scene::theme::Theme,
) {
    let &OverlayFrame {
        theme_picker,
        dashboard,
        connection,
        popup_scale,
        help_open,
        onboarding,
    } = ov;
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
        paint_help_overlay(f, bounds, theme);
    }
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

/// Multiply every pixel of `buf` down by `factor` — the modal-backdrop dim. The
/// `factor >= 0.999` no-op short-circuit is the CALLER's; both gate on it.
pub(crate) fn apply_dim(buf: &mut RgbBuffer, factor: f32) {
    for px in buf.as_mut_slice() {
        px.r = (px.r as f32 * factor) as u8;
        px.g = (px.g as f32 * factor) as u8;
        px.b = (px.b as f32 * factor) as u8;
    }
}

/// The mascot under the cursor that the user can actually SEE — the one painted on
/// TOP, not merely the first in the roster. The painter y-sorts its drawables
/// ascending, so among overlapping mascots the greatest `pos.y` is drawn LAST and
/// occludes the others; picking the FIRST hit named whichever gateway sorted
/// earliest in the roster, regardless of which lobster was visible.
fn topmost_mascot_at(
    mascots: &[pixtuoid_scene::pixel_painter::MascotFrame],
    mx: u16,
    my: u16,
) -> Option<&pixtuoid_scene::pixel_painter::MascotFrame> {
    mascots
        .iter()
        .filter(|m| hit_test_mascot(m.pos, m.w, m.h, mx, my))
        .max_by_key(|m| m.pos.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hovering_overlapping_mascots_names_the_one_painted_on_top() {
        use pixtuoid_scene::layout::Point;
        use pixtuoid_scene::pixel_painter::MascotFrame;
        let frame = |instance: &str, x: u16, y: u16| MascotFrame {
            pos: Point { x, y },
            w: 14,
            h: 12,
            name: "OpenClaw",
            instance: Some(instance.to_string()),
            busy: false,
            degraded: false,
            active_sessions: 0,
        };
        // `hit_test_mascot` centres the 14x12 box on `pos` and DOUBLES the cell y
        // (half-block): 18789 covers x[33,47) my[22,28) and 19789 x[37,51) my[25,31)
        // — overlapping at x[37,47) my[25,28), with 19789 lower and so painted last.
        let mascots = vec![frame("18789", 40, 50), frame("19789", 44, 56)];
        let hit = |mx, my| {
            topmost_mascot_at(&mascots, mx, my)
                .and_then(|m| m.instance.clone())
                .unwrap_or_else(|| "none".into())
        };

        assert_eq!(hit(40, 26), "19789", "the visible lobster must be named");
        assert_eq!(hit(34, 23), "18789");
        assert_eq!(hit(49, 29), "19789");
        assert_eq!(hit(5, 5), "none");
        let reversed = vec![frame("19789", 44, 56), frame("18789", 40, 50)];
        assert_eq!(
            topmost_mascot_at(&reversed, 40, 26)
                .and_then(|m| m.instance.clone())
                .as_deref(),
            Some("19789"),
            "the pick must be by paint order, not slice position"
        );
    }

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

    // A zero-HEIGHT rect sits STRICTLY inside both entry guards, so only the final
    // collapse guard (`bot <= y`) can reject it — unlike the zero-WIDTH rect above,
    // which exits early.
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

    const HALF_BLOCK: &str = "\u{2580}";

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
        for x in 0..4u16 {
            assert_eq!(
                term_buf.cell((x, 0)).unwrap().symbol(),
                HALF_BLOCK,
                "column {x} is inside the rect and must be painted"
            );
        }
        for x in 4..6u16 {
            assert_ne!(
                term_buf.cell((x, 0)).unwrap().symbol(),
                HALF_BLOCK,
                "column {x} is past the rect's right edge and must be skipped"
            );
        }
    }

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
        term.draw(|f| flush_buffer_to_term_at_offset(f, &buf, rect, 0))
            .expect("draw must not panic on an oversized rect");
        let term_buf = term.backend().buffer();
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
