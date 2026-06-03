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
    paint_coffee_steam, paint_pet_hearts, paint_screen_glow, paint_sleep_z, paint_thinking_dots,
    paint_waiting_bubble, paint_walking_dust,
};
use super::furniture::{
    paint_area_rug, paint_coffee_table, paint_pantry_chair, paint_pantry_table, paint_side_table,
};
use super::paint_character_at;
use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, Point, DESK_H, DESK_W};
use crate::tui::pathfind::{find_path, snap_point_to_walkable};
use crate::tui::pet::PetKind;
use pixtuoid_core::walkable::OccupancyOverlay;

pub(super) struct Drawable<'a> {
    pub(super) anchor_y: u16,
    pub(super) kind: DrawableKind<'a>,
}

pub(super) enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit: divider + filing cabinet (every
    /// other desk) + desk sprite + trash bin + screen-glow if the
    /// occupant is Active. Bundled so the cubicle paints atomically at
    /// the desk's bottom-edge row.
    DeskCubicle {
        desk: Point,
        is_last_col: bool,
        has_cabinet: bool,
        screen_glow: Option<Rgb>,
        session_age_secs: u64,
        has_coffee: bool,
        coffee_steam: bool,
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
        thinking_dots: bool,
        walking_dust_frame: Option<usize>,
    },
    /// Lounge couch (mirror_vertical'd — back at bottom, seat at top).
    WaypointCouch {
        pos: Point,
    },
    /// Pantry counter (with coffee steam attached so steam rides above
    /// the counter in z-order). `use_large` picks the detailed 32×10
    /// kitchen sprite vs. the 20×8 compact fallback — derived from
    /// `layout.pantry_counter_size` at queue time.
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
    PantryTable {
        pos: Point,
    },
    PantryChair {
        pos: Point,
    },
    Plant {
        kind: crate::tui::layout::PlantKind,
        pos: Point,
    },
    /// Aisle decor item between desk pods (plant / whiteboard / TV /
    /// phone booth / standing desk). All are obstacles in the
    /// walkable mask; phone booth + standing desk additionally exist
    /// as waypoints so agents can wander to them.
    PodDecorItem {
        kind: crate::tui::layout::PodDecor,
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
        kind: crate::tui::layout::WallDecor,
        pos: Point,
    },
    VendingMachine {
        pos: Point,
    },
    Printer {
        pos: Point,
    },
    Pet {
        kind: PetKind,
        pos: Point,
        flip: bool,
        anim_name: &'static str,
        frame_idx: usize,
        pet_elapsed_ms: Option<u64>,
    },
    /// Horizontal (E-W) frosted-glass room divider, y-sorted at its south
    /// (front) edge so it composites over a character standing behind it.
    RoomWallH {
        x0: u16,
        x1: u16,
        y_top: u16,
    },
    /// Meeting-room coat rack (pole + base + coat blobs), y-sorted at its base
    /// row so a character walking in front of it occludes it (and one behind
    /// is occluded BY it) — was painted in the background pass, always under
    /// every character. `cy` is the pole top; the base sits at `cy + 7`.
    CoatRack {
        cx: u16,
        cy: u16,
    },
}

