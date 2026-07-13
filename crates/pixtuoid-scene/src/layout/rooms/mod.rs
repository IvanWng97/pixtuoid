//! Room aggregates — ONE seam per enclosed room (#557, absorbs #555).
//!
//! A room = its bounds PLUS what it owns: the meeting trio, the pantry's
//! counter/island. Before this module those lived as parallel flat
//! `SceneLayout` fields sharing one identity (`meeting_room` +
//! `meeting_room_2` + `meeting_furniture[i]`, all keyed by `room_id`), so a
//! single taste pass touched four placement sites and two cross-block
//! contracts held the geometry together from a distance (the split reading
//! the pantry's content height; the bookshelf drain clamp re-deriving sofa
//! geometry). Each room's placement now lives with the room.
//!
//! Deliberately NOT rooms: the free-standing whiteboard (corridor-level,
//! buffer-anchored) and interior plants — a padded plant inside a room's
//! walkable strips disconnects the door gap (documented sharp edge).

pub(crate) mod meeting;
pub(crate) mod pantry;

pub use meeting::{MeetingRoom, MeetingTrio};
pub use pantry::PantryRoom;
