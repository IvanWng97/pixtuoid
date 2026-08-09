//! Backend-agnostic ambient-audio MODEL: the scene emits semantic stem levels +
//! one-shot events, and each painter's audio system renders them its own way.
//! NO audio dependencies live in this crate — the crate boundary is the
//! compiler tooth, exactly like the terminal/window ban.

// The PURE synthesis stack: dsp kernels, frozen lofi compositions, per-voice
// synth recipes, runtime mixer/schedulers. Shared so the native device gateway
// and the wasm WebAudio painter build the SAME sample buffers, with no
// audio-device deps (pure math; the rodio/cpal ban still holds).
#[doc(hidden)]
pub mod bank;
#[doc(hidden)]
pub mod compose;
#[doc(hidden)]
pub mod dsp;
#[doc(hidden)]
pub mod engine;
#[doc(hidden)]
pub mod mixer;
// Private, not `pub`: the frozen takes are `#[cfg(test)]` fingerprint ANCHORS
// read only by the sibling `synth`/`compose`, so a `pub mod` would offer a
// publicly reachable empty path.
mod score;
#[doc(hidden)]
pub mod synth;

// The shared per-tick engine surface — both audio painters build on these, so
// they can't drift.
pub use bank::OneShotPool;
pub use engine::{AudioEngine, PlayCmd, TickCommands, MAX_DT_S};

use crate::board::StateCounts;

/// Fixed RNG seeds for the four ambient-synth voices, in ONE place because both
/// painters MUST seed identically — a per-crate copy silently desyncs the two
/// soundtracks on the next edit. `BUILD_SEED` seeds the build-time noise; the
/// rest seed the per-tick keystroke / rain-drop schedulers and their picker.
#[doc(hidden)]
pub const BUILD_SEED: u64 = 0xC0FF_EE01;
#[doc(hidden)]
pub const TYPING_SEED: u64 = 0xBEEF;
#[doc(hidden)]
pub const DROP_SEED: u64 = 0xFACE;
#[doc(hidden)]
pub const PICK_SEED: u64 = 0xDEAD;

/// Active-agent count at which the office reads BUSY (full band + dense
/// typing). 1..BUSY_ACTIVE_MIN is the moderate anchor tier; 0 is empty.
const BUSY_ACTIVE_MIN: usize = 3;

/// The rain stem's gain at full precipitation — tuned well under the tier-1
/// music, or the broadband rain masks the mix. The drop one-shots derive from
/// this via `DROP_GAIN × wanted.rain`.
const RAIN_GAIN: f32 = 0.30;

/// Per-tier stem gains, `[empty, moderate, busy]` — the ratified demo mixes.
const PAD_GAIN: [f32; 3] = [0.75, 0.70, 0.65];
const SPARKLE_GAIN: [f32; 3] = [0.70, 0.0, 0.0];
const KEYS_GAIN: [f32; 3] = [0.0, 0.60, 0.70];
const DRUMS_GAIN: [f32; 3] = [0.0, 0.35, 0.60];
// Boosted far past the Phase-0 ratification: the hiss+crackle layer was
// inaudible at the ratified level.
const TEXTURE_GAIN: [f32; 3] = [0.78, 0.84, 0.78];
// Never zero (the floor used to ride inside the night pad); the curve RISES
// where the pad's falls — the lane must hold up once drums+keys+typing enter.
const BASS_GAIN: [f32; 3] = [0.60, 0.70, 0.75];
const TYPING_GAIN: [f32; 3] = [0.0, 0.50, 0.80];

/// Target mix levels (0..=1) for every stem, derived once per frame. `typing`
/// is a PROCEDURAL stem: the consumer owns burst scheduling; the scene only says
/// how much typing the office holds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StemLevels {
    pub pad: f32,
    pub sparkle: f32,
    pub keys: f32,
    pub drums: f32,
    pub texture: f32,
    pub bass: f32,
    pub rain: f32,
    pub typing: f32,
}

/// A fire-once audio event, emitted on state EDGES by the cue tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneShot {
    DoorChime,
    PrinterWhir,
    VendingDrop,
}

/// One frame of audio intent: target stem levels + the events that fired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioFrame {
    pub stems: StemLevels,
    pub events: Vec<OneShot>,
    /// Which mood track the musical beds should play — selected scene-side from
    /// the SAME day/night boundary the lighting renders, plus the weather.
    pub track: TrackId,
}

