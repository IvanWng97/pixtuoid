//! The pantry aggregate: bounds + the counter size + the island.

use crate::layout::{
    Bounds, Facing, Furniture, OBSTACLE_PAD_PX, PANTRY_COUNTER_LARGE_W, Pivot, Point, Size,
    WALL_THICK_H, Waypoint, WaypointKind, anchored_top_left, furniture_def, pct,
};

/// The compact counter — the fallback for a pantry too narrow for
/// `LARGE_COUNTER`, and the size consumers read when no pantry exists. The
/// `Furniture::Pantry` row is runtime-sized, so this pair is the counter's only
/// size authority; each is its sprite's size, the ground being
/// `pantry_ground_rect`'s shallow strip.
pub(crate) const COMPACT_COUNTER: Size = Size { w: 20, h: 8 };

/// The detailed kitchen-run counter, for a pantry wide enough to host it.
pub(crate) const LARGE_COUNTER: Size = Size {
    w: PANTRY_COUNTER_LARGE_W,
    h: 10,
};

/// The pantry counter sprites, compact then large.
pub(crate) const PANTRY_COUNTER_ANIMS: [&str; 2] = ["pantry_small", "pantry"];

/// The pantry counter sprite for a counter `counter_w` px wide: the large
/// kitchen run when the room fits it, else the compact one.
pub(crate) fn pantry_counter_anim(counter_w: u16) -> &'static str {
    let [compact, large] = PANTRY_COUNTER_ANIMS;
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        large
    } else {
        compact
    }
}

/// The water cooler's size, [`PantryRoom::water_cooler_rect`]'s box.
pub(crate) const WATER_COOLER: Size = Size { w: 4, h: 9 };

/// The pantry room: its bounds plus what it owns — the counter's chosen
/// footprint and the kitchen-island body centre (`None` when the room can't host
/// it clear of walls + the counter — refuse-don't-force).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PantryRoom {
    /// The pantry room's interior rectangle (buffer pixels).
    pub bounds: Bounds,
    /// Size of the pantry counter sprite: `LARGE_COUNTER` when the pantry is
    /// wide enough, else `COMPACT_COUNTER`. The renderer reads this to
    /// pick which sprite to paint (`pantry` vs `pantry_small`).
    pub counter_size: Size,
    /// Kitchen-island body centre.
    pub kitchen_island: Option<Point>,
}

/// Vertical position of the pantry counter as a percent of the room height.
/// SINGLE SOURCE: the island clamp, the counter's own waypoint placement, and
/// [`PantryRoom::content_fit_h`]'s inverse all read it — were they to drift, a
/// clamp would guard a phantom counter position.
pub(crate) fn pantry_counter_y_pct(counter_w: u16) -> u16 {
    if counter_w >= PANTRY_COUNTER_LARGE_W {
        65
    } else {
        60
    }
}

impl PantryRoom {
    /// Absolute y of the counter's blocked centre line inside a room of `bounds`.
    /// THE one derivation the island clamp, the snack-shelf clamp and the
    /// counter's own waypoint all read, so a percent change can't move one and
    /// strand the others.
    pub(crate) fn counter_center_y(bounds: Bounds, counter: Size) -> u16 {
        bounds.y + pct(bounds.height, pantry_counter_y_pct(counter.w))
    }

    /// Northmost blocked row of the padded counter — the ceiling the island body
    /// and the snack shelf must sit clear of.
    pub(crate) fn counter_north(bounds: Bounds, counter: Size) -> u16 {
        Self::counter_center_y(bounds, counter).saturating_sub(counter.h / 2 + OBSTACLE_PAD_PX)
    }

