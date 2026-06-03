//! Office chitchat — short speech-bubble conversations between agents who
//! share a social venue. A venue is either a single social waypoint (pantry,
//! couch, vending machine, printer) or a whole meeting room (all its sofa +
//! standing slots), so a meeting room hosts one GROUP conversation rather than
//! a pile of independent pairs. Conversations are N-way: each turn the current
//! speaker rotates round-robin through whoever is present.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;

use crate::tui::layout::{Point, WaypointKind};

/// Total duration of a single chitchat exchange (4 turns + silent gap).
pub const CHITCHAT_TOTAL_MS: u64 = 6_000;

/// Each speaker gets 1.5 s per turn.
const TURN_MS: u64 = 1_500;

/// Number of speaking turns before the silent gap.
const TURNS: u64 = 4;

/// Pool of short speech-bubble quips — mostly dev humor, with a few
/// office/watercooler lines that fit the social venues (pantry, couch, meeting
/// room) where these conversations happen. Order doesn't matter: `current_bubble`
/// indexes `% CHITCHAT_LINES.len()`, so the pool can grow freely. Keep each line
/// short (≤ ~12 chars) so it fits the bubble at half-block scale.
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
    // Watercooler — fit the pantry/couch/meeting venues.
    "more coffee?",
    "standup?",
    "lunch?",
    "ship friday",
];

/// A social venue that hosts at most one conversation at a time. Meeting-room
/// slots all map to the same `Room` so the room hosts a single group chat;
/// every other social waypoint is its own `Waypoint` venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VenueKey {
    Room { floor_idx: usize, room_id: usize },
    Waypoint { floor_idx: usize, wp_idx: usize },
}

/// A live conversation among the agents currently at a venue.
pub struct ActiveChitchat {
    pub venue: VenueKey,
    /// Current attendees, sorted ascending by raw id for a stable speaker
    /// rotation. Refreshed each frame so agents joining/leaving the venue are
    /// folded into / out of the rotation.
    pub participants: Vec<AgentId>,
    pub started_at: SystemTime,
    seed: u64,
}

impl ActiveChitchat {
    pub fn new(venue: VenueKey, participants: Vec<AgentId>, now: SystemTime) -> Self {
        let ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut chat = Self {
            venue,
            participants: Vec::new(),
            started_at: now,
            seed: 0,
        };
        chat.set_participants(participants);
        // Seed from the SORTED participant set (set_participants sorts) + start
        // time, so the line choice is independent of the HashMap iteration order
        // the `present` vec was built in — restarting the same group never flips
        // the line just because agents were enumerated in a different order.
        chat.seed = chat
            .participants
            .iter()
            .fold(ms.wrapping_mul(0x9e37_79b9_7f4a_7c15), |acc, a| {
                acc.rotate_left(7) ^ a.raw()
            });
        chat
    }

    /// Replace the attendee set (sorted + de-duplicated) — called each frame so
    /// the rotation tracks who is actually present.
    pub fn set_participants(&mut self, mut participants: Vec<AgentId>) {
        participants.sort_by_key(|a| a.raw());
        participants.dedup();
        self.participants = participants;
    }

    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.elapsed_ms(now) >= CHITCHAT_TOTAL_MS
    }

    fn elapsed_ms(&self, now: SystemTime) -> u64 {
        now.duration_since(self.started_at)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(CHITCHAT_TOTAL_MS)
    }

    /// The agent speaking this turn and their line, or `None` in the silent
    /// gap / once expired / if nobody is present. The speaker rotates
    /// round-robin through `participants`.
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

/// The chitchat `wp_idx` a waypoint visitor groups under. The 3 lounge-couch
/// seats collapse to ONE venue (the first couch's waypoint index) so they host
/// a single group conversation like the meeting room — WITHOUT overloading the
/// meeting-only `room_id` field (which indexes `meeting_tables`). Every other
/// waypoint keys on its own index. `couch_group_idx` is the first `Couch`
/// waypoint's index, or `None` if the layout has no couch.
pub fn venue_wp_idx(kind: WaypointKind, wp_idx: usize, couch_group_idx: Option<usize>) -> usize {
    match kind {
        WaypointKind::Couch => couch_group_idx.unwrap_or(wp_idx),
        _ => wp_idx,
    }
}

