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
use pixtuoid_core::state::{DaemonState, FloorLocalDeskIndex};
use pixtuoid_core::{AgentSlot, SceneState};

use crate::chitchat::{ActiveChitchat, ChitchatBubble};
#[cfg(test)]
use crate::floor::LightingState;
use crate::frame_cache::FrameCache;
use crate::layout::{
    z_sort_row, Anchor, Layout, PlantItem, PodDecorItem, Point, Size, WallDecorItem, ELEVATOR_H,
};
use crate::motion::MotionState;
use crate::pet::PetFrame;

pub(super) use crate::anim::epoch_ms;

/// Everything the pure-pixel pass observed that the caller still needs.
pub struct PixelPassResult {
    /// The office pet's resolved frame this tick (for hit-testing), if present.
    pub pet_pos: Option<PetFrame>,
    /// One resolved frame per gateway mascot drawn this tick — a source can
    /// run ANY number of concurrent instances, each independently hoverable.
    pub mascots: Vec<MascotFrame>,
    /// Every character drawn this tick, in paint order: the last one covering
    /// a point is the one on top.
    pub agents: Vec<AgentFrame>,
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
    /// The mascot's centre screen position this tick.
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

/// Where a character's sprite was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentFrame {
    /// Whose sprite it is.
    pub agent_id: pixtuoid_core::AgentId,
    /// The sprite's top-left screen position.
    pub anchor: Point,
}

mod ambient;
mod anchors;
mod background;
mod debug_overlay;
mod dense;
mod drawable;
mod effects;
mod furniture;
pub(crate) mod hair;
mod palette;
pub(crate) mod seat;
mod sim;
mod wall;

pub use anchors::character_anchor;

#[doc(hidden)]
pub use anchors::seated_anchor_facing;
#[cfg(test)]
pub(crate) use drawable::DESK_BEZEL_RAISE;
pub(crate) use drawable::{
    desk_art_top, desk_sprite_name, DESK_CHAIR_SPRITE, MEETING_TABLE_SPRITE,
};
pub(crate) use palette::{appliance_overrides, DESK_BULB_KEY, SCREEN_GLASS_KEY, SCREEN_TEXT_KEY};

// The ToolKind→glow-hue seam the binary's footer tints tool segments with. The
// footer paints this hue RAW; the sprite's glow then takes the hour's wash, so
// the two match in HUE, not byte-for-byte — and only on a NORTH-facing desk,
// the only one whose screen the room can see.
pub use palette::tool_glow_for_kind;

/// Applies the hour's object terms to every pixel painted since `since`.
///
/// The branch-free XOR-OR reduction replaces a hard-to-predict per-pixel
/// branch with one predictable branch per `WASH_SCAN_CHUNK` on the clean
/// path, and the wash itself runs through a lazily-built [`palette::RgbLut`]
/// over the chunks that differ. Byte-identical to per-pixel [`wash_object`]
/// on the diff set.
fn wash_since(buf: &mut RgbBuffer, since: &RgbBuffer, wash: [(Rgb, f32); 2]) {
    const WASH_SCAN_CHUNK: usize = 64;
    let cur = buf.as_mut_slice();
    let old = since.as_slice();
    debug_assert_eq!(cur.len(), old.len());
    let mut lut: Option<palette::RgbLut> = None;
    for (cur_c, old_c) in cur
        .chunks_mut(WASH_SCAN_CHUNK)
        .zip(old.chunks(WASH_SCAN_CHUNK))
    {
        let differs = cur_c.iter().zip(old_c).fold(0u8, |acc, (a, b)| {
            acc | (a.r ^ b.r) | (a.g ^ b.g) | (a.b ^ b.b)
        });
        if differs != 0 {
            let lut =
                lut.get_or_insert_with(|| palette::RgbLut::tabulate(|c| wash_object(c, wash)));
            for (p, o) in cur_c.iter_mut().zip(old_c) {
                if *p != *o {
                    *p = lut.apply(*p);
                }
            }
        }
    }
}

