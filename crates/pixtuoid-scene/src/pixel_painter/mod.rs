//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! [`render_to_rgb_buffer`] is the world-render seam every painter rides, and
//! is itself TWO phases: `sim_step` advances the world with no pixel access
//! into an immutable [`SimFrame`], then `paint_frame` consumes it. The whole
//! public surface is on the published crate's api golden, so widen it
//! deliberately.

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
    z_sort_row, Anchor, Layout, PlantItem, PodDecorItem, Point, Size, WallDecorItem, DESK_W,
    ELEVATOR_H, ELEVATOR_W,
};
use crate::motion::MotionState;
use crate::pet::PetFrame;

/// Milliseconds since the Unix epoch for `now` (0 if the clock is before it).
pub(super) fn epoch_ms(now: SystemTime) -> u64 {
    crate::anim::elapsed_ms(now, SystemTime::UNIX_EPOCH)
}

/// Everything the pure-pixel pass observed that the caller still needs.
pub struct PixelPassResult {
    /// The office pet's resolved frame this tick (for hit-testing), if present.
    pub pet_pos: Option<PetFrame>,
    /// One resolved frame per gateway mascot drawn this tick — a source can
    /// run ANY number of concurrent instances, each independently hoverable.
    pub mascots: Vec<MascotFrame>,
    /// Active speech bubbles this frame, for the caller's widget pass.
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agent ids observed in `Walking { carrying_coffee: true }` this frame.
    /// The caller inserts them into the persistent `CoffeeState`.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
    /// Waypoint indices with an occupant this tick — the audio cue tracker's
    /// appliance feed.
    pub occupied_waypoints: std::collections::HashSet<usize>,
}

/// The gateway mascot's screen frame — enough to hover-identify it. Recaptured
/// each render, since the wandering position is recomputed every frame.
#[derive(Clone)]
pub struct MascotFrame {
    /// The mascot's top-left screen position this tick.
    pub pos: Point,
    /// The painted sprite's pixel width, read from the pack's real frame so
    /// the binary's `hit_test_mascot` click box derives from what's drawn.
    pub w: u16,
    /// The painted sprite's pixel height (paired with `w`).
    pub h: u16,
    /// Human-readable gateway name (e.g. "OpenClaw").
    pub name: &'static str,
    /// WHICH instance of that gateway this mascot is, so a hover over one of
    /// two concurrent lobsters names the one under the cursor. `None` when the
    /// source runs a single instance whose id means nothing to the user.
    pub instance: Option<String>,
    /// An agent run is in flight. Keyed on the run state, NOT the session count
    /// — a single-user gateway holds one persistent session even at rest.
    pub busy: bool,
    /// Gateway up but its model backend is failing every run.
    pub degraded: bool,
    /// Number of sessions the gateway currently holds (tooltip detail).
    pub active_sessions: u32,
}

mod ambient;
mod anchors;
mod background;
mod debug_overlay;
mod drawable;
mod effects;
mod furniture;
mod palette;
pub(crate) mod seat;
mod sim;
mod wall;

pub use anchors::character_anchor;

/// The painter's own seat anchor, for the binary's hit-test.
///
/// `#[doc(hidden)]` — public for MECHANISM, not contract (the overlay/board
/// pattern). It exists so the click target is computed by the same fn that
/// places the sprite, rather than by a second copy of its arithmetic that
/// drifts the first time a desk seats its occupant on the other side.
#[doc(hidden)]
pub fn seated_anchor_for(
    desk: crate::layout::Point,
    sprite_w: u16,
    facing: crate::layout::Facing,
) -> crate::layout::Point {
    anchors::seated_anchor_facing(desk, sprite_w, facing)
}

// The ToolKind→glow-hue seam the binary's footer tints tool segments with, so a
// footer tool colour matches the sprite's monitor glow exactly.
pub use palette::tool_glow_for_kind;
// `floor::FloorSession::observe` is the public entry to the sim tick; the step
// itself and its per-call borrow-set stay crate-internal.
pub(crate) use sim::{sim_step, SimStores};
// The flame-crown/ember colors, for render tests to assert the REAL painted
// values (`effects` itself stays private).
#[cfg(test)]
pub(crate) use effects::{FLAME_DEEP, FLAME_TIP};
#[cfg(test)]
pub(crate) use furniture::COOLER_WATER;
pub use sim::{CharacterGlow, CharacterPlacement, SimFrame};

/// The coffee-machine sub-region within the large pantry counter sprite, as a
/// sprite-local column range `[start, end)`. THE single source of truth shared
/// by the steam-anchor painter and the binary's `hit_test_coffee_machine`, so
/// the clickable box can't drift from the painted art when the sprite is
/// re-tuned.
pub const PANTRY_COFFEE_COLS_LARGE: (u16, u16) = (11, 18);
/// Coffee-machine column range for the 20-wide `pantry_small` sprite (see
/// [`PANTRY_COFFEE_COLS_LARGE`]).
pub const PANTRY_COFFEE_COLS_SMALL: (u16, u16) = (9, 12);

