//! Backend-agnostic ambient-audio MODEL — the sound twin of [`crate::overlay`]
//! / [`crate::board`]: the scene emits semantic stem levels + one-shot events,
//! and each painter's audio system (the binary's mixer today, WebAudio later)
//! renders them its own way. NO audio dependencies live in this crate — the
//! crate boundary is the compiler tooth, exactly like the terminal/window ban.
//!
//! Every level constant below is an owner-ratified mix gain from the Phase 0
//! audition (docs/superpowers/specs/2026-07-16-ambient-sound-phase0/, #633):
//! `demo_1_empty` / `demo_2_moderate` (THE taste anchor) / `demo_3_busy`.

// The PURE synthesis + direction stack (#633 web-audio): dsp kernels, the
// frozen lofi compositions, the per-voice synth recipes, and the runtime
// mixer/schedulers. Moved here from the binary so BOTH the native device
// gateway (rodio, in `pixtuoid`) AND the wasm WebAudio painter (`pixtuoid-web`)
// build the SAME sample buffers — the sound twin of `render_to_rgb_buffer`.
// NO audio-device deps live here (pure math; the rodio/cpal ban still holds).
// `#[doc(hidden)]`: workspace-internal, not stable engine API (overlay/board pattern).
#[doc(hidden)]
pub mod bank;
#[doc(hidden)]
pub mod dsp;
#[doc(hidden)]
pub mod mixer;
#[doc(hidden)]
pub mod score;
#[doc(hidden)]
pub mod synth;

use crate::board::StateCounts;

/// Active-agent count at which the office reads BUSY (full band + dense
/// typing). 1..BUSY_ACTIVE_MIN is the moderate anchor tier; 0 is empty.
const BUSY_ACTIVE_MIN: usize = 3;

/// The rain stem's gain at full precipitation (demo_4's ratified mix gain).
const RAIN_GAIN: f32 = 0.55;

/// Per-tier stem gains, `[empty, moderate, busy]` — the ratified demo mixes.
const PAD_GAIN: [f32; 3] = [0.75, 0.70, 0.65];
const SPARKLE_GAIN: [f32; 3] = [0.70, 0.0, 0.0];
const KEYS_GAIN: [f32; 3] = [0.0, 0.60, 0.70];
const DRUMS_GAIN: [f32; 3] = [0.0, 0.35, 0.60];
const TEXTURE_GAIN: [f32; 3] = [0.28, 0.30, 0.28];
const TYPING_GAIN: [f32; 3] = [0.0, 0.50, 0.80];

/// Target mix levels (0..=1) for every stem, derived once per frame. A VALUE
/// snapshot like [`crate::board::BoardModel`] — no identity, never persisted,
/// not a wire contract. `typing` is a PROCEDURAL stem: the consumer owns
/// burst scheduling; the scene only says how much typing the office holds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StemLevels {
    pub pad: f32,
    pub sparkle: f32,
    pub keys: f32,
    pub drums: f32,
    pub texture: f32,
    pub rain: f32,
    pub typing: f32,
}

/// A fire-once audio event. Emitted on state EDGES by the cue tracker —
/// consumers play it exactly once per emission.
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
    /// Which mood track the musical beds should play (#644) — selected
    /// scene-side from the SAME day/night boundary the lighting renders
    /// plus the weather. The binary's audio thread crossfades on change.
    pub track: TrackId,
}

/// The mood-track registry ids (#644). Day = the original ratified
/// composition; Night = the Lofi Girl-anchored slow take, also chosen
/// whenever it rains (the office's rainy mood).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackId {
    #[default]
    Day,
    Night,
}

/// Pure track selection: night hours (the painter's OWN sun window via
/// `pixel_painter::hour_is_day`/`is_day_at`) or any precipitation pick
/// the night take. Pure in its inputs so wasm can feed its parametric
/// hour and tests need no clock.
pub fn select_track(is_day: bool, precipitation: f32) -> TrackId {
    if !is_day || precipitation > 0.0 {
        TrackId::Night
    } else {
        TrackId::Day
    }
}

impl StemLevels {
    /// Zero the five TRACK-owned musical stems (pad/sparkle/keys/drums/
    /// texture), leaving rain + typing (weather + activity, track-independent).
    /// The "hold silent" half of the mood-track crossfade — a player calls it
    /// on the target while a switch is [`TrackSwitch::is_holding`].
    pub fn silence_track_stems(&mut self) {
        self.pad = 0.0;
        self.sparkle = 0.0;
        self.keys = 0.0;
        self.drums = 0.0;
        self.texture = 0.0;
    }
}