/// Pet roaming the whole office. Each 40s cycle picks a destination
/// from all available spots (desks, pantry, meeting sofas, lounge
/// couch, corridor), walks there from the previous spot, then sits or
/// sleeps until the next cycle.
pub(super) fn pet_position(
    kind: PetKind,
    layout: &Layout,
    pack: &Pack,
    now: SystemTime,
    idle_desk_indices: &[usize],
    all_idle: bool,
    pet_seed: u64,
) -> Option<(Point, bool, &'static str, usize)> {
    pack.animation(kind.walk_anim())?;
    layout.corridor?;

    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    const CYCLE_MS: u64 = 40_000;
    let cycle_n = (elapsed_ms / CYCLE_MS).wrapping_add(pet_seed);
    let frac = (elapsed_ms % CYCLE_MS) as f32 / CYCLE_MS as f32;

    // Gather all interesting spots the cat can visit.
    let mut spots: Vec<(Point, bool)> = Vec::new();
    for (i, desk) in layout.home_desks.iter().enumerate() {
        spots.push((
            Point {
                x: desk.x + DESK_W + 1,
                y: desk.y + DESK_H + 2,
            },
            idle_desk_indices.contains(&i),
        ));
    }
    if let Some(wp) = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, crate::tui::layout::WaypointKind::Pantry))
    {
        spots.push((
            Point {
                x: wp.pos.x + 4,
                y: wp.pos.y + 6,
            },
            false,
        ));
    }
    for sofa in &layout.meeting_sofas {
        spots.push((
            Point {
                x: sofa.x + 4,
                y: sofa.y + 4,
            },
            false,
        ));
    }
    if let Some(wp) = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, crate::tui::layout::WaypointKind::Couch))
    {
        spots.push((
            Point {
                x: wp.pos.x + 4,
                y: wp.pos.y + 6,
            },
            false,
        ));
    }
    if let Some(corridor) = layout.corridor {
        spots.push((
            Point {
                x: corridor.x + corridor.width / 2,
                y: corridor.y + corridor.height / 2,
            },
            false,
        ));
    }
    if spots.is_empty() {
        return None;
    }

    let pick = |n: u64| -> (Point, bool) {
        let h = n.wrapping_mul(0x9e37_79b9_7f4a_7c15) as usize;
        spots[h % spots.len()]
    };
    let (dest, is_idle_spot) = pick(cycle_n);
    let (prev, _) = pick(cycle_n.wrapping_sub(1));

    let frame_idx = (elapsed_ms / 220) as usize % 2;

    if frac < 0.35 {
        let t = (frac / 0.35).clamp(0.0, 1.0);
        // Facing follows the raw destination intent, independent of where the
        // snapped anchors land.
        let flip = dest.x < prev.x;
        // Pre-snap both endpoints to walkable cells so the leg starts AND ends
        // on floor — the raw furniture-adjacent spots are often blocked.
        let src_anchor = snap_point_to_walkable(&layout.walkable, prev).unwrap_or(prev);
        let dst_anchor = snap_point_to_walkable(&layout.walkable, dest).unwrap_or(dest);

        // A* on the STATIC mask with a throwaway EMPTY overlay: identical inputs
        // every frame of the leg (static mask + empty overlay + deterministic
        // prev/dest) ⇒ identical polyline ⇒ no flash, no per-frame state. The
        // empty overlay is deliberate — the pet ignores live-agent occupancy
        // (occasional sprite overlap is fine; a per-frame reroute flash is not).
        let empty_overlay = OccupancyOverlay::new();
        let pos = if let Some(mut pts) = find_path(
            &layout.walkable,
            &empty_overlay,
            layout.corridor,
            prev,
            dest,
        ) {
            // `reconstruct` writes the RAW prev/dest as the polyline ends, which
            // may be blocked — overwrite them with the snapped walkable anchors
            // so every sample (incl. t=0 and t=1) is on floor.
            if let Some(first) = pts.first_mut() {
                *first = src_anchor;
            }
            if let Some(last) = pts.last_mut() {
                *last = dst_anchor;
            }
            sample_polyline(&pts, t, dst_anchor)
        } else {
            // Degenerate layout (no route): straight lerp between snapped anchors
            // — still strictly better than lerping between the raw blocked spots.
            Point {
                x: (src_anchor.x as f32 + (dst_anchor.x as f32 - src_anchor.x as f32) * t) as u16,
                y: (src_anchor.y as f32 + (dst_anchor.y as f32 - src_anchor.y as f32) * t) as u16,
            }
        };
        return Some((pos, flip, kind.walk_anim(), frame_idx));
    }

    // Rest phase: snap to a walkable cell so the sit/sleep pose isn't on
    // furniture. Same snapped anchor as the leg END ⇒ no pop at the boundary.
    let rest_pos = snap_point_to_walkable(&layout.walkable, dest).unwrap_or(dest);
    let anim = if all_idle || (kind.sleeps_near_idle() && is_idle_spot) {
        kind.sleep_anim()
    } else {
        kind.sit_anim()
    };
    Some((rest_pos, false, anim, 0))
}

