//! The SIM half of the frame — advance the world, produce no pixels.
//!
//! `sim_step` mutates the `SimStores` and returns an immutable [`SimFrame`];
//! the paint pass consumes `&SimFrame` and writes only what `PaintCtx` lends it
//! mutably. The paint-local caches it borrows are deliberately NOT sim stores:
//! flushing them changes no behavior, only repaint cost. Headless consumers drive
//! `floor::FloorSession::step` to observe poses/positions without buying a
//! pixel pass.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::id::normalize_path_key;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{ActivityState, DaemonState, FloorLocalDeskIndex};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

use crate::chitchat::{self, ActiveChitchat, ChitchatBubble, VenueKey};
use crate::creatures::{gateway_mascot_def, mascot_position, mascot_seed, pet_position};
use crate::effects::{self, Effect};
use crate::floor::{CoffeeState, FloorInputs, FloorMeta, PetInputs, VacancyDim};
use crate::layout::{Anchor, Point, SceneLayout, Size, WALKING_Y_OFF};
use crate::motion::MotionState;
use crate::pathfind::Router;
use crate::pet::PetKind;
use crate::physics::walking_position;
use crate::pose::{self, Pose, PoseHistory};

use crate::layout::CHARACTER_SPRITE_W;
use crate::pack::{desk_art, desk_art_top};
use anchors::{
    badge_anchor, on_canvas, walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath,
};
use seat::{Seat, settle_seat};

pub(crate) mod anchors;
pub(crate) mod seat;

#[doc(hidden)]
pub use anchors::seated_anchor_facing;

/// The mutable world state one `sim_step` advances.
pub(crate) struct SimStores<'a> {
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut PoseHistory,
    pub motion: &'a mut HashMap<AgentId, MotionState>,
    pub vacancy_dim: &'a mut VacancyDim,
    pub neon: &'a mut crate::floor::NeonState,
    pub chitchat: &'a mut HashMap<VenueKey, ActiveChitchat>,
}

/// A theme-free glow decision for a character sprite. Sim decides WHETHER a
/// glow applies; paint maps it to a `Theme` color — colors are presentation
/// and must not leak into the sim layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterGlow {
    /// No glow.
    None,
    /// `SeatedThinking` — paint uses the theme's default tool-glow color.
    Thinking,
    /// `SeatedTyping` — paint resolves the per-tool tint (`tool_glow_tint`).
    Tool,
}

/// One character's fully resolved placement for this tick — everything the
/// paint pass needs to blit the sprite, with no sim access and no colors.
#[derive(Debug, Clone)]
pub struct CharacterPlacement {
    /// Index into [`SimFrame::agents`] for this character.
    pub agent_idx: usize,
    /// Y-sort key (breath-independent).
    pub anchor_y: u16,
    /// The sprite animation to blit (e.g. `"seated"`, `"walking"`).
    pub anim_name: &'static str,
    /// The frame within `anim_name` to draw this tick.
    pub frame_idx: usize,
    /// Top-left screen position to blit the sprite at.
    pub anchor: Point,
    /// Where its name badge hangs: the top-centre of the frame `anchor` was
    /// fitted for, without the breath, held clear of the art of the desk it sits
    /// at.
    pub label_anchor: Point,
    /// Whether to mirror the sprite horizontally.
    pub flip_x: bool,
    /// The glow decision for this character (paint maps it to a color).
    pub glow: CharacterGlow,
    /// What rides on it this tick, in paint order.
    pub(crate) effects: Vec<Effect>,
    /// Whether `anchor` takes the idle breath: a figure at rest does, a walker's
    /// stride is its own motion.
    pub breathes: bool,
    /// The home desk this placement is SEATED AT, in logical units — `None` for
    /// anyone not sitting at one (walking, at a waypoint, standing). Carried
    /// because `anchor` is already PROJECTED and cannot yield it back.
    pub seat_desk: Option<Point>,
    /// Whether this figure sits on furniture — a desk's chair, a couch, a sofa
    /// or a meeting chair — which grounds it in place of a shadow.
    pub seated: bool,
}

