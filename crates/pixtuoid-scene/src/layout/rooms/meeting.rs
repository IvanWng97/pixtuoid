//! The meeting room aggregate: bounds + the sofa/table trio.

use crate::layout::{Bounds, Point};

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