/// Composes the hour's two object terms in the floor overlays' own order.
fn wash_object(painted: Rgb, wash: [(Rgb, f32); 2]) -> Rgb {
    wash.into_iter().fold(painted, |c, (tint, a)| {
        if a > 0.0 {
            palette::blend_rgb(c, tint, a)
        } else {
            c
        }
    })
}
pub(crate) use background::BaseFillCache;
pub(crate) use dense::{densest_frame, DenseFrame};
#[cfg(test)]
pub(crate) use furniture::{paint_area_rug, COOLER_WATER};
// `floor::FloorSession::observe` is the public entry to the sim tick; the step
// itself and its per-call borrow-set stay crate-internal.
pub(crate) use sim::{desk_occupant, sim_step, PetInputs, SimInputs, SimStores};
pub use sim::{CharacterGlow, CharacterPlacement, SimFrame};
pub(crate) use wall::paint_wall;

/// The pantry counter sprites, compact then large.
pub(crate) const PANTRY_COUNTER_ANIMS: [&str; 2] = ["pantry_small", "pantry"];

/// The pantry counter sprite for a counter `counter_w` px wide: the large
/// kitchen run when the room fits it, else the compact one.
pub(crate) fn pantry_counter_anim(counter_w: u16) -> &'static str {
    let [compact, large] = PANTRY_COUNTER_ANIMS;
    if counter_w >= crate::layout::PANTRY_COUNTER_LARGE_W {
        large
    } else {
        compact
    }
}

use crate::atmosphere::Moment;
use crate::ground::Ellipse;
use crate::lighting::{DeskLights, LightInputs, Lights};
use anchors::compute_door_frame_idx;
use background::{
    paint_clock, paint_corridor_runner, paint_floor_and_walls, paint_floor_wash, paint_light,
    paint_neon_panel, paint_shadow,
};
use drawable::{paint_drawable, Drawable, DrawableKind};
pub(crate) use effects::SCREEN_GLASS_COLS;
use palette::{agent_overrides, outfit_seed_for};
use seat::paint_character_at;
use wall::enqueue_room_walls;

/// The weather names accepted by [`force_weather`], canonical order.
pub fn weather_names() -> Vec<&'static str> {
    crate::sky::Weather::ALL.iter().map(|w| w.name()).collect()
}

/// Force every subsequent render **on this thread** to a specific weather (by
/// name, case-insensitive), or `None` to restore the clock-based selection.
/// It's a thread-local shared by every `Office` in the one wasm module, so the
/// last writer before a render wins and each surface must set its own value
/// every frame. `Err` carries the valid names when `name` is unknown.
pub fn force_weather(name: Option<&str>) -> Result<(), Vec<&'static str>> {
    match name {
        None => {
            crate::sky::set_weather_override(None);
            Ok(())
        }
        Some(s) => match crate::sky::Weather::from_name(s) {
            Some(w) => {
                crate::sky::set_weather_override(Some(w));
                Ok(())
            }
            None => Err(weather_names()),
        },
    }
}

/// How hard it is raining at `now` (0.0 dry … 1.0 storm; snow and fog are 0.0) —
/// the audio model's weather feed.
pub fn precipitation_level(now: std::time::SystemTime) -> f32 {
    crate::sky::Sky::at(now).precipitation()
}

/// Whether the office's sky shows the SUN at hour-of-day `hour` (0..24).
/// Exposed so the wasm painter's `Office::is_day` can hand the site's
/// sky-slider the SAME day/night boundary the office renders.
pub fn hour_is_day(hour: f32) -> bool {
    crate::sky::hour_is_day(hour)
}