/// The neon wall-sign panel geometry, in PIXELS: origin `(X, Y)` and OUTER size
/// `W×H`, drawn with a `NEON_PANEL_BORDER`-px frame on every side. A pixel
/// column maps 1:1 to a terminal cell column in the half-block flush, so these
/// px widths ARE cell widths on the horizontal. The board's TEXT overlay must
/// pin to the dark INTERIOR (`NEON_PANEL_INNER_*`), not the OUTER `W`, or the
/// lit text overruns the glowing frame by the border on each side.
pub(crate) const NEON_PANEL_X: u16 = 1;
pub(crate) const NEON_PANEL_Y: u16 = 1;
/// The neon panel's OUTER width in pixels (frame included).
pub const NEON_PANEL_W: u16 = 30;
pub(crate) const NEON_PANEL_H: u16 = 8;
/// The frame thickness `paint_neon_panel` lights on every side — it reads THIS,
/// so the interior derivations below match the pixels it leaves dark.
pub(crate) const NEON_PANEL_BORDER: u16 = 1;
/// The dark interior's left cell-origin — where board text starts.
pub const NEON_PANEL_INNER_X: u16 = NEON_PANEL_X + NEON_PANEL_BORDER;
/// The dark interior's cell WIDTH — the board's usable text width.
pub const NEON_PANEL_INNER_W: u16 = NEON_PANEL_W - 2 * NEON_PANEL_BORDER;
/// The dark interior's top pixel-origin — where the floating / wasm painters
/// anchor the board's first text row.
pub const NEON_PANEL_INNER_Y: u16 = NEON_PANEL_Y + NEON_PANEL_BORDER;
/// The dark interior's pixel HEIGHT.
pub const NEON_PANEL_INNER_H: u16 = NEON_PANEL_H - 2 * NEON_PANEL_BORDER;
// The interior must be a non-empty strict subset of the outer frame (catches a
// degenerate BORDER=0 / oversized-border config at compile time).
const _: () = assert!(NEON_PANEL_INNER_W > 0 && NEON_PANEL_INNER_W < NEON_PANEL_W);
const _: () = assert!(NEON_PANEL_INNER_H > 0 && NEON_PANEL_INNER_H < NEON_PANEL_H);

use crate::creatures::{gateway_mascot_def, mascot_position, pet_position};
use anchors::compute_door_frame_idx;
use background::{
    daylight_floor_overlay, dim_floor_overlay, paint_ceiling_pool, paint_clock,
    paint_corridor_runner, paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel,
    paint_shadow, time_of_day_look, Ellipse,
};
use drawable::{paint_drawable, Drawable, DrawableKind};
use palette::{agent_palette, outfit_seed_for, recolor_frame};
use seat::paint_character_at;
use wall::{
    enqueue_room_walls_h, enqueue_room_walls_v, paint_door_jamb_h, paint_door_jamb_v,
    paint_glass_wall_h, paint_glass_wall_v, DOOR_JAMB_PX,
};

/// The weather names accepted by [`force_weather`], canonical order.
pub fn weather_names() -> Vec<&'static str> {
    background::Weather::ALL.iter().map(|w| w.name()).collect()
}

/// Force every subsequent render **on this thread** to a specific weather (by
/// name, case-insensitive), or `None` to restore the clock-based selection.
/// It's a thread-local shared by every `Office` in the one wasm module, so the
/// last writer before a render wins and each surface must set its own value
/// every frame. `Err` carries the valid names when `name` is unknown.
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

/// How hard it is raining, as a scalar (0.0 clear … 1.0 storm) — the audio
/// model's weather feed. A deliberate SCALAR query so the module-private
/// `background::Weather` enum never widens. Snow/fog/etc. are 0.0 —
/// precipitation you can HEAR, not precipitation per se.
pub fn precipitation_level(now: std::time::SystemTime) -> f32 {
    // The gap to Storm is an audible "getting heavier", not a new mix profile.
    const RAIN_LEVEL: f32 = 0.6;
    match background::weather_state(now) {
        background::Weather::Storm => 1.0,
        background::Weather::Rain => RAIN_LEVEL,
        _ => 0.0,
    }
}

/// Whether the office's sky shows the SUN at hour-of-day `hour` (0..24).
/// Exposed so the wasm painter's `Office::is_day` can hand the site's
/// sky-slider the SAME day/night boundary the office renders.
pub fn hour_is_day(hour: f32) -> bool {
    background::hour_is_day(hour)
}

/// Day/night at `now` on the LOCAL clock — the native painters' feed for the
/// audio track selector (wasm passes its own hour). Same sun window the
/// lighting renders: the music follows what the office SHOWS.
pub fn is_day_at(now: std::time::SystemTime) -> bool {
    background::hour_is_day(background::local_hour_frac(now))
}

