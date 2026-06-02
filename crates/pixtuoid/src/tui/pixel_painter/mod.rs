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

use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Rgb, RgbBuffer};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::tui::chitchat::{self, ActiveChitchat, ChitchatBubble};
use crate::tui::floor::LightingState;
use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{z_sort_row, Anchor, Layout, Point, DESK_H, DESK_W};
use crate::tui::motion::MotionState;
use crate::tui::pathfind::Router;
use crate::tui::pet::PetKind;
use crate::tui::pose::{self, Pose};

/// Result of the pure-pixel pass — carries the resolved cat position
/// (for hit-testing), active chitchat bubbles (for widget rendering),
/// and agent ids that were seen carrying coffee this frame (so the
/// caller can persist them into `coffee_holders`).
pub struct PixelPassResult {
    pub pet_pos: Option<(Point, &'static str, PetKind)>,
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
mod palette;

pub(in crate::tui) use anchors::character_anchor;
pub(in crate::tui) use anchors::walking_position;
use anchors::{
    back_couch_anchor, compute_door_frame_idx, seated_anchor, standing_at_desk_anchor,
    walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath, CHARACTER_SPRITE_W,
};
use background::{
    dim_floor_overlay, paint_ceiling_pool, paint_clock, paint_corridor_runner,
    paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel, paint_shadow, time_of_day_look,
};
use drawable::{paint_drawable, pet_position, Drawable, DrawableKind};
use palette::{agent_palette, blend, recolor_frame};

const COFFEE_STEAM_WINDOW_SECS: u64 = 120;
const DOOR_SPRITE_WIDTH: u16 = 16;

// Room-divider frosted-glass partitions. The E-W (horizontal) wall shows its
// face — 6 px tall, kept in sync with `mask.rs` WALL_THICK_H — while the N-S
// (vertical) wall is seen edge-on at 3 px (wider than its 1 px footprint). The
// 2:1 ratio sells the top-down fake-3D. Each strip is a cool gradient (bright
// specular edge → tinted body → soft slate edge, all alpha-composited over
// what's behind so the room glows through) with a brighter seam every
// `GLASS_SEAM_STRIDE` px. The horizontal wall paints in the y-sorted drawable
// pass (so it composites over — frostily occluding — a walker standing behind
// it); the vertical paints in the background.
pub(super) const WALL_THICK_V_PX: u16 = 3; // visual; footprint is 1 px (mask.rs)
                                           // Derived from the core mask const so the visible glass face and the blocked
                                           // ground footprint share a single source of truth (can't drift apart).
pub(super) const WALL_THICK_H_PX: u16 = pixtuoid_core::layout::WALL_THICK_H;
const GLASS_SEAM_STRIDE: u16 = 16;
// The horizontal wall's frosted glass rises this many px NORTH of its walkable
// footprint — a "back cap" giving the wall height. Because the strip is
// y-sorted at its south (front) base, a character standing just north of the
// wall has their feet/legs composited behind this translucent cap (occluded
// behind the glass). The cap is over floor (visual only), not the mask.
//
// Derived from WALL_THICK_H_PX (the E-W wall face height) so the cap reaches
// into the legs of a walker at the northmost walkable row (footprint top `W`
// minus OBSTACLE_PAD+1 = `W-3`): the 12px sprite spans `W-15..W-3`, the cap
// covers `W-6..W-1`, so the bottom ~4px (feet + lower legs) read behind the
// pane. At the old value of 3 only the single feet row was grazed. Derived (not
// a bare 6) so retuning the wall face thickness moves the cap with it.
const GLASS_CAP_PX: u16 = WALL_THICK_H_PX;

fn glass_tones(theme: &crate::tui::theme::Theme) -> (Rgb, Rgb, Rgb) {
    let tl = theme.office.room_wall_trim_light;
    (
        Rgb(
            tl.0.saturating_add(125),
            tl.1.saturating_add(135),
            tl.2.saturating_add(124),
        ),
        Rgb(
            tl.0.saturating_add(70),
            tl.1.saturating_add(100),
            tl.2.saturating_add(116),
        ),
        Rgb(
            tl.0.saturating_add(18),
            tl.1.saturating_add(52),
            tl.2.saturating_add(86),
        ),
    )
}

/// Stitch a vertical (N-S) wall segment's `[y_top, y_bot]` to its joints — the
/// terminal-agnostic layout emits raw geometry; the render thicknesses/offsets
/// that open the gaps live here:
///   • Top: a segment starting at `top_margin` abuts the north wall band, which
///     ends 4 px higher at `top_wall_h` — raise it so no floor shows between
///     window and wall. A segment sitting just below a horizontal wall (the
///     dual-meeting layout offsets its lower segment ~6 px to clear the cross
///     wall — see `compute_room_walls`) is bridged up to meet it.
///   • Bottom: where the vertical meets a horizontal wall, extend it down by
///     the horizontal's thickness to fill the inside corner (else its right
///     columns leave an L-notch beside the horizontal run).
fn stitch_vertical_wall(
    start_y: u16,
    end_y: u16,
    top_margin: u16,
    top_wall_h: u16,
    h_rows: &[u16],
) -> (u16, u16) {
    let y_top = if start_y == top_margin {
        top_wall_h
    } else if let Some(&hr) = h_rows
        .iter()
        .find(|&&hr| hr < start_y && start_y - hr <= WALL_THICK_H_PX + 2)
    {
        hr
    } else {
        start_y
    };
    let y_bot = if h_rows.contains(&end_y) {
        end_y + (WALL_THICK_H_PX - 1)
    } else {
        end_y
    };
    (y_top, y_bot)
}

fn glass_over(buf: &RgbBuffer, x: u16, y: u16, g: Rgb, a: f32) -> Rgb {
    let b = buf.get(x, y);
    Rgb(blend(b.0, g.0, a), blend(b.1, g.1, a), blend(b.2, g.2, a))
}

/// Paint a horizontal (E-W) frosted-glass wall strip: lit top edge → body →
/// soft bottom edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_h(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x0: u16,
    x1: u16,
    y_top: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    // The strip spans the back cap (rising north of the footprint) + the
    // 6 px face. Row 0 = lit far/top edge (north), last row = soft front base.
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    let rows = GLASS_CAP_PX + WALL_THICK_H_PX;
    for x in x0..=x1.min(bw.saturating_sub(1)) {
        let seam = (x - x0) % GLASS_SEAM_STRIDE == 0;
        for i in 0..rows {
            let y = cap_top + i;
            if y >= bh {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.55)
            } else if i == 0 {
                (hi, 0.82)
            } else if i == rows - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.58)
            };
            let color = glass_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Paint a vertical (N-S) frosted-glass wall strip: lit left edge → body →
/// soft right edge, seam glints every `GLASS_SEAM_STRIDE` px.
fn paint_glass_wall_v(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x_left: u16,
    y_top: u16,
    y_bot: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    for y in y_top..=y_bot.min(bh.saturating_sub(1)) {
        let seam = (y - y_top) % GLASS_SEAM_STRIDE == 0;
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x >= bw {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.6)
            } else if dx == 0 {
                (hi, 0.85)
            } else if dx == WALL_THICK_V_PX - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.6)
            };
            let color = glass_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

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
            .1,
    )
}

/// Extrude `frame`'s top edge `rows` px NORTH of its blit origin `(sx, sy)` into
/// an opaque back face. Tall free-standing objects are reachable from the north
/// and a feet-anchored character extends UP/north, so a walker standing behind
/// one would otherwise float above it with a gap. The object's drawable y-sorts
/// at its south base, so this band paints AFTER — on top of — anyone behind it,
/// giving ¾-view depth (same idea as the glass wall's `GLASS_CAP_PX`). `rows` is
/// the per-object back-cap DEPTH from `FurnitureDef::occludes_behind`, so a short
/// printer hides less than a tall booth. Each column repeats its topmost opaque
/// sprite color, darkened with distance; transparent-top columns are skipped,
/// preserving the silhouette. Render-only — emit inside the object's drawable.
pub(super) fn paint_furniture_back(
    buf: &mut RgbBuffer,
    frame: &Frame,
    sx: u16,
    sy: u16,
    rows: u16,
) {
    paint_furniture_back_impl(buf, frame, sx, sy, rows, None);
}

/// Like [`paint_furniture_back`] but paints a single uniform `tint` (still
/// darkened with distance) for every opaque column rather than repeating each
/// column's own top pixel. Wide multi-material objects — the pantry counter —
/// have a top row that alternates materials (white fridge + brown counter), so
/// the per-column form smears it into vertical streaks; one receding shade
/// reads as a single back surface. Gives the same clean over-the-body occlusion
/// the couch gives its sitter, for an object a character stands *behind*.
pub(super) fn paint_furniture_back_uniform(
    buf: &mut RgbBuffer,
    frame: &Frame,
    sx: u16,
    sy: u16,
    tint: Rgb,
    rows: u16,
) {
    paint_furniture_back_impl(buf, frame, sx, sy, rows, Some(tint));
}