/// Day/night at `now` on the LOCAL clock — the native painters' feed for the
/// audio track selector (wasm passes its own hour). Same sun window the
/// lighting renders: the music follows what the office SHOWS.
pub fn is_day_at(now: std::time::SystemTime) -> bool {
    crate::sky::hour_is_day(crate::sky::local_hour_frac(now))
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
/// `&mut`s are the pixel buffer and the paint-local caches (`FrameCache`,
/// `BaseFillCache`); the sim stores are absent BY TYPE (`motion` is an
/// immutable view, read by the debug route overlay), so painting cannot move
/// the world.
struct PaintCtx<'a> {
    scene: &'a SceneState,
    layout: &'a Layout,
    pack: &'a Pack,
    /// Animation phase, event ages and the wall clock — every sky fact reads
    /// [`Self::sky`].
    now: SystemTime,
    /// The sky at `now`, sampled once for the whole pass.
    sky: crate::sky::Sky,
    buf: &'a mut RgbBuffer,
    cache: &'a mut FrameCache,
    base_fill: &'a mut background::BaseFillCache,
    theme: &'a crate::theme::Theme,
    floor: crate::floor::FloorMeta,
    motion: &'a HashMap<pixtuoid_core::AgentId, MotionState>,
    door_anim_max_ms: u64,
    debug_walkable: bool,
}

/// Render `ctx`'s scene into its buffer — the shared world render; the paint
/// half borrows only `PaintCtx`.
pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let frame = sim_step(
        &mut SimStores {
            router: &mut ctx.store.router,
            overlay: &mut ctx.store.overlay,
            history: &mut ctx.store.history,
            motion: &mut ctx.store.motion,
            light: &mut ctx.store.light,
            neon: &mut ctx.store.neon,
            chitchat: &mut *ctx.chitchat_state,
        },
        SimInputs {
            scene: ctx.scene,
            layout: ctx.layout,
            pack: ctx.pack,
            coffee: ctx.coffee,
            pets: PetInputs {
                pet: ctx.floor_pet,
                petting: ctx.active_pet,
            },
            floor: ctx.floor,
            now: ctx.now,
        },
    );
    let (pet_pos, mascots, agents) = paint_frame(
        &mut PaintCtx {
            scene: ctx.scene,
            layout: ctx.layout,
            pack: ctx.pack,
            now: ctx.now,
            sky: crate::sky::Sky::at(ctx.now),
            buf: &mut *ctx.buf,
            cache: &mut ctx.store.cache,
            base_fill: &mut ctx.store.base_fill,
            theme: ctx.theme,
            floor: ctx.floor,
            motion: &ctx.store.motion,
            door_anim_max_ms: ctx.store.door_anim_max_ms,
            debug_walkable: ctx.debug_walkable,
        },
        &frame,
    );
    PixelPassResult {
        pet_pos,
        mascots,
        agents,
        chitchat_bubbles: frame.chitchat_bubbles,
        new_coffee_carriers: frame.new_coffee_carriers,
        occupied_waypoints: frame.occupied_waypoints,
    }
}

/// The floor shadow under one home desk. `cy` reads the same authority
/// `enqueue_desk_cubicles` keys the sprite on, so a `DESK_H` retune moves the
/// shadow WITH the sprite's south base. `half_h` is a taste literal.
fn desk_shadow_ellipse(desk: Point) -> Ellipse {
    // Every axis off the ONE furniture row, so the shadow cannot drift from the
    // desk it falls under: `DESK_W` is the surface, not the piece — the side
    // cabinets make the painted (and ground-contacting) width `visual.w`.
    let v = crate::layout::desk_furniture_def().visual;
    Ellipse {
        cx: desk.x + v.w / 2,
        cy: desk.y + v.h,
        half_w: v.w / 2 - 1,
        half_h: 3,
    }
}