// A reference to the window `CoffeeState::record` refreshes on, not a copy.
const COFFEE_STEAM_WINDOW_SECS: u64 = crate::floor::CoffeeState::STEAM_WINDOW_SECS;

/// Z-sort offset from a center-pinned sprite's center to its SOUTH (front) row.
/// A sprite blitted at `py = center - h/2` souths at `center + (h - 1) / 2` —
/// correct for BOTH parities, where the naive `h/2 - 1` is one row short for
/// ODD `h`. The z-key must land ON the south row; one row past it lets the
/// sprite paint over a character standing immediately in front.
fn center_pin_south_offset(h: u16) -> u16 {
    h.saturating_sub(1) / 2
}

/// South-row (base) offset of the floor-lamp sprite, derived so the halo /
/// shadow / z-anchor all move together if the lamp's visual height changes.
fn floor_lamp_south_offset() -> u16 {
    center_pin_south_offset(
        crate::layout::furniture_def(crate::layout::Furniture::FloorLamp)
            .visual
            .h,
    )
}

/// Bundled input for the pixel-painting pass.
pub struct PixelCtx<'a> {
    /// The per-floor sim/paint STORES borrowed as ONE group. `buf` stays a
    /// SEPARATE field: it is a sibling of the `FloorCtx` on a `PerFloor`,
    /// borrowed disjointly by a multi-floor painter's `split_at_mut`.
    pub store: &'a mut crate::floor::FloorCtx,
    /// The RGB pixel buffer this pass paints into. Its pixels ARE `layout`'s
    /// logical units — this pass has no scale of its own.
    pub buf: &'a mut RgbBuffer,
    /// The live scene state to render.
    pub scene: &'a SceneState,
    /// The computed office geometry for this frame.
    pub layout: &'a Layout,
    /// The character/furniture sprite pack.
    pub pack: &'a Pack,
    /// The current time (the engine never reads the clock itself — it's a parameter).
    pub now: SystemTime,
    /// The active color theme.
    pub theme: &'a crate::theme::Theme,
    /// Which floor of the office this pass renders.
    pub floor: crate::floor::FloorMeta,
    /// The pet-interaction (heart-anim) state, if a pet is being petted.
    pub active_pet: Option<&'a crate::pet::PetState>,
    /// The pet on this floor (kind drives the sprite).
    pub floor_pet: Option<&'a crate::pet::Pet>,
    /// Carrier → fetch-time view of [`crate::floor::CoffeeState`]: key present
    /// = has a desk cup, value = steam-window anchor.
    pub coffee: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    /// Per-venue active speech-bubble state, advanced across frames.
    pub chitchat_state: &'a mut HashMap<crate::chitchat::VenueKey, ActiveChitchat>,
    /// When set, composite the walkable / approach / route debug layer over the
    /// finished scene (the live `w` toggle).
    pub debug_walkable: bool,
}

/// The paint pass's borrow set — everything `paint_frame` may touch. The only
/// `&mut`s are the pixel buffer and the paint-local `FrameCache`; the sim
/// stores are absent BY TYPE (`motion` is an immutable view, read by the debug
/// route overlay), so painting cannot move the world.
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

/// Render `ctx`'s scene into its buffer — the SHARED world render, TWO phases:
/// `sim_step` advances the world (no pixels) into a [`SimFrame`], then the paint
/// pass consumes it, mutating only the buffer + recolor cache.
pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
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
    let (pet_pos, mascots) = paint_frame(
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
        mascots,
        chitchat_bubbles: frame.chitchat_bubbles,
        new_coffee_carriers: frame.new_coffee_carriers,
        occupied_waypoints: frame.occupied_waypoints,
    }
}

/// The soft floor shadow under one home desk. `cy` reads the same authority
/// `enqueue_desk_cubicles` keys the sprite on, so a `DESK_H` retune moves the
/// shadow WITH the sprite's south base. `half_h` is a taste literal.
fn desk_shadow_ellipse(desk: Point) -> Ellipse {
    Ellipse {
        cx: desk.x + DESK_W / 2,
        cy: desk.y + crate::layout::desk_furniture_def().visual.h,
        half_w: DESK_W / 2 + 1,
        half_h: 3,
    }
}

/// The ceiling-fluorescent light pools, in paint order. The floor-lamp halo is
/// deliberately NOT here — it is a different painter with its own strength +
/// anchor, and the order (pools THEN halo) is load-bearing for byte-identity.
/// How much of the ceiling pools' NIGHT gain each kind keeps.
///
/// The desk tubes go nearly out after dark. An office working late kills the
/// overhead light over the workstations and the room reads by screen glow —
/// that is the look, and it is also the only way twelve 20x10 pools stop
/// overlapping into one pale sheet across the whole cubicle band, which is
/// what they did at full gain (invisible by day at the 0.15 base, so it only
/// ever showed on a dark theme). The shared spaces keep theirs: someone still
/// has to find the coffee machine.
const DESK_POOL_NIGHT_KEEP: f32 = 0.18;
/// Night-gain retention for the pantry and corridor pools — unchanged.
const SHARED_POOL_NIGHT_KEEP: f32 = 1.0;