    /// The counter's centre in a room of `bounds`, or `None` for a room
    /// narrower than the counter: refuse rather than force.
    pub(crate) fn counter_center(bounds: Bounds, counter: Size) -> Option<Point> {
        let half_cw = counter.w / 2;
        let max_cx = bounds.x + bounds.width.saturating_sub(half_cw + 1);
        let min_cx = bounds.x + half_cw;
        (min_cx <= max_cx).then(|| Point {
            x: if counter.w >= PANTRY_COUNTER_LARGE_W {
                (bounds.x + bounds.width / 2).clamp(min_cx, max_cx)
            } else {
                (bounds.x + pct(bounds.width, 60)).clamp(min_cx, max_cx)
            },
            // Single-sourced with the island clamp; only x is size-shaped.
            y: Self::counter_center_y(bounds, counter),
        })
    }

    /// `r`, or `r` slid out past the counter's nearer end when the counter's
    /// art would cover it — `None` when that leaves the room.
    fn off_the_counter(&self, r: Bounds) -> Option<Bounds> {
        let Some(c) = Self::counter_center(self.bounds, self.counter_size) else {
            return Some(r);
        };
        let (cw, ch) = (self.counter_size.w, self.counter_size.h);
        let at = anchored_top_left(Pivot::Center, c, cw, ch);
        let counter = Bounds {
            x: at.x,
            y: at.y,
            width: cw,
            height: ch,
        };
        if !r.overlaps(counter) {
            return Some(r);
        }
        let x = if r.x + r.width / 2 < c.x {
            counter.x.checked_sub(r.width)?
        } else {
            counter.x + cw
        };
        let b = self.bounds;
        (x >= b.x && x + r.width <= b.x + b.width).then_some(Bounds { x, ..r })
    }

    /// The room height at which the pantry can actually HOST its content — the
    /// inverse of the island's y-clamps, where `div_ceil` is the exact inverse of
    /// the truncating `pct()`. An associated fn, not a method: the split
    /// negotiation needs the answer BEFORE any bounds exist.
    pub(crate) fn content_fit_h(counter: Size) -> u16 {
        let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
        let island_half_h = furniture_def(Furniture::KitchenIsland).visual.h / 2;
        let island_need =
            clr + 2 * (island_half_h + OBSTACLE_PAD_PX) + 1 + counter.h / 2 + OBSTACLE_PAD_PX;
        (u32::from(island_need) * 100).div_ceil(u32::from(pantry_counter_y_pct(counter.w))) as u16
    }

    /// The water cooler's sprite box against the pantry's east side, clear of
    /// the counter, or `None` when the room can't fit it. THE one authority
    /// `paint_water_cooler` AND the binary's hover hit-test both read, so the
    /// drawn sprite and its hover box can't drift across the crate boundary.
    pub fn water_cooler_rect(&self) -> Option<Bounds> {
        /// Columns between the cooler's east edge and the room's.
        const EAST_GAP: u16 = 3;
        /// Rows from the room's top to just past the cooler's base.
        const BASE_DY: u16 = 15;
        let b = self.bounds;
        // Gated first: the `b.width - …` would `u16`-underflow in a sub-gate room.
        if b.height <= 25 || b.width <= 12 {
            return None;
        }
        self.off_the_counter(Bounds {
            x: b.x + b.width - EAST_GAP - WATER_COOLER.w,
            y: b.y + BASE_DY - WATER_COOLER.h,
            width: WATER_COOLER.w,
            height: WATER_COOLER.h,
        })
    }

    /// The trash bin's sprite box near the pantry's west counter, clear of it,
    /// or `None` when the room can't fit it. Shared placement authority for
    /// `paint_trash_bin` and the hover hit-test — see [`Self::water_cooler_rect`].
    pub fn trash_bin_rect(&self) -> Option<Bounds> {
        let b = self.bounds;
        // Gated first: `b.height - 14` must not run below the gate.
        if b.height <= 20 {
            return None;
        }
        self.off_the_counter(Bounds {
            x: b.x + 3,
            y: b.y + b.height - 14,
            width: 4,
            height: 5,
        })
    }
}

