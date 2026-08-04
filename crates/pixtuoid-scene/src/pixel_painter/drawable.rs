//! Y-sorted drawable enum (painter's algorithm).
//!
//! Every mid-ground entity carries an `anchor_y` = the y-pixel row where it
//! touches the floor (front-facing bottom edge for items with thickness).
//! Drawables sort ascending by `anchor_y`, so larger `anchor_y` = closer to
//! camera = paints last.

use std::time::SystemTime;

use pixtuoid_core::sprite::blit::{blit_frame, blit_frame_scaled};
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Rgb, RgbBuffer};
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
use crate::layout::{Point, Size};
use crate::pet::PetKind;

/// Coffee-steam plume column offset from the pantry sprite CENTER (`pos.x`), per
/// size — hand-tuned to the sprite art so the steam sits within
/// [`super::PANTRY_COFFEE_COLS_LARGE`] / [`super::PANTRY_COFFEE_COLS_SMALL`].
const PANTRY_STEAM_DX_LARGE: i16 = -2;
const PANTRY_STEAM_DX_SMALL: i16 = 1;

// Vending pickup-slot offset from the sprite's top-left — the ONE cell where
// the idle trim paints and the busy can-drop lands.
pub(crate) const VENDING_PICKUP_SLOT: (u16, u16) = (2, 4);

/// Vending machine + printer body sizes — CENTER-anchored on their waypoint
/// `pos` (origin = `pos − body/2`).
pub(crate) const VENDING_BODY: Size = Size { w: 4, h: 6 };
pub(crate) const PRINTER_BODY: Size = Size { w: 5, h: 4 };

pub(super) struct Drawable<'a> {
    pub(super) anchor_y: u16,
    pub(super) kind: DrawableKind<'a>,
}

pub(super) enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit, so it paints atomically at the desk's
    /// bottom-edge row.
    DeskCubicle {
        desk: Point,
        /// Buffer column of this cubicle's pod divider, or `None` when the desk
        /// has no east pod-mate to be divided FROM (the row's last desk, or a
        /// pod whose second column the band clamp dropped).
        divider_x: Option<u16>,
        has_cabinet: bool,
        screen_glow: Option<Rgb>,
        has_coffee: bool,
        coffee_steam: bool,
        /// 0 = no tower (byte-identical to the pre-meter desk), 1..=3 = reams.
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
        /// Tool-derived monitor glow color; tints the skin so a row of typing
        /// agents shows tool type at a glance. `None` for non-desk poses.
        glow_tint: Option<Rgb>,
        sleep_z_seed: Option<u64>,
        waiting_bubble: bool,
        walking_dust_frame: Option<usize>,
    },
    /// Pantry counter, with coffee steam attached so the steam rides above it
    /// in z-order. `use_large` picks the detailed 32×10 kitchen sprite vs. the
    /// 20×8 compact fallback.
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
    /// Area rug, painted BEFORE the furniture in z-order (`anchor_y` at the top
    /// of the rug) so chairs / couches sit on top.
    AreaRug {
        pos: Point,
        width: u16,
        height: u16,
    },
    /// Lounge side table (5×3 wood + magazine), centred at `pos`.
    LoungeSideTable {
        pos: Point,
    },
    /// Kitchen-island body, centred at `pos`.
    KitchenIsland {
        pos: Point,
    },
    /// Snack shelf, centred at `pos` — the tall shelf overhangs its shallow
    /// 2-row base (walk-behind class).
    SnackShelf {
        pos: Point,
    },
    Plant {
        kind: crate::layout::PlantKind,
        pos: Point,
    },
    PodDecorItem {
        kind: crate::layout::PodDecor,
        pos: Point,
    },
    FloorLamp {
        pos: Point,
    },
    Door {
        pos: Point,
        /// Frame index into the `door` animation: 0 = closed, 1 = half-open,
        /// 2 = fully open.
        frame_idx: usize,
    },
    WallDecor {
        kind: crate::layout::WallDecor,
        pos: Point,
    },
    VendingMachine {
        pos: Point,
        /// An agent stands here this frame — drives the drink-drop animation.
        busy: bool,
    },
    Printer {
        pos: Point,
        /// An agent stands here this frame — drives the page-eject animation.
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
    /// The gateway lobster mascot — a presence-gated wandering creature, NOT an
    /// agent (lives in `daemons`, not `scene.agents`); y-sorted at its south row
    /// like a pet.
    GatewayMascot {
        pos: Point,
        anim_name: &'static str,
        frame_idx: usize,
        run_count: u32,
        /// Gateway up but model-broken → render the lobster sickly red.
        degraded: bool,
    },
    /// Horizontal (E-W) frosted-glass room divider, y-sorted at its south
    /// (front) edge so it composites over a character standing behind it.
    RoomWallH {
        x0: u16,
        x1: u16,
        y_top: u16,
        /// This end abuts a doorway ⇒ paint its dark jamb.
        jamb_left: bool,
        jamb_right: bool,
    },
    /// Vertical (N-S, edge-on) frosted-glass room divider. `y_top`/`y_bot` are
    /// the stitched PAINT extent (`stitch_vertical_wall`); the z-key is the raw
    /// south end so a corner-extended `y_bot` doesn't flip H-over-V.
    RoomWallV {
        x: u16,
        y_top: u16,
        y_bot: u16,
        /// This segment's north/south end abuts a doorway ⇒ paint its dark jamb
        /// on that cut end.
        jamb_north: bool,
        jamb_south: bool,
    },
    /// Meeting-room coat rack, y-sorted at its base row. `pos` is the pole top;
    /// the base sits at `pos.y + 7`.
    CoatRack {
        pos: Point,
    },
    /// Lounge aquarium, y-sorted at its cabinet's south row. `pos` is the sprite
    /// CENTER (matches the mask stamp's `Anchor::Center`).
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

/// Busy "working" cue — bubbles rising above the lobster's head while a run is
/// in flight, one per concurrent run over a small baseline (capped).
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
        let phase = ((now_ms / 110) + i as u64 * 7) % 6;
        let by = top.saturating_sub(phase as u16);
        let bx = (pos.x + i * 2).saturating_sub(n);
        if bx < buf.width() && by < buf.height() {
            buf.put(bx, by, bubble);
        }
    }
}

