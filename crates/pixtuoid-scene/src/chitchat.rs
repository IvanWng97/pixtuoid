//! Office chitchat — short speech-bubble conversations between agents who share
//! a social venue: either a single social waypoint or a whole meeting room, so a
//! room hosts one GROUP conversation rather than a pile of independent pairs.
//! Conversations are N-way, the speaker rotating round-robin each turn.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;

use crate::layout::{Point, WaypointKind};

/// Total duration of a single chitchat exchange — the speaking turns fill it
/// exactly, with no trailing silent gap.
pub const CHITCHAT_TOTAL_MS: u64 = TURNS * TURN_MS;

const TURN_MS: u64 = 1_500;

const TURNS: u64 = 4;

/// Pool of short speech-bubble quips. Order doesn't matter (`current_bubble`
/// indexes `% CHITCHAT_LINES.len()`), but keep each line short (≤ ~12 chars) so
/// it fits the bubble at half-block scale.
pub const CHITCHAT_LINES: &[&str] = &[
    "git push -f",
    "// TODO",
    "LGTM!",
    "works on my",
    "ship it!",
    "npm install",
    "sudo !!",
    "404",
    "seg fault",
    "it compiled!",
    "rebase time",
    "merge pls",
    "async await",
    "rm -rf node_",
    "NaN === NaN",
    "overflow",
    "undefined?",
    "coffee++",
    "looks good",
    "trust me",
    "no tests?",
    "WONTFIX",
    "type: any",
    "blame git",
    "it's DNS",
    "flaky test",
    "force push",
    "cherry-pick",
    "off by one",
    "heisenbug",
    "rubber duck",
    "stash pop",
    "bisect bad",
    "hotfix!",
    "revert?",
    "memory leak",
    "cache miss",
    "deadlock",
    "panic!()",
    "unwrap()",
    "borrow chk",
    "CI is red",
    "rollback!",
    "vibe coding",
    "needs rebase",
    "more coffee?",
    "standup?",
    "lunch?",
    "ship friday",
];

/// A social venue that hosts at most one conversation at a time. Meeting-room
/// slots all map to the same `Room`; every other social waypoint is its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VenueKey {
    /// A whole meeting room's group conversation (all its slots share one venue).
    Room {
        /// Index of the floor the room is on.
        floor_idx: usize,
        /// The room's id (index into `layout.meeting_rooms`).
        room_id: usize,
    },
    /// A single social waypoint's conversation (pantry, couch, vending, printer).
    Waypoint {
        /// Index of the floor the waypoint is on.
        floor_idx: usize,
        /// Index of the waypoint in `layout.waypoints`.
        wp_idx: usize,
    },
}

/// A live conversation among the agents currently at a venue.
pub struct ActiveChitchat {
    /// The venue this conversation belongs to.
    pub venue: VenueKey,
    /// Current attendees, sorted ascending by raw id for a stable rotation.
    pub participants: Vec<AgentId>,
    /// When the conversation began — the turn/expiry clock.
    pub started_at: SystemTime,
    seed: u64,
}

impl ActiveChitchat {
    /// Starts a conversation at `venue` among `participants`.
    pub fn new(venue: VenueKey, participants: Vec<AgentId>, now: SystemTime) -> Self {
        // NOT the render-layer `pixel_painter::epoch_ms` forwarder — chitchat is a
        // model module and must not depend on the render layer.
        let ms = crate::anim::elapsed_ms(now, SystemTime::UNIX_EPOCH);
        let mut chat = Self {
            venue,
            participants: Vec::new(),
            started_at: now,
            seed: 0,
        };
        chat.set_participants(participants);
        // Seed from the SORTED set so the line choice is independent of the
        // HashMap iteration order `present` was built in — restarting the same
        // group must never flip the line.
        chat.seed = chat
            .participants
            .iter()
            .fold(ms.wrapping_mul(0x9e37_79b9_7f4a_7c15), |acc, a| {
                acc.rotate_left(7) ^ a.raw()
            });
        chat
    }

    /// Replace the attendee set (sorted + de-duplicated), tracking who is
    /// actually present this frame.
    pub fn set_participants(&mut self, mut participants: Vec<AgentId>) {
        participants.sort_by_key(|a| a.raw());
        participants.dedup();
        self.participants = participants;
    }