/// The classic painter's floor shadows, in PAINT ORDER — the overlaps
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
            let half_w = if vis_w > 0 {
                (vis_w / 2 + crate::ground::CONTACT_REACH).min(7)
            } else {
                7
            };
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
            cy: crate::layout::z_sort_row(Anchor::Center, island, vis.h),
            half_w: vis.w / 2 + crate::ground::CONTACT_REACH,
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
            cy: crate::layout::z_sort_row(
                Anchor::Center,
                pos,
                furniture_def(kind.furniture()).visual.h,
            ),
            half_w: 3,
            half_h: 1,
        });
    let lamp = layout.floor_lamp_base().map(|base| Ellipse {
        cx: base.x,
        cy: base.y,
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
fn paint_frame(
    ctx: &mut PaintCtx<'_>,
    frame: &SimFrame,
) -> (Option<PetFrame>, Vec<MascotFrame>, Vec<AgentFrame>) {
    let agents: &[AgentSlot] = &frame.agents;
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;

    let moment = Moment::resolve(ctx.sky, ctx.theme, ctx.floor.altitude, ctx.now);
    let look = &moment.look;
    let lights = Lights::of(
        ctx.layout,
        look,
        &LightInputs {
            agents,
            seated: &frame.seated_agents,
            floor_idx: ctx.floor.floor_idx,
            indoor_scale: frame.indoor_scale,
            neon: frame.neon,
            now: ctx.now,
        },
    );
    let top_wall_h = ctx.layout.wall_band_h();
    assert_eq!(
        (ctx.buf.width(), ctx.buf.height()),
        (buf_w, buf_h),
        "the classic pass draws layout units 1:1"
    );
    paint_floor_and_walls(
        ctx.base_fill,
        ctx.buf,
        top_wall_h,
        ctx.layout.window_bays(),
        &moment,
        ctx.pack,
        ctx.theme,
    );
    for spill in &lights.spills {
        paint_light(ctx.buf, spill, ctx.theme.lighting.sun_spill);
    }

    // An empty floor reads dark because its artificial lights go out with
    // `indoor_scale`, not because the FLOOR takes a second darkening of its own.
    paint_floor_wash(ctx.buf, top_wall_h, buf_h, look.floor_wash);
    if let Some(lamp) = &lights.floor_lamp {
        paint_light(ctx.buf, lamp, ctx.theme.lighting.floor_lamp_halo);
    }

    let neon = background::neon_look(frame.neon, ctx.theme);
    paint_neon_panel(
        ctx.buf,
        crate::layout::NEON_PANEL.x,
        crate::layout::NEON_PANEL.y,
        crate::layout::NEON_PANEL.width,
        crate::layout::NEON_PANEL.height,
        &neon,
    );

    // After the wall (so its hands sit on top) but before wall decor (the
    // bookshelf shouldn't cover it).
    let clock = ctx.layout.clock_pos();
    paint_clock(ctx.buf, clock.x, clock.y, ctx.now, ctx.theme);
    // These overwrite the floor, so the overlays above cannot reach them, and
    // they paint before the drawable snapshot, so that pass cannot either — the
    // corridor runner used to stay full-daylight tan in a dimmed office, the
    // brightest thing in the room. Hence their own group, which also keeps the
    // EMITTERS painted above (the floor-lamp halo) and the self-lit
    // wall fixtures (neon panel, clock) out of it.
    let pre_floor_fixtures = ctx.buf.clone();
    if let Some(corridor) = ctx.layout.corridor {
        paint_corridor_runner(ctx.buf, corridor, ctx.theme);
    }
    // Nothing room-scale (walls, sofas, the kitchen island) belongs in this
    // background pass — it would double-paint under its y-sorted copy below.
    // Only these small mask-free items do.
    for room in &ctx.layout.meeting_rooms {
        if let Some(board) = room.notice_board_rect() {
            furniture::paint_notice_board(ctx.buf, board, ctx.theme);
        }
        furniture::paint_doormat(ctx.buf, room, ctx.theme);
    }
    // Floor-level mats paint FIRST so they sit under every upright pantry
    // fixture: on a narrow pantry the entry mat's box reaches the water-cooler
    // column, and mats-after-cooler would clip the cooler's west edge.
    for mat in [ctx.layout.pantry_entry_mat(), ctx.layout.island_bar_mat()]
        .into_iter()
        .flatten()
    {
        furniture::paint_area_rug(ctx.buf, mat, ctx.theme);
    }
    if let Some(pantry) = &ctx.layout.pantry {
        furniture::paint_water_cooler(ctx.buf, pantry, ctx.now, ctx.theme);
        furniture::paint_trash_bin(ctx.buf, pantry);
    }
    wash_since(ctx.buf, &pre_floor_fixtures, look.object_wash);

    let shadow_strength = crate::ground::shadow_strength(look.darkness);
    for ell in floor_shadow_ellipses(ctx.layout) {
        paint_shadow(ctx.buf, ell, shadow_strength, ctx.theme);
    }

    ambient::paint_ambient(ctx, look, &lights.monitor_halos);

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

    enqueue_desk_cubicles(ctx, frame, &lights.desks, &mut drawables);

    enqueue_meeting_furniture(ctx.layout, &mut drawables);

    enqueue_lounge_pantry_appliances(ctx.layout, &frame.occupied_waypoints, &mut drawables);

    enqueue_pod_decor_and_plants(ctx.layout, &mut drawables);
    enqueue_floor_fixtures(ctx, agents, &mut drawables);
    enqueue_wall_decor(ctx.layout, &mut drawables);

    let resolved_pet_pos = frame.pet.map(|pet| enqueue_pet(ctx, pet, &mut drawables));
    let resolved_mascots = enqueue_gateway_mascots(ctx, &frame.mascots, &mut drawables);

    enqueue_characters(ctx, frame, &mut drawables);
    enqueue_desk_chairs(ctx.layout, &mut drawables);

    enqueue_room_walls(ctx.layout, &mut drawables);

    // `sort_by_key` is stable, so ties preserve the insertion order above —
    // decor first, characters last — and a character tied with a piece of
    // furniture paints BEFORE it.
    drawables.sort_by_key(|d| d.anchor_y);
    let drawn_agents = drawables
        .iter()
        .filter_map(|d| match d.kind {
            DrawableKind::Character { agent, anchor, .. } => Some(AgentFrame {
                agent_id: agent.agent_id,
                anchor,
            }),
            _ => None,
        })
        .collect();
    // A per-pixel diff finds EXACTLY what the foreground wrote; a rectangular
    // band seamed the window glass and washed floor-between-pieces twice.
    // AFTER `paint_shadow`/`paint_ambient`: both already take `look`, so folding
    // them in here would apply the hour twice.
    let pre_foreground = ctx.buf.clone();
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
    // The floor's day/night wash, over the foreground: the overlays above run
    // before any drawable exists, so nothing painted carries a time-of-day term.
    wash_since(ctx.buf, &pre_foreground, look.object_wash);

    // LATE, so the light lands ON the shelf and the clock instead of hiding
    // behind them; after the wash, since the sign is an emitter and its light
    // isn't dimmed with the room it falls on.
    paint_light(ctx.buf, &lights.neon, neon.halo);

    // LAST, so a Storm strike briefly flares the whole interior (floor, walls,
    // furniture, characters), not just the window strip.
    background::paint_lightning_flash(ctx.buf, &ctx.sky);

    if ctx.debug_walkable {
        debug_overlay::paint(ctx.buf, ctx.layout, ctx.scene, ctx.motion);
    }

    (resolved_pet_pos, resolved_mascots, drawn_agents)
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
        drawables.push(Drawable {
            anchor_y: p.anchor_y,
            kind: DrawableKind::Character {
                agent,
                pose: seat::SpritePose::of(p, agent, ctx.theme),
                anchor: p.anchor,
                sleep_z_seed: p.sleep_z_seed,
                waiting_bubble: p.waiting_bubble,
                walking_dust_frame: p.walking_dust_frame,
            },
        });
    }
}

/// The frame to paint for `idx`, via [`frame_index`]. `None` only for a
/// genuinely empty animation.
pub(super) fn frame_at(anim: &Sprite, idx: usize) -> Option<&Frame> {
    anim.frames().get(frame_index(anim, idx))
}

/// `idx`, or `0` once it runs past the animation: a custom pack's animation
/// with fewer frames than the shared cycle's `frame_idx` would
/// otherwise vanish the sprite.
pub(super) fn frame_index(anim: &Sprite, idx: usize) -> usize {
    if idx < anim.frames().len() {
        idx
    } else {
        0
    }
}

/// The pack art a corridor appliance at a `kind` waypoint is drawn from.
pub(crate) fn appliance_art(kind: crate::layout::WaypointKind) -> Option<&'static str> {
    use crate::layout::WaypointKind as K;
    match kind {
        K::VendingMachine => Some("vending_machine"),
        K::Printer => Some("printer"),
        K::Couch
        | K::Pantry
        | K::PhoneBooth
        | K::StandingDesk
        | K::MeetingSofa
        | K::MeetingChair
        | K::Island
        | K::SnackShelf => None,
    }
}

