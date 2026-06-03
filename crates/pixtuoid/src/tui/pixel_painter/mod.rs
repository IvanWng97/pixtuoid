//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! Split from `tui/renderer.rs` to separate the pixel-painting pipeline
//! (called by any renderer impl — `TuiRenderer`, a future web canvas, PNG
//! export, GIF capture) from the ratatui-coupled half-block flush + widget
//! overlay + terminal lifecycle.
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
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::tui::chitchat::{self, ActiveChitchat, ChitchatBubble};
use crate::tui::floor::LightingState;
use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{
    z_sort_row, Anchor, Layout, PlantItem, PodDecorItem, Point, Size, WallDecorItem, WallSegment,
    DESK_H, DESK_W, ELEVATOR_H, ELEVATOR_W,
};
use crate::tui::motion::MotionState;
use crate::tui::pathfind::Router;
use crate::tui::pet::PetFrame;
use crate::tui::pose::{self, Pose};

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
/// caller can persist them into `coffee_holders`).
pub struct PixelPassResult {
    pub pet_pos: Option<PetFrame>,
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agent ids observed in `Walking { carrying_coffee: true }` this
    /// frame. The caller inserts them into the persistent
    /// `coffee_holders` set and records `coffee_fetched_at`.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
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

pub(in crate::tui) use anchors::character_anchor;
pub(in crate::tui) use anchors::walking_position;
use anchors::{
    back_couch_anchor, compute_door_frame_idx, seated_anchor, standing_at_desk_anchor,
    walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath, CHARACTER_SPRITE_W,
};
use background::{
    daylight_floor_overlay, dim_floor_overlay, paint_ceiling_pool, paint_clock,
    paint_corridor_runner, paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel,
    paint_shadow, time_of_day_look, Ellipse,
};
use drawable::{paint_drawable, pet_position, Drawable, DrawableKind};
use glass::{paint_glass_wall_h, paint_glass_wall_v, stitch_vertical_wall, WALL_THICK_H_PX};
use palette::{agent_palette, recolor_frame};
use seat::{paint_character_at, seat_sprite, settle_seat_view, SeatView};

const COFFEE_STEAM_WINDOW_SECS: u64 = 120;

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
        crate::tui::layout::furniture_def(crate::tui::layout::Furniture::FloorLamp)
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
    pub theme: &'a crate::tui::theme::Theme,
    pub floor: crate::tui::floor::FloorMeta,
    pub active_pet: Option<&'a crate::tui::renderer::PetState>,
    /// The pet on this floor (kind drives the sprite; name is unused here — the
    /// pixel pass doesn't render the name, the tooltip does).
    pub floor_pet: Option<&'a crate::tui::pet::Pet>,
    pub chitchat_state: &'a mut HashMap<crate::tui::chitchat::VenueKey, ActiveChitchat>,
    pub coffee_holders: &'a std::collections::HashSet<pixtuoid_core::AgentId>,
    pub coffee_fetched_at: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub light: &'a mut crate::tui::floor::LightingState,
    /// When set, composite the walkable / approach / route debug layer over the
    /// finished scene (the live `w` toggle). Off by default; transient.
    pub debug_walkable: bool,
}

pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let agents: Vec<_> = ctx.scene.agents.values().cloned().collect();
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;
    let mut resolved_pet_pos: Option<PetFrame> = None;
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
        use crate::tui::layout::WaypointKind;
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
        .filter(|w| w.kind == crate::tui::layout::WaypointKind::Printer)
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
            + center_pin_south_offset(crate::tui::layout::furniture_def(kind.furniture()).visual.h);
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

    // Per-desk "is the occupant actually seated right now" map (pose is
    // SeatedTyping/Thinking, not walking in / snapping back). Built ONCE here
    // (before the ambient pass) and reused by the desk-cubicle screen glow
    // below — so the ceiling halo and the screen glow share one gate and one
    // pose derivation (no double A*).
    let seated_agents: HashMap<usize, bool> = agents
        .iter()
        .filter(|a| a.desk_index < ctx.layout.home_desks.len() && a.exiting_at.is_none())
        .map(|a| {
            let p = pose::derive_with_routing(
                a,
                ctx.now,
                ctx.layout,
                &mut crate::tui::pose::RouteCtx {
                    router: &mut *ctx.router,
                    overlay: &*ctx.overlay,
                    history: &mut *ctx.history,
                    motion: &mut *ctx.motion,
                },
            );
            let seated = matches!(p, Some(Pose::SeatedTyping { .. } | Pose::SeatedThinking));
            (a.desk_index, seated)
        })
        .collect();

    // Ceiling halos gate on `seated_agents` so a tool-glow halo never floats
    // above an empty desk while its Active occupant is mid-walk (entry/snap).
    ambient::paint_ambient(ctx, &seated_agents);

    // Build per-frame occupancy from STATIONARY agent positions only.
    // Walkers are deliberately excluded — their position interpolates
    // every frame, which would change the overlay signature every frame,
    // wipe the path cache, recompute A*, and snap walkers to new path
    // segments (the visible "flash"). Sitters at desks are already
    // covered by the static desk mask. Only waypoint visitors
    // contribute here — they have stable positions across frames,
    // so the signature is stable and the cache hits.
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
                    .home_desks
                    .get(agent.desk_index)
                    .copied()
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

    // --- Build the y-sortable middle pass -------------------------------
    //
    // Every entity gets an `anchor_y` representing its front-facing /
    // floor-touching row. Sort ascending and paint in order so things
    // closer to the camera (larger anchor_y) appear in front. This is
    // the painter's algorithm applied to a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::new();

    // Desk cubicles (each carries its divider + cabinet + bin + screen
    // glow). Sprite is 16×8, so the actual bottom edge is desk.y + 8 —
    // just past the seated character's feet (desk.y + 4), which keeps
    // the seated worker visually behind the desk like it always was.
    // (`seated_agents` was built once above, before the ambient pass.)
    for (i, &desk) in ctx.layout.home_desks.iter().enumerate() {
        let Size {
            w: desk_fp_w,
            h: desk_fp_h,
        } = crate::tui::layout::desk_furniture_def()
            .footprint
            .unwrap_or(Size {
                w: DESK_W,
                h: DESK_H,
            });
        let is_last_col = desk.x + desk_fp_w + DESK_W
            >= ctx.layout.cubicle_band.x + ctx.layout.cubicle_band.width;
        let occupant = agents
            .iter()
            .find(|a| a.desk_index == i && a.exiting_at.is_none());
        let screen_glow = occupant
            .filter(|_| seated_agents.get(&i).copied().unwrap_or(false))
            .and_then(|a| palette::tool_glow_tint(a, &ctx.theme.tool_glow));
        let session_age_secs = occupant
            .and_then(|a| ctx.now.duration_since(a.created_at).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let has_coffee = occupant.is_some_and(|a| ctx.coffee_holders.contains(&a.agent_id));
        let coffee_steam = has_coffee
            && occupant.is_some_and(|a| {
                ctx.coffee_fetched_at
                    .get(&a.agent_id)
                    .and_then(|t| ctx.now.duration_since(*t).ok())
                    .is_some_and(|d| d.as_secs() < COFFEE_STEAM_WINDOW_SECS)
            });
        drawables.push(Drawable {
            // z-sort baseline = the footprint's south/front edge + the desk
            // sprite's front-lip overhang, mirroring every other drawable's
            // "footprint front + sprite overhang" form (here: 6 + 2 = 8). The
            // desk's front legs/lip extend DESK_FRONT_OVERHANG px past its
            // blocked footprint (top-down 3/4 bevel), so it sorts there.
            anchor_y: desk.y + desk_fp_h + DESK_FRONT_OVERHANG,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col,
                has_cabinet: i % 2 == 0,
                screen_glow,
                session_age_secs,
                has_coffee,
                coffee_steam,
            },
        });
    }

    // Meeting-room area rug — sized to span both sofas + the coffee
    // table with a small margin. Anchored at the TOP so y-sort paints
    // it before the furniture sitting on top of it.
    // Meeting-room area rugs + sofas + tables. For dual-meeting layouts,
    // sofas come in pairs (2 per room), tables 1 per room.
    let sofas_per_room = if ctx.layout.meeting_tables.len() > 1 {
        2
    } else {
        ctx.layout.meeting_sofas.len()
    };
    for (room_idx, &table) in ctx.layout.meeting_tables.iter().enumerate() {
        let sofa_start = room_idx * sofas_per_room;
        let top_sofa = ctx.layout.meeting_sofas.get(sofa_start);
        let bot_sofa = ctx.layout.meeting_sofas.get(sofa_start + 1);
        if let (Some(&ts), Some(&bs)) = (top_sofa, bot_sofa) {
            let rug_w = 18u16;
            let rug_h =
                bs.y.saturating_sub(ts.y)
                    .saturating_add(8)
                    .min(ctx.layout.buf_h.saturating_sub(table.y).saturating_add(8));
            drawables.push(Drawable {
                anchor_y: table.y.saturating_sub(rug_h / 2),
                kind: DrawableKind::AreaRug {
                    pos: table,
                    width: rug_w,
                    height: rug_h,
                },
            });
        }
    }
    for (i, &sofa) in ctx.layout.meeting_sofas.iter().enumerate() {
        let mirrored = i % 2 != 0;
        // A south-of-table sofa faces away (Facing::North → `back_couch`
        // sprite), so the sitter sits BEHIND the sofa back and must be
        // occluded by it — same as the lounge couch. The sitter's y-sort
        // key is `sofa.y + 2`, so the back sofa needs +3 to win that tie;
        // the front (north) sofa stays +2 so its sitter paints on top
        // (insertion order breaks the tie in the sitter's favor). Mirrors
        // core's facing rule (`compute.rs`): back iff sofa.y >= table.y.
        let room_id = i / 2;
        let table_y = ctx
            .layout
            .meeting_tables
            .get(room_id)
            .map_or(sofa.y, |t| t.y);
        let faces_away = sofa.y >= table_y;
        drawables.push(Drawable {
            anchor_y: sofa.y + if faces_away { 3 } else { 2 },
            kind: DrawableKind::MeetingSofa {
                pos: sofa,
                mirrored,
            },
        });
    }
    for &table in &ctx.layout.meeting_tables {
        drawables.push(Drawable {
            // z-key = sprite south row, derived from the table (== +2 for the
            // 11×5 coffee-table sprite) so it can't drift from a visual edit.
            anchor_y: z_sort_row(
                Anchor::Center,
                table,
                crate::tui::layout::furniture_def(crate::tui::layout::Furniture::MeetingTable)
                    .visual
                    .h,
            ),
            kind: DrawableKind::MeetingTable { pos: table },
        });
    }

    // Pantry bistro table — z-key = sprite south row, derived from the table's
    // own visual height (was a hand-rolled `table.y + 1`).
    if let Some(table) = ctx.layout.pantry_table {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                table,
                crate::tui::layout::furniture_def(crate::tui::layout::Furniture::PantryTable)
                    .visual
                    .h,
            ),
            kind: DrawableKind::PantryTable { pos: table },
        });
    }
    // Pantry stools (centered) — z-key derived from the stool visual height.
    for chair in &ctx.layout.pantry_chairs {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                *chair,
                crate::tui::layout::furniture_def(crate::tui::layout::Furniture::PantryChair)
                    .visual
                    .h,
            ),
            kind: DrawableKind::PantryChair { pos: *chair },
        });
    }

    // Lounge couch furniture — emitted ONCE, centred on the sofa via
    // `couch_sprite_center`. The couch is now 3 separate seat waypoints, so
    // per-seat emission would triple-paint the sofa/rug/table. Decor → pushed
    // before the character loop so the y-sort tie-break keeps the couch behind
    // its sitters. The rug anchors BEHIND (north of) the couch so it spans the
    // floor on the south side; y-sort anchor at the top so the couch sits on it.
    if let Some(center) = ctx.layout.couch_sprite_center {
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
                crate::tui::layout::furniture_def(crate::tui::layout::Furniture::Couch)
                    .visual
                    .h,
            ),
            kind: DrawableKind::WaypointCouch { pos: center },
        });
        if let Some(table) = ctx.layout.lounge_side_table {
            drawables.push(Drawable {
                anchor_y: z_sort_row(
                    Anchor::Center,
                    table,
                    crate::tui::layout::furniture_def(
                        crate::tui::layout::Furniture::LoungeSideTable,
                    )
                    .visual
                    .h,
                ),
                kind: DrawableKind::LoungeSideTable { pos: table },
            });
        }
    }

    // Waypoint furniture — pantry counter, vending, printer — centered on the
    // waypoint position. PhoneBooth/StandingDesk render via the `pod_decor`
    // drawables below (they ARE the decor). The lounge couch is emitted once
    // above (it spans 3 seat waypoints).
    for wp in &ctx.layout.waypoints {
        use crate::tui::layout::{furniture_def, WaypointKind};
        // Depth (y-sort) baseline = the sprite's south row, via
        // `center_pin_south_offset` (these appliances are center-pinned at
        // `pos`). Read the VISUAL height — the drawn sprite's south, NOT the
        // (now shallow) footprint: if an appliance ever grows an overhang the
        // z-key must still track what's painted. Equal for today's flat boxes.
        let visual_h = furniture_def(wp.kind.furniture()).visual.h;
        match wp.kind {
            // Rendered once via `couch_sprite_center` above (3 seats, 1 sprite).
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let Size { w: cw, h: ch } = ctx.layout.pantry_counter_size; // runtime-sized
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, ch),
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        use_large: cw >= 32,
                    },
                });
            }
            // Rendered via the `pod_decor` drawables below (they ARE the decor).
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
            // Rendered via the `meeting_sofas` / `meeting_tables` drawables
            // elsewhere (the slots ride on the sofa/table) — nothing per-slot.
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {}
        }
    }

    // Pod-aisle decor (plant / whiteboard / TV / phone booth /
    // standing desk). All centered at `pos`; anchor at the bottom of
    // the sprite footprint so y-sort places them correctly against
    // walkers and characters in the aisles.
    for &PodDecorItem { kind, pos } in &ctx.layout.pod_decor {
        // Visual sprite height from the one furniture table (the mask reads the
        // separate `footprint` off the same row — so a tall plant's canopy can
        // sort correctly without blocking the aisle).
        let Size { h, .. } = crate::tui::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, pos, h),
            kind: DrawableKind::PodDecorItem { kind, pos },
        });
    }

    // Plants — center-pinned; z-key = sprite south row. Height is the single
    // source `furniture_def(kind.furniture()).visual.1` (was a parallel fudged
    // match that drifted: it over-shot Flower/Succulent by one and faked Tall
    // via `9` instead of `(10-1)/2`). The drop-shadow uses the same offset off
    // the same height, so the two can't diverge.
    for &PlantItem { kind, pos } in &ctx.layout.plants {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                pos,
                crate::tui::layout::furniture_def(kind.furniture()).visual.h,
            ),
            kind: DrawableKind::Plant { kind, pos },
        });
    }

    // Floor lamp (4×10 centered). z-key = sprite south row = lamp.y + h/2 - 1
    // (10/2 - 1 = 4), the visual base — was +5 (one row past, floated the
    // shadow + let the lamp paint over a character standing just in front).
    if let Some(lamp) = ctx.layout.floor_lamp {
        drawables.push(Drawable {
            anchor_y: lamp.y + floor_lamp_south_offset(),
            kind: DrawableKind::FloorLamp { pos: lamp },
        });
    }

    // Meeting-room coat rack — y-sorted at its base row (cy+7) so a character
    // in front occludes it. Same geometry the background pass used to draw.
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

    // Elevator door (16×14, top-left anchored). Frame is computed
    // stateless from agents in their entry/exit window: door opens
    // (0→1→2) over the first DOOR_TRANSITION_MS of the agent's
    // transit, holds open (2) in the middle, then closes (2→1→0)
    // over the final DOOR_TRANSITION_MS. With multiple agents in
    // flight we take the MAX frame so the door is at least as open
    // as the most-in-progress agent needs.
    if let Some(door_pos) = ctx.layout.door {
        let frame_idx = compute_door_frame_idx(&agents, ctx.now, ctx.door_anim_max_ms);
        drawables.push(Drawable {
            anchor_y: door_pos.y + ELEVATOR_H,
            kind: DrawableKind::Door {
                pos: door_pos,
                frame_idx,
            },
        });
    }

    // Wall decor — hung on walls, TOP-LEFT anchored at `pos`, so its y-sort
    // row is its south base (`pos.y + h - 1`), same helper the mask + every
    // other drawable use. (Was a hand-rolled `pos.y + h`, one row past the
    // sprite's actual bottom.)
    for &WallDecorItem { kind, pos } in &ctx.layout.wall_decor {
        let Size { h, .. } = crate::tui::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::TopLeft, pos, h),
            kind: DrawableKind::WallDecor { kind, pos },
        });
    }

    let idle_desk_indices: Vec<usize> = agents
        .iter()
        .filter(|a| {
            matches!(a.state, ActivityState::Idle)
                && a.desk_index < ctx.layout.home_desks.len()
                && a.exiting_at.is_none()
        })
        .map(|a| a.desk_index)
        .collect();
    let all_idle = agents
        .iter()
        .all(|a| matches!(a.state, ActivityState::Idle));

    if let Some(kind) = ctx.floor_pet.map(|p| p.kind) {
        let active_pet = ctx.active_pet.filter(|p| {
            p.is_active(ctx.now) && p.kind == kind && p.floor_idx == ctx.floor.floor_idx
        });
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
        if let Some((pos, flip, anim_name, frame_idx, pet_elapsed)) = pet_data {
            resolved_pet_pos = Some(PetFrame {
                pos,
                anim: anim_name,
                kind,
            });
            // South row derived from the CHOSEN anim's sprite height, not a
            // literal: the h=4 sleep sprite sorts at pos.y+1, the h=6 walk/sit
            // sprites at pos.y+2. A hardcoded +2 rendered a sleeping pet OVER a
            // character whose feet land on pos.y+1 (one row too far south).
            let pet_h = ctx
                .pack
                .animation(anim_name)
                .and_then(|a| a.frames.first())
                .map_or(6, |f| f.height);
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
        }
    }

    // Characters. Anchor = feet (anchor.y + sprite_height). Decollision
    // rank for crowded waypoints — stable across frames thanks to
    // BTreeMap iteration order.
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    let mut waypoint_visitors: Vec<chitchat::Visitor> = Vec::new();
    // All 3 lounge-couch seat waypoints collapse to ONE chitchat venue (keyed
    // on the first couch's index) so the couch hosts a single group
    // conversation like the meeting room — without overloading the
    // meeting-only `room_id` field (which indexes `meeting_tables`).
    let couch_group_idx = ctx
        .layout
        .waypoints
        .iter()
        .position(|w| w.kind == crate::tui::layout::WaypointKind::Couch);
    // The pack's character sprite width (8 for the bundled pack, 10 for the
    // robot pack). All character poses share one width, so resolve it ONCE from
    // a reference pose and center every anchor on it — a non-8-wide pack would
    // otherwise blit ~1px off (the anchors hardcoded 8). Fallback to the bundled
    // default if the pack lacks the reference anim.
    let char_w = ctx
        .pack
        .animation("standing")
        .and_then(|a| a.frames.first())
        .map_or(CHARACTER_SPRITE_W, |f| f.width);
    for agent in &agents {
        let Some(desk) = ctx.layout.home_desks.get(agent.desk_index).copied() else {
            continue;
        };
        let Some(p) = pose::derive_with_routing(
            agent,
            ctx.now,
            ctx.layout,
            &mut crate::tui::pose::RouteCtx {
                router: &mut *ctx.router,
                overlay: &*ctx.overlay,
                history: &mut *ctx.history,
                motion: &mut *ctx.motion,
            },
        ) else {
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
                        thinking_dots: false,
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
                        thinking_dots: true,
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
                        thinking_dots: false,
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
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = ctx.layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    use crate::tui::layout::WaypointKind;
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
                            thinking_dots: false,
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
                        thinking_dots: false,
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
                // render-side state), but we know from coffee_holders.
                if agent.exiting_at.is_some() && ctx.coffee_holders.contains(&agent.agent_id) {
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
                        thinking_dots: false,
                        walking_dust_frame: Some(frame),
                    },
                });
            }
        }
    }

    // Horizontal (E-W) room dividers join the y-sort, anchored at their south
    // (front) edge, so a character standing behind (north of) the wall is
    // composited over by the frosted glass instead of painting on top of it.
    // The vertical (edge-on) dividers already painted in the background pass.
    for &WallSegment { start, end } in &ctx.layout.room_walls {
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
        chitchat_bubbles,
        new_coffee_carriers,
    }
}

#[cfg(test)]
mod tests;
