//! The pre-rendered sample banks — the ONE place the office's sounds are
//! synthesized into buffers, so the native rodio gateway (`pixtuoid`) and the
//! wasm WebAudio painter (`pixtuoid-web`) build byte-identical audio from one
//! source (#633 web-audio). Pure: no device deps, `Arc<Vec<f32>>` buffers only.
//!
//! The `rng` DRAW ORDER is the sound — `AssetBank::build` then
//! `TrackBeds::build` continue ONE stream in the ratified order, so every
//! buffer matches the LISTEN-ratified renders. Don't reorder the synth calls.

use std::sync::Arc;

use super::mixer::LoopStem;
use super::{dsp, synth, OneShot, TrackId};

/// Per-key / per-drop pre-rendered variant pool sizes: playback picks randomly
/// so typing/rain never sound repeated, while runtime stays synthesis-free.
pub const KEYSTROKE_POOL: usize = 16;
pub const DROP_POOL: usize = 12;

/// One-shot playback gains relative to master — the loudness-matched Phase 0
/// unit levels (±2.2dB across the set), with typing under the beds.
pub const KEYSTROKE_GAIN: f32 = 0.35;
pub const ONE_SHOT_GAIN: f32 = 0.5;
/// Foreground raindrops sit 12-14dB ABOVE the wash per the reference — the
/// bed peaks well under 1.0, so drops ride at the rain level itself.
pub const DROP_GAIN: f32 = 0.9;

/// The five TRACK-OWNED loop stems, in registration order. Rain is not here —
/// it is weather, shared by every mood track (#644).
pub const TRACK_STEMS: [LoopStem; 5] = [
    LoopStem::Pad,
    LoopStem::Sparkle,
    LoopStem::Keys,
    LoopStem::Drums,
    LoopStem::Texture,
];

/// Which one-shot pool a play draws from — the ONE vocabulary both backends
/// share. The native gateway resolves it to an [`Arc<Vec<f32>>`] via
/// [`AssetBank::sample`]; the wasm painter sends `(wire, index)` to JS (which
/// holds one `AudioBuffer` per pool slot). Moved here from the wasm crate so
/// the engine that emits it and the bank that resolves it agree by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneShotPool {
    Keystroke,
    Drop,
    DoorChime,
    PrinterWhir,
    VendingDrop,
}

impl OneShotPool {
    /// Which appliance-cue pool a scene [`OneShot`] plays from.
    pub(crate) fn from_event(event: OneShot) -> Self {
        match event {
            OneShot::DoorChime => OneShotPool::DoorChime,
            OneShot::PrinterWhir => OneShotPool::PrinterWhir,
            OneShot::VendingDrop => OneShotPool::VendingDrop,
        }
    }

    /// Stable index for the wasm JSON wire (JS maps it back to its buffer bank).
    pub fn wire(self) -> u8 {
        match self {
            OneShotPool::Keystroke => 0,
            OneShotPool::Drop => 1,
            OneShotPool::DoorChime => 2,
            OneShotPool::PrinterWhir => 3,
            OneShotPool::VendingDrop => 4,
        }
    }

    /// Every pool in wire order — the one-shot mirror of [`LoopStem::ALL`].
    /// [`OneShotPool::from_wire`] and the wasm round-trip test both derive from
    /// it, so a new pool is declared in exactly one place.
    pub const ALL: [OneShotPool; 5] = [
        OneShotPool::Keystroke,
        OneShotPool::Drop,
        OneShotPool::DoorChime,
        OneShotPool::PrinterWhir,
        OneShotPool::VendingDrop,
    ];

    /// The inverse of [`OneShotPool::wire`] — decode a wasm-wire pool index back
    /// to the pool (`None` for an unknown wire). Derived from [`OneShotPool::ALL`]
    /// and [`OneShotPool::wire`], so the whole `u8`↔pool bijection lives beside
    /// the enum and can't drift across the crate seam (was a hand-synced
    /// `pool_from_wire` in the wasm crate whose `_ => None` let a new pool decode
    /// to silence with no structural failure).
    pub fn from_wire(wire: u8) -> Option<Self> {
        OneShotPool::ALL.into_iter().find(|p| p.wire() == wire)
    }
}