/// The frame of an appliance's `anim` showing at `now`: frame 0 at rest, else
/// its busy loop — the frames after 0, one each of the art's own `frame_ms`.
pub(crate) fn appliance_frame(anim: &Sprite, busy: bool, now: std::time::SystemTime) -> usize {
    let loop_len = anim.frames().len().saturating_sub(1);
    if !busy || loop_len == 0 {
        return 0;
    }
    let step = crate::anim::epoch_ms(now) / u64::from(anim.frame_ms().max(1));
    1 + usize::try_from(step % loop_len as u64).unwrap_or(0)
}

/// One chair per NORTH-facing home desk, occupied or not. Keyed to TIE with its
/// occupant, so the stable sort paints it over them.
fn enqueue_desk_chairs<'a>(layout: &Layout, drawables: &mut Vec<Drawable<'a>>) {
    for (i, &desk) in layout.home_desks.iter().enumerate() {
        let facing = layout.desk_facing(FloorLocalDeskIndex(i));
        let Some(pos) = crate::layout::desk_chair_top_left(desk, facing) else {
            continue;
        };
        drawables.push(Drawable {
            anchor_y: crate::layout::desk_chair_z_key(desk, facing),
            kind: DrawableKind::DeskChair { pos },
        });
    }
}

/// The glow of a desk's screen: its occupant's [`lit_screen`](crate::lighting::lit_screen),
/// tinted by the tool. Both profiles light screens from this.
pub(crate) fn desk_screen_glow(
    occupant: Option<&AgentSlot>,
    facing: crate::layout::Facing,
    seated: bool,
    theme: &crate::theme::Theme,
) -> Option<pixtuoid_core::sprite::Rgb> {
    occupant
        .and_then(|a| crate::lighting::lit_screen(a, facing, seated))
        .map(|tool| palette::tool_glow_for_kind(tool, &theme.tool_glow))
}