/// The office pet this tick.
#[derive(Debug, Clone)]
pub(crate) struct PetPlacement {
    /// Which pet.
    pub(crate) kind: PetKind,
    /// Its centre, in layout units, fitted so its frame lands on the canvas.
    pub(crate) pos: Point,
    /// Whether to mirror the sprite horizontally.
    pub(crate) flip: bool,
    /// The sprite animation to draw.
    pub(crate) anim_name: &'static str,
    /// The frame within `anim_name`.
    pub(crate) frame_idx: usize,
    /// What rides on it this tick, in paint order.
    pub(crate) effects: Vec<Effect>,
}

/// One gateway mascot this tick.
#[derive(Debug, Clone)]
pub(crate) struct MascotPlacement {
    /// Its centre, in layout units, fitted so its frame lands on the canvas.
    pub(crate) pos: Point,
    /// The frame `pos` was fitted for.
    pub(crate) size: Size,
    /// The sprite animation to draw.
    pub(crate) anim_name: &'static str,
    /// The frame within `anim_name`.
    pub(crate) frame_idx: usize,
    /// The gateway's display name.
    pub(crate) name: &'static str,
    /// The instance id, when its source runs more than one.
    pub(crate) instance: Option<String>,
    /// Its presence's [`display_state`](pixtuoid_core::state::DaemonPresence::display_state).
    pub(crate) state: DaemonState,
    /// What rides on it this tick: a bubble per run in flight.
    pub(crate) effects: Vec<Effect>,
    /// Sessions the gateway holds.
    pub(crate) active_sessions: u32,
}

/// A coffee on a desk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cup {
    /// Fetched within [`CoffeeState::STEAM_WINDOW_SECS`].
    Steaming,
    /// Past that window, still on the desk.
    Cold,
}

/// One home desk's live props this tick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DeskProps {
    /// The coffee its occupant fetched, while it sits on the desk.
    pub(crate) cup: Option<Cup>,
    /// 0 = no tower (the plain desk), else the reams up to
    /// [`MAX_TIER`](crate::token_meter::MAX_TIER).
    pub(crate) token_tier: u8,
    /// A falling sheet's distance fallen, in layout units, when a big usage
    /// reading is mid-drop, else `None`.
    pub(crate) sheet_fall: Option<u16>,
    /// What rides on it this tick: a steaming cup's steam.
    pub(crate) effects: Vec<Effect>,
    /// Which glass column its lit screen's scanline is on, from the glass's
    /// west edge ([`scanline_col`]).
    pub(crate) scanline: u16,
}

/// How long a screen's scanline holds each glass column.
const SCANLINE_STEP_MS: u64 = 120;

/// The glass column a desk at column `desk_x`'s scanline is on at `now`: the
/// line sweeps east and wraps, each desk a column on from its west neighbour's.
pub(crate) fn scanline_col(desk_x: u16, now: SystemTime) -> u16 {
    let glass = crate::layout::SCREEN_GLASS_COLS;
    let glass_w = u64::from(glass.end() - glass.start() + 1);
    let phase = crate::anim::epoch_ms(now) / SCANLINE_STEP_MS + u64::from(desk_x);
    // Below `glass_w`, a u16.
    (phase % glass_w) as u16
}

/// The immutable outcome of one `sim_step`: the world advanced, observed.
/// Paint consumes it by `&` — rendering the same frame twice is byte-identical
/// and cannot move the sim. Owned data, so the stores are free again the moment
/// `sim_step` returns.
#[derive(Clone)]
pub struct SimFrame {
    /// The tick's agent snapshot — placements index into it, paint borrows
    /// from it.
    pub agents: Vec<AgentSlot>,
    /// The authoritative routed pose per home-desk agent this tick (`None` =
    /// no renderable pose). Unread by paint BY DESIGN;
    /// `floor::FloorSession::step` is the lib-side consumer.
    pub poses: HashMap<AgentId, Option<Pose>>,
    /// Per-desk "occupant is actually seated right now" (drives screen glow +
    /// ceiling halos; exiting agents absent by construction).
    pub seated_agents: HashMap<FloorLocalDeskIndex, bool>,
    /// Fully resolved character sprites for this tick, in agent order.
    pub characters: Vec<CharacterPlacement>,
    /// Smoothed indoor-lighting level from `VacancyDim::tick`.
    pub indoor_scale: f32,
    /// The neon sign's light from `NeonState::tick`.
    pub(crate) neon: crate::floor::NeonLevels,
    /// Active speech bubbles after this tick's venue update.
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agents observed walking back with coffee this tick — the caller
    /// persists them into its `CoffeeState`.
    pub new_coffee_carriers: Vec<AgentId>,
    /// Waypoint indices with an occupant this tick — drives the appliance
    /// feedback animations.
    pub occupied_waypoints: std::collections::HashSet<usize>,
    /// The office pet this tick; `None` on a floor without one, or where
    /// [`FloorInputs::pets`] brings none.
    pub(crate) pet: Option<PetPlacement>,
    /// Each gateway mascot present this tick.
    pub(crate) mascots: Vec<MascotPlacement>,
    /// Each home desk's live props, index-parallel to
    /// [`home_desks`](crate::layout::SceneLayout::home_desks); read through
    /// [`Self::desk`].
    pub(crate) desks: Vec<DeskProps>,
    /// The elevator art's frame, shut to open.
    pub(crate) door_frame: usize,
}