/// Whether agents at this waypoint kind can start a chitchat.
pub fn supports_chitchat(kind: WaypointKind) -> bool {
    matches!(
        kind,
        WaypointKind::Pantry
            | WaypointKind::Couch
            | WaypointKind::VendingMachine
            | WaypointKind::Printer
            | WaypointKind::MeetingSofa
            | WaypointKind::MeetingStand
    )
}

/// A single speech bubble ready for the widget layer to render.
pub struct ChitchatBubble {
    pub text: &'static str,
    /// Pixel coords of the speaking agent's anchor.
    pub anchor: Point,
}

/// A chitchat-eligible agent present at a venue this frame. `room_id` is
/// `Some` for meeting slots (they group by room) and `None` for single-point
/// waypoints (which group by `wp_idx`). Named (not a tuple) so the producer
/// and consumer can't transpose the two `usize`-ish fields.
#[derive(Debug, Clone, Copy)]
pub struct Visitor {
    pub wp_idx: usize,
    pub agent_id: AgentId,
    pub anchor: Point,
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
    // Expire old conversations.
    state.retain(|_, chat| !chat.is_expired(now));

    // Group visitors by venue (meeting slots → their room, others → the point).
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

    let mut bubbles = Vec::new();
    for (venue, agents) in &by_venue {
        if agents.len() < 2 {
            continue;
        }
        let present: Vec<AgentId> = agents.iter().map(|(id, _)| *id).collect();

        let chat = state
            .entry(*venue)
            .or_insert_with(|| ActiveChitchat::new(*venue, present.clone(), now));
        // Refresh the rotation so joiners/leavers are tracked.
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
        // Sorted ascending: participants[0] speaks turn 0, [1] turn 1, [0] 2...
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
        // Four turns, four participants → every participant speaks exactly once.
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
        // turns 0,1,2,3 → p0,p1,p2,p0
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
        // The participants.is_empty() short-circuit in current_bubble: a venue
        // with no attendees never speaks even at turn 0.
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
    fn meeting_slots_in_same_room_form_one_conversation() {
        let now = base_time();
        let mut state = HashMap::new();
        // Two different meeting-room waypoints (wp 4 and 5) in room 0.
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
        // A dual-meeting-room floor: room 0 and room 1 each get a pair. They
        // must NOT merge — `room_id` keys distinct venues.
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
        // Two agents at wp 0 and one agent each at wp 1 — only wp 0 (with 2)
        // chats; wp 1's lone agent does not.
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
        // Start with two agents in room 0.
        let v2: Vec<Visitor> = vec![vis(4, "/a", Some(0)), vis(5, "/b", Some(0))];
        update_and_collect(&mut state, 0, &v2, now);
        let key = VenueKey::Room {
            floor_idx: 0,
            room_id: 0,
        };
        assert_eq!(state.get(&key).unwrap().participants.len(), 2);

        // A third joins mid-conversation → rotation now includes them.
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
        // Past expiry → reaped, then a fresh one created (both still present).
        update_and_collect(
            &mut state,
            0,
            &visitors,
            start + Duration::from_millis(7_000),
        );
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn lounge_couch_seats_share_one_venue() {
        // The 3 couch seats (distinct wp_idx) all collapse to the first
        // couch's index → one VenueKey → one group conversation. Other
        // waypoints keep their own index. This is what makes the lounge a
        // group-chat venue without touching the meeting-only room_id.
        let gi = Some(7);
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 7, gi), 7);
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 8, gi), 7);
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 9, gi), 7);
        // Non-couch waypoints are unaffected.
        assert_eq!(venue_wp_idx(WaypointKind::Pantry, 12, gi), 12);
        assert_eq!(venue_wp_idx(WaypointKind::MeetingSofa, 3, gi), 3);
        // No couch in the layout → falls back to the visitor's own index.
        assert_eq!(venue_wp_idx(WaypointKind::Couch, 5, None), 5);
    }

    #[test]
    fn supports_chitchat_kinds() {
        assert!(supports_chitchat(WaypointKind::Pantry));
        assert!(supports_chitchat(WaypointKind::Couch));
        assert!(supports_chitchat(WaypointKind::VendingMachine));
        assert!(supports_chitchat(WaypointKind::Printer));
        assert!(supports_chitchat(WaypointKind::MeetingSofa));
        assert!(supports_chitchat(WaypointKind::MeetingStand));
        assert!(!supports_chitchat(WaypointKind::PhoneBooth));
        assert!(!supports_chitchat(WaypointKind::StandingDesk));
    }
}