/// Desk cubicles — each one z-unit: the desk, its lamp, screens and props, and a
/// filing cabinet where [`desk_has_cabinet`](crate::layout::SceneLayout::desk_has_cabinet)
/// stands one. The desk sorts one row past its visual south row, just past the
/// seated worker's feet, so the sitter stays visually behind it. Z is a VISUAL
/// property: it tracks the sprite, not the blocked ground.
fn enqueue_desk_cubicles<'a>(
    ctx: &PaintCtx<'_>,
    frame: &SimFrame,
    lights: &[DeskLights],
    drawables: &mut Vec<Drawable<'a>>,
) {
    debug_assert_eq!(
        lights.len(),
        ctx.layout.home_desks.len(),
        "desk lights are index-parallel to the home desks"
    );
    for ((i, &desk), light) in ctx.layout.home_desks.iter().enumerate().zip(lights) {
        let local = FloorLocalDeskIndex(i);
        let desk_def = crate::layout::desk_furniture_def();
        let occupant = desk_occupant(&frame.agents, local);
        let facing = ctx.layout.desk_facing(local);
        let screen_glow = desk_screen_glow(
            occupant,
            facing,
            frame.seated_agents.get(&local).copied().unwrap_or(false),
            ctx.theme,
        );
        drawables.push(Drawable {
            anchor_y: desk.y + desk_def.visual.h,
            kind: DrawableKind::DeskCubicle {
                desk,
                facing,
                cabinet: ctx.layout.filing_cabinet_top_left(local),
                screen_glow,
                lights: *light,
                props: frame.desk(local),
            },
        });
    }
}