fn ceiling_pool_regions(layout: &Layout) -> impl Iterator<Item = (Ellipse, f32)> + '_ {
    // Half-extents (a fluorescent tube's lit footprint) per pool kind.
    const DESK_POOL_HALF: (u16, u16) = (10, 5);
    const PANTRY_POOL_HALF: (u16, u16) = (12, 6);
    const CORRIDOR_POOL_HALF: (u16, u16) = (14, 5);
    // Centre comes from the SEAT, so the light tracks the occupant a facing put
    // there — see `layout::desk_ceiling_pool_center`. A hardcoded lift left the
    // pool over empty floor behind a back-turned worker.
    let desks = layout.home_desks.iter().enumerate().map(|(i, desk)| {
        let c = crate::layout::desk_ceiling_pool_center(
            *desk,
            layout.desk_facing(FloorLocalDeskIndex(i)),
        );
        (
            Ellipse {
                cx: c.x,
                cy: c.y,
                half_w: DESK_POOL_HALF.0,
                half_h: DESK_POOL_HALF.1,
            },
            DESK_POOL_NIGHT_KEEP,
        )
    });
    let pantry = layout.pantry.map(|p| p.bounds).map(|pr| {
        (
            Ellipse {
                cx: pr.x + pr.width / 2,
                cy: pr.y + pr.height / 2,
                half_w: PANTRY_POOL_HALF.0,
                half_h: PANTRY_POOL_HALF.1,
            },
            SHARED_POOL_NIGHT_KEEP,
        )
    });
    let corridor = layout.corridor.map(|c| {
        (
            Ellipse {
                cx: c.x + c.width / 2,
                cy: c.y + c.height / 2,
                half_w: CORRIDOR_POOL_HALF.0,
                half_h: CORRIDOR_POOL_HALF.1,
            },
            SHARED_POOL_NIGHT_KEEP,
        )
    });
    desks.chain(pantry).chain(corridor)
}

/// THE authority for the soft floor shadows, in PAINT ORDER — the overlaps
/// blend, so the order is load-bearing. The per-piece `half_w`/`half_h` are
/// owner-tuned taste literals.
fn floor_shadow_ellipses(layout: &Layout) -> impl Iterator<Item = Ellipse> + '_ {
    use crate::layout::{furniture_def, Furniture, WaypointKind};

    let desks = layout
        .home_desks
        .iter()
        .map(|desk| desk_shadow_ellipse(*desk));
    // Couch/Printer/Island get fitted shadows below, so skip them here: a
    // per-seat couch shadow would overlap-darken, the printer souths at +1 not
    // the generic +2, and the island STANDS are empty floor beside the body.
    let generic = layout
        .waypoints
        .iter()
        .filter(|w| {
            !matches!(
                w.kind,
                WaypointKind::Couch | WaypointKind::Printer | WaypointKind::Island
            )
        })
        .map(|wp| {
            // Fit the ellipse to the sprite width — a flat 7 half-width doubles
            // a narrow shelf's shadow; `.min(7)` caps a future wide piece.
            let vis_w = furniture_def(wp.kind.furniture()).visual.w;
            let half_w = if vis_w > 0 { (vis_w / 2 + 1).min(7) } else { 7 };
            Ellipse {
                cx: wp.pos.x,
                cy: wp.pos.y + 2,
                half_w,
                half_h: 2,
            }
        });
    let island = layout.pantry.and_then(|p| p.kitchen_island).map(|island| {
        let vis = furniture_def(Furniture::KitchenIsland).visual;
        Ellipse {
            cx: island.x,
            cy: island.y + center_pin_south_offset(vis.h),
            half_w: vis.w / 2 + 1,
            half_h: 2,
        }
    });
    let printers = layout
        .waypoints
        .iter()
        .filter(|w| w.kind == WaypointKind::Printer)
        .map(|wp| Ellipse {
            cx: wp.pos.x,
            cy: wp.pos.y + 1,
            half_w: 5,
            half_h: 1,
        });
    let couch = layout.couch_sprite_center().map(|center| Ellipse {
        cx: center.x,
        cy: center.y + 2,
        half_w: 7,
        half_h: 2,
    });
    // Off the same height the z-anchor uses: a fixed +3 only suited the taller
    // plants and floated the rest.
    let plants = layout
        .plants
        .iter()
        .map(|&PlantItem { kind, pos }| Ellipse {
            cx: pos.x,
            cy: pos.y + center_pin_south_offset(furniture_def(kind.furniture()).visual.h),
            half_w: 3,
            half_h: 1,
        });
    let lamp = layout.floor_lamp().map(|lamp| Ellipse {
        cx: lamp.x,
        cy: lamp.y + floor_lamp_south_offset(),
        half_w: 2,
        half_h: 1,
    });

    desks
        .chain(generic)
        .chain(island)
        .chain(printers)
        .chain(couch)
        .chain(plants)
        .chain(lamp)
}

