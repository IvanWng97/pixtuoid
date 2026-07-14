//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! Split from `tui/renderer.rs` to separate the pixel-painting pipeline
//! (called by any renderer impl — `TuiRenderer`, a future web canvas, PNG
//! export, GIF capture) from the ratatui-coupled half-block flush + widget
//! overlay (terminal lifecycle lives with the event loop in `tui/mod.rs`).
//!
//! `render_to_rgb_buffer` is the public entry point, and is itself TWO
//! phases behind one seam: [`sim_step`] (the `sim` module) advances the
//! world — motion, poses, lighting, chitchat — with no pixel access and
//! returns an immutable [`SimFrame`]; the paint pass (`paint_frame`)
//! consumes `&SimFrame` and mutates only the buffer + the paint-local
//! `FrameCache`. Everything else is private to this module except
//! `character_anchor`, which `widgets.rs` uses for label placement and
//! `hit_test.rs` for mouse hit-testing.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Rgb, RgbBuffer, Sprite};
use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex};
use pixtuoid_core::{AgentSlot, SceneState};

use crate::chitchat::{ActiveChitchat, ChitchatBubble};
use crate::floor::LightingState;
use crate::frame_cache::FrameCache;
use crate::layout::{
    z_sort_row, Anchor, Layout, PlantItem, PodDecorItem, Point, Size, WallDecorItem, WallSegment,
    DESK_W, ELEVATOR_H, ELEVATOR_W,
};
use crate::motion::MotionState;
use crate::pet::PetFrame;

/// Milliseconds since the Unix epoch for `now` (0 if the clock is before it).
/// The wall-clock decode the pixel-pass animation timers share — 8 callers read
/// better with this name than an inline `elapsed_ms(now, UNIX_EPOCH)`. A one-line
/// forwarder to the scene-wide `anim::elapsed_ms` (same saturate-to-0 semantics).
pub(super) fn epoch_ms(now: SystemTime) -> u64 {
    crate::anim::elapsed_ms(now, SystemTime::UNIX_EPOCH)
}

/// Result of the pure-pixel pass — carries the resolved cat position
/// (for hit-testing), active chitchat bubbles (for widget rendering),
/// and agent ids that were seen carrying coffee this frame (so the
/// caller can persist them into its `CoffeeState`).
pub struct PixelPassResult {
    pub pet_pos: Option<PetFrame>,
    /// The gateway mascot's resolved frame this tick (for hover identity).
    /// `None` when no gateway is present.
    pub mascot_pos: Option<MascotFrame>,
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agent ids observed in `Walking { carrying_coffee: true }` this
    /// frame. The caller inserts them into the persistent
    /// `CoffeeState` (carrier + steam-window stamp in one map).
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
}

/// The gateway mascot's screen frame — enough to hover-identify it (which
/// gateway, how busy). The wandering position is recomputed every frame, so
/// this is recaptured each render like `PetFrame`.
#[derive(Clone, Copy)]
pub struct MascotFrame {
    pub pos: Point,
    /// Human-readable gateway name (e.g. "OpenClaw").
    pub name: &'static str,
    /// An agent run is in flight (the tooltip's idle-vs-working verb). Keyed on
    /// the run state, NOT the session count — a single-user gateway holds one
    /// persistent session even at rest, so session count is a poor idle/busy tell.
    pub busy: bool,
    /// Gateway up but its model backend is failing every run (#317) — the tooltip
    /// reads "model error" and the lobster renders sickly red.
    pub degraded: bool,
    pub active_sessions: u32,
}

mod ambient;
mod anchors;
mod background;
mod debug_overlay;
mod drawable;
mod effects;
mod furniture;
mod glass;
mod palette;
mod seat;
mod sim;

pub use anchors::character_anchor;
// The ToolKind→glow-hue seam the binary's footer tints tool segments with, so a
// footer tool colour matches the sprite's monitor glow exactly.
pub use palette::tool_glow_for_kind;
// The γ3 widening that PR-450 planned for: the observation TYPES a
// `floor::FloorSession::observe` caller reads go pub WITH the facade;
// `sim_step` + `SimStores` (the per-call borrow-set) stay crate-internal —
// the session is the public entry to the sim tick.
pub(crate) use sim::{sim_step, SimStores};
// The flame-crown/ember colors, for render tests (floor.rs) to assert the
// REAL painted values (effects itself stays a private submodule) — test-only,
// hence the cfg: the lib target has no consumer.
#[cfg(test)]
pub(crate) use effects::{FLAME_DEEP, FLAME_TIP};
#[cfg(test)]
pub(crate) use furniture::COOLER_WATER;
pub use sim::{CharacterGlow, CharacterPlacement, SimFrame};

/// The coffee-machine sub-region within the pantry counter sprite, as sprite-local
/// column ranges `[start, end)` per pantry size (the 32-wide `pantry` sprite vs the
/// 20-wide `pantry_small`). THE single source of truth shared by the steam-anchor
/// painter (`drawable`'s `WaypointPantry` arm) and the binary's
/// `hit_test_coffee_machine`, so the clickable machine box can't silently drift
/// from the painted art / steam anchor when the sprite is re-tuned. Pinned to the
/// steam anchor by `steam_anchor_sits_within_the_coffee_machine_columns`.
pub const PANTRY_COFFEE_COLS_LARGE: (u16, u16) = (11, 18);
pub const PANTRY_COFFEE_COLS_SMALL: (u16, u16) = (9, 12);