/// The mood-track switch machine (#644) — the PURE state half both players
/// run (the native rodio thread AND the wasm WebAudio driver, #633 web-audio),
/// so the latch/hold/silent-gate can't drift between them. It owns ONLY the
/// state: the BUILD (blocking synth on native, chunked warmup on web) stays
/// caller-side, since that's the one thing the two backends do differently.
///
/// Lifecycle: `init` on the first frame (build + register the right mood) →
/// `request` a switch on a changed [`TrackId`] (LATCHED — a hour/weather
/// flap at a boundary can't thrash the synths) → while `is_holding`, the
/// caller silences the track stems → once they reach silence, `try_swap`
/// hands back the new track to build + swap in and releases the hold.
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
    /// build + register its beds. Returns `None` once initialized (use
    /// [`TrackSwitch::request`] thereafter).
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

    /// When the held track stems have reached silence (`track_silent`),
    /// commit the pending switch: adopt it as current, clear the latch, and
    /// return `Some(to)` to build + swap in. `None` until then.
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

/// Cross-frame cue state — the audio twin of the painter session halves
/// (`PerOffice` pattern). Diffs identity/occupancy sets frame-to-frame and
/// emits each [`OneShot`] exactly once on the EDGE (a 30fps rebuild never
/// re-fires; state updates as frames arrive — never derived by scanning
/// backward). The FIRST observe only primes: attaching to a full office
/// must not fire a door-chime volley.
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
    /// frame's edges. `waypoint_kind` resolves an occupied-waypoint index to
    /// its kind (painters pass a closure over `layout.waypoints`) so the
    /// tracker never holds a `Layout` borrow and tests need no layout at all.
    pub fn observe<'a>(
        &mut self,
        agent_ids: impl IntoIterator<Item = &'a pixtuoid_core::AgentId>,
        occupied_waypoints: &std::collections::HashSet<usize>,
        waypoint_kind: impl Fn(usize) -> Option<crate::layout::WaypointKind>,
        now: std::time::SystemTime,
    ) -> Vec<OneShot> {
        use crate::layout::WaypointKind;

        let _ = now; // per-frame clock; unused since the glug cut, kept for edge-cue timing
        let ids: std::collections::HashSet<pixtuoid_core::AgentId> =
            agent_ids.into_iter().cloned().collect();

        if !self.primed {
            self.primed = true;
            self.seen_agents = ids;
            self.occupied = occupied_waypoints.clone();
            return Vec::new();
        }

        let mut events = Vec::new();

        // Door chime: an id we have never seen walked in. Capped at ONE per
        // frame — a workflow fleet arriving together is one door moment, not
        // a chime chord.
        if ids.difference(&self.seen_agents).next().is_some() {
            events.push(OneShot::DoorChime);
        }
        self.seen_agents = ids;

        // Appliance cues: a waypoint BECOMING occupied is the moment the
        // matching feedback animation starts (sim.rs keys the printer-eject /
        // vending-drop anims on this same set).
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

/// Map the office's activity + weather onto target stem levels — the ratified
/// tier profiles, with rain scaling on the precipitation scalar (0 clear …
/// 1 storm, from `pixel_painter::precipitation_level`).
pub fn stem_levels(counts: &StateCounts, precipitation: f32) -> StemLevels {
    let t = tier(counts);
    StemLevels {
        pad: PAD_GAIN[t],
        sparkle: SPARKLE_GAIN[t],
        keys: KEYS_GAIN[t],
        drums: DRUMS_GAIN[t],
        texture: TEXTURE_GAIN[t],
        rain: RAIN_GAIN * precipitation.clamp(0.0, 1.0),
        typing: TYPING_GAIN[t],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_track_truth_table() {
        // day + dry = Day; night hours OR any rain = Night
        assert_eq!(select_track(true, 0.0), TrackId::Day);
        assert_eq!(select_track(false, 0.0), TrackId::Night);
        assert_eq!(select_track(true, 0.6), TrackId::Night);
        assert_eq!(select_track(false, 1.0), TrackId::Night);
        assert_eq!(select_track(true, f32::MIN_POSITIVE), TrackId::Night);
    }

    #[test]
    fn track_switch_inits_then_latches_a_change_until_silence() {
        let mut sw = TrackSwitch::new();
        assert_eq!(sw.current(), None);
        // first frame builds+registers, does NOT request
        assert_eq!(sw.init(TrackId::Day), Some(TrackId::Day));
        assert_eq!(sw.current(), Some(TrackId::Day));
        assert_eq!(sw.init(TrackId::Night), None, "init is first-frame only");
        assert!(!sw.is_holding());

        // an unchanged request is a no-op; a real change latches the hold
        sw.request(TrackId::Day);
        assert!(!sw.is_holding());
        sw.request(TrackId::Night);
        assert!(sw.is_holding(), "a changed track holds the stems silent");

        // a SECOND change mid-flight is ignored (the settling latch)
        sw.request(TrackId::Day);
        // not silent yet → no swap
        assert_eq!(sw.try_swap(false), None);
        assert!(sw.is_holding());
        // silence reached → swap to the FIRST-latched target, release the hold
        assert_eq!(sw.try_swap(true), Some(TrackId::Night));
        assert_eq!(sw.current(), Some(TrackId::Night));
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
            rain: 0.7,
            typing: 0.8,
        };
        s.silence_track_stems();
        assert_eq!(
            (s.pad, s.sparkle, s.keys, s.drums, s.texture),
            (0.0, 0.0, 0.0, 0.0, 0.0)
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
        // empty: pad + sparkle + texture only (demo_1)
        let empty = stem_levels(&counts(0), 0.0);
        assert_eq!(empty.pad, PAD_GAIN[0]);
        assert_eq!(empty.sparkle, SPARKLE_GAIN[0]);
        assert_eq!(empty.keys, 0.0);
        assert_eq!(empty.drums, 0.0);
        assert_eq!(empty.typing, 0.0);

        // both sides of the empty→moderate edge (1 = first moderate value)
        let moderate = stem_levels(&counts(1), 0.0);
        assert_eq!(moderate.keys, KEYS_GAIN[1]);
        assert_eq!(moderate.sparkle, 0.0);

        // both sides of the moderate→busy edge, offsets from the const
        let last_moderate = stem_levels(&counts(BUSY_ACTIVE_MIN - 1), 0.0);
        assert_eq!(last_moderate.drums, DRUMS_GAIN[1]);
        let busy = stem_levels(&counts(BUSY_ACTIVE_MIN), 0.0);
        assert_eq!(busy.drums, DRUMS_GAIN[2]);
        assert_eq!(busy.typing, TYPING_GAIN[2]);
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
        // out-of-range precipitation is clamped, both sides
        assert_eq!(stem_levels(&counts(0), -1.0).rain, 0.0);
        assert_eq!(stem_levels(&counts(0), 2.0).rain, RAIN_GAIN);
    }

    use crate::layout::WaypointKind;
    use pixtuoid_core::AgentId;
    use std::collections::HashSet;
    use std::time::SystemTime;

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

    const T0: SystemTime = SystemTime::UNIX_EPOCH;

    #[test]
    fn tracker_primes_silently_then_chimes_once_per_new_agent_wave() {
        let mut tr = AudioCueTracker::new();
        let none = HashSet::new();
        // priming frame: an already-full office fires NOTHING (mid-attach)
        assert!(tr.observe(&[aid(1)], &none, kinds, T0).is_empty());
        // a new agent walks in → exactly one chime…
        assert_eq!(
            tr.observe(&[aid(1), aid(2)], &none, kinds, T0),
            vec![OneShot::DoorChime]
        );
        // …and the same roster next frame re-fires nothing
        assert!(tr.observe(&[aid(1), aid(2)], &none, kinds, T0).is_empty());
        // THREE simultaneous arrivals = one door moment, not a chord
        assert_eq!(
            tr.observe(&[aid(1), aid(2), aid(3), aid(4), aid(5)], &none, kinds, T0),
            vec![OneShot::DoorChime]
        );
        // an exit fires nothing; the SAME id returning chimes again
        assert!(tr.observe(&[aid(1)], &none, kinds, T0).is_empty());
        assert_eq!(
            tr.observe(&[aid(1), aid(2)], &none, kinds, T0),
            vec![OneShot::DoorChime]
        );
    }

    #[test]
    fn tracker_emits_printer_whir_exactly_once_per_animation() {
        let mut tr = AudioCueTracker::new();
        let ids = [aid(1)];
        tr.observe(&ids, &HashSet::new(), kinds, T0); // prime
        let at_printer: HashSet<usize> = [5].into();
        assert_eq!(
            tr.observe(&ids, &at_printer, kinds, T0),
            vec![OneShot::PrinterWhir]
        );
        // still standing there N frames later → silence
        assert!(tr.observe(&ids, &at_printer, kinds, T0).is_empty());
        assert!(tr.observe(&ids, &at_printer, kinds, T0).is_empty());
        // leaves, comes back → the animation restarts → a second whir
        assert!(tr.observe(&ids, &HashSet::new(), kinds, T0).is_empty());
        assert_eq!(
            tr.observe(&ids, &at_printer, kinds, T0),
            vec![OneShot::PrinterWhir]
        );
    }

    #[test]
    fn tracker_maps_vending_and_ignores_non_appliance_waypoints() {
        let mut tr = AudioCueTracker::new();
        let ids = [aid(1)];
        tr.observe(&ids, &HashSet::new(), kinds, T0); // prime
                                                      // couch (idx 2) is not an appliance; vending (idx 7) drops a can
        let occupied: HashSet<usize> = [2, 7].into();
        assert_eq!(
            tr.observe(&ids, &occupied, kinds, T0),
            vec![OneShot::VendingDrop]
        );
    }

    #[test]
    fn waiting_and_idle_agents_do_not_raise_the_tier() {
        // an office full of WAITING agents is quiet company, not a busy band
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
