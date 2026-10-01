//! Y-sorted drawable enum (painter's algorithm).
//!
//! Every mid-ground entity carries an `anchor_y` = the y-pixel row where it
//! touches the floor (front-facing bottom edge for items with thickness).
//! Drawables sort ascending by `anchor_y` and then [`Layer`], so larger
//! `anchor_y` = closer to camera = paints last. A backdrop fixture is a
//! [`DrawableKind`] the background pass paints flat instead.

use std::time::SystemTime;

use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Rgb, RgbBuffer};

use crate::sim::{Cup, DeskProps, desk_cup_at};
use pixtuoid_core::AgentSlot;

use super::AgentFrame;
use super::background::{paint_clock, paint_corridor_runner, paint_neon_panel};
use super::effects::{paint_effects, paint_screen_glow, paint_screen_idle};
use super::furniture::{
    paint_area_rug, paint_coat_rack, paint_doormat, paint_fish_tank, paint_kitchen_island,
    paint_meeting_chair, paint_notice_board, paint_side_table, paint_trash_bin, paint_water_cooler,
};
use crate::character::{CharacterFrame, SpritePose, character_frame};
use crate::effects::{Effect, STEAM_PUFFS};
use crate::frame_cache::FrameCache;
pub(super) use crate::layout::Layer;
use crate::layout::{Point, SceneLayout, Size};
use crate::pack::{DESK_CHAIR_SPRITE, MEETING_TABLE_SPRITE, desk_art, desk_art_top, frame_at};

/// Coffee-steam plume column offset from the pantry sprite CENTER (`pos.x`), per
/// size — hand-tuned to the sprite art so the steam sits within the coffee
/// machine ([`SceneLayout::coffee_machine`](crate::layout::SceneLayout::coffee_machine)).
const PANTRY_STEAM_DX_LARGE: i16 = -2;
const PANTRY_STEAM_DX_SMALL: i16 = 1;

/// The steam offset for `anim`, a [`pantry_counter_anim`](crate::layout::pantry_counter_anim) pick.
fn pantry_steam_dx(anim: &str) -> i16 {
    let [_, large] = crate::layout::PANTRY_COUNTER_ANIMS;
    if anim == large {
        PANTRY_STEAM_DX_LARGE
    } else {
        PANTRY_STEAM_DX_SMALL
    }
}

/// Where the steam rises from a pantry counter centred at `pos` drawn as
/// `anim`.
pub(super) fn pantry_steam_at(pos: Point, anim: &str) -> Point {
    Point {
        x: (pos.x as i32 + pantry_steam_dx(anim) as i32).max(0) as u16,
        y: pos.y.saturating_sub(2),
    }
}

pub(super) struct Drawable<'a> {
    pub(super) anchor_y: u16,
    pub(super) layer: Layer,
    pub(super) kind: DrawableKind<'a>,
}

