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
    /// DAY continues the boot rng stream in the ratified order (drums, then
    /// texture — the pure musical stems draw nothing), keeping every day
    /// buffer byte-identical to the #642/#643 renders. NIGHT draws from
    /// wherever the stream stands — its identity is the frozen score plus
    /// spectral pins, not byte equality.
    pub fn build(rng: &mut dsp::NoiseStream, track: TrackId) -> Self {
        let beds = match track {
            TrackId::Day => [
                Arc::new(synth::stem_pad()),
                Arc::new(synth::stem_sparkle()),
                Arc::new(synth::stem_keys()),
                Arc::new(synth::stem_drums(rng)),
                Arc::new(synth::texture_bed(rng)),
            ],
            TrackId::Night => [
                Arc::new(synth::night_pad()),
                Arc::new(synth::night_sparkle()),
                Arc::new(synth::night_keys()),
                Arc::new(synth::night_drums(rng)),
                Arc::new(synth::night_texture(rng)),
            ],
        };
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