    /// Whether the exchange has run its full `CHITCHAT_TOTAL_MS`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.elapsed_ms(now) >= CHITCHAT_TOTAL_MS
    }

    fn elapsed_ms(&self, now: SystemTime) -> u64 {
        now.duration_since(self.started_at)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(CHITCHAT_TOTAL_MS)
    }

    /// The agent speaking this turn and their line, or `None` once expired / if
    /// nobody is present. The speaker rotates round-robin through `participants`.
    pub fn current_bubble(&self, now: SystemTime) -> Option<(AgentId, &'static str)> {
        let elapsed = self.elapsed_ms(now);
        if elapsed >= CHITCHAT_TOTAL_MS {
            return None;
        }
        let turn = elapsed / TURN_MS;
        if turn >= TURNS || self.participants.is_empty() {
            return None;
        }
        let speaker = self.participants[(turn as usize) % self.participants.len()];
        let line_idx = (self.seed.wrapping_add(turn) as usize) % CHITCHAT_LINES.len();
        Some((speaker, CHITCHAT_LINES[line_idx]))
    }
}

/// How a waypoint kind participates in chitchat — the ONE declared social fact
/// both [`supports_chitchat`] and [`venue_wp_idx`] read. Matched EXHAUSTIVELY
/// (no `_`), so a new [`WaypointKind`] must classify itself or the build fails
/// instead of silently defaulting to non-social + un-collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChitchatVenue {
    /// Not a social spot (phone booth, standing desk).
    None,
    /// Eligible; chats key to the agent's own waypoint slot.
    Solo,
    /// Eligible; every slot of this kind collapses to ONE venue, keyed to the
    /// first-of-kind waypoint. ORTHOGONAL to meeting-room group chat, which is
    /// keyed by `room_id`.
    Group,
}

/// The single social classification `supports_chitchat` + `venue_wp_idx` derive
/// from.
fn chitchat_venue(kind: WaypointKind) -> ChitchatVenue {
    match kind {
        WaypointKind::Couch | WaypointKind::Island => ChitchatVenue::Group,
        WaypointKind::Pantry
        | WaypointKind::VendingMachine
        | WaypointKind::Printer
        | WaypointKind::MeetingSofa
        | WaypointKind::MeetingChair
        | WaypointKind::SnackShelf => ChitchatVenue::Solo,
        WaypointKind::PhoneBooth | WaypointKind::StandingDesk => ChitchatVenue::None,
    }
}

/// The chitchat `wp_idx` a waypoint visitor groups under. Multi-slot venues
/// collapse to ONE venue — the first waypoint OF THAT KIND — so each hosts a
/// single group conversation like the meeting room, WITHOUT overloading the
/// meeting-only `room_id` field. Finds the group anchor ITSELF from the waypoint
/// slice: a caller-supplied index invites the bug that merged island standers
/// into the couch's conversation.
pub fn venue_wp_idx(
    kind: WaypointKind,
    wp_idx: usize,
    waypoints: &[crate::layout::Waypoint],
) -> usize {
    match chitchat_venue(kind) {
        ChitchatVenue::Group => waypoints
            .iter()
            .position(|w| w.kind == kind)
            .unwrap_or(wp_idx),
        ChitchatVenue::Solo | ChitchatVenue::None => wp_idx,
    }
}

/// Whether agents at this waypoint kind can start a chitchat.
pub fn supports_chitchat(kind: WaypointKind) -> bool {
    chitchat_venue(kind) != ChitchatVenue::None
}

/// A single speech bubble ready for the widget layer to render.
pub struct ChitchatBubble {
    /// The quip to render.
    pub text: &'static str,
    /// Pixel coords of the speaking agent's anchor.
    pub anchor: Point,
}

/// A chitchat-eligible agent present at a venue this frame. Named (not a tuple)
/// so the producer and consumer can't transpose the two `usize`-ish fields.
#[derive(Debug, Clone, Copy)]
pub struct Visitor {
    /// The visitor's waypoint slot index.
    pub wp_idx: usize,
    /// The visiting agent.
    pub agent_id: AgentId,
    /// The agent's pixel anchor (bubble placement).
    pub anchor: Point,
    /// `Some(room_id)` for meeting slots, `None` for single-point waypoints.
    pub room_id: Option<usize>,
}