/// The neon wall-sign panel geometry, in PIXELS: origin `(X, Y)` and OUTER size
/// `W×H`, drawn with a `NEON_PANEL_BORDER`-px frame on every side. THE single
/// source of truth shared by the pixel painter (`paint_neon_panel`) and the
/// wall-clock collision clamp. A pixel column maps 1:1 to a terminal cell column
/// in the half-block flush, so these px widths ARE cell widths on the horizontal.
///
/// The board's TEXT overlay lives in the dark INTERIOR (`NEON_PANEL_INNER_*` = the
/// panel minus its frame): the binary's `tui::widgets::hud::paint_wall_display`
/// pins its cell-origin AND width to those, so the lit text can't overrun the
/// glowing frame. Laying text to the full OUTER `NEON_PANEL_W` overran it by the
/// border on each side (the board-overflow bug). Only the interior pair + the
/// outer width cross the crate boundary (`pub`); `X`/`Y`/`H`/`BORDER` have no
/// cross-crate consumer (`pub(crate)`, don't widen the semver surface).
pub(crate) const NEON_PANEL_X: u16 = 1;
pub(crate) const NEON_PANEL_Y: u16 = 1;
pub const NEON_PANEL_W: u16 = 30;
pub(crate) const NEON_PANEL_H: u16 = 8;
/// The frame thickness `paint_neon_panel` lights on every side (it reads THIS, so
/// the interior derivation below provably matches the pixels it leaves dark).
pub(crate) const NEON_PANEL_BORDER: u16 = 1;
/// The dark interior's left cell-origin (`X` + the frame) — where board text starts.
pub const NEON_PANEL_INNER_X: u16 = NEON_PANEL_X + NEON_PANEL_BORDER;
/// The dark interior's cell WIDTH (`W` minus the frame on both sides) — the board's
/// usable text width; `BOARD_W` pins to this.
pub const NEON_PANEL_INNER_W: u16 = NEON_PANEL_W - 2 * NEON_PANEL_BORDER;
/// The dark interior's top pixel-origin (`Y` + the frame) — where the floating /
/// wasm painters anchor the board's first text row over the panel.
pub const NEON_PANEL_INNER_Y: u16 = NEON_PANEL_Y + NEON_PANEL_BORDER;
/// The dark interior's pixel HEIGHT (`H` minus the frame on both sides).
pub const NEON_PANEL_INNER_H: u16 = NEON_PANEL_H - 2 * NEON_PANEL_BORDER;
// The interior must be a non-empty strict subset of the outer frame (catches a
// degenerate BORDER=0 / oversized-border config at compile time).
const _: () = assert!(NEON_PANEL_INNER_W > 0 && NEON_PANEL_INNER_W < NEON_PANEL_W);
const _: () = assert!(NEON_PANEL_INNER_H > 0 && NEON_PANEL_INNER_H < NEON_PANEL_H);

use anchors::compute_door_frame_idx;
use background::{
    daylight_floor_overlay, dim_floor_overlay, paint_ceiling_pool, paint_clock,
    paint_corridor_runner, paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel,
    paint_shadow, time_of_day_look, Ellipse,
};
use drawable::{
    gateway_mascot_def, mascot_position, paint_drawable, pet_position, Drawable, DrawableKind,
};
use glass::{
    paint_door_frame_v, paint_door_jamb_h, paint_glass_wall_h, paint_glass_wall_v,
    stitch_vertical_wall, DOOR_JAMB_PX, WALL_THICK_H_PX,
};
use palette::{agent_palette, outfit_seed_for, recolor_frame};
use seat::paint_character_at;

/// The weather names accepted by [`force_weather`], canonical order — for
/// `--weather` error text and the manifest drift-guard test. (The gallery
/// generator itself reads site/src/weather.json; the
/// `weather_gallery_manifest_matches_the_weather_enum` test keeps that manifest
/// aligned with this list.)
pub fn weather_names() -> Vec<&'static str> {
    background::Weather::ALL.iter().map(|w| w.name()).collect()
}

/// Force every subsequent render **on this thread** to a specific weather (by
/// name, case-insensitive), or `None` to restore the clock-based selection.
/// `snapshot --weather` drives the docs stills; the wasm `Office` (pixtuoid-web)
/// ALSO re-applies its OWN override every `step` — the hero backdrop defaults
/// `None` (clock-based), the VIBING playground sets its chip. It's a thread-local
/// shared by every `Office` in the one wasm module, so the last writer before a
/// render wins and each surface must set its own value each frame. `Err` carries
/// the valid names when `name` is unknown.
pub fn force_weather(name: Option<&str>) -> Result<(), Vec<&'static str>> {
    match name {
        None => {
            background::set_weather_override(None);
            Ok(())
        }
        Some(s) => match background::Weather::from_name(s) {
            Some(w) => {
                background::set_weather_override(Some(w));
                Ok(())
            }
            None => Err(weather_names()),
        },
    }
}

/// Whether the office's sky shows the SUN at hour-of-day `hour` (0..24), per the
/// engine's own `SUN_RISE_H`/`SUN_SET_H` window (`background/sky.rs`). Exposed so
/// the wasm painter's `Office::is_day` can hand the site's sky-slider the SAME
/// day/night boundary the office renders — one source of truth, no drift across
/// the Rust↔JS boundary.
pub fn hour_is_day(hour: f32) -> bool {
    background::hour_is_day(hour)
}

// The steam gate reads the SAME window `CoffeeState::record` refreshes on —
// a reference, not a second copy of the value.
const COFFEE_STEAM_WINDOW_SECS: u64 = crate::floor::CoffeeState::STEAM_WINDOW_SECS;

/// Z-sort offset from a center-pinned sprite's center to its SOUTH (front) row.
/// A sprite of height `h` blitted at `py = center - h/2` occupies rows
/// `[py, py + h - 1]`, so its south row is `center + (h - 1) / 2`. This works
/// for BOTH parities: the naive `h/2 - 1` is one row short for ODD `h` (e.g. the
/// 11px whiteboard would sort one row in front of its own base). The z-key must
/// land ON the south row — one row past it lets the sprite paint over a
/// character standing immediately in front.
fn center_pin_south_offset(h: u16) -> u16 {
    h.saturating_sub(1) / 2
}

/// South-row (base) offset of the floor-lamp sprite, derived from the one
/// furniture table so the halo / shadow / z-anchor all move together if the
/// lamp's visual height changes (locked by a unit test).
fn floor_lamp_south_offset() -> u16 {
    center_pin_south_offset(
        crate::layout::furniture_def(crate::layout::Furniture::FloorLamp)
            .visual
            .h,
    )
}

/// Bundled input for the pixel-painting pass. Constructed at the `render_floor`
/// / `draw_scene` call site.
pub struct PixelCtx<'a> {
    /// The per-floor sim/paint STORES borrowed as ONE group (was seven flat
    /// fields: `router`/`overlay`/`history`/`cache`/`motion`/`light` +
    /// `door_anim_max_ms`). `render_to_rgb_buffer` reads them as disjoint field
    /// projections (`store.router`, `store.overlay`, …). `buf` stays a SEPARATE
    /// field: it is a sibling of the `FloorCtx` on a `PerFloor`, borrowed
    /// disjointly by a multi-floor painter's `split_at_mut`.
    pub store: &'a mut crate::floor::FloorCtx,
    pub buf: &'a mut RgbBuffer,
    pub scene: &'a SceneState,
    pub layout: &'a Layout,
    pub pack: &'a Pack,
    pub now: SystemTime,
    pub theme: &'a crate::theme::Theme,
    pub floor: crate::floor::FloorMeta,
    pub active_pet: Option<&'a crate::pet::PetState>,
    /// The pet on this floor (kind drives the sprite; name is unused here — the
    /// pixel pass doesn't render the name, the tooltip does).
    pub floor_pet: Option<&'a crate::pet::Pet>,
    /// Carrier → fetch-time view of [`crate::floor::CoffeeState`] (one map:
    /// key present = has a desk cup, value = steam-window anchor).
    pub coffee: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub chitchat_state: &'a mut HashMap<crate::chitchat::VenueKey, ActiveChitchat>,
    /// When set, composite the walkable / approach / route debug layer over the
    /// finished scene (the live `w` toggle). Off by default; transient.
    pub debug_walkable: bool,
}

