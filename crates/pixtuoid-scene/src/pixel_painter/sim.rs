//! The SIM half of the frame — advance the world, produce no pixels.
//!
//! `sim_step` mutates the [`SimStores`] and returns an immutable [`SimFrame`];
//! the paint pass consumes `&SimFrame` and only ever writes the pixel buffer +
//! the paint-local `FrameCache`. That cache is deliberately NOT a sim store:
//! flushing it changes no behavior, only repaint cost. Headless consumers drive
//! `floor::FloorSession::observe` to observe poses/positions without buying a
//! pixel pass.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

use crate::chitchat::{self, ActiveChitchat, ChitchatBubble, VenueKey};
use crate::floor::LightingState;
use crate::layout::{Layout, Point, WALKING_Y_OFF};
use crate::motion::{walking_position, MotionState};
use crate::pathfind::Router;
use crate::pose::{self, Pose, PoseHistory};

use super::anchors::{
    walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath, CHARACTER_SPRITE_W,
};
use super::seat::{settle_seat, Seat};

/// The mutable world state one `sim_step` advances.
pub(crate) struct SimStores<'a> {
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut PoseHistory,
    pub motion: &'a mut HashMap<AgentId, MotionState>,
    pub light: &'a mut LightingState,
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
#[derive(Debug, Clone, Copy)]
pub struct CharacterPlacement {
    /// Index into [`SimFrame::agents`] for this character.
    pub agent_idx: usize,
    /// Y-sort key (breath-independent).
    pub anchor_y: u16,
    /// The sprite animation to blit (e.g. `"seated"`, `"walk"`).
    pub anim_name: &'static str,
    /// The frame within `anim_name` to draw this tick.
    pub frame_idx: usize,
    /// Top-left screen position to blit the sprite at.
    pub anchor: Point,
    /// Whether to mirror the sprite horizontally (facing west).
    pub flip_x: bool,
    /// The glow decision for this character (paint maps it to a color).
    pub glow: CharacterGlow,
    /// `Some(seed)` drives the sleeping "z" particles; `None` when awake.
    pub sleep_z_seed: Option<u64>,
    /// Whether to draw the waiting/permission bubble over this character.
    pub waiting_bubble: bool,
    /// `Some(frame)` draws the walking dust puff; `None` when standing still.
    pub walking_dust_frame: Option<usize>,
    /// The home desk this placement is SEATED AT, in logical units — `None` for
    /// anyone not sitting at one (walking, at a waypoint, standing).
    ///
    /// The occupant's desk, carried because `anchor` is already PROJECTED and cannot yield it back.
    pub seat_desk: Option<Point>,
}

/// The immutable outcome of one `sim_step`: the world advanced, observed.
/// Paint consumes it by `&` — rendering the same frame twice is byte-identical
/// and cannot move the sim. Owned data, so the stores are free again the moment
/// `sim_step` returns.
pub struct SimFrame {
    /// The tick's agent snapshot — placements index into it, paint borrows
    /// from it.
    pub agents: Vec<AgentSlot>,
    /// The authoritative routed pose per home-desk agent this tick (`None` =
    /// no renderable pose). Unread by paint BY DESIGN;
    /// `floor::FloorSession::observe` is the lib-side consumer.
    pub poses: HashMap<AgentId, Option<Pose>>,
    /// Per-desk "occupant is actually seated right now" (drives screen glow +
    /// ceiling halos; exiting agents absent by construction).
    pub seated_agents: HashMap<FloorLocalDeskIndex, bool>,
    /// Fully resolved character sprites for this tick, in agent order.
    pub characters: Vec<CharacterPlacement>,
    /// Smoothed indoor-lighting level from `LightingState::tick`.
    pub indoor_scale: f32,
    /// Active speech bubbles after this tick's venue update.
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agents observed walking back with coffee this tick — the caller
    /// persists them into its `CoffeeState`.
    pub new_coffee_carriers: Vec<AgentId>,
    /// Waypoint indices with an occupant this tick — drives the appliance
    /// feedback animations.
    pub occupied_waypoints: std::collections::HashSet<usize>,
}