/// The PAINT half of the frame: blit the world the sim already advanced. Every
/// positional/lifecycle decision was made in `sim_step` — this pass only
/// resolves presentation (theme colors, sprite pixels) and composites.
fn paint_frame(ctx: &mut PaintCtx<'_>, frame: &SimFrame) -> (Option<PetFrame>, Vec<MascotFrame>) {
    let agents: &[AgentSlot] = &frame.agents;
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;

    // Threaded through every dependent helper, so the chrono local hour isn't
    // recomputed per window + ceiling pool + lamp halo.
    let look = time_of_day_look(ctx.now, ctx.theme);
    let top_wall_h = ctx.layout.wall_band_h();
    // The elevator door replaces the rightmost window, so `paint_floor_and_walls`
    // must skip a window that would otherwise bleed through the elevator frame.
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

    let indoor_scale = frame.indoor_scale;
    // Empty floors get an extra floor-darken boost on top of the time-of-day
    // dim: with no monitor/lamp sources to balance the overhead darkness, they
    // otherwise read as "lights off but room weirdly bright."
    let min_level = LightingState::MIN_LEVEL;
    let boost_ceiling = LightingState::EMPTY_FLOOR_DIM_BOOST;
    let empty_floor_boost = 1.0 + (1.0 - indoor_scale) * (boost_ceiling - 1.0) / (1.0 - min_level);

    // The night floor-dim dial, symmetric with `DAYLIGHT_FLOOR_LIFT` below.
    const NIGHT_FLOOR_DIM_STRENGTH: f32 = 0.45;
    let dim_strength = NIGHT_FLOOR_DIM_STRENGTH;
    dim_floor_overlay(
        ctx.buf,
        top_wall_h,
        buf_h,
        look.darkness * dim_strength * empty_floor_boost,
        ctx.theme,
    );
    // The positive mirror of the night dim. Independent of occupancy — sun
    // enters an empty office too.
    const DAYLIGHT_FLOOR_LIFT: f32 = 0.22;
    daylight_floor_overlay(
        ctx.buf,
        top_wall_h,
        buf_h,
        look.spill_strength * DAYLIGHT_FLOOR_LIFT,
    );
    // Split base from night gain so a pool kind can opt out of the DARK half
    // without losing its daylight presence — see `DESK_POOL_NIGHT_KEEP`.
    const POOL_BASE: f32 = 0.15;
    const POOL_NIGHT_GAIN: f32 = 0.30;
    for (pool, night_keep) in ceiling_pool_regions(ctx.layout) {
        let strength = (POOL_BASE + POOL_NIGHT_GAIN * look.darkness * night_keep) * indoor_scale;
        paint_ceiling_pool(ctx.buf, pool, strength, ctx.theme);
    }
    if let Some(lamp) = ctx.layout.floor_lamp() {
        paint_floor_lamp_halo(
            ctx.buf,
            lamp.x,
            lamp.y + floor_lamp_south_offset(),
            look.darkness * 0.55 * indoor_scale,
            ctx.theme,
        );
    }

    // The panel's text overlay is a separate ratatui widget pass, not painted
    // here.
    paint_neon_panel(
        ctx.buf,
        NEON_PANEL_X,
        NEON_PANEL_Y,
        NEON_PANEL_W,
        NEON_PANEL_H,
        ctx.now,
        ctx.theme,
    );

    // After the wall (so its hands sit on top) but before wall decor (the
    // bookshelf shouldn't cover it). The clamp keeps it clear of the neon
    // panel's right edge plus a 1px gap.
    let clock_x = (buf_w / 2)
        .saturating_sub(3)
        .max(NEON_PANEL_X + NEON_PANEL_W + 1);
    paint_clock(ctx.buf, clock_x, 1, ctx.now, ctx.theme);
    // The runner paints over the floor but BEFORE walls/decor so walls cleanly
    // overlap it where they cross.
    if let Some(corridor) = ctx.layout.corridor {
        paint_corridor_runner(ctx.buf, corridor, ctx.theme);
    }
    // Nothing room-scale (walls, sofas, the kitchen island) belongs in this
    // background pass — it would double-paint under its y-sorted copy below.
    // Only these small mask-free items do.
    for room in &ctx.layout.meeting_rooms {
        furniture::paint_notice_board(ctx.buf, room.bounds, ctx.theme);
        furniture::paint_doormat(ctx.buf, room, ctx.theme);
    }
    // Floor-level mats paint FIRST so they sit under every upright pantry
    // fixture: on a narrow pantry the entry mat's box reaches the water-cooler
    // column, and mats-after-cooler would clip the cooler's west edge.
    furniture::paint_pantry_entry_mat(ctx.buf, ctx.layout, ctx.theme);
    furniture::paint_island_bar_mat(ctx.buf, ctx.layout, ctx.theme);
    if let Some(pantry) = &ctx.layout.pantry {
        furniture::paint_water_cooler(ctx.buf, pantry, ctx.now, ctx.theme);
        furniture::paint_trash_bin(ctx.buf, pantry);
    }

    // Strength is a function of daylight so noon shadows are crisp and night
    // shadows subtle.
    let shadow_strength = 0.5 - 0.3 * look.darkness;
    for ell in floor_shadow_ellipses(ctx.layout) {
        paint_shadow(ctx.buf, ell, shadow_strength, ctx.theme);
    }

    // Ceiling halos gate on the sim's `seated_agents` so a tool-glow halo never
    // floats above an empty desk while its Active occupant is mid-walk.
    ambient::paint_ambient(ctx, &look, &frame.seated_agents);

    // Every entity gets an `anchor_y` — its floor-touching row — so sorting
    // ascending and painting in order puts things closer to the camera in
    // front: the painter's algorithm on a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::with_capacity(
        ctx.layout.home_desks.len()
            + ctx.layout.waypoints.len()
            + ctx.layout.plants.len()
            + ctx.layout.pod_decor.len()
            + ctx.layout.wall_decor.len()
            + agents.len(),
    );

    enqueue_desk_cubicles(
        ctx,
        agents,
        &frame.seated_agents,
        look.darkness,
        &mut drawables,
    );

    enqueue_meeting_furniture(ctx.layout, &mut drawables);

    enqueue_lounge_pantry_appliances(ctx.layout, &frame.occupied_waypoints, &mut drawables);

    enqueue_pod_decor_and_plants(ctx.layout, &mut drawables);
    enqueue_floor_fixtures(ctx, agents, &mut drawables);
    enqueue_wall_decor(ctx.layout, &mut drawables);

    let resolved_pet_pos = enqueue_pet(ctx, agents, &mut drawables);
    let resolved_mascots = enqueue_gateway_mascots(ctx, &mut drawables);

    enqueue_characters(ctx, frame, &mut drawables);

    // V before H: at an inside corner the vertical's stitched `y_bot` ties the
    // horizontal's south-base anchor, and inserting V first keeps H winning
    // that tie under the stable sort.
    enqueue_room_walls_v(ctx.layout, top_wall_h, &mut drawables);
    enqueue_room_walls_h(ctx.layout, &mut drawables);

    // `sort_by_key` is stable, so ties preserve the insertion order above —
    // decor first, characters last — and a character tied with a piece of
    // furniture paints BEFORE it.
    drawables.sort_by_key(|d| d.anchor_y);
    for d in &drawables {
        paint_drawable(
            d,
            &mut drawable::DrawableCtx {
                buf: &mut *ctx.buf,
                pack: ctx.pack,
                cache: &mut *ctx.cache,
                now: ctx.now,
                theme: ctx.theme,
            },
        );
    }

    // LAST, so a Storm strike briefly flares the whole interior (floor, walls,
    // furniture, characters), not just the window strip.
    background::paint_lightning_flash(ctx.buf, ctx.now, background::weather_state(ctx.now));

    if ctx.debug_walkable {
        debug_overlay::paint(ctx.buf, ctx.layout, ctx.scene, ctx.motion);
    }

    (resolved_pet_pos, resolved_mascots)
}