/// Sample a polyline at arc-length fraction `t ∈ [0, 1]`, using octile segment
/// length so a diagonal leg doesn't move faster than a cardinal one. `t >= 1`
/// returns `fallback` (the caller's snapped goal) exactly — no float overshoot
/// onto a non-last cell. Precondition: `pts` non-empty (find_path guarantees it).
fn sample_polyline(pts: &[Point], t: f32, fallback: Point) -> Point {
    let Some(&last_pt) = pts.last() else {
        return fallback;
    };
    if pts.len() == 1 || t >= 1.0 {
        return last_pt;
    }
    let mut seg_lens: Vec<f32> = Vec::with_capacity(pts.len() - 1);
    let mut total = 0.0_f32;
    for w in pts.windows(2) {
        let dx = (w[1].x as i32 - w[0].x as i32).unsigned_abs() as f32;
        let dy = (w[1].y as i32 - w[0].y as i32).unsigned_abs() as f32;
        let len = dx.max(dy) + dx.min(dy) * (std::f32::consts::SQRT_2 - 1.0);
        seg_lens.push(len);
        total += len;
    }
    if total < 1e-3 {
        return last_pt;
    }
    let target = (t * total).min(total);
    let mut cumul = 0.0_f32;
    for (i, &slen) in seg_lens.iter().enumerate() {
        let is_last_seg = i == seg_lens.len() - 1;
        if cumul + slen >= target || is_last_seg {
            let local_t = if slen < 1e-3 {
                0.0
            } else {
                ((target - cumul) / slen).clamp(0.0, 1.0)
            };
            let a = pts[i];
            let b = pts[i + 1];
            return Point {
                x: (a.x as f32 + (b.x as f32 - a.x as f32) * local_t) as u16,
                y: (a.y as f32 + (b.y as f32 - a.y as f32) * local_t) as u16,
            };
        }
        cumul += slen;
    }
    last_pt
}