/// The paint pass's borrow set — everything `paint_frame` may touch. The only
/// `&mut`s are the pixel buffer and the paint-local `FrameCache` (a render
/// cache, not a sim store); the sim stores are absent BY TYPE (`motion` is an
/// immutable view, read by the debug route overlay), so painting cannot move
/// the world — see the `sim` module docs for the classification.
struct PaintCtx<'a> {
    scene: &'a SceneState,
    layout: &'a Layout,
    pack: &'a Pack,
    now: SystemTime,
    buf: &'a mut RgbBuffer,
    cache: &'a mut FrameCache,
    theme: &'a crate::theme::Theme,
    floor: crate::floor::FloorMeta,
    active_pet: Option<&'a crate::pet::PetState>,
    floor_pet: Option<&'a crate::pet::Pet>,
    coffee: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    motion: &'a HashMap<pixtuoid_core::AgentId, MotionState>,
    door_anim_max_ms: u64,
    debug_walkable: bool,
}

pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    // Phase 1 — SIM: advance the world (motion/poses/lighting/chitchat),
    // producing no pixels. See `sim::sim_step`.
    let frame = sim_step(
        &mut SimStores {
            router: &mut ctx.store.router,
            overlay: &mut ctx.store.overlay,
            history: &mut ctx.store.history,
            motion: &mut ctx.store.motion,
            light: &mut ctx.store.light,
            chitchat: &mut *ctx.chitchat_state,
        },
        ctx.scene,
        ctx.layout,
        ctx.pack,
        ctx.coffee,
        ctx.floor.floor_idx,
        ctx.now,
    );
    // Phase 2 — PAINT: an immutable read of the SimFrame that mutates only
    // the buffer + the recolor cache. Painting the same frame twice is
    // byte-identical (pinned by `paint_frame_is_pure_and_byte_identical`).
    let (pet_pos, mascot_pos) = paint_frame(
        &mut PaintCtx {
            scene: ctx.scene,
            layout: ctx.layout,
            pack: ctx.pack,
            now: ctx.now,
            buf: &mut *ctx.buf,
            cache: &mut ctx.store.cache,
            theme: ctx.theme,
            floor: ctx.floor,
            active_pet: ctx.active_pet,
            floor_pet: ctx.floor_pet,
            coffee: ctx.coffee,
            motion: &ctx.store.motion,
            door_anim_max_ms: ctx.store.door_anim_max_ms,
            debug_walkable: ctx.debug_walkable,
        },
        &frame,
    );
    PixelPassResult {
        pet_pos,
        mascot_pos,
        chitchat_bubbles: frame.chitchat_bubbles,
        new_coffee_carriers: frame.new_coffee_carriers,
    }
}

