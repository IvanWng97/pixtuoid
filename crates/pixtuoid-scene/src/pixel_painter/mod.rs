//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! [`render_to_rgb_buffer`] is the world-render seam every painter rides, and
//! is itself TWO phases: `sim_step` advances the world with no pixel access
//! into an immutable [`SimFrame`], then `paint_frame` consumes it. The whole
//! public surface is on the published crate's api golden, so widen it
//! deliberately.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::DaemonState;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::chitchat::{ActiveChitchat, ChitchatBubble};
#[cfg(test)]
use crate::floor::VacancyDim;
use crate::frame_cache::FrameCache;
use crate::layout::{Anchor, Depth, Facing, FixtureKind, Point, SceneLayout, Station, z_sort_row};
use crate::motion::MotionState;
use crate::pet::PetFrame;

pub(super) use crate::anim::epoch_ms;

/// Everything the pure-pixel pass observed that the caller still needs.
pub struct PixelPassResult {
    /// The office pet's resolved frame this tick (for hit-testing), if present.
    pub pet_pos: Option<PetFrame>,
    /// Every gateway mascot drawn this tick, in paint order — a source can run
    /// ANY number of concurrent instances, each independently hoverable.
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
    /// The sprite's top-left, in buffer pixels.
    pub anchor: Point,
    /// The painted frame's pixel width.
    pub w: u16,
    /// The painted frame's pixel height.
    pub h: u16,
    /// Its placement's [`CharacterPlacement::label_anchor`](crate::sim::CharacterPlacement::label_anchor).
    pub label_anchor: Point,
}

/// What [`paint_frame`] drew that hover can name.
struct Hoverables {
    pet_pos: Option<PetFrame>,
    mascots: Vec<MascotFrame>,
    agents: Vec<AgentFrame>,
}

mod ambient;
mod background;
mod debug_overlay;
pub(crate) mod drawable;
pub(crate) mod effects;
mod furniture;
pub(crate) mod palette;

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
use crate::sim::{SimFrame, SimInputs, desk_occupant, sim_step};
pub(crate) use background::BaseFillCache;
#[cfg(test)]
pub(crate) use furniture::paint_area_rug;

use crate::atmosphere::Moment;
use crate::lighting::{DeskLights, LightInputs, Lights};
use background::{
    paint_ground_and_walls, paint_ground_wash, paint_light, paint_neon_halo, paint_shadows,
};
use drawable::{Drawable, DrawableKind, Drawn, Layer, enqueue_room_walls, paint_drawable};

pub use crate::sky::{Weather, WeatherPolicy};

/// The weather names [`WeatherPolicy::from_name`] accepts, canonical order.
pub fn weather_names() -> Vec<&'static str> {
    crate::sky::Weather::ALL.iter().map(|w| w.name()).collect()
}

/// How hard it is raining at `now` under `weather` (0.0 dry … 1.0 storm; snow
/// and fog are 0.0) — the audio model's weather feed.
pub fn precipitation_level(now: std::time::SystemTime, weather: WeatherPolicy) -> f32 {
    crate::sky::Sky::at(now, weather).precipitation()
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
    /// The floor this pass renders.
    pub world: crate::floor::FloorInputs<'a>,
    /// The computed office geometry for this frame.
    pub layout: &'a SceneLayout,
    /// The active color theme.
    pub theme: &'a crate::theme::Theme,
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
    layout: &'a SceneLayout,
    pack: &'a Pack,
    /// Animation phase, event ages and the wall clock — every sky fact reads
    /// [`Self::sky`].
    now: SystemTime,
    /// The sky at `now`, sampled once for the whole pass.
    sky: crate::sky::Sky,
    buf: &'a mut RgbBuffer,
    cache: &'a mut FrameCache,
    base_fill: &'a mut background::BaseFillCache,
    shadows: &'a mut crate::ground::DepthsCache,
    theme: &'a crate::theme::Theme,
    floor: crate::floor::FloorMeta,
    motion: &'a HashMap<pixtuoid_core::AgentId, MotionState>,
    debug_walkable: bool,
}

