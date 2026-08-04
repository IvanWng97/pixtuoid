//! The meeting room aggregate: bounds + the sofa/table trio.

use crate::layout::{furniture_def, pct, Bounds, Furniture, Point, OBSTACLE_PAD_PX};

/// One meeting room's furniture trio. The fixed-size array encodes the
/// invariant that a fitted room produces exactly 2 sofas + 1 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeetingTrio {
    /// The two sofa centres: `[0]` north, `[1]` south (pixel-space).
    pub sofas: [Point; 2],
    /// The table centre, midway between the two sofas (pixel-space).
    pub table: Point,
}

/// A meeting room: its bounds plus the trio it hosts. Its index in
/// `SceneLayout::meeting_rooms` IS the `room_id` every waypoint and painter
/// joins on, so the Vec must never be compacted or reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeetingRoom {
    /// The room's interior rectangle (buffer pixels).
    pub bounds: Bounds,
    /// The sofa/table trio, or `None` when the room is too small to fit it
    /// (bare floor — the room still exists to hold its `room_id`).
    pub trio: Option<MeetingTrio>,
}

/// Horizontal offset of each head-of-table chair from the table centre —
/// mirrored ±: west chair at `table.x − DX` (faces East), east at `+DX`.
pub(crate) const MEETING_CHAIR_TABLE_DX: u16 = 9;

impl MeetingRoom {
    /// The coat rack's spot beside the corridor door (east wall, room-centre
    /// row) — or `None` when a fitted room is too narrow for the rack's coats
    /// (west reach `x − 2`) to clear the east chair and its sitter.
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
            // The seated sprite shares the chair body's east edge, so the
            // body reach IS the sitter's reach.
            let chair_east_reach = t.table.x
                + MEETING_CHAIR_TABLE_DX
                + furniture_def(Furniture::MeetingChair).visual.w / 2;
            let rack_west_reach = pos.x.saturating_sub(2);
            // Drop only on true overlap: a rack and chair that sit exactly
            // adjacent keep it.
            if rack_west_reach <= chair_east_reach {
                return None;
            }
        }
        Some(pos)
    }

    /// Minimum room height that fits the sofa/table trio — the fit gate AND the
    /// floor of `compute_with_seed`'s meeting/pantry split negotiation. Prices
    /// the TABLE between the sofas, not just the two sofa bodies: with both sofa
    /// clamps bound the centred table's ground would otherwise clip both.
    /// Derived from the furniture rows the mask stamps, so a sprite that grows
    /// can't leave a too-short room passing.
    pub(crate) fn trio_fit_h() -> u16 {
        let sofa = furniture_def(Furniture::MeetingSofaBody);
        let table_fp_h = furniture_def(Furniture::MeetingTable)
            .footprint
            .map_or(0, |s| s.h);
        sofa.visual.h * 2 + sofa.footprint.map_or(0, |s| s.h) + table_fp_h
    }

    /// Place the sofa/table trio inside `bounds` (the caller gates on
    /// `room_fits_furniture` first).
    ///
    /// Sofas sit SYMMETRICALLY about the room mid-line (20%/80%) so each gets
    /// equal front clearance to the centred table, which follows to the sofa
    /// midpoint. `dense` picks the north-sofa floor: a NON-dense room (room 0)
    /// sits above the wall band's walkable carpet apron, so its sofa may tuck to
    /// `sofa_h/2`; the DENSE room (room 1) sits under the glass divider (which
    /// stamps `WALL_THICK_H` rows into its top), so its sofa needs a full
    /// `sofa_h` for its ground to clear the wall.
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

    /// The entrance doormat's sprite box (bordered rug on the cubicle side, one
    /// clear column east of the room's east wall) — `None` on a room too narrow.
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

    /// The east edge past which the wall-band bookshelf must drain to clear the
    /// sofa's padded ground — read from the REAL placed sofa, NOT reconstructed
    /// from `bounds`, so a sofa resize can't desync it from `mask`'s
    /// Center-anchored sofa stamp. `None` for a bare room.
    pub(crate) fn sofa_east_drain_edge(&self) -> Option<u16> {
        self.trio.map(|t| {
            let sofa_fp_w = furniture_def(Furniture::MeetingSofaBody)
                .footprint
                .map_or(0, |s| s.w);
            t.sofas[0].x + sofa_fp_w / 2 + OBSTACLE_PAD_PX
        })
    }
}