/// The soundtrack ids — ALL-GENERATIVE: every [`TRACK_EPOCH_SECS`] block
/// COMPOSES a fresh take. The payload is the compose seed (the [`track_epoch`]
/// block), so the id changing IS the song change and the [`TrackSwitch`]
/// crossfade machinery needs no new state. Deterministic everywhere: the same
/// block renders the same song on native, wasm, and in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackId {
    /// The block's generated day-mood take.
    GenDay(u64),
    /// The block's generated night-mood take (also the rainy mood).
    GenNight(u64),
}

impl Default for TrackId {
    fn default() -> Self {
        TrackId::GenDay(0)
    }
}

/// One song per this many wall-clock seconds, owner-tuned: agent sessions are
/// usually SHORT, and an hourly rotation meant most sessions never heard the
/// song change. The weather's matching re-roll cadence lives in `sky.rs` — a
/// separate domain, deliberately not shared.
pub const TRACK_EPOCH_SECS: u64 = 600;

/// The soundtrack epoch (blocks since UNIX epoch) — the compose-seed input,
/// derived ONCE here so the native observer and the wasm painter can't drift.
/// Pre-epoch clocks read as block 0.
pub fn track_epoch(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / TRACK_EPOCH_SECS)
}

/// Pure track selection: night hours or any precipitation pick the night MOOD,
/// and the [`track_epoch`] block is the compose seed. Pure in its inputs so wasm
/// can feed its parametric clock and tests need none; within a block the pick is
/// stable, so the crossfade fires at most once per [`TRACK_EPOCH_SECS`].
pub fn select_track(is_day: bool, precipitation: f32, track_epoch: u64) -> TrackId {
    if !is_day || precipitation > 0.0 {
        TrackId::GenNight(track_epoch)
    } else {
        TrackId::GenDay(track_epoch)
    }
}

/// Decode a [`TrackId`] to its composed score — the ONE
/// `TrackId → (Mood, seed) → compose` bridge both build shells call, so the mood
/// decode lives once instead of a hand-synced match per shell.
pub fn compose_track(track: TrackId) -> compose::GeneratedScore {
    let (mood, seed) = match track {
        TrackId::GenDay(seed) => (compose::Mood::Day, seed),
        TrackId::GenNight(seed) => (compose::Mood::Night, seed),
    };
    compose::compose(mood, seed)
}

impl StemLevels {
    /// Zero the TRACK-owned musical stems, leaving rain + typing (weather +
    /// activity, track-independent). The "hold silent" half of the mood-track
    /// crossfade, called while a switch is [`TrackSwitch::is_holding`].
    pub fn silence_track_stems(&mut self) {
        // Driven off the ONE track-owned set so a forgotten new music stem is
        // compiler-caught rather than a silently un-zeroed field here.
        for stem in crate::audio::bank::TRACK_STEMS {
            *stem.field_mut(self) = 0.0;
        }
    }
}

/// The mood-track switch machine — the PURE state half both players run, so the
/// latch/hold/silent gate can't drift between them. It owns ONLY the state: the
/// BUILD stays caller-side, since that's the one thing the two backends do
/// differently.
///
/// Lifecycle: `init` on the first frame → `request` a switch on a changed
/// [`TrackId`] (LATCHED, so an hour/weather flap at a boundary can't thrash the
/// synths) → while `is_holding`, the caller silences the track stems → once they
/// reach silence, `try_swap` hands back the new track and releases the hold.
#[derive(Debug, Default)]
pub struct TrackSwitch {
    current: Option<TrackId>,
    pending: Option<TrackId>,
}

impl TrackSwitch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registered track, or `None` before the first `init`.
    pub fn current(&self) -> Option<TrackId> {
        self.current
    }

    /// First frame ONLY: adopt `track` as current and return `Some(track)` to
    /// build + register its beds. `None` once initialized — use
    /// [`TrackSwitch::request`] thereafter.
    pub fn init(&mut self, track: TrackId) -> Option<TrackId> {
        if self.current.is_none() {
            self.current = Some(track);
            Some(track)
        } else {
            None
        }
    }

    /// Record a requested switch — ignored while unchanged or while a switch
    /// is already in flight (the settling latch). No-op before `init`.
    pub fn request(&mut self, track: TrackId) {
        if let Some(cur) = self.current {
            if track != cur && self.pending.is_none() {
                self.pending = Some(track);
            }
        }
    }

    /// Whether a switch is in flight (the caller holds the track stems silent).
    pub fn is_holding(&self) -> bool {
        self.pending.is_some()
    }

    /// Once the held track stems have reached silence, commit the pending
    /// switch and return `Some(to)` to build + swap in. `None` until then.
    pub fn try_swap(&mut self, track_silent: bool) -> Option<TrackId> {
        if let Some(to) = self.pending {
            if track_silent {
                self.current = Some(to);
                self.pending = None;
                return Some(to);
            }
        }
        None
    }
}