/// Place the kitchen island in room `pr`: refuse-don't-force with BOTH-axis
/// clamps, staying clear of the counter's padded north
/// ([`PantryRoom::counter_north`], the anti-merge routing constraint). Returns
/// the island body centre, or `None` when the room can't host it clear of walls +
/// counter. On success it ALSO pushes the four `WaypointKind::Island` bartender
/// stand slots (E/W behind the body, two S at the ±w/4 quarter points) onto
/// `waypoints`.
pub(crate) fn place_kitchen_island(
    pr: Bounds,
    counter: Size,
    waypoints: &mut Vec<Waypoint>,
) -> Option<Point> {
    let vis = furniture_def(Furniture::KitchenIsland).visual;
    let (half_w, half_h) = (vis.w / 2, vis.h / 2);
    let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
    // Stands flank the island 1 walkable cell beyond the body's padded footprint,
    // and must stay in-room too — so the x clamps price the stand extent, not the
    // body.
    let stand_dx = half_w + OBSTACLE_PAD_PX + 1;
    let counter_north = PantryRoom::counter_north(pr, counter);
    let min_x = pr.x + clr + stand_dx;
    let max_x = (pr.x + pr.width).saturating_sub(clr + stand_dx);
    // The bartenders' approach lane — the walkable row above the body's padded
    // strip — must be in-room too.
    let min_y = pr.y + clr + half_h + OBSTACLE_PAD_PX;
    let max_y = counter_north.saturating_sub(half_h + OBSTACLE_PAD_PX + 1);
    if min_x > max_x || min_y > max_y {
        return None;
    }
    let ix = (pr.x + pr.width / 2).clamp(min_x, max_x);
    let iy = (pr.y + pct(pr.height, 40)).clamp(min_y, max_y);
    // Bartender slots sit ON the island's center row at its quarter points, where
    // the sprites can't overlap each other. A BLOCKED pos is fine for an
    // `occupies_pos` slot (the couch-seat pattern): approach_point finds the lane
    // BEHIND the island, the settle glide bridges in, and the island's south-row
    // sort row occludes the standers' legs.
    let bar_dx = (vis.w / 4) as i16;
    for (dx, facing) in [
        (-(stand_dx as i16), Facing::East),
        (stand_dx as i16, Facing::West),
        (-bar_dx, Facing::South),
        (bar_dx, Facing::South),
    ] {
        waypoints.push(Waypoint {
            pos: Point {
                x: ix.saturating_add_signed(dx),
                y: iy,
            },
            kind: WaypointKind::Island,
            facing,
            room_id: None,
        });
    }
    Some(Point { x: ix, y: iy })
}

/// Place the snack shelf in room `pr`, pushing its single `WaypointKind::SnackShelf`
/// slot. It hugs the WEST wall — the pantry's only wall-free side is the EAST
/// bridge, which must stay open — and refuses rooms too narrow for a shelf plus an
/// east-side stander cell, with the same both-axis clamp / counter-north clearance
/// as [`place_kitchen_island`].
pub(crate) fn place_snack_shelf(pr: Bounds, counter: Size, waypoints: &mut Vec<Waypoint>) {
    let vis = furniture_def(Furniture::SnackShelf).visual;
    let (half_w, half_h) = (vis.w / 2, vis.h / 2);
    let clr = WALL_THICK_H + OBSTACLE_PAD_PX;
    let counter_north = PantryRoom::counter_north(pr, counter);
    let sx = pr.x + 1 + half_w;
    // Width gate: 1px west margin + the shelf + 3px so the east-side stander has
    // an in-room walkable cell. Narrower rooms refuse.
    let width_fits = pr.width >= vis.w + 4;
    let min_y = pr.y + clr + half_h;
    let max_y = counter_north.saturating_sub(half_h + 1);
    let target = pr.y + pct(pr.height, 30);
    let candidate = (width_fits && min_y <= max_y).then(|| target.clamp(min_y, max_y));
    if let Some(sy) = candidate {
        waypoints.push(Waypoint {
            pos: Point { x: sx, y: sy },
            kind: WaypointKind::SnackShelf,
            facing: Facing::West,
            room_id: None,
        });
    }
}