/// The PAINT half of the frame: blit the world the sim already advanced.
/// Reads the [`SimFrame`] immutably; every positional/lifecycle decision was
/// made in `sim_step` — this pass only resolves presentation (theme colors,
/// sprite pixels) and composites. Returns the resolved pet + mascot frames
/// for the caller's hit-testing.
fn paint_frame(
    ctx: &mut PaintCtx<'_>,
    frame: &SimFrame,
) -> (Option<PetFrame>, Option<MascotFrame>) {
    let agents: &[AgentSlot] = &frame.agents;
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;

    // Compute time-of-day once per frame and pass to every paint
    // helper that depends on it. Avoids recomputing the chrono local
    // hour for each window + ceiling pool + lamp halo.
    let look = time_of_day_look(ctx.now, ctx.theme);
    // Wall band height tracks layout.top_margin (which is buf_h/4 with
    // a floor) — leaves a 4-px buffer between wall trim and cubicles.
    let top_wall_h = ctx.layout.wall_band_h();
    // The elevator door replaces the rightmost window — pass its x-range
    // so `paint_floor_and_walls` skips drawing a window that would
    // otherwise bleed through behind the elevator frame.
    let door_x_range = ctx.layout.door.map(|d| (d.x, d.x + ELEVATOR_W));
    paint_floor_and_walls(
        ctx.buf,
        buf_w,
        buf_h,
        ctx.now,
        &look,
        top_wall_h,
        door_x_range,
        ctx.theme,
        ctx.floor.altitude,
    );

    // Per-floor lighting: `sim_step` already ticked the fade state with the
    // current occupancy. `indoor_scale` smoothly travels from MIN_LEVEL
    // (empty + past debounce) to 1.0 (populated). Windows/skyline are
    // unaffected.
    let indoor_scale = frame.indoor_scale;
    // Empty floors get an extra floor-darken boost on top of the time-of-
    // day dim — there are no monitor/lamp light sources to balance against
    // the overhead darkness, so without the boost they read as "lights
    // off but room weirdly bright."
    let min_level = LightingState::MIN_LEVEL;
    let boost_ceiling = LightingState::EMPTY_FLOOR_DIM_BOOST;
    let empty_floor_boost = 1.0 + (1.0 - indoor_scale) * (boost_ceiling - 1.0) / (1.0 - min_level);

    // The night floor-dim dial (symmetric with `DAYLIGHT_FLOOR_LIFT` below); the
    // per-floor lighting offset it replaced was always 0 (indoor lighting is
    // uniform across floors), so this is now a flat constant.
    const NIGHT_FLOOR_DIM_STRENGTH: f32 = 0.45;
    let dim_strength = NIGHT_FLOOR_DIM_STRENGTH;
    dim_floor_overlay(
        ctx.buf,
        top_wall_h,
        buf_h,
        look.darkness * dim_strength * empty_floor_boost,
        ctx.theme,
    );
    // Daytime warm light-lift — the positive mirror of the night dim above.
    // Brightens/warms the floor in proportion to effective daylight
    // (`spill_strength` = `day_eff`), so sunny days read sunlit instead of flat
    // carpet. Independent of occupancy (sun enters an empty office too) and a
    // no-op at night where `day_eff` is 0. `DAYLIGHT_FLOOR_LIFT` is the dial.
    const DAYLIGHT_FLOOR_LIFT: f32 = 0.22;
    daylight_floor_overlay(
        ctx.buf,
        top_wall_h,
        buf_h,
        look.spill_strength * DAYLIGHT_FLOOR_LIFT,
    );
    let pool_strength = (0.15 + 0.30 * look.darkness) * indoor_scale;
    for desk in &ctx.layout.home_desks {
        paint_ceiling_pool(
            ctx.buf,
            Ellipse {
                cx: desk.x + DESK_W / 2,
                cy: desk.y.saturating_sub(2),
                half_w: 10,
                half_h: 5,
            },
            pool_strength,
            ctx.theme,
        );
    }
    // Two ceiling fluorescents over the pantry and a third over the
    // corridor so the floor is lit consistently with the lounge_band gone.
    if let Some(pr) = ctx.layout.pantry.map(|p| p.bounds) {
        paint_ceiling_pool(
            ctx.buf,
            Ellipse {
                cx: pr.x + pr.width / 2,
                cy: pr.y + pr.height / 2,
                half_w: 12,
                half_h: 6,
            },
            pool_strength,
            ctx.theme,
        );
    }
    if let Some(corridor) = ctx.layout.corridor {
        paint_ceiling_pool(
            ctx.buf,
            Ellipse {
                cx: corridor.x + corridor.width / 2,
                cy: corridor.y + corridor.height / 2,
                half_w: 14,
                half_h: 5,
            },
            pool_strength,
            ctx.theme,
        );
    }
    if let Some(lamp) = ctx.layout.floor_lamp {
        paint_floor_lamp_halo(
            ctx.buf,
            lamp.x,
            lamp.y + floor_lamp_south_offset(), // glow emanates from the lamp BASE, not the pole
            look.darkness * 0.55 * indoor_scale,
            ctx.theme,
        );
    }

    // Neon sign panel in the wall band — dark bg with glow border.
    // Text overlay (branding, dots, star link) is rendered by the ratatui
    // widget pass in renderer.rs::paint_wall_display.
    paint_neon_panel(
        ctx.buf,
        NEON_PANEL_X,
        NEON_PANEL_Y,
        NEON_PANEL_W,
        NEON_PANEL_H,
        ctx.now,
        ctx.theme,
    );

    // Live wall clock painted after the wall (so hands sit on top of it)
    // but before wall decor — the bookshelf etc. shouldn't cover it.
    // 7x7 sprite, center at clock_x+3; clamp so it never collides with
    // the neon panel on the left (its right edge + a 1px gap).
    let clock_x = (buf_w / 2)
        .saturating_sub(3)
        .max(NEON_PANEL_X + NEON_PANEL_W + 1);
    paint_clock(ctx.buf, clock_x, 1, ctx.now, ctx.theme);
    // Corridor runner — painted over the floor but BEFORE walls/decor
    // so walls cleanly overlap it where they cross.
    if let Some(corridor) = ctx.layout.corridor {
        paint_corridor_runner(ctx.buf, corridor, ctx.theme);
    }
    // Room dividers — frosted-glass partitions (see the module-level glass
    // helpers + WALL_THICK_*_PX). The VERTICAL (N-S, edge-on) wall paints here
    // in the background; the HORIZONTAL (E-W, face-on) wall is emitted into the
    // y-sorted drawable pass below so it composites over a walker standing
    // behind it. Stitch the vertical's joints (the layout emits geometry only;
    // the render thicknesses/offsets that open the gaps live here):
    //   • Top: a segment starting at top_margin abuts the north wall band,
    //     which ends 4 px higher at top_wall_h — raise it so no floor shows
    //     between window and wall. A segment just below a horizontal wall (the
    //     dual-meeting layout offsets it ~6 px to clear the cross wall) is
    //     bridged up to meet it.
    //   • Bottom: where the vertical meets a horizontal wall, extend it down by
    //     the horizontal's thickness to fill the inside corner (else its right
    //     columns leave an L-notch beside the horizontal run).
    let h_rows: Vec<u16> = ctx
        .layout
        .room_walls
        .iter()
        .filter(|w| w.start.y == w.end.y)
        .map(|w| w.start.y)
        .collect();
    for &WallSegment { start, end } in &ctx.layout.room_walls {
        if start.x != end.x {
            continue; // horizontal walls paint in the drawable pass
        }
        let (y_top, y_bot) =
            stitch_vertical_wall(start.y, end.y, ctx.layout.top_margin, top_wall_h, &h_rows);
        paint_glass_wall_v(ctx.buf, ctx.theme, start.x, y_top, y_bot.min(buf_h - 1));
    }
    // Door frames on VERTICAL walls paint here, over the strips above;
    // horizontal walls' frames ride their y-sorted RoomWallH drawable (the
    // background pass would be overpainted by it).
    for dw in ctx.layout.doorways.iter().filter(|d| d.start.x == d.end.x) {
        paint_door_frame_v(ctx.buf, ctx.theme, dw.start, dw.end);
    }

    // Meeting sofas + table and the kitchen island paint in the y-sorted
    // Drawable pass below — nothing room-scale belongs in this background
    // pass, or it double-paints under the sorted copy.

    // Procedural room fill — small pixel items that make rooms feel lived-in.
    // Ground footprint rule: walkable mask is NOT affected by these (they're
    // small items characters can walk around or over).
    // EVERY meeting room (#555: dense room 1 used to render bare — the
    // decor all keyed room 0).
    for mr in ctx.layout.meeting_rooms.iter().map(|r| r.bounds) {
        furniture::paint_notice_board(ctx.buf, mr, ctx.theme);

        // Coat rack is now a y-sorted DrawableKind::CoatRack (pushed in the
        // drawable pass) so characters in front occlude it / behind it are
        // occluded — was painted here in the background pass, always under
        // every character.

        furniture::paint_doormat(ctx.buf, mr, ctx.theme);
    }
    // Soft goods (decor arc) paint FIRST: floor-level mats sit under every
    // upright pantry fixture — on a narrow pantry the entry mat's box reaches
    // the water-cooler column, and mats-after-cooler clipped the cooler's
    // west edge (lens-1 catch at 120x160).
    furniture::paint_pantry_entry_mat(ctx.buf, ctx.layout, ctx.theme);
    furniture::paint_island_bar_mat(ctx.buf, ctx.layout, ctx.theme);
    if let Some(pr) = ctx.layout.pantry.map(|p| p.bounds) {
        furniture::paint_water_cooler(ctx.buf, pr, ctx.now, ctx.theme);
        furniture::paint_trash_bin(ctx.buf, pr);
    }

    // Shadow pass — soft floor shadows under desks + lounge furniture
    // so nothing floats. Painted BEFORE the y-sorted entity pass so
    // every entity sits on top of its own shadow. Strength is a
    // function of daylight so noon shadows are crisp and night shadows
    // are subtle.
    let shadow_strength = 0.5 - 0.3 * look.darkness;
    for desk in &ctx.layout.home_desks {
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: desk.x + DESK_W / 2,
                cy: desk.y + 7,
                half_w: DESK_W / 2 + 1,
                half_h: 3,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    for wp in &ctx.layout.waypoints {
        use crate::layout::WaypointKind;
        // Couch shadow is emitted once below (3 seat waypoints; per-seat
        // shadows would overlap). Printer is handled just after — its 4px-tall
        // sprite's south is pos.y+1, so the generic +2 would float 1px below.
        // Island stands are EMPTY FLOOR cells beside the island (the body
        // carries its own fitted shadow below) — a generic blob at each stand
        // painted phantom shadows left/right of the island.
        if matches!(
            wp.kind,
            WaypointKind::Couch | WaypointKind::Printer | WaypointKind::Island
        ) {
            continue;
        }
        // Fit the ellipse to the piece's actual sprite width — a flat 7
        // half-width doubles a narrow shelf's shadow. The .min(7) caps a
        // future wide piece from over-shadowing.
        let vis_w = crate::layout::furniture_def(wp.kind.furniture()).visual.w;
        let half_w = if vis_w > 0 { (vis_w / 2 + 1).min(7) } else { 7 };
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: wp.pos.x,
                cy: wp.pos.y + 2,
                half_w,
                half_h: 2,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    if let Some(island) = ctx.layout.pantry.and_then(|p| p.kitchen_island) {
        // One fitted shadow under the island BODY (its stands are skipped
        // above): south edge = the visual south row, width tracks the sprite.
        let vis = crate::layout::furniture_def(crate::layout::Furniture::KitchenIsland).visual;
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: island.x,
                cy: island.y + center_pin_south_offset(vis.h),
                half_w: vis.w / 2 + 1,
                half_h: 2,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    for wp in ctx
        .layout
        .waypoints
        .iter()
        .filter(|w| w.kind == crate::layout::WaypointKind::Printer)
    {
        // Flush against the printer's sprite south (pos.y+1).
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: wp.pos.x,
                cy: wp.pos.y + 1,
                half_w: 5,
                half_h: 1,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    if let Some(center) = ctx.layout.couch_sprite_center {
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: center.x,
                cy: center.y + 2,
                half_w: 7,
                half_h: 2,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    for &PlantItem { kind, pos } in &ctx.layout.plants {
        // Shadow sits under the sprite's south row — same offset the z-anchor
        // uses, off the same height (Succulent/Flower were floating at a fixed
        // +3 that only suited the taller Ficus/Tall).
        let cy = pos.y
            + center_pin_south_offset(crate::layout::furniture_def(kind.furniture()).visual.h);
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: pos.x,
                cy,
                half_w: 3,
                half_h: 1,
            },
            shadow_strength,
            ctx.theme,
        );
    }
    if let Some(lamp) = ctx.layout.floor_lamp {
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: lamp.x,
                cy: lamp.y + floor_lamp_south_offset(), // flush with the lamp base (sprite south)
                half_w: 2,
                half_h: 1,
            },
            shadow_strength,
            ctx.theme,
        );
    }

    // Ceiling halos gate on the sim's `seated_agents` so a tool-glow halo
    // never floats above an empty desk while its Active occupant is mid-walk
    // (entry/snap). `look` was already computed once per frame above —
    // forward it so the ambient sub-passes don't recompute
    // `time_of_day_look(now, theme)`.
    ambient::paint_ambient(ctx, &look, &frame.seated_agents);

    // --- Build the y-sortable middle pass -------------------------------
    //
    // Every entity gets an `anchor_y` representing its front-facing /
    // floor-touching row. Sort ascending and paint in order so things
    // closer to the camera (larger anchor_y) appear in front. This is
    // the painter's algorithm applied to a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::new();

    enqueue_desk_cubicles(ctx, agents, &frame.seated_agents, &mut drawables);

    enqueue_meeting_furniture(ctx.layout, &mut drawables);

    enqueue_lounge_pantry_appliances(ctx.layout, &frame.occupied_waypoints, &mut drawables);

    enqueue_pod_decor_and_plants(ctx.layout, &mut drawables);
    enqueue_floor_fixtures(ctx, agents, &mut drawables);
    enqueue_wall_decor(ctx.layout, &mut drawables);

    let resolved_pet_pos = enqueue_pet(ctx, agents, &mut drawables);
    let resolved_mascot_pos = enqueue_gateway_mascot(ctx, &mut drawables);

    enqueue_characters(ctx, frame, &mut drawables);

    enqueue_room_walls_h(ctx.layout, &mut drawables);

    // Stable sort (Rust's `sort_by_key` is stable) — ties preserve
    // insertion order. Insertion order above: decor first, characters
    // last, so a character tied with a piece of furniture paints
    // BEFORE the furniture (matches the prior pass-1 → pass-1.5
    // → pass-2 layering for waypoint couch / pantry counter).
    drawables.sort_by_key(|d| d.anchor_y);
    // Occlusion is emergent now: every overhanging object's mask footprint is a
    // shallow south-anchored ground strip, so a walker parks DEEP behind it and
    // the object's own sprite (y-sorted at its south base, painted after the
    // walker) hides their lower body — no snapshot, no synthetic back-cap.
    for d in &drawables {
        paint_drawable(d, ctx.buf, ctx.pack, ctx.cache, ctx.now, ctx.theme);
    }

    // Room-wide lightning bounce — LAST, so a Storm strike briefly flares the
    // whole interior (floor, walls, furniture, characters), not just the window
    // strip. No-op outside a strike / non-storm weather.
    background::paint_lightning_flash(ctx.buf, ctx.now, background::weather_state(ctx.now));

    // Debug layer (the `w` toggle) — composited LAST, over the finished scene:
    // walkable mask + approach sides + live A* routes. Off by default.
    if ctx.debug_walkable {
        debug_overlay::paint(ctx.buf, ctx.layout, ctx.scene, ctx.motion);
    }

    (resolved_pet_pos, resolved_mascot_pos)
}

/// Map the sim's resolved [`sim::CharacterPlacement`]s 1:1 onto y-sorted
/// drawables. Every positional decision (pose, anchor, z-key, sprite pick,
/// rank fan-out) was made by `sim_step`; the ONLY paint-side work here is
/// presentation — resolving the theme-free [`CharacterGlow`] to a `Theme`
/// color. The Character drawable borrows its agent from `frame.agents`, so
/// this is the ONE phase tied to the frame's lifetime `'a`.
fn enqueue_characters<'a>(
    ctx: &PaintCtx<'_>,
    frame: &'a SimFrame,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for p in &frame.characters {
        let agent = &frame.agents[p.agent_idx];
        let glow_tint = match p.glow {
            CharacterGlow::None => None,
            CharacterGlow::Thinking => Some(ctx.theme.tool_glow.default),
            CharacterGlow::Tool => palette::tool_glow_tint(agent, &ctx.theme.tool_glow),
        };
        drawables.push(Drawable {
            anchor_y: p.anchor_y,
            kind: DrawableKind::Character {
                agent,
                anim_name: p.anim_name,
                frame_idx: p.frame_idx,
                anchor: p.anchor,
                flip_x: p.flip_x,
                glow_tint,
                sleep_z_seed: p.sleep_z_seed,
                waiting_bubble: p.waiting_bubble,
                walking_dust_frame: p.walking_dust_frame,
            },
        });
    }
}