/// Resolve the sim's theme-free [`CharacterGlow`] to a `Theme` colour.
///
/// The ONE mapping, shared with the cutaway profile: the sim deliberately emits
/// a theme-free decision, so if each painter resolved it itself the same agent
/// would glow differently in the two profiles for no reason anyone chose.
pub(crate) fn character_glow_tint(
    glow: CharacterGlow,
    agent: &pixtuoid_core::state::AgentSlot,
    theme: &crate::theme::Theme,
) -> Option<pixtuoid_core::sprite::Rgb> {
    match glow {
        CharacterGlow::None => None,
        CharacterGlow::Thinking => Some(theme.tool_glow.default),
        CharacterGlow::Tool => palette::tool_glow_tint(agent, &theme.tool_glow),
    }
}

/// Map the sim's resolved [`sim::CharacterPlacement`]s 1:1 onto y-sorted
/// drawables. The ONLY paint-side work is presentation — resolving the
/// theme-free [`CharacterGlow`] to a `Theme` color.
fn enqueue_characters<'a>(
    ctx: &PaintCtx<'_>,
    frame: &'a SimFrame,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for p in &frame.characters {
        let agent = &frame.agents[p.agent_idx];
        let glow_tint = character_glow_tint(p.glow, agent, ctx.theme);
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

/// The frame to paint for `idx`, clamped into range: a custom `--pack-dir`
/// animation with fewer frames than the shared cycle's `frame_idx` would
/// otherwise vanish the sprite. `None` only for a genuinely empty animation.
pub(super) fn frame_at(anim: &Sprite, idx: usize) -> Option<&Frame> {
    anim.frames.get(idx).or_else(|| anim.frames.first())
}

