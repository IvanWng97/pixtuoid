//! The meeting room aggregate: bounds + the sofa/table trio.

use crate::layout::{furniture_def, pct, Bounds, Furniture, Point};

/// One meeting room's furniture trio, grouped so the per-room structure is
/// explicit instead of reconstructed by index arithmetic over two flat Vecs.
/// `sofas[0]` is the north sofa, `sofas[1]` the south (the order the old flat
/// `meeting_sofas` Vec was extended in); `table` is centered between them. A
/// fitted room always produces exactly 2 sofas + 1 table (see
/// `compute::room_furniture`), so the fixed-size array encodes that invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeetingTrio {
    pub sofas: [Point; 2],
    pub table: Point,
}

/// A meeting room: its bounds plus the trio it hosts. `trio` is `None` when
/// the room is too small for the sofa/table set (`room_fits_furniture` — the
/// bare-floor degradation), but the ROOM still exists: its index in
/// `SceneLayout::meeting_rooms` IS the `room_id` every waypoint and painter
/// joins on. The old shape compacted fitted trios into a separate Vec while
/// bounds lived in two scalar fields — a bare room 0 above a fitted room 1
/// would have mis-joined `meeting_furniture[0]` to room 0's bounds (latent
/// only because `MIN_DUAL_MEETING_H` keeps both dense rooms ≥ the trio fit);
/// keeping bounds and trio in ONE element makes that class unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeetingRoom {
    pub bounds: Bounds,
    pub trio: Option<MeetingTrio>,
}

/// Horizontal offset of each head-of-table chair from the table centre —
/// mirrored ±: west chair at `table.x − DX` (faces East), east at `+DX`.
/// Single-sourced here so the waypoint push (compute.rs) and the coat-rack
/// clearance math below can't drift apart.
pub(crate) const MEETING_CHAIR_TABLE_DX: u16 = 9;

impl MeetingRoom {
    /// The coat rack's spot beside the corridor door (east wall, room-centre
    /// row) — or `None` when a fitted room is too narrow for the rack's coats
    /// (west reach `x − 2`) to clear the east chair and its sitter (arc-final
    /// audit catch: at ≲40-wide rooms they interpenetrated). Bare rooms keep
    /// the rack at any width past the sprite gate. THE one authority: the
    /// painter's enqueue and the binary's hover hit-test both read this.
    pub fn coat_rack_pos(&self) -> Option<Point> {
        let b = self.bounds;
        if b.width <= 20 {
            return None;
        }
        let pos = Point {
            x: b.x + b.width - 5,
            y: b.y + b.height / 2 - 4,
        };
        if let Some(t) = &self.trio {
            // The 8-wide seated sprite centered on the chair cell shares the
            // chair body's east edge (pos+3), so the body reach IS the reach.
            let chair_east_reach = t.table.x
                + MEETING_CHAIR_TABLE_DX
                + furniture_def(Furniture::MeetingChair).visual.w / 2;
            let rack_west_reach = pos.x.saturating_sub(2);
            // Drop only on true overlap (coats start at or west of the chair
            // edge) — dense 42-wide rooms sit exactly adjacent and keep it.
            if rack_west_reach <= chair_east_reach {
                return None;
            }
        }
        Some(pos)
    }

    /// Minimum room height that fits the sofa/table trio — the fit gate AND
    /// the floor of the split negotiation (`compute_with_seed` donates
    /// meeting rows to the pantry only while the trio still fits). Height
    /// must price the TABLE between the sofas, not just the two sofa
    /// bodies: with both sofa clamps bound (short room) the mirror positions
    /// leave `height − 2·sofa_h` between the sofa centres, and the centred
    /// table needs its own footprint depth plus the sofa's not to overlap
    /// either body (placement-sweep catch: at 96×60 the table ground clipped
    /// BOTH sofas by a row). Derived from the same furniture rows the mask
    /// stamps — a bare literal would silently let 1px-too-short rooms pass
    /// if a sprite ever grows (MeetingSofa seat teleport on the coarse grid).
    pub(crate) fn trio_fit_h() -> u16 {
        let sofa = furniture_def(Furniture::MeetingSofaBody);
        let table_fp_h = furniture_def(Furniture::MeetingTable)
            .footprint
            .map_or(0, |s| s.h);
        sofa.visual.h * 2 + sofa.footprint.map_or(0, |s| s.h) + table_fp_h
    }

    /// Place the sofa/table trio inside `bounds` (the caller gates on
    /// `room_fits_furniture` first). Lives HERE — next to `bounds`,
    /// `trio_fit_h`, and `coat_rack_pos` — so ALL meeting-room geometry has one
    /// home, symmetric with the pantry's `place_kitchen_island`/
    /// `place_snack_shelf` in `rooms/pantry.rs` (was an inline closure in
    /// `compute_with_seed`).
    ///
    /// Sofas sit SYMMETRICALLY about the room mid-line (20%/80%) so each gets
    /// equal front clearance to the centred table (the old 30% packed the north
    /// sofa's front against the table). The table follows to the sofa midpoint,
    /// keeping both fronts equally routable. `dense` picks the north-sofa floor:
    /// a NON-dense room (room 0) sits above the wall band's walkable carpet
    /// apron, so its sofa may tuck to `sofa_h/2`; the DENSE room (room 1) sits
    /// under the glass divider (which stamps `WALL_THICK_H` rows into its top),
    /// so its sofa needs a full `sofa_h` for its ground to clear the wall. The
    /// `sofa_h/2` floor binds only if the sprite grows (the trio fit gate keeps
    /// pct-20 ≥ 4 > sofa_h/2 today, so pct-20 governs); the 1-row apron strip
    /// above the padded body drains laterally through the screen-west/
    /// bookshelf-east channel the wall-decor placement guarantees — do NOT
    /// weaken that channel or the strip strands (the 150×68 sealed-pocket
    /// class). The south clamp keeps a full `sofa_h` off the bottom wall on both.
    pub(crate) fn place_trio(bounds: Bounds, dense: bool) -> MeetingTrio {
        let sofa_h = furniture_def(Furniture::MeetingSofaBody).visual.h;
        let north_floor = if dense { sofa_h } else { sofa_h / 2 };
        let cx = bounds.x + bounds.width / 2;
        let north_y = (bounds.y + pct(bounds.height, 20)).max(bounds.y + north_floor);
        let south_y = (bounds.y + pct(bounds.height, 80))
            .min(bounds.y + bounds.height.saturating_sub(sofa_h));
        MeetingTrio {
            sofas: [Point { x: cx, y: north_y }, Point { x: cx, y: south_y }],
            table: Point {
                x: cx,
                y: (north_y + south_y) / 2,
            },
        }
    }

    /// The entrance doormat's 4×5 sprite box (bordered rug on the cubicle side,
    /// one clear column east of the room's east wall) — present only when the
    /// room is wide enough (`width > 10`), `None` otherwise. THE one authority
    /// `paint_doormat` AND the binary's hover hit-test both read — the
    /// `coat_rack_pos` pattern for the room's other procedural decor, so the mat
    /// and its hover box can't drift.
    pub fn doormat_rect(&self) -> Option<Bounds> {
        let b = self.bounds;
        // Lazy `.then`: `b.height / 2 - 2` must not run for a sub-gate room.
        (b.width > 10).then(|| Bounds {
            x: b.x + b.width + 1,
            y: b.y + b.height / 2 - 2,
            width: 4,
            height: 5,
        })
    }
}