/// Blit `frame` CENTRED on `pos` (origin = `pos − size/2`, saturating).
fn blit_centered(
    frame: &Frame,
    pos: Point,
    scale: crate::render_scale::RenderScale,
    buf: &mut RgbBuffer,
) {
    // Centre in LOGICAL space, THEN convert. Centring in buffer space would
    // halve a scaled sprite's width, drifting odd-width art half a logical unit
    // off the footprint its mask stamped.
    let px = pos.x.saturating_sub(frame.width() / 2);
    let py = pos.y.saturating_sub(frame.height() / 2);
    blit_frame_scaled(
        frame,
        scale.to_buffer(px),
        scale.to_buffer(py),
        scale.factor(),
        buf,
    );
}

/// Look up `anim_name`, take its FIRST frame, and [`blit_centered`] it on `pos`
/// — a no-op if the pack lacks the animation.
fn blit_centered_first_frame(
    pack: &Pack,
    anim_name: &str,
    pos: Point,
    scale: crate::render_scale::RenderScale,
    buf: &mut RgbBuffer,
) {
    if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
        blit_centered(f, pos, scale, buf);
    }
}

/// Dispatch one Drawable's paint; character-attached effects paint inline so
/// they ride along with the character in z-order.
///
/// The paint-time context one [`Drawable`] arm needs — the subset of `PaintCtx`
/// they touch. Bundled rather than passed flat: this was six positional
/// parameters and the render scale would have made seven, the growth
/// `PixelCtx`/`PaintCtx` already answered the same way.
pub(super) struct DrawableCtx<'a> {
    pub buf: &'a mut RgbBuffer,
    pub pack: &'a Pack,
    pub cache: &'a mut FrameCache,
    pub now: SystemTime,
    pub theme: &'a crate::theme::Theme,
    pub scale: crate::render_scale::RenderScale,
}