/// Cross-frame cue state: diffs identity/occupancy sets frame-to-frame and
/// emits each [`OneShot`] exactly once on the EDGE. The FIRST observe only
/// primes — attaching to a full office must not fire a door-chime volley.
#[derive(Debug, Default)]
pub struct AudioCueTracker {
    primed: bool,
    seen_agents: std::collections::HashSet<pixtuoid_core::AgentId>,
    occupied: std::collections::HashSet<usize>,
}

impl AudioCueTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one frame's observations; returns the events that fired on this
    /// frame's edges. `waypoint_kind` resolves an occupied-waypoint index to its
    /// kind so the tracker never holds a `Layout` borrow and tests need no
    /// layout at all. Purely EDGE-triggered — it takes no clock, so a caller
    /// can't read it as time-dependent.
    pub fn observe<'a>(
        &mut self,
        agent_ids: impl IntoIterator<Item = &'a pixtuoid_core::AgentId>,
        occupied_waypoints: &std::collections::HashSet<usize>,
        waypoint_kind: impl Fn(usize) -> Option<crate::layout::WaypointKind>,
    ) -> Vec<OneShot> {
        use crate::layout::WaypointKind;

        let ids: std::collections::HashSet<pixtuoid_core::AgentId> =
            agent_ids.into_iter().cloned().collect();

        if !self.primed {
            self.primed = true;
            self.seen_agents = ids;
            self.occupied = occupied_waypoints.clone();
            return Vec::new();
        }

        let mut events = Vec::new();

        // Capped at ONE per frame — a workflow fleet arriving together is one
        // door moment, not a chime chord.
        if ids.difference(&self.seen_agents).next().is_some() {
            events.push(OneShot::DoorChime);
        }
        self.seen_agents = ids;

        // A waypoint BECOMING occupied is the moment the matching feedback
        // animation starts — `sim.rs` keys its anims on this same set.
        for &idx in occupied_waypoints.difference(&self.occupied) {
            match waypoint_kind(idx) {
                Some(WaypointKind::Printer) => events.push(OneShot::PrinterWhir),
                Some(WaypointKind::VendingMachine) => events.push(OneShot::VendingDrop),
                _ => {}
            }
        }
        self.occupied = occupied_waypoints.clone();

        events
    }
}

/// The busy-ness tier index for the gain tables: 0 empty, 1 moderate, 2 busy.
fn tier(counts: &StateCounts) -> usize {
    if counts.active >= BUSY_ACTIVE_MIN {
        2
    } else if counts.active >= 1 {
        1
    } else {
        0
    }
}