impl SimFrame {
    /// Desk `i`'s props this tick.
    pub(crate) fn desk(&self, i: FloorLocalDeskIndex) -> DeskProps {
        debug_assert!(
            i.0 < self.desks.len(),
            "desk props are index-parallel to the home desks"
        );
        self.desks.get(i.0).cloned().unwrap_or_default()
    }
}

/// What one `sim_step` reads, besides the stores it advances.
pub(crate) struct SimInputs<'a> {
    pub(crate) world: FloorInputs<'a>,
    pub(crate) layout: &'a SceneLayout,
    /// Carrier → fetch time of each desk cup.
    pub(crate) coffee: &'a HashMap<AgentId, SystemTime>,
    /// [`FloorCtx::door_anim_max_ms`](crate::floor::FloorCtx::door_anim_max_ms).
    pub(crate) door_anim_max_ms: u64,
}

/// Advance the world one tick WITHOUT painting, into the [`SimFrame`] every
/// painter reads.
pub(crate) fn sim_step(stores: &mut SimStores<'_>, inputs: SimInputs<'_>) -> SimFrame {
    let SimInputs {
        world:
            FloorInputs {
                scene,
                pack,
                now,
                floor,
                pets,
            },
        layout,
        coffee,
        door_anim_max_ms,
    } = inputs;
    let agents: Vec<AgentSlot> = scene.agents.values().cloned().collect();

    let indoor_scale = stores.vacancy_dim.tick(scene.agents.is_empty(), now);
    let neon = stores.neon.tick(
        crate::board::OfficeMood::of(crate::board::scene_stats(scene)),
        stores.vacancy_dim.dimmed(),
        now,
    );

    let char_w = pack
        .animation("standing")
        .and_then(|a| a.frames().first())
        .map_or(CHARACTER_SPRITE_W, |f| f.width());
    // Per-frame occupancy from STATIONARY agent positions only, BEFORE the
    // routed pose pass (which routes Walking poses against THIS overlay).
    // Walkers are deliberately excluded: their position interpolates every
    // frame, which would change the overlay signature every frame, wipe the
    // path cache, recompute A*, and snap walkers to new path segments (the
    // visible "flash"). Sitters at desks are already covered by the static desk
    // mask, so only waypoint visitors — stable across frames — contribute.
    stores.overlay.clear();
    for agent in &agents {
        let Some(pose) = pose::derive(agent, now, layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose
            && let Some(w) = layout.waypoints.get(wp)
        {
            // Reserve the cell the agent actually stands on, NOT the
            // blocked furniture center — else another agent's A* routes
            // straight through the stander.
            let origin = layout
                .home_desk(agent.desk_index.single_floor_local())
                .unwrap_or(w.pos);
            let stand = layout.stand_point(w.kind, w.pos, origin, w.facing);
            stores.overlay.add(
                stand.x.saturating_sub(char_w / 2),
                stand.y.saturating_sub(WALKING_Y_OFF / 2),
                char_w,
                WALKING_Y_OFF,
            );
        }
    }

    // The AUTHORITATIVE pose derivation, ONCE per frame: it runs the
    // advance_wander / walk_path / history side effects, and placement
    // resolution below looks the result up instead of re-deriving (a second
    // derive would double the A*). The `exiting_at` filter is INTENTIONALLY
    // absent — an exiting agent's pose is still needed to place its character.
    let poses: HashMap<AgentId, Option<Pose>> = agents
        .iter()
        .filter(|a| {
            layout
                .home_desk(a.desk_index.single_floor_local())
                .is_some()
        })
        .map(|a| {
            let p = pose::derive_with_routing(
                a,
                now,
                layout,
                &mut pose::RouteCtx {
                    router: &mut *stores.router,
                    overlay: &*stores.overlay,
                    history: &mut *stores.history,
                    motion: &mut *stores.motion,
                },
            );
            (a.agent_id, p)
        })
        .collect();

    // Derived from the cached poses so the desk-cubicle screen glow and the
    // ceiling halos share one gate.
    let seated_agents: HashMap<FloorLocalDeskIndex, bool> = agents
        .iter()
        .filter(|a| {
            layout
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

    let (characters, waypoint_visitors, new_coffee_carriers, occupied_waypoints) =
        resolve_characters(&agents, &poses, layout, pack, char_w, coffee, now);

    let chitchat_bubbles =
        chitchat::update_and_collect(stores.chitchat, floor.floor_idx, &waypoint_visitors, now);
    let pet = pet_placement(&agents, layout, pack, pets, floor, now);
    let mascots = mascot_placements(scene, layout, pack, now);
    let desks = desk_props(&agents, layout, coffee, now);

    let door_frame = anchors::compute_door_frame_idx(&agents, now, door_anim_max_ms);
    SimFrame {
        agents,
        poses,
        seated_agents,
        characters,
        indoor_scale,
        neon,
        chitchat_bubbles,
        new_coffee_carriers,
        occupied_waypoints,
        pet,
        mascots,
        desks,
        door_frame,
    }
}

/// The size of `anim`'s frame `frame_idx`, or `fallback` where the pack lacks
/// it, so a figure still sorts and fits sanely while its blit no-ops.
pub(crate) fn frame_size(pack: &Pack, anim: &str, frame_idx: usize, fallback: Size) -> Size {
    pack_frame_size(pack, anim, frame_idx).unwrap_or(fallback)
}

/// The size of `anim`'s frame `frame_idx`, or `None` where the pack lacks it.
pub(crate) fn pack_frame_size(pack: &Pack, anim: &str, frame_idx: usize) -> Option<Size> {
    pack.animation(anim)
        .and_then(|a| crate::pack::frame_at(a, frame_idx))
        .map(|f| Size {
            w: f.width(),
            h: f.height(),
        })
}

/// The bundled cat's size, for a pack that lacks the pet's anim.
pub(crate) const PET_FALLBACK: Size = Size { w: 8, h: 6 };
/// The bundled lobster's size, for a pack that lacks the mascot's anim.
const MASCOT_FALLBACK: Size = Size { w: 14, h: 12 };

/// The floor's pet this tick. A pet being petted holds still where it was
/// clicked; otherwise `pet_position` roams it around the idle desks.
fn pet_placement(
    agents: &[AgentSlot],
    layout: &SceneLayout,
    pack: &Pack,
    pets: PetInputs<'_>,
    floor: FloorMeta,
    now: SystemTime,
) -> Option<PetPlacement> {
    let kind = pets.pet.map(|p| p.kind)?;
    let petting = pets
        .petting
        .filter(|p| p.is_active(now) && p.kind == kind && p.floor_idx == floor.floor_idx);
    let fit = |anim, frame_idx, pos| {
        on_canvas(
            layout,
            Anchor::Center,
            pos,
            frame_size(pack, anim, frame_idx, PET_FALLBACK),
        )
    };
    if let Some(p) = petting {
        let pos = fit(kind.sit_anim(), 0, p.pet_pos);
        return Some(PetPlacement {
            kind,
            pos,
            flip: false,
            anim_name: kind.sit_anim(),
            frame_idx: 0,
            effects: pet_effects(kind, pos, kind.sit_anim(), Some(p.elapsed_ms(now)), now),
        });
    }
    let idle_desk_indices: Vec<FloorLocalDeskIndex> = agents
        .iter()
        .filter(|a| {
            matches!(a.state, ActivityState::Idle)
                && layout
                    .home_desk(a.desk_index.single_floor_local())
                    .is_some()
                && a.exiting_at.is_none()
        })
        .map(|a| a.desk_index.single_floor_local())
        .collect();
    let all_idle = agents
        .iter()
        .all(|a| matches!(a.state, ActivityState::Idle));
    let (pos, flip, anim_name, frame_idx) = pet_position(
        kind,
        layout,
        pack,
        now,
        &idle_desk_indices,
        all_idle,
        floor.floor_seed,
    )?;
    let pos = fit(anim_name, frame_idx, pos);
    Some(PetPlacement {
        kind,
        pos,
        flip,
        anim_name,
        frame_idx,
        effects: pet_effects(kind, pos, anim_name, None, now),
    })
}

/// Staggers the pet's sleep z off every sleeper's at its desk.
const PET_SLEEP_Z_SEED: u64 = 0xCAFE;

/// What rides on a `kind` pet at `pos` drawn as `anim_name`: hearts while it
/// is being petted, `petted_ms` in, else a z while it sleeps.
pub(crate) fn pet_effects(
    kind: PetKind,
    pos: Point,
    anim_name: &str,
    petted_ms: Option<u64>,
    now: SystemTime,
) -> Vec<Effect> {
    match petted_ms {
        Some(ms) => effects::pet_hearts(pos, ms).collect(),
        None if anim_name == kind.sleep_anim() => effects::sleep_z(pos, PET_SLEEP_Z_SEED, now)
            .into_iter()
            .collect(),
        None => Vec::new(),
    }
}

/// Every gateway mascot present in the scene's daemon roster. The runtime keeps
/// the roster honest, so "entry present" tracks "connected + alive", not merely
/// "a hook arrived"; only the ground floor carries it, so each mascot shows once.
fn mascot_placements(
    scene: &SceneState,
    layout: &SceneLayout,
    pack: &Pack,
    now: SystemTime,
) -> Vec<MascotPlacement> {
    scene
        .daemons()
        .filter_map(|(source, instance, presence)| {
            let def = gateway_mascot_def(source)?;
            let seed = mascot_seed(source, instance);
            let (pos, anim_name, frame_idx) =
                mascot_position(layout, presence, def.walk, def.rest, now, seed)?;
            let size = frame_size(pack, anim_name, frame_idx, MASCOT_FALLBACK);
            let pos = on_canvas(layout, Anchor::Center, pos, size);
            // The busy tell keys on in-flight RUNS, not the (persistent,
            // single-user) session count, which sticks at 1 at rest.
            let runs = presence.in_flight_runs.len() as u32;
            Some(MascotPlacement {
                pos,
                size,
                anim_name,
                frame_idx,
                name: def.display_name,
                // Only worth showing when there is something to disambiguate, and
                // that is per SOURCE: two gateways of ONE daemon need their ports,
                // while two daemon sources already read apart by name and sprite.
                instance: (scene.daemons().filter(|(s, _, _)| *s == source).count() > 1)
                    .then(|| instance.as_str().to_string()),
                state: presence.display_state(),
                effects: if runs > 0 {
                    effects::mascot_bubbles(pos, size.h, runs, now).collect()
                } else {
                    Vec::new()
                },
                active_sessions: presence.active_sessions,
            })
        })
        .collect()
}

/// Each home desk's live props, from its occupant: the coffee it fetched and the
/// tokens it has spent.
fn desk_props(
    agents: &[AgentSlot],
    layout: &SceneLayout,
    coffee: &HashMap<AgentId, SystemTime>,
    now: SystemTime,
) -> Vec<DeskProps> {
    (0..layout.home_desks.len())
        .map(|i| {
            let occupant = desk_occupant(agents, FloorLocalDeskIndex(i));
            let fetched_at = occupant.and_then(|a| coffee.get(&a.agent_id));
            let cup = fetched_at.map(|t| {
                let fresh = now
                    .duration_since(*t)
                    .is_ok_and(|d| d.as_secs() < CoffeeState::STEAM_WINDOW_SECS);
                if fresh { Cup::Steaming } else { Cup::Cold }
            });
            DeskProps {
                cup,
                token_tier: occupant.map_or(0, |a| crate::token_meter::token_tier(a.tokens_used)),
                sheet_fall: occupant.and_then(|a| crate::token_meter::sheet_fall_dist(a, now)),
                effects: cup_effects(layout.home_desks[i], cup, now),
                scanline: scanline_col(layout.home_desks[i].x, now),
            }
        })
        .collect()
}

/// What rides on the cup on the desk at `desk`: steam while it is fresh.
pub(crate) fn cup_effects(desk: Point, cup: Option<Cup>, now: SystemTime) -> Vec<Effect> {
    match cup {
        Some(Cup::Steaming) => effects::steam(desk_cup_at(desk), now).to_vec(),
        Some(Cup::Cold) | None => Vec::new(),
    }
}

/// What a pose arm puts on its figure, before the fit settles where it stands.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Cues {
    /// Asleep: its z's stagger off this seed.
    pub(crate) sleep_seed: Option<u64>,
    /// Waiting on the human.
    pub(crate) waiting: bool,
    /// Walking, on this stride frame.
    pub(crate) stride: Option<usize>,
}

/// What rides on `agent`, whose `w`-wide frame (`None` where its pack lacks
/// one) stands at `anchor` this tick, in paint order: dust underfoot, then a
/// burning head's crown, then a sleeper's z or a waiter's mark.
pub(crate) fn character_effects(
    agent: &AgentSlot,
    anchor: Point,
    w: Option<u16>,
    cues: Cues,
    now: SystemTime,
) -> Vec<Effect> {
    let mut out = Vec::new();
    out.extend(cues.stride.map(|s| effects::walking_dust(anchor, s)));
    if let Some(w) = w
        && crate::burn::slot_burn_tier(agent, now) == crate::burn::BurnTier::Top
    {
        out.push(effects::flame_crown(anchor, w, now));
    }
    out.extend(
        cues.sleep_seed
            .and_then(|seed| effects::sleep_z(anchor, seed, now)),
    );
    if cues.waiting {
        out.push(effects::waiting_mark(anchor));
    }
    out
}

/// Resolve every character's placement for this tick from the routed poses
/// `sim_step` already derived. Returns the placements (paint maps them 1:1 to
/// drawables), the waypoint visitors (for the chitchat venues), the agents seen
/// carrying coffee, and the occupied waypoint indices.
pub(crate) fn resolve_characters(
    agents: &[AgentSlot],
    poses: &HashMap<AgentId, Option<Pose>>,
    layout: &SceneLayout,
    pack: &Pack,
    char_w: u16,
    coffee: &HashMap<AgentId, SystemTime>,
    now: SystemTime,
) -> (
    Vec<CharacterPlacement>,
    Vec<chitchat::Visitor>,
    Vec<AgentId>,
    std::collections::HashSet<usize>,
) {
    let mut placements: Vec<(CharacterPlacement, Cues)> = Vec::new();
    let mut new_coffee_carriers: Vec<AgentId> = Vec::new();
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    let mut waypoint_visitors: Vec<chitchat::Visitor> = Vec::new();
    for (agent_idx, agent) in agents.iter().enumerate() {
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        let Some(p) = poses.get(&agent.agent_id).copied().flatten() else {
            continue;
        };
        let is_waiting = matches!(agent.state, ActivityState::Waiting { .. });
        let seated =
            |base: &'static str, frame_idx: usize, glow: CharacterGlow, sleep_seed: Option<u64>| {
                let facing = layout.desk_facing(agent.desk_index.single_floor_local());
                let seat = Seat::at_desk(desk, facing);
                let anchor = seat.render_anchor(char_w);
                let (anim_name, flip_x) = seat.sprite_in_pack(base, pack);
                let placement = CharacterPlacement {
                    agent_idx,
                    // Breath-independent z-key: the breath's 1 px rise must not flip
                    // sort order against nearby desk decor frame-to-frame.
                    anchor_y: seat.z_key(),
                    anim_name,
                    frame_idx,
                    anchor,
                    label_anchor: anchor,
                    flip_x,
                    glow,
                    effects: Vec::new(),
                    // The one arm that IS seated at a desk — see the field's doc.
                    seat_desk: Some(desk),
                    seated: true,
                    breathes: true,
                };
                let cues = Cues {
                    sleep_seed,
                    waiting: is_waiting,
                    stride: None,
                };
                (placement, cues)
            };
        match p {
            Pose::SeatedIdle if is_waiting => {
                // Waiting is the one state that WANTS the human — the `N wait`
                // counter's twin. Asleep-with-zzz reads as the opposite.
                placements.push(seated("seated", 0, CharacterGlow::None, None));
            }
            Pose::SeatedIdle => {
                let sleep_variant = if agent.agent_id.raw() % 2 == 0 {
                    "seated_sleeping"
                } else {
                    "seated_sleeping_alt"
                };
                placements.push(seated(
                    sleep_variant,
                    0,
                    CharacterGlow::None,
                    Some(agent.agent_id.raw()),
                ));
            }
            Pose::SeatedThinking => {
                placements.push(seated("seated", 0, CharacterGlow::Thinking, None));
            }
            Pose::SeatedTyping { frame } => {
                placements.push(seated("typing", frame, CharacterGlow::Tool, None));
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    let stand = layout.stand_point(wp_obj.kind, wp_obj.pos, desk, wp_obj.facing);
                    let seat = Seat::at_waypoint(kind, stand, wp_obj.facing);
                    let anchor_base = seat.render_anchor(char_w);
                    let (anim_name, flip_x) = seat.sprite_in_pack("seated", pack);
                    let anchor_no_breath = Point {
                        x: anchor_base.x.saturating_add_signed(dx),
                        y: anchor_base.y,
                    };
                    if chitchat::supports_chitchat(kind) {
                        waypoint_visitors.push(chitchat::Visitor {
                            // The couch's seats collapse to ONE venue so it
                            // hosts a single group conversation; other
                            // waypoints key on their own index.
                            wp_idx: chitchat::venue_wp_idx(kind, wp, &layout.waypoints),
                            agent_id: agent.agent_id,
                            room_id: wp_obj.room_id,
                        });
                    }
                    placements.push((
                        CharacterPlacement {
                            agent_idx,
                            // The glide's own key, so nothing pops at the
                            // walk→seat seam.
                            anchor_y: seat.z_key(),
                            anim_name,
                            frame_idx: 0,
                            anchor: anchor_no_breath,
                            label_anchor: anchor_no_breath,
                            flip_x,
                            glow: CharacterGlow::None,
                            effects: Vec::new(),
                            seat_desk: None,
                            seated: seat.seated_furniture(),
                            breathes: true,
                        },
                        Cues::default(),
                    ));
                }
            }
            Pose::AimlessAt { dest } => {
                let anchor = waypoint_anchor(dest, char_w);
                placements.push((
                    CharacterPlacement {
                        agent_idx,
                        anchor_y: anchor.y + WALKING_Y_OFF,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        label_anchor: anchor,
                        flip_x: false,
                        glow: CharacterGlow::None,
                        effects: Vec::new(),
                        seat_desk: None,
                        seated: false,
                        breathes: true,
                    },
                    Cues::default(),
                ));
            }
            Pose::Walking {
                from,
                to,
                t_x1000,
                frame,
                mut carrying_coffee,
            } => {
                // Exit walks: core sets carrying_coffee=false (it holds no
                // render-side state), but the coffee map knows better.
                if agent.exiting_at.is_some() && coffee.contains_key(&agent.agent_id) {
                    carrying_coffee = true;
                }
                if carrying_coffee {
                    new_coffee_carriers.push(agent.agent_id);
                }
                let pos = walking_position(from, to, t_x1000);
                let walker_anchor = walking_anchor(pos, char_w);
                let dx = to.x as i32 - from.x as i32;
                let dy = to.y as i32 - from.y as i32;
                // A glide on/off a seat (`to` is a foot-cell sitting down,
                // `from` rising) renders in the SEAT's view and at the SEAT's
                // z-key, NOT the travel direction's. Without it a window-facing
                // seat renders a FRONT walk and the agent sits facing the
                // camera until it snaps at AtWaypoint. Ordinary travel segments
                // keep the travel-direction facing and foot-position z-key.
                let settle = settle_seat(to, layout).or_else(|| settle_seat(from, layout));
                let (going_back, flip) = match settle {
                    Some(seat) => seat.settle_walk(),
                    None => (
                        dy.unsigned_abs() > dx.unsigned_abs() && dy < 0,
                        to.x < from.x,
                    ),
                };
                // walking_back always wins (no back-facing coffee sprite).
                let anim_name: &'static str = if going_back {
                    "walking_back"
                } else if carrying_coffee && pack.animation("walking_coffee").is_some() {
                    "walking_coffee"
                } else {
                    "walking"
                };
                placements.push((
                    CharacterPlacement {
                        agent_idx,
                        anchor_y: match settle {
                            Some(seat) => seat.z_key(),
                            None => walker_anchor.y + WALKING_Y_OFF,
                        },
                        anim_name,
                        frame_idx: frame,
                        anchor: walker_anchor,
                        label_anchor: walker_anchor,
                        flip_x: flip,
                        glow: CharacterGlow::None,
                        effects: Vec::new(),
                        seat_desk: None,
                        seated: false,
                        breathes: false,
                    },
                    Cues {
                        stride: Some(frame),
                        ..Cues::default()
                    },
                ));
            }
        }
    }
    // ONE fit for every pose arm, on the frame each placement will blit, read by
    // both the sprite and its badge. The z-key keeps pre-fit geometry.
    let fallback = Size {
        w: char_w,
        h: crate::layout::CHARACTER_SPRITE_H,
    };
    for (p, cues) in &mut placements {
        let art = pack_frame_size(pack, p.anim_name, p.frame_idx);
        let size = art.unwrap_or(fallback);
        let fitted = on_canvas(layout, Anchor::TopLeft, p.anchor, size);
        // The painter's own desk art: whatever it raises behind the sitter's
        // head, the badge clears.
        let ceiling = p.seat_desk.and_then(|d| {
            desk_art(pack, layout.desk_facing_at(d))
                .map(|art| desk_art_top(pack, d.y, art.height()))
        });
        p.label_anchor = badge_anchor(fitted, size, ceiling);
        // Breath after the fit, so it never moves the badge.
        let agent = &agents[p.agent_idx];
        p.anchor = if p.breathes {
            with_breath(fitted, agent.agent_id, now)
        } else {
            fitted
        };
        p.effects = character_effects(agent, p.anchor, art.map(|s| s.w), *cues, now);
    }

    // wp_rank's keys ARE this tick's occupied waypoints — every AtWaypoint
    // occupant registers a rank.
    (
        placements.into_iter().map(|(p, _)| p).collect(),
        waypoint_visitors,
        new_coffee_carriers,
        wp_rank.into_keys().collect(),
    )
}