/// The office pet, y-sorted at its anim's south row, since the anims differ in
/// height — a hardcoded offset once painted a sleeping pet over a character in
/// front.
fn enqueue_pet<'a>(
    ctx: &PaintCtx<'_>,
    pet: sim::PetPlacement,
    drawables: &mut Vec<Drawable<'a>>,
) -> PetFrame {
    /// Fallback when a custom pack lacks the resolved pet anim: the bundled
    /// cat's size, so the z-sort row and the canvas clamp stay sane — the blit
    /// itself no-ops, `paint_drawable` bails.
    const PET_FALLBACK: Size = Size { w: 8, h: 6 };
    let (pet_w, pet_h) = ctx
        .pack
        .animation(pet.anim_name)
        .and_then(|a| a.frames().first())
        .map_or((PET_FALLBACK.w, PET_FALLBACK.h), |f| {
            (f.width(), f.height())
        });
    let pos = anchors::keep_sprite_on_canvas(
        Anchor::Center,
        pet.pos,
        Size { w: pet_w, h: pet_h },
        Size {
            w: ctx.layout.buf_w,
            h: ctx.layout.buf_h,
        },
    );
    drawables.push(Drawable {
        anchor_y: z_sort_row(Anchor::Center, pos, pet_h),
        kind: DrawableKind::Pet {
            kind: pet.kind,
            pos,
            flip: pet.flip,
            anim_name: pet.anim_name,
            frame_idx: pet.frame_idx,
            pet_elapsed_ms: pet.petted_ms,
        },
    });
    PetFrame {
        pos,
        anim: pet.anim_name,
        kind: pet.kind,
    }
}

/// Enqueue the gateway mascots, each fitted to the canvas.
fn enqueue_gateway_mascots<'a>(
    ctx: &PaintCtx<'_>,
    mascots: &[sim::MascotPlacement],
    drawables: &mut Vec<Drawable<'a>>,
) -> Vec<MascotFrame> {
    mascots
        .iter()
        .map(|m| {
            /// Fallback when a custom pack lacks the mascot anim: the bundled
            /// lobster's size, so the z-sort row and the canvas clamp stay sane —
            /// the blit itself no-ops.
            const MASCOT_FALLBACK: Size = Size { w: 14, h: 12 };
            let (mascot_w, mascot_h) = ctx
                .pack
                .animation(m.anim_name)
                .and_then(|a| a.frames().first())
                .map_or((MASCOT_FALLBACK.w, MASCOT_FALLBACK.h), |f| {
                    (f.width(), f.height())
                });
            let pos = anchors::keep_sprite_on_canvas(
                Anchor::Center,
                m.pos,
                Size {
                    w: mascot_w,
                    h: mascot_h,
                },
                Size {
                    w: ctx.layout.buf_w,
                    h: ctx.layout.buf_h,
                },
            );
            drawables.push(Drawable {
                anchor_y: z_sort_row(Anchor::Center, pos, mascot_h),
                kind: DrawableKind::GatewayMascot {
                    pos,
                    anim_name: m.anim_name,
                    frame_idx: m.frame_idx,
                    run_count: m.run_count,
                    degraded: m.state == DaemonState::Degraded,
                },
            });
            MascotFrame {
                pos,
                w: mascot_w,
                h: mascot_h,
                name: m.name,
                instance: m.instance.clone(),
                busy: m.state == DaemonState::Busy,
                degraded: m.state == DaemonState::Degraded,
                active_sessions: m.active_sessions,
            }
        })
        .collect()
}