/// Advance the world one tick WITHOUT painting: lighting fade, occupancy
/// overlay, the authoritative `derive_with_routing` pose pass, character
/// placement resolution, and the chitchat venue update.
///
/// `pack` is a genuine sim input: character anchors center on the pack's
/// sprite width, and placement is position. Time is a parameter — never read
/// the clock here (wasm).
pub(crate) fn sim_step(
    stores: &mut SimStores<'_>,
    scene: &SceneState,
    layout: &Layout,
    pack: &Pack,
    coffee: &HashMap<AgentId, SystemTime>,
    floor_idx: usize,
    now: SystemTime,
) -> SimFrame {
    let agents: Vec<AgentSlot> = scene.agents.values().cloned().collect();

    let indoor_scale = stores.light.tick(scene.agents.is_empty(), now);

    // Per-frame occupancy from STATIONARY agent positions only, BEFORE the
    // routed pose pass (which routes Walking poses against THIS overlay).
    // Walkers are deliberately excluded: their position interpolates every
    // frame, which would change the overlay signature every frame, wipe the
    // path cache, recompute A*, and snap walkers to new path segments (the
    // visible "flash"). Sitters at desks are already covered by the static desk
    // mask, so only waypoint visitors — stable across frames — contribute.
    let char_w = pack
        .animation("standing")
        .and_then(|a| a.frames.first())
        .map_or(CHARACTER_SPRITE_W, |f| f.width());
    stores.overlay.clear();
    for agent in &agents {
        let Some(pose) = pose::derive(agent, now, layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = layout.waypoints.get(wp) {
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
        chitchat::update_and_collect(stores.chitchat, floor_idx, &waypoint_visitors, now);

    SimFrame {
        agents,
        poses,
        seated_agents,
        characters,
        indoor_scale,
        chitchat_bubbles,
        new_coffee_carriers,
        occupied_waypoints,
    }
}

/// Resolve every character's placement for this tick from the routed poses
/// `sim_step` already derived. Returns the placements (paint maps them 1:1 to
/// drawables), the waypoint visitors (for the chitchat venues), the agents seen
/// carrying coffee, and the occupied waypoint indices.
fn resolve_characters(
    agents: &[AgentSlot],
    poses: &HashMap<AgentId, Option<Pose>>,
    layout: &Layout,
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
    let mut placements: Vec<CharacterPlacement> = Vec::new();
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
        let seated = |base: &'static str,
                      frame_idx: usize,
                      glow: CharacterGlow,
                      sleep_z_seed: Option<u64>| {
            let facing = layout.desk_facing(agent.desk_index.single_floor_local());
            let seat = Seat::at_desk(desk, facing);
            let anchor_no_breath = seat.render_anchor(char_w);
            let (anim_name, flip_x) = seat.sprite_in_pack(base, pack);
            let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
            CharacterPlacement {
                agent_idx,
                // Breath-independent z-key: the ±1px breath must not flip sort
                // order against nearby desk decor frame-to-frame.
                anchor_y: seat.z_key(),
                anim_name,
                frame_idx,
                anchor,
                flip_x,
                glow,
                sleep_z_seed,
                waiting_bubble: matches!(agent.state, ActivityState::Waiting { .. }),
                // The one arm that IS seated at a desk — see the field's doc.
                seat_desk: Some(desk),
                walking_dust_frame: None,
            }
        };
        match p {
            Pose::SeatedIdle if matches!(agent.state, ActivityState::Waiting { .. }) => {
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
                    // The label twin in `anchors::character_anchor` rides this
                    // SAME call, so sprite and badge can't drift.
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
                            anchor: anchor_no_breath,
                            room_id: wp_obj.room_id,
                        });
                    }
                    let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
                    placements.push(CharacterPlacement {
                        agent_idx,
                        // The glide's own key, so nothing pops at the
                        // walk→seat seam.
                        anchor_y: seat.z_key(),
                        anim_name,
                        frame_idx: 0,
                        anchor,
                        flip_x,
                        glow: CharacterGlow::None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        seat_desk: None,
                        walking_dust_frame: None,
                    });
                }
            }
            Pose::AimlessAt { dest } => {
                let anchor_no_breath = waypoint_anchor(dest, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
                placements.push(CharacterPlacement {
                    agent_idx,
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    anim_name: "standing",
                    frame_idx: 0,
                    anchor,
                    flip_x: false,
                    glow: CharacterGlow::None,
                    sleep_z_seed: None,
                    waiting_bubble: false,
                    seat_desk: None,
                    walking_dust_frame: None,
                });
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
                placements.push(CharacterPlacement {
                    agent_idx,
                    anchor_y: match settle {
                        Some(seat) => seat.z_key(),
                        None => walker_anchor.y + WALKING_Y_OFF,
                    },
                    anim_name,
                    frame_idx: frame,
                    anchor: walker_anchor,
                    flip_x: flip,
                    glow: CharacterGlow::None,
                    sleep_z_seed: None,
                    waiting_bubble: false,
                    seat_desk: None,
                    walking_dust_frame: Some(frame),
                });
            }
        }
    }
    // wp_rank's keys ARE this tick's occupied waypoints — every AtWaypoint
    // occupant registers a rank.
    (
        placements,
        waypoint_visitors,
        new_coffee_carriers,
        wp_rank.into_keys().collect(),
    )
}