/// Where the cup stands on the desk at `desk`: its top-left cell.
pub(crate) fn desk_cup_at(desk: Point) -> Point {
    Point {
        x: desk.x + 2,
        y: desk.y + 2,
    }
}

/// The agent whose home desk is `local`, while they have not begun to leave.
pub(crate) fn desk_occupant(
    agents: &[AgentSlot],
    local: FloorLocalDeskIndex,
) -> Option<&AgentSlot> {
    agents
        .iter()
        .find(|a| a.desk_index.single_floor_local() == local && a.exiting_at.is_none())
}

/// Deterministic seed from a normalized cwd string: byte-fold, then the
/// splitmix64 finalizer. NOT `DefaultHasher`: its algorithm may change between
/// Rust releases, which would re-dress every agent on a toolchain bump.
fn cwd_outfit_seed(cwd_norm: &str) -> u64 {
    let folded = cwd_norm
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64));
    pixtuoid_core::id::splitmix64(folded)
}

/// The outfit-determining seed for `agent`. Extracted so
/// `FrameCache::note_outfit_seed` watches the mid-lifetime cwd backfill through
/// the EXACT unknown-cwd fallback [`agent_overrides`](crate::character::agent_overrides)
/// uses; a second copy would drift.
pub(crate) fn outfit_seed_for(agent: &AgentSlot) -> u64 {
    if agent.unknown_cwd || agent.cwd.as_os_str().is_empty() {
        agent.agent_id.raw()
    } else {
        cwd_outfit_seed(&normalize_path_key(&agent.cwd.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The phase is epoch-based, so it outgrows a u16: the scanline must keep
    /// stepping one column per step rather than overflow or jump.
    #[test]
    fn the_scanline_keeps_stepping_past_a_u16_phase() {
        let col = |step: u64| {
            scanline_col(
                0,
                SystemTime::UNIX_EPOCH + Duration::from_millis(step * SCANLINE_STEP_MS + 1),
            )
        };
        let glass = crate::layout::SCREEN_GLASS_COLS;
        let glass_w = glass.end() - glass.start() + 1;
        let before = col(u64::from(u16::MAX));
        assert_eq!(col(u64::from(u16::MAX) + 1), (before + 1) % glass_w);
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(7 * SCANLINE_STEP_MS);
        assert_eq!(
            scanline_col(3, at),
            (scanline_col(0, at) + 3) % glass_w,
            "each desk a column on from its west neighbour's"
        );
    }
}