fn paint_furniture_back_impl(
    buf: &mut RgbBuffer,
    frame: &Frame,
    sx: u16,
    sy: u16,
    requested_rows: u16,
    uniform: Option<Rgb>,
) {
    if sy == 0 {
        return;
    }
    let rows = requested_rows.min(sy);
    let w = frame.width as usize;
    let denom = (rows.max(2) - 1) as f32;
    for fx in 0..frame.width {
        // A fully-transparent column has no back face — preserves the
        // silhouette (and the counter's inter-appliance gaps).
        let Some(col_top) =
            (0..frame.height).find_map(|fy| frame.pixels[(fy as usize) * w + fx as usize])
        else {
            continue;
        };
        let top = uniform.unwrap_or(col_top);
        let x = sx.saturating_add(fx);
        if x >= buf.width {
            continue;
        }
        for i in 0..rows {
            // i = 0 is the northmost (most-receded → darkest) row; the last row
            // sits flush against the sprite top and keeps the full color.
            let y = sy - rows + i;
            let f = 0.55 + 0.45 * (i as f32 / denom);
            buf.put(
                x,
                y,
                Rgb(
                    (top.0 as f32 * f) as u8,
                    (top.1 as f32 * f) as u8,
                    (top.2 as f32 * f) as u8,
                ),
            );
        }
    }
}

/// Solid-rect variant of [`paint_furniture_back`] for procedurally-drawn boxes
/// (vending machine, printer) that have no `Frame` to read a silhouette from.
/// Fills `width` columns from `sx`, `rows` px north of `sy`, with `tint`
/// darkened by distance — the same receding back face. `rows` (the per-object
/// back-cap DEPTH) and whether to call this at all both come from the
/// FurnitureDef table's `occludes_behind` — one source of truth, no allowlist.
pub(super) fn paint_furniture_back_rect(
    buf: &mut RgbBuffer,
    sx: u16,
    width: u16,
    sy: u16,
    tint: Rgb,
    rows: u16,
) {
    if sy == 0 {
        return;
    }
    let rows = rows.min(sy);
    let denom = (rows.max(2) - 1) as f32;
    for fx in 0..width {
        let x = sx.saturating_add(fx);
        if x >= buf.width {
            continue;
        }
        for i in 0..rows {
            let y = sy - rows + i;
            let f = 0.55 + 0.45 * (i as f32 / denom);
            buf.put(
                x,
                y,
                Rgb(
                    (tint.0 as f32 * f) as u8,
                    (tint.1 as f32 * f) as u8,
                    (tint.2 as f32 * f) as u8,
                ),
            );
        }
    }
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
    pub floor_pet_kind: Option<PetKind>,
    pub chitchat_state: &'a mut HashMap<crate::tui::chitchat::VenueKey, ActiveChitchat>,
    pub coffee_holders: &'a std::collections::HashSet<pixtuoid_core::AgentId>,
    pub coffee_fetched_at: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub light: &'a mut crate::tui::floor::LightingState,
    /// When set, composite the walkable / approach / route debug layer over the
    /// finished scene (the live `w` toggle). Off by default; transient.
    pub debug_walkable: bool,
}

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving. `glow_tint` should carry the tool-derived monitor color
/// when the character is at a lit screen (SeatedTyping); tints the skin
/// toward that color so the eye reads "the monitor is lighting their face."
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    glow_tint: Option<Rgb>,
    cache: &mut FrameCache,
) {
    let Some(anim) = pack.animation(anim_name) else {
        return;
    };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else {
        return;
    };
    let cached = cache.get_or_make(
        agent.agent_id,
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
        || {
            let pal = agent_palette(&pack.palette, agent, glow_tint);
            let recolored = recolor_frame(frame, &pal, &pack.palette);
            if flip_x {
                recolored.mirror_horizontal()
            } else {
                recolored
            }
        },
    );
    blit_frame(cached, anchor.x, anchor.y, buf);
}

/// Sprite name + horizontal flip for an agent SEATED at a seat slot, by its
/// SEATED facing (the `facing` field = which way the sitter LOOKS, decoupled
/// from the approach side). A `Facing::North` sitter shows its back (`back_couch`):
/// the lounge couch (always looks at the window/North) and the south-side meeting
/// sofa; other meeting-sofa seats face the viewer across the table (front
/// `seated`); a meeting stand faces inward (west stander marked `Facing::East` is
/// mirrored). Extracted so the facing→sprite mapping is unit-testable.
pub(super) fn seat_sprite(
    kind: crate::tui::layout::WaypointKind,
    facing: crate::tui::layout::Facing,
) -> (&'static str, bool) {
    SeatView::of(kind, facing).seated_sprite()
}

/// The single orientation a seat occupant is shown in — the ONE source BOTH the
/// seated render (`AtWaypoint`, via [`seat_sprite`]) and the sit-down WALK glide
/// onto the seat derive from, so the two can never disagree.
///
/// This is the data-model fix for the recurring "sit facing the wrong way then
/// snap" bug. The two renders used to compute facing independently — the seated
/// sprite from the seat's `facing` field, the glide from the travel direction —
/// and disagreed whenever a seat faces away from its open approach side. A
/// window-facing (`North`) seat (lounge couch AND the south-of-table meeting
/// sofa) is reached from the north, but its foot-cell is pinned SOUTH
/// (`seated_foot_cell` = `pos + (WALKING_Y_OFF − SEAT_RENDER_Y_OFF)`, fixed by
/// pop-free head-alignment); so the settle travels south, the directional walk
/// rule rendered a FRONT walk, and the agent sat facing the camera for ~1s before
/// snapping to `back_couch` at `AtWaypoint`.
///
/// Routing both renders through `SeatView` makes the disagreement structurally
/// impossible: a new seatable furniture picks a view here ONCE (or falls through
/// to the upright default) and the seated sprite, the flip, and the sit-down
/// glide all follow — the bug cannot reappear for a future seat. Sprite names
/// stay in the painter because pixtuoid-core forbids terminal/pack deps; the
/// per-instance `facing` is the core data this projects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeatView {
    /// Faces the camera (south) — front `seated` / `walking`.
    Front,
    /// Faces away (the window / back wall) — `back_couch` / `walking_back`.
    Back,
    /// Faces sideways; `flip` mirrors east↔west.
    Side { flip: bool },
}

impl SeatView {
    /// The view a `kind` occupant looks in, from its seat `facing`. The ONE place
    /// a seat's orientation is decided — extend HERE to add a seatable furniture.
    fn of(kind: crate::tui::layout::WaypointKind, facing: crate::tui::layout::Facing) -> Self {
        use crate::tui::layout::{Facing, WaypointKind};
        match kind {
            // Couch + sofa: North looks at the window/back wall (back view); the
            // other seats face the viewer across the table.
            WaypointKind::Couch | WaypointKind::MeetingSofa => match facing {
                Facing::North => SeatView::Back,
                _ => SeatView::Front,
            },
            // Stand beside the table, facing inward; west stand marked East.
            WaypointKind::MeetingStand => SeatView::Side {
                flip: matches!(facing, Facing::East),
            },
            // Not seat slots — the caller dispatches these directly (they never
            // reach a seated render through SeatView); upright is the safe default.
            // Listed EXPLICITLY (no `_`) so a new WaypointKind is a compile error
            // HERE, forcing a deliberate decision instead of silently rendering as
            // a stander. The totality-guard test still pins the seat-kind set.
            WaypointKind::Pantry
            | WaypointKind::PhoneBooth
            | WaypointKind::StandingDesk
            | WaypointKind::VendingMachine
            | WaypointKind::Printer => SeatView::Side { flip: false },
        }
    }

    /// Sprite + horizontal flip for the SEATED / standing render (`AtWaypoint`).
    fn seated_sprite(self) -> (&'static str, bool) {
        match self {
            SeatView::Front => ("seated", false),
            SeatView::Back => ("back_couch", false),
            SeatView::Side { flip } => ("standing", flip),
        }
    }

    /// `(going_back, flip)` for the sit-down WALK glide that settles onto the
    /// seat — the SAME orientation as [`seated_sprite`](Self::seated_sprite), so
    /// the sit-down never faces the wrong way (overrides the travel-direction
    /// rule for this terminal segment).
    fn settle_walk(self) -> (bool, bool) {
        match self {
            SeatView::Front => (false, false),
            SeatView::Back => (true, false),
            SeatView::Side { flip } => (false, flip),
        }
    }