/// Dispatch one Drawable's paint. Effects attached to characters paint
/// inline so they ride along with the character in z-order.
pub(super) fn paint_drawable(
    d: &Drawable<'_>,
    buf: &mut RgbBuffer,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
    theme: &crate::tui::theme::Theme,
) {
    match &d.kind {
        DrawableKind::DeskCubicle {
            desk,
            is_last_col,
            has_cabinet,
            screen_glow,
            session_age_secs,
            has_coffee,
            coffee_steam,
        } => {
            let divider = theme.office.cubicle_divider;
            if !is_last_col {
                let div_x = desk.x + DESK_W + 3;
                for dy in 0..(DESK_H + 1) {
                    let py = desk.y.saturating_sub(1) + dy;
                    if div_x < buf.width && py < buf.height {
                        buf.put(div_x, py, divider);
                    }
                }
            }
            if *has_cabinet {
                if let Some(cab) = pack
                    .animation("filing_cabinet")
                    .and_then(|a| a.frames.first())
                {
                    let cab_x = desk.x.saturating_sub(cab.width + 1);
                    let cab_y = desk.y;
                    if cab_y + cab.height <= buf.height {
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
            if let Some(bin) = pack.animation("trash_bin").and_then(|a| a.frames.first()) {
                let bin_x = desk.x + DESK_W;
                let bin_y = desk.y + 4;
                if bin_x + bin.width <= buf.width && bin_y + bin.height <= buf.height {
                    blit_frame(bin, bin_x, bin_y, buf);
                }
            }
            paint_desk_personalization(
                buf,
                *desk,
                *session_age_secs,
                *has_coffee,
                *coffee_steam,
                now,
                theme,
            );
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
            thinking_dots,
            walking_dust_frame,
        } => {
            if let Some(dust_frame) = walking_dust_frame {
                paint_walking_dust(buf, *anchor, *dust_frame, theme);
            }
            paint_character_at(
                buf, anim_name, *frame_idx, *anchor, agent, pack, *flip_x, *glow_tint, cache,
            );
            if let Some(seed) = sleep_z_seed {
                paint_sleep_z(buf, *anchor, now, *seed, theme);
            }
            if *waiting_bubble {
                paint_waiting_bubble(buf, *anchor, theme);
            }
            if *thinking_dots {
                paint_thinking_dots(buf, *anchor, now, theme);
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
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
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
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
                // A character behind the counter is occluded by the counter's own
                // sprite (it y-sorts at the south base → paints over a north-
                // stander). The mask south-anchors a shallow strip to that base so
                // the walker parks deep behind the visual; no synthetic cap.
                blit_frame(f, cx, cy, buf);
            }
            // Large sprite: coffee machine at sprite cols 11-18 of
            // a 32-wide sprite → world x ≈ pos.x - 2.
            // Small sprite: coffee at sprite cols 9-11 of a 20-wide
            // sprite → world x = pos.x + 1.
            let steam_dx: i16 = if *use_large { -2 } else { 1 };
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
                let sx = pos.x.saturating_sub(f.width / 2);
                let sy = pos.y.saturating_sub(f.height / 2);
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
            // the painted coffee table can't drift from the masked obstacle.
            let (w, h) =
                crate::tui::layout::furniture_def(crate::tui::layout::Furniture::MeetingTable)
                    .visual;
            paint_coffee_table(buf, pos.x, pos.y, w, h, theme);
        }
        DrawableKind::AreaRug { pos, width, height } => {
            paint_area_rug(buf, pos.x, pos.y, *width, *height, theme);
        }
        DrawableKind::LoungeSideTable { pos } => {
            paint_side_table(buf, pos.x, pos.y, theme);
        }
        DrawableKind::PantryTable { pos } => {
            paint_pantry_table(buf, pos.x, pos.y, theme);
        }
        DrawableKind::PantryChair { pos } => {
            paint_pantry_chair(buf, pos.x, pos.y, theme);
        }
        DrawableKind::Plant { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
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
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::FloorLamp { pos } => {
            if let Some(f) = pack.animation("floor_lamp").and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::Door { pos, frame_idx } => {
            if let Some(f) = pack
                .animation("door")
                .and_then(|a| a.frames.get(*frame_idx).or_else(|| a.frames.first()))
            {
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
        DrawableKind::VendingMachine { pos } => {
            let body = theme.appliance.vending_body;
            let panel = theme.appliance.vending_panel;
            let drinks = theme.appliance.vending_drinks;
            let vx = pos.x.saturating_sub(2);
            let vy = pos.y.saturating_sub(3);
            for dy in 0..6u16 {
                for dx in 0..4u16 {
                    let px = vx + dx;
                    let py = vy + dy;
                    if px < buf.width && py < buf.height {
                        let color = if dy == 0 {
                            panel
                        } else if (1..=3).contains(&dy) && (1..=2).contains(&dx) {
                            let idx = ((dy - 1) * 2 + (dx - 1)) as usize;
                            if idx < drinks.len() {
                                drinks[idx]
                            } else {
                                body
                            }
                        } else if dy == 4 && dx == 2 {
                            theme.appliance.vending_trim
                        } else if dy == 5 {
                            theme.appliance.vending_dark
                        } else {
                            body
                        };
                        buf.put(px, py, color);
                    }
                }
            }
        }
        DrawableKind::Printer { pos } => {
            let body_white = theme.appliance.printer_body;
            let top_dark = theme.appliance.printer_top;
            let glass = theme.appliance.printer_glass;
            let paper = theme.appliance.printer_paper;
            let tray = theme.appliance.printer_tray;
            let px0 = pos.x.saturating_sub(2);
            let py0 = pos.y.saturating_sub(2);
            for dy in 0..4u16 {
                for dx in 0..5u16 {
                    let px = px0 + dx;
                    let py = py0 + dy;
                    if px < buf.width && py < buf.height {
                        let color = if dy == 0 {
                            if (1..=3).contains(&dx) {
                                glass
                            } else {
                                top_dark
                            }
                        } else if dy == 3 {
                            if (1..=3).contains(&dx) {
                                paper
                            } else {
                                tray
                            }
                        } else if dx == 0 || dx == 4 {
                            tray
                        } else {
                            body_white
                        };
                        buf.put(px, py, color);
                    }
                }
            }
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
            let Some(frame) = anim.frames.get(*frame_idx).or(anim.frames.first()) else {
                return;
            };
            let final_frame = if *flip {
                frame.mirror_horizontal()
            } else {
                frame.clone()
            };
            let px = pos.x.saturating_sub(final_frame.width / 2);
            let py = pos.y.saturating_sub(final_frame.height / 2);
            blit_frame(&final_frame, px, py, buf);
            if let Some(elapsed) = pet_elapsed_ms {
                paint_pet_hearts(buf, *pos, *elapsed);
            } else if *anim_name == kind.sleep_anim() {
                paint_sleep_z(buf, *pos, now, 0xCAFE, theme);
            }
        }
        DrawableKind::RoomWallH { x0, x1, y_top } => {
            super::paint_glass_wall_h(buf, theme, *x0, *x1, *y_top);
        }
        DrawableKind::CoatRack { cx, cy } => {
            let (cx, cy) = (*cx, *cy);
            let pole = theme.furniture.wood_trim;
            let base = theme.furniture.wood_top;
            let coats = theme.appliance.coats;
            // Pole (1px wide, 8 tall).
            for dy in 0..8u16 {
                let py = cy + dy;
                if py < buf.height && cx < buf.width {
                    buf.put(cx, py, pole);
                }
            }
            // Base (3px wide) at the rack's south row.
            let by = cy + 7;
            for dx in 0..3u16 {
                let px = cx.saturating_sub(1) + dx;
                if px < buf.width && by < buf.height {
                    buf.put(px, by, base);
                }
            }
            // Coat blobs (2×2 blocks on alternating hooks).
            for (i, &coat_color) in coats.iter().enumerate() {
                let hook_y = cy + 1 + (i as u16) * 2;
                let side: i16 = if i % 2 == 0 { -1 } else { 1 };
                let hx = (cx as i16 + side) as u16;
                for dy in 0..2u16 {
                    for dx in 0..2u16 {
                        let px = hx.wrapping_add(if side < 0 { dx.wrapping_sub(1) } else { dx });
                        let py = hook_y + dy;
                        if px < buf.width && py < buf.height {
                            buf.put(px, py, coat_color);
                        }
                    }
                }
            }
        }
    }
}

fn paint_desk_personalization(
    buf: &mut RgbBuffer,
    desk: Point,
    age_secs: u64,
    has_coffee: bool,
    coffee_steam: bool,
    now: SystemTime,
    theme: &crate::tui::theme::Theme,
) {
    if age_secs == 0 && !has_coffee {
        return;
    }
    let put = |buf: &mut RgbBuffer, x: u16, y: u16, c: Rgb| {
        if x < buf.width && y < buf.height {
            buf.put(x, y, c);
        }
    };
    if has_coffee {
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
    if age_secs >= 1800 {
        let px = desk.x + DESK_W - 2;
        let py = desk.y + 1;
        put(buf, px, py, theme.furniture.desk_plant_light);
        put(buf, px + 1, py, theme.furniture.desk_plant_dark);
        put(buf, px, py + 1, theme.furniture.desk_plant_light);
        put(buf, px + 1, py + 1, theme.furniture.desk_plant_light);
        put(buf, px, py + 2, theme.furniture.desk_plant_pot);
        put(buf, px + 1, py + 2, theme.furniture.desk_plant_pot);
    }
    if age_secs >= 3600 {
        let fx = desk.x + 1;
        let fy = desk.y;
        put(buf, fx, fy, theme.furniture.photo_frame);
        put(buf, fx + 1, fy, theme.furniture.photo_frame);
        put(buf, fx, fy + 1, theme.furniture.photo_bg);
        put(buf, fx + 1, fy + 1, theme.furniture.photo_bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: u16, y: u16) -> Point {
        Point { x, y }
    }

    #[test]
    fn sample_polyline_empty_returns_fallback() {
        assert_eq!(sample_polyline(&[], 0.5, p(9, 9)), p(9, 9));
    }

    #[test]
    fn sample_polyline_single_point_returns_it() {
        assert_eq!(sample_polyline(&[p(3, 4)], 0.5, p(9, 9)), p(3, 4));
    }

    #[test]
    fn sample_polyline_t_at_or_past_one_returns_last() {
        let pts = [p(0, 0), p(10, 0)];
        assert_eq!(sample_polyline(&pts, 1.0, p(9, 9)), p(10, 0));
        assert_eq!(sample_polyline(&pts, 2.5, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_t_zero_returns_first() {
        assert_eq!(sample_polyline(&[p(0, 0), p(10, 0)], 0.0, p(9, 9)), p(0, 0));
    }

    #[test]
    fn sample_polyline_midpoint_on_straight_segment() {
        assert_eq!(sample_polyline(&[p(0, 0), p(10, 0)], 0.5, p(9, 9)), p(5, 0));
    }

    #[test]
    fn sample_polyline_arc_length_hits_corner_of_l() {
        // L: (0,0)->(10,0) len 10, ->(10,10) len 10; total 20. t=0.5 → arc 10 →
        // exactly the corner.
        let pts = [p(0, 0), p(10, 0), p(10, 10)];
        assert_eq!(sample_polyline(&pts, 0.5, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_octile_weights_diagonal() {
        // Cardinal leg len 10, diagonal leg octile len ≈14.14; total ≈24.14.
        // Sampling at arc-distance 10/total lands exactly on the corner — proves
        // the diagonal is weighted by octile length, not raw point count.
        let pts = [p(0, 0), p(10, 0), p(20, 10)];
        let total = 10.0 + 10.0 * std::f32::consts::SQRT_2;
        assert_eq!(sample_polyline(&pts, 10.0 / total, p(9, 9)), p(10, 0));
    }

    #[test]
    fn sample_polyline_zero_length_leading_segment_no_div_by_zero() {
        // Duplicate first point (zero-length segment) must not panic.
        let pts = [p(5, 5), p(5, 5), p(15, 5)];
        assert_eq!(sample_polyline(&pts, 0.5, p(0, 0)), p(10, 5));
    }
}
