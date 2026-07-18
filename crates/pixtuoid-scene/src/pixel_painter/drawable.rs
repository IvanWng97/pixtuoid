//! Y-sorted drawable enum (painter's algorithm).
//!
//! Top-down depth: every mid-ground entity carries an `anchor_y` = the
//! y-pixel row where it touches the floor (front-facing bottom edge for
//! items with thickness). Drawables sort ascending by `anchor_y` and
//! paint in order. Larger `anchor_y` = closer to camera = paints last
//! (on top). Solves the classic "character standing south of a desk
//! should appear in front of it" problem without per-pair special cases.
//!
//! What stays OUTSIDE the sort:
//!   - Background (floor / walls / lighting / corridor / room walls /
//!     entry mat / clock / shadows). All depth-independent.
//!   - Per-character attached effects (chair-behind, sleep_z,
//!     waiting_bubble, walking dust, coffee steam, screen glow)
//!     paint AS PART of their parent `Drawable` — they ride along with
//!     the entity in z-order, not as a global foreground pass.

use std::time::SystemTime;

use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::AgentSlot;

use super::effects::{
    paint_coffee_steam, paint_pet_hearts, paint_screen_glow, paint_sleep_z, paint_waiting_bubble,
    paint_walking_dust,
};
use super::epoch_ms;
use super::frame_at;
use super::furniture::{
    paint_area_rug, paint_coat_rack, paint_fish_tank, paint_kitchen_island, paint_meeting_chair,
    paint_meeting_table, paint_printer, paint_side_table, paint_vending_machine,
};
use super::paint_character_at;
use crate::frame_cache::FrameCache;
use crate::layout::{Point, Size, DESK_H, DESK_W};
use crate::pet::PetKind;

/// Coffee-steam plume column offset from the pantry sprite CENTER (`pos.x`), per
/// size — hand-tuned to the sprite art so the steam sits within
/// [`super::PANTRY_COFFEE_COLS_LARGE`] / [`super::PANTRY_COFFEE_COLS_SMALL`]
/// (pinned by `steam_anchor_sits_within_the_coffee_machine_columns`).
const PANTRY_STEAM_DX_LARGE: i16 = -2;
const PANTRY_STEAM_DX_SMALL: i16 = 1;

// Vending pickup-slot offset from the sprite's top-left — the ONE cell where
// the idle trim paints and the busy can-drop lands; the pixel test derives
// the same cell from here.
pub(crate) const VENDING_PICKUP_SLOT: (u16, u16) = (2, 4);

pub(super) struct Drawable<'a> {
    pub(super) anchor_y: u16,
    pub(super) kind: DrawableKind<'a>,
}