/// Desk cubicles — each carries its divider + cabinet + screen glow. The desk
/// sorts one row past its visual south row, just past the seated worker's feet,
/// so the sitter stays visually behind it. Z is a VISUAL property: it tracks
/// the sprite, not the blocked ground.
///
/// `home_desks` is the authority for whether a pod divider exists: the band
/// clamp in `compute_pod_desks` drops a pod's second column when it wouldn't
/// fit, so pitch arithmetic here would be a second, drifting copy of that rule.
fn enqueue_desk_cubicles<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    seated_agents: &HashMap<FloorLocalDeskIndex, bool>,
    darkness: f32,
    drawables: &mut Vec<Drawable<'a>>,
) {
    // Peak standby tint, scaled by `darkness` — the INTERIOR illuminance, NOT a
    // clock. The screens therefore read strongest exactly when the room needs
    // them (the desk ceiling pools go out after dark) and fade as daylight fills
    // it. They do NOT vanish by day: an office lit through one window band stays
    // dim enough at noon that a powered screen still shows, which is true of the
    // real thing too. Checked against a pre-change noon render rather than
    // assumed — an earlier revision of this comment claimed daylight was
    // untouched, and the pixel diff said otherwise.
    const SCREEN_IDLE_MAX: f32 = 0.55;
    for (i, &desk) in ctx.layout.home_desks.iter().enumerate() {
        let local = FloorLocalDeskIndex(i);
        let desk_def = crate::layout::desk_furniture_def();
        let occupant = agents
            .iter()
            .find(|a| a.desk_index.single_floor_local() == local && a.exiting_at.is_none());
        // A screen lights only where the room can SEE one. A far-seated desk
        // puts its occupant BEHIND the monitor, so what faces the viewer is the
        // machine's back — `desk.sprite` draws it that way — and a glow there
        // would be light leaking out of a case. Both the tool glow and the
        // standby tint gate on the same fact, or the desk contradicts itself.
        let facing = ctx.layout.desk_facing(local);
        let shows_screen = facing == crate::layout::Facing::North;
        let screen_glow = occupant
            .filter(|_| shows_screen)
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
        let token_tier = occupant.map_or(0, |a| crate::token_meter::token_tier(a.tokens_used));
        let sheet_fall = occupant.and_then(|a| crate::token_meter::sheet_fall_dist(a, ctx.now));
        drawables.push(Drawable {
            anchor_y: desk.y + desk_def.visual.h,
            kind: DrawableKind::DeskCubicle {
                desk,
                facing,
                has_cabinet: i % 2 == 0,
                screen_glow,
                screen_idle: if shows_screen {
                    SCREEN_IDLE_MAX * darkness
                } else {
                    0.0
                },
                has_coffee,
                coffee_steam,
                token_tier,
                sheet_fall,
            },
        });
    }
}

/// Nudge a CENTER-anchored sprite's position so the whole sprite lands inside
/// the canvas. Free-roaming creatures target the WHOLE walkable mask, which
/// reaches within a few columns of the buffer edge, and `blit_centered` clips
/// silently — so a creature resting there rendered sliced in half. Clamping
/// HERE keeps `mascot_position`/`pet_position` pure and keeps the hover box on
/// the pixels actually drawn.
fn keep_sprite_on_canvas(pos: Point, w: u16, h: u16, buf_w: u16, buf_h: u16) -> Point {
    // `min` before `max`: on a buffer narrower than the sprite the lower bound
    // wins (sprite flush left/top) instead of `clamp`'s inverted-range panic.
    Point {
        x: pos.x.min(buf_w.saturating_sub(w.div_ceil(2))).max(w / 2),
        y: pos.y.min(buf_h.saturating_sub(h.div_ceil(2))).max(h / 2),
    }
}

/// The office pet (one per floor). An `active_pet` (mid heart-animation) is
/// pinned in place; otherwise `pet_position` roams it around the idle desks.
/// y-sorted at the CHOSEN anim's south row, since the anims differ in height —
/// a hardcoded offset once painted a sleeping pet over a character in front.
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
    /// Fallback when a custom pack lacks the resolved pet anim: the bundled
    /// cat's size, so the z-sort row and the canvas clamp stay sane — the blit
    /// itself no-ops, `paint_drawable` bails.
    const PET_FALLBACK: Size = Size { w: 8, h: 6 };
    let (pet_w, pet_h) = ctx
        .pack
        .animation(anim_name)
        .and_then(|a| a.frames.first())
        .map_or((PET_FALLBACK.w, PET_FALLBACK.h), |f| {
            (f.width(), f.height())
        });
    let pos = keep_sprite_on_canvas(pos, pet_w, pet_h, ctx.layout.buf_w, ctx.layout.buf_h);
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