/// Sorts `drawables` into paint order. Stable, so drawables tied on row and
/// layer keep their queue order: the roster's among fixtures.
pub(super) fn sort_drawables(drawables: &mut [Drawable<'_>]) {
    drawables.sort_by_key(|d| (d.anchor_y, d.layer));
}

pub(super) enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit, so it paints atomically at the desk's
    /// bottom-edge row.
    DeskCubicle {
        desk: Point,
        /// Which way this desk seats its occupant; picks the art (`desk_sprite_name`).
        facing: crate::layout::Facing,
        screen_glow: Option<Rgb>,
        lights: crate::lighting::DeskLights,
        props: DeskProps,
    },
    Character {
        agent: &'a AgentSlot,
        pose: SpritePose,
        anchor: Point,
        label_anchor: Point,
        effects: &'a [Effect],
    },
    FilingCabinet {
        pos: Point,
    },
    /// Office chair, sorted at its occupant's row in [`Layer::Over`], so it
    /// paints over them.
    DeskChair {
        pos: Point,
    },
    /// Pantry counter, with coffee steam attached so the steam rides above it
    /// in z-order. `anim` is [`pantry_counter_anim`](crate::layout::pantry_counter_anim)'s pick.
    WaypointPantry {
        pos: Point,
        anim: &'static str,
        steam: [Effect; STEAM_PUFFS],
    },
    MeetingSofa {
        pos: Point,
        /// Flip the art top-to-bottom. `meeting_sofa` draws its backrest in its
        /// top rows, so a mirrored sofa's back is its SOUTH edge and it seats
        /// people facing north: the south sofa of a meeting trio, and the
        /// lounge couch facing the windows.
        mirrored: bool,
    },
    MeetingTable {
        pos: Point,
    },
    /// A rug or mat filling its box, under the furniture that stands on it.
    AreaRug(crate::layout::Bounds),
    /// Lounge side table (wood + magazine), centred at `pos`.
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
    /// A corridor appliance: its pack art ([`crate::pack::appliance_sprite`]), centred at
    /// `pos`.
    Appliance {
        pos: Point,
        sprite: &'static str,
        /// An agent stands here this frame: the art plays its busy loop.
        busy: bool,
    },
    Pet {
        pos: Point,
        flip: bool,
        anim_name: &'static str,
        frame_idx: usize,
        effects: &'a [Effect],
    },
    /// The gateway lobster mascot — a presence-gated wandering creature, NOT an
    /// agent (lives in `daemons`, not `scene.agents`); y-sorted at its south row
    /// like a pet.
    GatewayMascot {
        /// Its index in [`SimFrame::mascots`](super::SimFrame::mascots).
        mascot_idx: usize,
        pos: Point,
        anim_name: &'static str,
        frame_idx: usize,
        effects: &'a [Effect],
        /// Gateway up but model-broken → render the lobster sickly red.
        degraded: bool,
    },
    /// One [sort band](crate::layout::WallPiece::sort_bands) of a glass room
    /// wall, so it composites over a character standing behind it.
    RoomWall {
        piece: crate::layout::WallPiece,
        rows: std::ops::Range<u16>,
    },
    /// Meeting-room coat rack, y-sorted at its base row (the bottom of
    /// `coat_rack_rect_at` its pole top). `pos` is the pole top.
    CoatRack {
        pos: Point,
    },
    /// Lounge aquarium, y-sorted at its cabinet's south row. `pos` is the sprite
    /// CENTER (matches the mask stamp's `Pivot::Center`).
    FishTank {
        pos: Point,
    },
    /// Head-of-table meeting chair, sorted at its sitter's row in
    /// [`Layer::Under`], so the sitter paints over it.
    MeetingChair {
        pos: Point,
        back_west: bool,
    },
    WaterCooler(crate::layout::Bounds),
    TrashBin(crate::layout::Bounds),
    Doormat(crate::layout::Bounds),
    NoticeBoard(crate::layout::Bounds),
    Runner(crate::layout::Bounds),
    /// The neon sign's panel in this frame's colours.
    NeonSign {
        panel: crate::layout::Bounds,
        look: crate::floor::NeonLook,
    },
    Clock {
        pos: Point,
    },
}

/// Blit `frame` CENTRED on `pos` (origin = `pos − size/2`, saturating).
///
/// Layout units ARE buffer pixels in this pass, so the centring is plain
/// integer arithmetic. If a scaled classic painter is ever built, halve the
/// sprite in LOGICAL space and convert AFTERWARDS — halving an already-scaled
/// width drifts odd-width art half a logical unit off the footprint its mask
/// stamped, and no scale-1 test can see it.
fn blit_centered(frame: &Frame, pos: Point, buf: &mut RgbBuffer) {
    let px = pos.x.saturating_sub(frame.width() / 2);
    let py = pos.y.saturating_sub(frame.height() / 2);
    blit_frame(frame, px, py, buf);
}

/// Look up `anim_name`, take its FIRST frame, and [`blit_centered`] it on `pos`
/// — a no-op if the pack lacks the animation.
fn blit_centered_first_frame(pack: &Pack, anim_name: &str, pos: Point, buf: &mut RgbBuffer) {
    if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames().first()) {
        blit_centered(f, pos, buf);
    }
}

/// The subset of `PaintCtx` a [`Drawable`] arm paints with.
pub(super) struct DrawableCtx<'a> {
    pub buf: &'a mut RgbBuffer,
    pub pack: &'a Pack,
    pub cache: &'a mut FrameCache,
    pub clock: crate::anim::Clock,
    pub theme: &'a crate::theme::Theme,
}

