//! The meeting room aggregate: bounds + the sofa/table trio.

use crate::layout::{furniture_def, Bounds, Furniture, Point};

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

impl MeetingRoom {
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
}