pub(super) fn paint_drawable(d: &Drawable<'_>, c: &mut DrawableCtx<'_>) {
    // Re-bound to the original names so the arms below are untouched.
    let buf = &mut *c.buf;
    let cache = &mut *c.cache;
    let (pack, now, theme, scale) = (c.pack, c.now, c.theme, c.scale);
    match &d.kind {
        DrawableKind::DeskCubicle {
            desk,
            divider_x,
            has_cabinet,
            screen_glow,
            has_coffee,
            coffee_steam,
            token_tier,
            sheet_fall,
        } => {
            let divider = theme.office.cubicle_divider;
            if let Some(div_x) = *divider_x {
                // Spans the desk sprite's own painted rows so it reads as tall
                // as the workstation.
                let visual_h = crate::layout::desk_furniture_def().visual.h;
                for dy in 0..=visual_h {
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
                // The desk sprite's top row is the monitor's raised bezel, 1px
                // above the desk back, so blit 1px higher.
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
        DrawableKind::WaypointPantry { pos, use_large } => {
            let anim_name = if *use_large { "pantry" } else { "pantry_small" };
            // A character behind the counter is occluded by the counter's own
            // sprite (it y-sorts at the south base, and the mask south-anchors a
            // shallow strip there) — no synthetic cap needed.
            blit_centered_first_frame(pack, anim_name, *pos, scale, buf);
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
                // Mirrored (south sofa / lounge couch): back faces NORTH toward
                // the windows.
                if *mirrored {
                    blit_centered(&f.mirror_vertical(), *pos, scale, buf);
                } else {
                    blit_centered(f, *pos, scale, buf);
                }
            }
        }
        DrawableKind::MeetingTable { pos } => {
            // Size from the furniture def (== footprint here) so the painted
            // table can't drift from the masked obstacle.
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
            blit_centered_first_frame(pack, "snack_shelf", *pos, scale, buf);
        }
        DrawableKind::Plant { kind, pos } => {
            // Occlusion is the sprite's own job: the foliage overhangs north of
            // the mask's shallow south-anchored pot strip, so it hides a walker
            // parked behind it. No synthetic back-cap.
            blit_centered_first_frame(pack, kind.sprite_name(), *pos, scale, buf);
        }
        DrawableKind::PodDecorItem { kind, pos } => {
            blit_centered_first_frame(pack, kind.sprite_name(), *pos, scale, buf);
        }
        DrawableKind::FloorLamp { pos } => {
            blit_centered_first_frame(pack, "floor_lamp", *pos, scale, buf);
        }
        DrawableKind::Door { pos, frame_idx } => {
            if let Some(f) = pack.animation("door").and_then(|a| frame_at(a, *frame_idx)) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::WallDecor { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
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
            // Declared out here so the flipped path's temporary outlives the `if`.
            let mirrored;
            let final_frame = if *flip {
                mirrored = frame.mirror_horizontal();
                &mirrored
            } else {
                frame
            };
            blit_centered(final_frame, *pos, scale, buf);
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
            if *degraded {
                blit_centered(&super::palette::degraded_frame(frame), *pos, scale, buf);
            } else {
                blit_centered(frame, *pos, scale, buf);
            }
            // The busy tell keys on in-flight RUNS, not the (persistent,
            // single-user) session count, which sticks at 1 at rest.
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
            // Jambs ride the y-sorted glass — the background pass would be
            // overpainted by it.
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
        buf.put_checked(x, y, c);
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

/// Token-meter paper tower: `tier` reams stacked on the desk surface against
/// the monitor's east side, growing NORTH past the bezel at T3 so the
/// silhouette reads across the room; the T3 top sheet teeters 1px east.
///
/// Tier 0 suppresses the SHEET too, deliberately: a sheet needs a pile to land
/// on, it keeps the tier-0 desk byte-identical, and the early return is what
/// makes the `h - 1` math below safe.
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
        buf.put_checked(x, y, c);
    };
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
        // The sheet starts SHEET_FALL_PX above the stack top and has fallen
        // `dist`; at landing it merges into the pile (not painted).
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

/// Tower geometry, relative to the 14×8 desk sprite: the stack hugs the
/// monitor's east side on the right wood wing, its base on the surface row.
/// 2px per ream — 1px of vertical detail is sub-legible at half-block scale.
const STACK_X_OFF: u16 = 11;
const STACK_W: u16 = 3;
const STACK_BASE_DY: u16 = 3;
const STACK_PX_PER_TIER: u16 = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DESK_W;

    #[test]
    fn steam_anchor_sits_within_the_coffee_machine_columns() {
        // steam_x = pos.x + steam_dx; sprite_x = pos.x - cw/2 → sprite-local
        // steam col = steam_dx + cw/2.
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
                divider_x: None,
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
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let th = theme();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, None);
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now: SystemTime::UNIX_EPOCH,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        assert_eq!(paper_pixel_count(&buf, th), 0);
        // …including a mid-fall sheet: a big EARLY reading can clear the sheet
        // minimum before cumulative usage reaches T1.
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, Some(2));
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now: SystemTime::UNIX_EPOCH,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
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
            paint_drawable(
                &d,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    now: SystemTime::UNIX_EPOCH,
                    theme: th,
                    scale: crate::render_scale::RenderScale::ONE,
                },
            );
            counts.push(paper_pixel_count(&buf, th));
            for xoff in 0..STACK_W {
                assert_eq!(
                    buf.get(desk.x + STACK_X_OFF + xoff, base_y),
                    th.furniture.paper,
                    "tier {tier} base row col {xoff}"
                );
            }
            let top_y = base_y - (tier as u16 * STACK_PX_PER_TIER - 1);
            let above = buf.get(desk.x + STACK_X_OFF, top_y - 1);
            assert!(
                above != th.furniture.paper && above != th.furniture.paper_shade,
                "tier {tier} must top out at {top_y}"
            );
        }
        assert!(counts[0] < counts[1] && counts[1] < counts[2], "{counts:?}");
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 3, None);
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now: SystemTime::UNIX_EPOCH,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
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
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 1, Some(2));
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now: SystemTime::UNIX_EPOCH,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        let sy = stack_top - (crate::token_meter::SHEET_FALL_PX - 2);
        assert_eq!(buf.get(desk.x + STACK_X_OFF, sy), th.furniture.paper);
        let mut cache = FrameCache::new();
        let mut buf2 = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 1, Some(crate::token_meter::SHEET_FALL_PX));
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf2,
                pack: &pack,
                cache: &mut cache,
                now: SystemTime::UNIX_EPOCH,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
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
        let pack = test_pack();
        // Pin the ASSET removal, not just its pixels: with no `trash_bin` in
        // the pack, a re-added blit would be dead code.
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
                divider_x: None,
                has_cabinet: true,
                screen_glow: None,
                has_coffee: false,
                coffee_steam: false,
                token_tier: 0,
                sheet_fall: None,
            },
        };
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme: theme(),
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
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
        // The removed bin's chrome grey. The old bin cell may show the desk
        // sprite's own east columns, but never this.
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
            paint_drawable(
                &d,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    now,
                    theme: theme(),
                    scale: crate::render_scale::RenderScale::ONE,
                },
            );
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
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme: theme(),
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), bg, "missing pet anim must paint nothing");
            }
        }
    }

    #[test]
    fn pet_drawable_sleep_anim_paints_sleep_z() {
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
            paint_drawable(
                &d,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    now,
                    theme: theme(),
                    scale: crate::render_scale::RenderScale::ONE,
                },
            );
            buf
        };
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
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        let vx = pos.x - VENDING_BODY.w / 2;
        let vy = pos.y - VENDING_BODY.h / 2;
        assert_eq!(
            buf.get(vx, vy),
            th.appliance.vending_panel,
            "top row = panel"
        );
        // Drink index = (dy-1)*2 + (dx-1).
        assert_eq!(
            buf.get(vx + 1, vy + 1),
            th.appliance.vending_drinks[0],
            "first drink slot = drinks[0]"
        );
        assert_eq!(
            buf.get(vx + 2, vy + 4),
            th.appliance.vending_trim,
            "the (2,4) cell = trim"
        );
        assert_eq!(
            buf.get(vx, vy + 5),
            th.appliance.vending_dark,
            "bottom row = dark"
        );
        assert_eq!(
            buf.get(vx, vy + 2),
            th.appliance.vending_body,
            "a non-special cell = body"
        );
    }

    #[test]
    fn printer_paints_glass_paper_and_tray_cells() {
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
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme: th,
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        let px0 = pos.x - PRINTER_BODY.w / 2;
        let py0 = pos.y - PRINTER_BODY.h / 2;
        assert_eq!(
            buf.get(px0 + 2, py0),
            th.appliance.printer_glass,
            "top-centre = glass"
        );
        assert_eq!(
            buf.get(px0, py0),
            th.appliance.printer_top,
            "top-corner = top_dark"
        );
        assert_eq!(
            buf.get(px0 + 2, py0 + 3),
            th.appliance.printer_paper,
            "bottom-centre = paper"
        );
        assert_eq!(
            buf.get(px0, py0 + 1),
            th.appliance.printer_tray,
            "side column = tray"
        );
        assert_eq!(
            buf.get(px0 + 2, py0 + 1),
            th.appliance.printer_body,
            "interior = body"
        );
    }

    #[test]
    fn blit_centered_lands_top_left_at_pos_minus_half_size() {
        // The 3×2 frame's ODD width pins the FLOOR division (3/2 == 1, not a
        // rounded 2) — a rounding change would move only this case, and the
        // "two renders differ" sofa/pet tests shift equally either way.
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let marker = Rgb { r: 9, g: 8, b: 7 };
        let frame = Frame::from_pixels(3, 2, vec![Some(marker); 6]);
        let mut buf = RgbBuffer::filled(20, 20, bg);
        blit_centered(
            &frame,
            Point { x: 10, y: 10 },
            crate::render_scale::RenderScale::ONE,
            &mut buf,
        );
        assert_eq!(buf.get(9, 9), marker, "top-left lands at pos − size/2");
        assert_eq!(buf.get(11, 10), marker, "bottom-right at (9+2, 9+1)");
        assert_eq!(buf.get(8, 9), bg, "one column west of the frame stays bg");
        assert_eq!(buf.get(9, 8), bg, "one row north of the frame stays bg");
    }

    /// The sibling of the test above, at scale. Centring must happen in
    /// LOGICAL space and convert ONCE: centring in buffer space would halve
    /// the already-scaled width, drifting the sprite off the footprint its
    /// mask stamped — a drift no single-scale test can see.
    #[test]
    fn a_centred_blit_scales_both_its_position_and_its_art() {
        let bg = Rgb { r: 0, g: 0, b: 0 };
        let marker = Rgb { r: 9, g: 8, b: 7 };
        let two = crate::render_scale::RenderScale::new(2).expect("2 is nonzero");
        // Same 3x2 odd-width frame, so the floor division is pinned here too:
        // logical top-left (9, 9) -> buffer (18, 18), art 3x2 -> 6x4 pixels.
        let frame = Frame::from_pixels(3, 2, vec![Some(marker); 6]);
        let mut buf = RgbBuffer::filled(40, 40, bg);
        blit_centered(&frame, Point { x: 10, y: 10 }, two, &mut buf);

        assert_eq!(buf.get(18, 18), marker, "top-left at scale.to_buffer(9)");
        assert_eq!(buf.get(23, 21), marker, "bottom-right at (18+5, 18+3)");
        assert_eq!(buf.get(17, 18), bg, "one column west stays bg");
        assert_eq!(
            buf.get(24, 18),
            bg,
            "one column east of the 6px span stays bg"
        );
        assert_eq!(buf.get(18, 17), bg, "one row north stays bg");
    }

    #[test]
    fn gateway_mascot_missing_anim_is_a_noop() {
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
        paint_drawable(
            &d,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                now,
                theme: theme(),
                scale: crate::render_scale::RenderScale::ONE,
            },
        );
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                assert_eq!(buf.get(x, y), bg, "missing mascot anim must paint nothing");
            }
        }
    }

    #[test]
    fn gateway_mascot_degraded_renders_distinct_pixels() {
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
            paint_drawable(
                &d,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    now,
                    theme: theme(),
                    scale: crate::render_scale::RenderScale::ONE,
                },
            );
            buf
        };
        let plain = render(false);
        let degraded = render(true);
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
