//! Room aggregates — ONE seam per enclosed room (#557, absorbs #555).
//!
//! A room = its bounds PLUS what it owns: the meeting trio, the pantry's
//! counter/island. Before this module those lived as parallel flat
//! `SceneLayout` fields sharing one identity (`meeting_room` +
//! `meeting_room_2` + `meeting_furniture[i]`, all keyed by `room_id`), so a
//! single taste pass touched four placement sites and two cross-block
//! contracts held the geometry together from a distance — e.g. the split
//! reading the pantry's content height (now `PantryRoom::content_fit_h`).
//! The wall-band decor (screen/bookshelf) and its sofa-derived drain clamp
//! still live in compute.rs's wall-decor block: absorbing band decor into
//! the units is the polish arc's next step, not this refactor.
//!
//! Deliberately NOT rooms: the free-standing whiteboard (corridor-level,
//! buffer-anchored) and interior plants — a padded plant inside a room's
//! walkable strips disconnects the door gap (documented sharp edge).

pub(crate) mod meeting;
pub(crate) mod pantry;
pub(crate) mod walls;

pub use meeting::{MeetingRoom, MeetingTrio};
pub use pantry::PantryRoom;
