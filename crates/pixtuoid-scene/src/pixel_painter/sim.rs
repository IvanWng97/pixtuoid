//! The SIM half of the frame — advance the world, produce no pixels.
//!
//! `render_to_rgb_buffer` used to fuse sim-step and paint in one pass:
//! `PixelCtx` carried six `&mut` sim stores, so there was no way to advance
//! motion/lifecycle/venue state without painting, and no way to observe the
//! outcomes except through a full render. The split: [`sim_step`] mutates the
//! [`SimStores`] and returns an immutable [`SimFrame`] snapshot; the paint
//! pass (still inside `render_to_rgb_buffer`) consumes `&SimFrame` and only
//! ever writes the pixel buffer + the paint-local `FrameCache` (a render
//! cache, deliberately NOT a sim store). Headless consumers drive
//! `floor::FloorSession::observe` (the facade over `sim_step`) to observe
//! poses/positions without buying a pixel pass.
//!
//! Store classification (what makes something SIM vs PAINT):
//! * SIM — state that advances with time and must persist across frames:
//!   `router` (A* cache), `overlay` (per-tick occupancy), `history` (pose
//!   continuity), `motion` (walk legs/wander timeline), `light` (occupancy
//!   fade), `chitchat` (venue conversations).
//! * PAINT-LOCAL — `FrameCache` (recolored-sprite cache): flushing it changes
//!   no behavior, only repaint cost. It stays on the paint side.

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
    back_couch_anchor, seated_anchor, standing_at_desk_anchor, walking_anchor, waypoint_anchor,
    waypoint_rank_offset_x, with_breath, CHARACTER_SPRITE_W,
};
use super::seat::{seat_sprite_in_pack, settle_seat_view, SeatView};

/// The mutable world state one [`sim_step`] advances — every `&mut` store the
/// fused pass used to hide inside `PixelCtx`. `render_to_rgb_buffer` builds
/// one from its `PixelCtx`; a headless consumer builds one from its own
/// `FloorCtx` + chitchat map.
pub(crate) struct SimStores<'a> {
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut PoseHistory,
    pub motion: &'a mut HashMap<AgentId, MotionState>,
    pub light: &'a mut LightingState,
    pub chitchat: &'a mut HashMap<VenueKey, ActiveChitchat>,
}

/// A theme-free glow decision for a character sprite. Sim decides WHETHER a
/// glow applies (a pose/lifecycle fact); paint maps it to a `Theme` color —
/// colors are presentation and must not leak into the sim layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterGlow {
    None,
    /// `SeatedThinking` — paint uses the theme's default tool-glow color.
    Thinking,
    /// `SeatedTyping` — paint resolves the per-tool tint (`tool_glow_tint`).
    Tool,
}

/// One character's fully resolved placement for this tick — everything the
/// paint pass needs to blit the sprite, with no sim access and no colors.
/// `agent_idx` indexes [`SimFrame::agents`].
#[derive(Debug, Clone, Copy)]
pub struct CharacterPlacement {
    pub agent_idx: usize,
    /// Y-sort key (breath-independent — see the arm comments in
    /// `resolve_characters`).
    pub anchor_y: u16,
    pub anim_name: &'static str,
    pub frame_idx: usize,
    pub anchor: Point,
    pub flip_x: bool,
    pub glow: CharacterGlow,
    pub sleep_z_seed: Option<u64>,
    pub waiting_bubble: bool,
    pub walking_dust_frame: Option<usize>,
}

/// The immutable outcome of one [`sim_step`]: the world advanced, observed.
/// Paint consumes it by `&` — rendering the same frame twice is byte-identical
/// and cannot move the sim (the purity the split exists for). Owned data (no
/// borrows into the stores) so the stores are free again the moment
/// `sim_step` returns.
pub struct SimFrame {
    /// The tick's agent snapshot (the one clone per frame the fused pass
    /// already made) — placements index into it, paint borrows from it.
    pub agents: Vec<AgentSlot>,
    /// The authoritative routed pose per home-desk agent this tick — the
    /// headless observation payload (`None` = no renderable pose, e.g. the
    /// exit window passed). Unread by paint BY DESIGN (that's the split's
    /// point); `floor::FloorSession::observe` is the lib-side consumer.
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
    /// persists them into its `CoffeeState` (unchanged epilogue contract).
    pub new_coffee_carriers: Vec<AgentId>,
    /// Waypoint indices with an occupant this tick — the paint-facing
    /// observation (like `seated_agents`) that drives the appliance feedback
    /// animations (printer eject / vending drop, B-4).
    pub occupied_waypoints: std::collections::HashSet<usize>,
}