/// Horizontal (E-W) room dividers join the y-sort, anchored at their south
/// (front) edge so a character standing behind (north of) the wall is
/// composited over by the frosted glass rather than painting on top of it.
/// The vertical (edge-on) dividers already painted in the background pass.
/// Emitted LAST so a character tied with a wall row still paints behind it.
fn enqueue_room_walls_h<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.y == end.y {
            let (x0, x1) = (start.x.min(end.x), start.x.max(end.x));
            // A cut end abutting a doorway gets a jamb — flagged HERE because
            // the paint pass has no layout access. gap.start == this
            // segment's x1 (the run was cut there), gap.end == a segment x0.
            let jamb_right = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.start.x == x1);
            let jamb_left = layout
                .doorways
                .iter()
                .any(|d| d.start.y == start.y && d.end.y == start.y && d.end.x == x0);
            drawables.push(Drawable {
                anchor_y: start.y + (WALL_THICK_H_PX - 1),
                kind: DrawableKind::RoomWallH {
                    x0,
                    x1,
                    y_top: start.y,
                    jamb_left,
                    jamb_right,
                },
            });
        }
    }
}

/// The frame to paint for `idx`, clamped into range: a custom `--pack-dir`
/// animation whose sprite has fewer frames than the shared cycle's `frame_idx`
/// would otherwise yield `None` and vanish the sprite, so fall back to the
/// first frame. `None` only for a genuinely empty animation (the caller skips).
/// The ONE spelling of this out-of-range guard — was open-coded at four sites.
pub(super) fn frame_at(anim: &Sprite, idx: usize) -> Option<&Frame> {
    anim.frames.get(idx).or_else(|| anim.frames.first())
}