pub(super) enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit: divider + filing cabinet (every
    /// other desk) + desk sprite + screen-glow if the
    /// occupant is Active. Bundled so the cubicle paints atomically at
    /// the desk's bottom-edge row.
    DeskCubicle {
        desk: Point,
        is_last_col: bool,
        has_cabinet: bool,
        screen_glow: Option<Rgb>,
        has_coffee: bool,
        coffee_steam: bool,
        /// Token-meter paper tower (#632): 0 = no tower (byte-identical to
        /// the pre-meter desk), 1..=3 = ream count. Derived at enqueue via
        /// `token_meter::token_tier(occupant.tokens_used)`.
        token_tier: u8,
        /// A falling sheet's distance FALLEN (px) when a big usage reading
        /// is mid-drop (`token_meter::sheet_fall_dist`), else `None`.
        sheet_fall: Option<u16>,
    },
    Character {
        agent: &'a AgentSlot,
        anim_name: &'static str,
        frame_idx: usize,
        anchor: Point,
        flip_x: bool,
        /// Tool-derived monitor glow color. `Some(color)` tints the
        /// skin toward that color so scanning a row of typing agents
        /// shows tool type at a glance. `None` for non-desk poses.
        glow_tint: Option<Rgb>,
        sleep_z_seed: Option<u64>,
        waiting_bubble: bool,
        walking_dust_frame: Option<usize>,
    },
    /// Lounge couch (mirror_vertical'd — back at bottom, seat at top).
    WaypointCouch {
        pos: Point,
    },
    /// Pantry counter (with coffee steam attached so steam rides above
    /// the counter in z-order). `use_large` picks the detailed 32×10
    /// kitchen sprite vs. the 20×8 compact fallback — derived from
    /// `layout.pantry_counter_size()` at queue time.
    WaypointPantry {
        pos: Point,
        use_large: bool,
    },
    MeetingSofa {
        pos: Point,
        mirrored: bool,
    },
    MeetingTable {
        pos: Point,
    },
    /// Area rug — warm patterned rectangle that anchors a seating
    /// arrangement visually. Used for the meeting room (large) and
    /// the lounge (smaller). Painted BEFORE the furniture in z-order
    /// (anchor_y at top of rug) so chairs / couches sit on top.
    AreaRug {
        pos: Point,
        width: u16,
        height: u16,
    },
    /// Lounge side table (5×3 wood + magazine) next to the viewing
    /// couch. Centred at `pos`.
    LoungeSideTable {
        pos: Point,
    },
    /// Kitchen-island body (centred at `pos`) — procedural, like the pantry
    /// table but counter-height with a dressed top (bowl + cups).
    KitchenIsland {
        pos: Point,
    },
    /// Snack shelf (centred at `pos`) — pack sprite `snack_shelf`; the tall
    /// shelf overhangs its shallow 2-row base (walk-behind class).
    SnackShelf {
        pos: Point,
    },
    Plant {
        kind: crate::layout::PlantKind,
        pos: Point,
    },
    /// Aisle decor item between desk pods (plant / whiteboard / TV /
    /// phone booth / standing desk). All are obstacles in the
    /// walkable mask; phone booth + standing desk additionally exist
    /// as waypoints so agents can wander to them.
    PodDecorItem {
        kind: crate::layout::PodDecor,
        pos: Point,
    },
    FloorLamp {
        pos: Point,
    },
    Door {
        pos: Point,
        /// Frame index into the `door` animation. 0 = closed,
        /// 1 = half-open, 2 = fully open. Computed stateless from
        /// agents' entry/exit windows in the orchestrator so the door
        /// transitions smoothly closed → half → open at the start of a
        /// transit and back open → half → closed at the end.
        frame_idx: usize,
    },
    WallDecor {
        kind: crate::layout::WallDecor,
        pos: Point,
    },
    VendingMachine {
        pos: Point,
        /// An agent stands at this waypoint this frame — drives the
        /// drink-drop feedback animation (B-4).
        busy: bool,
    },
    Printer {
        pos: Point,
        /// An agent stands at this waypoint this frame — drives the
        /// page-eject feedback animation (B-4).
        busy: bool,
    },
    Pet {
        kind: PetKind,
        pos: Point,
        flip: bool,
        anim_name: &'static str,
        frame_idx: usize,
        pet_elapsed_ms: Option<u64>,
    },
    /// The OpenClaw (or any gateway) lobster mascot — a presence-gated wandering
    /// creature, NOT an agent (lives in `daemons`, not `scene.agents`).
    /// y-sorted at its south row like a pet. `run_count > 0` (an in-flight agent
    /// run) adds a rising activity-bubble cue — the busy tell keys on RUNS, not
    /// the (persistent, single-user) session count, which sticks at 1 at rest.
    GatewayMascot {
        pos: Point,
        anim_name: &'static str,
        frame_idx: usize,
        run_count: u32,
        /// Gateway up but model-broken (#317) → render the lobster sickly red.
        degraded: bool,
    },
    /// Horizontal (E-W) frosted-glass room divider, y-sorted at its south
    /// (front) edge so it composites over a character standing behind it.
    RoomWallH {
        x0: u16,
        x1: u16,
        y_top: u16,
        /// This end abuts a doorway ⇒ paint its dark jamb (#559). Flagged at
        /// enqueue (the paint pass has no layout access).
        jamb_left: bool,
        jamb_right: bool,
    },
    /// Vertical (N-S, edge-on) frosted-glass room divider, y-sorted at its raw
    /// south end (see `enqueue_room_walls_v`) like the horizontal wall — so a
    /// character standing north of the wall's north cap (the visual-only overhang
    /// above the blocked footprint) is composited behind the frosted glass.
    /// `y_top`/`y_bot` are the stitched PAINT extent (`stitch_vertical_wall`); the
    /// z-key is the raw end so a corner-extended `y_bot` doesn't flip H-over-V.
    RoomWallV {
        x: u16,
        y_top: u16,
        y_bot: u16,
        /// This segment's north/south end abuts a doorway ⇒ paint its dark jamb
        /// on that cut end (#559). Flagged at enqueue (no layout in the paint pass).
        jamb_north: bool,
        jamb_south: bool,
    },
    /// Meeting-room coat rack (pole + base + coat blobs), y-sorted at its base
    /// row so a character walking in front of it occludes it (and one behind
    /// is occluded BY it) — was painted in the background pass, always under
    /// every character. `pos` is the pole top; the base sits at `pos.y + 7`.
    CoatRack {
        pos: Point,
    },
    /// Lounge aquarium (decor arc), y-sorted at its cabinet's south row so a
    /// walker in front occludes it. `pos` is the sprite CENTER (matches the
    /// mask stamp's `Anchor::Center`); fish animate on the paint clock.
    FishTank {
        pos: Point,
    },
    /// Head-of-table meeting chair, y-sorted one row ABOVE its occupant's
    /// seated anchor (`wp.y + 2`) so the sitter always paints over it.
    MeetingChair {
        pos: Point,
        back_west: bool,
    },
}

/// Busy "working" cue — a few bubbles rising above the lobster's head while a run is
/// in flight. Count is a small baseline + concurrent-run count (capped): a
/// single serialized run reads as a calm stream, a power-user fan-out bubbles
/// harder. Stateless: phase derives from `now`.
fn paint_mascot_bubbles(buf: &mut RgbBuffer, pos: Point, frame_h: u16, runs: u32, now: SystemTime) {
    let now_ms = epoch_ms(now);
    let bubble = Rgb {
        r: 0xd6,
        g: 0xf2,
        b: 0xf8,
    };
    let top = pos.y.saturating_sub(frame_h / 2 + 1);
    let n = (runs + 1).min(4) as u16;
    for i in 0..n {
        // Each bubble rises on its own phase over a ~6px column above the head.
        let phase = ((now_ms / 110) + i as u64 * 7) % 6;
        let by = top.saturating_sub(phase as u16);
        let bx = (pos.x + i * 2).saturating_sub(n);
        if bx < buf.width() && by < buf.height() {
            buf.put(bx, by, bubble);
        }
    }
}