impl PaintCtx<'_> {
    /// The subset of the pass a [`Drawable`] paints with.
    fn drawable_ctx(&mut self) -> drawable::DrawableCtx<'_> {
        drawable::DrawableCtx {
            buf: &mut *self.buf,
            pack: self.pack,
            cache: &mut *self.cache,
            now: self.now,
            theme: self.theme,
        }
    }
}

/// Render `ctx`'s scene into its buffer — the shared world render; the paint
/// half borrows only `PaintCtx`.
pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let door_anim_max_ms = ctx.store.door_anim_max_ms;
    let frame = sim_step(
        &mut ctx.store.sim_stores(ctx.chitchat_state),
        SimInputs {
            world: ctx.world,
            layout: ctx.layout,
            coffee: ctx.coffee,
            door_anim_max_ms,
        },
    );
    let Hoverables {
        pet_pos,
        mascots,
        agents,
    } = paint_frame(
        &mut PaintCtx {
            scene: ctx.world.scene,
            layout: ctx.layout,
            pack: ctx.world.pack,
            now: ctx.world.now,
            sky: crate::sky::Sky::at(ctx.world.now, ctx.world.floor.weather),
            buf: &mut *ctx.buf,
            cache: &mut ctx.store.cache,
            base_fill: &mut ctx.store.base_fill,
            shadows: &mut ctx.store.shadows,
            theme: ctx.theme,
            floor: ctx.world.floor,
            motion: &ctx.store.motion,
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

/// The PAINT half of the frame: blit the world the sim already advanced. Every
/// positional/lifecycle decision was made in `sim_step` — this pass only
/// resolves presentation (theme colors, sprite pixels) and composites.
fn paint_frame(ctx: &mut PaintCtx<'_>, frame: &SimFrame) -> Hoverables {
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
    paint_ground_and_walls(
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

    // An empty ground reads dark because its artificial lights go out with
    // `indoor_scale`, not because the GROUND takes a second darkening of its own.
    paint_ground_wash(ctx.buf, top_wall_h, buf_h, look.ground_wash);
    if let Some(lamp) = &lights.floor_lamp {
        paint_light(ctx.buf, lamp, ctx.theme.lighting.floor_lamp_halo);
    }

    let neon = crate::floor::neon_look(frame.neon, ctx.theme);
    let Furnishings {
        backdrop,
        sorted: mut drawables,
    } = queue_fixtures(ctx, frame, &lights.desks, neon);
    paint_backdrop(ctx, &backdrop, Wash::Spared);
    // The rest overwrite the floor, so the overlays above cannot reach them, and
    // they paint before the drawable snapshot, so that pass cannot either. Hence
    // their own wash, which also keeps the EMITTERS painted above (the floor-lamp
    // halo) out of it.
    let pre_floor_fixtures = ctx.buf.clone();
    paint_backdrop(ctx, &backdrop, Wash::Washed);
    wash_since(ctx.buf, &pre_floor_fixtures, look.object_wash);

    let shadow_strength = crate::ground::shadow_strength(look.darkness);
    let contacts = ctx.layout.fixtures().filter_map(|f| f.contact()).collect();
    paint_shadows(
        ctx.buf,
        ctx.shadows.cells(contacts, 1),
        shadow_strength,
        ctx.theme.office.shadow,
    );

    ambient::paint_ceiling_halos(ctx.buf, ctx.theme, &lights.monitor_halos);

    // Every entity gets an `anchor_y` — its floor-touching row — so sorting
    // ascending and painting in order puts things closer to the camera in
    // front: the painter's algorithm on a top-down 2D scene.
    let pet_pos = frame
        .pet
        .as_ref()
        .map(|pet| enqueue_pet(ctx, pet, &mut drawables));
    enqueue_gateway_mascots(&frame.mascots, &mut drawables);
    enqueue_characters(ctx, frame, &mut drawables);
    enqueue_room_walls(ctx.layout, &mut drawables);
    drawable::sort_drawables(&mut drawables);
    let mut hover = Hoverables {
        pet_pos,
        mascots: Vec::new(),
        agents: Vec::new(),
    };
    // A per-pixel diff finds EXACTLY what the foreground wrote. AFTER
    // `paint_shadows`/`paint_ceiling_halos`: both already carry the hour, so folding
    // them in here would apply it twice.
    let pre_foreground = ctx.buf.clone();
    for d in &drawables {
        match paint_drawable(&d.kind, &mut ctx.drawable_ctx()) {
            Some(Drawn::Agent(agent)) => hover.agents.push(agent),
            Some(Drawn::Mascot { mascot_idx, w, h }) => {
                hover
                    .mascots
                    .push(MascotFrame::of(&frame.mascots[mascot_idx], w, h));
            }
            None => {}
        }
    }
    // The floor's day/night wash, over the foreground: the overlays above run
    // before any drawable exists, so nothing painted carries a time-of-day term.
    wash_since(ctx.buf, &pre_foreground, look.object_wash);

    // LATE, so the light lands ON the shelf and the clock instead of hiding
    // behind them; after the wash, since the sign is an emitter and its light
    // isn't dimmed with the room it falls on.
    paint_neon_halo(ctx.buf, ctx.layout, &lights.neon, neon.halo);

    // LAST, so a Storm strike briefly flares the whole interior (floor, walls,
    // furniture, characters), not just the window strip.
    background::paint_lightning_flash(ctx.buf, &ctx.sky);

    if ctx.debug_walkable {
        debug_overlay::paint(ctx.buf, ctx.layout, ctx.scene, ctx.motion);
    }

    hover
}

/// Map the sim's resolved [`crate::sim::CharacterPlacement`]s 1:1 onto y-sorted
/// drawables. The ONLY paint-side work is presentation — resolving the
/// theme-free [`CharacterGlow`](crate::sim::CharacterGlow) to a `Theme` color.
fn enqueue_characters<'a>(
    ctx: &PaintCtx<'_>,
    frame: &'a SimFrame,
    drawables: &mut Vec<Drawable<'a>>,
) {
    for p in &frame.characters {
        let agent = &frame.agents[p.agent_idx];
        drawables.push(Drawable {
            anchor_y: p.anchor_y,
            layer: Layer::Figure,
            kind: DrawableKind::Character {
                agent,
                pose: crate::character::SpritePose::of(p, agent, ctx.theme),
                anchor: p.anchor,
                label_anchor: p.label_anchor,
                effects: &p.effects,
            },
        });
    }
}

/// The office pet, y-sorted at its anim's south row, since the anims differ in
/// height.
fn enqueue_pet<'a>(
    ctx: &PaintCtx<'_>,
    pet: &'a crate::sim::PetPlacement,
    drawables: &mut Vec<Drawable<'a>>,
) -> PetFrame {
    let pos = pet.pos;
    let pet_h = crate::sim::frame_size(
        ctx.pack,
        pet.anim_name,
        pet.frame_idx,
        crate::sim::PET_FALLBACK,
    )
    .h;
    drawables.push(Drawable {
        anchor_y: z_sort_row(Anchor::Center, pos, pet_h),
        layer: Layer::Figure,
        kind: DrawableKind::Pet {
            pos,
            flip: pet.flip,
            anim_name: pet.anim_name,
            frame_idx: pet.frame_idx,
            effects: &pet.effects,
        },
    });
    PetFrame {
        pos,
        anim: pet.anim_name,
        kind: pet.kind,
    }
}

/// Enqueue the gateway mascots.
fn enqueue_gateway_mascots<'a>(
    mascots: &'a [crate::sim::MascotPlacement],
    drawables: &mut Vec<Drawable<'a>>,
) {
    for (mascot_idx, m) in mascots.iter().enumerate() {
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, m.pos, m.size.h),
            layer: Layer::Figure,
            kind: DrawableKind::GatewayMascot {
                mascot_idx,
                pos: m.pos,
                anim_name: m.anim_name,
                frame_idx: m.frame_idx,
                effects: &m.effects,
                degraded: m.state == DaemonState::Degraded,
            },
        });
    }
}