/// Desk cubicles — each carries its divider + cabinet + screen glow.
/// The desk sprite (14×7) sorts at `desk.y + visual.h` = `desk.y + 7`
/// (`DESK_H + 2`) — one row past its visual south
/// row, just past the seated worker's feet (`desk.y + 4`) so the sitter stays
/// visually behind the desk. Z is a VISUAL property: it tracks the sprite, not
/// the blocked ground — which is why the walk-behind footprint change (shrinking
/// the ground to a shallow south strip, #551) was z-neutral by construction.
/// `seated_agents` (built once before the ambient pass) gates the screen glow
/// so it only paints for a worker actually at the desk. The DeskCubicle
/// drawable is Copy, so this borrows nothing from the agent set.
fn enqueue_desk_cubicles<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    seated_agents: &HashMap<FloorLocalDeskIndex, bool>,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for (i, &desk) in ctx.layout.home_desks.iter().enumerate() {
        let local = FloorLocalDeskIndex(i);
        let desk_def = crate::layout::desk_furniture_def();
        let Some(Size { w: desk_fp_w, .. }) = desk_def.footprint else {
            continue;
        };
        let is_last_col = desk.x + desk_fp_w + DESK_W
            >= ctx.layout.cubicle_band.x + ctx.layout.cubicle_band.width;
        let occupant = agents
            .iter()
            .find(|a| a.desk_index.single_floor_local() == local && a.exiting_at.is_none());
        let screen_glow = occupant
            .filter(|_| seated_agents.get(&local).copied().unwrap_or(false))
            .and_then(|a| palette::tool_glow_tint(a, &ctx.theme.tool_glow));
        let has_coffee = occupant.is_some_and(|a| ctx.coffee.contains_key(&a.agent_id));
        let coffee_steam = has_coffee
            && occupant.is_some_and(|a| {
                ctx.coffee
                    .get(&a.agent_id)
                    .and_then(|t| ctx.now.duration_since(*t).ok())
                    .is_some_and(|d| d.as_secs() < COFFEE_STEAM_WINDOW_SECS)
            });
        drawables.push(Drawable {
            anchor_y: desk.y + desk_def.visual.h,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col,
                has_cabinet: i % 2 == 0,
                screen_glow,
                has_coffee,
                coffee_steam,
            },
        });
    }
}