/// Enqueue every gateway mascot present in `daemons` (only the ground floor
/// carries the roster, so each mascot shows once). The runtime is responsible
/// for KEEPING the roster honest, so "entry present" tracks "connected +
/// alive", not merely "a hook arrived".
fn enqueue_gateway_mascots<'a>(
    ctx: &PaintCtx<'_>,
    drawables: &mut Vec<Drawable<'a>>,
) -> Vec<MascotFrame> {
    let mut frames = Vec::new();
    for (source, instance, presence) in ctx.scene.daemons() {
        let Some(def) = gateway_mascot_def(source) else {
            continue;
        };
        let seed = crate::creatures::mascot_seed(source, instance);
        let Some((pos, anim_name, frame_idx)) = mascot_position(
            ctx.layout, ctx.pack, presence, def.walk, def.rest, ctx.now, seed,
        ) else {
            continue;
        };
        let (mascot_w, mascot_h) = ctx
            .pack
            .animation(anim_name)
            .and_then(|a| a.frames.first())
            .map_or((14, 12), |f| (f.width(), f.height()));
        let pos =
            keep_sprite_on_canvas(pos, mascot_w, mascot_h, ctx.layout.buf_w, ctx.layout.buf_h);
        let run_count = presence.in_flight_runs.len() as u32;
        let degraded = presence.display_state() == pixtuoid_core::state::DaemonState::Degraded;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, pos, mascot_h),
            kind: DrawableKind::GatewayMascot {
                pos,
                anim_name,
                frame_idx,
                run_count,
                degraded,
            },
        });
        frames.push(MascotFrame {
            pos,
            w: mascot_w,
            h: mascot_h,
            name: def.display_name,
            // Only worth showing when there is something to disambiguate, and
            // that is per SOURCE: two gateways of ONE daemon need their ports,
            // while two daemon sources already read apart by name and sprite.
            instance: (ctx.scene.daemons().filter(|(s, _, _)| *s == source).count() > 1)
                .then(|| instance.as_str().to_string()),
            busy: presence.is_busy(),
            degraded,
            active_sessions: presence.active_sessions,
        });
    }
    frames
}

/// Meeting-room rugs + sofas + tables. A south-of-table sofa faces away, so it
/// y-sorts +3 to occlude its sitter; the north sofa stays +2 so insertion order
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
            // sofas[0] is the north sofa, sofas[1] the south.
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
            // z-key = sprite south row, derived so it can't drift from a
            // visual edit.
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

/// The kitchen island, the lounge couch (emitted ONCE — its seat waypoints
/// share one sprite), and the center-pinned waypoint appliances. The remaining
/// waypoint kinds render via pod-decor or ride the sofa/table, so they emit
/// nothing here.
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

    // Pushed before the character loop so the y-sort tie-break keeps the couch
    // behind its sitters; the rug anchors north of it so the couch sits on it.
    if let Some(center) = layout.couch_sprite_center() {
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
            // The lounge couch IS a vertical-mirrored meeting sofa — same
            // sprite, back facing NORTH toward the windows.
            kind: DrawableKind::MeetingSofa {
                pos: center,
                mirrored: true,
            },
        });
        if let Some(table) = layout.lounge_side_table() {
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
        // The VISUAL height, not the (shallow) footprint, so an overhang still
        // sorts by what's painted.
        let visual_h = furniture_def(wp.kind.furniture()).visual.h;
        match wp.kind {
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let Size { w: cw, h: ch } = layout.pantry_counter_size();
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
            // via `layout.kitchen_island`.
            WaypointKind::MeetingSofa | WaypointKind::MeetingChair | WaypointKind::Island => {}
        }
    }
}

/// Pod-aisle decor and free-standing plants — all center-pinned, y-sorted at
/// the sprite's south row. The mask reads the separate, shallower `footprint`
/// off the same row, so a tall canopy sorts without blocking the aisle.
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
/// elevator door — whose frame is computed stateless from the agents in their
/// entry/exit window, taking the MAX so the door is at least as open as the
/// most-in-progress agent needs.
fn enqueue_floor_fixtures<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    drawables: &mut Vec<Drawable<'a>>,
) {
    if let Some(lamp) = ctx.layout.floor_lamp() {
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
            // One row UNDER the sitter's z — derived from the occupant's OWN
            // view's seat key, so the pair can't drift apart.
            anchor_y: seat::SeatView::of(wp.kind, wp.facing).z_key_for_seat(wp.pos) - 1,
            kind: DrawableKind::MeetingChair {
                pos: wp.pos,
                // The backrest rides the side AWAY from the table: a chair
                // FACING East sits west of the table, bar on its west.
                back_west: wp.facing == crate::layout::Facing::East,
            },
        });
    }
    if let Some(tank) = ctx.layout.fish_tank() {
        let h = crate::layout::furniture_def(crate::layout::Furniture::FishTank)
            .visual
            .h;
        drawables.push(Drawable {
            anchor_y: tank.y + center_pin_south_offset(h),
            kind: DrawableKind::FishTank { pos: tank },
        });
    }
    // Placement + the narrow-fitted-room yield live in `coat_rack_pos` — THE
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

/// Enqueue wall decor (clocks/whiteboards hung on walls). TOP-LEFT anchored at
/// `pos`, unlike the center-pinned furniture.
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