impl MascotFrame {
    fn of(m: &crate::sim::MascotPlacement, w: u16, h: u16) -> Self {
        Self {
            pos: m.pos,
            w,
            h,
            name: m.name,
            instance: m.instance.clone(),
            busy: m.state == DaemonState::Busy,
            degraded: m.state == DaemonState::Degraded,
            active_sessions: m.active_sessions,
        }
    }
}

/// Whether the hour's object wash reaches a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wash {
    /// It lights itself, so the wash would dim a light source.
    Spared,
    /// It is lit by the room, and dims with it.
    Washed,
}

/// A fixture's [`Wash`]. Only the background pass can spare a fixture: the
/// foreground wash reaches everything the y-sort paints.
fn wash_of(kind: FixtureKind) -> Wash {
    match kind {
        FixtureKind::NeonSign | FixtureKind::Clock => Wash::Spared,
        FixtureKind::Desk(_)
        | FixtureKind::FilingCabinet(_)
        | FixtureKind::DeskChair(_)
        | FixtureKind::Station { .. }
        | FixtureKind::Plant { .. }
        | FixtureKind::Pod { .. }
        | FixtureKind::Wall { .. }
        | FixtureKind::MeetingRug { .. }
        | FixtureKind::MeetingSofa { .. }
        | FixtureKind::MeetingTable { .. }
        | FixtureKind::MeetingChair { .. }
        | FixtureKind::CoatRack { .. }
        | FixtureKind::Doormat { .. }
        | FixtureKind::NoticeBoard { .. }
        | FixtureKind::LoungeRug
        | FixtureKind::LoungeCouch
        | FixtureKind::SideTable
        | FixtureKind::FloorLamp
        | FixtureKind::FishTank
        | FixtureKind::KitchenIsland
        | FixtureKind::PantryMat
        | FixtureKind::IslandMat
        | FixtureKind::WaterCooler
        | FixtureKind::TrashBin
        | FixtureKind::Door
        | FixtureKind::Runner => Wash::Washed,
    }
}