/// A hoverable [`paint_drawable`] drew, sized by the frame it blitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Drawn {
    Agent(AgentFrame),
    Mascot { mascot_idx: usize, w: u16, h: u16 },
}

/// Dispatch one Drawable's paint; character-attached effects paint inline so
/// they ride along with the character in z-order.
pub(super) fn paint_drawable(kind: &DrawableKind<'_>, c: &mut DrawableCtx<'_>) -> Option<Drawn> {
    let buf = &mut *c.buf;
    let cache = &mut *c.cache;
    let (pack, theme) = (c.pack, c.theme);
    let crate::anim::Clock { now, beat } = c.clock;
    match kind {
        DrawableKind::DeskCubicle {
            desk,
            facing,
            screen_glow,
            lights,
            props,
        } => {
            let art = desk_art(pack, *facing);
            // The effects address the monitor by the sprite's OWN row numbering, so this is
            // the blit origin; passing `sprite_top + DESK_BEZEL_RAISE` caps every glow with a bar.
            let mut sprite_top = desk.y;
            if let Some(frame) = art {
                sprite_top = desk_art_top(pack, desk.y, frame.height());
                blit_frame(frame, desk.x, sprite_top, buf);
            }
            paint_desk_lamp_pool(buf, lights, theme);
            paint_screen_idle(
                buf,
                desk.x,
                sprite_top,
                theme.effects.monitor_idle,
                lights.screen_idle,
            );
            paint_desk_coffee(buf, *desk, props.cup, &props.effects, theme);
            paint_token_stack(buf, *desk, props.token_tier, props.sheet_fall, theme);
            if let Some(tint) = screen_glow {
                paint_screen_glow(buf, desk.x, sprite_top, props.scanline, *tint, theme);
            }
        }
        DrawableKind::Character {
            agent,
            pose,
            anchor,
            label_anchor,
            effects,
        } => {
            paint_effects(buf, effects.iter().filter(|e| e.kind.beneath()), theme);
            let drawn = paint_character_at(buf, *pose, *anchor, agent, pack, cache, now);
            paint_effects(buf, effects.iter().filter(|e| !e.kind.beneath()), theme);
            return drawn.map(|Size { w, h }| {
                Drawn::Agent(AgentFrame {
                    agent_id: agent.agent_id,
                    anchor: *anchor,
                    w,
                    h,
                    label_anchor: *label_anchor,
                })
            });
        }
        DrawableKind::FilingCabinet { pos } => {
            if let Some(cab) = pack
                .animation("filing_cabinet")
                .and_then(|a| a.frames().first())
            {
                blit_frame(cab, pos.x, pos.y, buf);
            }
        }
        DrawableKind::DeskChair { pos } => paint_chair_back(buf, *pos, pack),
        DrawableKind::WaypointPantry { pos, anim, steam } => {
            // A character behind the counter is occluded by the counter's own
            // sprite (it y-sorts at the south base, and the mask south-anchors a
            // shallow strip there) — no synthetic cap needed.
            blit_centered_first_frame(pack, anim, *pos, buf);
            paint_effects(buf, steam, theme);
        }
        DrawableKind::MeetingSofa { pos, mirrored } => {
            if let Some(f) = pack
                .animation("meeting_sofa")
                .and_then(|a| a.frames().first())
            {
                if *mirrored {
                    blit_centered(&f.mirror_vertical(), *pos, buf);
                } else {
                    blit_centered(f, *pos, buf);
                }
            }
        }
        DrawableKind::MeetingTable { pos } => {
            blit_centered_first_frame(pack, MEETING_TABLE_SPRITE, *pos, buf);
        }
        DrawableKind::AreaRug(rug) => paint_area_rug(buf, *rug, theme),
        DrawableKind::LoungeSideTable { pos } => {
            paint_side_table(buf, pos.x, pos.y, theme);
        }
        DrawableKind::KitchenIsland { pos } => {
            paint_kitchen_island(buf, pos.x, pos.y, theme);
        }
        DrawableKind::SnackShelf { pos } => {
            blit_centered_first_frame(pack, "snack_shelf", *pos, buf);
        }
        DrawableKind::Plant { kind, pos } => {
            // Occlusion is the sprite's own job: the foliage overhangs north of
            // the mask's shallow south-anchored pot strip, so it hides a walker
            // parked behind it. No synthetic back-cap.
            blit_centered_first_frame(pack, kind.sprite_name(), *pos, buf);
        }
        DrawableKind::PodDecorItem { kind, pos } => {
            blit_centered_first_frame(pack, kind.sprite_name(), *pos, buf);
        }
        DrawableKind::FloorLamp { pos } => {
            blit_centered_first_frame(pack, "floor_lamp", *pos, buf);
        }
        DrawableKind::Door { pos, frame_idx } => {
            if let Some(f) = pack.animation("door").and_then(|a| frame_at(a, *frame_idx)) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::WallDecor { kind, pos } => {
            let anim_name = kind.sprite_name();
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames().first()) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::Appliance { pos, sprite, busy } => {
            let art = pack.animation(sprite).and_then(|anim| {
                anim.recolorable(crate::pack::appliance_frame_index(anim, *busy, beat))
            });
            if let Some(art) = art {
                let themed = art.recolored(&crate::pack::appliance_overrides(&theme.appliance));
                blit_centered(&themed, *pos, buf);
            }
        }
        DrawableKind::Pet {
            pos,
            flip,
            anim_name,
            frame_idx,
            effects,
        } => {
            let anim = pack.animation(anim_name)?;
            let frame = frame_at(anim, *frame_idx)?;
            // Declared out here so the flipped path's temporary outlives the `if`.
            let mirrored;
            let final_frame = if *flip {
                mirrored = frame.mirror_horizontal();
                &mirrored
            } else {
                frame
            };
            blit_centered(final_frame, *pos, buf);
            paint_effects(buf, *effects, theme);
        }
        DrawableKind::GatewayMascot {
            mascot_idx,
            pos,
            anim_name,
            frame_idx,
            effects,
            degraded,
        } => {
            let anim = pack.animation(anim_name)?;
            let frame = frame_at(anim, *frame_idx)?;
            if *degraded {
                blit_centered(&super::palette::degraded_frame(frame), *pos, buf);
            } else {
                blit_centered(frame, *pos, buf);
            }
            paint_effects(buf, *effects, theme);
            return Some(Drawn::Mascot {
                mascot_idx: *mascot_idx,
                w: frame.width(),
                h: frame.height(),
            });
        }
        DrawableKind::RoomWall { piece, rows } => {
            crate::wall::paint_wall(
                buf,
                theme,
                *piece,
                rows.clone(),
                crate::cutaway::pen::Pen::UNIT,
            );
        }
        DrawableKind::FishTank { pos } => {
            paint_fish_tank(buf, *pos, beat, theme);
        }
        DrawableKind::MeetingChair { pos, back_west } => {
            paint_meeting_chair(buf, *pos, *back_west, theme);
        }
        DrawableKind::CoatRack { pos } => {
            paint_coat_rack(buf, *pos, theme);
        }
        DrawableKind::WaterCooler(cooler) => paint_water_cooler(buf, *cooler, beat, theme),
        DrawableKind::TrashBin(bin) => paint_trash_bin(buf, *bin),
        DrawableKind::Doormat(mat) => paint_doormat(buf, *mat, theme),
        DrawableKind::NoticeBoard(board) => paint_notice_board(buf, *board, theme),
        DrawableKind::Runner(runner) => paint_corridor_runner(buf, *runner, theme),
        DrawableKind::NeonSign { panel, look } => {
            paint_neon_panel(buf, panel.x, panel.y, panel.width, panel.height, look);
        }
        DrawableKind::Clock { pos } => paint_clock(buf, pos.x, pos.y, now, theme),
    }
    None
}

/// The cup, then the `steam` riding on it.
fn paint_desk_coffee(
    buf: &mut RgbBuffer,
    desk: Point,
    cup: Option<Cup>,
    steam: &[Effect],
    theme: &crate::theme::Theme,
) {
    if cup.is_none() {
        return;
    }
    let put = |buf: &mut RgbBuffer, x: u16, y: u16, c: Rgb| {
        buf.put_checked(x, y, c);
    };
    let Point { x: cx, y: cy } = desk_cup_at(desk);
    put(buf, cx, cy, theme.furniture.coffee_cup);
    put(buf, cx + 1, cy, theme.furniture.coffee_cup);
    put(buf, cx, cy + 1, theme.furniture.coffee_cup_shadow);
    put(buf, cx + 1, cy + 1, theme.furniture.coffee_cup_shadow);
    paint_effects(buf, steam, theme);
}

/// The desk task chair's art — the ONE authority for its size, so the enqueue
/// site centres on what is actually drawn even under a custom pack.
pub(super) fn desk_chair_frame(pack: &Pack) -> Option<&Frame> {
    pack.animation(DESK_CHAIR_SPRITE)
        .and_then(|a| a.frames().first())
}

/// Office-chair back, crossing a back-turned occupant's lower torso.
pub(super) fn paint_chair_back(buf: &mut RgbBuffer, top_left: Point, pack: &Pack) {
    if let Some(frame) = desk_chair_frame(pack) {
        blit_frame(frame, top_left.x, top_left.y, buf);
    }
}

/// The task lamp's warm pool; the desk art draws the lamp itself.
pub(super) fn paint_desk_lamp_pool(
    buf: &mut RgbBuffer,
    lights: &crate::lighting::DeskLights,
    theme: &crate::theme::Theme,
) {
    if lights.lamp.strength <= 0.0 {
        return;
    }
    super::background::paint_light(buf, &lights.lamp, theme.lighting.desk_lamp);
}

/// Token-meter paper tower: `tier` reams stacked on the desk surface against
/// the monitor's east side, growing NORTH past the bezel at
/// [`MAX_TIER`](crate::token_meter::MAX_TIER) so the silhouette reads across the
/// room; that tier's top sheet teeters 1px east.
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

/// Tower geometry, relative to the desk sprite: the stack hugs the
/// monitor's east side on the right wood wing, its base on the surface row.
const STACK_X_OFF: u16 = 11;
const STACK_W: u16 = 3;
const STACK_BASE_DY: u16 = 3;
/// Rows per ream: one row of vertical detail is sub-legible at half-block scale.
const STACK_PX_PER_TIER: u16 = 2;

/// Paint a character at an arbitrary anchor with per-agent recolor, returning
/// the size of the frame it drew.
pub(crate) fn paint_character_at(
    buf: &mut RgbBuffer,
    pose: SpritePose,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
) -> Option<Size> {
    let CharacterFrame {
        frame: cached,
        blit_at: _,
        rise: _,
    } = character_frame(
        pose,
        agent,
        pack,
        crate::render_scale::RenderScale::ONE,
        cache,
        now,
    )?;
    let size = Size {
        w: cached.width(),
        h: cached.height(),
    };
    blit_frame(cached, anchor.x, anchor.y, buf);
    Some(size)
}

/// Queue every room wall's bands into the y-sort, emitted after the fixtures so
/// the glass also covers a chair tied with a band's row.
pub(super) fn enqueue_room_walls<'a>(layout: &'a SceneLayout, drawables: &mut Vec<Drawable<'a>>) {
    for &piece in &layout.wall_pieces {
        for (rows, depth) in piece.sort_bands() {
            drawables.push(Drawable {
                anchor_y: depth,
                // A character tied with a band's row still paints behind the glass.
                layer: Layer::Over,
                kind: DrawableKind::RoomWall { piece, rows },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Motion;
    use crate::layout::DESK_W;
    use crate::pet::PetKind;

    /// The 1x tower and sheet art are the classic's: a frame per tier up to
    /// [`MAX_TIER`](crate::token_meter::MAX_TIER), each [`STACK_PX_PER_TIER`]
    /// rows a tier and [`STACK_W`] wide, the full tower a column wider for its
    /// teeter.
    #[test]
    fn the_1x_token_art_is_the_classics_stack() {
        let pack = crate::pack::test_default_pack();
        let tower = pack.animation("token_tower").expect("the tower");
        let max = crate::token_meter::MAX_TIER;
        assert_eq!(tower.frames().len(), usize::from(max));
        for (tier, f) in (1..=max).zip(tower.frames()) {
            let teeter = u16::from(tier == max);
            assert_eq!(
                (f.width(), f.height()),
                (STACK_W + 1, u16::from(tier) * STACK_PX_PER_TIER),
                "tier {tier}"
            );
            let drawn = |x: u16, y: u16| f.get(x, y).copied().flatten().is_some();
            assert!(
                (0..f.height()).all(|y| {
                    let dx = if y == 0 { teeter } else { 0 };
                    (0..f.width()).all(|x| drawn(x, y) == (dx..dx + STACK_W).contains(&x))
                }),
                "tier {tier} is not {STACK_W} wide with its teeter"
            );
        }
        let sheet = &pack.animation("token_sheet").expect("the sheet").frames()[0];
        assert_eq!((sheet.width(), sheet.height()), (STACK_W, 1));
    }

    /// The 1x desk arts mark the cells the classic stands its props on, each
    /// the prop's foot: the cup's under [`desk_cup_at`], the tower's at
    /// [`STACK_X_OFF`] on [`STACK_BASE_DY`].
    #[test]
    fn the_1x_desk_arts_mark_the_classics_prop_cells() {
        let pack = crate::pack::test_default_pack();
        let cup_h = pack.animation("desk_cup").expect("the cup").frames()[0].height();
        let desk = Point { x: 20, y: 30 };
        for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
            let name = crate::pack::desk_sprite_name(facing);
            let anim = pack.animation(name).expect("the desk");
            let top = desk_art_top(&pack, desk.y, anim.frames()[0].height());
            let mark = |n: &str| {
                let m = anim.marks(0).iter().find(|m| m.name() == n)?;
                Some(Point {
                    x: desk.x + m.x(),
                    y: top + m.y(),
                })
            };
            let cup = desk_cup_at(desk);
            assert_eq!(
                mark("cup"),
                Some(Point {
                    x: cup.x,
                    y: cup.y + cup_h - 1
                }),
                "{name}'s cup"
            );
            assert_eq!(
                mark("tower"),
                Some(Point {
                    x: desk.x + STACK_X_OFF,
                    y: desk.y + STACK_BASE_DY
                }),
                "{name}'s tower"
            );
        }
    }

    #[test]
    fn steam_anchor_sits_within_the_coffee_machine_columns() {
        let pack = crate::pack::test_default_pack();
        let width = |name: &str| pack.animation(name).expect(name).frames()[0].width() as i16;
        // steam_x = pos.x + steam_dx; sprite_x = pos.x - cw/2 → sprite-local
        // steam col = steam_dx + cw/2.
        let large_w = crate::layout::PANTRY_COUNTER_LARGE_W;
        for counter_w in [large_w, large_w - 1] {
            let (lo, hi) = crate::layout::coffee_machine_cols(counter_w);
            let anim = crate::layout::pantry_counter_anim(counter_w);
            let steam_col = pantry_steam_dx(anim) + width(anim) / 2;
            assert!(
                steam_col >= lo as i16 && steam_col < hi as i16,
                "steam col {steam_col} must sit within the machine cols [{lo},{hi})"
            );
        }
    }

    fn test_pack() -> Pack {
        crate::pack::test_default_pack()
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
            layer: Layer::Under,
            kind: DrawableKind::DeskCubicle {
                desk,
                facing: crate::layout::Facing::South,
                screen_glow: None,
                lights: crate::lighting::DeskLights::new(desk, 0.0, 0.0),
                props: DeskProps {
                    cup: None,
                    token_tier,
                    sheet_fall,
                    effects: Vec::new(),
                    scanline: 0,
                },
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
    fn only_a_steaming_cup_steams() {
        let th = theme();
        let bg = Rgb { r: 1, g: 2, b: 3 };
        let desk = Point { x: 20, y: 30 };
        let render = |cup, ms| {
            let mut buf = RgbBuffer::filled(60, 60, bg);
            let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms);
            let steam = crate::sim::cup_effects(desk, cup, Motion::Full.beat(now));
            paint_desk_coffee(&mut buf, desk, cup, &steam, th);
            buf.as_slice().iter().filter(|&&c| c != bg).count()
        };
        let instants = (0..20u64).map(|i| i * 97);
        assert!(
            instants.clone().all(|ms| render(Some(Cup::Cold), ms) == 4),
            "a cold cup paints its four cells and nothing above"
        );
        assert!(
            instants
                .clone()
                .any(|ms| render(Some(Cup::Steaming), ms) > 4),
            "a fresh cup steams"
        );
        assert!(instants.clone().all(|ms| render(None, ms) == 0));
    }

    #[test]
    fn tier_zero_desk_paints_no_paper() {
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let th = theme();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, None);
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
            },
        );
        assert_eq!(paper_pixel_count(&buf, th), 0);
        // …including a mid-fall sheet: a big EARLY reading can clear the sheet
        // minimum before cumulative usage reaches T1.
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(Point { x: 40, y: 30 }, 0, Some(2));
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
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
                &d.kind,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                    theme: th,
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
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
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
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
            },
        );
        let sy = stack_top - (crate::token_meter::SHEET_FALL_PX - 2);
        assert_eq!(buf.get(desk.x + STACK_X_OFF, sy), th.furniture.paper);
        let mut cache = FrameCache::new();
        let mut buf2 = RgbBuffer::filled(120, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = desk_cubicle_drawable(desk, 1, Some(crate::token_meter::SHEET_FALL_PX));
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf2,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
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
    fn a_desk_and_its_cabinet_paint_but_no_per_desk_bin() {
        let pack = test_pack();
        // Pin the ASSET removal, not just its pixels: with no `trash_bin` in
        // the pack, a re-added blit would be dead code.
        assert!(pack.animation("trash_bin").is_none());
        let mut cache = FrameCache::new();
        let now = SystemTime::UNIX_EPOCH;
        let layout = crate::layout::SceneLayout::compute(160, 120, None).expect("fits");
        let first = pixtuoid_core::state::FloorLocalDeskIndex(0);
        let desk = layout.home_desks[first.0];
        let cabinet = layout
            .fixtures()
            .find(|f| f.kind == crate::layout::FixtureKind::FilingCabinet(first))
            .expect("desk 0 stands a cabinet")
            .at;
        let cab = pack
            .animation("filing_cabinet")
            .and_then(|a| a.frames().first())
            .expect("filing_cabinet anim");
        let bg = Rgb { r: 1, g: 2, b: 3 };
        let mut buf = RgbBuffer::filled(layout.buf_w, layout.buf_h, bg);
        for kind in [
            DrawableKind::FilingCabinet { pos: cabinet },
            DrawableKind::DeskCubicle {
                desk,
                facing: crate::layout::Facing::South,
                screen_glow: None,
                lights: crate::lighting::DeskLights::new(desk, 0.0, 0.0),
                props: DeskProps::default(),
            },
        ] {
            paint_drawable(
                &kind,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    clock: Motion::Full.clock(now),
                    theme: theme(),
                },
            );
        }
        let mut cab_painted = false;
        for dy in 0..cab.height() {
            for dx in 0..cab.width() {
                if buf.get(cabinet.x + dx, cabinet.y + dy) != bg {
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
                layer: Layer::Under,
                kind: DrawableKind::MeetingSofa { pos, mirrored },
            };
            paint_drawable(
                &d.kind,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    clock: Motion::Full.clock(now),
                    theme: theme(),
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
            layer: Layer::Figure,
            kind: DrawableKind::Pet {
                pos: Point { x: 30, y: 30 },
                flip: false,
                anim_name: "nonexistent_anim",
                frame_idx: 0,
                effects: &[],
            },
        };
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(now),
                theme: theme(),
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
            let effects =
                crate::sim::pet_effects(PetKind::Cat, pos, anim_name, None, Motion::Full.beat(now));
            let d = Drawable {
                anchor_y: pos.y,
                layer: Layer::Figure,
                kind: DrawableKind::Pet {
                    pos,
                    flip: false,
                    anim_name,
                    frame_idx: 0,
                    effects: &effects,
                },
            };
            paint_drawable(
                &d.kind,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    clock: Motion::Full.clock(now),
                    theme: theme(),
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

    /// Paint the appliance `sprite` at rest, centred at `pos`, in `th`.
    fn appliance_at_rest(sprite: &'static str, pos: Point, th: &crate::theme::Theme) -> RgbBuffer {
        let pack = test_pack();
        let mut cache = FrameCache::new();
        let mut buf = RgbBuffer::filled(80, 80, Rgb { r: 1, g: 2, b: 3 });
        let d = Drawable {
            anchor_y: pos.y,
            layer: Layer::Under,
            kind: DrawableKind::Appliance {
                pos,
                sprite,
                busy: false,
            },
        };
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(SystemTime::UNIX_EPOCH),
                theme: th,
            },
        );
        buf
    }

    /// In every theme, each `appliance_overrides` role
    /// lands on the cells the 1x vending art draws in it.
    #[test]
    fn a_vending_machine_takes_its_themes_colours() {
        let pos = Point { x: 30, y: 30 };
        let vis = crate::layout::furniture_def(crate::layout::Furniture::VendingMachine).visual;
        let (vx, vy) = (pos.x - vis.w / 2, pos.y - vis.h / 2);
        for th in crate::theme::ALL_THEMES {
            let a = &th.appliance;
            let buf = appliance_at_rest("vending_machine", pos, th);
            for ((dx, dy), want, role) in [
                ((0, 1), a.vending_panel, "the panel, under its lit top row"),
                ((1, 2), a.vending_drinks[0], "the first drink"),
                ((4, 2), a.vending_drinks[3], "the fourth drink"),
                ((5, 5), a.vending_trim, "the coin plate"),
                ((1, 10), a.vending_dark, "the pickup tray"),
                ((0, 2), a.vending_body, "the body"),
            ] {
                assert_eq!(buf.get(vx + dx, vy + dy), want, "{}: {role}", th.name);
            }
        }
    }

    /// See [`a_vending_machine_takes_its_themes_colours`].
    #[test]
    fn a_printer_takes_its_themes_colours() {
        let pos = Point { x: 30, y: 30 };
        let vis = crate::layout::furniture_def(crate::layout::Furniture::Printer).visual;
        let (px, py) = (pos.x - vis.w / 2, pos.y - vis.h / 2);
        for th in crate::theme::ALL_THEMES {
            let a = &th.appliance;
            let buf = appliance_at_rest("printer", pos, th);
            for ((dx, dy), want, role) in [
                ((2, 1), a.printer_glass, "the scanner glass"),
                ((4, 0), a.printer_top, "the lid, east of its lit end"),
                ((2, 3), a.printer_paper, "the stack"),
                ((1, 2), a.printer_tray, "the output bay"),
                ((2, 4), a.printer_body, "the chassis"),
            ] {
                assert_eq!(buf.get(px + dx, py + dy), want, "{}: {role}", th.name);
            }
        }
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
        blit_centered(&frame, Point { x: 10, y: 10 }, &mut buf);
        assert_eq!(buf.get(9, 9), marker, "top-left lands at pos − size/2");
        assert_eq!(buf.get(11, 10), marker, "bottom-right at (9+2, 9+1)");
        assert_eq!(buf.get(8, 9), bg, "one column west of the frame stays bg");
        assert_eq!(buf.get(9, 8), bg, "one row north of the frame stays bg");
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
            layer: Layer::Figure,
            kind: DrawableKind::GatewayMascot {
                mascot_idx: 0,
                pos: Point { x: 30, y: 30 },
                anim_name: "nonexistent_anim",
                frame_idx: 0,
                effects: &[],
                degraded: false,
            },
        };
        paint_drawable(
            &d.kind,
            &mut DrawableCtx {
                buf: &mut buf,
                pack: &pack,
                cache: &mut cache,
                clock: Motion::Full.clock(now),
                theme: theme(),
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
                layer: Layer::Figure,
                kind: DrawableKind::GatewayMascot {
                    mascot_idx: 0,
                    pos,
                    anim_name: def.rest,
                    frame_idx: 0,
                    effects: &[],
                    degraded,
                },
            };
            paint_drawable(
                &d.kind,
                &mut DrawableCtx {
                    buf: &mut buf,
                    pack: &pack,
                    cache: &mut cache,
                    clock: Motion::Full.clock(now),
                    theme: theme(),
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