/// Advance the world one tick WITHOUT painting: lighting fade, occupancy
/// overlay, the authoritative `derive_with_routing` pose pass (walk legs /
/// wander timeline / pose history side effects), character placement
/// resolution, and the chitchat venue update. Returns the immutable
/// [`SimFrame`] the paint pass (or a headless observer) consumes.
///
/// `pack` is a genuine sim input: character anchors center on the pack's
/// sprite width, and placement is position. `coffee` is the immutable
/// carrier→fetch-time view (`CoffeeState::map`); `floor_idx` keys the
/// chitchat venues. Time is a parameter — never read the clock here (wasm).
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

    // Per-floor lighting: tick the fade state with the current occupancy.
    // `indoor_scale` smoothly travels from MIN_LEVEL (empty + past debounce)
    // to 1.0 (populated). Windows/skyline are unaffected.
    let indoor_scale = stores.light.tick(scene.agents.is_empty(), now);

    // Build per-frame occupancy from STATIONARY agent positions only — BEFORE
    // the routed pose pass, which routes Walking poses against THIS overlay.
    // Walkers are deliberately excluded — their position interpolates every
    // frame, which would change the overlay signature every frame, wipe the
    // path cache, recompute A*, and snap walkers to new path segments (the
    // visible "flash"). Sitters at desks are already covered by the static
    // desk mask. Only waypoint visitors contribute here — they have stable
    // positions across frames, so the signature is stable and the cache hits.
    // Reads only the STATELESS `pose::derive` + stand_point (no dependency on
    // the seated map / ambient), so it's safe up here.
    stores.overlay.clear();
    for agent in &agents {
        let Some(pose) = pose::derive(agent, now, layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = layout.waypoints.get(wp) {
                // Reserve the cell the agent actually stands on (the stand cell,
                // off the furniture), NOT the blocked furniture center — else
                // another agent's A* routes straight through the stander. Same
                // `desk` origin as every other stand_point caller.
                let origin = layout
                    .home_desk(agent.desk_index.single_floor_local())
                    .unwrap_or(w.pos);
                let stand = crate::layout::stand_point(
                    w.kind,
                    w.pos,
                    layout.pantry_counter_size(),
                    &layout.walkable,
                    origin,
                    w.facing,
                    &layout.reachable,
                );
                stores
                    .overlay
                    .add(stand.x.saturating_sub(4), stand.y.saturating_sub(6), 8, 12);
            }
        }
    }

    // Derive every home-desk agent's routed pose ONCE per frame. This is the
    // AUTHORITATIVE pose derivation — it runs the advance_wander / walk_path /
    // history side effects exactly once; placement resolution below just looks
    // the cached pose up by agent_id instead of re-deriving (the old double-A*).
    // The `exiting_at` filter is INTENTIONALLY absent: exiting agents are never
    // SeatedTyping/Thinking (so `seated_agents` is unchanged), but their pose is
    // needed for the character placement. Only the home_desk filter remains (a
    // deskless agent can't render anyway).
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

    // Per-desk "is the occupant actually seated right now" map (pose is
    // SeatedTyping/Thinking, not walking in / snapping back), derived from the
    // cached poses so the desk-cubicle screen glow + ceiling halos share one gate
    // and one pose derivation (no double A*). Exiting agents are absent from the
    // seated set by construction (their pose is Walking, not Seated).
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
        resolve_characters(&agents, &poses, layout, pack, coffee, now);

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