/// The ONE-SHOT pools a player keeps for its whole life, synthesized once at
/// spawn/warmup. The loop beds live in [`TrackBeds`] instead — handed to the
/// sink at registration and NOT retained (`RodioSink` copies each into its own
/// `SamplesBuffer`, so retaining the Arcs would double the bed RAM).
pub struct AssetBank {
    pub keystrokes: Vec<Arc<Vec<f32>>>,
    pub drops: Vec<Arc<Vec<f32>>>,
    pub door_chime: Arc<Vec<f32>>,
    pub printer_whir: Arc<Vec<f32>>,
    pub vending_drop: Arc<Vec<f32>>,
}

impl AssetBank {
    /// `rng` is the ONE asset stream — rain then `TrackBeds::build` continue
    /// it, so the draw order (and thus every buffer) is byte-identical to the
    /// LISTEN-ratified renders. Don't reorder the synth calls.
    pub fn build(rng: &mut dsp::NoiseStream) -> Self {
        Self {
            keystrokes: (0..KEYSTROKE_POOL)
                .map(|_| Arc::new(synth::keystroke(rng)))
                .collect(),
            drops: (0..DROP_POOL)
                .map(|_| Arc::new(synth::rain_drop(rng)))
                .collect(),
            door_chime: Arc::new(synth::door_chime()),
            printer_whir: Arc::new(synth::printer_whir(rng)),
            vending_drop: Arc::new(synth::vending_drop(rng)),
        }
    }

    pub fn one_shot(&self, event: OneShot) -> Arc<Vec<f32>> {
        match event {
            OneShot::DoorChime => Arc::clone(&self.door_chime),
            OneShot::PrinterWhir => Arc::clone(&self.printer_whir),
            OneShot::VendingDrop => Arc::clone(&self.vending_drop),
        }
    }

    /// Resolve an [`AudioEngine`](super::engine::AudioEngine)-emitted
    /// `(pool, index)` play to its buffer — the native gateway's counterpart to
    /// the wasm painter's zero-copy `(wire, index)` JS export. `index` is
    /// modulo the pool size (the engine already picks in-range; the guard keeps
    /// a future caller from panicking). The single-sample appliance pools
    /// ignore `index`.
    pub fn sample(&self, pool: OneShotPool, index: usize) -> Arc<Vec<f32>> {
        match pool {
            OneShotPool::Keystroke => Arc::clone(&self.keystrokes[index % self.keystrokes.len()]),
            OneShotPool::Drop => Arc::clone(&self.drops[index % self.drops.len()]),
            OneShotPool::DoorChime => Arc::clone(&self.door_chime),
            OneShotPool::PrinterWhir => Arc::clone(&self.printer_whir),
            OneShotPool::VendingDrop => Arc::clone(&self.vending_drop),
        }
    }
}

/// One mood track's loop beds — built per [`TrackId`], registered (or swapped
/// in) with the sink, then DROPPED. Within a track the four musical beds share
/// ONE sample count and register together (phase-locked); the NIGHT texture
/// shares it too (its kick-duck is baked at frozen kick times); the DAY texture
/// keeps its free-running power-of-two length.
pub struct TrackBeds {
    beds: [Arc<Vec<f32>>; TRACK_STEMS.len()],
}

impl TrackBeds {
    /// Compose the id's take (the seed is the track-epoch block) and render it
    /// through the SAME cores the owner-ratified takes were built on —
    /// ALL-GENERATIVE runtime (owner decision 2026-07-20). Identity is
    /// the generated score + the cores' fingerprint pins (the frozen
    /// tables in `score`/`synth` remain as those pins' test anchors);
    /// noise content draws from wherever `rng` stands, like every
    /// non-boot build always did.
    pub fn build(rng: &mut dsp::NoiseStream, track: TrackId) -> Self {
        let score = super::compose_track(track);
        Self {
            beds: synth::gen_beds(&score, rng).map(Arc::new),
        }
    }

