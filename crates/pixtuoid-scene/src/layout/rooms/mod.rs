//! Room aggregates — ONE seam per enclosed room: its bounds PLUS what it owns
//! (the meeting trio, the pantry's counter/island).
//!
//! Deliberately NOT rooms: the free-standing whiteboard (corridor-level,
//! buffer-anchored) and interior plants — a padded plant inside a room's
//! walkable strips disconnects the door gap.

pub(crate) mod meeting;
pub(crate) mod pantry;
pub(crate) mod walls;

pub use meeting::{MeetingRoom, MeetingTrio};
pub use pantry::PantryRoom;
