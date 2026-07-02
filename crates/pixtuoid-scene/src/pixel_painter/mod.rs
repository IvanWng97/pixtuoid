//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! Split from `tui/renderer.rs` to separate the pixel-painting pipeline
//! (called by any renderer impl — `TuiRenderer`, a future web canvas, PNG
//! export, GIF capture) from the ratatui-coupled half-block flush + widget
//! overlay (terminal lifecycle lives with the event loop in `tui/mod.rs`).
//!
//! `render_to_rgb_buffer` is the public entry point. Everything else is
//! private to this module except `character_anchor`, which `widgets.rs`
//! uses for label placement and `hit_test.rs` for mouse hit-testing.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::layout::WALKING_Y_OFF;
use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::chitchat::{self, ActiveChitchat, ChitchatBubble};
use crate::floor::LightingState;
use crate::frame_cache::FrameCache;
use crate::layout::{
    z_sort_row, Anchor, Layout, PlantItem, PodDecorItem, Point, Size, WallDecorItem, WallSegment,
    DESK_H, DESK_W, ELEVATOR_H, ELEVATOR_W,
};
use crate::motion::MotionState;
use crate::pathfind::Router;
use crate::pet::PetFrame;
use crate::pose::{self, Pose};

/// Milliseconds since the Unix epoch for `now` (0 if the clock is before it).
/// The wall-clock decode the pixel-pass animation timers share — was hand-rolled
/// identically at the top of half a dozen paint helpers.
pub(super) fn epoch_ms(now: SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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

pub(crate) use crate::motion::walking_position;
pub use anchors::character_anchor;
use anchors::{
    back_couch_anchor, compute_door_frame_idx, seated_anchor, standing_at_desk_anchor,
    walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath, CHARACTER_SPRITE_W,
};
use background::{
    daylight_floor_overlay, dim_floor_overlay, paint_ceiling_pool, paint_clock,
    paint_corridor_runner, paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel,
    paint_shadow, time_of_day_look, Ellipse,
};
use drawable::{
    gateway_mascot_def, mascot_position, paint_drawable, pet_position, Drawable, DrawableKind,
};
use glass::{paint_glass_wall_h, paint_glass_wall_v, stitch_vertical_wall, WALL_THICK_H_PX};
use palette::{agent_palette, outfit_seed_for, recolor_frame};
use seat::{paint_character_at, seat_sprite, settle_seat_view, SeatView};

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
/// A screenshot/test affordance (`snapshot --weather`) — production never calls
/// it, so live rendering is byte-identical. `Err` carries the valid names when
/// `name` is unknown.
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

// The steam gate reads the SAME window `CoffeeState::record` refreshes on —
// a reference, not a second copy of the value.
const COFFEE_STEAM_WINDOW_SECS: u64 = crate::floor::CoffeeState::STEAM_WINDOW_SECS;

/// The home desk sprite's front lip extends this many px past its blocked
/// footprint (the top-down 3/4 bevel), so the desk's z-sort baseline is the
/// footprint front edge + this overhang — the same "footprint front + sprite
/// overhang" form every other drawable's z-key uses (was a bare `desk.y + 8`).
const DESK_FRONT_OVERHANG: u16 = 2;

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

/// Bundled input for the pixel-painting pass. Constructed from `DrawCtx`
/// fields + per-frame inputs at the `draw_scene` call site.
pub struct PixelCtx<'a> {
    pub scene: &'a SceneState,
    pub layout: &'a Layout,
    pub pack: &'a Pack,
    pub now: SystemTime,
    pub buf: &'a mut RgbBuffer,
    pub cache: &'a mut FrameCache,
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut pose::PoseHistory,
    /// Forwarded from `DrawCtx.motion` — identical lifetime, identical
    /// borrow rules. `derive_with_routing` reads/writes per-agent entries.
    pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
    /// Per-floor max in-flight entry/exit physics duration (ms), forwarded
    /// from `DrawCtx.door_anim_max_ms`. Used by `compute_door_frame_idx`
    /// instead of the old hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
    pub theme: &'a crate::theme::Theme,
    pub floor: crate::floor::FloorMeta,
    pub active_pet: Option<&'a crate::pet::PetState>,
    /// The pet on this floor (kind drives the sprite; name is unused here — the
    /// pixel pass doesn't render the name, the tooltip does).
    pub floor_pet: Option<&'a crate::pet::Pet>,
    pub chitchat_state: &'a mut HashMap<crate::chitchat::VenueKey, ActiveChitchat>,
    /// Carrier → fetch-time view of [`crate::floor::CoffeeState`] (one map:
    /// key present = has a desk cup, value = steam-window anchor).
    pub coffee: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub light: &'a mut crate::floor::LightingState,
    /// When set, composite the walkable / approach / route debug layer over the
    /// finished scene (the live `w` toggle). Off by default; transient.
    pub debug_walkable: bool,
}

pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let agents: Vec<_> = ctx.scene.agents.values().cloned().collect();
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;
    let mut new_coffee_carriers: Vec<pixtuoid_core::AgentId> = Vec::new();

    // Compute time-of-day once per frame and pass to every paint
    // helper that depends on it. Avoids recomputing the chrono local
    // hour for each window + ceiling pool + lamp halo.
    let look = time_of_day_look(ctx.now, ctx.theme);
    // Wall band height tracks layout.top_margin (which is buf_h/4 with
    // a floor) — leaves a 4-px buffer between wall trim and cubicles.
    let top_wall_h = ctx
        .layout
        .top_margin
        .saturating_sub(pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN);
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

    // Per-floor lighting: tick the fade state with the current occupancy.
    // `indoor_scale` smoothly travels from MIN_LEVEL (empty + past
    // debounce) to 1.0 (populated). Windows/skyline are unaffected.
    let indoor_scale = ctx.light.tick(ctx.scene.agents.is_empty(), ctx.now);
    // Empty floors get an extra floor-darken boost on top of the time-of-
    // day dim — there are no monitor/lamp light sources to balance against
    // the overhead darkness, so without the boost they read as "lights
    // off but room weirdly bright."
    let min_level = LightingState::MIN_LEVEL;
    let boost_ceiling = LightingState::EMPTY_FLOOR_DIM_BOOST;
    let empty_floor_boost = 1.0 + (1.0 - indoor_scale) * (boost_ceiling - 1.0) / (1.0 - min_level);

    let dim_strength = (0.45 - ctx.floor.sunlight_boost).max(0.1);
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
    if let Some(pr) = ctx.layout.pantry_room {
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
    let neon_w = 30u16;
    let neon_h = 8u16;
    paint_neon_panel(ctx.buf, 1, 1, neon_w, neon_h, ctx.now, ctx.theme);

    // Live wall clock painted after the wall (so hands sit on top of it)
    // but before wall decor — the bookshelf etc. shouldn't cover it.
    // 7x7 sprite, center at clock_x+3; clamp so it never collides with
    // the 30-wide neon panel on the left.
    let clock_x = (buf_w / 2).saturating_sub(3).max(neon_w + 2);
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

    // Meeting sofas + table, pantry table + chairs are all painted by
    // the y-sorted Drawable pass below (MeetingSofa / MeetingTable /
    // PantryTable / PantryChair variants). They used to be painted
    // here in the background pass too — leftover from before the
    // y-sort refactor; the duplicate paints were dead pixels
    // overwritten 50 lines later. Removed.
    //
    // Entry mat was also painted here (a small blue rug just south of
    // the door). The old wooden-door era used it to define the arrival
    // zone, but the elevator already defines that visually + the blue
    // rectangle looked out of place under the elevator.

    // Procedural room fill — small pixel items that make rooms feel lived-in.
    // Ground footprint rule: walkable mask is NOT affected by these (they're
    // small items characters can walk around or over).
    if let Some(mr) = ctx.layout.meeting_room {
        furniture::paint_notice_board(ctx.buf, mr, ctx.theme);

        // Coat rack is now a y-sorted DrawableKind::CoatRack (pushed in the
        // drawable pass) so characters in front occlude it / behind it are
        // occluded — was painted here in the background pass, always under
        // every character.

        furniture::paint_doormat(ctx.buf, mr, ctx.theme);
    }
    if let Some(pr) = ctx.layout.pantry_room {
        furniture::paint_water_cooler(ctx.buf, pr, ctx.theme);
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
        if matches!(wp.kind, WaypointKind::Couch | WaypointKind::Printer) {
            continue;
        }
        paint_shadow(
            ctx.buf,
            Ellipse {
                cx: wp.pos.x,
                cy: wp.pos.y + 2,
                half_w: 7,
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

    // Build per-frame occupancy from STATIONARY agent positions only — BEFORE
    // the seated prepass, since the single per-agent `derive_with_routing` below
    // routes Walking poses against THIS overlay (it used to run after the prepass,
    // which then re-derived each agent a second time inside `enqueue_characters`).
    // Walkers are deliberately excluded — their position interpolates every frame,
    // which would change the overlay signature every frame, wipe the path cache,
    // recompute A*, and snap walkers to new path segments (the visible "flash").
    // Sitters at desks are already covered by the static desk mask. Only waypoint
    // visitors contribute here — they have stable positions across frames, so the
    // signature is stable and the cache hits. Reads only the STATELESS `pose::derive`
    // + stand_point (no dependency on the seated map / ambient), so it's safe up here.
    ctx.overlay.clear();
    for agent in &agents {
        let Some(pose) = pose::derive(agent, ctx.now, ctx.layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = ctx.layout.waypoints.get(wp) {
                // Reserve the cell the agent actually stands on (the stand cell,
                // off the furniture), NOT the blocked furniture center — else
                // another agent's A* routes straight through the stander. Same
                // `desk` origin as every other stand_point caller.
                let origin = ctx
                    .layout
                    .home_desk(agent.desk_index.single_floor_local())
                    .unwrap_or(w.pos);
                let stand = pixtuoid_core::layout::stand_point(
                    w.kind,
                    w.pos,
                    ctx.layout.pantry_counter_size,
                    &ctx.layout.walkable,
                    origin,
                    w.facing,
                );
                ctx.overlay
                    .add(stand.x.saturating_sub(4), stand.y.saturating_sub(6), 8, 12);
            }
        }
    }

    // Derive every home-desk agent's routed pose ONCE per frame. This is the
    // AUTHORITATIVE pose derivation — it runs the advance_wander / walk_path /
    // history side effects exactly once; `enqueue_characters` later just looks the
    // cached pose up by agent_id instead of re-deriving (the old double-A*). The
    // `exiting_at` filter is INTENTIONALLY absent: exiting agents are never
    // SeatedTyping/Thinking (so `seated_agents` is unchanged), but their pose is
    // needed for the character enqueue. Only the home_desk filter remains (a
    // deskless agent can't render anyway).
    let poses: HashMap<pixtuoid_core::AgentId, Option<Pose>> = agents
        .iter()
        .filter(|a| {
            ctx.layout
                .home_desk(a.desk_index.single_floor_local())
                .is_some()
        })
        .map(|a| {
            let p = pose::derive_with_routing(
                a,
                ctx.now,
                ctx.layout,
                &mut crate::pose::RouteCtx {
                    router: &mut *ctx.router,
                    overlay: &*ctx.overlay,
                    history: &mut *ctx.history,
                    motion: &mut *ctx.motion,
                },
            );
            (a.agent_id, p)
        })
        .collect();

    // Per-desk "is the occupant actually seated right now" map (pose is
    // SeatedTyping/Thinking, not walking in / snapping back), derived from the
    // cached poses so the desk-cubicle screen glow + ceiling halos share one gate
    // and one pose derivation (no double A*). Exiting agents are absent from the
    // seated set by construction (their pose is Walking, not Seated).
    let seated_agents: HashMap<FloorLocalDeskIndex, bool> = agents
        .iter()
        .filter(|a| {
            ctx.layout
                .home_desk(a.desk_index.single_floor_local())
                .is_some()
                && a.exiting_at.is_none()
        })
        .map(|a| {
            let seated = matches!(
                poses.get(&a.agent_id),
                Some(Some(Pose::SeatedTyping { .. } | Pose::SeatedThinking))
            );
            (a.desk_index.single_floor_local(), seated)
        })
        .collect();

    // Ceiling halos gate on `seated_agents` so a tool-glow halo never floats
    // above an empty desk while its Active occupant is mid-walk (entry/snap).
    // `look` was already computed once per frame above — forward it so the
    // ambient sub-passes don't recompute `time_of_day_look(now, theme)`.
    ambient::paint_ambient(ctx, &look, &seated_agents);

    // --- Build the y-sortable middle pass -------------------------------
    //
    // Every entity gets an `anchor_y` representing its front-facing /
    // floor-touching row. Sort ascending and paint in order so things
    // closer to the camera (larger anchor_y) appear in front. This is
    // the painter's algorithm applied to a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::new();

    enqueue_desk_cubicles(ctx, &agents, &seated_agents, &mut drawables);

    enqueue_meeting_furniture(ctx.layout, &mut drawables);

    enqueue_lounge_pantry_appliances(ctx.layout, &mut drawables);

    enqueue_pod_decor_and_plants(ctx.layout, &mut drawables);
    enqueue_floor_fixtures(ctx, &agents, &mut drawables);
    enqueue_wall_decor(ctx.layout, &mut drawables);

    let resolved_pet_pos = enqueue_pet(ctx, &agents, &mut drawables);
    let resolved_mascot_pos = enqueue_gateway_mascot(ctx, &mut drawables);

    let waypoint_visitors = enqueue_characters(
        ctx,
        &agents,
        &poses,
        &mut drawables,
        &mut new_coffee_carriers,
    );

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

    let chitchat_bubbles = chitchat::update_and_collect(
        ctx.chitchat_state,
        ctx.floor.floor_idx,
        &waypoint_visitors,
        ctx.now,
    );

    PixelPassResult {
        pet_pos: resolved_pet_pos,
        mascot_pos: resolved_mascot_pos,
        chitchat_bubbles,
        new_coffee_carriers,
    }
}

/// All character sprites — the y-sorted middle pass's main subject. For each
/// agent it looks up the routed pose (entry/exit/wander/seated) from `poses` —
/// the cached map the authoritative `derive_with_routing` prepass built ONCE this
/// frame (so the advance_wander / walk_path / history side effects run exactly
/// once, not twice) — and enqueues the sprite at the feet anchor. Returns the
/// waypoint visitors (for the chitchat venues) and pushes any agent seen
/// carrying coffee this frame into `new_coffee_carriers`. The Character
/// drawable borrows the agent, so this is the ONE phase tied to the agent
/// set's lifetime `'a`.
fn enqueue_characters<'a>(
    ctx: &mut PixelCtx<'_>,
    agents: &'a [AgentSlot],
    poses: &HashMap<pixtuoid_core::AgentId, Option<Pose>>,
    drawables: &mut Vec<Drawable<'a>>,
    new_coffee_carriers: &mut Vec<pixtuoid_core::AgentId>,
) -> Vec<chitchat::Visitor> {
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    let mut waypoint_visitors: Vec<chitchat::Visitor> = Vec::new();
    // All 3 lounge-couch seat waypoints collapse to ONE chitchat venue (keyed
    // on the first couch's index) so the couch hosts a single group
    // conversation like the meeting room — without overloading the
    // meeting-only `room_id` field (which indexes `meeting_furniture`).
    let couch_group_idx = ctx
        .layout
        .waypoints
        .iter()
        .position(|w| w.kind == crate::layout::WaypointKind::Couch);
    // The pack's character sprite width (8 for the bundled pack, 10 for the
    // robot pack). All character poses share one width, so resolve it ONCE from
    // a reference pose and center every anchor on it — a non-8-wide pack would
    // otherwise blit ~1px off (the anchors hardcoded 8). Fallback to the bundled
    // default if the pack lacks the reference anim.
    let char_w = ctx
        .pack
        .animation("standing")
        .and_then(|a| a.frames.first())
        .map_or(CHARACTER_SPRITE_W, |f| f.width());
    for agent in agents {
        let Some(desk) = ctx.layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // Look up the pose the authoritative prepass already derived (one
        // derive_with_routing per agent per frame) instead of re-deriving — the
        // prepass ran the advance_wander/walk_path/history side effects once.
        let Some(p) = poses.get(&agent.agent_id).copied().flatten() else {
            continue;
        };
        match p {
            Pose::SeatedIdle => {
                let anchor_no_breath = seated_anchor(desk, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                let sleep_variant = if agent.agent_id.raw() % 2 == 0 {
                    "seated_sleeping"
                } else {
                    "seated_sleeping_alt"
                };
                drawables.push(Drawable {
                    // Breath-independent z-key (matches AtWaypoint/AimlessAt):
                    // the ±1px breath must not flip sort order against nearby
                    // desk decor frame-to-frame.
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: sleep_variant,
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: Some(agent.agent_id.raw()),
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedThinking => {
                let anchor_no_breath = seated_anchor(desk, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    // Breath-independent z-key (matches AtWaypoint/AimlessAt):
                    // the ±1px breath must not flip sort order against nearby
                    // desk decor frame-to-frame.
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "seated",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: Some(ctx.theme.tool_glow.default),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedTyping { frame } => {
                let anchor_no_breath = seated_anchor(desk, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    // Breath-independent z-key (matches AtWaypoint/AimlessAt):
                    // the ±1px breath must not flip sort order against nearby
                    // desk decor frame-to-frame.
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "typing",
                        frame_idx: frame,
                        anchor,
                        flip_x: false,
                        glow_tint: palette::tool_glow_tint(agent, &ctx.theme.tool_glow),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::StandingAtDesk => {
                let anchor_no_breath = standing_at_desk_anchor(desk, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                let is_waiting = matches!(agent.state, ActivityState::Waiting { .. });
                drawables.push(Drawable {
                    // Breath-independent z-key (matches AtWaypoint/AimlessAt):
                    // the ±1px breath must not flip sort order against nearby
                    // desk decor frame-to-frame.
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: is_waiting,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = ctx.layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    use crate::layout::WaypointKind;
                    // Render anchor: the cell the agent occupies. For obstacles
                    // this is the side stand cell (side-aware); for seats it is
                    // `wp.pos` (the sprite sits ON the furniture) — the walk-in
                    // approach cell is resolved separately by `approach_point`.
                    let stand = pixtuoid_core::layout::stand_point(
                        wp_obj.kind,
                        wp_obj.pos,
                        ctx.layout.pantry_counter_size,
                        &ctx.layout.walkable,
                        desk,
                        wp_obj.facing,
                    );
                    let (anim_name, anchor_base, sprite_h, flip_x) = match kind {
                        WaypointKind::Pantry => (
                            "holding_coffee",
                            waypoint_anchor(stand, char_w),
                            12u16,
                            false,
                        ),
                        // Lounge couch + meeting sofa: the sprite follows the
                        // SEATED facing (couch always North/window → back_couch;
                        // the sofa's two seats face each other across the table).
                        // Both reuse the 16×7-sofa anchor.
                        WaypointKind::Couch | WaypointKind::MeetingSofa => {
                            let (anim, flip) = seat_sprite(kind, wp_obj.facing);
                            (anim, back_couch_anchor(stand, char_w), 9u16, flip)
                        }
                        // Meeting stand: beside the table, facing inward.
                        WaypointKind::MeetingStand => {
                            let (anim, flip) = seat_sprite(kind, wp_obj.facing);
                            (anim, waypoint_anchor(stand, char_w), 12u16, flip)
                        }
                        // PhoneBooth + StandingDesk → agent just stands at the
                        // decor. waypoint_anchor positions them directly above
                        // the decor centre (sprite footprint sits just north
                        // of the decor's centre, head visible above).
                        WaypointKind::PhoneBooth
                        | WaypointKind::StandingDesk
                        | WaypointKind::VendingMachine
                        | WaypointKind::Printer => {
                            ("standing", waypoint_anchor(stand, char_w), 12u16, false)
                        }
                    };
                    let anchor_no_breath = Point {
                        x: anchor_base.x.saturating_add_signed(dx),
                        y: anchor_base.y,
                    };
                    if chitchat::supports_chitchat(kind) {
                        waypoint_visitors.push(chitchat::Visitor {
                            // Couch seats share one venue (group chat); other
                            // waypoints key on their own index.
                            wp_idx: chitchat::venue_wp_idx(kind, wp, couch_group_idx),
                            agent_id: agent.agent_id,
                            anchor: anchor_no_breath,
                            room_id: wp_obj.room_id,
                        });
                    }
                    let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                    drawables.push(Drawable {
                        // Breath-independent sort key: a seated occupant must
                        // y-sort identically every frame so the breath ±1px never
                        // flips it under its sofa (the overlap bug). The visual
                        // `anchor` above still breathes; only the z-order is pinned.
                        //
                        // Seats route through `SeatView::z_key_for_seat` — the SAME
                        // key the sit-down/stand-up glide uses, so the agent can't
                        // pop across its furniture's z-key at the walk→seat seam.
                        // (back/front sofa+couch → pos+2; stand → pos+3, clearing
                        // the meeting table.) Obstacles (pantry/booth/vending/
                        // printer) keep the stand-at-the-approach-cell key — the
                        // agent stands AT them, there is no settle onto them.
                        anchor_y: match kind {
                            WaypointKind::Couch
                            | WaypointKind::MeetingSofa
                            | WaypointKind::MeetingStand => {
                                SeatView::of(kind, wp_obj.facing).z_key_for_seat(stand)
                            }
                            _ => anchor_no_breath.y + sprite_h,
                        },
                        kind: DrawableKind::Character {
                            agent,
                            anim_name,
                            frame_idx: 0,
                            anchor,
                            flip_x,
                            glow_tint: None,
                            sleep_z_seed: None,
                            waiting_bubble: false,
                            walking_dust_frame: None,
                        },
                    });
                }
            }
            Pose::AimlessAt { dest } => {
                // Breath-independent sort key (like the AtWaypoint arm): the
                // ±1px breath bob must not flicker the z-order frame to frame.
                let anchor_no_breath = waypoint_anchor(dest, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::Walking {
                from,
                to,
                t_x1000,
                frame,
                mut carrying_coffee,
            } => {
                // Exit walks: core sets carrying_coffee=false (no
                // render-side state), but we know from the coffee map.
                if agent.exiting_at.is_some() && ctx.coffee.contains_key(&agent.agent_id) {
                    carrying_coffee = true;
                }
                if carrying_coffee {
                    new_coffee_carriers.push(agent.agent_id);
                }
                let pos = walking_position(from, to, t_x1000);
                let walker_anchor = walking_anchor(pos, char_w);
                let dx = to.x as i32 - from.x as i32;
                let dy = to.y as i32 - from.y as i32;
                // A sit-down glide onto a seat faces the SEAT's seated direction
                // (single source of truth — same `facing` as the seated render),
                // NOT the travel direction. Without this a window-facing seat
                // (couch / south meeting sofa, approached from the north, foot-cell
                // to the south) renders a FRONT walk and the agent sits facing the
                // camera until it snaps to `back_couch` at AtWaypoint. With it the
                // agent backs into the seat already facing the window — no late
                // flip. Ordinary travel segments keep the travel-direction rule.
                // On the sit arc? `to` is a foot-cell while settling ONTO a seat
                // (sit-down); `from` is a foot-cell while rising OFF one
                // (stand-up). Either way the agent renders in the SEAT's view and
                // at the SEAT's stable z-key for the whole glide — same single
                // source as the seated render — so it neither faces the wrong way
                // nor crosses its furniture's z-key mid-glide. Ordinary travel
                // segments keep the travel-direction facing and foot-position
                // z-key.
                let settle =
                    settle_seat_view(to, ctx.layout).or_else(|| settle_seat_view(from, ctx.layout));
                let (going_back, flip) = match settle {
                    Some((view, _)) => view.settle_walk(),
                    None => (
                        dy.unsigned_abs() > dx.unsigned_abs() && dy < 0,
                        to.x < from.x,
                    ),
                };
                // walking_back always wins (no back-facing coffee sprite).
                let anim_name: &'static str = if going_back {
                    "walking_back"
                } else if carrying_coffee && ctx.pack.animation("walking_coffee").is_some() {
                    "walking_coffee"
                } else {
                    "walking"
                };
                drawables.push(Drawable {
                    anchor_y: match settle {
                        Some((_, z_key)) => z_key,
                        None => walker_anchor.y + WALKING_Y_OFF,
                    },
                    kind: DrawableKind::Character {
                        agent,
                        anim_name,
                        frame_idx: frame,
                        anchor: walker_anchor,
                        flip_x: flip,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: Some(frame),
                    },
                });
            }
        }
    }
    waypoint_visitors
}

/// Horizontal (E-W) room dividers join the y-sort, anchored at their south
/// (front) edge so a character standing behind (north of) the wall is
/// composited over by the frosted glass rather than painting on top of it.
/// The vertical (edge-on) dividers already painted in the background pass.
/// Emitted LAST so a character tied with a wall row still paints behind it.
fn enqueue_room_walls_h<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for &WallSegment { start, end } in &layout.room_walls {
        if start.y == end.y {
            drawables.push(Drawable {
                anchor_y: start.y + (WALL_THICK_H_PX - 1),
                kind: DrawableKind::RoomWallH {
                    x0: start.x.min(end.x),
                    x1: start.x.max(end.x),
                    y_top: start.y,
                },
            });
        }
    }
}

/// Desk cubicles — each carries its divider + cabinet + bin + screen glow.
/// The desk sprite (16×8) sorts at `desk.y + footprint_h + DESK_FRONT_OVERHANG`
/// (front-lip overhang past the blocked footprint), just past the seated
/// worker's feet (`desk.y + 4`) so the sitter stays visually behind the desk.
/// `seated_agents` (built once before the ambient pass) gates the screen glow
/// so it only paints for a worker actually at the desk. The DeskCubicle
/// drawable is Copy, so this borrows nothing from the agent set.
fn enqueue_desk_cubicles<'a>(
    ctx: &PixelCtx<'_>,
    agents: &[AgentSlot],
    seated_agents: &HashMap<FloorLocalDeskIndex, bool>,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for (i, &desk) in ctx.layout.home_desks.iter().enumerate() {
        let local = FloorLocalDeskIndex(i);
        let Size {
            w: desk_fp_w,
            h: desk_fp_h,
        } = crate::layout::desk_furniture_def()
            .footprint
            .unwrap_or(Size {
                w: DESK_W,
                h: DESK_H,
            });
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
            anchor_y: desk.y + desk_fp_h + DESK_FRONT_OVERHANG,
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
    ctx: &PixelCtx<'_>,
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
    ctx: &PixelCtx<'_>,
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
                degraded: presence.state == pixtuoid_core::state::DaemonState::Degraded,
            },
        });
        // First present gateway wins the hover frame (single-gateway today).
        hover.get_or_insert(MascotFrame {
            pos,
            name: def.display_name,
            busy: presence.state == pixtuoid_core::state::DaemonState::Busy,
            degraded: presence.state == pixtuoid_core::state::DaemonState::Degraded,
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
    for room in &layout.meeting_furniture {
        let table = room.table;
        let [ts, bs] = room.sofas;
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
    for room in &layout.meeting_furniture {
        for (i, sofa) in room.sofas.into_iter().enumerate() {
            // sofas[0] is the north sofa, sofas[1] the south — the south sofa
            // faces away (`mirrored`) and y-sorts +3 to occlude its sitter.
            let mirrored = i % 2 != 0;
            let faces_away = sofa.y >= room.table.y;
            drawables.push(Drawable {
                anchor_y: sofa.y + if faces_away { 3 } else { 2 },
                kind: DrawableKind::MeetingSofa {
                    pos: sofa,
                    mirrored,
                },
            });
        }
    }
    for room in &layout.meeting_furniture {
        drawables.push(Drawable {
            // z-key = sprite south row, derived from the table (== +2 for the
            // 11×5 meeting-table sprite) so it can't drift from a visual edit.
            anchor_y: z_sort_row(
                Anchor::Center,
                room.table,
                crate::layout::furniture_def(crate::layout::Furniture::MeetingTable)
                    .visual
                    .h,
            ),
            kind: DrawableKind::MeetingTable { pos: room.table },
        });
    }
}

/// Pantry bistro table + stools, the lounge couch (emitted ONCE via
/// `couch_sprite_center` — 3 seat waypoints share one sprite), and the
/// center-pinned waypoint appliances (pantry counter, vending, printer).
/// PhoneBooth/StandingDesk render via pod-decor; meeting slots ride the
/// sofa/table — so those waypoint kinds emit nothing here.
fn enqueue_lounge_pantry_appliances<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    if let Some(table) = layout.pantry_table {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                table,
                crate::layout::furniture_def(crate::layout::Furniture::PantryTable)
                    .visual
                    .h,
            ),
            kind: DrawableKind::PantryTable { pos: table },
        });
    }
    for chair in &layout.pantry_chairs {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                *chair,
                crate::layout::furniture_def(crate::layout::Furniture::PantryChair)
                    .visual
                    .h,
            ),
            kind: DrawableKind::PantryChair { pos: *chair },
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

    for wp in &layout.waypoints {
        use crate::layout::{furniture_def, WaypointKind};
        // y-sort baseline = the sprite's south row (these appliances are
        // center-pinned at `pos`). Read the VISUAL height, not the (shallow)
        // footprint, so an overhang would still sort by what's painted.
        let visual_h = furniture_def(wp.kind.furniture()).visual.h;
        match wp.kind {
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let Size { w: cw, h: ch } = layout.pantry_counter_size; // runtime-sized
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, ch),
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        use_large: cw >= 32,
                    },
                });
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            WaypointKind::VendingMachine => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                    kind: DrawableKind::VendingMachine { pos: wp.pos },
                });
            }
            WaypointKind::Printer => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                    kind: DrawableKind::Printer { pos: wp.pos },
                });
            }
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {}
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
    ctx: &PixelCtx<'_>,
    agents: &[AgentSlot],
    drawables: &mut Vec<Drawable<'a>>,
) {
    if let Some(lamp) = ctx.layout.floor_lamp {
        drawables.push(Drawable {
            anchor_y: lamp.y + floor_lamp_south_offset(),
            kind: DrawableKind::FloorLamp { pos: lamp },
        });
    }
    if let Some(mr) = ctx.layout.meeting_room {
        if mr.width > 20 {
            let cx = mr.x + mr.width - 5;
            let cy = mr.y + mr.height / 2 - 4;
            drawables.push(Drawable {
                anchor_y: cy + 7,
                kind: DrawableKind::CoatRack {
                    pos: Point { x: cx, y: cy },
                },
            });
        }
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
