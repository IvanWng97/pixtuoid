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
use pixtuoid_core::state::DaemonState;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::chitchat::{ActiveChitchat, ChitchatBubble};
#[cfg(test)]
use crate::floor::LightingState;
use crate::frame_cache::FrameCache;
use crate::layout::{
    z_sort_row, Anchor, Depth, Facing, FixtureKind, Layout, Point, Size, Station, WaypointKind,
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
pub(crate) use background::{
    clock_reading, neon_look, octant_offset, ClockReading, RUNNER_LATTICE_STRIDE,
};
#[cfg(test)]
pub(crate) use drawable::DESK_BEZEL_RAISE;
pub(crate) use drawable::{
    desk_art_top, desk_sprite_name, DESK_CHAIR_SPRITE, MEETING_TABLE_SPRITE,
};
pub(crate) use palette::{
    appliance_overrides, blend_rgb, fixture_overrides, CLOCK_FACE_KEY, DESK_BULB_KEY,
    SCREEN_GLASS_KEY, SCREEN_TEXT_KEY,
};

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
use background::{paint_floor_and_walls, paint_floor_wash, paint_light, paint_shadow};
use drawable::{paint_drawable, Drawable, DrawableKind, Layer};
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
            door_anim_max_ms: ctx.store.door_anim_max_ms,
        },
    );
    let (pet_pos, mascots) = paint_frame(
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

/// The floor shadow under one home desk. `cy` is the row the roster sorts
/// the desk at, off the same furniture row, so a `DESK_H` retune moves the
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

/// The classic painter's floor shadows, handed to `shadow` in roster
/// order, which is their PAINT ORDER — the overlaps blend, so the order is
/// load-bearing (a meeting sofa's seat shadows overlap each other). The
/// per-piece `half_w`/`half_h` are owner-tuned taste literals.
fn floor_shadow_ellipses(layout: &Layout, mut shadow: impl FnMut(Ellipse)) {
    use crate::layout::{furniture_def, Furniture};

    // Fit the ellipse to the sprite width — a flat 7 half-width doubles a
    // narrow shelf's shadow; `.min(7)` caps a future wide piece.
    let fitted = |pos: Point, kind: WaypointKind| {
        let vis_w = furniture_def(kind.furniture()).visual.w;
        let half_w = if vis_w > 0 {
            (vis_w / 2 + crate::ground::CONTACT_REACH).min(7)
        } else {
            7
        };
        Ellipse {
            cx: pos.x,
            cy: pos.y + 2,
            half_w,
            half_h: 2,
        }
    };
    for f in layout.fixtures() {
        match f.kind {
            FixtureKind::Desk(_) => shadow(desk_shadow_ellipse(Point {
                x: f.visual.x,
                y: f.visual.y,
            })),
            FixtureKind::Station { waypoint, station } => {
                let wp = &layout.waypoints[waypoint];
                shadow(match station {
                    // It souths at +1, not the fitted +2.
                    Station::Printer => Ellipse {
                        cx: wp.pos.x,
                        cy: wp.pos.y + 1,
                        half_w: 5,
                        half_h: 1,
                    },
                    Station::PantryCounter | Station::VendingMachine | Station::SnackShelf => {
                        fitted(wp.pos, wp.kind)
                    }
                });
            }
            FixtureKind::Pod { kind, .. } => {
                if let Some(wp) = kind.waypoint() {
                    shadow(fitted(f.at, wp));
                }
            }
            // One under each of its seats.
            FixtureKind::MeetingSofa {
                room, faces_away, ..
            } => layout
                .waypoints
                .iter()
                .filter(|w| {
                    w.kind == WaypointKind::MeetingSofa
                        && w.room_id == Some(room)
                        && (w.facing == Facing::North) == faces_away
                })
                .for_each(|w| shadow(fitted(w.pos, w.kind))),
            FixtureKind::MeetingChair { .. } => {
                shadow(fitted(f.at, WaypointKind::MeetingChair));
            }
            // Under the body: its seats' stands are empty floor beside it.
            FixtureKind::KitchenIsland => {
                let vis = furniture_def(Furniture::KitchenIsland).visual;
                shadow(Ellipse {
                    cx: f.at.x,
                    cy: z_sort_row(Anchor::Center, f.at, vis.h),
                    half_w: vis.w / 2 + crate::ground::CONTACT_REACH,
                    half_h: 2,
                });
            }
            // One for the whole couch: a per-seat shadow would overlap-darken.
            FixtureKind::LoungeCouch => shadow(Ellipse {
                cx: f.at.x,
                cy: f.at.y + 2,
                half_w: 7,
                half_h: 2,
            }),
            // Off the same height the z-anchor uses: a fixed +3 only suited the
            // taller plants and floated the rest.
            FixtureKind::Plant { kind, .. } => shadow(Ellipse {
                cx: f.at.x,
                cy: z_sort_row(
                    Anchor::Center,
                    f.at,
                    furniture_def(kind.furniture()).visual.h,
                ),
                half_w: 3,
                half_h: 1,
            }),
            FixtureKind::FloorLamp => {
                if let Some(base) = layout.floor_lamp_base() {
                    shadow(Ellipse {
                        cx: base.x,
                        cy: base.y,
                        half_w: 2,
                        half_h: 1,
                    });
                }
            }
            FixtureKind::FilingCabinet(_)
            | FixtureKind::DeskChair(_)
            | FixtureKind::Wall { .. }
            | FixtureKind::MeetingRug { .. }
            | FixtureKind::MeetingTable { .. }
            | FixtureKind::CoatRack { .. }
            | FixtureKind::Doormat { .. }
            | FixtureKind::NoticeBoard { .. }
            | FixtureKind::LoungeRug
            | FixtureKind::SideTable
            | FixtureKind::FishTank
            | FixtureKind::PantryMat
            | FixtureKind::IslandMat
            | FixtureKind::WaterCooler
            | FixtureKind::TrashBin
            | FixtureKind::Door
            | FixtureKind::Runner
            | FixtureKind::NeonSign
            | FixtureKind::Clock => {}
        }
    }
}

/// The PAINT half of the frame: blit the world the sim already advanced. Every
/// positional/lifecycle decision was made in `sim_step` — this pass only
/// resolves presentation (theme colors, sprite pixels) and composites.
fn paint_frame(ctx: &mut PaintCtx<'_>, frame: &SimFrame) -> (Option<PetFrame>, Vec<MascotFrame>) {
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
    let Furnishings {
        backdrop,
        sorted: mut drawables,
    } = queue_fixtures(ctx, frame, &lights.desks, neon);
    paint_backdrop(ctx, &backdrop, Wash::Spared);
    // The rest overwrite the floor, so the overlays above cannot reach them, and
    // they paint before the drawable snapshot, so that pass cannot either — the
    // corridor runner used to stay full-daylight tan in a dimmed office, the
    // brightest thing in the room. Hence their own wash, which also keeps the
    // EMITTERS painted above (the floor-lamp halo) out of it.
    let pre_floor_fixtures = ctx.buf.clone();
    paint_backdrop(ctx, &backdrop, Wash::Washed);
    wash_since(ctx.buf, &pre_floor_fixtures, look.object_wash);

    let shadow_strength = crate::ground::shadow_strength(look.darkness);
    floor_shadow_ellipses(ctx.layout, |ell| {
        paint_shadow(ctx.buf, ell, shadow_strength, ctx.theme);
    });

    ambient::paint_ambient(ctx, look, &lights.monitor_halos);

    // Every entity gets an `anchor_y` — its floor-touching row — so sorting
    // ascending and painting in order puts things closer to the camera in
    // front: the painter's algorithm on a top-down 2D scene.
    let resolved_pet_pos = frame.pet.map(|pet| enqueue_pet(ctx, pet, &mut drawables));
    let resolved_mascots = enqueue_gateway_mascots(ctx, &frame.mascots, &mut drawables);
    enqueue_characters(ctx, frame, &mut drawables);
    enqueue_room_walls(ctx.layout, &mut drawables);
    drawable::sort_drawables(&mut drawables);
    // A per-pixel diff finds EXACTLY what the foreground wrote; a rectangular
    // band seamed the window glass and washed floor-between-pieces twice.
    // AFTER `paint_shadow`/`paint_ambient`: both already take `look`, so folding
    // them in here would apply the hour twice.
    let pre_foreground = ctx.buf.clone();
    for d in &drawables {
        paint_drawable(&d.kind, &mut ctx.drawable_ctx());
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

    (resolved_pet_pos, resolved_mascots)
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
            layer: Layer::Figure,
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

const VENDING_MACHINE_SPRITE: &str = "vending_machine";
const PRINTER_SPRITE: &str = "printer";

/// The pack art a corridor appliance at a `kind` waypoint is drawn from.
pub(crate) fn appliance_art(kind: crate::layout::WaypointKind) -> Option<&'static str> {
    use crate::layout::WaypointKind as K;
    match kind {
        K::VendingMachine => Some(VENDING_MACHINE_SPRITE),
        K::Printer => Some(PRINTER_SPRITE),
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

/// The frame of a looping `anim` showing at `now`: one each of the art's own
/// `frame_ms`, round and round.
pub(crate) fn looping_frame(anim: &Sprite, now: std::time::SystemTime) -> usize {
    let frames = anim.frames().len().max(1) as u64;
    let step = crate::anim::epoch_ms(now) / u64::from(anim.frame_ms().max(1));
    usize::try_from(step % frames).unwrap_or(0)
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
        layer: Layer::Figure,
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
                layer: Layer::Figure,
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
    neon: background::NeonLook,
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
                    screen_glow: desk_screen_glow(
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
                    Station::PantryCounter => DrawableKind::WaypointPantry {
                        pos: f.at,
                        anim: pantry_counter_anim(layout.pantry_counter_size().w),
                    },
                    Station::VendingMachine => appliance(VENDING_MACHINE_SPRITE),
                    Station::Printer => appliance(PRINTER_SPRITE),
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