/// The office pet (one per floor). An `active_pet` (mid heart-animation) is
/// pinned in place; otherwise `pet_position` roams it around the idle desks.
/// Returns the resolved `PetFrame` (for hit-testing) and enqueues the Pet
/// drawable, y-sorted at the chosen anim's south row (the h=4 sleep sprite
/// sorts one row shallower than the h=6 walk/sit sprites — a hardcoded +2 once
/// painted a sleeping pet over a character whose feet land at pos.y+1).
fn enqueue_pet<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    drawables: &mut Vec<Drawable<'a>>,
) -> Option<PetFrame> {
    let kind = ctx.floor_pet.map(|p| p.kind)?;
    let idle_desk_indices: Vec<FloorLocalDeskIndex> = agents
        .iter()
        .filter(|a| {
            matches!(a.state, ActivityState::Idle)
                && ctx
                    .layout
                    .home_desk(a.desk_index.single_floor_local())
                    .is_some()
                && a.exiting_at.is_none()
        })
        .map(|a| a.desk_index.single_floor_local())
        .collect();
    let all_idle = agents
        .iter()
        .all(|a| matches!(a.state, ActivityState::Idle));

    let active_pet = ctx
        .active_pet
        .filter(|p| p.is_active(ctx.now) && p.kind == kind && p.floor_idx == ctx.floor.floor_idx);
    let pet_data = if let Some(pet) = active_pet {
        Some((
            pet.pet_pos,
            false,
            kind.sit_anim(),
            0usize,
            Some(pet.elapsed_ms(ctx.now)),
        ))
    } else {
        pet_position(
            kind,
            ctx.layout,
            ctx.pack,
            ctx.now,
            &idle_desk_indices,
            all_idle,
            ctx.floor.floor_seed,
        )
        .map(|(pos, flip, anim, frame)| (pos, flip, anim, frame, None))
    };
    let (pos, flip, anim_name, frame_idx, pet_elapsed) = pet_data?;
    let pet_h = ctx
        .pack
        .animation(anim_name)
        .and_then(|a| a.frames.first())
        .map_or(6, |f| f.height());
    drawables.push(Drawable {
        anchor_y: z_sort_row(Anchor::Center, pos, pet_h),
        kind: DrawableKind::Pet {
            kind,
            pos,
            flip,
            anim_name,
            frame_idx,
            pet_elapsed_ms: pet_elapsed,
        },
    });
    Some(PetFrame {
        pos,
        anim: anim_name,
        kind,
    })
}

/// Enqueue any gateway mascots present in `daemons` (only the ground
/// floor carries the map, so a mascot shows once). Presence-gated: an absent
/// entry draws nothing, so the ~99% who don't run a gateway see a normal office.
/// The runtime is responsible for KEEPING the map honest — a never-connected or
/// panel-disconnected gateway has no live entry (the driver's presence
/// connection-gate drops its hooks and the sweep walks any lingering entry out),
/// so "entry present" tracks "connected + alive", not merely "a hook arrived".
/// y-sorted at the mascot's south row.
fn enqueue_gateway_mascot<'a>(
    ctx: &PaintCtx<'_>,
    drawables: &mut Vec<Drawable<'a>>,
) -> Option<MascotFrame> {
    let mut hover = None;
    for (source, presence) in ctx.scene.daemons() {
        let Some(def) = gateway_mascot_def(source) else {
            continue;
        };
        // Per-source deterministic seed so two gateways don't wander in lockstep.
        let seed = source
            .bytes()
            .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64));
        let Some((pos, anim_name, frame_idx)) =
            mascot_position(ctx.layout, presence, def.walk, def.rest, ctx.now, seed)
        else {
            continue;
        };
        let h = ctx
            .pack
            .animation(anim_name)
            .and_then(|a| a.frames.first())
            .map_or(12, |f| f.height());
        let run_count = presence.in_flight_run_keys.len() as u32;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, pos, h),
            kind: DrawableKind::GatewayMascot {
                pos,
                anim_name,
                frame_idx,
                run_count,
                degraded: presence.display_state() == pixtuoid_core::state::DaemonState::Degraded,
            },
        });
        // First present gateway wins the hover frame (single-gateway today).
        hover.get_or_insert(MascotFrame {
            pos,
            name: def.display_name,
            busy: presence.is_busy(),
            degraded: presence.display_state() == pixtuoid_core::state::DaemonState::Degraded,
            active_sessions: presence.active_sessions,
        });
    }
    hover
}

/// Meeting-room rugs + sofas + tables. For dual-meeting layouts sofas come in
/// pairs (2 per room), tables 1 per room. A south-of-table sofa faces away
/// (`Facing::North` → `back_couch`), so it y-sorts +3 to occlude its sitter
/// (whose key is `sofa.y + 2`); the north sofa stays +2 so insertion order
/// breaks the tie in its sitter's favor.
fn enqueue_meeting_furniture<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        let table = trio.table;
        let [ts, bs] = trio.sofas;
        let rug_w = 18u16;
        let rug_h =
            bs.y.saturating_sub(ts.y)
                .saturating_add(8)
                .min(layout.buf_h.saturating_sub(table.y).saturating_add(8));
        drawables.push(Drawable {
            anchor_y: table.y.saturating_sub(rug_h / 2),
            kind: DrawableKind::AreaRug {
                pos: table,
                width: rug_w,
                height: rug_h,
            },
        });
    }
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for (i, sofa) in trio.sofas.into_iter().enumerate() {
            // sofas[0] is the north sofa, sofas[1] the south — the south sofa
            // faces away (`mirrored`) and y-sorts +3 to occlude its sitter.
            let mirrored = i % 2 != 0;
            let faces_away = sofa.y >= trio.table.y;
            drawables.push(Drawable {
                anchor_y: sofa.y + if faces_away { 3 } else { 2 },
                kind: DrawableKind::MeetingSofa {
                    pos: sofa,
                    mirrored,
                },
            });
        }
    }
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        drawables.push(Drawable {
            // z-key = sprite south row, derived from the table (== +2 for the
            // 11×5 meeting-table sprite) so it can't drift from a visual edit.
            anchor_y: z_sort_row(
                Anchor::Center,
                trio.table,
                crate::layout::furniture_def(crate::layout::Furniture::MeetingTable)
                    .visual
                    .h,
            ),
            kind: DrawableKind::MeetingTable { pos: trio.table },
        });
    }
}