/// Map the office's activity + weather onto target stem levels, with rain
/// scaling on the precipitation scalar (0 clear … 1 storm).
pub fn stem_levels(counts: &StateCounts, precipitation: f32) -> StemLevels {
    let t = tier(counts);
    StemLevels {
        pad: PAD_GAIN[t],
        sparkle: SPARKLE_GAIN[t],
        keys: KEYS_GAIN[t],
        drums: DRUMS_GAIN[t],
        texture: TEXTURE_GAIN[t],
        bass: BASS_GAIN[t],
        rain: RAIN_GAIN * precipitation.clamp(0.0, 1.0),
        typing: TYPING_GAIN[t],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_track_composes_the_hour_by_mood() {
        for h in 0..48 {
            assert_eq!(select_track(true, 0.0, h), TrackId::GenDay(h));
            assert_eq!(select_track(false, 0.0, h), TrackId::GenNight(h));
            assert_eq!(select_track(true, 0.6, h), TrackId::GenNight(h));
            assert_eq!(select_track(false, 1.0, h), TrackId::GenNight(h));
            assert_eq!(
                select_track(true, f32::MIN_POSITIVE, h),
                TrackId::GenNight(h)
            );
        }
    }

    #[test]
    fn track_is_stable_within_a_block_and_changes_across_blocks() {
        use std::time::{Duration, UNIX_EPOCH};
        for b in 0..24u64 {
            let early = UNIX_EPOCH + Duration::from_secs(b * TRACK_EPOCH_SECS + 1);
            let late = UNIX_EPOCH + Duration::from_secs((b + 1) * TRACK_EPOCH_SECS - 1);
            assert_eq!(
                select_track(true, 0.0, track_epoch(early)),
                select_track(true, 0.0, track_epoch(late)),
                "take must hold steady within block {b}"
            );
            assert_ne!(
                select_track(true, 0.0, b),
                select_track(true, 0.0, b + 1),
                "the crossfade must fire at the block boundary"
            );
        }
    }

    #[test]
    fn track_epoch_derivation() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(track_epoch(UNIX_EPOCH), 0);
        assert_eq!(
            track_epoch(UNIX_EPOCH + Duration::from_secs(TRACK_EPOCH_SECS - 1)),
            0
        );
        assert_eq!(
            track_epoch(UNIX_EPOCH + Duration::from_secs(TRACK_EPOCH_SECS)),
            1
        );
        assert_eq!(
            track_epoch(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            1_700_000_000 / TRACK_EPOCH_SECS
        );
        assert_eq!(track_epoch(UNIX_EPOCH - Duration::from_secs(10)), 0);
    }

    #[test]
    fn track_switch_inits_then_latches_a_change_until_silence() {
        let mut sw = TrackSwitch::new();
        assert_eq!(sw.current(), None);
        assert_eq!(sw.init(TrackId::GenDay(0)), Some(TrackId::GenDay(0)));
        assert_eq!(sw.current(), Some(TrackId::GenDay(0)));
        assert_eq!(
            sw.init(TrackId::GenNight(0)),
            None,
            "init is first-frame only"
        );
        assert!(!sw.is_holding());

        sw.request(TrackId::GenDay(0));
        assert!(!sw.is_holding());
        sw.request(TrackId::GenNight(0));
        assert!(sw.is_holding(), "a changed track holds the stems silent");

        sw.request(TrackId::GenDay(0));
        assert_eq!(sw.try_swap(false), None);
        assert!(sw.is_holding());
        assert_eq!(sw.try_swap(true), Some(TrackId::GenNight(0)));
        assert_eq!(sw.current(), Some(TrackId::GenNight(0)));
        assert!(!sw.is_holding());
        assert_eq!(sw.try_swap(true), None, "nothing pending after the swap");
    }

    #[test]
    fn silence_track_stems_zeroes_music_keeps_weather_and_typing() {
        let mut s = StemLevels {
            pad: 0.4,
            sparkle: 0.3,
            keys: 0.5,
            drums: 0.6,
            texture: 0.2,
            bass: 0.5,
            rain: 0.7,
            typing: 0.8,
        };
        s.silence_track_stems();
        assert_eq!(
            (s.pad, s.sparkle, s.keys, s.drums, s.texture, s.bass),
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            (s.rain, s.typing),
            (0.7, 0.8),
            "rain + typing are track-independent"
        );
    }

    fn counts(active: usize) -> StateCounts {
        StateCounts {
            active,
            waiting: 0,
            idle: 0,
            exiting: 0,
            total: active,
        }
    }

    #[test]
    fn stem_levels_map_the_busyness_tiers() {
        let empty = stem_levels(&counts(0), 0.0);
        assert_eq!(empty.pad, PAD_GAIN[0]);
        assert_eq!(empty.sparkle, SPARKLE_GAIN[0]);
        assert_eq!(empty.keys, 0.0);
        assert_eq!(empty.drums, 0.0);
        assert_eq!(empty.typing, 0.0);

        let moderate = stem_levels(&counts(1), 0.0);
        assert_eq!(moderate.keys, KEYS_GAIN[1]);
        assert_eq!(moderate.sparkle, 0.0);

        let last_moderate = stem_levels(&counts(BUSY_ACTIVE_MIN - 1), 0.0);
        assert_eq!(last_moderate.drums, DRUMS_GAIN[1]);
        let busy = stem_levels(&counts(BUSY_ACTIVE_MIN), 0.0);
        assert_eq!(busy.drums, DRUMS_GAIN[2]);
        assert_eq!(busy.typing, TYPING_GAIN[2]);
        assert_eq!(empty.bass, BASS_GAIN[0], "the floor never empties");
        assert_eq!(busy.bass, BASS_GAIN[2]);
    }

    #[test]
    fn stem_levels_typing_scales_with_active_agents() {
        assert_eq!(stem_levels(&counts(0), 0.0).typing, 0.0);
        assert_eq!(stem_levels(&counts(1), 0.0).typing, TYPING_GAIN[1]);
        assert_eq!(
            stem_levels(&counts(BUSY_ACTIVE_MIN), 0.0).typing,
            TYPING_GAIN[2]
        );
    }

    #[test]
    fn stem_levels_rain_tracks_precipitation() {
        assert_eq!(stem_levels(&counts(0), 0.0).rain, 0.0);
        assert_eq!(stem_levels(&counts(0), 1.0).rain, RAIN_GAIN);
        let half = stem_levels(&counts(0), 0.5).rain;
        assert!((half - RAIN_GAIN * 0.5).abs() < 1e-6);
        assert_eq!(stem_levels(&counts(0), -1.0).rain, 0.0);
        assert_eq!(stem_levels(&counts(0), 2.0).rain, RAIN_GAIN);
    }

    use crate::layout::WaypointKind;
    use pixtuoid_core::AgentId;
    use std::collections::HashSet;

    fn aid(n: usize) -> AgentId {
        AgentId::from_parts("test", &n.to_string())
    }

    /// A fixed waypoint-kind table: 5 = printer, 7 = vending, else couch.
    fn kinds(idx: usize) -> Option<WaypointKind> {
        match idx {
            5 => Some(WaypointKind::Printer),
            7 => Some(WaypointKind::VendingMachine),
            _ => Some(WaypointKind::Couch),
        }
    }

    #[test]
    fn tracker_primes_silently_then_chimes_once_per_new_agent_wave() {
        let mut tr = AudioCueTracker::new();
        let none = HashSet::new();
        // priming frame: an already-full office fires NOTHING (mid-attach)
        assert!(tr.observe(&[aid(1)], &none, kinds).is_empty());
        assert_eq!(
            tr.observe(&[aid(1), aid(2)], &none, kinds),
            vec![OneShot::DoorChime]
        );
        assert!(tr.observe(&[aid(1), aid(2)], &none, kinds).is_empty());
        assert_eq!(
            tr.observe(&[aid(1), aid(2), aid(3), aid(4), aid(5)], &none, kinds),
            vec![OneShot::DoorChime]
        );
        assert!(tr.observe(&[aid(1)], &none, kinds).is_empty());
        assert_eq!(
            tr.observe(&[aid(1), aid(2)], &none, kinds),
            vec![OneShot::DoorChime]
        );
    }

    #[test]
    fn tracker_emits_printer_whir_exactly_once_per_animation() {
        let mut tr = AudioCueTracker::new();
        let ids = [aid(1)];
        tr.observe(&ids, &HashSet::new(), kinds); // prime
        let at_printer: HashSet<usize> = [5].into();
        assert_eq!(
            tr.observe(&ids, &at_printer, kinds),
            vec![OneShot::PrinterWhir]
        );
        assert!(tr.observe(&ids, &at_printer, kinds).is_empty());
        assert!(tr.observe(&ids, &at_printer, kinds).is_empty());
        assert!(tr.observe(&ids, &HashSet::new(), kinds).is_empty());
        assert_eq!(
            tr.observe(&ids, &at_printer, kinds),
            vec![OneShot::PrinterWhir]
        );
    }

    #[test]
    fn tracker_maps_vending_and_ignores_non_appliance_waypoints() {
        let mut tr = AudioCueTracker::new();
        let ids = [aid(1)];
        tr.observe(&ids, &HashSet::new(), kinds); // prime
        let occupied: HashSet<usize> = [2, 7].into();
        assert_eq!(
            tr.observe(&ids, &occupied, kinds),
            vec![OneShot::VendingDrop]
        );
    }

    #[test]
    fn waiting_and_idle_agents_do_not_raise_the_tier() {
        let c = StateCounts {
            active: 0,
            waiting: 5,
            idle: 3,
            exiting: 1,
            total: 9,
        };
        assert_eq!(stem_levels(&c, 0.0).drums, 0.0);
    }
}