/// Resolve every character's placement for this tick — the sim half of the
/// old `enqueue_characters`. For each agent it looks up the routed pose from
/// `poses` (the authoritative prepass ran the side effects once) and computes
/// the sprite/anchor/z-key decisions. Returns the placements (paint maps them
/// 1:1 to drawables), the waypoint visitors (for the chitchat venues), and the
/// agents seen carrying coffee this tick.
fn resolve_characters(
    agents: &[AgentSlot],
    poses: &HashMap<AgentId, Option<Pose>>,
    layout: &Layout,
    pack: &Pack,
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
    // The lounge-couch seat waypoints collapse to ONE chitchat venue (keyed on
    // the first couch's index) so the couch hosts a single group conversation
    // like the meeting room, without overloading the meeting-only `room_id`
    // field (which indexes `meeting_rooms`). Pinned by
    // multi_slot_venues_collapse_to_first_of_their_own_kind.
    // The pack's character sprite width (8 for the bundled pack, 10 for the
    // robot pack). All character poses share one width, so resolve it ONCE from
    // a reference pose and center every anchor on it — a non-8-wide pack would
    // otherwise blit ~1px off (the anchors hardcoded 8). Fallback to the bundled
    // default if the pack lacks the reference anim.
    let char_w = pack
        .animation("standing")
        .and_then(|a| a.frames.first())
        .map_or(CHARACTER_SPRITE_W, |f| f.width());
    for (agent_idx, agent) in agents.iter().enumerate() {
        let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
            continue;
        };
        // Look up the pose the authoritative prepass already derived (one
        // derive_with_routing per agent per frame) instead of re-deriving — the
        // prepass ran the advance_wander/walk_path/history side effects once.
        let Some(p) = poses.get(&agent.agent_id).copied().flatten() else {
            continue;
        };
        // The three seated-at-desk arms differ only in anim/frame/glow/sleep-z;
        // everything else (the desk anchor, the breath, the breath-independent
        // z-key, flip/waiting/dust) is identical. One closure builds the
        // `CharacterPlacement` so the arms stay a single delta-then-push line each.
        let seated = |anim_name: &'static str,
                      frame_idx: usize,
                      glow: CharacterGlow,
                      sleep_z_seed: Option<u64>| {
            let anchor_no_breath = seated_anchor(desk, char_w);
            let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
            CharacterPlacement {
                agent_idx,
                // Breath-independent z-key (matches AtWaypoint/AimlessAt): the
                // ±1px breath must not flip sort order against nearby desk decor
                // frame-to-frame.
                anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                anim_name,
                frame_idx,
                anchor,
                flip_x: false,
                glow,
                sleep_z_seed,
                waiting_bubble: false,
                walking_dust_frame: None,
            }
        };
        match p {
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
            Pose::StandingAtDesk => {
                let anchor_no_breath = standing_at_desk_anchor(desk, char_w);
                let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
                let is_waiting = matches!(agent.state, ActivityState::Waiting { .. });
                placements.push(CharacterPlacement {
                    agent_idx,
                    // Breath-independent z-key (matches AtWaypoint/AimlessAt):
                    // the ±1px breath must not flip sort order against nearby
                    // desk decor frame-to-frame.
                    anchor_y: anchor_no_breath.y + WALKING_Y_OFF,
                    anim_name: "standing",
                    frame_idx: 0,
                    anchor,
                    flip_x: false,
                    glow: CharacterGlow::None,
                    sleep_z_seed: None,
                    waiting_bubble: is_waiting,
                    walking_dust_frame: None,
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    use crate::layout::WaypointKind;
                    // Render anchor: the cell the agent occupies. For obstacles
                    // this is the side stand cell (side-aware); for seats it is
                    // `wp.pos` (the sprite sits ON the furniture) — the walk-in
                    // approach cell is resolved separately by `approach_point`.
                    let stand = crate::layout::stand_point(
                        wp_obj.kind,
                        wp_obj.pos,
                        layout.pantry_counter_size(),
                        &layout.walkable,
                        desk,
                        wp_obj.facing,
                        &layout.reachable,
                    );
                    let (anim_name, anchor_base, sprite_h, flip_x) = match kind {
                        WaypointKind::Pantry => (
                            "holding_coffee",
                            waypoint_anchor(stand, char_w),
                            12u16,
                            false,
                        ),
                        // Lounge couch + meeting sofa + head-of-table chair:
                        // the sprite follows the SEATED facing (couch always
                        // North/window → back_couch; the sofa's two seats face
                        // each other across the table; the chair sits in
                        // PROFILE facing the table — SideSeated).
                        // All reuse the seat anchor — pairing the chair with
                        // the stand-era waypoint_anchor left its 10-row seated
                        // sprite hovering 5 rows above the chair body.
                        WaypointKind::Couch
                        | WaypointKind::MeetingSofa
                        | WaypointKind::MeetingChair => {
                            let (anim, flip) = seat_sprite_in_pack(pack, kind, wp_obj.facing);
                            (anim, back_couch_anchor(stand, char_w), 9u16, flip)
                        }
                        // Island stander anchors on the waypoint cell itself.
                        WaypointKind::Island => {
                            let (anim, flip) = seat_sprite_in_pack(pack, kind, wp_obj.facing);
                            (anim, waypoint_anchor(stand, char_w), 12u16, flip)
                        }
                        // PhoneBooth + StandingDesk → agent just stands at the
                        // decor. waypoint_anchor positions them directly above
                        // the decor centre (sprite footprint sits just north
                        // of the decor's centre, head visible above).
                        WaypointKind::PhoneBooth
                        | WaypointKind::StandingDesk
                        | WaypointKind::VendingMachine
                        | WaypointKind::Printer
                        | WaypointKind::SnackShelf => {
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
                            wp_idx: chitchat::venue_wp_idx(kind, wp, &layout.waypoints),
                            agent_id: agent.agent_id,
                            anchor: anchor_no_breath,
                            room_id: wp_obj.room_id,
                        });
                    }
                    let anchor = with_breath(anchor_no_breath, agent.agent_id, now);
                    placements.push(CharacterPlacement {
                        agent_idx,
                        // Breath-independent sort key: a seated occupant must
                        // y-sort identically every frame so the breath ±1px never
                        // flips it under its sofa (the overlap bug). The visual
                        // `anchor` above still breathes; only the z-order is pinned.
                        //
                        // Seats route through `SeatView::z_key_for_seat` — the SAME
                        // key the sit-down/stand-up glide uses, so the agent can't
                        // pop across its furniture's z-key at the walk→seat seam.
                        // (back/front sofa+couch + the head-of-table chair →
                        // pos+2, clearing the chair body; island stander → the plain
                        // feet row, staying BEHIND the island's south-row key —
                        // the bartender occlusion.) Obstacles (pantry/booth/
                        // vending/printer) keep the stand-at-the-approach-cell
                        // key — the agent stands AT them, there is no settle onto
                        // them.
                        anchor_y: match kind {
                            WaypointKind::Couch
                            | WaypointKind::MeetingSofa
                            | WaypointKind::MeetingChair
                            | WaypointKind::Island => {
                                SeatView::of(kind, wp_obj.facing).z_key_for_seat(stand)
                            }
                            _ => anchor_no_breath.y + sprite_h,
                        },
                        anim_name,
                        frame_idx: 0,
                        anchor,
                        flip_x,
                        glow: CharacterGlow::None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    });
                }
            }
            Pose::AimlessAt { dest } => {
                // Breath-independent sort key (like the AtWaypoint arm): the
                // ±1px breath bob must not flicker the z-order frame to frame.
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
                // Exit walks: core sets carrying_coffee=false (no
                // render-side state), but we know from the coffee map.
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
                    settle_seat_view(to, layout).or_else(|| settle_seat_view(from, layout));
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
                } else if carrying_coffee && pack.animation("walking_coffee").is_some() {
                    "walking_coffee"
                } else {
                    "walking"
                };
                placements.push(CharacterPlacement {
                    agent_idx,
                    anchor_y: match settle {
                        Some((_, z_key)) => z_key,
                        None => walker_anchor.y + WALKING_Y_OFF,
                    },
                    anim_name,
                    frame_idx: frame,
                    anchor: walker_anchor,
                    flip_x: flip,
                    glow: CharacterGlow::None,
                    sleep_z_seed: None,
                    waiting_bubble: false,
                    walking_dust_frame: Some(frame),
                });
            }
        }
    }
    // wp_rank's keys ARE this tick's occupied waypoints (every AtWaypoint
    // occupant registers a rank) — no second bookkeeping pass.
    (
        placements,
        waypoint_visitors,
        new_coffee_carriers,
        wp_rank.into_keys().collect(),
    )
}