/// Dispatch one Drawable's paint. Effects attached to characters paint
/// inline so they ride along with the character in z-order.
pub(super) fn paint_drawable(
    d: &Drawable<'_>,
    buf: &mut RgbBuffer,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
    theme: &crate::theme::Theme,
) {
    match &d.kind {
        DrawableKind::DeskCubicle {
            desk,
            is_last_col,
            has_cabinet,
            screen_glow,
            has_coffee,
            coffee_steam,
            token_tier,
            sheet_fall,
        } => {
            let divider = theme.office.cubicle_divider;
            if !is_last_col {
                let div_x = desk.x + DESK_W + 3;
                for dy in 0..(DESK_H + 1) {
                    let py = desk.y.saturating_sub(1) + dy;
                    if div_x < buf.width() && py < buf.height() {
                        buf.put(div_x, py, divider);
                    }
                }
            }
            if *has_cabinet {
                if let Some(cab) = pack
                    .animation("filing_cabinet")
                    .and_then(|a| a.frames.first())
                {
                    let cab_x = desk.x.saturating_sub(cab.width() + 1);
                    let cab_y = desk.y;
                    if cab_y + cab.height() <= buf.height() {
                        blit_frame(cab, cab_x, cab_y, buf);
                    }
                }
            }
            if let Some(frame) = pack.animation("desk").and_then(|a| a.frames.first()) {
                // The desk sprite's top row is the monitor's raised bezel (1px
                // above the desk back), so blit 1px higher — the surface/keyboard
                // rows still land at their original desk.y-relative positions.
                blit_frame(frame, desk.x, desk.y.saturating_sub(1), buf);
            }
            paint_desk_coffee(buf, *desk, *has_coffee, *coffee_steam, now, theme);
            paint_token_stack(buf, *desk, *token_tier, *sheet_fall, theme);
            if let Some(tint) = screen_glow {
                paint_screen_glow(buf, desk.x, desk.y, now, *tint, theme);
            }
        }
        DrawableKind::Character {
            agent,
            anim_name,
            frame_idx,
            anchor,
            flip_x,
            glow_tint,
            sleep_z_seed,
            waiting_bubble,
            walking_dust_frame,
        } => {
            if let Some(dust_frame) = walking_dust_frame {
                paint_walking_dust(buf, *anchor, *dust_frame, theme);
            }
            paint_character_at(
                buf, anim_name, *frame_idx, *anchor, agent, pack, *flip_x, *glow_tint, cache, now,
            );
            if let Some(seed) = sleep_z_seed {
                paint_sleep_z(buf, *anchor, now, *seed, theme);
            }
            if *waiting_bubble {
                paint_waiting_bubble(buf, *anchor, theme);
            }
        }
        DrawableKind::WaypointCouch { pos } => {
            // Lounge couch reuses the meeting_sofa sprite (20×7) so
            // both seating areas have the same readable 3-cushion
            // silhouette. Flipped vertically so the back faces NORTH
            // (toward the windows the viewer is looking at).
            if let Some(f) = pack
                .animation("meeting_sofa")
                .and_then(|a| a.frames.first())
            {
                let cx = pos.x.saturating_sub(f.width() / 2);
                let cy = pos.y.saturating_sub(f.height() / 2);
                let flipped = f.mirror_vertical();
                blit_frame(&flipped, cx, cy, buf);
            }
        }
        DrawableKind::WaypointPantry { pos, use_large } => {
            // Pick the big detailed kitchen sprite when the pantry is
            // large enough; fall back to the compact 20×8 layout on
            // narrow terminals.
            let anim_name = if *use_large { "pantry" } else { "pantry_small" };
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let cx = pos.x.saturating_sub(f.width() / 2);
                let cy = pos.y.saturating_sub(f.height() / 2);
                // A character behind the counter is occluded by the counter's own
                // sprite (it y-sorts at the south base → paints over a north-
                // stander). The mask south-anchors a shallow strip to that base so
                // the walker parks deep behind the visual; no synthetic cap.
                blit_frame(f, cx, cy, buf);
            }
            // The coffee machine occupies `PANTRY_COFFEE_COLS_{LARGE,SMALL}` (the
            // shared source of truth, also used by the binary's hit-test). The
            // steam plumes from within that column range — hand-tuned per sprite
            // art (`PANTRY_STEAM_DX_*`), pinned within the cols by
            // `steam_anchor_sits_within_the_coffee_machine_columns`.
            let steam_dx: i16 = if *use_large {
                PANTRY_STEAM_DX_LARGE
            } else {
                PANTRY_STEAM_DX_SMALL
            };
            let steam_x = (pos.x as i32 + steam_dx as i32).max(0) as u16;
            paint_coffee_steam(
                buf,
                Point {
                    x: steam_x,
                    y: pos.y.saturating_sub(2),
                },
                now,
                theme,
            );
        }
        DrawableKind::MeetingSofa { pos, mirrored } => {
            if let Some(f) = pack
                .animation("meeting_sofa")
                .and_then(|a| a.frames.first())
            {
                let sx = pos.x.saturating_sub(f.width() / 2);
                let sy = pos.y.saturating_sub(f.height() / 2);
                if *mirrored {
                    let flipped = f.mirror_vertical();
                    blit_frame(&flipped, sx, sy, buf);
                } else {
                    blit_frame(f, sx, sy, buf);
                }
            }
        }
        DrawableKind::MeetingTable { pos } => {
            // Sprite size from the table (== footprint for the meeting table) so
            // the painted meeting table can't drift from the masked obstacle.
            let Size { w, h } =
                crate::layout::furniture_def(crate::layout::Furniture::MeetingTable).visual;
            paint_meeting_table(buf, pos.x, pos.y, w, h, theme);
        }
        DrawableKind::AreaRug { pos, width, height } => {
            paint_area_rug(buf, pos.x, pos.y, *width, *height, theme);
        }
        DrawableKind::LoungeSideTable { pos } => {
            paint_side_table(buf, pos.x, pos.y, theme);
        }
        DrawableKind::KitchenIsland { pos } => {
            paint_kitchen_island(buf, pos.x, pos.y, theme);
        }
        DrawableKind::SnackShelf { pos } => {
            if let Some(f) = pack.animation("snack_shelf").and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width() / 2);
                let py = pos.y.saturating_sub(f.height() / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::Plant { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width() / 2);
                let py = pos.y.saturating_sub(f.height() / 2);
                // Occlusion is the sprite's own job: the foliage overhangs north
                // of the mask's shallow south-anchored pot strip, so a walker
                // parks deep behind the pot and the leaves (y-sorted over them)
                // hide their lower body. No synthetic back-cap.
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::PodDecorItem { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width() / 2);
                let py = pos.y.saturating_sub(f.height() / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::FloorLamp { pos } => {
            if let Some(f) = pack.animation("floor_lamp").and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width() / 2);
                let py = pos.y.saturating_sub(f.height() / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::Door { pos, frame_idx } => {
            if let Some(f) = pack.animation("door").and_then(|a| frame_at(a, *frame_idx)) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::WallDecor { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                // The free-standing board's panel overhangs its south-anchored
                // wheel strip; a walker behind it is occluded by the panel's own
                // y-sort. Wall-hung decor has no footprint and nothing behind it.
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::VendingMachine { pos, busy } => {
            paint_vending_machine(buf, *pos, *busy, now, theme);
        }
        DrawableKind::Printer { pos, busy } => {
            paint_printer(buf, *pos, *busy, now, theme);
        }
        DrawableKind::Pet {
            kind,
            pos,
            flip,
            anim_name,
            frame_idx,
            pet_elapsed_ms,
        } => {
            let Some(anim) = pack.animation(anim_name) else {
                return;
            };
            let Some(frame) = frame_at(anim, *frame_idx) else {
                return;
            };
            let final_frame = if *flip {
                frame.mirror_horizontal()
            } else {
                frame.clone()
            };
            let px = pos.x.saturating_sub(final_frame.width() / 2);
            let py = pos.y.saturating_sub(final_frame.height() / 2);
            blit_frame(&final_frame, px, py, buf);
            if let Some(elapsed) = pet_elapsed_ms {
                paint_pet_hearts(buf, *pos, *elapsed);
            } else if *anim_name == kind.sleep_anim() {
                paint_sleep_z(buf, *pos, now, 0xCAFE, theme);
            }
        }
        DrawableKind::GatewayMascot {
            pos,
            anim_name,
            frame_idx,
            run_count,
            degraded,
        } => {
            let Some(anim) = pack.animation(anim_name) else {
                return;
            };
            let Some(frame) = frame_at(anim, *frame_idx) else {
                return;
            };
            let px = pos.x.saturating_sub(frame.width() / 2);
            let py = pos.y.saturating_sub(frame.height() / 2);
            // Degraded (#317): blit a sickly-red tinted copy of the frame.
            if *degraded {
                blit_frame(&super::palette::degraded_frame(frame), px, py, buf);
            } else {
                blit_frame(frame, px, py, buf);
            }
            // Busy (an in-flight agent run) → a rising activity-bubble stream
            // above the lobster's head. `run_count > 0` IS the busy gate (busy ⟺
            // in-flight runs); a persistent idle session must NOT bubble.
            if *run_count > 0 {
                paint_mascot_bubbles(buf, *pos, frame.height(), *run_count, now);
            }
        }
        DrawableKind::RoomWallH {
            x0,
            x1,
            y_top,
            jamb_left,
            jamb_right,
        } => {
            super::paint_glass_wall_h(buf, theme, *x0, *x1, *y_top);
            // Jambs ride the y-sorted glass (the background pass would be
            // overpainted by it). The post sits ON this segment's cut end.
            if *jamb_left {
                super::paint_door_jamb_h(buf, theme, *x0, *y_top);
            }
            if *jamb_right {
                super::paint_door_jamb_h(
                    buf,
                    theme,
                    x1.saturating_sub(super::DOOR_JAMB_PX - 1),
                    *y_top,
                );
            }
        }
        DrawableKind::RoomWallV {
            x,
            y_top,
            y_bot,
            jamb_north,
            jamb_south,
        } => {
            super::paint_glass_wall_v(buf, theme, *x, *y_top, *y_bot);
            // Jambs ride the y-sorted glass (same z as the strip). The post sits
            // ON this segment's cut end: north jamb from the top row down, south
            // jamb ending on the bottom row (both `DOOR_JAMB_PX` deep) — the exact
            // rows the old background `paint_door_frame_v` covered.
            if *jamb_north {
                super::paint_door_jamb_v(buf, theme, *x, *y_top);
            }
            if *jamb_south {
                super::paint_door_jamb_v(
                    buf,
                    theme,
                    *x,
                    y_bot.saturating_sub(super::DOOR_JAMB_PX - 1),
                );
            }
        }
        DrawableKind::FishTank { pos } => {
            paint_fish_tank(buf, *pos, now, theme);
        }
        DrawableKind::MeetingChair { pos, back_west } => {
            paint_meeting_chair(buf, *pos, *back_west, theme);
        }
        DrawableKind::CoatRack { pos } => {
            paint_coat_rack(buf, *pos, theme);
        }
    }
}

fn paint_desk_coffee(
    buf: &mut RgbBuffer,
    desk: Point,
    has_coffee: bool,
    coffee_steam: bool,
    now: SystemTime,
    theme: &crate::theme::Theme,
) {
    if !has_coffee {
        return;
    }
    let put = |buf: &mut RgbBuffer, x: u16, y: u16, c: Rgb| {
        if x < buf.width() && y < buf.height() {
            buf.put(x, y, c);
        }
    };
    let cx = desk.x + 2;
    let cy = desk.y + 2;
    put(buf, cx, cy, theme.furniture.coffee_cup);
    put(buf, cx + 1, cy, theme.furniture.coffee_cup);
    put(buf, cx, cy + 1, theme.furniture.coffee_cup_shadow);
    put(buf, cx + 1, cy + 1, theme.furniture.coffee_cup_shadow);
    if coffee_steam {
        paint_coffee_steam(buf, Point { x: cx, y: cy }, now, theme);
    }
}

/// Token-meter paper tower (#632): `tier` reams (2px each, 3px wide) stacked
/// on the desk surface against the monitor's east side (sprite cols 11-13 —
/// the right wood wing, the coffee cup's mirror), growing NORTH past the
/// bezel at T3 so the silhouette reads across the room. The T3 top sheet
/// teeters 1px east ("about to topple" = the maxed-out statement). Alternate
/// rows use `paper_shade` so the block reads as stacked reams, not a slab.
/// A `sheet_fall` mid-drop paints one loose sheet above the stack top —
/// mock-verified at half-block scale before this implementation (the
/// pin-visual round: the 3px-wide low "wing pile" variant was rejected as
/// sub-legible; the vertical tower won on silhouette).
///
/// Tier 0 suppresses the SHEET too, deliberately: a sheet needs a pile to
/// land on (one flashing onto a bare desk then vanishing reads as an
/// artifact), it keeps the tier-0 desk byte-identical (default-on safety),
/// and the early return is what makes the `h - 1` math below safe.
fn paint_token_stack(
    buf: &mut RgbBuffer,
    desk: Point,
    tier: u8,
    sheet_fall: Option<u16>,
    theme: &crate::theme::Theme,
) {
    if tier == 0 {
        return;
    }
    let put = |buf: &mut RgbBuffer, x: u16, y: u16, c: Rgb| {
        if x < buf.width() && y < buf.height() {
            buf.put(x, y, c);
        }
    };
    // Base row = the desk surface (same row the coffee cup's shadow sits on).
    let base_y = desk.y + STACK_BASE_DY;
    let h = tier as u16 * STACK_PX_PER_TIER;
    for i in 0..h {
        let y = base_y.saturating_sub(i);
        let c = if i % 2 == 1 {
            theme.furniture.paper_shade
        } else {
            theme.furniture.paper
        };
        let teeter = tier == crate::token_meter::MAX_TIER && i == h - 1;
        let dx = u16::from(teeter);
        for xoff in 0..STACK_W {
            put(buf, desk.x + STACK_X_OFF + xoff + dx, y, c);
        }
    }
    if let Some(dist) = sheet_fall {
        // The loose sheet starts SHEET_FALL_PX above the stack top and has
        // fallen `dist`; at landing it merges into the pile (not painted).
        let stack_top = base_y.saturating_sub(h - 1);
        let remaining = crate::token_meter::SHEET_FALL_PX.saturating_sub(dist);
        if remaining > 0 {
            let sy = stack_top.saturating_sub(remaining);
            for xoff in 0..STACK_W {
                put(buf, desk.x + STACK_X_OFF + xoff, sy, theme.furniture.paper);
            }
        }
    }
}

/// Tower geometry, all relative to the 14×8 desk sprite: the stack hugs the
/// monitor's east side on the right wood wing (cols 11-13), its base on the
/// surface row `desk.y + 3`. 2px per ream: the beautify skill pins 1px
/// vertical detail as sub-legible — a 2px step is the smallest that reads.
const STACK_X_OFF: u16 = 11;
const STACK_W: u16 = 3;
const STACK_BASE_DY: u16 = 3;
const STACK_PX_PER_TIER: u16 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_anchor_sits_within_the_coffee_machine_columns() {
        // The steam plume must land inside the machine's clickable column range
        // (the binary hit-tests the same PANTRY_COFFEE_COLS_*), so a re-tuned
        // sprite can't drift the steam off the machine or out of the hit box.
        // steam_x = pos.x + steam_dx; sprite_x = pos.x - cw/2 → sprite-local steam
        // col = steam_dx + cw/2.
        for (dx, cw, (lo, hi)) in [
            (
                PANTRY_STEAM_DX_LARGE,
                32i16,
                crate::pixel_painter::PANTRY_COFFEE_COLS_LARGE,
            ),
            (
                PANTRY_STEAM_DX_SMALL,
                20i16,
                crate::pixel_painter::PANTRY_COFFEE_COLS_SMALL,
            ),
        ] {
            let steam_col = dx + cw / 2;
            assert!(
                steam_col >= lo as i16 && steam_col < hi as i16,
                "steam col {steam_col} must sit within the machine cols [{lo},{hi})"
            );
        }
    }

    fn test_pack() -> Pack {
        crate::embedded_pack::test_default_pack()
    }

    fn desk_cubicle_drawable(
        desk: Point,
        token_tier: u8,
        sheet_fall: Option<u16>,
    ) -> Drawable<'static> {
        Drawable {
            anchor_y: desk.y
                + crate::layout::furniture_def(crate::layout::Furniture::Desk)
                    .visual
                    .h,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col: true,
                has_cabinet: false,
                screen_glow: None,
                has_coffee: false,
                coffee_steam: false,
                token_tier,
                sheet_fall,
            },
        }
    }

    fn paper_pixel_count(buf: &RgbBuffer, th: &crate::theme::Theme) -> usize {
        let mut n = 0;
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                let c = buf.get(x, y);
                if c == th.furniture.paper || c == th.furniture.paper_shade {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn tier_zero_desk_paints_no_paper() {
        // Default-on safety (#632): a desk with no usage renders byte-free of
        // the paper palette — sources with no usage wire look exactly as
        // before the meter existed.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let th = theme();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, None);
        paint_drawable(&d, &mut buf, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
        assert_eq!(paper_pixel_count(&buf, th), 0);
        // …including a mid-fall sheet: a big EARLY reading (delta cleared the
        // sheet minimum before cumulative reached T1) paints nothing — a
        // sheet needs a pile to land on (see paint_token_stack's doc).
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, Some(2));
        paint_drawable(&d, &mut buf, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
        assert_eq!(paper_pixel_count(&buf, th), 0);
    }

    #[test]
    fn token_stack_grows_two_px_per_tier_with_a_t3_teeter() {
        let pack = test_pack();
        let th = theme();
        let desk = Point { x: 40, y: 30 };
        let base_y = desk.y + STACK_BASE_DY;
        let mut counts = Vec::new();
        for tier in 1..=3u8 {
            let mut cache = FrameCache::new();
            let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
            let d = desk_cubicle_drawable(desk, tier, None);
            paint_drawable(&d, &mut buf, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
            counts.push(paper_pixel_count(&buf, th));
            // Base row always paints paper across the 3-wide stack column.
            for xoff in 0..STACK_W {
                assert_eq!(
                    buf.get(desk.x + STACK_X_OFF + xoff, base_y),
                    th.furniture.paper,
                    "tier {tier} base row col {xoff}"
                );
            }
            // The row just above the stack top stays unpainted (height pins
            // the tier; the T3 teeter shifts east, it doesn't grow taller).
            let top_y = base_y - (tier as u16 * STACK_PX_PER_TIER - 1);
            let above = buf.get(desk.x + STACK_X_OFF, top_y - 1);
            assert!(
                above != th.furniture.paper && above != th.furniture.paper_shade,
                "tier {tier} must top out at {top_y}"
            );
        }
        // Strictly taller stacks per tier…
        assert!(counts[0] < counts[1] && counts[1] < counts[2], "{counts:?}");
        // …and the T3 top sheet teeters: its east overhang column is painted.
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 3, None);
        paint_drawable(&d, &mut buf, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
        let t3_top = base_y - (3 * STACK_PX_PER_TIER - 1);
        let overhang = buf.get(desk.x + STACK_X_OFF + STACK_W, t3_top);
        assert!(
            overhang == th.furniture.paper || overhang == th.furniture.paper_shade,
            "T3 top sheet must overhang 1px east, got {overhang:?}"
        );
    }

    #[test]
    fn falling_sheet_paints_above_the_stack_and_lands_silently() {
        let pack = test_pack();
        let th = theme();
        let desk = Point { x: 40, y: 30 };
        let base_y = desk.y + STACK_BASE_DY;
        let stack_top = base_y - (STACK_PX_PER_TIER - 1);
        // Mid-fall: fallen 2 of SHEET_FALL_PX → the sheet paints at
        // stack_top − (FALL_PX − 2).
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 1, Some(2));
        paint_drawable(&d, &mut buf, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
        let sy = stack_top - (crate::token_meter::SHEET_FALL_PX - 2);
        assert_eq!(buf.get(desk.x + STACK_X_OFF, sy), th.furniture.paper);
        // Fully fallen: the sheet has merged into the pile — nothing paints
        // above the stack top.
        let mut cache = FrameCache::new();
        let mut buf2 = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 1, Some(crate::token_meter::SHEET_FALL_PX));
        paint_drawable(&d, &mut buf2, &pack, &mut cache, SystemTime::UNIX_EPOCH, th);
        for y in 0..stack_top {
            for xoff in 0..STACK_W {
                let c = buf2.get(desk.x + STACK_X_OFF + xoff, y);
                assert!(
                    c != th.furniture.paper && c != th.furniture.paper_shade,
                    "landed sheet must not linger at ({xoff},{y})"
                );
            }
        }
    }

    fn theme() -> &'static crate::theme::Theme {
        crate::theme::theme_by_name("normal").expect("theme")
    }

    #[test]
    fn desk_cubicle_blits_cabinet_but_no_per_desk_bin() {
        // A DeskCubicle with has_cabinet=true paints the filing cabinet (west
        // of the desk) and NOTHING at the old east-edge bin cell — the owner
        // removed the per-desk trash bins (25 grey cylinders read as noise at
        // half-block scale; the office keeps the pantry's one bin).
        let pack = test_pack();
        // Pin the ASSET removal directly, not just its pixels: the embedded
        // pack no longer ships the animation, so a blit re-add alone is dead
        // code.
        assert!(pack.animation("trash_bin").is_none());
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let desk = Point { x: 40, y: 30 };
        let cab = pack
            .animation("filing_cabinet")
            .and_then(|a| a.frames.first())
            .expect("filing_cabinet anim");
        let bg = Rgb { r: 1, g: 2, b: 3 };
        let mut buf = RgbBuffer::filled(120, 80, bg);
        let d = Drawable {
            anchor_y: desk.y
                + crate::layout::furniture_def(crate::layout::Furniture::Desk)
                    .visual
                    .h,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col: true,
                has_cabinet: true,
                screen_glow: None,
                has_coffee: false,
                coffee_steam: false,
                token_tier: 0,
                sheet_fall: None,
            },
        };
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
        // Cabinet lands at desk.x - cab.width - 1 .. ; sample a pixel inside it.
        let cab_x = desk.x.saturating_sub(cab.width() + 1);
        let mut cab_painted = false;
        for dy in 0..cab.height() {
            for dx in 0..cab.width() {
                if buf.get(cab_x + dx, desk.y + dy) != bg {
                    cab_painted = true;
                }
            }
        }
        assert!(cab_painted, "filing cabinet should paint west of the desk");
        // The old bin cell (desk.x + DESK_W, desk.y + 4, 3x4) may show the
        // desk sprite's own east columns, but never the bin's chrome grey
        // ('K' #a8a8b0 — the removed cylinder's body color).
        let bin_grey = Rgb {
            r: 0xa8,
            g: 0xa8,
            b: 0xb0,
        };
        for dy in 0..4u16 {
            for dx in 0..3u16 {
                assert_ne!(
                    buf.get(desk.x + DESK_W + dx, desk.y + 4 + dy),
                    bin_grey,
                    "no per-desk bin pixels at the old east-edge cell"
                );
            }
        }
    }

    #[test]
    fn meeting_sofa_mirrored_flips_vertically() {
        // A mirrored MeetingSofa paints the vertically-flipped sprite — assert it
        // differs from the unmirrored render (the `mirrored=true` arm).
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let pos = Point { x: 30, y: 30 };
        let mut render = |mirrored: bool| {
            let mut buf = RgbBuffer::filled(80, 80, Rgb { r: 0, g: 0, b: 0 });
            let d = Drawable {
                anchor_y: pos.y,
                kind: DrawableKind::MeetingSofa { pos, mirrored },
            };
            paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
            buf
        };
        let plain = render(false);
        let flipped = render(true);
        let mut differs = false;
        for y in 0..80u16 {
            for x in 0..80u16 {
                if plain.get(x, y) != flipped.get(x, y) {
                    differs = true;
                }
            }
        }
        assert!(differs, "mirrored sofa must render distinct pixels");
    }

    #[test]
    fn pet_drawable_missing_anim_is_a_noop() {
        // A Pet drawable whose anim_name is absent from the pack early-returns
        // (the `let Some(anim) = ... else { return }` defensive guard) and paints
        // nothing.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let bg = Rgb { r: 7, g: 8, b: 9 };
        let mut buf = RgbBuffer::filled(60, 60, bg);
        let d = Drawable {
            anchor_y: 30,
            kind: DrawableKind::Pet {
                kind: PetKind::Cat,
                pos: Point { x: 30, y: 30 },
                flip: false,
                anim_name: "nonexistent_anim",
                frame_idx: 0,
                pet_elapsed_ms: None,
            },
        };
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), bg, "missing pet anim must paint nothing");
            }
        }
    }

    #[test]
    fn pet_drawable_sleep_anim_paints_sleep_z() {
        // A Pet drawable with the sleep anim and pet_elapsed_ms=None takes the
        // sleep-z branch (paints the floating z's glyph near the pet).
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let pos = Point { x: 30, y: 40 };
        let mut render = |anim_name: &'static str| {
            let mut buf = RgbBuffer::filled(60, 60, Rgb { r: 0, g: 0, b: 0 });
            let d = Drawable {
                anchor_y: pos.y,
                kind: DrawableKind::Pet {
                    kind: PetKind::Cat,
                    pos,
                    flip: false,
                    anim_name,
                    frame_idx: 0,
                    pet_elapsed_ms: None,
                },
            };
            paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
            buf
        };
        // Count non-background pixels ABOVE the pet (where the z's float) — the
        // sleep render should add some vs. the sit render.
        let count_above = |buf: &RgbBuffer| {
            let mut n = 0u32;
            for y in 0..pos.y.saturating_sub(4) {
                for x in 0..60u16 {
                    if buf.get(x, y) != (Rgb { r: 0, g: 0, b: 0 }) {
                        n += 1;
                    }
                }
            }
            n
        };
        let sit = count_above(&render(PetKind::Cat.sit_anim()));
        let sleep = count_above(&render(PetKind::Cat.sleep_anim()));
        assert!(
            sleep > sit,
            "sleep anim must add floating z's above the pet (sleep={sleep}, sit={sit})"
        );
    }

    #[test]
    fn vending_machine_paints_panel_drinks_and_trim_cells() {
        // The 4×6 vending block maps each (dx,dy) cell to a specific theme appliance
        // color. Pin the per-cell mapping at the exact vx/vy-relative positions.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let th = theme();
        let pos = Point { x: 30, y: 30 };
        let bg = Rgb { r: 1, g: 2, b: 3 };
        let mut buf = RgbBuffer::filled(80, 80, bg);
        let d = Drawable {
            anchor_y: pos.y,
            kind: DrawableKind::VendingMachine { pos, busy: false },
        };
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, th);
        let vx = pos.x - 2;
        let vy = pos.y - 3;
        // dy==0 row → panel.
        assert_eq!(
            buf.get(vx, vy),
            th.appliance.vending_panel,
            "top row = panel"
        );
        // dy==1,dx==1 → drinks[0] (idx = (dy-1)*2 + (dx-1) = 0).
        assert_eq!(
            buf.get(vx + 1, vy + 1),
            th.appliance.vending_drinks[0],
            "first drink slot = drinks[0]"
        );
        // dy==4,dx==2 → trim.
        assert_eq!(
            buf.get(vx + 2, vy + 4),
            th.appliance.vending_trim,
            "the (2,4) cell = trim"
        );
        // dy==5 → dark base row.
        assert_eq!(
            buf.get(vx, vy + 5),
            th.appliance.vending_dark,
            "bottom row = dark"
        );
        // A plain body cell (dy==2, dx==0) → body.
        assert_eq!(
            buf.get(vx, vy + 2),
            th.appliance.vending_body,
            "a non-special cell = body"
        );
    }

    #[test]
    fn printer_paints_glass_paper_and_tray_cells() {
        // The 5×4 printer block maps each (dx,dy) to a specific appliance color.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let th = theme();
        let pos = Point { x: 30, y: 30 };
        let bg = Rgb { r: 4, g: 5, b: 6 };
        let mut buf = RgbBuffer::filled(80, 80, bg);
        let d = Drawable {
            anchor_y: pos.y,
            kind: DrawableKind::Printer { pos, busy: false },
        };
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, th);
        let px0 = pos.x - 2;
        let py0 = pos.y - 2;
        // dy==0, dx in 1..=3 → glass.
        assert_eq!(
            buf.get(px0 + 2, py0),
            th.appliance.printer_glass,
            "top-centre = glass"
        );
        // dy==0, dx==0 → top_dark.
        assert_eq!(
            buf.get(px0, py0),
            th.appliance.printer_top,
            "top-corner = top_dark"
        );
        // dy==3, dx in 1..=3 → paper.
        assert_eq!(
            buf.get(px0 + 2, py0 + 3),
            th.appliance.printer_paper,
            "bottom-centre = paper"
        );
        // dx==0, mid row (dy==1) → tray (the dx==0||dx==4 side arm).
        assert_eq!(
            buf.get(px0, py0 + 1),
            th.appliance.printer_tray,
            "side column = tray"
        );
        // an interior body cell (dy==1, dx==2) → body_white.
        assert_eq!(
            buf.get(px0 + 2, py0 + 1),
            th.appliance.printer_body,
            "interior = body"
        );
    }

    #[test]
    fn gateway_mascot_missing_anim_is_a_noop() {
        // A GatewayMascot whose anim_name is absent early-returns (913-914) and
        // paints nothing — the exact analogue of pet_drawable_missing_anim_is_a_noop.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let bg = Rgb { r: 7, g: 8, b: 9 };
        let mut buf = RgbBuffer::filled(60, 60, bg);
        let d = Drawable {
            anchor_y: 30,
            kind: DrawableKind::GatewayMascot {
                pos: Point { x: 30, y: 30 },
                anim_name: "nonexistent_anim",
                frame_idx: 0,
                run_count: 0,
                degraded: false,
            },
        };
        paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), bg, "missing mascot anim must paint nothing");
            }
        }
    }

    #[test]
    fn gateway_mascot_degraded_renders_distinct_pixels() {
        // degraded:true blits palette::degraded_frame (a sickly-red tinted copy) at
        // 923 instead of the raw frame at 925 — so the rendered buffer differs
        // pixel-for-pixel from the degraded:false render.
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let pos = Point { x: 30, y: 30 };
        let def =
            crate::creatures::gateway_mascot_def(pixtuoid_core::source::openclaw::SOURCE_NAME)
                .expect("openclaw mascot def");
        let black = Rgb { r: 0, g: 0, b: 0 };
        let mut render = |degraded: bool| {
            let mut buf = RgbBuffer::filled(80, 80, black);
            let d = Drawable {
                anchor_y: pos.y,
                kind: DrawableKind::GatewayMascot {
                    pos,
                    anim_name: def.rest,
                    frame_idx: 0,
                    run_count: 0,
                    degraded,
                },
            };
            paint_drawable(&d, &mut buf, &pack, &mut cache, now, theme());
            buf
        };
        let plain = render(false);
        let degraded = render(true);
        // Both must actually paint something (else the "differs" test is vacuous).
        let mut plain_painted = false;
        let mut differs = false;
        for y in 0..80u16 {
            for x in 0..80u16 {
                let pp = plain.get(x, y);
                if pp != black {
                    plain_painted = true;
                }
                if pp != degraded.get(x, y) {
                    differs = true;
                }
            }
        }
        assert!(plain_painted, "the plain lobster must actually render");
        assert!(
            differs,
            "the degraded lobster must render distinct (tinted) pixels vs the plain one"
        );
    }
}