    /// The y-sort key for an agent occupying this seat at waypoint centre
    /// `wp_pos` — used BOTH for the settled `AtWaypoint` render AND for the
    /// sit-down / stand-up WALK glide. Using one key for the whole sit arc is the
    /// z-sort half of the single-source fix: the settle is a `Walking` pose whose
    /// natural z-key is the foot position (`pos.y`), which glides from the
    /// approach point down to `seated_foot_cell = pos+5` and so CROSSES the
    /// furniture's own z-key (`pos+3` for a couch/back sofa) for a frame or two
    /// before snapping to the seated key (`pos+2`) — the agent pops in front of
    /// the sofa mid-glide, then jumps behind it. Pinning the glide to this stable
    /// key keeps the agent on the correct side of its furniture for the entire
    /// arc. Values match the historical `AtWaypoint` formulas exactly (back/front:
    /// `back_couch_anchor.y + 9 = pos+2`; side/stand: `waypoint_anchor.y + 12 + 3
    /// = pos+3`, the +3 clearing the meeting-table z-key), so the seated render is
    /// unchanged — only the glide is pulled into agreement with it.
    fn z_key_for_seat(self, wp_pos: Point) -> u16 {
        match self {
            // Behind a couch/sofa back (furniture sorts at pos+3) or tied with a
            // front sofa (pos+2, insertion order puts the sitter on top).
            SeatView::Front | SeatView::Back => wp_pos.y + 2,
            // Stand clears the meeting table (table.y+2) it stands beside.
            SeatView::Side { .. } => wp_pos.y + 3,
        }
    }
}

/// The seated [`SeatView`] (for the glide facing) and the seat's stable z-key
/// for the seat whose settle foot-cell is `cell`, or `None` if `cell` is not a
/// seat foot-cell. The caller passes the glide's `to` (sit-down: settling ONTO
/// the seat) and/or `from` (stand-up: rising OFF it) — either endpoint landing
/// on a foot-cell means the agent is on the sit arc and must render in the
/// seat's view and at the seat's stable z-key, not the travel-direction /
/// foot-position values.
///
/// Covers ALL seatables:
/// - Wander seats — any `layout.waypoints` entry whose furniture has a
///   `seated_foot_cell`; the view comes from [`SeatView::of`], the z-key from
///   [`SeatView::z_key_for_seat`].
/// - The home desk — `layout.home_desks` are NOT waypoints, but the chair
///   (`seated_foot_cell(Desk)` = `desk_walk_anchor`) is a settle target too once
///   the desk's arrival glides onto it (see `pose::desk_approach_cell`). The desk
///   sitter faces the camera (front) and renders at the desk's seated z-key
///   (`seated_anchor.y + 12 = desk.y + 4`, below the desk furniture's `desk.y+8`),
///   so the glide stays behind the desk — no front-cross.
fn settle_seat_view(cell: Point, layout: &Layout) -> Option<(SeatView, u16)> {
    use pixtuoid_core::layout::{seated_foot_cell, Furniture};
    layout
        .waypoints
        .iter()
        .find_map(|w| {
            (seated_foot_cell(w.kind.furniture(), w.pos) == Some(cell)).then(|| {
                let view = SeatView::of(w.kind, w.facing);
                (view, view.z_key_for_seat(w.pos))
            })
        })
        .or_else(|| {
            layout.home_desks.iter().find_map(|&desk| {
                (seated_foot_cell(Furniture::Desk, desk) == Some(cell))
                    // == the seated arms' `anchor_no_breath.y + 12` (= desk.y+4);
                    // pinned by `desk_settle_z_key_matches_the_seated_arm`.
                    .then_some((SeatView::Front, desk.y + DESK_SEAT_Z_OFF))
            })
        })
}

/// The home-desk sitter's z-key offset south of `desk`: `seated_anchor.y(=desk.y
/// − 8) + sprite_h(12) = desk.y + 4`. Below the desk furniture key (`desk.y + 8`)
/// so the sitter and its sit-down glide always sort behind the desk monitor.
const DESK_SEAT_Z_OFF: u16 = 4;

pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let agents: Vec<_> = ctx.scene.agents.values().cloned().collect();
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;
    let mut resolved_pet_pos: Option<(Point, &'static str, PetKind)> = None;
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
    let door_x_range = ctx.layout.door.map(|d| (d.x, d.x + DOOR_SPRITE_WIDTH));
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
    let pool_strength = (0.15 + 0.30 * look.darkness) * indoor_scale;
    for desk in &ctx.layout.home_desks {
        paint_ceiling_pool(
            ctx.buf,
            desk.x + DESK_W / 2,
            desk.y.saturating_sub(2),
            10,
            5,
            pool_strength,
            ctx.theme,
        );
    }
    // Two ceiling fluorescents over the pantry and a third over the
    // corridor so the floor is lit consistently with the lounge_band gone.
    if let Some(pr) = ctx.layout.pantry_room {
        paint_ceiling_pool(
            ctx.buf,
            pr.x + pr.width / 2,
            pr.y + pr.height / 2,
            12,
            6,
            pool_strength,
            ctx.theme,
        );
    }
    if let Some(corridor) = ctx.layout.corridor {
        paint_ceiling_pool(
            ctx.buf,
            corridor.x + corridor.width / 2,
            corridor.y + corridor.height / 2,
            14,
            5,
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
        .filter(|(s, e)| s.y == e.y)
        .map(|(s, _)| s.y)
        .collect();
    for (start, end) in &ctx.layout.room_walls {
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
        let wall_color = ctx.theme.office.room_wall_trim_dark;
        let accent = ctx.theme.furniture.rug_accent;

        // Notice board on the south wall (8×5)
        if mr.height > 20 && mr.width > 15 {
            let bx = mr.x + 4;
            let by = mr.y + mr.height - 8;
            for dy in 0..5u16 {
                for dx in 0..8u16 {
                    let px = bx + dx;
                    let py = by + dy;
                    if px < buf_w && py < buf_h {
                        let on_edge = dx == 0 || dx == 7 || dy == 0 || dy == 4;
                        ctx.buf
                            .put(px, py, if on_edge { wall_color } else { accent });
                    }
                }
            }
        }

        // Coat rack is now a y-sorted DrawableKind::CoatRack (pushed in the
        // drawable pass) so characters in front occlude it / behind it are
        // occluded — was painted here in the background pass, always under
        // every character.

        // Small doormat at the meeting room entrance (on the cubicle side)
        if mr.width > 10 {
            let mat_x = mr.x + mr.width;
            let mat_y = mr.y + mr.height / 2 - 2;
            let mat_color = ctx.theme.furniture.rug_trim;
            let mat_accent = ctx.theme.furniture.rug_field;
            for dy in 0..5u16 {
                for dx in 0..4u16 {
                    let px = mat_x + dx + 1;
                    let py = mat_y + dy;
                    if px < buf_w && py < buf_h {
                        let on_border = dx == 0 || dx == 3 || dy == 0 || dy == 4;
                        ctx.buf
                            .put(px, py, if on_border { mat_color } else { mat_accent });
                    }
                }
            }
        }
    }
    if let Some(pr) = ctx.layout.pantry_room {
        let cooler_body = ctx.theme.office.building_light;
        let cooler_water = Rgb(100, 180, 230);

        // Water cooler near the pantry wall (3×6)
        if pr.height > 25 && pr.width > 12 {
            let wx = pr.x + pr.width - 6;
            let wy = pr.y + 8;
            for dy in 0..6u16 {
                for dx in 0..3u16 {
                    let px = wx + dx;
                    let py = wy + dy;
                    if px < buf_w && py < buf_h {
                        let color = if dy < 2 { cooler_water } else { cooler_body };
                        ctx.buf.put(px, py, color);
                    }
                }
            }
        }

        // Trash bin near the pantry counter (4×5 with visible bag liner)
        if pr.height > 20 {
            let tx = pr.x + 3;
            let ty = pr.y + pr.height - 14;
            let bin_outer = Rgb(70, 70, 78);
            let bin_rim = Rgb(100, 100, 108);
            let bag_liner = Rgb(200, 200, 210);
            let bag_fill = Rgb(160, 160, 170);
            for dy in 0..5u16 {
                for dx in 0..4u16 {
                    let px = tx + dx;
                    let py = ty + dy;
                    if px < buf_w && py < buf_h {
                        let color = if dy == 0 {
                            // Rim row — lighter metal rim with bag liner peek
                            if dx == 0 || dx == 3 {
                                bin_rim
                            } else {
                                bag_liner
                            }
                        } else if dy == 1 {
                            // Bag liner visible
                            if dx == 0 || dx == 3 {
                                bin_outer
                            } else {
                                bag_fill
                            }
                        } else {
                            // Bin body
                            bin_outer
                        };
                        ctx.buf.put(px, py, color);
                    }
                }
            }
        }
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
            desk.x + DESK_W / 2,
            desk.y + 7,
            DESK_W / 2 + 1,
            3,
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
            wp.pos.x,
            wp.pos.y + 2,
            7,
            2,
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
            wp.pos.x,
            wp.pos.y + 1,
            5,
            1,
            shadow_strength,
            ctx.theme,
        );
    }
    if let Some(center) = ctx.layout.couch_sprite_center {
        paint_shadow(
            ctx.buf,
            center.x,
            center.y + 2,
            7,
            2,
            shadow_strength,
            ctx.theme,
        );
    }
    for (kind, p) in &ctx.layout.plants {
        // Shadow sits under the sprite's south row — same offset the z-anchor
        // uses, off the same height (Succulent/Flower were floating at a fixed
        // +3 that only suited the taller Ficus/Tall).
        let cy = p.y
            + center_pin_south_offset(crate::tui::layout::furniture_def(kind.furniture()).visual.1);
        paint_shadow(ctx.buf, p.x, cy, 3, 1, shadow_strength, ctx.theme);
    }
    if let Some(lamp) = ctx.layout.floor_lamp {
        paint_shadow(
            ctx.buf,
            lamp.x,
            lamp.y + floor_lamp_south_offset(), // flush with the lamp base (sprite south)
            2,
            1,
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
                ctx.router,
                ctx.overlay,
                ctx.history,
                ctx.motion,
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
        let (desk_fp_w, desk_fp_h) = crate::tui::layout::desk_furniture_def()
            .footprint
            .unwrap_or((DESK_W, DESK_H));
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
                    .1,
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
                    .1,
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
                    .1,
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
                    .1,
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
                    .1,
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
        // `pos`). h is the footprint height (= sprite height for these), from
        // furniture_def — no drift.
        let footprint_h = furniture_def(wp.kind.furniture())
            .footprint
            .map_or(0, |(_, h)| h);
        match wp.kind {
            // Rendered once via `couch_sprite_center` above (3 seats, 1 sprite).
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let (cw, ch) = ctx.layout.pantry_counter_size; // runtime-sized
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
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, footprint_h),
                    kind: DrawableKind::VendingMachine { pos: wp.pos },
                });
            }
            WaypointKind::Printer => {
                drawables.push(Drawable {
                    anchor_y: z_sort_row(Anchor::Center, wp.pos, footprint_h),
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
    for (kind, pos) in &ctx.layout.pod_decor {
        // Visual sprite height from the one furniture table (the mask reads the
        // separate `footprint` off the same row — so a tall plant's canopy can
        // sort correctly without blocking the aisle).
        let (_, h) = crate::tui::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::Center, *pos, h),
            kind: DrawableKind::PodDecorItem {
                kind: *kind,
                pos: *pos,
            },
        });
    }

    // Plants — center-pinned; z-key = sprite south row. Height is the single
    // source `furniture_def(kind.furniture()).visual.1` (was a parallel fudged
    // match that drifted: it over-shot Flower/Succulent by one and faked Tall
    // via `9` instead of `(10-1)/2`). The drop-shadow uses the same offset off
    // the same height, so the two can't diverge.
    for (kind, p) in &ctx.layout.plants {
        drawables.push(Drawable {
            anchor_y: z_sort_row(
                Anchor::Center,
                *p,
                crate::tui::layout::furniture_def(kind.furniture()).visual.1,
            ),
            kind: DrawableKind::Plant {
                kind: *kind,
                pos: *p,
            },
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
                kind: DrawableKind::CoatRack { cx, cy },
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
            anchor_y: door_pos.y + 14,
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
    for (kind, pos) in &ctx.layout.wall_decor {
        let (_, h) = crate::tui::layout::furniture_def(kind.furniture()).visual;
        drawables.push(Drawable {
            anchor_y: z_sort_row(Anchor::TopLeft, *pos, h),
            kind: DrawableKind::WallDecor {
                kind: *kind,
                pos: *pos,
            },
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

    if let Some(kind) = ctx.floor_pet_kind {
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
            resolved_pet_pos = Some((pos, anim_name, kind));
            drawables.push(Drawable {
                // center-pinned pet sprite (walk/sit 8×6, 6×6) south row =
                // pos.y + h/2 - 1 = pos.y + 2 (was +3, one row past).
                anchor_y: pos.y + 2,
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
            ctx.router,
            ctx.overlay,
            ctx.history,
            ctx.motion,
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
                    anchor_y: anchor_no_breath.y + 12,
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
                    anchor_y: anchor_no_breath.y + 12,
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
                    anchor_y: anchor_no_breath.y + 12,
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
                    anchor_y: anchor_no_breath.y + 12,
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
                    anchor_y: anchor_no_breath.y + 12,
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
                        None => walker_anchor.y + 12,
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
    for (start, end) in &ctx.layout.room_walls {
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
mod tests {
    use super::*;
    use pixtuoid_core::sprite::{Frame, Palette};
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn stitch_vertical_wall_connects_each_joint() {
        let top_margin = 48u16;
        let top_wall_h = top_margin - 4; // 44
        let h_y = 90u16; // a horizontal divider row
        let h_rows = [h_y];

        // Top joint: a segment starting at top_margin rises to the window band.
        let (yt, _) = stitch_vertical_wall(top_margin, 70, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            yt, top_wall_h,
            "top segment should connect up to the window band"
        );

        // Corner joint: a segment ending on the horizontal row extends down by
        // the horizontal's thickness to fill the inside corner.
        let (_, yb) = stitch_vertical_wall(60, h_y, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            yb,
            h_y + (WALL_THICK_H_PX - 1),
            "bottom should fill the corner"
        );

        // Bridge-up joint (the dual-meeting case): a segment starting ~6 px
        // below the cross wall is bridged up to meet it. This branch only fires
        // on variant-2 floors, so it has no end-to-end render guard.
        let (yt2, _) = stitch_vertical_wall(h_y + 6, 120, top_margin, top_wall_h, &h_rows);
        assert_eq!(yt2, h_y, "lower segment should bridge up to the cross wall");

        // No false bridge: a segment well below the tolerance stays put, and a
        // segment with no joints is returned unchanged.
        let (yt3, yb3) = stitch_vertical_wall(h_y + 20, 130, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            (yt3, yb3),
            (h_y + 20, 130),
            "distant segment must not bridge"
        );
        let (yt4, yb4) = stitch_vertical_wall(60, 80, top_margin, top_wall_h, &[]);
        assert_eq!((yt4, yb4), (60, 80), "no joints → unchanged");
    }

    // The vertical-wall top raise lives in TWO crates — the renderer
    // (stitch_vertical_wall, here) and the mask (build_walkable_mask, core).
    // The mask raises a top_margin-rooted segment to
    // `top_margin - WALL_BAND_TO_TOP_MARGIN`; the renderer raises it to
    // `top_wall_h`, which the binary derives from the SAME const. If they ever
    // disagree a walkable slot opens at the wall top (the regression
    // `vertical_wall_is_impassable_except_through_the_door` guards). Extracting
    // the rule into core would drag tui wall constants across the crate
    // boundary (invariant #1); this test pins the agreement instead.
    #[test]
    fn vertical_wall_top_raise_agrees_between_renderer_and_mask() {
        let top_margin = 48u16;
        let tbm = pixtuoid_core::layout::WALL_BAND_TO_TOP_MARGIN;
        let top_wall_h = top_margin - tbm; // what the binary passes the renderer
        let mask_raise = top_margin.saturating_sub(tbm); // what mask.rs computes
        let (renderer_raise, _) = stitch_vertical_wall(top_margin, 90, top_margin, top_wall_h, &[]);
        assert_eq!(
            renderer_raise, mask_raise,
            "renderer + mask must raise the vertical wall top to the same row"
        );
    }

    #[test]
    fn glass_wall_h_back_cap_composites_over_a_character_behind_it() {
        // Occlusion: the horizontal wall's frosted glass rises GLASS_CAP_PX
        // north of its footprint, y-sorted at the south base — so a character
        // standing just NORTH of the wall (drawn earlier) is composited over
        // by the translucent glass. Stand in for that character with a vivid
        // warm pixel inside the cap band; the glass must shift it toward the
        // cool tone (red drops, blue rises) rather than leave it untouched.
        let theme = crate::tui::theme::theme_by_name("normal").expect("theme");
        let y_top = 20u16;
        // Place the stand-in at the REAL northmost row a routed walker's feet
        // can reach: footprint top `y_top` minus (OBSTACLE_PAD_PX=2 + 1) = the
        // first walkable row north of the wall. With GLASS_CAP_PX=6 the cap
        // (rows y_top-6..y_top-1) covers this row, so a walker's feet/lower legs
        // composite behind the glass. (The old test used y_top-2, a row inside
        // the blocked footprint+pad band that no walker ever occupies.)
        let cap_row = y_top - 3;
        let character = Rgb(220, 40, 40);
        let mut buf = RgbBuffer::filled(48, 48, Rgb(150, 110, 72)); // carpet
        for x in 4..20 {
            buf.put(x, cap_row, character);
        }
        paint_glass_wall_h(&mut buf, theme, 0, 47, y_top);
        let after = buf.get(8, cap_row);
        assert_ne!(after, character, "glass must composite over the character");
        assert!(
            after.0 < character.0 && after.2 > character.2,
            "frosted glass should cool the occluded pixel (red↓ blue↑): {after:?}"
        );
    }

    /// Tiny solid frame for back-face tests: `w`×`h`, every pixel opaque `c`.
    fn solid_frame(w: u16, h: u16, c: Rgb) -> Frame {
        Frame {
            width: w,
            height: h,
            pixels: vec![Some(c); (w as usize) * (h as usize)],
        }
    }

    #[test]
    fn furniture_back_occludes_a_character_behind_it() {
        // A tall pod's back face rises `rows` (its occludes_behind depth) north
        // of its sprite top, in a darkened shade of the column's top color — so a
        // character standing just north (drawn earlier) is painted over.
        let blue = Rgb(40, 60, 200);
        let frame = solid_frame(6, 8, blue);
        let (sx, sy) = (10u16, 20u16);
        let character = Rgb(220, 40, 40);
        let mut buf = RgbBuffer::filled(48, 48, Rgb(150, 110, 72));
        // Stand-in pixel one row north of the sprite top — inside the band.
        let row = sy - 1;
        buf.put(sx + 2, row, character);
        paint_furniture_back(&mut buf, &frame, sx, sy, 5);
        let after = buf.get(sx + 2, row);
        assert_ne!(after, character, "back face must paint over the character");
        // Shade of the (blue) sprite top, not the warm character/floor: blue
        // channel dominates and red is gone.
        assert!(
            after.2 > after.0 && after.0 < character.0,
            "occluded pixel should take the pod's cool shade: {after:?}"
        );
    }

    #[test]
    fn furniture_back_skips_transparent_columns() {
        // A fully-transparent column must NOT extrude (preserve silhouette /
        // avoid smearing floor north of the object's clipped corners).
        let c = Rgb(80, 80, 90);
        let mut frame = solid_frame(4, 6, c);
        for fy in 0..frame.height {
            frame.pixels[(fy as usize) * 4] = None; // column 0 transparent
        }
        let floor = Rgb(150, 110, 72);
        let (sx, sy) = (10u16, 20u16);
        let mut buf = RgbBuffer::filled(48, 48, floor);
        paint_furniture_back(&mut buf, &frame, sx, sy, 5);
        assert_eq!(
            buf.get(sx, sy - 1),
            floor,
            "transparent column must leave floor untouched"
        );
        assert_ne!(
            buf.get(sx + 1, sy - 1),
            floor,
            "opaque column must paint its back face"
        );
    }

    #[test]
    fn seat_sprite_maps_facing_to_sprite_and_flip() {
        use crate::tui::layout::{Facing, WaypointKind};
        // Lounge couch always looks at the window (Facing::North) → back view.
        assert_eq!(
            seat_sprite(WaypointKind::Couch, Facing::North),
            ("back_couch", false),
            "couch's seated facing is North (window) → back_couch, same path as the sofa"
        );
        // North-side sofa seat faces away → back view, no flip.
        assert_eq!(
            seat_sprite(WaypointKind::MeetingSofa, Facing::North),
            ("back_couch", false)
        );
        // South-side sofa seat faces the viewer → front seated, no flip.
        assert_eq!(
            seat_sprite(WaypointKind::MeetingSofa, Facing::South),
            ("seated", false)
        );
        // West stand (layout marks it Facing::East) mirrors toward the table.
        assert_eq!(
            seat_sprite(WaypointKind::MeetingStand, Facing::East),
            ("standing", true)
        );
        // East stand (Facing::West) is unmirrored.
        assert_eq!(
            seat_sprite(WaypointKind::MeetingStand, Facing::West),
            ("standing", false)
        );
    }

    fn make_slot(id: pixtuoid_core::AgentId, state: ActivityState) -> AgentSlot {
        let now = SystemTime::UNIX_EPOCH;
        AgentSlot {
            agent_id: id,
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/x").as_path()),
            label: Arc::from("x"),
            state,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,

            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        }
    }

    fn base_palette() -> Palette {
        let mut p = Palette::new();
        p.insert('B', Some(Rgb(10, 20, 30))); // shirt
        p.insert('H', Some(Rgb(40, 50, 60))); // hair
        p.insert('S', Some(Rgb(70, 80, 90))); // skin
        p.insert('X', Some(Rgb(99, 99, 99))); // unrelated key
        p
    }

    #[test]
    fn agent_palette_is_deterministic_per_id() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let a = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        let b = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        assert_eq!(a.get('B'), b.get('B'));
        assert_eq!(a.get('H'), b.get('H'));
        assert_eq!(a.get('S'), b.get('S'));
    }

    #[test]
    fn agent_palette_overrides_only_bhs_keys() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let p = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        // X is not a recolor target — must pass through unchanged.
        assert_eq!(p.get('X'), Some(Some(Rgb(99, 99, 99))));
        // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
        // unlikely to be in any preset, so they should differ.
        assert_ne!(p.get('B'), Some(Some(Rgb(10, 20, 30))));
        assert_ne!(p.get('H'), Some(Some(Rgb(40, 50, 60))));
        assert_ne!(p.get('S'), Some(Some(Rgb(70, 80, 90))));
    }

    #[test]
    fn agent_palette_glow_tint_shifts_skin_toward_given_color() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let slot = make_slot(id, ActivityState::Idle);
        let unlit = agent_palette(&base, &slot, None);
        let green_glow = agent_palette(&base, &slot, Some(Rgb(140, 240, 170)));
        let blue_glow = agent_palette(&base, &slot, Some(Rgb(100, 160, 255)));
        // Shirt / hair / pants are unaffected by glow.
        assert_eq!(unlit.get('B'), green_glow.get('B'));
        assert_eq!(unlit.get('H'), green_glow.get('H'));
        assert_eq!(unlit.get('P'), green_glow.get('P'));
        // Green glow pushes skin's green channel up.
        let (Some(Some(Rgb(_, ug, _))), Some(Some(Rgb(_, gg, _)))) =
            (unlit.get('S'), green_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            gg > ug,
            "green glow should push skin green (lit={gg}, unlit={ug})"
        );
        // Blue glow pushes skin's blue channel up.
        let (Some(Some(Rgb(_, _, ub))), Some(Some(Rgb(_, _, bb)))) =
            (unlit.get('S'), blue_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            bb > ub,
            "blue glow should push skin blue (lit={bb}, unlit={ub})"
        );
    }

    #[test]
    fn tool_glow_tint_maps_known_tools() {
        use pixtuoid_core::source::Activity;
        let id = pixtuoid_core::AgentId::from_transcript_path("/t.jsonl");
        let edit_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Edit src/main.rs")),
            },
        );
        let bash_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Bash: ls")),
            },
        );
        let idle_slot = make_slot(id, ActivityState::Idle);
        let glow = &crate::tui::theme::NORMAL.tool_glow;
        let edit_tint = palette::tool_glow_tint(&edit_slot, glow);
        let bash_tint = palette::tool_glow_tint(&bash_slot, glow);
        let idle_tint = palette::tool_glow_tint(&idle_slot, glow);
        assert!(edit_tint.is_some(), "Edit should produce glow");
        assert!(bash_tint.is_some(), "Bash should produce glow");
        assert_eq!(idle_tint, None, "Idle should produce no glow");
        // Edit and Bash should be different colors.
        assert_ne!(edit_tint, bash_tint, "Edit and Bash should differ");
    }

    #[test]
    fn recolor_frame_substitutes_bhs_pixels() {
        let base = base_palette();
        // Build an agent palette where B/H/S are clearly distinguishable.
        let mut agent_pal = base.clone();
        agent_pal.insert('B', Some(Rgb(200, 0, 0))); // red shirt
        agent_pal.insert('H', Some(Rgb(0, 200, 0))); // green hair
        agent_pal.insert('S', Some(Rgb(0, 0, 200))); // blue skin

        // Frame: 1 pixel per palette key + 1 unrelated pixel + 1 transparent.
        let frame = Frame {
            width: 5,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),  // matches base B → should become red
                Some(Rgb(40, 50, 60)),  // matches base H → should become green
                Some(Rgb(70, 80, 90)),  // matches base S → should become blue
                Some(Rgb(123, 45, 67)), // unrelated     → unchanged
                None,                   // transparent   → unchanged
            ],
        };

        let out = recolor_frame(&frame, &agent_pal, &base);
        assert_eq!(out.width, 5);
        assert_eq!(out.height, 1);
        assert_eq!(out.pixels[0], Some(Rgb(200, 0, 0)));
        assert_eq!(out.pixels[1], Some(Rgb(0, 200, 0)));
        assert_eq!(out.pixels[2], Some(Rgb(0, 0, 200)));
        assert_eq!(out.pixels[3], Some(Rgb(123, 45, 67)));
        assert_eq!(out.pixels[4], None);
    }

    #[test]
    fn recolor_frame_handles_palette_with_no_overrides() {
        // If agent palette equals base, frame must come back identical.
        let base = base_palette();
        let frame = Frame {
            width: 3,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),
                Some(Rgb(40, 50, 60)),
                Some(Rgb(70, 80, 90)),
            ],
        };
        let out = recolor_frame(&frame, &base, &base);
        assert_eq!(out.pixels, frame.pixels);
    }

    /// Helper — build a minimal Drawable for sort-order tests. Uses the
    /// MeetingTable variant since it carries no borrowed data.
    fn drawable(anchor_y: u16) -> Drawable<'static> {
        Drawable {
            anchor_y,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 0, y: 0 },
            },
        }
    }

    #[test]
    fn drawables_sort_ascending_by_anchor_y() {
        let mut v = [drawable(30), drawable(10), drawable(20)];
        v.sort_by_key(|d| d.anchor_y);
        let ys: Vec<u16> = v.iter().map(|d| d.anchor_y).collect();
        assert_eq!(ys, [10, 20, 30]);
    }

    #[test]
    fn drawables_sort_is_stable_on_ties() {
        // Same anchor_y values — TimSort (Rust's stable sort) must
        // preserve insertion order. The y-sort relies on this so that
        // a character at the same anchor_y as the couch behind them
        // still paints first (matches the prior Pass 1 → Pass 1.5
        // layering).
        let mut v = [
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 1, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 2, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 3, y: 0 },
                },
            },
        ];
        v.sort_by_key(|d| d.anchor_y);
        let xs: Vec<u16> = v
            .iter()
            .map(|d| match &d.kind {
                DrawableKind::MeetingTable { pos } => pos.x,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(xs, [1, 2, 3]);
    }

    #[test]
    fn back_view_meeting_sofa_sorts_over_its_sitter() {
        // A south-of-table meeting sofa renders the `back_couch` sprite
        // (Facing::North) — the sitter's body must be occluded BEHIND the
        // sofa back, same as the lounge couch. The back-view sitter's
        // y-sort key is `sofa.y + 2` (back_couch_anchor = stand.y - 7,
        // sprite_h = 9, stand.y = sofa.y); the back sofa must beat that.
        let sofa_y: u16 = 40;
        let sitter_anchor_y = (sofa_y - 7) + 9; // back_couch_anchor + sprite_h
        let back_sofa_anchor_y = sofa_y + 3; // faces_away bump
        let front_sofa_anchor_y = sofa_y + 2; // sitter-on-top default
        assert!(
            back_sofa_anchor_y > sitter_anchor_y,
            "back-view sofa must sort AFTER its sitter (paint on top): \
             sofa={back_sofa_anchor_y}, sitter={sitter_anchor_y}"
        );
        // Front sofa ties the sitter; insertion order (decor first) then
        // keeps the sitter on top — so it must NOT exceed the sitter.
        assert!(
            front_sofa_anchor_y <= sitter_anchor_y,
            "front-view sofa must not sort after its sitter: \
             sofa={front_sofa_anchor_y}, sitter={sitter_anchor_y}"
        );
    }

    #[test]
    fn center_pin_south_offset_lands_on_the_sprite_south_row() {
        // A center-pinned sprite of height h blits at py = center - h/2, so its
        // south (front) ROW is `center + h - 1 - h/2`. The z-key must equal that
        // for BOTH parities — the round-1 fix used `h/2 - 1`, which is one short
        // for ODD h (the 11px whiteboard sorted in front of its own base).
        for h in 1u16..=16 {
            let expected_south = h - 1 - h / 2;
            assert_eq!(
                center_pin_south_offset(h),
                expected_south,
                "h={h}: z-key must land on the sprite south row, not one past it",
            );
        }
    }

    #[test]
    fn floor_lamp_south_offset_is_the_base_row() {
        // The lamp's halo / shadow / z-anchor all use floor_lamp_south_offset();
        // for the 4×10 sprite that's +4 (the base disc). Locks the value so a
        // visual-height edit in the table surfaces HERE, not as a floating halo.
        assert_eq!(floor_lamp_south_offset(), 4);
    }

    #[test]
    fn waypoint_depth_baseline_is_center_pinned_sprite_south() {
        use crate::tui::layout::{furniture_def, WaypointKind};
        // These appliances are center-pinned, so the z-sort key is the sprite's
        // south ROW = pos.y + footprint.h/2 - 1 (NOT +h/2 — that overshoots by
        // one and lets the sprite paint over a character just in front). Lock
        // the corrected offsets (vending 6→2, printer 4→1), DERIVED from the
        // footprint so a shape edit surfaces here, not as a visual layering bug.
        let south_off = |k: WaypointKind| {
            furniture_def(k.furniture())
                .footprint
                .expect("has footprint")
                .1
                / 2
                - 1
        };
        assert_eq!(south_off(WaypointKind::VendingMachine), 2);
        assert_eq!(south_off(WaypointKind::Printer), 1);
    }

    #[test]
    fn desk_walk_anchor_settles_exactly_on_the_seat() {
        // The home desk's walk anchor (desk_furniture_def's geometry, pure
        // algebraic) must land so the WALKING sprite anchor equals the SEATED
        // sprite anchor — zero pop on arrival. This identity is the contract
        // that lets desk_walk_anchor stay a pure fn instead of a side-probe; if
        // seated_anchor or walking_anchor ever change, this fails loudly.
        use crate::tui::layout::desk_walk_anchor;
        for desk in [
            Point { x: 40, y: 30 },
            Point { x: 100, y: 60 },
            Point { x: 7, y: 5 }, // near-origin: saturating_sub edge
        ] {
            // The identity must hold for ANY pack character width — the bundled
            // 8-wide AND the robot 10-wide — because desk_walk_anchor's +4 / -8
            // cancel against the width-centering for every w.
            for w in [CHARACTER_SPRITE_W, 10] {
                assert_eq!(
                    walking_anchor(desk_walk_anchor(desk), w),
                    seated_anchor(desk, w),
                    "walking_anchor(desk_walk_anchor({desk:?}), {w}) must equal seated_anchor",
                );
            }
        }
    }

    #[test]
    fn seated_foot_cell_settles_exactly_on_the_render_anchor() {
        // The UNIFIED zero-pop identity: for every occupies_pos Furniture (the
        // seat kinds AND the home desk), the WALKING sprite anchor at
        // seated_foot_cell(S) must equal the SEATED render anchor at pos — so the
        // post-A* settle ends with zero pop on every arrival side. back_couch
        // render for couch/sofa, waypoint render for stand, seated_anchor for the
        // desk: ONE fn, the correctness lock for the whole convergence.
        use pixtuoid_core::layout::{seated_foot_cell, Furniture};
        for pos in [
            Point { x: 40, y: 30 },
            Point { x: 100, y: 60 },
            Point { x: 6, y: 8 }, // near-origin: saturating_sub edge
        ] {
            for w in [CHARACTER_SPRITE_W, 10] {
                for f in [Furniture::Couch, Furniture::MeetingSofa] {
                    let s = seated_foot_cell(f, pos).expect("occupies_pos seat");
                    assert_eq!(
                        walking_anchor(s, w),
                        back_couch_anchor(pos, w),
                        "{f:?}: walking_anchor(S={s:?}) must equal back_couch_anchor(pos={pos:?}) w={w}",
                    );
                }
                let s = seated_foot_cell(Furniture::MeetingStand, pos).expect("occupies_pos seat");
                assert_eq!(
                    walking_anchor(s, w),
                    waypoint_anchor(pos, w),
                    "MeetingStand: walking_anchor(S={s:?}) must equal waypoint_anchor(pos={pos:?}) w={w}",
                );
                // The home desk flows through the SAME fn — its S is
                // desk_walk_anchor, its render seated_anchor. Same identity,
                // proving the desk genuinely converged into Furniture.
                let sd = seated_foot_cell(Furniture::Desk, pos).expect("desk is occupies_pos");
                assert_eq!(
                    walking_anchor(sd, w),
                    seated_anchor(pos, w),
                    "Desk: walking_anchor(seated_foot_cell)={:?} must equal seated_anchor",
                    walking_anchor(sd, w),
                );
            }
            // Obstacles have no fixed seat — their sprite renders AT the approach
            // cell, so seated_foot_cell is None.
            assert_eq!(seated_foot_cell(Furniture::Pantry, pos), None);
            assert_eq!(seated_foot_cell(Furniture::VendingMachine, pos), None);
        }
    }

    #[test]
    fn settle_view_matches_the_seated_view_for_every_seat() {
        // The unification guarantee: the sit-down settle and the seated render
        // derive from ONE source (`SeatView::of`), so they can never disagree —
        // the "sit facing the wrong way then snap" bug cannot recur, for current
        // OR future seatable furniture (matched generically by having a settle
        // foot-cell, not a hardcoded kind list).
        use crate::tui::layout::{Facing, WaypointKind, MAX_VISIBLE_DESKS};
        let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
        let seats: Vec<_> = l
            .waypoints
            .iter()
            .filter(|w| {
                pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some()
            })
            .collect();
        assert!(
            seats.iter().any(
                |w| matches!(w.kind, WaypointKind::Couch | WaypointKind::MeetingSofa)
                    && w.facing == Facing::North
            ),
            "this layout size must have a window-facing (North) seat to exercise the fix"
        );
        for w in &seats {
            let foot = pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos)
                .expect("seat occupies_pos → has a settle foot cell");
            let view = SeatView::of(w.kind, w.facing);
            // The sit-down glide onto this seat renders in the seat's view, at the
            // seat's stable z-key.
            assert_eq!(
                settle_seat_view(foot, &l),
                Some((view, view.z_key_for_seat(w.pos))),
                "settle onto {:?}@{:?} must use the seat view {view:?}",
                w.kind,
                w.pos
            );
            // Totality guard (review finding): a seat detected generically by its
            // foot-cell must NOT fall through `SeatView::of`'s upright catch-all —
            // every real seat maps to an explicitly-handled view, so a future seat
            // added to the Furniture table without a `SeatView::of` arm fails HERE
            // rather than silently rendering as an upright stander.
            assert!(
                matches!(w.kind, WaypointKind::Couch | WaypointKind::MeetingSofa)
                    || matches!(w.kind, WaypointKind::MeetingStand),
                "seat kind {:?} has a settle foot-cell but is not explicitly handled \
                 in SeatView::of — add an arm there",
                w.kind
            );
            // Single-source invariant: the seated sprite and the sit-down settle
            // agree on orientation (both back-view, or neither) — they cannot
            // diverge because both come from `view`.
            let seated_is_back = view.seated_sprite().0 == "back_couch";
            let (settle_is_back, _) = view.settle_walk();
            assert_eq!(
                seated_is_back, settle_is_back,
                "{:?}: seated render and sit-down settle must share orientation",
                w.kind
            );
            // For seats whose foot-cell is offset from the centre (couch/sofa),
            // the centre is an ordinary travel target — keeps travel facing.
            if foot != w.pos {
                assert_eq!(
                    settle_seat_view(w.pos, &l),
                    None,
                    "seat centre {:?} is not a settle foot cell",
                    w.pos
                );
            }
        }
    }

    #[test]
    fn settle_seat_view_recognizes_the_home_desk() {
        // The home desk joins the unified settle: its chair (seated_foot_cell(Desk)
        // = desk_walk_anchor) is a settle target, so the arrival glide onto it goes
        // through SeatView::Front (front-facing, stable z-key) — same path as the
        // sofas, no front-cross.
        use crate::tui::layout::MAX_VISIBLE_DESKS;
        use pixtuoid_core::layout::{desk_walk_anchor, Furniture};
        let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
        let desk = *l.home_desks.first().expect("at least one home desk");
        let chair = desk_walk_anchor(desk);
        assert_eq!(
            settle_seat_view(chair, &l),
            Some((SeatView::Front, desk.y + DESK_SEAT_Z_OFF)),
            "the desk chair {chair:?} must settle as SeatView::Front at the desk z-key"
        );
        // seated_foot_cell(Desk) is exactly desk_walk_anchor — the hook keys off it.
        assert_eq!(
            pixtuoid_core::layout::seated_foot_cell(Furniture::Desk, desk),
            Some(chair)
        );
        // A non-chair cell near the desk is ordinary travel.
        assert_eq!(
            settle_seat_view(desk, &l),
            None,
            "the desk corner is not the chair"
        );
    }

    #[test]
    fn desk_settle_z_key_matches_the_seated_arm() {
        // The desk's settle z-key (desk.y + DESK_SEAT_Z_OFF) must equal the z-key
        // the seated desk arms use (anchor_no_breath.y + 12 with anchor =
        // seated_anchor) so the glide and the settled render sort identically —
        // and both stay below the desk furniture z-key (desk.y + 8).
        for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
            for w in [CHARACTER_SPRITE_W, 10] {
                let seated_arm_z = seated_anchor(desk, w).y + 12;
                assert_eq!(
                    desk.y + DESK_SEAT_Z_OFF,
                    seated_arm_z,
                    "desk settle z-key must equal the SeatedIdle/Typing arm z-key"
                );
                let fp_h = crate::tui::layout::desk_furniture_def()
                    .footprint
                    .expect("desk footprint")
                    .1;
                assert!(
                    desk.y + DESK_SEAT_Z_OFF < desk.y + fp_h + DESK_FRONT_OVERHANG,
                    "desk sitter must sort behind the desk furniture"
                );
            }
        }
    }

    #[test]
    fn sit_arc_z_key_is_stable_and_on_the_right_side_of_its_furniture() {
        // The z-sort flicker fix. The sit-down/stand-up GLIDE and the SEATED state
        // must share ONE z-key (`z_key_for_seat`) so the agent never crosses its
        // furniture's z-key mid-glide (pop in front of the sofa for a frame, then
        // snap behind it). Asserts: (1) the seat z-key equals the historical
        // AtWaypoint formula (seated render unchanged); (2) it lands the agent on
        // the correct side of the furniture for every seat — behind a back-view
        // sofa/couch, on top of (tie with) a front sofa, and in front of the
        // meeting table for a stand.
        use crate::tui::layout::{
            furniture_def, z_sort_row, Anchor, Facing, Furniture, WaypointKind, MAX_VISIBLE_DESKS,
        };
        let l = Layout::compute(192, 158, MAX_VISIBLE_DESKS).expect("fits");
        let mut saw_back = false;
        for w in l.waypoints.iter().filter(|w| {
            pixtuoid_core::layout::seated_foot_cell(w.kind.furniture(), w.pos).is_some()
        }) {
            let view = SeatView::of(w.kind, w.facing);
            let z = view.z_key_for_seat(w.pos);

            // (1) Behavior-preserving: equals the historical seated AtWaypoint key.
            let historical = match view {
                // back_couch_anchor.y + sprite_h(9) = (pos.y - 7) + 9
                SeatView::Front | SeatView::Back => {
                    back_couch_anchor(w.pos, CHARACTER_SPRITE_W).y + 9
                }
                // waypoint_anchor.y + sprite_h(12) + 3 = (pos.y - 12) + 12 + 3
                SeatView::Side { .. } => waypoint_anchor(w.pos, CHARACTER_SPRITE_W).y + 12 + 3,
            };
            assert_eq!(
                z, historical,
                "{:?}@{:?}: seat z-key {z} must equal the historical AtWaypoint key {historical}",
                w.kind, w.pos
            );

            // (2) Correct side of the furniture.
            match w.kind {
                WaypointKind::Couch => {
                    // Lounge couch furniture z-key = z_sort_row(Center, center, visual.h).
                    let couch_z = z_sort_row(
                        Anchor::Center,
                        w.pos,
                        furniture_def(Furniture::Couch).visual.1,
                    );
                    assert!(
                        z < couch_z,
                        "couch sitter z {z} must be BEHIND the couch back {couch_z}"
                    );
                    saw_back = true;
                }
                WaypointKind::MeetingSofa => {
                    // Furniture z-key: faces_away (North) → sofa.y+3; else sofa.y+2.
                    if w.facing == Facing::North {
                        assert!(z < w.pos.y + 3, "back sofa sitter z {z} must be < sofa.y+3");
                        saw_back = true;
                    } else {
                        // Front sofa: tie at sofa.y+2 (insertion order keeps the
                        // sitter on top).
                        assert!(
                            z <= w.pos.y + 2,
                            "front sofa sitter z {z} must be <= sofa.y+2"
                        );
                    }
                }
                WaypointKind::MeetingStand => {
                    // Stand clears the meeting table row it stands beside.
                    assert!(
                        z > w.pos.y + 2,
                        "stand z {z} must clear the table at pos.y+2"
                    );
                }
                _ => {}
            }
        }
        assert!(
            saw_back,
            "layout must contain a back-view seat to exercise the flicker fix"
        );
    }

    #[test]
    fn desk_occupant_always_sorts_behind_its_desk() {
        // The same "agent on the correct side of its furniture" guarantee the
        // wander-seat invariant gives, extended to the home desk so EVERY seatable
        // is covered. A seated or standing desk occupant must y-sort BEHIND the
        // desk cubicle (which sorts at `desk.y + footprint.h + DESK_FRONT_OVERHANG`
        // — pinned by `desk_z_key_is_footprint_front_plus_overhang`). The desk
        // keeps its own render arms (different sprite/work-state by design), but
        // ties its character z-key to its furniture z-key so a footprint or anchor
        // edit can never drift the agent in front of its own desk (no flicker,
        // matching the wander seats — the z-order GUARANTEE is unified even though
        // the render code is intentionally not).
        let fp_h = crate::tui::layout::desk_furniture_def()
            .footprint
            .expect("desk has a footprint")
            .1;
        for desk in [Point { x: 40, y: 30 }, Point { x: 100, y: 60 }] {
            for w in [CHARACTER_SPRITE_W, 10] {
                let desk_furniture_z = desk.y + fp_h + DESK_FRONT_OVERHANG;
                // SeatedIdle / SeatedThinking / SeatedTyping z-key.
                let seated_z = seated_anchor(desk, w).y + 12;
                // StandingAtDesk z-key.
                let standing_z = standing_at_desk_anchor(desk, w).y + 12;
                assert!(
                    seated_z < desk_furniture_z,
                    "seated desk occupant z {seated_z} must be BEHIND the desk {desk_furniture_z}"
                );
                assert!(
                    standing_z < desk_furniture_z,
                    "standing desk occupant z {standing_z} must be BEHIND the desk {desk_furniture_z}"
                );
            }
        }
    }

    #[test]
    fn desk_z_key_is_footprint_front_plus_overhang() {
        // The DeskCubicle z-sort baseline is `desk.y + footprint.h +
        // DESK_FRONT_OVERHANG` — footprint-front-derived (consistent with the
        // waypoint/wall baselines), not a bare sprite-bottom literal. Equals
        // the historical `desk.y + 8` (6 + 2). Locks the relationship so a
        // footprint or overhang edit surfaces here, not as a layering bug.
        let fp_h = crate::tui::layout::desk_furniture_def()
            .footprint
            .expect("desk has a footprint")
            .1;
        assert_eq!(fp_h + DESK_FRONT_OVERHANG, 8, "desk z-key offset (was +8)");
    }

    #[test]
    fn every_pod_declares_behind_occlusion() {
        // Per-kind occlusion policy now lives in the FurnitureDef table
        // (`occludes_behind`), not a render-local allowlist. Every SOLID aisle pod
        // is something a walker can pass behind, so it carries a back-cap depth (the
        // exact px per kind is pinned in core's furniture_def invariant test). The
        // ONE exception is the plant: its thin foliage's 1px back-cap rendered as an
        // ugly dark line across the top, so PlantTall carries none. Exhaustive over
        // PodDecor::ALL so a new pod kind must declare its policy in the table.
        use crate::tui::layout::{furniture_def, PodDecor};
        assert_eq!(
            PodDecor::ALL.len(),
            5,
            "PodDecor variant added/removed — update ALL (and this count)"
        );
        for &kind in PodDecor::ALL {
            let def = furniture_def(kind.furniture());
            let expect_occlusion = !matches!(kind, PodDecor::PlantTall);
            assert_eq!(
                def.occludes_behind.is_some(),
                expect_occlusion,
                "{kind:?}: aisle-pod behind-occlusion policy (plants are the exception)"
            );
            // z-sort precondition: the pod-decor loop anchors at
            // `center_pin_south_offset(visual.1)`, so a 0-height visual would
            // sort the sprite at its own center. Every pod must have visible h.
            assert!(
                def.visual.1 > 0,
                "{kind:?}: pod decor needs a non-zero visual height for the z-sort"
            );
        }
    }

    #[test]
    fn back_view_seats_sort_over_their_sitter() {
        // Occlusion for BOTH back-view seat renderers (lounge couch + the
        // north meeting sofa): the furniture must y-sort OVER the back-view
        // sitter so the sofa back occludes the body. The sitter's z-key is
        // `base + 2` (back_couch_anchor stand-7 + sprite_h 9); the back
        // furniture is `base + 3`. Lounge couch (`center.y + 3`) and the north
        // meeting sofa (`sofa.y + 3`) both satisfy it.
        let base: u16 = 40;
        let sitter = (base - 7) + 9; // = base + 2
        let couch_furniture = base + 3; // WaypointCouch drawable
        let back_meeting_sofa = base + 3; // faces_away meeting sofa
        assert!(couch_furniture > sitter, "couch must sort over its sitter");
        assert!(
            back_meeting_sofa > sitter,
            "north meeting sofa must sort over its sitter"
        );
    }

    #[test]
    fn character_anchor_y_exceeds_desk_when_south_of_it() {
        // The bug-fix invariant: a character whose feet (anchor.y + 12)
        // land BELOW the desk's bottom row (desk.y + 8) must sort AFTER
        // the desk and therefore paint on top.
        let desk_y: u16 = 20;
        let desk_anchor_y = desk_y + 8;
        let char_feet_anchor = (desk_y + 10) + 12; // walker south of desk
        assert!(
            char_feet_anchor > desk_anchor_y,
            "walker south of desk must sort after it: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }

    #[test]
    fn character_anchor_y_below_desk_when_seated_at_it() {
        // Inverse invariant — a SEATED character at this desk has feet
        // that land ABOVE the desk's bottom (because they're tucked
        // under the desktop). They must sort BEFORE the desk so the
        // desk occludes their lower body in top-down view.
        let desk_y: u16 = 20;
        let seated_anchor = seated_anchor(Point { x: 0, y: desk_y }, CHARACTER_SPRITE_W);
        let char_feet_anchor = seated_anchor.y + 12;
        let desk_anchor_y = desk_y + 8;
        assert!(
            char_feet_anchor < desk_anchor_y,
            "seated char must sort before desk: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }

    // --- compute_door_frame_idx -------------------------------------------

    fn entry_slot(created_at_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = pixtuoid_core::AgentId::from_transcript_path("/door.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_millis(created_at_ms_ago);
        s
    }

    fn exit_slot(exit_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = pixtuoid_core::AgentId::from_transcript_path("/exit.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_secs(300);
        s.exiting_at = Some(now - std::time::Duration::from_millis(exit_ms_ago));
        s
    }

    #[test]
    fn door_frame_closed_when_no_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(compute_door_frame_idx(&[], now, 0), 0);
    }

    #[test]
    fn door_frame_just_spawned_is_half_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 50 ms into the 200 ms opening ramp — first half = frame 1.
        let slot = entry_slot(50, now);
        assert_eq!(compute_door_frame_idx(&[slot], now, 0), 1);
    }

    #[test]
    fn door_frame_after_opening_ramp_is_fully_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms (still inside opening ramp but past midpoint) → frame 2.
        let s1 = entry_slot(150, now);
        assert_eq!(compute_door_frame_idx(&[s1], now, 0), 2);
        // 2 s into the 4 s window → fully open.
        let s2 = entry_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[s2], now, 0), 2);
    }

    #[test]
    fn door_frame_closing_then_closed_at_end_of_entry() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms left in the entry window → closing ramp first half → frame 1.
        let mid_close = entry_slot(pose::ENTRY_ANIMATION_MS - 150, now);
        assert_eq!(compute_door_frame_idx(&[mid_close], now, 0), 1);
        // 50 ms left → closing ramp final half → frame 0 (closed).
        let near_end = entry_slot(pose::ENTRY_ANIMATION_MS - 50, now);
        assert_eq!(compute_door_frame_idx(&[near_end], now, 0), 0);
    }

    #[test]
    fn door_frame_expired_entry_contributes_nothing() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Older than the 4 s entry window → no contribution.
        let old = entry_slot(pose::ENTRY_ANIMATION_MS + 1, now);
        assert_eq!(compute_door_frame_idx(&[old], now, 0), 0);
    }

    #[test]
    fn door_frame_exit_window_uses_4500ms_total() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 2 s into a 4.5 s exit window → mid-flight → fully open.
        let exiting = exit_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[exiting], now, 0), 2);
    }

    #[test]
    fn door_frame_takes_max_across_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let opening = entry_slot(50, now); // frame 1
        let open = entry_slot(2_000, now); // frame 2
        assert_eq!(compute_door_frame_idx(&[opening, open], now, 0), 2);
    }

    #[test]
    fn door_frame_uses_physics_window_when_nonzero() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Slot spawned 3 s ago; with old ENTRY_ANIMATION_MS=4000 it would still
        // be mid-flight. Supply a short physics window (2500 ms) so it reads as
        // near the closing ramp instead.
        let short_window_ms: u64 = 2_500;
        // elapsed=3000, total=2500 → elapsed > total → door should be in closing
        // ramp or closed (remaining = 0 → frame 0).
        let slot = entry_slot(3_000, now);
        let frame = compute_door_frame_idx(&[slot], now, short_window_ms);
        assert_eq!(
            frame, 0,
            "with short physics window elapsed>total should yield closed door, got frame {frame}"
        );

        // Slot spawned 500 ms ago; physics window = 2500 ms → still well in the
        // middle (fully open frame = 2).
        let slot_mid = entry_slot(500, now);
        let frame_mid = compute_door_frame_idx(&[slot_mid], now, short_window_ms);
        assert_eq!(
            frame_mid, 2,
            "500ms into 2500ms window should be fully open, got frame {frame_mid}"
        );
    }

    #[test]
    fn weather_state_covers_all_variants() {
        let mut seen = std::collections::HashSet::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..200u64 {
            let now = base + std::time::Duration::from_secs(cycle * 600);
            seen.insert(std::mem::discriminant(&background::weather_state(now)));
        }
        assert!(
            seen.len() >= 8,
            "expected all 8 weather variants in 200 cycles, got {}",
            seen.len()
        );
    }

    #[test]
    fn weather_state_deterministic() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        let a = background::weather_state(now);
        let b = background::weather_state(now);
        assert_eq!(a, b);
    }

    #[test]
    fn weather_state_changes_across_cycles() {
        let mut states = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..20u64 {
            states.push(background::weather_state(
                base + std::time::Duration::from_secs(cycle * 600),
            ));
        }
        let unique: std::collections::HashSet<_> =
            states.iter().map(std::mem::discriminant).collect();
        assert!(unique.len() >= 2, "weather should vary across cycles");
    }

    #[test]
    fn sunset_strength_varies_across_day() {
        let mut strengths = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for hour in 0..24u64 {
            strengths.push(background::sunset_strength(
                base + std::time::Duration::from_secs(hour * 3600),
            ));
        }
        let has_zero = strengths.iter().any(|s| *s < 0.05);
        let has_nonzero = strengths.iter().any(|s| *s > 0.1);
        assert!(has_zero, "sunset should be ~0 at some hours");
        assert!(has_nonzero, "sunset should be >0 at dawn/dusk hours");
    }
}