/// The kitchen island, the lounge couch (emitted ONCE via
/// `couch_sprite_center` — 3 seat waypoints share one sprite), and the
/// center-pinned waypoint appliances (pantry counter, vending, printer).
/// PhoneBooth/StandingDesk render via pod-decor; meeting slots ride the
/// sofa/table — so those waypoint kinds emit nothing here.
fn enqueue_lounge_pantry_appliances<'a>(
    layout: &'a Layout,
    occupied_waypoints: &std::collections::HashSet<usize>,
    drawables: &mut Vec<Drawable<'a>>,
) {
    if let Some(island) = layout.pantry.and_then(|p| p.kitchen_island) {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                island,
                crate::layout::furniture_def(crate::layout::Furniture::KitchenIsland)
                    .visual
                    .h,
            ),
            kind: DrawableKind::KitchenIsland { pos: island },
        });
    }

    // Lounge couch — pushed before the character loop so the y-sort tie-break
    // keeps the couch behind its sitters. The rug anchors north of the couch
    // (y-sort at its top) so the couch sits on it.
    if let Some(center) = layout.couch_sprite_center {
        drawables.push(Drawable {
            anchor_y: center.y.saturating_sub(2),
            kind: DrawableKind::AreaRug {
                pos: Point {
                    x: center.x,
                    y: center.y + 3,
                },
                width: 22,
                height: 7,
            },
        });
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                center,
                crate::layout::furniture_def(crate::layout::Furniture::Couch)
                    .visual
                    .h,
            ),
            kind: DrawableKind::WaypointCouch { pos: center },
        });
        if let Some(table) = layout.lounge_side_table {
            drawables.push(Drawable {
                anchor_y: z_sort_row(
                    Anchor::Center,
                    table,
                    crate::layout::furniture_def(crate::layout::Furniture::LoungeSideTable)
                        .visual
                        .h,
                ),
                kind: DrawableKind::LoungeSideTable { pos: table },
            });
        }
    }

    for (wp_idx, wp) in layout.waypoints.iter().enumerate() {
        use crate::layout::{furniture_def, WaypointKind};
        let busy = occupied_waypoints.contains(&wp_idx);
        // y-sort baseline = the sprite's south row (these appliances are
        // center-pinned at `pos`). Read the VISUAL height, not the (shallow)
        // footprint, so an overhang would still sort by what's painted.
        let visual_h = furniture_def(wp.kind.furniture()).visual.h;
        match wp.kind {
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let Size { w: cw, h: ch } = layout.pantry_counter_size(); // runtime-sized
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, ch),
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        use_large: cw >= crate::layout::PANTRY_COUNTER_LARGE_W,
                    },
                });
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            WaypointKind::VendingMachine => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                    kind: DrawableKind::VendingMachine { pos: wp.pos, busy },
                });
            }
            WaypointKind::Printer => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                    kind: DrawableKind::Printer { pos: wp.pos, busy },
                });
            }
            WaypointKind::SnackShelf => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                    kind: DrawableKind::SnackShelf { pos: wp.pos },
                });
            }
            // Island stands carry no art of their own: the island BODY draws
            // via `layout.kitchen_island` (like the meeting furniture).
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair | WaypointKind::Island => {}
        }
    }
}

/// Pod-aisle decor (plant / whiteboard / TV / phone booth / standing desk)
/// and free-standing plants — all center-pinned, y-sorted at the sprite's
/// south row from the one furniture table (the mask reads the separate,
/// shallower `footprint` off the same row, so a tall canopy sorts without
/// blocking the aisle).
fn enqueue_pod_decor_and_plants<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &PodDecorItem { kind, pos } in &layout.pod_decor {
        let Size { h, .. } = crate::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, pos, h),
            kind: DrawableKind::PodDecorItem { kind, pos },
        });
    }
    for &PlantItem { kind, pos } in &layout.plants {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                pos,
                crate::layout::furniture_def(kind.furniture()).visual.h,
            ),
            kind: DrawableKind::Plant { kind, pos },
        });
    }
}

/// Free-standing fixtures: the floor lamp, the meeting-room coat rack, and the
/// elevator door (whose open/close frame is computed stateless from the agents
/// currently in their entry/exit window — the MAX frame so the door is at least
/// as open as the most-in-progress agent needs).
fn enqueue_floor_fixtures<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    drawables: &mut Vec<Drawable<'a>>,
) {
    if let Some(lamp) = ctx.layout.floor_lamp {
        drawables.push(Drawable {
            anchor_y: lamp.y + floor_lamp_south_offset(),
            kind: DrawableKind::FloorLamp { pos: lamp },
        });
    }
    for wp in ctx
        .layout
        .waypoints
        .iter()
        .filter(|w| w.kind == crate::layout::WaypointKind::MeetingChair)
    {
        drawables.push(Drawable {
            // One row UNDER the sitter's z — derived from the occupant's
            // OWN view's seat key, so the pair can't drift apart.
            anchor_y: seat::SeatView::of(wp.kind, wp.facing).z_key_for_seat(wp.pos) - 1,
            kind: DrawableKind::MeetingChair {
                pos: wp.pos,
                // The backrest rides the side AWAY from the table: a chair
                // FACING East sits west of the table, bar on its west.
                back_west: wp.facing == crate::layout::Facing::East,
            },
        });
    }
    if let Some(tank) = ctx.layout.fish_tank {
        // Center anchor: z at the sprite's south (cabinet base) row, via the
        // SAME center-pin helper the lamp derives its base from.
        let h = crate::layout::furniture_def(crate::layout::Furniture::FishTank)
            .visual
            .h;
        drawables.push(Drawable {
            anchor_y: tank.y + center_pin_south_offset(h),
            kind: DrawableKind::FishTank { pos: tank },
        });
    }
    // One coat rack per meeting room (#555: room 1 used to go without);
    // placement + the narrow-fitted-room yield live in coat_rack_pos — THE
    // one authority the hover hit-test shares.
    for rack in ctx
        .layout
        .meeting_rooms
        .iter()
        .filter_map(|r| r.coat_rack_pos())
    {
        drawables.push(Drawable {
            anchor_y: rack.y + 7,
            kind: DrawableKind::CoatRack { pos: rack },
        });
    }
    if let Some(door_pos) = ctx.layout.door {
        let frame_idx = compute_door_frame_idx(agents, ctx.now, ctx.door_anim_max_ms);
        drawables.push(Drawable {
            anchor_y: door_pos.y + ELEVATOR_H,
            kind: DrawableKind::Door {
                pos: door_pos,
                frame_idx,
            },
        });
    }
}

/// Enqueue wall decor (clocks/whiteboards hung on walls). TOP-LEFT anchored
/// at `pos`, so the y-sort row is the sprite's south base (`pos.y + h - 1`),
/// the same `z_sort_row` helper the mask and every other drawable use. A
/// pure furniture phase of `render_to_rgb_buffer` — borrows nothing from the
/// agent set, so it carries no character lifetime.
fn enqueue_wall_decor<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &WallDecorItem { kind, pos } in &layout.wall_decor {
        let Size { h, .. } = crate::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::TopLeft, pos, h),
            kind: DrawableKind::WallDecor { kind, pos },
        });
    }
}

#[cfg(test)]
mod tests;