/// Meeting-room rugs + sofas + tables. A south-of-table sofa faces away, so it
/// y-sorts +3 to occlude its sitter; the north sofa stays +2 so insertion order
/// breaks the tie in its sitter's favor.
fn enqueue_meeting_furniture<'a>(layout: &'a Layout, drawables: &mut Vec<Drawable<'a>>) {
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        let rug = trio.rug(layout.buf_h);
        drawables.push(Drawable {
            anchor_y: rug.y,
            kind: DrawableKind::AreaRug(rug),
        });
    }
    for trio in layout.meeting_rooms.iter().filter_map(|r| r.trio.as_ref()) {
        for (i, sofa) in trio.sofas.into_iter().enumerate() {
            // sofas[0] is the north sofa, sofas[1] the south.
            let mirrored = i % 2 != 0;
            let faces_away = sofa.y >= trio.table.y;
            drawables.push(Drawable {
                anchor_y: seat::sofa_sitter_z_key(sofa) + u16::from(faces_away),
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
    if let Some(lounge) = layout.lounge {
        let center = lounge.couch_center;
        drawables.push(Drawable {
            anchor_y: center.y.saturating_sub(2),
            kind: DrawableKind::AreaRug(lounge.rug()),
        });
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                center,
                crate::layout::furniture_def(crate::layout::Furniture::MeetingSofaBody)
                    .visual
                    .h,
            ),
            // The lounge couch IS the meeting sofa's sprite, mirrored.
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
        // The VISUAL height, not the (shallow) footprint, so an overhang still
        // sorts by what's painted.
        let visual_h = furniture_def(wp.kind.furniture()).visual.h;
        if let Some(sprite) = appliance_art(wp.kind) {
            drawables.push(Drawable {
                anchor_y: z_sort_row(Anchor::Center, wp.pos, visual_h),
                kind: DrawableKind::Appliance {
                    pos: wp.pos,
                    sprite,
                    busy: occupied_waypoints.contains(&wp_idx),
                },
            });
            continue;
        }
        match wp.kind {
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let Size { w: cw, h: ch } = layout.pantry_counter_size();
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, ch),
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        anim: pantry_counter_anim(cw),
                    },
                });
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            // Their art is `appliance_art`'s, pushed above.
            WaypointKind::VendingMachine | WaypointKind::Printer => {}
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

/// Free-standing fixtures, and the elevator door at the frame
/// `compute_door_frame_idx` picks.
fn enqueue_floor_fixtures<'a>(
    ctx: &PaintCtx<'_>,
    agents: &[AgentSlot],
    drawables: &mut Vec<Drawable<'a>>,
) {
    if let Some(lamp) = ctx.layout.floor_lamp() {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                lamp,
                crate::layout::furniture_def(crate::layout::Furniture::FloorLamp)
                    .visual
                    .h,
            ),
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
            anchor_y: seat::Seat::at_waypoint(wp.kind, wp.pos, wp.facing).z_key() - 1,
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
            anchor_y: z_sort_row(Anchor::Center, tank, h),
            kind: DrawableKind::FishTank { pos: tank },
        });
    }
    // Placement + the narrow-fitted-room yield live in `coat_rack_pos`, the
    // drawn box in `coat_rack_rect_at` — the authorities the hover hit-test shares.
    for rack in ctx
        .layout
        .meeting_rooms
        .iter()
        .filter_map(|r| r.coat_rack_pos())
    {
        let rect = crate::layout::coat_rack_rect_at(rack);
        drawables.push(Drawable {
            anchor_y: rect.y + rect.height - 1,
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
pub(crate) mod tests;