/// The roster, as the classic paints it.
struct Furnishings<'a> {
    /// The [`Depth::Backdrop`] fixtures in roster order, each with its [`Wash`].
    backdrop: Vec<(Wash, DrawableKind<'a>)>,
    /// The rest, queued for the y-sort in roster order.
    sorted: Vec<Drawable<'a>>,
}

/// Every fixture [`SceneLayout::fixtures`](crate::layout::SceneLayout::fixtures)
/// yields, joined to this frame's live state on the ids its kind carries.
fn queue_fixtures<'a>(
    ctx: &PaintCtx<'_>,
    frame: &SimFrame,
    desk_lights: &[DeskLights],
    neon: crate::floor::NeonLook,
) -> Furnishings<'a> {
    let layout = ctx.layout;
    debug_assert_eq!(
        desk_lights.len(),
        layout.home_desks.len(),
        "desk lights are index-parallel to the home desks"
    );
    let mut out = Furnishings {
        backdrop: Vec::new(),
        sorted: Vec::new(),
    };
    for f in layout.fixtures() {
        let kind = match f.kind {
            FixtureKind::Desk(i) => {
                let facing = layout.desk_facing(i);
                DrawableKind::DeskCubicle {
                    desk: f.top_left(),
                    facing,
                    screen_glow: crate::lighting::desk_screen_glow(
                        desk_occupant(&frame.agents, i),
                        facing,
                        frame.seated_agents.get(&i).copied().unwrap_or(false),
                        ctx.theme,
                    ),
                    lights: desk_lights[i.0],
                    props: frame.desk(i),
                }
            }
            FixtureKind::FilingCabinet(_) => DrawableKind::FilingCabinet { pos: f.top_left() },
            FixtureKind::DeskChair(_) => DrawableKind::DeskChair { pos: f.top_left() },
            FixtureKind::Station { waypoint, station } => {
                let appliance = |sprite| DrawableKind::Appliance {
                    pos: f.at,
                    sprite,
                    busy: frame.occupied_waypoints.contains(&waypoint),
                };
                match station {
                    Station::PantryCounter => {
                        let anim =
                            crate::layout::pantry_counter_anim(layout.pantry_counter_size().w);
                        DrawableKind::WaypointPantry {
                            pos: f.at,
                            anim,
                            // A fixture, which always steams: nothing for the sim to decide.
                            steam: crate::effects::steam(
                                drawable::pantry_steam_at(f.at, anim),
                                ctx.now,
                            ),
                        }
                    }
                    Station::VendingMachine => appliance(crate::pack::VENDING_MACHINE_SPRITE),
                    Station::Printer => appliance(crate::pack::PRINTER_SPRITE),
                    Station::SnackShelf => DrawableKind::SnackShelf { pos: f.at },
                }
            }
            FixtureKind::Plant { kind, .. } => DrawableKind::Plant { kind, pos: f.at },
            FixtureKind::Pod { kind, .. } => DrawableKind::PodDecorItem { kind, pos: f.at },
            FixtureKind::Wall { kind, .. } => DrawableKind::WallDecor {
                kind,
                pos: f.top_left(),
            },
            FixtureKind::MeetingRug { .. }
            | FixtureKind::LoungeRug
            | FixtureKind::PantryMat
            | FixtureKind::IslandMat => DrawableKind::AreaRug(f.visual),
            FixtureKind::MeetingSofa { faces_away, .. } => DrawableKind::MeetingSofa {
                pos: f.at,
                mirrored: faces_away,
            },
            FixtureKind::MeetingTable { .. } => DrawableKind::MeetingTable { pos: f.at },
            FixtureKind::MeetingChair { facing, .. } => DrawableKind::MeetingChair {
                pos: f.at,
                // The backrest rides the side AWAY from the table: a chair
                // FACING East sits west of the table, bar on its west.
                back_west: facing == Facing::East,
            },
            FixtureKind::CoatRack { .. } => DrawableKind::CoatRack { pos: f.at },
            FixtureKind::Doormat { .. } => DrawableKind::Doormat(f.visual),
            FixtureKind::NoticeBoard { .. } => DrawableKind::NoticeBoard(f.visual),
            // The lounge couch IS the meeting sofa's sprite, mirrored.
            FixtureKind::LoungeCouch => DrawableKind::MeetingSofa {
                pos: f.at,
                mirrored: true,
            },
            FixtureKind::SideTable => DrawableKind::LoungeSideTable { pos: f.at },
            FixtureKind::FloorLamp => DrawableKind::FloorLamp { pos: f.at },
            FixtureKind::FishTank => DrawableKind::FishTank { pos: f.at },
            FixtureKind::KitchenIsland => DrawableKind::KitchenIsland { pos: f.at },
            FixtureKind::WaterCooler => DrawableKind::WaterCooler(f.visual),
            FixtureKind::TrashBin => DrawableKind::TrashBin(f.visual),
            FixtureKind::Door => DrawableKind::Door {
                pos: f.top_left(),
                frame_idx: frame.door_frame,
            },
            FixtureKind::Runner => DrawableKind::Runner(f.visual),
            FixtureKind::NeonSign => DrawableKind::NeonSign {
                panel: f.visual,
                look: neon,
            },
            FixtureKind::Clock => DrawableKind::Clock { pos: f.top_left() },
        };
        match f.depth {
            Depth::Backdrop => out.backdrop.push((wash_of(f.kind), kind)),
            Depth::Sorted { row, tie } => out.sorted.push(Drawable {
                anchor_y: row,
                layer: tie.into(),
                kind,
            }),
        }
    }
    out
}

/// Paint the backdrop fixtures whose [`Wash`] is `pass`, in roster order.
fn paint_backdrop(ctx: &mut PaintCtx<'_>, backdrop: &[(Wash, DrawableKind<'_>)], pass: Wash) {
    for (_, kind) in backdrop.iter().filter(|(wash, _)| *wash == pass) {
        paint_drawable(kind, &mut ctx.drawable_ctx());
    }
}

#[cfg(test)]
pub(crate) mod tests;