/// Expire old conversations, start/refresh one per venue that has ≥2 agents,
/// and return the active speech bubbles for this frame.
pub fn update_and_collect(
    state: &mut HashMap<VenueKey, ActiveChitchat>,
    floor_idx: usize,
    visitors: &[Visitor],
    now: SystemTime,
) -> Vec<ChitchatBubble> {
    state.retain(|_, chat| !chat.is_expired(now));

    let mut by_venue: HashMap<VenueKey, Vec<(AgentId, Point)>> = HashMap::new();
    for v in visitors {
        let venue = match v.room_id {
            Some(room_id) => VenueKey::Room { floor_idx, room_id },
            None => VenueKey::Waypoint {
                floor_idx,
                wp_idx: v.wp_idx,
            },
        };
        by_venue
            .entry(venue)
            .or_default()
            .push((v.agent_id, v.anchor));
    }

    // A TOTAL emission order, never `by_venue`'s: the Vec is painted in order, so
    // hash order would make two overlapping bubbles z-fight between runs.
    let mut venues: Vec<(&VenueKey, &Vec<(AgentId, Point)>)> = by_venue.iter().collect();
    venues.sort_by_key(|(v, _)| match v {
        VenueKey::Room { floor_idx, room_id } => (*floor_idx, 0usize, *room_id),
        VenueKey::Waypoint { floor_idx, wp_idx } => (*floor_idx, 1usize, *wp_idx),
    });

    let mut bubbles = Vec::new();
    for (venue, agents) in venues {
        if agents.len() < 2 {
            continue;
        }
        let present: Vec<AgentId> = agents.iter().map(|(id, _)| *id).collect();

        let chat = state
            .entry(*venue)
            .or_insert_with(|| ActiveChitchat::new(*venue, present.clone(), now));
        chat.set_participants(present);

        if let Some((speaker_id, text)) = chat.current_bubble(now) {
            if let Some((_, anchor)) = agents.iter().find(|(id, _)| *id == speaker_id) {
                bubbles.push(ChitchatBubble {
                    text,
                    anchor: *anchor,
                });
            }
        }
    }

    bubbles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn base_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn aid(s: &str) -> AgentId {
        AgentId::from_transcript_path(s)
    }

    fn vk(wp: usize) -> VenueKey {
        VenueKey::Waypoint {
            floor_idx: 0,
            wp_idx: wp,
        }
    }

    fn vis(wp_idx: usize, id: &str, room_id: Option<usize>) -> Visitor {
        Visitor {
            wp_idx,
            agent_id: aid(id),
            anchor: Point {
                x: (wp_idx as u16) * 4 + 10,
                y: 20,
            },
            room_id,
        }
    }

    // `by_venue` is a HashMap and std seeds a fresh `RandomState` per instance, so
    // two calls in ONE process can iterate identical contents in different orders;
    // 32 rounds makes a 3-venue permutation impossible to pass by luck.
    #[test]
    fn bubbles_are_emitted_in_a_deterministic_venue_order() {
        let now = base_time();
        let visitors = [
            vis(9, "/i", None),
            vis(9, "/j", None),
            vis(1, "/a", None),
            vis(1, "/b", None),
            vis(5, "/e", None),
            vis(5, "/f", None),
        ];
        for round in 0..32 {
            let mut state = HashMap::new();
            let anchors: Vec<u16> = update_and_collect(&mut state, 0, &visitors, now)
                .iter()
                .map(|b| b.anchor.x)
                .collect();
            assert_eq!(
                anchors,
                vec![14, 30, 46],
                "round {round}: bubbles must emit in venue order (wp 1, 5, 9)"
            );
        }
    }

    #[test]
    fn test_expires_after_total_ms() {
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![aid("/a"), aid("/b")], start);
        assert!(chat.is_expired(start + Duration::from_millis(7_000)));
    }

    #[test]
    fn test_not_expired_before_total_ms() {
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![aid("/a"), aid("/b")], start);
        assert!(!chat.is_expired(start + Duration::from_millis(3_000)));
    }

    #[test]
    fn round_robin_two_participants_alternates() {
        let start = base_time();
        let (a, b) = (aid("/a"), aid("/b"));
        let chat = ActiveChitchat::new(vk(0), vec![a, b], start);
        let p0 = chat.participants[0];
        let p1 = chat.participants[1];
        assert_eq!(chat.current_bubble(start).unwrap().0, p0);
        assert_eq!(
            chat.current_bubble(start + Duration::from_millis(1_500))
                .unwrap()
                .0,
            p1
        );
        assert_eq!(
            chat.current_bubble(start + Duration::from_millis(3_000))
                .unwrap()
                .0,
            p0
        );
    }

    #[test]
    fn round_robin_cycles_all_participants() {
        let start = base_time();
        let ids: Vec<AgentId> = (0..4).map(|i| aid(&format!("/g{i}"))).collect();
        let chat = ActiveChitchat::new(vk(0), ids.clone(), start);
        let mut speakers = std::collections::HashSet::new();
        for turn in 0..4u64 {
            let t = start + Duration::from_millis(turn * 1_500);
            speakers.insert(chat.current_bubble(t).unwrap().0);
        }
        assert_eq!(speakers.len(), 4, "all four should get a turn");
        for id in &ids {
            assert!(speakers.contains(id));
        }
    }

    #[test]
    fn round_robin_three_participants_wraps() {
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![aid("/x"), aid("/y"), aid("/z")], start);
        let p = chat.participants.clone();
        let speaker = |turn: u64| {
            chat.current_bubble(start + Duration::from_millis(turn * 1_500))
                .unwrap()
                .0
        };
        assert_eq!(speaker(0), p[0]);
        assert_eq!(speaker(1), p[1]);
        assert_eq!(speaker(2), p[2]);
        assert_eq!(speaker(3), p[0]);
    }

    #[test]
    fn empty_participants_yields_no_bubble() {
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![], start);
        assert!(chat.current_bubble(start).is_none());
    }

    #[test]
    fn no_bubble_after_four_turns() {
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![aid("/a"), aid("/b")], start);
        assert!(chat
            .current_bubble(start + Duration::from_millis(6_000))
            .is_none());
    }

    #[test]
    fn current_bubble_line_advances_across_turns_not_frozen() {
        // Every round-robin test above asserts only the SPEAKER (`.0`), so dropping
        // `turn` from `line_idx` — freezing every turn on one quip — goes uncaught.
        let start = base_time();
        let chat = ActiveChitchat::new(vk(0), vec![aid("/a"), aid("/b")], start);
        let lines: Vec<&str> = (0..TURNS)
            .map(|t| {
                chat.current_bubble(start + Duration::from_millis(t * TURN_MS))
                    .unwrap()
                    .1
            })
            .collect();
        for l in &lines {
            assert!(CHITCHAT_LINES.contains(l), "{l:?} not in the quip pool");
        }
        assert!(
            lines.iter().collect::<std::collections::HashSet<_>>().len() > 1,
            "the line must vary across turns (index tracks `turn`), got all {lines:?}"
        );
    }

    #[test]
    fn meeting_slots_in_same_room_form_one_conversation() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors: Vec<Visitor> = vec![vis(4, "/a", Some(0)), vis(5, "/b", Some(0))];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert_eq!(state.len(), 1, "one room conversation, not two");
        assert!(state.contains_key(&VenueKey::Room {
            floor_idx: 0,
            room_id: 0
        }));
        assert_eq!(bubbles.len(), 1);
    }

    #[test]
    fn two_meeting_rooms_host_separate_conversations() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors: Vec<Visitor> = vec![
            vis(4, "/a", Some(0)),
            vis(5, "/b", Some(0)),
            vis(8, "/c", Some(1)),
            vis(9, "/d", Some(1)),
        ];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert_eq!(state.len(), 2, "two rooms → two conversations");
        assert!(state.contains_key(&VenueKey::Room {
            floor_idx: 0,
            room_id: 0
        }));
        assert!(state.contains_key(&VenueKey::Room {
            floor_idx: 0,
            room_id: 1
        }));
        assert_eq!(bubbles.len(), 2);
    }

    #[test]
    fn distinct_waypoints_do_not_merge() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors: Vec<Visitor> =
            vec![vis(0, "/a", None), vis(0, "/b", None), vis(1, "/c", None)];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert_eq!(state.len(), 1, "only the 2-agent waypoint chats");
        assert!(state.contains_key(&VenueKey::Waypoint {
            floor_idx: 0,
            wp_idx: 0
        }));
        assert_eq!(bubbles.len(), 1);
    }

    #[test]
    fn single_visitor_starts_no_conversation() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors: Vec<Visitor> = vec![vis(0, "/a", None)];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert!(state.is_empty());
        assert!(bubbles.is_empty());
    }

    #[test]
    fn participant_join_extends_rotation() {
        let now = base_time();
        let mut state = HashMap::new();
        let v2: Vec<Visitor> = vec![vis(4, "/a", Some(0)), vis(5, "/b", Some(0))];
        update_and_collect(&mut state, 0, &v2, now);
        let key = VenueKey::Room {
            floor_idx: 0,
            room_id: 0,
        };
        assert_eq!(state.get(&key).unwrap().participants.len(), 2);

        let v3: Vec<Visitor> = vec![
            vis(4, "/a", Some(0)),
            vis(5, "/b", Some(0)),
            vis(6, "/c", Some(0)),
        ];
        update_and_collect(&mut state, 0, &v3, now + Duration::from_millis(500));
        assert_eq!(state.get(&key).unwrap().participants.len(), 3);
    }

    #[test]
    fn update_and_collect_expires_old() {
        let start = base_time();
        let mut state = HashMap::new();
        let visitors: Vec<Visitor> = vec![vis(0, "/a", None), vis(0, "/b", None)];
        update_and_collect(&mut state, 0, &visitors, start);
        assert_eq!(state.len(), 1);
        // Past expiry the old chat is reaped and a fresh one created — hence 1,
        // not 0 and not 2.
        update_and_collect(
            &mut state,
            0,
            &visitors,
            start + Duration::from_millis(7_000),
        );
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn multi_slot_venues_collapse_to_first_of_their_own_kind() {
        // Each kind must anchor on its OWN first waypoint: a single group index
        // for every kind merges island standers into the couch conversation.
        use crate::layout::{Facing, Point, Waypoint};
        let wp = |kind, x| Waypoint {
            pos: Point { x, y: 10 },
            kind,
            facing: Facing::South,
            room_id: None,
        };
        let wps = vec![
            wp(WaypointKind::Pantry, 0),      // 0
            wp(WaypointKind::Couch, 10),      // 1 ← couch anchor
            wp(WaypointKind::Couch, 16),      // 2
            wp(WaypointKind::Couch, 22),      // 3
            wp(WaypointKind::Island, 40),     // 4 ← island anchor
            wp(WaypointKind::Island, 50),     // 5
            wp(WaypointKind::SnackShelf, 60), // 6
        ];
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 1, &wps), 1);
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 3, &wps), 1);
        assert_eq!(venue_wp_idx(WaypointKind::Island, 5, &wps), 4);
        assert_eq!(venue_wp_idx(WaypointKind::Island, 4, &wps), 4);
        assert_eq!(venue_wp_idx(WaypointKind::Pantry, 0, &wps), 0);
        assert_eq!(venue_wp_idx(WaypointKind::SnackShelf, 6, &wps), 6);
        assert_eq!(venue_wp_idx(WaypointKind::MeetingSofa, 3, &wps), 3);
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 5, &[]), 5);
    }

    #[test]
    fn supports_chitchat_kinds() {
        assert!(supports_chitchat(WaypointKind::Pantry));
        assert!(supports_chitchat(WaypointKind::Island));
        assert!(supports_chitchat(WaypointKind::SnackShelf));
        assert!(supports_chitchat(WaypointKind::Couch));
        assert!(supports_chitchat(WaypointKind::VendingMachine));
        assert!(supports_chitchat(WaypointKind::Printer));
        assert!(supports_chitchat(WaypointKind::MeetingSofa));
        assert!(supports_chitchat(WaypointKind::MeetingChair));
        assert!(!supports_chitchat(WaypointKind::PhoneBooth));
        assert!(!supports_chitchat(WaypointKind::StandingDesk));
    }

    #[test]
    fn every_waypoint_kind_declares_a_chitchat_venue() {
        // Pins the derivations to `chitchat_venue` over ALL, so supports_chitchat /
        // venue_wp_idx can never drift from the one declared fact.
        let (mut social, mut group) = (0, 0);
        for &kind in WaypointKind::ALL {
            let venue = chitchat_venue(kind);
            assert_eq!(
                supports_chitchat(kind),
                venue != ChitchatVenue::None,
                "{kind:?}: supports_chitchat must track chitchat_venue"
            );
            social += usize::from(venue != ChitchatVenue::None);
            group += usize::from(venue == ChitchatVenue::Group);
        }
        assert_eq!(
            social, 8,
            "8 kinds are social (all but phone booth + standing desk)"
        );
        assert_eq!(
            group, 2,
            "couch + island are the two multi-slot group venues"
        );
    }
}