    /// Assemble from beds already built lane-by-lane (the wasm driver's
    /// chunked rebuild) — `TRACK_STEMS` order, like [`TrackBeds::build`].
    pub fn from_arcs(beds: [Arc<Vec<f32>>; TRACK_STEMS.len()]) -> Self {
        Self { beds }
    }

    pub fn bed(&self, stem: LoopStem) -> Arc<Vec<f32>> {
        Arc::clone(&self.beds[self.index(stem)])
    }

    /// The bed samples as a borrow tied to `&self` — for a consumer that reads
    /// (not retains) the buffer (the wasm driver's zero-copy JS export), where
    /// an `Arc` clone's slice would dangle.
    pub fn bed_slice(&self, stem: LoopStem) -> &[f32] {
        &self.beds[self.index(stem)]
    }

    fn index(&self, stem: LoopStem) -> usize {
        TRACK_STEMS
            .iter()
            .position(|s| *s == stem)
            .expect("every track stem has a bed")
    }
}

/// Whether every TRACK-owned stem's live gain (from a mixer step) has reached
/// exactly 0.0 — the silence gate a player checks before swapping a mood
/// track's beds (`TrackSwitch::try_swap`). Rain/typing gains are ignored
/// (track-independent).
pub fn track_stems_silent(gains: &[(LoopStem, f32)]) -> bool {
    gains
        .iter()
        .filter(|(s, _)| TRACK_STEMS.contains(s))
        .all(|(_, g)| *g == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_beds_wire_each_lane_to_the_right_synth() {
        // Generated takes vary per seed, so the frozen take's exact
        // centroid ORDER no longer holds — the lane-wiring pin becomes
        // structural: the lead (sparkle) is the brightest musical bed by
        // register + voice design, the day texture free-runs the 2^19
        // noise loop, and the four musical beds phase-lock. (A bed() arm
        // swap still fails: pad↔sparkle flips the centroid gap, and a
        // texture mixup breaks the length contract.)
        let mut rng = dsp::NoiseStream::new(crate::audio::BUILD_SEED);
        let day = TrackBeds::build(&mut rng, TrackId::GenDay(0));
        let rain = synth::rain_bed(&mut rng);
        assert_eq!(
            day.bed_slice(LoopStem::Texture).len(),
            1 << 19,
            "day texture = the free-running noise-bed loop"
        );
        assert_eq!(rain.len(), 1 << 19, "rain = the noise-bed loop");
        let n = day.bed_slice(LoopStem::Pad).len();
        for stem in [LoopStem::Sparkle, LoopStem::Keys, LoopStem::Drums] {
            assert_eq!(day.bed_slice(stem).len(), n, "musical beds phase-lock");
        }
        let c = |s| dsp::centroid_hz(day.bed_slice(s));
        assert!(
            c(LoopStem::Sparkle) > c(LoopStem::Pad) * 1.5,
            "the lead must sit clearly above the pad: {:.0} vs {:.0}",
            c(LoopStem::Sparkle),
            c(LoopStem::Pad)
        );
    }

    #[test]
    fn a_mood_swap_never_touches_the_rain_stem() {
        // Rain is weather, shared by every track — TRACK_STEMS (the set a swap
        // rebuilds) must exclude it, so a mood change can't cut the rain.
        assert!(!TRACK_STEMS.contains(&LoopStem::Rain));
    }

    #[test]
    fn the_night_arm_builds_a_bed_distinct_from_day() {
        // A Day->Night swap must install a genuinely different bed (68 vs 72 BPM
        // → different pad loop length), not a silent Day clone. Pins the
        // TrackBeds::build Night arm — relocated from the retired native
        // track-switch thread test, which asserted `night_pad_len != day_pad_len`.
        let mut rng = dsp::NoiseStream::new(crate::audio::BUILD_SEED);
        let day = TrackBeds::build(&mut rng, TrackId::GenDay(0));
        let night = TrackBeds::build(&mut rng, TrackId::GenNight(0));
        assert_ne!(
            day.bed_slice(LoopStem::Pad).len(),
            night.bed_slice(LoopStem::Pad).len(),
            "the night pad is a distinct bed (different BPM/loop length), not a Day clone"
        );
    }
}
